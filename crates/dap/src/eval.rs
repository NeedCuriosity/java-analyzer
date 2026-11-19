//! 求值用的纯函数：把表达式文本解析成语法树，字面量、基本类型运算、类型转换和
//! `String.valueOf` 的格式，方法签名的拆分，以及断点的命中次数条件。
//!
//! 和目标 JVM 打交道的部分（变量、字段、方法调用）在 `evaluator.rs`。

use base_db::{tr, trs};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxToken,
};

use crate::jdwp::Value;

/// 把表达式包进一个字段的初始化里解析：表达式必须正好占满初始化部分，不能有语法错误。
pub fn parse(text: &str) -> Result<ast::Expr, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(trs!("表达式为空", "The expression is empty").to_owned());
    }
    const PREFIX: &str = "class __Eval { Object __eval =\n";
    let source = format!("{PREFIX}{trimmed}\n; }}");
    let parse = ast::CompilationUnit::parse(&source);
    if let Some(error) = parse.errors().first() {
        return Err(tr!(
            "表达式有语法错误：{}",
            "Syntax error in the expression: {}",
            base_db::syntax_error_message(&error.message, base_db::lang())
        ));
    }
    let start = PREFIX.len();
    let end = start + trimmed.len();
    parse
        .syntax_node()
        .descendants()
        .filter_map(ast::VariableDeclarator::cast)
        .find_map(|declarator| declarator.initializer())
        .filter(|expr| {
            let range = expr.syntax().text_range();
            usize::from(range.start()) == start && usize::from(range.end()) == end
        })
        .ok_or_else(|| {
            tr!(
                "`{trimmed}` 不是一个表达式",
                "`{trimmed}` is not an expression"
            )
        })
}

/// 字面量的值。字符串在调试器这边先留着，用到时再放进目标 JVM。
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Value(Value),
    Str(String),
    Null,
}

pub fn literal(token: &SyntaxToken) -> Result<Lit, String> {
    let text = token.text();
    let bad = || tr!("无法识别的字面量 `{text}`", "Unrecognized literal `{text}`");
    Ok(match token.kind() {
        SyntaxKind::TRUE_KW => Lit::Value(Value::Boolean(true)),
        SyntaxKind::FALSE_KW => Lit::Value(Value::Boolean(false)),
        SyntaxKind::NULL_KW => Lit::Null,
        SyntaxKind::INT_LITERAL => Lit::Value(Value::Int(
            parse_integer(text, false).ok_or_else(bad)? as u32 as i32,
        )),
        SyntaxKind::LONG_LITERAL => Lit::Value(Value::Long(
            parse_integer(text.trim_end_matches(['l', 'L']), true).ok_or_else(bad)? as i64,
        )),
        SyntaxKind::FLOAT_LITERAL => Lit::Value(Value::Float(
            parse_floating(text.trim_end_matches(['f', 'F'])).ok_or_else(bad)? as f32,
        )),
        SyntaxKind::DOUBLE_LITERAL => Lit::Value(Value::Double(
            parse_floating(text.trim_end_matches(['d', 'D'])).ok_or_else(bad)?,
        )),
        SyntaxKind::CHAR_LITERAL => {
            let inner = text
                .strip_prefix('\'')
                .and_then(|t| t.strip_suffix('\''))
                .ok_or_else(bad)?;
            let units: Vec<u16> = unescape(inner).ok_or_else(bad)?.encode_utf16().collect();
            match units.as_slice() {
                [unit] => Lit::Value(Value::Char(*unit)),
                _ => return Err(bad()),
            }
        }
        SyntaxKind::STRING_LITERAL => {
            let inner = text
                .strip_prefix('"')
                .and_then(|t| t.strip_suffix('"'))
                .ok_or_else(bad)?;
            Lit::Str(unescape(inner).ok_or_else(bad)?)
        }
        SyntaxKind::TEXT_BLOCK => Lit::Str(text_block(text).ok_or_else(bad)?),
        _ => return Err(bad()),
    })
}

/// 整数字面量（十进制、`0x`、`0b`、八进制）。十六进制 / 八进制 / 二进制可以占满全部位。
fn parse_integer(text: &str, long: bool) -> Option<u64> {
    let text = text.replace('_', "");
    let (digits, radix) = if let Some(hex) = text.strip_prefix("0x").or(text.strip_prefix("0X")) {
        (hex.to_owned(), 16)
    } else if let Some(bin) = text.strip_prefix("0b").or(text.strip_prefix("0B")) {
        (bin.to_owned(), 2)
    } else if text.len() > 1 && text.starts_with('0') {
        (text[1..].to_owned(), 8)
    } else {
        (text.clone(), 10)
    };
    let value = u64::from_str_radix(&digits, radix).ok()?;
    let limit: u64 = match (long, radix) {
        (false, 10) => 1 << 31,
        (true, 10) => 1 << 63,
        (false, _) => u64::from(u32::MAX),
        (true, _) => u64::MAX,
    };
    (value <= limit).then_some(value)
}

fn parse_floating(text: &str) -> Option<f64> {
    text.replace('_', "").parse().ok()
}

/// 字符 / 字符串字面量里的转义。
fn unescape(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            's' => out.push(' '),
            '\n' => {}
            c @ '0'..='7' => {
                // 八进制转义：最多三位，值不超过 0o377
                let mut value = c.to_digit(8)?;
                for _ in 0..2 {
                    match chars.peek().and_then(|c| c.to_digit(8)) {
                        Some(d) if value * 8 + d <= 0o377 => {
                            value = value * 8 + d;
                            chars.next();
                        }
                        _ => break,
                    }
                }
                out.push(char::from_u32(value)?);
            }
            'u' => {
                while chars.peek() == Some(&'u') {
                    chars.next();
                }
                let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                let unit = u16::from_str_radix(&hex, 16).ok()?;
                out.push(char::from_u32(u32::from(unit))?);
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// 文本块：去掉开头的 `"""` 和换行、结尾的 `"""`，按最小缩进去掉每行的公共前导空白。
fn text_block(text: &str) -> Option<String> {
    let body = text.strip_prefix("\"\"\"")?.strip_suffix("\"\"\"")?;
    let body = body.split_once('\n')?.1;
    let lines: Vec<&str> = body.split('\n').collect();
    let indent = lines
        .iter()
        .enumerate()
        .filter(|(i, line)| !line.trim().is_empty() || *i == lines.len() - 1)
        .map(|(_, line)| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let stripped: Vec<&str> = lines
        .iter()
        .map(|line| line.get(indent..).unwrap_or("").trim_end())
        .collect();
    unescape(&stripped.join("\n"))
}

// ---------------------------------------------------------------
// 基本类型
// ---------------------------------------------------------------

/// 基本类型的签名字符（`I`、`J`……）；对象返回 `None`。
pub fn primitive_tag(value: &Value) -> Option<u8> {
    match value {
        Value::Object { .. } | Value::Void => None,
        other => Some(other.tag()),
    }
}

/// 二元数值提升后的类型。
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Numeric {
    Int,
    Long,
    Float,
    Double,
}

fn numeric_kind(value: &Value) -> Option<Numeric> {
    match value {
        Value::Byte(_) | Value::Short(_) | Value::Char(_) | Value::Int(_) => Some(Numeric::Int),
        Value::Long(_) => Some(Numeric::Long),
        Value::Float(_) => Some(Numeric::Float),
        Value::Double(_) => Some(Numeric::Double),
        _ => None,
    }
}

fn as_i64(value: &Value) -> Option<i64> {
    Some(match *value {
        Value::Byte(v) => i64::from(v),
        Value::Short(v) => i64::from(v),
        Value::Char(v) => i64::from(v),
        Value::Int(v) => i64::from(v),
        Value::Long(v) => v,
        _ => return None,
    })
}

fn as_f64(value: &Value) -> Option<f64> {
    Some(match *value {
        Value::Float(v) => f64::from(v),
        Value::Double(v) => v,
        ref other => as_i64(other)? as f64,
    })
}

/// 一元运算：`- + ~ !`。
pub fn unary(op: SyntaxKind, operand: Value) -> Result<Value, String> {
    let bad = || {
        tr!(
            "运算符 `{}` 不能用在 {operand:?} 上",
            "Operator `{}` cannot be applied to {operand:?}",
            op_text(op)
        )
    };
    Ok(match (op, operand) {
        (SyntaxKind::BANG, Value::Boolean(b)) => Value::Boolean(!b),
        (SyntaxKind::PLUS, v) => match numeric_kind(&v).ok_or_else(bad)? {
            Numeric::Int => Value::Int(as_i64(&v).unwrap_or(0) as i32),
            _ => v,
        },
        (SyntaxKind::MINUS, v) => match v {
            Value::Long(x) => Value::Long(x.wrapping_neg()),
            Value::Float(x) => Value::Float(-x),
            Value::Double(x) => Value::Double(-x),
            v => Value::Int((as_i64(&v).ok_or_else(bad)? as i32).wrapping_neg()),
        },
        (SyntaxKind::TILDE, v) => match v {
            Value::Long(x) => Value::Long(!x),
            v if numeric_kind(&v) == Some(Numeric::Int) => {
                Value::Int(!(as_i64(&v).unwrap_or(0) as i32))
            }
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    })
}

pub fn op_text(op: SyntaxKind) -> &'static str {
    use SyntaxKind::*;
    match op {
        PLUS => "+",
        MINUS => "-",
        STAR => "*",
        SLASH => "/",
        PERCENT => "%",
        AMP => "&",
        PIPE => "|",
        CARET => "^",
        SHL => "<<",
        SHR => ">>",
        USHR => ">>>",
        LT => "<",
        GT => ">",
        LT_EQ => "<=",
        GT_EQ => ">=",
        EQ_EQ => "==",
        NEQ => "!=",
        AMP_AMP => "&&",
        PIPE_PIPE => "||",
        BANG => "!",
        TILDE => "~",
        _ => "?",
    }
}

/// 两个基本类型值的二元运算（字符串连接、引用比较和短路运算由调用方处理）。
pub fn binary(op: SyntaxKind, lhs: Value, rhs: Value) -> Result<Value, String> {
    use SyntaxKind::*;
    let bad = || {
        tr!(
            "运算符 `{}` 不能用在 {lhs:?} 和 {rhs:?} 上",
            "Operator `{}` cannot be applied to {lhs:?} and {rhs:?}",
            op_text(op)
        )
    };
    if let (Value::Boolean(a), Value::Boolean(b)) = (lhs, rhs) {
        return Ok(Value::Boolean(match op {
            AMP | AMP_AMP => a & b,
            PIPE | PIPE_PIPE => a | b,
            CARET | NEQ => a != b,
            EQ_EQ => a == b,
            _ => return Err(bad()),
        }));
    }
    // 移位：结果类型只看左操作数
    if matches!(op, SHL | SHR | USHR) {
        let distance = as_i64(&rhs).ok_or_else(bad)?;
        return match numeric_kind(&lhs).ok_or_else(bad)? {
            Numeric::Int => {
                let v = as_i64(&lhs).unwrap_or(0) as i32;
                let d = (distance & 31) as u32;
                Ok(Value::Int(match op {
                    SHL => v.wrapping_shl(d),
                    SHR => v.wrapping_shr(d),
                    _ => ((v as u32) >> d) as i32,
                }))
            }
            Numeric::Long => {
                let v = as_i64(&lhs).unwrap_or(0);
                let d = (distance & 63) as u32;
                Ok(Value::Long(match op {
                    SHL => v.wrapping_shl(d),
                    SHR => v.wrapping_shr(d),
                    _ => ((v as u64) >> d) as i64,
                }))
            }
            _ => Err(bad()),
        };
    }
    let kind = numeric_kind(&lhs)
        .ok_or_else(bad)?
        .max(numeric_kind(&rhs).ok_or_else(bad)?);
    match kind {
        Numeric::Int | Numeric::Long => {
            let (a, b) = (as_i64(&lhs).unwrap_or(0), as_i64(&rhs).unwrap_or(0));
            let int = kind == Numeric::Int;
            let wrap = |v: i64| {
                if int {
                    Value::Int(v as i32)
                } else {
                    Value::Long(v)
                }
            };
            let (a32, b32) = (a as i32, b as i32);
            Ok(match op {
                PLUS if int => wrap(i64::from(a32.wrapping_add(b32))),
                PLUS => wrap(a.wrapping_add(b)),
                MINUS if int => wrap(i64::from(a32.wrapping_sub(b32))),
                MINUS => wrap(a.wrapping_sub(b)),
                STAR if int => wrap(i64::from(a32.wrapping_mul(b32))),
                STAR => wrap(a.wrapping_mul(b)),
                SLASH | PERCENT if b == 0 => {
                    return Err("java.lang.ArithmeticException: / by zero".to_owned())
                }
                SLASH if int => wrap(i64::from(a32.wrapping_div(b32))),
                SLASH => wrap(a.wrapping_div(b)),
                PERCENT if int => wrap(i64::from(a32.wrapping_rem(b32))),
                PERCENT => wrap(a.wrapping_rem(b)),
                AMP => wrap(a & b),
                PIPE => wrap(a | b),
                CARET => wrap(a ^ b),
                LT => Value::Boolean(a < b),
                GT => Value::Boolean(a > b),
                LT_EQ => Value::Boolean(a <= b),
                GT_EQ => Value::Boolean(a >= b),
                EQ_EQ => Value::Boolean(a == b),
                NEQ => Value::Boolean(a != b),
                _ => return Err(bad()),
            })
        }
        Numeric::Float | Numeric::Double => {
            let (a, b) = (as_f64(&lhs).unwrap_or(0.0), as_f64(&rhs).unwrap_or(0.0));
            let float = kind == Numeric::Float;
            let (a, b) = if float {
                (f64::from(a as f32), f64::from(b as f32))
            } else {
                (a, b)
            };
            let wrap = |v: f64| {
                if float {
                    Value::Float(v as f32)
                } else {
                    Value::Double(v)
                }
            };
            Ok(match op {
                PLUS => wrap(a + b),
                MINUS => wrap(a - b),
                STAR => wrap(a * b),
                SLASH => wrap(a / b),
                PERCENT => wrap(a % b),
                LT => Value::Boolean(a < b),
                GT => Value::Boolean(a > b),
                LT_EQ => Value::Boolean(a <= b),
                GT_EQ => Value::Boolean(a >= b),
                EQ_EQ => Value::Boolean(a == b),
                NEQ => Value::Boolean(a != b),
                _ => return Err(bad()),
            })
        }
    }
}

/// 基本类型之间的转换（强制转换和方法实参的拓宽），`target` 是签名字符。
pub fn convert_primitive(value: Value, target: u8) -> Result<Value, String> {
    if value.tag() == target {
        return Ok(value);
    }
    if target == b'Z' || matches!(value, Value::Boolean(_)) {
        return Err(tr!(
            "{value:?} 不能转换成 {}",
            "{value:?} cannot be converted to {}",
            crate::jdwp::signature_to_type_name(&(target as char).to_string())
        ));
    }
    let float = matches!(value, Value::Float(_) | Value::Double(_));
    let f = as_f64(&value).ok_or_else(|| tr!("{value:?} 不是数值", "{value:?} is not a number"))?;
    let i = if float {
        // Java 的浮点转整数：NaN 是 0，超出范围时取边界值（`as` 的语义相同）
        f as i64
    } else {
        as_i64(&value).unwrap_or(0)
    };
    let int_of = |bits: i64| -> i32 {
        if float {
            f as i32
        } else {
            bits as i32
        }
    };
    Ok(match target {
        b'B' => Value::Byte(int_of(i) as i8),
        b'S' => Value::Short(int_of(i) as i16),
        b'C' => Value::Char(int_of(i) as u16),
        b'I' => Value::Int(int_of(i)),
        b'J' => Value::Long(i),
        b'F' => Value::Float(if float { f as f32 } else { i as f32 }),
        b'D' => Value::Double(if float { f } else { i as f64 }),
        _ => {
            return Err(tr!(
                "未知的基本类型 `{}`",
                "Unknown primitive type `{}`",
                target as char
            ))
        }
    })
}

/// 实参的基本类型能否不经装箱拓宽成形参的基本类型。
pub fn widens_to(from: u8, to: u8) -> bool {
    if from == to {
        return true;
    }
    let targets: &[u8] = match from {
        b'B' => b"SIJFD",
        b'S' => b"IJFD",
        b'C' => b"IJFD",
        b'I' => b"JFD",
        b'J' => b"FD",
        b'F' => b"D",
        _ => b"",
    };
    targets.contains(&to)
}

/// 基本类型对应的包装类签名。
pub fn box_signature(tag: u8) -> Option<&'static str> {
    Some(match tag {
        b'Z' => "Ljava/lang/Boolean;",
        b'B' => "Ljava/lang/Byte;",
        b'C' => "Ljava/lang/Character;",
        b'S' => "Ljava/lang/Short;",
        b'I' => "Ljava/lang/Integer;",
        b'J' => "Ljava/lang/Long;",
        b'F' => "Ljava/lang/Float;",
        b'D' => "Ljava/lang/Double;",
        _ => return None,
    })
}

/// 包装类签名对应的基本类型。
pub fn unboxed_tag(signature: &str) -> Option<u8> {
    (b"ZBCSIJFD")
        .iter()
        .copied()
        .find(|&tag| box_signature(tag) == Some(signature))
}

/// `String.valueOf` 对基本类型的格式。
pub fn primitive_to_string(value: &Value) -> String {
    match *value {
        Value::Boolean(v) => v.to_string(),
        Value::Char(v) => char::decode_utf16([v])
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect(),
        Value::Float(v) => java_floating(f64::from(v), format!("{v:?}"), format!("{v:e}")),
        Value::Double(v) => java_floating(v, format!("{v:?}"), format!("{v:e}")),
        Value::Void => "void".to_owned(),
        ref other => as_i64(other).map(|v| v.to_string()).unwrap_or_default(),
    }
}

/// `Double.toString` 的格式：`1.0`、`0.001`、`1.0E7`、`1.0E-4`、`NaN`、`Infinity`。
fn java_floating(v: f64, plain: String, sci: String) -> String {
    if v.is_nan() {
        return "NaN".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let abs = v.abs();
    if (abs == 0.0 || (1e-3..1e7).contains(&abs)) && !plain.contains('e') {
        return plain;
    }
    let Some((mantissa, exp)) = sci.split_once('e') else {
        return sci;
    };
    let mantissa = if mantissa.contains('.') {
        mantissa.to_owned()
    } else {
        format!("{mantissa}.0")
    };
    format!("{mantissa}E{exp}")
}

// ---------------------------------------------------------------
// 签名
// ---------------------------------------------------------------

/// 方法签名 `(ILjava/lang/String;[J)V` 的形参签名和返回类型签名。
pub fn split_method_signature(signature: &str) -> Option<(Vec<String>, String)> {
    let rest = signature.strip_prefix('(')?;
    let (params, ret) = rest.split_once(')')?;
    let mut out = Vec::new();
    let bytes = params.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        while bytes.get(i) == Some(&b'[') {
            i += 1;
        }
        match bytes.get(i)? {
            b'L' => {
                i += params[i..].find(';')? + 1;
            }
            _ => i += 1,
        }
        out.push(params[start..i].to_owned());
    }
    Some((out, ret.to_owned()))
}

/// 类型名转成签名：`int` → `I`，`java.lang.String` → `Ljava/lang/String;`（`$` 保持原样）。
pub fn type_name_to_signature(name: &str) -> String {
    let dims = name.matches("[]").count();
    let base = name.trim_end_matches("[]");
    let base_sig = match base {
        "boolean" => "Z".to_owned(),
        "byte" => "B".to_owned(),
        "char" => "C".to_owned(),
        "short" => "S".to_owned(),
        "int" => "I".to_owned(),
        "long" => "J".to_owned(),
        "float" => "F".to_owned(),
        "double" => "D".to_owned(),
        "void" => "V".to_owned(),
        other => format!("L{};", other.replace('.', "/")),
    };
    format!("{}{base_sig}", "[".repeat(dims))
}

/// 断点命中次数条件：`5`（第 5 次）、`>= 5`、`> 5`、`< 5`、`<= 5`、`== 5`、`% 5`（每 5 次）。
pub fn hit_condition_met(condition: &str, hits: u64) -> Result<bool, String> {
    let condition = condition.trim();
    let (op, number) = ["%", ">=", "<=", "==", ">", "<", "="]
        .iter()
        .find_map(|op| condition.strip_prefix(op).map(|rest| (*op, rest)))
        .unwrap_or(("==", condition));
    let n: u64 = number.trim().parse().map_err(|_| {
        tr!(
            "无法识别的命中次数条件 `{condition}`",
            "Unrecognized hit condition `{condition}`"
        )
    })?;
    Ok(match op {
        "%" => n != 0 && hits.is_multiple_of(n),
        ">=" => hits >= n,
        "<=" => hits <= n,
        ">" => hits > n,
        "<" => hits < n,
        _ => hits == n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(text: &str) -> String {
        format!("{:?}", parse(text).unwrap().syntax().kind())
    }

    #[test]
    fn parses_java_expressions() {
        assert_eq!(kind("x"), "NAME_EXPR");
        assert_eq!(kind("list.size()"), "METHOD_CALL");
        assert_eq!(kind("a + b * 2"), "BINARY_EXPR");
        assert_eq!(kind("  new java.util.ArrayList<String>()  "), "NEW_EXPR");
        assert_eq!(kind("c ? 1 : 2"), "CONDITIONAL_EXPR");
        assert_eq!(kind("(int) x"), "CAST_EXPR");
        assert_eq!(kind("o instanceof String"), "INSTANCEOF_EXPR");
        assert_eq!(kind("arr[i + 1]"), "ARRAY_ACCESS");
        assert_eq!(kind("x > -3"), "BINARY_EXPR");
        assert!(parse("").is_err());
        assert!(parse("a +").is_err());
        assert!(parse("a) + (b").is_err());
        assert!(parse("a; b").is_err());
    }

    fn lit(text: &str) -> Lit {
        let expr = parse(text).unwrap();
        let token = expr.syntax().first_token().expect("字面量至少有一个 token");
        literal(&token).unwrap()
    }

    #[test]
    fn literals() {
        assert_eq!(lit("10"), Lit::Value(Value::Int(10)));
        assert_eq!(lit("0x7fff_ffff"), Lit::Value(Value::Int(i32::MAX)));
        assert_eq!(lit("0xFFFFFFFF"), Lit::Value(Value::Int(-1)));
        assert_eq!(lit("10L"), Lit::Value(Value::Long(10)));
        assert_eq!(lit("1.5"), Lit::Value(Value::Double(1.5)));
        assert_eq!(lit("2f"), Lit::Value(Value::Float(2.0)));
        assert_eq!(lit("'a'"), Lit::Value(Value::Char('a' as u16)));
        assert_eq!(lit("'\\n'"), Lit::Value(Value::Char('\n' as u16)));
        assert_eq!(lit("\"a\\tb\\u0041\""), Lit::Str("a\tbA".into()));
        assert_eq!(lit("true"), Lit::Value(Value::Boolean(true)));
        assert_eq!(lit("null"), Lit::Null);
        assert_eq!(
            lit("\"\"\"\n    hello\n      world\n    \"\"\""),
            Lit::Str("hello\n  world\n".into())
        );
    }

    #[test]
    fn arithmetic() {
        use SyntaxKind::*;
        assert_eq!(
            binary(PLUS, Value::Int(2), Value::Int(3)).unwrap(),
            Value::Int(5)
        );
        assert_eq!(
            binary(STAR, Value::Int(i32::MAX), Value::Int(2)).unwrap(),
            Value::Int(-2)
        );
        assert_eq!(
            binary(PLUS, Value::Int(1), Value::Long(2)).unwrap(),
            Value::Long(3)
        );
        assert_eq!(
            binary(SLASH, Value::Int(7), Value::Double(2.0)).unwrap(),
            Value::Double(3.5)
        );
        assert_eq!(
            binary(PLUS, Value::Char('a' as u16), Value::Int(1)).unwrap(),
            Value::Int(98)
        );
        assert!(binary(SLASH, Value::Int(1), Value::Int(0))
            .unwrap_err()
            .contains("ArithmeticException"));
        assert_eq!(
            binary(USHR, Value::Int(-1), Value::Int(28)).unwrap(),
            Value::Int(15)
        );
        assert_eq!(
            binary(AMP, Value::Int(6), Value::Int(3)).unwrap(),
            Value::Int(2)
        );
        assert_eq!(
            binary(LT, Value::Int(2), Value::Float(2.5)).unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            binary(CARET, Value::Boolean(true), Value::Boolean(false)).unwrap(),
            Value::Boolean(true)
        );
        assert!(binary(PLUS, Value::Boolean(true), Value::Int(1)).is_err());
        assert_eq!(unary(MINUS, Value::Int(3)).unwrap(), Value::Int(-3));
        assert_eq!(unary(TILDE, Value::Int(0)).unwrap(), Value::Int(-1));
        assert_eq!(
            unary(BANG, Value::Boolean(false)).unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn conversions_and_formatting() {
        assert_eq!(
            convert_primitive(Value::Double(3.9), b'I').unwrap(),
            Value::Int(3)
        );
        assert_eq!(
            convert_primitive(Value::Int(65), b'C').unwrap(),
            Value::Char(65)
        );
        assert_eq!(
            convert_primitive(Value::Int(300), b'B').unwrap(),
            Value::Byte(44)
        );
        assert_eq!(
            convert_primitive(Value::Long(5), b'D').unwrap(),
            Value::Double(5.0)
        );
        assert!(convert_primitive(Value::Boolean(true), b'I').is_err());
        assert!(widens_to(b'I', b'J'));
        assert!(widens_to(b'C', b'I'));
        assert!(!widens_to(b'I', b'S'));
        assert_eq!(primitive_to_string(&Value::Double(1.0)), "1.0");
        assert_eq!(primitive_to_string(&Value::Double(1e10)), "1.0E10");
        assert_eq!(primitive_to_string(&Value::Float(0.5)), "0.5");
        assert_eq!(primitive_to_string(&Value::Char('x' as u16)), "x");
        assert_eq!(primitive_to_string(&Value::Long(-7)), "-7");
        assert_eq!(unboxed_tag("Ljava/lang/Integer;"), Some(b'I'));
        assert_eq!(box_signature(b'Z'), Some("Ljava/lang/Boolean;"));
    }

    #[test]
    fn signatures() {
        assert_eq!(
            split_method_signature("(ILjava/lang/String;[J[[Ljava/util/List;)V"),
            Some((
                vec![
                    "I".to_owned(),
                    "Ljava/lang/String;".to_owned(),
                    "[J".to_owned(),
                    "[[Ljava/util/List;".to_owned()
                ],
                "V".to_owned()
            ))
        );
        assert_eq!(
            split_method_signature("()Ljava/lang/Object;"),
            Some((vec![], "Ljava/lang/Object;".to_owned()))
        );
        assert_eq!(type_name_to_signature("int[]"), "[I");
        assert_eq!(
            type_name_to_signature("java.util.Map$Entry"),
            "Ljava/util/Map$Entry;"
        );
    }

    #[test]
    fn hit_conditions() {
        assert!(hit_condition_met("3", 3).unwrap());
        assert!(!hit_condition_met("3", 4).unwrap());
        assert!(hit_condition_met(">= 2", 5).unwrap());
        assert!(hit_condition_met("% 2", 4).unwrap());
        assert!(!hit_condition_met("%2", 3).unwrap());
        assert!(hit_condition_met("<3", 2).unwrap());
        assert!(hit_condition_met("x", 1).is_err());
    }
}
