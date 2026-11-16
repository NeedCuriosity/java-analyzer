//! 控制流图的整理和分析：空跳转块的穿透、短路条件的合并、支配树、后支配树、自然循环。

use crate::{
    ir::*,
    translate::{Cfg, Term},
    Fail,
};

/// 删掉只有一个 `goto` 的空块，把指向它的边直接连到目标。
pub fn thread_jumps(cfg: &mut Cfg) {
    let n = cfg.blocks.len();
    let protected_starts: Vec<u32> = cfg.exceptions.iter().map(|e| e.start).collect();
    let mut forward: Vec<usize> = (0..n).collect();
    for b in 1..n {
        let blk = &cfg.blocks[b];
        if blk.stmts.is_empty() && !cfg.is_handler(b) && !protected_starts.contains(&blk.pc) {
            if let Term::Goto(t) = blk.term {
                if t != b {
                    forward[b] = t;
                }
            }
        }
    }
    let resolve = |mut b: usize| {
        let mut steps = 0;
        while forward[b] != b && steps < n {
            b = forward[b];
            steps += 1;
        }
        b
    };
    for block in cfg.blocks.iter_mut().take(n) {
        block.term.succs_mut(&mut |t| *t = resolve(*t));
    }
    remove_unreachable(cfg);
}

/// 从入口（含异常边）到不了的块标成死块。
pub fn remove_unreachable(cfg: &mut Cfg) {
    let n = cfg.blocks.len();
    let mut seen = vec![false; n];
    let mut stack = vec![0usize];
    while let Some(b) = stack.pop() {
        if seen[b] || matches!(cfg.blocks[b].term, Term::Dead) {
            continue;
        }
        seen[b] = true;
        stack.extend(cfg.blocks[b].term.succs());
        stack.extend(cfg.blocks[b].handlers.iter().copied());
    }
    for b in 0..n {
        if !seen[b] {
            cfg.blocks[b].term = Term::Dead;
            cfg.blocks[b].stmts.clear();
            cfg.blocks[b].handlers.clear();
        }
    }
    cfg.exceptions.retain(|e| seen[e.handler]);
}

fn preds_count(cfg: &Cfg) -> Vec<usize> {
    let mut count = vec![0usize; cfg.blocks.len()];
    for blk in &cfg.blocks {
        for s in blk.term.succs() {
            count[s] += 1;
        }
    }
    count
}

/// 合并短路求值：`if (a) goto T; if (b) goto T; goto E` → `if (a || b) goto T; goto E`。
pub fn merge_conditions(cfg: &mut Cfg) {
    let n = cfg.blocks.len();
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 64 {
        changed = false;
        rounds += 1;
        let preds = preds_count(cfg);
        for b in 0..n {
            let Term::If {
                then: t1, els: e1, ..
            } = cfg.blocks[b].term
            else {
                continue;
            };
            for x in [e1, t1] {
                if x == b || preds[x] != 1 || cfg.is_handler(x) {
                    continue;
                }
                let xb = &cfg.blocks[x];
                if !xb.stmts.is_empty() || xb.handlers != cfg.blocks[b].handlers {
                    continue;
                }
                let Term::If {
                    cond: ref c2,
                    then: t2,
                    els: e2,
                } = xb.term
                else {
                    continue;
                };
                if t2 == x || e2 == x {
                    continue;
                }
                let c2 = c2.clone();
                let Term::If { cond: c1, .. } =
                    std::mem::replace(&mut cfg.blocks[b].term, Term::Dead)
                else {
                    unreachable!()
                };
                let new = if x == e1 {
                    if t2 == t1 {
                        Some((Expr::bin(BinOp::LOr, c1.clone(), c2), t1, e2))
                    } else if e2 == t1 {
                        Some((Expr::bin(BinOp::LOr, c1.clone(), negate(c2)), t1, t2))
                    } else {
                        None
                    }
                } else if e2 == e1 {
                    Some((Expr::bin(BinOp::LAnd, c1.clone(), c2), t2, e1))
                } else if t2 == e1 {
                    Some((Expr::bin(BinOp::LAnd, c1.clone(), negate(c2)), e2, e1))
                } else {
                    None
                };
                match new {
                    Some((cond, then, els)) => {
                        cfg.blocks[b].term = Term::If { cond, then, els };
                        cfg.blocks[x].term = Term::Dead;
                        cfg.blocks[x].handlers.clear();
                        changed = true;
                        break;
                    }
                    None => {
                        cfg.blocks[b].term = Term::If {
                            cond: c1,
                            then: t1,
                            els: e1,
                        };
                    }
                }
            }
        }
    }
}

/// 条件取反，尽量写成不带 `!` 的形式。
pub fn negate(e: Expr) -> Expr {
    match e {
        Expr::Unary(UnOp::Not, inner) => *inner,
        Expr::Binary(op, a, b) if op.is_comparison() => {
            Expr::Binary(op.negate_comparison().unwrap(), a, b)
        }
        Expr::Binary(BinOp::LAnd, a, b) => Expr::bin(BinOp::LOr, negate(*a), negate(*b)),
        Expr::Binary(BinOp::LOr, a, b) => Expr::bin(BinOp::LAnd, negate(*a), negate(*b)),
        Expr::Lit(Lit::Bool(v)) => Expr::Lit(Lit::Bool(!v)),
        e => Expr::not(e),
    }
}

/// 结构化需要的图信息。
pub struct Analysis {
    pub n: usize,
    pub live: Vec<bool>,
    pub succ: Vec<Vec<usize>>,
    pub pred: Vec<Vec<usize>>,
    /// 含异常边的前驱
    pub all_pred: Vec<Vec<usize>>,
    pub idom: Vec<usize>,
    pre: Vec<u32>,
    post: Vec<u32>,
    /// 后支配：`n` 表示虚拟出口
    pub ipdom: Vec<usize>,
    pub rpo_num: Vec<usize>,
    /// 循环头 → 循环体
    pub loops: Vec<Option<Vec<bool>>>,
}

const NONE: usize = usize::MAX;

fn reverse_postorder(n: usize, entry: usize, succ: &dyn Fn(usize) -> Vec<usize>) -> Vec<usize> {
    let mut seen = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut stack: Vec<(usize, Vec<usize>, usize)> = Vec::new();
    seen[entry] = true;
    stack.push((entry, succ(entry), 0));
    while let Some((node, succs, i)) = stack.last_mut() {
        if *i < succs.len() {
            let s = succs[*i];
            *i += 1;
            if !seen[s] {
                seen[s] = true;
                let ss = succ(s);
                stack.push((s, ss, 0));
            }
        } else {
            order.push(*node);
            stack.pop();
        }
    }
    order.reverse();
    order
}

/// Cooper–Harvey–Kennedy 迭代求直接支配者。
fn compute_idom(
    n: usize,
    entry: usize,
    succ: &dyn Fn(usize) -> Vec<usize>,
    pred: &dyn Fn(usize) -> Vec<usize>,
) -> (Vec<usize>, Vec<usize>) {
    let rpo = reverse_postorder(n, entry, succ);
    let mut num = vec![NONE; n];
    for (i, &v) in rpo.iter().enumerate() {
        num[v] = i;
    }
    let mut idom = vec![NONE; n];
    idom[entry] = entry;
    let mut changed = true;
    while changed {
        changed = false;
        for &v in rpo.iter().skip(1) {
            let mut new = NONE;
            for p in pred(v) {
                if num[p] == NONE || idom[p] == NONE {
                    continue;
                }
                new = if new == NONE {
                    p
                } else {
                    let (mut a, mut b) = (p, new);
                    while a != b {
                        while num[a] > num[b] {
                            a = idom[a];
                        }
                        while num[b] > num[a] {
                            b = idom[b];
                        }
                    }
                    a
                };
            }
            if new != NONE && idom[v] != new {
                idom[v] = new;
                changed = true;
            }
        }
    }
    (idom, num)
}

impl Analysis {
    pub fn new(cfg: &Cfg) -> Result<Analysis, Fail> {
        let n = cfg.blocks.len();
        let live: Vec<bool> = cfg
            .blocks
            .iter()
            .map(|b| !matches!(b.term, Term::Dead))
            .collect();
        let mut succ = vec![Vec::new(); n];
        let mut pred = vec![Vec::new(); n];
        let mut all_pred = vec![Vec::new(); n];
        let mut all_succ = vec![Vec::new(); n];
        for b in 0..n {
            if !live[b] {
                continue;
            }
            let mut s = cfg.blocks[b].term.succs();
            s.dedup();
            for &t in &s {
                if !pred[t].contains(&b) {
                    pred[t].push(b);
                }
                if !all_pred[t].contains(&b) {
                    all_pred[t].push(b);
                }
            }
            all_succ[b] = s.clone();
            for &h in &cfg.blocks[b].handlers {
                if !all_pred[h].contains(&b) {
                    all_pred[h].push(b);
                }
                if !all_succ[b].contains(&h) {
                    all_succ[b].push(h);
                }
            }
            succ[b] = s;
        }
        let (idom, rpo_num) =
            compute_idom(n, 0, &|v| all_succ[v].clone(), &|v| all_pred[v].clone());

        // 支配树的先序 / 后序编号，O(1) 判断支配关系
        let mut children = vec![Vec::new(); n];
        for v in 0..n {
            if v != 0 && idom[v] != NONE {
                children[idom[v]].push(v);
            }
        }
        let mut pre = vec![0u32; n];
        let mut post = vec![0u32; n];
        let mut counter = 0u32;
        let mut stack = vec![(0usize, 0usize)];
        pre[0] = 0;
        while let Some((v, i)) = stack.last_mut() {
            if *i == 0 {
                pre[*v] = counter;
                counter += 1;
            }
            if *i < children[*v].len() {
                let c = children[*v][*i];
                *i += 1;
                stack.push((c, 0));
            } else {
                post[*v] = counter;
                counter += 1;
                stack.pop();
            }
        }

        // 后支配：反向图，虚拟出口 n 连到所有 return / throw，以及走不到出口的块
        let exit = n;
        let mut rsucc: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
        let mut rpred: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
        for b in 0..n {
            if !live[b] {
                continue;
            }
            for &t in &succ[b] {
                rsucc[t].push(b);
                rpred[b].push(t);
            }
            if matches!(cfg.blocks[b].term, Term::Return(_) | Term::Throw(_)) {
                rsucc[exit].push(b);
                rpred[b].push(exit);
            }
        }
        // 反向可达性
        let mut reach = vec![false; n + 1];
        let mut st = vec![exit];
        while let Some(v) = st.pop() {
            if reach[v] {
                continue;
            }
            reach[v] = true;
            st.extend(rsucc[v].iter().copied());
        }
        for b in 0..n {
            if live[b] && !reach[b] {
                rsucc[exit].push(b);
                rpred[b].push(exit);
            }
        }
        let (ipdom, _) = compute_idom(n + 1, exit, &|v| rsucc[v].clone(), &|v| rpred[v].clone());

        let mut a = Analysis {
            n,
            live,
            succ,
            pred,
            all_pred,
            idom,
            pre,
            post,
            ipdom,
            rpo_num,
            loops: vec![None; n],
        };
        a.find_loops()?;
        Ok(a)
    }

    pub fn dominates(&self, a: usize, b: usize) -> bool {
        if self.idom[a] == NONE || self.idom[b] == NONE {
            return false;
        }
        self.pre[a] <= self.pre[b] && self.post[b] <= self.post[a]
    }

    fn find_loops(&mut self) -> Result<(), Fail> {
        for u in 0..self.n {
            if !self.live[u] || self.idom[u] == NONE {
                continue;
            }
            for &v in &self.succ[u] {
                if self.rpo_num[v] <= self.rpo_num[u] && !self.dominates(v, u) {
                    return Err(Fail("不可归约的控制流"));
                }
            }
        }
        for h in 0..self.n {
            if !self.live[h] || self.idom[h] == NONE {
                continue;
            }
            let latches: Vec<usize> = self.pred[h]
                .iter()
                .copied()
                .filter(|&p| self.dominates(h, p))
                .collect();
            if latches.is_empty() {
                continue;
            }
            let mut body = vec![false; self.n];
            body[h] = true;
            let mut stack = latches;
            while let Some(v) = stack.pop() {
                if body[v] {
                    continue;
                }
                body[v] = true;
                for &p in &self.all_pred[v] {
                    if !body[p] && self.dominates(h, p) {
                        stack.push(p);
                    }
                }
            }
            self.loops[h] = Some(body);
        }
        Ok(())
    }

    /// 后支配树上的直接后支配者；虚拟出口返回 `None`。
    pub fn merge_point(&self, b: usize) -> Option<usize> {
        let p = self.ipdom[b];
        (p != NONE && p < self.n).then_some(p)
    }
}
