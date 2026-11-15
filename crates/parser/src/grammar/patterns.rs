//! 模式：类型模式、记录模式、未命名模式 `_`。

use super::{modifiers, name, types};
use crate::{parser::Parser, SyntaxKind::*, T};

/// `instanceof` 右侧或 `case` 之后是否是模式（而不是类型 / 常量表达式）。
pub(super) fn at_pattern(p: &Parser<'_>) -> bool {
    if p.at(T![final]) || (p.at(T![@]) && p.nth(1) != T![interface]) {
        return true;
    }
    matches!(types::scan_type(p, 0), Some(end) if matches!(p.nth(end), IDENT | L_PAREN))
}

pub(super) fn pattern(p: &mut Parser<'_>) {
    if p.at_contextual(UNDERSCORE_KW) && matches!(p.nth(1), COMMA | R_PAREN) {
        let m = p.start();
        p.bump(UNDERSCORE_KW);
        m.complete(p, UNNAMED_PATTERN);
        return;
    }
    let m = p.start();
    let has_modifiers = modifiers(p);
    if !types::at_type_start(p) {
        p.error("此处应为模式");
        if has_modifiers {
            m.complete(p, ERROR);
        } else {
            m.abandon(p);
        }
        return;
    }
    types::type_(p);
    if p.at(T!['(']) {
        p.bump(T!['(']);
        while !p.at(EOF) && !p.at(T![')']) {
            let before = p.pos();
            pattern(p);
            if p.pos() == before || !p.eat(T![,]) {
                break;
            }
        }
        p.expect(T![')']);
        m.complete(p, RECORD_PATTERN);
    } else {
        if p.at(IDENT) {
            name(p);
        } else {
            p.error("此处应为绑定变量名");
        }
        m.complete(p, TYPE_PATTERN);
    }
}
