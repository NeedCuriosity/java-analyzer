//! 字节码 → 基本块：模拟操作数栈得到表达式树，局部变量按定义-使用链拆成"网"。
//!
//! 每个基本块得到一串语句和一个终结（跳转 / 条件 / switch / return / throw）。
//! 跨块留在栈上的值（三元表达式、构造器实参里的条件）存进合成的栈变量，
//! 之后由结构化和内联把它们还原成表达式。

use classfile::{
    code::{Code, Constant, MemberRef},
    signature::{self, BaseType, JType},
};
use rustc_hash::FxHashMap;

use crate::{
    bytecode::{self, Arith, ArrayKind, Cond, Insn, InvokeKind, Kind, Op, Prim},
    ir::*,
    Fail, MethodCx,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    Goto(usize),
    If {
        cond: Expr,
        then: usize,
        els: usize,
    },
    Switch {
        expr: Expr,
        cases: Vec<(i32, usize)>,
        default: usize,
    },
    Return(Option<Expr>),
    Throw(Expr),
    /// 已删除的块
    Dead,
}

impl Term {
    pub fn succs(&self) -> Vec<usize> {
        match self {
            Term::Goto(t) => vec![*t],
            Term::If { then, els, .. } => vec![*then, *els],
            Term::Switch { cases, default, .. } => {
                let mut v: Vec<usize> = cases.iter().map(|(_, t)| *t).collect();
                v.push(*default);
                v
            }
            _ => Vec::new(),
        }
    }

    pub fn succs_mut(&mut self, f: &mut dyn FnMut(&mut usize)) {
        match self {
            Term::Goto(t) => f(t),
            Term::If { then, els, .. } => {
                f(then);
                f(els);
            }
            Term::Switch { cases, default, .. } => {
                cases.iter_mut().for_each(|(_, t)| f(t));
                f(default);
            }
            _ => {}
        }
    }

    pub fn expr_mut(&mut self) -> Option<&mut Expr> {
        match self {
            Term::If { cond: e, .. } | Term::Switch { expr: e, .. } | Term::Throw(e) => Some(e),
            Term::Return(e) => e.as_mut(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub pc: u32,
    pub stmts: Vec<Stmt>,
    pub term: Term,
    /// 保护这个块的异常处理器（块下标）
    pub handlers: Vec<usize>,
}

/// 异常表的一项，换算成块。
#[derive(Debug, Clone)]
pub struct ExcEntry {
    pub start: u32,
    pub end: u32,
    pub handler: usize,
    pub catch_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Cfg {
    pub blocks: Vec<BasicBlock>,
    pub exceptions: Vec<ExcEntry>,
    /// 处理器块 → catch 变量
    pub catch_vars: FxHashMap<usize, VarId>,
}

impl Cfg {
    pub fn is_handler(&self, b: usize) -> bool {
        self.exceptions.iter().any(|e| e.handler == b)
    }
}

/// 方法体翻译的结果。
pub struct Translated {
    pub cfg: Cfg,
    /// 按描述符顺序的形参变量
    pub params: Vec<VarId>,
}

/// 翻译过程中 `Expr::Var` 里放的是"出现"的编号，带这个标志位，和最终的变量编号
/// （内联进来的 lambda 体里已经是最终编号）区分开。
const OCC: u32 = 0x8000_0000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OccKind {
    /// 局部变量槽位的读写
    Local,
    /// 栈变量或临时变量
    Temp,
}

struct Occ {
    kind: OccKind,
    slot: u16,
    value_kind: Kind,
    pc: u32,
    /// 写的话：这条指令之后的 pc（局部变量作用域从这里开始）
    next_pc: u32,
    is_def: bool,
}

#[derive(Clone)]
struct SE {
    e: Expr,
    kind: Kind,
}

#[derive(Clone, PartialEq)]
enum EntrySlot {
    Var(u32),
    Uninit(u32, String),
}

enum Event {
    Use(u16, u32),
    Def(u16, u32),
}

struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn add(&mut self) -> u32 {
        self.parent.push(self.parent.len() as u32);
        (self.parent.len() - 1) as u32
    }
    fn find(&mut self, x: u32) -> u32 {
        let mut x = x & !OCC;
        while self.parent[x as usize] != x {
            let p = self.parent[x as usize];
            self.parent[x as usize] = self.parent[p as usize];
            x = p;
        }
        x
    }
    fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[a.max(b) as usize] = a.min(b);
        }
    }
}

pub struct Translator<'c, 'a> {
    cx: &'c mut MethodCx<'a>,
    code: &'a Code,
    insns: Vec<Insn>,
    occs: Vec<Occ>,
    uf: UnionFind,
    this_fixed: bool,
    uninit_counter: u32,
    // 当前块的状态
    stack: Vec<SE>,
    stmts: Vec<Stmt>,
    events: Vec<Event>,
}

/// 方法描述符拆成形参类型和返回类型（`None` 表示 void）。
pub fn method_desc(desc: &str) -> Result<(Vec<JType>, Option<JType>), Fail> {
    let sig = signature::parse_method_sig(desc).ok_or(Fail("方法描述符不合法"))?;
    Ok((sig.params, sig.ret))
}

pub fn field_desc(desc: &str) -> Result<JType, Fail> {
    signature::parse_field_type(desc).ok_or(Fail("字段描述符不合法"))
}

/// `CONSTANT_Class` 里的名字转成类型（数组类是描述符形式）。
pub fn class_name_ty(name: &str) -> Result<JType, Fail> {
    if name.starts_with('[') {
        field_desc(name)
    } else {
        Ok(class_ty(name))
    }
}

fn prim_ty(p: Prim) -> JType {
    base(match p {
        Prim::Int => BaseType::Int,
        Prim::Long => BaseType::Long,
        Prim::Float => BaseType::Float,
        Prim::Double => BaseType::Double,
        Prim::Byte => BaseType::Byte,
        Prim::Char => BaseType::Char,
        Prim::Short => BaseType::Short,
    })
}

fn prim_kind(p: Prim) -> Kind {
    match p {
        Prim::Long => Kind::Long,
        Prim::Float => Kind::Float,
        Prim::Double => Kind::Double,
        _ => Kind::Int,
    }
}

fn arith_op(a: Arith) -> BinOp {
    match a {
        Arith::Add => BinOp::Add,
        Arith::Sub => BinOp::Sub,
        Arith::Mul => BinOp::Mul,
        Arith::Div => BinOp::Div,
        Arith::Rem => BinOp::Rem,
        Arith::Shl => BinOp::Shl,
        Arith::Shr => BinOp::Shr,
        Arith::Ushr => BinOp::Ushr,
        Arith::And => BinOp::And,
        Arith::Or => BinOp::Or,
        Arith::Xor => BinOp::Xor,
    }
}

fn cond_op(c: Cond) -> BinOp {
    match c {
        Cond::Eq => BinOp::Eq,
        Cond::Ne => BinOp::Ne,
        Cond::Lt => BinOp::Lt,
        Cond::Ge => BinOp::Ge,
        Cond::Gt => BinOp::Gt,
        Cond::Le => BinOp::Le,
    }
}

impl<'c, 'a> Translator<'c, 'a> {
    pub fn new(cx: &'c mut MethodCx<'a>, code: &'a Code) -> Result<Self, Fail> {
        let insns = bytecode::decode(&code.bytecode)?;
        if insns.is_empty() {
            return Err(Fail("方法体为空"));
        }
        let is_static = cx.method.is_static();
        let this_fixed = !is_static
            && !insns
                .iter()
                .any(|i| matches!(i.op, Op::Store(_, 0) | Op::Iinc(0, _)));
        if !is_static && !this_fixed {
            return Err(Fail("this 被重新赋值"));
        }
        Ok(Translator {
            cx,
            code,
            insns,
            occs: Vec::new(),
            uf: UnionFind { parent: Vec::new() },
            this_fixed,
            uninit_counter: 0,
            stack: Vec::new(),
            stmts: Vec::new(),
            events: Vec::new(),
        })
    }

    fn new_occ(
        &mut self,
        kind: OccKind,
        slot: u16,
        value_kind: Kind,
        pc: u32,
        next_pc: u32,
        is_def: bool,
    ) -> u32 {
        self.occs.push(Occ {
            kind,
            slot,
            value_kind,
            pc,
            next_pc,
            is_def,
        });
        self.uf.add() | OCC
    }

    fn temp_occ(&mut self, kind: Kind) -> u32 {
        self.new_occ(OccKind::Temp, 0, kind, 0, 0, true)
    }

    fn pop(&mut self) -> Result<SE, Fail> {
        self.stack.pop().ok_or(Fail("操作数栈下溢"))
    }

    fn push(&mut self, e: Expr, kind: Kind) {
        self.stack.push(SE { e, kind });
    }

    fn reads_slots(&self, e: &Expr, slots: &[u16]) -> bool {
        e.any(&mut |x| match x {
            Expr::Var(o) if *o & OCC != 0 => {
                let occ = &self.occs[(*o & !OCC) as usize];
                occ.kind == OccKind::Local && slots.contains(&occ.slot)
            }
            _ => false,
        })
    }

    /// 把栈上第 `i` 个值存进临时变量。
    fn spill(&mut self, i: usize) {
        let kind = self.stack[i].kind;
        let t = self.temp_occ(kind);
        let e = std::mem::replace(&mut self.stack[i].e, Expr::Var(t));
        self.stmts.push(Stmt::Expr(Expr::Assign(
            Box::new(Expr::Var(t)),
            Box::new(e),
        )));
    }

    /// 输出一条语句。栈上较早求值、又可能被这条语句影响的值先存进临时变量，保证求值顺序不变。
    fn emit(&mut self, stmt: Stmt) {
        let mut slots: Vec<u16> = Vec::new();
        let side_effects = match &stmt {
            Stmt::Expr(Expr::Assign(lhs, rhs)) => {
                if let Expr::Var(o) = &**lhs {
                    let occ = &self.occs[(*o & !OCC) as usize];
                    if *o & OCC != 0 && occ.kind == OccKind::Local {
                        slots.push(occ.slot);
                    }
                    !rhs.is_pure()
                } else {
                    true
                }
            }
            Stmt::Expr(Expr::IncDec { target, .. })
            | Stmt::Expr(Expr::CompoundAssign(_, target, _))
                if matches!(**target, Expr::Var(_)) =>
            {
                if let Expr::Var(o) = &**target {
                    let occ = &self.occs[(*o & !OCC) as usize];
                    if *o & OCC != 0 && occ.kind == OccKind::Local {
                        slots.push(occ.slot);
                    }
                }
                false
            }
            _ => true,
        };
        for i in 0..self.stack.len() {
            let e = &self.stack[i].e;
            if matches!(e, Expr::Uninit(..)) {
                continue;
            }
            if (side_effects && !e.is_pure()) || (!slots.is_empty() && self.reads_slots(e, &slots))
            {
                self.spill(i);
            }
        }
        self.stmts.push(stmt);
    }

    /// 让栈上第 `i` 个值可以复制。
    fn make_copyable(&mut self, i: usize) {
        if !self.stack[i].e.is_copyable() {
            // 存临时变量这一步本身要求更早的值先求值
            for j in 0..i {
                let e = &self.stack[j].e;
                if !e.is_pure() && !matches!(e, Expr::Uninit(..)) {
                    self.spill(j);
                }
            }
            self.spill(i);
        }
    }

    fn discard(&mut self, se: SE) {
        if se.e.is_statement_expr() {
            self.emit(Stmt::Expr(se.e));
        } else if !se.e.is_pure() {
            let t = self.temp_occ(se.kind);
            self.emit(Stmt::Expr(Expr::Assign(
                Box::new(Expr::Var(t)),
                Box::new(se.e),
            )));
        }
    }

    fn load_const(&mut self, idx: u16) -> Result<(), Fail> {
        let pool = &self.cx.class.pool;
        let (e, kind) = match pool.get(idx).ok_or(Fail("常量池下标越界"))? {
            Constant::Int(v) => (Expr::Lit(Lit::Int(*v)), Kind::Int),
            Constant::Float(v) => (Expr::Lit(Lit::Float(*v)), Kind::Float),
            Constant::Long(v) => (Expr::Lit(Lit::Long(*v)), Kind::Long),
            Constant::Double(v) => (Expr::Lit(Lit::Double(*v)), Kind::Double),
            Constant::Str(s) => (Expr::Lit(Lit::Str(s.clone())), Kind::Ref),
            Constant::Class(_) => {
                let name = pool.class_name(idx).ok_or(Fail("类常量不合法"))?;
                (Expr::Lit(Lit::Class(class_name_ty(name)?)), Kind::Ref)
            }
            _ => return Err(Fail("不支持的 ldc 常量")),
        };
        self.push(e, kind);
        Ok(())
    }

    fn member(&self, idx: u16) -> Result<MemberRef, Fail> {
        self.cx.class.pool.member(idx).ok_or(Fail("成员引用不合法"))
    }

    fn pop_args(&mut self, params: &[JType]) -> Result<Vec<Expr>, Fail> {
        let mut args = Vec::with_capacity(params.len());
        for p in params.iter().rev() {
            let se = self.pop()?;
            if se.kind != kind_of(p) {
                return Err(Fail("实参类别与描述符不符"));
            }
            args.push(se.e);
        }
        args.reverse();
        Ok(args)
    }

    /// 翻译一条指令。返回 `Some(终结)` 表示块到此结束。
    fn step(
        &mut self,
        insn: &Insn,
        block_of: &dyn Fn(u32) -> Result<usize, Fail>,
    ) -> Result<Option<Term>, Fail> {
        let pc = insn.pc;
        match &insn.op {
            Op::Nop => {}
            Op::AConstNull => self.push(Expr::Lit(Lit::Null), Kind::Ref),
            Op::IConst(v) => self.push(Expr::Lit(Lit::Int(*v)), Kind::Int),
            Op::LConst(v) => self.push(Expr::Lit(Lit::Long(*v)), Kind::Long),
            Op::FConst(v) => self.push(Expr::Lit(Lit::Float(v.to_bits())), Kind::Float),
            Op::DConst(v) => self.push(Expr::Lit(Lit::Double(v.to_bits())), Kind::Double),
            Op::Ldc(idx) => self.load_const(*idx)?,
            Op::Load(kind, slot) => {
                if *slot == 0 && self.this_fixed {
                    self.push(Expr::This, Kind::Ref);
                } else {
                    let o = self.new_occ(OccKind::Local, *slot, *kind, pc, insn.next, false);
                    self.events.push(Event::Use(*slot, o));
                    self.push(Expr::Var(o), *kind);
                }
            }
            Op::Store(kind, slot) => {
                let v = self.pop()?;
                if v.kind != *kind {
                    return Err(Fail("存入局部变量的值类别不符"));
                }
                if matches!(v.e, Expr::Uninit(..)) {
                    return Err(Fail("未初始化的对象存进了局部变量"));
                }
                let o = self.new_occ(OccKind::Local, *slot, *kind, pc, insn.next, true);
                self.emit(Stmt::Expr(Expr::Assign(
                    Box::new(Expr::Var(o)),
                    Box::new(v.e),
                )));
                self.events.push(Event::Def(*slot, o));
            }
            Op::Iinc(slot, delta) => {
                let u = self.new_occ(OccKind::Local, *slot, Kind::Int, pc, insn.next, false);
                let d = self.new_occ(OccKind::Local, *slot, Kind::Int, pc, insn.next, true);
                self.uf.union(u, d);
                // `x = i++`：栈顶恰好是刚读出的 i，换成后置自增表达式
                let readers: Vec<usize> = (0..self.stack.len())
                    .filter(|&i| self.reads_slots(&self.stack[i].e, &[*slot]))
                    .collect();
                if (*delta == 1 || *delta == -1)
                    && readers.len() == 1
                    && readers[0] == self.stack.len() - 1
                {
                    if let Expr::Var(top) = self.stack[readers[0]].e {
                        self.uf.union(top, u);
                        self.events.push(Event::Use(*slot, u));
                        self.events.push(Event::Def(*slot, d));
                        let top_se = self.stack.last_mut().unwrap();
                        top_se.e = Expr::IncDec {
                            target: Box::new(Expr::Var(top)),
                            inc: *delta == 1,
                            prefix: false,
                        };
                        return Ok(None);
                    }
                }
                self.events.push(Event::Use(*slot, u));
                let stmt = if *delta == 1 || *delta == -1 {
                    Expr::IncDec {
                        target: Box::new(Expr::Var(u)),
                        inc: *delta == 1,
                        prefix: false,
                    }
                } else if *delta < 0 {
                    Expr::CompoundAssign(
                        BinOp::Sub,
                        Box::new(Expr::Var(u)),
                        Box::new(Expr::int(-*delta)),
                    )
                } else {
                    Expr::CompoundAssign(
                        BinOp::Add,
                        Box::new(Expr::Var(u)),
                        Box::new(Expr::int(*delta)),
                    )
                };
                self.emit(Stmt::Expr(stmt));
                self.events.push(Event::Def(*slot, d));
            }
            Op::ArrayLoad(ak) => {
                let idx = self.pop()?;
                let arr = self.pop()?;
                self.push(Expr::Index(Box::new(arr.e), Box::new(idx.e)), ak.kind());
            }
            Op::ArrayStore(ak) => {
                let v = self.pop()?;
                let idx = self.pop()?;
                let arr = self.pop()?;
                if v.kind != ak.kind() {
                    return Err(Fail("数组元素类别不符"));
                }
                let v = coerce_array_store(*ak, v.e);
                self.emit(Stmt::Expr(Expr::Assign(
                    Box::new(Expr::Index(Box::new(arr.e), Box::new(idx.e))),
                    Box::new(v),
                )));
            }
            Op::Pop => {
                let v = self.pop()?;
                if v.kind.is_wide() {
                    return Err(Fail("pop 弹出了宽类型"));
                }
                self.discard(v);
            }
            Op::Pop2 => {
                let v = self.pop()?;
                if v.kind.is_wide() {
                    self.discard(v);
                } else {
                    let w = self.pop()?;
                    if w.kind.is_wide() {
                        return Err(Fail("pop2 形态不对"));
                    }
                    self.discard(v);
                    self.discard(w);
                }
            }
            Op::Dup => {
                let n = self.stack.len();
                if n == 0 || self.stack[n - 1].kind.is_wide() {
                    return Err(Fail("dup 形态不对"));
                }
                self.make_copyable(n - 1);
                let top = self.stack[n - 1].clone();
                self.stack.push(top);
            }
            Op::DupX1 => {
                let n = self.stack.len();
                if n < 2 || self.stack[n - 1].kind.is_wide() || self.stack[n - 2].kind.is_wide() {
                    return Err(Fail("dup_x1 形态不对"));
                }
                self.make_copyable(n - 1);
                let top = self.stack[n - 1].clone();
                self.stack.insert(n - 2, top);
            }
            Op::DupX2 => {
                let n = self.stack.len();
                if n < 2 || self.stack[n - 1].kind.is_wide() {
                    return Err(Fail("dup_x2 形态不对"));
                }
                self.make_copyable(n - 1);
                let top = self.stack[n - 1].clone();
                // 形态一：v3 v2 v1（都是单字）；形态二：v2(宽) v1
                let depth = if self.stack[n - 2].kind.is_wide() {
                    2
                } else {
                    3
                };
                if depth == 3 && (n < 3 || self.stack[n - 3].kind.is_wide()) {
                    return Err(Fail("dup_x2 形态不对"));
                }
                self.stack.insert(n - depth, top);
            }
            Op::Dup2 => {
                let n = self.stack.len();
                if n == 0 {
                    return Err(Fail("dup2 形态不对"));
                }
                if self.stack[n - 1].kind.is_wide() {
                    self.make_copyable(n - 1);
                    let top = self.stack[n - 1].clone();
                    self.stack.push(top);
                } else {
                    if n < 2 || self.stack[n - 2].kind.is_wide() {
                        return Err(Fail("dup2 形态不对"));
                    }
                    self.make_copyable(n - 2);
                    self.make_copyable(n - 1);
                    let a = self.stack[n - 2].clone();
                    let b = self.stack[n - 1].clone();
                    self.stack.push(a);
                    self.stack.push(b);
                }
            }
            Op::Dup2X1 => {
                let n = self.stack.len();
                if n < 2 {
                    return Err(Fail("dup2_x1 形态不对"));
                }
                if self.stack[n - 1].kind.is_wide() {
                    // v2 v1(宽) → v1 v2 v1
                    if self.stack[n - 2].kind.is_wide() {
                        return Err(Fail("dup2_x1 形态不对"));
                    }
                    self.make_copyable(n - 1);
                    let top = self.stack[n - 1].clone();
                    self.stack.insert(n - 2, top);
                } else {
                    // v3 v2 v1 → v2 v1 v3 v2 v1
                    if n < 3 || self.stack[n - 2].kind.is_wide() || self.stack[n - 3].kind.is_wide()
                    {
                        return Err(Fail("dup2_x1 形态不对"));
                    }
                    self.make_copyable(n - 2);
                    self.make_copyable(n - 1);
                    let a = self.stack[n - 2].clone();
                    let b = self.stack[n - 1].clone();
                    self.stack.insert(n - 3, b);
                    self.stack.insert(n - 3, a);
                }
            }
            Op::Dup2X2 => {
                let n = self.stack.len();
                if n < 2 {
                    return Err(Fail("dup2_x2 形态不对"));
                }
                // 只支持最常见的两种：v2(宽) v1(宽) 和 v3 v2 v1(宽)
                if self.stack[n - 1].kind.is_wide() {
                    let depth = if self.stack[n - 2].kind.is_wide() {
                        2
                    } else if n >= 3 && !self.stack[n - 3].kind.is_wide() {
                        3
                    } else {
                        return Err(Fail("dup2_x2 形态不支持"));
                    };
                    self.make_copyable(n - 1);
                    let top = self.stack[n - 1].clone();
                    self.stack.insert(n - depth, top);
                } else {
                    return Err(Fail("dup2_x2 形态不支持"));
                }
            }
            Op::Swap => {
                let n = self.stack.len();
                if n < 2 || self.stack[n - 1].kind.is_wide() || self.stack[n - 2].kind.is_wide() {
                    return Err(Fail("swap 形态不对"));
                }
                // 交换后求值顺序会变，两个值都得先固定下来
                for i in [n - 2, n - 1] {
                    let e = &self.stack[i].e;
                    if !e.is_pure() {
                        self.make_copyable(i);
                    }
                }
                self.stack.swap(n - 1, n - 2);
            }
            Op::Arith(kind, op) => {
                let b = self.pop()?;
                let a = self.pop()?;
                let shift = matches!(op, Arith::Shl | Arith::Shr | Arith::Ushr);
                if a.kind != *kind || (b.kind != *kind && !(shift && b.kind == Kind::Int)) {
                    return Err(Fail("算术运算的操作数类别不符"));
                }
                self.push(Expr::bin(arith_op(*op), a.e, b.e), *kind);
            }
            Op::Neg(kind) => {
                let a = self.pop()?;
                self.push(Expr::Unary(UnOp::Neg, Box::new(a.e)), *kind);
            }
            Op::Convert(from, to) => {
                let a = self.pop()?;
                if a.kind != *from {
                    return Err(Fail("类型转换的操作数类别不符"));
                }
                self.push(Expr::Cast(prim_ty(*to), Box::new(a.e)), prim_kind(*to));
            }
            Op::Compare(_) => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(Expr::Cmp(Box::new(a.e), Box::new(b.e)), Kind::Int);
            }
            Op::If(c, target) => {
                let v = self.pop()?;
                let cond = match v.e {
                    Expr::Cmp(a, b) => Expr::Binary(cond_op(*c), a, b),
                    e => Expr::bin(cond_op(*c), e, Expr::int(0)),
                };
                return Ok(Some(Term::If {
                    cond,
                    then: block_of(*target)?,
                    els: block_of(insn.next)?,
                }));
            }
            Op::IfCmp(c, target) => {
                let b = self.pop()?;
                let a = self.pop()?;
                return Ok(Some(Term::If {
                    cond: Expr::bin(cond_op(*c), a.e, b.e),
                    then: block_of(*target)?,
                    els: block_of(insn.next)?,
                }));
            }
            Op::IfACmp(eq, target) => {
                let b = self.pop()?;
                let a = self.pop()?;
                let op = if *eq { BinOp::Eq } else { BinOp::Ne };
                return Ok(Some(Term::If {
                    cond: Expr::bin(op, a.e, b.e),
                    then: block_of(*target)?,
                    els: block_of(insn.next)?,
                }));
            }
            Op::IfNull(is_null, target) => {
                let a = self.pop()?;
                let op = if *is_null { BinOp::Eq } else { BinOp::Ne };
                return Ok(Some(Term::If {
                    cond: Expr::bin(op, a.e, Expr::Lit(Lit::Null)),
                    then: block_of(*target)?,
                    els: block_of(insn.next)?,
                }));
            }
            Op::Goto(target) => return Ok(Some(Term::Goto(block_of(*target)?))),
            Op::Jsr | Op::Ret => return Err(Fail("不支持 jsr/ret")),
            Op::Switch { default, cases } => {
                let v = self.pop()?;
                let mut mapped = Vec::with_capacity(cases.len());
                for (k, t) in cases {
                    mapped.push((*k, block_of(*t)?));
                }
                return Ok(Some(Term::Switch {
                    expr: v.e,
                    cases: mapped,
                    default: block_of(*default)?,
                }));
            }
            Op::Return(kind) => {
                let value = match kind {
                    Some(_) => Some(self.pop()?.e),
                    None => None,
                };
                // 栈上剩下的值（javac 不会留）按顺序求值掉
                let rest: Vec<SE> = std::mem::take(&mut self.stack);
                for se in rest {
                    self.discard(se);
                }
                return Ok(Some(Term::Return(value)));
            }
            Op::AThrow => {
                let v = self.pop()?;
                let rest: Vec<SE> = std::mem::take(&mut self.stack);
                for se in rest {
                    self.discard(se);
                }
                return Ok(Some(Term::Throw(v.e)));
            }
            Op::GetStatic(idx) => {
                let m = self.member(*idx)?;
                let kind = kind_of(&field_desc(&m.descriptor)?);
                self.push(
                    Expr::Field {
                        target: None,
                        owner: m.owner,
                        name: m.name,
                        desc: m.descriptor,
                    },
                    kind,
                );
            }
            Op::GetField(idx) => {
                let m = self.member(*idx)?;
                let kind = kind_of(&field_desc(&m.descriptor)?);
                let obj = self.pop()?;
                self.push(
                    Expr::Field {
                        target: Some(Box::new(obj.e)),
                        owner: m.owner,
                        name: m.name,
                        desc: m.descriptor,
                    },
                    kind,
                );
            }
            Op::PutStatic(idx) | Op::PutField(idx) => {
                let m = self.member(*idx)?;
                let ty = field_desc(&m.descriptor)?;
                let v = self.pop()?;
                if v.kind != kind_of(&ty) {
                    return Err(Fail("字段赋值的类别不符"));
                }
                let target = if matches!(insn.op, Op::PutField(_)) {
                    Some(Box::new(self.pop()?.e))
                } else {
                    None
                };
                let lhs = Expr::Field {
                    target,
                    owner: m.owner,
                    name: m.name,
                    desc: m.descriptor,
                };
                self.emit(Stmt::Expr(Expr::Assign(Box::new(lhs), Box::new(v.e))));
            }
            Op::Invoke(kind, idx) => self.invoke(*kind, *idx)?,
            Op::InvokeDynamic(idx) => self.invoke_dynamic(*idx)?,
            Op::New(idx) => {
                let name = self
                    .cx
                    .class
                    .pool
                    .class_name(*idx)
                    .ok_or(Fail("new 的类型不合法"))?
                    .to_owned();
                self.uninit_counter += 1;
                self.push(Expr::Uninit(self.uninit_counter, name), Kind::Ref);
            }
            Op::NewPrimArray(t) => {
                let count = self.pop()?;
                let elem = match t {
                    4 => BaseType::Boolean,
                    5 => BaseType::Char,
                    6 => BaseType::Float,
                    7 => BaseType::Double,
                    8 => BaseType::Byte,
                    9 => BaseType::Short,
                    10 => BaseType::Int,
                    11 => BaseType::Long,
                    _ => return Err(Fail("newarray 的类型代码不合法")),
                };
                self.push(
                    Expr::NewArray {
                        elem: base(elem),
                        dims: vec![count.e],
                        extra: 0,
                        init: None,
                    },
                    Kind::Ref,
                );
            }
            Op::ANewArray(idx) => {
                let count = self.pop()?;
                let name = self
                    .cx
                    .class
                    .pool
                    .class_name(*idx)
                    .ok_or(Fail("anewarray 的类型不合法"))?;
                let mut elem = class_name_ty(name)?;
                let mut extra = 0u8;
                while let JType::Array(inner) = elem {
                    elem = *inner;
                    extra = extra.saturating_add(1);
                }
                self.push(
                    Expr::NewArray {
                        elem,
                        dims: vec![count.e],
                        extra,
                        init: None,
                    },
                    Kind::Ref,
                );
            }
            Op::MultiANewArray(idx, n) => {
                let name = self
                    .cx
                    .class
                    .pool
                    .class_name(*idx)
                    .ok_or(Fail("multianewarray 的类型不合法"))?;
                let mut elem = class_name_ty(name)?;
                let mut total = 0u8;
                while let JType::Array(inner) = elem {
                    elem = *inner;
                    total = total.saturating_add(1);
                }
                if *n == 0 || *n > total {
                    return Err(Fail("multianewarray 维数不合法"));
                }
                let mut dims = Vec::new();
                for _ in 0..*n {
                    dims.push(self.pop()?.e);
                }
                dims.reverse();
                self.push(
                    Expr::NewArray {
                        elem,
                        dims,
                        extra: total - n,
                        init: None,
                    },
                    Kind::Ref,
                );
            }
            Op::ArrayLength => {
                let a = self.pop()?;
                self.push(Expr::ArrayLen(Box::new(a.e)), Kind::Int);
            }
            Op::CheckCast(idx) => {
                let a = self.pop()?;
                let name = self
                    .cx
                    .class
                    .pool
                    .class_name(*idx)
                    .ok_or(Fail("checkcast 的类型不合法"))?;
                self.push(Expr::Cast(class_name_ty(name)?, Box::new(a.e)), Kind::Ref);
            }
            Op::InstanceOf(idx) => {
                let a = self.pop()?;
                let name = self
                    .cx
                    .class
                    .pool
                    .class_name(*idx)
                    .ok_or(Fail("instanceof 的类型不合法"))?;
                self.push(
                    Expr::InstanceOf(Box::new(a.e), class_name_ty(name)?),
                    Kind::Int,
                );
            }
            Op::MonitorEnter => {
                let a = self.pop()?;
                self.emit(Stmt::MonitorEnter(a.e));
            }
            Op::MonitorExit => {
                let a = self.pop()?;
                self.emit(Stmt::MonitorExit(a.e));
            }
        }
        Ok(None)
    }

    fn invoke(&mut self, kind: InvokeKind, idx: u16) -> Result<(), Fail> {
        let m = self.member(idx)?;
        let (params, ret) = method_desc(&m.descriptor)?;
        let args = self.pop_args(&params)?;
        if kind == InvokeKind::Static {
            let e = Expr::Invoke {
                kind,
                target: None,
                owner: m.owner,
                name: m.name,
                desc: m.descriptor,
                args,
            };
            return self.push_call(e, ret.as_ref());
        }
        let recv = self.pop()?;
        if recv.kind != Kind::Ref {
            return Err(Fail("调用的接收者不是引用"));
        }
        if m.name == "<init>" {
            if kind != InvokeKind::Special {
                return Err(Fail("构造器不是用 invokespecial 调用的"));
            }
            match recv.e {
                Expr::Uninit(id, class) => {
                    let new = self.make_new(class, m.descriptor, args)?;
                    let copies: Vec<usize> = (0..self.stack.len())
                        .filter(|&i| matches!(self.stack[i].e, Expr::Uninit(x, _) if x == id))
                        .collect();
                    match copies.len() {
                        0 => self.emit(Stmt::Expr(new)),
                        1 => self.stack[copies[0]].e = new,
                        _ => {
                            let t = self.temp_occ(Kind::Ref);
                            self.emit(Stmt::Expr(Expr::Assign(
                                Box::new(Expr::Var(t)),
                                Box::new(new),
                            )));
                            for i in copies {
                                self.stack[i].e = Expr::Var(t);
                            }
                        }
                    }
                }
                Expr::This if self.cx.method.name == "<init>" => {
                    self.emit(Stmt::Expr(Expr::Invoke {
                        kind,
                        target: Some(Box::new(Expr::This)),
                        owner: m.owner,
                        name: m.name,
                        desc: m.descriptor,
                        args,
                    }));
                }
                _ => return Err(Fail("构造器的接收者不是新对象")),
            }
            return Ok(());
        }
        if matches!(recv.e, Expr::Uninit(..)) {
            return Err(Fail("在未初始化的对象上调用方法"));
        }
        let e = Expr::Invoke {
            kind,
            target: Some(Box::new(recv.e)),
            owner: m.owner,
            name: m.name,
            desc: m.descriptor,
            args,
        };
        self.push_call(e, ret.as_ref())
    }

    fn push_call(&mut self, e: Expr, ret: Option<&JType>) -> Result<(), Fail> {
        match ret {
            None => self.emit(Stmt::Expr(e)),
            Some(ty) => self.push(e, kind_of(ty)),
        }
        Ok(())
    }

    fn make_new(&mut self, class: String, desc: String, args: Vec<Expr>) -> Result<Expr, Fail> {
        if let Some(anon) = crate::anon::anonymous_new(self.cx, &class, &desc, &args) {
            return Ok(Expr::AnonNew(Box::new(anon)));
        }
        Ok(Expr::New { class, desc, args })
    }

    fn invoke_dynamic(&mut self, idx: u16) -> Result<(), Fail> {
        let class = self.cx.class;
        let (bsm_idx, name, desc) = class
            .pool
            .dynamic(idx)
            .ok_or(Fail("invokedynamic 常量不合法"))?;
        let (params, _) = method_desc(desc)?;
        let bsm = class
            .bootstrap_methods
            .get(bsm_idx as usize)
            .ok_or(Fail("引导方法下标越界"))?;
        let handle = class
            .pool
            .method_handle(bsm.method)
            .ok_or(Fail("引导方法不合法"))?;
        let args = self.pop_args(&params)?;
        let e = match (handle.member.owner.as_str(), handle.member.name.as_str()) {
            ("java/lang/invoke/StringConcatFactory", "makeConcatWithConstants") => {
                let recipe = match bsm.arguments.first().and_then(|i| class.pool.get(*i)) {
                    Some(Constant::Str(s)) => s.clone(),
                    _ => return Err(Fail("字符串拼接的模板不合法")),
                };
                let mut consts = Vec::new();
                for &i in &bsm.arguments[1..] {
                    consts.push(match class.pool.get(i) {
                        Some(Constant::Str(s)) => (Expr::Lit(Lit::Str(s.clone())), true),
                        Some(Constant::Int(v)) => (Expr::int(*v), false),
                        Some(Constant::Long(v)) => (Expr::Lit(Lit::Long(*v)), false),
                        Some(Constant::Float(v)) => (Expr::Lit(Lit::Float(*v)), false),
                        Some(Constant::Double(v)) => (Expr::Lit(Lit::Double(*v)), false),
                        _ => return Err(Fail("字符串拼接的常量不支持")),
                    });
                }
                concat_from_recipe(&recipe, args, &params, consts)?
            }
            ("java/lang/invoke/StringConcatFactory", "makeConcat") => {
                let parts = args
                    .into_iter()
                    .zip(&params)
                    .map(|(e, ty)| (e, is_string(ty)))
                    .collect();
                build_concat(parts)
            }
            ("java/lang/invoke/LambdaMetafactory", "metafactory" | "altMetafactory") => {
                crate::lambda::lambda_expr(self.cx, bsm, name, args)?
            }
            ("java/lang/runtime/ObjectMethods", "bootstrap") => {
                record_method(class, bsm, name, args)?
            }
            ("java/lang/runtime/SwitchBootstraps", _) => {
                return Err(Fail("不支持模式匹配的 switch"))
            }
            _ => return Err(Fail("不支持的 invokedynamic 引导方法")),
        };
        self.push(e, Kind::Ref);
        Ok(())
    }

    /// 翻译整个方法。
    pub fn run(mut self) -> Result<Translated, Fail> {
        let code = self.code;
        let code_len = code.bytecode.len();
        let mut pc_index = vec![u32::MAX; code_len + 1];
        for (i, insn) in self.insns.iter().enumerate() {
            pc_index[insn.pc as usize] = i as u32;
        }
        pc_index[code_len] = self.insns.len() as u32;
        let valid = |pc: u32| (pc as usize) <= code_len && pc_index[pc as usize] != u32::MAX;

        // 基本块的起点
        let mut leader = vec![false; self.insns.len() + 1];
        leader[0] = true;
        for (i, insn) in self.insns.iter().enumerate() {
            for t in insn.targets() {
                if !valid(t) {
                    return Err(Fail("跳转到指令中间"));
                }
                leader[pc_index[t as usize] as usize] = true;
            }
            if insn.is_branch() {
                leader[i + 1] = true;
            }
            if matches!(insn.op, Op::MonitorEnter | Op::MonitorExit) {
                // monitor 指令单独成句，便于识别 synchronized
            }
        }
        let mut exc_entries = Vec::new();
        for e in &code.exception_table {
            let (s, en, h) = (e.start as u32, e.end as u32, e.handler as u32);
            if !valid(s) || !valid(en) || !valid(h) || s >= en || (h as usize) >= code_len {
                return Err(Fail("异常表不合法"));
            }
            // 保护自身的处理器（synchronized 的 monitorexit 处理器）不参与结构化
            if h >= s && h < en {
                continue;
            }
            leader[pc_index[s as usize] as usize] = true;
            leader[pc_index[en as usize] as usize] = true;
            leader[pc_index[h as usize] as usize] = true;
            exc_entries.push(e.clone());
        }
        let n_insns = self.insns.len();
        let starts: Vec<usize> = (0..n_insns).filter(|&i| leader[i]).collect();
        let mut block_at = vec![usize::MAX; n_insns];
        for (b, &s) in starts.iter().enumerate() {
            block_at[s] = b;
        }
        let n_blocks = starts.len();
        if n_blocks > crate::MAX_BLOCKS {
            return Err(Fail("方法太大"));
        }
        let insns = std::mem::take(&mut self.insns);
        let block_of = |pc: u32| -> Result<usize, Fail> {
            if !valid(pc) || pc as usize >= code_len {
                return Err(Fail("跳转目标不合法"));
            }
            let b = block_at[pc_index[pc as usize] as usize];
            if b == usize::MAX {
                return Err(Fail("跳转目标不是块的起点"));
            }
            Ok(b)
        };

        let mut blocks: Vec<BasicBlock> = starts
            .iter()
            .map(|&s| BasicBlock {
                pc: insns[s].pc,
                stmts: Vec::new(),
                term: Term::Dead,
                handlers: Vec::new(),
            })
            .collect();
        let mut exceptions = Vec::new();
        for e in &exc_entries {
            let handler = block_of(e.handler as u32)?;
            exceptions.push(ExcEntry {
                start: e.start as u32,
                end: e.end as u32,
                handler,
                catch_type: e.catch_type.clone(),
            });
            for blk in blocks.iter_mut() {
                if blk.pc >= e.start as u32
                    && blk.pc < e.end as u32
                    && !blk.handlers.contains(&handler)
                {
                    blk.handlers.push(handler);
                }
            }
        }
        let handler_types: FxHashMap<usize, Option<String>> = exceptions
            .iter()
            .map(|e| (e.handler, e.catch_type.clone()))
            .collect();

        // 形参
        let mut params = Vec::new();
        let mut param_defs: Vec<(u16, u32)> = Vec::new();
        let (param_tys, _) = method_desc(&self.cx.method.descriptor)?;
        let mut slot: u16 = if self.cx.method.is_static() { 0 } else { 1 };
        for ty in &param_tys {
            let k = kind_of(ty);
            let o = self.new_occ(OccKind::Local, slot, k, 0, 0, true);
            param_defs.push((slot, o));
            params.push(o);
            slot += if k.is_wide() { 2 } else { 1 };
        }

        // 按逆后序逐块模拟（异常边也算），入口栈取第一个处理过的前驱的出口栈
        let mut succs: Vec<Vec<usize>> = vec![Vec::new(); n_blocks];
        let mut entry: Vec<Option<Vec<(EntrySlot, Kind)>>> = vec![None; n_blocks];
        let mut exit_stacks: Vec<Vec<SE>> = vec![Vec::new(); n_blocks];
        let mut block_events: Vec<Vec<Event>> = (0..n_blocks).map(|_| Vec::new()).collect();
        let mut done = vec![false; n_blocks];
        entry[0] = Some(Vec::new());
        let mut worklist = vec![0usize];
        for h in handler_types.keys() {
            entry[*h] = Some(Vec::new());
        }
        let mut order = Vec::new();
        while let Some(b) = worklist.pop() {
            if done[b] {
                continue;
            }
            done[b] = true;
            order.push(b);
            self.cx.budget.spend(1)?;
            let shape = entry[b].clone().unwrap_or_default();
            self.stack.clear();
            if let Some(ty) = handler_types.get(&b) {
                self.stack.push(SE {
                    e: Expr::Caught(ty.clone()),
                    kind: Kind::Ref,
                });
            } else {
                for (slot, kind) in &shape {
                    let e = match slot {
                        EntrySlot::Var(o) => Expr::Var(*o),
                        EntrySlot::Uninit(id, c) => Expr::Uninit(*id, c.clone()),
                    };
                    self.stack.push(SE { e, kind: *kind });
                }
            }
            self.stmts.clear();
            self.events.clear();
            let first = starts[b];
            let end = starts.get(b + 1).copied().unwrap_or(n_insns);
            let mut term = None;
            for insn in &insns[first..end] {
                if let Some(t) = self.step(insn, &block_of)? {
                    term = Some(t);
                    break;
                }
            }
            let term = match term {
                Some(t) => t,
                None => {
                    if end >= n_insns {
                        return Err(Fail("执行落到了方法末尾之外"));
                    }
                    Term::Goto(b + 1)
                }
            };
            for e in self.stack.iter() {
                if e.e.depth() > crate::MAX_EXPR_DEPTH {
                    return Err(Fail("表达式太深"));
                }
            }
            let s = term.succs();
            // 后继的入口栈形状
            for &t in &s {
                if handler_types.contains_key(&t) {
                    return Err(Fail("异常处理器也能顺序到达"));
                }
                let exit_shape: Vec<(EntrySlot, Kind)> = self
                    .stack
                    .iter()
                    .map(|se| match &se.e {
                        Expr::Uninit(id, c) => (EntrySlot::Uninit(*id, c.clone()), se.kind),
                        _ => (EntrySlot::Var(u32::MAX), se.kind),
                    })
                    .collect();
                match &entry[t] {
                    None => {
                        let slots = exit_shape
                            .into_iter()
                            .map(|(slot, kind)| match slot {
                                EntrySlot::Var(_) => (EntrySlot::Var(self.temp_occ(kind)), kind),
                                u => (u, kind),
                            })
                            .collect();
                        entry[t] = Some(slots);
                    }
                    Some(existing) => {
                        let same = existing.len() == exit_shape.len()
                            && existing.iter().zip(&exit_shape).all(|((a, ka), (b, kb))| {
                                ka == kb
                                    && match (a, b) {
                                        (EntrySlot::Var(_), EntrySlot::Var(_)) => true,
                                        (EntrySlot::Uninit(x, _), EntrySlot::Uninit(y, _)) => {
                                            x == y
                                        }
                                        _ => false,
                                    }
                            });
                        if !same {
                            return Err(Fail("汇合处操作数栈不一致"));
                        }
                    }
                }
                if !done[t] {
                    worklist.push(t);
                }
            }
            for &h in &blocks[b].handlers {
                if !done[h] {
                    worklist.push(h);
                }
            }
            if matches!(term, Term::Return(_) | Term::Throw(_)) && !self.stack.is_empty() {
                return Err(Fail("返回时栈不空"));
            }
            succs[b] = s;
            exit_stacks[b] = std::mem::take(&mut self.stack);
            blocks[b].stmts = std::mem::take(&mut self.stmts);
            blocks[b].term = term;
            block_events[b] = std::mem::take(&mut self.events);
        }
        // 没到达的块
        for b in 0..n_blocks {
            if !done[b] {
                blocks[b].term = Term::Dead;
                blocks[b].handlers.clear();
            }
        }
        exceptions.retain(|e| done[e.handler]);

        // 栈变量：同一条边两端的同一深度合并成一个变量
        for b in 0..n_blocks {
            if !done[b] || exit_stacks[b].is_empty() {
                continue;
            }
            let s = &succs[b];
            for d in 0..exit_stacks[b].len() {
                let mut first: Option<u32> = None;
                for &t in s {
                    if let Some(Some((EntrySlot::Var(o), _))) =
                        entry[t].as_ref().map(|v| v.get(d).cloned())
                    {
                        match first {
                            None => first = Some(o),
                            Some(f) => self.uf.union(f, o),
                        }
                    }
                }
            }
        }
        for b in 0..n_blocks {
            if !done[b] || exit_stacks[b].is_empty() {
                continue;
            }
            let Some(&t) = succs[b].first() else {
                continue;
            };
            let targets = entry[t].clone().unwrap_or_default();
            let exit = std::mem::take(&mut exit_stacks[b]);
            let mut assigns: Vec<(u32, Expr)> = Vec::new();
            for (d, se) in exit.into_iter().enumerate() {
                let Some((EntrySlot::Var(o), _)) = targets.get(d) else {
                    continue;
                };
                if let Expr::Var(x) = &se.e {
                    if *x & OCC != 0 && self.uf.find(*x) == self.uf.find(*o) {
                        continue;
                    }
                }
                assigns.push((*o, se.e));
            }
            // 并行赋值：后面的值不能读前面刚写过的栈变量
            for i in 0..assigns.len() {
                for j in i + 1..assigns.len() {
                    let target = self.uf.find(assigns[i].0);
                    let mut conflict = false;
                    let uf = &mut self.uf;
                    assigns[j].1.any(&mut |e| {
                        if let Expr::Var(v) = e {
                            if *v & OCC != 0 && uf.find(*v) == target {
                                conflict = true;
                            }
                        }
                        false
                    });
                    if conflict {
                        return Err(Fail("栈变量的并行赋值有冲突"));
                    }
                }
            }
            for (o, e) in assigns {
                blocks[b].stmts.push(Stmt::Expr(Expr::Assign(
                    Box::new(Expr::Var(o)),
                    Box::new(e),
                )));
            }
        }

        // 局部变量的到达定义 → 网
        self.build_webs(&blocks, &block_events, &param_defs, &order)?;

        // 出现次数 → 变量
        let var_of = self.assign_vars(&params)?;
        let map = |e: &mut Expr| {
            if let Expr::Var(o) = e {
                if *o & OCC != 0 {
                    *o = var_of[(*o & !OCC) as usize];
                }
            }
        };
        let mut map = map;
        for blk in blocks.iter_mut() {
            rewrite_exprs(&mut blk.stmts, &mut map);
            if let Some(e) = blk.term.expr_mut() {
                rewrite_expr_deep(e, &mut map);
            }
        }
        let params = params
            .iter()
            .map(|o| var_of[(*o & !OCC) as usize])
            .collect();

        // catch 变量
        let mut catch_vars = FxHashMap::default();
        for (&h, ty) in &handler_types {
            if !done[h] {
                continue;
            }
            let blk = &mut blocks[h];
            let first_is_store = matches!(
                blk.stmts.first(),
                Some(Stmt::Expr(Expr::Assign(lhs, rhs)))
                    if matches!(**lhs, Expr::Var(_)) && matches!(**rhs, Expr::Caught(_))
            );
            let var = if first_is_store {
                let Stmt::Expr(Expr::Assign(lhs, _)) = blk.stmts.remove(0) else {
                    unreachable!()
                };
                let Expr::Var(v) = *lhs else { unreachable!() };
                v
            } else {
                let v = self.cx.vars.add(VarInfo {
                    name: None,
                    ty: Some(class_ty(ty.as_deref().unwrap_or("java/lang/Throwable"))),
                    kind: Kind::Ref,
                    named_by_lvt: false,
                    stack_temp: false,
                    is_param: false,
                });
                let mut rw = |e: &mut Expr| {
                    if matches!(e, Expr::Caught(_)) {
                        *e = Expr::Var(v);
                    }
                };
                rewrite_exprs(&mut blk.stmts, &mut rw);
                if let Some(e) = blk.term.expr_mut() {
                    rewrite_expr_deep(e, &mut rw);
                }
                v
            };
            let info = self.cx.vars.get_mut(var);
            if info.ty.is_none() || !info.named_by_lvt {
                info.ty = Some(class_ty(ty.as_deref().unwrap_or("java/lang/Throwable")));
            }
            catch_vars.insert(h, var);
        }

        Ok(Translated {
            cfg: Cfg {
                blocks,
                exceptions,
                catch_vars,
            },
            params,
        })
    }

    fn build_webs(
        &mut self,
        blocks: &[BasicBlock],
        events: &[Vec<Event>],
        param_defs: &[(u16, u32)],
        order: &[usize],
    ) -> Result<(), Fail> {
        let n = blocks.len();
        // 每个槽位的定义编号
        let mut def_index: FxHashMap<u32, usize> = FxHashMap::default();
        let mut defs: Vec<u32> = Vec::new();
        let mut slots: Vec<u16> = Vec::new();
        for &(slot, o) in param_defs {
            def_index.insert(o, defs.len());
            defs.push(o);
            slots.push(slot);
        }
        for evs in events {
            for ev in evs {
                if let Event::Def(slot, o) = ev {
                    def_index.insert(*o, defs.len());
                    defs.push(*o);
                    slots.push(*slot);
                }
            }
        }
        let max_slot = slots.iter().copied().max().unwrap_or(0) as usize;
        let n_defs = defs.len();
        if n_defs == 0 {
            return Ok(());
        }
        let words = n_defs.div_ceil(64);
        if words * n > 4_000_000 {
            return Err(Fail("局部变量太多"));
        }
        // 位集：OUT[b] = 块出口处到达的定义
        let mut out = vec![vec![0u64; words]; n];
        let mut inn = vec![vec![0u64; words]; n];
        let mut entry_set = vec![0u64; words];
        for &(_, o) in param_defs {
            let i = def_index[&o];
            entry_set[i / 64] |= 1 << (i % 64);
        }
        // 每个槽位的全部定义（用于 kill）
        let mut slot_mask: Vec<Vec<u64>> = vec![vec![0u64; words]; max_slot + 2];
        for (i, &s) in slots.iter().enumerate() {
            slot_mask[s as usize][i / 64] |= 1 << (i % 64);
        }
        // 块内全部定义（异常边用：处理器能看到块内任意时刻的状态）
        let mut all_defs = vec![vec![0u64; words]; n];
        for b in 0..n {
            for ev in &events[b] {
                if let Event::Def(_, o) = ev {
                    let i = def_index[o];
                    all_defs[b][i / 64] |= 1 << (i % 64);
                }
            }
        }
        let mut preds: Vec<Vec<(usize, bool)>> = vec![Vec::new(); n];
        for b in 0..n {
            if matches!(blocks[b].term, Term::Dead) {
                continue;
            }
            for s in blocks[b].term.succs() {
                preds[s].push((b, false));
            }
            for &h in &blocks[b].handlers {
                preds[h].push((b, true));
            }
        }
        let transfer = |b: usize, input: &[u64]| -> Vec<u64> {
            let mut cur = input.to_vec();
            for ev in &events[b] {
                if let Event::Def(slot, o) = ev {
                    for (w, m) in cur.iter_mut().zip(&slot_mask[*slot as usize]) {
                        *w &= !m;
                    }
                    let i = def_index[o];
                    cur[i / 64] |= 1 << (i % 64);
                }
            }
            cur
        };
        let mut changed = true;
        let mut rounds = 0;
        while changed {
            changed = false;
            rounds += 1;
            if rounds > 200 {
                return Err(Fail("到达定义分析不收敛"));
            }
            self.cx.budget.spend(order.len())?;
            for &b in order {
                let mut new_in = if b == 0 {
                    entry_set.clone()
                } else {
                    vec![0u64; words]
                };
                for &(p, exc) in &preds[b] {
                    for w in 0..words {
                        new_in[w] |= out[p][w];
                        if exc {
                            new_in[w] |= inn[p][w] | all_defs[p][w];
                        }
                    }
                }
                if new_in != inn[b] || rounds == 1 {
                    let new_out = transfer(b, &new_in);
                    inn[b] = new_in;
                    if new_out != out[b] {
                        out[b] = new_out;
                        changed = true;
                    }
                }
            }
        }
        // 每个使用和到达它的定义合并
        for &b in order {
            let mut cur = inn[b].clone();
            for ev in &events[b] {
                match ev {
                    Event::Use(slot, o) => {
                        let mask = &slot_mask[*slot as usize];
                        for w in 0..words {
                            let mut bits = cur[w] & mask[w];
                            while bits != 0 {
                                let i = w * 64 + bits.trailing_zeros() as usize;
                                bits &= bits - 1;
                                self.uf.union(*o, defs[i]);
                            }
                        }
                    }
                    Event::Def(slot, o) => {
                        for (w, m) in cur.iter_mut().zip(&slot_mask[*slot as usize]) {
                            *w &= !m;
                        }
                        let i = def_index[o];
                        cur[i / 64] |= 1 << (i % 64);
                    }
                }
            }
        }
        Ok(())
    }

    /// LocalVariableTable 里覆盖这次出现的项。
    fn lvt_entry(&self, o: u32) -> Option<usize> {
        let occ = &self.occs[o as usize];
        if occ.kind != OccKind::Local {
            return None;
        }
        let lvt = &self.code.local_variables;
        if occ.is_def && occ.pc == 0 && occ.next_pc == 0 {
            // 形参
            return lvt.iter().position(|e| e.slot == occ.slot && e.start == 0);
        }
        let covers = |e: &classfile::code::LocalVariable, pc: u32| {
            e.slot == occ.slot && pc >= e.start as u32 && pc < e.start as u32 + e.length as u32
        };
        if occ.is_def {
            lvt.iter()
                .position(|e| covers(e, occ.next_pc))
                .or_else(|| lvt.iter().position(|e| covers(e, occ.pc)))
                .or_else(|| {
                    lvt.iter().position(|e| {
                        e.slot == occ.slot && e.start as u32 + e.length as u32 == occ.next_pc
                    })
                })
        } else {
            lvt.iter().position(|e| covers(e, occ.pc))
        }
    }

    fn assign_vars(&mut self, params: &[u32]) -> Result<Vec<VarId>, Fail> {
        let n = self.occs.len();
        // 同一个 LVT 项里的出现是同一个源码变量
        let mut lvt_of = vec![None; n];
        let mut first_in_entry: FxHashMap<usize, u32> = FxHashMap::default();
        for o in 0..n as u32 {
            if let Some(e) = self.lvt_entry(o) {
                lvt_of[o as usize] = Some(e);
                match first_in_entry.get(&e) {
                    Some(&f) => self.uf.union(f, o),
                    None => {
                        first_in_entry.insert(e, o);
                    }
                }
            }
        }
        let mut root_var: FxHashMap<u32, VarId> = FxHashMap::default();
        let mut var_of = vec![0 as VarId; n];
        let param_set: rustc_hash::FxHashSet<u32> = params.iter().map(|p| p & !OCC).collect();
        for o in 0..n as u32 {
            let root = self.uf.find(o);
            let occ = &self.occs[o as usize];
            let var = match root_var.get(&root) {
                Some(&v) => v,
                None => {
                    let v = self.cx.vars.add(VarInfo {
                        name: None,
                        ty: None,
                        kind: occ.value_kind,
                        named_by_lvt: false,
                        stack_temp: occ.kind == OccKind::Temp,
                        is_param: false,
                    });
                    root_var.insert(root, v);
                    v
                }
            };
            var_of[o as usize] = var;
            let info = self.cx.vars.get_mut(var);
            if param_set.contains(&o) {
                info.is_param = true;
                info.stack_temp = false;
            }
            if occ.kind == OccKind::Local {
                info.stack_temp = false;
            }
            if let Some(e) = lvt_of[o as usize] {
                if !info.named_by_lvt {
                    let entry = &self.code.local_variables[e];
                    info.named_by_lvt = true;
                    info.name = Some(entry.name.clone());
                    // 泛型签名优先
                    let generic = self
                        .code
                        .local_variable_types
                        .iter()
                        .find(|t| {
                            t.slot == entry.slot && t.start == entry.start && t.name == entry.name
                        })
                        .and_then(|t| signature::parse_field_type(&t.descriptor));
                    info.ty = generic.or_else(|| signature::parse_field_type(&entry.descriptor));
                }
            }
        }
        // 有 LocalVariableTable 时，表里没有的局部变量是编译器合成的（finally 里暂存的返回值、
        // for-each 的迭代器等），和栈变量一样可以内联
        if !self.code.local_variables.is_empty() {
            for o in 0..n {
                if self.occs[o].kind == OccKind::Local {
                    let info = self.cx.vars.get_mut(var_of[o]);
                    if !info.named_by_lvt && !info.is_param {
                        info.stack_temp = true;
                    }
                }
            }
        }
        Ok(var_of)
    }
}

/// `bastore` 等对 boolean / char 数组：整数常量按元素类型写。
fn coerce_array_store(_ak: ArrayKind, e: Expr) -> Expr {
    e
}

/// 按 `makeConcatWithConstants` 的模板拼出 `a + "x" + b`。
fn concat_from_recipe(
    recipe: &[u16],
    args: Vec<Expr>,
    arg_tys: &[JType],
    consts: Vec<(Expr, bool)>,
) -> Result<Expr, Fail> {
    let mut parts: Vec<(Expr, bool)> = Vec::new();
    let mut text: Vec<u16> = Vec::new();
    let mut args = args.into_iter().zip(arg_tys.iter());
    let mut consts = consts.into_iter();
    let flush = |text: &mut Vec<u16>, parts: &mut Vec<(Expr, bool)>| {
        if !text.is_empty() {
            parts.push((Expr::Lit(Lit::Str(std::mem::take(text))), true));
        }
    };
    for &u in recipe {
        match u {
            1 => {
                flush(&mut text, &mut parts);
                let (e, ty) = args.next().ok_or(Fail("拼接模板的参数不够"))?;
                parts.push((e, is_string(ty)));
            }
            2 => {
                flush(&mut text, &mut parts);
                parts.push(consts.next().ok_or(Fail("拼接模板的常量不够"))?);
            }
            _ => text.push(u),
        }
    }
    flush(&mut text, &mut parts);
    if args.next().is_some() {
        return Err(Fail("拼接模板的参数多了"));
    }
    Ok(build_concat(parts))
}

/// record 自动生成的 `toString` / `hashCode` / `equals`（`ObjectMethods.bootstrap`）
/// 写成等价的普通表达式。
fn record_method(
    class: &classfile::code::CodeClass,
    bsm: &classfile::code::BootstrapMethod,
    name: &str,
    args: Vec<Expr>,
) -> Result<Expr, Fail> {
    const BAD: Fail = Fail("record 方法的引导参数不合法");
    let record = match bsm
        .arguments
        .first()
        .and_then(|&i| class.pool.class_name(i))
    {
        Some(c) => c.to_owned(),
        None => return Err(BAD),
    };
    let names: Vec<u16> = match bsm.arguments.get(1).and_then(|&i| class.pool.get(i)) {
        Some(Constant::Str(s)) => s.clone(),
        _ => return Err(BAD),
    };
    // (所属类, 字段名, 描述符, 类型)
    let mut fields: Vec<(String, String, String, JType)> = Vec::new();
    for &i in bsm.arguments.iter().skip(2) {
        let h = class.pool.method_handle(i).ok_or(BAD)?;
        if h.kind != classfile::code::handle_kind::GET_FIELD {
            return Err(BAD);
        }
        let ty = field_desc(&h.member.descriptor)?;
        fields.push((h.member.owner, h.member.name, h.member.descriptor, ty));
    }
    // 参数只能是变量或 this，下面会重复使用
    if args.iter().any(|a| !matches!(a, Expr::Var(_) | Expr::This)) {
        return Err(BAD);
    }
    let field = |target: Expr, f: &(String, String, String, JType)| Expr::Field {
        target: Some(Box::new(target)),
        owner: f.0.clone(),
        name: f.1.clone(),
        desc: f.2.clone(),
    };
    let static_call = |owner: &str, name: &str, desc: &str, args: Vec<Expr>| Expr::Invoke {
        kind: InvokeKind::Static,
        target: None,
        owner: owner.to_owned(),
        name: name.to_owned(),
        desc: desc.to_owned(),
        args,
    };
    match (name, args.as_slice()) {
        ("toString", [recv]) => {
            let simple = record.rsplit(['/', '$']).next().unwrap_or(&record);
            let labels: Vec<&[u16]> = if names.is_empty() {
                Vec::new()
            } else {
                names.split(|&u| u == u16::from(b';')).collect()
            };
            if labels.len() != fields.len() {
                return Err(BAD);
            }
            let mut parts: Vec<(Expr, bool)> = Vec::new();
            let mut text: Vec<u16> = format!("{simple}[").encode_utf16().collect();
            for (k, (label, f)) in labels.iter().zip(&fields).enumerate() {
                if k > 0 {
                    text.extend(", ".encode_utf16());
                }
                text.extend_from_slice(label);
                text.push(u16::from(b'='));
                parts.push((Expr::Lit(Lit::Str(std::mem::take(&mut text))), true));
                parts.push((field(recv.clone(), f), is_string(&f.3)));
            }
            text.push(u16::from(b']'));
            parts.push((Expr::Lit(Lit::Str(text)), true));
            Ok(build_concat(parts))
        }
        ("hashCode", [recv]) => {
            // result = 31 * result + hash(c)，从 0 开始
            let mut acc: Option<Expr> = None;
            for f in &fields {
                let v = field(recv.clone(), f);
                let h = match &f.3 {
                    JType::Base(b) => {
                        let (owner, d) = match b {
                            BaseType::Boolean => ("java/lang/Boolean", "(Z)I"),
                            BaseType::Byte => ("java/lang/Byte", "(B)I"),
                            BaseType::Char => ("java/lang/Character", "(C)I"),
                            BaseType::Short => ("java/lang/Short", "(S)I"),
                            BaseType::Int => ("java/lang/Integer", "(I)I"),
                            BaseType::Long => ("java/lang/Long", "(J)I"),
                            BaseType::Float => ("java/lang/Float", "(F)I"),
                            BaseType::Double => ("java/lang/Double", "(D)I"),
                        };
                        static_call(owner, "hashCode", d, vec![v])
                    }
                    _ => static_call(
                        "java/util/Objects",
                        "hashCode",
                        "(Ljava/lang/Object;)I",
                        vec![v],
                    ),
                };
                acc = Some(match acc {
                    None => h,
                    Some(a) => Expr::bin(BinOp::Add, Expr::bin(BinOp::Mul, Expr::int(31), a), h),
                });
            }
            Ok(acc.unwrap_or(Expr::int(0)))
        }
        ("equals", [recv, other]) => {
            let ty = class_ty(&record);
            let cast = Expr::Cast(ty.clone(), Box::new(other.clone()));
            let mut cond = Expr::InstanceOf(Box::new(other.clone()), ty);
            for f in &fields {
                let a = field(recv.clone(), f);
                let b = field(cast.clone(), f);
                let same = match &f.3 {
                    JType::Base(BaseType::Float) => Expr::bin(
                        BinOp::Eq,
                        static_call("java/lang/Float", "compare", "(FF)I", vec![a, b]),
                        Expr::int(0),
                    ),
                    JType::Base(BaseType::Double) => Expr::bin(
                        BinOp::Eq,
                        static_call("java/lang/Double", "compare", "(DD)I", vec![a, b]),
                        Expr::int(0),
                    ),
                    JType::Base(_) => Expr::bin(BinOp::Eq, a, b),
                    _ => static_call(
                        "java/util/Objects",
                        "equals",
                        "(Ljava/lang/Object;Ljava/lang/Object;)Z",
                        vec![a, b],
                    ),
                };
                cond = Expr::bin(BinOp::LAnd, cond, same);
            }
            Ok(Expr::bin(
                BinOp::LOr,
                Expr::bin(BinOp::Eq, recv.clone(), other.clone()),
                cond,
            ))
        }
        _ => Err(BAD),
    }
}

/// 把若干段拼成 `+` 链。前两段都不是字符串时在最前面补 `""`，保证是字符串拼接而不是数值加法。
pub fn build_concat(mut parts: Vec<(Expr, bool)>) -> Expr {
    let stringy = |i: usize, parts: &[(Expr, bool)]| parts.get(i).is_some_and(|p| p.1);
    if !(stringy(0, &parts) || (parts.len() >= 2 && stringy(1, &parts))) {
        parts.insert(0, (Expr::Lit(Lit::Str(Vec::new())), true));
    }
    let mut iter = parts.into_iter();
    let mut acc = iter
        .next()
        .map(|p| p.0)
        .unwrap_or(Expr::Lit(Lit::Str(Vec::new())));
    for (e, _) in iter {
        acc = Expr::bin(BinOp::Add, acc, e);
    }
    acc
}
