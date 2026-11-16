//! Spring Boot 配置文件（`application*.properties` / `.yml`）里的补全：
//! 键（properties 里是完整的点分键，YAML 里是当前嵌套层次下的下一段）和值（布尔、枚举、提示里的值）。

use base_db::{Db, ResourceFile};
use ide_db::{
    fuzzy::fuzzy_score,
    spring::{self, canonical, ConfigCursor},
    spring_metadata::{self, MetadataSet, PropertyMeta, Shape},
    SymbolKind,
};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use syntax::{TextRange, TextSize};

use crate::{
    CompletionConfig, CompletionItem, CompletionItemKind, CompletionList, CompletionResolve,
};

/// 一个可以补全的键：来自元数据，或者只在别的配置文件里出现过（自定义的键）。
struct KeyCandidate<'a> {
    name: String,
    meta: Option<&'a PropertyMeta>,
    /// 自定义的键出现在哪个文件里
    file: Option<SmolStr>,
}

fn candidates<'a>(
    db: &dyn Db,
    resource: &ResourceFile,
    set: &'a MetadataSet,
) -> Vec<KeyCandidate<'a>> {
    let mut seen = rustc_hash::FxHashSet::default();
    let mut res = Vec::new();
    for property in set.properties() {
        seen.insert(canonical(&property.name));
        res.push(KeyCandidate {
            name: property.name.clone(),
            meta: Some(property),
            file: None,
        });
    }
    // 别的配置文件（其他 profile）里写过、元数据里没有的键
    for property in spring::config_properties(db, resource.module) {
        if property.file_id == resource.file_id || property.key.contains('[') {
            continue;
        }
        if !seen.insert(canonical(&property.key)) {
            continue;
        }
        if matches!(
            set.resolve(&property.key),
            spring_metadata::KeyMatch::Within(_)
        ) {
            continue;
        }
        res.push(KeyCandidate {
            name: property.key,
            meta: None,
            file: Some(property.file_name),
        });
    }
    res
}

fn type_detail(meta: Option<&PropertyMeta>, file: Option<&SmolStr>) -> Option<String> {
    match meta {
        Some(meta) => meta.ty.clone(),
        None => file.map(|f| f.to_string()),
    }
}

pub(crate) fn complete(
    db: &dyn Db,
    resource: &ResourceFile,
    offset: TextSize,
    config: &CompletionConfig,
) -> CompletionList {
    let Some(cursor) = spring::config_cursor(&resource.name, &resource.text, offset) else {
        return CompletionList::default();
    };
    let set = MetadataSet::load(db);
    let yaml = !resource.name.ends_with(".properties");
    match cursor {
        ConfigCursor::Key {
            parent,
            range,
            typed,
            has_separator,
            indent,
        } => {
            if yaml {
                yaml_keys(
                    db,
                    resource,
                    &set,
                    &parent,
                    range,
                    &typed,
                    has_separator,
                    indent,
                )
            } else {
                properties_keys(db, resource, &set, range, &typed, has_separator, config)
            }
        }
        ConfigCursor::Value { key, range } => values(db, resource, &set, &key, range, offset),
    }
}

fn key_item(
    label: String,
    insert_text: String,
    range: TextRange,
    detail: Option<String>,
    label_detail: Option<String>,
    deprecated: bool,
    resolve: Option<CompletionResolve>,
) -> CompletionItem {
    CompletionItem {
        lookup: label.clone(),
        label,
        kind: CompletionItemKind::Symbol(SymbolKind::Field),
        label_detail,
        detail,
        insert_text,
        is_snippet: false,
        source_range: range,
        additional_edits: Vec::new(),
        deprecated,
        score: 0,
        resolve,
    }
}

fn properties_keys(
    db: &dyn Db,
    resource: &ResourceFile,
    set: &MetadataSet,
    range: TextRange,
    typed: &str,
    has_separator: bool,
    config: &CompletionConfig,
) -> CompletionList {
    let query = typed.to_lowercase();
    let mut scored: Vec<(i32, CompletionItem)> = Vec::new();
    for candidate in candidates(db, resource, set) {
        let Some(score) = fuzzy_score(&query, &candidate.name) else {
            continue;
        };
        let deprecated = candidate.meta.is_some_and(|m| m.deprecation.is_some());
        let mut insert_text = candidate.name.clone();
        if !has_separator {
            insert_text.push('=');
        }
        let label_detail = candidate
            .meta
            .and_then(|m| m.default_value.as_ref())
            .map(|d| format!(" = {d}"));
        let item = key_item(
            candidate.name.clone(),
            insert_text,
            range,
            type_detail(candidate.meta, candidate.file.as_ref()),
            label_detail,
            deprecated,
            candidate.meta.map(|_| CompletionResolve::ConfigKey {
                key: SmolStr::new(&candidate.name),
            }),
        );
        scored.push((score - if deprecated { 1000 } else { 0 }, item));
    }
    finish(scored, config.limit, !typed.is_empty())
}

fn finish(mut scored: Vec<(i32, CompletionItem)>, limit: usize, prefix: bool) -> CompletionList {
    scored.sort_by(|(a, x), (b, y)| b.cmp(a).then_with(|| x.label.cmp(&y.label)));
    let truncated = scored.len() > limit;
    scored.truncate(limit);
    CompletionList {
        items: scored.into_iter().map(|(_, item)| item).collect(),
        is_incomplete: truncated || prefix,
    }
}

/// YAML：外层键 `parent` 下面的下一段。
#[allow(clippy::too_many_arguments)]
fn yaml_keys(
    db: &dyn Db,
    resource: &ResourceFile,
    set: &MetadataSet,
    parent: &str,
    range: TextRange,
    typed: &str,
    has_separator: bool,
    indent: usize,
) -> CompletionList {
    if parent.contains('[') {
        return CompletionList::default();
    }
    let parent_canonical = canonical(parent);
    let depth = if parent.is_empty() {
        0
    } else {
        parent.split('.').count()
    };
    // 同一映射里已经写了的键
    let siblings: rustc_hash::FxHashSet<String> = spring::yaml_keys(&resource.text)
        .into_iter()
        .filter(|(_, r)| !r.contains_range(range) && !range.contains_range(*r))
        .filter_map(|(key, _)| {
            let (head, last) = match key.rsplit_once('.') {
                Some((head, last)) => (head.to_owned(), last.to_owned()),
                None => (String::new(), key.clone()),
            };
            (canonical(&head) == parent_canonical).then(|| canonical(&last))
        })
        .collect();
    let unit = spring::yaml_indent_unit(&resource.text);
    let child_indent = " ".repeat(indent + unit);

    // 段 → 完整键、是不是叶子、元数据
    let mut segments: FxHashMap<String, Segment<'_>> = FxHashMap::default();
    let mut order: Vec<String> = Vec::new();
    for candidate in candidates(db, resource, set) {
        let segs: Vec<&str> = candidate.name.split('.').collect();
        if segs.len() <= depth {
            continue;
        }
        if depth > 0 && canonical(&segs[..depth].join(".")) != parent_canonical {
            continue;
        }
        let seg = segs[depth].to_owned();
        if siblings.contains(&canonical(&seg)) {
            continue;
        }
        let leaf = segs.len() == depth + 1;
        let full = segs[..=depth].join(".");
        match segments.get_mut(&seg) {
            Some(entry) => {
                if !leaf {
                    // 既是叶子又有下一级时按分组处理
                    entry.leaf = false;
                }
            }
            None => {
                order.push(seg.clone());
                segments.insert(
                    seg,
                    Segment {
                        full,
                        leaf,
                        meta: candidate.meta,
                        file: candidate.file,
                    },
                );
            }
        }
    }
    let query = typed.to_lowercase();
    let mut scored = Vec::new();
    for seg in order {
        let Some(score) = fuzzy_score(&query, &seg) else {
            continue;
        };
        let Segment {
            full,
            leaf,
            meta,
            file,
        } = &segments[&seg];
        let meta = if *leaf { *meta } else { None };
        let insert_text = if has_separator {
            seg.clone()
        } else if *leaf
            && meta.is_none_or(|m| m.shape == Shape::Scalar || m.shape == Shape::Unknown)
        {
            format!("{seg}: ")
        } else if *leaf && meta.is_some_and(|m| m.shape == Shape::List) {
            format!("{seg}:\n{child_indent}- ")
        } else {
            format!("{seg}:\n{child_indent}")
        };
        let detail = if *leaf {
            type_detail(meta, file.as_ref())
        } else {
            set.group(full)
                .and_then(|g| g.ty.as_deref())
                .map(spring_metadata::simplify_type)
        };
        let deprecated = meta.is_some_and(|m| m.deprecation.is_some());
        let item = key_item(
            seg.clone(),
            insert_text,
            range,
            detail,
            meta.and_then(|m| m.default_value.as_ref())
                .map(|d| format!(" = {d}")),
            deprecated,
            meta.map(|_| CompletionResolve::ConfigKey {
                key: SmolStr::new(full),
            }),
        );
        scored.push((score - if deprecated { 1000 } else { 0 }, item));
    }
    finish(scored, usize::MAX, !typed.is_empty())
}

/// YAML 里外层键下面的一段。
struct Segment<'a> {
    full: String,
    /// 没有下一级（是配置项本身）
    leaf: bool,
    meta: Option<&'a PropertyMeta>,
    file: Option<SmolStr>,
}

fn values(
    db: &dyn Db,
    resource: &ResourceFile,
    set: &MetadataSet,
    key: &str,
    mut range: TextRange,
    offset: TextSize,
) -> CompletionList {
    // 列表元素 `key[0]` 和逗号分隔的值都按元素补全
    let base = match key
        .strip_suffix(']')
        .and_then(|k| k.rfind('[').map(|i| &k[..i]))
    {
        Some(base) => base,
        None => key,
    };
    let property = match set.resolve(base) {
        spring_metadata::KeyMatch::Property(p) => Some(p),
        spring_metadata::KeyMatch::Within(p) if p.shape == Shape::Map => Some(p),
        _ => None,
    };
    let Some(property) = property else {
        return CompletionList::default();
    };
    let text = &resource.text;
    let start = usize::from(range.start());
    let cursor = usize::from(offset).clamp(start, text.len());
    if let Some(comma) = text[start..cursor].rfind(',') {
        let element = start + comma + 1;
        let lead = text[element..cursor].len() - text[element..cursor].trim_start().len();
        let end = text[cursor..usize::from(range.end()).max(cursor)]
            .find(',')
            .map_or(usize::from(range.end()).max(cursor), |i| cursor + i);
        range = TextRange::new(
            TextSize::from((element + lead) as u32),
            TextSize::from(end as u32),
        );
    }
    let items = spring_metadata::value_candidates(db, property)
        .into_iter()
        .map(|hint| CompletionItem {
            label: hint.value.clone(),
            kind: CompletionItemKind::Symbol(SymbolKind::EnumConstant),
            label_detail: None,
            detail: hint
                .description
                .as_deref()
                .map(|d| first_sentence(d).to_owned()),
            lookup: hint.value.clone(),
            insert_text: hint.value,
            is_snippet: false,
            source_range: range,
            additional_edits: Vec::new(),
            deprecated: false,
            score: 0,
            resolve: None,
        })
        .collect();
    CompletionList {
        items,
        is_incomplete: false,
    }
}

fn first_sentence(text: &str) -> &str {
    let end = text.find(". ").map_or(text.len(), |i| i + 1);
    text[..end].trim()
}
