//! 小型的代码改写（意图操作）：加花括号、拆分 / 合并声明、翻转比较、三元表达式和 if 互转、
//! lambda 和方法引用互转、转成增强 for、switch 转成 `->` 形式、给构造器形参创建字段。
//!
//! 都只改当前文件，大多只看语法；要语义的（方法引用、增强 for 的元素类型）在拿不准时不提供。

use hir::{class_data, Definition, Ty};
use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize,
};

use super::{fixes::import_for, Assist, AssistCtx, AssistKind};

fn cursor_token(ctx: &AssistCtx<'_>) -> Option<SyntaxToken> {
    match ctx.root.token_at_offset(ctx.range.start()) {
        syntax::TokenAtOffset::None => None,
        syntax::TokenAtOffset::Single(t) => Some(t),
        // 两个 token 的交界处：优先非空白的那个
        syntax::TokenAtOffset::Between(left, right) => {
            if right.kind().is_trivia() {
                Some(left)
            } else {
                Some(right)
            }
        }
    }
}

/// 节点之前最后一个非空白、非注释的 token。
fn prev_significant(node: &SyntaxNode) -> Option<SyntaxToken> {
    let mut token = node.first_token()?.prev_token();
    while let Some(t) = token {
        if !t.kind().is_trivia() {
            return Some(t);
        }
        token = t.prev_token();
    }
    None
}

/// `if` / `else` / `for` / `while` / `do` 的不带花括号的分支：加上花括号。
pub(super) fn add_braces(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    let Some(stmt) = token.parent_ancestors().find(|n| {
        matches!(
            n.kind(),
            SyntaxKind::IF_STMT
                | SyntaxKind::FOR_STMT
                | SyntaxKind::FOREACH_STMT
                | SyntaxKind::WHILE_STMT
                | SyntaxKind::DO_WHILE_STMT
        )
    }) else {
        return;
    };
    let bodies: Vec<ast::Stmt> = match stmt.kind() {
        SyntaxKind::IF_STMT => {
            let it = ast::IfStmt::cast(stmt.clone()).unwrap();
            let mut res: Vec<ast::Stmt> = it.then_branch().into_iter().collect();
            // `else if` 不加
            if let Some(other) = it.else_branch() {
                if !matches!(other, ast::Stmt::IfStmt(_)) {
                    res.push(other);
                }
            }
            res
        }
        SyntaxKind::FOR_STMT => ast::ForStmt::cast(stmt.clone())
            .and_then(|it| it.body())
            .into_iter()
            .collect(),
        SyntaxKind::FOREACH_STMT => ast::ForeachStmt::cast(stmt.clone())
            .and_then(|it| it.body())
            .into_iter()
            .collect(),
        SyntaxKind::WHILE_STMT => ast::WhileStmt::cast(stmt.clone())
            .and_then(|it| it.body())
            .into_iter()
            .collect(),
        _ => ast::DoWhileStmt::cast(stmt.clone())
            .and_then(|it| it.body())
            .into_iter()
            .collect(),
    };
    let unbraced: Vec<ast::Stmt> = bodies
        .into_iter()
        .filter(|b| !matches!(b, ast::Stmt::Block(_) | ast::Stmt::EmptyStmt(_)))
        .collect();
    if unbraced.is_empty() {
        return;
    }
    // 光标在语句头上或者在某个不带花括号的分支里
    let offset = ctx.range.start();
    let in_body = unbraced
        .iter()
        .any(|b| b.syntax().text_range().contains_inclusive(offset));
    let header_end = unbraced[0].syntax().text_range().start();
    if !in_body && offset > header_end {
        // 光标在 `else` 关键字上也算
        let on_else = token.kind() == SyntaxKind::ELSE_KW;
        if !on_else {
            return;
        }
    }
    let indent = ctx.indent_at(stmt.text_range().start());
    let mut edits = Vec::new();
    for body in &unbraced {
        let Some(prev) = prev_significant(body.syntax()) else {
            return;
        };
        let text = body.syntax().text().to_string();
        // 后面紧跟着 `else` / `while` 时把它们接到右花括号的同一行
        let mut end = body.syntax().text_range().end();
        let mut tail = "";
        let mut next = body.syntax().last_token().and_then(|t| t.next_token());
        while let Some(t) = next.clone().filter(|t| t.kind().is_trivia()) {
            if t.kind() != SyntaxKind::WHITESPACE {
                break;
            }
            next = t.next_token();
        }
        if let Some(t) =
            next.filter(|t| matches!(t.kind(), SyntaxKind::ELSE_KW | SyntaxKind::WHILE_KW))
        {
            end = t.text_range().start();
            tail = " ";
        }
        edits.push(TextEdit::replace(
            TextRange::new(prev.text_range().end(), end),
            format!(" {{\n{indent}    {}\n{indent}}}{tail}", text.trim()),
        ));
    }
    out.push(ctx.assist(
        "add_braces",
        base_db::trs!("加上花括号", "Add braces"),
        AssistKind::RefactorRewrite,
        edits,
    ));
}

/// `T x = e;` 拆成 `T x;` 和 `x = e;`。
pub(super) fn split_declaration(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    let Some(decl) = token.parent_ancestors().find_map(ast::LocalVarDecl::cast) else {
        return;
    };
    // 只在声明的头部（初值之前）提供
    let mut declarators = decl.declarators();
    let (Some(declarator), None) = (declarators.next(), declarators.next()) else {
        return;
    };
    let (Some(name), Some(init), Some(ty)) =
        (declarator.name(), declarator.initializer(), decl.ty())
    else {
        return;
    };
    if ctx.range.start() > init.syntax().text_range().start() {
        return;
    }
    let ty_text = ty.syntax().text().to_string();
    if ty_text == "var"
        || decl
            .syntax()
            .parent()
            .is_some_and(|p| p.kind() != SyntaxKind::BLOCK)
    {
        return;
    }
    let rhs = if init.syntax().kind() == SyntaxKind::ARRAY_INITIALIZER {
        if declarator.extra_dims() > 0 {
            return;
        }
        format!("new {ty_text} {}", init.syntax().text())
    } else {
        init.syntax().text().to_string()
    };
    let indent = ctx.indent_at(decl.syntax().text_range().start());
    let edit = TextEdit::replace(
        TextRange::new(
            name.syntax().text_range().end(),
            decl.syntax().text_range().end(),
        ),
        format!(";\n{indent}{} = {rhs};", name.text()),
    );
    out.push(ctx.assist(
        "split_declaration",
        base_db::trs!("拆分声明和赋值", "Split declaration and assignment"),
        AssistKind::RefactorRewrite,
        vec![edit],
    ));
}

/// `T x;` 后面紧跟着 `x = e;`：合并成 `T x = e;`。
pub(super) fn join_declaration(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    let Some(decl) = token.parent_ancestors().find_map(ast::LocalVarDecl::cast) else {
        return;
    };
    let mut declarators = decl.declarators();
    let (Some(declarator), None) = (declarators.next(), declarators.next()) else {
        return;
    };
    let Some(name) = declarator.name() else {
        return;
    };
    if declarator.initializer().is_some() {
        return;
    }
    let Some(next) = decl.syntax().next_sibling() else {
        return;
    };
    let Some(assign) = ast::ExprStmt::cast(next.clone()).and_then(|s| s.expr()) else {
        return;
    };
    let ast::Expr::AssignExpr(assign) = assign else {
        return;
    };
    let is_plain_assign = assign.op_token().is_some_and(|t| t.text() == "=");
    let (Some(ast::Expr::NameExpr(lhs)), Some(rhs)) = (assign.lhs(), assign.rhs()) else {
        return;
    };
    if !is_plain_assign || lhs.syntax().text() != name.text().as_str() {
        return;
    }
    let edit = TextEdit::replace(
        TextRange::new(name.syntax().text_range().end(), next.text_range().end()),
        format!(" = {};", rhs.syntax().text()),
    );
    out.push(ctx.assist(
        "join_declaration",
        base_db::trs!("合并声明和赋值", "Join declaration and assignment"),
        AssistKind::RefactorRewrite,
        vec![edit],
    ));
}

/// 翻转比较：`a == b` → `b == a`，`a < b` → `b > a`；`a.equals(b)` → `b.equals(a)`。
pub(super) fn flip_binary(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    if let Some(binary) = token.parent().and_then(ast::BinaryExpr::cast) {
        let Some(op) = binary.op_token() else {
            return;
        };
        if op.text_range() != token.text_range() {
            return;
        }
        let flipped = match op.text() {
            "==" => "==",
            "!=" => "!=",
            "<" => ">",
            ">" => "<",
            "<=" => ">=",
            ">=" => "<=",
            "&&" => "&&",
            "||" => "||",
            _ => return,
        };
        let (Some(lhs), Some(rhs)) = (binary.lhs(), binary.rhs()) else {
            return;
        };
        // `a == b == c` 这类链只翻转最外层两边的写法会改变结合方式，不提供
        if matches!(rhs, ast::Expr::BinaryExpr(_)) && !matches!(op.text(), "&&" | "||") {
            return;
        }
        let text = format!("{} {flipped} {}", rhs.syntax().text(), lhs.syntax().text());
        out.push(ctx.assist(
            "flip_binary",
            base_db::tr!("翻转 `{}`", "Flip `{}`", op.text()),
            AssistKind::RefactorRewrite,
            vec![TextEdit::replace(binary.syntax().text_range(), text)],
        ));
        return;
    }
    // `a.equals(b)`
    let Some(call) = token
        .parent()
        .and_then(ast::NameRef::cast)
        .and_then(|n| n.syntax().parent())
        .and_then(ast::MethodCall::cast)
    else {
        return;
    };
    if call.name_ref().map(|n| n.text()).as_deref() != Some("equals") {
        return;
    }
    let (Some(receiver), Some(args)) = (call.receiver(), call.arg_list()) else {
        return;
    };
    let mut args = args.args();
    let (Some(arg), None) = (args.next(), args.next()) else {
        return;
    };
    // 实参是基本类型或者 null 时翻过来就编不过了
    match ctx.sema.type_of_expr(ctx.file, &arg) {
        Some(Ty::Class(..)) => {}
        _ => return,
    }
    let arg_text = match &arg {
        ast::Expr::NameExpr(_)
        | ast::Expr::FieldAccess(_)
        | ast::Expr::MethodCall(_)
        | ast::Expr::Literal(_)
        | ast::Expr::ThisExpr(_)
        | ast::Expr::ParenExpr(_) => arg.syntax().text().to_string(),
        _ => format!("({})", arg.syntax().text()),
    };
    out.push(ctx.assist(
        "flip_equals",
        base_db::trs!("翻转 `equals()`", "Flip `equals()`"),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(
            call.syntax().text_range(),
            format!("{arg_text}.equals({})", receiver.syntax().text()),
        )],
    ));
}

/// 去掉一层外面的花括号：只有一条语句的块返回那条语句。
fn single_statement(stmt: &ast::Stmt) -> Option<ast::Stmt> {
    match stmt {
        ast::Stmt::Block(block) => {
            let mut stmts = block.statements();
            match (stmts.next(), stmts.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
        other => Some(other.clone()),
    }
}

/// `return c ? a : b;` / `x = c ? a : b;` / `T x = c ? a : b;` 改成 if / else。
pub(super) fn ternary_to_if(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    // 光标在 `?:` 里，或者在值是 `?:` 的 return / 赋值 / 声明语句上
    let cond = token
        .parent_ancestors()
        .find_map(ast::ConditionalExpr::cast)
        .or_else(|| {
            let stmt = token.parent_ancestors().find(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::RETURN_STMT | SyntaxKind::EXPR_STMT | SyntaxKind::LOCAL_VAR_DECL
                )
            })?;
            let value = match stmt.kind() {
                SyntaxKind::RETURN_STMT => ast::ReturnStmt::cast(stmt)?.expr()?,
                SyntaxKind::EXPR_STMT => match ast::ExprStmt::cast(stmt)?.expr()? {
                    ast::Expr::AssignExpr(a) => a.rhs()?,
                    _ => return None,
                },
                _ => {
                    let decl = ast::LocalVarDecl::cast(stmt)?;
                    let mut declarators = decl.declarators();
                    let declarator = declarators.next()?;
                    declarators.next().is_none().then_some(())?;
                    declarator.initializer()?
                }
            };
            match value {
                ast::Expr::ConditionalExpr(c) => Some(c),
                _ => None,
            }
        });
    let Some(cond) = cond else {
        return;
    };
    let (Some(c), Some(a), Some(b)) = (cond.condition(), cond.then_expr(), cond.else_expr()) else {
        return;
    };
    let parent = cond.syntax().parent();
    let Some(parent) = parent else {
        return;
    };
    let indent;
    type MakeStmt = Box<dyn Fn(&str) -> String>;
    let (stmt_range, prefix, make): (TextRange, String, MakeStmt) = match parent.kind() {
        SyntaxKind::RETURN_STMT => {
            indent = ctx.indent_at(parent.text_range().start());
            (
                parent.text_range(),
                String::new(),
                Box::new(|v: &str| format!("return {v};")),
            )
        }
        SyntaxKind::ASSIGN_EXPR => {
            let assign = ast::AssignExpr::cast(parent.clone()).unwrap();
            let Some(stmt) = parent
                .parent()
                .filter(|p| p.kind() == SyntaxKind::EXPR_STMT)
            else {
                return;
            };
            let (Some(lhs), Some(op)) = (assign.lhs(), assign.op_token()) else {
                return;
            };
            if assign.rhs().map(|r| r.syntax().clone()) != Some(cond.syntax().clone()) {
                return;
            }
            indent = ctx.indent_at(stmt.text_range().start());
            let lhs = lhs.syntax().text().to_string();
            let op = op.text().to_string();
            (
                stmt.text_range(),
                String::new(),
                Box::new(move |v: &str| format!("{lhs} {op} {v};")),
            )
        }
        SyntaxKind::VARIABLE_DECLARATOR => {
            let declarator = ast::VariableDeclarator::cast(parent.clone()).unwrap();
            let Some(decl) = parent.parent().and_then(ast::LocalVarDecl::cast) else {
                return;
            };
            if decl.declarators().count() != 1
                || decl
                    .syntax()
                    .parent()
                    .is_some_and(|p| p.kind() != SyntaxKind::BLOCK)
            {
                return;
            }
            let (Some(name), Some(ty)) = (declarator.name(), decl.ty()) else {
                return;
            };
            if ty.syntax().text() == "var" {
                return;
            }
            indent = ctx.indent_at(decl.syntax().text_range().start());
            let head = ctx.text[usize::from(decl.syntax().text_range().start())
                ..usize::from(name.syntax().text_range().end())]
                .to_owned();
            let name = name.text().to_string();
            (
                decl.syntax().text_range(),
                format!("{head};\n{indent}"),
                Box::new(move |v: &str| format!("{name} = {v};")),
            )
        }
        _ => return,
    };
    let text = format!(
        "{prefix}if ({}) {{\n{indent}    {}\n{indent}}} else {{\n{indent}    {}\n{indent}}}",
        c.syntax().text(),
        make(&a.syntax().text().to_string()),
        make(&b.syntax().text().to_string()),
    );
    out.push(ctx.assist(
        "ternary_to_if",
        base_db::trs!("把 `?:` 换成 if / else", "Replace `?:` with if / else"),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(stmt_range, text)],
    ));
}

/// 两个分支都是 `return x;`（或者给同一个变量赋值）的 if / else 改成 `?:`。
pub(super) fn if_to_ternary(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    if token.kind() != SyntaxKind::IF_KW {
        return;
    }
    let Some(if_stmt) = token.parent().and_then(ast::IfStmt::cast) else {
        return;
    };
    let (Some(c), Some(then), Some(other)) = (
        if_stmt.condition(),
        if_stmt.then_branch().as_ref().and_then(single_statement),
        if_stmt.else_branch().as_ref().and_then(single_statement),
    ) else {
        return;
    };
    let wrap = |e: &ast::Expr| match e {
        ast::Expr::ConditionalExpr(_) | ast::Expr::AssignExpr(_) | ast::Expr::LambdaExpr(_) => {
            format!("({})", e.syntax().text())
        }
        _ => e.syntax().text().to_string(),
    };
    let cond_text = match &c {
        ast::Expr::ConditionalExpr(_) | ast::Expr::AssignExpr(_) => {
            format!("({})", c.syntax().text())
        }
        _ => c.syntax().text().to_string(),
    };
    let text = match (&then, &other) {
        (ast::Stmt::ReturnStmt(a), ast::Stmt::ReturnStmt(b)) => {
            let (Some(a), Some(b)) = (a.expr(), b.expr()) else {
                return;
            };
            format!("return {cond_text} ? {} : {};", wrap(&a), wrap(&b))
        }
        (ast::Stmt::ExprStmt(a), ast::Stmt::ExprStmt(b)) => {
            let (Some(ast::Expr::AssignExpr(a)), Some(ast::Expr::AssignExpr(b))) =
                (a.expr(), b.expr())
            else {
                return;
            };
            let (Some(la), Some(lb), Some(oa), Some(ob), Some(ra), Some(rb)) = (
                a.lhs(),
                b.lhs(),
                a.op_token(),
                b.op_token(),
                a.rhs(),
                b.rhs(),
            ) else {
                return;
            };
            if la.syntax().text() != lb.syntax().text() || oa.text() != ob.text() {
                return;
            }
            format!(
                "{} {} {cond_text} ? {} : {};",
                la.syntax().text(),
                oa.text(),
                wrap(&ra),
                wrap(&rb)
            )
        }
        _ => return,
    };
    out.push(ctx.assist(
        "if_to_ternary",
        base_db::trs!("把 if / else 换成 `?:`", "Replace if / else with `?:`"),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(if_stmt.syntax().text_range(), text)],
    ));
}

fn param_names(lambda: &ast::LambdaExpr) -> Option<Vec<String>> {
    lambda
        .param_list()?
        .params()
        .map(|p| p.name().map(|n| n.text().to_string()))
        .collect()
}

fn mentions_any(node: &SyntaxNode, names: &[String]) -> bool {
    node.descendants()
        .filter_map(ast::NameRef::cast)
        .any(|n| names.iter().any(|p| p == n.text().as_str()))
}

fn args_are(args: &ast::ArgList, names: &[String]) -> bool {
    let args: Vec<ast::Expr> = args.args().collect();
    args.len() == names.len()
        && args.iter().zip(names).all(|(arg, name)| {
            matches!(arg, ast::Expr::NameExpr(n) if n.syntax().text() == name.as_str())
        })
}

/// lambda 的体是单个表达式（或只有一条 `return e;` / `e;` 的块）时取出它。
fn lambda_body_expr(lambda: &ast::LambdaExpr) -> Option<ast::Expr> {
    if let Some(e) = lambda.body_expr() {
        return Some(e);
    }
    let block = lambda.body_block()?;
    let mut stmts = block.statements();
    match (stmts.next()?, stmts.next()) {
        (ast::Stmt::ReturnStmt(r), None) => r.expr(),
        (ast::Stmt::ExprStmt(s), None) => s.expr(),
        _ => None,
    }
}

/// 方法 `name` 在 `class` 里是否同时有 `arity` 个形参的静态和实例重载（这时 `T::m` 有歧义）。
fn ambiguous_method_ref(
    db: &dyn base_db::Db,
    class: hir::ClassId,
    name: &str,
    arity: usize,
) -> bool {
    let mut statics = false;
    let mut instances = false;
    for candidate in hir::all_methods(db, &hir::this_ty(db, class)) {
        let Some(data) = class_data(db, candidate.id.class) else {
            continue;
        };
        let Some(method) = data.methods.get(candidate.id.idx as usize) else {
            continue;
        };
        if method.name != name {
            continue;
        }
        // 静态方法收全部形参，实例方法的第一个形参是接收者
        if method.flags.is_static() && method.params.len() == arity {
            statics = true;
        }
        if !method.flags.is_static() && method.params.len() + 1 == arity {
            instances = true;
        }
    }
    statics && instances
}

/// 简单 lambda 换成方法引用：`x -> foo(x)` → `this::foo`，`s -> s.length()` → `String::length`，
/// `x -> new Foo(x)` → `Foo::new`。
pub(super) fn lambda_to_method_ref(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    let Some(lambda) = token.parent_ancestors().find_map(ast::LambdaExpr::cast) else {
        return;
    };
    // 只在 lambda 的形参和箭头上提供，免得嵌套的 lambda 里到处都是
    if lambda
        .arrow_token()
        .is_some_and(|arrow| ctx.range.start() > arrow.text_range().end())
    {
        return;
    }
    let Some(params) = param_names(&lambda) else {
        return;
    };
    let Some(body) = lambda_body_expr(&lambda) else {
        return;
    };
    let db = ctx.db;
    let mut edits = Vec::new();
    let replacement = match &body {
        ast::Expr::NewExpr(new) => {
            if new.body().is_some() || new.qualifier().is_some() {
                return;
            }
            let (Some(ast::Type::ClassType(ty)), Some(args)) = (new.ty(), new.arg_list()) else {
                return;
            };
            if !args_are(&args, &params) {
                return;
            }
            format!("{}::new", ty.segments().join("."))
        }
        ast::Expr::MethodCall(call) => {
            let (Some(name), Some(args)) = (call.name_ref(), call.arg_list()) else {
                return;
            };
            if call.type_arg_list().is_some() {
                return;
            }
            let Some(Definition::Method(method)) = ctx.sema.classify_name_ref(ctx.file, &name)
            else {
                return;
            };
            let Some(method_data) = class_data(db, method.class)
                .and_then(|d| d.methods.get(method.idx as usize).cloned())
            else {
                return;
            };
            match call.receiver() {
                None => {
                    if !args_are(&args, &params) {
                        return;
                    }
                    if method_data.flags.is_static() {
                        let Some(class) = class_data(db, method.class) else {
                            return;
                        };
                        format!("{}::{}", class.name, name.text())
                    } else {
                        format!("this::{}", name.text())
                    }
                }
                Some(receiver) => {
                    let first_is_receiver = matches!(&receiver,
                        ast::Expr::NameExpr(n) if params.first().is_some_and(|p| n.syntax().text() == p.as_str()));
                    if first_is_receiver {
                        // `s -> s.length()`：未绑定接收者的引用，类型取 lambda 第一个形参的类型
                        if !args_are(&args, &params[1..]) || method_data.flags.is_static() {
                            return;
                        }
                        let Some(Ty::Class(class, _)) = ctx.sema.type_of_expr(ctx.file, &receiver)
                        else {
                            return;
                        };
                        if ambiguous_method_ref(db, class, name.text().as_str(), params.len()) {
                            return;
                        }
                        let Some(data) = class_data(db, class) else {
                            return;
                        };
                        if let Some(import) = import_for(ctx, &Ty::class(class), lambda.syntax()) {
                            edits.push(import);
                        }
                        format!("{}::{}", data.name, name.text())
                    } else {
                        // 接收者在 lambda 外求值一次：只接受没有副作用的写法
                        let simple = match &receiver {
                            ast::Expr::NameExpr(_)
                            | ast::Expr::ThisExpr(_)
                            | ast::Expr::SuperExpr(_) => true,
                            ast::Expr::FieldAccess(f) => f.syntax().descendants().all(|n| {
                                matches!(
                                    n.kind(),
                                    SyntaxKind::FIELD_ACCESS
                                        | SyntaxKind::NAME_EXPR
                                        | SyntaxKind::NAME_REF
                                        | SyntaxKind::THIS_EXPR
                                )
                            }),
                            _ => false,
                        };
                        if !simple
                            || mentions_any(receiver.syntax(), &params)
                            || !args_are(&args, &params)
                        {
                            return;
                        }
                        // `Math.abs(x)` 这种静态调用：同名的实例方法会让 `Math::abs` 有歧义
                        if method_data.flags.is_static()
                            && ambiguous_method_ref(
                                db,
                                method.class,
                                name.text().as_str(),
                                params.len(),
                            )
                        {
                            return;
                        }
                        format!("{}::{}", receiver.syntax().text(), name.text())
                    }
                }
            }
        }
        _ => return,
    };
    edits.push(TextEdit::replace(lambda.syntax().text_range(), replacement));
    out.push(ctx.assist(
        "lambda_to_method_ref",
        base_db::trs!(
            "把 lambda 换成方法引用",
            "Replace lambda with method reference"
        ),
        AssistKind::RefactorRewrite,
        edits,
    ));
}

/// 方法引用换成 lambda：`String::length` → `s -> s.length()`。
pub(super) fn method_ref_to_lambda(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    let Some(method_ref) = token.parent_ancestors().find_map(ast::MethodRef::cast) else {
        return;
    };
    let db = ctx.db;
    let expr = ast::Expr::MethodRef(method_ref.clone());
    let Some(target) = ctx.sema.type_of_expr(ctx.file, &expr) else {
        return;
    };
    let Some(sam) = hir::functional_method(db, &target) else {
        return;
    };
    let param_tys = sam.param_tys(db);
    let arity = param_tys.len();
    // 形参名：按函数式接口方法的形参类型起名，重名时加序号
    let mut names: Vec<String> = Vec::new();
    for ty in &param_tys {
        let base = super::fixes::name_for_type(&ty.display(db));
        let mut name = base.clone();
        let mut n = 2;
        while names.contains(&name) {
            name = format!("{base}{n}");
            n += 1;
        }
        names.push(name);
    }
    let body = if method_ref.is_constructor() {
        let Some(ty) = method_ref.receiver_type() else {
            return;
        };
        if matches!(ty, ast::Type::ArrayType(_)) {
            format!(
                "new {}[{}]",
                ty.syntax().text().to_string().trim_end_matches("[]"),
                names.join(", ")
            )
        } else {
            format!("new {}({})", ty.syntax().text(), names.join(", "))
        }
    } else {
        let Some(name) = method_ref.name_ref() else {
            return;
        };
        let Some(Definition::Method(method)) = ctx.sema.classify_name_ref(ctx.file, &name) else {
            return;
        };
        let Some(method_data) =
            class_data(db, method.class).and_then(|d| d.methods.get(method.idx as usize).cloned())
        else {
            return;
        };
        let receiver_text = method_ref
            .receiver_expr()
            .map(|e| e.syntax().text().to_string())
            .or_else(|| {
                method_ref
                    .receiver_type()
                    .map(|t| t.syntax().text().to_string())
            });
        let Some(receiver_text) = receiver_text else {
            return;
        };
        // 接收者写的是类型：静态方法照原样调用；实例方法则第一个形参是接收者
        let receiver_is_type = method_ref.receiver_type().is_some()
            || method_ref.receiver_expr().is_some_and(|e| {
                matches!(&e, ast::Expr::NameExpr(n) if n.name_ref().is_some_and(|r| {
                    matches!(ctx.sema.classify_name_ref(ctx.file, &r), Some(Definition::Class(_)))
                })) || matches!(&e, ast::Expr::FieldAccess(_))
                    && ctx.sema.type_of_expr(ctx.file, &e).is_none()
            });
        if receiver_is_type && !method_data.flags.is_static() {
            if arity == 0 {
                return;
            }
            // 接收者形参按接收者的类型起名：`String::length` → `string -> string.length()`
            let receiver_name =
                super::fixes::name_for_type(receiver_text.rsplit('.').next().unwrap_or(""));
            if !names[1..].contains(&receiver_name) {
                names[0] = receiver_name;
            }
            format!("{}.{}({})", names[0], name.text(), names[1..].join(", "))
        } else {
            format!("{receiver_text}.{}({})", name.text(), names.join(", "))
        }
    };
    out.push(ctx.assist(
        "method_ref_to_lambda",
        base_db::trs!(
            "把方法引用换成 lambda",
            "Replace method reference with lambda"
        ),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(
            method_ref.syntax().text_range(),
            format!(
                "{} -> {body}",
                match arity {
                    1 => names[0].clone(),
                    _ => format!("({})", names.join(", ")),
                }
            ),
        )],
    ));
}

/// 由集合名起元素名：`names` → `name`，`entries` → `entry`，其它的叫 `item`。
fn element_name(collection: &str) -> String {
    let last = collection.rsplit('.').next().unwrap_or(collection);
    if let Some(stem) = last.strip_suffix("ies") {
        if !stem.is_empty() {
            return format!("{stem}y");
        }
    }
    if let Some(stem) = last.strip_suffix('s') {
        if stem.len() > 1 && !stem.ends_with('s') {
            return stem.to_owned();
        }
    }
    "item".to_owned()
}

/// 下标 for 循环改成增强 for：`for (int i = 0; i < a.length; i++)`，体里只以 `a[i]` /
/// `list.get(i)` 的形式用到 `i`。
pub(super) fn convert_to_enhanced_for(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    if token.kind() != SyntaxKind::FOR_KW {
        return;
    }
    let Some(for_stmt) = token.parent().and_then(ast::ForStmt::cast) else {
        return;
    };
    let (Some(init), Some(cond), Some(update), Some(body)) = (
        for_stmt.init().and_then(|i| i.local_var_decl()),
        for_stmt.condition(),
        for_stmt.update(),
        for_stmt.body(),
    ) else {
        return;
    };
    // int i = 0
    let mut declarators = init.declarators();
    let (Some(index_decl), None) = (declarators.next(), declarators.next()) else {
        return;
    };
    let (Some(index), Some(start)) = (index_decl.name(), index_decl.initializer()) else {
        return;
    };
    if start.syntax().text() != "0" {
        return;
    }
    let index = index.text().to_string();
    // i++ / ++i / i += 1
    let mut updates = update.exprs();
    let (Some(step), None) = (updates.next(), updates.next()) else {
        return;
    };
    let step_text: String = step
        .syntax()
        .text()
        .to_string()
        .split_whitespace()
        .collect();
    if ![
        format!("{index}++"),
        format!("++{index}"),
        format!("{index}+=1"),
    ]
    .contains(&step_text)
    {
        return;
    }
    // i < a.length / i < list.size()
    let ast::Expr::BinaryExpr(cond) = cond else {
        return;
    };
    if cond.op_token().map(|t| t.text().to_owned()).as_deref() != Some("<")
        || cond.lhs().map(|l| l.syntax().text().to_string()).as_deref() != Some(index.as_str())
    {
        return;
    }
    let Some(bound) = cond.rhs() else {
        return;
    };
    let (collection, is_array) = match &bound {
        ast::Expr::FieldAccess(f) if f.name_ref().is_some_and(|n| n.text() == "length") => {
            (f.receiver(), true)
        }
        ast::Expr::MethodCall(c)
            if c.name_ref().is_some_and(|n| n.text() == "size")
                && c.arg_list().is_some_and(|a| a.args().next().is_none()) =>
        {
            (c.receiver(), false)
        }
        _ => return,
    };
    let Some(collection) = collection else {
        return;
    };
    if !matches!(
        collection,
        ast::Expr::NameExpr(_) | ast::Expr::FieldAccess(_)
    ) {
        return;
    }
    let collection_text = collection.syntax().text().to_string();
    // 体里每个 `i` 都必须是 `a[i]` / `list.get(i)` 的下标
    let mut accesses: Vec<SyntaxNode> = Vec::new();
    for name_ref in body.syntax().descendants().filter_map(ast::NameRef::cast) {
        if name_ref.text() != index.as_str() {
            continue;
        }
        let Some(name_expr) = name_ref
            .syntax()
            .parent()
            .filter(|p| p.kind() == SyntaxKind::NAME_EXPR)
        else {
            continue;
        };
        let access = if is_array {
            name_expr
                .parent()
                .and_then(ast::ArrayAccess::cast)
                .and_then(|a| {
                    let ok = a.index().is_some_and(|i| i.syntax() == &name_expr)
                        && a.array()
                            .is_some_and(|arr| arr.syntax().text() == collection_text.as_str());
                    ok.then(|| a.syntax().clone())
                })
        } else {
            name_expr
                .parent()
                .filter(|p| p.kind() == SyntaxKind::ARG_LIST)
                .and_then(|args| args.parent())
                .and_then(ast::MethodCall::cast)
                .and_then(|call| {
                    let ok = call.name_ref().is_some_and(|n| n.text() == "get")
                        && call.arg_list().is_some_and(|a| a.args().count() == 1)
                        && call
                            .receiver()
                            .is_some_and(|r| r.syntax().text() == collection_text.as_str());
                    ok.then(|| call.syntax().clone())
                })
        };
        let Some(access) = access else {
            return;
        };
        // 不能是赋值的左边：`a[i] = x`
        if access
            .parent()
            .and_then(ast::AssignExpr::cast)
            .and_then(|a| a.lhs())
            .is_some_and(|lhs| lhs.syntax() == &access)
        {
            return;
        }
        accesses.push(access);
    }
    let Some(element_ty) = (if is_array {
        match ctx.sema.type_of_expr(ctx.file, &collection) {
            Some(Ty::Array(elem)) => Some((*elem).clone()),
            _ => None,
        }
    } else {
        accesses
            .first()
            .and_then(|a| ast::Expr::cast(a.clone()))
            .and_then(|e| ctx.sema.type_of_expr(ctx.file, &e))
    }) else {
        return;
    };
    if element_ty.contains_unknown() || matches!(element_ty, Ty::Null | Ty::Void) {
        return;
    }
    // 元素名不能和体里已有的名字冲突
    let mut name = element_name(&collection_text);
    let used: Vec<String> = body
        .syntax()
        .descendants()
        .filter_map(|n| {
            ast::Name::cast(n.clone())
                .map(|n| n.text().to_string())
                .or_else(|| ast::NameRef::cast(n).map(|n| n.text().to_string()))
        })
        .collect();
    while used.contains(&name) || name == index {
        name.push('_');
    }
    let mut edits = Vec::new();
    if let Some(import) = import_for(ctx, &element_ty, for_stmt.syntax()) {
        edits.push(import);
    }
    let header_end = prev_significant(body.syntax()).map(|t| t.text_range().end());
    let Some(header_end) = header_end else {
        return;
    };
    let for_start = for_stmt.syntax().text_range().start();
    edits.push(TextEdit::replace(
        TextRange::new(for_start, header_end),
        format!(
            "for ({} {name} : {collection_text})",
            element_ty.display(ctx.db)
        ),
    ));
    for access in accesses {
        edits.push(TextEdit::replace(access.text_range(), name.clone()));
    }
    out.push(ctx.assist(
        "convert_to_enhanced_for",
        base_db::trs!("改成增强 for 循环", "Convert to enhanced for loop"),
        AssistKind::RefactorRewrite,
        edits,
    ));
}

/// 体里有没有跳出这个 switch 的 `break`（不在内层的循环 / switch 里、不带标签的）。
fn breaks_out(stmt: &SyntaxNode) -> bool {
    fn walk(node: &SyntaxNode) -> bool {
        for child in node.children() {
            match child.kind() {
                SyntaxKind::BREAK_STMT => {
                    if ast::BreakStmt::cast(child.clone()).is_some_and(|b| b.label().is_none()) {
                        return true;
                    }
                }
                SyntaxKind::FOR_STMT
                | SyntaxKind::FOREACH_STMT
                | SyntaxKind::WHILE_STMT
                | SyntaxKind::DO_WHILE_STMT
                | SyntaxKind::SWITCH_STMT
                | SyntaxKind::SWITCH_EXPR
                | SyntaxKind::LAMBDA_EXPR
                | SyntaxKind::CLASS_BODY => {}
                _ => {
                    if walk(&child) {
                        return true;
                    }
                }
            }
        }
        false
    }
    if stmt.kind() == SyntaxKind::BREAK_STMT {
        return ast::BreakStmt::cast(stmt.clone()).is_some_and(|b| b.label().is_none());
    }
    walk(stmt)
}

fn completes_abruptly(stmt: &ast::Stmt) -> bool {
    matches!(
        stmt,
        ast::Stmt::ReturnStmt(_) | ast::Stmt::ThrowStmt(_) | ast::Stmt::ContinueStmt(_)
    ) || matches!(stmt, ast::Stmt::BreakStmt(b) if b.label().is_some())
}

/// 老式的 `case X:` switch 语句（每组都以 `break` / `return` / `throw` 结束，没有贯穿）改成
/// `case X ->` 形式。
pub(super) fn switch_to_arrow(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    if token.kind() != SyntaxKind::SWITCH_KW {
        return;
    }
    let Some(switch) = token.parent().and_then(ast::SwitchStmt::cast) else {
        return;
    };
    let Some(block) = switch.block() else {
        return;
    };
    let cases: Vec<ast::SwitchCase> = block.cases().collect();
    if cases.is_empty() || cases.iter().any(|c| c.is_arrow()) {
        return;
    }
    // 按组归并：没有语句的 case 和下一个 case 合在一起
    struct Group {
        labels: Vec<ast::SwitchLabel>,
        stmts: Vec<ast::Stmt>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut pending: Vec<ast::SwitchLabel> = Vec::new();
    for (idx, case) in cases.iter().enumerate() {
        let Some(label) = case.label() else {
            return;
        };
        pending.push(label);
        let stmts: Vec<ast::Stmt> = case.statements().collect();
        if stmts.is_empty() {
            if idx + 1 == cases.len() {
                groups.push(Group {
                    labels: std::mem::take(&mut pending),
                    stmts,
                });
            }
            continue;
        }
        let last = stmts.last().unwrap();
        let is_last_group = idx + 1 == cases.len();
        let mut body = stmts.clone();
        if matches!(last, ast::Stmt::BreakStmt(b) if b.label().is_none()) {
            body.pop();
        } else if !completes_abruptly(last) && !is_last_group {
            // 会贯穿到下一组
            return;
        }
        if body.iter().any(|s| breaks_out(s.syntax())) {
            return;
        }
        groups.push(Group {
            labels: std::mem::take(&mut pending),
            stmts: body,
        });
    }
    // 组里声明的局部变量在原来的写法里对后面的组可见，改写后就不可见了
    for (idx, group) in groups.iter().enumerate() {
        let declared: Vec<String> = group
            .stmts
            .iter()
            .filter_map(|s| match s {
                ast::Stmt::LocalVarDecl(d) => Some(d.declarators()),
                _ => None,
            })
            .flatten()
            .filter_map(|d| d.name().map(|n| n.text().to_string()))
            .collect();
        if !declared.is_empty()
            && groups[idx + 1..]
                .iter()
                .any(|g| g.stmts.iter().any(|s| mentions_any(s.syntax(), &declared)))
        {
            return;
        }
    }
    // `default` 和别的标签合在一起的写法改写后不合法（`case null, default` 除外）
    for group in &groups {
        if group.labels.len() > 1 && group.labels.iter().any(|l| l.is_default()) {
            return;
        }
    }
    let case_indent = ctx.indent_at(cases[0].syntax().text_range().start());
    let inner = format!("{case_indent}    ");
    let mut text = String::from("{\n");
    for group in &groups {
        text.push_str(&case_indent);
        if group.labels.iter().any(|l| l.is_default()) {
            text.push_str("default");
        } else {
            let mut items: Vec<String> = Vec::new();
            for label in &group.labels {
                for e in label.exprs() {
                    items.push(e.syntax().text().to_string());
                }
                for p in label.patterns() {
                    items.push(p.syntax().text().to_string());
                }
            }
            text.push_str("case ");
            text.push_str(&items.join(", "));
            if let Some(guard) = group.labels.last().and_then(|l| l.guard()) {
                text.push(' ');
                text.push_str(&guard.syntax().text().to_string());
            }
        }
        text.push_str(" -> ");
        match group.stmts.as_slice() {
            [] => text.push_str("{}\n"),
            [only @ (ast::Stmt::ExprStmt(_) | ast::Stmt::ThrowStmt(_))] => {
                text.push_str(&only.syntax().text().to_string());
                text.push('\n');
            }
            [only @ ast::Stmt::Block(_)] => {
                text.push_str(&only.syntax().text().to_string());
                text.push('\n');
            }
            stmts => {
                text.push_str("{\n");
                let start = stmts[0].syntax().text_range().start();
                let end = stmts.last().unwrap().syntax().text_range().end();
                let line_start = ctx.text[..usize::from(start)]
                    .rfind('\n')
                    .map_or(0, |i| i + 1);
                let original = &ctx.text[line_start..usize::from(end)];
                let old_indent: String = original
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                for line in original.lines() {
                    if line.trim().is_empty() {
                        text.push('\n');
                        continue;
                    }
                    let stripped = line
                        .strip_prefix(old_indent.as_str())
                        .unwrap_or(line.trim_start());
                    text.push_str(&inner);
                    text.push_str(stripped);
                    text.push('\n');
                }
                text.push_str(&case_indent);
                text.push_str("}\n");
            }
        }
    }
    let close_indent = ctx.indent_at(switch.syntax().text_range().start());
    text.push_str(&close_indent);
    text.push('}');
    out.push(ctx.assist(
        "switch_to_arrow",
        base_db::trs!("改成 `->` 形式的 switch", "Convert to `->` switch"),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(block.syntax().text_range(), text)],
    ));
}

/// 构造器形参还没有对应的字段：创建字段并在构造器里赋值。
pub(super) fn field_from_parameter(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    let Some(param) = token.parent_ancestors().find_map(ast::Param::cast) else {
        return;
    };
    let Some(ctor) = param
        .syntax()
        .parent()
        .and_then(|list| list.parent())
        .and_then(ast::ConstructorDecl::cast)
    else {
        return;
    };
    let (Some(name), Some(ty), Some(body)) = (param.name(), param.ty(), ctor.body()) else {
        return;
    };
    let Some(class_decl) = ctor.syntax().ancestors().find_map(ast::TypeDecl::cast) else {
        return;
    };
    if class_decl.syntax().kind() == SyntaxKind::RECORD_DECL {
        return;
    }
    let name = name.text().to_string();
    // 已经有同名字段，或者构造器里已经赋给了某个字段
    let fields = super::fields_of(&class_decl);
    if fields.iter().any(|f| f.name == name) {
        return;
    }
    if body
        .syntax()
        .descendants()
        .filter_map(ast::AssignExpr::cast)
        .any(|a| a.rhs().is_some_and(|r| r.syntax().text() == name.as_str()))
    {
        return;
    }
    let mut ty_text = ty.syntax().text().to_string();
    if param.is_varargs() {
        ty_text = format!("{}[]", ty_text.trim_end_matches("..."));
    }
    let Some(class_body) = class_decl.body() else {
        return;
    };
    let class_indent = ctx.indent_at(class_decl.syntax().text_range().start());
    let member_indent = format!("{class_indent}    ");
    // 字段放在已有字段的后面；没有字段时放在类体开头
    let last_field = class_body
        .members()
        .filter(|m| matches!(m, ast::Member::FieldDecl(_)))
        .last();
    let field_line = format!("private final {ty_text} {name};");
    let field_edit = match last_field {
        Some(field) => TextEdit::insert(
            field.syntax().text_range().end(),
            format!("\n{member_indent}{field_line}"),
        ),
        None => {
            let Some(open) = class_body
                .syntax()
                .first_token()
                .filter(|t| t.kind() == SyntaxKind::L_CURLY)
            else {
                return;
            };
            TextEdit::insert(
                open.text_range().end(),
                format!("\n{member_indent}{field_line}\n"),
            )
        }
    };
    // 赋值放在构造器体的最后
    let Some(close) = body.r_curly_token() else {
        return;
    };
    let ctor_indent = ctx.indent_at(ctor.syntax().text_range().start());
    let stmt_indent = format!("{ctor_indent}    ");
    let close_start = usize::from(close.text_range().start());
    let line_start = ctx.text[..close_start].rfind('\n').map_or(0, |i| i + 1);
    let assign = format!("this.{name} = {name};");
    let assign_edit = if ctx.text[line_start..close_start].trim().is_empty() && line_start > 0 {
        TextEdit::insert(
            TextSize::from(line_start as u32),
            format!("{stmt_indent}{assign}\n"),
        )
    } else {
        TextEdit::insert(
            close.text_range().start(),
            format!("\n{stmt_indent}{assign}\n{ctor_indent}"),
        )
    };
    out.push(ctx.assist(
        "field_from_parameter",
        base_db::tr!(
            "为形参 `{name}` 创建字段",
            "Create field for parameter `{name}`"
        ),
        AssistKind::QuickFix,
        vec![field_edit, assign_edit],
    ));
}

// ---------------------------------------------------------------------------
// De Morgan
// ---------------------------------------------------------------------------

/// 把同一个运算符连起来的操作数摊平：`a && b && c` → `[a, b, c]`。
fn flatten_chain(expr: &ast::Expr, op: SyntaxKind, out: &mut Vec<ast::Expr>) {
    match expr {
        ast::Expr::BinaryExpr(binary) if binary.op_kind() == Some(op) => {
            if let (Some(lhs), Some(rhs)) = (binary.lhs(), binary.rhs()) {
                flatten_chain(&lhs, op, out);
                flatten_chain(&rhs, op, out);
            } else {
                out.push(expr.clone());
            }
        }
        _ => out.push(expr.clone()),
    }
}

/// 取反一个操作数，结果要接到 `joiner` 连起来的表达式里。
fn negate_operand(expr: &ast::Expr, joiner: SyntaxKind) -> String {
    let text = super::refactor::negate(expr);
    // `!(a || b)` 取反得到 `a || b`，接到 `&&` 里要保留括号
    if let ast::Expr::UnaryExpr(unary) = expr {
        if let Some(ast::Expr::ParenExpr(paren)) = unary.operand() {
            let loose = paren
                .expr()
                .is_some_and(|inner| match inner.syntax().kind() {
                    SyntaxKind::BINARY_EXPR => {
                        let op =
                            ast::BinaryExpr::cast(inner.syntax().clone()).and_then(|b| b.op_kind());
                        op == Some(SyntaxKind::PIPE_PIPE) && joiner == SyntaxKind::AMP_AMP
                    }
                    SyntaxKind::CONDITIONAL_EXPR
                    | SyntaxKind::ASSIGN_EXPR
                    | SyntaxKind::LAMBDA_EXPR => true,
                    _ => false,
                });
            if loose {
                return format!("({text})");
            }
        }
    }
    text
}

/// `a && b` → `!(!a || !b)`，`!(a && b)` → `!a || !b`（`||` 同理）。光标在运算符上。
pub(super) fn demorgan(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    let (op, other, other_text) = match token.kind() {
        SyntaxKind::AMP_AMP => (SyntaxKind::AMP_AMP, SyntaxKind::PIPE_PIPE, "||"),
        SyntaxKind::PIPE_PIPE => (SyntaxKind::PIPE_PIPE, SyntaxKind::AMP_AMP, "&&"),
        _ => return,
    };
    let Some(mut top) = token.parent().and_then(ast::BinaryExpr::cast) else {
        return;
    };
    // 整条 `a && b && c`
    while let Some(parent) = top
        .syntax()
        .parent()
        .and_then(ast::BinaryExpr::cast)
        .filter(|p| p.op_kind() == Some(op))
    {
        top = parent;
    }
    let mut operands = Vec::new();
    flatten_chain(&ast::Expr::BinaryExpr(top.clone()), op, &mut operands);
    if operands.len() < 2 {
        return;
    }
    let joined = operands
        .iter()
        .map(|e| negate_operand(e, other))
        .collect::<Vec<_>>()
        .join(&format!(" {other_text} "));
    // 外面是 `!(…)`：去掉这层取反
    let negation = top
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::PAREN_EXPR)
        .and_then(|p| p.parent())
        .and_then(ast::UnaryExpr::cast)
        .filter(|u| u.op_token().is_some_and(|t| t.kind() == SyntaxKind::BANG));
    let edit = match negation {
        Some(unary) => {
            let needs_parens = unary.syntax().parent().is_some_and(|p| match p.kind() {
                SyntaxKind::BINARY_EXPR => {
                    ast::BinaryExpr::cast(p).and_then(|b| b.op_kind()) != Some(other)
                }
                SyntaxKind::UNARY_EXPR
                | SyntaxKind::CAST_EXPR
                | SyntaxKind::INSTANCEOF_EXPR
                | SyntaxKind::FIELD_ACCESS
                | SyntaxKind::METHOD_CALL
                | SyntaxKind::ARRAY_ACCESS => true,
                _ => false,
            });
            let text = if needs_parens {
                format!("({joined})")
            } else {
                joined
            };
            TextEdit::replace(unary.syntax().text_range(), text)
        }
        None => TextEdit::replace(top.syntax().text_range(), format!("!({joined})")),
    };
    out.push(ctx.assist(
        "demorgan",
        base_db::tr!(
            "把 `{}` 换成 `{other_text}`",
            "Replace `{}` with `{other_text}`",
            token.text()
        ),
        AssistKind::RefactorRewrite,
        vec![edit],
    ));
}

// ---------------------------------------------------------------------------
// 删除多余的 else
// ---------------------------------------------------------------------------

/// 语句（或块的最后一条语句）是 `return` / `throw` / `break` / `continue`。
fn ends_abruptly(stmt: &ast::Stmt) -> bool {
    match stmt {
        ast::Stmt::Block(block) => block.statements().last().is_some_and(|s| ends_abruptly(&s)),
        ast::Stmt::ReturnStmt(_)
        | ast::Stmt::ThrowStmt(_)
        | ast::Stmt::BreakStmt(_)
        | ast::Stmt::ContinueStmt(_) => true,
        _ => false,
    }
}

/// 去掉每行开头的一级缩进（四个空格或一个制表符）。
fn dedent_line(line: &str) -> &str {
    line.strip_prefix("    ")
        .or_else(|| line.strip_prefix('\t'))
        .unwrap_or(line)
}

/// `if (c) { …; return; } else { B }` → `if (c) { …; return; } B`。光标在 `else` 上。
pub(super) fn remove_redundant_else(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    if token.kind() != SyntaxKind::ELSE_KW {
        return;
    }
    let Some(if_stmt) = token.parent().and_then(ast::IfStmt::cast) else {
        return;
    };
    let Some(block) = if_stmt
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::BLOCK)
    else {
        return;
    };
    let (Some(then_branch), Some(else_branch)) = (if_stmt.then_branch(), if_stmt.else_branch())
    else {
        return;
    };
    if !ends_abruptly(&then_branch) {
        return;
    }
    // then 和 else 之间的注释会丢，不提供
    let between = TextRange::new(
        then_branch.syntax().text_range().end(),
        else_branch.syntax().text_range().start(),
    );
    let has_comment = if_stmt
        .syntax()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| {
            t.kind().is_trivia()
                && t.kind() != SyntaxKind::WHITESPACE
                && between.contains_range(t.text_range())
        });
    if has_comment {
        return;
    }
    // else 块里声明的名字移到外层后不能和后面的声明冲突
    if let ast::Stmt::Block(else_block) = &else_branch {
        let declared: Vec<String> = else_block
            .statements()
            .filter_map(|s| ast::LocalVarDecl::cast(s.syntax().clone()))
            .flat_map(|d| {
                d.declarators()
                    .filter_map(|v| v.name())
                    .map(|n| n.text().to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        if !declared.is_empty() {
            let later = block
                .children()
                .filter(|c| c.text_range().start() >= if_stmt.syntax().text_range().end())
                .flat_map(|c| c.descendants().collect::<Vec<_>>())
                .filter_map(ast::Name::cast)
                .any(|n| declared.iter().any(|d| n.text() == d.as_str()));
            if later {
                return;
            }
        }
    }
    let indent = ctx.indent_at(if_stmt.syntax().text_range().start());
    let replacement = match &else_branch {
        ast::Stmt::Block(else_block) => {
            let text = else_block.syntax().text().to_string();
            let inner = text
                .strip_prefix('{')
                .and_then(|t| t.strip_suffix('}'))
                .unwrap_or("");
            let lines: Vec<&str> = inner.lines().collect();
            let first = lines.iter().position(|l| !l.trim().is_empty());
            let last = lines.iter().rposition(|l| !l.trim().is_empty());
            match (first, last) {
                (Some(first), Some(last)) if lines.len() > 1 && first > 0 => lines[first..=last]
                    .iter()
                    .map(|l| format!("\n{}", dedent_line(l.trim_end())))
                    .collect::<String>(),
                // `else { foo(); }` 写在一行里
                (Some(first), Some(_)) => format!("\n{indent}{}", lines[first].trim()),
                _ => String::new(),
            }
        }
        other => format!("\n{indent}{}", other.syntax().text()),
    };
    let edit = TextEdit::replace(
        TextRange::new(
            then_branch.syntax().text_range().end(),
            else_branch.syntax().text_range().end(),
        ),
        replacement,
    );
    out.push(ctx.assist(
        "remove_redundant_else",
        base_db::trs!("删除多余的 else", "Remove redundant else"),
        AssistKind::RefactorRewrite,
        vec![edit],
    ));
}

// ---------------------------------------------------------------------------
// 显式类型 → var
// ---------------------------------------------------------------------------

/// 初值的类型要靠目标类型推断（菱形、泛型方法、条件表达式、lambda 等）时换成 `var` 会改变类型。
fn initializer_is_standalone(ctx: &AssistCtx<'_>, init: &ast::Expr) -> bool {
    match init.syntax().kind() {
        SyntaxKind::LAMBDA_EXPR
        | SyntaxKind::METHOD_REF
        | SyntaxKind::ARRAY_INITIALIZER
        | SyntaxKind::CONDITIONAL_EXPR
        | SyntaxKind::SWITCH_EXPR => false,
        SyntaxKind::LITERAL => init.syntax().text() != "null",
        SyntaxKind::NEW_EXPR => {
            let Some(new) = ast::NewExpr::cast(init.syntax().clone()) else {
                return false;
            };
            new.body().is_none()
                && new
                    .ty()
                    .is_some_and(|t| !t.syntax().text().to_string().contains("<>"))
        }
        SyntaxKind::METHOD_CALL => {
            let Some(call) = ast::MethodCall::cast(init.syntax().clone()) else {
                return false;
            };
            if call.type_arg_list().is_some() {
                return true;
            }
            let Some(name) = call.name_ref() else {
                return false;
            };
            match ctx.sema.classify_name_ref(ctx.file, &name) {
                Some(Definition::Method(method)) => class_data(ctx.db, method.class)
                    .and_then(|d| {
                        d.methods
                            .get(method.idx as usize)
                            .map(|m| m.type_params.is_empty())
                    })
                    .unwrap_or(false),
                _ => false,
            }
        }
        SyntaxKind::PAREN_EXPR => ast::ParenExpr::cast(init.syntax().clone())
            .and_then(|p| p.expr())
            .is_some_and(|e| initializer_is_standalone(ctx, &e)),
        _ => true,
    }
}

/// `ArrayList<String> list = new ArrayList<String>();` → `var list = …`。只在初值的类型和声明的
/// 类型完全一样时提供。
pub(super) fn replace_type_with_var(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    if ide_diagnostics::language_level(ctx.db, ctx.file).is_some_and(|l| l < 10) {
        return;
    }
    let Some(token) = cursor_token(ctx) else {
        return;
    };
    let Some(decl) = token.parent_ancestors().find_map(ast::LocalVarDecl::cast) else {
        return;
    };
    if !decl
        .syntax()
        .parent()
        .is_some_and(|p| matches!(p.kind(), SyntaxKind::BLOCK | SyntaxKind::FOR_STMT))
    {
        return;
    }
    let mut declarators = decl.declarators();
    let (Some(declarator), None) = (declarators.next(), declarators.next()) else {
        return;
    };
    let (Some(ty), Some(init)) = (decl.ty(), declarator.initializer()) else {
        return;
    };
    if ctx.range.start() > init.syntax().text_range().start()
        || declarator.extra_dims() > 0
        || ty.syntax().text() == "var"
    {
        return;
    }
    if !initializer_is_standalone(ctx, &init) {
        return;
    }
    let declared = ctx.sema.resolve_type(ctx.file, &ty);
    let Some(actual) = ctx.sema.type_of_expr(ctx.file, &init) else {
        return;
    };
    if declared.contains_unknown()
        || actual != declared
        || matches!(actual, Ty::Null | Ty::Void | Ty::Intersection(_))
    {
        return;
    }
    out.push(ctx.assist(
        "replace_type_with_var",
        base_db::trs!("把显式类型替换为 var", "Replace explicit type with var"),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(ty.syntax().text_range(), "var")],
    ));
}
