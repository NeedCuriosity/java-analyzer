//! 查找引用。
//!
//! 先按名字做纯文本筛选（绝大多数文件根本不含这个标识符），
//! 只对命中的位置做语义确认，避免对整个工作区做类型推断。

use base_db::{workspace, Db, FileId, FileKind, SourceFile};
use hir::{
    class_data, class_decl_file, is_subclass_of, method_sig, ClassKind, Definition, MethodId,
    Modifiers, Semantics, Visibility,
};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use crate::definition_flags;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReferenceKind {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reference {
    pub file_id: FileId,
    pub range: TextRange,
    pub kind: ReferenceKind,
}

/// 两个方法是否处于同一条覆写链上（同名、同参数个数与类型、所在类有继承关系）。
fn same_method_family(db: &dyn Db, a: MethodId, b: MethodId) -> bool {
    if a == b {
        return true;
    }
    let (Some(a_data), Some(b_data)) = (class_data(db, a.class), class_data(db, b.class)) else {
        return false;
    };
    let (Some(a_method), Some(b_method)) = (
        a_data.methods.get(a.idx as usize),
        b_data.methods.get(b.idx as usize),
    ) else {
        return false;
    };
    if a_method.is_constructor
        || b_method.is_constructor
        || a_method.name != b_method.name
        || a_method.params.len() != b_method.params.len()
        || a_method.flags.is_static()
        || b_method.flags.is_static()
    {
        return false;
    }
    if !is_subclass_of(db, a.class, b.class) && !is_subclass_of(db, b.class, a.class) {
        return false;
    }
    // 形参类型按"类是否相同"粗略比较，足以区分重载
    let a_sig = method_sig(db, a);
    let b_sig = method_sig(db, b);
    a_sig
        .params
        .iter()
        .zip(b_sig.params.iter())
        .all(|(pa, pb)| match (pa, pb) {
            (hir::Ty::Class(ca, _), hir::Ty::Class(cb, _)) => ca == cb,
            (hir::Ty::Primitive(x), hir::Ty::Primitive(y)) => x == y,
            (hir::Ty::Array(_), hir::Ty::Array(_)) => true,
            (hir::Ty::TypeParam(_), _) | (_, hir::Ty::TypeParam(_)) => true,
            (hir::Ty::Unknown, _) | (_, hir::Ty::Unknown) => true,
            _ => false,
        })
}

/// `found`（某处名字解析到的定义）是否算作对 `target` 的引用。
pub fn refers_to(db: &dyn Db, found: &Definition, target: &Definition) -> bool {
    if found == target {
        return true;
    }
    match (found, target) {
        // `new Foo()` 解析到构造器，同时也是对类 Foo 的引用
        (Definition::Method(method), Definition::Class(class)) => {
            method.class == *class
                && class_data(db, method.class)
                    .and_then(|d| d.methods.get(method.idx as usize).map(|m| m.is_constructor))
                    .unwrap_or(false)
        }
        (Definition::Method(found), Definition::Method(target)) => {
            same_method_family(db, *found, *target)
        }
        // 记录的隐式访问器 `p.x()` 也是对组件 `x` 的引用
        (Definition::Method(method), Definition::Field(field)) => {
            method.class == field.class
                && class_data(db, method.class).is_some_and(|data| {
                    let accessor = data.methods.get(method.idx as usize);
                    let component = data.fields.get(field.idx as usize);
                    matches!((accessor, component), (Some(m), Some(f))
                        if data.kind == ClassKind::Record
                            && m.flags.contains(Modifiers::SYNTHETIC)
                            && m.params.is_empty()
                            && !f.flags.is_static()
                            && m.name == f.name)
                })
        }
        _ => false,
    }
}

/// 需要搜索哪些文件。
fn search_scope(db: &dyn Db, def: &Definition) -> Vec<(FileId, SourceFile)> {
    let single = |class| {
        class_decl_file(db, class)
            .map(|file| vec![(file.file_id(db), file)])
            .unwrap_or_default()
    };
    match def {
        Definition::Local(local) => single(local.body.class()),
        Definition::TypeParam(_) => def.container_class().map(single).unwrap_or_default(),
        Definition::Field(_) | Definition::Method(_)
            if definition_flags(db, def).visibility() == Visibility::Private =>
        {
            def.container_class().map(single).unwrap_or_default()
        }
        // 局部类 / 匿名类（及其成员类）只在声明它的文件里可见
        Definition::Class(class) if in_local_class(db, *class) => single(*class),
        Definition::Field(field) if in_local_class(db, field.class) => single(field.class),
        // 覆写父类型方法的除外：通过父类型的调用也算引用
        Definition::Method(method)
            if in_local_class(db, method.class) && !overrides_something(db, *method) =>
        {
            single(method.class)
        }
        _ => {
            let files = workspace(db).files(db);
            let mut res = files.workspace_files();
            // 依赖库里的符号：打开的库源码文件里的用法也算（库源码文件里的高亮、引用）
            let class = match def {
                Definition::Class(class) => Some(*class),
                _ => def.container_class(),
            };
            let is_library = class
                .is_some_and(|class| class_data(db, class).is_some_and(|data| !data.is_source));
            if is_library {
                let mut library: Vec<(FileId, SourceFile)> = files
                    .iter()
                    .filter(|(_, entry)| matches!(entry.kind, FileKind::Library))
                    .map(|(id, entry)| (id, entry.source))
                    .collect();
                library.sort_by_key(|(id, _)| *id);
                res.extend(library);
            }
            res
        }
    }
}

/// 局部类、匿名类或者它们的成员类：二进制名里有以数字开头的一段（`Outer$1Local`、`Outer$1`）。
fn in_local_class(db: &dyn Db, class: hir::ClassId) -> bool {
    class
        .binary_name(db)
        .split('$')
        .skip(1)
        .any(|part| part.starts_with(|c: char| c.is_ascii_digit()))
        && class_data(db, class).is_some_and(|data| data.is_source)
}

/// 父类型里有同名、同参数个数的方法（可能被 `method` 覆写）。
fn overrides_something(db: &dyn Db, method: MethodId) -> bool {
    let Some(data) = class_data(db, method.class) else {
        return true;
    };
    let Some(m) = data.methods.get(method.idx as usize) else {
        return true;
    };
    hir::all_supers(db, method.class).into_iter().any(|sup| {
        class_data(db, sup).is_none_or(|sup_data| {
            sup_data
                .methods_named(&m.name)
                .any(|(_, other)| other.params.len() == m.params.len())
        })
    })
}

/// 名字引用所在的表达式是不是被写入（赋值的左侧、`++` / `--` 的操作数）。
fn is_write_access(name_ref: &SyntaxNode) -> bool {
    let Some(expr) = name_ref
        .parent()
        .filter(|p| matches!(p.kind(), SyntaxKind::NAME_EXPR | SyntaxKind::FIELD_ACCESS))
    else {
        return false;
    };
    let Some(parent) = expr.parent() else {
        return false;
    };
    match parent.kind() {
        SyntaxKind::ASSIGN_EXPR => parent.first_child().is_some_and(|first| first == expr),
        SyntaxKind::UNARY_EXPR | SyntaxKind::POSTFIX_EXPR => parent
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .any(|t| matches!(t.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS)),
        _ => false,
    }
}

/// 在一个文件里找对 `def` 的引用（不含声明本身）。
pub fn references_in_file(
    db: &dyn Db,
    file: SourceFile,
    def: &Definition,
    name: &str,
) -> Vec<Reference> {
    let text = file.text(db);
    if name.is_empty() || !text.contains(name) {
        return Vec::new();
    }
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let file_id = file.file_id(db);
    let mut res = Vec::new();

    for (offset, _) in text.match_indices(name) {
        let Some(token) = root
            .token_at_offset(TextSize::from(offset as u32))
            .right_biased()
        else {
            continue;
        };
        if token.kind() != SyntaxKind::IDENT || token.text() != name {
            continue;
        }
        let Some(name_ref) = token.parent().and_then(ast::NameRef::cast) else {
            continue;
        };
        let Some(found) = sema.classify_name_ref(file, &name_ref) else {
            continue;
        };
        if refers_to(db, &found, def) {
            let kind = if is_write_access(name_ref.syntax()) {
                ReferenceKind::Write
            } else {
                ReferenceKind::Read
            };
            res.push(Reference {
                file_id,
                range: token.text_range(),
                kind,
            });
        }
    }
    res
}

/// 在整个搜索范围里找对 `def` 的引用（不含声明本身）。
pub fn find_references(db: &dyn Db, def: &Definition) -> Vec<Reference> {
    let Some(name) = def.name(db) else {
        return Vec::new();
    };
    let mut res = Vec::new();
    for (_, file) in search_scope(db, def) {
        // 引用搜索可能很久；每个文件检查一次是否已被新的编辑取消
        db.unwind_if_revision_cancelled();
        res.extend(references_in_file(db, file, def, &name));
    }
    res
}
