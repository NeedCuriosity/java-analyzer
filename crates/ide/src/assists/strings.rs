//! 字符串拼接的改写：
//!
//! - `"a " + x + " b"` → `String.format("a %s b", x)`（整数用 `%d`）；
//! - 只由字符串字面量拼成、带换行的文本 → 文本块（需要 Java 15+）。

use hir::{PrimitiveType, Ty};
use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind,
};

use super::{surround::is_string_typed, Assist, AssistCtx, AssistKind};

/// 光标所在的、最外层的字符串 `+` 拼接。
fn concatenation_at(ctx: &AssistCtx<'_>) -> Option<ast::BinaryExpr> {
    let token = ctx.root.token_at_offset(ctx.range.start()).right_biased()?;
    let mut found: Option<ast::BinaryExpr> = None;
    for node in token.parent_ancestors() {
        match ast::BinaryExpr::cast(node.clone()) {
            Some(binary)
                if binary.op_kind() == Some(SyntaxKind::PLUS)
                    && is_string_typed(ctx, &ast::Expr::BinaryExpr(binary.clone())) =>
            {
                found = Some(binary);
            }
            // 括号、字面量、名字之类的表达式继续往上找；遇到别的就停
            _ if found.is_none()
                && (ast::Expr::can_cast(node.kind()) || node.kind() == SyntaxKind::NAME_REF) => {}
            _ => break,
        }
    }
    found
}

/// 拼接展开成从左到右的操作数。左边是数值加法（`1 + 2 + "a"`）时那一段整体算一个。
fn operands(ctx: &AssistCtx<'_>, concat: &ast::BinaryExpr) -> Option<Vec<ast::Expr>> {
    let mut res = vec![concat.rhs()?];
    let mut lhs = concat.lhs()?;
    loop {
        match &lhs {
            ast::Expr::BinaryExpr(inner)
                if inner.op_kind() == Some(SyntaxKind::PLUS) && is_string_typed(ctx, &lhs) =>
            {
                res.push(inner.rhs()?);
                let next = inner.lhs()?;
                lhs = next;
            }
            _ => {
                res.push(lhs);
                break;
            }
        }
    }
    res.reverse();
    Some(res)
}

fn string_literal(expr: &ast::Expr) -> Option<String> {
    let ast::Expr::Literal(lit) = expr else {
        return None;
    };
    if lit.kind() != Some(SyntaxKind::STRING_LITERAL) {
        return None;
    }
    let text = lit.token()?.text().to_string();
    Some(text.strip_prefix('"')?.strip_suffix('"')?.to_owned())
}

pub(super) fn concat_to_format(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(concat) = concatenation_at(ctx) else {
        return;
    };
    let Some(parts) = operands(ctx, &concat) else {
        return;
    };
    let mut format = String::new();
    let mut args: Vec<String> = Vec::new();
    for part in &parts {
        if let Some(raw) = string_literal(part) {
            format.push_str(&raw.replace('%', "%%"));
            continue;
        }
        let integral = ctx.sema.type_of_expr(ctx.file, part).is_some_and(|ty| {
            let primitive = match &ty {
                Ty::Primitive(p) => Some(*p),
                other => other.unboxed(ctx.db),
            };
            matches!(
                primitive,
                Some(
                    PrimitiveType::Byte
                        | PrimitiveType::Short
                        | PrimitiveType::Int
                        | PrimitiveType::Long
                )
            )
        });
        format.push_str(if integral { "%d" } else { "%s" });
        let text = match part {
            ast::Expr::ParenExpr(p) => p
                .expr()
                .map(|e| e.syntax().text().to_string())
                .unwrap_or_else(|| part.syntax().text().to_string()),
            _ => part.syntax().text().to_string(),
        };
        args.push(text);
    }
    // 至少要有一个字面量和一个别的操作数才有意义
    if args.is_empty() || args.len() == parts.len() {
        return;
    }
    let replacement = format!("String.format(\"{format}\", {})", args.join(", "));
    out.push(ctx.assist(
        "concat_to_format",
        base_db::trs!(
            "把拼接改写成 `String.format()`",
            "Replace concatenation with `String.format()`"
        ),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(concat.syntax().text_range(), replacement)],
    ));
}

/// 字面量原文里的转义：`\n` 变成真正的换行，`\"` 变成引号，其它转义原样保留。
fn text_block_content(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('"') => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

pub(super) fn concat_to_text_block(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    if ide_diagnostics::language_level(ctx.db, ctx.file).is_some_and(|l| l < 15) {
        return;
    }
    // 拼接，或者单个字符串字面量
    let target: ast::Expr = match concatenation_at(ctx) {
        Some(concat) => ast::Expr::BinaryExpr(concat),
        None => {
            let Some(token) = ctx.root.token_at_offset(ctx.range.start()).right_biased() else {
                return;
            };
            if token.kind() != SyntaxKind::STRING_LITERAL {
                return;
            }
            let Some(lit) = token.parent().and_then(ast::Expr::cast) else {
                return;
            };
            lit
        }
    };
    let parts = match &target {
        ast::Expr::BinaryExpr(concat) => match operands(ctx, concat) {
            Some(parts) => parts,
            None => return,
        },
        other => vec![other.clone()],
    };
    let mut content = String::new();
    for part in &parts {
        let Some(raw) = string_literal(part) else {
            return;
        };
        content.push_str(&text_block_content(&raw));
    }
    let lines: Vec<&str> = content.split('\n').collect();
    // 只有一行（或只有末尾一个换行）时文本块没有好处
    if lines.len() < 2 || (lines.len() == 2 && lines[1].is_empty()) {
        return;
    }
    let ends_with_newline = content.ends_with('\n');
    let body_lines = if ends_with_newline {
        &lines[..lines.len() - 1]
    } else {
        &lines[..]
    };
    // 不以换行结尾时没有单独一行的结束分隔符：所有行都以空白开头的话公共缩进会被吃掉
    if !ends_with_newline
        && body_lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .all(|l| l.starts_with([' ', '\t']))
    {
        return;
    }
    let indent = format!(
        "{}    ",
        ctx.indent_at(target.syntax().text_range().start())
    );
    let mut block = String::from("\"\"\"\n");
    let count = body_lines.len();
    for (i, line) in body_lines.iter().enumerate() {
        // 三个连续引号要转义；行尾空白会被去掉，换成 `\s`
        let mut line = line.replace("\"\"\"", "\"\"\\\"");
        if line.ends_with(' ') {
            line.pop();
            line.push_str("\\s");
        }
        let last = i + 1 == count;
        if last && !ends_with_newline && line.ends_with('"') {
            line.pop();
            line.push_str("\\\"");
        }
        if !line.is_empty() {
            block.push_str(&indent);
            block.push_str(&line);
        }
        if !last || ends_with_newline {
            block.push('\n');
        }
    }
    if ends_with_newline {
        block.push_str(&indent);
    }
    block.push_str("\"\"\"");
    out.push(ctx.assist(
        "concat_to_text_block",
        base_db::trs!("改写成文本块", "Replace with text block"),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(target.syntax().text_range(), block)],
    ));
}
