//! 和循环有关的检查：
//!
//! - 循环条件只由局部变量和常量组成，但循环里没有修改其中任何一个变量，也没有
//!   `break` / `return` / `throw`：条件永远不会变，循环要么不执行要么不会结束；
//! - 在循环里用 `+=` / `s = s + …` 拼接循环外声明的字符串：每次都复制整个字符串，
//!   应当用 `StringBuilder`。

use base_db::{Db, SourceFile};
use hir::{body_syntax, class_data, infer, InferenceResult, LocalId, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode,
};

use crate::{bodies_in_file, Diagnostic, DiagnosticCode, Severity};

fn local_of(result: &InferenceResult, expr: &ast::Expr) -> Option<LocalId> {
    let ast::Expr::NameExpr(name) = expr else {
        return None;
    };
    match result.resolution(name.name_ref()?.syntax().text_range())? {
        Resolution::Local(id) => Some(*id),
        _ => None,
    }
}

/// 条件里用到的局部变量；条件里有别的东西（方法调用、字段、数组元素……）时返回 `None`。
fn condition_locals(result: &InferenceResult, cond: &SyntaxNode) -> Option<Vec<LocalId>> {
    let mut locals = Vec::new();
    for node in cond.descendants() {
        match node.kind() {
            SyntaxKind::NAME_EXPR => {
                let expr = ast::Expr::cast(node)?;
                locals.push(local_of(result, &expr)?);
            }
            SyntaxKind::NAME_REF
            | SyntaxKind::LITERAL
            | SyntaxKind::BINARY_EXPR
            | SyntaxKind::UNARY_EXPR
            | SyntaxKind::PAREN_EXPR => {}
            _ => return None,
        }
    }
    (!locals.is_empty()).then_some(locals)
}

/// `node` 里给 `locals` 中的变量赋值 / 自增自减的地方。
fn writes_any(result: &InferenceResult, node: &SyntaxNode, locals: &[LocalId]) -> bool {
    node.descendants().any(|n| {
        let target = match n.kind() {
            SyntaxKind::ASSIGN_EXPR => ast::AssignExpr::cast(n).and_then(|a| a.lhs()),
            SyntaxKind::POSTFIX_EXPR => n.children().find_map(ast::Expr::cast),
            SyntaxKind::UNARY_EXPR => {
                let increments = n.first_token().is_some_and(|t| {
                    matches!(t.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS)
                });
                if increments {
                    n.children().find_map(ast::Expr::cast)
                } else {
                    None
                }
            }
            _ => None,
        };
        target
            .and_then(|t| local_of(result, &t))
            .is_some_and(|id| locals.contains(&id))
    })
}

/// 循环体里有能离开循环的语句。
fn can_exit(body: &SyntaxNode) -> bool {
    body.descendants().any(|n| {
        matches!(
            n.kind(),
            SyntaxKind::BREAK_STMT
                | SyntaxKind::RETURN_STMT
                | SyntaxKind::THROW_STMT
                | SyntaxKind::YIELD_STMT
        ) || (n.kind() == SyntaxKind::CONTINUE_STMT
            && ast::ContinueStmt::cast(n).is_some_and(|c| c.label().is_some()))
    })
}

fn check_loop(
    result: &InferenceResult,
    cond: Option<ast::Expr>,
    parts: &[SyntaxNode],
    out: &mut Vec<Diagnostic>,
) {
    let Some(cond) = cond else { return };
    // `while (true)` 之类的常量条件由别的检查负责
    let Some(locals) = condition_locals(result, cond.syntax()) else {
        return;
    };
    if writes_any(result, cond.syntax(), &locals) {
        return;
    }
    if parts
        .iter()
        .any(|p| writes_any(result, p, &locals) || can_exit(p))
    {
        return;
    }
    let names: Vec<String> = {
        let mut names: Vec<String> = locals
            .iter()
            .map(|id| format!("`{}`", result.local(*id).name))
            .collect();
        names.dedup();
        names
    };
    let names = names.join(", ");
    out.push(Diagnostic {
        range: cond.syntax().text_range(),
        message: base_db::tr!(
            "循环条件里的 {names} 在循环里没有被修改，条件永远不会改变",
            "{names} in the loop condition is never updated inside the loop"
        ),
        severity: Severity::Warning,
        code: DiagnosticCode::LoopConditionNotUpdated,
        unused: false,
    });
}

fn is_string(db: &dyn Db, ty: &Ty) -> bool {
    ty.as_class()
        .and_then(|c| class_data(db, c))
        .is_some_and(|d| &*d.binary_name == "java/lang/String")
}

/// 拼接：`s += e` 或 `s = s + e`。返回被拼接的局部变量。
fn concatenated_local(
    db: &dyn Db,
    result: &InferenceResult,
    assign: &ast::AssignExpr,
) -> Option<LocalId> {
    let lhs = assign.lhs()?;
    let local = local_of(result, &lhs)?;
    if !is_string(db, &result.local(local).ty) {
        return None;
    }
    match assign.op_token()?.kind() {
        SyntaxKind::PLUS_EQ => Some(local),
        SyntaxKind::EQ => {
            // 最左边的操作数是变量自己
            let mut rhs = assign.rhs()?;
            while let ast::Expr::BinaryExpr(binary) = &rhs {
                if binary.op_kind() != Some(SyntaxKind::PLUS) {
                    return None;
                }
                rhs = binary.lhs()?;
            }
            (local_of(result, &rhs) == Some(local)
                && matches!(assign.rhs(), Some(ast::Expr::BinaryExpr(_))))
            .then_some(local)
        }
        _ => None,
    }
}

const LOOPS: &[SyntaxKind] = &[
    SyntaxKind::WHILE_STMT,
    SyntaxKind::DO_WHILE_STMT,
    SyntaxKind::FOR_STMT,
    SyntaxKind::FOREACH_STMT,
];

fn check_concat(
    db: &dyn Db,
    result: &InferenceResult,
    assign: &ast::AssignExpr,
    out: &mut Vec<Diagnostic>,
) {
    let Some(local) = concatenated_local(db, result, assign) else {
        return;
    };
    let decl = result.local(local).decl_range;
    // 最近的循环（不跨过 lambda / 类体）在变量的声明之外
    let Some(lp) = assign
        .syntax()
        .ancestors()
        .take_while(|n| {
            !matches!(
                n.kind(),
                SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY | SyntaxKind::METHOD_DECL
            )
        })
        .find(|n| LOOPS.contains(&n.kind()))
    else {
        return;
    };
    if lp.text_range().contains_range(decl) {
        return;
    }
    // 循环里还有普通赋值（每轮重新开始）时并没有在循环之间累积
    let reset = lp
        .descendants()
        .filter_map(ast::AssignExpr::cast)
        .any(|other| {
            other.op_token().is_some_and(|t| t.kind() == SyntaxKind::EQ)
                && other.lhs().and_then(|l| local_of(result, &l)) == Some(local)
                && concatenated_local(db, result, &other).is_none()
        });
    if reset {
        return;
    }
    let Some(lhs) = assign.lhs() else { return };
    let name = result.local(local).name.clone();
    out.push(Diagnostic {
        range: lhs.syntax().text_range(),
        message: base_db::tr!(
            "在循环里拼接字符串 `{name}`，每次都会复制整个字符串，考虑用 `StringBuilder`",
            "String `{name}` is concatenated in a loop, copying it every time; consider a `StringBuilder`"
        ),
        severity: Severity::Hint,
        code: DiagnosticCode::StringConcatInLoop,
        unused: false,
    });
}

pub(crate) fn loop_diagnostics(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    for body in bodies_in_file(db, file) {
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for n in node.descendants() {
            match n.kind() {
                SyntaxKind::WHILE_STMT => {
                    let Some(stmt) = ast::WhileStmt::cast(n) else {
                        continue;
                    };
                    let result = result.get_or_insert_with(|| infer(db, body));
                    let parts: Vec<SyntaxNode> = stmt
                        .body()
                        .map(|b| b.syntax().clone())
                        .into_iter()
                        .collect();
                    check_loop(result, stmt.condition(), &parts, out);
                }
                SyntaxKind::DO_WHILE_STMT => {
                    let Some(stmt) = ast::DoWhileStmt::cast(n) else {
                        continue;
                    };
                    let result = result.get_or_insert_with(|| infer(db, body));
                    let parts: Vec<SyntaxNode> = stmt
                        .body()
                        .map(|b| b.syntax().clone())
                        .into_iter()
                        .collect();
                    check_loop(result, stmt.condition(), &parts, out);
                }
                SyntaxKind::FOR_STMT => {
                    let Some(stmt) = ast::ForStmt::cast(n) else {
                        continue;
                    };
                    let result = result.get_or_insert_with(|| infer(db, body));
                    let parts: Vec<SyntaxNode> = stmt
                        .body()
                        .map(|b| b.syntax().clone())
                        .into_iter()
                        .chain(stmt.update().map(|u| u.syntax().clone()))
                        .collect();
                    check_loop(result, stmt.condition(), &parts, out);
                }
                SyntaxKind::ASSIGN_EXPR => {
                    let Some(assign) = ast::AssignExpr::cast(n) else {
                        continue;
                    };
                    let result = result.get_or_insert_with(|| infer(db, body));
                    check_concat(db, result, &assign, out);
                }
                _ => {}
            }
        }
    }
}

/// 循环里拼接字符串的诊断（给快速修复用）。
pub fn string_concat_in_loops(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    loop_diagnostics(db, file, &mut out);
    out.retain(|d| d.code == DiagnosticCode::StringConcatInLoop);
    out
}
