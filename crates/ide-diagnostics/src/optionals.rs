//! `Optional.get()`（以及 `OptionalInt.getAsInt()` 等）之前没有检查是否有值。
//!
//! 简化的判断：接收者是变量时，同一个方法体里对这个变量调用过 `isPresent()` / `isEmpty()`
//! 就认为检查过了；接收者是方法调用（`stream.findFirst().get()`）时直接报告，
//! `Optional.of(…).get()` 除外。

use base_db::{Db, SourceFile};
use hir::{class_data, InferenceResult, Resolution};
use syntax::{
    ast::{self, AstNode},
    SyntaxNode,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

const GETTERS: &[&str] = &["get", "getAsInt", "getAsLong", "getAsDouble"];
const CHECKS: &[&str] = &["isPresent", "isEmpty"];

fn optional_method(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
) -> Option<String> {
    let name = call.name_ref()?;
    let Resolution::Method(method) = result.resolution(name.syntax().text_range())? else {
        return None;
    };
    let data = class_data(db, method.class)?;
    data.binary_name
        .starts_with("java/util/Optional")
        .then(|| name.text().to_string())
}

/// 变量（局部变量 / 形参 / 字段）的解析结果，用来判断两处用的是不是同一个变量。
fn variable(result: &InferenceResult, expr: &ast::Expr) -> Option<Resolution> {
    let ast::Expr::NameExpr(name) = expr else {
        return None;
    };
    match result.resolution(name.name_ref()?.syntax().text_range())? {
        r @ (Resolution::Local(_) | Resolution::Field(_)) => Some(r.clone()),
        _ => None,
    }
}

fn checked(db: &dyn Db, result: &InferenceResult, body: &SyntaxNode, var: &Resolution) -> bool {
    body.descendants()
        .filter_map(ast::MethodCall::cast)
        .any(|call| {
            optional_method(db, result, &call).is_some_and(|n| CHECKS.contains(&n.as_str()))
                && call
                    .receiver()
                    .and_then(|r| variable(result, &r))
                    .is_some_and(|r| &r == var)
        })
}

fn normalized(node: &SyntaxNode) -> String {
    node.text()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn checked_by_text(
    db: &dyn Db,
    result: &InferenceResult,
    body: &SyntaxNode,
    receiver: &ast::Expr,
) -> bool {
    let text = normalized(receiver.syntax());
    body.descendants()
        .filter_map(ast::MethodCall::cast)
        .any(|call| {
            optional_method(db, result, &call).is_some_and(|n| CHECKS.contains(&n.as_str()))
                && call
                    .receiver()
                    .is_some_and(|r| normalized(r.syntax()) == text)
        })
}

pub(crate) fn unchecked_optional_gets(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    for body in &view.bodies {
        let node = body.node.clone();
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for call in node.descendants().filter_map(ast::MethodCall::cast) {
            let Some(name) = call.name_ref() else {
                continue;
            };
            if !GETTERS.contains(&name.text().as_str())
                || call.arg_list().is_none_or(|a| a.args().next().is_some())
            {
                continue;
            }
            let result = result.get_or_insert_with(|| body.result(view));
            if optional_method(db, result, &call).is_none() {
                continue;
            }
            let Some(receiver) = call.receiver() else {
                continue;
            };
            let report = match &receiver {
                ast::Expr::NameExpr(_) => match variable(result, &receiver) {
                    Some(var) => !checked(db, result, &node, &var),
                    None => false,
                },
                // `Optional.of(x).get()` 一定有值；`a.b().isPresent()` 之后的 `a.b().get()`
                // 按原文认为检查过了
                ast::Expr::MethodCall(inner) => {
                    !inner.name_ref().is_some_and(|n| n.text() == "of")
                        && !checked_by_text(db, result, &node, &receiver)
                }
                _ => false,
            };
            if !report {
                continue;
            }
            let method = name.text().to_string();
            out.push(Diagnostic {
                range: name.syntax().text_range(),
                message: base_db::tr!(
                    "`{method}()` 之前没有检查 `isPresent()`，没有值时会抛异常",
                    "`{method}()` without an `isPresent()` check throws when there is no value"
                ),
                severity: Severity::Warning,
                code: DiagnosticCode::OptionalGetWithoutCheck,
                unused: false,
            });
        }
    }
}

/// 给快速修复用。
pub fn unchecked_optional_get_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    unchecked_optional_gets(&View::file(db, file), &mut out);
    out
}
