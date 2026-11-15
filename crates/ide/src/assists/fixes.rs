//! 针对诊断的快速修复：光标在出错的位置上时提供。

use hir::{class_data, is_subtype, ClassKind, Definition, Ty};
use ide_db::{imports::insert_import, TextEdit};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use super::{Assist, AssistCtx, AssistKind};

/// 类型的源码写法；未知类型和 `null` 写成 `Object`。
pub(super) fn ty_text(ctx: &AssistCtx<'_>, ty: Option<&Ty>) -> String {
    match ty {
        Some(ty) if !ty.contains_unknown() && !matches!(ty, Ty::Null | Ty::Void) => {
            ty.display(ctx.db)
        }
        _ => "Object".to_owned(),
    }
}

/// 某个类型的"零值"表达式。
pub(super) fn default_value(ty: &str) -> &'static str {
    match ty {
        "boolean" => "false",
        "byte" | "short" | "int" | "long" | "char" => "0",
        "float" => "0f",
        "double" => "0.0",
        _ => "null",
    }
}

fn decapitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// 由类型起一个形参名：`String` → `string`，`int` → `i`，`List<String>` → `list`。
pub(super) fn name_for_type(ty: &str) -> String {
    let base: String = ty
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    match base.as_str() {
        "" => "arg".to_owned(),
        "int" | "long" | "short" | "byte" => "i".to_owned(),
        "boolean" => "flag".to_owned(),
        "char" => "c".to_owned(),
        "double" | "float" => "v".to_owned(),
        other => decapitalize(other),
    }
}

/// 光标处的名字引用（必须解析不了）。
pub(super) fn unresolved_name_at_cursor(ctx: &AssistCtx<'_>) -> Option<ast::NameRef> {
    let token = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start())?;
    if token.kind() != SyntaxKind::IDENT {
        return None;
    }
    let name_ref = token.parent().and_then(ast::NameRef::cast)?;
    if ctx.sema.classify_name_ref(ctx.file, &name_ref).is_some() {
        return None;
    }
    Some(name_ref)
}

fn enclosing_callable(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors().find(|a| {
        matches!(
            a.kind(),
            SyntaxKind::METHOD_DECL
                | SyntaxKind::CONSTRUCTOR_DECL
                | SyntaxKind::INITIALIZER_BLOCK
                | SyntaxKind::FIELD_DECL
        )
    })
}

fn is_static_callable(callable: &SyntaxNode) -> bool {
    callable
        .children()
        .find_map(ast::ModifierList::cast)
        .is_some_and(|m| m.has(SyntaxKind::STATIC_KW))
        || callable
            .children_with_tokens()
            .any(|e| e.kind() == SyntaxKind::STATIC_KW)
}

/// 表达式所在位置期望的类型的写法（声明的类型、方法的返回类型、条件）。
pub(super) fn expected_type_text(ctx: &AssistCtx<'_>, expr: &SyntaxNode) -> Option<String> {
    let parent = expr.parent()?;
    match parent.kind() {
        SyntaxKind::EXPR_STMT => Some("void".to_owned()),
        SyntaxKind::VARIABLE_DECLARATOR => {
            let ty = parent.parent()?.children().find_map(ast::Type::cast)?;
            let text = ty.syntax().text().to_string();
            (text != "var").then_some(text)
        }
        SyntaxKind::RETURN_STMT => {
            let method = parent.ancestors().find_map(|a| {
                if a.kind() == SyntaxKind::LAMBDA_EXPR {
                    return Some(None);
                }
                ast::MethodDecl::cast(a).map(Some)
            })??;
            Some(method.return_type()?.syntax().text().to_string())
        }
        SyntaxKind::IF_STMT | SyntaxKind::WHILE_STMT | SyntaxKind::DO_WHILE_STMT => {
            Some("boolean".to_owned())
        }
        // `x + 1`：按另一个操作数的类型来猜
        SyntaxKind::BINARY_EXPR => {
            let binary = ast::BinaryExpr::cast(parent)?;
            let other = [binary.lhs(), binary.rhs()]
                .into_iter()
                .flatten()
                .find(|operand| operand.syntax() != expr)?;
            let ty = ctx.sema.type_of_expr(ctx.file, &other)?;
            (!ty.contains_unknown() && !matches!(ty, Ty::Null)).then(|| ty.display(ctx.db))
        }
        SyntaxKind::ASSIGN_EXPR => {
            let assign = ast::AssignExpr::cast(parent)?;
            let lhs = assign.lhs()?;
            if lhs.syntax() == expr {
                let rhs = assign.rhs()?;
                Some(ty_text(ctx, ctx.sema.type_of_expr(ctx.file, &rhs).as_ref()))
            } else {
                Some(ty_text(ctx, ctx.sema.type_of_expr(ctx.file, &lhs).as_ref()))
            }
        }
        _ => None,
    }
}

/// 调用了不存在的方法（不带接收者或 `this.`）：在当前类里创建它。
pub(super) fn create_method(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(name_ref) = unresolved_name_at_cursor(ctx) else {
        return;
    };
    let Some(call) = name_ref.syntax().parent().and_then(ast::MethodCall::cast) else {
        return;
    };
    if !matches!(call.receiver(), None | Some(ast::Expr::ThisExpr(_))) {
        return;
    }
    let Some((_, decl)) = ctx.enclosing_class() else {
        return;
    };
    let args: Vec<ast::Expr> = call
        .arg_list()
        .map(|l| l.args().collect())
        .unwrap_or_default();
    let mut used: Vec<String> = Vec::new();
    let params: Vec<String> = args
        .iter()
        .map(|arg| {
            let ty = ty_text(ctx, ctx.sema.type_of_expr(ctx.file, arg).as_ref());
            let base = match arg {
                ast::Expr::NameExpr(name) => name.syntax().text().to_string(),
                _ => name_for_type(&ty),
            };
            let mut name = base.clone();
            let mut n = 2;
            while used.contains(&name) {
                name = format!("{base}{n}");
                n += 1;
            }
            used.push(name.clone());
            format!("{ty} {name}")
        })
        .collect();
    let ret = expected_type_text(ctx, call.syntax()).unwrap_or_else(|| "Object".to_owned());
    let is_static = enclosing_callable(call.syntax()).is_some_and(|c| is_static_callable(&c));
    let mut lines = vec![format!(
        "private {}{ret} {}({}) {{",
        if is_static { "static " } else { "" },
        name_ref.text(),
        params.join(", ")
    )];
    if ret != "void" {
        lines.push(format!("    return {};", default_value(&ret)));
    }
    lines.push("}".to_owned());
    let Some(edit) = ctx.insert_members(&decl, &[lines]) else {
        return;
    };
    out.push(ctx.assist(
        "create_method",
        base_db::tr!("创建方法 {}", "Create method {}", name_ref.text()),
        AssistKind::QuickFix,
        vec![edit],
    ));
}

/// 用了不存在的变量名：创建字段；如果是给它赋值的语句，也可以就地声明成局部变量。
pub(super) fn create_variable(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(name_ref) = unresolved_name_at_cursor(ctx) else {
        return;
    };
    let Some(name_expr) = name_ref
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::NAME_EXPR)
    else {
        return;
    };
    let name = name_ref.text();
    // 大写开头的多半是没导入的类型，交给 add_import
    if name.starts_with(|c: char| c.is_uppercase()) && name.chars().any(|c| c.is_lowercase()) {
        return;
    }
    let ty = expected_type_text(ctx, &name_expr)
        .filter(|t| t != "void")
        .unwrap_or_else(|| "Object".to_owned());

    // 就地声明：`total = compute();` → `int total = compute();`
    let assigned_in_statement =
        name_expr
            .parent()
            .and_then(ast::AssignExpr::cast)
            .filter(|assign| {
                assign.lhs().is_some_and(|l| *l.syntax() == name_expr)
                    && assign
                        .op_token()
                        .is_some_and(|op| op.kind() == SyntaxKind::EQ)
                    && assign
                        .syntax()
                        .parent()
                        .is_some_and(|p| p.kind() == SyntaxKind::EXPR_STMT)
            });
    if let Some(assign) = assigned_in_statement {
        out.push(ctx.assist(
            "create_local_variable",
            base_db::tr!("声明局部变量 {name}", "Declare local variable {name}"),
            AssistKind::QuickFix,
            vec![TextEdit::insert(
                assign.syntax().text_range().start(),
                format!("{ty} "),
            )],
        ));
    }

    let Some((_, decl)) = ctx.enclosing_class() else {
        return;
    };
    let is_static = enclosing_callable(&name_expr).is_some_and(|c| is_static_callable(&c));
    let is_constant = name
        .chars()
        .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit());
    let line = if is_constant {
        format!("private static final {ty} {name} = {};", default_value(&ty))
    } else {
        format!(
            "private {}{ty} {name};",
            if is_static { "static " } else { "" }
        )
    };
    if let Some(edit) = ctx.insert_members(&decl, &[vec![line]]) {
        out.push(ctx.assist(
            "create_field",
            base_db::tr!("创建字段 {name}", "Create field {name}"),
            AssistKind::QuickFix,
            vec![edit],
        ));
    }
}

/// 类型在这个文件里能不能直接用简单名写出来；不能时返回需要加的 import。
pub(super) fn import_for(ctx: &AssistCtx<'_>, ty: &Ty, at: &SyntaxNode) -> Option<TextEdit> {
    let class = ty.as_class()?;
    let data = class_data(ctx.db, class)?;
    let scope = ctx.sema.type_scope(ctx.file, at);
    match hir::resolve_simple_type(ctx.db, &scope, &data.name) {
        Some(hir::TypeNs::Class(found)) if found == class => None,
        _ => {
            let qualified = ide_db::imports::importable_name(ctx.db, class)?;
            Some(insert_import(ctx.db, ctx.file, &qualified))
        }
    }
}

/// 光标处没有处理的受检异常：加到方法的 `throws` 上，或者用 try/catch 包起来。
pub(super) fn handle_exception(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let unhandled = ide_diagnostics::unhandled_exceptions(ctx.db, ctx.file);
    let Some(target) = unhandled.iter().find(|u| {
        u.range.contains_range(ctx.range) || u.range.contains_inclusive(ctx.range.start())
    }) else {
        return;
    };
    let site = match ctx.root.covering_element(target.site) {
        syntax::NodeOrToken::Node(node) => node,
        syntax::NodeOrToken::Token(token) => match token.parent() {
            Some(node) => node,
            None => return,
        },
    };
    let exception = target.ty.display(ctx.db);
    let import = import_for(ctx, &target.ty, &site);

    // 加 throws：所在的方法 / 构造器（lambda 和类体里的代码不属于它）
    let callable = site
        .ancestors()
        .take_while(|a| !matches!(a.kind(), SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY))
        .find(|a| {
            matches!(
                a.kind(),
                SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL
            )
        });
    if let Some(callable) = callable {
        let throws = callable
            .children()
            .find(|c| c.kind() == SyntaxKind::THROWS_CLAUSE);
        let edit = match throws {
            Some(clause) => Some(TextEdit::insert(
                clause.text_range().end(),
                format!(", {exception}"),
            )),
            None => callable
                .children()
                .find(|c| c.kind() == SyntaxKind::PARAM_LIST)
                .map(|params| {
                    TextEdit::insert(params.text_range().end(), format!(" throws {exception}"))
                }),
        };
        if let Some(edit) = edit {
            let mut edits = vec![edit];
            edits.extend(import.clone());
            edits.sort_by_key(|e| e.range.start());
            out.push(ctx.assist(
                "add_throws",
                base_db::tr!(
                    "在方法上声明 throws {exception}",
                    "Add throws {exception} to the method"
                ),
                AssistKind::QuickFix,
                edits,
            ));
        }
    }

    // try/catch：包住所在的语句。声明语句包起来会改变变量的作用域，不提供
    let statement = site
        .ancestors()
        .take_while(|a| !matches!(a.kind(), SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY))
        .find(|a| {
            ast::Stmt::can_cast(a.kind())
                && a.parent().is_some_and(|p| p.kind() == SyntaxKind::BLOCK)
        });
    let Some(statement) = statement else { return };
    if statement.kind() == SyntaxKind::LOCAL_VAR_DECL {
        return;
    }
    let indent = ctx.indent_at(statement.text_range().start());
    let body: String = statement
        .text()
        .to_string()
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                format!("{indent}    {line}")
            } else if line.trim().is_empty() {
                String::new()
            } else {
                format!("    {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let replacement = format!(
        "try {{\n{body}\n{indent}}} catch ({exception} e) {{\n{indent}    throw new RuntimeException(e);\n{indent}}}"
    );
    let mut edits = vec![TextEdit::replace(statement.text_range(), replacement)];
    edits.extend(import);
    edits.sort_by_key(|e| e.range.start());
    out.push(ctx.assist(
        "surround_with_try_catch",
        base_db::tr!(
            "用 try/catch 捕获 {exception}",
            "Surround with try/catch for {exception}"
        ),
        AssistKind::QuickFix,
        edits,
    ));
}

fn is_atomic(expr: &ast::Expr) -> bool {
    matches!(
        expr,
        ast::Expr::Literal(_)
            | ast::Expr::NameExpr(_)
            | ast::Expr::FieldAccess(_)
            | ast::Expr::MethodCall(_)
            | ast::Expr::ParenExpr(_)
            | ast::Expr::ThisExpr(_)
            | ast::Expr::ArrayAccess(_)
            | ast::Expr::NewExpr(_)
    )
}

/// 初值 / 返回值的类型和需要的类型不符，但可以强制转换：加上转换。
pub(super) fn add_cast(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    // 光标所在的、直接作为初值 / 返回值 / 赋值右边的那个表达式
    let Some(expr) = token.parent_ancestors().find_map(|node| {
        let parent = node.parent()?;
        let is_value = match parent.kind() {
            SyntaxKind::VARIABLE_DECLARATOR | SyntaxKind::RETURN_STMT => true,
            SyntaxKind::ASSIGN_EXPR => {
                ast::AssignExpr::cast(parent.clone())?.rhs()?.syntax() == &node
            }
            _ => false,
        };
        if is_value {
            ast::Expr::cast(node)
        } else {
            None
        }
    }) else {
        return;
    };
    let parent = match expr.syntax().parent() {
        Some(parent) => parent,
        None => return,
    };
    let Some(actual) = ctx.sema.type_of_expr(ctx.file, &expr) else {
        return;
    };
    let (expected, expected_text) = match parent.kind() {
        SyntaxKind::VARIABLE_DECLARATOR => {
            let Some(ty) = parent
                .parent()
                .and_then(|decl| decl.children().find_map(ast::Type::cast))
            else {
                return;
            };
            if ty.syntax().text() == "var" {
                return;
            }
            (
                ctx.sema.resolve_type(ctx.file, &ty),
                ty.syntax().text().to_string(),
            )
        }
        SyntaxKind::RETURN_STMT => {
            let method = parent
                .ancestors()
                .take_while(|a| a.kind() != SyntaxKind::LAMBDA_EXPR)
                .find_map(ast::MethodDecl::cast);
            let Some(ty) = method.and_then(|m| m.return_type()) else {
                return;
            };
            (
                ctx.sema.resolve_type(ctx.file, &ty),
                ty.syntax().text().to_string(),
            )
        }
        _ => {
            let Some(lhs) = ast::AssignExpr::cast(parent).and_then(|a| a.lhs()) else {
                return;
            };
            let Some(ty) = ctx.sema.type_of_expr(ctx.file, &lhs) else {
                return;
            };
            let text = ty.display(ctx.db);
            (ty, text)
        }
    };
    if expected.contains_unknown()
        || actual.contains_unknown()
        || matches!(expected, Ty::Void)
        || hir::is_assignable(ctx.db, &actual, &expected)
    {
        return;
    }
    let numeric = |ty: &Ty| ty.as_primitive().is_some_and(|p| p.is_numeric());
    let castable = (numeric(&actual) && numeric(&expected))
        || (!actual.is_primitive()
            && !expected.is_primitive()
            && is_subtype(ctx.db, &expected, &actual));
    if !castable {
        return;
    }
    let range = expr.syntax().text_range();
    let edits = if is_atomic(&expr) {
        vec![TextEdit::insert(
            range.start(),
            format!("({expected_text}) "),
        )]
    } else {
        vec![
            TextEdit::insert(range.start(), format!("({expected_text}) (")),
            TextEdit::insert(range.end(), ")".to_owned()),
        ]
    };
    out.push(ctx.assist(
        "add_cast",
        base_db::tr!("强制转换为 {expected_text}", "Cast to {expected_text}"),
        AssistKind::QuickFix,
        edits,
    ));
}

/// 光标在对枚举的 `switch` 上：补上没写的常量分支。
pub(super) fn add_missing_switch_cases(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    // 只在 switch 的头部（关键字和被判断的表达式）上提供
    let Some(switch) = token
        .parent_ancestors()
        .find(|n| matches!(n.kind(), SyntaxKind::SWITCH_STMT | SyntaxKind::SWITCH_EXPR))
    else {
        return;
    };
    let Some(block) = switch.children().find_map(ast::SwitchBlock::cast) else {
        return;
    };
    if block.syntax().text_range().contains_range(ctx.range)
        && ctx.range.start() > block.syntax().text_range().start()
    {
        return;
    }
    let Some(scrutinee) = switch.children().find_map(ast::Expr::cast) else {
        return;
    };
    let Some(Ty::Class(class, _)) = ctx.sema.type_of_expr(ctx.file, &scrutinee) else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    if data.kind != ClassKind::Enum {
        return;
    }
    let mut covered: Vec<String> = Vec::new();
    let mut arrow = None;
    for case in block.cases() {
        arrow.get_or_insert(case.is_arrow());
        if let Some(label) = case.label() {
            for expr in label.exprs() {
                if let Some(name) = expr
                    .syntax()
                    .descendants()
                    .filter_map(ast::NameRef::cast)
                    .last()
                {
                    covered.push(name.text().to_string());
                }
            }
        }
    }
    let missing: Vec<&str> = data
        .fields
        .iter()
        .filter(|f| f.is_enum_constant())
        .map(|f| f.name.as_str())
        .filter(|name| !covered.iter().any(|c| c == name))
        .collect();
    if missing.is_empty() {
        return;
    }
    let Some(close) = block
        .syntax()
        .last_token()
        .filter(|t| t.kind() == SyntaxKind::R_CURLY)
    else {
        return;
    };
    let is_expr = switch.kind() == SyntaxKind::SWITCH_EXPR;
    let arrow = arrow.unwrap_or(true);
    let indent = ctx.indent_at(switch.text_range().start());
    let mut text = String::new();
    for name in &missing {
        if arrow && is_expr {
            text.push_str(&format!(
                "{indent}    case {name} -> throw new UnsupportedOperationException();\n"
            ));
        } else if arrow {
            text.push_str(&format!("{indent}    case {name} -> {{\n{indent}    }}\n"));
        } else {
            text.push_str(&format!(
                "{indent}    case {name}:\n{indent}        break;\n"
            ));
        }
    }
    // 右花括号独占一行时插在它那一行之前
    let close_start = usize::from(close.text_range().start());
    let line_start = ctx.text[..close_start].rfind('\n').map_or(0, |i| i + 1);
    let edit = if ctx.text[line_start..close_start].trim().is_empty() {
        TextEdit::insert(TextSize::from(line_start as u32), text)
    } else {
        TextEdit::insert(close.text_range().start(), format!("\n{text}{indent}"))
    };
    out.push(ctx.assist(
        "add_missing_switch_cases",
        base_db::tr!(
            "补全缺少的分支（{}）",
            "Add missing cases ({})",
            missing.join(", ")
        ),
        AssistKind::QuickFix,
        vec![edit],
    ));
}

/// 光标在一个声明时没有初值的局部变量的引用上：给声明补上初值。
pub(super) fn initialize_variable(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(name_ref) = token.parent().and_then(ast::NameRef::cast) else {
        return;
    };
    let Some(def @ Definition::Local(_)) = ctx.sema.classify_name_ref(ctx.file, &name_ref) else {
        return;
    };
    let Some(source) = def.source(ctx.db) else {
        return;
    };
    let declarator = match ctx.root.covering_element(source.focus_range) {
        syntax::NodeOrToken::Node(node) => Some(node),
        syntax::NodeOrToken::Token(token) => token.parent(),
    }
    .and_then(|n| n.ancestors().find_map(ast::VariableDeclarator::cast));
    let Some(declarator) = declarator else { return };
    if declarator.initializer().is_some() {
        return;
    }
    let Some(decl) = declarator
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::LOCAL_VAR_DECL)
    else {
        return;
    };
    let Some(ty) = decl.children().find_map(ast::Type::cast) else {
        return;
    };
    let value = if declarator.extra_dims() > 0 {
        "null"
    } else {
        default_value(&ty.syntax().text().to_string())
    };
    out.push(ctx.assist(
        "initialize_variable",
        base_db::tr!("初始化变量 {}", "Initialize variable {}", name_ref.text()),
        AssistKind::QuickFix,
        vec![TextEdit::insert(
            declarator.syntax().text_range().end(),
            format!(" = {value}"),
        )],
    ));
}

/// `a == b` 两边都是字符串：换成 `a.equals(b)`。
pub(super) fn replace_with_equals(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = ctx
        .root
        .token_at_offset(ctx.range.start())
        .find(|t| matches!(t.kind(), SyntaxKind::EQ_EQ | SyntaxKind::NEQ))
    else {
        return;
    };
    let Some(binary) = token.parent().and_then(ast::BinaryExpr::cast) else {
        return;
    };
    let (Some(lhs), Some(rhs)) = (binary.lhs(), binary.rhs()) else {
        return;
    };
    // 字符串，或者包装类型的数字（`Integer` 和 `Integer` 用 `==` 比较的是引用）
    let compared_by_value = |expr: &ast::Expr| {
        ctx.sema.type_of_expr(ctx.file, expr).is_some_and(|ty| {
            let is_string = ty
                .as_class()
                .and_then(|class| class_data(ctx.db, class))
                .is_some_and(|data| data.binary_name == "java/lang/String");
            let boxed_number = matches!(ty, Ty::Class(..))
                && ty
                    .unboxed(ctx.db)
                    .is_some_and(|p| p != hir::PrimitiveType::Boolean);
            is_string || boxed_number
        })
    };
    if !compared_by_value(&lhs) || !compared_by_value(&rhs) {
        return;
    }
    let lhs_text = lhs.syntax().text().to_string();
    let receiver = if is_atomic(&lhs) {
        lhs_text
    } else {
        format!("({lhs_text})")
    };
    let negate = if token.kind() == SyntaxKind::NEQ {
        "!"
    } else {
        ""
    };
    let range: TextRange = binary.syntax().text_range();
    out.push(ctx.assist(
        "replace_with_equals",
        base_db::trs!("换成 equals() 比较内容", "Replace with equals()"),
        AssistKind::QuickFix,
        vec![TextEdit::replace(
            range,
            format!("{negate}{receiver}.equals({})", rhs.syntax().text()),
        )],
    ));
}
