//! 几种常见的错误写法（能安全改写的带修复，其余只报不改）：
//!
//! - 创建了异常对象却没有抛出：`new IllegalStateException("x");`；
//! - 在非 final 字段上同步：`synchronized (lock)`，字段被重新赋值后锁就换了；
//! - 对 volatile 字段做非原子的读改写：`count++`、`count += n`（在 `synchronized` 里的不报）；
//! - int 乘法 / 左移的结果直接当 long 用：`long ms = days * 24 * 3600 * 1000`，先按 int 溢出；
//! - 调用可变参数方法时多余地创建数组：`String.format("%s %s", new Object[] {a, b})`；
//! - `StringBuilder.append(a + b)`：拼接可以拆成连续的 `append`；
//! - 没有被 `break` / `continue` 用到的标签；
//! - `if` / `else` 两个分支完全相同；`if` / `else if` 链里重复的条件。

use base_db::{Db, SourceFile};
use hir::{
    class_data, infer, method_sig, InferenceResult, Modifiers, PrimitiveType, Resolution, Ty,
};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{redundancy::RedundancyFix, Diagnostic, DiagnosticCode, Severity};

fn fix(
    range: TextRange,
    severity: Severity,
    code: DiagnosticCode,
    message: String,
    label: &'static str,
    edits: Vec<(TextRange, String)>,
) -> RedundancyFix {
    RedundancyFix {
        diagnostic: Diagnostic {
            range,
            message,
            severity,
            code,
            unused: false,
        },
        label,
        edits,
    }
}

/// 去掉空白和注释后的 token 序列，用来比较两段代码是否相同。
fn normalized(node: &SyntaxNode) -> String {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .map(|t| t.text().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn class_name(db: &dyn Db, ty: Option<&Ty>) -> Option<String> {
    match ty? {
        Ty::Class(class, _) => class_data(db, *class).map(|d| d.binary_name.to_string()),
        _ => None,
    }
}

fn is_string(db: &dyn Db, ty: Option<&Ty>) -> bool {
    class_name(db, ty).as_deref() == Some("java/lang/String")
}

fn is_subtype_of(db: &dyn Db, ty: Option<&Ty>, binary_name: &str) -> bool {
    let (Some(ty @ Ty::Class(..)), Some(target)) = (ty, hir::lookup_class(db, binary_name)) else {
        return false;
    };
    hir::is_subtype(db, ty, &Ty::class(target))
}

fn is_prim(ty: Option<&Ty>, prim: PrimitiveType) -> bool {
    ty.and_then(Ty::as_primitive) == Some(prim)
}

fn strip_parens(expr: &SyntaxNode) -> SyntaxNode {
    let mut expr = expr.clone();
    while let Some(inner) = ast::ParenExpr::cast(expr.clone()).and_then(|p| p.expr()) {
        expr = inner.syntax().clone();
    }
    expr
}

/// 表达式（去掉括号）指向的字段的修饰符。
fn field_flags(db: &dyn Db, result: &InferenceResult, expr: &SyntaxNode) -> Option<Modifiers> {
    let expr = strip_parens(expr);
    let name = match ast::Expr::cast(expr)? {
        ast::Expr::NameExpr(e) => e.name_ref()?,
        ast::Expr::FieldAccess(e) => e.name_ref()?,
        _ => return None,
    };
    let Some(Resolution::Field(field)) = result.resolution(name.syntax().text_range()) else {
        return None;
    };
    class_data(db, field.class).and_then(|d| d.fields.get(field.idx as usize).map(|f| f.flags))
}

fn op_kind(node: &SyntaxNode) -> Option<SyntaxKind> {
    match node.kind() {
        SyntaxKind::BINARY_EXPR => ast::BinaryExpr::cast(node.clone())?.op_kind(),
        SyntaxKind::ASSIGN_EXPR => Some(ast::AssignExpr::cast(node.clone())?.op_token()?.kind()),
        SyntaxKind::UNARY_EXPR => Some(ast::UnaryExpr::cast(node.clone())?.op_token()?.kind()),
        SyntaxKind::POSTFIX_EXPR => Some(ast::PostfixExpr::cast(node.clone())?.op_token()?.kind()),
        _ => None,
    }
}

fn is_multiplicative(node: &SyntaxNode) -> bool {
    node.kind() == SyntaxKind::BINARY_EXPR
        && matches!(op_kind(node), Some(SyntaxKind::STAR | SyntaxKind::SHL))
}

/// 需要类型信息的候选。
enum Candidate {
    ThrowableNotThrown(ast::NewExpr, SyntaxNode),
    SyncOnField(ast::Expr),
    VolatileUpdate(SyntaxNode, SyntaxNode),
    IntToLong(ast::BinaryExpr),
    VarargsArray(ast::ArgList, ast::ArrayCreationExpr),
    AppendConcat(ast::MethodCall),
}

/// 文件里可能有需要类型信息的候选（便宜的文本预检）。
fn may_have_typed_candidates(text: &str) -> bool {
    text.contains("new ")
        || text.contains("synchronized")
        || text.contains("volatile")
        || text.contains('*')
        || text.contains("<<")
        || text.contains("append(")
}

pub(crate) fn check(
    db: &dyn Db,
    file: SourceFile,
    root: &SyntaxNode,
    out: &mut Vec<RedundancyFix>,
) {
    let text: &str = file.text(db);
    let typed = may_have_typed_candidates(text);
    let volatile = text.contains("volatile");
    let mut candidates = Vec::new();
    for node in root.descendants() {
        match node.kind() {
            SyntaxKind::LABELED_STMT => unused_label(&node, out),
            SyntaxKind::IF_STMT => if_branches(text, &node, out),
            _ if !typed => {}
            SyntaxKind::EXPR_STMT => {
                // `case 1 -> new X(...);`：switch 规则右边的表达式是结果值，不是丢弃的语句
                let rule_result = node.parent().is_some_and(|p| {
                    p.kind() == SyntaxKind::SWITCH_CASE
                        && p.children_with_tokens()
                            .any(|t| t.kind() == SyntaxKind::ARROW)
                });
                let new = node.children().find_map(ast::NewExpr::cast);
                if let Some(new) = new.filter(|n| n.body().is_none() && !rule_result) {
                    candidates.push(Candidate::ThrowableNotThrown(new, node));
                }
            }
            SyntaxKind::SYNCHRONIZED_STMT => {
                if let Some(lock) = ast::SynchronizedStmt::cast(node).and_then(|s| s.lock()) {
                    candidates.push(Candidate::SyncOnField(lock));
                }
            }
            SyntaxKind::POSTFIX_EXPR | SyntaxKind::UNARY_EXPR if volatile => {
                if matches!(
                    op_kind(&node),
                    Some(SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS)
                ) {
                    if let Some(target) = node.children().find(|c| ast::Expr::can_cast(c.kind())) {
                        candidates.push(Candidate::VolatileUpdate(node, target));
                    }
                }
            }
            SyntaxKind::ASSIGN_EXPR if volatile => {
                let compound = op_kind(&node).is_some_and(|k| k != SyntaxKind::EQ);
                let lhs = ast::AssignExpr::cast(node.clone()).and_then(|a| a.lhs());
                if let (true, Some(lhs)) = (compound, lhs) {
                    candidates.push(Candidate::VolatileUpdate(node, lhs.syntax().clone()));
                }
            }
            SyntaxKind::BINARY_EXPR if is_multiplicative(&node) => {
                // 只看最外层的乘法（`a * b * c` 报一次）
                let outer = node
                    .ancestors()
                    .skip(1)
                    .find(|a| a.kind() != SyntaxKind::PAREN_EXPR)
                    .is_some_and(|a| is_multiplicative(&a));
                if !outer && !is_constant_expr(&node) {
                    candidates.extend(ast::BinaryExpr::cast(node).map(Candidate::IntToLong));
                }
            }
            SyntaxKind::ARG_LIST => {
                let Some(list) = ast::ArgList::cast(node) else {
                    continue;
                };
                if let Some(ast::Expr::ArrayCreationExpr(array)) = list.args().last() {
                    if array.initializer().is_some() && array.dims().all(|d| d.expr().is_none()) {
                        candidates.push(Candidate::VarargsArray(list, array));
                    }
                }
            }
            SyntaxKind::METHOD_CALL => {
                let Some(call) = ast::MethodCall::cast(node) else {
                    continue;
                };
                let single_concat = call.arg_list().is_some_and(|l| {
                    let args: Vec<_> = l.args().collect();
                    matches!(args.as_slice(), [ast::Expr::BinaryExpr(b)] if b.op_kind() == Some(SyntaxKind::PLUS))
                });
                if single_concat && call.name_ref().is_some_and(|n| n.text() == "append") {
                    candidates.push(Candidate::AppendConcat(call));
                }
            }
            _ => {}
        }
    }
    if candidates.is_empty() {
        return;
    }
    let sema = hir::Semantics::new(db);
    for candidate in candidates {
        let node = match &candidate {
            Candidate::ThrowableNotThrown(_, stmt) => stmt.clone(),
            Candidate::SyncOnField(lock) => lock.syntax().clone(),
            Candidate::VolatileUpdate(node, _) => node.clone(),
            Candidate::IntToLong(binary) => binary.syntax().clone(),
            Candidate::VarargsArray(list, _) => list.syntax().clone(),
            Candidate::AppendConcat(call) => call.syntax().clone(),
        };
        let Some(body) = sema.body_of(file, &node) else {
            continue;
        };
        let result = infer(db, body);
        match candidate {
            Candidate::ThrowableNotThrown(new, stmt) => {
                throwable_not_thrown(db, &result, &new, &stmt, out)
            }
            Candidate::SyncOnField(lock) => sync_on_non_final(db, &result, &lock, out),
            Candidate::VolatileUpdate(node, target) => {
                non_atomic_volatile(db, &result, &node, &target, out)
            }
            Candidate::IntToLong(binary) => int_to_long(db, &result, &binary, out),
            Candidate::VarargsArray(list, array) => {
                redundant_array_creation(db, &result, &list, &array, out)
            }
            Candidate::AppendConcat(call) => append_concatenation(db, &result, &call, out),
        }
    }
}

/// `label:` 没有被任何 `break label` / `continue label` 用到
fn unused_label(node: &SyntaxNode, out: &mut Vec<RedundancyFix>) {
    let Some(stmt) = ast::LabeledStmt::cast(node.clone()) else {
        return;
    };
    let (Some(label), Some(body)) = (stmt.label(), stmt.body()) else {
        return;
    };
    let name = label.syntax().text().to_string();
    let used = body.syntax().descendants().any(|n| {
        let target = match n.kind() {
            SyntaxKind::BREAK_STMT => ast::BreakStmt::cast(n).and_then(|b| b.label()),
            SyntaxKind::CONTINUE_STMT => ast::ContinueStmt::cast(n).and_then(|c| c.label()),
            _ => None,
        };
        target.is_some_and(|t| t.syntax().text() == name.as_str())
    });
    if used {
        return;
    }
    // 删掉 `label:` 和它后面的空白
    let start = stmt.syntax().text_range().start();
    let end = body.syntax().text_range().start();
    out.push(fix(
        label.syntax().text_range(),
        Severity::Warning,
        DiagnosticCode::UnusedLabel,
        base_db::tr!("没有用到的标签 `{}`", "Unused label `{}`", name),
        base_db::trs!("删除标签", "Remove label"),
        vec![(TextRange::new(start, end), String::new())],
    ));
}

/// 表达式里没有调用、赋值、自增自减、对象创建（重复出现时一定是同一个值）。
fn is_pure(expr: &SyntaxNode) -> bool {
    !expr.descendants().any(|n| match n.kind() {
        SyntaxKind::METHOD_CALL
        | SyntaxKind::NEW_EXPR
        | SyntaxKind::ARRAY_CREATION_EXPR
        | SyntaxKind::ASSIGN_EXPR
        | SyntaxKind::POSTFIX_EXPR
        | SyntaxKind::LAMBDA_EXPR
        | SyntaxKind::SWITCH_EXPR => true,
        SyntaxKind::UNARY_EXPR => {
            matches!(
                op_kind(&n),
                Some(SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS)
            )
        }
        _ => false,
    })
}

/// 偏移所在行开头的空白。
fn line_indent(text: &str, offset: usize) -> &str {
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[line_start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// 用分支替换整个 `if` 语句：代码块去掉花括号，内容减一级缩进。
fn collapsed_branch(text: &str, if_stmt: &SyntaxNode, branch: &ast::Stmt) -> Option<String> {
    let ast::Stmt::Block(block) = branch else {
        return Some(branch.syntax().text().to_string());
    };
    let tokens: Vec<_> = block
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .collect();
    let open = tokens.iter().find(|t| t.kind() == SyntaxKind::L_CURLY)?;
    let close = tokens
        .iter()
        .rev()
        .find(|t| t.kind() == SyntaxKind::R_CURLY)?;
    let inner = text
        .get(usize::from(open.text_range().end())..usize::from(close.text_range().start()))?
        .trim();
    // 块里声明的变量搬出来后作用域会变大，可能和后面的声明冲突
    if block.syntax().children().any(|c| {
        matches!(
            c.kind(),
            SyntaxKind::LOCAL_VAR_DECL | SyntaxKind::CLASS_DECL | SyntaxKind::RECORD_DECL
        )
    }) {
        return None;
    }
    let outer = line_indent(text, usize::from(if_stmt.text_range().start()));
    let first = inner_start(text, open.text_range().end().into());
    let extra = line_indent(text, first)
        .strip_prefix(outer)
        .unwrap_or("")
        .to_owned();
    let mut lines = inner.lines();
    let mut out = lines.next()?.to_owned();
    for line in lines {
        out.push('\n');
        out.push_str(line.strip_prefix(extra.as_str()).unwrap_or(line));
    }
    Some(out)
}

/// 偏移之后第一个非空白字符的位置。
fn inner_start(text: &str, offset: usize) -> usize {
    offset + (text[offset..].len() - text[offset..].trim_start().len())
}

fn if_branches(text: &str, node: &SyntaxNode, out: &mut Vec<RedundancyFix>) {
    let Some(if_stmt) = ast::IfStmt::cast(node.clone()) else {
        return;
    };
    // `else if` 链只从链头开始看一次
    let is_chain_tail = node
        .parent()
        .and_then(ast::IfStmt::cast)
        .and_then(|parent| parent.else_branch())
        .is_some_and(|e| e.syntax() == node);
    if !is_chain_tail {
        duplicate_conditions(&if_stmt, out);
    }
    let (Some(then_branch), Some(else_branch)) = (if_stmt.then_branch(), if_stmt.else_branch())
    else {
        return;
    };
    if matches!(else_branch, ast::Stmt::IfStmt(_)) {
        return;
    }
    let then_text = normalized(then_branch.syntax());
    if then_text.is_empty() || then_text == "{ }" || then_text != normalized(else_branch.syntax()) {
        return;
    }
    let Some(else_kw) = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == SyntaxKind::ELSE_KW)
    else {
        return;
    };
    // 条件有副作用时只报不改；分支里有声明时搬出来会改变作用域，也不改
    let replacement = if_stmt
        .condition()
        .filter(|cond| is_pure(cond.syntax()))
        .and_then(|_| collapsed_branch(text, node, &then_branch));
    let edits = replacement.map_or_else(Vec::new, |r| vec![(node.text_range(), r)]);
    out.push(fix(
        else_kw.text_range(),
        Severity::Warning,
        DiagnosticCode::DuplicateBranches,
        base_db::trs!(
            "`if` 和 `else` 两个分支完全相同",
            "`if` and `else` branches are identical"
        )
        .to_owned(),
        base_db::trs!("合并相同的分支", "Collapse identical branches"),
        edits,
    ));
}

/// `if (a) ... else if (a) ...`：后面的分支永远走不到
fn duplicate_conditions(head: &ast::IfStmt, out: &mut Vec<RedundancyFix>) {
    let mut seen: Vec<String> = Vec::new();
    let mut current = Some(head.clone());
    while let Some(stmt) = current {
        if let Some(cond) = stmt.condition() {
            if !is_pure(cond.syntax()) {
                return;
            }
            let text = normalized(cond.syntax());
            if seen.contains(&text) {
                out.push(fix(
                    cond.syntax().text_range(),
                    Severity::Warning,
                    DiagnosticCode::DuplicateCondition,
                    base_db::trs!(
                        "重复的条件：前面的分支已经判断过，这个分支永远不会执行",
                        "Duplicate condition: an earlier branch already tests it, so this branch never runs"
                    )
                    .to_owned(),
                    "",
                    Vec::new(),
                ));
            } else {
                seen.push(text);
            }
        }
        current = match stmt.else_branch() {
            Some(ast::Stmt::IfStmt(next)) => Some(next),
            _ => None,
        };
    }
}

/// `new XxxException(...);` 单独成句
fn throwable_not_thrown(
    db: &dyn Db,
    result: &InferenceResult,
    new: &ast::NewExpr,
    stmt: &SyntaxNode,
    out: &mut Vec<RedundancyFix>,
) {
    if !is_subtype_of(
        db,
        result.type_of(new.syntax().text_range()),
        "java/lang/Throwable",
    ) {
        return;
    }
    out.push(fix(
        new.ty()
            .map_or(new.syntax().text_range(), |t| t.syntax().text_range()),
        Severity::Warning,
        DiagnosticCode::ThrowableNotThrown,
        base_db::trs!(
            "创建了异常对象却没有抛出",
            "Exception object is created but not thrown"
        )
        .to_owned(),
        base_db::trs!("加上 `throw`", "Add `throw`"),
        vec![(
            TextRange::empty(stmt.text_range().start()),
            "throw ".to_owned(),
        )],
    ));
}

fn sync_on_non_final(
    db: &dyn Db,
    result: &InferenceResult,
    lock: &ast::Expr,
    out: &mut Vec<RedundancyFix>,
) {
    let Some(flags) = field_flags(db, result, lock.syntax()) else {
        return;
    };
    if flags.is_final() {
        return;
    }
    out.push(fix(
        lock.syntax().text_range(),
        Severity::Warning,
        DiagnosticCode::SyncOnNonFinalField,
        base_db::trs!(
            "在非 final 字段上同步：字段被重新赋值后，不同线程拿到的可能是不同的锁",
            "Synchronization on a non-final field: after reassignment threads may lock different objects"
        )
        .to_owned(),
        "",
        Vec::new(),
    ));
}

/// 方法体里调用了 `lock()` / `tryLock()` / `lockInterruptibly()`（显式锁保护的写操作）。
fn takes_explicit_lock(body: &SyntaxNode) -> bool {
    body.descendants()
        .filter_map(ast::MethodCall::cast)
        .any(|call| {
            call.name_ref().is_some_and(|n| {
                matches!(n.text().as_str(), "lock" | "tryLock" | "lockInterruptibly")
            })
        })
}

fn inside_synchronized(node: &SyntaxNode) -> bool {
    for ancestor in node.ancestors() {
        match ancestor.kind() {
            SyntaxKind::SYNCHRONIZED_STMT => return true,
            SyntaxKind::METHOD_DECL => {
                // `synchronized` 方法，或者标了 `@GuardedBy`（由调用方持锁）
                let guarded = ast::MethodDecl::cast(ancestor.clone())
                    .and_then(|m| m.modifier_list())
                    .is_some_and(|mods| {
                        mods.has(SyntaxKind::SYNCHRONIZED_KW)
                            || mods.annotations().any(|a| {
                                a.name()
                                    .and_then(|n| n.segments().last().map(|s| s.to_string()))
                                    .is_some_and(|n| n == "GuardedBy")
                            })
                    });
                return guarded || takes_explicit_lock(&ancestor);
            }
            // lambda / 匿名类可能在别的线程里执行，外层的锁不算
            SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY => {
                return takes_explicit_lock(&ancestor)
            }
            _ => {}
        }
    }
    false
}

fn non_atomic_volatile(
    db: &dyn Db,
    result: &InferenceResult,
    node: &SyntaxNode,
    target: &SyntaxNode,
    out: &mut Vec<RedundancyFix>,
) {
    let Some(flags) = field_flags(db, result, target) else {
        return;
    };
    if !flags.contains(Modifiers::VOLATILE) || inside_synchronized(node) {
        return;
    }
    out.push(fix(
        node.text_range(),
        Severity::Warning,
        DiagnosticCode::NonAtomicVolatile,
        base_db::trs!(
            "对 volatile 字段的读改写不是原子操作",
            "Non-atomic read-modify-write of a volatile field"
        )
        .to_owned(),
        "",
        Vec::new(),
    ));
}

/// 只由字面量和运算符组成的表达式（编译期就能算出来）
fn is_constant_expr(expr: &SyntaxNode) -> bool {
    expr.descendants().all(|n| {
        matches!(
            n.kind(),
            SyntaxKind::LITERAL
                | SyntaxKind::BINARY_EXPR
                | SyntaxKind::PAREN_EXPR
                | SyntaxKind::UNARY_EXPR
        )
    })
}

fn declared_type_is_long(ty: Option<ast::Type>) -> bool {
    ty.is_some_and(|t| t.syntax().text() == "long")
}

fn int_to_long(
    db: &dyn Db,
    result: &InferenceResult,
    binary: &ast::BinaryExpr,
    out: &mut Vec<RedundancyFix>,
) {
    let node = binary.syntax();
    if !is_prim(result.type_of(node.text_range()), PrimitiveType::Int) {
        return;
    }
    // 编译期常量（`24 * ONE_HOUR`）的值是确定的，不按溢出风险报
    if hir::expr_constant(db, result, node).is_some() {
        return;
    }
    let outer = node
        .ancestors()
        .take_while(|a| a == node || a.kind() == SyntaxKind::PAREN_EXPR)
        .last()
        .unwrap_or_else(|| node.clone());
    // 结果直接用在 long 的位置：long 变量的初值 / 赋值、返回 long 的方法的 return
    let Some(context) = outer.parent() else {
        return;
    };
    let widened = match context.kind() {
        SyntaxKind::VARIABLE_DECLARATOR => {
            let local = context
                .children()
                .find(|c| c.kind() == SyntaxKind::NAME)
                .and_then(|n| result.local_at_decl(n.text_range()));
            match local {
                Some(id) => is_prim(Some(&result.local(id).ty), PrimitiveType::Long),
                None => {
                    let parent = context.parent();
                    let ty = parent
                        .as_ref()
                        .and_then(|p| p.children().find_map(ast::Type::cast));
                    // `long a[] = ...` 之类的不管
                    declared_type_is_long(ty)
                        && !context
                            .children_with_tokens()
                            .any(|t| t.kind() == SyntaxKind::L_BRACK)
                }
            }
        }
        SyntaxKind::ASSIGN_EXPR => ast::AssignExpr::cast(context.clone())
            .filter(|a| a.op_token().is_some_and(|t| t.kind() == SyntaxKind::EQ))
            .and_then(|a| a.lhs())
            .is_some_and(|lhs| {
                is_prim(
                    result.type_of(lhs.syntax().text_range()),
                    PrimitiveType::Long,
                )
            }),
        SyntaxKind::RETURN_STMT => {
            let owner = context
                .ancestors()
                .find(|a| matches!(a.kind(), SyntaxKind::METHOD_DECL | SyntaxKind::LAMBDA_EXPR));
            owner
                .and_then(ast::MethodDecl::cast)
                .is_some_and(|m| declared_type_is_long(m.return_type()))
        }
        _ => false,
    };
    if !widened {
        return;
    }
    // 修复：最左边的操作数先转成 long
    let mut first = binary.lhs().map(|l| l.syntax().clone());
    while let Some(inner) = first.clone().filter(is_multiplicative) {
        first = ast::BinaryExpr::cast(inner)
            .and_then(|b| b.lhs())
            .map(|l| l.syntax().clone());
    }
    let Some(first) = first else {
        return;
    };
    let edits = match first.kind() {
        // 十进制整数字面量直接加后缀
        SyntaxKind::LITERAL
            if first
                .text()
                .to_string()
                .chars()
                .all(|c| c.is_ascii_digit() || c == '_') =>
        {
            vec![(TextRange::empty(first.text_range().end()), "L".to_owned())]
        }
        SyntaxKind::LITERAL
        | SyntaxKind::NAME_EXPR
        | SyntaxKind::FIELD_ACCESS
        | SyntaxKind::METHOD_CALL
        | SyntaxKind::PAREN_EXPR
        | SyntaxKind::ARRAY_ACCESS => {
            vec![(
                TextRange::empty(first.text_range().start()),
                "(long) ".to_owned(),
            )]
        }
        _ => vec![
            (
                TextRange::empty(first.text_range().start()),
                "(long) (".to_owned(),
            ),
            (TextRange::empty(first.text_range().end()), ")".to_owned()),
        ],
    };
    out.push(fix(
        node.text_range(),
        Severity::Warning,
        DiagnosticCode::IntMultiplicationToLong,
        base_db::trs!(
            "int 运算的结果当 long 用：运算本身可能已经溢出",
            "int arithmetic used as long: it may overflow before the conversion"
        )
        .to_owned(),
        base_db::trs!("先转成 long 再运算", "Convert to long before the operation"),
        edits,
    ));
}

fn redundant_array_creation(
    db: &dyn Db,
    result: &InferenceResult,
    list: &ast::ArgList,
    array: &ast::ArrayCreationExpr,
    out: &mut Vec<RedundancyFix>,
) {
    let Some(init) = array.initializer() else {
        return;
    };
    let Some(method) = result
        .call_info(list.syntax().text_range())
        .and_then(|i| i.chosen)
    else {
        return;
    };
    let sig = method_sig(db, method);
    if !sig.varargs || sig.params.len() != list.args().count() {
        return;
    }
    // 数组类型必须正好是可变参数的类型（`new Object[] {...}` 传给 `Object...`）
    let array_ty = result.type_of(array.syntax().text_range());
    let Some(param) = sig.params.last() else {
        return;
    };
    if param.contains_unknown() || array_ty != Some(param) || !matches!(param, Ty::Array(_)) {
        return;
    }
    let elements: Vec<ast::Expr> = init.exprs().collect();
    // 去掉数组后实参个数变了，同名、形参个数正好相同的固定参数重载可能把调用抢走
    let arity = sig.params.len() - 1 + elements.len();
    let overloaded = class_data(db, method.class).is_some_and(|data| {
        let name = &data.methods[method.idx as usize].name;
        data.methods.iter().enumerate().any(|(i, m)| {
            i != method.idx as usize
                && &m.name == name
                && !m.is_varargs()
                && m.params.len() == arity
        })
    });
    if overloaded {
        return;
    }
    // 空数组、一个 null、一个本身是数组的元素：去掉数组后含义会变或者写不出来
    let ambiguous = match elements.as_slice() {
        [] => true,
        [only] => {
            only.syntax().text() == "null"
                || matches!(only, ast::Expr::ArrayInitializer(_))
                || matches!(
                    result.type_of(only.syntax().text_range()),
                    Some(Ty::Array(_)) | None
                )
        }
        _ => elements
            .iter()
            .any(|e| matches!(e, ast::Expr::ArrayInitializer(_))),
    };
    if ambiguous {
        return;
    }
    let replacement = elements
        .iter()
        .map(|e| e.syntax().text().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    out.push(fix(
        array.syntax().text_range(),
        Severity::Hint,
        DiagnosticCode::RedundantArrayCreation,
        base_db::trs!(
            "调用可变参数方法时多余地创建了数组",
            "Redundant array creation for a varargs call"
        )
        .to_owned(),
        base_db::trs!("去掉数组创建", "Remove array creation"),
        vec![(array.syntax().text_range(), replacement)],
    ));
}

fn append_concatenation(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) {
    let Some(receiver) = call.receiver() else {
        return;
    };
    if !matches!(
        class_name(db, result.type_of(receiver.syntax().text_range())).as_deref(),
        Some("java/lang/StringBuilder" | "java/lang/StringBuffer")
    ) {
        return;
    }
    let Some(ast::Expr::BinaryExpr(concat)) = call.arg_list().and_then(|l| l.args().next()) else {
        return;
    };
    // 展开左结合的 `+` 链；每一步都必须是字符串拼接，最左边的操作数也必须是字符串，
    // 否则 `1 + 2 + "x"` 前面的 `+` 是数值加法
    let mut operands = Vec::new();
    let mut current = concat;
    loop {
        if current.op_kind() != Some(SyntaxKind::PLUS)
            || !is_string(db, result.type_of(current.syntax().text_range()))
        {
            return;
        }
        let (Some(lhs), Some(rhs)) = (current.lhs(), current.rhs()) else {
            return;
        };
        operands.push(rhs);
        match lhs {
            ast::Expr::BinaryExpr(inner) if inner.op_kind() == Some(SyntaxKind::PLUS) => {
                current = inner;
            }
            other => {
                if !is_string(db, result.type_of(other.syntax().text_range())) {
                    return;
                }
                operands.push(other);
                break;
            }
        }
    }
    operands.reverse();
    let Some(name) = call.name_ref() else {
        return;
    };
    let chained = operands
        .iter()
        .map(|o| format!("append({})", o.syntax().text()))
        .collect::<Vec<_>>()
        .join(".");
    let range = TextRange::new(
        name.syntax().text_range().start(),
        call.syntax().text_range().end(),
    );
    out.push(fix(
        name.syntax().text_range(),
        Severity::Hint,
        DiagnosticCode::AppendConcatenation,
        base_db::trs!(
            "`append()` 的实参是字符串拼接，可以拆成连续的 `append()`",
            "String concatenation as `append()` argument can be chained `append()` calls"
        )
        .to_owned(),
        base_db::trs!("拆成连续的 `append()`", "Chain `append()` calls"),
        vec![(range, chained)],
    ));
}
