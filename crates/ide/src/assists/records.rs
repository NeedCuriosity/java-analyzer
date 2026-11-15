//! 把只装数据的类转换成 `record`（需要 Java 16+）。
//!
//! 适用的类：不继承别的类、不是抽象类、没有子类；实例字段全是没有初值的 `private final`；
//! 有一个构造器按顺序给每个字段赋值（`this.x = x;`），其它构造器都委托给 `this(...)`。
//! 转换后字段成为记录组件，构造器和平凡的访问器（`x()` 返回 `x`）删掉；构造器里赋值之前的
//! 校验语句保留在紧凑构造器里。`getX()` 之类的方法、静态成员和别的方法原样保留。

use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use super::{refactor::statement_removal_range, Assist, AssistCtx, AssistKind};

struct Field {
    name: String,
    /// 去掉空白后的类型原文（`int[]`），用来和构造器形参比较
    ty_key: String,
    /// 字段上的注解原文
    annotations: Vec<String>,
}

fn type_key(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace("...", "[]")
}

/// `this.x` 或 `x`（`x` 不是同名形参时）形式的赋值目标字段名。
fn assigned_field(lhs: &ast::Expr, params: &[String]) -> Option<String> {
    match lhs {
        ast::Expr::FieldAccess(access) => {
            let receiver = access.receiver()?;
            let ast::Expr::ThisExpr(this) = receiver else {
                return None;
            };
            if this.qualifier().is_some() {
                return None;
            }
            Some(access.name_ref()?.text().to_string())
        }
        ast::Expr::NameExpr(name) => {
            let name = name.name_ref()?.text().to_string();
            (!params.contains(&name)).then_some(name)
        }
        _ => None,
    }
}

/// 构造器语句 `this.f = p;` 拆成 `(f, p)`。
fn field_assignment(stmt: &ast::Stmt, params: &[String]) -> Option<(String, String)> {
    let ast::Stmt::ExprStmt(stmt) = stmt else {
        return None;
    };
    let ast::Expr::AssignExpr(assign) = stmt.expr()? else {
        return None;
    };
    if assign.op_token()?.kind() != SyntaxKind::EQ {
        return None;
    }
    let field = assigned_field(&assign.lhs()?, params)?;
    let ast::Expr::NameExpr(rhs) = assign.rhs()? else {
        return None;
    };
    let param = rhs.name_ref()?.text().to_string();
    params.contains(&param).then_some((field, param))
}

/// 语句（或其中任何位置）给某个字段赋值。
fn assigns_field(node: &SyntaxNode, fields: &[Field], params: &[String]) -> bool {
    let is_field = |target: Option<ast::Expr>| {
        target
            .and_then(|e| assigned_field(&e, params))
            .is_some_and(|f| fields.iter().any(|field| field.name == f))
    };
    node.descendants().any(|n| match n.kind() {
        SyntaxKind::ASSIGN_EXPR => is_field(ast::AssignExpr::cast(n).and_then(|a| a.lhs())),
        SyntaxKind::POSTFIX_EXPR => is_field(n.children().find_map(ast::Expr::cast)),
        SyntaxKind::UNARY_EXPR => {
            n.first_token().is_some_and(|t| {
                matches!(t.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS)
            }) && is_field(n.children().find_map(ast::Expr::cast))
        }
        _ => false,
    })
}

fn is_delegating(ctor: &ast::ConstructorDecl) -> bool {
    let Some(first) = ctor.body().and_then(|b| {
        b.syntax()
            .children()
            .find(|n| n.kind() != SyntaxKind::EMPTY_STMT)
    }) else {
        return false;
    };
    first.kind() == SyntaxKind::EXPR_STMT
        && first.first_child().is_some_and(|c| {
            c.kind() == SyntaxKind::EXPLICIT_CONSTRUCTOR_CALL
                && c.first_token()
                    .is_some_and(|t| t.kind() == SyntaxKind::THIS_KW)
        })
}

fn access_rank(mods: Option<&ast::ModifierList>) -> u8 {
    match mods {
        Some(m) if m.has(SyntaxKind::PUBLIC_KW) => 3,
        Some(m) if m.has(SyntaxKind::PROTECTED_KW) => 2,
        Some(m) if m.has(SyntaxKind::PRIVATE_KW) => 0,
        _ => 1,
    }
}

fn access_keyword(mods: Option<&ast::ModifierList>) -> &'static str {
    match access_rank(mods) {
        3 => "public ",
        2 => "protected ",
        0 => "private ",
        _ => "",
    }
}

/// 平凡的访问器：方法体只有 `return x;` / `return this.x;`。
fn trivial_accessor(method: &ast::MethodDecl, field: &str) -> bool {
    let Some(body) = method.body() else {
        return false;
    };
    let stmts: Vec<ast::Stmt> = body.statements().collect();
    let [ast::Stmt::ReturnStmt(ret)] = stmts.as_slice() else {
        return false;
    };
    match ret.expr() {
        Some(ast::Expr::NameExpr(e)) => e.name_ref().is_some_and(|r| r.text() == field),
        Some(ast::Expr::FieldAccess(a)) => {
            matches!(a.receiver(), Some(ast::Expr::ThisExpr(t)) if t.qualifier().is_none())
                && a.name_ref().is_some_and(|r| r.text() == field)
        }
        _ => false,
    }
}

/// 删除若干成员；删掉后留下的多余空行一并去掉。
fn member_removals(text: &str, members: &[SyntaxNode]) -> Vec<TextRange> {
    let mut ranges: Vec<TextRange> = members
        .iter()
        .map(|m| statement_removal_range(text, m.text_range()))
        .collect();
    ranges.sort_by_key(|r| r.start());
    let mut merged: Vec<TextRange> = Vec::new();
    for range in ranges {
        // 两个删除之间只有空白：合并成一个
        if let Some(last) = merged.last_mut() {
            let between = &text[usize::from(last.end())..usize::from(range.start())];
            if between.trim().is_empty() {
                *last = TextRange::new(last.start(), range.end().max(last.end()));
                continue;
            }
        }
        merged.push(range);
    }
    for range in &mut merged {
        let start = usize::from(range.start());
        let end = usize::from(range.end());
        let before = text[..start].trim_end_matches([' ', '\t']);
        let preceded_by_gap = before.ends_with("\n\n") || before.ends_with("{\n");
        let blank_after = text[end..]
            .find('\n')
            .filter(|&i| text[end..end + i].trim().is_empty());
        if let (true, Some(i)) = (preceded_by_gap, blank_after) {
            *range = TextRange::new(range.start(), TextSize::from((end + i + 1) as u32));
        }
    }
    merged
}

pub(super) fn convert_to_record(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = ctx.root.token_at_offset(ctx.range.start()).right_biased() else {
        return;
    };
    let Some(decl) = token.parent_ancestors().find_map(ast::TypeDecl::cast) else {
        return;
    };
    if decl.syntax().kind() != SyntaxKind::CLASS_DECL {
        return;
    }
    let Some(body) = decl.body() else {
        return;
    };
    // 只在类头上提供（类体里光标太多地方都会命中）
    if ctx.range.start() >= body.syntax().text_range().start() {
        return;
    }
    if ide_diagnostics::language_level(ctx.db, ctx.file).is_some_and(|l| l < 16) {
        return;
    }
    if decl.extends_clause().is_some() || decl.permits_clause().is_some() {
        return;
    }
    let class_mods = decl.modifier_list();
    if class_mods.as_ref().is_some_and(|m| {
        m.has(SyntaxKind::ABSTRACT_KW)
            || m.has(SyntaxKind::SEALED_KW)
            || m.has(SyntaxKind::NON_SEALED_KW)
    }) {
        return;
    }
    // 记录类隐式是 static 的：内部类（会用外部实例）和局部类不转换
    let container = decl.syntax().parent().and_then(|p| p.parent());
    match decl.syntax().parent().map(|p| p.kind()) {
        Some(SyntaxKind::CLASS_BODY) => {
            let in_interface = container.as_ref().is_some_and(|c| {
                matches!(
                    c.kind(),
                    SyntaxKind::INTERFACE_DECL | SyntaxKind::ANNOTATION_TYPE_DECL
                )
            });
            let is_static = class_mods
                .as_ref()
                .is_some_and(|m| m.has(SyntaxKind::STATIC_KW));
            if !in_interface && !is_static {
                return;
            }
        }
        Some(SyntaxKind::COMPILATION_UNIT) => {}
        _ => return,
    }
    let Some(class) = ctx.sema.class_of_decl(ctx.file, &decl) else {
        return;
    };
    if !hir::source_subclasses(ctx.db, class).is_empty() {
        return;
    }
    let (Some(name), Some(class_kw)) = (
        decl.name(),
        decl.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == SyntaxKind::CLASS_KW),
    ) else {
        return;
    };
    let class_name = name.text().to_string();

    // 实例字段
    let mut fields: Vec<Field> = Vec::new();
    let mut removed: Vec<SyntaxNode> = Vec::new();
    for member in body.syntax().children() {
        match member.kind() {
            SyntaxKind::FIELD_DECL => {
                let Some(field) = ast::FieldDecl::cast(member.clone()) else {
                    return;
                };
                let mods = field.modifier_list();
                if mods.as_ref().is_some_and(|m| m.has(SyntaxKind::STATIC_KW)) {
                    continue;
                }
                let Some(mods) = mods else { return };
                let keywords: Vec<SyntaxKind> = mods.keywords().map(|t| t.kind()).collect();
                if keywords.len() != 2
                    || !keywords.contains(&SyntaxKind::PRIVATE_KW)
                    || !keywords.contains(&SyntaxKind::FINAL_KW)
                {
                    return;
                }
                let Some(ty) = field.ty() else { return };
                let annotations: Vec<String> = mods
                    .annotations()
                    .map(|a| a.syntax().text().to_string())
                    .collect();
                for declarator in field.declarators() {
                    let Some(name) = declarator.name() else {
                        return;
                    };
                    if declarator.initializer().is_some() {
                        return;
                    }
                    fields.push(Field {
                        name: name.text().to_string(),
                        ty_key: type_key(&format!(
                            "{}{}",
                            ty.syntax().text(),
                            "[]".repeat(declarator.extra_dims())
                        )),
                        annotations: annotations.clone(),
                    });
                }
                removed.push(member);
            }
            // 实例初始化块在记录里不允许
            SyntaxKind::INITIALIZER_BLOCK
                if !ast::InitializerBlock::cast(member).is_some_and(|b| b.is_static()) =>
            {
                return;
            }
            _ => {}
        }
    }
    if fields.is_empty() {
        return;
    }

    // 构造器：一个规范构造器，其余都委托给 this(...)
    let mut canonical: Option<(ast::ConstructorDecl, Vec<String>, Vec<SyntaxNode>)> = None;
    for ctor in body
        .syntax()
        .children()
        .filter_map(ast::ConstructorDecl::cast)
    {
        if is_delegating(&ctor) {
            continue;
        }
        if canonical.is_some() {
            return;
        }
        let (Some(params), Some(ctor_body)) = (ctor.param_list(), ctor.body()) else {
            return;
        };
        if ctor.throws_clause().is_some() || ctor.type_param_list().is_some() {
            return;
        }
        // 私有构造器（工厂方法模式）转换后会变得可以直接调用
        if access_rank(ctor.modifier_list().as_ref()) == 0 && access_rank(class_mods.as_ref()) != 0
        {
            return;
        }
        let params: Vec<ast::Param> = params.params().collect();
        if params.len() != fields.len() {
            return;
        }
        let param_names: Vec<String> = params
            .iter()
            .filter_map(|p| p.name().map(|n| n.text().to_string()))
            .collect();
        if param_names.len() != params.len() {
            return;
        }
        let stmts: Vec<ast::Stmt> = ctor_body.statements().collect();
        if stmts.len() != ctor_body.syntax().children().count() {
            return;
        }
        // 末尾连续的字段赋值
        let mut assignments: Vec<(String, String)> = Vec::new();
        let mut split = stmts.len();
        while split > 0 {
            match field_assignment(&stmts[split - 1], &param_names) {
                Some(pair) => {
                    assignments.push(pair);
                    split -= 1;
                }
                None => break,
            }
        }
        if assignments.len() != fields.len() {
            return;
        }
        // 第 i 个形参必须赋给某个字段，且每个字段恰好赋一次；组件顺序按形参顺序
        let mut order: Vec<usize> = Vec::new();
        for (i, param) in params.iter().enumerate() {
            let Some((field, _)) = assignments.iter().find(|(_, p)| p == &param_names[i]) else {
                return;
            };
            let Some(index) = fields.iter().position(|f| &f.name == field) else {
                return;
            };
            if order.contains(&index) {
                return;
            }
            let Some(ty) = param.ty() else { return };
            let param_key = type_key(&format!(
                "{}{}{}",
                ty.syntax().text(),
                "[]".repeat(param.extra_dims()),
                if param.is_varargs() { "[]" } else { "" }
            ));
            if param_key != fields[index].ty_key {
                return;
            }
            order.push(index);
        }
        // `super();` 在记录里不允许，也没有意义，直接丢掉
        let prefix: Vec<SyntaxNode> = stmts[..split]
            .iter()
            .map(|s| s.syntax().clone())
            .filter(|s| type_key(&s.text().to_string()) != "super();")
            .collect();
        if !prefix.is_empty() {
            // 紧凑构造器里不能有 return，也不能再调用别的构造器
            if prefix.iter().any(|s| {
                s.descendants().any(|n| {
                    matches!(
                        n.kind(),
                        SyntaxKind::RETURN_STMT | SyntaxKind::EXPLICIT_CONSTRUCTOR_CALL
                    )
                })
            }) {
                return;
            }
            // 紧凑构造器里形参名就是组件名；前面的语句也不能直接给字段赋值
            if params
                .iter()
                .zip(&order)
                .any(|(p, &i)| p.name().is_none_or(|n| n.text() != fields[i].name.as_str()))
            {
                return;
            }
            if prefix
                .iter()
                .any(|s| assigns_field(s, &fields, &param_names) || has_this_use(s))
            {
                return;
            }
        }
        let components: Vec<String> = params
            .iter()
            .zip(&order)
            .map(|(p, &i)| {
                let ty = p
                    .ty()
                    .map(|t| t.syntax().text().to_string())
                    .unwrap_or_default();
                let mut text = String::new();
                for annotation in &fields[i].annotations {
                    text.push_str(annotation);
                    text.push(' ');
                }
                if p.is_varargs() {
                    text.push_str(&format!("{ty}... {}", fields[i].name));
                } else {
                    let dims = "[]".repeat(p.extra_dims());
                    text.push_str(&format!("{ty}{dims} {}", fields[i].name));
                }
                text
            })
            .collect();
        canonical = Some((ctor, components, prefix));
    }
    let Some((ctor, components, prefix)) = canonical else {
        return;
    };

    // 和字段同名的无参方法：平凡的删掉；其它的必须是 public（记录的访问器要求）
    for method in body.syntax().children().filter_map(ast::MethodDecl::cast) {
        let Some(method_name) = method.name() else {
            continue;
        };
        let no_params = method
            .param_list()
            .is_none_or(|p| p.params().next().is_none());
        if !no_params || !fields.iter().any(|f| f.name == method_name.text().as_str()) {
            continue;
        }
        let mods = method.modifier_list();
        if mods.as_ref().is_some_and(|m| m.has(SyntaxKind::STATIC_KW)) {
            continue;
        }
        let same_type = method.return_type().is_some_and(|t| {
            fields.iter().any(|f| {
                f.name == method_name.text().as_str()
                    && f.ty_key
                        == type_key(&format!(
                            "{}{}",
                            t.syntax().text(),
                            "[]".repeat(method.extra_dims())
                        ))
            })
        });
        if !same_type {
            return;
        }
        if trivial_accessor(&method, method_name.text().as_str())
            && method.throws_clause().is_none()
            && method.type_param_list().is_none()
        {
            removed.push(method.syntax().clone());
        } else if access_rank(mods.as_ref()) != 3 || method.throws_clause().is_some() {
            return;
        }
    }

    let mut edits: Vec<TextEdit> = Vec::new();
    // `final class` → `record`
    if let Some(final_kw) = class_mods
        .as_ref()
        .and_then(|m| m.keywords().find(|t| t.kind() == SyntaxKind::FINAL_KW))
    {
        let start = final_kw.text_range().start();
        let mut end = final_kw.text_range().end();
        let rest = &ctx.text[usize::from(end)..];
        end += TextSize::from((rest.len() - rest.trim_start().len()) as u32);
        edits.push(TextEdit::replace(TextRange::new(start, end), String::new()));
    }
    edits.push(TextEdit::replace(
        class_kw.text_range(),
        "record".to_owned(),
    ));
    let header_end = decl
        .type_param_list()
        .map(|t| t.syntax().text_range().end())
        .unwrap_or_else(|| name.syntax().text_range().end());
    edits.push(TextEdit::insert(
        header_end,
        format!("({})", components.join(", ")),
    ));

    if prefix.is_empty() {
        removed.push(ctor.syntax().clone());
    } else {
        // 规范构造器 → 紧凑构造器，只留下赋值之前的语句
        let indent = ctx.indent_at(ctor.syntax().text_range().start());
        let mut text = format!("{}{class_name} {{\n", access_keyword(class_mods.as_ref()));
        for stmt in &prefix {
            let stmt_indent = ctx.indent_at(stmt.text_range().start());
            let stmt_text = stmt.text().to_string();
            text.push_str(&stmt_indent);
            text.push_str(&stmt_text);
            text.push('\n');
        }
        text.push_str(&indent);
        text.push('}');
        // 构造器上的注释和注解保留，从修饰符关键字（没有就从名字）开始替换
        let Some(start) = ctor
            .modifier_list()
            .and_then(|m| m.keywords().next())
            .map(|k| k.text_range().start())
            .or_else(|| ctor.name().map(|n| n.syntax().text_range().start()))
        else {
            return;
        };
        edits.push(TextEdit::replace(
            TextRange::new(start, ctor.syntax().text_range().end()),
            text,
        ));
    }
    for range in member_removals(ctx.text, &removed) {
        edits.push(TextEdit::replace(range, String::new()));
    }
    out.push(ctx.assist(
        "convert_to_record",
        base_db::tr!(
            "把 `{class_name}` 转换成记录类",
            "Convert `{class_name}` to a record"
        ),
        AssistKind::RefactorRewrite,
        edits,
    ));
}

/// 语句里用到了 `this`（紧凑构造器里字段还没赋值，不能照搬）。
fn has_this_use(node: &SyntaxNode) -> bool {
    node.descendants()
        .any(|n| n.kind() == SyntaxKind::THIS_EXPR)
}
