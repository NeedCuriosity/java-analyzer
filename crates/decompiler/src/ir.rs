//! 反编译的中间表示：表达式、语句、局部变量。

use classfile::signature::{BaseType, JType};

use crate::bytecode::{InvokeKind, Kind};

pub type VarId = u32;
pub type LabelId = u32;

pub fn class_ty(name: &str) -> JType {
    JType::Class {
        name: name.to_owned(),
        args: Vec::new(),
    }
}

pub fn object_ty() -> JType {
    class_ty("java/lang/Object")
}

pub fn string_ty() -> JType {
    class_ty("java/lang/String")
}

pub fn base(b: BaseType) -> JType {
    JType::Base(b)
}

pub fn is_string(ty: &JType) -> bool {
    matches!(ty, JType::Class { name, .. } if name == "java/lang/String")
}

pub fn kind_of(ty: &JType) -> Kind {
    match ty {
        JType::Base(BaseType::Long) => Kind::Long,
        JType::Base(BaseType::Float) => Kind::Float,
        JType::Base(BaseType::Double) => Kind::Double,
        JType::Base(_) => Kind::Int,
        _ => Kind::Ref,
    }
}

pub fn ty_of_kind(kind: Kind) -> JType {
    match kind {
        Kind::Int => base(BaseType::Int),
        Kind::Long => base(BaseType::Long),
        Kind::Float => base(BaseType::Float),
        Kind::Double => base(BaseType::Double),
        Kind::Ref => object_ty(),
    }
}

/// 擦除：类型变量当 Object，类型实参去掉。
pub fn erase(ty: &JType) -> JType {
    match ty {
        JType::Base(_) => ty.clone(),
        JType::Class { name, .. } => class_ty(name),
        JType::TypeVar(_) => object_ty(),
        JType::Array(e) => JType::Array(Box::new(erase(e))),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Int(i32),
    Long(i64),
    Float(u32),
    Double(u64),
    Bool(bool),
    Char(u16),
    Str(Vec<u16>),
    Null,
    /// `Foo.class`
    Class(JType),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    Ushr,
    And,
    Or,
    Xor,
    Eq,
    Ne,
    Lt,
    Ge,
    Gt,
    Le,
    LAnd,
    LOr,
}

impl BinOp {
    pub fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Ushr => ">>>",
            BinOp::And => "&",
            BinOp::Or => "|",
            BinOp::Xor => "^",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Ge => ">=",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::LAnd => "&&",
            BinOp::LOr => "||",
        }
    }

    /// Java 运算符优先级，数字越大越紧。
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::LOr => 3,
            BinOp::LAnd => 4,
            BinOp::Or => 5,
            BinOp::Xor => 6,
            BinOp::And => 7,
            BinOp::Eq | BinOp::Ne => 8,
            BinOp::Lt | BinOp::Ge | BinOp::Gt | BinOp::Le => 9,
            BinOp::Shl | BinOp::Shr | BinOp::Ushr => 10,
            BinOp::Add | BinOp::Sub => 11,
            BinOp::Mul | BinOp::Div | BinOp::Rem => 12,
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Ge | BinOp::Gt | BinOp::Le
        )
    }

    /// 比较取反（浮点数的 NaN 语义不区分，和常见的源码写法一致）。
    pub fn negate_comparison(self) -> Option<BinOp> {
        Some(match self {
            BinOp::Eq => BinOp::Ne,
            BinOp::Ne => BinOp::Eq,
            BinOp::Lt => BinOp::Ge,
            BinOp::Ge => BinOp::Lt,
            BinOp::Gt => BinOp::Le,
            BinOp::Le => BinOp::Gt,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MethodRefTarget {
    /// `Type::name`
    Type(JType),
    /// `expr::name`
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LambdaBody {
    Expr(Expr),
    Block(Vec<Stmt>),
}

/// 匿名类实例：类体单独反编译，捕获的外层值放在 `captured` 里（类体里用
/// [`Expr::Captured`] 引用）。
#[derive(Debug, Clone, PartialEq)]
pub struct AnonClass {
    /// `new` 后面写的类型（父类或接口）
    pub super_ty: JType,
    pub super_args: Vec<Expr>,
    pub captured: Vec<Expr>,
    pub body: std::rc::Rc<crate::anon::AnonBody>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Lit),
    Var(VarId),
    This,
    /// `Outer.this`，参数是外层类的二进制名
    OuterThis(String),
    /// 异常处理器入口处栈上的异常对象
    Caught(Option<String>),
    /// 还没调用构造器的 `new`：(编号, 类)
    Uninit(u32, String),
    Field {
        target: Option<Box<Expr>>,
        owner: String,
        name: String,
        desc: String,
    },
    Invoke {
        kind: InvokeKind,
        target: Option<Box<Expr>>,
        owner: String,
        name: String,
        desc: String,
        args: Vec<Expr>,
    },
    New {
        class: String,
        desc: String,
        args: Vec<Expr>,
    },
    AnonNew(Box<AnonClass>),
    NewArray {
        elem: JType,
        dims: Vec<Expr>,
        /// 维度表达式之后还有几个 `[]`
        extra: u8,
        init: Option<Vec<Expr>>,
    },
    ArrayLen(Box<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// lcmp / fcmp / dcmp 的结果，等着被条件跳转消耗
    Cmp(Box<Expr>, Box<Expr>),
    Cast(JType, Box<Expr>),
    InstanceOf(Box<Expr>, JType),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Assign(Box<Expr>, Box<Expr>),
    CompoundAssign(BinOp, Box<Expr>, Box<Expr>),
    IncDec {
        target: Box<Expr>,
        inc: bool,
        prefix: bool,
    },
    Lambda {
        params: Vec<VarId>,
        body: Box<LambdaBody>,
    },
    MethodRef {
        target: MethodRefTarget,
        name: String,
    },
    /// 匿名类体里引用外层捕获的第几个值
    Captured(u32),
}

impl Expr {
    pub fn int(v: i32) -> Expr {
        Expr::Lit(Lit::Int(v))
    }

    pub fn bin(op: BinOp, a: Expr, b: Expr) -> Expr {
        Expr::Binary(op, Box::new(a), Box::new(b))
    }

    pub fn not(e: Expr) -> Expr {
        Expr::Unary(UnOp::Not, Box::new(e))
    }

    pub fn is_int_lit(&self, v: i32) -> bool {
        matches!(self, Expr::Lit(Lit::Int(x)) if *x == v)
    }

    /// 可以作为表达式语句的表达式。
    pub fn is_statement_expr(&self) -> bool {
        matches!(
            self,
            Expr::Invoke { .. }
                | Expr::New { .. }
                | Expr::AnonNew(_)
                | Expr::Assign(..)
                | Expr::CompoundAssign(..)
                | Expr::IncDec { .. }
        )
    }

    /// 对子表达式逐个调用（不进入 lambda / 匿名类体）。
    pub fn children(&self, f: &mut dyn FnMut(&Expr)) {
        match self {
            Expr::Lit(_)
            | Expr::Var(_)
            | Expr::This
            | Expr::OuterThis(_)
            | Expr::Caught(_)
            | Expr::Uninit(..)
            | Expr::Captured(_)
            | Expr::Lambda { .. } => {}
            Expr::Field { target, .. } => {
                if let Some(t) = target {
                    f(t);
                }
            }
            Expr::Invoke { target, args, .. } => {
                if let Some(t) = target {
                    f(t);
                }
                args.iter().for_each(f);
            }
            Expr::New { args, .. } => args.iter().for_each(f),
            Expr::AnonNew(anon) => {
                anon.super_args.iter().for_each(&mut *f);
                anon.captured.iter().for_each(f);
            }
            Expr::NewArray { dims, init, .. } => {
                dims.iter().for_each(&mut *f);
                if let Some(init) = init {
                    init.iter().for_each(f);
                }
            }
            Expr::ArrayLen(e) | Expr::Unary(_, e) | Expr::Cast(_, e) | Expr::InstanceOf(e, _) => {
                f(e)
            }
            Expr::IncDec { target, .. } => f(target),
            Expr::Index(a, b)
            | Expr::Binary(_, a, b)
            | Expr::Cmp(a, b)
            | Expr::Assign(a, b)
            | Expr::CompoundAssign(_, a, b) => {
                f(a);
                f(b);
            }
            Expr::Cond(a, b, c) => {
                f(a);
                f(b);
                f(c);
            }
            Expr::MethodRef { target, .. } => {
                if let MethodRefTarget::Expr(e) = target {
                    f(e);
                }
            }
        }
    }

    pub fn children_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        match self {
            Expr::Lit(_)
            | Expr::Var(_)
            | Expr::This
            | Expr::OuterThis(_)
            | Expr::Caught(_)
            | Expr::Uninit(..)
            | Expr::Captured(_)
            | Expr::Lambda { .. } => {}
            Expr::Field { target, .. } => {
                if let Some(t) = target {
                    f(t);
                }
            }
            Expr::Invoke { target, args, .. } => {
                if let Some(t) = target {
                    f(t);
                }
                args.iter_mut().for_each(f);
            }
            Expr::New { args, .. } => args.iter_mut().for_each(f),
            Expr::AnonNew(anon) => {
                anon.super_args.iter_mut().for_each(&mut *f);
                anon.captured.iter_mut().for_each(f);
            }
            Expr::NewArray { dims, init, .. } => {
                dims.iter_mut().for_each(&mut *f);
                if let Some(init) = init {
                    init.iter_mut().for_each(f);
                }
            }
            Expr::ArrayLen(e) | Expr::Unary(_, e) | Expr::Cast(_, e) | Expr::InstanceOf(e, _) => {
                f(e)
            }
            Expr::IncDec { target, .. } => f(target),
            Expr::Index(a, b)
            | Expr::Binary(_, a, b)
            | Expr::Cmp(a, b)
            | Expr::Assign(a, b)
            | Expr::CompoundAssign(_, a, b) => {
                f(a);
                f(b);
            }
            Expr::Cond(a, b, c) => {
                f(a);
                f(b);
                f(c);
            }
            Expr::MethodRef { target, .. } => {
                if let MethodRefTarget::Expr(e) = target {
                    f(e);
                }
            }
        }
    }

    /// 是否出现满足条件的子表达式（含自身，不进入 lambda）。
    pub fn any(&self, pred: &mut dyn FnMut(&Expr) -> bool) -> bool {
        if pred(self) {
            return true;
        }
        let mut found = false;
        self.children(&mut |c| {
            if !found && c.any(pred) {
                found = true;
            }
        });
        found
    }

    pub fn depth(&self) -> usize {
        let mut d = 0;
        self.children(&mut |c| d = d.max(c.depth()));
        d + 1
    }

    /// 求值不会有副作用，也不读可能被别处改掉的内存（常量、局部变量、this 和它们的运算）。
    pub fn is_pure(&self) -> bool {
        match self {
            Expr::Lit(_)
            | Expr::Var(_)
            | Expr::This
            | Expr::OuterThis(_)
            | Expr::Caught(_)
            | Expr::Uninit(..)
            | Expr::Captured(_) => true,
            // 除法可能抛异常，但不影响别的值，按纯表达式处理
            Expr::Unary(_, e) | Expr::Cast(_, e) | Expr::InstanceOf(e, _) => e.is_pure(),
            Expr::Binary(_, a, b) | Expr::Cmp(a, b) => a.is_pure() && b.is_pure(),
            Expr::ArrayLen(e) => e.is_pure(),
            Expr::Cond(a, b, c) => a.is_pure() && b.is_pure() && c.is_pure(),
            Expr::Lambda { .. } | Expr::MethodRef { .. } => {
                let mut pure = true;
                self.children(&mut |c| pure &= c.is_pure());
                pure
            }
            _ => false,
        }
    }

    /// 可以随意复制的表达式（`dup` 时不必引入临时变量）。
    pub fn is_copyable(&self) -> bool {
        match self {
            Expr::Lit(_)
            | Expr::Var(_)
            | Expr::This
            | Expr::OuterThis(_)
            | Expr::Caught(_)
            | Expr::Uninit(..)
            | Expr::Captured(_) => true,
            Expr::Field { target: None, .. } => false,
            _ => false,
        }
    }

    pub fn reads_var(&self, var: VarId) -> bool {
        self.any(&mut |e| matches!(e, Expr::Var(v) if *v == var))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CaseLabel {
    Int(i32),
    /// 渲染时按目标类型写成字符或枚举常量
    Char(u16),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// `None` 表示 `default`
    pub labels: Vec<Option<CaseLabel>>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Catch {
    /// 空表示捕获一切（渲染成 `Throwable`）
    pub types: Vec<String>,
    pub var: VarId,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopKind {
    While(Expr),
    DoWhile(Expr),
    For {
        init: Vec<Stmt>,
        cond: Option<Expr>,
        update: Vec<Expr>,
    },
    ForEach {
        var: VarId,
        iterable: Expr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    Local {
        var: VarId,
        init: Option<Expr>,
    },
    Return(Option<Expr>),
    Throw(Expr),
    If {
        cond: Expr,
        then: Vec<Stmt>,
        els: Vec<Stmt>,
    },
    Loop {
        label: Option<LabelId>,
        kind: LoopKind,
        body: Vec<Stmt>,
    },
    Switch {
        label: Option<LabelId>,
        expr: Expr,
        cases: Vec<Case>,
    },
    Try {
        body: Vec<Stmt>,
        catches: Vec<Catch>,
        finally: Option<Vec<Stmt>>,
    },
    Sync {
        lock: Expr,
        body: Vec<Stmt>,
    },
    Break(Option<LabelId>),
    Continue(Option<LabelId>),
    MonitorEnter(Expr),
    MonitorExit(Expr),
    Assert {
        cond: Expr,
        msg: Option<Expr>,
    },
}

impl Stmt {
    /// 语句执行完不会落到下一句。
    pub fn is_jump(&self) -> bool {
        match self {
            Stmt::Return(_) | Stmt::Throw(_) | Stmt::Break(_) | Stmt::Continue(_) => true,
            Stmt::If { then, els, .. } => ends_with_jump(then) && ends_with_jump(els),
            // finally 正常结束时 try 语句照样跳走
            Stmt::Try { body, catches, .. } => {
                ends_with_jump(body) && catches.iter().all(|c| ends_with_jump(&c.body))
            }
            Stmt::Sync { body, .. } => ends_with_jump(body),
            _ => false,
        }
    }

    /// 本层的表达式（不进入嵌套语句块）。
    pub fn exprs_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        match self {
            Stmt::Expr(e)
            | Stmt::Throw(e)
            | Stmt::If { cond: e, .. }
            | Stmt::Switch { expr: e, .. }
            | Stmt::Sync { lock: e, .. }
            | Stmt::MonitorEnter(e)
            | Stmt::MonitorExit(e) => f(e),
            Stmt::Local { init, .. } | Stmt::Return(init) => {
                if let Some(e) = init {
                    f(e);
                }
            }
            Stmt::Assert { cond, msg } => {
                f(cond);
                if let Some(m) = msg {
                    f(m);
                }
            }
            Stmt::Loop { kind, .. } => match kind {
                LoopKind::While(c) | LoopKind::DoWhile(c) => f(c),
                LoopKind::For { cond, update, .. } => {
                    if let Some(c) = cond {
                        f(c);
                    }
                    update.iter_mut().for_each(f);
                }
                LoopKind::ForEach { iterable, .. } => f(iterable),
            },
            Stmt::Try { .. } | Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    pub fn exprs(&self, f: &mut dyn FnMut(&Expr)) {
        match self {
            Stmt::Expr(e)
            | Stmt::Throw(e)
            | Stmt::If { cond: e, .. }
            | Stmt::Switch { expr: e, .. }
            | Stmt::Sync { lock: e, .. }
            | Stmt::MonitorEnter(e)
            | Stmt::MonitorExit(e) => f(e),
            Stmt::Local { init, .. } | Stmt::Return(init) => {
                if let Some(e) = init {
                    f(e);
                }
            }
            Stmt::Assert { cond, msg } => {
                f(cond);
                if let Some(m) = msg {
                    f(m);
                }
            }
            Stmt::Loop { kind, .. } => match kind {
                LoopKind::While(c) | LoopKind::DoWhile(c) => f(c),
                LoopKind::For { cond, update, .. } => {
                    if let Some(c) = cond {
                        f(c);
                    }
                    update.iter().for_each(f);
                }
                LoopKind::ForEach { iterable, .. } => f(iterable),
            },
            Stmt::Try { .. } | Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    /// 直接嵌套的语句块。
    pub fn blocks_mut(&mut self, f: &mut dyn FnMut(&mut Vec<Stmt>)) {
        match self {
            Stmt::If { then, els, .. } => {
                f(then);
                f(els);
            }
            Stmt::Loop { kind, body, .. } => {
                if let LoopKind::For { init, .. } = kind {
                    f(init);
                }
                f(body);
            }
            Stmt::Switch { cases, .. } => cases.iter_mut().for_each(|c| f(&mut c.body)),
            Stmt::Try {
                body,
                catches,
                finally,
            } => {
                f(body);
                catches.iter_mut().for_each(|c| f(&mut c.body));
                if let Some(fin) = finally {
                    f(fin);
                }
            }
            Stmt::Sync { body, .. } => f(body),
            _ => {}
        }
    }

    pub fn blocks(&self, f: &mut dyn FnMut(&Vec<Stmt>)) {
        match self {
            Stmt::If { then, els, .. } => {
                f(then);
                f(els);
            }
            Stmt::Loop { kind, body, .. } => {
                if let LoopKind::For { init, .. } = kind {
                    f(init);
                }
                f(body);
            }
            Stmt::Switch { cases, .. } => cases.iter().for_each(|c| f(&c.body)),
            Stmt::Try {
                body,
                catches,
                finally,
            } => {
                f(body);
                catches.iter().for_each(|c| f(&c.body));
                if let Some(fin) = finally {
                    f(fin);
                }
            }
            Stmt::Sync { body, .. } => f(body),
            _ => {}
        }
    }
}

pub fn ends_with_jump(block: &[Stmt]) -> bool {
    block.last().is_some_and(Stmt::is_jump)
}

/// 递归访问一棵语句树里的全部表达式（含嵌套块，进入 lambda 的语句体）。
pub fn walk_exprs(block: &[Stmt], f: &mut dyn FnMut(&Expr)) {
    for stmt in block {
        stmt.exprs(&mut |e| walk_expr_deep(e, f));
        stmt.blocks(&mut |b| walk_exprs(b, f));
    }
}

/// 访问表达式及其全部子表达式，进入 lambda 体。
pub fn walk_expr_deep(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    if let Expr::Lambda { body, .. } = e {
        match &**body {
            LambdaBody::Expr(b) => walk_expr_deep(b, f),
            LambdaBody::Block(stmts) => walk_exprs(stmts, f),
        }
    }
    e.children(&mut |c| walk_expr_deep(c, f));
}

/// 递归改写语句树里的每个表达式（后序，进入 lambda 体）。
pub fn rewrite_exprs(block: &mut [Stmt], f: &mut dyn FnMut(&mut Expr)) {
    for stmt in block.iter_mut() {
        stmt.exprs_mut(&mut |e| rewrite_expr_deep(e, f));
        stmt.blocks_mut(&mut |b| rewrite_exprs(b, f));
    }
}

pub fn rewrite_expr_deep(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    if let Expr::Lambda { body, .. } = e {
        match &mut **body {
            LambdaBody::Expr(b) => rewrite_expr_deep(b, f),
            LambdaBody::Block(stmts) => rewrite_exprs(stmts, f),
        }
    }
    e.children_mut(&mut |c| rewrite_expr_deep(c, f));
    f(e);
}

/// 局部变量（含形参、合成的临时变量）。
#[derive(Debug, Clone)]
pub struct VarInfo {
    pub name: Option<String>,
    pub ty: Option<JType>,
    pub kind: Kind,
    /// 来自 LocalVariableTable（源码里写过的变量）
    pub named_by_lvt: bool,
    /// 编译器为操作数栈引入的临时变量
    pub stack_temp: bool,
    pub is_param: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Vars {
    pub vars: Vec<VarInfo>,
}

impl Vars {
    pub fn add(&mut self, info: VarInfo) -> VarId {
        self.vars.push(info);
        (self.vars.len() - 1) as VarId
    }

    pub fn get(&self, id: VarId) -> &VarInfo {
        &self.vars[id as usize]
    }

    pub fn get_mut(&mut self, id: VarId) -> &mut VarInfo {
        &mut self.vars[id as usize]
    }

    pub fn name(&self, id: VarId) -> &str {
        self.vars[id as usize].name.as_deref().unwrap_or("var")
    }
}
