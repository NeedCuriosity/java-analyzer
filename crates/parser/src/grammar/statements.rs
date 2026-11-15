//! 语句。

use super::{expressions, items, modifiers, name, name_ref, patterns, types};
use crate::{
    parser::{Marker, Parser},
    SyntaxKind::*,
    T,
};

pub(super) fn block(p: &mut Parser<'_>) {
    if !p.at(T!['{']) {
        p.error("此处应为 `{`");
        return;
    }
    let m = p.start();
    p.bump(T!['{']);
    while !p.at(EOF) && !p.at(T!['}']) {
        // 访问修饰符不可能出现在语句位置：多半是上面的块少了 `}`，
        // 在这里收尾，让外层的类体继续解析后面的成员。
        if matches!(p.current(), PUBLIC_KW | PRIVATE_KW | PROTECTED_KW) {
            break;
        }
        let before = p.pos();
        statement(p);
        if p.pos() == before {
            p.err_and_bump("此处应为语句");
        }
    }
    p.expect(T!['}']);
    m.complete(p, BLOCK);
}

/// 作为 `if` / 循环等的子语句：没有消费任何 token 时只报错，不跳过。
fn body_stmt(p: &mut Parser<'_>) {
    let before = p.pos();
    statement(p);
    if p.pos() == before {
        p.error("此处应为语句");
    }
}

fn paren_expr(p: &mut Parser<'_>) {
    if p.expect(T!['(']) {
        expressions::expr(p);
        p.expect(T![')']);
    }
}

pub(super) fn statement(p: &mut Parser<'_>) {
    match p.current() {
        L_CURLY => block(p),
        SEMICOLON => {
            let m = p.start();
            p.bump(T![;]);
            m.complete(p, EMPTY_STMT);
        }
        IF_KW => {
            let m = p.start();
            p.bump(T![if]);
            paren_expr(p);
            body_stmt(p);
            if p.eat(T![else]) {
                body_stmt(p);
            }
            m.complete(p, IF_STMT);
        }
        WHILE_KW => {
            let m = p.start();
            p.bump(T![while]);
            paren_expr(p);
            body_stmt(p);
            m.complete(p, WHILE_STMT);
        }
        DO_KW => {
            let m = p.start();
            p.bump(T![do]);
            body_stmt(p);
            if p.expect(T![while]) {
                paren_expr(p);
            }
            p.expect(T![;]);
            m.complete(p, DO_WHILE_STMT);
        }
        FOR_KW => for_stmt(p),
        RETURN_KW => {
            let m = p.start();
            p.bump(T![return]);
            if !p.at(T![;]) && !p.at(T!['}']) {
                expressions::expr(p);
            }
            p.expect(T![;]);
            m.complete(p, RETURN_STMT);
        }
        BREAK_KW | CONTINUE_KW => {
            let m = p.start();
            let kind = if p.at(T![break]) {
                BREAK_STMT
            } else {
                CONTINUE_STMT
            };
            p.bump_any();
            if p.at(IDENT) {
                name_ref(p);
            }
            p.expect(T![;]);
            m.complete(p, kind);
        }
        THROW_KW => {
            let m = p.start();
            p.bump(T![throw]);
            expressions::expr(p);
            p.expect(T![;]);
            m.complete(p, THROW_STMT);
        }
        TRY_KW => try_stmt(p),
        SWITCH_KW => {
            let m = p.start();
            switch_rest(p);
            m.complete(p, SWITCH_STMT);
        }
        SYNCHRONIZED_KW if p.nth(1) == T!['('] => {
            let m = p.start();
            p.bump(T![synchronized]);
            paren_expr(p);
            block(p);
            m.complete(p, SYNCHRONIZED_STMT);
        }
        ASSERT_KW => {
            let m = p.start();
            p.bump(T![assert]);
            expressions::expr(p);
            if p.eat(T![:]) {
                expressions::expr(p);
            }
            p.expect(T![;]);
            m.complete(p, ASSERT_STMT);
        }
        IDENT if p.nth(1) == T![:] => {
            let m = p.start();
            name(p);
            p.bump(T![:]);
            body_stmt(p);
            m.complete(p, LABELED_STMT);
        }
        IDENT if at_yield_stmt(p) => {
            let m = p.start();
            p.bump(YIELD_KW);
            expressions::expr(p);
            p.expect(T![;]);
            m.complete(p, YIELD_STMT);
        }
        _ => decl_or_expr_stmt(p),
    }
}

fn at_yield_stmt(p: &Parser<'_>) -> bool {
    if !p.at_contextual(YIELD_KW) {
        return false;
    }
    // `yield = 1;`、`yield.foo()`、`yield++` 等仍然把 yield 当普通标识符
    !matches!(
        p.nth(1),
        EQ | DOT
            | L_BRACK
            | PLUS_PLUS
            | MINUS_MINUS
            | SEMICOLON
            | COMMA
            | R_PAREN
            | ARROW
            | COLON_COLON
            | PLUS_EQ
            | MINUS_EQ
            | STAR_EQ
            | SLASH_EQ
            | AMP_EQ
            | PIPE_EQ
            | CARET_EQ
            | PERCENT_EQ
            | SHL_EQ
            | EOF
    )
}

fn at_local_modifier(p: &Parser<'_>) -> bool {
    match p.current() {
        FINAL_KW | ABSTRACT_KW | STATIC_KW | STRICTFP_KW => true,
        AT => p.nth(1) != T![interface],
        _ => false,
    }
}

/// 形如 `Type name` 的开头视为局部变量声明。
fn at_local_var_decl(p: &Parser<'_>) -> bool {
    matches!(types::scan_type(p, 0), Some(end) if p.nth(end) == IDENT)
}

fn decl_or_expr_stmt(p: &mut Parser<'_>) {
    if items::at_type_decl(p) {
        let m = p.start();
        items::type_decl_rest(p, m);
        return;
    }
    if at_local_modifier(p) {
        let m = p.start();
        modifiers(p);
        if items::at_type_decl(p) {
            items::type_decl_rest(p, m);
        } else {
            local_var_decl_rest(p, m, true);
        }
        return;
    }
    if at_local_var_decl(p) {
        let m = p.start();
        local_var_decl_rest(p, m, true);
        return;
    }
    let m = p.start();
    if expressions::expr(p).is_none() {
        m.abandon(p);
        return;
    }
    p.expect(T![;]);
    m.complete(p, EXPR_STMT);
}

/// `类型 声明符 {, 声明符} [;]`。`for` 的初始化部分不带分号。
fn local_var_decl_rest(p: &mut Parser<'_>, m: Marker, with_semicolon: bool) {
    types::type_(p);
    items::variable_declarators(p);
    if with_semicolon {
        p.expect(T![;]);
    }
    m.complete(p, LOCAL_VAR_DECL);
}

fn at_foreach(p: &Parser<'_>) -> bool {
    let mut i = 0;
    loop {
        let j = types::skip_annotations(p, i);
        if p.nth(j) == FINAL_KW {
            i = j + 1;
        } else {
            i = j;
            break;
        }
    }
    matches!(types::scan_type(p, i), Some(end) if p.nth(end) == IDENT && p.nth(end + 1) == T![:])
}

fn for_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(T![for]);
    if !p.expect(T!['(']) {
        m.complete(p, FOR_STMT);
        return;
    }
    if at_foreach(p) {
        let param = p.start();
        modifiers(p);
        types::type_(p);
        name(p);
        param.complete(p, PARAM);
        p.bump(T![:]);
        expressions::expr(p);
        p.expect(T![')']);
        body_stmt(p);
        m.complete(p, FOREACH_STMT);
        return;
    }

    if !p.at(T![;]) {
        let init = p.start();
        if at_local_modifier(p) || at_local_var_decl(p) {
            let decl = p.start();
            modifiers(p);
            local_var_decl_rest(p, decl, false);
        } else {
            expr_list(p);
        }
        init.complete(p, FOR_INIT);
    }
    p.expect(T![;]);
    if !p.at(T![;]) {
        expressions::expr(p);
    }
    p.expect(T![;]);
    if !p.at(T![')']) {
        let update = p.start();
        expr_list(p);
        update.complete(p, FOR_UPDATE);
    }
    p.expect(T![')']);
    body_stmt(p);
    m.complete(p, FOR_STMT);
}

fn expr_list(p: &mut Parser<'_>) {
    expressions::expr(p);
    while p.eat(T![,]) {
        expressions::expr(p);
    }
}

fn try_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(T![try]);
    if p.at(T!['(']) {
        let resources = p.start();
        p.bump(T!['(']);
        while !p.at(EOF) && !p.at(T![')']) {
            let before = p.pos();
            let resource = p.start();
            if at_local_modifier(p) || at_local_var_decl(p) {
                modifiers(p);
                types::type_(p);
                items::variable_declarator(p);
            } else {
                expressions::expr(p);
            }
            resource.complete(p, RESOURCE);
            if p.pos() == before || !p.eat(T![;]) {
                break;
            }
        }
        p.expect(T![')']);
        resources.complete(p, RESOURCE_LIST);
    }
    block(p);
    while p.at(T![catch]) {
        let c = p.start();
        p.bump(T![catch]);
        if p.expect(T!['(']) {
            let param = p.start();
            modifiers(p);
            let tm = p.start();
            types::type_(p);
            if p.at(T![|]) {
                while p.eat(T![|]) {
                    types::type_(p);
                }
                tm.complete(p, UNION_TYPE);
            } else {
                tm.abandon(p);
            }
            name(p);
            param.complete(p, PARAM);
            p.expect(T![')']);
        }
        block(p);
        c.complete(p, CATCH_CLAUSE);
    }
    if p.at(T![finally]) {
        let f = p.start();
        p.bump(T![finally]);
        block(p);
        f.complete(p, FINALLY_CLAUSE);
    }
    m.complete(p, TRY_STMT);
}

/// 当前在 `switch`；语句和表达式共用，外层节点由调用者完成。
pub(super) fn switch_rest(p: &mut Parser<'_>) {
    p.bump(T![switch]);
    paren_expr(p);
    if !p.at(T!['{']) {
        p.error("此处应为 `{`");
        return;
    }
    let m = p.start();
    p.bump(T!['{']);
    while !p.at(EOF) && !p.at(T!['}']) {
        if p.at(T![case]) || p.at(T![default]) {
            switch_case(p);
        } else {
            p.err_and_bump("此处应为 `case` 或 `default`");
        }
    }
    p.expect(T!['}']);
    m.complete(p, SWITCH_BLOCK);
}

fn switch_case(p: &mut Parser<'_>) {
    let m = p.start();
    switch_label(p);
    if p.eat(T![->]) {
        match p.current() {
            L_CURLY => block(p),
            THROW_KW => statement(p),
            _ => {
                let s = p.start();
                expressions::expr(p);
                p.expect(T![;]);
                s.complete(p, EXPR_STMT);
            }
        }
    } else {
        p.expect(T![:]);
        while !p.at(EOF) && !p.at(T!['}']) && !p.at(T![case]) && !p.at(T![default]) {
            let before = p.pos();
            statement(p);
            if p.pos() == before {
                p.err_and_bump("此处应为语句");
            }
        }
    }
    m.complete(p, SWITCH_CASE);
}

fn switch_label(p: &mut Parser<'_>) {
    let m = p.start();
    if !p.eat(T![default]) {
        p.bump(T![case]);
        // `case (int) X -> ...`：强制转换后面的 `X ->` 不是 lambda
        let saved = p.set_no_lambda(true);
        loop {
            if p.at(T![default]) {
                p.bump(T![default]);
            } else if patterns::at_pattern(p) {
                patterns::pattern(p);
            } else {
                expressions::conditional_expr(p, false);
            }
            if !p.eat(T![,]) {
                break;
            }
        }
        if p.at_contextual(WHEN_KW) {
            let g = p.start();
            p.bump(WHEN_KW);
            expressions::conditional_expr(p, false);
            g.complete(p, GUARD);
        }
        p.set_no_lambda(saved);
    }
    m.complete(p, SWITCH_LABEL);
}
