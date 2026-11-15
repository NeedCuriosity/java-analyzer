//! 类型声明与成员声明。

use super::{annotation, element_value, expressions, modifiers, name, name_ref, statements, types};
use crate::{
    parser::{Marker, Parser},
    SyntaxKind::*,
    T,
};

pub(super) fn at_type_decl(p: &Parser<'_>) -> bool {
    match p.current() {
        T![class] | T![interface] | T![enum] => true,
        T![@] => p.nth(1) == T![interface],
        IDENT => at_record_decl(p),
        _ => false,
    }
}

fn at_record_decl(p: &Parser<'_>) -> bool {
    p.at_contextual(RECORD_KW) && p.nth(1) == IDENT && matches!(p.nth(2), T!['('] | T![<])
}

/// 修饰符之后的类型声明。调用前必须保证 `at_type_decl(p)`。
pub(super) fn type_decl_rest(p: &mut Parser<'_>, m: Marker) {
    match p.current() {
        T![class] => {
            p.bump(T![class]);
            name(p);
            opt_type_params(p);
            opt_extends(p);
            opt_implements(p);
            opt_permits(p);
            class_body(p);
            m.complete(p, CLASS_DECL);
        }
        T![interface] => {
            p.bump(T![interface]);
            name(p);
            opt_type_params(p);
            opt_extends(p);
            opt_permits(p);
            class_body(p);
            m.complete(p, INTERFACE_DECL);
        }
        T![enum] => {
            p.bump(T![enum]);
            name(p);
            opt_implements(p);
            enum_body(p);
            m.complete(p, ENUM_DECL);
        }
        T![@] => {
            p.bump(T![@]);
            p.bump(T![interface]);
            name(p);
            class_body(p);
            m.complete(p, ANNOTATION_TYPE_DECL);
        }
        _ => {
            p.bump(RECORD_KW);
            name(p);
            opt_type_params(p);
            if p.at(T!['(']) {
                param_list(p);
            } else {
                p.error("此处应为记录的组件列表");
            }
            opt_implements(p);
            class_body(p);
            m.complete(p, RECORD_DECL);
        }
    }
}

fn opt_type_params(p: &mut Parser<'_>) {
    if p.at(T![<]) {
        types::type_param_list(p);
    }
}

fn opt_extends(p: &mut Parser<'_>) {
    if p.at(T![extends]) {
        let m = p.start();
        p.bump(T![extends]);
        types::type_list(p);
        m.complete(p, EXTENDS_CLAUSE);
    }
}

fn opt_implements(p: &mut Parser<'_>) {
    if p.at(T![implements]) {
        let m = p.start();
        p.bump(T![implements]);
        types::type_list(p);
        m.complete(p, IMPLEMENTS_CLAUSE);
    }
}

fn opt_permits(p: &mut Parser<'_>) {
    if p.at_contextual(PERMITS_KW) {
        let m = p.start();
        p.bump(PERMITS_KW);
        types::type_list(p);
        m.complete(p, PERMITS_CLAUSE);
    }
}

pub(super) fn class_body(p: &mut Parser<'_>) {
    if !p.at(T!['{']) {
        p.error("此处应为 `{`");
        return;
    }
    let m = p.start();
    p.bump(T!['{']);
    members(p);
    p.expect(T!['}']);
    m.complete(p, CLASS_BODY);
}

fn enum_body(p: &mut Parser<'_>) {
    if !p.at(T!['{']) {
        p.error("此处应为 `{`");
        return;
    }
    let m = p.start();
    p.bump(T!['{']);
    while p.at(IDENT) || (p.at(T![@]) && p.nth(1) != T![interface]) {
        enum_constant(p);
        if !p.eat(T![,]) {
            break;
        }
    }
    if p.eat(T![;]) {
        members(p);
    } else if !p.at(T!['}']) {
        p.error("此处应为 `;` 或 `}`");
        members(p);
    }
    p.expect(T!['}']);
    m.complete(p, CLASS_BODY);
}

fn enum_constant(p: &mut Parser<'_>) {
    let m = p.start();
    while p.at(T![@]) {
        annotation(p);
    }
    name(p);
    if p.at(T!['(']) {
        expressions::arg_list(p);
    }
    if p.at(T!['{']) {
        class_body(p);
    }
    m.complete(p, ENUM_CONSTANT);
}

fn members(p: &mut Parser<'_>) {
    while !p.at(EOF) && !p.at(T!['}']) {
        let before = p.pos();
        member(p);
        if p.pos() == before {
            p.err_and_bump("此处应为成员声明");
        }
    }
}

fn member(p: &mut Parser<'_>) {
    match p.current() {
        T![;] => {
            p.bump(T![;]);
            return;
        }
        T!['{'] => {
            let m = p.start();
            statements::block(p);
            m.complete(p, INITIALIZER_BLOCK);
            return;
        }
        _ => {}
    }
    let m = p.start();
    let has_modifiers = modifiers(p);
    if p.at(T!['{']) {
        statements::block(p);
        m.complete(p, INITIALIZER_BLOCK);
        return;
    }
    member_rest(p, m, has_modifiers);
}

/// 修饰符之后的成员：类型声明、构造器、方法或字段。
pub(super) fn member_rest(p: &mut Parser<'_>, m: Marker, has_modifiers: bool) {
    if at_type_decl(p) {
        type_decl_rest(p, m);
        return;
    }
    let has_type_params = p.at(T![<]);
    if has_type_params {
        types::type_param_list(p);
    }

    // 构造器：`Name(`；记录的紧凑构造器：`Name {`
    if p.at(IDENT) && p.nth(1) == T!['('] {
        name(p);
        param_list(p);
        opt_throws(p);
        body_or_semicolon(p);
        m.complete(p, CONSTRUCTOR_DECL);
        return;
    }
    if p.at(IDENT) && p.nth(1) == T!['{'] && !has_type_params {
        name(p);
        statements::block(p);
        m.complete(p, CONSTRUCTOR_DECL);
        return;
    }

    if p.at(T![void]) {
        let t = p.start();
        p.bump(T![void]);
        t.complete(p, PRIMITIVE_TYPE);
    } else if types::at_type_start(p) {
        types::type_(p);
    } else {
        if has_modifiers || has_type_params {
            p.error("此处应为成员声明");
            m.complete(p, ERROR);
        } else {
            m.abandon(p);
        }
        return;
    }

    if p.at(IDENT) && p.nth(1) == T!['('] {
        name(p);
        param_list(p);
        while p.at(T!['[']) && p.nth(1) == T![']'] {
            p.bump(T!['[']);
            p.bump(T![']']);
        }
        opt_throws(p);
        if p.at(T![default]) {
            let d = p.start();
            p.bump(T![default]);
            element_value(p);
            d.complete(p, DEFAULT_VALUE);
        }
        body_or_semicolon(p);
        m.complete(p, METHOD_DECL);
    } else {
        variable_declarators(p);
        p.expect(T![;]);
        m.complete(p, FIELD_DECL);
    }
}

fn body_or_semicolon(p: &mut Parser<'_>) {
    if p.at(T!['{']) {
        statements::block(p);
    } else {
        p.expect(T![;]);
    }
}

fn opt_throws(p: &mut Parser<'_>) {
    if p.at(T![throws]) {
        let m = p.start();
        p.bump(T![throws]);
        types::type_list(p);
        m.complete(p, THROWS_CLAUSE);
    }
}

pub(super) fn param_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(T!['(']);
    while !p.at(EOF) && !p.at(T![')']) {
        let before = p.pos();
        param(p);
        if p.pos() == before || !p.eat(T![,]) {
            break;
        }
    }
    p.expect(T![')']);
    m.complete(p, PARAM_LIST);
}

/// 形式参数（也用于记录组件）：`修饰符 类型 [...] 名字 []`，以及接收者参数 `Foo this`。
pub(super) fn param(p: &mut Parser<'_>) {
    let m = p.start();
    let has_modifiers = modifiers(p);
    if !types::at_type_start(p) {
        p.error("此处应为参数");
        if has_modifiers {
            m.complete(p, ERROR);
        } else {
            m.abandon(p);
        }
        return;
    }
    types::type_(p);
    while p.at(T![@]) {
        annotation(p);
    }
    p.eat(T![...]);
    if p.at(T![this]) {
        p.bump(T![this]);
    } else if p.at(IDENT) && p.nth(1) == T![.] && p.nth(2) == T![this] {
        name_ref(p);
        p.bump(T![.]);
        p.bump(T![this]);
    } else {
        name(p);
    }
    while p.at(T!['[']) && p.nth(1) == T![']'] {
        p.bump(T!['[']);
        p.bump(T![']']);
    }
    m.complete(p, PARAM);
}

pub(super) fn variable_declarators(p: &mut Parser<'_>) {
    variable_declarator(p);
    while p.eat(T![,]) {
        variable_declarator(p);
    }
}

pub(super) fn variable_declarator(p: &mut Parser<'_>) {
    let m = p.start();
    name(p);
    while p.at(T!['[']) && p.nth(1) == T![']'] {
        p.bump(T!['[']);
        p.bump(T![']']);
    }
    if p.eat(T![=]) {
        if p.at(T!['{']) {
            expressions::array_initializer(p);
        } else {
            expressions::expr(p);
        }
    }
    m.complete(p, VARIABLE_DECLARATOR);
}
