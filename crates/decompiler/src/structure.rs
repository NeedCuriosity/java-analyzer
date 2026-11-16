//! 结构化：把控制流图还原成 if / 循环 / switch / try 的语句树。
//!
//! 从入口沿控制流逐块输出语句。遇到循环头时输出 `while (true)`，循环头和循环出口
//! 分别作为 `continue` / `break` 的目标；条件分支在后支配点汇合；try 区间按异常表分组，
//! 由外向内展开。到不了任何已知目标、又跳出当前区域的边让整个方法放弃反编译。

use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    cfg::{negate, Analysis},
    ir::*,
    translate::{Cfg, Term},
    Budget, Fail,
};

struct TryGroup {
    start_block: usize,
    end_pc: u32,
    start_pc: u32,
    /// (捕获的类型，`None` 表示一切；处理器块)
    handlers: Vec<(Vec<Option<String>>, usize)>,
    body: Rc<Vec<bool>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TKind {
    Break,
    Continue,
}

#[derive(Clone)]
struct Target {
    node: usize,
    kind: TKind,
    construct: LabelId,
    is_loop: bool,
}

#[derive(Clone, Default)]
struct Ctx {
    stop: Option<usize>,
    targets: Vec<Target>,
    region: Option<Rc<Vec<bool>>>,
}

pub struct Structurer<'a> {
    cfg: &'a Cfg,
    a: &'a Analysis,
    budget: &'a mut Budget,
    groups: Vec<TryGroup>,
    group_at: FxHashMap<usize, Vec<usize>>,
    next_label: &'a mut LabelId,
    pub used_labels: FxHashSet<LabelId>,
    loop_info: FxHashMap<usize, (Option<usize>, Rc<Vec<bool>>)>,
    handler_regions: FxHashMap<usize, Rc<Vec<bool>>>,
}

impl<'a> Structurer<'a> {
    pub fn new(
        cfg: &'a Cfg,
        a: &'a Analysis,
        budget: &'a mut Budget,
        next_label: &'a mut LabelId,
    ) -> Result<Self, Fail> {
        let mut s = Structurer {
            cfg,
            a,
            budget,
            groups: Vec::new(),
            group_at: FxHashMap::default(),
            next_label,
            used_labels: FxHashSet::default(),
            loop_info: FxHashMap::default(),
            handler_regions: FxHashMap::default(),
        };
        s.build_groups()?;
        Ok(s)
    }

    fn handler_region(&mut self, h: usize) -> Rc<Vec<bool>> {
        if let Some(r) = self.handler_regions.get(&h) {
            return r.clone();
        }
        let r: Rc<Vec<bool>> = Rc::new(
            (0..self.a.n)
                .map(|v| self.a.live[v] && self.a.dominates(h, v))
                .collect(),
        );
        self.handler_regions.insert(h, r.clone());
        r
    }

    fn build_groups(&mut self) -> Result<(), Fail> {
        // 每个处理器：保护的区间和捕获的类型
        let mut per_handler: Vec<(usize, Vec<(u32, u32)>, Vec<Option<String>>)> = Vec::new();
        for e in &self.cfg.exceptions {
            let idx = match per_handler.iter().position(|(h, _, _)| *h == e.handler) {
                Some(i) => i,
                None => {
                    per_handler.push((e.handler, Vec::new(), Vec::new()));
                    per_handler.len() - 1
                }
            };
            per_handler[idx].1.push((e.start, e.end));
            if !per_handler[idx].2.contains(&e.catch_type) {
                per_handler[idx].2.push(e.catch_type.clone());
            }
        }
        // 区间相同的处理器属于同一个 try
        let mut keyed: Vec<((u32, u32), Vec<(Vec<Option<String>>, usize)>)> = Vec::new();
        for (h, ranges, mut types) in per_handler {
            let start = ranges.iter().map(|r| r.0).min().unwrap();
            let end = ranges.iter().map(|r| r.1).max().unwrap();
            if types.contains(&None) {
                types = vec![None];
            }
            match keyed.iter_mut().find(|(k, _)| *k == (start, end)) {
                Some((_, hs)) => hs.push((types, h)),
                None => keyed.push(((start, end), vec![(types, h)])),
            }
        }
        for ((start, end), handlers) in keyed {
            let Some(start_block) =
                (0..self.a.n).find(|&b| self.a.live[b] && self.cfg.blocks[b].pc == start)
            else {
                continue;
            };
            let regions: Vec<Rc<Vec<bool>>> = handlers
                .iter()
                .map(|(_, h)| self.handler_region(*h))
                .collect();
            let body: Vec<bool> = (0..self.a.n)
                .map(|v| {
                    let pc = self.cfg.blocks[v].pc;
                    self.a.live[v] && pc >= start && pc < end && !regions.iter().any(|r| r[v])
                })
                .collect();
            for v in 0..self.a.n {
                if body[v] && !self.a.dominates(start_block, v) {
                    return Err(Fail("跳进了 try 区间中间"));
                }
            }
            self.groups.push(TryGroup {
                start_block,
                start_pc: start,
                end_pc: end,
                handlers,
                body: Rc::new(body),
            });
        }
        // 区间必须嵌套或不相交
        for i in 0..self.groups.len() {
            for j in i + 1..self.groups.len() {
                let (a, b) = (&self.groups[i], &self.groups[j]);
                let disjoint = a.end_pc <= b.start_pc || b.end_pc <= a.start_pc;
                let nested = (a.start_pc <= b.start_pc && b.end_pc <= a.end_pc)
                    || (b.start_pc <= a.start_pc && a.end_pc <= b.end_pc);
                if !disjoint && !nested {
                    return Err(Fail("交叉的 try 区间"));
                }
            }
        }
        for (i, g) in self.groups.iter().enumerate() {
            self.group_at.entry(g.start_block).or_default().push(i);
        }
        let groups = &self.groups;
        for list in self.group_at.values_mut() {
            list.sort_by_key(|&g| std::cmp::Reverse(groups[g].end_pc - groups[g].start_pc));
        }
        Ok(())
    }

    fn new_label(&mut self) -> LabelId {
        *self.next_label += 1;
        *self.next_label
    }

    pub fn run(&mut self) -> Result<Vec<Stmt>, Fail> {
        let ctx = Ctx::default();
        self.seq(0, &ctx, None, &[])
    }

    fn jump(&mut self, node: usize, ctx: &Ctx) -> Option<Stmt> {
        for i in (0..ctx.targets.len()).rev() {
            let t = &ctx.targets[i];
            if t.node != node {
                continue;
            }
            let inner = &ctx.targets[i + 1..];
            let need_label = match t.kind {
                TKind::Break => inner.iter().any(|x| x.construct != t.construct),
                TKind::Continue => inner
                    .iter()
                    .any(|x| x.is_loop && x.construct != t.construct),
            };
            let label = need_label.then_some(t.construct);
            if need_label {
                self.used_labels.insert(t.construct);
            }
            return Some(match t.kind {
                TKind::Break => Stmt::Break(label),
                TKind::Continue => Stmt::Continue(label),
            });
        }
        None
    }

    fn is_target(&self, node: usize, ctx: &Ctx) -> bool {
        ctx.targets.iter().any(|t| t.node == node)
    }

    fn in_region(&self, node: usize, ctx: &Ctx) -> bool {
        ctx.region.as_ref().is_none_or(|r| r[node])
    }

    /// 只有 return / throw 的小块：可以复制到每个跳到它的地方。
    fn is_small_exit(&self, node: usize) -> bool {
        let blk = &self.cfg.blocks[node];
        matches!(blk.term, Term::Return(_) | Term::Throw(_))
            && (blk.stmts.is_empty() || (blk.stmts.len() <= 2 && blk.handlers.is_empty()))
            && !self.cfg.is_handler(node)
    }

    fn merge(&self, node: usize, ctx: &Ctx) -> Option<usize> {
        let m = self.a.merge_point(node)?;
        (Some(m) == ctx.stop || self.is_target(m, ctx) || self.in_region(m, ctx)).then_some(m)
    }

    fn seq(
        &mut self,
        start: usize,
        ctx: &Ctx,
        open_loop: Option<usize>,
        open_trys: &[usize],
    ) -> Result<Vec<Stmt>, Fail> {
        let mut out = Vec::new();
        let mut node = start;
        let mut first = true;
        loop {
            self.budget.spend(1)?;
            if Some(node) == ctx.stop {
                return Ok(out);
            }
            // 循环体从循环头开始，循环头本身是 continue 的目标，第一次不算跳转
            let skip_loop = first && open_loop == Some(node);
            if !skip_loop {
                if let Some(j) = self.jump(node, ctx) {
                    out.push(j);
                    return Ok(out);
                }
            }
            if !self.a.live[node] {
                return Err(Fail("走到了死块"));
            }
            if !self.in_region(node, ctx) {
                if self.is_small_exit(node) {
                    let blk = &self.cfg.blocks[node];
                    out.extend(blk.stmts.iter().cloned());
                    out.push(match &blk.term {
                        Term::Return(e) => Stmt::Return(e.clone()),
                        Term::Throw(e) => Stmt::Throw(e.clone()),
                        _ => unreachable!(),
                    });
                    return Ok(out);
                }
                return Err(Fail("跳出了可以结构化的区域"));
            }
            let opened: &[usize] = if first { open_trys } else { &[] };
            first = false;

            let pending = self
                .group_at
                .get(&node)
                .and_then(|list| list.iter().copied().find(|g| !opened.contains(g)));
            let is_loop = !skip_loop && self.a.loops[node].is_some();
            if let Some(g) = pending {
                let loop_inside = is_loop && {
                    let body = self.a.loops[node].as_ref().unwrap();
                    let gb = &self.groups[g].body;
                    (0..self.a.n).all(|v| !body[v] || gb[v])
                };
                if !is_loop || loop_inside {
                    let mut now_open = opened.to_vec();
                    now_open.push(g);
                    let (stmt, follow) = self.emit_try(
                        g,
                        node,
                        ctx,
                        if skip_loop { Some(node) } else { None },
                        &now_open,
                    )?;
                    out.push(stmt);
                    match follow {
                        Some(f) => {
                            node = f;
                            continue;
                        }
                        None => return Ok(out),
                    }
                }
            }
            if is_loop {
                let (stmt, follow) = self.emit_loop(node, ctx, opened)?;
                out.push(stmt);
                match follow {
                    Some(f) => {
                        node = f;
                        continue;
                    }
                    None => return Ok(out),
                }
            }

            let blk = &self.cfg.blocks[node];
            out.extend(blk.stmts.iter().cloned());
            match blk.term.clone() {
                Term::Return(e) => {
                    out.push(Stmt::Return(e));
                    return Ok(out);
                }
                Term::Throw(e) => {
                    out.push(Stmt::Throw(e));
                    return Ok(out);
                }
                Term::Goto(t) => node = t,
                Term::Dead => return Err(Fail("走到了死块")),
                Term::If { cond, then, els } => {
                    if then == els {
                        if !cond.is_pure() {
                            return Err(Fail("两个分支相同的条件有副作用"));
                        }
                        node = then;
                        continue;
                    }
                    let jt = if Some(then) != ctx.stop {
                        self.jump(then, ctx)
                    } else {
                        None
                    };
                    let je = if Some(els) != ctx.stop {
                        self.jump(els, ctx)
                    } else {
                        None
                    };
                    match (jt, je) {
                        (Some(j), None) => {
                            out.push(Stmt::If {
                                cond,
                                then: vec![j],
                                els: Vec::new(),
                            });
                            node = els;
                            continue;
                        }
                        (None, Some(j)) => {
                            out.push(Stmt::If {
                                cond: negate(cond),
                                then: vec![j],
                                els: Vec::new(),
                            });
                            node = then;
                            continue;
                        }
                        (Some(j1), Some(j2)) => {
                            out.push(Stmt::If {
                                cond,
                                then: vec![j1],
                                els: vec![j2],
                            });
                            return Ok(out);
                        }
                        (None, None) => {}
                    }
                    // 源码的 then 分支通常紧跟在条件后面（顺序执行到的那个）
                    let (cond, then, els) = (negate(cond), els, then);
                    // 汇合点是 break / continue 的目标时，让两个分支各自写出跳转
                    let m = self
                        .merge(node, ctx)
                        .filter(|&m| Some(m) == ctx.stop || !self.is_target(m, ctx));
                    let mut bctx = ctx.clone();
                    bctx.stop = m.or(ctx.stop);
                    let t = self.seq(then, &bctx, None, &[])?;
                    if m.is_none() && ends_with_jump(&t) {
                        // then 分支不会落下来：else 分支直接接在 if 后面（避免嵌套过深）
                        out.push(Stmt::If {
                            cond,
                            then: t,
                            els: Vec::new(),
                        });
                        node = els;
                        continue;
                    }
                    let e = self.seq(els, &bctx, None, &[])?;
                    if m.is_none() && ends_with_jump(&e) {
                        out.push(Stmt::If {
                            cond: negate(cond),
                            then: e,
                            els: Vec::new(),
                        });
                        out.extend(t);
                        return Ok(out);
                    }
                    out.push(Stmt::If {
                        cond,
                        then: t,
                        els: e,
                    });
                    match m {
                        Some(m) => node = m,
                        None => return Ok(out),
                    }
                }
                Term::Switch {
                    expr,
                    cases,
                    default,
                } => {
                    let m = self.merge(node, ctx);
                    let label = self.new_label();
                    let mut sctx = ctx.clone();
                    // 没有汇合点时也要占一层，内层不带标号的 break 会落到这个 switch 上
                    sctx.targets.push(Target {
                        node: m.unwrap_or(usize::MAX),
                        kind: TKind::Break,
                        construct: label,
                        is_loop: false,
                    });
                    let mut targets: Vec<usize> = cases.iter().map(|(_, t)| *t).collect();
                    targets.push(default);
                    targets.sort_by_key(|&t| (self.cfg.blocks[t].pc, t));
                    targets.dedup();
                    targets.retain(|&t| Some(t) != m);
                    let mut out_cases = Vec::new();
                    for (i, &t) in targets.iter().enumerate() {
                        let mut labels: Vec<Option<CaseLabel>> = cases
                            .iter()
                            .filter(|(_, x)| *x == t && t != default)
                            .map(|(v, _)| Some(CaseLabel::Int(*v)))
                            .collect();
                        if default == t {
                            labels.push(None);
                        }
                        sctx.stop = targets.get(i + 1).copied().or(m).or(ctx.stop);
                        let body = self.seq(t, &sctx, None, &[])?;
                        out_cases.push(Case { labels, body });
                    }
                    if let Some(m) = m {
                        if default != m {
                            let labels: Vec<Option<CaseLabel>> = cases
                                .iter()
                                .filter(|(_, x)| *x == m)
                                .map(|(v, _)| Some(CaseLabel::Int(*v)))
                                .collect();
                            if !labels.is_empty() {
                                out_cases.push(Case {
                                    labels,
                                    body: vec![Stmt::Break(None)],
                                });
                            }
                        }
                    }
                    out.push(Stmt::Switch {
                        label: Some(label),
                        expr,
                        cases: out_cases,
                    });
                    match m {
                        Some(m) => node = m,
                        None => return Ok(out),
                    }
                }
            }
        }
    }

    fn emit_try(
        &mut self,
        g: usize,
        node: usize,
        ctx: &Ctx,
        open_loop: Option<usize>,
        now_open: &[usize],
    ) -> Result<(Stmt, Option<usize>), Fail> {
        let follow = self.try_follow(g);
        let handlers = self.groups[g].handlers.clone();
        let regions: Vec<Rc<Vec<bool>>> = handlers
            .iter()
            .map(|(_, h)| self.handler_region(*h))
            .collect();
        // try 体：区间里的块，加上从它们出发、到 follow 之前能走到的块（javac 内联的 finally 副本）
        let mut body_region = vec![false; self.a.n];
        let mut stack = vec![node];
        let interval = self.groups[g].body.clone();
        for v in 0..self.a.n {
            if interval[v] {
                stack.push(v);
            }
        }
        while let Some(v) = stack.pop() {
            if body_region[v]
                || Some(v) == follow
                || Some(v) == ctx.stop
                || self.is_target(v, ctx)
                || !self.in_region(v, ctx)
                || regions.iter().any(|r| r[v])
                || !self.a.dominates(node, v)
            {
                continue;
            }
            body_region[v] = true;
            stack.extend(self.a.succ[v].iter().copied());
        }
        let mut bctx = ctx.clone();
        bctx.stop = follow.or(ctx.stop);
        bctx.region = Some(Rc::new(body_region));
        let body = self.seq(node, &bctx, open_loop, now_open)?;
        let mut catches = Vec::new();
        for ((types, h), region) in handlers.into_iter().zip(regions) {
            let mut hctx = ctx.clone();
            hctx.stop = follow.or(ctx.stop);
            hctx.region = Some(region);
            let hbody = self.seq(h, &hctx, None, &[])?;
            let var = *self
                .cfg
                .catch_vars
                .get(&h)
                .ok_or(Fail("处理器没有 catch 变量"))?;
            catches.push(Catch {
                types: types.into_iter().flatten().collect(),
                var,
                body: hbody,
            });
        }
        Ok((
            Stmt::Try {
                body,
                catches,
                finally: None,
            },
            follow,
        ))
    }

    /// 后支配链（不含虚拟出口）。
    fn pdom_chain(&self, mut x: usize) -> Vec<usize> {
        let mut v = vec![x];
        while let Some(p) = self.a.merge_point(x) {
            if v.len() > self.a.n {
                break;
            }
            v.push(p);
            x = p;
        }
        v
    }

    /// try 语句之后接着执行的块：try 体的后支配链上第一个在区间外、又是各个
    /// 会正常结束的处理器的汇合点的块。
    fn try_follow(&mut self, g: usize) -> Option<usize> {
        let start = self.groups[g].start_block;
        let interval = self.groups[g].body.clone();
        let handlers: Vec<usize> = self.groups[g].handlers.iter().map(|(_, h)| *h).collect();
        let regions: Vec<Rc<Vec<bool>>> =
            handlers.iter().map(|h| self.handler_region(*h)).collect();
        let outside = |v: usize| !interval[v] && !regions.iter().any(|r| r[v]);
        let start_chain = self.pdom_chain(start);
        let mut candidates: Vec<usize> = start_chain
            .iter()
            .copied()
            .filter(|&v| outside(v))
            .collect();
        let mut handler_chains = Vec::new();
        for &h in &handlers {
            let chain: Vec<usize> = self
                .pdom_chain(h)
                .into_iter()
                .filter(|&v| outside(v))
                .collect();
            if !chain.is_empty() {
                handler_chains.push(chain);
            }
        }
        if candidates.is_empty() {
            // try 体总是 return / throw：看处理器
            let mut iter = handler_chains.into_iter();
            let mut c = iter.next()?;
            for other in iter {
                c.retain(|v| other.contains(v));
            }
            return c.first().copied();
        }
        for chain in &handler_chains {
            if chain.iter().any(|v| candidates.contains(v)) {
                candidates.retain(|v| chain.contains(v));
            }
        }
        // 只有 try 体会走到的 return 块（javac 把 return 指令放在保护区间外）并进 try 体
        if let Some(&f) = candidates.first() {
            if self.is_small_exit(f) && !handler_chains.iter().any(|c| c.contains(&f)) {
                return None;
            }
        }
        candidates.first().copied()
    }

    fn loop_info(&mut self, h: usize) -> (Option<usize>, Rc<Vec<bool>>) {
        if let Some(info) = self.loop_info.get(&h) {
            return info.clone();
        }
        let n = self.a.n;
        let body = self.a.loops[h].clone().unwrap();
        let outside = |b: &Vec<bool>, s: usize| !b[s];
        let mut exits: Vec<usize> = Vec::new();
        for v in 0..n {
            if body[v] {
                for &s in &self.a.succ[v] {
                    if outside(&body, s) && !exits.contains(&s) {
                        exits.push(s);
                    }
                }
            }
        }
        let cond_exit = |b: usize| -> Option<usize> {
            if let Term::If { then, els, .. } = self.cfg.blocks[b].term {
                match (body[then], body[els]) {
                    (true, false) => Some(els),
                    (false, true) => Some(then),
                    _ => None,
                }
            } else {
                None
            }
        };
        let mut follow = cond_exit(h);
        if follow.is_none() {
            let mut latches: Vec<usize> = self.a.pred[h]
                .iter()
                .copied()
                .filter(|&p| body[p])
                .collect();
            latches.sort_by_key(|&l| std::cmp::Reverse(self.cfg.blocks[l].pc));
            follow = latches.iter().find_map(|&l| cond_exit(l));
        }
        let chain = |mut x: usize| {
            let mut v = vec![x];
            let mut steps = 0;
            while let Some(p) = self.a.merge_point(x) {
                v.push(p);
                x = p;
                steps += 1;
                if steps > n {
                    break;
                }
            }
            v
        };
        // 所有（会汇合的）出口的公共后支配点
        let merging: Vec<usize> = exits
            .iter()
            .copied()
            .filter(|&x| self.a.merge_point(x).is_some() || Some(x) == follow)
            .collect();
        let common = merging.split_first().and_then(|(first, rest)| {
            let mut common = chain(*first);
            for &x in rest {
                let c = chain(x);
                common.retain(|v| c.contains(v));
            }
            common.first().copied().filter(|&c| !body[c])
        });
        match follow {
            None => follow = common,
            // 条件出口后面还有只属于循环的代码再汇合到别的 break 的目标
            // （`for (;;) { if (c) { X; break; } ... break; }`）：follow 取汇合点，X 并进循环
            Some(f) if merging.len() > 1 => {
                if let Some(c) = common {
                    // f 到 c 之间的块都只能从循环里到达（否则 c 已经在外层结构里了）
                    let fc = chain(f);
                    let only_from_loop = fc
                        .iter()
                        .position(|&v| v == c)
                        .is_some_and(|k| fc[..k].iter().all(|&v| self.a.dominates(h, v)));
                    if c != f && only_from_loop {
                        follow = Some(c);
                    }
                }
            }
            Some(_) => {}
        }
        // 其余出口（return、跳到外层）一路走到 follow 之前的部分并入循环区域
        let mut region = body.clone();
        let mut stack: Vec<usize> = exits
            .iter()
            .copied()
            .filter(|&x| Some(x) != follow)
            .collect();
        let mut steps = 0;
        while let Some(v) = stack.pop() {
            steps += 1;
            if steps > 4 * n + 16 {
                break;
            }
            if region[v] || Some(v) == follow || !self.a.dominates(h, v) {
                continue;
            }
            region[v] = true;
            stack.extend(self.a.succ[v].iter().copied());
        }
        let info = (follow, Rc::new(region));
        self.loop_info.insert(h, info.clone());
        info
    }

    fn emit_loop(
        &mut self,
        h: usize,
        ctx: &Ctx,
        opened: &[usize],
    ) -> Result<(Stmt, Option<usize>), Fail> {
        let (follow, region) = self.loop_info(h);
        let label = self.new_label();
        let mut lctx = ctx.clone();
        lctx.stop = None;
        lctx.region = Some(region);
        lctx.targets.push(Target {
            node: h,
            kind: TKind::Continue,
            construct: label,
            is_loop: true,
        });
        if let Some(f) = follow {
            lctx.targets.push(Target {
                node: f,
                kind: TKind::Break,
                construct: label,
                is_loop: true,
            });
        }
        let body = self.seq(h, &lctx, Some(h), opened)?;
        Ok((
            Stmt::Loop {
                label: Some(label),
                kind: LoopKind::While(Expr::Lit(Lit::Bool(true))),
                body,
            },
            follow,
        ))
    }
}
