//! 局部变量的空值数据流分析。
//!
//! 沿着方法体跟踪每个局部变量 / 形参"一定是 null"还是"一定不是 null"，报两类问题：
//!
//! - 解引用一个此处一定为 null 的变量（`x.foo()`、`x.f`、`x[i]`）；
//! - `x == null` / `x != null` 的结果在此处是确定的（条件恒真或恒假）。
//!
//! 空值注解（`@Nullable` / `@NotNull` 等）参与分析：标了 `@Nullable` 的方法返回值、字段和形参
//! "可能是 null"，解引用它们（没检查过时）报警告；把 null 传给 `@NotNull` 形参、
//! 从 `@NotNull` 方法返回 null 也报警告。
//!
//! 只跟踪局部变量：字段可能被任何调用修改。分析是保守的，拿不准的地方一律当作未知：
//! 循环、`try`、`switch`、带标签的语句在入口处丢掉内部赋值过的变量的信息，
//! 出口处只保留入口时就成立的事实。

use base_db::{Db, SourceFile};
use hir::{
    body_syntax, class_data, infer, BodyId, InferenceResult, LocalId, LocalKind, Modifiers,
    NullAnnotation, Resolution, Ty,
};
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
    /// 来自 `@Nullable`：可能是 null，用之前应当检查
    MaybeNull,
}

/// 变量 → 已知的空值状态；不在表里的是未知。
type State = FxHashMap<LocalId, Nullness>;

/// 两条路径汇合：只保留两边一致的事实。`None` 表示那条路径走不到。
fn join(a: Option<State>, b: Option<State>) -> Option<State> {
    match (a, b) {
        (None, other) | (other, None) => other,
        (Some(a), Some(b)) => Some(
            a.into_iter()
                .filter_map(|(id, x)| {
                    let y = *b.get(&id)?;
                    if x == y {
                        Some((id, x))
                    } else if x == Nullness::MaybeNull || y == Nullness::MaybeNull {
                        // 某条路径上可能是 null，汇合后仍然可能是 null
                        Some((id, Nullness::MaybeNull))
                    } else {
                        None
                    }
                })
                .collect(),
        ),
    }
}

struct Analysis<'a> {
    db: &'a dyn Db,
    result: &'a InferenceResult,
    /// 方法体的源码文本，用来判断某个调用有没有被判空过
    text: String,
    /// 所在方法标了 `@NotNull`
    returns_not_null: bool,
    /// 当前在几层 lambda 里面（lambda 里的 `return` 不是方法的返回）
    lambda_depth: u32,
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
            SyntaxKind::NAME_EXPR => match self.local_of(&expr) {
                Some((id, _)) => self.get(id),
                None => self.annotated_maybe_null(&expr),
            },
            SyntaxKind::METHOD_CALL | SyntaxKind::FIELD_ACCESS => self.annotated_maybe_null(&expr),
            SyntaxKind::CAST_EXPR => exprs(&expr).last().and_then(|e| self.nullness_of(e)),
            SyntaxKind::ASSIGN_EXPR => exprs(&expr).get(1).and_then(|e| self.nullness_of(e)),
            SyntaxKind::CONDITIONAL_EXPR => {
                let parts = exprs(&expr);
                let then = self.nullness_of(parts.get(1)?)?;
                let otherwise = self.nullness_of(parts.get(2)?)?;
                if then == otherwise {
                    Some(then)
                } else if then == Nullness::MaybeNull || otherwise == Nullness::MaybeNull {
                    Some(Nullness::MaybeNull)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// 方法调用 / 字段引用的目标上的空值注解。
    fn annotated(&self, expr: &SyntaxNode) -> Option<Nullness> {
        let name = match expr.kind() {
            SyntaxKind::METHOD_CALL => ast::MethodCall::cast(expr.clone())?
                .name_ref()?
                .syntax()
                .clone(),
            SyntaxKind::FIELD_ACCESS | SyntaxKind::NAME_EXPR => expr
                .children()
                .filter(|c| c.kind() == SyntaxKind::NAME_REF)
                .last()?,
            _ => return None,
        };
        let flags = match self.result.resolution(name.text_range())? {
            Resolution::Method(m) => {
                class_data(self.db, m.class)?
                    .methods
                    .get(m.idx as usize)?
                    .flags
            }
            Resolution::Field(f) => {
                class_data(self.db, f.class)?
                    .fields
                    .get(f.idx as usize)?
                    .flags
            }
            _ => return None,
        };
        match flags.null_annotation() {
            NullAnnotation::Nullable => Some(Nullness::MaybeNull),
            NullAnnotation::NotNull => Some(Nullness::NotNull),
            NullAnnotation::Unknown => None,
        }
    }

    /// 标了 `@Nullable` 的字段 / 方法调用，而且方法体里没有检查过它（字段不跟踪，
    /// 检查过的就不当作可能为 null）。
    fn annotated_maybe_null(&self, expr: &SyntaxNode) -> Option<Nullness> {
        self.annotated(expr)
            .filter(|n| *n == Nullness::MaybeNull && !self.null_checked_text(expr))
    }

    /// 方法体里有没有拿 `expr` 的文本和 null 比较过（`getX() != null`、`x instanceof T` 之类）。
    fn null_checked_text(&self, expr: &SyntaxNode) -> bool {
        let text = expr.text().to_string();
        [
            format!("{text} != null"),
            format!("{text} == null"),
            format!("null != {text}"),
            format!("null == {text}"),
            format!("requireNonNull({text}"),
            format!("{text} instanceof"),
        ]
        .iter()
        .any(|p| self.text.contains(p.as_str()))
    }

    /// 这些实参里可能为 null 的变量改为未知。
    fn forget_maybe_null(&mut self, args: &[ast::Expr]) {
        for arg in args {
            if let Some((id, _)) = self.local_of(arg.syntax()) {
                if self.get(id) == Some(Nullness::MaybeNull) {
                    self.set(id, None);
                }
            }
        }
    }

    /// `receiver` 在这里被解引用。
    fn dereference(&mut self, receiver: &SyntaxNode) {
        let Some((id, range)) = self.local_of(receiver) else {
            // 直接解引用 `@Nullable` 方法的返回值：`getParent().getName()`
            let receiver = strip_parens(receiver.clone());
            if receiver.kind() == SyntaxKind::METHOD_CALL
                && self.annotated(&receiver) == Some(Nullness::MaybeNull)
                && !self.null_checked_text(&receiver)
            {
                if let Some(name) =
                    ast::MethodCall::cast(receiver.clone()).and_then(|c| c.name_ref())
                {
                    let method = name.text();
                    self.out.push((
                        name.syntax().text_range(),
                        DiagnosticCode::NullableDereference,
                        base_db::tr!(
                            "`{method}` 标注了 @Nullable，返回值可能是 null，直接解引用可能抛出 NullPointerException",
                            "`{method}` is annotated @Nullable; dereferencing its result may throw NullPointerException"
                        ),
                    ));
                }
            }
            return;
        };
        let name = self.result.local(id).name.clone();
        match self.get(id) {
            Some(Nullness::Null) => self.out.push((
                range,
                DiagnosticCode::NullDereference,
                base_db::tr!(
                    "`{name}` 在这里一定是 null，解引用会抛出 NullPointerException",
                    "`{name}` is always null here; dereferencing it throws NullPointerException"
                ),
            )),
            Some(Nullness::MaybeNull) => self.out.push((
                range,
                DiagnosticCode::NullableDereference,
                base_db::tr!(
                    "`{name}` 在这里可能是 null，解引用可能抛出 NullPointerException",
                    "`{name}` may be null here; dereferencing it may throw NullPointerException"
                ),
            )),
            _ => {}
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
                                // `(x = next()) != null` 检查的是赋值之后的 x
                                let tested_local = self.local_of(tested).or_else(|| {
                                    let assign = strip_parens(tested.clone());
                                    (assign.kind() == SyntaxKind::ASSIGN_EXPR
                                        && operator(&assign) == Some(SyntaxKind::EQ))
                                    .then(|| exprs(&assign).into_iter().next())
                                    .flatten()
                                    .and_then(|lhs| self.local_of(&lhs))
                                });
                                if let Some((id, _)) = tested_local {
                                    let equals = op == SyntaxKind::EQ_EQ;
                                    if let Some(known) =
                                        self.get(id).filter(|n| *n != Nullness::MaybeNull)
                                    {
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
        // 条件里把可能为 null 的变量交给某个方法判断（`hasText(s)`、`isEmpty(s)`、`nonNull(s)`），
        // 两个分支上都不再认为它可能是 null
        if node.kind() == SyntaxKind::METHOD_CALL {
            self.node(&node);
            let args: Vec<ast::Expr> = ast::MethodCall::cast(node.clone())
                .and_then(|c| c.arg_list())
                .map(|l| l.args().collect())
                .unwrap_or_default();
            self.forget_maybe_null(&args);
            return (self.state.clone(), self.state.clone());
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
            Nullness::NotNull | Nullness::MaybeNull => {
                base_db::trs!("一定不是 null", "is never null")
            }
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
                self.lambda_depth += 1;
                self.children(node);
                self.lambda_depth -= 1;
                self.state = entry;
            }
            SyntaxKind::RETURN_STMT if self.returns_not_null && self.lambda_depth == 0 => {
                self.children(node);
                if let Some(value) = exprs(node).first() {
                    self.check_not_null(
                        value,
                        base_db::trs!(
                            "方法标注了 @NotNull，却返回了 null",
                            "`null` is returned by a method annotated @NotNull"
                        )
                        .to_string(),
                        |value| {
                            base_db::tr!(
                                "`{value}` 可能是 null，但方法标注了 @NotNull",
                                "`{value}` may be null but the method is annotated @NotNull"
                            )
                        },
                    );
                }
                self.state = None;
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
                // `Objects.requireNonNull(x)`、`Preconditions.checkNotNull(x)`、`Assert.notNull(x, ..)`
                // 之后 x 一定不是 null
                let method_name = call.name_ref().map(|n| n.text()).unwrap_or_default();
                let receiver_text = call
                    .receiver()
                    .map(|r| r.syntax().text().to_string())
                    .unwrap_or_default();
                let requires_non_null = match method_name.as_str() {
                    "requireNonNull" => receiver_text.ends_with("Objects") || receiver_text.is_empty(),
                    "checkNotNull" => true,
                    "notNull" => receiver_text.ends_with("Assert") || receiver_text.ends_with("Validate"),
                    _ => false,
                };
                // 断言类方法（`assertNotNull(x)` 等）：不再认为 x 可能是 null
                if method_name.starts_with("assert") {
                    self.forget_maybe_null(&args);
                }
                if let Some(list) = call.arg_list() {
                    self.check_args(list.syntax());
                }
                if requires_non_null {
                    if let Some((id, _)) = args.first().and_then(|a| self.local_of(a.syntax())) {
                        self.set(id, Some(Nullness::NotNull));
                    }
                }
            }
            SyntaxKind::NEW_EXPR => {
                self.children(node);
                if let Some(list) = node.children().find(|c| c.kind() == SyntaxKind::ARG_LIST) {
                    self.check_args(&list);
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

impl Analysis<'_> {
    /// `value` 流向一个 `@NotNull` 的位置：一定是 null 时报 `when_null`，可能是 null 时报 `when_maybe`。
    fn check_not_null(
        &mut self,
        value: &SyntaxNode,
        when_null: String,
        when_maybe: impl FnOnce(String) -> String,
    ) {
        let message = match self.nullness_of(value) {
            Some(Nullness::Null) => when_null,
            Some(Nullness::MaybeNull) => when_maybe(value.text().to_string()),
            _ => return,
        };
        self.out.push((
            value.text_range(),
            DiagnosticCode::NullabilityViolation,
            message,
        ));
    }

    /// 实参流向 `@NotNull` 形参。
    fn check_args(&mut self, arg_list: &SyntaxNode) {
        if self.state.is_none() {
            return;
        }
        let Some(method) = self
            .result
            .call_info(arg_list.text_range())
            .and_then(|c| c.chosen)
        else {
            return;
        };
        let Some(data) = class_data(self.db, method.class) else {
            return;
        };
        let Some(decl) = data.methods.get(method.idx as usize) else {
            return;
        };
        if decl
            .params
            .iter()
            .all(|p| p.nullness != NullAnnotation::NotNull)
        {
            return;
        }
        let varargs = decl.flags.contains(Modifiers::VARARGS);
        let args = exprs(arg_list);
        for (i, (arg, param)) in args.iter().zip(&decl.params).enumerate() {
            // 可变参数位置上的实参不一定对应那个数组形参
            if varargs && i + 1 == decl.params.len() {
                break;
            }
            if param.nullness != NullAnnotation::NotNull {
                continue;
            }
            let param_name = param
                .name
                .as_ref()
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("#{}", i + 1));
            self.check_not_null(
                arg,
                base_db::tr!(
                    "形参 `{param_name}` 标注了 @NotNull，却传入了 null",
                    "Passing `null` to parameter `{param_name}` annotated @NotNull"
                ),
                |value| {
                    base_db::tr!(
                        "`{value}` 可能是 null，但形参 `{param_name}` 标注了 @NotNull",
                        "`{value}` may be null but parameter `{param_name}` is annotated @NotNull"
                    )
                },
            );
        }
    }
}

fn analyze(
    db: &dyn Db,
    body: BodyId,
    node: &SyntaxNode,
    result: &InferenceResult,
) -> Vec<(TextRange, DiagnosticCode, String)> {
    let mut analysis = Analysis {
        db,
        result,
        text: node.text().to_string(),
        returns_not_null: false,
        lambda_depth: 0,
        state: Some(State::default()),
        out: Vec::new(),
    };
    if let BodyId::Method(method) = body {
        let flags = class_data(db, method.class)
            .and_then(|d| d.methods.get(method.idx as usize).map(|m| m.flags));
        analysis.returns_not_null =
            flags.is_some_and(|f| f.null_annotation() == NullAnnotation::NotNull);
        // `@Nullable` 形参在入口处可能是 null
        let params = node
            .children()
            .find(|c| c.kind() == SyntaxKind::PARAM_LIST)
            .into_iter()
            .flat_map(|list| list.children().filter_map(ast::Param::cast));
        for param in params {
            if hir::param_null_annotation(&param) != NullAnnotation::Nullable {
                continue;
            }
            let id = param
                .name()
                .and_then(|n| result.local_at_decl(n.syntax().text_range()));
            if let Some(id) = id {
                analysis.set(id, Some(Nullness::MaybeNull));
            }
        }
    }
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
        for (range, code, message) in analyze(db, body, &node, &result) {
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
