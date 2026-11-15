//! 用 `if` / `if-else` / `while` / `try-catch` / `try-finally` / `synchronized` / 代码块
//! 包裹选中的语句，以及拆分 / 合并嵌套的 `if`。

use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use super::{Assist, AssistCtx, AssistKind};

/// 选区完整覆盖的、同一个块里连续的若干条语句。
fn selected_statements(ctx: &AssistCtx<'_>) -> Option<Vec<SyntaxNode>> {
    if ctx.range.is_empty() {
        return None;
    }
    let text = ctx.text;
    // 选区两端的空白不算
    let start = usize::from(ctx.range.start());
    let end = usize::from(ctx.range.end()).min(text.len());
    let trimmed_start = start + (text[start..end].len() - text[start..end].trim_start().len());
    let trimmed_end = end - (text[start..end].len() - text[start..end].trim_end().len());
    if trimmed_start >= trimmed_end {
        return None;
    }
    let range = TextRange::new((trimmed_start as u32).into(), (trimmed_end as u32).into());
    let covering = match ctx.root.covering_element(range) {
        syntax::NodeOrToken::Node(n) => n,
        syntax::NodeOrToken::Token(t) => t.parent()?,
    };
    let block = covering
        .ancestors()
        .find(|n| n.kind() == SyntaxKind::BLOCK)?;
    let stmts: Vec<SyntaxNode> = block
        .children()
        .filter(|c| ast::Stmt::can_cast(c.kind()))
        .filter(|c| range.contains_range(c.text_range()))
        .collect();
    let first = stmts.first()?;
    let last = stmts.last()?;
    // 选区必须正好从第一条语句开始、到最后一条语句结束
    if first.text_range().start() != range.start() || last.text_range().end() != range.end() {
        return None;
    }
    Some(stmts)
}

/// 选中的声明在选区之后还被用到：包起来后变量出了作用域，不提供。
fn declarations_escape(stmts: &[SyntaxNode]) -> bool {
    let names: Vec<String> = stmts
        .iter()
        .filter_map(|s| ast::LocalVarDecl::cast(s.clone()))
        .flat_map(|d| d.declarators())
        .filter_map(|d| d.name().map(|n| n.text().to_string()))
        .collect();
    if names.is_empty() {
        return false;
    }
    let Some(last) = stmts.last() else {
        return false;
    };
    let mut next = last.next_sibling();
    while let Some(stmt) = next {
        let used = stmt
            .descendants()
            .filter(|d| d.kind() == SyntaxKind::NAME_REF)
            .any(|d| names.contains(&d.text().to_string()));
        if used {
            return true;
        }
        next = stmt.next_sibling();
    }
    false
}

pub(super) fn surround_with(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(stmts) = selected_statements(ctx) else {
        return;
    };
    if declarations_escape(&stmts) {
        return;
    }
    let (Some(first), Some(last)) = (stmts.first(), stmts.last()) else {
        return;
    };
    let range = TextRange::new(first.text_range().start(), last.text_range().end());
    let indent = ctx.indent_at(range.start());
    let original = &ctx.text[usize::from(range.start())..usize::from(range.end())];
    // 里面的语句多缩进一层
    let inner = original.replace('\n', "\n    ");
    let in_static = first.ancestors().any(|a| {
        ast::MethodDecl::cast(a.clone())
            .and_then(|m| m.modifier_list())
            .is_some_and(|m| m.keywords().any(|k| k.kind() == SyntaxKind::STATIC_KW))
    });
    let lock = if in_static {
        ctx.enclosing_class()
            .and_then(|(_, decl)| decl.name())
            .map(|n| format!("{}.class", n.text()))
            .unwrap_or_else(|| "this".to_owned())
    } else {
        "this".to_owned()
    };
    let wrappers: Vec<(&'static str, String, String)> = vec![
        (
            "surround_if",
            base_db::trs!("用 if 包裹", "Surround with if").to_owned(),
            format!("if (true) {{\n{indent}    {inner}\n{indent}}}"),
        ),
        (
            "surround_if_else",
            base_db::trs!("用 if / else 包裹", "Surround with if / else").to_owned(),
            format!("if (true) {{\n{indent}    {inner}\n{indent}}} else {{\n{indent}}}"),
        ),
        (
            "surround_while",
            base_db::trs!("用 while 包裹", "Surround with while").to_owned(),
            format!("while (true) {{\n{indent}    {inner}\n{indent}}}"),
        ),
        (
            "surround_try_catch",
            base_db::trs!("用 try / catch 包裹", "Surround with try / catch").to_owned(),
            format!(
                "try {{\n{indent}    {inner}\n{indent}}} catch (Exception e) {{\n{indent}    throw new RuntimeException(e);\n{indent}}}"
            ),
        ),
        (
            "surround_try_finally",
            base_db::trs!("用 try / finally 包裹", "Surround with try / finally").to_owned(),
            format!("try {{\n{indent}    {inner}\n{indent}}} finally {{\n{indent}}}"),
        ),
        (
            "surround_synchronized",
            base_db::trs!("用 synchronized 包裹", "Surround with synchronized").to_owned(),
            format!("synchronized ({lock}) {{\n{indent}    {inner}\n{indent}}}"),
        ),
        (
            "surround_block",
            base_db::trs!("用代码块包裹", "Surround with block").to_owned(),
            format!("{{\n{indent}    {inner}\n{indent}}}"),
        ),
    ];
    for (id, label, text) in wrappers {
        out.push(ctx.assist(
            id,
            label,
            AssistKind::RefactorRewrite,
            vec![TextEdit::replace(range, text)],
        ));
    }
}

/// `if (a && b) { body }`（没有 else）拆成 `if (a) { if (b) { body } }`；
/// 反过来，只含一个 `if` 的 `if`（都没有 else）合并成 `if (a && b)`。
pub(super) fn split_or_merge_if(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = ctx.root.token_at_offset(ctx.range.start()).right_biased() else {
        return;
    };
    // 只在 `if` 关键字上，或者在 `&&` 上
    let if_stmt = match token.kind() {
        SyntaxKind::IF_KW | SyntaxKind::AMP_AMP => {
            token.parent_ancestors().find_map(ast::IfStmt::cast)
        }
        _ => None,
    };
    let Some(if_stmt) = if_stmt else {
        return;
    };
    if if_stmt.else_branch().is_some() {
        return;
    }
    let Some(condition) = if_stmt.condition() else {
        return;
    };
    let Some(then) = if_stmt.then_branch() else {
        return;
    };
    let indent = ctx.indent_at(if_stmt.syntax().text_range().start());
    let body_text = |stmt: &ast::Stmt| -> String {
        match stmt {
            ast::Stmt::Block(block) => {
                let text = block.syntax().text().to_string();
                text.trim_start_matches('{')
                    .trim_end_matches('}')
                    .trim()
                    .to_owned()
            }
            other => other.syntax().text().to_string(),
        }
    };

    // 拆分：条件是顶层的 `a && b`
    if let ast::Expr::BinaryExpr(binary) = &condition {
        if binary.op_kind() == Some(SyntaxKind::AMP_AMP) {
            if let (Some(lhs), Some(rhs)) = (binary.lhs(), binary.rhs()) {
                let body = body_text(&then).replace('\n', "\n    ");
                let text = format!(
                    "if ({}) {{\n{indent}    if ({}) {{\n{indent}        {body}\n{indent}    }}\n{indent}}}",
                    lhs.syntax().text(),
                    rhs.syntax().text()
                );
                out.push(ctx.assist(
                    "split_if",
                    base_db::trs!("拆分成嵌套的 if", "Split into nested if"),
                    AssistKind::RefactorRewrite,
                    vec![TextEdit::replace(if_stmt.syntax().text_range(), text)],
                ));
            }
        }
    }

    // 合并：体里只有一个没有 else 的 if
    if token.kind() != SyntaxKind::IF_KW {
        return;
    }
    let inner = match &then {
        ast::Stmt::Block(block) => {
            let mut stmts = block.statements();
            match (stmts.next(), stmts.next()) {
                (Some(ast::Stmt::IfStmt(inner)), None) => Some(inner),
                _ => None,
            }
        }
        ast::Stmt::IfStmt(inner) => Some(inner.clone()),
        _ => None,
    };
    let Some(inner) = inner.filter(|i| i.else_branch().is_none()) else {
        return;
    };
    // 块里有注释时不合并，免得丢掉
    let has_comment = then
        .syntax()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind().is_trivia() && t.kind() != SyntaxKind::WHITESPACE);
    let (Some(inner_cond), Some(inner_then)) = (inner.condition(), inner.then_branch()) else {
        return;
    };
    if has_comment {
        return;
    }
    let wrap = |e: &ast::Expr| -> String {
        let text = e.syntax().text().to_string();
        let needs_parens = matches!(e, ast::Expr::BinaryExpr(b) if b.op_kind() == Some(SyntaxKind::PIPE_PIPE))
            || matches!(e, ast::Expr::ConditionalExpr(_) | ast::Expr::AssignExpr(_));
        if needs_parens {
            format!("({text})")
        } else {
            text
        }
    };
    let body = body_text(&inner_then);
    // 内层体相对外层多缩进了一层，去掉一层
    let body = body.replace(&format!("\n{indent}        "), &format!("\n{indent}    "));
    let text = format!(
        "if ({} && {}) {{\n{indent}    {body}\n{indent}}}",
        wrap(&condition),
        wrap(&inner_cond)
    );
    out.push(ctx.assist(
        "merge_nested_if",
        base_db::trs!("合并嵌套的 if", "Merge nested if"),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(if_stmt.syntax().text_range(), text)],
    ));
}
