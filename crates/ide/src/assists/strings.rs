//! 字符串拼接的改写：
//!
//! - `"a " + x + " b"` → `String.format("a %s b", x)`（整数用 `%d`）；
//! - 只由字符串字面量拼成、带换行的文本 → 文本块（需要 Java 15+）。

use hir::{PrimitiveType, Ty};
use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange,
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

/// StringBuilder 上和 String 同名、同义的方法：接收者不用转成字符串。
const BUILDER_COMPATIBLE: &[&str] = &[
    "length",
    "charAt",
    "indexOf",
    "lastIndexOf",
    "substring",
    "subSequence",
];

/// 追加的内容：拼接展开成各个操作数，别的表达式整个追加。
fn append_chain(parts: &[ast::Expr]) -> String {
    parts
        .iter()
        .map(|p| format!(".append({})", p.syntax().text()))
        .collect()
}

/// 循环里拼接的字符串变量改成 `StringBuilder`：`s += x` → `s.append(x)`，
/// 其它读取的地方加 `.toString()`。
pub(super) fn concat_loop_to_builder(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    use hir::Definition;
    use ide_db::search::references_in_file;

    let db = ctx.db;
    let diagnostics = ide_diagnostics::string_concat_in_loops(db, ctx.file);
    let Some(diagnostic) = diagnostics
        .iter()
        .find(|d| d.range.contains_inclusive(ctx.range.start()))
    else {
        return;
    };
    let Some(name_ref) =
        syntax::algo::find_node_at_range::<ast::NameRef>(&ctx.root, diagnostic.range)
    else {
        return;
    };
    let Some(def @ Definition::Local(local)) = ctx.sema.classify_name_ref(ctx.file, &name_ref)
    else {
        return;
    };
    let data = hir::infer(db, local.body).local(local.id).clone();
    let name = data.name.to_string();
    // 声明：`String s = …;`，只有一个变量
    let Some(decl_name) = syntax::algo::find_node_at_range::<ast::Name>(&ctx.root, data.decl_range)
    else {
        return;
    };
    let Some(declarator) = decl_name
        .syntax()
        .parent()
        .and_then(ast::VariableDeclarator::cast)
    else {
        return;
    };
    let Some(decl) = declarator
        .syntax()
        .parent()
        .and_then(ast::LocalVarDecl::cast)
    else {
        return;
    };
    let Some(ty) = decl.ty() else { return };
    if ty.syntax().text() != "String" || decl.declarators().count() != 1 {
        return;
    }
    let Some(init) = declarator.initializer() else {
        return;
    };
    let init_text = match &init {
        ast::Expr::Literal(lit) if lit.syntax().text() == "\"\"" => {
            "new StringBuilder()".to_owned()
        }
        other => format!("new StringBuilder({})", other.syntax().text()),
    };
    let mut edits = vec![
        TextEdit::replace(ty.syntax().text_range(), "StringBuilder"),
        TextEdit::replace(init.syntax().text_range(), init_text),
    ];

    let is_self = |e: &ast::Expr| -> bool {
        matches!(e, ast::Expr::NameExpr(n) if n.name_ref().is_some_and(|r| {
            matches!(ctx.sema.classify_name_ref(ctx.file, &r), Some(d) if d == def)
        }))
    };
    let mut covered: Vec<TextRange> = Vec::new();
    let mut reads: Vec<TextRange> = Vec::new();
    for reference in references_in_file(db, ctx.file, &def, &name) {
        if covered.iter().any(|c| c.contains_range(reference.range)) {
            continue;
        }
        let Some(r) = syntax::algo::find_node_at_range::<ast::NameRef>(&ctx.root, reference.range)
        else {
            return;
        };
        let Some(expr) = r.syntax().parent().and_then(ast::NameExpr::cast) else {
            return;
        };
        let parent = expr.syntax().parent();
        // 写：必须是单独成句的 `s += …` 或 `s = s + …`
        if let Some(assign) = parent.clone().and_then(ast::AssignExpr::cast) {
            if assign.lhs().map(|l| l.syntax().clone()).as_ref() == Some(expr.syntax()) {
                if assign
                    .syntax()
                    .parent()
                    .is_none_or(|p| p.kind() != SyntaxKind::EXPR_STMT)
                {
                    return;
                }
                let Some(rhs) = assign.rhs() else { return };
                let parts: Vec<ast::Expr> = match assign.op_token().map(|t| t.kind()) {
                    Some(SyntaxKind::PLUS_EQ) => match &rhs {
                        ast::Expr::BinaryExpr(b)
                            if b.op_kind() == Some(SyntaxKind::PLUS)
                                && is_string_typed(ctx, &rhs) =>
                        {
                            match operands(ctx, b) {
                                Some(parts) => parts,
                                None => return,
                            }
                        }
                        other => vec![other.clone()],
                    },
                    Some(SyntaxKind::EQ) => {
                        let ast::Expr::BinaryExpr(b) = &rhs else {
                            return;
                        };
                        let Some(parts) = operands(ctx, b) else {
                            return;
                        };
                        match parts.split_first() {
                            Some((first, rest)) if is_self(first) && !rest.is_empty() => {
                                rest.to_vec()
                            }
                            _ => return,
                        }
                    }
                    _ => return,
                };
                // 追加的内容里不能再用到这个变量（还得转字符串，容易出错）
                if parts.iter().any(|p| {
                    p.syntax()
                        .descendants()
                        .filter_map(ast::NameExpr::cast)
                        .any(|n| is_self(&ast::Expr::NameExpr(n)))
                }) {
                    return;
                }
                edits.push(TextEdit::replace(
                    assign.syntax().text_range(),
                    format!("{name}{}", append_chain(&parts)),
                ));
                covered.push(assign.syntax().text_range());
                continue;
            }
        }
        // 读：StringBuilder 上有同义方法的直接用，其余转成字符串
        let compatible = parent
            .and_then(ast::MethodCall::cast)
            .filter(|call| {
                call.receiver().map(|r| r.syntax().clone()).as_ref() == Some(expr.syntax())
            })
            .and_then(|call| call.name_ref())
            .is_some_and(|n| BUILDER_COMPATIBLE.contains(&n.text().as_str()));
        if !compatible {
            reads.push(expr.syntax().text_range());
        }
    }
    for range in reads {
        if covered.iter().any(|c| c.contains_range(range)) {
            continue;
        }
        edits.push(TextEdit::replace(range, format!("{name}.toString()")));
    }
    edits.sort_by_key(|e| e.range.start());
    out.push(ctx.assist(
        "concat_loop_to_builder",
        base_db::tr!(
            "把 `{name}` 改成 `StringBuilder`",
            "Convert `{name}` to `StringBuilder`"
        ),
        AssistKind::RefactorRewrite,
        edits,
    ));
}
