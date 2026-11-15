//! 无限递归：方法在任何能返回的地方之前，无条件地调用了自己
//! （`void setName(String n) { setName(n); }`、`int size() { return size(); }`）。
//!
//! 只沿着一定会执行的路径找：按顺序看方法体的语句，遇到可能离开方法的语句（`return`、
//! `throw`、`break`……）就停；表达式里跳过 `&&` / `||` 的右侧、`?:` 的分支、lambda、
//! 匿名类和 `switch` 的分支；`if` / 循环只看条件，`try` 整个跳过。

use base_db::{Db, SourceFile};
use hir::{body_syntax, infer, InferenceResult, MethodId, Resolution};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{bodies_in_file, Diagnostic, DiagnosticCode, Severity};

struct Finder<'a> {
    result: &'a InferenceResult,
    method: MethodId,
}

impl Finder<'_> {
    /// 一定会求值的子表达式里对自己的调用。
    fn in_expr(&self, node: &SyntaxNode) -> Option<TextRange> {
        match node.kind() {
            SyntaxKind::LAMBDA_EXPR | SyntaxKind::SWITCH_EXPR | SyntaxKind::METHOD_REF => {
                return None
            }
            SyntaxKind::NEW_EXPR => {
                // 实参会求值，匿名类体不会
                for child in node.children() {
                    if child.kind() == SyntaxKind::CLASS_BODY {
                        continue;
                    }
                    if let Some(found) = self.in_expr(&child) {
                        return Some(found);
                    }
                }
                return None;
            }
            SyntaxKind::BINARY_EXPR => {
                let binary = ast::BinaryExpr::cast(node.clone())?;
                if matches!(
                    binary.op_kind(),
                    Some(SyntaxKind::AMP_AMP | SyntaxKind::PIPE_PIPE)
                ) {
                    return binary.lhs().and_then(|l| self.in_expr(l.syntax()));
                }
            }
            SyntaxKind::CONDITIONAL_EXPR => {
                let cond = ast::ConditionalExpr::cast(node.clone())?;
                return cond.condition().and_then(|c| self.in_expr(c.syntax()));
            }
            SyntaxKind::METHOD_CALL => {
                let call = ast::MethodCall::cast(node.clone())?;
                if let Some(found) = self.self_call(&call) {
                    return Some(found);
                }
            }
            _ => {}
        }
        node.children().find_map(|c| self.in_expr(&c))
    }

    fn self_call(&self, call: &ast::MethodCall) -> Option<TextRange> {
        let name = call.name_ref()?;
        let Resolution::Method(target) = self.result.resolution(name.syntax().text_range())? else {
            return None;
        };
        if *target != self.method {
            return None;
        }
        // 不带限定，或者以不带限定的 `this` 限定（`other.foo()` 是别的对象）
        match call.receiver() {
            None => Some(name.syntax().text_range()),
            Some(ast::Expr::ThisExpr(this)) if this.qualifier().is_none() => {
                Some(name.syntax().text_range())
            }
            _ => None,
        }
    }

    /// 按顺序执行的语句里一定会发生的自调用；`Err(())` 表示遇到了可能离开的语句。
    fn in_stmts(&self, stmts: impl Iterator<Item = SyntaxNode>) -> Result<Option<TextRange>, ()> {
        for stmt in stmts {
            if let Some(found) = self.in_stmt(&stmt) {
                return Ok(Some(found));
            }
            if may_leave(&stmt) {
                return Err(());
            }
        }
        Ok(None)
    }

    fn in_stmt(&self, stmt: &SyntaxNode) -> Option<TextRange> {
        let first_expr = |n: &SyntaxNode| -> Option<TextRange> {
            n.children()
                .find(|c| ast::Expr::can_cast(c.kind()))
                .and_then(|e| self.in_expr(&e))
        };
        match stmt.kind() {
            SyntaxKind::EXPR_STMT
            | SyntaxKind::RETURN_STMT
            | SyntaxKind::THROW_STMT
            | SyntaxKind::LOCAL_VAR_DECL => self.in_expr(stmt),
            SyntaxKind::IF_STMT => ast::IfStmt::cast(stmt.clone())
                .and_then(|s| s.condition())
                .and_then(|c| self.in_expr(c.syntax())),
            SyntaxKind::WHILE_STMT => ast::WhileStmt::cast(stmt.clone())
                .and_then(|s| s.condition())
                .and_then(|c| self.in_expr(c.syntax())),
            SyntaxKind::FOR_STMT => {
                let s = ast::ForStmt::cast(stmt.clone())?;
                s.init()
                    .and_then(|i| self.in_expr(i.syntax()))
                    .or_else(|| s.condition().and_then(|c| self.in_expr(c.syntax())))
            }
            SyntaxKind::FOREACH_STMT => ast::ForeachStmt::cast(stmt.clone())
                .and_then(|s| s.iterable())
                .and_then(|e| self.in_expr(e.syntax())),
            SyntaxKind::SWITCH_STMT => first_expr(stmt),
            SyntaxKind::BLOCK => self.in_stmts(stmt.children()).ok().flatten(),
            SyntaxKind::SYNCHRONIZED_STMT => first_expr(stmt).or_else(|| {
                stmt.children()
                    .find(|c| c.kind() == SyntaxKind::BLOCK)
                    .and_then(|b| self.in_stmts(b.children()).ok().flatten())
            }),
            _ => None,
        }
    }
}

/// 语句里有可能离开方法（或跳出顺序执行）的东西。
fn may_leave(stmt: &SyntaxNode) -> bool {
    stmt.descendants().any(|n| {
        matches!(
            n.kind(),
            SyntaxKind::RETURN_STMT
                | SyntaxKind::THROW_STMT
                | SyntaxKind::BREAK_STMT
                | SyntaxKind::CONTINUE_STMT
                | SyntaxKind::YIELD_STMT
        )
    })
}

pub(crate) fn infinite_recursion(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    for body in bodies_in_file(db, file) {
        let hir::BodyId::Method(method) = body else {
            continue;
        };
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let Some(method_decl) = ast::MethodDecl::cast(node) else {
            continue;
        };
        let (Some(name), Some(block)) = (method_decl.name(), method_decl.body()) else {
            continue;
        };
        let node = block.syntax().clone();
        // 快速排除：方法体里根本没出现方法名
        let text: &str = file.text(db);
        if !text[node.text_range()].contains(name.text().as_str()) {
            continue;
        }
        let result = infer(db, body);
        let finder = Finder {
            result: &result,
            method,
        };
        if let Ok(Some(range)) = finder.in_stmts(node.children()) {
            let method_name = name.text().to_string();
            out.push(Diagnostic {
                range,
                message: base_db::tr!(
                    "`{method_name}()` 在返回之前总会调用自己，递归不会结束",
                    "`{method_name}()` always calls itself before it can return; the recursion never ends"
                ),
                severity: Severity::Warning,
                code: DiagnosticCode::InfiniteRecursion,
                unused: false,
            });
        }
    }
}
