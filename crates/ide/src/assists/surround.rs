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

/// 一个分支条件：比较的对象（文本）和 case 标签。
struct Branch {
    subject: String,
    labels: Vec<String>,
}

/// `x == 1`、`x == Color.RED`、`x.equals("a")`，以及用 `||` 连起来的几个。
fn branch_of(ctx: &AssistCtx<'_>, cond: &ast::Expr) -> Option<Branch> {
    let cond = strip_parens(cond.clone());
    match &cond {
        ast::Expr::BinaryExpr(binary) if binary.op_kind() == Some(SyntaxKind::PIPE_PIPE) => {
            let a = branch_of(ctx, &binary.lhs()?)?;
            let b = branch_of(ctx, &binary.rhs()?)?;
            if a.subject != b.subject {
                return None;
            }
            let mut labels = a.labels;
            labels.extend(b.labels);
            Some(Branch {
                subject: a.subject,
                labels,
            })
        }
        ast::Expr::BinaryExpr(binary) if binary.op_kind() == Some(SyntaxKind::EQ_EQ) => {
            let (lhs, rhs) = (binary.lhs()?, binary.rhs()?);
            let (subject, label) = match (case_label(ctx, &rhs), case_label(ctx, &lhs)) {
                (Some(label), None) => (lhs, label),
                (None, Some(label)) => (rhs, label),
                _ => return None,
            };
            Some(Branch {
                subject: subject.syntax().text().to_string(),
                labels: vec![label],
            })
        }
        // `subject.equals("a")`（反过来写的 `"a".equals(x)` 对 null 安全，换成 switch 会抛异常，不处理）
        ast::Expr::MethodCall(call) if call.name_ref()?.text() == "equals" => {
            let receiver = call.receiver()?;
            let args: Vec<ast::Expr> = call.arg_list()?.args().collect();
            let [arg] = args.as_slice() else {
                return None;
            };
            let literal = strip_parens(arg.clone());
            let is_string = matches!(&literal, ast::Expr::Literal(l) if l.kind() == Some(SyntaxKind::STRING_LITERAL));
            if !is_string || !is_string_typed(ctx, &receiver) {
                return None;
            }
            Some(Branch {
                subject: receiver.syntax().text().to_string(),
                labels: vec![literal.syntax().text().to_string()],
            })
        }
        _ => None,
    }
}

fn strip_parens(mut expr: ast::Expr) -> ast::Expr {
    while let ast::Expr::ParenExpr(paren) = &expr {
        match paren.syntax().children().find_map(ast::Expr::cast) {
            Some(inner) => expr = inner,
            None => break,
        }
    }
    expr
}

pub(super) fn is_string_typed(ctx: &AssistCtx<'_>, expr: &ast::Expr) -> bool {
    ctx.sema
        .type_of_expr(ctx.file, expr)
        .and_then(|ty| ty.as_class())
        .and_then(|c| hir::class_data(ctx.db, c))
        .is_some_and(|d| &*d.binary_name == "java/lang/String")
}

/// 能当 case 标签的表达式：整数 / 字符字面量（可带负号），或者枚举常量（写成简单名）。
fn case_label(ctx: &AssistCtx<'_>, expr: &ast::Expr) -> Option<String> {
    let expr = strip_parens(expr.clone());
    match &expr {
        ast::Expr::Literal(lit)
            if matches!(
                lit.kind(),
                Some(SyntaxKind::INT_LITERAL | SyntaxKind::CHAR_LITERAL)
            ) =>
        {
            Some(expr.syntax().text().to_string())
        }
        ast::Expr::UnaryExpr(_) => {
            let text = expr.syntax().text().to_string();
            let inner = text.strip_prefix('-')?.trim();
            (!inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit() || c == '_'))
                .then_some(text)
        }
        ast::Expr::NameExpr(_) | ast::Expr::FieldAccess(_) => {
            let name = expr
                .syntax()
                .children()
                .filter_map(ast::NameRef::cast)
                .last()?;
            match ctx.sema.classify_name_ref(ctx.file, &name)? {
                hir::Definition::Field(field) => {
                    let data = hir::class_data(ctx.db, field.class)?;
                    let f = data.fields.get(field.idx as usize)?;
                    f.is_enum_constant().then(|| f.name.to_string())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// 块里有没有会跳出 switch 的 `break`（不在内层循环 / switch 里的）。
fn has_escaping_break(body: &SyntaxNode) -> bool {
    body.descendants()
        .filter(|d| d.kind() == SyntaxKind::BREAK_STMT)
        .filter(|b| ast::BreakStmt::cast(b.clone()).is_some_and(|b| b.label().is_none()))
        .any(|b| {
            !b.ancestors().take_while(|a| a != body).any(|a| {
                matches!(
                    a.kind(),
                    SyntaxKind::WHILE_STMT
                        | SyntaxKind::DO_WHILE_STMT
                        | SyntaxKind::FOR_STMT
                        | SyntaxKind::FOREACH_STMT
                        | SyntaxKind::SWITCH_STMT
                        | SyntaxKind::SWITCH_EXPR
                        | SyntaxKind::LAMBDA_EXPR
                        | SyntaxKind::CLASS_BODY
                )
            })
        })
}

/// `if (x == 1) … else if (x == 2) … else …` → `switch (x) { case 1 -> … }`（需要 Java 14+）。
pub(super) fn replace_if_with_switch(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = ctx.root.token_at_offset(ctx.range.start()).right_biased() else {
        return;
    };
    if token.kind() != SyntaxKind::IF_KW {
        return;
    }
    let Some(head) = token.parent().and_then(ast::IfStmt::cast) else {
        return;
    };
    // 只从链的开头提供
    if head.syntax().parent().and_then(ast::IfStmt::cast).is_some() {
        return;
    }
    if ide_diagnostics::language_level(ctx.db, ctx.file).is_some_and(|l| l < 14) {
        return;
    }
    let mut branches: Vec<(Branch, ast::Stmt)> = Vec::new();
    let mut otherwise: Option<ast::Stmt> = None;
    let mut current = head.clone();
    loop {
        let (Some(cond), Some(then)) = (current.condition(), current.then_branch()) else {
            return;
        };
        let Some(branch) = branch_of(ctx, &cond) else {
            return;
        };
        branches.push((branch, then));
        match current.else_branch() {
            Some(ast::Stmt::IfStmt(next)) => current = next,
            Some(other) => {
                otherwise = Some(other);
                break;
            }
            None => break,
        }
    }
    if branches.len() < 2 {
        return;
    }
    let subject = branches[0].0.subject.clone();
    let mut seen: Vec<&str> = Vec::new();
    for (branch, body) in &branches {
        if branch.subject != subject || has_escaping_break(body.syntax()) {
            return;
        }
        for label in &branch.labels {
            if seen.contains(&label.as_str()) {
                return;
            }
            seen.push(label);
        }
    }
    if otherwise
        .as_ref()
        .is_some_and(|o| has_escaping_break(o.syntax()))
    {
        return;
    }
    let indent = ctx.indent_at(head.syntax().text_range().start());
    let render_body = |stmt: &ast::Stmt| -> String {
        match stmt {
            ast::Stmt::Block(block) => {
                let stmts: Vec<ast::Stmt> = block.statements().collect();
                if let [single] = stmts.as_slice() {
                    if matches!(single, ast::Stmt::ExprStmt(_) | ast::Stmt::ThrowStmt(_))
                        && !single.syntax().text().to_string().contains('\n')
                    {
                        return single.syntax().text().to_string();
                    }
                }
                let text = block.syntax().text().to_string();
                let inner = text.trim_start_matches('{').trim_end_matches('}').trim();
                format!(
                    "{{\n{indent}        {}\n{indent}    }}",
                    inner.replace('\n', "\n    ")
                )
            }
            other => {
                let text = other.syntax().text().to_string();
                if matches!(other, ast::Stmt::ExprStmt(_) | ast::Stmt::ThrowStmt(_))
                    && !text.contains('\n')
                {
                    text
                } else {
                    format!(
                        "{{\n{indent}        {}\n{indent}    }}",
                        text.replace('\n', "\n    ")
                    )
                }
            }
        }
    };
    let mut text = format!("switch ({subject}) {{\n");
    for (branch, body) in &branches {
        text.push_str(&format!(
            "{indent}    case {} -> {}\n",
            branch.labels.join(", "),
            render_body(body)
        ));
    }
    if let Some(otherwise) = &otherwise {
        text.push_str(&format!(
            "{indent}    default -> {}\n",
            render_body(otherwise)
        ));
    }
    text.push_str(&format!("{indent}}}"));
    out.push(ctx.assist(
        "replace_if_with_switch",
        base_db::trs!("把 if 换成 switch", "Replace 'if' with 'switch'"),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(head.syntax().text_range(), text)],
    ));
}
