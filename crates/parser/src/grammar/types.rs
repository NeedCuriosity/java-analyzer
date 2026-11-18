//! 类型的语法，以及不产生事件的类型前瞻（用于消解声明/表达式歧义）。

use super::{annotation, name, name_ref};
use crate::{
    parser::{CompletedMarker, Marker, Parser},
    SyntaxKind::*,
    T,
};

pub(super) fn at_type_start(p: &Parser<'_>) -> bool {
    let kind = p.current();
    kind == IDENT || kind == T![@] || kind.is_primitive_type_keyword()
}

/// 完整的类型，包含数组维度。
pub(super) fn type_(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let mut cm = type_no_array(p)?;
    loop {
        // 维度前可以有类型注解：`int @Nullable []`
        let j = skip_annotations(p, 0);
        if !(p.nth(j) == T!['['] && p.nth(j + 1) == T![']']) {
            break;
        }
        let m = cm.precede(p);
        while p.at(T![@]) {
            annotation(p);
        }
        p.bump(T!['[']);
        p.bump(T![']']);
        cm = m.complete(p, ARRAY_TYPE);
    }
    Some(cm)
}

/// 不含数组维度的类型：基本类型或（可带限定和类型实参的）类类型。
pub(super) fn type_no_array(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let m = p.start();
    let has_annotations = p.at(T![@]);
    while p.at(T![@]) && p.nth(1) != T![interface] {
        annotation(p);
    }
    if p.current().is_primitive_type_keyword() {
        p.bump_any();
        return Some(m.complete(p, PRIMITIVE_TYPE));
    }
    if p.at(IDENT) {
        return Some(class_type_rest(p, m));
    }
    p.error("此处应为类型");
    if has_annotations {
        m.complete(p, ERROR);
    } else {
        m.abandon(p);
    }
    None
}

fn class_type_rest(p: &mut Parser<'_>, m: Marker) -> CompletedMarker {
    name_ref(p);
    if p.at(T![<]) {
        type_arg_list(p);
    }
    let mut cm = m.complete(p, CLASS_TYPE);
    while p.at(T![.]) {
        let next = p.nth(1);
        if next == IDENT || next == T![@] {
            let m = cm.precede(p);
            p.bump(T![.]);
            while p.at(T![@]) {
                annotation(p);
            }
            name_ref(p);
            if p.at(T![<]) {
                type_arg_list(p);
            }
            cm = m.complete(p, CLASS_TYPE);
        } else if !matches!(
            next,
            STAR | CLASS_KW | THIS_KW | SUPER_KW | NEW_KW | LT | DOT | ELLIPSIS
        ) {
            // `Foo.` 后面还没写名字：把点吃进类型节点，补全时能看到限定符
            let m = cm.precede(p);
            p.bump(T![.]);
            p.error("此处应为类型名");
            cm = m.complete(p, CLASS_TYPE);
            break;
        } else {
            break;
        }
    }
    cm
}

pub(super) fn type_arg_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(T![<]);
    while !p.at(EOF) && !p.at(T![>]) {
        let before = p.pos();
        type_arg(p);
        if p.pos() == before || !p.eat(T![,]) {
            break;
        }
    }
    p.expect(T![>]);
    m.complete(p, TYPE_ARG_LIST);
}

fn type_arg(p: &mut Parser<'_>) {
    // 通配符前也可以有类型注解：`List<@NonNull ?>`
    if p.nth(skip_annotations(p, 0)) == T![?] {
        let m = p.start();
        while p.at(T![@]) {
            annotation(p);
        }
        p.bump(T![?]);
        if p.at(T![extends]) || p.at(T![super]) {
            p.bump_any();
            type_(p);
        }
        m.complete(p, WILDCARD_TYPE);
    } else {
        type_(p);
    }
}

pub(super) fn type_param_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(T![<]);
    while !p.at(EOF) && !p.at(T![>]) {
        let before = p.pos();
        type_param(p);
        if p.pos() == before || !p.eat(T![,]) {
            break;
        }
    }
    p.expect(T![>]);
    m.complete(p, TYPE_PARAM_LIST);
}

fn type_param(p: &mut Parser<'_>) {
    let m = p.start();
    while p.at(T![@]) {
        annotation(p);
    }
    name(p);
    if p.eat(T![extends]) {
        type_(p);
        while p.eat(T![&]) {
            type_(p);
        }
    }
    m.complete(p, TYPE_PARAM);
}

/// `A, B, C`
pub(super) fn type_list(p: &mut Parser<'_>) {
    type_(p);
    while p.eat(T![,]) {
        type_(p);
    }
}

// ---- 前瞻 ----

/// 从第 `start` 个前瞻 token 开始尝试匹配一个类型，成功时返回类型之后的前瞻下标。
/// 不产生任何事件。
pub(super) fn scan_type(p: &Parser<'_>, start: usize) -> Option<usize> {
    let mut i = skip_annotations(p, start);
    if p.nth(i).is_primitive_type_keyword() {
        i += 1;
    } else if p.nth(i) == IDENT {
        loop {
            i += 1;
            if p.nth(i) == T![<] {
                i = scan_type_args(p, i)?;
            }
            if p.nth(i) == T![.] && matches!(p.nth(i + 1), IDENT | AT) {
                i = skip_annotations(p, i + 1);
                if p.nth(i) != IDENT {
                    return None;
                }
                continue;
            }
            break;
        }
    } else {
        return None;
    }
    loop {
        let j = skip_annotations(p, i);
        if p.nth(j) == T!['['] && p.nth(j + 1) == T![']'] {
            i = j + 2;
        } else {
            break;
        }
    }
    Some(i)
}

fn scan_type_args(p: &Parser<'_>, start: usize) -> Option<usize> {
    debug_assert!(p.nth(start) == T![<]);
    let mut i = start + 1;
    if p.nth(i) == T![>] {
        return Some(i + 1);
    }
    loop {
        i = skip_annotations(p, i);
        if p.nth(i) == T![?] {
            i += 1;
            if matches!(p.nth(i), EXTENDS_KW | SUPER_KW) {
                i = scan_type(p, i + 1)?;
            }
        } else {
            i = scan_type(p, i)?;
        }
        match p.nth(i) {
            COMMA => i += 1,
            GT => return Some(i + 1),
            _ => return None,
        }
    }
}

/// 跳过若干个注解（含实参），返回之后的前瞻下标。
pub(super) fn skip_annotations(p: &Parser<'_>, start: usize) -> usize {
    let mut i = start;
    while p.nth(i) == T![@] && p.nth(i + 1) == IDENT {
        i += 2;
        while p.nth(i) == T![.] && p.nth(i + 1) == IDENT {
            i += 2;
        }
        if p.nth(i) == T!['('] {
            let mut depth = 0usize;
            loop {
                match p.nth(i) {
                    L_PAREN => depth += 1,
                    R_PAREN => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    EOF => return i,
                    _ => {}
                }
                i += 1;
            }
        }
    }
    i
}
