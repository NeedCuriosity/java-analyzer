//! `LambdaMetafactory` 生成的 invokedynamic：本类合成的 `lambda$...` 方法内联成 lambda，
//! 其余写成方法引用。

use classfile::{
    access,
    code::{handle_kind, BootstrapMethod, CodeMethod, MethodHandle},
};

use crate::{ir::*, types::owner_ty, Fail, MethodCx, MAX_NESTING};

pub fn lambda_expr(
    cx: &mut MethodCx<'_>,
    bsm: &BootstrapMethod,
    _sam_name: &str,
    captured: Vec<Expr>,
) -> Result<Expr, Fail> {
    let class = cx.class;
    let handle = bsm
        .arguments
        .get(1)
        .and_then(|&i| class.pool.method_handle(i))
        .ok_or(Fail("lambda 的实现方法不合法"))?;
    if handle.member.owner == class.name && cx.depth < MAX_NESTING {
        if let Some(target) = class.find_method(&handle.member.name, &handle.member.descriptor) {
            if target.access & access::SYNTHETIC != 0 && target.code.is_some() {
                if let Some(e) = inline_lambda(cx, target, &captured) {
                    return Ok(e);
                }
            }
        }
    }
    method_ref(handle, captured)
}

fn inline_lambda<'a>(
    cx: &mut MethodCx<'a>,
    target: &'a CodeMethod,
    captured: &[Expr],
) -> Option<Expr> {
    let mut captured = captured.to_vec();
    if !target.is_static() {
        if captured.first() != Some(&Expr::This) {
            return None;
        }
        captured.remove(0);
    }
    if !captured.iter().all(Expr::is_copyable) {
        return None;
    }
    let mark = cx.vars.vars.len();
    let result = {
        let mut sub = MethodCx {
            class: cx.class,
            method: target,
            env: cx.env,
            vars: &mut *cx.vars,
            budget: &mut *cx.budget,
            labels: &mut *cx.labels,
            depth: cx.depth + 1,
            raw_ctor: false,
            synthetic_params: 0,
        };
        crate::decompile_body(&mut sub)
    };
    let ast = match result {
        Ok(ast) => ast,
        Err(_) => {
            cx.vars.vars.truncate(mark);
            return None;
        }
    };
    if ast.params.len() < captured.len() {
        return None;
    }
    let (cap_params, lambda_params) = ast.params.split_at(captured.len());
    let mut body = ast.body;
    let mut reassigned = false;
    walk_exprs(&body, &mut |e| {
        let target = match e {
            Expr::Assign(l, _) | Expr::CompoundAssign(_, l, _) => Some(&**l),
            Expr::IncDec { target, .. } => Some(&**target),
            _ => None,
        };
        if let Some(Expr::Var(v)) = target {
            if cap_params.contains(v) {
                reassigned = true;
            }
        }
    });
    if reassigned {
        return None;
    }
    let subst: Vec<(VarId, Expr)> = cap_params.iter().copied().zip(captured).collect();
    rewrite_exprs(&mut body, &mut |e| {
        if let Expr::Var(v) = e {
            if let Some((_, x)) = subst.iter().find(|(p, _)| p == v) {
                *e = x.clone();
            }
        }
    });
    let void = target.descriptor.ends_with(")V");
    let body = match body.as_slice() {
        [Stmt::Return(Some(e))] if !void => LambdaBody::Expr(e.clone()),
        [Stmt::Expr(e)] if void && e.is_statement_expr() => LambdaBody::Expr(e.clone()),
        _ => LambdaBody::Block(body),
    };
    Some(Expr::Lambda {
        params: lambda_params.to_vec(),
        body: Box::new(body),
    })
}

fn method_ref(handle: MethodHandle, mut captured: Vec<Expr>) -> Result<Expr, Fail> {
    let m = handle.member;
    let owner = owner_ty(&m.owner);
    let target = match (handle.kind, captured.len()) {
        (handle_kind::INVOKE_STATIC, 0)
        | (handle_kind::INVOKE_VIRTUAL | handle_kind::INVOKE_INTERFACE, 0) => {
            MethodRefTarget::Type(owner)
        }
        (
            handle_kind::INVOKE_VIRTUAL
            | handle_kind::INVOKE_INTERFACE
            | handle_kind::INVOKE_SPECIAL,
            1,
        ) => MethodRefTarget::Expr(Box::new(captured.remove(0))),
        (handle_kind::NEW_INVOKE_SPECIAL, 0) => {
            return Ok(Expr::MethodRef {
                target: MethodRefTarget::Type(owner),
                name: "new".to_owned(),
            })
        }
        _ => return Err(Fail("不支持的方法引用形式")),
    };
    Ok(Expr::MethodRef {
        target,
        name: m.name,
    })
}
