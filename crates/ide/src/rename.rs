//! 重命名。
//!
//! 收集定义本身、它的全部引用，以及必须跟着一起改名的关联声明：
//! - 方法：整条覆写链上的所有声明；
//! - 类：它的构造器声明，以及与类同名的源文件；
//! - 记录组件：同名的访问器调用。
//!
//! 冲突检查是保守的：只拦截确定会出错的情况（名字不合法、同一作用域里已有同名符号、
//! 目标在依赖库里）。

use base_db::Db;
use hir::{class_data, lookup_class, ClassId, ClassKind, Definition, MethodId, Semantics};
use ide_db::{
    search::{find_references, refers_to},
    TextEdit,
};
use rustc_hash::FxHashSet;
use syntax::{ast, TextRange};

use crate::{
    navigation::{all_subclasses, definition_at, position_file, super_methods},
    FileId, FilePosition, RangeInfo,
};

/// 跨文件的一组修改。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceChange {
    /// 每个文件里的文本替换，按起始位置排序、互不重叠
    pub edits: Vec<(FileId, Vec<TextEdit>)>,
    /// 需要改名的文件：`(文件, 新文件名)`，新文件名不含目录
    pub file_renames: Vec<(FileId, String)>,
}

impl SourceChange {
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty() && self.file_renames.is_empty()
    }

    pub fn single_file(file_id: FileId, mut edits: Vec<TextEdit>) -> SourceChange {
        edits.sort_by_key(|edit| (edit.range.start(), edit.range.end()));
        SourceChange {
            edits: vec![(file_id, edits)],
            file_renames: Vec::new(),
        }
    }

    fn from_replacements(mut items: Vec<(FileId, TextRange)>, new_text: &str) -> SourceChange {
        items.sort_by_key(|(file, range)| (*file, range.start(), range.end()));
        items.dedup();
        let mut edits: Vec<(FileId, Vec<TextEdit>)> = Vec::new();
        for (file_id, range) in items {
            let edit = TextEdit::replace(range, new_text);
            match edits.last_mut() {
                Some((last, list)) if *last == file_id => list.push(edit),
                _ => edits.push((file_id, vec![edit])),
            }
        }
        SourceChange {
            edits,
            file_renames: Vec::new(),
        }
    }
}

/// 是否是合法的、非关键字的 Java 标识符。
pub(crate) fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    if !chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
        return false;
    }
    // 借解析器排除关键字：`class <name> {}` 必须能无错误地解析
    ast::CompilationUnit::parse(&format!("class {name} {{}}"))
        .errors()
        .is_empty()
        && !matches!(
            name,
            "var" | "yield" | "record" | "sealed" | "permits" | "_"
        )
}

fn is_constructor(db: &dyn Db, method: MethodId) -> bool {
    class_data(db, method.class)
        .and_then(|d| d.methods.get(method.idx as usize).map(|m| m.is_constructor))
        .unwrap_or(false)
}

/// 构造器上的重命名实际是重命名它的类。
fn rename_target(db: &dyn Db, def: Definition) -> Definition {
    match def {
        Definition::Method(method) if is_constructor(db, method) => Definition::Class(method.class),
        other => other,
    }
}

pub(crate) fn prepare_rename(
    db: &dyn Db,
    position: FilePosition,
) -> Result<RangeInfo<String>, String> {
    let file = position_file(db, position).ok_or(base_db::trs!("未知的文件", "Unknown file"))?;
    let (range, def) = definition_at(db, file, position.offset).ok_or(base_db::trs!(
        "光标处没有可以重命名的符号",
        "No symbol to rename at the cursor"
    ))?;
    let def = rename_target(db, def);
    check_renameable(db, &def)?;
    let name = def
        .name(db)
        .ok_or(base_db::trs!("这个符号没有名字", "This symbol has no name"))?;
    Ok(RangeInfo {
        range,
        info: name.to_string(),
    })
}

fn check_renameable(db: &dyn Db, def: &Definition) -> Result<(), String> {
    match def {
        Definition::Package(_) => Err(base_db::tr!(
            "暂不支持重命名包",
            "Renaming packages is not supported yet"
        )),
        Definition::Local(_) | Definition::TypeParam(_) => Ok(()),
        Definition::Class(_) | Definition::Field(_) | Definition::Method(_) => {
            let class = match def {
                Definition::Class(class) => *class,
                _ => def.container_class().ok_or(base_db::trs!(
                    "找不到所属的类",
                    "Cannot find the enclosing class"
                ))?,
            };
            let is_source = class_data(db, class).is_some_and(|d| d.is_source);
            if !is_source {
                return Err(base_db::tr!(
                    "不能重命名依赖库或 JDK 里的符号",
                    "Cannot rename a symbol from a library or the JDK"
                ));
            }
            if def.source(db).is_none() {
                return Err(base_db::tr!(
                    "这个成员是自动生成的，没有可以修改的声明",
                    "This member is generated and has no declaration to edit"
                ));
            }
            Ok(())
        }
    }
}

/// 方法所在的整条覆写链：它覆写的、覆写它的，以及这些方法的其它覆写者。
fn method_family(db: &dyn Db, method: MethodId) -> Vec<MethodId> {
    let target = Definition::Method(method);
    let Some(name) = target.name(db) else {
        return vec![method];
    };
    let mut family: Vec<MethodId> = vec![method];
    family.extend(super_methods(db, method));

    // 链上每个声明类的全部子类里，同一族的方法
    let roots: Vec<ClassId> = family.iter().map(|m| m.class).collect();
    let mut seen: FxHashSet<MethodId> = family.iter().copied().collect();
    for root in roots {
        for sub in all_subclasses(db, root) {
            let Some(data) = class_data(db, sub) else {
                continue;
            };
            for (idx, _) in data.methods_named(&name) {
                let candidate = MethodId { class: sub, idx };
                if seen.contains(&candidate) {
                    continue;
                }
                if refers_to(db, &Definition::Method(candidate), &target) {
                    seen.insert(candidate);
                    family.push(candidate);
                }
            }
        }
    }
    family
}

fn check_conflicts(db: &dyn Db, def: &Definition, new_name: &str) -> Result<(), String> {
    match def {
        Definition::Local(local) => {
            let result = hir::infer(db, local.body);
            let clash = result
                .locals()
                .iter()
                .enumerate()
                .any(|(id, other)| id as u32 != local.id && other.name == new_name);
            if clash {
                return Err(base_db::tr!(
                    "这个方法体里已经有名为 `{new_name}` 的局部符号",
                    "A local symbol named `{new_name}` already exists in this method body"
                ));
            }
        }
        Definition::Field(field) => {
            if class_data(db, field.class).is_some_and(|d| d.field_named(new_name).is_some()) {
                return Err(base_db::tr!(
                    "类里已经有名为 `{new_name}` 的字段",
                    "The class already has a field named `{new_name}`"
                ));
            }
        }
        Definition::Class(class) => {
            let binary_name = class.binary_name(db);
            let sibling = match binary_name.rfind(['/', '$']) {
                Some(idx) => format!("{}{new_name}", &binary_name[..=idx]),
                None => new_name.to_owned(),
            };
            if lookup_class(db, &sibling).is_some() {
                return Err(base_db::tr!(
                    "已经存在名为 `{new_name}` 的类",
                    "A class named `{new_name}` already exists"
                ));
            }
        }
        Definition::Method(method) => {
            // 同一个类里同名同参数个数的方法会变成重复声明
            if let Some(data) = class_data(db, method.class) {
                let arity = data
                    .methods
                    .get(method.idx as usize)
                    .map(|m| m.params.len());
                let clash = data.methods.iter().any(|m| {
                    !m.is_constructor && m.name == new_name && Some(m.params.len()) == arity
                });
                if clash {
                    return Err(base_db::tr!("类里已经有参数个数相同的方法 `{new_name}`", "The class already has a method `{new_name}` with the same number of parameters"));
                }
            }
        }
        Definition::TypeParam(_) | Definition::Package(_) => {}
    }
    Ok(())
}

pub(crate) fn rename(
    db: &dyn Db,
    position: FilePosition,
    new_name: &str,
) -> Result<SourceChange, String> {
    if !is_valid_identifier(new_name) {
        return Err(base_db::tr!(
            "`{new_name}` 不是合法的 Java 标识符",
            "`{new_name}` is not a valid Java identifier"
        ));
    }
    let file = position_file(db, position).ok_or(base_db::trs!("未知的文件", "Unknown file"))?;
    let (_, def) = definition_at(db, file, position.offset).ok_or(base_db::trs!(
        "光标处没有可以重命名的符号",
        "No symbol to rename at the cursor"
    ))?;
    let def = rename_target(db, def);
    check_renameable(db, &def)?;
    let old_name = def
        .name(db)
        .ok_or(base_db::trs!("这个符号没有名字", "This symbol has no name"))?;
    if old_name == new_name {
        return Ok(SourceChange::default());
    }
    check_conflicts(db, &def, new_name)?;

    let mut items: Vec<(FileId, TextRange)> = Vec::new();
    let add_declaration = |def: &Definition, items: &mut Vec<(FileId, TextRange)>| {
        if let Some(source) = def.source(db) {
            items.push((source.file.file_id(db), source.focus_range));
        }
    };
    let add_references = |def: &Definition, items: &mut Vec<(FileId, TextRange)>| {
        items.extend(
            find_references(db, def)
                .into_iter()
                .map(|r| (r.file_id, r.range)),
        );
    };

    let mut file_renames = Vec::new();
    match &def {
        Definition::Method(method) => {
            let family = method_family(db, *method);
            for member in &family {
                let member_def = Definition::Method(*member);
                check_renameable(db, &member_def).map_err(|_| {
                    base_db::tr!(
                        "这个方法覆写了依赖库里的方法，不能重命名",
                        "This method overrides a library method and cannot be renamed"
                    )
                })?;
                add_declaration(&member_def, &mut items);
            }
            // 引用搜索本身就按覆写链匹配，搜一次即可
            add_references(&def, &mut items);
        }
        Definition::Class(class) => {
            add_declaration(&def, &mut items);
            add_references(&def, &mut items);
            let data = class_data(db, *class).ok_or(base_db::trs!(
                "找不到类的数据",
                "Cannot find the class data"
            ))?;
            for (idx, ctor) in data.constructors() {
                // 隐式构造器的声明位置就是类本身，不重复添加
                if ctor.decl != data.decl {
                    add_declaration(
                        &Definition::Method(MethodId { class: *class, idx }),
                        &mut items,
                    );
                }
            }
            // 顶层类与文件同名时文件也要改名
            if data.outer.is_none() {
                if let Some(source) = def.source(db) {
                    let sema = Semantics::new(db);
                    let cu = sema.parse(source.file);
                    let top_level_count = cu.type_decls().count();
                    let is_public = data.flags.visibility() == hir::Visibility::Public;
                    if is_public || top_level_count == 1 {
                        file_renames.push((source.file.file_id(db), format!("{new_name}.java")));
                    }
                }
            }
        }
        Definition::Field(field) => {
            add_declaration(&def, &mut items);
            add_references(&def, &mut items);
            // 记录组件的访问器与组件同名
            if let Some(data) = class_data(db, field.class) {
                if data.kind == ClassKind::Record {
                    let accessor = data.methods.iter().position(|m| {
                        !m.is_constructor && m.name == old_name && m.params.is_empty()
                    });
                    if let Some(idx) = accessor {
                        let method = Definition::Method(MethodId {
                            class: field.class,
                            idx: idx as u32,
                        });
                        add_references(&method, &mut items);
                    }
                }
            }
        }
        Definition::Local(_) | Definition::TypeParam(_) => {
            add_declaration(&def, &mut items);
            add_references(&def, &mut items);
        }
        Definition::Package(_) => {
            return Err(base_db::tr!(
                "暂不支持重命名包",
                "Renaming packages is not supported yet"
            ))
        }
    }

    let mut change = SourceChange::from_replacements(items, new_name);
    change.file_renames = file_renames;
    Ok(change)
}
