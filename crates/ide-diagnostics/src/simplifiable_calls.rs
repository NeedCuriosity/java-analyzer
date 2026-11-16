//! 可以用更直接的 API 写的调用（都带修复）：
//!
//! - Stream 调用链：`.filter(p).findFirst().isPresent()` → `.anyMatch(p)`（`isEmpty()` →
//!   `noneMatch(p)`），`coll.stream().forEach(f)` → `coll.forEach(f)`，
//!   `Arrays.asList(array).stream()` → `Arrays.stream(array)`，`.map(x -> x)` 删掉；
//! - `Collections.sort(list[, cmp])` → `list.sort(cmp | null)`；
//! - `s.indexOf(x) != -1`（`>= 0` / `== -1` / `< 0`）→ `s.contains(x)` / `!s.contains(x)`，
//!   接收者是 `String`（实参也是字符串）或 `List`；
//! - 字符串拼接里多余的 `String.valueOf(x)`（`x` 不是 `char[]`）。
//!
//! 每个改写都要求调用确实解析到了 JDK 里的那个方法。

use base_db::Db;
use hir::{class_data, InferenceResult, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange,
};

use crate::{redundancy::RedundancyFix, Diagnostic, DiagnosticCode, Severity};

/// 语法上像是候选的调用：`isPresent` / `isEmpty` / `forEach` / `stream` / `map` / `sort` /
/// `indexOf` / `valueOf`。
pub(crate) fn is_candidate(call: &ast::MethodCall) -> bool {
    call.name_ref().is_some_and(|n| {
        matches!(
            n.text().as_str(),
            "isPresent" | "isEmpty" | "forEach" | "stream" | "map" | "sort" | "indexOf" | "valueOf"
        )
    })
}

/// 文件里可能有候选（便宜的文本预检）。
pub(crate) fn may_have_candidates(text: &str) -> bool {
    text.contains(".stream()")
        || text.contains("Collections.sort")
        || text.contains("indexOf(")
        || text.contains("String.valueOf")
        || text.contains(".map(")
}

fn name(call: &ast::MethodCall) -> String {
    call.name_ref()
        .map(|n| n.text().to_string())
        .unwrap_or_default()
}

fn args(call: &ast::MethodCall) -> Vec<ast::Expr> {
    call.arg_list()
        .map(|l| l.args().collect())
        .unwrap_or_default()
}

fn text_of(node: &impl AstNode) -> String {
    node.syntax().text().to_string()
}

/// 调用解析到的方法所在类的二进制名。
fn declaring_class(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
) -> Option<String> {
    let arg_list = call.arg_list()?;
    let method = result.call_info(arg_list.syntax().text_range())?.chosen?;
    class_data(db, method.class).map(|d| d.binary_name.to_string())
}

fn class_name(db: &dyn Db, ty: Option<&Ty>) -> Option<String> {
    match ty? {
        Ty::Class(class, _) => class_data(db, *class).map(|d| d.binary_name.to_string()),
        _ => None,
    }
}

fn is_subtype_of(db: &dyn Db, ty: Option<&Ty>, binary_name: &str) -> bool {
    let (Some(ty @ Ty::Class(..)), Some(target)) = (ty, hir::lookup_class(db, binary_name)) else {
        return false;
    };
    hir::is_subtype(db, ty, &Ty::class(target))
}

/// 从 `.name(...)` 的点号到调用末尾。
fn suffix_range(call: &ast::MethodCall) -> Option<TextRange> {
    let name = call.name_ref()?;
    let dot = name
        .syntax()
        .first_token()?
        .prev_token()
        .filter(|t| t.kind() == SyntaxKind::DOT)?;
    Some(TextRange::new(
        dot.text_range().start(),
        call.syntax().text_range().end(),
    ))
}

fn receiver_call(call: &ast::MethodCall) -> Option<ast::MethodCall> {
    match call.receiver()? {
        ast::Expr::MethodCall(inner) => Some(inner),
        _ => None,
    }
}

fn fix(
    range: TextRange,
    code: DiagnosticCode,
    message: String,
    label: &'static str,
    edits: Vec<(TextRange, String)>,
) -> RedundancyFix {
    RedundancyFix {
        diagnostic: Diagnostic {
            range,
            message,
            severity: Severity::Hint,
            code,
            unused: false,
        },
        label,
        edits,
    }
}

pub(crate) fn check(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    match name(call).as_str() {
        "isPresent" | "isEmpty" => any_match(db, result, call, out),
        "forEach" => stream_for_each(db, result, call, out),
        "stream" => arrays_stream(db, result, call, out),
        "map" => identity_map(db, result, call, out),
        "sort" => collections_sort(db, result, call, out),
        "indexOf" => index_of(db, result, call, out),
        "valueOf" => string_value_of(db, result, call, out),
        _ => None,
    }
}

/// `.filter(p).findFirst().isPresent()` → `.anyMatch(p)`
fn any_match(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if !args(call).is_empty() || declaring_class(db, result, call)? != "java/util/Optional" {
        return None;
    }
    let find = receiver_call(call)?;
    if !matches!(name(&find).as_str(), "findFirst" | "findAny") || !args(&find).is_empty() {
        return None;
    }
    let filter = receiver_call(&find)?;
    if name(&filter) != "filter"
        || declaring_class(db, result, &filter)? != "java/util/stream/Stream"
    {
        return None;
    }
    let [predicate] = args(&filter).try_into().ok()?;
    let replacement = if name(call) == "isPresent" {
        "anyMatch"
    } else {
        "noneMatch"
    };
    let start = suffix_range(&filter)?.start();
    let range = TextRange::new(start, call.syntax().text_range().end());
    out.push(fix(
        call.name_ref()?.syntax().text_range(),
        DiagnosticCode::StreamApiChain,
        base_db::tr!(
            "可以换成 `{replacement}()`",
            "Can be replaced with `{replacement}()`"
        ),
        if replacement == "anyMatch" {
            base_db::trs!("换成 `anyMatch()`", "Replace with `anyMatch()`")
        } else {
            base_db::trs!("换成 `noneMatch()`", "Replace with `noneMatch()`")
        },
        vec![(range, format!(".{replacement}({})", text_of(&predicate)))],
    ));
    Some(())
}

/// `coll.stream().forEach(f)` → `coll.forEach(f)`
fn stream_for_each(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if declaring_class(db, result, call)? != "java/util/stream/Stream" {
        return None;
    }
    let stream = receiver_call(call)?;
    if name(&stream) != "stream" || !args(&stream).is_empty() {
        return None;
    }
    let collection = stream.receiver()?;
    if !is_subtype_of(
        db,
        result.type_of(collection.syntax().text_range()),
        "java/util/Collection",
    ) {
        return None;
    }
    let range = suffix_range(&stream)?;
    out.push(fix(
        stream.name_ref()?.syntax().text_range(),
        DiagnosticCode::StreamApiChain,
        base_db::trs!(
            "`stream().forEach()` 可以直接写成 `forEach()`",
            "`stream().forEach()` can be simplified to `forEach()`"
        )
        .to_owned(),
        base_db::trs!("去掉 `stream()`", "Remove `stream()`"),
        vec![(range, String::new())],
    ));
    Some(())
}

/// `Arrays.asList(array).stream()` → `Arrays.stream(array)`
fn arrays_stream(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if !args(call).is_empty() {
        return None;
    }
    let as_list = receiver_call(call)?;
    if name(&as_list) != "asList" || declaring_class(db, result, &as_list)? != "java/util/Arrays" {
        return None;
    }
    let [array] = args(&as_list).try_into().ok()?;
    if !matches!(
        result.type_of(array.syntax().text_range()),
        Some(Ty::Array(_))
    ) {
        return None;
    }
    let qualifier = as_list.receiver()?;
    out.push(fix(
        as_list.name_ref()?.syntax().text_range(),
        DiagnosticCode::StreamApiChain,
        base_db::trs!(
            "可以换成 `Arrays.stream()`",
            "Can be replaced with `Arrays.stream()`"
        )
        .to_owned(),
        base_db::trs!("换成 `Arrays.stream()`", "Replace with `Arrays.stream()`"),
        vec![(
            call.syntax().text_range(),
            format!("{}.stream({})", text_of(&qualifier), text_of(&array)),
        )],
    ));
    Some(())
}

/// `.map(x -> x)`：什么都没做
fn identity_map(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if declaring_class(db, result, call)? != "java/util/stream/Stream" {
        return None;
    }
    let [ast::Expr::LambdaExpr(lambda)] = <[ast::Expr; 1]>::try_from(args(call)).ok()? else {
        return None;
    };
    let params: Vec<String> = lambda
        .syntax()
        .descendants()
        .filter_map(ast::Param::cast)
        .filter_map(|p| p.name().map(|n| n.text().to_string()))
        .collect();
    let [param] = params.as_slice() else {
        return None;
    };
    let body = lambda
        .syntax()
        .children()
        .filter_map(ast::Expr::cast)
        .last()?;
    if !matches!(&body, ast::Expr::NameExpr(n) if n.syntax().text() == param.as_str()) {
        return None;
    }
    // 只有类型不变时才能删：`Stream<T>.map(x -> x)` 的结果还是 `Stream<T>`
    let receiver = call.receiver()?;
    if result.type_of(receiver.syntax().text_range()) != result.type_of(call.syntax().text_range())
    {
        return None;
    }
    out.push(fix(
        call.name_ref()?.syntax().text_range(),
        DiagnosticCode::StreamApiChain,
        base_db::trs!(
            "多余的 `map()`：映射到自身",
            "Redundant `map()`: maps to itself"
        )
        .to_owned(),
        base_db::trs!("删除 `map()`", "Remove `map()`"),
        vec![(suffix_range(call)?, String::new())],
    ));
    Some(())
}

/// `Collections.sort(list[, cmp])` → `list.sort(cmp | null)`
fn collections_sort(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if declaring_class(db, result, call)? != "java/util/Collections" {
        return None;
    }
    let list_args = args(call);
    let (list, comparator) = match list_args.as_slice() {
        [list] => (list, "null".to_owned()),
        [list, comparator] => (list, text_of(comparator)),
        _ => return None,
    };
    let list_text = match list {
        ast::Expr::NameExpr(_)
        | ast::Expr::FieldAccess(_)
        | ast::Expr::MethodCall(_)
        | ast::Expr::ThisExpr(_)
        | ast::Expr::ParenExpr(_) => text_of(list),
        _ => format!("({})", text_of(list)),
    };
    out.push(fix(
        call.name_ref()?.syntax().text_range(),
        DiagnosticCode::CollectionsSort,
        base_db::trs!(
            "`Collections.sort()` 可以换成 `List.sort()`",
            "`Collections.sort()` can be replaced with `List.sort()`"
        )
        .to_owned(),
        base_db::trs!("换成 `List.sort()`", "Replace with `List.sort()`"),
        vec![(
            call.syntax().text_range(),
            format!("{list_text}.sort({comparator})"),
        )],
    ));
    Some(())
}

fn int_literal(expr: &ast::Expr) -> Option<i64> {
    match expr {
        ast::Expr::Literal(lit) => lit.syntax().text().to_string().parse().ok(),
        ast::Expr::UnaryExpr(unary) => {
            let text = unary.syntax().text().to_string().replace(' ', "");
            text.parse().ok()
        }
        ast::Expr::ParenExpr(paren) => {
            int_literal(&paren.syntax().children().find_map(ast::Expr::cast)?)
        }
        _ => None,
    }
}

/// `s.indexOf(x) != -1` → `s.contains(x)`
fn index_of(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    let [arg] = args(call).try_into().ok()?;
    let receiver = call.receiver()?;
    let receiver_ty = result.type_of(receiver.syntax().text_range());
    let ok = match class_name(db, receiver_ty)?.as_str() {
        "java/lang/String" => {
            class_name(db, result.type_of(arg.syntax().text_range())).as_deref()
                == Some("java/lang/String")
        }
        _ => is_subtype_of(db, receiver_ty, "java/util/List"),
    };
    if !ok {
        return None;
    }
    let binary = ast::BinaryExpr::cast(call.syntax().parent()?)?;
    let op = binary.op_token()?.text().to_string();
    let other = [binary.lhs(), binary.rhs()]
        .into_iter()
        .flatten()
        .find(|e| e.syntax() != call.syntax())?;
    let constant = int_literal(&other)?;
    let call_on_left = binary.lhs().is_some_and(|l| l.syntax() == call.syntax());
    // 规范成 `indexOf(x) <op> constant`
    let op = if call_on_left {
        op
    } else {
        match op.as_str() {
            "<" => ">".into(),
            ">" => "<".into(),
            "<=" => ">=".into(),
            ">=" => "<=".into(),
            other => other.into(),
        }
    };
    let contains = match (op.as_str(), constant) {
        ("!=", -1) | (">=", 0) | (">", -1) => true,
        ("==", -1) | ("<", 0) | ("<=", -1) => false,
        _ => return None,
    };
    let replacement = format!(
        "{}{}.contains({})",
        if contains { "" } else { "!" },
        text_of(&receiver),
        text_of(&arg)
    );
    out.push(fix(
        binary.syntax().text_range(),
        DiagnosticCode::IndexOfContains,
        base_db::trs!(
            "`indexOf()` 的判断可以换成 `contains()`",
            "`indexOf()` check can be replaced with `contains()`"
        )
        .to_owned(),
        base_db::trs!("换成 `contains()`", "Replace with `contains()`"),
        vec![(binary.syntax().text_range(), replacement)],
    ));
    Some(())
}

fn is_string_ty(db: &dyn Db, ty: Option<&Ty>) -> bool {
    class_name(db, ty).as_deref() == Some("java/lang/String")
}

/// `"a" + String.valueOf(x)` → `"a" + x`
fn string_value_of(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if declaring_class(db, result, call)? != "java/lang/String" {
        return None;
    }
    let [arg] = args(call).try_into().ok()?;
    let arg_ty = result.type_of(arg.syntax().text_range())?;
    // `String.valueOf(char[])` 拼的是内容，直接拼数组是它的地址
    if matches!(arg_ty, Ty::Array(_)) || arg_ty.contains_unknown() {
        return None;
    }
    let binary = ast::BinaryExpr::cast(call.syntax().parent()?)?;
    if binary.op_token()?.kind() != SyntaxKind::PLUS {
        return None;
    }
    let other = [binary.lhs(), binary.rhs()]
        .into_iter()
        .flatten()
        .find(|e| e.syntax() != call.syntax())?;
    // 另一边是字符串时去掉才不改变含义；`1 + String.valueOf(2)` 去掉就成了加法
    let call_on_left = binary.lhs().is_some_and(|l| l.syntax() == call.syntax());
    let other_is_string = is_string_ty(db, result.type_of(other.syntax().text_range()));
    if !other_is_string {
        return None;
    }
    // 在左边时，`String.valueOf(a) + b` 去掉后 `a + b` 可能变成数值加法（b 是字符串时不会）
    if call_on_left && !is_string_ty(db, result.type_of(binary.rhs()?.syntax().text_range())) {
        return None;
    }
    let needs_parens = matches!(
        arg,
        ast::Expr::BinaryExpr(_)
            | ast::Expr::ConditionalExpr(_)
            | ast::Expr::AssignExpr(_)
            | ast::Expr::LambdaExpr(_)
            | ast::Expr::InstanceofExpr(_)
            | ast::Expr::CastExpr(_)
    );
    let text = if needs_parens {
        format!("({})", text_of(&arg))
    } else {
        text_of(&arg)
    };
    out.push(fix(
        call.syntax().text_range(),
        DiagnosticCode::RedundantValueOf,
        base_db::trs!(
            "字符串拼接里多余的 `String.valueOf()`",
            "Unnecessary `String.valueOf()` in string concatenation"
        )
        .to_owned(),
        base_db::trs!("去掉 `String.valueOf()`", "Remove `String.valueOf()`"),
        vec![(call.syntax().text_range(), text)],
    ));
    Some(())
}
