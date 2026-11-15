//! 可以改成 try-with-resources 的 `try` / `finally`（Java 7+）：
//!
//! ```java
//! InputStream in = open();
//! try {
//!     read(in);
//! } finally {
//!     in.close();          // 或 if (in != null) in.close();
//! }
//! // →  try (InputStream in = open()) { read(in); }
//! ```
//!
//! 要求：声明紧挨在 `try` 前面，`try` 没有 `catch`，`finally` 里只有关闭这个变量的一条语句，
//! 变量的类型实现了 `AutoCloseable`，`try` 块里不给它重新赋值，`try` 之后也不再用到它。

use base_db::Db;
use hir::InferenceResult;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange,
};

use crate::{
    pattern_vars::statement_line_range, redundancy::RedundancyFix, Diagnostic, DiagnosticCode,
    Severity,
};

/// 只看语法的预检查。
pub(crate) fn is_candidate(try_stmt: &ast::TryStmt) -> bool {
    try_stmt.resource_list().is_none()
        && try_stmt.catch_clauses().next().is_none()
        && try_stmt.finally_clause().is_some()
        && try_stmt
            .syntax()
            .prev_sibling()
            .is_some_and(|p| p.kind() == SyntaxKind::LOCAL_VAR_DECL)
}

/// `x.close()`
fn is_close_of(expr: &ast::Expr, var: &str) -> bool {
    let ast::Expr::MethodCall(call) = expr else {
        return false;
    };
    call.name_ref().is_some_and(|n| n.text() == "close")
        && call.arg_list().is_some_and(|a| a.args().next().is_none())
        && matches!(call.receiver(), Some(ast::Expr::NameExpr(n)) if n.syntax().text() == var)
}

fn closes(stmt: &ast::Stmt, var: &str) -> bool {
    match stmt {
        ast::Stmt::ExprStmt(s) => s.expr().is_some_and(|e| is_close_of(&e, var)),
        ast::Stmt::Block(block) => {
            let mut stmts = block.statements();
            matches!((stmts.next(), stmts.next()), (Some(only), None) if closes(&only, var))
        }
        // `if (x != null) x.close();`
        ast::Stmt::IfStmt(if_stmt) => {
            if if_stmt.else_branch().is_some() {
                return false;
            }
            let condition = match if_stmt.condition() {
                Some(ast::Expr::ParenExpr(p)) => p.expr(),
                other => other,
            };
            let null_check = matches!(condition, Some(ast::Expr::BinaryExpr(b))
                if b.op_kind() == Some(SyntaxKind::NEQ)
                    && matches!((b.lhs(), b.rhs()), (Some(l), Some(r))
                        if (l.syntax().text() == var && r.syntax().text() == "null")
                            || (l.syntax().text() == "null" && r.syntax().text() == var)));
            null_check && if_stmt.then_branch().is_some_and(|t| closes(&t, var))
        }
        _ => false,
    }
}

pub(crate) fn check(
    db: &dyn Db,
    text: &str,
    result: &InferenceResult,
    try_stmt: &ast::TryStmt,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    let decl = ast::LocalVarDecl::cast(try_stmt.syntax().prev_sibling()?)?;
    let mut declarators = decl.declarators();
    let (Some(declarator), None) = (declarators.next(), declarators.next()) else {
        return None;
    };
    let init = declarator.initializer()?;
    if declarator.extra_dims() > 0 || init.syntax().text() == "null" {
        return None;
    }
    let name = declarator.name()?;
    let var = name.text().to_string();
    let finally = try_stmt.finally_clause()?;
    let finally_block = finally.body()?;
    let mut stmts = finally_block.statements();
    let (Some(only), None) = (stmts.next(), stmts.next()) else {
        return None;
    };
    if !closes(&only, &var) {
        return None;
    }
    // finally 里的注释会丢
    if finally
        .syntax()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind().is_trivia() && t.kind() != SyntaxKind::WHITESPACE)
    {
        return None;
    }
    // 资源变量是隐式 final 的
    let body = try_stmt.body()?;
    let reassigned = body.syntax().descendants().any(|n| {
        ast::AssignExpr::cast(n)
            .and_then(|a| a.lhs())
            .is_some_and(|l| l.syntax().text() == var.as_str())
    });
    if reassigned {
        return None;
    }
    // try 之后不能再用到它（作用域会缩小到 try 里）
    let mut next = try_stmt.syntax().next_sibling();
    while let Some(sibling) = next {
        let used = sibling
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == SyntaxKind::IDENT && t.text() == var);
        if used {
            return None;
        }
        next = sibling.next_sibling();
    }
    // 类型实现了 AutoCloseable
    let local = result.local_at_decl(name.syntax().text_range())?;
    let closeable = hir::lookup_class(db, "java/lang/AutoCloseable")?;
    let ty = &result.local(local).ty;
    if !matches!(ty, hir::Ty::Class(..)) || hir::find_super(db, ty, closeable).is_none() {
        return None;
    }
    let try_kw = try_stmt
        .syntax()
        .children_with_tokens()
        .find(|e| e.kind() == SyntaxKind::TRY_KW)?
        .into_token()?;
    let resource = decl
        .syntax()
        .text()
        .to_string()
        .trim_end()
        .trim_end_matches(';')
        .trim_end()
        .to_owned();
    out.push(RedundancyFix {
        diagnostic: Diagnostic {
            range: try_kw.text_range(),
            message: base_db::trs!(
                "可以改成 try-with-resources",
                "Can be replaced with try-with-resources"
            )
            .to_owned(),
            severity: Severity::Hint,
            code: DiagnosticCode::TryWithResources,
            unused: false,
        },
        label: base_db::trs!("改成 try-with-resources", "Replace with try-with-resources"),
        edits: vec![
            (
                statement_line_range(text, decl.syntax().text_range()),
                String::new(),
            ),
            (
                TextRange::empty(try_kw.text_range().end()),
                format!(" ({resource})"),
            ),
            (
                TextRange::new(
                    body.syntax().text_range().end(),
                    finally.syntax().text_range().end(),
                ),
                String::new(),
            ),
        ],
    });
    Some(())
}
