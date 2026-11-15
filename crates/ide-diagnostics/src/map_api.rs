//! 可以用 Java 8 的 `Map` 方法简化的写法：
//!
//! - `m.containsKey(k) ? m.get(k) : d` → `m.getOrDefault(k, d)`；
//! - `if (!m.containsKey(k)) m.put(k, v);` → `m.putIfAbsent(k, v)`（`v` 是没有副作用的简单表达式）；
//! - `V v = m.get(k); if (v == null) { v = new …; m.put(k, v); }` →
//!   `V v = m.computeIfAbsent(k, key -> new …);`。
//!
//! `m` 和 `k` 只接受变量、字段和 `this.f` 这类重复求值不会有区别的写法。

use base_db::{Db, SourceFile};
use hir::{body_syntax, infer, InferenceResult, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{
    bodies_in_file, pattern_vars::statement_line_range, redundancy::RedundancyFix, Diagnostic,
    DiagnosticCode, Severity,
};

/// 重复求值不会有区别的表达式：变量、字面量、`this`、`a.b.c`。
fn is_simple(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::NameExpr(_) | ast::Expr::Literal(_) | ast::Expr::ThisExpr(_) => true,
        ast::Expr::FieldAccess(access) => access.receiver().is_some_and(|r| is_simple(&r)),
        _ => false,
    }
}

fn unparen(expr: ast::Expr) -> ast::Expr {
    match &expr {
        ast::Expr::ParenExpr(paren) => paren.expr().map_or(expr, unparen),
        _ => expr,
    }
}

fn text(node: &SyntaxNode) -> String {
    node.text()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// `m.name(args…)`，`m` 是 `Map`：返回 `(m, args)`。
fn map_call(
    db: &dyn Db,
    result: &InferenceResult,
    expr: &ast::Expr,
    name: &str,
    arity: usize,
) -> Option<(ast::Expr, Vec<ast::Expr>)> {
    let ast::Expr::MethodCall(call) = expr else {
        return None;
    };
    if call.name_ref()?.text() != name {
        return None;
    }
    let receiver = call.receiver()?;
    let args: Vec<ast::Expr> = call.arg_list()?.args().collect();
    if args.len() != arity || !is_simple(&receiver) {
        return None;
    }
    let ty = result.type_of(receiver.syntax().text_range())?;
    let map = hir::lookup_class(db, "java/util/Map")?;
    if !matches!(ty, Ty::Class(..)) || hir::find_super(db, ty, map).is_none() {
        return None;
    }
    Some((receiver, args))
}

fn diagnostic(range: TextRange, method: &str) -> Diagnostic {
    Diagnostic {
        range,
        message: base_db::tr!("可以换成 `{method}()`", "Can be replaced with `{method}()`"),
        severity: Severity::Hint,
        code: DiagnosticCode::MapApi,
        unused: false,
    }
}

fn get_or_default(
    db: &dyn Db,
    result: &InferenceResult,
    cond: &ast::ConditionalExpr,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    let condition = unparen(cond.condition()?);
    let (map, args) = map_call(db, result, &condition, "containsKey", 1)?;
    let key = &args[0];
    if !is_simple(key) {
        return None;
    }
    let (get_map, get_args) = map_call(db, result, &cond.then_expr()?, "get", 1)?;
    if text(get_map.syntax()) != text(map.syntax())
        || text(get_args[0].syntax()) != text(key.syntax())
    {
        return None;
    }
    let default = cond.else_expr()?;
    out.push(RedundancyFix {
        diagnostic: diagnostic(condition.syntax().text_range(), "getOrDefault"),
        label: base_db::trs!("换成 `getOrDefault()`", "Replace with `getOrDefault()`"),
        edits: vec![(
            cond.syntax().text_range(),
            format!(
                "{}.getOrDefault({}, {})",
                map.syntax().text(),
                key.syntax().text(),
                default.syntax().text()
            ),
        )],
    });
    Some(())
}

fn single_statement(stmt: ast::Stmt) -> Option<ast::Stmt> {
    match stmt {
        ast::Stmt::Block(block) => {
            let mut stmts = block.statements();
            match (stmts.next(), stmts.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
        other => Some(other),
    }
}

fn put_if_absent(
    db: &dyn Db,
    result: &InferenceResult,
    if_stmt: &ast::IfStmt,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if if_stmt.else_branch().is_some() {
        return None;
    }
    let ast::Expr::UnaryExpr(not) = unparen(if_stmt.condition()?) else {
        return None;
    };
    if not.op_token()?.kind() != SyntaxKind::BANG {
        return None;
    }
    let contains = unparen(not.operand()?);
    let (map, args) = map_call(db, result, &contains, "containsKey", 1)?;
    let key = &args[0];
    if !is_simple(key) {
        return None;
    }
    let ast::Stmt::ExprStmt(put_stmt) = single_statement(if_stmt.then_branch()?)? else {
        return None;
    };
    let put = put_stmt.expr()?;
    let (put_map, put_args) = map_call(db, result, &put, "put", 2)?;
    if text(put_map.syntax()) != text(map.syntax())
        || text(put_args[0].syntax()) != text(key.syntax())
    {
        return None;
    }
    let value = &put_args[1];
    // `putIfAbsent` 把映射到 null 的键当作不存在，放 null 时意图就不清楚了
    if !is_simple(value) || value.syntax().text() == "null" {
        return None;
    }
    out.push(RedundancyFix {
        diagnostic: diagnostic(contains.syntax().text_range(), "putIfAbsent"),
        label: base_db::trs!("换成 `putIfAbsent()`", "Replace with `putIfAbsent()`"),
        edits: vec![(
            if_stmt.syntax().text_range(),
            format!(
                "{}.putIfAbsent({}, {});",
                map.syntax().text(),
                key.syntax().text(),
                value.syntax().text()
            ),
        )],
    });
    Some(())
}

/// 方法体里没有用到的 lambda 形参名。
fn fresh_name(body: &SyntaxNode) -> Option<&'static str> {
    let used: Vec<String> = body
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::IDENT)
        .map(|t| t.text().to_string())
        .collect();
    ["k", "key", "mapKey", "absentKey"]
        .into_iter()
        .find(|name| !used.iter().any(|u| u == name))
}

/// `V v = m.get(k); if (v == null) { v = new …; m.put(k, v); }`
fn compute_if_absent(
    db: &dyn Db,
    result: &InferenceResult,
    body: &SyntaxNode,
    if_stmt: &ast::IfStmt,
    text_all: &str,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if if_stmt.else_branch().is_some() {
        return None;
    }
    let decl = ast::LocalVarDecl::cast(if_stmt.syntax().prev_sibling()?)?;
    let mut declarators = decl.declarators();
    let (Some(declarator), None) = (declarators.next(), declarators.next()) else {
        return None;
    };
    let var = declarator.name()?.text().to_string();
    let (map, args) = map_call(db, result, &declarator.initializer()?, "get", 1)?;
    let key = &args[0];
    if !is_simple(key) {
        return None;
    }
    // `v == null`
    let ast::Expr::BinaryExpr(check) = unparen(if_stmt.condition()?) else {
        return None;
    };
    if check.op_kind() != Some(SyntaxKind::EQ_EQ) {
        return None;
    }
    let (lhs, rhs) = (check.lhs()?, check.rhs()?);
    let is_var =
        |e: &ast::Expr| matches!(e, ast::Expr::NameExpr(n) if n.syntax().text() == var.as_str());
    let is_null = |e: &ast::Expr| e.syntax().text() == "null";
    if !(is_var(&lhs) && is_null(&rhs) || is_null(&lhs) && is_var(&rhs)) {
        return None;
    }
    let ast::Stmt::Block(block) = if_stmt.then_branch()? else {
        return None;
    };
    let stmts: Vec<ast::Stmt> = block.statements().collect();
    let [ast::Stmt::ExprStmt(assign_stmt), ast::Stmt::ExprStmt(put_stmt)] = stmts.as_slice() else {
        return None;
    };
    let ast::Expr::AssignExpr(assign) = assign_stmt.expr()? else {
        return None;
    };
    if assign.op_token()?.kind() != SyntaxKind::EQ || !assign.lhs().is_some_and(|l| is_var(&l)) {
        return None;
    }
    let ast::Expr::NewExpr(value) = assign.rhs()? else {
        return None;
    };
    // 构造器实参只接受字面量（lambda 里不能引用会变的局部变量）
    if !value
        .arg_list()?
        .args()
        .all(|a| matches!(a, ast::Expr::Literal(_)))
        || value.body().is_some()
    {
        return None;
    }
    let (put_map, put_args) = map_call(db, result, &put_stmt.expr()?, "put", 2)?;
    if text(put_map.syntax()) != text(map.syntax())
        || text(put_args[0].syntax()) != text(key.syntax())
        || !is_var(&put_args[1])
    {
        return None;
    }
    let param = fresh_name(body)?;
    let init = declarator.initializer()?;
    out.push(RedundancyFix {
        diagnostic: diagnostic(check.syntax().text_range(), "computeIfAbsent"),
        label: base_db::trs!(
            "换成 `computeIfAbsent()`",
            "Replace with `computeIfAbsent()`"
        ),
        edits: vec![
            (
                init.syntax().text_range(),
                format!(
                    "{}.computeIfAbsent({}, {param} -> {})",
                    map.syntax().text(),
                    key.syntax().text(),
                    value.syntax().text()
                ),
            ),
            (
                statement_line_range(text_all, if_stmt.syntax().text_range()),
                String::new(),
            ),
        ],
    });
    Some(())
}

pub(crate) fn map_api(db: &dyn Db, file: SourceFile, out: &mut Vec<RedundancyFix>) {
    if crate::language_level(db, file).is_some_and(|l| l < 8) {
        return;
    }
    let text_all: &str = file.text(db);
    if !text_all.contains("containsKey") && !text_all.contains("null") {
        return;
    }
    for body in bodies_in_file(db, file) {
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for expr in node.descendants() {
            match expr.kind() {
                SyntaxKind::CONDITIONAL_EXPR => {
                    let Some(cond) = ast::ConditionalExpr::cast(expr) else {
                        continue;
                    };
                    if !cond.syntax().text().to_string().contains("containsKey") {
                        continue;
                    }
                    let result = result.get_or_insert_with(|| infer(db, body));
                    get_or_default(db, result, &cond, out);
                }
                SyntaxKind::IF_STMT => {
                    let Some(if_stmt) = ast::IfStmt::cast(expr) else {
                        continue;
                    };
                    let source = if_stmt.syntax().text().to_string();
                    if !source.contains(".put(") {
                        continue;
                    }
                    let result = result.get_or_insert_with(|| infer(db, body));
                    if source.contains("containsKey") {
                        put_if_absent(db, result, &if_stmt, out);
                    } else {
                        compute_if_absent(db, result, &node, &if_stmt, text_all, out);
                    }
                }
                _ => {}
            }
        }
    }
}
