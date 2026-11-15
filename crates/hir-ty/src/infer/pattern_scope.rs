//! `instanceof` 模式变量的作用域（JLS §6.3.1）。
//!
//! 模式变量只在"匹配一定成功"的地方有效，这取决于控制流：
//!
//! - `a && b`：`a` 为真时引入的变量在 `b` 里有效；`a || b`：`a` 为假时引入的在 `b` 里有效；
//! - `a ? b : c`：`a` 为真时引入的在 `b` 里，为假时引入的在 `c` 里；
//! - `if (e) S else T`：`e` 为真时引入的在 `S` 里，为假时引入的在 `T` 里；某个分支不能正常结束时，
//!   另一种情况引入的变量在 `if` 之后的语句里也有效（`if (!(o instanceof String s)) return;`）；
//! - `while (e) S` / `for (..; e; u) S`：`e` 为真时引入的在循环体（和 `u`）里，循环里没有跳出它的
//!   `break` 时，`e` 为假时引入的在循环之后有效；`do S while (e)` 同理。
//!
//! 这里纯按语法算出每个模式变量有效的文本范围。拿不准的地方（语句能不能正常结束说不清）
//! 一律按"有效"处理：宁可漏报，不可误报。

use rustc_hash::FxHashMap;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

/// 模式变量（以声明处名字的范围为键）→ 它有效的文本范围。不在表里的模式变量不受限制。
pub(super) type PatternScopes = FxHashMap<TextRange, Vec<TextRange>>;

pub(super) fn pattern_scopes(body: &SyntaxNode) -> PatternScopes {
    let mut scopes = PatternScopes::default();
    for instanceof in body.descendants().filter_map(ast::InstanceofExpr::cast) {
        // `case ... when o instanceof String s ->` 的守卫不在这里建模
        if instanceof
            .syntax()
            .ancestors()
            .any(|n| n.kind() == SyntaxKind::GUARD)
        {
            continue;
        }
        if let Some(pattern) = instanceof.pattern() {
            for name in pattern_names(&pattern) {
                scopes.entry(name).or_default();
            }
        }
    }
    if scopes.is_empty() {
        return scopes;
    }
    let mut add = |names: Vec<TextRange>, region: TextRange| {
        for name in names {
            if let Some(regions) = scopes.get_mut(&name) {
                regions.push(region);
            }
        }
    };
    for node in body.descendants() {
        match node.kind() {
            SyntaxKind::BINARY_EXPR => {
                let Some(bin) = ast::BinaryExpr::cast(node) else {
                    continue;
                };
                let (Some(lhs), Some(rhs)) = (bin.lhs(), bin.rhs()) else {
                    continue;
                };
                match bin.op_kind() {
                    Some(SyntaxKind::AMP_AMP) => add(introduced(&lhs, true), range_of(&rhs)),
                    Some(SyntaxKind::PIPE_PIPE) => add(introduced(&lhs, false), range_of(&rhs)),
                    _ => {}
                }
            }
            SyntaxKind::CONDITIONAL_EXPR => {
                let Some(cond_expr) = ast::ConditionalExpr::cast(node) else {
                    continue;
                };
                let Some(cond) = cond_expr.condition() else {
                    continue;
                };
                if let Some(then) = cond_expr.then_expr() {
                    add(introduced(&cond, true), range_of(&then));
                }
                if let Some(otherwise) = cond_expr.else_expr() {
                    add(introduced(&cond, false), range_of(&otherwise));
                }
            }
            SyntaxKind::IF_STMT => {
                let Some(if_stmt) = ast::IfStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(cond) = if_stmt.condition() else {
                    continue;
                };
                let then = if_stmt.then_branch();
                let otherwise = if_stmt.else_branch();
                if let Some(then) = &then {
                    add(introduced(&cond, true), range_of(then));
                }
                if let Some(otherwise) = &otherwise {
                    add(introduced(&cond, false), range_of(otherwise));
                }
                let then_completes = then.as_ref().map_or(Some(true), can_complete_normally);
                let after = rest_after(&node);
                match &otherwise {
                    None => {
                        if then_completes != Some(true) {
                            add(introduced(&cond, false), after);
                        }
                    }
                    Some(otherwise) => {
                        let else_completes = can_complete_normally(otherwise);
                        if then_completes != Some(false) && else_completes != Some(true) {
                            add(introduced(&cond, true), after);
                        }
                        if else_completes != Some(false) && then_completes != Some(true) {
                            add(introduced(&cond, false), after);
                        }
                    }
                }
            }
            SyntaxKind::WHILE_STMT => {
                let Some(stmt) = ast::WhileStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(cond) = stmt.condition() else {
                    continue;
                };
                if let Some(body) = stmt.body() {
                    add(introduced(&cond, true), range_of(&body));
                }
                if !has_break_to(&node) {
                    add(introduced(&cond, false), rest_after(&node));
                }
            }
            SyntaxKind::DO_WHILE_STMT => {
                let Some(stmt) = ast::DoWhileStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(cond) = stmt.condition() else {
                    continue;
                };
                if !has_break_to(&node) {
                    add(introduced(&cond, false), rest_after(&node));
                }
            }
            SyntaxKind::FOR_STMT => {
                let Some(stmt) = ast::ForStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(cond) = stmt.condition() else {
                    continue;
                };
                if let Some(update) = stmt.update() {
                    add(introduced(&cond, true), update.syntax().text_range());
                }
                if let Some(body) = stmt.body() {
                    add(introduced(&cond, true), range_of(&body));
                }
                if !has_break_to(&node) {
                    add(introduced(&cond, false), rest_after(&node));
                }
            }
            _ => {}
        }
    }
    scopes
}

fn range_of(node: &impl AstNode) -> TextRange {
    node.syntax().text_range()
}

/// 模式里声明的全部变量名的范围。
fn pattern_names(pattern: &ast::Pattern) -> Vec<TextRange> {
    match pattern {
        ast::Pattern::TypePattern(it) => it
            .name()
            .filter(|name| name.text() != "_")
            .map(|name| name.syntax().text_range())
            .into_iter()
            .collect(),
        ast::Pattern::RecordPattern(it) => it.patterns().flat_map(|p| pattern_names(&p)).collect(),
        ast::Pattern::UnnamedPattern(_) => Vec::new(),
    }
}

/// 表达式为真（`when_true`）或为假时引入的模式变量。
fn introduced(expr: &ast::Expr, when_true: bool) -> Vec<TextRange> {
    match expr {
        ast::Expr::InstanceofExpr(it) if when_true => {
            it.pattern().map(|p| pattern_names(&p)).unwrap_or_default()
        }
        ast::Expr::ParenExpr(it) => it
            .expr()
            .map(|inner| introduced(&inner, when_true))
            .unwrap_or_default(),
        ast::Expr::UnaryExpr(it) if it.op_token().is_some_and(|t| t.kind() == SyntaxKind::BANG) => {
            it.operand()
                .map(|inner| introduced(&inner, !when_true))
                .unwrap_or_default()
        }
        ast::Expr::BinaryExpr(it) => {
            let combines = match it.op_kind() {
                Some(SyntaxKind::AMP_AMP) => when_true,
                Some(SyntaxKind::PIPE_PIPE) => !when_true,
                _ => false,
            };
            if !combines {
                return Vec::new();
            }
            let mut res = it
                .lhs()
                .map(|e| introduced(&e, when_true))
                .unwrap_or_default();
            res.extend(
                it.rhs()
                    .map(|e| introduced(&e, when_true))
                    .unwrap_or_default(),
            );
            res
        }
        _ => Vec::new(),
    }
}

/// 语句之后、同一个块（或 `switch` 分支）结束之前的范围。
fn rest_after(stmt: &SyntaxNode) -> TextRange {
    let end = stmt.text_range().end();
    let container = stmt
        .ancestors()
        .skip(1)
        .find(|n| matches!(n.kind(), SyntaxKind::BLOCK | SyntaxKind::SWITCH_CASE));
    match container {
        Some(container) => TextRange::new(end, container.text_range().end()),
        None => TextRange::empty(end),
    }
}

/// 语句能否正常结束（JLS §14.22）：`Some(false)` 表示一定不能（`return`、`throw` …），
/// `None` 表示这里判断不了。
fn can_complete_normally(stmt: &ast::Stmt) -> Option<bool> {
    match stmt {
        ast::Stmt::ReturnStmt(_)
        | ast::Stmt::ThrowStmt(_)
        | ast::Stmt::BreakStmt(_)
        | ast::Stmt::ContinueStmt(_)
        | ast::Stmt::YieldStmt(_) => Some(false),
        ast::Stmt::Block(block) => {
            let mut res = Some(true);
            for stmt in block.statements() {
                match can_complete_normally(&stmt) {
                    Some(false) => return Some(false),
                    None => res = None,
                    Some(true) => {}
                }
            }
            res
        }
        ast::Stmt::IfStmt(it) => match it.else_branch() {
            None => Some(true),
            Some(otherwise) => {
                let then = it
                    .then_branch()
                    .map_or(Some(true), |t| can_complete_normally(&t));
                match (then, can_complete_normally(&otherwise)) {
                    (Some(false), Some(false)) => Some(false),
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    _ => None,
                }
            }
        },
        ast::Stmt::SynchronizedStmt(it) => it
            .syntax()
            .children()
            .find_map(ast::Block::cast)
            .and_then(|b| can_complete_normally(&ast::Stmt::Block(b))),
        ast::Stmt::WhileStmt(it) => {
            let infinite = it.condition().is_some_and(|c| is_true_literal(&c));
            (infinite && !has_break_to(it.syntax())).then_some(false)
        }
        ast::Stmt::ForStmt(it) => {
            let infinite = it.condition().is_none_or(|c| is_true_literal(&c));
            (infinite && !has_break_to(it.syntax())).then_some(false)
        }
        ast::Stmt::LocalVarDecl(_)
        | ast::Stmt::ExprStmt(_)
        | ast::Stmt::EmptyStmt(_)
        | ast::Stmt::AssertStmt(_)
        | ast::Stmt::ForeachStmt(_)
        | ast::Stmt::ClassDecl(_)
        | ast::Stmt::InterfaceDecl(_)
        | ast::Stmt::EnumDecl(_)
        | ast::Stmt::RecordDecl(_) => Some(true),
        _ => None,
    }
}

fn is_true_literal(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Literal(lit) => lit.syntax().text() == "true",
        ast::Expr::ParenExpr(it) => it.expr().is_some_and(|e| is_true_literal(&e)),
        _ => false,
    }
}

/// 循环里有没有跳出它的 `break`：不带标签、且不在内层循环 / `switch` 里的，
/// 或者带着这个循环的标签的。lambda 和类体里的不算。
fn has_break_to(loop_stmt: &SyntaxNode) -> bool {
    let label = loop_stmt
        .parent()
        .and_then(ast::LabeledStmt::cast)
        .and_then(|l| l.label())
        .map(|l| l.text());
    fn walk(node: &SyntaxNode, label: Option<&str>, nested: bool) -> bool {
        for child in node.children() {
            match child.kind() {
                SyntaxKind::BREAK_STMT => {
                    let target = ast::BreakStmt::cast(child.clone())
                        .and_then(|b| b.label())
                        .map(|l| l.text());
                    let hits = match &target {
                        None => !nested,
                        Some(target) => label == Some(target.as_str()),
                    };
                    if hits {
                        return true;
                    }
                }
                SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY => {}
                SyntaxKind::WHILE_STMT
                | SyntaxKind::DO_WHILE_STMT
                | SyntaxKind::FOR_STMT
                | SyntaxKind::FOREACH_STMT
                | SyntaxKind::SWITCH_STMT
                | SyntaxKind::SWITCH_EXPR => {
                    if walk(&child, label, true) {
                        return true;
                    }
                }
                _ => {
                    if walk(&child, label, nested) {
                        return true;
                    }
                }
            }
        }
        false
    }
    walk(loop_stmt, label.as_deref(), false)
}

/// 偏移 `offset` 处的引用能否看到模式变量 `decl`。
pub(super) fn in_scope(scopes: &PatternScopes, decl: TextRange, offset: TextSize) -> bool {
    match scopes.get(&decl) {
        Some(regions) => regions.iter().any(|r| r.contains(offset)),
        None => true,
    }
}
