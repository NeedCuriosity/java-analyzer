//! 语句级的流分析（JLS §14.22 的保守子集）和几个不依赖数据流的检查项。
//!
//! - 不可达的语句：同一个块里跟在 `return` / `throw` / `break` / `continue` / `yield`
//!   或不会结束的循环之后的语句；
//! - 缺少 `return`：返回值不是 `void` 的方法，方法体可以正常执行到末尾；
//! - 用 `==` / `!=` 比较字符串、自我赋值、自我比较。
//!
//! 分析只在没有语法错误的文件上进行（残缺的树会导致误报）。遇到 `switch` 这类这里不精确建模的
//! 结构时，结论标记为"不确定"，此时不报告缺少 `return`。

use base_db::Db;
use rustc_hash::FxHashSet;
use syntax::{
    ast::{self, AstNode},
    SmolStr, SyntaxKind, SyntaxNode, TextRange,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

struct Flow<'a> {
    out: &'a mut Vec<Diagnostic>,
    /// 遇到了不精确建模的结构
    uncertain: bool,
}

fn is_true_literal(expr: Option<ast::Expr>) -> bool {
    match expr {
        Some(ast::Expr::Literal(literal)) => literal.kind() == Some(SyntaxKind::TRUE_KW),
        Some(ast::Expr::ParenExpr(paren)) => is_true_literal(paren.expr()),
        _ => false,
    }
}

const LOOPS: &[SyntaxKind] = &[
    SyntaxKind::WHILE_STMT,
    SyntaxKind::DO_WHILE_STMT,
    SyntaxKind::FOR_STMT,
    SyntaxKind::FOREACH_STMT,
];

/// `body` 里有没有能跳出 `body` 所属循环的 `break`。
/// 带标签的 `break` 一律算作能跳出（保守）。
pub(crate) fn has_break(body: &SyntaxNode) -> bool {
    fn walk(node: &SyntaxNode, nested: bool) -> bool {
        for child in node.children() {
            let kind = child.kind();
            if kind == SyntaxKind::BREAK_STMT {
                let labeled =
                    ast::BreakStmt::cast(child.clone()).is_some_and(|b| b.label().is_some());
                if labeled || !nested {
                    return true;
                }
                continue;
            }
            if matches!(kind, SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY) {
                continue;
            }
            let inner_target = LOOPS.contains(&kind)
                || matches!(kind, SyntaxKind::SWITCH_STMT | SyntaxKind::SWITCH_EXPR);
            if walk(&child, nested || inner_target) {
                return true;
            }
        }
        false
    }
    walk(body, false)
}

/// 直接标在 `stmt` 上的标签（`a: b: while (..)` 里的 `a` 和 `b`）。
fn own_labels(stmt: &SyntaxNode) -> Vec<SmolStr> {
    let mut labels = Vec::new();
    let mut node = stmt.clone();
    while let Some(parent) = node.parent().and_then(ast::LabeledStmt::cast) {
        if let Some(label) = parent.label() {
            labels.push(label.text());
        }
        node = parent.syntax().clone();
    }
    labels
}

/// 块能否正常结束（JLS §14.22）。`Some(false)` 是确定的；能结束、但途中遇到了不精确建模的
/// 结构时为 `None`。
pub(crate) fn block_completes(block: &ast::Block) -> Option<bool> {
    let mut scratch = Vec::new();
    let mut flow = Flow {
        out: &mut scratch,
        uncertain: false,
    };
    match flow.block(block) {
        false => Some(false),
        true if !flow.uncertain => Some(true),
        true => None,
    }
}

impl Flow<'_> {
    /// `body` 里有没有以 `body` 所属语句为目标、并且确实会跳出去的 `break`（`continues` 为真时
    /// 看 `continue`）：不带标签且不在内层循环 / switch 里的，或者标签是 `labels` 之一的。
    /// `labeled_only` 时不带标签的不算（带标签的普通语句只能用 `break 标签` 跳出）。
    fn jumps_to(
        &mut self,
        body: &SyntaxNode,
        labels: &[SmolStr],
        continues: bool,
        labeled_only: bool,
    ) -> bool {
        let jump_kind = if continues {
            SyntaxKind::CONTINUE_STMT
        } else {
            SyntaxKind::BREAK_STMT
        };
        let mut jumps = Vec::new();
        let mut walk = body.preorder();
        while let Some(event) = walk.next() {
            let syntax::WalkEvent::Enter(node) = event else {
                continue;
            };
            let kind = node.kind();
            if matches!(kind, SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY) {
                walk.skip_subtree();
                continue;
            }
            if kind != jump_kind {
                continue;
            }
            let label = if continues {
                ast::ContinueStmt::cast(node.clone()).and_then(|c| c.label())
            } else {
                ast::BreakStmt::cast(node.clone()).and_then(|b| b.label())
            };
            let targets_body = match label {
                Some(label) => labels.iter().any(|l| *l == label.text()),
                // 不带标签的跳转作用于最近的循环（`break` 还有 switch）
                None => {
                    !labeled_only
                        && !node.ancestors().skip(1).take_while(|a| a != body).any(|a| {
                            LOOPS.contains(&a.kind())
                                || (!continues
                                    && matches!(
                                        a.kind(),
                                        SyntaxKind::SWITCH_STMT | SyntaxKind::SWITCH_EXPR
                                    ))
                        })
                }
            };
            if targets_body {
                jumps.push(node);
            }
        }
        // 途经的 try 有不能正常结束的 finally 时，跳转被 finally 吞掉（JLS §14.22）
        jumps.iter().any(|jump| {
            let mut inner = jump.clone();
            for ancestor in jump.ancestors().skip(1) {
                if ancestor == *body {
                    break;
                }
                if let Some(try_stmt) = ast::TryStmt::cast(ancestor.clone()) {
                    if let Some(finally) = try_stmt.finally_clause() {
                        if *finally.syntax() != inner {
                            match finally.body().map(|b| block_completes(&b)) {
                                Some(Some(false)) => return false,
                                Some(Some(true)) => {}
                                _ => self.uncertain = true,
                            }
                        }
                    }
                }
                inner = ancestor;
            }
            true
        })
    }

    /// 块能否正常结束；顺带报告块里第一条不可达的语句。
    fn block(&mut self, block: &ast::Block) -> bool {
        let mut reachable = true;
        for stmt in block.statements() {
            if !reachable {
                // 局部类声明、空语句跟在后面不算错
                if !matches!(
                    stmt,
                    ast::Stmt::EmptyStmt(_)
                        | ast::Stmt::ClassDecl(_)
                        | ast::Stmt::InterfaceDecl(_)
                        | ast::Stmt::EnumDecl(_)
                        | ast::Stmt::RecordDecl(_)
                ) {
                    self.out.push(Diagnostic {
                        range: first_line_range(stmt.syntax()),
                        message: base_db::tr!("不可达的语句", "Unreachable statement"),
                        severity: Severity::Error,
                        code: DiagnosticCode::UnreachableCode,
                        unused: true,
                    });
                }
                // 只报第一条；后面的语句不再分析，避免连锁报告
                return false;
            }
            reachable = self.stmt(&stmt);
        }
        reachable
    }

    fn opt_block(&mut self, block: Option<ast::Block>) -> bool {
        block.is_none_or(|b| self.block(&b))
    }

    fn opt_stmt(&mut self, stmt: Option<ast::Stmt>) -> bool {
        stmt.is_none_or(|s| self.stmt(&s))
    }

    /// 语句能否正常结束。
    fn stmt(&mut self, stmt: &ast::Stmt) -> bool {
        match stmt {
            ast::Stmt::ReturnStmt(_)
            | ast::Stmt::ThrowStmt(_)
            | ast::Stmt::BreakStmt(_)
            | ast::Stmt::ContinueStmt(_)
            | ast::Stmt::YieldStmt(_) => false,
            ast::Stmt::Block(block) => self.block(block),
            ast::Stmt::IfStmt(if_stmt) => {
                let then_completes = self.opt_stmt(if_stmt.then_branch());
                match if_stmt.else_branch() {
                    Some(else_branch) => {
                        let else_completes = self.stmt(&else_branch);
                        then_completes || else_completes
                    }
                    // 没有 else 的 if 总能正常结束（JLS 特意这样规定，方便 `if (DEBUG)`）
                    None => true,
                }
            }
            ast::Stmt::WhileStmt(while_stmt) => {
                let body = while_stmt.body();
                let labels = own_labels(while_stmt.syntax());
                let breaks = body
                    .as_ref()
                    .is_some_and(|b| self.jumps_to(b.syntax(), &labels, false, false));
                self.opt_stmt(body);
                !is_true_literal(while_stmt.condition()) || breaks
            }
            ast::Stmt::DoWhileStmt(do_while) => {
                let body = do_while.body();
                let labels = own_labels(do_while.syntax());
                let breaks = body
                    .as_ref()
                    .is_some_and(|b| self.jumps_to(b.syntax(), &labels, false, false));
                let continues = body
                    .as_ref()
                    .is_some_and(|b| self.jumps_to(b.syntax(), &labels, true, false));
                let body_completes = self.opt_stmt(body);
                let reaches_condition = body_completes || continues;
                breaks || (reaches_condition && !is_true_literal(do_while.condition()))
            }
            ast::Stmt::ForStmt(for_stmt) => {
                let body = for_stmt.body();
                let labels = own_labels(for_stmt.syntax());
                let breaks = body
                    .as_ref()
                    .is_some_and(|b| self.jumps_to(b.syntax(), &labels, false, false));
                self.opt_stmt(body);
                let endless = match for_stmt.condition() {
                    None => true,
                    condition => is_true_literal(condition),
                };
                !endless || breaks
            }
            ast::Stmt::ForeachStmt(foreach) => {
                if let Some(body) = foreach
                    .syntax()
                    .children()
                    .filter_map(ast::Stmt::cast)
                    .last()
                {
                    self.stmt(&body);
                }
                true
            }
            ast::Stmt::LabeledStmt(labeled) => {
                let body = labeled.body();
                // `break 标签` 可以跳到这条语句之后
                let breaks = match (&body, labeled.label()) {
                    (Some(body), Some(label)) => {
                        self.jumps_to(body.syntax(), &[label.text()], false, true)
                    }
                    _ => false,
                };
                self.opt_stmt(body) || breaks
            }
            ast::Stmt::SynchronizedStmt(sync) => self.opt_block(sync.body()),
            ast::Stmt::TryStmt(try_stmt) => {
                let mut completes = self.opt_block(try_stmt.body());
                for catch in try_stmt.catch_clauses() {
                    completes |= self.opt_block(catch.body());
                }
                match try_stmt.finally_clause() {
                    Some(finally) => self.opt_block(finally.body()) && completes,
                    None => completes,
                }
            }
            ast::Stmt::SwitchStmt(switch) => {
                // 分支里的块照常检查。没有 `default` 的普通 switch 可能一个分支都不进，
                // 一定能正常结束；其余情形整体能否结束不精确建模
                if !switch_may_match_nothing(switch.syntax()) {
                    self.uncertain = true;
                }
                let blocks: Vec<ast::Block> = switch
                    .syntax()
                    .descendants()
                    .filter(|n| n.parent().is_some_and(|p| p.kind() != SyntaxKind::BLOCK))
                    .filter_map(ast::Block::cast)
                    .filter(|b| {
                        // 只取直接属于这个 switch 的块（不进入 lambda、嵌套语句）
                        b.syntax()
                            .ancestors()
                            .skip(1)
                            .take_while(|a| a != switch.syntax())
                            .all(|a| {
                                !matches!(
                                    a.kind(),
                                    SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY
                                ) && !ast::Stmt::can_cast(a.kind())
                            })
                    })
                    .collect();
                for block in blocks {
                    self.block(&block);
                }
                true
            }
            _ => true,
        }
    }
}

/// 多行语句只标第一行，避免整段代码被划线。
fn first_line_range(node: &SyntaxNode) -> TextRange {
    let range = node.text_range();
    let text = node.text().to_string();
    match text.find('\n') {
        Some(idx) => TextRange::at(range.start(), (text[..idx].trim_end().len() as u32).into()),
        None => range,
    }
}

fn check_callable(node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    let (body, needs_value, name_range) = if let Some(method) = ast::MethodDecl::cast(node.clone())
    {
        let returns_value = method
            .return_type()
            .is_some_and(|ty| ty.syntax().text().to_string().trim() != "void");
        (
            method.body(),
            returns_value,
            method.name().map(|n| n.syntax().text_range()),
        )
    } else if let Some(ctor) = ast::ConstructorDecl::cast(node.clone()) {
        (ctor.body(), false, None)
    } else if let Some(lambda) = ast::LambdaExpr::cast(node.clone()) {
        (lambda.body_block(), false, None)
    } else {
        // 初始化块
        (node.children().find_map(ast::Block::cast), false, None)
    };
    let Some(body) = body else { return };
    let mut flow = Flow {
        out,
        uncertain: false,
    };
    let completes = flow.block(&body);
    if completes && needs_value && !flow.uncertain {
        let range = body
            .r_curly_token()
            .map(|t| t.text_range())
            .or(name_range)
            .unwrap_or_else(|| body.syntax().text_range());
        flow.out.push(Diagnostic {
            range,
            message: base_db::tr!("方法缺少 return 语句", "Missing return statement"),
            severity: Severity::Error,
            code: DiagnosticCode::MissingReturn,
            unused: false,
        });
    }
}

/// 不可达语句与缺少 `return`。调用方要保证文件没有语法错误。
pub(crate) fn flow_diagnostics(nodes: impl Iterator<Item = SyntaxNode>, out: &mut Vec<Diagnostic>) {
    for node in nodes {
        if matches!(
            node.kind(),
            SyntaxKind::METHOD_DECL
                | SyntaxKind::CONSTRUCTOR_DECL
                | SyntaxKind::LAMBDA_EXPR
                | SyntaxKind::INITIALIZER_BLOCK
        ) {
            check_callable(&node, out);
        }
    }
}

fn strip_parens(expr: ast::Expr) -> ast::Expr {
    match expr {
        ast::Expr::ParenExpr(paren) => match paren.expr() {
            Some(inner) => strip_parens(inner),
            None => ast::Expr::ParenExpr(paren),
        },
        other => other,
    }
}

/// 简单名字或 `this.x` 这种没有副作用、可以按文本比较的表达式。
fn is_simple_reference(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::NameExpr(_) => true,
        ast::Expr::FieldAccess(access) => access
            .syntax()
            .children()
            .filter_map(ast::Expr::cast)
            .all(|receiver| matches!(receiver, ast::Expr::ThisExpr(_) | ast::Expr::NameExpr(_))),
        _ => false,
    }
}

/// 确定赋值（JLS §16）：跟踪一个没有初值的局部变量，沿语句结构计算"此处是否一定已赋值"。
///
/// 所有拿不准的地方都往"已赋值"的方向近似，所以可能漏报、不会误报：
/// - 条件表达式（`&&`、`||`、`?:`）里的赋值一律算数；
/// - `switch` 里只要出现对该变量的赋值，整个 `switch` 之后都算已赋值；
/// - 循环之后的状态只看进入循环之前（`while (true)` 之后算已赋值），不看循环里的 `break`；
/// - lambda、匿名类 / 局部类里的读取不检查。
///
/// 也用来检查构造器是否给 blank final 字段赋了值：这时 `refs` 里是字段的简单名和 `this.x`
/// 的名字。
pub(crate) struct DefiniteAssignment<'a> {
    /// 该变量所有引用（`NAME_REF`）的范围
    pub(crate) refs: &'a FxHashSet<TextRange>,
    /// 解析到 `static final` 字段的名字引用：`if (FLAG)` 的条件是它时可能是常量
    pub(crate) field_refs: &'a FxHashSet<TextRange>,
    pub(crate) assigned: bool,
    /// 未赋值就读取的位置。和 javac 一样，报过一次之后把变量当作已赋值，避免同一条路径上连串重复
    pub(crate) bad_reads: Vec<TextRange>,
    /// 还没走到目标语句的 `break 标签`：(标签, 跳转时是否已赋值)
    pub(crate) labeled_breaks: Vec<(SmolStr, bool)>,
}

impl DefiniteAssignment<'_> {
    /// 被赋值的是这个变量：`x`，或者（字段的）`this.x`。
    fn is_var(&self, target: &SyntaxNode) -> bool {
        let named = |node: &SyntaxNode| {
            node.children()
                .any(|c| c.kind() == SyntaxKind::NAME_REF && self.refs.contains(&c.text_range()))
        };
        match target.kind() {
            SyntaxKind::NAME_EXPR => named(target),
            SyntaxKind::FIELD_ACCESS => {
                ast::FieldAccess::cast(target.clone())
                    .and_then(|access| access.receiver())
                    .is_some_and(
                        |receiver| matches!(receiver, ast::Expr::ThisExpr(this) if this.qualifier().is_none()),
                    )
                    && named(target)
            }
            SyntaxKind::PAREN_EXPR => target
                .children()
                .find(|c| ast::Expr::can_cast(c.kind()))
                .is_some_and(|inner| self.is_var(&inner)),
            _ => false,
        }
    }

    fn read(&mut self, range: TextRange) {
        if !self.assigned {
            if !self.bad_reads.contains(&range) {
                self.bad_reads.push(range);
            }
            self.assigned = true;
        }
    }

    fn assigns_var(&self, node: &SyntaxNode) -> bool {
        node.descendants().any(|n| {
            n.kind() == SyntaxKind::ASSIGN_EXPR
                && n.first_child().is_some_and(|lhs| self.is_var(&lhs))
        })
    }

    /// 按求值顺序走一遍表达式（或任何不含需要单独处理的语句的节点）。
    pub(crate) fn expr(&mut self, node: &SyntaxNode) {
        match node.kind() {
            SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY => {}
            SyntaxKind::NAME_REF => {
                if self.refs.contains(&node.text_range()) {
                    self.read(node.text_range());
                }
            }
            SyntaxKind::ASSIGN_EXPR if node.first_child().is_some_and(|lhs| self.is_var(&lhs)) => {
                let compound = ast::AssignExpr::cast(node.clone())
                    .and_then(|a| a.op_token())
                    .is_some_and(|t| t.kind() != SyntaxKind::EQ);
                let mut children = node.children();
                let lhs = children.next();
                if compound {
                    // `x += 1` 先读后写
                    if let Some(lhs) = &lhs {
                        self.read(lhs.text_range());
                    }
                }
                for rhs in children {
                    self.expr(&rhs);
                }
                self.assigned = true;
            }
            SyntaxKind::SWITCH_EXPR | SyntaxKind::SWITCH_STMT => self.switch(node),
            kind if ast::Stmt::can_cast(kind) => self.stmt(node),
            _ => {
                for child in node.children() {
                    self.expr(&child);
                }
            }
        }
    }

    fn switch(&mut self, node: &SyntaxNode) {
        if self.assigns_var(node) {
            // 不建模各分支，里面的读取也不检查。之后是否算已赋值：没有 `default` 的普通
            // switch 语句可能一个分支都不进，赋值就不确定；其余情形一律算已赋值
            self.assigned = self.assigned || !switch_may_match_nothing(node);
            return;
        }
        let before = self.assigned;
        for child in node.children() {
            self.expr(&child);
        }
        // 每个分支都以跳转结束的 switch 之后的代码不可达，按"已赋值"处理
        self.assigned = before || switch_cannot_complete(node);
    }

    /// 在当前状态下分析 `node`，返回分析完之后的状态，并把当前状态恢复成 `entry`。
    fn branch(&mut self, node: Option<SyntaxNode>, entry: bool) -> bool {
        self.assigned = entry;
        if let Some(node) = node {
            self.expr(&node);
        }
        std::mem::replace(&mut self.assigned, entry)
    }

    fn stmt(&mut self, node: &SyntaxNode) {
        match node.kind() {
            SyntaxKind::BLOCK => {
                for child in node.children() {
                    self.expr(&child);
                }
            }
            // 跳走的语句：先求值它的表达式；之后的代码不可达，按"已赋值"处理
            SyntaxKind::RETURN_STMT
            | SyntaxKind::THROW_STMT
            | SyntaxKind::BREAK_STMT
            | SyntaxKind::CONTINUE_STMT
            | SyntaxKind::YIELD_STMT => {
                for child in node.children() {
                    self.expr(&child);
                }
                if let Some(label) = ast::BreakStmt::cast(node.clone()).and_then(|b| b.label()) {
                    // 途经 finally 时 finally 里的赋值也算数，这里不建模，按已赋值处理
                    let through_finally = node.ancestors().any(|a| {
                        ast::TryStmt::cast(a)
                            .and_then(|t| t.finally_clause())
                            .is_some_and(|f| {
                                !f.syntax().text_range().contains_range(node.text_range())
                            })
                    });
                    self.labeled_breaks
                        .push((label.text(), self.assigned || through_finally));
                }
                self.assigned = true;
            }
            SyntaxKind::IF_STMT => {
                let Some(if_stmt) = ast::IfStmt::cast(node.clone()) else {
                    return;
                };
                if let Some(condition) = if_stmt.condition() {
                    self.expr(condition.syntax());
                }
                let entry = self.assigned;
                // `if (FLAG) { x = ...; }`：条件是常量 `true` 时，"条件为假"的路径不存在。
                // 这里不求常量的值，条件是一个单独的 `static final` 字段时就按可能是常量处理。
                let maybe_constant = if_stmt.condition().map(strip_parens).is_some_and(|c| {
                    matches!(c, ast::Expr::NameExpr(_) | ast::Expr::FieldAccess(_))
                        && c.syntax()
                            .descendants()
                            .filter(|n| n.kind() == SyntaxKind::NAME_REF)
                            .last()
                            .is_some_and(|n| self.field_refs.contains(&n.text_range()))
                });
                // 常量 `false` 时 then 分支不可达，里面的读取不算数；常量 `true` 时 else 同理
                let then_state = {
                    let state = self.branch(
                        if_stmt.then_branch().map(|s| s.syntax().clone()),
                        entry || maybe_constant,
                    );
                    state
                };
                let else_state = match if_stmt.else_branch() {
                    Some(else_branch) => {
                        self.branch(Some(else_branch.syntax().clone()), entry || maybe_constant)
                    }
                    None => entry || maybe_constant,
                };
                self.assigned = then_state && else_state;
            }
            SyntaxKind::WHILE_STMT => {
                let Some(while_stmt) = ast::WhileStmt::cast(node.clone()) else {
                    return;
                };
                let endless = is_true_literal(while_stmt.condition());
                if let Some(condition) = while_stmt.condition() {
                    self.expr(condition.syntax());
                }
                let entry = self.assigned;
                self.branch(while_stmt.body().map(|s| s.syntax().clone()), entry);
                // 条件为假时循环体一次都不执行；`while (true)` 只能经 break 离开
                self.assigned = entry || endless;
            }
            SyntaxKind::DO_WHILE_STMT => {
                let Some(do_while) = ast::DoWhileStmt::cast(node.clone()) else {
                    return;
                };
                if let Some(body) = do_while.body() {
                    self.expr(body.syntax());
                }
                if let Some(condition) = do_while.condition() {
                    self.expr(condition.syntax());
                }
            }
            SyntaxKind::FOR_STMT => {
                let Some(for_stmt) = ast::ForStmt::cast(node.clone()) else {
                    return;
                };
                if let Some(init) = for_stmt.init() {
                    self.expr(init.syntax());
                }
                let endless = match for_stmt.condition() {
                    None => true,
                    condition => is_true_literal(condition),
                };
                if let Some(condition) = for_stmt.condition() {
                    self.expr(condition.syntax());
                }
                let entry = self.assigned;
                let after_body = self.branch(for_stmt.body().map(|s| s.syntax().clone()), entry);
                self.branch(for_stmt.update().map(|u| u.syntax().clone()), after_body);
                self.assigned = entry || endless;
            }
            SyntaxKind::FOREACH_STMT => {
                // 最后一个子语句是循环体，之前的是变量声明和被遍历的表达式
                let body = node
                    .children()
                    .filter(|c| ast::Stmt::can_cast(c.kind()))
                    .last();
                for child in node.children() {
                    if Some(&child) != body.as_ref() {
                        self.expr(&child);
                    }
                }
                let entry = self.assigned;
                self.branch(body, entry);
            }
            SyntaxKind::LABELED_STMT => {
                let label = ast::LabeledStmt::cast(node.clone())
                    .and_then(|l| l.label())
                    .map(|l| l.text());
                let mark = self.labeled_breaks.len();
                for child in node.children() {
                    self.expr(&child);
                }
                // 语句之后：正常结束时的状态和每个跳到这里的 `break` 处的状态都要已赋值。
                // 标签不能在其作用域里重名，名字相同的就是跳到这里的
                let mut pending = self.labeled_breaks.split_off(mark);
                if let Some(label) = label {
                    pending.retain(|(l, assigned)| {
                        if *l == label {
                            self.assigned &= *assigned;
                            false
                        } else {
                            true
                        }
                    });
                }
                self.labeled_breaks.extend(pending);
            }
            SyntaxKind::TRY_STMT => {
                let Some(try_stmt) = ast::TryStmt::cast(node.clone()) else {
                    return;
                };
                if let Some(resources) = try_stmt.resource_list() {
                    self.expr(resources.syntax());
                }
                let entry = self.assigned;
                let mut state = self.branch(try_stmt.body().map(|b| b.syntax().clone()), entry);
                for catch in try_stmt.catch_clauses() {
                    // 异常可能在 try 块的任何位置抛出：catch 从 try 之前的状态开始
                    state &= self.branch(catch.body().map(|b| b.syntax().clone()), entry);
                }
                let finally_state = match try_stmt.finally_clause() {
                    Some(finally) => self.branch(finally.body().map(|b| b.syntax().clone()), entry),
                    None => false,
                };
                self.assigned = state || finally_state;
            }
            // 局部类里的代码有自己的规则，不进去
            SyntaxKind::CLASS_DECL
            | SyntaxKind::INTERFACE_DECL
            | SyntaxKind::ENUM_DECL
            | SyntaxKind::RECORD_DECL => {}
            _ => {
                for child in node.children() {
                    self.expr(&child);
                }
            }
        }
    }
}

/// 保守判断一个 `switch` 语句不可能正常结束：有 `default`，没有跳出它的 `break`，
/// 并且每个带语句的分支都以 `return` / `throw` / `continue` 收尾。
/// `switch` 自己的各个分支（不含嵌套 switch 的）。
fn own_cases(switch: &SyntaxNode) -> Vec<SyntaxNode> {
    switch
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::SWITCH_CASE)
        .filter(|n| {
            n.ancestors()
                .skip(1)
                .find(|a| matches!(a.kind(), SyntaxKind::SWITCH_STMT | SyntaxKind::SWITCH_EXPR))
                == Some(switch.clone())
        })
        .collect()
}

fn case_labels(case: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> {
    case.children()
        .filter(|c| c.kind() == SyntaxKind::SWITCH_LABEL)
}

fn has_default_case(cases: &[SyntaxNode]) -> bool {
    cases.iter().any(|case| {
        case_labels(case)
            .flat_map(|label| label.children_with_tokens())
            .any(|element| element.kind() == SyntaxKind::DEFAULT_KW)
    })
}

/// 普通的 switch 语句（标签都是常量，没有模式和 `null`）并且没有 `default`：
/// 运行时可能一个分支都不匹配。带模式的 switch 语句必须穷尽，不在此列。
fn switch_may_match_nothing(switch: &SyntaxNode) -> bool {
    if switch.kind() != SyntaxKind::SWITCH_STMT {
        return false;
    }
    let cases = own_cases(switch);
    if has_default_case(&cases) {
        return false;
    }
    !cases.iter().flat_map(case_labels).any(|label| {
        label.descendants_with_tokens().any(|element| {
            matches!(
                element.kind(),
                SyntaxKind::TYPE_PATTERN
                    | SyntaxKind::RECORD_PATTERN
                    | SyntaxKind::UNNAMED_PATTERN
                    | SyntaxKind::NULL_KW
            )
        })
    })
}

fn switch_cannot_complete(switch: &SyntaxNode) -> bool {
    if switch.kind() != SyntaxKind::SWITCH_STMT || has_break(switch) {
        return false;
    }
    let cases = own_cases(switch);
    let has_default = has_default_case(&cases);
    fn ends_with_jump(stmt: &SyntaxNode) -> bool {
        match stmt.kind() {
            SyntaxKind::RETURN_STMT | SyntaxKind::THROW_STMT | SyntaxKind::CONTINUE_STMT => true,
            SyntaxKind::BLOCK => stmt
                .children()
                .filter(|c| ast::Stmt::can_cast(c.kind()))
                .last()
                .is_some_and(|last| ends_with_jump(&last)),
            _ => false,
        }
    }
    has_default
        && cases.iter().all(|case| {
            match case
                .children()
                .filter(|c| ast::Stmt::can_cast(c.kind()))
                .last()
            {
                Some(last) => ends_with_jump(&last),
                // 没有语句的 `case A:` 落到下一组
                None => true,
            }
        })
}

/// 读取时不一定已经赋值的局部变量。
fn uninitialized_reads(
    db: &dyn Db,
    body: &SyntaxNode,
    result: &hir::InferenceResult,
    out: &mut Vec<Diagnostic>,
) {
    let field_refs: FxHashSet<TextRange> = result
        .resolutions()
        .filter(|(_, resolution)| match resolution {
            hir::Resolution::Field(field) => hir::class_data(db, field.class)
                .and_then(|data| data.fields.get(field.idx as usize).map(|f| f.flags))
                .is_some_and(|flags| flags.is_static() && flags.contains(hir::Modifiers::FINAL)),
            _ => false,
        })
        .map(|(range, _)| range)
        .collect();
    for (idx, local) in result.locals().iter().enumerate() {
        if local.kind != hir::LocalKind::Local
            || !body.text_range().contains_range(local.decl_range)
        {
            continue;
        }
        let declarator = match body.covering_element(local.decl_range) {
            syntax::NodeOrToken::Node(node) => Some(node),
            syntax::NodeOrToken::Token(token) => token.parent(),
        }
        .and_then(|n| n.ancestors().find_map(ast::VariableDeclarator::cast));
        let Some(declarator) = declarator else {
            continue;
        };
        if declarator.initializer().is_some() {
            continue;
        }
        // 只处理直接写在块里的 `T x;`（for 的初始化部分、switch 分支里的声明不处理）
        let Some(decl) = declarator
            .syntax()
            .parent()
            .filter(|p| p.kind() == SyntaxKind::LOCAL_VAR_DECL)
        else {
            continue;
        };
        if decl.parent().is_none_or(|p| p.kind() != SyntaxKind::BLOCK) {
            continue;
        }

        let refs: FxHashSet<TextRange> = result
            .resolutions()
            .filter(|(_, resolution)| matches!(resolution, hir::Resolution::Local(id) if *id as usize == idx))
            .map(|(range, _)| range)
            .collect();
        if refs.is_empty() {
            continue;
        }
        let mut analysis = DefiniteAssignment {
            refs: &refs,
            field_refs: &field_refs,
            assigned: false,
            bad_reads: Vec::new(),
            labeled_breaks: Vec::new(),
        };
        // 同一条声明里靠后的声明符（`int a, b = a;`）
        for later in decl
            .children()
            .filter(|c| c.text_range().start() > declarator.syntax().text_range().end())
        {
            analysis.expr(&later);
        }
        let mut next = decl.next_sibling();
        while let Some(stmt) = next {
            analysis.expr(&stmt);
            next = stmt.next_sibling();
        }
        for range in analysis.bad_reads {
            out.push(Diagnostic {
                range,
                message: base_db::tr!(
                    "变量 `{}` 可能尚未初始化",
                    "Variable `{}` might not have been initialized",
                    local.name
                ),
                severity: Severity::Error,
                code: DiagnosticCode::UninitializedVariable,
                unused: false,
            });
        }
    }
}

/// 基于表达式本身的检查项。
pub(crate) fn expression_inspections(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    for body in &view.bodies {
        let node = &body.node;
        let result = body.result(view);
        uninitialized_reads(db, node, &result, out);
        for expr in node.descendants() {
            match expr.kind() {
                SyntaxKind::BINARY_EXPR => {
                    let Some(binary) = ast::BinaryExpr::cast(expr.clone()) else {
                        continue;
                    };
                    let Some(op) = binary.op_kind() else { continue };
                    let (Some(lhs), Some(rhs)) = (binary.lhs(), binary.rhs()) else {
                        continue;
                    };
                    let (lhs, rhs) = (strip_parens(lhs), strip_parens(rhs));
                    let is_equality = matches!(op, SyntaxKind::EQ_EQ | SyntaxKind::NEQ);
                    let is_comparison = is_equality
                        || matches!(
                            op,
                            SyntaxKind::LT | SyntaxKind::GT | SyntaxKind::LT_EQ | SyntaxKind::GT_EQ
                        );

                    if is_comparison
                        && is_simple_reference(&lhs)
                        && lhs.syntax().text() == rhs.syntax().text()
                    {
                        // `x != x` 是判断 NaN 的老写法，浮点数不提示
                        let floating =
                            result.type_of(lhs.syntax().text_range()).is_some_and(|ty| {
                                matches!(
                                    ty.display(db).as_str(),
                                    "float" | "double" | "Float" | "Double"
                                )
                            });
                        if !floating {
                            out.push(Diagnostic {
                                range: binary.syntax().text_range(),
                                message: base_db::tr!(
                                    "表达式的两边相同",
                                    "Both sides of the expression are identical"
                                ),
                                severity: Severity::Warning,
                                code: DiagnosticCode::SelfComparison,
                                unused: false,
                            });
                        }
                        continue;
                    }

                    if is_equality {
                        let is_string = |e: &ast::Expr| {
                            !matches!(e, ast::Expr::Literal(l) if l.kind() == Some(SyntaxKind::NULL_KW))
                                && result
                                    .type_of(e.syntax().text_range())
                                    .is_some_and(|ty| ty.display(db) == "String")
                        };
                        if is_string(&lhs) && is_string(&rhs) {
                            out.push(Diagnostic {
                                range: binary
                                    .op_token()
                                    .map(|t| t.text_range())
                                    .unwrap_or_else(|| binary.syntax().text_range()),
                                message: base_db::tr!("用 `==` 比较的是字符串的引用，比较内容请用 `equals()`", "`==` compares string references; use `equals()` to compare contents"),
                                severity: Severity::Warning,
                                code: DiagnosticCode::StringEquality,
                                unused: false,
                            });
                        }
                        // 两边都是包装类型的数字：比较的是对象引用（只有 -128..127 有缓存）
                        let boxed_number = |e: &ast::Expr| {
                            result
                                .type_of(e.syntax().text_range())
                                .and_then(|ty| match ty {
                                    hir::Ty::Class(..) => ty
                                        .unboxed(db)
                                        .filter(|p| *p != hir::PrimitiveType::Boolean)
                                        .map(|_| ty.display(db)),
                                    _ => None,
                                })
                        };
                        if let (Some(left), Some(_)) = (boxed_number(&lhs), boxed_number(&rhs)) {
                            out.push(Diagnostic {
                                range: binary
                                    .op_token()
                                    .map(|t| t.text_range())
                                    .unwrap_or_else(|| binary.syntax().text_range()),
                                message: base_db::tr!(
                                    "用 `==` 比较的是 `{left}` 对象的引用，比较数值请用 `equals()`",
                                    "`==` compares `{left}` object references; use `equals()` to compare values"
                                ),
                                severity: Severity::Warning,
                                code: DiagnosticCode::NumberEquality,
                                unused: false,
                            });
                        }
                    }
                }
                SyntaxKind::ASSIGN_EXPR => {
                    let Some(assign) = ast::AssignExpr::cast(expr.clone()) else {
                        continue;
                    };
                    if assign.op_token().is_none_or(|t| t.kind() != SyntaxKind::EQ) {
                        continue;
                    }
                    let (Some(lhs), Some(rhs)) = (assign.lhs(), assign.rhs()) else {
                        continue;
                    };
                    let rhs = strip_parens(rhs);
                    if is_simple_reference(&lhs) && lhs.syntax().text() == rhs.syntax().text() {
                        out.push(Diagnostic {
                            range: assign.syntax().text_range(),
                            message: base_db::tr!(
                                "变量赋值给了自己",
                                "Variable is assigned to itself"
                            ),
                            severity: Severity::Warning,
                            code: DiagnosticCode::SelfAssignment,
                            unused: false,
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(text: &str) -> Vec<(DiagnosticCode, String)> {
        let parse = ast::CompilationUnit::parse(text);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let mut out = Vec::new();
        flow_diagnostics(parse.syntax_node().descendants(), &mut out);
        out.into_iter()
            .map(|d| {
                (
                    d.code,
                    text[usize::from(d.range.start())..usize::from(d.range.end())].to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn statement_after_return_is_unreachable() {
        let diags = flow("class A { int f() { return 1; int x = 2; } }");
        assert_eq!(
            diags,
            [(DiagnosticCode::UnreachableCode, "int x = 2;".to_owned())]
        );
    }

    #[test]
    fn code_after_endless_loop_is_unreachable() {
        let diags = flow("class A { void f() { while (true) { g(); } g(); } void g() {} }");
        assert_eq!(
            diags,
            [(DiagnosticCode::UnreachableCode, "g();".to_owned())]
        );

        let ok = flow("class A { void f() { while (true) { if (h()) break; } g(); } void g() {} boolean h() { return true; } }");
        assert!(ok.is_empty(), "{ok:?}");

        // 内层循环的 break 跳不出外层
        let diags =
            flow("class A { void f() { for (;;) { while (true) { break; } } g(); } void g() {} }");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn if_else_that_always_returns() {
        let ok = flow("class A { int f(boolean b) { if (b) { return 1; } else { return 2; } } }");
        assert!(ok.is_empty(), "{ok:?}");

        let diags = flow("class A { int f(boolean b) { if (b) { return 1; } } }");
        assert_eq!(diags, [(DiagnosticCode::MissingReturn, "}".to_owned())]);
    }

    #[test]
    fn try_finally_and_throw() {
        let ok = flow("class A { int f() { try { return g(); } catch (RuntimeException e) { throw e; } } int g() { return 1; } }");
        assert!(ok.is_empty(), "{ok:?}");

        let ok = flow(
            "class A { int f() { try { g(); } finally { return 1; } } int g() { return 1; } }",
        );
        assert!(ok.is_empty(), "{ok:?}");
    }

    #[test]
    fn switch_and_abstract_methods_are_not_reported() {
        let ok = flow("abstract class A { abstract int f(); int g(int x) { switch (x) { case 1: return 1; default: return 2; } } }");
        assert!(ok.is_empty(), "{ok:?}");
    }

    #[test]
    fn labeled_jumps_decide_missing_return() {
        // `break outer` 跳出外层循环，方法可以执行到末尾
        let diags = flow("class A { int f(int[] a) { outer: for (int v : a) { while (true) { if (v > 0) break outer; return v; } } } }");
        assert_eq!(diags, [(DiagnosticCode::MissingReturn, "}".to_owned())]);

        // 内层带标签语句的 break 跳不出 `while (true)`
        let ok = flow(
            "class A { int f(boolean b) { while (true) { inner: { if (b) break inner; } } } }",
        );
        assert!(ok.is_empty(), "{ok:?}");

        // `continue outer` 不会让循环结束
        let ok = flow("class A { int f(int[][] g) { outer: while (true) { for (int[] r : g) { if (r.length == 0) continue outer; return r[0]; } } } }");
        assert!(ok.is_empty(), "{ok:?}");

        // 被不能正常结束的 finally 吞掉的 break
        let ok = flow("class A { int f(int n) { outer: for (;;) { try { if (n > 0) break outer; } finally { return n; } } } }");
        assert!(ok.is_empty(), "{ok:?}");

        // 带标签的语句块之后可以到达
        let diags = flow("class A { int f(boolean b) { done: { if (b) break done; return 1; } } }");
        assert_eq!(diags, [(DiagnosticCode::MissingReturn, "}".to_owned())]);

        let diags = flow(
            "class A { void f() { while (true) { inner: { break inner; } } g(); } void g() {} }",
        );
        assert_eq!(
            diags,
            [(DiagnosticCode::UnreachableCode, "g();".to_owned())]
        );
    }

    #[test]
    fn void_methods_and_lambdas_need_no_return() {
        let ok = flow("class A { void f() { Runnable r = () -> { g(); }; r.run(); } void g() {} A() { g(); } }");
        assert!(ok.is_empty(), "{ok:?}");
    }
}
