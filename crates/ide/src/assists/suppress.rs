//! 压制光标处的警告：在语句前加 `//noinspection`，或者给所在的局部变量 / 字段 / 方法 / 类
//! 加上 `@SuppressWarnings`（已经有的话把名字并进去）。

use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    NodeOrToken, SyntaxKind, SyntaxNode,
};

use super::{Assist, AssistCtx, AssistKind};

/// 声明上已有的 `@SuppressWarnings`。
fn existing_annotation(decl: &SyntaxNode) -> Option<ast::Annotation> {
    let modifiers = decl.children().find_map(ast::ModifierList::cast)?;
    modifiers.annotations().find(|a| {
        a.name().is_some_and(|n| {
            let text = n.text();
            text == "SuppressWarnings" || text == "java.lang.SuppressWarnings"
        })
    })
}

/// 给声明加上压制 `key` 的编辑。
fn annotate(ctx: &AssistCtx<'_>, decl: &SyntaxNode, key: &str) -> Option<TextEdit> {
    if let Some(annotation) = existing_annotation(decl) {
        let arg = annotation.arg_list()?.args().next()?;
        return match arg.value_expr()? {
            // `{"a", "b"}`：插在 `}` 前面
            ast::Expr::ArrayInitializer(init) => {
                let close = init
                    .syntax()
                    .children_with_tokens()
                    .filter_map(NodeOrToken::into_token)
                    .find(|t| t.kind() == SyntaxKind::R_CURLY)?;
                let empty = init.syntax().children().next().is_none();
                let text = if empty {
                    format!("\"{key}\"")
                } else {
                    format!(", \"{key}\"")
                };
                Some(TextEdit::insert(close.text_range().start(), text))
            }
            // `"a"`：换成 `{"a", "key"}`
            single => Some(TextEdit::replace(
                single.syntax().text_range(),
                format!("{{{}, \"{key}\"}}", single.syntax().text()),
            )),
        };
    }
    // 插在声明的第一个有效 token 之前（文档注释之后）
    let first = ast::support::first_token_in_subtree(decl)?;
    let start = first.text_range().start();
    let indent = ctx.indent_at(start);
    Some(TextEdit::insert(
        start,
        format!("@SuppressWarnings(\"{key}\")\n{indent}"),
    ))
}

fn is_class_decl(node: &SyntaxNode) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::CLASS_DECL
            | SyntaxKind::INTERFACE_DECL
            | SyntaxKind::ENUM_DECL
            | SyntaxKind::RECORD_DECL
    )
}

pub(super) fn suppress_warning(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let diagnostics = ide_diagnostics::diagnostics(ctx.db, ctx.file, &Default::default());
    let mut seen: Vec<&str> = Vec::new();
    for diagnostic in diagnostics
        .iter()
        .filter(|d| d.range.contains_inclusive(ctx.range.start()))
    {
        let Some(key) = ide_diagnostics::suppression_key(diagnostic) else {
            continue;
        };
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let node = match ctx.root.covering_element(diagnostic.range) {
            NodeOrToken::Node(node) => node,
            NodeOrToken::Token(token) => match token.parent() {
                Some(p) => p,
                None => continue,
            },
        };
        // 语句：块里的一条语句（局部变量声明用注解，下面处理）
        let stmt = node.ancestors().find(|n| {
            ast::Stmt::can_cast(n.kind())
                && !is_class_decl(n)
                && n.parent().is_some_and(|p| p.kind() == SyntaxKind::BLOCK)
        });
        if let Some(stmt) = stmt.filter(|s| s.kind() != SyntaxKind::LOCAL_VAR_DECL) {
            if let Some(first) = ast::support::first_token_in_subtree(&stmt) {
                let start = first.text_range().start();
                let indent = ctx.indent_at(start);
                out.push(ctx.assist(
                    "suppress_statement",
                    base_db::tr!("对这条语句压制 `{key}`", "Suppress `{key}` for statement"),
                    AssistKind::QuickFix,
                    vec![TextEdit::insert(
                        start,
                        format!("//noinspection {key}\n{indent}"),
                    )],
                ));
            }
        }
        // 声明：局部变量、字段、方法 / 构造器、类
        let mut offered_class = false;
        for decl in node.ancestors() {
            let (id, label) = match decl.kind() {
                SyntaxKind::LOCAL_VAR_DECL => (
                    "suppress_declaration",
                    base_db::tr!("对这个变量压制 `{key}`", "Suppress `{key}` for variable"),
                ),
                SyntaxKind::FIELD_DECL => (
                    "suppress_declaration",
                    base_db::tr!("对这个字段压制 `{key}`", "Suppress `{key}` for field"),
                ),
                SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL => (
                    "suppress_method",
                    base_db::tr!("对这个方法压制 `{key}`", "Suppress `{key}` for method"),
                ),
                _ if is_class_decl(&decl) && !offered_class => {
                    offered_class = true;
                    (
                        "suppress_class",
                        base_db::tr!("对这个类压制 `{key}`", "Suppress `{key}` for class"),
                    )
                }
                _ => continue,
            };
            if let Some(edit) = annotate(ctx, &decl, key) {
                out.push(ctx.assist(id, label, AssistKind::QuickFix, vec![edit]));
            }
            if is_class_decl(&decl) {
                break;
            }
        }
    }
}
