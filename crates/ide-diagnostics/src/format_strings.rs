//! 格式字符串检查：`String.format`、`String.formatted`、`PrintStream` / `PrintWriter` 的
//! `printf` / `format`、`Formatter.format`。格式串是常量时检查：
//!
//! - 未知的转换符（运行时抛 `UnknownFormatConversionException`）；
//! - 实参个数和格式说明符要的个数不一致；
//! - 明显的类型不符：`%d` 配字符串 / 布尔 / 浮点，`%f` 配字符串 / 布尔 / 整数。

use base_db::Db;
use hir::{class_data, InferenceResult, PrimitiveType, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxNode,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

/// 一个格式说明符要的实参。
#[derive(Debug, PartialEq, Eq)]
struct Spec {
    /// 第几个实参（从 0 开始）
    index: usize,
    conversion: char,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Ok(Vec<Spec>),
    UnknownConversion(char),
    Malformed,
}

const CONVERSIONS: &str = "bBhHsScCdoxXeEfgGaAtT%n";

/// 解析格式串，得到每个说明符引用的实参。
fn parse_format(format: &str) -> Parsed {
    let chars: Vec<char> = format.chars().collect();
    let mut specs = Vec::new();
    let mut next_ordinary = 0usize;
    let mut last_index: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            i += 1;
            continue;
        }
        i += 1;
        // argument_index$ 或 `<`
        let mut explicit: Option<usize> = None;
        let mut relative = false;
        let digits_start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i < chars.len() && chars[i] == '$' && i > digits_start {
            let n: String = chars[digits_start..i].iter().collect();
            match n.parse::<usize>() {
                Ok(n) if n > 0 => explicit = Some(n - 1),
                _ => return Parsed::Malformed,
            }
            i += 1;
        } else {
            // 不是索引：数字是宽度（或 `0` 标志），退回去按标志 / 宽度解析
            i = digits_start;
        }
        // flags
        while i < chars.len() && "-#+ 0,(<".contains(chars[i]) {
            if chars[i] == '<' {
                relative = true;
            }
            i += 1;
        }
        // width
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        // precision
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i == start {
                return Parsed::Malformed;
            }
        }
        let Some(&conversion) = chars.get(i) else {
            return Parsed::Malformed;
        };
        i += 1;
        if !CONVERSIONS.contains(conversion) {
            return Parsed::UnknownConversion(conversion);
        }
        if conversion == 't' || conversion == 'T' {
            // 日期时间还要一个后缀字符
            if i >= chars.len() {
                return Parsed::Malformed;
            }
            i += 1;
        }
        if conversion == '%' || conversion == 'n' {
            continue;
        }
        let index = if relative {
            match last_index {
                Some(index) => index,
                None => return Parsed::Malformed,
            }
        } else if let Some(index) = explicit {
            index
        } else {
            next_ordinary += 1;
            next_ordinary - 1
        };
        last_index = Some(index);
        specs.push(Spec { index, conversion });
    }
    Parsed::Ok(specs)
}

/// 调用的是不是格式化方法；是的话返回格式串实参的位置（`None` 表示格式串是接收者）。
fn format_call(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
) -> Option<Option<usize>> {
    let name = call.name_ref()?;
    let Resolution::Method(method) = result.resolution(name.syntax().text_range())? else {
        return None;
    };
    let data = class_data(db, method.class)?;
    let method_data = data.methods.get(method.idx as usize)?;
    let class: &str = &data.binary_name;
    let sig = hir::method_sig(db, *method);
    let locale_first = sig
        .params
        .first()
        .and_then(Ty::as_class)
        .and_then(|c| class_data(db, c))
        .is_some_and(|d| &*d.binary_name == "java/util/Locale");
    let format_index = usize::from(locale_first);
    match (class, method_data.name.as_str()) {
        ("java/lang/String", "format") => Some(Some(format_index)),
        ("java/lang/String", "formatted") => Some(None),
        (
            "java/io/PrintStream" | "java/io/PrintWriter" | "java/util/Formatter",
            "printf" | "format",
        ) => Some(Some(format_index)),
        _ => None,
    }
}

fn is_class(db: &dyn Db, ty: &Ty, binary: &str) -> bool {
    ty.as_class()
        .and_then(|c| class_data(db, c))
        .is_some_and(|d| &*d.binary_name == binary)
}

/// 实参类型和转换符明显不符时返回说明。
fn mismatch(db: &dyn Db, conversion: char, ty: &Ty) -> bool {
    let unboxed = match ty {
        Ty::Primitive(p) => Some(*p),
        other => other.unboxed(db),
    };
    let is_string = is_class(db, ty, "java/lang/String");
    let integral = matches!(
        unboxed,
        Some(PrimitiveType::Byte | PrimitiveType::Short | PrimitiveType::Int | PrimitiveType::Long)
    ) || is_class(db, ty, "java/math/BigInteger");
    let floating = matches!(unboxed, Some(PrimitiveType::Float | PrimitiveType::Double))
        || is_class(db, ty, "java/math/BigDecimal");
    let boolean = unboxed == Some(PrimitiveType::Boolean);
    match conversion {
        'd' | 'o' | 'x' | 'X' => is_string || boolean || floating,
        'e' | 'E' | 'f' | 'g' | 'G' | 'a' | 'A' => is_string || boolean || integral,
        'c' | 'C' => is_string || boolean || floating,
        _ => false,
    }
}

fn warn(out: &mut Vec<Diagnostic>, node: &SyntaxNode, message: String) {
    out.push(Diagnostic {
        range: node.text_range(),
        message,
        severity: Severity::Warning,
        code: DiagnosticCode::MalformedFormatString,
        unused: false,
    });
}

pub(crate) fn format_string_diagnostics(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    if !view.file_contains("format") && !view.file_contains("printf") {
        return;
    }
    for body in &view.bodies {
        let result = body.result(view);
        for call in body.node.descendants().filter_map(ast::MethodCall::cast) {
            let Some(format_at) = format_call(db, &result, &call) else {
                continue;
            };
            let args: Vec<ast::Expr> = call
                .arg_list()
                .map(|l| l.args().collect())
                .unwrap_or_default();
            let (format_expr, rest) = match format_at {
                Some(index) => match args.get(index) {
                    Some(f) => (f.syntax().clone(), &args[index + 1..]),
                    None => continue,
                },
                None => match call.receiver() {
                    Some(r) => (r.syntax().clone(), &args[..]),
                    None => continue,
                },
            };
            let Some(hir::Constant::Str(format)) = hir::expr_constant(db, &result, &format_expr)
            else {
                continue;
            };
            let specs = match parse_format(&format) {
                Parsed::Ok(specs) => specs,
                Parsed::UnknownConversion(c) => {
                    warn(
                        out,
                        &format_expr,
                        base_db::tr!(
                            "格式串里有未知的转换符 `%{c}`",
                            "Unknown format conversion `%{c}`"
                        ),
                    );
                    continue;
                }
                Parsed::Malformed => {
                    warn(
                        out,
                        &format_expr,
                        base_db::trs!("格式串不合法", "Malformed format string").to_owned(),
                    );
                    continue;
                }
            };
            // 把数组直接当可变参数传进来时数不清实参个数
            if let [single] = rest {
                let ty = result.type_of(single.syntax().text_range());
                if matches!(ty, Some(Ty::Array(element)) if element.is_reference())
                    || ty == Some(&Ty::Null)
                {
                    continue;
                }
            }
            let needed = specs.iter().map(|s| s.index + 1).max().unwrap_or(0);
            if rest.len() < needed {
                let given = rest.len();
                warn(
                    out,
                    &format_expr,
                    base_db::tr!(
                        "格式串需要 {needed} 个实参，只给了 {given} 个",
                        "Format string requires {needed} arguments but {given} given"
                    ),
                );
                continue;
            }
            if rest.len() > needed {
                for extra in &rest[needed..] {
                    warn(
                        out,
                        extra.syntax(),
                        base_db::trs!(
                            "多余的实参：格式串里没有对应的说明符",
                            "Too many arguments for the format string"
                        )
                        .to_owned(),
                    );
                }
            }
            for spec in &specs {
                let Some(arg) = rest.get(spec.index) else {
                    continue;
                };
                let Some(ty) = result.type_of(arg.syntax().text_range()) else {
                    continue;
                };
                if mismatch(db, spec.conversion, ty) {
                    let conversion = spec.conversion;
                    let shown = ty.display(db);
                    warn(
                        out,
                        arg.syntax(),
                        base_db::tr!(
                            "`%{conversion}` 不能用于 `{shown}` 类型的实参",
                            "`%{conversion}` cannot format an argument of type `{shown}`"
                        ),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indices(format: &str) -> Vec<usize> {
        match parse_format(format) {
            Parsed::Ok(specs) => specs.into_iter().map(|s| s.index).collect(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_specifiers() {
        assert_eq!(indices("%s and %d%n100%%"), vec![0, 1]);
        assert_eq!(indices("%2$s %1$s %<s"), vec![1, 0, 0]);
        assert_eq!(indices("%-10.3f|%08X|%,d"), vec![0, 1, 2]);
        assert_eq!(indices("%tY-%<tm"), vec![0, 0]);
        assert_eq!(parse_format("%q"), Parsed::UnknownConversion('q'));
        assert_eq!(parse_format("50%"), Parsed::Malformed);
    }
}
