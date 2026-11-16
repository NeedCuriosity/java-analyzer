//! 常量表达式求值（JLS 15.29）：字面量、一元 / 二元运算、条件表达式、到基本类型和 `String`
//! 的强制转换，以及对其它常量变量（带常量初值的 `final` 字段和局部变量）的引用。
//!
//! 依赖库字段的常量值来自 class 文件的 `ConstantValue` 属性。

use base_db::Db;
use hir_def::{FieldConstant, FieldId, PrimitiveType, TypeRef};
use hir_ty::{body_syntax, field_data, infer, BodyId, InferenceResult, LocalKind, Resolution, Ty};

use crate::LocalDef;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    Bool(bool),
    Char(u16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Str(String),
}

/// 常量变量互相引用的最大深度（防止循环定义）。
const MAX_DEPTH: u32 = 16;

impl Constant {
    /// Java 源码写法：`42`、`42L`、`1.5f`、`'a'`、`"text"`。
    pub fn render(&self) -> String {
        match self {
            Constant::Bool(b) => b.to_string(),
            Constant::Char(c) => format!("'{}'", escape(&char_string(*c), '\'')),
            Constant::Int(v) => v.to_string(),
            Constant::Long(v) => format!("{v}L"),
            Constant::Float(v) => format!("{}f", java_float(*v)),
            Constant::Double(v) => java_double(*v),
            Constant::Str(s) => {
                let shown: String = s.chars().take(200).collect();
                let more = if s.chars().count() > 200 { "…" } else { "" };
                format!("\"{}\"{more}", escape(&shown, '"'))
            }
        }
    }

    /// 字符串连接时的字符串形式（`String.valueOf`）。
    fn to_java_string(&self) -> String {
        match self {
            Constant::Bool(b) => b.to_string(),
            Constant::Char(c) => char_string(*c),
            Constant::Int(v) => v.to_string(),
            Constant::Long(v) => v.to_string(),
            Constant::Float(v) => java_float(*v),
            Constant::Double(v) => java_double(*v),
            Constant::Str(s) => s.clone(),
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            Constant::Char(c) => Some(i64::from(*c)),
            Constant::Int(v) => Some(i64::from(*v)),
            Constant::Long(v) => Some(*v),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Constant::Float(v) => Some(f64::from(*v)),
            Constant::Double(v) => Some(*v),
            other => other.as_i64().map(|v| v as f64),
        }
    }

    /// 转换成某个基本类型（强制转换语义）。
    fn cast_to(&self, target: PrimitiveType) -> Option<Constant> {
        if target == PrimitiveType::Boolean {
            return matches!(self, Constant::Bool(_)).then(|| self.clone());
        }
        if matches!(self, Constant::Bool(_) | Constant::Str(_)) {
            return None;
        }
        let float_value = matches!(self, Constant::Float(_) | Constant::Double(_));
        let as_long = || -> i64 {
            if float_value {
                // Java 的浮点转整数：NaN 为 0，越界取边界值（Rust 的 `as` 语义相同）
                self.as_f64().unwrap_or(0.0) as i64
            } else {
                self.as_i64().unwrap_or(0)
            }
        };
        let as_int = || -> i32 {
            if float_value {
                self.as_f64().unwrap_or(0.0) as i32
            } else {
                as_long() as i32
            }
        };
        Some(match target {
            PrimitiveType::Byte => Constant::Int(i32::from(as_int() as i8)),
            PrimitiveType::Short => Constant::Int(i32::from(as_int() as i16)),
            PrimitiveType::Char => Constant::Char(as_int() as u16),
            PrimitiveType::Int => Constant::Int(as_int()),
            PrimitiveType::Long => Constant::Long(as_long()),
            PrimitiveType::Float => Constant::Float(match self {
                Constant::Double(v) => *v as f32,
                Constant::Float(v) => *v,
                other => other.as_i64()? as f32,
            }),
            PrimitiveType::Double => Constant::Double(self.as_f64()?),
            PrimitiveType::Boolean => return None,
        })
    }
}

fn char_string(c: u16) -> String {
    char::decode_utf16([c])
        .map(|r| {
            r.map(String::from)
                .unwrap_or_else(|_| format!("\\u{c:04x}"))
        })
        .collect()
}

fn escape(s: &str, quote: char) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// `Double.toString` 的格式：`1.0`、`0.001`、`1.0E7`、`1.0E-4`、`NaN`、`Infinity`。
fn java_double(v: f64) -> String {
    java_floating(v, format!("{v:?}"), format!("{v:e}"))
}

fn java_float(v: f32) -> String {
    java_floating(f64::from(v), format!("{v:?}"), format!("{v:e}"))
}

fn java_floating(v: f64, plain: String, sci: String) -> String {
    if v.is_nan() {
        return "NaN".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let abs = v.abs();
    if abs == 0.0 || (1e-3..1e7).contains(&abs) {
        // Rust 的 Debug 格式在这个范围里就是最短的十进制写法，整数也带 `.0`
        if plain.contains('e') {
            return sci_to_java(&sci);
        }
        return plain;
    }
    sci_to_java(&sci)
}

/// `1.5e7` → `1.5E7`，`1e7` → `1.0E7`。
fn sci_to_java(sci: &str) -> String {
    let Some((mantissa, exp)) = sci.split_once('e') else {
        return sci.to_owned();
    };
    let mantissa = if mantissa.contains('.') {
        mantissa.to_owned()
    } else {
        format!("{mantissa}.0")
    };
    format!("{mantissa}E{exp}")
}

/// 解析数值 / 字符 / 字符串字面量。
fn literal(token: &syntax::SyntaxToken) -> Option<Constant> {
    let text = token.text();
    match token.kind() {
        SyntaxKind::TRUE_KW => Some(Constant::Bool(true)),
        SyntaxKind::FALSE_KW => Some(Constant::Bool(false)),
        SyntaxKind::INT_LITERAL => parse_integer(text, false).map(|v| Constant::Int(v as i32)),
        SyntaxKind::LONG_LITERAL => {
            parse_integer(text.trim_end_matches(['l', 'L']), true).map(|v| Constant::Long(v as i64))
        }
        SyntaxKind::FLOAT_LITERAL => {
            parse_floating(text.trim_end_matches(['f', 'F'])).map(|v| Constant::Float(v as f32))
        }
        SyntaxKind::DOUBLE_LITERAL => {
            parse_floating(text.trim_end_matches(['d', 'D'])).map(Constant::Double)
        }
        SyntaxKind::CHAR_LITERAL => {
            let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
            let units: Vec<u16> = unescape(inner)?.encode_utf16().collect();
            match units.as_slice() {
                [unit] => Some(Constant::Char(*unit)),
                _ => None,
            }
        }
        SyntaxKind::STRING_LITERAL => {
            let inner = text.strip_prefix('"')?.strip_suffix('"')?;
            unescape(inner).map(Constant::Str)
        }
        _ => None,
    }
}

/// 整数字面量（十进制、`0x`、`0b`、八进制）。十六进制 / 八进制 / 二进制可以占满全部位，
/// 按无符号解析后截断。
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
    let text = text.replace('_', "");
    if text.starts_with("0x") || text.starts_with("0X") {
        // 十六进制浮点很少见，不求值
        return None;
    }
    text.parse().ok()
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
            '0'..='7' => return None,
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

struct Eval<'a> {
    db: &'a dyn Db,
    result: &'a InferenceResult,
    depth: u32,
    /// 引用到的字段的常量值走缓存的查询（不在常量字段的查询内部时）
    cached_fields: bool,
}

impl Eval<'_> {
    fn children(node: &SyntaxNode) -> Vec<SyntaxNode> {
        node.children()
            .filter(|c| ast::Expr::can_cast(c.kind()))
            .collect()
    }

    fn operator(node: &SyntaxNode) -> Option<SyntaxKind> {
        node.children_with_tokens()
            .filter_map(|e| e.into_token())
            .map(|t| t.kind())
            .find(|k| !k.is_trivia())
    }

    fn expr(&self, node: &SyntaxNode) -> Option<Constant> {
        match node.kind() {
            SyntaxKind::LITERAL => literal(&ast::Literal::cast(node.clone())?.token()?),
            SyntaxKind::PAREN_EXPR => self.expr(Self::children(node).first()?),
            SyntaxKind::NAME_EXPR | SyntaxKind::FIELD_ACCESS => {
                let name = node
                    .children()
                    .filter(|c| c.kind() == SyntaxKind::NAME_REF)
                    .last()?;
                match self.result.resolution(name.text_range())? {
                    Resolution::Field(field) if self.cached_fields => {
                        field_constant(self.db, *field)
                    }
                    Resolution::Field(field) => constant_of_field(self.db, *field, self.depth + 1),
                    _ => None,
                }
            }
            SyntaxKind::CAST_EXPR => {
                let operand = self.expr(Self::children(node).last()?)?;
                match self.result.type_of(node.text_range())? {
                    Ty::Primitive(p) => operand.cast_to(*p),
                    ty if is_string(self.db, ty) => {
                        matches!(operand, Constant::Str(_)).then_some(operand)
                    }
                    _ => None,
                }
            }
            SyntaxKind::UNARY_EXPR => {
                let operand = self.expr(Self::children(node).first()?)?;
                unary(Self::operator(node)?, operand)
            }
            SyntaxKind::BINARY_EXPR => {
                let op = ast::BinaryExpr::cast(node.clone())?.op_kind()?;
                let parts = Self::children(node);
                let [lhs, rhs] = parts.as_slice() else {
                    return None;
                };
                binary(op, self.expr(lhs)?, self.expr(rhs)?)
            }
            SyntaxKind::CONDITIONAL_EXPR => {
                let parts = Self::children(node);
                let [cond, then, otherwise] = parts.as_slice() else {
                    return None;
                };
                let (then, otherwise) = (self.expr(then)?, self.expr(otherwise)?);
                match self.expr(cond)? {
                    Constant::Bool(true) => Some(then),
                    Constant::Bool(false) => Some(otherwise),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

fn is_string(db: &dyn Db, ty: &Ty) -> bool {
    ty.as_class()
        .is_some_and(|c| c.binary_name(db) == "java/lang/String")
}

fn unary(op: SyntaxKind, operand: Constant) -> Option<Constant> {
    Some(match (op, operand) {
        (SyntaxKind::BANG, Constant::Bool(b)) => Constant::Bool(!b),
        (SyntaxKind::PLUS, c) => match c {
            Constant::Char(v) => Constant::Int(i32::from(v)),
            c @ (Constant::Int(_)
            | Constant::Long(_)
            | Constant::Float(_)
            | Constant::Double(_)) => c,
            _ => return None,
        },
        (SyntaxKind::MINUS, c) => match c {
            Constant::Char(v) => Constant::Int(-i32::from(v)),
            Constant::Int(v) => Constant::Int(v.wrapping_neg()),
            Constant::Long(v) => Constant::Long(v.wrapping_neg()),
            Constant::Float(v) => Constant::Float(-v),
            Constant::Double(v) => Constant::Double(-v),
            _ => return None,
        },
        (SyntaxKind::TILDE, c) => match c {
            Constant::Char(v) => Constant::Int(!i32::from(v)),
            Constant::Int(v) => Constant::Int(!v),
            Constant::Long(v) => Constant::Long(!v),
            _ => return None,
        },
        _ => return None,
    })
}

/// 二元数值提升后的类型。
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Numeric {
    Int,
    Long,
    Float,
    Double,
}

fn numeric_kind(c: &Constant) -> Option<Numeric> {
    match c {
        Constant::Char(_) | Constant::Int(_) => Some(Numeric::Int),
        Constant::Long(_) => Some(Numeric::Long),
        Constant::Float(_) => Some(Numeric::Float),
        Constant::Double(_) => Some(Numeric::Double),
        _ => None,
    }
}

fn binary(op: SyntaxKind, lhs: Constant, rhs: Constant) -> Option<Constant> {
    use SyntaxKind::*;
    // 字符串连接
    if op == PLUS && (matches!(lhs, Constant::Str(_)) || matches!(rhs, Constant::Str(_))) {
        return Some(Constant::Str(lhs.to_java_string() + &rhs.to_java_string()));
    }
    if let (Constant::Bool(a), Constant::Bool(b)) = (&lhs, &rhs) {
        let (a, b) = (*a, *b);
        return Some(Constant::Bool(match op {
            AMP_AMP | AMP => a && b,
            PIPE_PIPE | PIPE => a || b,
            CARET | NEQ => a != b,
            EQ_EQ => a == b,
            _ => return None,
        }));
    }
    if let (Constant::Str(a), Constant::Str(b)) = (&lhs, &rhs) {
        // 常量字符串是驻留的：`==` 比较内容
        return match op {
            EQ_EQ => Some(Constant::Bool(a == b)),
            NEQ => Some(Constant::Bool(a != b)),
            _ => None,
        };
    }
    // 移位：结果类型只看左操作数
    if matches!(op, SHL | SHR | USHR) {
        let distance = rhs.as_i64()?;
        return match numeric_kind(&lhs)? {
            Numeric::Int => {
                let v = lhs.as_i64()? as i32;
                let d = (distance & 31) as u32;
                Some(Constant::Int(match op {
                    SHL => v.wrapping_shl(d),
                    SHR => v.wrapping_shr(d),
                    _ => ((v as u32) >> d) as i32,
                }))
            }
            Numeric::Long => {
                let v = lhs.as_i64()?;
                let d = (distance & 63) as u32;
                Some(Constant::Long(match op {
                    SHL => v.wrapping_shl(d),
                    SHR => v.wrapping_shr(d),
                    _ => ((v as u64) >> d) as i64,
                }))
            }
            _ => None,
        };
    }
    let kind = numeric_kind(&lhs)?.max(numeric_kind(&rhs)?);
    match kind {
        Numeric::Int | Numeric::Long => {
            let (a, b) = (lhs.as_i64()?, rhs.as_i64()?);
            let wrap = |v: i64| match kind {
                Numeric::Int => Constant::Int(v as i32),
                _ => Constant::Long(v),
            };
            let (a32, b32) = (a as i32, b as i32);
            let int = kind == Numeric::Int;
            Some(match op {
                PLUS => wrap(if int {
                    i64::from(a32.wrapping_add(b32))
                } else {
                    a.wrapping_add(b)
                }),
                MINUS => wrap(if int {
                    i64::from(a32.wrapping_sub(b32))
                } else {
                    a.wrapping_sub(b)
                }),
                STAR => wrap(if int {
                    i64::from(a32.wrapping_mul(b32))
                } else {
                    a.wrapping_mul(b)
                }),
                SLASH if b == 0 => return None,
                SLASH => wrap(if int {
                    i64::from(a32.wrapping_div(b32))
                } else {
                    a.wrapping_div(b)
                }),
                PERCENT if b == 0 => return None,
                PERCENT => wrap(if int {
                    i64::from(a32.wrapping_rem(b32))
                } else {
                    a.wrapping_rem(b)
                }),
                AMP => wrap(a & b),
                PIPE => wrap(a | b),
                CARET => wrap(a ^ b),
                LT => Constant::Bool(a < b),
                GT => Constant::Bool(a > b),
                LT_EQ => Constant::Bool(a <= b),
                GT_EQ => Constant::Bool(a >= b),
                EQ_EQ => Constant::Bool(a == b),
                NEQ => Constant::Bool(a != b),
                _ => return None,
            })
        }
        Numeric::Float | Numeric::Double => {
            let (a, b) = (lhs.as_f64()?, rhs.as_f64()?);
            let (a, b) = if kind == Numeric::Float {
                (f64::from(a as f32), f64::from(b as f32))
            } else {
                (a, b)
            };
            let wrap = |v: f64| match kind {
                Numeric::Float => Constant::Float(v as f32),
                _ => Constant::Double(v),
            };
            Some(match op {
                PLUS => wrap(a + b),
                MINUS => wrap(a - b),
                STAR => wrap(a * b),
                SLASH => wrap(a / b),
                PERCENT => wrap(a % b),
                LT => Constant::Bool(a < b),
                GT => Constant::Bool(a > b),
                LT_EQ => Constant::Bool(a <= b),
                GT_EQ => Constant::Bool(a >= b),
                EQ_EQ => Constant::Bool(a == b),
                NEQ => Constant::Bool(a != b),
                _ => return None,
            })
        }
    }
}

/// 赋给声明类型时的转换：`long x = 1` 是 `1L`，`char c = 65` 是 `'A'`。
fn coerce(value: Constant, ty: &TypeRef) -> Option<Constant> {
    match ty {
        TypeRef::Primitive(p) => value.cast_to(*p),
        _ if is_string_ref(ty) => matches!(value, Constant::Str(_)).then_some(value),
        _ => None,
    }
}

/// 声明的类型是不是 `String`（源码里写 `String` 或 `java.lang.String`）。
fn is_string_ref(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Class { name, args } => args.is_empty() && name == "java/lang/String",
        TypeRef::Path(segments) => {
            let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
            segments.iter().all(|s| s.args.is_empty())
                && matches!(names.as_slice(), ["String"] | ["java", "lang", "String"])
        }
        _ => false,
    }
}

fn from_class_file(value: &FieldConstant, ty: &TypeRef) -> Option<Constant> {
    let raw = match value {
        FieldConstant::Int(v) => Constant::Int(*v),
        FieldConstant::Long(v) => Constant::Long(*v),
        FieldConstant::Float(v) => Constant::Float(f32::from_bits(*v)),
        FieldConstant::Double(v) => Constant::Double(f64::from_bits(*v)),
        FieldConstant::Str(s) => Constant::Str(s.to_string()),
    };
    // class 文件里 boolean / char / byte / short 都存成 int
    match (ty, &raw) {
        (TypeRef::Primitive(PrimitiveType::Boolean), Constant::Int(v)) => {
            Some(Constant::Bool(*v != 0))
        }
        (TypeRef::Primitive(p), _) => raw.cast_to(*p),
        _ => Some(raw),
    }
}

fn constant_of_field(db: &dyn Db, field: FieldId, depth: u32) -> Option<Constant> {
    if depth > MAX_DEPTH {
        return None;
    }
    let f = field_data(db, field)?;
    if !f.flags.is_final() || f.is_enum_constant() {
        return None;
    }
    if let Some(value) = &f.constant {
        return from_class_file(value, &f.ty);
    }
    if !matches!(f.ty, TypeRef::Primitive(_)) && !is_string_ref(&f.ty) {
        return None;
    }
    let body = BodyId::Field(field);
    let (_, node) = body_syntax(db, body)?;
    let initializer = ast::VariableDeclarator::cast(node)?.initializer()?;
    let result = infer(db, body);
    let value = Eval {
        db,
        result: &result,
        depth,
        cached_fields: false,
    }
    .expr(initializer.syntax())?;
    coerce(value, &f.ty)
}

/// 表达式的常量值（不是常量表达式时为 `None`）。
pub fn expr_constant(db: &dyn Db, result: &InferenceResult, expr: &SyntaxNode) -> Option<Constant> {
    Eval {
        db,
        result,
        depth: 0,
        cached_fields: true,
    }
    .expr(expr)
}

/// 字段的常量值：带常量初值的 `final` 字段，或者 class 文件里带 `ConstantValue` 的字段。
pub fn field_constant(db: &dyn Db, field: FieldId) -> Option<Constant> {
    field_constant_tracked(db, base_db::workspace(db), field)
}

// 别的文件里用到这个常量时只依赖算出来的值：字段所在文件的其它编辑（包括让初值挪动位置的
// 编辑）不会让它们失效。引用链在内部按深度展开（不经过这个查询），循环定义不会成为查询的环。
#[salsa::tracked]
fn field_constant_tracked(
    db: &dyn Db,
    _ws: base_db::Workspace,
    field: FieldId,
) -> Option<Constant> {
    constant_of_field(db, field, 0)
}

/// `final` 局部变量的常量值。
pub fn local_constant(db: &dyn Db, local: &LocalDef) -> Option<Constant> {
    let result = infer(db, local.body);
    let data = result.local(local.id);
    if data.kind != LocalKind::Local {
        return None;
    }
    let (_, node) = body_syntax(db, local.body)?;
    let name = node.covering_element(data.decl_range);
    let declarator = name
        .parent()?
        .ancestors()
        .find_map(ast::VariableDeclarator::cast)?;
    let decl = ast::LocalVarDecl::cast(declarator.syntax().parent()?)?;
    let is_final = decl
        .modifier_list()
        .is_some_and(|m| m.keywords().any(|k| k.kind() == SyntaxKind::FINAL_KW));
    if !is_final {
        return None;
    }
    let value = Eval {
        db,
        result: &result,
        depth: 0,
        cached_fields: true,
    }
    .expr(declarator.initializer()?.syntax())?;
    match &data.ty {
        Ty::Primitive(p) => value.cast_to(*p),
        ty if is_string(db, ty) => matches!(value, Constant::Str(_)).then_some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_java() {
        assert_eq!(java_double(1.0), "1.0");
        assert_eq!(java_double(0.1), "0.1");
        assert_eq!(java_double(1e7), "1.0E7");
        assert_eq!(java_double(1.5e-5), "1.5E-5");
        assert_eq!(java_double(123456.5), "123456.5");
        assert_eq!(java_float(1.5), "1.5");
        assert_eq!(Constant::Long(3).render(), "3L");
        assert_eq!(Constant::Char(u16::from(b'\n')).render(), "'\\n'");
        assert_eq!(Constant::Str("a\"b".into()).render(), "\"a\\\"b\"");
    }

    #[test]
    fn evaluates_operators() {
        assert_eq!(
            binary(SyntaxKind::SHL, Constant::Int(1), Constant::Int(33)),
            Some(Constant::Int(2))
        );
        assert_eq!(
            binary(
                SyntaxKind::PLUS,
                Constant::Str("v".into()),
                Constant::Char(65)
            ),
            Some(Constant::Str("vA".into()))
        );
        assert_eq!(
            binary(SyntaxKind::STAR, Constant::Int(i32::MAX), Constant::Int(2)),
            Some(Constant::Int(-2))
        );
        assert_eq!(
            binary(SyntaxKind::SLASH, Constant::Int(1), Constant::Int(0)),
            None
        );
        assert_eq!(parse_integer("0xFFFFFFFF", false), Some(0xFFFF_FFFF));
        assert_eq!(
            literal_int("0xFFFFFFFF"),
            Some(Constant::Int(-1)),
            "十六进制可以占满全部位"
        );
        assert_eq!(parse_integer("2147483648", false), Some(1 << 31));
        assert_eq!(parse_integer("017", false), Some(15));
    }

    fn literal_int(text: &str) -> Option<Constant> {
        parse_integer(text, false).map(|v| Constant::Int(v as i32))
    }
}
