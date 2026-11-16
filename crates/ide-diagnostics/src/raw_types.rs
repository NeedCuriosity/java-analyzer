//! 裸用泛型类：`List names = new ArrayList();` 里的 `List` 和 `ArrayList`。
//!
//! 语言本身不得不用裸类型的地方不报：`instanceof`、强制转换、数组创建（`new List[n]`）、
//! 类字面量、方法引用、注解、`throws`、模式；覆写方法的签名（跟着父类型走）也不报。
//! `new X()` 赋给带类型实参的声明时，修复是加上 `<>`。

use base_db::{Db, SourceFile};
use hir::{class_data, Semantics, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use crate::{redundancy::RedundancyFix, Diagnostic, DiagnosticCode, Severity};

/// 语法上可能是裸类型的位置：没有类型实参、不是限定名的前缀、不在豁免的语境里。
pub(crate) fn is_candidate(node: &SyntaxNode) -> bool {
    let Some(class_ty) = ast::ClassType::cast(node.clone()) else {
        return false;
    };
    if class_ty.type_arg_list().is_some() {
        return false;
    }
    // 越过数组维度，看类型用在哪里
    let mut parent = node.parent();
    while let Some(p) = parent
        .as_ref()
        .filter(|p| p.kind() == SyntaxKind::ARRAY_TYPE)
    {
        parent = p.parent();
    }
    let Some(parent) = parent else {
        return false;
    };
    if matches!(
        parent.kind(),
        SyntaxKind::CLASS_TYPE
            | SyntaxKind::INSTANCEOF_EXPR
            | SyntaxKind::CAST_EXPR
            | SyntaxKind::ARRAY_CREATION_EXPR
            | SyntaxKind::CLASS_LITERAL
            | SyntaxKind::METHOD_REF
            | SyntaxKind::ANNOTATION
            | SyntaxKind::THROWS_CLAUSE
            | SyntaxKind::TYPE_PATTERN
            | SyntaxKind::RECORD_PATTERN
            | SyntaxKind::IMPORT_DECL
            | SyntaxKind::CATCH_CLAUSE
    ) {
        return false;
    }
    // 覆写方法的形参和返回类型
    let in_override = parent
        .ancestors()
        .take_while(|a| !matches!(a.kind(), SyntaxKind::BLOCK | SyntaxKind::CLASS_BODY))
        .find_map(ast::MethodDecl::cast)
        .is_some_and(|method| {
            method.modifier_list().is_some_and(|mods| {
                mods.annotations().any(|a| {
                    a.name()
                        .and_then(|n| n.segments().last().map(|s| s.to_string()))
                        .as_deref()
                        == Some("Override")
                })
            })
        });
    !in_override
}

/// 同一个类里同一个名字解析到同一个类：按（最内层类型声明，名字）缓存"是不是泛型类"。
pub(crate) type Cache = rustc_hash::FxHashMap<(TextSize, String), Option<String>>;

pub(crate) fn check(
    db: &dyn Db,
    sema: &Semantics<'_>,
    file: SourceFile,
    node: &SyntaxNode,
    cache: &mut Cache,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    let class_ty = ast::ClassType::cast(node.clone())?;
    let scope = node
        .ancestors()
        .find(|a| ast::TypeDecl::can_cast(a.kind()))
        .map_or(TextSize::from(0), |a| a.text_range().start());
    let key = (scope, class_ty.syntax().text().to_string());
    let generic_name = match cache.get(&key) {
        Some(found) => found.clone(),
        None => {
            let found = match sema.resolve_type(file, &ast::Type::ClassType(class_ty.clone())) {
                Ty::Class(class, args) if args.is_empty() => class_data(db, class)
                    .filter(|d| !d.type_params.is_empty())
                    .map(|d| d.name.to_string()),
                _ => None,
            };
            cache.insert(key, found.clone());
            found
        }
    };
    let name = generic_name?;
    let name_range = class_ty.name_ref()?.syntax().text_range();
    let edits = diamond_fix(node).unwrap_or_default();
    out.push(RedundancyFix {
        diagnostic: Diagnostic {
            range: name_range,
            message: base_db::tr!(
                "裸用了泛型类 `{}`",
                "Raw use of parameterized class `{}`",
                name
            ),
            severity: Severity::Warning,
            code: DiagnosticCode::RawType,
            unused: false,
        },
        label: base_db::trs!("加上 `<>`", "Add `<>`"),
        edits,
    });
    Some(())
}

/// `List<String> xs = new ArrayList();` → `new ArrayList<>()`
fn diamond_fix(node: &SyntaxNode) -> Option<Vec<(TextRange, String)>> {
    let new_expr = ast::NewExpr::cast(node.parent()?)?;
    if new_expr.body().is_some() {
        return None;
    }
    let declarator = new_expr
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::VARIABLE_DECLARATOR)?;
    let declared = declarator.parent()?.children().find_map(ast::Type::cast)?;
    let ast::Type::ClassType(declared) = declared else {
        return None;
    };
    declared.type_arg_list()?;
    Some(vec![(
        TextRange::empty(node.text_range().end()),
        "<>".to_owned(),
    )])
}
