//! 多余的修饰符：写不写都一样的那些。
//!
//! - 接口 / 注解里：方法的 `public`、无方法体方法的 `abstract`；字段的 `public` `static`
//!   `final`；嵌套类型的 `public` `static`；
//! - 嵌套的接口、枚举、记录、注解类型本来就是静态的：`static`；
//! - 枚举的构造器本来就是私有的：`private`；
//! - 记录本来就是 final 的：`final`；接口本来就是 abstract 的：`abstract`。

use syntax::{ast, ast::AstNode, SyntaxKind, SyntaxNode, SyntaxToken};

use crate::{Diagnostic, DiagnosticCode, Severity};

fn is_type_decl(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::CLASS_DECL
            | SyntaxKind::INTERFACE_DECL
            | SyntaxKind::ENUM_DECL
            | SyntaxKind::RECORD_DECL
            | SyntaxKind::ANNOTATION_TYPE_DECL
    )
}

/// 声明上哪些修饰符是多余的。
fn redundant(decl: &SyntaxNode) -> &'static [SyntaxKind] {
    use SyntaxKind::*;
    // 所在的类型（成员声明时）
    let container = decl
        .parent()
        .filter(|p| p.kind() == CLASS_BODY)
        .and_then(|b| b.parent())
        .map(|c| c.kind());
    let in_interface = matches!(container, Some(INTERFACE_DECL | ANNOTATION_TYPE_DECL));
    match decl.kind() {
        METHOD_DECL if in_interface => {
            let has_body = decl.children().any(|c| c.kind() == BLOCK);
            if has_body {
                &[PUBLIC_KW]
            } else {
                &[PUBLIC_KW, ABSTRACT_KW]
            }
        }
        FIELD_DECL if in_interface => &[PUBLIC_KW, STATIC_KW, FINAL_KW],
        CONSTRUCTOR_DECL if container == Some(ENUM_DECL) => &[PRIVATE_KW],
        kind if is_type_decl(kind) && in_interface => match kind {
            INTERFACE_DECL | ANNOTATION_TYPE_DECL => &[PUBLIC_KW, STATIC_KW, ABSTRACT_KW],
            RECORD_DECL => &[PUBLIC_KW, STATIC_KW, FINAL_KW],
            _ => &[PUBLIC_KW, STATIC_KW],
        },
        INTERFACE_DECL | ANNOTATION_TYPE_DECL if container.is_some() => &[STATIC_KW, ABSTRACT_KW],
        INTERFACE_DECL | ANNOTATION_TYPE_DECL => &[ABSTRACT_KW],
        ENUM_DECL if container.is_some() => &[STATIC_KW],
        RECORD_DECL if container.is_some() => &[STATIC_KW, FINAL_KW],
        RECORD_DECL => &[FINAL_KW],
        _ => &[],
    }
}

/// 文件里所有多余的修饰符 token。
pub(crate) fn redundant_modifier_tokens(root: &SyntaxNode) -> Vec<SyntaxToken> {
    let mut res = Vec::new();
    for modifiers in root.descendants().filter_map(ast::ModifierList::cast) {
        let Some(decl) = modifiers.syntax().parent() else {
            continue;
        };
        let kinds = redundant(&decl);
        if kinds.is_empty() {
            continue;
        }
        res.extend(modifiers.keywords().filter(|t| kinds.contains(&t.kind())));
    }
    res
}

pub(crate) fn redundant_modifiers(root: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    for token in redundant_modifier_tokens(root) {
        let text = token.text();
        out.push(Diagnostic {
            range: token.text_range(),
            message: base_db::tr!("修饰符 `{text}` 是多余的", "Modifier `{text}` is redundant"),
            severity: Severity::Hint,
            code: DiagnosticCode::RedundantModifier,
            unused: true,
        });
    }
}
