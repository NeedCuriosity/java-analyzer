//! 结构化之后的整理：finally / synchronized、三元表达式、临时变量内联、数组初始化、
//! 字符串拼接、布尔和字符常量、复合赋值、assert、构造器里编译器生成的语句、
//! 循环形态（while / do-while / for / for-each）。

use classfile::{access, code::CodeClass, signature::JType};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    cfg::negate,
    ir::*,
    translate::{build_concat, field_desc, method_desc},
    types::{self, TypeCx},
    Fail, MethodCx,
};

// ---------------------------------------------------------------------------
// 通用遍历

/// 访问每个语句块，含 lambda 体里的。
fn each_block_deep(block: &mut Vec<Stmt>, f: &mut dyn FnMut(&mut Vec<Stmt>)) {
    for stmt in block.iter_mut() {
        stmt.exprs_mut(&mut |e| each_lambda_block(e, f));
        stmt.blocks_mut(&mut |b| each_block_deep(b, f));
    }
    f(block);
}

fn each_lambda_block(e: &mut Expr, f: &mut dyn FnMut(&mut Vec<Stmt>)) {
    if let Expr::Lambda { body, .. } = e {
        if let LambdaBody::Block(b) = &mut **body {
            each_block_deep(b, f);
        }
    }
    e.children_mut(&mut |c| each_lambda_block(c, f));
}

#[derive(Default, Clone, Copy)]
struct Counts {
    defs: u32,
    uses: u32,
    /// 在 lambda 体或匿名类捕获里的使用
    deferred: u32,
}

fn count_expr(e: &Expr, counts: &mut FxHashMap<VarId, Counts>, deferred: bool) {
    match e {
        Expr::Var(v) => {
            let c = counts.entry(*v).or_default();
            if deferred {
                c.deferred += 1;
            } else {
                c.uses += 1;
            }
        }
        Expr::Assign(lhs, rhs) => {
            if let Expr::Var(v) = &**lhs {
                counts.entry(*v).or_default().defs += 1;
            } else {
                count_expr(lhs, counts, deferred);
            }
            count_expr(rhs, counts, deferred);
        }
        Expr::CompoundAssign(_, lhs, rhs) => {
            if let Expr::Var(v) = &**lhs {
                counts.entry(*v).or_default().defs += 1;
            }
            count_expr(lhs, counts, deferred);
            count_expr(rhs, counts, deferred);
        }
        Expr::IncDec { target, .. } => {
            if let Expr::Var(v) = &**target {
                counts.entry(*v).or_default().defs += 1;
            }
            count_expr(target, counts, deferred);
        }
        Expr::Lambda { body, .. } => match &**body {
            LambdaBody::Expr(b) => count_expr(b, counts, true),
            LambdaBody::Block(stmts) => count_block(stmts, counts, true),
        },
        Expr::AnonNew(anon) => {
            for a in &anon.super_args {
                count_expr(a, counts, deferred);
            }
            for c in &anon.captured {
                count_expr(c, counts, true);
            }
        }
        _ => e.children(&mut |c| count_expr(c, counts, deferred)),
    }
}

fn count_block(block: &[Stmt], counts: &mut FxHashMap<VarId, Counts>, deferred: bool) {
    for stmt in block {
        match stmt {
            Stmt::Local { var, init } => {
                if init.is_some() {
                    counts.entry(*var).or_default().defs += 1;
                }
            }
            Stmt::Loop {
                kind: LoopKind::ForEach { var, .. },
                ..
            } => counts.entry(*var).or_default().defs += 1,
            Stmt::Try { catches, .. } => {
                for c in catches {
                    counts.entry(c.var).or_default().defs += 1;
                }
            }
            _ => {}
        }
        stmt.exprs(&mut |e| count_expr(e, counts, deferred));
        stmt.blocks(&mut |b| count_block(b, counts, deferred));
    }
}

fn counts_of(block: &[Stmt]) -> FxHashMap<VarId, Counts> {
    let mut counts = FxHashMap::default();
    count_block(block, &mut counts, false);
    counts
}

// ---------------------------------------------------------------------------
// finally 和 synchronized

/// 把 `stmts` 里的变量（只算在里面第一次被赋值的）按出现顺序重新编号，用于比较两份副本。
fn canonical(stmts: &[Stmt]) -> Vec<Stmt> {
    let mut assigned: Vec<VarId> = Vec::new();
    for s in stmts {
        let mut st = s.clone();
        let mut collect = |e: &mut Expr| {
            if let Expr::Assign(lhs, _) = e {
                if let Expr::Var(v) = **lhs {
                    if !assigned.contains(&v) {
                        assigned.push(v);
                    }
                }
            }
        };
        rewrite_exprs(std::slice::from_mut(&mut st), &mut collect);
        if let Stmt::Local { var, .. } = s {
            if !assigned.contains(var) {
                assigned.push(*var);
            }
        }
    }
    let mut out = stmts.to_vec();
    let map = |e: &mut Expr| {
        if let Expr::Var(v) = e {
            if let Some(i) = assigned.iter().position(|a| a == v) {
                *v = u32::MAX - i as u32;
            }
        }
    };
    let mut map = map;
    rewrite_exprs(&mut out, &mut map);
    out
}

fn same_stmts(a: &[Stmt], b: &[Stmt]) -> bool {
    a.len() == b.len() && canonical(a) == canonical(b)
}

/// 从 try 体里去掉 finally 代码的副本：正常落下的末尾、以及每个跳出 try 的语句之前。
/// `inner_loops` 表示当前在 try 内部的循环 / switch 里（不带标号的 break / continue 不离开 try）。
fn strip_copies(
    block: &mut Vec<Stmt>,
    fin: &[Stmt],
    at_end: bool,
    inner: u32,
    inner_labels: &mut Vec<LabelId>,
) -> bool {
    let n = fin.len();
    if n == 0 {
        return true;
    }
    let mut i = 0;
    while i < block.len() {
        let leaves = match &block[i] {
            Stmt::Return(_) => true,
            Stmt::Break(None) | Stmt::Continue(None) => inner == 0,
            Stmt::Break(Some(l)) | Stmt::Continue(Some(l)) => !inner_labels.contains(l),
            _ => false,
        };
        if leaves {
            // return 的值先存进了临时变量时是 `t = v; F; return t;`，副本同样紧挨在跳转之前；
            // 副本和跳转之间还可能夹着栈变量之间的复制 `t2 = t1;`（F 不会碰它们）
            let mut end = i;
            while end > 0 && end >= n {
                if same_stmts(&block[end - n..end], fin) {
                    break;
                }
                match &block[end - 1] {
                    // 外层 synchronized 的 monitorexit 在 finally 副本之后，留给外层处理
                    Stmt::MonitorExit(Expr::Var(v))
                        if !fin
                            .iter()
                            .any(|s| matches!(s, Stmt::MonitorExit(Expr::Var(w)) if w == v)) =>
                    {
                        end -= 1
                    }
                    Stmt::Expr(Expr::Assign(lhs, rhs))
                        if matches!((&**lhs, &**rhs), (Expr::Var(a), Expr::Var(b))
                            if !refs_in(fin, *a) && !refs_in(fin, *b)) =>
                    {
                        end -= 1
                    }
                    _ => return false,
                }
            }
            if end < n {
                return false;
            }
            let start = end - n;
            block.drain(start..end);
            i = i - n + 1;
            continue;
        }
        let ok = match &mut block[i] {
            Stmt::Loop { label, body, kind } => {
                if let Some(l) = label {
                    inner_labels.push(*l);
                }
                let mut ok = strip_copies(body, fin, false, inner + 1, inner_labels);
                if let LoopKind::For { init, .. } = kind {
                    ok &= strip_copies(init, fin, false, inner, inner_labels);
                }
                ok
            }
            Stmt::Switch { label, cases, .. } => {
                if let Some(l) = label {
                    inner_labels.push(*l);
                }
                let mut ok = true;
                for c in cases.iter_mut() {
                    ok &= strip_copies(&mut c.body, fin, false, inner + 1, inner_labels);
                }
                ok
            }
            other => {
                let mut ok = true;
                other.blocks_mut(&mut |b| ok &= strip_copies(b, fin, false, inner, inner_labels));
                ok
            }
        };
        if !ok {
            return false;
        }
        i += 1;
    }
    if at_end && !ends_with_jump(block) {
        if block.len() < n || !same_stmts(&block[block.len() - n..], fin) {
            return false;
        }
        block.truncate(block.len() - n);
    }
    true
}

/// `Try { catches: [catch (任意 t) { F; throw t; }] }` 的 F。
fn finally_handler(stmt: &Stmt) -> Option<Vec<Stmt>> {
    let Stmt::Try {
        catches,
        finally: None,
        ..
    } = stmt
    else {
        return None;
    };
    let [c] = catches.as_slice() else {
        return None;
    };
    if !c.types.is_empty() {
        return None;
    }
    match c.body.split_last() {
        Some((Stmt::Throw(Expr::Var(t)), fin)) if *t == c.var && !refs_in(fin, c.var) => {
            Some(fin.to_vec())
        }
        _ => None,
    }
}

/// 把 `b[i]` 处的 try 改写成 try-finally 或 synchronized。返回在 `i` 之前删掉的语句数。
fn rewrite_try_at(b: &mut Vec<Stmt>, i: usize) -> Option<usize> {
    let fin = finally_handler(&b[i])?;
    let n = fin.len();
    let Stmt::Try { body, .. } = &b[i] else {
        return None;
    };
    let mut body = body.clone();
    let monitor = matches!(fin.as_slice(), [Stmt::MonitorExit(Expr::Var(_))]);
    let mut remove_after = 0;
    let mut tail: Vec<Stmt> = Vec::new();
    let inner_try = !monitor
        && matches!(body.as_slice(), [Stmt::Try { catches, finally: None, .. }] if !catches.is_empty());
    let stripped = if inner_try {
        let Stmt::Try {
            body: ib,
            catches: ic,
            ..
        } = &mut body[0]
        else {
            unreachable!()
        };
        let mut ok = strip_copies(ib, &fin, true, 0, &mut Vec::new());
        for c in ic.iter_mut() {
            ok &= strip_copies(&mut c.body, &fin, true, 0, &mut Vec::new());
        }
        ok
    } else {
        let mut attempt = body.clone();
        if strip_copies(&mut attempt, &fin, true, 0, &mut Vec::new()) {
            body = attempt;
            true
        } else {
            // 正常落下的副本在 try 语句之后
            let mut attempt = body.clone();
            if !ends_with_jump(&attempt)
                && strip_copies(&mut attempt, &fin, false, 0, &mut Vec::new())
                && b.len() >= i + 1 + n
                && same_stmts(&b[i + 1..i + 1 + n], &fin)
            {
                body = attempt;
                remove_after = n;
                true
            } else if let Some(k) = (0..body.len().saturating_sub(n))
                .rev()
                .find(|&k| same_stmts(&body[k..k + n], &fin))
            {
                // 正常出口的副本后面还跟着本不受保护的代码（例如循环里的 `i++; continue;`），
                // 结构化时被并进了 try 体：把它们挪到 try 语句后面
                let mut attempt = body.clone();
                let rest = attempt.split_off(k + n);
                attempt.truncate(k);
                if strip_copies(&mut attempt, &fin, false, 0, &mut Vec::new()) {
                    body = attempt;
                    tail = rest;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }
    };
    if !stripped {
        return None;
    }
    if monitor {
        let Stmt::MonitorExit(Expr::Var(lock_var)) = fin[0] else {
            unreachable!()
        };
        if i < 2 {
            return None;
        }
        let Stmt::MonitorEnter(entered) = &b[i - 1] else {
            return None;
        };
        let mut lock = match &b[i - 2] {
            Stmt::Expr(Expr::Assign(lhs, rhs)) if **lhs == Expr::Var(lock_var) => (**rhs).clone(),
            _ => return None,
        };
        if *entered != lock && *entered != Expr::Var(lock_var) {
            return None;
        }
        // `t = 锁; lock = t; monitorenter(t)`：锁表达式先存进了栈临时变量（dup）
        let mut removed = 2;
        if let (Expr::Var(t), true) = (&lock, i >= 3) {
            let t = *t;
            if let Stmt::Expr(Expr::Assign(lhs, rhs)) = &b[i - 3] {
                let rest_uses = refs_in(&body, t) || refs_in(&b[i + 1..], t);
                if **lhs == Expr::Var(t) && !rest_uses {
                    lock = (**rhs).clone();
                    removed = 3;
                }
            }
        }
        b[i] = Stmt::Sync { lock, body };
        b.splice(i + 1..i + 1 + remove_after, tail);
        b.drain(i - removed..i);
        return Some(removed);
    }
    let new = if inner_try {
        let Stmt::Try {
            body: ib,
            catches: ic,
            ..
        } = body.remove(0)
        else {
            unreachable!()
        };
        Stmt::Try {
            body: ib,
            catches: ic,
            finally: Some(fin),
        }
    } else {
        Stmt::Try {
            body,
            catches: Vec::new(),
            finally: Some(fin),
        }
    };
    b[i] = new;
    b.splice(i + 1..i + 1 + remove_after, tail);
    Some(0)
}

fn finally_and_sync(block: &mut Vec<Stmt>) {
    each_block_deep(block, &mut |b| {
        let mut i = 0;
        while i < b.len() {
            match rewrite_try_at(b, i) {
                Some(removed) => i = i + 1 - removed,
                None => i += 1,
            }
        }
    });
}

// ---------------------------------------------------------------------------
// 三元表达式、数组初始化、临时变量内联

fn ternaries(block: &mut Vec<Stmt>, vars: &Vars) -> bool {
    let mut changed = false;
    each_block_deep(block, &mut |b| {
        for stmt in b.iter_mut() {
            let Stmt::If { cond, then, els } = stmt else {
                continue;
            };
            let ([Stmt::Expr(Expr::Assign(l1, a))], [Stmt::Expr(Expr::Assign(l2, c))]) =
                (then.as_slice(), els.as_slice())
            else {
                continue;
            };
            let (Expr::Var(v1), Expr::Var(v2)) = (&**l1, &**l2) else {
                continue;
            };
            if v1 != v2 || !vars.get(*v1).stack_temp {
                continue;
            }
            let v = *v1;
            let e = Expr::Cond(Box::new(cond.clone()), a.clone(), c.clone());
            *stmt = Stmt::Expr(Expr::Assign(Box::new(Expr::Var(v)), Box::new(e)));
            changed = true;
        }
    });
    changed
}

fn default_value(elem: &JType, extra: u8) -> Option<Expr> {
    if extra > 0 {
        return Some(Expr::Lit(Lit::Null));
    }
    use classfile::signature::BaseType as B;
    Some(match elem {
        JType::Base(B::Long) => Expr::Lit(Lit::Long(0)),
        JType::Base(B::Float) => Expr::Lit(Lit::Float(0)),
        JType::Base(B::Double) => Expr::Lit(Lit::Double(0)),
        JType::Base(B::Boolean) => Expr::Lit(Lit::Bool(false)),
        JType::Base(_) => Expr::int(0),
        _ => Expr::Lit(Lit::Null),
    })
}

fn array_inits(block: &mut Vec<Stmt>, vars: &Vars) -> bool {
    let mut changed = false;
    each_block_deep(block, &mut |b| {
        let mut i = 0;
        while i < b.len() {
            let found = match &b[i] {
                Stmt::Expr(Expr::Assign(lhs, rhs)) => match (&**lhs, &**rhs) {
                    (
                        Expr::Var(t),
                        Expr::NewArray {
                            elem,
                            dims,
                            extra,
                            init: None,
                        },
                    ) if dims.len() == 1 && vars.get(*t).stack_temp => match dims[0] {
                        Expr::Lit(Lit::Int(n)) if n > 0 && n <= 4096 => {
                            Some((*t, n as usize, elem.clone(), *extra))
                        }
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            };
            let Some((t, n, elem, extra)) = found else {
                i += 1;
                continue;
            };
            let mut values: Vec<Option<Expr>> = vec![None; n];
            let mut j = i + 1;
            let mut last = -1i64;
            while j < b.len() {
                let Stmt::Expr(Expr::Assign(lhs, v)) = &b[j] else {
                    break;
                };
                let Expr::Index(arr, idx) = &**lhs else {
                    break;
                };
                let (Expr::Var(a), Expr::Lit(Lit::Int(k))) = (&**arr, &**idx) else {
                    break;
                };
                if *a != t || (*k as i64) <= last || *k as usize >= n || v.reads_var(t) {
                    break;
                }
                last = *k as i64;
                values[*k as usize] = Some((**v).clone());
                j += 1;
            }
            if j == i + 1 {
                i += 1;
                continue;
            }
            let Some(default) = default_value(&elem, extra) else {
                i += 1;
                continue;
            };
            let init: Vec<Expr> = values
                .into_iter()
                .map(|v| v.unwrap_or_else(|| default.clone()))
                .collect();
            b[i] = Stmt::Expr(Expr::Assign(
                Box::new(Expr::Var(t)),
                Box::new(Expr::NewArray {
                    elem,
                    dims: Vec::new(),
                    extra,
                    init: Some(init),
                }),
            ));
            b.drain(i + 1..j);
            changed = true;
            i += 1;
        }
    });
    changed
}

enum Walk {
    NotFound,
    Done,
    Blocked,
}

struct Subst {
    var: VarId,
    value: Option<Expr>,
    /// 被内联的值：字面量随处可放；纯表达式要求之前没有副作用；否则之前还不能读内存
    literal: bool,
    pure: bool,
    effects: bool,
    reads: bool,
}

impl Subst {
    fn ok_here(&self, conditional: bool) -> bool {
        if self.literal {
            return true;
        }
        if conditional {
            return false;
        }
        if self.pure {
            !self.effects
        } else {
            !self.effects && !self.reads
        }
    }

    fn walk(&mut self, e: &mut Expr, conditional: bool) -> Walk {
        macro_rules! go {
            ($x:expr, $cond:expr) => {
                match self.walk($x, $cond) {
                    Walk::NotFound => {}
                    other => return other,
                }
            };
        }
        match e {
            Expr::Var(v) if *v == self.var => {
                if self.ok_here(conditional) {
                    *e = self.value.take().unwrap();
                    Walk::Done
                } else {
                    Walk::Blocked
                }
            }
            Expr::Lambda { .. } => Walk::NotFound,
            Expr::AnonNew(anon) => {
                for a in anon.super_args.iter_mut() {
                    go!(a, conditional);
                }
                if anon.captured.iter().any(|c| c.reads_var(self.var)) {
                    return Walk::Blocked;
                }
                self.effects = true;
                Walk::NotFound
            }
            Expr::Assign(lhs, rhs) => {
                match &mut **lhs {
                    Expr::Var(_) => {}
                    Expr::Field { target, .. } => {
                        if let Some(t) = target {
                            go!(t, conditional);
                        }
                    }
                    Expr::Index(a, i) => {
                        go!(a, conditional);
                        go!(i, conditional);
                    }
                    other => go!(other, conditional),
                }
                go!(rhs, conditional);
                self.effects = true;
                Walk::NotFound
            }
            Expr::CompoundAssign(_, lhs, rhs) => {
                go!(lhs, conditional);
                go!(rhs, conditional);
                self.effects = true;
                Walk::NotFound
            }
            Expr::IncDec { target, .. } => {
                go!(target, conditional);
                self.effects = true;
                Walk::NotFound
            }
            Expr::Binary(BinOp::LAnd | BinOp::LOr, a, b) => {
                go!(a, conditional);
                go!(b, true);
                Walk::NotFound
            }
            Expr::Cond(c, a, b) => {
                go!(c, conditional);
                go!(a, true);
                go!(b, true);
                Walk::NotFound
            }
            Expr::Invoke { target, args, .. } => {
                if let Some(t) = target {
                    go!(t, conditional);
                }
                for a in args.iter_mut() {
                    go!(a, conditional);
                }
                self.effects = true;
                Walk::NotFound
            }
            Expr::New { args, .. } => {
                for a in args.iter_mut() {
                    go!(a, conditional);
                }
                self.effects = true;
                Walk::NotFound
            }
            Expr::Field { target, .. } => {
                if let Some(t) = target {
                    go!(t, conditional);
                }
                self.reads = true;
                Walk::NotFound
            }
            Expr::Index(a, i) => {
                go!(a, conditional);
                go!(i, conditional);
                self.reads = true;
                Walk::NotFound
            }
            other => {
                let mut result = Walk::NotFound;
                other.children_mut(&mut |c| {
                    if matches!(result, Walk::NotFound) {
                        result = self.walk(c, conditional);
                    }
                });
                result
            }
        }
    }
}

/// 在语句本层的表达式里（按求值顺序）替换唯一的一次使用。
fn substitute_in_stmt(stmt: &mut Stmt, s: &mut Subst) -> Walk {
    let walk_opt = |e: &mut Option<Expr>, s: &mut Subst| match e {
        Some(e) => s.walk(e, false),
        None => Walk::NotFound,
    };
    match stmt {
        Stmt::Expr(e) | Stmt::Throw(e) | Stmt::MonitorEnter(e) | Stmt::MonitorExit(e) => {
            s.walk(e, false)
        }
        Stmt::If { cond, .. } => s.walk(cond, false),
        Stmt::Switch { expr, .. } => s.walk(expr, false),
        Stmt::Sync { lock, .. } => s.walk(lock, false),
        Stmt::Return(e) => walk_opt(e, s),
        Stmt::Local { init, .. } => walk_opt(init, s),
        Stmt::Loop {
            kind: LoopKind::ForEach { iterable, .. },
            ..
        } => s.walk(iterable, false),
        _ => Walk::NotFound,
    }
}

fn inline_temps(block: &mut Vec<Stmt>, vars: &Vars) -> bool {
    let counts = counts_of(block);
    let mut changed = false;
    each_block_deep(block, &mut |b| {
        let mut i = 0;
        while i + 1 < b.len() {
            // `x = e; return x;` 里没有源码名字的 x 也内联
            let returns_next =
                |t: VarId| matches!(&b[i + 1], Stmt::Return(Some(Expr::Var(v))) if *v == t);
            let candidate = match &b[i] {
                Stmt::Expr(Expr::Assign(lhs, rhs)) => match &**lhs {
                    Expr::Var(t)
                        if (vars.get(*t).stack_temp
                            || (!vars.get(*t).named_by_lvt
                                && !vars.get(*t).is_param
                                && returns_next(*t)))
                            && counts
                                .get(t)
                                .is_some_and(|c| c.defs == 1 && c.uses == 1 && c.deferred == 0)
                            && !rhs.reads_var(*t) =>
                    {
                        Some((*t, (**rhs).clone()))
                    }
                    _ => None,
                },
                _ => None,
            };
            let Some((t, value)) = candidate else {
                i += 1;
                continue;
            };
            let literal = matches!(value, Expr::Lit(_));
            let pure = value.is_pure();
            let mut s = Subst {
                var: t,
                value: Some(value),
                literal,
                pure,
                effects: false,
                reads: false,
            };
            let mut next = b[i + 1].clone();
            if let Walk::Done = substitute_in_stmt(&mut next, &mut s) {
                b[i + 1] = next;
                b.remove(i);
                changed = true;
                continue;
            }
            i += 1;
        }
    });
    changed
}

// ---------------------------------------------------------------------------
// 变量类型推断（没有 LocalVariableTable 时）

fn infer_var_types(block: &[Stmt], class: &CodeClass, vars: &mut Vars) {
    for _ in 0..2 {
        let mut defs: FxHashMap<VarId, Vec<Option<JType>>> = FxHashMap::default();
        let tcx = TypeCx { class, vars };
        let mut record = |v: VarId, e: &Expr| {
            let is01 = |x: &Expr| matches!(x, Expr::Lit(Lit::Int(0 | 1)));
            let ty = match e {
                Expr::Lit(Lit::Int(0 | 1)) => Some(JType::TypeVar("01".into())),
                Expr::Cond(_, a, b) if is01(a) && is01(b) => {
                    Some(base(classfile::signature::BaseType::Boolean))
                }
                Expr::Lit(Lit::Int(k)) if (0..=0xFFFF).contains(k) => {
                    Some(JType::TypeVar("int-lit".into()))
                }
                Expr::Lit(Lit::Null) => Some(JType::TypeVar("null".into())),
                _ => tcx.ty(e),
            };
            defs.entry(v).or_default().push(ty);
        };
        walk_exprs(block, &mut |e| {
            if let Expr::Assign(lhs, rhs) = e {
                if let Expr::Var(v) = &**lhs {
                    record(*v, rhs);
                }
            }
        });
        fn walk_locals(block: &[Stmt], f: &mut dyn FnMut(VarId, &Expr)) {
            for s in block {
                if let Stmt::Local { var, init: Some(e) } = s {
                    f(*var, e);
                }
                s.blocks(&mut |b| walk_locals(b, f));
            }
        }
        walk_locals(block, &mut record);
        for (v, tys) in defs {
            let info = vars.get(v);
            if info.named_by_lvt && info.ty.is_some() {
                continue;
            }
            if info.is_param {
                continue;
            }
            let is_marker =
                |t: &Option<JType>, m: &str| matches!(t, Some(JType::TypeVar(x)) if x == m);
            let real: Vec<&JType> = tys
                .iter()
                .filter(|t| {
                    !is_marker(t, "01") && !is_marker(t, "int-lit") && !is_marker(t, "null")
                })
                .filter_map(|t| t.as_ref())
                .collect();
            let new_ty = match info.kind {
                crate::bytecode::Kind::Int => {
                    let all =
                        |f: &dyn Fn(&JType) -> bool| !real.is_empty() && real.iter().all(|t| f(t));
                    let known = tys.iter().all(|t| t.is_some());
                    if known
                        && all(&types::is_boolean)
                        && tys.iter().all(|t| !is_marker(t, "int-lit"))
                    {
                        Some(base(classfile::signature::BaseType::Boolean))
                    } else if known && all(&types::is_char) {
                        Some(base(classfile::signature::BaseType::Char))
                    } else if !real.is_empty()
                        && real.iter().all(|t| **t == *real[0])
                        && matches!(real[0], JType::Base(_))
                        && tys.iter().all(|t| t.is_some())
                    {
                        Some(real[0].clone())
                    } else {
                        Some(base(classfile::signature::BaseType::Int))
                    }
                }
                crate::bytecode::Kind::Ref => {
                    let unknown = tys.iter().any(|t| t.is_none());
                    if real.is_empty() || unknown {
                        info.ty.clone().or_else(|| Some(object_ty()))
                    } else if real.iter().all(|t| erase(t) == erase(real[0])) {
                        Some(real[0].clone())
                    } else {
                        Some(object_ty())
                    }
                }
                k => Some(ty_of_kind(k)),
            };
            vars.get_mut(v).ty = new_ty;
        }
    }
    // 剩下没有类型的（只读不写的栈变量等）按类别给
    for info in vars.vars.iter_mut() {
        if info.ty.is_none() {
            info.ty = Some(ty_of_kind(info.kind));
        }
    }
}

// ---------------------------------------------------------------------------
// 表达式化简

fn is_one(e: &Expr) -> bool {
    matches!(e, Expr::Lit(Lit::Int(1)) | Expr::Lit(Lit::Long(1)))
        || matches!(e, Expr::Lit(Lit::Float(b)) if f32::from_bits(*b) == 1.0)
        || matches!(e, Expr::Lit(Lit::Double(b)) if f64::from_bits(*b) == 1.0)
}

fn coerce(e: &mut Expr, ty: &JType) {
    if types::is_boolean(ty) {
        match e {
            Expr::Lit(Lit::Int(0)) => *e = Expr::Lit(Lit::Bool(false)),
            Expr::Lit(Lit::Int(1)) => *e = Expr::Lit(Lit::Bool(true)),
            Expr::Cond(c, a, b) => {
                coerce(a, ty);
                coerce(b, ty);
                match (&**a, &**b) {
                    (Expr::Lit(Lit::Bool(true)), Expr::Lit(Lit::Bool(false))) => *e = (**c).clone(),
                    (Expr::Lit(Lit::Bool(false)), Expr::Lit(Lit::Bool(true))) => {
                        *e = negate((**c).clone())
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    } else if types::is_char(ty) {
        match e {
            Expr::Lit(Lit::Int(k)) if (0..=0xFFFF).contains(k) => {
                *e = Expr::Lit(Lit::Char(*k as u16))
            }
            Expr::Cond(_, a, b) => {
                coerce(a, ty);
                coerce(b, ty);
            }
            _ => {}
        }
    }
}

/// `new StringBuilder().append(a).append(b).toString()` → `a + b`。
fn fold_string_builder(e: &Expr) -> Option<Expr> {
    let Expr::Invoke {
        target: Some(target),
        owner,
        name,
        args,
        ..
    } = e
    else {
        return None;
    };
    if name != "toString" || !args.is_empty() {
        return None;
    }
    let is_builder = |o: &str| o == "java/lang/StringBuilder" || o == "java/lang/StringBuffer";
    if !is_builder(owner) {
        return None;
    }
    let mut parts: Vec<(Expr, bool)> = Vec::new();
    let mut cur = &**target;
    loop {
        match cur {
            Expr::Invoke {
                target: Some(t),
                owner,
                name,
                desc,
                args,
                ..
            } if name == "append" && is_builder(owner) && args.len() == 1 => {
                let (params, _) = method_desc(desc).ok()?;
                let p = params.first()?;
                if matches!(p, JType::Array(_)) {
                    return None;
                }
                parts.push((args[0].clone(), is_string(p)));
                cur = t;
            }
            Expr::New { class, desc, args } if is_builder(class) => {
                match args.as_slice() {
                    [] => {}
                    [a] if desc == "(Ljava/lang/String;)V" => parts.push((a.clone(), true)),
                    [a] if desc == "(Ljava/lang/CharSequence;)V" => parts.push((a.clone(), false)),
                    [_] if desc == "(I)V" => {}
                    _ => return None,
                }
                break;
            }
            _ => return None,
        }
    }
    if parts.is_empty() {
        return Some(Expr::Lit(Lit::Str(Vec::new())));
    }
    parts.reverse();
    // `String.valueOf(x)` 开头（老编译器的写法）
    if let Some((
        Expr::Invoke {
            owner,
            name,
            args,
            target: None,
            ..
        },
        true,
    )) = parts.first()
    {
        if owner == "java/lang/String" && name == "valueOf" && args.len() == 1 {
            let a = args[0].clone();
            parts[0] = (a, false);
        }
    }
    Some(build_concat(parts))
}

fn simplify_expr(e: &mut Expr, cx: &TypeCx<'_>) {
    // 先处理子表达式
    e.children_mut(&mut |c| simplify_expr(c, cx));
    if let Expr::Lambda { body, .. } = e {
        match &mut **body {
            LambdaBody::Expr(b) => simplify_expr(b, cx),
            LambdaBody::Block(_) => {}
        }
    }
    if let Some(folded) = fold_string_builder(e) {
        *e = folded;
        return;
    }
    match e {
        Expr::Binary(op @ (BinOp::Eq | BinOp::Ne), a, b) => {
            let (op, a_bool, b_bool) = (*op, cx.is_boolean(a), cx.is_boolean(b));
            if a_bool && matches!(**b, Expr::Lit(Lit::Int(0 | 1)))
                || b_bool && matches!(**a, Expr::Lit(Lit::Int(0 | 1)))
            {
                let (x, k) = if a_bool {
                    ((**a).clone(), (**b).clone())
                } else {
                    ((**b).clone(), (**a).clone())
                };
                let positive = (op == BinOp::Ne) == k.is_int_lit(0);
                *e = if positive { x } else { negate(x) };
                return;
            }
            if a_bool {
                coerce(b, &base(classfile::signature::BaseType::Boolean));
            } else if b_bool {
                coerce(a, &base(classfile::signature::BaseType::Boolean));
            }
            if cx.is_char(a) {
                coerce(b, &base(classfile::signature::BaseType::Char));
            } else if cx.is_char(b) {
                coerce(a, &base(classfile::signature::BaseType::Char));
            }
            // `(c ? 1 : 0) != 0` → `c`
            if let (Expr::Cond(c, x, y), Expr::Lit(Lit::Int(0))) = (&**a, &**b) {
                if x.is_int_lit(1) && y.is_int_lit(0) {
                    *e = if op == BinOp::Ne {
                        (**c).clone()
                    } else {
                        negate((**c).clone())
                    };
                }
            }
        }
        Expr::Binary(BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, a, b) => {
            if cx.is_char(a) {
                coerce(b, &base(classfile::signature::BaseType::Char));
            } else if cx.is_char(b) {
                coerce(a, &base(classfile::signature::BaseType::Char));
            }
        }
        Expr::Unary(UnOp::Not, inner) => {
            let x = (**inner).clone();
            let n = negate(x.clone());
            if n != Expr::not(x) {
                *e = n;
            }
        }
        // `x ^ -1` → `~x`
        Expr::Binary(BinOp::Xor, a, b)
            if matches!(**b, Expr::Lit(Lit::Int(-1)) | Expr::Lit(Lit::Long(-1))) =>
        {
            *e = Expr::Unary(UnOp::BitNot, a.clone());
        }
        Expr::Cast(t, inner) => match (&*t, &**inner) {
            (JType::Base(classfile::signature::BaseType::Long), Expr::Lit(Lit::Int(k))) => {
                *e = Expr::Lit(Lit::Long(*k as i64))
            }
            (JType::Base(classfile::signature::BaseType::Float), Expr::Lit(Lit::Int(k)))
                if (*k as f32) as i32 == *k =>
            {
                *e = Expr::Lit(Lit::Float((*k as f32).to_bits()))
            }
            (JType::Base(classfile::signature::BaseType::Double), Expr::Lit(Lit::Int(k))) => {
                *e = Expr::Lit(Lit::Double((*k as f64).to_bits()))
            }
            _ => {}
        },
        Expr::Assign(lhs, rhs) => {
            if let Some(ty) = cx.ty(lhs) {
                coerce(rhs, &ty);
            }
            // `x = x op y` → `x op= y`
            let target_pure = match &**lhs {
                Expr::Var(_) | Expr::Field { target: None, .. } => true,
                Expr::Field {
                    target: Some(t), ..
                } => t.is_pure(),
                Expr::Index(a, i) => a.is_pure() && i.is_pure(),
                _ => false,
            };
            if target_pure {
                let lhs_ty = cx.ty(lhs);
                let (inner, narrowed) = match &**rhs {
                    Expr::Cast(t, inner)
                        if lhs_ty.as_ref() == Some(t)
                            && matches!(
                                t,
                                JType::Base(
                                    classfile::signature::BaseType::Byte
                                        | classfile::signature::BaseType::Short
                                        | classfile::signature::BaseType::Char
                                )
                            ) =>
                    {
                        (&**inner, true)
                    }
                    other => (other, false),
                };
                if let Expr::Binary(op, a, b) = inner {
                    let compound_ok =
                        !op.is_comparison() && !matches!(op, BinOp::LAnd | BinOp::LOr);
                    // 字符串的 `s = s + x` 也可以写成 `s += x`
                    if compound_ok
                        && **a == **lhs
                        && !(narrowed && *op == BinOp::Add && cx.is_string(a))
                    {
                        *e = Expr::CompoundAssign(*op, lhs.clone(), b.clone());
                    }
                }
            }
        }
        Expr::Invoke { desc, args, .. } | Expr::New { desc, args, .. } => {
            if let Ok((params, _)) = method_desc(desc) {
                for (a, p) in args.iter_mut().zip(&params) {
                    coerce(a, p);
                }
            }
        }
        _ => {}
    }
    // `x += 1` → `++x`
    if let Expr::CompoundAssign(op @ (BinOp::Add | BinOp::Sub), lhs, rhs) = e {
        if is_one(rhs) && !cx.is_string(lhs) {
            *e = Expr::IncDec {
                target: lhs.clone(),
                inc: *op == BinOp::Add,
                prefix: true,
            };
        }
    }
}

fn simplify(block: &mut [Stmt], class: &CodeClass, vars: &Vars, ret: Option<&JType>) {
    let cx = TypeCx { class, vars };
    // lambda 的语句体在它自己的方法里已经化简过
    fn go(block: &mut [Stmt], cx: &TypeCx<'_>, ret: Option<&JType>) {
        for stmt in block.iter_mut() {
            stmt.exprs_mut(&mut |e| simplify_expr(e, cx));
            match stmt {
                Stmt::Return(Some(e)) => {
                    if let Some(t) = ret {
                        coerce(e, t);
                    }
                }
                Stmt::Local { var, init: Some(e) } => {
                    if let Some(t) = cx.vars.get(*var).ty.clone() {
                        coerce(e, &t);
                    }
                }
                Stmt::Switch { expr, cases, .. } if cx.is_char(expr) => {
                    for c in cases.iter_mut() {
                        for l in c.labels.iter_mut().flatten() {
                            if let CaseLabel::Int(k) = l {
                                if (0..=0xFFFF).contains(k) {
                                    *l = CaseLabel::Char(*k as u16);
                                }
                            }
                        }
                    }
                }
                Stmt::Expr(Expr::IncDec { prefix, .. }) => *prefix = false,
                _ => {}
            }
            stmt.blocks_mut(&mut |b| go(b, cx, ret));
        }
    }
    go(block, &cx, ret);
}

// ---------------------------------------------------------------------------
// assert、构造器

fn is_assertions_disabled(e: &Expr) -> bool {
    matches!(e, Expr::Field { target: None, name, .. } if name == "$assertionsDisabled")
}

fn asserts(block: &mut Vec<Stmt>) {
    each_block_deep(block, &mut |b| {
        b.retain(|s| !matches!(s, Stmt::Expr(Expr::Assign(lhs, _)) if is_assertions_disabled(lhs)));
        for stmt in b.iter_mut() {
            let Stmt::If { cond, then, els } = stmt else {
                continue;
            };
            if !els.is_empty() {
                continue;
            }
            let [Stmt::Throw(Expr::New { class, args, .. })] = then.as_slice() else {
                continue;
            };
            if class != "java/lang/AssertionError" || args.len() > 1 {
                continue;
            }
            let enabled =
                |e: &Expr| matches!(e, Expr::Unary(UnOp::Not, x) if is_assertions_disabled(x));
            let assert_cond = match cond {
                c if enabled(c) => Some(Expr::Lit(Lit::Bool(false))),
                Expr::Binary(BinOp::LAnd, a, rest) if enabled(a) => Some(negate((**rest).clone())),
                _ => None,
            };
            if let Some(c) = assert_cond {
                let msg = args.first().cloned();
                *stmt = Stmt::Assert { cond: c, msg };
            }
        }
    });
}

/// `this.this$0` → `Outer.this`。
fn outer_this(block: &mut [Stmt], class: &CodeClass) {
    rewrite_exprs(block, &mut |e| {
        if let Expr::Field {
            target: Some(t),
            owner,
            name,
            desc,
        } = e
        {
            if **t == Expr::This
                && *owner == class.name
                && name.starts_with("this$")
                && class
                    .find_field(name)
                    .is_some_and(|f| f.access & access::SYNTHETIC != 0)
            {
                if let Ok(JType::Class { name: outer, .. }) = field_desc(desc) {
                    *e = Expr::OuterThis(outer);
                }
            }
        }
    });
}

/// javac 在内部类构造器里对外层实例做的空值检查：`Objects.requireNonNull(p)` / `p.getClass()`。
pub(crate) fn is_null_check_of(s: &Stmt, is_param: &dyn Fn(VarId) -> bool) -> bool {
    match s {
        Stmt::Expr(Expr::Invoke {
            target: None,
            owner,
            name,
            args,
            ..
        }) if owner == "java/util/Objects" && name == "requireNonNull" && args.len() == 1 => {
            matches!(args[0], Expr::Var(v) if is_param(v))
        }
        Stmt::Expr(Expr::Invoke {
            target: Some(t),
            name,
            args,
            ..
        }) if name == "getClass" && args.is_empty() => matches!(**t, Expr::Var(v) if is_param(v)),
        _ => false,
    }
}

fn ctor_cleanup(block: &mut Vec<Stmt>, class: &CodeClass, synthetic_params: &[VarId]) {
    let is_enum = class.access & access::ENUM != 0;
    let super_name = class.super_name.clone().unwrap_or_default();
    block.retain(|s| !is_null_check_of(s, &|v| synthetic_params.contains(&v)));
    block.retain(|s| {
        // 编译器给合成字段（外层实例、捕获的局部变量）赋值
        if let Stmt::Expr(Expr::Assign(lhs, rhs)) = s {
            if let Expr::Field { target: Some(t), owner, name, .. } = &**lhs {
                if **t == Expr::This
                    && *owner == class.name
                    && class
                        .find_field(name)
                        .is_some_and(|f| f.access & access::SYNTHETIC != 0)
                    && matches!(&**rhs, Expr::Var(v) if synthetic_params.contains(v) || name.starts_with("val$") || name.starts_with("this$"))
                {
                    return false;
                }
            }
        }
        true
    });
    for s in block.iter_mut() {
        if let Stmt::Expr(Expr::Invoke {
            target: Some(t),
            name,
            owner,
            args,
            ..
        }) = s
        {
            if **t == Expr::This && name == "<init>" {
                // 去掉传给 this(...) / super(...) 的合成实参
                args.retain(|a| !matches!(a, Expr::Var(v) if synthetic_params.contains(v)));
                let _ = owner;
            }
        }
    }
    block.retain(|s| {
        if let Stmt::Expr(Expr::Invoke {
            target: Some(t),
            name,
            owner,
            args,
            ..
        }) = s
        {
            if **t == Expr::This && name == "<init>" {
                if is_enum && owner == "java/lang/Enum" {
                    return false;
                }
                if *owner == super_name && args.is_empty() {
                    return false;
                }
            }
        }
        true
    });
}

/// 装箱拆箱：算术里的 `x.intValue()` → `x`；赋给包装类型、返回包装类型时的 `Integer.valueOf(x)` → `x`。
fn unbox(block: &mut [Stmt], class: &CodeClass, vars: &Vars, ret: Option<&JType>) {
    fn wrapper_of(name: &str) -> Option<&'static str> {
        Some(match name {
            "java/lang/Integer" => "I",
            "java/lang/Long" => "J",
            "java/lang/Double" => "D",
            "java/lang/Float" => "F",
            "java/lang/Boolean" => "Z",
            "java/lang/Character" => "C",
            "java/lang/Short" => "S",
            "java/lang/Byte" => "B",
            _ => return None,
        })
    }
    fn unboxed(e: &Expr) -> Option<Expr> {
        let Expr::Invoke {
            target: Some(t),
            owner,
            name,
            args,
            desc,
            ..
        } = e
        else {
            return None;
        };
        let prim = wrapper_of(owner)?;
        let expected = match prim {
            "I" => "intValue",
            "J" => "longValue",
            "D" => "doubleValue",
            "F" => "floatValue",
            "Z" => "booleanValue",
            "C" => "charValue",
            "S" => "shortValue",
            _ => "byteValue",
        };
        (name == expected && args.is_empty() && desc == &format!("(){prim}")).then(|| (**t).clone())
    }
    fn boxed(e: &Expr) -> Option<Expr> {
        let Expr::Invoke {
            target: None,
            owner,
            name,
            args,
            desc,
            ..
        } = e
        else {
            return None;
        };
        let prim = wrapper_of(owner)?;
        (name == "valueOf" && args.len() == 1 && desc == &format!("({prim})L{owner};"))
            .then(|| args[0].clone())
    }
    let cx = TypeCx { class, vars };
    let is_wrapper = |t: Option<JType>| matches!(t, Some(JType::Class { name, .. }) if wrapper_of(&name).is_some());
    rewrite_exprs(block, &mut |e| match e {
        Expr::Binary(op, a, b) if !matches!(op, BinOp::Eq | BinOp::Ne) => {
            if let Some(x) = unboxed(a) {
                **a = x;
            }
            if let Some(x) = unboxed(b) {
                **b = x;
            }
        }
        Expr::Unary(_, a) | Expr::CompoundAssign(_, _, a) => {
            if let Some(x) = unboxed(a) {
                **a = x;
            }
        }
        Expr::Assign(lhs, rhs) if is_wrapper(cx.ty(lhs)) => {
            if let Some(x) = boxed(rhs) {
                **rhs = x;
            }
        }
        Expr::Lambda { body, .. } => {
            if let LambdaBody::Expr(b) = &mut **body {
                if let Some(x) = boxed(b) {
                    *b = x;
                }
            }
        }
        _ => {}
    });
    let ret_wrapper = is_wrapper(ret.cloned());
    fn stmts(block: &mut [Stmt], f: &mut dyn FnMut(&mut Stmt)) {
        for s in block.iter_mut() {
            f(s);
            s.blocks_mut(&mut |b| stmts(b, f));
        }
    }
    stmts(block, &mut |s| match s {
        Stmt::Return(Some(e)) if ret_wrapper => {
            if let Some(x) = boxed(e) {
                *e = x;
            }
        }
        Stmt::If { cond, .. } => {
            if let Some(x) = unboxed(cond) {
                *cond = x;
            }
        }
        _ => {}
    });
}

/// `try { ...; t = x; } catch (...) { ...; return ...; } return t;` → `try { ...; return x; } ...`
fn return_into_try(block: &mut Vec<Stmt>, vars: &Vars) {
    let counts = counts_of(block);
    each_block_deep(block, &mut |b| {
        let mut i = 0;
        while i + 1 < b.len() {
            let Stmt::Return(Some(Expr::Var(t))) = &b[i + 1] else {
                i += 1;
                continue;
            };
            let t = *t;
            let ok_var = (vars.get(t).stack_temp
                || (!vars.get(t).named_by_lvt && !vars.get(t).is_param))
                && counts
                    .get(&t)
                    .is_some_and(|c| c.defs == 1 && c.uses == 1 && c.deferred == 0);
            let fits = match &b[i] {
                Stmt::Try { body, catches, .. } => {
                    matches!(body.last(), Some(Stmt::Expr(Expr::Assign(lhs, _))) if **lhs == Expr::Var(t))
                        && catches.iter().all(|c| ends_with_jump(&c.body))
                }
                _ => false,
            };
            if ok_var && fits {
                if let Stmt::Try { body, .. } = &mut b[i] {
                    if let Some(Stmt::Expr(Expr::Assign(_, value))) = body.pop() {
                        body.push(Stmt::Return(Some(*value)));
                    }
                }
                b.remove(i + 1);
            }
            i += 1;
        }
    });
}

// ---------------------------------------------------------------------------
// 控制流整理

/// 块里（不进入内层循环 / switch）有没有跳到 `label` 所在循环的 continue。
fn has_own_continue(block: &[Stmt], label: Option<LabelId>, nested: bool) -> bool {
    block.iter().any(|s| match s {
        Stmt::Continue(None) => !nested,
        Stmt::Continue(Some(l)) => Some(*l) == label,
        Stmt::Loop { body, .. } => has_own_continue(body, label, true),
        other => {
            let mut found = false;
            other.blocks(&mut |b| found |= has_own_continue(b, label, nested));
            found
        }
    })
}

fn is_own_jump(s: &Stmt, label: Option<LabelId>, brk: bool) -> bool {
    match (s, brk) {
        (Stmt::Break(None), true) | (Stmt::Continue(None), false) => true,
        (Stmt::Break(Some(l)), true) | (Stmt::Continue(Some(l)), false) => Some(*l) == label,
        _ => false,
    }
}

/// 去掉循环体末尾多余的 continue（含末尾 if 的各分支里的）。
fn strip_trailing_continue(block: &mut Vec<Stmt>, label: Option<LabelId>) {
    if let Some(last) = block.last_mut() {
        match last {
            s if is_own_jump(s, label, false) => {
                block.pop();
            }
            Stmt::If { then, els, .. } => {
                strip_trailing_continue(then, label);
                strip_trailing_continue(els, label);
            }
            Stmt::Try { body, catches, .. } => {
                strip_trailing_continue(body, label);
                for c in catches.iter_mut() {
                    strip_trailing_continue(&mut c.body, label);
                }
            }
            Stmt::Sync { body, .. } => strip_trailing_continue(body, label),
            _ => {}
        }
    }
}

fn strip_trailing_return(block: &mut Vec<Stmt>) {
    if let Some(last) = block.last_mut() {
        match last {
            Stmt::Return(None) => {
                block.pop();
            }
            Stmt::If { then, els, .. } => {
                strip_trailing_return(then);
                strip_trailing_return(els);
            }
            Stmt::Try { body, catches, .. } => {
                strip_trailing_return(body);
                for c in catches.iter_mut() {
                    strip_trailing_return(&mut c.body);
                }
            }
            Stmt::Sync { body, .. } => strip_trailing_return(body),
            _ => {}
        }
    }
}

fn control_flow(block: &mut Vec<Stmt>) {
    each_block_deep(block, &mut |b| {
        // if 的整理
        let mut i = 0;
        while i < b.len() {
            if let Stmt::If { cond, then, els } = &mut b[i] {
                if then.is_empty() && !els.is_empty() {
                    *cond = negate(cond.clone());
                    std::mem::swap(then, els);
                }
                if !els.is_empty() && ends_with_jump(then) {
                    let rest = std::mem::take(els);
                    b.splice(i + 1..i + 1, rest);
                }
            }
            i += 1;
        }
        // 循环形态
        for stmt in b.iter_mut() {
            let Stmt::Loop { label, kind, body } = stmt else {
                continue;
            };
            let label = *label;
            strip_trailing_continue(body, label);
            if !matches!(kind, LoopKind::While(Expr::Lit(Lit::Bool(true)))) {
                continue;
            }
            // while (true) { if (c) break; ... } → while (!c) { ... }
            if let Some(Stmt::If { cond, then, els }) = body.first() {
                if els.is_empty() && then.len() == 1 && is_own_jump(&then[0], label, true) {
                    *kind = LoopKind::While(negate(cond.clone()));
                    body.remove(0);
                    continue;
                }
            }
            // while (true) { ...; if (c) continue; break; } → do { ... } while (c);
            let n = body.len();
            if n >= 2 && is_own_jump(&body[n - 1], label, true) {
                if let Stmt::If { cond, then, els } = &body[n - 2] {
                    if els.is_empty() && then.len() == 1 && is_own_jump(&then[0], label, false) {
                        let cond = cond.clone();
                        let rest = &body[..n - 2];
                        if !has_own_continue(rest, label, false) {
                            *kind = LoopKind::DoWhile(cond);
                            body.truncate(n - 2);
                            continue;
                        }
                    }
                }
            }
            // while (true) { ...; if (c) break; } → do { ... } while (!c);
            if n >= 1 {
                if let Stmt::If { cond, then, els } = &body[n - 1] {
                    if els.is_empty()
                        && then.len() == 1
                        && is_own_jump(&then[0], label, true)
                        && !has_own_continue(&body[..n - 1], label, false)
                    {
                        let cond = negate(cond.clone());
                        *kind = LoopKind::DoWhile(cond);
                        body.truncate(n - 1);
                    }
                }
            }
        }
    });
}

/// 只有 break / continue 引用了的标号保留。
fn strip_labels(block: &mut Vec<Stmt>) {
    let mut used = FxHashSet::default();
    fn collect(block: &[Stmt], used: &mut FxHashSet<LabelId>) {
        for s in block {
            match s {
                Stmt::Break(Some(l)) | Stmt::Continue(Some(l)) => {
                    used.insert(*l);
                }
                _ => {}
            }
            s.exprs(&mut |e| {
                walk_expr_deep(e, &mut |x| {
                    if let Expr::Lambda { body, .. } = x {
                        if let LambdaBody::Block(b) = &**body {
                            collect(b, used);
                        }
                    }
                })
            });
            s.blocks(&mut |b| collect(b, used));
        }
    }
    collect(block, &mut used);
    each_block_deep(block, &mut |b| {
        for s in b.iter_mut() {
            match s {
                Stmt::Loop { label, .. } | Stmt::Switch { label, .. }
                    if label.is_some_and(|l| !used.contains(&l)) =>
                {
                    *label = None;
                }
                _ => {}
            }
        }
    });
}

// ---------------------------------------------------------------------------
// for / for-each（变量声明放好之后）

fn refs_in(stmts: &[Stmt], v: VarId) -> bool {
    let mut found = false;
    walk_exprs(stmts, &mut |e| {
        found |= matches!(e, Expr::Var(x) if *x == v)
    });
    fn locals(stmts: &[Stmt], v: VarId) -> bool {
        stmts.iter().any(|s| {
            matches!(s, Stmt::Local { var, .. } if *var == v)
                || matches!(s, Stmt::Loop { kind: LoopKind::ForEach { var, .. }, .. } if *var == v)
                || {
                    let mut f = false;
                    s.blocks(&mut |b| f |= locals(b, v));
                    f
                }
        })
    }
    found || locals(stmts, v)
}

fn update_var(s: &Stmt) -> Option<VarId> {
    let Stmt::Expr(e) = s else {
        return None;
    };
    let target = match e {
        Expr::IncDec { target, .. } | Expr::CompoundAssign(_, target, _) => target,
        Expr::Assign(lhs, _) => lhs,
        _ => return None,
    };
    match **target {
        Expr::Var(v) => Some(v),
        _ => None,
    }
}

/// continue 之前的更新语句副本去掉；有 continue 前面不是这句时失败。
fn strip_update_before_continue(
    block: &mut Vec<Stmt>,
    update: &Stmt,
    label: Option<LabelId>,
    nested: bool,
) -> bool {
    let mut i = 0;
    while i < block.len() {
        let own = match &block[i] {
            Stmt::Continue(None) => !nested,
            Stmt::Continue(Some(l)) => Some(*l) == label,
            _ => false,
        };
        if own {
            if i == 0 || block[i - 1] != *update {
                return false;
            }
            block.remove(i - 1);
            continue;
        }
        let ok = match &mut block[i] {
            Stmt::Loop { body, .. } => strip_update_before_continue(body, update, label, true),
            other => {
                let mut ok = true;
                other.blocks_mut(&mut |b| {
                    ok &= strip_update_before_continue(b, update, label, nested)
                });
                ok
            }
        };
        if !ok {
            return false;
        }
        i += 1;
    }
    true
}

fn for_loops(block: &mut Vec<Stmt>, vars: &Vars) {
    each_block_deep(block, &mut |b| {
        let mut i = 0;
        while i < b.len() {
            if try_foreach(b, i, vars) {
                i += 1;
                continue;
            }
            let Stmt::Loop {
                label,
                kind: LoopKind::While(cond),
                body,
            } = &b[i]
            else {
                i += 1;
                continue;
            };
            let Some(update) = body.last().cloned() else {
                i += 1;
                continue;
            };
            let Some(v) = update_var(&update) else {
                i += 1;
                continue;
            };
            if !cond.reads_var(v) || i == 0 {
                i += 1;
                continue;
            }
            let label = *label;
            let cond = cond.clone();
            let mut new_body = body[..body.len() - 1].to_vec();
            if !strip_update_before_continue(&mut new_body, &update, label, false) {
                i += 1;
                continue;
            }
            let init_stmt = match &b[i - 1] {
                Stmt::Local { var, init: Some(_) } if *var == v && !refs_in(&b[i + 1..], v) => {
                    Some(b[i - 1].clone())
                }
                Stmt::Expr(Expr::Assign(lhs, _)) if **lhs == Expr::Var(v) => Some(b[i - 1].clone()),
                _ => None,
            };
            let Some(init_stmt) = init_stmt else {
                i += 1;
                continue;
            };
            let Stmt::Expr(update_expr) = update else {
                unreachable!()
            };
            b[i] = Stmt::Loop {
                label,
                kind: LoopKind::For {
                    init: vec![init_stmt],
                    cond: Some(cond),
                    update: vec![update_expr],
                },
                body: new_body,
            };
            b.remove(i - 1);
        }
    });
}

/// javac 的 for-each 展开形式还原成 `for (T x : e)`。
fn try_foreach(b: &mut Vec<Stmt>, i: usize, vars: &Vars) -> bool {
    let synthetic = |v: VarId| !vars.get(v).named_by_lvt;
    // Iterable：`Iterator it = e.iterator(); while (it.hasNext()) { T x = (T) it.next(); ... }`
    if i >= 1 {
        if let (
            Stmt::Local {
                var: it,
                init:
                    Some(Expr::Invoke {
                        target: Some(src),
                        name,
                        args,
                        ..
                    }),
            },
            Stmt::Loop {
                label,
                kind:
                    LoopKind::While(Expr::Invoke {
                        target: Some(t2),
                        name: has_next,
                        ..
                    }),
                body,
            },
        ) = (&b[i - 1], &b[i])
        {
            let it = *it;
            if name == "iterator"
                && args.is_empty()
                && has_next == "hasNext"
                && **t2 == Expr::Var(it)
                && synthetic(it)
            {
                if let Some(Stmt::Local {
                    var: x,
                    init: Some(next),
                }) = body.first()
                {
                    let next_inner = match next {
                        Expr::Cast(_, inner) => &**inner,
                        other => other,
                    };
                    let is_next = matches!(next_inner, Expr::Invoke { target: Some(t3), name, args, .. } if **t3 == Expr::Var(it) && name == "next" && args.is_empty());
                    if is_next && !refs_in(&body[1..], it) && !refs_in(&b[i + 1..], it) {
                        let x = *x;
                        let label = *label;
                        let src = (**src).clone();
                        let rest = body[1..].to_vec();
                        b[i] = Stmt::Loop {
                            label,
                            kind: LoopKind::ForEach {
                                var: x,
                                iterable: src,
                            },
                            body: rest,
                        };
                        b.remove(i - 1);
                        return true;
                    }
                }
            }
        }
    }
    // 数组：`T[] a = e; int n = a.length; int k = 0; while (k < n) { T x = a[k]; ...; k++; }`
    if i >= 3 {
        let (
            Stmt::Local {
                var: arr,
                init: Some(src),
            },
            Stmt::Local {
                var: len,
                init: Some(Expr::ArrayLen(len_of)),
            },
            Stmt::Local {
                var: k,
                init: Some(Expr::Lit(Lit::Int(0))),
            },
        ) = (&b[i - 3], &b[i - 2], &b[i - 1])
        else {
            return false;
        };
        let (arr, len, k) = (*arr, *len, *k);
        if !(synthetic(arr) && synthetic(len) && synthetic(k)) || **len_of != Expr::Var(arr) {
            return false;
        }
        let Stmt::Loop {
            label,
            kind: LoopKind::While(Expr::Binary(BinOp::Lt, c1, c2)),
            body,
        } = &b[i]
        else {
            return false;
        };
        if **c1 != Expr::Var(k) || **c2 != Expr::Var(len) {
            return false;
        }
        let Some(Stmt::Local {
            var: x,
            init: Some(Expr::Index(a2, k2)),
        }) = body.first()
        else {
            return false;
        };
        if **a2 != Expr::Var(arr) || **k2 != Expr::Var(k) {
            return false;
        }
        let Some(update) = body.last().cloned() else {
            return false;
        };
        if !matches!(&update, Stmt::Expr(Expr::IncDec { target, inc: true, .. }) if **target == Expr::Var(k))
        {
            return false;
        }
        let label = *label;
        let x = *x;
        let mut rest = body[1..body.len() - 1].to_vec();
        if !strip_update_before_continue(&mut rest, &update, label, false) {
            return false;
        }
        if refs_in(&rest, arr)
            || refs_in(&rest, len)
            || refs_in(&rest, k)
            || refs_in(&b[i + 1..], arr)
            || refs_in(&b[i + 1..], k)
        {
            return false;
        }
        let src = src.clone();
        b[i] = Stmt::Loop {
            label,
            kind: LoopKind::ForEach {
                var: x,
                iterable: src,
            },
            body: rest,
        };
        b.drain(i - 3..i);
        return true;
    }
    false
}

// ---------------------------------------------------------------------------

/// 最后检查：中间形态不能留到输出里。
fn validate(block: &[Stmt]) -> Result<(), Fail> {
    let mut err = None;
    walk_exprs(block, &mut |e| {
        if err.is_none() {
            match e {
                Expr::Caught(_) => err = Some(Fail("异常对象没有落到 catch 变量里")),
                Expr::Uninit(..) => err = Some(Fail("未初始化的 new")),
                Expr::Cmp(..) => err = Some(Fail("比较指令的结果没有被条件消耗")),
                _ => {}
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    fn stmts(block: &[Stmt]) -> Result<(), Fail> {
        for s in block {
            match s {
                Stmt::MonitorEnter(_) | Stmt::MonitorExit(_) => {
                    return Err(Fail("monitorenter / monitorexit 没有配成 synchronized"))
                }
                Stmt::Expr(e) if !e.is_statement_expr() => return Err(Fail("表达式不能作为语句")),
                Stmt::Sync {
                    lock: Expr::Lit(Lit::Null),
                    ..
                } => return Err(Fail("synchronized 的锁对象丢失")),
                _ => {}
            }
            let mut r = Ok(());
            s.exprs(&mut |e| {
                if r.is_ok() && e.depth() > crate::MAX_EXPR_DEPTH {
                    r = Err(Fail("表达式太深"));
                }
                walk_expr_deep(e, &mut |x| {
                    if let Expr::Lambda { body, .. } = x {
                        if let LambdaBody::Block(b) = &**body {
                            if r.is_ok() {
                                r = stmts(b);
                            }
                        }
                    }
                });
            });
            r?;
            let mut r = Ok(());
            s.blocks(&mut |b| {
                if r.is_ok() {
                    r = stmts(b);
                }
            });
            r?;
        }
        Ok(())
    }
    stmts(block)
}

/// 结构化之后的全部整理。
pub fn run(
    body: &mut Vec<Stmt>,
    cx: &mut MethodCx<'_>,
    synthetic_params: &[VarId],
) -> Result<(), Fail> {
    finally_and_sync(body);
    for _ in 0..16 {
        let mut changed = ternaries(body, cx.vars);
        changed |= array_inits(body, cx.vars);
        changed |= inline_temps(body, cx.vars);
        if !changed {
            break;
        }
        cx.budget.spend(16)?;
    }
    infer_var_types(body, cx.class, cx.vars);
    let (_, ret) = types::method_types(cx.method);
    if cx.method.name == "<init>" && !cx.raw_ctor {
        ctor_cleanup(body, cx.class, synthetic_params);
    }
    if !cx.raw_ctor {
        outer_this(body, cx.class);
    }
    simplify(body, cx.class, cx.vars, ret.as_ref());
    unbox(body, cx.class, cx.vars, ret.as_ref());
    asserts(body);
    return_into_try(body, cx.vars);
    control_flow(body);
    if ret.is_none() {
        strip_trailing_return(body);
    }
    validate(body)?;
    crate::vars::declare(body, cx.vars);
    for_loops(body, cx.vars);
    strip_labels(body);
    Ok(())
}
