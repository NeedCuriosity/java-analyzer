//! import 语句的编辑：让某个类在文件里能用简单名引用。

use base_db::{Db, SourceFile};
use hir::{class_data, resolve_simple_type, ClassId, TypeNs, TypeScope};
use syntax::{ast::AstNode, TextSize};

use crate::TextEdit;

/// 想用简单名引用一个类时需要做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportAction {
    /// 已经可见（同包、已 import、`java.lang`、嵌套 / 继承来的成员类型）
    None,
    /// 加一条 import
    Add(TextEdit),
    /// 简单名已被别的类型占用，只能写全限定名
    Qualify,
}

/// 类的可导入名，例如 `java.util.Map.Entry`。匿名类 / 局部类返回 `None`。
pub fn importable_name(db: &dyn Db, class: ClassId) -> Option<String> {
    let binary_name = class.binary_name(db);
    let simple = binary_name.rsplit(['/', '$']).next()?;
    if simple.is_empty() || simple.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(binary_name.replace(['/', '$'], "."))
}

pub fn import_action(
    db: &dyn Db,
    file: SourceFile,
    scope: &TypeScope,
    class: ClassId,
) -> ImportAction {
    let Some(data) = class_data(db, class) else {
        return ImportAction::Qualify;
    };
    match resolve_simple_type(db, scope, &data.name) {
        Some(TypeNs::Class(found)) if found == class => ImportAction::None,
        Some(_) => ImportAction::Qualify,
        None => match importable_name(db, class) {
            Some(name) => ImportAction::Add(insert_import(db, file, &name)),
            None => ImportAction::Qualify,
        },
    }
}

/// 生成插入 `import <qualified_name>;` 的编辑：放在按字母序的位置上。
pub fn insert_import(db: &dyn Db, file: SourceFile, qualified_name: &str) -> TextEdit {
    let cu = base_db::parse(db, file).tree();
    let line = format!("import {qualified_name};");

    let mut last_plain = None;
    for import in cu.imports() {
        if import.is_static() {
            continue;
        }
        let existing = import.name().map(|n| n.text()).unwrap_or_default();
        if existing.as_str() > qualified_name {
            let offset = import.syntax().text_range().start();
            return TextEdit::insert(offset, format!("{line}\n"));
        }
        last_plain = Some(import);
    }
    if let Some(last) = last_plain {
        return TextEdit::insert(last.syntax().text_range().end(), format!("\n{line}"));
    }
    // 只有静态 import：放在它们前面
    if let Some(first) = cu.imports().next() {
        return TextEdit::insert(first.syntax().text_range().start(), format!("{line}\n\n"));
    }
    if let Some(package) = cu.package() {
        return TextEdit::insert(package.syntax().text_range().end(), format!("\n\n{line}"));
    }
    TextEdit::insert(TextSize::from(0), format!("{line}\n\n"))
}
