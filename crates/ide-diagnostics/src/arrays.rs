//! 数组的常见误用：
//!
//! - `a.equals(b)` 比较的是引用，不是内容（应当用 `Arrays.equals`）；
//! - 把数组转成字符串：`"x" + a`、`a.toString()`、`String.valueOf(a)`、`println(a)`、
//!   `sb.append(a)` 得到的是 `[I@1b6d3586` 这样的类型名加哈希值（应当用 `Arrays.toString`）。
//!   `char[]` 有专门的 `valueOf` / `println` / `append` 重载，按内容输出，不报。

use base_db::{Db, SourceFile};
use hir::{class_header, InferenceResult, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

fn is_array(ty: Option<&Ty>) -> bool {
    matches!(ty, Some(Ty::Array(_)))
}

fn class_name(db: &dyn Db, ty: &Ty) -> Option<String> {
    ty.as_class()
        .and_then(|c| class_header(db, c))
        .map(|d| d.binary_name.to_string())
}

fn to_string_warning(range: TextRange) -> Diagnostic {
    Diagnostic {
        range,
        message: base_db::trs!(
            "数组转成字符串只会得到类型名和哈希值，应当用 `Arrays.toString()`",
            "Converting an array to a string gives its type and hash code; use `Arrays.toString()`"
        )
        .to_owned(),
        severity: Severity::Warning,
        code: DiagnosticCode::ArrayToString,
        unused: false,
    }
}

fn check_call(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<Diagnostic>,
) -> Option<()> {
    let name = call.name_ref()?;
    let Resolution::Method(method) = result.resolution(name.syntax().text_range())? else {
        return None;
    };
    let method_name = name.text().to_string();
    let type_of = |e: &ast::Expr| result.type_of(e.syntax().text_range());
    let args: Vec<ast::Expr> = call.arg_list()?.args().collect();
    let receiver = call.receiver();

    // `a.equals(b)` / `a.toString()`：数组上的方法都继承自 Object
    if let Some(receiver) = &receiver {
        if is_array(type_of(receiver)) {
            match (method_name.as_str(), args.len()) {
                ("equals", 1) => out.push(Diagnostic {
                    range: call.syntax().text_range(),
                    message: base_db::trs!(
                        "数组的 `equals()` 比较的是引用，不是内容，应当用 `Arrays.equals()`",
                        "`equals()` on an array compares references, not contents; use `Arrays.equals()`"
                    )
                    .to_owned(),
                    severity: Severity::Warning,
                    code: DiagnosticCode::ArrayEquals,
                    unused: false,
                }),
                ("toString", 0) => out.push(to_string_warning(call.syntax().text_range())),
                _ => {}
            }
            return Some(());
        }
    }

    // 把数组当 Object 传给会把它转成字符串的方法
    let owner = class_header(db, method.class)?.binary_name.to_string();
    let stringifies = matches!(
        (owner.as_str(), method_name.as_str()),
        ("java/lang/String", "valueOf")
            | (
                "java/io/PrintStream" | "java/io/PrintWriter",
                "print" | "println"
            )
            | (
                "java/lang/StringBuilder"
                    | "java/lang/StringBuffer"
                    | "java/lang/AbstractStringBuilder",
                "append"
            )
    );
    if !stringifies || args.len() != 1 {
        return Some(());
    }
    let sig = hir::method_sig(db, *method);
    let object_param = sig
        .params
        .first()
        .and_then(|p| class_name(db, p))
        .is_some_and(|n| n == "java/lang/Object");
    if object_param && is_array(type_of(&args[0])) {
        out.push(to_string_warning(args[0].syntax().text_range()));
    }
    Some(())
}

fn is_string(db: &dyn Db, ty: Option<&Ty>) -> bool {
    ty.and_then(|t| class_name(db, t))
        .is_some_and(|n| n == "java/lang/String")
}

pub(crate) fn array_diagnostics(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    for body in &view.bodies {
        let result = body.result(view);
        for n in body.node.descendants() {
            match n.kind() {
                SyntaxKind::METHOD_CALL => {
                    if let Some(call) = ast::MethodCall::cast(n) {
                        check_call(db, &result, &call, out);
                    }
                }
                SyntaxKind::BINARY_EXPR => {
                    let Some(binary) = ast::BinaryExpr::cast(n) else {
                        continue;
                    };
                    if binary.op_kind() != Some(SyntaxKind::PLUS)
                        || !is_string(db, result.type_of(binary.syntax().text_range()))
                    {
                        continue;
                    }
                    for operand in [binary.lhs(), binary.rhs()].into_iter().flatten() {
                        if is_array(result.type_of(operand.syntax().text_range())) {
                            out.push(to_string_warning(operand.syntax().text_range()));
                        }
                    }
                }
                SyntaxKind::ASSIGN_EXPR => {
                    // `s += a`
                    let Some(assign) = ast::AssignExpr::cast(n) else {
                        continue;
                    };
                    if assign.op_token().map(|t| t.kind()) != Some(SyntaxKind::PLUS_EQ) {
                        continue;
                    }
                    let (Some(lhs), Some(rhs)) = (assign.lhs(), assign.rhs()) else {
                        continue;
                    };
                    if is_string(db, result.type_of(lhs.syntax().text_range()))
                        && is_array(result.type_of(rhs.syntax().text_range()))
                    {
                        out.push(to_string_warning(rhs.syntax().text_range()));
                    }
                }
                _ => {}
            }
        }
    }
}

/// 数组误用的诊断（给快速修复用）。
pub fn array_misuse(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    array_diagnostics(&View::file(db, file), &mut out);
    out
}
