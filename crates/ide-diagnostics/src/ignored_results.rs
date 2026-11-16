//! 返回值被忽略的调用：不可变类型上的方法只会返回新值，单独写成一条语句什么也不做
//! （`s.trim();`、`d.plusDays(1);`、`bd.setScale(2);`），只写了中间操作的 stream 也不会执行。

use base_db::Db;
use hir::{class_header, InferenceResult, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

/// 所有返回值的方法都没有副作用的类。
const IMMUTABLE: &[&str] = &[
    "java/lang/String",
    "java/lang/Boolean",
    "java/lang/Byte",
    "java/lang/Character",
    "java/lang/Short",
    "java/lang/Integer",
    "java/lang/Long",
    "java/lang/Float",
    "java/lang/Double",
    "java/math/BigInteger",
    "java/math/BigDecimal",
    "java/time/Duration",
    "java/time/Instant",
    "java/time/LocalDate",
    "java/time/LocalDateTime",
    "java/time/LocalTime",
    "java/time/MonthDay",
    "java/time/OffsetDateTime",
    "java/time/OffsetTime",
    "java/time/Period",
    "java/time/Year",
    "java/time/YearMonth",
    "java/time/ZonedDateTime",
    "java/util/Optional",
    "java/util/OptionalInt",
    "java/util/OptionalLong",
    "java/util/OptionalDouble",
];

const STREAMS: &[&str] = &[
    "java/util/stream/BaseStream",
    "java/util/stream/Stream",
    "java/util/stream/IntStream",
    "java/util/stream/LongStream",
    "java/util/stream/DoubleStream",
];

/// 返回值没用、但常被拿来做校验（抛异常）的方法。
fn used_for_side_effect(class: &str, method: &str) -> bool {
    // `checkIndex(...)`、`requireXxx(...)` 这类靠抛异常做校验的方法
    if ["check", "require", "validate"]
        .iter()
        .any(|p| method.starts_with(p))
    {
        return true;
    }
    match class {
        c if c.starts_with("java/util/Optional") => {
            matches!(
                method,
                "get" | "orElseThrow" | "getAsInt" | "getAsLong" | "getAsDouble"
            )
        }
        // `Integer.parseInt(s);` 之类常用来校验格式
        "java/lang/Byte" | "java/lang/Short" | "java/lang/Integer" | "java/lang/Long"
        | "java/lang/Float" | "java/lang/Double" | "java/lang/Boolean" => {
            method.starts_with("parse") || method == "valueOf" || method == "decode"
        }
        "java/math/BigInteger" | "java/math/BigDecimal" => {
            // `xxxValueExact()` 溢出时抛异常，常用来做范围检查
            method.ends_with("ValueExact")
        }
        // 换算成毫秒溢出时抛异常
        "java/time/Duration" | "java/time/Instant" => {
            method.ends_with("Exact") || method == "toMillis" || method == "toEpochMilli"
        }
        _ => method == "intern" || method.starts_with("parse"),
    }
}

/// `x.length();` / `x.hashCode();` / `x.toString();` 常被单独写出来做空值检查。
fn null_check_idiom(method: &str) -> bool {
    matches!(method, "length" | "hashCode" | "toString" | "getClass")
}

fn is_stream(db: &dyn Db, ty: &Ty) -> bool {
    ty.as_class()
        .and_then(|c| class_header(db, c))
        .is_some_and(|d| STREAMS.contains(&&*d.binary_name))
}

fn check(db: &dyn Db, result: &InferenceResult, call: &ast::MethodCall) -> Option<Diagnostic> {
    let name = call.name_ref()?;
    let Resolution::Method(method) = result.resolution(name.syntax().text_range())? else {
        return None;
    };
    let data = class_header(db, method.class)?;
    let class: &str = &data.binary_name;
    let method_name = name.text().to_string();
    let sig = hir::method_sig(db, *method);
    // 有数组形参的方法可能把结果写进数组（`Character.toChars(cp, dst, i)`）
    if matches!(sig.ret, Ty::Void)
        || null_check_idiom(&method_name)
        || sig.params.iter().any(|p| matches!(p, Ty::Array(_)))
    {
        return None;
    }
    let flagged = if IMMUTABLE.contains(&class) {
        !used_for_side_effect(class, &method_name)
    } else if STREAMS.contains(&class) {
        // 中间操作：返回的还是 stream（`collect()`、`iterator()` 之类终端操作不算）；
        // `BaseStream` 上的 `parallel()` 等返回类型变量 `S`
        is_stream(db, &sig.ret)
            || (class == "java/util/stream/BaseStream" && matches!(sig.ret, Ty::TypeParam(_)))
    } else {
        false
    };
    if !flagged {
        return None;
    }
    let simple = data.name.to_string();
    Some(Diagnostic {
        range: name.syntax().text_range(),
        message: base_db::tr!(
            "`{simple}.{method_name}()` 的返回值被忽略了",
            "Result of `{simple}.{method_name}()` is ignored"
        ),
        severity: Severity::Warning,
        code: DiagnosticCode::IgnoredResult,
        unused: false,
    })
}

pub(crate) fn ignored_results(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    for body in &view.bodies {
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for stmt in body.node.descendants().filter_map(ast::ExprStmt::cast) {
            let Some(ast::Expr::MethodCall(call)) = stmt.expr() else {
                continue;
            };
            // switch 表达式里 `case X -> expr;` 的 expr 是分支的值
            if in_switch_expression_arm(&stmt) {
                continue;
            }
            let result = result.get_or_insert_with(|| body.result(view));
            out.extend(check(db, result, &call));
        }
    }
}

fn in_switch_expression_arm(stmt: &ast::ExprStmt) -> bool {
    let Some(case) = stmt
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::SWITCH_CASE)
    else {
        return false;
    };
    case.ancestors()
        .find(|n| matches!(n.kind(), SyntaxKind::SWITCH_EXPR | SyntaxKind::SWITCH_STMT))
        .is_some_and(|n| n.kind() == SyntaxKind::SWITCH_EXPR)
}
