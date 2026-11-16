//! 空的代码块和空语句：
//!
//! - 空的 `catch` 块（异常被悄悄吞掉）。形参名是 `ignored` / `ignore` / `expected` / `_` 开头，
//!   或者块里写了注释时，认为是有意为之，不报；
//! - 空的 `finally` 块；
//! - `if` / `else` 的分支、条件没有副作用的 `while` 的循环体只有一个 `;`（多半是多打了分号）。

use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode,
};

use crate::{Diagnostic, DiagnosticCode, Severity};

fn is_empty_block(block: &ast::Block) -> bool {
    block.statements().next().is_none()
        && !block
            .syntax()
            .descendants_with_tokens()
            .any(|e| e.kind().is_trivia() && e.kind() != SyntaxKind::WHITESPACE)
}

/// 条件里有调用、赋值、自增自减（`while` 的循环体为空时在条件里干活）。
fn has_side_effects(stmt: &SyntaxNode) -> bool {
    stmt.children()
        .filter(|c| c.kind() != SyntaxKind::EMPTY_STMT)
        .flat_map(|c| c.descendants().collect::<Vec<_>>())
        .any(|n| {
            matches!(
                n.kind(),
                SyntaxKind::METHOD_CALL
                    | SyntaxKind::NEW_EXPR
                    | SyntaxKind::ASSIGN_EXPR
                    | SyntaxKind::POSTFIX_EXPR
            ) || (n.kind() == SyntaxKind::UNARY_EXPR
                && n.children_with_tokens()
                    .any(|t| matches!(t.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS)))
        })
}

fn warning(range: syntax::TextRange, code: DiagnosticCode, message: String) -> Diagnostic {
    Diagnostic {
        range,
        message,
        severity: Severity::Warning,
        code,
        unused: false,
    }
}

pub(crate) fn empty_blocks(root: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    for node in root.descendants() {
        match node.kind() {
            SyntaxKind::CATCH_CLAUSE => {
                let Some(catch) = ast::CatchClause::cast(node) else {
                    continue;
                };
                let Some(body) = catch.body() else {
                    continue;
                };
                let intended = catch.param().and_then(|p| p.name()).is_some_and(|n| {
                    let name = n.text();
                    name.starts_with("ignore")
                        || name.starts_with("expected")
                        || name.starts_with('_')
                });
                if intended || !is_empty_block(&body) {
                    continue;
                }
                let Some(keyword) = catch.syntax().first_token() else {
                    continue;
                };
                out.push(warning(
                    keyword.text_range(),
                    DiagnosticCode::EmptyBlock,
                    base_db::trs!(
                        "空的 `catch` 块：异常被忽略了",
                        "Empty `catch` block: the exception is ignored"
                    )
                    .to_owned(),
                ));
            }
            SyntaxKind::FINALLY_CLAUSE => {
                let Some(finally) = ast::FinallyClause::cast(node) else {
                    continue;
                };
                if !finally.body().is_some_and(|b| is_empty_block(&b)) {
                    continue;
                }
                let Some(keyword) = finally.syntax().first_token() else {
                    continue;
                };
                out.push(warning(
                    keyword.text_range(),
                    DiagnosticCode::EmptyBlock,
                    base_db::trs!("空的 `finally` 块", "Empty `finally` block").to_owned(),
                ));
            }
            SyntaxKind::EMPTY_STMT => {
                let Some(parent) = node.parent() else {
                    continue;
                };
                let body_of = match parent.kind() {
                    SyntaxKind::IF_STMT => "if",
                    // `while ((c = read()) != -1);` 在条件里干活，是有意的写法
                    SyntaxKind::WHILE_STMT if !has_side_effects(&parent) => "while",
                    _ => continue,
                };
                out.push(warning(
                    node.text_range(),
                    DiagnosticCode::EmptyStatementBody,
                    base_db::tr!(
                        "`{body_of}` 的语句体是空语句 `;`",
                        "`{body_of}` has an empty statement `;` as its body"
                    ),
                ));
            }
            _ => {}
        }
    }
}
