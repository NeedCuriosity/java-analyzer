//! 局部变量：声明放在包含全部使用的最内层语句块里；起名并消除作用域内的重名。

use classfile::signature::{BaseType, JType};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    bytecode::Kind,
    ir::*,
    names::{is_valid_identifier, KEYWORDS},
};

fn collect_refs_expr(e: &Expr, out: &mut FxHashSet<VarId>) {
    walk_expr_deep(e, &mut |x| {
        if let Expr::Var(v) = x {
            out.insert(*v);
        }
        if let Expr::Lambda { params, .. } = x {
            out.extend(params.iter().copied());
        }
    });
}

/// 语句（含嵌套块和 lambda）引用到的变量。
fn refs_deep(stmt: &Stmt, out: &mut FxHashSet<VarId>) {
    stmt.exprs(&mut |e| collect_refs_expr(e, out));
    match stmt {
        Stmt::Local { var, .. } => {
            out.insert(*var);
        }
        Stmt::Loop {
            kind: LoopKind::ForEach { var, .. },
            ..
        } => {
            out.insert(*var);
        }
        Stmt::Try { catches, .. } => {
            for c in catches {
                out.insert(c.var);
            }
        }
        _ => {}
    }
    stmt.blocks(&mut |b| {
        for s in b {
            refs_deep(s, out);
        }
    });
}

/// 已经有声明的变量：Local、catch、for-each、lambda 形参。
fn collect_declared(block: &[Stmt], out: &mut FxHashSet<VarId>) {
    for s in block {
        match s {
            Stmt::Local { var, .. } => {
                out.insert(*var);
            }
            Stmt::Loop {
                kind: LoopKind::ForEach { var, .. },
                ..
            } => {
                out.insert(*var);
            }
            Stmt::Try { catches, .. } => {
                for c in catches {
                    out.insert(c.var);
                }
            }
            _ => {}
        }
        s.exprs(&mut |e| {
            walk_expr_deep(e, &mut |x| {
                if let Expr::Lambda { params, body } = x {
                    out.extend(params.iter().copied());
                    if let LambdaBody::Block(b) = &**body {
                        collect_declared(b, out);
                    }
                }
            })
        });
        s.blocks(&mut |b| collect_declared(b, out));
    }
}

/// 给还没有声明的局部变量补上声明。
pub fn declare(body: &mut Vec<Stmt>, vars: &Vars) {
    let mut declared = FxHashSet::default();
    collect_declared(body, &mut declared);
    let mut all = FxHashSet::default();
    for s in body.iter() {
        refs_deep(s, &mut all);
    }
    let todo: FxHashSet<VarId> = all
        .into_iter()
        .filter(|v| !declared.contains(v) && !vars.get(*v).is_param)
        .collect();
    place(body, &todo);
}

fn place(block: &mut Vec<Stmt>, todo: &FxHashSet<VarId>) {
    if todo.is_empty() {
        return;
    }
    let n = block.len();
    let mut refs: Vec<FxHashSet<VarId>> = Vec::with_capacity(n);
    let mut direct: Vec<FxHashSet<VarId>> = Vec::with_capacity(n);
    for s in block.iter() {
        let mut r = FxHashSet::default();
        refs_deep(s, &mut r);
        r.retain(|v| todo.contains(v));
        let mut d = FxHashSet::default();
        s.exprs(&mut |e| collect_refs_expr(e, &mut d));
        refs.push(r);
        direct.push(d);
    }
    // (语句下标, 块下标) → 交给内层处理的变量
    let mut delegated: FxHashMap<(usize, usize), FxHashSet<VarId>> = FxHashMap::default();
    // 语句下标 → 在它之前声明的变量
    let mut here: Vec<(usize, VarId)> = Vec::new();
    let mut vars_sorted: Vec<VarId> = todo.iter().copied().collect();
    vars_sorted.sort_unstable();
    for v in vars_sorted {
        let users: Vec<usize> = (0..n).filter(|&i| refs[i].contains(&v)).collect();
        let Some(&first) = users.first() else {
            continue;
        };
        if users.len() == 1 && !direct[first].contains(&v) {
            // 只在一条语句的某一个内层块里用到
            let mut blocks_using = Vec::new();
            let mut idx = 0;
            block[first].blocks(&mut |b| {
                let mut r = FxHashSet::default();
                for s in b {
                    refs_deep(s, &mut r);
                }
                if r.contains(&v) {
                    blocks_using.push(idx);
                }
                idx += 1;
            });
            let is_switch = matches!(block[first], Stmt::Switch { .. });
            if blocks_using.len() == 1 && !is_switch {
                delegated
                    .entry((first, blocks_using[0]))
                    .or_default()
                    .insert(v);
                continue;
            }
            if blocks_using.len() == 1 && is_switch {
                // switch 的各个 case 共用一个作用域，可以放进用到它的那个 case
                delegated
                    .entry((first, blocks_using[0]))
                    .or_default()
                    .insert(v);
                continue;
            }
        }
        here.push((first, v));
    }
    for ((stmt_idx, block_idx), set) in delegated {
        let mut idx = 0;
        block[stmt_idx].blocks_mut(&mut |b| {
            if idx == block_idx {
                place(b, &set);
            }
            idx += 1;
        });
    }
    // 从后往前插入，下标保持有效
    here.sort_by_key(|(i, v)| (std::cmp::Reverse(*i), *v));
    for (i, v) in here {
        let merged = match &block[i] {
            Stmt::Expr(Expr::Assign(lhs, rhs)) if **lhs == Expr::Var(v) && !rhs.reads_var(v) => {
                Some((**rhs).clone())
            }
            _ => None,
        };
        match merged {
            Some(init) => {
                block[i] = Stmt::Local {
                    var: v,
                    init: Some(init),
                }
            }
            None => block.insert(i, Stmt::Local { var: v, init: None }),
        }
    }
}

// ---------------------------------------------------------------------------
// 起名

fn decapitalize(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.iter().all(|c| c.is_uppercase() || c.is_ascii_digit()) {
        return s.to_lowercase();
    }
    // `URLConnection` → `urlConnection`
    let mut upper = 0;
    while upper < chars.len() && chars[upper].is_uppercase() {
        upper += 1;
    }
    let cut = if upper > 1 { upper - 1 } else { upper };
    let mut out: String = chars[..cut].iter().collect::<String>().to_lowercase();
    out.extend(&chars[cut..]);
    out
}

/// 按类型给出变量名的词干。
pub fn base_name_for(ty: Option<&JType>, kind: Kind) -> String {
    match ty {
        Some(JType::Base(b)) => match b {
            BaseType::Boolean => "flag",
            BaseType::Byte => "b",
            BaseType::Char => "c",
            BaseType::Short => "s",
            BaseType::Int => "i",
            BaseType::Long => "l",
            BaseType::Float => "f",
            BaseType::Double => "d",
        }
        .to_owned(),
        Some(JType::Array(e)) => match &**e {
            JType::Base(_) => format!("{}Arr", base_name_for(Some(e), kind)),
            other => format!("{}s", base_name_for(Some(other), kind)),
        },
        Some(JType::TypeVar(n)) => n.to_lowercase(),
        Some(JType::Class { name, .. }) => {
            let simple = name.rsplit(['/', '$']).next().unwrap_or("obj");
            match simple {
                "String" => "s".to_owned(),
                "Object" => "obj".to_owned(),
                "Integer" => "n".to_owned(),
                "Iterator" => "it".to_owned(),
                "StringBuilder" | "StringBuffer" => "sb".to_owned(),
                s if s.ends_with("Exception") || s.ends_with("Error") || s == "Throwable" => {
                    "e".to_owned()
                }
                s if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) => {
                    "obj".to_owned()
                }
                s => decapitalize(s),
            }
        }
        None => match kind {
            Kind::Int => "i",
            Kind::Long => "l",
            Kind::Float => "f",
            Kind::Double => "d",
            Kind::Ref => "obj",
        }
        .to_owned(),
    }
}

struct Namer<'a> {
    vars: &'a mut Vars,
    scopes: Vec<FxHashSet<String>>,
    /// 方法里用过的全部名字（生成的新名字避开它们）
    taken: FxHashSet<String>,
    done: FxHashSet<VarId>,
}

impl Namer<'_> {
    fn fresh(&mut self, base: &str) -> String {
        let base = if is_valid_identifier(base) {
            base.to_owned()
        } else {
            format!("{base}1")
        };
        let base = if is_valid_identifier(&base) {
            base
        } else {
            "v".to_owned()
        };
        let visible =
            |n: &str, s: &Self| s.scopes.iter().any(|sc| sc.contains(n)) || s.taken.contains(n);
        if !visible(&base, self) {
            return base;
        }
        for k in 2.. {
            let cand = format!("{base}{k}");
            if !visible(&cand, self) {
                return cand;
            }
        }
        unreachable!()
    }

    fn declare(&mut self, v: VarId) {
        if !self.done.insert(v) {
            // 同一个变量的又一处声明（复制出来的代码），名字不变
            let name = self.vars.name(v).to_owned();
            self.scopes.last_mut().unwrap().insert(name);
            return;
        }
        let info = self.vars.get(v);
        let current = info.name.clone().filter(|n| is_valid_identifier(n));
        let name = match current {
            Some(n) if !self.scopes.iter().any(|s| s.contains(&n)) => n,
            Some(n) => self.fresh(&n),
            None => {
                let base = base_name_for(info.ty.as_ref(), info.kind);
                self.fresh(&base)
            }
        };
        self.taken.insert(name.clone());
        self.vars.get_mut(v).name = Some(name.clone());
        self.scopes.last_mut().unwrap().insert(name);
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Lambda { params, body } => {
                self.scopes.push(FxHashSet::default());
                for p in params {
                    self.declare(*p);
                }
                match &**body {
                    LambdaBody::Expr(b) => self.expr(b),
                    LambdaBody::Block(stmts) => self.block(stmts, false),
                }
                self.scopes.pop();
            }
            _ => e.children(&mut |c| self.expr(c)),
        }
    }

    fn block(&mut self, block: &[Stmt], push: bool) {
        if push {
            self.scopes.push(FxHashSet::default());
        }
        for s in block {
            self.stmt(s);
        }
        if push {
            self.scopes.pop();
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Local { var, init } => {
                if let Some(e) = init {
                    self.expr(e);
                }
                self.declare(*var);
            }
            Stmt::Loop { kind, body, .. } => {
                self.scopes.push(FxHashSet::default());
                match kind {
                    LoopKind::ForEach { var, iterable } => {
                        self.expr(iterable);
                        self.declare(*var);
                    }
                    LoopKind::For { init, cond, update } => {
                        for st in init {
                            self.stmt(st);
                        }
                        if let Some(c) = cond {
                            self.expr(c);
                        }
                        for u in update {
                            self.expr(u);
                        }
                    }
                    LoopKind::While(c) | LoopKind::DoWhile(c) => self.expr(c),
                }
                self.block(body, true);
                self.scopes.pop();
            }
            Stmt::Switch { expr, cases, .. } => {
                self.expr(expr);
                // 各个 case 共用一个作用域
                self.scopes.push(FxHashSet::default());
                for c in cases {
                    self.block(&c.body, false);
                }
                self.scopes.pop();
            }
            Stmt::Try {
                body,
                catches,
                finally,
            } => {
                self.block(body, true);
                for c in catches {
                    self.scopes.push(FxHashSet::default());
                    self.declare(c.var);
                    self.block(&c.body, false);
                    self.scopes.pop();
                }
                if let Some(f) = finally {
                    self.block(f, true);
                }
            }
            other => {
                other.exprs(&mut |e| self.expr(e));
                other.blocks(&mut |b| self.block(b, true));
            }
        }
    }
}

/// 给方法里的全部局部变量起名。形参的名字已经定好。
pub fn name_all(body: &[Stmt], vars: &mut Vars, params: &[VarId]) {
    let mut namer = Namer {
        vars,
        scopes: vec![FxHashSet::default()],
        taken: FxHashSet::default(),
        done: FxHashSet::default(),
    };
    for &p in params {
        let name = namer.vars.name(p).to_owned();
        namer.done.insert(p);
        namer.taken.insert(name.clone());
        namer.scopes[0].insert(name);
    }
    // LVT 里的名字先占上，生成的名字不和它们撞
    for info in namer.vars.vars.iter() {
        if let Some(n) = &info.name {
            if info.named_by_lvt && is_valid_identifier(n) {
                namer.taken.insert(n.clone());
            }
        }
    }
    namer.scopes.push(FxHashSet::default());
    namer.block(body, false);
    // 没有出现在声明里的变量（不应该有）也给个名字，保证输出合法
    for i in 0..namer.vars.vars.len() {
        let v = i as VarId;
        let info = namer.vars.get(v);
        if info.name.as_deref().is_none_or(|n| !is_valid_identifier(n)) {
            let base = base_name_for(info.ty.as_ref(), info.kind);
            let name = namer.fresh(&base);
            namer.taken.insert(name.clone());
            namer.vars.get_mut(v).name = Some(name);
        }
    }
    let _ = KEYWORDS;
}
