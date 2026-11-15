//! Java 语法规则。
//!
//! 每个函数对应一条产生式，只通过 `Parser` 的原语操作 token，并保证：
//! - 任何输入都不会 panic，也不会死循环（所有循环都检查"是否前进"）；
//! - 出错时尽量产出形状正常的节点（缺失的部分留空），方便补全等功能在残缺代码上工作。

mod expressions;
mod items;
mod patterns;
mod statements;
mod types;

use crate::{
    parser::Parser,
    SyntaxKind::{self, *},
    T,
};

/// 单独解析一个块。见 [`crate::parse_block`]。
pub(crate) fn block_fragment(p: &mut Parser<'_>) {
    statements::block(p);
}

pub(crate) fn compilation_unit(p: &mut Parser<'_>) {
    let m = p.start();
    while !p.at(EOF) {
        let before = p.pos();
        top_level(p);
        if p.pos() == before {
            p.err_and_bump("此处应为声明");
        }
    }
    m.complete(p, COMPILATION_UNIT);
}

fn top_level(p: &mut Parser<'_>) {
    match p.current() {
        T![;] => {
            p.bump(T![;]);
            return;
        }
        T![import] => {
            import_decl(p);
            return;
        }
        _ => {}
    }

    let m = p.start();
    let has_modifiers = modifiers(p);
    if p.at(T![package]) {
        p.bump(T![package]);
        qualified_name(p);
        p.expect(T![;]);
        m.complete(p, PACKAGE_DECL);
    } else if at_module_decl(p) {
        module_decl_rest(p);
        m.complete(p, MODULE_DECL);
    } else {
        // 类型声明；也接受顶层的方法和字段（紧凑源文件 / 隐式类）
        items::member_rest(p, m, has_modifiers);
    }
}

fn import_decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(T![import]);
    p.eat(T![static]);
    qualified_name(p);
    if p.eat(T![.]) && !p.eat(T![*]) {
        p.error("此处应为标识符或 `*`");
    }
    p.expect(T![;]);
    m.complete(p, IMPORT_DECL);
}

fn at_module_decl(p: &Parser<'_>) -> bool {
    (p.at_contextual(MODULE_KW) && p.nth(1) == IDENT)
        || (p.at_contextual(OPEN_KW) && p.nth_contextual(1, MODULE_KW) && p.nth(2) == IDENT)
}

fn module_decl_rest(p: &mut Parser<'_>) {
    p.eat(OPEN_KW);
    p.bump(MODULE_KW);
    qualified_name(p);
    if !p.at(T!['{']) {
        p.error("此处应为 `{`");
        return;
    }
    let m = p.start();
    p.bump(T!['{']);
    while !p.at(EOF) && !p.at(T!['}']) {
        let before = p.pos();
        module_directive(p);
        if p.pos() == before {
            p.err_and_bump("此处应为模块指令");
        }
    }
    p.expect(T!['}']);
    m.complete(p, MODULE_BODY);
}

fn module_directive(p: &mut Parser<'_>) {
    let m = p.start();
    let kind = if p.at_contextual(REQUIRES_KW) {
        p.bump(REQUIRES_KW);
        loop {
            if p.at(T![static]) {
                p.bump(T![static]);
            } else if p.at_contextual(TRANSITIVE_KW) && p.nth(1) == IDENT {
                p.bump(TRANSITIVE_KW);
            } else {
                break;
            }
        }
        qualified_name(p);
        REQUIRES_DIRECTIVE
    } else if p.at_contextual(EXPORTS_KW) || p.at_contextual(OPENS_KW) {
        let kind = if p.at_contextual(EXPORTS_KW) {
            p.bump(EXPORTS_KW);
            EXPORTS_DIRECTIVE
        } else {
            p.bump(OPENS_KW);
            OPENS_DIRECTIVE
        };
        qualified_name(p);
        if p.at_contextual(TO_KW) {
            p.bump(TO_KW);
            qualified_name_list(p);
        }
        kind
    } else if p.at_contextual(USES_KW) {
        p.bump(USES_KW);
        qualified_name(p);
        USES_DIRECTIVE
    } else if p.at_contextual(PROVIDES_KW) {
        p.bump(PROVIDES_KW);
        qualified_name(p);
        if p.at_contextual(WITH_KW) {
            p.bump(WITH_KW);
            qualified_name_list(p);
        } else {
            p.error("此处应为 `with`");
        }
        PROVIDES_DIRECTIVE
    } else {
        m.abandon(p);
        return;
    };
    p.expect(T![;]);
    m.complete(p, kind);
}

fn qualified_name_list(p: &mut Parser<'_>) {
    qualified_name(p);
    while p.eat(T![,]) {
        qualified_name(p);
    }
}

// ---- 名字 ----

fn name(p: &mut Parser<'_>) {
    if p.at(IDENT) {
        let m = p.start();
        p.bump(IDENT);
        m.complete(p, NAME);
    } else {
        p.error("此处应为名字");
    }
}

fn name_ref(p: &mut Parser<'_>) {
    if p.at(IDENT) {
        let m = p.start();
        p.bump(IDENT);
        m.complete(p, NAME_REF);
    } else {
        p.error("此处应为标识符");
    }
}

/// `a.b.c`，左结合地嵌套成 `QUALIFIED_NAME`。不吃结尾的 `.*`。
fn qualified_name(p: &mut Parser<'_>) {
    let m = p.start();
    name_ref(p);
    let mut cm = m.complete(p, QUALIFIED_NAME);
    while p.at(T![.]) && p.nth(1) == IDENT {
        let m = cm.precede(p);
        p.bump(T![.]);
        name_ref(p);
        cm = m.complete(p, QUALIFIED_NAME);
    }
}

// ---- 修饰符与注解 ----

/// 解析修饰符和注解，至少有一个时产出 `MODIFIER_LIST` 并返回 `true`。
fn modifiers(p: &mut Parser<'_>) -> bool {
    let m = p.start();
    let mut any = false;
    loop {
        match p.current() {
            T![@] if p.nth(1) != T![interface] => annotation(p),
            PUBLIC_KW | PROTECTED_KW | PRIVATE_KW | STATIC_KW | ABSTRACT_KW | FINAL_KW
            | NATIVE_KW | TRANSIENT_KW | VOLATILE_KW | STRICTFP_KW => p.bump_any(),
            // `synchronized (lock) {}` 是语句，不是修饰符
            SYNCHRONIZED_KW if p.nth(1) != T!['('] => p.bump_any(),
            // `default:` / `default ->` 是 switch 标签
            DEFAULT_KW if p.nth(1) != T![:] && p.nth(1) != T![->] => p.bump_any(),
            IDENT if p.at(NON_SEALED_KW) => p.bump(NON_SEALED_KW),
            IDENT if p.at_contextual(SEALED_KW) && can_follow_sealed(p.nth(1)) => p.bump(SEALED_KW),
            _ => break,
        }
        any = true;
    }
    if any {
        m.complete(p, MODIFIER_LIST);
    } else {
        m.abandon(p);
    }
    any
}

fn can_follow_sealed(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        CLASS_KW
            | INTERFACE_KW
            | ABSTRACT_KW
            | PUBLIC_KW
            | PROTECTED_KW
            | PRIVATE_KW
            | STATIC_KW
            | FINAL_KW
            | STRICTFP_KW
            | AT
    )
}

fn annotation(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(T![@]);
    qualified_name(p);
    if p.at(T!['(']) {
        let args = p.start();
        p.bump(T!['(']);
        while !p.at(EOF) && !p.at(T![')']) {
            let before = p.pos();
            let arg = p.start();
            if p.at(IDENT) && p.nth(1) == T![=] {
                name_ref(p);
                p.bump(T![=]);
            }
            element_value(p);
            arg.complete(p, ANNOTATION_ARG);
            if !p.eat(T![,]) || p.pos() == before {
                break;
            }
        }
        p.expect(T![')']);
        args.complete(p, ANNOTATION_ARG_LIST);
    }
    m.complete(p, ANNOTATION);
}

/// 注解元素值：嵌套注解、`{...}` 数组或条件表达式。
fn element_value(p: &mut Parser<'_>) {
    match p.current() {
        T![@] => annotation(p),
        T!['{'] => {
            let m = p.start();
            p.bump(T!['{']);
            while !p.at(EOF) && !p.at(T!['}']) {
                let before = p.pos();
                element_value(p);
                if !p.eat(T![,]) || p.pos() == before {
                    break;
                }
            }
            p.expect(T!['}']);
            m.complete(p, ARRAY_INITIALIZER);
        }
        _ => {
            expressions::conditional_expr(p, true);
        }
    }
}
