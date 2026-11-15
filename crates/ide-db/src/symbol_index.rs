//! 按名字检索符号的索引（`workspace/symbol`、未导入类型的补全）。
//!
//! 名字小写后放进 FST，查询时用"子序列"自动机在 FST 上筛出候选，再用
//! [`fuzzy_score`] 打分排序。两个索引分开建立：
//! - 源码符号：类、字段、方法；任何文件的签名变化后惰性重建；
//! - 二进制类：依赖和 JDK 里的公开类；只在类路径变化时重建。

use std::sync::Arc;

use base_db::{workspace, Db};
use classfile::access;
use fst::{IntoStreamer, Streamer};
use hir::{item_tree, source_class_index, ClassId, Definition, FieldId, MethodId, Modifiers};
use smol_str::SmolStr;

use crate::{fuzzy::fuzzy_score, SymbolKind};

/// 名字 → 条目 的模糊检索结构。
pub struct NameIndex<T> {
    map: fst::Map<Vec<u8>>,
    /// 按小写名字排序；FST 的值是 `(起始下标 << 32) | 个数`
    entries: Vec<(SmolStr, T)>,
}

impl<T> NameIndex<T> {
    pub fn build(items: Vec<(SmolStr, T)>) -> NameIndex<T> {
        let mut keyed: Vec<(String, SmolStr, T)> = items
            .into_iter()
            .map(|(name, item)| (name.to_lowercase(), name, item))
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));

        let mut builder = fst::MapBuilder::memory();
        let mut start = 0;
        while start < keyed.len() {
            let mut end = start + 1;
            while end < keyed.len() && keyed[end].0 == keyed[start].0 {
                end += 1;
            }
            let value = ((start as u64) << 32) | (end - start) as u64;
            // 键已按字节序排好且去重，插入不会失败
            let _ = builder.insert(keyed[start].0.as_bytes(), value);
            start = end;
        }
        let map = builder.into_map();
        let entries = keyed
            .into_iter()
            .map(|(_, name, item)| (name, item))
            .collect();
        NameIndex { map, entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[(SmolStr, T)] {
        &self.entries
    }

    /// 模糊检索，按分数从高到低返回至多 `limit` 条。
    pub fn search(&self, query: &str, limit: usize) -> Vec<(i32, &SmolStr, &T)> {
        if query.is_empty() {
            return Vec::new();
        }
        let lower = query.to_lowercase();
        let automaton = fst::automaton::Subsequence::new(&lower);
        let mut stream = self.map.search(automaton).into_stream();
        let mut res = Vec::new();
        while let Some((_, value)) = stream.next() {
            let start = (value >> 32) as usize;
            let len = (value & 0xffff_ffff) as usize;
            for (name, item) in &self.entries[start..start + len] {
                if let Some(score) = fuzzy_score(query, name) {
                    res.push((score, name, item));
                }
            }
        }
        res.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        res.truncate(limit);
        res
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEntry {
    pub def: Definition,
    pub kind: SymbolKind,
    /// 所在类（成员）或所在包（顶层类）的点分名
    pub container: Option<SmolStr>,
    pub deprecated: bool,
}

pub struct SymbolIndex {
    index: NameIndex<SymbolEntry>,
}

impl PartialEq for SymbolIndex {
    fn eq(&self, other: &Self) -> bool {
        self.index.entries == other.index.entries
    }
}

impl Eq for SymbolIndex {}

impl std::fmt::Debug for SymbolIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymbolIndex")
            .field("len", &self.index.len())
            .finish()
    }
}

impl SymbolIndex {
    pub fn search(&self, query: &str, limit: usize) -> Vec<(i32, &SmolStr, &SymbolEntry)> {
        self.index.search(query, limit)
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

fn container_of(binary_name: &str) -> Option<SmolStr> {
    let idx = binary_name.rfind(['/', '$'])?;
    Some(SmolStr::new(binary_name[..idx].replace(['/', '$'], ".")))
}

/// 工作区源码里的类、字段、方法。
#[salsa::tracked]
pub fn source_symbol_index(db: &dyn Db) -> Arc<SymbolIndex> {
    let classes = source_class_index(db);
    let mut items: Vec<(SmolStr, SymbolEntry)> = Vec::with_capacity(classes.len() * 8);
    for (binary_name, file, idx) in classes.iter() {
        let tree = item_tree(db, file);
        let Some(data) = tree.classes.get(idx as usize) else {
            continue;
        };
        let class = ClassId::from_binary_name(db, binary_name);
        let class_name = SmolStr::new(binary_name.replace(['/', '$'], "."));
        items.push((
            data.name.clone(),
            SymbolEntry {
                def: Definition::Class(class),
                kind: SymbolKind::of_class(data.kind),
                container: container_of(binary_name),
                deprecated: data.flags.is_deprecated(),
            },
        ));
        for (field_idx, field) in data.fields.iter().enumerate() {
            if field.flags.contains(Modifiers::SYNTHETIC) {
                continue;
            }
            items.push((
                field.name.clone(),
                SymbolEntry {
                    def: Definition::Field(FieldId {
                        class,
                        idx: field_idx as u32,
                    }),
                    kind: if field.is_enum_constant() {
                        SymbolKind::EnumConstant
                    } else {
                        SymbolKind::Field
                    },
                    container: Some(class_name.clone()),
                    deprecated: field.flags.is_deprecated(),
                },
            ));
        }
        for (method_idx, method) in data.methods.iter().enumerate() {
            if method.flags.contains(Modifiers::SYNTHETIC) {
                continue;
            }
            items.push((
                method.name.clone(),
                SymbolEntry {
                    def: Definition::Method(MethodId {
                        class,
                        idx: method_idx as u32,
                    }),
                    kind: if method.is_constructor {
                        SymbolKind::Constructor
                    } else {
                        SymbolKind::Method
                    },
                    container: Some(class_name.clone()),
                    deprecated: method.flags.is_deprecated(),
                },
            ));
        }
    }
    Arc::new(SymbolIndex {
        index: NameIndex::build(items),
    })
}

/// 依赖库和 JDK 里的公开类（不含匿名类、局部类）。
#[salsa::tracked]
pub fn binary_class_index(db: &dyn Db) -> Arc<SymbolIndex> {
    let class_path = workspace(db).class_path(db);
    let items = class_path
        .summaries()
        .iter()
        .filter(|s| s.is_public() && !s.is_anonymous_or_local())
        .map(|summary| {
            let kind = if summary.access & access::ANNOTATION != 0 {
                SymbolKind::Annotation
            } else if summary.access & access::INTERFACE != 0 {
                SymbolKind::Interface
            } else if summary.access & access::ENUM != 0 {
                SymbolKind::Enum
            } else {
                SymbolKind::Class
            };
            (
                SmolStr::new(summary.simple_name()),
                SymbolEntry {
                    def: Definition::Class(ClassId::from_binary_name(db, &summary.name)),
                    kind,
                    container: container_of(&summary.name),
                    deprecated: false,
                },
            )
        })
        .collect();
    Arc::new(SymbolIndex {
        index: NameIndex::build(items),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: SmolStr,
    pub entry: SymbolEntry,
    pub score: i32,
}

/// 在源码符号（以及可选的二进制类）里模糊检索。源码符号排在同分的库符号前面。
pub fn search_symbols(
    db: &dyn Db,
    query: &str,
    limit: usize,
    include_libraries: bool,
) -> Vec<Symbol> {
    let to_symbol = |(score, name, entry): (i32, &SmolStr, &SymbolEntry), bonus: i32| Symbol {
        name: name.clone(),
        entry: entry.clone(),
        score: score + bonus,
    };
    let source = source_symbol_index(db);
    let mut res: Vec<Symbol> = source
        .search(query, limit)
        .into_iter()
        .map(|hit| to_symbol(hit, 5))
        .collect();
    if include_libraries {
        let binary = binary_class_index(db);
        res.extend(
            binary
                .search(query, limit)
                .into_iter()
                .map(|hit| to_symbol(hit, 0)),
        );
    }
    res.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    res.truncate(limit);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_index_groups_case_insensitive_duplicates() {
        let index = NameIndex::build(vec![
            (SmolStr::new("List"), 1),
            (SmolStr::new("list"), 2),
            (SmolStr::new("ArrayList"), 3),
            (SmolStr::new("Map"), 4),
        ]);
        let hits: Vec<i32> = index
            .search("list", 10)
            .into_iter()
            .map(|(_, _, v)| *v)
            .collect();
        assert_eq!(hits.len(), 3);
        assert!(!hits.contains(&4));
        // 大写查询只匹配大写开头的
        let hits: Vec<i32> = index
            .search("List", 10)
            .into_iter()
            .map(|(_, _, v)| *v)
            .collect();
        assert_eq!(hits, vec![1, 3]);
        assert!(index.search("", 10).is_empty());
    }
}
