//! 赋了值却永远不会被读到的写入（"死存储"）：`int x = 0; x = compute(); use(x);` 里的 `0`。
//!
//! 对每个局部变量做"可能被读到"的前向分析：状态是还没被读过的写入的集合，读取时它们都算
//! 用过了；分支汇合时取并集，循环体分析两遍让回边带回来的写入也有机会被读到。分析结束时
//! 从没被用过的写入就是死存储。
//!
//! 所有拿不准的地方都往"会被读到"的方向近似：带标签的跳转、`finally` 里的读取都直接放弃这个
//! 变量；`catch` 从 try 块里任何一次写入之后开始。只报告简单赋值 `x = ...` 和初值，不报告
//! `x += 1`、`x++`。

use base_db::Db;
use hir::{infer, LocalKind, Resolution};
use rustc_hash::{FxHashMap, FxHashSet};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{Diagnostic, DiagnosticCode, FileBody, Severity};

type Pending = FxHashSet<u32>;

#[derive(Default)]
struct JumpTarget {
    /// 循环：`continue` 也落在这里；switch：只收 `break` / `yield`
    is_loop: bool,
    breaks: Pending,
    continues: Pending,
}

struct Analysis<'a> {
    /// 该变量所有名字引用的范围
    refs: &'a FxHashSet<TextRange>,
    /// 含有该变量的引用或者跳转语句的节点：其它子树不影响分析结果，直接跳过
    interesting: FxHashSet<(TextRange, SyntaxKind)>,
    /// 写入：赋值表达式（或声明符）的范围 → 编号
    writes: FxHashMap<TextRange, u32>,
    pending: Pending,
    used: Pending,
    targets: Vec<JumpTarget>,
}

impl Analysis<'_> {
    fn is_var(&self, node: &SyntaxNode) -> bool {
        node.kind() == SyntaxKind::NAME_EXPR
            && node
                .children()
                .any(|c| c.kind() == SyntaxKind::NAME_REF && self.refs.contains(&c.text_range()))
    }

    fn read(&mut self) {
        self.used.extend(self.pending.iter().copied());
    }

    fn write(&mut self, at: TextRange) {
        self.pending.clear();
        if let Some(id) = self.writes.get(&at) {
            self.pending.insert(*id);
        }
    }

    /// 从 `entry` 开始分析 `node`，返回结束时的状态；分析完把当前状态恢复成 `entry`。
    fn branch(&mut self, node: Option<&SyntaxNode>, entry: &Pending) -> Pending {
        self.pending = entry.clone();
        if let Some(node) = node {
            self.visit(node);
        }
        std::mem::replace(&mut self.pending, entry.clone())
    }

    fn visit(&mut self, node: &SyntaxNode) {
        if !self.interesting.contains(&(node.text_range(), node.kind())) {
            return;
        }
        match node.kind() {
            SyntaxKind::NAME_REF => {
                if self.refs.contains(&node.text_range()) {
                    self.read();
                }
            }
            // lambda 和匿名类 / 局部类的体：里面的 return / break 不影响外层的流程，
            // 读取按创建时算（被捕获的变量本来就是事实上的 final）
            SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY => {
                let reads = node.descendants().any(|d| {
                    d.kind() == SyntaxKind::NAME_REF && self.refs.contains(&d.text_range())
                });
                if reads {
                    self.read();
                }
            }
            SyntaxKind::ASSIGN_EXPR if node.first_child().is_some_and(|lhs| self.is_var(&lhs)) => {
                let compound = ast::AssignExpr::cast(node.clone())
                    .and_then(|a| a.op_token())
                    .is_some_and(|t| t.kind() != SyntaxKind::EQ);
                if compound {
                    self.read();
                }
                for rhs in node.children().skip(1) {
                    self.visit(&rhs);
                }
                self.write(node.text_range());
            }
            SyntaxKind::VARIABLE_DECLARATOR => {
                for child in node.children() {
                    self.visit(&child);
                }
                if self.writes.contains_key(&node.text_range()) {
                    self.write(node.text_range());
                }
            }
            SyntaxKind::CONDITIONAL_EXPR => {
                let mut children = node.children();
                if let Some(cond) = children.next() {
                    self.visit(&cond);
                }
                let entry = self.pending.clone();
                let a = children.next();
                let b = children.next();
                let mut out = self.branch(a.as_ref(), &entry);
                out.extend(self.branch(b.as_ref(), &entry));
                self.pending = out;
            }
            SyntaxKind::BINARY_EXPR
                if ast::BinaryExpr::cast(node.clone())
                    .and_then(|b| b.op_kind())
                    .is_some_and(|op| {
                        matches!(op, SyntaxKind::AMP_AMP | SyntaxKind::PIPE_PIPE)
                    }) =>
            {
                let mut children = node.children();
                if let Some(lhs) = children.next() {
                    self.visit(&lhs);
                }
                let entry = self.pending.clone();
                let rhs = children.next();
                let mut out = self.branch(rhs.as_ref(), &entry);
                out.extend(entry);
                self.pending = out;
            }
            SyntaxKind::IF_STMT => {
                let Some(if_stmt) = ast::IfStmt::cast(node.clone()) else {
                    return;
                };
                if let Some(cond) = if_stmt.condition() {
                    self.visit(cond.syntax());
                }
                let entry = self.pending.clone();
                let then = if_stmt.then_branch().map(|s| s.syntax().clone());
                let other = if_stmt.else_branch().map(|s| s.syntax().clone());
                let mut out = self.branch(then.as_ref(), &entry);
                out.extend(match other {
                    Some(other) => self.branch(Some(&other), &entry),
                    None => entry,
                });
                self.pending = out;
            }
            SyntaxKind::WHILE_STMT => {
                let Some(it) = ast::WhileStmt::cast(node.clone()) else {
                    return;
                };
                let cond = it.condition().map(|c| c.syntax().clone());
                let body = it.body().map(|b| b.syntax().clone());
                let endless = cond.as_ref().is_some_and(|c| c.text() == "true");
                self.run_loop(cond.as_ref(), body.as_ref(), None, !endless, false);
            }
            SyntaxKind::DO_WHILE_STMT => {
                let Some(it) = ast::DoWhileStmt::cast(node.clone()) else {
                    return;
                };
                let cond = it.condition().map(|c| c.syntax().clone());
                let body = it.body().map(|b| b.syntax().clone());
                self.run_loop(cond.as_ref(), body.as_ref(), None, true, true);
            }
            SyntaxKind::FOR_STMT => {
                let Some(it) = ast::ForStmt::cast(node.clone()) else {
                    return;
                };
                if let Some(init) = it.init() {
                    self.visit(init.syntax());
                }
                let cond = it.condition().map(|c| c.syntax().clone());
                let body = it.body().map(|b| b.syntax().clone());
                let update = it.update().map(|u| u.syntax().clone());
                let endless = cond.as_ref().is_none_or(|c| c.text() == "true");
                self.run_loop(
                    cond.as_ref(),
                    body.as_ref(),
                    update.as_ref(),
                    !endless,
                    false,
                );
            }
            SyntaxKind::FOREACH_STMT => {
                let body = node
                    .children()
                    .filter(|c| ast::Stmt::can_cast(c.kind()))
                    .last();
                for child in node.children() {
                    if Some(&child) != body.as_ref() {
                        self.visit(&child);
                    }
                }
                self.run_loop(None, body.as_ref(), None, true, false);
            }
            SyntaxKind::SWITCH_STMT | SyntaxKind::SWITCH_EXPR => self.switch(node),
            SyntaxKind::TRY_STMT => self.try_stmt(node),
            SyntaxKind::RETURN_STMT | SyntaxKind::THROW_STMT => {
                for child in node.children() {
                    self.visit(&child);
                }
                self.pending.clear();
            }
            SyntaxKind::BREAK_STMT => {
                let pending = std::mem::take(&mut self.pending);
                if let Some(target) = self.targets.last_mut() {
                    target.breaks.extend(pending);
                }
            }
            SyntaxKind::YIELD_STMT => {
                for child in node.children() {
                    self.visit(&child);
                }
                let pending = std::mem::take(&mut self.pending);
                if let Some(target) = self.targets.iter_mut().rev().find(|t| !t.is_loop) {
                    target.breaks.extend(pending);
                }
            }
            SyntaxKind::CONTINUE_STMT => {
                let pending = std::mem::take(&mut self.pending);
                if let Some(target) = self.targets.iter_mut().rev().find(|t| t.is_loop) {
                    target.continues.extend(pending);
                }
            }
            SyntaxKind::ASSERT_STMT => {
                // 断言可能不执行
                let entry = self.pending.clone();
                for child in node.children() {
                    self.visit(&child);
                }
                self.pending.extend(entry);
            }
            // `a + b + c + …`：左结合的长链迭代展开，避免递归过深
            SyntaxKind::BINARY_EXPR => {
                let mut operands: Vec<SyntaxNode> = Vec::new();
                let mut current = node.clone();
                loop {
                    let mut children = current.children();
                    let (Some(lhs), rhs) = (children.next(), children.next()) else {
                        break;
                    };
                    operands.extend(rhs);
                    let short_circuit = ast::BinaryExpr::cast(lhs.clone())
                        .and_then(|b| b.op_kind())
                        .is_some_and(|op| {
                            matches!(op, SyntaxKind::AMP_AMP | SyntaxKind::PIPE_PIPE)
                        });
                    if lhs.kind() == SyntaxKind::BINARY_EXPR && !short_circuit {
                        current = lhs;
                    } else {
                        operands.push(lhs);
                        break;
                    }
                }
                for operand in operands.iter().rev() {
                    self.visit(operand);
                }
            }
            _ => {
                for child in node.children() {
                    self.visit(&child);
                }
            }
        }
    }

    /// 循环：`cond` 每轮先求值（`cond_first` 为假时是 do-while，先执行体），`update` 在体之后。
    fn run_loop(
        &mut self,
        cond: Option<&SyntaxNode>,
        body: Option<&SyntaxNode>,
        update: Option<&SyntaxNode>,
        may_exit_normally: bool,
        body_first: bool,
    ) {
        let mut head = self.pending.clone();
        let mut exit = Pending::default();
        for _ in 0..2 {
            self.pending = head.clone();
            if !body_first {
                if let Some(cond) = cond {
                    self.visit(cond);
                }
                if may_exit_normally {
                    exit.extend(self.pending.iter().copied());
                }
            }
            self.targets.push(JumpTarget {
                is_loop: true,
                ..JumpTarget::default()
            });
            if let Some(body) = body {
                self.visit(body);
            }
            let target = self.targets.pop().unwrap_or_default();
            self.pending.extend(target.continues);
            if let Some(update) = update {
                self.visit(update);
            }
            if body_first {
                if let Some(cond) = cond {
                    self.visit(cond);
                }
                exit.extend(self.pending.iter().copied());
            }
            exit.extend(target.breaks);
            head.extend(self.pending.iter().copied());
        }
        self.pending = exit;
    }

    fn switch(&mut self, node: &SyntaxNode) {
        let Some(scrutinee) = node.children().find(|c| ast::Expr::can_cast(c.kind())) else {
            return;
        };
        self.visit(&scrutinee);
        let Some(block) = node
            .children()
            .find(|c| c.kind() == SyntaxKind::SWITCH_BLOCK)
        else {
            return;
        };
        let entry = self.pending.clone();
        self.targets.push(JumpTarget::default());
        let mut exit = Pending::default();
        let mut fallthrough = Pending::default();
        let mut has_default = false;
        for case in block
            .children()
            .filter(|c| c.kind() == SyntaxKind::SWITCH_CASE)
        {
            let Some(case_ast) = ast::SwitchCase::cast(case.clone()) else {
                continue;
            };
            if case_ast.label().is_some_and(|l| {
                l.syntax()
                    .children_with_tokens()
                    .any(|e| e.kind() == SyntaxKind::DEFAULT_KW)
            }) {
                has_default = true;
            }
            let mut start = entry.clone();
            start.extend(fallthrough.iter().copied());
            self.pending = start;
            for child in case.children() {
                self.visit(&child);
            }
            if case_ast.is_arrow() {
                exit.extend(self.pending.iter().copied());
                fallthrough.clear();
            } else {
                fallthrough = self.pending.clone();
            }
        }
        exit.extend(fallthrough);
        let target = self.targets.pop().unwrap_or_default();
        exit.extend(target.breaks);
        // 可能一个分支都不进（没有 `default` 的普通 switch 语句）
        if !has_default {
            exit.extend(entry);
        }
        self.pending = exit;
    }

    /// `node` 里所有对这个变量的写入。
    fn writes_in(&self, node: &SyntaxNode) -> Pending {
        node.descendants()
            .filter_map(|n| self.writes.get(&n.text_range()).copied())
            .collect()
    }

    fn try_stmt(&mut self, node: &SyntaxNode) {
        let Some(it) = ast::TryStmt::cast(node.clone()) else {
            return;
        };
        if let Some(resources) = it.resource_list() {
            self.visit(resources.syntax());
        }
        let entry = self.pending.clone();
        let body = it.body().map(|b| b.syntax().clone());
        let after_body = self.branch(body.as_ref(), &entry);
        // 异常可能在 try 块的任何地方抛出
        let mut catch_entry = entry.clone();
        if let Some(body) = &body {
            catch_entry.extend(self.writes_in(body));
        }
        let mut exit = after_body;
        for catch in it.catch_clauses() {
            let block = catch.body().map(|b| b.syntax().clone());
            exit.extend(self.branch(block.as_ref(), &catch_entry));
        }
        if let Some(finally) = it.finally_clause().and_then(|f| f.body()) {
            // finally 在任何路径上都会执行（包括 return / throw 之后）
            let mut finally_entry = exit.clone();
            finally_entry.extend(catch_entry.iter().copied());
            for catch in it.catch_clauses() {
                finally_entry.extend(self.writes_in(catch.syntax()));
            }
            let after = self.branch(Some(finally.syntax()), &finally_entry);
            let finally_writes = self.writes_in(finally.syntax());
            // finally 正常结束后继续走 try / catch 结束时的状态（被 finally 覆盖的除外）
            exit = if finally_writes.is_empty() {
                exit
            } else {
                after
            };
        }
        self.pending = exit;
    }
}

/// 写入在源码里的位置：声明符的初值，或者赋值的右边。
struct Write {
    /// 用作键的范围（赋值表达式 / 声明符）
    key: TextRange,
    /// 被赋的值（报告的范围）
    value: SyntaxNode,
}

pub(crate) fn dead_stores(db: &dyn Db, bodies: &[FileBody], out: &mut Vec<Diagnostic>) {
    for body in bodies {
        let node = &body.node;
        let result = infer(db, body.id);
        let mut refs_by_local: FxHashMap<u32, FxHashSet<TextRange>> = FxHashMap::default();
        for (range, resolution) in result.resolutions() {
            if let Resolution::Local(id) = resolution {
                refs_by_local.entry(*id).or_default().insert(range);
            }
        }
        // 被 `x = ...` 赋值过的变量：只有初值、从没被重新赋值的变量不可能有死存储
        let mut reassigned: FxHashSet<u32> = FxHashSet::default();
        for assign in node.descendants().filter_map(ast::AssignExpr::cast) {
            if assign.op_token().is_none_or(|t| t.kind() != SyntaxKind::EQ) {
                continue;
            }
            let Some(ast::Expr::NameExpr(lhs)) = assign.lhs() else {
                continue;
            };
            if let Some(Resolution::Local(id)) = lhs
                .name_ref()
                .and_then(|r| result.resolution(r.syntax().text_range()))
            {
                reassigned.insert(*id);
            }
        }
        for (idx, local) in result.locals().iter().enumerate() {
            if !reassigned.contains(&(idx as u32))
                || !matches!(local.kind, LocalKind::Local | LocalKind::Param)
                || !node.text_range().contains_range(local.decl_range)
                || local.name.starts_with('_')
            {
                continue;
            }
            let Some(refs) = refs_by_local.get(&(idx as u32)) else {
                continue;
            };
            check_local(node, &local.name, local.decl_range, refs, out);
        }
    }
}

fn check_local(
    body: &SyntaxNode,
    name: &str,
    decl_range: TextRange,
    refs: &FxHashSet<TextRange>,
    out: &mut Vec<Diagnostic>,
) {
    // 变量的作用域：声明所在的块（形参是整个体）
    let decl_node = match body.covering_element(decl_range) {
        syntax::NodeOrToken::Node(n) => n,
        syntax::NodeOrToken::Token(t) => match t.parent() {
            Some(p) => p,
            None => return,
        },
    };
    let declarator = decl_node
        .ancestors()
        .find_map(ast::VariableDeclarator::cast)
        .filter(|d| {
            d.name()
                .is_some_and(|n| n.syntax().text_range() == decl_range)
        });
    let scope = match &declarator {
        Some(d) => {
            // 只处理块里的普通声明（for 的初始化部分、资源、switch 分支里的声明不处理）
            let Some(decl) = d
                .syntax()
                .parent()
                .filter(|p| p.kind() == SyntaxKind::LOCAL_VAR_DECL)
            else {
                return;
            };
            match decl.parent() {
                Some(block) if block.kind() == SyntaxKind::BLOCK => block,
                _ => return,
            }
        }
        None => {
            // 形参：所属的整个方法（lambda / catch 的形参前面已经排除）。匿名类、局部类里
            // 方法的形参也在外层的体里，作用域是那个方法而不是外层的体
            let Some(owner) = decl_node
                .ancestors()
                .find(|a| a.kind() == SyntaxKind::PARAM_LIST)
                .and_then(|list| list.parent())
                .filter(|owner| {
                    matches!(
                        owner.kind(),
                        SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL
                    )
                })
            else {
                return;
            };
            owner
        }
    };
    // 收集写入
    let mut writes: Vec<Write> = Vec::new();
    if let Some(d) = &declarator {
        if let Some(init) = d.initializer() {
            writes.push(Write {
                key: d.syntax().text_range(),
                value: init.syntax().clone(),
            });
        }
    }
    let declarator_write = writes.first().map(|w| w.key);
    let mut has_read = false;
    // 带标签的跳转、finally 里的读取：放弃
    let mut finally_clauses: Vec<TextRange> = Vec::new();
    // 变量的引用、初值所在的声明符和跳转语句的所有祖先（到作用域为止）
    let mut interesting: FxHashSet<(TextRange, SyntaxKind)> = FxHashSet::default();
    for n in scope.descendants() {
        let is_ref = n.kind() == SyntaxKind::NAME_REF && refs.contains(&n.text_range());
        let relevant = match n.kind() {
            SyntaxKind::VARIABLE_DECLARATOR => declarator_write == Some(n.text_range()),
            SyntaxKind::NAME_REF => is_ref,
            SyntaxKind::BREAK_STMT => {
                if ast::BreakStmt::cast(n.clone()).is_some_and(|b| b.label().is_some()) {
                    return;
                }
                true
            }
            SyntaxKind::CONTINUE_STMT => {
                if ast::ContinueStmt::cast(n.clone()).is_some_and(|c| c.label().is_some()) {
                    return;
                }
                true
            }
            SyntaxKind::RETURN_STMT | SyntaxKind::THROW_STMT | SyntaxKind::YIELD_STMT => true,
            SyntaxKind::FINALLY_CLAUSE => {
                finally_clauses.push(n.text_range());
                false
            }
            _ => false,
        };
        if relevant {
            for a in n.ancestors() {
                if !interesting.insert((a.text_range(), a.kind())) {
                    break;
                }
                if a == scope {
                    break;
                }
            }
        }
        if is_ref {
            if finally_clauses
                .iter()
                .any(|f| f.contains_range(n.text_range()))
            {
                return;
            }
            let is_plain_write_target = n
                .parent()
                .filter(|p| p.kind() == SyntaxKind::NAME_EXPR)
                .and_then(|p| {
                    let assign = p.parent().and_then(ast::AssignExpr::cast)?;
                    let is_lhs = assign.lhs()?.syntax() == &p;
                    let plain = assign.op_token()?.kind() == SyntaxKind::EQ;
                    Some(is_lhs && plain)
                })
                .unwrap_or(false);
            if !is_plain_write_target {
                has_read = true;
            }
        }
        let Some(assign) = ast::AssignExpr::cast(n.clone()) else {
            continue;
        };
        let target_is_var = assign.lhs().is_some_and(|lhs| {
            lhs.syntax().kind() == SyntaxKind::NAME_EXPR
                && lhs
                    .syntax()
                    .children()
                    .any(|c| refs.contains(&c.text_range()))
        });
        let plain = assign
            .op_token()
            .is_some_and(|t| t.kind() == SyntaxKind::EQ);
        if target_is_var && plain {
            if let Some(rhs) = assign.rhs() {
                writes.push(Write {
                    key: assign.syntax().text_range(),
                    value: rhs.syntax().clone(),
                });
            }
        }
    }
    // 从没读过的变量由"未使用的局部变量"负责
    if !has_read || writes.is_empty() {
        return;
    }
    let mut analysis = Analysis {
        refs,
        interesting,
        writes: writes
            .iter()
            .enumerate()
            .map(|(i, w)| (w.key, i as u32))
            .collect(),
        pending: Pending::default(),
        used: Pending::default(),
        targets: Vec::new(),
    };
    match &declarator {
        Some(d) => {
            // 从声明开始往后走完所在的块
            let Some(decl) = d.syntax().parent() else {
                return;
            };
            analysis.visit(d.syntax());
            for later in decl
                .children()
                .filter(|c| c.text_range().start() > d.syntax().text_range().end())
            {
                analysis.visit(&later);
            }
            let mut next = decl.next_sibling();
            while let Some(stmt) = next {
                analysis.visit(&stmt);
                next = stmt.next_sibling();
            }
        }
        None => analysis.visit(&scope),
    }
    for (idx, write) in writes.iter().enumerate() {
        if analysis.used.contains(&(idx as u32)) {
            continue;
        }
        let value_text = write.value.text().to_string();
        let shown = if value_text.len() > 40 || value_text.contains('\n') {
            "…".to_owned()
        } else {
            value_text
        };
        out.push(Diagnostic {
            range: write.value.text_range(),
            message: base_db::tr!(
                "赋给 `{name}` 的值 `{shown}` 从未被使用",
                "The value `{shown}` assigned to `{name}` is never used"
            ),
            severity: Severity::Warning,
            code: DiagnosticCode::UnusedAssignment,
            unused: true,
        });
    }
}
