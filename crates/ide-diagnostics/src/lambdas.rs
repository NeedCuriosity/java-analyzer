//! 可以换成方法引用的 lambda：`s -> s.trim()`、`x -> foo(x)`、`() -> new Box()`、
//! `x -> list.add(x)`（`list` 是局部变量）。
//!
//! 条件和"把 lambda 换成方法引用"一致，另外为了不改变语义，绑定的接收者只接受局部变量、
//! `this` 和 `super`（字段可能在两次调用之间被改掉，而方法引用只在创建时求值一次接收者）。

use base_db::Db;
use hir::{class_data, InferenceResult, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

fn param_names(lambda: &ast::LambdaExpr) -> Option<Vec<String>> {
    lambda
        .param_list()?
        .params()
        .map(|p| p.name().map(|n| n.text().to_string()))
        .collect()
}

fn args_are(args: &ast::ArgList, names: &[String]) -> bool {
    let args: Vec<ast::Expr> = args.args().collect();
    args.len() == names.len()
        && args.iter().zip(names).all(|(arg, name)| {
            matches!(arg, ast::Expr::NameExpr(n) if n.syntax().text() == name.as_str())
        })
}

fn body_expr(lambda: &ast::LambdaExpr) -> Option<ast::Expr> {
    if let Some(e) = lambda.body_expr() {
        return Some(e);
    }
    let block = lambda.body_block()?;
    let mut stmts = block.statements();
    match (stmts.next()?, stmts.next()) {
        (ast::Stmt::ReturnStmt(r), None) => r.expr(),
        (ast::Stmt::ExprStmt(s), None) => s.expr(),
        _ => None,
    }
}

/// `class` 里名为 `name` 的方法同时有能接收 `arity` 个实参的静态和实例重载。
fn ambiguous(db: &dyn Db, class: hir::ClassId, name: &str, arity: usize) -> bool {
    let mut statics = false;
    let mut instances = false;
    for candidate in hir::all_methods(db, &hir::this_ty(db, class)) {
        let Some(method) = class_data(db, candidate.id.class)
            .and_then(|d| d.methods.get(candidate.id.idx as usize).cloned())
        else {
            continue;
        };
        if method.name != name {
            continue;
        }
        if method.flags.is_static() && method.params.len() == arity {
            statics = true;
        }
        if !method.flags.is_static() && method.params.len() + 1 == arity {
            instances = true;
        }
    }
    statics && instances
}

fn convertible(db: &dyn Db, result: &InferenceResult, lambda: &ast::LambdaExpr) -> Option<()> {
    let params = param_names(lambda)?;
    match body_expr(lambda)? {
        ast::Expr::NewExpr(new) => {
            if new.body().is_some() || new.qualifier().is_some() {
                return None;
            }
            let ast::Type::ClassType(_) = new.ty()? else {
                return None;
            };
            args_are(&new.arg_list()?, &params).then_some(())
        }
        ast::Expr::MethodCall(call) => {
            if call.type_arg_list().is_some() {
                return None;
            }
            let name = call.name_ref()?;
            let args = call.arg_list()?;
            let Resolution::Method(method) = result.resolution(name.syntax().text_range())? else {
                return None;
            };
            let method_data = class_data(db, method.class)?
                .methods
                .get(method.idx as usize)?
                .clone();
            match call.receiver() {
                None => args_are(&args, &params).then_some(()),
                Some(receiver) => {
                    let first_is_receiver = matches!(&receiver,
                        ast::Expr::NameExpr(n) if params.first().is_some_and(|p| n.syntax().text() == p.as_str()));
                    if first_is_receiver {
                        if !args_are(&args, &params[1..]) || method_data.flags.is_static() {
                            return None;
                        }
                        let Ty::Class(class, _) = result.type_of(receiver.syntax().text_range())?
                        else {
                            return None;
                        };
                        (!ambiguous(db, *class, name.text().as_str(), params.len())).then_some(())
                    } else {
                        let stable = match &receiver {
                            ast::Expr::ThisExpr(_) | ast::Expr::SuperExpr(_) => true,
                            ast::Expr::NameExpr(n) => {
                                let local = n.name_ref().and_then(|r| {
                                    result.resolution(r.syntax().text_range()).cloned()
                                });
                                matches!(local, Some(Resolution::Local(_)))
                                    && !params.iter().any(|p| n.syntax().text() == p.as_str())
                            }
                            _ => false,
                        };
                        if !stable || !args_are(&args, &params) {
                            return None;
                        }
                        if method_data.flags.is_static()
                            && ambiguous(db, method.class, name.text().as_str(), params.len())
                        {
                            return None;
                        }
                        Some(())
                    }
                }
            }
        }
        _ => None,
    }
}

pub(crate) fn lambdas_to_method_refs(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    for body in &view.bodies {
        let node = &body.node;
        if !node
            .descendants()
            .any(|n| n.kind() == SyntaxKind::LAMBDA_EXPR)
        {
            continue;
        }
        let result = body.result(view);
        for lambda in node.descendants().filter_map(ast::LambdaExpr::cast) {
            if convertible(db, &result, &lambda).is_none() {
                continue;
            }
            // 标在形参和箭头上（换成方法引用的操作在这里提供）
            let start = lambda.syntax().text_range().start();
            let end = lambda
                .arrow_token()
                .map(|t| t.text_range().end())
                .unwrap_or_else(|| lambda.syntax().text_range().end());
            out.push(Diagnostic {
                range: TextRange::new(start, end),
                message: base_db::trs!(
                    "lambda 可以换成方法引用",
                    "Lambda can be replaced with a method reference"
                )
                .to_owned(),
                severity: Severity::Hint,
                code: DiagnosticCode::LambdaToMethodRef,
                unused: false,
            });
        }
    }
}
