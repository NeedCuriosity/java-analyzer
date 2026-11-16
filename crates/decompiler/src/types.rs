//! 表达式的静态类型（尽力而为，只用于常量的写法、字符串拼接和变量声明）。

use classfile::{
    code::CodeClass,
    signature::{self, BaseType, JType},
};

use crate::{
    ir::*,
    translate::{class_name_ty, field_desc, method_desc},
};

pub struct TypeCx<'a> {
    pub class: &'a CodeClass,
    pub vars: &'a Vars,
}

fn is_base(ty: &JType, b: BaseType) -> bool {
    matches!(ty, JType::Base(x) if *x == b)
}

pub fn is_boolean(ty: &JType) -> bool {
    is_base(ty, BaseType::Boolean)
}

pub fn is_char(ty: &JType) -> bool {
    is_base(ty, BaseType::Char)
}

/// 二元数值提升。
fn promote(a: Option<JType>, b: Option<JType>) -> Option<JType> {
    let rank = |t: &Option<JType>| match t {
        Some(JType::Base(BaseType::Double)) => 4,
        Some(JType::Base(BaseType::Float)) => 3,
        Some(JType::Base(BaseType::Long)) => 2,
        _ => 1,
    };
    Some(base(match rank(&a).max(rank(&b)) {
        4 => BaseType::Double,
        3 => BaseType::Float,
        2 => BaseType::Long,
        _ => BaseType::Int,
    }))
}

impl TypeCx<'_> {
    pub fn ty(&self, e: &Expr) -> Option<JType> {
        match e {
            Expr::Lit(l) => match l {
                Lit::Int(_) => Some(base(BaseType::Int)),
                Lit::Long(_) => Some(base(BaseType::Long)),
                Lit::Float(_) => Some(base(BaseType::Float)),
                Lit::Double(_) => Some(base(BaseType::Double)),
                Lit::Bool(_) => Some(base(BaseType::Boolean)),
                Lit::Char(_) => Some(base(BaseType::Char)),
                Lit::Str(_) => Some(string_ty()),
                Lit::Null => None,
                Lit::Class(_) => Some(class_ty("java/lang/Class")),
            },
            Expr::Var(v) => {
                let info = self.vars.get(*v);
                info.ty.clone().or_else(|| match info.kind {
                    crate::bytecode::Kind::Ref => None,
                    k => Some(ty_of_kind(k)),
                })
            }
            Expr::This => Some(class_ty(&self.class.name)),
            Expr::OuterThis(c) => Some(class_ty(c)),
            Expr::Caught(t) => Some(class_ty(t.as_deref().unwrap_or("java/lang/Throwable"))),
            Expr::Uninit(_, c) => Some(class_ty(c)),
            Expr::Field { desc, .. } => field_desc(desc).ok(),
            Expr::Invoke { desc, .. } => method_desc(desc).ok().and_then(|(_, r)| r),
            Expr::New { class, .. } => Some(class_ty(class)),
            Expr::AnonNew(a) => Some(a.super_ty.clone()),
            Expr::NewArray {
                elem,
                dims,
                extra,
                init,
            } => {
                let mut t = elem.clone();
                let n = if init.is_some() { 1 } else { dims.len() };
                for _ in 0..n + *extra as usize {
                    t = JType::Array(Box::new(t));
                }
                Some(t)
            }
            Expr::ArrayLen(_) => Some(base(BaseType::Int)),
            Expr::Index(a, _) => match self.ty(a) {
                Some(JType::Array(e)) => Some(*e),
                _ => None,
            },
            Expr::Unary(UnOp::Not, _) => Some(base(BaseType::Boolean)),
            Expr::Unary(_, x) => promote(self.ty(x), None),
            Expr::Binary(op, a, b) => match op {
                BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Ge
                | BinOp::Gt
                | BinOp::Le
                | BinOp::LAnd
                | BinOp::LOr => Some(base(BaseType::Boolean)),
                BinOp::Add => {
                    let (ta, tb) = (self.ty(a), self.ty(b));
                    if ta.as_ref().is_some_and(is_string) || tb.as_ref().is_some_and(is_string) {
                        Some(string_ty())
                    } else {
                        promote(ta, tb)
                    }
                }
                BinOp::And | BinOp::Or | BinOp::Xor => {
                    let ta = self.ty(a);
                    if ta.as_ref().is_some_and(is_boolean) {
                        ta
                    } else {
                        promote(ta, self.ty(b))
                    }
                }
                BinOp::Shl | BinOp::Shr | BinOp::Ushr => promote(self.ty(a), None),
                _ => promote(self.ty(a), self.ty(b)),
            },
            Expr::Cmp(..) => Some(base(BaseType::Int)),
            Expr::Cast(t, _) => Some(t.clone()),
            Expr::InstanceOf(..) => Some(base(BaseType::Boolean)),
            Expr::Cond(_, a, b) => self.ty(a).or_else(|| self.ty(b)),
            Expr::Assign(l, _) | Expr::CompoundAssign(_, l, _) => self.ty(l),
            Expr::IncDec { target, .. } => self.ty(target),
            Expr::Lambda { .. } | Expr::MethodRef { .. } | Expr::Captured(_) => None,
        }
    }

    pub fn is_boolean(&self, e: &Expr) -> bool {
        self.ty(e).as_ref().is_some_and(is_boolean)
    }

    pub fn is_char(&self, e: &Expr) -> bool {
        self.ty(e).as_ref().is_some_and(is_char)
    }

    pub fn is_string(&self, e: &Expr) -> bool {
        self.ty(e).as_ref().is_some_and(is_string)
    }
}

/// 方法的返回类型和形参类型：有泛型签名且个数对得上时用签名。
pub fn method_types(method: &classfile::code::CodeMethod) -> (Vec<JType>, Option<JType>) {
    let desc = method_desc(&method.descriptor).unwrap_or_default();
    if let Some(sig) = method
        .signature
        .as_deref()
        .and_then(signature::parse_method_sig)
    {
        if sig.params.len() == desc.0.len() {
            return (sig.params, sig.ret);
        }
        return (desc.0, sig.ret.or(desc.1));
    }
    desc
}

pub fn owner_ty(owner: &str) -> JType {
    class_name_ty(owner).unwrap_or_else(|_| class_ty(owner))
}
