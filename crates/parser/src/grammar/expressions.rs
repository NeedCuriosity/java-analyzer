//! 表达式：赋值 → 条件 / lambda → 二元（优先级爬升）→ 一元 / 强转 → 后缀 → 基本表达式。

use super::{items, name, name_ref, patterns, statements, types};
use crate::{
    parser::{CompletedMarker, Marker, Parser},
    SyntaxKind::{self, *},
    T,
};

pub(super) fn expr(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let lhs = conditional_expr(p, true)?;
    if let Some(op) = assign_op(p) {
        let m = lhs.precede(p);
        p.bump(op);
        if p.at(T!['{']) {
            array_initializer(p);
        } else {
            expr(p);
        }
        return Some(m.complete(p, ASSIGN_EXPR));
    }
    Some(lhs)
}

fn assign_op(p: &Parser<'_>) -> Option<SyntaxKind> {
    if p.at(USHR_EQ) {
        return Some(USHR_EQ);
    }
    if p.at(SHR_EQ) {
        return Some(SHR_EQ);
    }
    match p.current() {
        EQ | PLUS_EQ | MINUS_EQ | STAR_EQ | SLASH_EQ | AMP_EQ | PIPE_EQ | CARET_EQ | PERCENT_EQ
        | SHL_EQ => Some(p.current()),
        _ => None,
    }
}

/// 条件表达式或 lambda。`allow_lambda` 为 `false` 用于 `case` 标签和守卫，
/// 那里的 `x ->` 不是 lambda。
pub(super) fn conditional_expr(p: &mut Parser<'_>, allow_lambda: bool) -> Option<CompletedMarker> {
    if allow_lambda && at_lambda(p) {
        return Some(lambda_expr(p));
    }
    let lhs = binary_expr(p, 1)?;
    if p.at(T![?]) {
        let m = lhs.precede(p);
        p.bump(T![?]);
        expr(p);
        p.expect(T![:]);
        conditional_expr(p, allow_lambda);
        return Some(m.complete(p, CONDITIONAL_EXPR));
    }
    Some(lhs)
}

const INSTANCEOF_BP: u8 = 7;

fn binary_op(p: &Parser<'_>) -> Option<(u8, SyntaxKind)> {
    // `>>=` / `>>>=` 是赋值运算符
    if p.at(USHR_EQ) || p.at(SHR_EQ) {
        return None;
    }
    if p.at(USHR) {
        return Some((8, USHR));
    }
    if p.at(SHR) {
        return Some((8, SHR));
    }
    let kind = p.current();
    let bp = match kind {
        PIPE_PIPE => 1,
        AMP_AMP => 2,
        PIPE => 3,
        CARET => 4,
        AMP => 5,
        EQ_EQ | NEQ => 6,
        LT | GT | LT_EQ | GT_EQ => 7,
        SHL => 8,
        PLUS | MINUS => 9,
        STAR | SLASH | PERCENT => 10,
        _ => return None,
    };
    Some((bp, kind))
}

fn binary_expr(p: &mut Parser<'_>, min_bp: u8) -> Option<CompletedMarker> {
    let mut lhs = unary_expr(p)?;
    loop {
        if p.at(T![instanceof]) && INSTANCEOF_BP >= min_bp {
            let m = lhs.precede(p);
            p.bump(T![instanceof]);
            if patterns::at_pattern(p) {
                patterns::pattern(p);
            } else {
                types::type_(p);
            }
            lhs = m.complete(p, INSTANCEOF_EXPR);
            continue;
        }
        let (bp, op) = match binary_op(p) {
            Some(it) => it,
            None => break,
        };
        if bp < min_bp {
            break;
        }
        let m = lhs.precede(p);
        p.bump(op);
        binary_expr(p, bp + 1);
        lhs = m.complete(p, BINARY_EXPR);
    }
    Some(lhs)
}

fn unary_expr(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    match p.current() {
        PLUS | MINUS | BANG | TILDE | PLUS_PLUS | MINUS_MINUS => {
            let m = p.start();
            p.bump_any();
            unary_expr(p);
            Some(m.complete(p, UNARY_EXPR))
        }
        L_PAREN if at_cast(p) => Some(cast_expr(p)),
        _ => postfix_expr(p),
    }
}

fn starts_unary_not_plus_minus(kind: SyntaxKind) -> bool {
    kind.is_literal()
        || kind.is_primitive_type_keyword()
        || matches!(
            kind,
            IDENT | L_PAREN | BANG | TILDE | THIS_KW | SUPER_KW | NEW_KW | SWITCH_KW | VOID_KW
        )
}

/// 当前在 `(`，判断这是不是一个强制类型转换。
fn at_cast(p: &Parser<'_>) -> bool {
    let mut end = match types::scan_type(p, 1) {
        Some(end) => end,
        None => return false,
    };
    let single_primitive = end == 2 && p.nth(1).is_primitive_type_keyword();
    while p.nth(end) == T![&] {
        end = match types::scan_type(p, end + 1) {
            Some(end) => end,
            None => return false,
        };
    }
    if p.nth(end) != T![')'] {
        return false;
    }
    let next = p.nth(end + 1);
    if single_primitive && matches!(next, PLUS | MINUS | PLUS_PLUS | MINUS_MINUS) {
        return true;
    }
    starts_unary_not_plus_minus(next)
}

fn cast_expr(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump(T!['(']);
    let tm = p.start();
    types::type_(p);
    if p.at(T![&]) {
        while p.eat(T![&]) {
            types::type_(p);
        }
        tm.complete(p, INTERSECTION_TYPE);
    } else {
        tm.abandon(p);
    }
    p.expect(T![')']);
    if at_lambda(p) {
        lambda_expr(p);
    } else {
        unary_expr(p);
    }
    m.complete(p, CAST_EXPR)
}

fn postfix_expr(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let mut lhs = primary_expr(p)?;
    loop {
        lhs = match p.current() {
            DOT => match p.nth(1) {
                IDENT => {
                    let m = lhs.precede(p);
                    p.bump(T![.]);
                    name_ref(p);
                    if p.at(T!['(']) {
                        arg_list(p);
                        m.complete(p, METHOD_CALL)
                    } else {
                        m.complete(p, FIELD_ACCESS)
                    }
                }
                LT => {
                    let m = lhs.precede(p);
                    p.bump(T![.]);
                    types::type_arg_list(p);
                    if p.at(IDENT) {
                        name_ref(p);
                    } else if p.at(T![super]) || p.at(T![this]) {
                        p.bump_any();
                    } else {
                        p.error("此处应为方法名");
                    }
                    if p.at(T!['(']) {
                        arg_list(p);
                    } else {
                        p.error("此处应为实参列表");
                    }
                    m.complete(p, METHOD_CALL)
                }
                NEW_KW => {
                    let m = lhs.precede(p);
                    p.bump(T![.]);
                    new_expr_rest(p, m)
                }
                THIS_KW => {
                    let m = lhs.precede(p);
                    p.bump(T![.]);
                    p.bump(T![this]);
                    m.complete(p, THIS_EXPR)
                }
                SUPER_KW => {
                    let m = lhs.precede(p);
                    p.bump(T![.]);
                    p.bump(T![super]);
                    m.complete(p, SUPER_EXPR)
                }
                CLASS_KW => {
                    let m = lhs.precede(p);
                    p.bump(T![.]);
                    p.bump(T![class]);
                    m.complete(p, CLASS_LITERAL)
                }
                _ => {
                    // `foo.` 后面还没写成员名：仍然产出 FIELD_ACCESS，补全依赖这个形状
                    let m = lhs.precede(p);
                    p.bump(T![.]);
                    p.error("此处应为成员名");
                    m.complete(p, FIELD_ACCESS)
                }
            },
            L_BRACK => {
                let m = lhs.precede(p);
                p.bump(T!['[']);
                expr(p);
                p.expect(T![']']);
                m.complete(p, ARRAY_ACCESS)
            }
            PLUS_PLUS | MINUS_MINUS => {
                let m = lhs.precede(p);
                p.bump_any();
                m.complete(p, POSTFIX_EXPR)
            }
            COLON_COLON => {
                let m = lhs.precede(p);
                method_ref_rest(p);
                m.complete(p, METHOD_REF)
            }
            L_PAREN if matches!(lhs.kind(), THIS_EXPR | SUPER_EXPR) => {
                let m = lhs.precede(p);
                arg_list(p);
                m.complete(p, EXPLICIT_CONSTRUCTOR_CALL)
            }
            _ => break,
        };
    }
    Some(lhs)
}

/// 当前在 `::`。
fn method_ref_rest(p: &mut Parser<'_>) {
    p.bump(T![::]);
    if p.at(T![<]) {
        types::type_arg_list(p);
    }
    if p.at(IDENT) {
        name_ref(p);
    } else if p.at(T![new]) {
        p.bump(T![new]);
    } else {
        p.error("此处应为方法名或 `new`");
    }
}

fn primary_expr(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let kind = p.current();
    if kind.is_literal() {
        let m = p.start();
        p.bump_any();
        return Some(m.complete(p, LITERAL));
    }
    if kind.is_primitive_type_keyword() || kind == VOID_KW {
        return Some(type_expr(p));
    }
    match kind {
        IDENT => {
            if matches!(p.nth(1), LT | L_BRACK | DOT) && at_type_in_expr(p) {
                return Some(type_expr(p));
            }
            let m = p.start();
            name_ref(p);
            if p.at(T!['(']) {
                arg_list(p);
                Some(m.complete(p, METHOD_CALL))
            } else {
                Some(m.complete(p, NAME_EXPR))
            }
        }
        THIS_KW => {
            let m = p.start();
            p.bump(T![this]);
            Some(m.complete(p, THIS_EXPR))
        }
        SUPER_KW => {
            let m = p.start();
            p.bump(T![super]);
            Some(m.complete(p, SUPER_EXPR))
        }
        L_PAREN => {
            let m = p.start();
            p.bump(T!['(']);
            expr(p);
            p.expect(T![')']);
            Some(m.complete(p, PAREN_EXPR))
        }
        NEW_KW => {
            let m = p.start();
            Some(new_expr_rest(p, m))
        }
        SWITCH_KW => {
            let m = p.start();
            statements::switch_rest(p);
            Some(m.complete(p, SWITCH_EXPR))
        }
        _ => {
            p.error("此处应为表达式");
            None
        }
    }
}

/// 表达式位置上的 `List<String>::new`、`String[]::new`、`int[].class` 这类以类型开头的写法。
fn at_type_in_expr(p: &Parser<'_>) -> bool {
    match types::scan_type(p, 0) {
        // 只有带类型实参或数组维度时才必须按类型解析；`a.b::c`、`a.b.class` 走普通表达式路径
        Some(end) if matches!(p.nth(end - 1), GT | R_BRACK) => {
            p.nth(end) == T![::] || (p.nth(end) == T![.] && p.nth(end + 1) == T![class])
        }
        _ => false,
    }
}

fn type_expr(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    if p.at(T![void]) {
        let t = p.start();
        p.bump(T![void]);
        t.complete(p, PRIMITIVE_TYPE);
    } else {
        types::type_(p);
    }
    if p.at(T![.]) && p.nth(1) == T![class] {
        p.bump(T![.]);
        p.bump(T![class]);
        m.complete(p, CLASS_LITERAL)
    } else if p.at(T![::]) {
        method_ref_rest(p);
        m.complete(p, METHOD_REF)
    } else {
        p.error("此处应为 `.class` 或 `::`");
        m.complete(p, ERROR)
    }
}

/// 当前在 `new`。`m` 可能已经包含了限定表达式（`outer.new Inner()`）。
fn new_expr_rest(p: &mut Parser<'_>, m: Marker) -> CompletedMarker {
    p.bump(T![new]);
    if p.at(T![<]) {
        types::type_arg_list(p);
    }
    types::type_no_array(p);
    if p.at(T!['[']) {
        while p.at(T!['[']) {
            let d = p.start();
            p.bump(T!['[']);
            if !p.at(T![']']) {
                expr(p);
            }
            p.expect(T![']']);
            d.complete(p, DIM_EXPR);
        }
        if p.at(T!['{']) {
            array_initializer(p);
        }
        m.complete(p, ARRAY_CREATION_EXPR)
    } else {
        if p.at(T!['(']) {
            arg_list(p);
        } else {
            p.error("此处应为 `(` 或 `[`");
        }
        if p.at(T!['{']) {
            items::class_body(p);
        }
        m.complete(p, NEW_EXPR)
    }
}

fn at_lambda(p: &Parser<'_>) -> bool {
    if p.no_lambda() {
        return false;
    }
    match p.current() {
        IDENT => p.nth(1) == T![->],
        L_PAREN => {
            let mut depth = 0usize;
            let mut i = 0;
            loop {
                match p.nth(i) {
                    L_PAREN => depth += 1,
                    R_PAREN => {
                        depth -= 1;
                        if depth == 0 {
                            return p.nth(i + 1) == T![->];
                        }
                    }
                    EOF | SEMICOLON | L_CURLY | R_CURLY => return false,
                    _ => {}
                }
                i += 1;
            }
        }
        _ => false,
    }
}

fn lambda_expr(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    let params = p.start();
    if p.at(IDENT) {
        let param = p.start();
        name(p);
        param.complete(p, PARAM);
    } else {
        p.bump(T!['(']);
        while !p.at(EOF) && !p.at(T![')']) {
            let before = p.pos();
            if p.at(IDENT) && matches!(p.nth(1), COMMA | R_PAREN) {
                let param = p.start();
                name(p);
                param.complete(p, PARAM);
            } else {
                items::param(p);
            }
            if p.pos() == before || !p.eat(T![,]) {
                break;
            }
        }
        p.expect(T![')']);
    }
    params.complete(p, PARAM_LIST);
    p.expect(T![->]);
    if p.at(T!['{']) {
        statements::block(p);
    } else {
        expr(p);
    }
    m.complete(p, LAMBDA_EXPR)
}

pub(super) fn arg_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(T!['(']);
    while !p.at(EOF) && !p.at(T![')']) {
        if expr(p).is_none() && !p.at(T![,]) {
            break;
        }
        if !p.eat(T![,]) {
            break;
        }
    }
    p.expect(T![')']);
    m.complete(p, ARG_LIST);
}

pub(super) fn array_initializer(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(T!['{']);
    while !p.at(EOF) && !p.at(T!['}']) {
        if p.at(T!['{']) {
            array_initializer(p);
        } else if expr(p).is_none() && !p.at(T![,]) {
            break;
        }
        if !p.eat(T![,]) {
            break;
        }
    }
    p.expect(T!['}']);
    m.complete(p, ARRAY_INITIALIZER);
}
