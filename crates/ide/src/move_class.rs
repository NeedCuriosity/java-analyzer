//! 移动类：把一个源文件挪到另一个包。
//!
//! 由编辑器里的"移动文件"触发（`workspace/willRenameFiles`）。产出的修改：
//! - 被移动文件的 `package` 声明；
//! - 其它文件里指向这些类的 import（含静态 import）；
//! - 原来和它同包、因此不需要 import 的使用者：补上 import；
//! - 被移动文件自己用到的、原包里的其它类：补上 import。
//!
//! 以全限定名直接书写的引用（`a.b.Foo x`）不改写。

use base_db::Db;
use hir::{Definition, Semantics};
use ide_db::{
    imports::{importable_name, insert_import},
    search::find_references_with_docs,
    TextEdit,
};
use rustc_hash::{FxHashMap, FxHashSet};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange, TextSize,
};

use crate::{rename::SourceChange, FileId};

fn package_of(cu: &ast::CompilationUnit) -> String {
    cu.package()
        .and_then(|p| p.name())
        .map(|n| n.text().to_string())
        .unwrap_or_default()
}

fn qualify(package: &str, name: &str) -> String {
    if package.is_empty() {
        name.to_owned()
    } else {
        format!("{package}.{name}")
    }
}

/// 把 `file_id` 移到包 `new_package`（点分形式，空串表示默认包）需要的修改。
pub(crate) fn move_to_package(db: &dyn Db, file_id: FileId, new_package: &str) -> SourceChange {
    move_files(db, &[(file_id, new_package.to_owned())])
}

/// 一起移动一批文件（移动 / 重命名整个目录时）：互相引用的文件按移动后的包处理，
/// 不会给已经在同一个新包里的类补 import。
pub(crate) fn move_files(db: &dyn Db, moves: &[(FileId, String)]) -> SourceChange {
    let sema = Semantics::new(db);
    // 被移动的文件 → 新包；被移动的类 → 新包
    let new_package_of_file: FxHashMap<FileId, &str> =
        moves.iter().map(|(f, p)| (*f, p.as_str())).collect();
    let mut new_package_of_class: FxHashMap<hir::ClassId, &str> = FxHashMap::default();
    for (file_id, new_package) in moves {
        let Some(file) = base_db::source_file(db, *file_id) else {
            continue;
        };
        for decl in sema.parse(file).type_decls() {
            if let Some(class) = sema.class_of_decl(file, &decl) {
                new_package_of_class.insert(class, new_package.as_str());
            }
        }
    }
    let mut edits: FxHashMap<FileId, Vec<TextEdit>> = FxHashMap::default();
    for (file_id, new_package) in moves {
        move_one(
            db,
            &sema,
            *file_id,
            new_package,
            &new_package_of_file,
            &new_package_of_class,
            &mut edits,
        );
    }
    let mut edits: Vec<(FileId, Vec<TextEdit>)> = edits
        .into_iter()
        .filter(|(_, list)| !list.is_empty())
        .map(|(file_id, mut list)| {
            list.sort_by_key(|edit| (edit.range.start(), edit.range.end()));
            list.dedup();
            (file_id, list)
        })
        .collect();
    edits.sort_by_key(|(file_id, _)| *file_id);
    SourceChange {
        edits,
        file_renames: Vec::new(),
        file_creates: Vec::new(),
    }
}

fn move_one(
    db: &dyn Db,
    sema: &Semantics<'_>,
    file_id: FileId,
    new_package: &str,
    new_package_of_file: &FxHashMap<FileId, &str>,
    new_package_of_class: &FxHashMap<hir::ClassId, &str>,
    edits: &mut FxHashMap<FileId, Vec<TextEdit>>,
) {
    let Some(file) = base_db::source_file(db, file_id) else {
        return;
    };
    let cu = sema.parse(file);
    let old_package = package_of(&cu);
    if old_package == new_package {
        return;
    }
    let text: &str = file.text(db);

    // 1. package 声明
    let package_edit = match (cu.package(), new_package.is_empty()) {
        (Some(decl), false) => match decl.name() {
            Some(name) => TextEdit::replace(name.syntax().text_range(), new_package),
            None => return,
        },
        (Some(decl), true) => {
            // 移到默认包：删掉声明和它后面的空行
            let end = usize::from(decl.syntax().text_range().end());
            let blank = text[end..].len() - text[end..].trim_start().len();
            TextEdit::replace(
                TextRange::new(
                    decl.syntax().text_range().start(),
                    TextSize::from((end + blank) as u32),
                ),
                "",
            )
        }
        (None, false) => TextEdit::insert(TextSize::from(0), format!("package {new_package};\n\n")),
        (None, true) => return,
    };
    edits.entry(file_id).or_default().push(package_edit);

    // 2. 其它文件里的使用者
    for decl in cu.type_decls() {
        let (Some(class), Some(name)) = (sema.class_of_decl(file, &decl), decl.name()) else {
            continue;
        };
        let name = name.text().to_string();
        let old_fqn = qualify(&old_package, &name);
        let new_fqn = qualify(new_package, &name);

        let mut user_files: FxHashSet<FileId> = FxHashSet::default();
        for reference in find_references_with_docs(db, &Definition::Class(class)) {
            if reference.file_id != file_id {
                user_files.insert(reference.file_id);
            }
        }
        for user_id in user_files {
            let Some(user) = base_db::source_file(db, user_id) else {
                continue;
            };
            let user_cu = sema.parse(user);
            let user_edits = edits.entry(user_id).or_default();

            // import 里的旧名字换成新名字（`import a.Foo;`、`import static a.Foo.BAR;`、`import a.Foo.Inner;`）
            let mut imported = false;
            for import in user_cu.imports() {
                if import.is_module() {
                    continue;
                }
                let Some(import_name) = import.name() else {
                    continue;
                };
                let path = import_name.text().to_string();
                let rest = match path.strip_prefix(old_fqn.as_str()) {
                    Some(rest) if rest.is_empty() || rest.starts_with('.') => rest,
                    _ => continue,
                };
                imported = true;
                if new_package.is_empty() {
                    // 默认包里的类无法被 import；留给用户处理
                    continue;
                }
                user_edits.push(TextEdit::replace(
                    import_name.syntax().text_range(),
                    format!("{new_fqn}{rest}"),
                ));
            }
            // 原来同包、不需要 import 的使用者（它自己也被移动时按移动后的包算）
            let user_package = package_of(&user_cu);
            let user_new_package = new_package_of_file
                .get(&user_id)
                .copied()
                .unwrap_or(user_package.as_str());
            if !imported
                && user_package == old_package
                && user_new_package != new_package
                && !new_package.is_empty()
            {
                user_edits.push(insert_import(db, user, &new_fqn));
            }
        }
    }

    // 3. 被移动的文件用到的、原包里的其它类
    if !old_package.is_empty() {
        let prefix = format!("{old_package}.");
        let mut needed: Vec<String> = Vec::new();
        for name_ref in cu.syntax().descendants().filter_map(ast::NameRef::cast) {
            // 只看简单名字：限定名的第一段之外的部分不需要 import
            let simple = name_ref
                .syntax()
                .parent()
                .is_some_and(|parent| match parent.kind() {
                    SyntaxKind::CLASS_TYPE => ast::ClassType::cast(parent.clone())
                        .is_some_and(|t| t.qualifier().is_none()),
                    SyntaxKind::NAME_EXPR => true,
                    SyntaxKind::QUALIFIED_NAME => {
                        ast::QualifiedName::cast(parent.clone())
                            .is_some_and(|q| q.qualifier().is_none())
                            && parent
                                .parent()
                                .is_some_and(|p| p.kind() == SyntaxKind::ANNOTATION)
                    }
                    _ => false,
                });
            if !simple {
                continue;
            }
            let Some(Definition::Class(class)) = sema.classify_name_ref(file, &name_ref) else {
                continue;
            };
            let Some(qualified) = importable_name(db, class) else {
                continue;
            };
            // 顶层类且就在原包里（不是子包）
            let same_package = qualified
                .strip_prefix(prefix.as_str())
                .is_some_and(|rest| !rest.contains('.') && rest == name_ref.text().as_str());
            if !same_package {
                continue;
            }
            // 它也被移动了：移到同一个新包时不需要 import，移到别处时 import 新的全名
            let qualified = match new_package_of_class.get(&class) {
                Some(moved_to) if *moved_to == new_package => continue,
                Some(&"") => continue,
                Some(moved_to) => qualify(moved_to, &name_ref.text()),
                None => qualified,
            };
            if !needed.contains(&qualified) {
                needed.push(qualified);
            }
        }
        needed.sort();
        let explicit: FxHashSet<String> = cu
            .imports()
            .filter_map(|i| i.name().map(|n| n.text().to_string()))
            .collect();
        for qualified in needed {
            if !explicit.contains(&qualified) {
                edits
                    .entry(file_id)
                    .or_default()
                    .push(insert_import(db, file, &qualified));
            }
        }
    }
}
