//! 匿名类：`new Outer$1(this, x)` 还原成 `new Runnable() { ... }`，类体就地反编译。
//!
//! 匿名类的构造器把外层实例和捕获的局部变量存进合成字段（`this$0`、`val$x`），
//! 类体里对这些字段的读取换成 [`Expr::Captured`]，渲染时写成外层的表达式。

use std::fmt;

use classfile::{
    access,
    code::{parse_class_code, CodeClass},
    signature::{self, JType},
};
use rustc_hash::FxHashMap;

use crate::{
    ir::*,
    render::{Printer, Scope},
    MethodCx, MAX_NESTING,
};

pub enum AnonMember {
    Field {
        access: u16,
        name: String,
        ty: JType,
    },
    /// 构造器里除了编译器生成的部分之外的语句（字段初始化等），写成实例初始化块
    Init { vars: Vars, body: Vec<Stmt> },
    Method {
        method: usize,
        vars: Vars,
        params: Vec<VarId>,
        body: Result<Vec<Stmt>, crate::Fail>,
    },
}

pub struct AnonBody {
    pub class: CodeClass,
    pub members: Vec<AnonMember>,
}

impl PartialEq for AnonBody {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl fmt::Debug for AnonBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AnonBody({})", self.class.name)
    }
}

fn is_anonymous(cx: &MethodCx<'_>, class: &str) -> bool {
    cx.class
        .inner_classes
        .iter()
        .any(|ic| ic.inner == class && ic.inner_name.is_none() && ic.outer.is_none())
}

/// 试着把 `new class(args)` 还原成匿名类表达式；做不到时返回 `None`，按普通 `new` 输出。
pub fn anonymous_new(
    cx: &mut MethodCx<'_>,
    class: &str,
    desc: &str,
    args: &[Expr],
) -> Option<AnonClass> {
    if cx.depth >= MAX_NESTING || !is_anonymous(cx, class) {
        return None;
    }
    let bytes = cx.env.class_bytes(class)?;
    let anon = parse_class_code(&bytes).ok()?;
    if anon.enclosing_method.as_ref().map(|(c, _)| c.as_str()) != Some(cx.class.name.as_str()) {
        return None;
    }
    let super_name = anon.super_name.clone()?;
    let class_sig = anon
        .signature
        .as_deref()
        .and_then(signature::parse_class_sig);
    let super_ty = if super_name == "java/lang/Object" && anon.interfaces.len() == 1 {
        class_sig
            .as_ref()
            .and_then(|s| s.interfaces.first().cloned())
            .unwrap_or_else(|| class_ty(&anon.interfaces[0]))
    } else {
        class_sig
            .as_ref()
            .map(|s| s.super_class.clone())
            .unwrap_or_else(|| class_ty(&super_name))
    };

    // 构造器：哪些形参存进了哪个合成字段、父类构造器的实参
    let ctor_idx = anon
        .methods
        .iter()
        .position(|m| m.name == "<init>" && m.descriptor == desc)?;
    let mut ctor_vars = Vars::default();
    let ast = {
        let mut sub = MethodCx {
            class: &anon,
            method: &anon.methods[ctor_idx],
            env: cx.env,
            vars: &mut ctor_vars,
            budget: &mut *cx.budget,
            labels: &mut *cx.labels,
            depth: cx.depth + 1,
            raw_ctor: true,
            synthetic_params: 0,
        };
        crate::decompile_body(&mut sub).ok()?
    };
    if ast.params.len() != args.len() {
        return None;
    }
    let param_index = |v: VarId| ast.params.iter().position(|p| *p == v);
    let is_synthetic_field = |name: &str| {
        anon.find_field(name)
            .is_some_and(|f| f.access & access::SYNTHETIC != 0)
    };
    let mut captured_fields: FxHashMap<String, u32> = FxHashMap::default();
    let mut super_args = None;
    let mut init = Vec::new();
    let is_param = |v: VarId| ast.params.contains(&v);
    for stmt in ast
        .body
        .iter()
        .filter(|s| !crate::transform::is_null_check_of(s, &is_param))
        .cloned()
        .collect::<Vec<_>>()
    {
        match &stmt {
            Stmt::Expr(Expr::Assign(lhs, rhs)) => {
                if let (
                    Expr::Field {
                        target: Some(t),
                        owner,
                        name,
                        ..
                    },
                    Expr::Var(v),
                ) = (&**lhs, &**rhs)
                {
                    if **t == Expr::This && *owner == anon.name && is_synthetic_field(name) {
                        if let Some(k) = param_index(*v) {
                            captured_fields.insert(name.clone(), k as u32);
                            continue;
                        }
                    }
                }
                init.push(stmt);
            }
            Stmt::Expr(Expr::Invoke {
                target: Some(t),
                name,
                owner,
                args: sargs,
                ..
            }) if **t == Expr::This
                && name == "<init>"
                && *owner == super_name
                && super_args.is_none() =>
            {
                // 父类构造器的实参用外层的值写
                let mut mapped = Vec::new();
                for a in sargs {
                    let mut a = a.clone();
                    let mut ok = true;
                    rewrite_expr_deep(&mut a, &mut |e| {
                        if let Expr::Var(v) = e {
                            match param_index(*v) {
                                Some(k) => *e = args[k].clone(),
                                None => ok = false,
                            }
                        }
                    });
                    if !ok {
                        return None;
                    }
                    mapped.push(a);
                }
                super_args = Some(mapped);
            }
            _ => init.push(stmt),
        }
    }
    let super_args = super_args?;
    // 初始化语句不能再引用构造器形参
    let mut uses_param = false;
    walk_exprs(&init, &mut |e| {
        if let Expr::Var(v) = e {
            if param_index(*v).is_some() {
                uses_param = true;
            }
        }
    });
    if uses_param {
        return None;
    }

    let captured_rewrite = |body: &mut Vec<Stmt>| {
        rewrite_exprs(body, &mut |e| {
            if let Expr::Field {
                target: Some(t),
                owner,
                name,
                ..
            } = e
            {
                if **t == Expr::This && *owner == anon.name {
                    if let Some(&k) = captured_fields.get(name) {
                        *e = Expr::Captured(k);
                    }
                }
            }
        })
    };

    let mut members = Vec::new();
    for f in &anon.fields {
        if f.access & access::SYNTHETIC != 0 {
            continue;
        }
        let ty = f
            .signature
            .as_deref()
            .and_then(signature::parse_field_type)
            .or_else(|| signature::parse_field_type(&f.descriptor))?;
        members.push(AnonMember::Field {
            access: f.access,
            name: f.name.clone(),
            ty,
        });
    }
    captured_rewrite(&mut init);
    if !init.is_empty() {
        crate::vars::name_all(&init, &mut ctor_vars, &[]);
        members.push(AnonMember::Init {
            vars: ctor_vars,
            body: init,
        });
    }
    for (idx, m) in anon.methods.iter().enumerate() {
        if m.name.starts_with('<') || m.access & (access::SYNTHETIC | access::BRIDGE) != 0 {
            continue;
        }
        let mut vars = Vars::default();
        let result = if m.code.is_some() {
            let mut sub = MethodCx {
                class: &anon,
                method: m,
                env: cx.env,
                vars: &mut vars,
                budget: &mut *cx.budget,
                labels: &mut *cx.labels,
                depth: cx.depth + 1,
                raw_ctor: false,
                synthetic_params: 0,
            };
            crate::decompile_body(&mut sub)
        } else {
            Err(crate::Fail("没有方法体"))
        };
        let (params, body) = match result {
            Ok(mut ast) => {
                captured_rewrite(&mut ast.body);
                crate::name_params(&mut vars, &ast.params, m, None);
                crate::vars::name_all(&ast.body, &mut vars, &ast.params);
                (ast.params, Ok(ast.body))
            }
            Err(e) => (Vec::new(), Err(e)),
        };
        members.push(AnonMember::Method {
            method: idx,
            vars,
            params,
            body,
        });
    }
    Some(AnonClass {
        super_ty,
        super_args,
        captured: args.to_vec(),
        body: std::rc::Rc::new(AnonBody {
            class: anon,
            members,
        }),
    })
}

pub fn render_members<'a>(p: &mut Printer<'_, 'a>, anon: &'a AnonClass, indent: usize) {
    let body: &'a AnonBody = &anon.body;
    let mut first = true;
    for member in &body.members {
        if !first {
            p.out.push('\n');
        }
        first = false;
        for _ in 0..indent {
            p.out.push_str("    ");
        }
        match member {
            AnonMember::Field {
                access: acc,
                name,
                ty,
            } => {
                let mods = crate::header::modifiers(*acc, false);
                p.out.push_str(&mods);
                let t = p.ty(ty);
                p.out.push_str(&t);
                p.out.push(' ');
                p.out.push_str(name);
                p.out.push_str(";\n");
            }
            AnonMember::Init { vars, body: stmts } => {
                p.scopes.push(Scope::new(&body.class, vars, &anon.captured));
                p.braced(stmts, indent);
                p.scopes.pop();
                p.out.push('\n');
            }
            AnonMember::Method {
                method,
                vars,
                params,
                body: result,
            } => {
                let m = &body.class.methods[*method];
                let names: Vec<String> = params.iter().map(|v| vars.name(*v).to_owned()).collect();
                let header = crate::header::method_header(p.names, p.env, &body.class, m, &names);
                p.out.push_str(&header);
                match result {
                    Ok(stmts) => {
                        p.out.push(' ');
                        p.scopes.push(Scope::new(&body.class, vars, &anon.captured));
                        p.braced(stmts, indent);
                        p.scopes.pop();
                        p.out.push('\n');
                    }
                    Err(e) if m.access & (access::ABSTRACT | access::NATIVE) == 0 => {
                        p.out
                            .push_str(&format!(" {{ /* 无法反编译：{} */ }}\n", e.0));
                    }
                    Err(_) => p.out.push_str(";\n"),
                }
            }
        }
    }
}
