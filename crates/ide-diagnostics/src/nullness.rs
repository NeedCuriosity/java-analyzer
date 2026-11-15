//! 局部变量的空值数据流分析。
//!
//! 沿着方法体跟踪每个局部变量 / 形参"一定是 null"还是"一定不是 null"，报两类问题：
//!
//! - 解引用一个此处一定为 null 的变量（`x.foo()`、`x.f`、`x[i]`）；
//! - `x == null` / `x != null` 的结果在此处是确定的（条件恒真或恒假）。
//!
//! 只跟踪局部变量：字段可能被任何调用修改。分析是保守的，拿不准的地方一律当作未知：
//! 循环、`try`、`switch`、带标签的语句在入口处丢掉内部赋值过的变量的信息，
//! 出口处只保留入口时就成立的事实。

use base_db::{Db, SourceFile};
use hir::{body_syntax, infer, InferenceResult, LocalId, LocalKind, Resolution, Ty};
use rustc_hash::FxHashMap;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{bodies_in_file, flow::has_break, Diagnostic, DiagnosticCode, Severity};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Nullness {
    Null,
    NotNull,
}

/// 变量 → 已知的空值状态；不在表里的是未知。
type State = FxHashMap<LocalId, Nullness>;

/// 两条路径汇合：只保留两边一致的事实。`None` 表示那条路径走不到。
fn join(a: Option<State>, b: Option<State>) -> Option<State> {
    match (a, b) {
        (None, other) | (other, None) => other,
        (Some(a), Some(b)) => Some(
            a.into_iter()
                .filter(|(id, nullness)| b.get(id) == Some(nullness))
                .collect(),
        ),
    }
}

struct Analysis<'a> {
    result: &'a InferenceResult,
    /// 当前位置的状态；`None` 表示当前位置不可达
    state: Option<State>,
    out: Vec<(TextRange, DiagnosticCode, String)>,
}

fn strip_parens(mut node: SyntaxNode) -> SyntaxNode {
    while node.kind() == SyntaxKind::PAREN_EXPR {
        match node.children().find(|c| ast::Expr::can_cast(c.kind())) {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

fn is_null_literal(node: &SyntaxNode) -> bool {
    let node = strip_parens(node.clone());
    node.kind() == SyntaxKind::LITERAL
        && node
            .first_token()
            .is_some_and(|t| t.kind() == SyntaxKind::NULL_KW)
}

fn operator(node: &SyntaxNode) -> Option<SyntaxKind> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .map(|t| t.kind())
        .find(|k| !k.is_trivia())
}

fn exprs(node: &SyntaxNode) -> Vec<SyntaxNode> {
    node.children()
        .filter(|c| ast::Expr::can_cast(c.kind()))
        .collect()
}

impl Analysis<'_> {
    /// 表达式是不是对一个被跟踪的局部变量的引用。
    fn local_of(&self, expr: &SyntaxNode) -> Option<(LocalId, TextRange)> {
        let expr = strip_parens(expr.clone());
        if expr.kind() != SyntaxKind::NAME_EXPR {
            return None;
        }
        let name = expr.children().find(|c| c.kind() == SyntaxKind::NAME_REF)?;
        let Some(Resolution::Local(id)) = self.result.resolution(name.text_range()) else {
            return None;
        };
        let local = self.result.local(*id);
        let tracked = !matches!(
            local.kind,
            LocalKind::LocalClassField | LocalKind::LocalClassMethod
        ) && !matches!(local.ty, Ty::Primitive(_) | Ty::Void);
        tracked.then_some((*id, name.text_range()))
    }

    fn get(&self, id: LocalId) -> Option<Nullness> {
        self.state.as_ref()?.get(&id).copied()
    }

    fn set(&mut self, id: LocalId, nullness: Option<Nullness>) {
        if let Some(state) = &mut self.state {
            match nullness {
                Some(n) => {
                    state.insert(id, n);
                }
                None => {
                    state.remove(&id);
                }
            }
        }
    }

    /// `node` 里赋值过的局部变量。
    fn assigned_in(&self, node: &SyntaxNode) -> Vec<LocalId> {
        let mut res = Vec::new();
        for n in node.descendants() {
            let target = match n.kind() {
                SyntaxKind::ASSIGN_EXPR | SyntaxKind::UNARY_EXPR | SyntaxKind::POSTFIX_EXPR => {
                    exprs(&n).into_iter().next()
                }
                _ => None,
            };
            if let Some((id, _)) = target.and_then(|t| self.local_of(&t)) {
                res.push(id);
            }
        }
        res
    }

    fn kill_assigned_in(&mut self, node: &SyntaxNode) {
        for id in self.assigned_in(node) {
            self.set(id, None);
        }
    }

    /// 表达式的值一定是 null / 一定不是 null。
    fn nullness_of(&self, expr: &SyntaxNode) -> Option<Nullness> {
        let expr = strip_parens(expr.clone());
        match expr.kind() {
            SyntaxKind::LITERAL => Some(if is_null_literal(&expr) {
                Nullness::Null
            } else {
                Nullness::NotNull
            }),
            SyntaxKind::NEW_EXPR
            | SyntaxKind::ARRAY_CREATION_EXPR
            | SyntaxKind::ARRAY_INITIALIZER
            | SyntaxKind::THIS_EXPR
            | SyntaxKind::LAMBDA_EXPR
            | SyntaxKind::METHOD_REF
            | SyntaxKind::BINARY_EXPR
            | SyntaxKind::CLASS_LITERAL => Some(Nullness::NotNull),
            SyntaxKind::NAME_EXPR => self.local_of(&expr).and_then(|(id, _)| self.get(id)),
            SyntaxKind::CAST_EXPR => exprs(&expr).last().and_then(|e| self.nullness_of(e)),
            SyntaxKind::ASSIGN_EXPR => exprs(&expr).get(1).and_then(|e| self.nullness_of(e)),
            SyntaxKind::CONDITIONAL_EXPR => {
                let parts = exprs(&expr);
                let then = self.nullness_of(parts.get(1)?)?;
                let otherwise = self.nullness_of(parts.get(2)?)?;
                (then == otherwise).then_some(then)
            }
            _ => None,
        }
    }

    /// `receiver` 在这里被解引用。
    fn dereference(&mut self, receiver: &SyntaxNode) {
        let Some((id, range)) = self.local_of(receiver) else {
            return;
        };
        if self.get(id) == Some(Nullness::Null) {
            let name = self.result.local(id).name.clone();
            self.out.push((
                range,
                DiagnosticCode::NullDereference,
                base_db::tr!(
                    "`{name}` 在这里一定是 null，解引用会抛出 NullPointerException",
                    "`{name}` is always null here; dereferencing it throws NullPointerException"
                ),
            ));
        }
        // 走到下一步说明没有抛异常
        self.set(id, Some(Nullness::NotNull));
    }

    /// 求值一个条件，返回它为真 / 为假时的状态。
    fn cond(&mut self, node: &SyntaxNode) -> (Option<State>, Option<State>) {
        let node = strip_parens(node.clone());
        if self.state.is_none() {
            return (None, None);
        }
        match node.kind() {
            SyntaxKind::LITERAL => {
                let token = node.first_token().map(|t| t.kind());
                match token {
                    Some(SyntaxKind::TRUE_KW) => return (self.state.clone(), None),
                    Some(SyntaxKind::FALSE_KW) => return (None, self.state.clone()),
                    _ => {}
                }
            }
            SyntaxKind::UNARY_EXPR if operator(&node) == Some(SyntaxKind::BANG) => {
                if let Some(operand) = exprs(&node).into_iter().next() {
                    let (t, f) = self.cond(&operand);
                    return (f, t);
                }
            }
            SyntaxKind::BINARY_EXPR => {
                let parts = exprs(&node);
                let op = ast::BinaryExpr::cast(node.clone()).and_then(|b| b.op_kind());
                if let (Some(op), [lhs, rhs]) = (op, parts.as_slice()) {
                    match op {
                        SyntaxKind::AMP_AMP => {
                            let (t1, f1) = self.cond(lhs);
                            self.state = t1;
                            let (t2, f2) = self.cond(rhs);
                            return (t2, join(f1, f2));
                        }
                        SyntaxKind::PIPE_PIPE => {
                            let (t1, f1) = self.cond(lhs);
                            self.state = f1;
                            let (t2, f2) = self.cond(rhs);
                            return (join(t1, t2), f2);
                        }
                        SyntaxKind::EQ_EQ | SyntaxKind::NEQ => {
                            let tested = if is_null_literal(rhs) {
                                Some(lhs)
                            } else if is_null_literal(lhs) {
                                Some(rhs)
                            } else {
                                None
                            };
                            if let Some(tested) = tested {
                                self.node(tested);
                                if let Some((id, _)) = self.local_of(tested) {
                                    let equals = op == SyntaxKind::EQ_EQ;
                                    if let Some(known) = self.get(id) {
                                        self.constant_condition(
                                            &node,
                                            id,
                                            known,
                                            (known == Nullness::Null) == equals,
                                        );
                                    }
                                    // 已知结果时，另一种情况走不到
                                    let known = self.get(id);
                                    let mut when_null = (known != Some(Nullness::NotNull))
                                        .then(|| self.state.clone())
                                        .flatten();
                                    let mut when_not_null = (known != Some(Nullness::Null))
                                        .then(|| self.state.clone())
                                        .flatten();
                                    if let Some(s) = &mut when_null {
                                        s.insert(id, Nullness::Null);
                                    }
                                    if let Some(s) = &mut when_not_null {
                                        s.insert(id, Nullness::NotNull);
                                    }
                                    return if equals {
                                        (when_null, when_not_null)
                                    } else {
                                        (when_not_null, when_null)
                                    };
                                }
                                return (self.state.clone(), self.state.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
            SyntaxKind::INSTANCEOF_EXPR => {
                if let Some(tested) = exprs(&node).into_iter().next() {
                    self.node(&tested);
                    let mut when_true = self.state.clone();
                    if let (Some((id, _)), Some(s)) = (self.local_of(&tested), &mut when_true) {
                        s.insert(id, Nullness::NotNull);
                    }
                    return (when_true, self.state.clone());
                }
            }
            _ => {}
        }
        // 这几种节点在 `node` 里会转回 `cond`，这里直接看它们的子节点
        match node.kind() {
            SyntaxKind::BINARY_EXPR | SyntaxKind::UNARY_EXPR | SyntaxKind::INSTANCEOF_EXPR => {
                self.children(&node)
            }
            _ => self.node(&node),
        }
        (self.state.clone(), self.state.clone())
    }

    fn constant_condition(&mut self, node: &SyntaxNode, id: LocalId, known: Nullness, value: bool) {
        // 断言里的检查是有意写的
        if node
            .ancestors()
            .any(|a| a.kind() == SyntaxKind::ASSERT_STMT)
        {
            return;
        }
        let name = self.result.local(id).name.clone();
        let fact = match known {
            Nullness::Null => base_db::trs!("一定是 null", "is always null"),
            Nullness::NotNull => base_db::trs!("一定不是 null", "is never null"),
        };
        self.out.push((
            node.text_range(),
            DiagnosticCode::ConstantCondition,
            base_db::tr!(
                "条件恒为 {value}：`{name}` 在这里{fact}",
                "Condition is always {value}: `{name}` {fact} here"
            ),
        ));
    }

    /// 分析 `node` 之后的状态；分析前后把当前状态恢复成 `entry`。
    fn branch(&mut self, node: Option<&SyntaxNode>, entry: Option<State>) -> Option<State> {
        self.state = entry;
        if let Some(node) = node {
            self.node(node);
        }
        self.state.take()
    }

    fn children(&mut self, node: &SyntaxNode) {
        for child in node.children() {
            self.node(&child);
        }
    }

    /// 在入口处丢掉 `node` 内部赋值过的变量；它的每个组成部分都从入口状态开始分析
    /// （各部分的执行顺序、是否执行都不建模），分析完之后回到入口状态。
    fn opaque(&mut self, node: &SyntaxNode) {
        self.kill_assigned_in(node);
        let entry = self.state.clone();
        for child in node.children() {
            self.branch(Some(&child), entry.clone());
        }
        self.state = entry;
    }

    fn node(&mut self, node: &SyntaxNode) {
        if self.state.is_none() {
            return;
        }
        match node.kind() {
            // 类体里是别的方法；lambda 体稍后执行，捕获的变量不会再变
            SyntaxKind::CLASS_BODY
            | SyntaxKind::CLASS_DECL
            | SyntaxKind::INTERFACE_DECL
            | SyntaxKind::ENUM_DECL
            | SyntaxKind::RECORD_DECL => {}
            SyntaxKind::LAMBDA_EXPR => {
                let entry = self.state.clone();
                self.children(node);
                self.state = entry;
            }

            // ---- 语句 ----
            SyntaxKind::RETURN_STMT
            | SyntaxKind::THROW_STMT
            | SyntaxKind::BREAK_STMT
            | SyntaxKind::CONTINUE_STMT
            | SyntaxKind::YIELD_STMT => {
                self.children(node);
                self.state = None;
            }
            SyntaxKind::IF_STMT => {
                let Some(if_stmt) = ast::IfStmt::cast(node.clone()) else {
                    return;
                };
                let (when_true, when_false) = match if_stmt.condition() {
                    Some(condition) => self.cond(condition.syntax()),
                    None => (self.state.clone(), self.state.clone()),
                };
                let then = if_stmt.then_branch();
                let otherwise = if_stmt.else_branch();
                let after_then = self.branch(then.as_ref().map(|s| s.syntax()), when_true);
                let after_else = self.branch(otherwise.as_ref().map(|s| s.syntax()), when_false);
                self.state = join(after_then, after_else);
            }
            SyntaxKind::WHILE_STMT => {
                self.kill_assigned_in(node);
                let condition = exprs(node).into_iter().next();
                let (when_true, when_false) = match &condition {
                    Some(condition) => self.cond(condition),
                    None => (self.state.clone(), self.state.clone()),
                };
                let entry = self.state.clone();
                let body = node.children().find(|c| ast::Stmt::can_cast(c.kind()));
                self.branch(body.as_ref(), when_true);
                // 有 break 时循环可能在任何地方结束，条件为假的事实不一定成立
                self.state = if has_break(node) { entry } else { when_false };
            }
            SyntaxKind::DO_WHILE_STMT
            | SyntaxKind::FOR_STMT
            | SyntaxKind::FOREACH_STMT
            | SyntaxKind::SWITCH_STMT
            | SyntaxKind::SWITCH_EXPR
            | SyntaxKind::LABELED_STMT
            | SyntaxKind::TRY_STMT
            | SyntaxKind::SWITCH_BLOCK
            // 断言可能被关掉，它的条件不能当作之后成立的事实
            | SyntaxKind::ASSERT_STMT => self.opaque(node),

            // ---- 表达式 ----
            SyntaxKind::VARIABLE_DECLARATOR => {
                let initializer = exprs(node).into_iter().next();
                if let Some(initializer) = &initializer {
                    self.node(initializer);
                }
                let declared = node
                    .children()
                    .find(|c| c.kind() == SyntaxKind::NAME)
                    .and_then(|name| self.result.local_at_decl(name.text_range()));
                if let (Some(id), Some(initializer)) = (declared, initializer) {
                    let local = self.result.local(id);
                    if !matches!(local.ty, Ty::Primitive(_)) {
                        let nullness = self.nullness_of(&initializer);
                        self.set(id, nullness);
                    }
                }
            }
            SyntaxKind::ASSIGN_EXPR => {
                let parts = exprs(node);
                let [lhs, rhs] = parts.as_slice() else {
                    self.children(node);
                    return;
                };
                match self.local_of(lhs) {
                    Some((id, _)) => {
                        self.node(rhs);
                        let simple = operator(node) == Some(SyntaxKind::EQ);
                        let nullness = if simple { self.nullness_of(rhs) } else { None };
                        self.set(id, nullness);
                    }
                    None => {
                        self.node(lhs);
                        self.node(rhs);
                    }
                }
            }
            SyntaxKind::UNARY_EXPR if operator(node) == Some(SyntaxKind::BANG) => {
                let (t, f) = self.cond(node);
                self.state = join(t, f);
            }
            SyntaxKind::UNARY_EXPR | SyntaxKind::POSTFIX_EXPR => {
                self.children(node);
                if let Some((id, _)) = exprs(node).first().and_then(|e| self.local_of(e)) {
                    self.set(id, None);
                }
            }
            SyntaxKind::BINARY_EXPR => {
                let op = ast::BinaryExpr::cast(node.clone()).and_then(|b| b.op_kind());
                if matches!(
                    op,
                    Some(
                        SyntaxKind::AMP_AMP
                            | SyntaxKind::PIPE_PIPE
                            | SyntaxKind::EQ_EQ
                            | SyntaxKind::NEQ
                    )
                ) {
                    let (t, f) = self.cond(node);
                    self.state = join(t, f);
                } else {
                    self.children(node);
                }
            }
            SyntaxKind::INSTANCEOF_EXPR => {
                let (t, f) = self.cond(node);
                self.state = join(t, f);
            }
            SyntaxKind::CONDITIONAL_EXPR => {
                let parts = exprs(node);
                let [condition, then, otherwise] = parts.as_slice() else {
                    self.children(node);
                    return;
                };
                let (when_true, when_false) = self.cond(condition);
                let after_then = self.branch(Some(then), when_true);
                let after_else = self.branch(Some(otherwise), when_false);
                self.state = join(after_then, after_else);
            }
            SyntaxKind::METHOD_CALL => {
                let Some(call) = ast::MethodCall::cast(node.clone()) else {
                    return;
                };
                if let Some(receiver) = call.receiver() {
                    self.node(receiver.syntax());
                    self.dereference(receiver.syntax());
                }
                let args: Vec<ast::Expr> = call
                    .arg_list()
                    .map(|l| l.args().collect())
                    .unwrap_or_default();
                for arg in &args {
                    self.node(arg.syntax());
                }
                // `Objects.requireNonNull(x)` 之后 x 一定不是 null
                let requires_non_null = call
                    .name_ref()
                    .is_some_and(|n| n.text() == "requireNonNull")
                    && call
                        .receiver()
                        .is_some_and(|r| r.syntax().text() == "Objects");
                if requires_non_null {
                    if let Some((id, _)) = args.first().and_then(|a| self.local_of(a.syntax())) {
                        self.set(id, Some(Nullness::NotNull));
                    }
                }
            }
            SyntaxKind::FIELD_ACCESS | SyntaxKind::ARRAY_ACCESS => {
                let parts = exprs(node);
                if let Some(receiver) = parts.first() {
                    self.node(receiver);
                    self.dereference(receiver);
                }
                for rest in parts.iter().skip(1) {
                    self.node(rest);
                }
            }
            _ => self.children(node),
        }
    }
}

fn analyze(
    node: &SyntaxNode,
    result: &InferenceResult,
) -> Vec<(TextRange, DiagnosticCode, String)> {
    let mut analysis = Analysis {
        result,
        state: Some(State::default()),
        out: Vec::new(),
    };
    analysis.children(node);
    analysis.out
}

pub(crate) fn nullness_diagnostics(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    for body in bodies_in_file(db, file) {
        db.unwind_if_revision_cancelled();
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let result = infer(db, body);
        for (range, code, message) in analyze(&node, &result) {
            out.push(Diagnostic {
                range,
                message,
                severity: Severity::Warning,
                code,
                unused: false,
            });
        }
    }
}
