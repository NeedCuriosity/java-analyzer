//! 和字段有关的重构：内联常量、把表达式提取成字段。

use hir::{class_data, Definition, Ty};
use ide_db::{search::find_references, TextEdit};
use syntax::{
    ast::{self, AstNode},
    NodeOrToken, SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use super::{
    fixes::import_for,
    refactor::{group_edits, is_atomic, is_pure_argument, statement_removal_range},
    Assist, AssistCtx, AssistKind,
};
use crate::FileId;

fn node_at(root: &SyntaxNode, range: TextRange) -> Option<SyntaxNode> {
    match root.covering_element(range) {
        NodeOrToken::Node(node) => Some(node),
        NodeOrToken::Token(token) => token.parent(),
    }
}

/// `static final` 常量：把所有引用换成它的初值，删掉声明。
///
/// 初值里不能有名字（只由字面量和运算符组成），这样在任何地方展开都表示同样的值；
/// 带名字的初值只在所有引用都位于声明它的类里时才内联。
pub(super) fn inline_constant(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(parent) = token.parent() else {
        return;
    };
    let def = if let Some(name) = ast::Name::cast(parent.clone()) {
        ctx.sema.classify_name(ctx.file, &name)
    } else if let Some(name_ref) = ast::NameRef::cast(parent) {
        ctx.sema.classify_name_ref(ctx.file, &name_ref)
    } else {
        None
    };
    let Some(Definition::Field(field)) = def else {
        return;
    };
    let Some(data) = class_data(db, field.class) else {
        return;
    };
    let Some(field_data) = data.fields.get(field.idx as usize) else {
        return;
    };
    if !field_data.flags.is_static()
        || !field_data.flags.contains(hir::Modifiers::FINAL)
        || field_data.is_enum_constant()
    {
        return;
    }
    let Some(source) = Definition::Field(field).source(db) else {
        return;
    };
    let decl_file = source.file;
    let decl_file_id: FileId = decl_file.file_id(db);
    let decl_root = base_db::parse(db, decl_file).syntax_node();
    let Some(declarator) = node_at(&decl_root, source.focus_range)
        .and_then(|n| n.ancestors().find_map(ast::VariableDeclarator::cast))
    else {
        return;
    };
    let Some(init) = declarator.initializer() else {
        return;
    };
    let Some(field_decl) = declarator
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::FIELD_DECL)
    else {
        return;
    };
    if field_decl
        .children()
        .filter(|c| c.kind() == SyntaxKind::VARIABLE_DECLARATOR)
        .count()
        != 1
        || init.syntax().kind() == SyntaxKind::ARRAY_INITIALIZER
        || !is_pure_argument(&init)
    {
        return;
    }
    let has_names = init
        .syntax()
        .descendants()
        .any(|n| n.kind() == SyntaxKind::NAME_REF);
    let init_text = init.syntax().text().to_string();
    let references = find_references(db, &Definition::Field(field));
    let decl_text: &str = decl_file.text(db);
    let mut edits: Vec<(FileId, TextEdit)> = vec![(
        decl_file_id,
        TextEdit::replace(
            statement_removal_range(decl_text, field_decl.text_range()),
            "",
        ),
    )];
    for reference in &references {
        let Some(file) = base_db::source_file(db, reference.file_id) else {
            return;
        };
        let root = base_db::parse(db, file).syntax_node();
        let text: &str = file.text(db);
        let Some(node) = node_at(&root, reference.range) else {
            return;
        };
        // `import static p.C.NAME;`：删掉这条 import
        if let Some(import) = node
            .ancestors()
            .find(|n| n.kind() == SyntaxKind::IMPORT_DECL)
        {
            edits.push((
                reference.file_id,
                TextEdit::replace(statement_removal_range(text, import.text_range()), ""),
            ));
            continue;
        }
        if has_names && ctx.sema.enclosing_class(file, &node) != Some(field.class) {
            return;
        }
        // `C.NAME` / `this.NAME` 整个换掉
        let Some(name_ref) = node.ancestors().find_map(ast::NameRef::cast) else {
            return;
        };
        let target = match name_ref.syntax().parent() {
            Some(p) if p.kind() == SyntaxKind::FIELD_ACCESS => p,
            Some(p) if p.kind() == SyntaxKind::NAME_EXPR => p,
            _ => return,
        };
        let Some(target_expr) = ast::Expr::cast(target.clone()) else {
            return;
        };
        // 被赋值的位置（不应该出现，final）放弃
        if target
            .parent()
            .and_then(ast::AssignExpr::cast)
            .and_then(|a| a.lhs())
            .is_some_and(|lhs| lhs.syntax() == target_expr.syntax())
        {
            return;
        }
        let safe_context = target.parent().is_some_and(|p| {
            matches!(
                p.kind(),
                SyntaxKind::ARG_LIST
                    | SyntaxKind::VARIABLE_DECLARATOR
                    | SyntaxKind::RETURN_STMT
                    | SyntaxKind::PAREN_EXPR
                    | SyntaxKind::ARRAY_INITIALIZER
            )
        });
        // 二元运算里：初值的运算符优先级更高（或相同且在左边）时不用括号
        let binds_tighter = match (&init, target.parent().and_then(ast::BinaryExpr::cast)) {
            (ast::Expr::BinaryExpr(inner), Some(outer)) => {
                match (
                    inner.op_kind().map(precedence),
                    outer.op_kind().map(precedence),
                ) {
                    (Some(i), Some(o)) => {
                        let is_lhs = outer.lhs().is_some_and(|l| l.syntax() == &target);
                        i > o || (i == o && is_lhs)
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        let replacement = if is_atomic(&init) || safe_context || binds_tighter {
            init_text.clone()
        } else {
            format!("({init_text})")
        };
        edits.push((
            reference.file_id,
            TextEdit::replace(target.text_range(), replacement),
        ));
    }
    let name = field_data.name.to_string();
    out.push(Assist {
        id: "inline_constant",
        label: base_db::tr!("内联常量 {name}", "Inline constant {name}"),
        kind: AssistKind::RefactorInline,
        change: group_edits(edits),
    });
}

/// 二元运算符的优先级，越大结合得越紧。
fn precedence(op: SyntaxKind) -> u8 {
    match op {
        SyntaxKind::PIPE_PIPE => 1,
        SyntaxKind::AMP_AMP => 2,
        SyntaxKind::PIPE => 3,
        SyntaxKind::CARET => 4,
        SyntaxKind::AMP => 5,
        SyntaxKind::EQ_EQ | SyntaxKind::NEQ => 6,
        SyntaxKind::LT | SyntaxKind::GT | SyntaxKind::LT_EQ | SyntaxKind::GT_EQ => 7,
        SyntaxKind::SHL | SyntaxKind::SHR | SyntaxKind::USHR => 8,
        SyntaxKind::PLUS | SyntaxKind::MINUS => 9,
        SyntaxKind::STAR | SyntaxKind::SLASH | SyntaxKind::PERCENT => 10,
        _ => 0,
    }
}

/// 在类体里插入一个字段：放在已有字段后面，没有字段时放在类体开头。
fn insert_field(ctx: &AssistCtx<'_>, class_decl: &ast::TypeDecl, line: &str) -> Option<TextEdit> {
    let body = class_decl.body()?;
    let class_indent = ctx.indent_at(class_decl.syntax().text_range().start());
    let member_indent = format!("{class_indent}    ");
    let last_field = body
        .members()
        .filter(|m| matches!(m, ast::Member::FieldDecl(_)))
        .last();
    match last_field {
        Some(field) => Some(TextEdit::insert(
            field.syntax().text_range().end(),
            format!("\n{member_indent}{line}"),
        )),
        None => {
            let open = body
                .syntax()
                .first_token()
                .filter(|t| t.kind() == SyntaxKind::L_CURLY)?;
            Some(TextEdit::insert(
                open.text_range().end(),
                format!("\n{member_indent}{line}\n"),
            ))
        }
    }
}

/// 选中的表达式提取成字段。表达式不用局部变量时成为字段的初值（`private final`），
/// 否则在当前语句前面给字段赋值。
pub(super) fn extract_field(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    if ctx.range.is_empty() {
        return;
    }
    let selected = &ctx.text[usize::from(ctx.range.start())..usize::from(ctx.range.end())];
    let leading = selected.len() - selected.trim_start().len();
    let trailing = selected.len() - selected.trim_end().len();
    let range = TextRange::new(
        ctx.range.start() + TextSize::from(leading as u32),
        ctx.range.end() - TextSize::from(trailing as u32),
    );
    let Some(expr) = node_at(&ctx.root, range)
        .and_then(|n| n.ancestors().find_map(ast::Expr::cast))
        .filter(|e| e.syntax().text_range() == range)
    else {
        return;
    };
    // 赋值的左边、lambda、单独的名字没有提取的意义
    if matches!(expr, ast::Expr::LambdaExpr(_) | ast::Expr::MethodRef(_))
        || expr
            .syntax()
            .parent()
            .and_then(ast::AssignExpr::cast)
            .and_then(|a| a.lhs())
            .is_some_and(|lhs| lhs.syntax() == expr.syntax())
    {
        return;
    }
    let Some(ty) = ctx.sema.type_of_expr(ctx.file, &expr) else {
        return;
    };
    if ty.contains_unknown() || matches!(ty, Ty::Null | Ty::Void | Ty::Intersection(_)) {
        return;
    }
    // 所在的成员和语句
    let Some(member) = expr.syntax().ancestors().find(|n| {
        matches!(
            n.kind(),
            SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL | SyntaxKind::INITIALIZER_BLOCK
        )
    }) else {
        return;
    };
    let Some(class_decl) = member.ancestors().find_map(ast::TypeDecl::cast) else {
        return;
    };
    if class_decl.syntax().kind() == SyntaxKind::INTERFACE_DECL
        || ctx.sema.class_of_decl(ctx.file, &class_decl).is_none()
    {
        return;
    }
    // 在 lambda / 匿名类里提取会改变求值时机，不提供
    if expr
        .syntax()
        .ancestors()
        .take_while(|n| n != &member)
        .any(|n| matches!(n.kind(), SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY))
    {
        return;
    }
    let is_static = member
        .children()
        .find_map(ast::ModifierList::cast)
        .is_some_and(|m| m.has(SyntaxKind::STATIC_KW))
        || member
            .children_with_tokens()
            .any(|e| e.kind() == SyntaxKind::STATIC_KW);
    let uses_locals = expr
        .syntax()
        .descendants()
        .filter_map(ast::NameRef::cast)
        .any(|n| {
            matches!(
                ctx.sema.classify_name_ref(ctx.file, &n),
                Some(Definition::Local(_))
            )
        });
    let Some(stmt) = expr
        .syntax()
        .ancestors()
        .find(|n| n.parent().is_some_and(|p| p.kind() == SyntaxKind::BLOCK))
    else {
        return;
    };
    // 字段名：按表达式起名，避开已有的字段和体里用到的名字
    let mut name = super::fresh_local_name(&expr, &stmt);
    let fields = super::fields_of(&class_decl);
    let base = name.clone();
    let mut n = 2;
    while fields.iter().any(|f| f.name == name) {
        name = format!("{base}{n}");
        n += 1;
    }
    let ty_text = ty.display(db);
    let modifiers = match (is_static, uses_locals) {
        (true, false) => "private static final",
        (true, true) => "private static",
        (false, false) => "private final",
        (false, true) => "private",
    };
    let expr_text = expr.syntax().text().to_string();
    let mut edits = Vec::new();
    let field_line = if uses_locals {
        format!("{modifiers} {ty_text} {name};")
    } else {
        format!("{modifiers} {ty_text} {name} = {expr_text};")
    };
    let Some(field_edit) = insert_field(ctx, &class_decl, &field_line) else {
        return;
    };
    edits.push(field_edit);
    if let Some(import) = import_for(ctx, &ty, expr.syntax()) {
        edits.push(import);
    }
    if uses_locals {
        let indent = ctx.indent_at(stmt.text_range().start());
        let qualified = if is_static {
            name.clone()
        } else {
            format!("this.{name}")
        };
        // 表达式就是整条表达式语句时直接换成赋值
        let is_whole_statement = stmt.kind() == SyntaxKind::EXPR_STMT
            && stmt.children().next().as_ref() == Some(expr.syntax());
        if is_whole_statement {
            edits.push(TextEdit::replace(
                expr.syntax().text_range(),
                format!("{qualified} = {expr_text}"),
            ));
        } else {
            edits.push(TextEdit::insert(
                stmt.text_range().start(),
                format!("{qualified} = {expr_text};\n{indent}"),
            ));
            edits.push(TextEdit::replace(expr.syntax().text_range(), name.clone()));
        }
    } else {
        edits.push(TextEdit::replace(expr.syntax().text_range(), name.clone()));
    }
    out.push(ctx.assist(
        "extract_field",
        base_db::trs!("提取为字段", "Extract field"),
        AssistKind::RefactorExtract,
        edits,
    ));
}

/// 在字段名上：为字段类型的公开实例方法生成转发方法（类里已有同名同参数个数的方法时跳过）。
pub(super) fn generate_delegates(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(name) = token.parent().and_then(ast::Name::cast) else {
        return;
    };
    let Some(declarator) = name
        .syntax()
        .parent()
        .and_then(ast::VariableDeclarator::cast)
    else {
        return;
    };
    if declarator
        .syntax()
        .parent()
        .is_none_or(|p| p.kind() != SyntaxKind::FIELD_DECL)
    {
        return;
    }
    let Some(hir::BodyId::Field(field)) = ctx.sema.body_of(ctx.file, declarator.syntax()) else {
        return;
    };
    let Some((class, decl)) = ctx.enclosing_class() else {
        return;
    };
    if class != field.class {
        return;
    }
    let Some(data) = class_data(db, class) else {
        return;
    };
    let field_ty = hir::field_ty(db, field);
    if !matches!(field_ty, Ty::Class(..)) {
        return;
    }
    let field_name = name.text();
    let mut seen: Vec<(String, usize)> = data
        .methods
        .iter()
        .map(|m| (m.name.to_string(), m.params.len()))
        .collect();
    let mut members: Vec<Vec<String>> = Vec::new();
    for candidate in hir::all_methods(db, &field_ty) {
        let Some(owner) = class_data(db, candidate.id.class) else {
            continue;
        };
        let Some(method) = owner.methods.get(candidate.id.idx as usize) else {
            continue;
        };
        if &*owner.binary_name == "java/lang/Object"
            || method.is_constructor
            || method.flags.is_static()
            || method.flags.visibility() != hir::Visibility::Public
            || method.flags.contains(hir::Modifiers::SYNTHETIC)
        {
            continue;
        }
        let key = (method.name.to_string(), method.params.len());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let params = ide_db::render::method_params(db, candidate.id, &candidate.subst);
        let args: Vec<&str> = params
            .iter()
            .filter_map(|p| p.split_whitespace().last())
            .collect();
        let ret = candidate.ret_ty(db);
        let type_params = if method.type_params.is_empty() {
            String::new()
        } else {
            let decls: Vec<String> = (0..method.type_params.len())
                .map(|idx| {
                    ide_db::render::type_param_decl(
                        db,
                        hir::TypeParamId {
                            owner: hir::GenericOwner::Method(candidate.id),
                            idx: idx as u32,
                        },
                    )
                })
                .collect();
            format!("<{}> ", decls.join(", "))
        };
        let sig = hir::method_sig(db, candidate.id);
        let throws = if sig.throws.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = sig.throws.iter().map(|t| t.display(db)).collect();
            format!(" throws {}", names.join(", "))
        };
        let call = format!("{field_name}.{}({})", method.name, args.join(", "));
        let body = if ret == Ty::Void {
            format!("    {call};")
        } else {
            format!("    return {call};")
        };
        members.push(vec![
            format!(
                "public {type_params}{} {}({}){throws} {{",
                ret.display(db),
                method.name,
                params.join(", ")
            ),
            body,
            "}".to_owned(),
        ]);
    }
    if members.is_empty() {
        return;
    }
    let count = members.len();
    let Some(edit) = ctx.insert_members(&decl, &members) else {
        return;
    };
    out.push(ctx.assist(
        "generate_delegates",
        base_db::tr!(
            "为 `{field_name}` 生成 {count} 个委托方法",
            "Generate {count} delegate method(s) for `{field_name}`"
        ),
        AssistKind::Generate,
        vec![edit],
    ));
}
