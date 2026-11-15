//! 更多针对诊断的快速修复：删除未使用的符号、新建类、修改可见性、去掉 `final`、改变量 /
//! 返回类型、补 `return`、把类改成 abstract。

use base_db::FileKind;
use hir::{class_data, Definition, Modifiers, Ty};
use ide_db::TextEdit;
use ide_diagnostics::DiagnosticCode;
use syntax::{
    ast::{self, AstNode},
    NodeOrToken, SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use super::{
    fixes::{import_for, name_for_type},
    refactor::{is_pure_argument, list_element_removal, negate, statement_removal_range},
    Assist, AssistCtx, AssistKind,
};
use crate::rename::SourceChange;

fn node_at(root: &SyntaxNode, range: TextRange) -> Option<SyntaxNode> {
    match root.covering_element(range) {
        NodeOrToken::Node(node) => Some(node),
        NodeOrToken::Token(token) => token.parent(),
    }
}

/// 可以单独成为语句的表达式（方法调用、`new`、赋值、自增自减）。
fn is_statement_expression(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::MethodCall(_)
        | ast::Expr::NewExpr(_)
        | ast::Expr::AssignExpr(_)
        | ast::Expr::PostfixExpr(_) => true,
        ast::Expr::UnaryExpr(u) => u
            .op_token()
            .is_some_and(|t| matches!(t.text(), "++" | "--")),
        _ => false,
    }
}

/// 删除声明（连同它所在的整行和前面的文档注释）。
fn removal_of_decl(text: &str, decl: &SyntaxNode) -> TextRange {
    statement_removal_range(text, decl.text_range())
}

/// 未使用的局部变量 / 私有字段 / 私有方法：删掉它。初值有副作用时保留初值表达式。
pub(super) fn remove_unused_symbol(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let diagnostics = ide_diagnostics::unused_symbols(ctx.db, ctx.file);
    let Some(diagnostic) = diagnostics
        .iter()
        .find(|d| d.range.contains_inclusive(ctx.range.start()))
    else {
        return;
    };
    let Some(node) = node_at(&ctx.root, diagnostic.range) else {
        return;
    };
    match diagnostic.code {
        DiagnosticCode::UnusedLocal | DiagnosticCode::UnusedPrivateMember => {}
        DiagnosticCode::UnusedAssignment => {
            remove_assignment(ctx, &node, diagnostic.range, out);
            return;
        }
        _ => return,
    }
    if let Some(method) = node.ancestors().find_map(ast::MethodDecl::cast) {
        if method
            .name()
            .is_some_and(|n| n.syntax().text_range() == diagnostic.range)
            || method.syntax().text_range() == diagnostic.range
        {
            let name = method
                .name()
                .map(|n| n.text().to_string())
                .unwrap_or_default();
            out.push(ctx.assist(
                "remove_unused",
                base_db::tr!("删除未使用的方法 `{name}`", "Remove unused method `{name}`"),
                AssistKind::QuickFix,
                vec![TextEdit::replace(
                    removal_of_decl(ctx.text, method.syntax()),
                    String::new(),
                )],
            ));
            return;
        }
    }
    let Some(declarator) = node.ancestors().find_map(ast::VariableDeclarator::cast) else {
        return;
    };
    let Some(decl) = declarator.syntax().parent() else {
        return;
    };
    if !matches!(
        decl.kind(),
        SyntaxKind::LOCAL_VAR_DECL | SyntaxKind::FIELD_DECL
    ) {
        return;
    }
    // 局部变量必须是块里的语句（不是 for 的初始化部分）
    if decl.kind() == SyntaxKind::LOCAL_VAR_DECL
        && !decl
            .parent()
            .is_some_and(|p| matches!(p.kind(), SyntaxKind::BLOCK | SyntaxKind::SWITCH_CASE))
    {
        return;
    }
    let name = declarator
        .name()
        .map(|n| n.text().to_string())
        .unwrap_or_default();
    let declarators: Vec<ast::VariableDeclarator> = decl
        .children()
        .filter_map(ast::VariableDeclarator::cast)
        .collect();
    let init = declarator.initializer();
    let pure = init.as_ref().is_none_or(is_pure_argument);
    let keeps_side_effects = !pure && init.as_ref().is_some_and(is_statement_expression);
    if !pure && !keeps_side_effects {
        return;
    }
    let edit = if declarators.len() == 1 {
        if keeps_side_effects {
            let init = init.unwrap();
            TextEdit::replace(decl.text_range(), format!("{};", init.syntax().text()))
        } else {
            TextEdit::replace(removal_of_decl(ctx.text, &decl), String::new())
        }
    } else {
        if keeps_side_effects {
            return;
        }
        let ranges: Vec<TextRange> = declarators
            .iter()
            .map(|d| d.syntax().text_range())
            .collect();
        let Some(idx) = declarators.iter().position(|d| d == &declarator) else {
            return;
        };
        let Some(range) = list_element_removal(&ranges, idx) else {
            return;
        };
        TextEdit::replace(range, String::new())
    };
    let label = match (decl.kind(), keeps_side_effects) {
        (SyntaxKind::FIELD_DECL, _) => {
            base_db::tr!("删除未使用的字段 `{name}`", "Remove unused field `{name}`")
        }
        (_, true) => base_db::tr!(
            "删除变量 `{name}`，保留初值的副作用",
            "Remove variable `{name}`, keeping side effects"
        ),
        _ => base_db::tr!(
            "删除未使用的变量 `{name}`",
            "Remove unused variable `{name}`"
        ),
    };
    out.push(ctx.assist("remove_unused", label, AssistKind::QuickFix, vec![edit]));
}

/// 从未被读到的赋值：删掉初值，或者删掉赋值语句（右边有副作用时保留成表达式语句）。
fn remove_assignment(
    ctx: &AssistCtx<'_>,
    node: &SyntaxNode,
    value: TextRange,
    out: &mut Vec<Assist>,
) {
    let Some(value_expr) = node
        .ancestors()
        .find_map(ast::Expr::cast)
        .filter(|e| e.syntax().text_range() == value)
    else {
        return;
    };
    let Some(parent) = value_expr.syntax().parent() else {
        return;
    };
    let pure = is_pure_argument(&value_expr);
    let edit = match parent.kind() {
        SyntaxKind::VARIABLE_DECLARATOR => {
            if !pure {
                return;
            }
            let Some(name) = ast::VariableDeclarator::cast(parent.clone()).and_then(|d| d.name())
            else {
                return;
            };
            TextEdit::replace(
                TextRange::new(name.syntax().text_range().end(), value.end()),
                String::new(),
            )
        }
        SyntaxKind::ASSIGN_EXPR => {
            let Some(stmt) = parent
                .parent()
                .filter(|p| p.kind() == SyntaxKind::EXPR_STMT)
            else {
                return;
            };
            if pure {
                TextEdit::replace(
                    statement_removal_range(ctx.text, stmt.text_range()),
                    String::new(),
                )
            } else if is_statement_expression(&value_expr) {
                TextEdit::replace(
                    stmt.text_range(),
                    format!("{};", value_expr.syntax().text()),
                )
            } else {
                return;
            }
        }
        _ => return,
    };
    out.push(ctx.assist(
        "remove_unused",
        base_db::trs!("删除多余的赋值", "Remove redundant assignment"),
        AssistKind::QuickFix,
        vec![edit],
    ));
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NewKind {
    Class,
    Interface,
    Enum,
    Record,
    Annotation,
    Exception,
}

/// 光标在一个解析不了、以大写字母开头的类型名上：在当前包里新建这个类型（新文件）。
pub(super) fn create_class(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    if token.kind() != SyntaxKind::IDENT {
        return;
    }
    let Some(name_ref) = token.parent().and_then(ast::NameRef::cast) else {
        return;
    };
    let Some(parent) = name_ref.syntax().parent() else {
        return;
    };
    let name = name_ref.text().to_string();
    if !name.starts_with(|c: char| c.is_ascii_uppercase())
        || ctx.sema.classify_name_ref(ctx.file, &name_ref).is_some()
    {
        return;
    }
    let mut type_args = 0;
    let kinds: Vec<NewKind> = match parent.kind() {
        SyntaxKind::CLASS_TYPE => {
            let class_ty = ast::ClassType::cast(parent.clone()).unwrap();
            if class_ty.qualifier().is_some() {
                return;
            }
            type_args = class_ty.type_arg_list().map_or(0, |l| l.args().count());
            // 往上越过外层的类型（`List<Foo>` 里的 `Foo` 不算在 implements 里）
            let context = parent.parent();
            match context.as_ref().map(|c| c.kind()) {
                Some(SyntaxKind::IMPLEMENTS_CLAUSE) => vec![NewKind::Interface],
                Some(SyntaxKind::EXTENDS_CLAUSE) => {
                    let in_interface = context
                        .as_ref()
                        .and_then(|c| c.parent())
                        .is_some_and(|d| d.kind() == SyntaxKind::INTERFACE_DECL);
                    if in_interface {
                        vec![NewKind::Interface]
                    } else {
                        vec![NewKind::Class]
                    }
                }
                Some(SyntaxKind::THROWS_CLAUSE) => vec![NewKind::Exception],
                Some(SyntaxKind::NEW_EXPR) => vec![NewKind::Class],
                Some(SyntaxKind::PARAM | SyntaxKind::UNION_TYPE)
                    if parent.ancestors().nth(2).is_some_and(|a| {
                        a.kind() == SyntaxKind::CATCH_CLAUSE
                            || a.parent()
                                .is_some_and(|p| p.kind() == SyntaxKind::CATCH_CLAUSE)
                    }) =>
                {
                    vec![NewKind::Exception]
                }
                _ => vec![
                    NewKind::Class,
                    NewKind::Interface,
                    NewKind::Enum,
                    NewKind::Record,
                ],
            }
        }
        // `Foo.bar()` / `Foo.CONSTANT`
        SyntaxKind::NAME_EXPR => {
            let receiver_of = parent.parent().is_some_and(|p| {
                matches!(p.kind(), SyntaxKind::METHOD_CALL | SyntaxKind::FIELD_ACCESS)
                    && p.children().next().as_ref() == Some(&parent)
            });
            if !receiver_of {
                return;
            }
            vec![NewKind::Class, NewKind::Enum]
        }
        SyntaxKind::QUALIFIED_NAME
            if parent
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::ANNOTATION) =>
        {
            vec![NewKind::Annotation]
        }
        _ => return,
    };
    // 新文件和当前文件在同一个包、同一个目录
    let Some(cu) = ast::CompilationUnit::cast(ctx.root.clone()) else {
        return;
    };
    let package = cu
        .package()
        .and_then(|p| p.name())
        .map(|n| n.text())
        .unwrap_or_default();
    let binary = if package.is_empty() {
        name.clone()
    } else {
        format!("{}/{name}", package.replace('.', "/"))
    };
    if hir::lookup_class(ctx.db, &binary).is_some() {
        return;
    }
    let type_params = match type_args {
        0 => String::new(),
        n => {
            let names = ["T", "U", "V", "W", "X", "Y", "Z"];
            format!("<{}>", names[..n.min(names.len())].join(", "))
        }
    };
    // `new Foo(a, b)`：带上匹配实参的构造器
    let mut imports: Vec<String> = Vec::new();
    let mut ctor_params: Option<Vec<String>> = None;
    // `new Foo(a, b)`：光标就在它上面，或者文件里别处有这样的构造
    let new_expr = parent.parent().and_then(ast::NewExpr::cast).or_else(|| {
        ctx.root
            .descendants()
            .filter_map(ast::NewExpr::cast)
            .find(|n| {
                matches!(n.ty(), Some(ast::Type::ClassType(t)) if t.qualifier().is_none()
                && t.name_ref().is_some_and(|r| r.text() == name.as_str()))
            })
    });
    if let Some(new) = new_expr.filter(|_| parent.kind() == SyntaxKind::CLASS_TYPE) {
        if let Some(args) = new.arg_list() {
            let mut params: Vec<String> = Vec::new();
            for arg in args.args() {
                let ty = ctx.sema.type_of_expr(ctx.file, &arg);
                let text = match &ty {
                    Some(ty) if !ty.contains_unknown() && !matches!(ty, Ty::Null | Ty::Void) => {
                        if let Some(class) = ty.as_class() {
                            if let Some(data) = class_data(ctx.db, class) {
                                let pkg = data.package().replace('/', ".");
                                if pkg != "java.lang" && pkg != package {
                                    if let Some(q) = ide_db::imports::importable_name(ctx.db, class)
                                    {
                                        imports.push(q);
                                    }
                                }
                            }
                        }
                        ty.display(ctx.db)
                    }
                    _ => "Object".to_owned(),
                };
                let mut param_name = name_for_type(&text);
                let base = param_name.clone();
                let mut n = 2;
                while params
                    .iter()
                    .any(|p| p.ends_with(&format!(" {param_name}")))
                {
                    param_name = format!("{base}{n}");
                    n += 1;
                }
                params.push(format!("{text} {param_name}"));
            }
            if !params.is_empty() {
                ctor_params = Some(params);
            }
        }
    }
    imports.sort();
    imports.dedup();
    for kind in kinds {
        let id = match kind {
            NewKind::Class | NewKind::Exception => "create_class",
            NewKind::Interface => "create_interface",
            NewKind::Enum => "create_enum",
            NewKind::Record => "create_record",
            NewKind::Annotation => "create_annotation",
        };
        let (keyword, label) = match kind {
            NewKind::Class | NewKind::Exception => (
                "class",
                base_db::tr!("新建类 `{name}`", "Create class `{name}`"),
            ),
            NewKind::Interface => (
                "interface",
                base_db::tr!("新建接口 `{name}`", "Create interface `{name}`"),
            ),
            NewKind::Enum => (
                "enum",
                base_db::tr!("新建枚举 `{name}`", "Create enum `{name}`"),
            ),
            NewKind::Record => (
                "record",
                base_db::tr!("新建 record `{name}`", "Create record `{name}`"),
            ),
            NewKind::Annotation => (
                "@interface",
                base_db::tr!("新建注解 `{name}`", "Create annotation `{name}`"),
            ),
        };
        let mut text = String::new();
        if !package.is_empty() {
            text.push_str(&format!("package {package};\n\n"));
        }
        if kind == NewKind::Class && !imports.is_empty() {
            for import in &imports {
                text.push_str(&format!("import {import};\n"));
            }
            text.push('\n');
        }
        let header = match kind {
            NewKind::Record => format!("public record {name}{type_params}()"),
            NewKind::Exception => format!("public class {name} extends Exception"),
            _ => format!("public {keyword} {name}{type_params}"),
        };
        text.push_str(&header);
        match (&ctor_params, kind) {
            (Some(params), NewKind::Class) => {
                text.push_str(&format!(
                    " {{\n    public {name}({}) {{\n    }}\n}}\n",
                    params.join(", ")
                ));
            }
            _ => text.push_str(" {\n}\n"),
        }
        out.push(Assist {
            id,
            label,
            kind: AssistKind::QuickFix,
            change: SourceChange {
                edits: Vec::new(),
                file_renames: Vec::new(),
                file_creates: vec![(ctx.file_id, format!("{name}.java"), text)],
            },
        });
    }
}

/// 声明（字段 / 方法 / 构造器）在它所在文件里的语法节点。
fn decl_node(ctx: &AssistCtx<'_>, def: &Definition) -> Option<(base_db::SourceFile, SyntaxNode)> {
    let source = def.source(ctx.db)?;
    let entry = base_db::workspace(ctx.db)
        .files(ctx.db)
        .get(source.file.file_id(ctx.db))?;
    if !matches!(entry.kind, FileKind::Workspace { .. }) {
        return None;
    }
    let root = base_db::parse(ctx.db, source.file).syntax_node();
    let node = node_at(&root, source.focus_range)?;
    let decl = node.ancestors().find(|n| {
        matches!(
            n.kind(),
            SyntaxKind::FIELD_DECL | SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL
        ) || ast::TypeDecl::can_cast(n.kind())
    })?;
    Some((source.file, decl))
}

/// 把声明的可见性改成 `visibility`（`None` 表示包内可见）的编辑。
fn set_visibility(decl: &SyntaxNode, text: &str, visibility: Option<&str>) -> Option<TextEdit> {
    let modifiers = decl.children().find_map(ast::ModifierList::cast);
    let existing = modifiers.as_ref().and_then(|m| {
        m.keywords().find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::PUBLIC_KW | SyntaxKind::PROTECTED_KW | SyntaxKind::PRIVATE_KW
            )
        })
    });
    match (existing, visibility) {
        (Some(old), Some(new)) => Some(TextEdit::replace(old.text_range(), new.to_owned())),
        (Some(old), None) => {
            // 连同后面的空格一起删
            let end = usize::from(old.text_range().end());
            let spaces = text[end..].len() - text[end..].trim_start_matches(' ').len();
            Some(TextEdit::replace(
                TextRange::new(
                    old.text_range().start(),
                    TextSize::from((end + spaces) as u32),
                ),
                String::new(),
            ))
        }
        (None, Some(new)) => {
            // 放在第一个修饰关键字前面；只有注解时放在注解后面的类型 / 名字前面
            let first_keyword = modifiers.as_ref().and_then(|m| m.keywords().next());
            let at = match first_keyword {
                Some(keyword) => keyword.text_range().start(),
                None => {
                    let after = decl
                        .children_with_tokens()
                        .filter(|e| !e.kind().is_trivia() && e.kind() != SyntaxKind::MODIFIER_LIST)
                        .find(|e| {
                            modifiers.as_ref().is_none_or(|m| {
                                e.text_range().start() >= m.syntax().text_range().end()
                            })
                        })?;
                    after.text_range().start()
                }
            };
            Some(TextEdit::insert(at, format!("{new} ")))
        }
        (None, None) => None,
    }
}

/// 引用了没有访问权限的成员：放宽它的可见性。
pub(super) fn change_visibility(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(name_ref) = token.parent().and_then(ast::NameRef::cast) else {
        return;
    };
    let Some(def) = ctx.sema.classify_name_ref(ctx.file, &name_ref) else {
        return;
    };
    let (owner, flags, member_name) = match &def {
        Definition::Field(field) => {
            let Some(data) = class_data(db, field.class) else {
                return;
            };
            let Some(f) = data.fields.get(field.idx as usize) else {
                return;
            };
            (field.class, f.flags, f.name.to_string())
        }
        Definition::Method(method) => {
            let Some(data) = class_data(db, method.class) else {
                return;
            };
            let Some(m) = data.methods.get(method.idx as usize) else {
                return;
            };
            (method.class, m.flags, m.name.to_string())
        }
        _ => return,
    };
    let Some((from, _)) = ctx.enclosing_class() else {
        return;
    };
    if hir::is_accessible(db, flags, owner, Some(from)) {
        return;
    }
    let (Some(owner_data), Some(from_data)) = (class_data(db, owner), class_data(db, from)) else {
        return;
    };
    let target = if owner_data.package() == from_data.package() {
        None
    } else if hir::is_subclass_of(db, from, owner) {
        Some("protected")
    } else {
        Some("public")
    };
    // 接口成员本来就是 public
    if owner_data.is_interface() {
        return;
    }
    let Some((file, decl)) = decl_node(ctx, &def) else {
        return;
    };
    let text: &str = file.text(db);
    let Some(edit) = set_visibility(&decl, text, target) else {
        return;
    };
    let shown = target.unwrap_or(base_db::trs!("包内可见", "package-private"));
    let change = SourceChange::single_file(file.file_id(db), vec![edit]);
    out.push(Assist {
        id: "change_visibility",
        label: base_db::tr!(
            "把 `{member_name}` 改成 {shown}",
            "Make `{member_name}` {shown}"
        ),
        kind: AssistKind::QuickFix,
        change,
    });
}

/// 给 final 变量 / 字段赋值：去掉声明上的 `final`。
pub(super) fn remove_final(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(name_ref) = token.parent().and_then(ast::NameRef::cast) else {
        return;
    };
    // 必须是赋值的左边或者自增自减的操作数
    let Some(expr) = name_ref.syntax().parent() else {
        return;
    };
    if !matches!(
        expr.kind(),
        SyntaxKind::NAME_EXPR | SyntaxKind::FIELD_ACCESS
    ) {
        return;
    }
    let written = expr.parent().is_some_and(|p| match p.kind() {
        SyntaxKind::ASSIGN_EXPR => ast::AssignExpr::cast(p)
            .and_then(|a| a.lhs())
            .is_some_and(|lhs| lhs.syntax() == &expr),
        SyntaxKind::POSTFIX_EXPR => true,
        SyntaxKind::UNARY_EXPR => ast::UnaryExpr::cast(p)
            .and_then(|u| u.op_token())
            .is_some_and(|t| matches!(t.text(), "++" | "--")),
        _ => false,
    });
    if !written {
        return;
    }
    let Some(def) = ctx.sema.classify_name_ref(ctx.file, &name_ref) else {
        return;
    };
    let (file, decl) = match &def {
        Definition::Field(field) => {
            let final_field = class_data(db, field.class)
                .and_then(|d| d.fields.get(field.idx as usize).map(|f| f.flags))
                .is_some_and(|f| f.contains(Modifiers::FINAL));
            if !final_field {
                return;
            }
            let Some(found) = decl_node(ctx, &def) else {
                return;
            };
            found
        }
        Definition::Local(_) => {
            let Some(source) = def.source(db) else {
                return;
            };
            let Some(node) = node_at(&ctx.root, source.focus_range) else {
                return;
            };
            let Some(decl) = node
                .ancestors()
                .find(|n| matches!(n.kind(), SyntaxKind::LOCAL_VAR_DECL | SyntaxKind::PARAM))
            else {
                return;
            };
            (ctx.file, decl)
        }
        _ => return,
    };
    let Some(final_kw) = decl
        .children()
        .find_map(ast::ModifierList::cast)
        .and_then(|m| m.keywords().find(|t| t.kind() == SyntaxKind::FINAL_KW))
    else {
        return;
    };
    let text: &str = file.text(db);
    let end = usize::from(final_kw.text_range().end());
    let spaces = text[end..].len() - text[end..].trim_start_matches(' ').len();
    let edit = TextEdit::replace(
        TextRange::new(
            final_kw.text_range().start(),
            TextSize::from((end + spaces) as u32),
        ),
        String::new(),
    );
    out.push(Assist {
        id: "remove_final",
        label: base_db::tr!("去掉 `{}` 的 final", "Make `{}` not final", name_ref.text()),
        kind: AssistKind::QuickFix,
        change: SourceChange::single_file(file.file_id(db), vec![edit]),
    });
}

/// 类型能否直接写进源码。
fn writable(ty: &Ty) -> bool {
    !ty.contains_unknown() && !matches!(ty, Ty::Null | Ty::Void | Ty::Intersection(_))
}

/// 类型不匹配：把变量的声明类型或者方法的返回类型改成值的类型。
pub(super) fn change_type_to_match(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start())
        .or_else(|| ctx.root.token_at_offset(ctx.range.start()).right_biased())
    else {
        return;
    };
    // 光标所在的、是初值或者 return 值的那个表达式；光标在变量名上时取它的初值
    let value = token
        .parent_ancestors()
        .filter_map(ast::Expr::cast)
        .find(|e| {
            e.syntax().parent().is_some_and(|p| {
                matches!(
                    p.kind(),
                    SyntaxKind::VARIABLE_DECLARATOR | SyntaxKind::RETURN_STMT
                )
            })
        })
        .or_else(|| {
            let name = token.parent().and_then(ast::Name::cast)?;
            ast::VariableDeclarator::cast(name.syntax().parent()?)?.initializer()
        });
    let Some(value) = value else {
        return;
    };
    let Some(value_ty) = ctx.sema.type_of_expr(ctx.file, &value) else {
        return;
    };
    if !writable(&value_ty) {
        return;
    }
    let parent = value.syntax().parent().unwrap();
    let (written, label): (ast::Type, String) = if parent.kind() == SyntaxKind::VARIABLE_DECLARATOR
    {
        let declarator = ast::VariableDeclarator::cast(parent.clone()).unwrap();
        let Some(decl) = parent.parent() else {
            return;
        };
        let Some(ty) = decl.children().find_map(ast::Type::cast) else {
            return;
        };
        if decl
            .children()
            .filter(|c| c.kind() == SyntaxKind::VARIABLE_DECLARATOR)
            .count()
            != 1
            || declarator.extra_dims() > 0
        {
            return;
        }
        let name = declarator
            .name()
            .map(|n| n.text().to_string())
            .unwrap_or_default();
        (
            ty,
            base_db::tr!(
                "把 `{name}` 的类型改成 `{}`",
                "Change type of `{name}` to `{}`",
                value_ty.display(db)
            ),
        )
    } else {
        let Some(method) = parent.ancestors().find_map(|a| {
            if a.kind() == SyntaxKind::LAMBDA_EXPR {
                return Some(None);
            }
            ast::MethodDecl::cast(a).map(Some)
        }) else {
            return;
        };
        let Some(method) = method else {
            return;
        };
        let Some(ty) = method.return_type() else {
            return;
        };
        if method.extra_dims() > 0 {
            return;
        }
        let name = method
            .name()
            .map(|n| n.text().to_string())
            .unwrap_or_default();
        (
            ty,
            base_db::tr!(
                "把方法 `{name}` 的返回类型改成 `{}`",
                "Change return type of `{name}` to `{}`",
                value_ty.display(db)
            ),
        )
    };
    if written.syntax().text() == "var" {
        return;
    }
    let declared = ctx.sema.resolve_type(ctx.file, &written);
    if declared.contains_unknown() || hir::is_assignable(db, &value_ty, &declared) {
        return;
    }
    let mut edits = vec![TextEdit::replace(
        written.syntax().text_range(),
        value_ty.display(db),
    )];
    if let Some(import) = import_for(ctx, &value_ty, written.syntax()) {
        edits.push(import);
    }
    out.push(ctx.assist("change_type_to_match", label, AssistKind::QuickFix, edits));
}

/// 缺少 `return` 的方法：在方法末尾补一条返回默认值的 `return`。
pub(super) fn add_return(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let missing = ide_diagnostics::missing_returns(ctx.db, ctx.file);
    let Some(range) = missing
        .into_iter()
        .find(|r| r.contains_inclusive(ctx.range.start()))
    else {
        return;
    };
    let Some(close) = ctx
        .root
        .covering_element(range)
        .into_token()
        .filter(|t| t.kind() == SyntaxKind::R_CURLY)
    else {
        return;
    };
    let Some(method) = close.parent_ancestors().find_map(ast::MethodDecl::cast) else {
        return;
    };
    let Some(ret) = method.return_type() else {
        return;
    };
    let value = match ret.syntax().text().to_string().as_str() {
        "boolean" => "false",
        "byte" | "short" | "int" | "long" | "char" => "0",
        "float" => "0f",
        "double" => "0.0",
        _ => "null",
    };
    let indent = ctx.indent_at(method.syntax().text_range().start());
    let close_start = usize::from(close.text_range().start());
    let line_start = ctx.text[..close_start].rfind('\n').map_or(0, |i| i + 1);
    let stmt = format!("{indent}    return {value};\n");
    let edit = if ctx.text[line_start..close_start].trim().is_empty() && line_start > 0 {
        TextEdit::insert(TextSize::from(line_start as u32), stmt)
    } else {
        TextEdit::insert(close.text_range().start(), format!("\n{stmt}{indent}"))
    };
    out.push(ctx.assist(
        "add_return",
        base_db::trs!("补上 return 语句", "Add return statement"),
        AssistKind::QuickFix,
        vec![edit],
    ));
}

/// 没有实现全部抽象方法的类：改成 abstract 类（光标在类的声明头上时提供）。
pub(super) fn make_class_abstract(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some((class, decl)) = ctx.enclosing_class() else {
        return;
    };
    if decl.syntax().kind() != SyntaxKind::CLASS_DECL {
        return;
    }
    // 只在类体之前（修饰符、名字、extends / implements）提供
    if decl
        .body()
        .is_some_and(|body| ctx.range.start() > body.syntax().text_range().start())
    {
        return;
    }
    let Some(data) = class_data(db, class) else {
        return;
    };
    if data.flags.is_abstract() || data.flags.contains(Modifiers::FINAL) {
        return;
    }
    let unimplemented = hir::all_methods(db, &hir::this_ty(db, class))
        .into_iter()
        .filter(|c| c.id.class != class)
        .any(|c| {
            class_data(db, c.id.class)
                .and_then(|owner| owner.methods.get(c.id.idx as usize).map(|m| m.flags))
                .is_some_and(|f| f.is_abstract() && !f.is_static())
        });
    if !unimplemented {
        return;
    }
    let Some(class_kw) = decl
        .syntax()
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == SyntaxKind::CLASS_KW)
    else {
        return;
    };
    let name = data.name.to_string();
    out.push(ctx.assist(
        "make_class_abstract",
        base_db::tr!(
            "把类 `{name}` 改成 abstract",
            "Make class `{name}` abstract"
        ),
        AssistKind::QuickFix,
        vec![TextEdit::insert(
            class_kw.text_range().start(),
            "abstract ".to_owned(),
        )],
    ));
}

fn strip_parens(mut expr: ast::Expr) -> ast::Expr {
    while let ast::Expr::ParenExpr(p) = &expr {
        match p.expr() {
            Some(inner) => expr = inner,
            None => break,
        }
    }
    expr
}

fn bool_value(expr: &ast::Expr) -> Option<bool> {
    match strip_parens(expr.clone()) {
        ast::Expr::Literal(l) => match l.kind()? {
            SyntaxKind::TRUE_KW => Some(true),
            SyntaxKind::FALSE_KW => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn single_statement(stmt: ast::Stmt) -> Option<ast::Stmt> {
    match stmt {
        ast::Stmt::Block(block) => {
            let mut stmts = block.statements();
            match (stmts.next(), stmts.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
        other => Some(other),
    }
}

/// 代码检查报出的可简化写法：换成简化后的写法。
pub(super) fn simplify(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let diagnostics = ide_diagnostics::simplification_diagnostics(ctx.db, ctx.file);
    for diagnostic in diagnostics
        .iter()
        .filter(|d| d.range.contains_inclusive(ctx.range.start()))
    {
        let Some(node) = node_at(&ctx.root, diagnostic.range) else {
            continue;
        };
        let fix = match diagnostic.code {
            DiagnosticCode::PointlessBoolean => simplify_boolean(&node),
            DiagnosticCode::UseIsEmpty => use_is_empty(&node),
            DiagnosticCode::RedundantIf => simplify_if(&node),
            DiagnosticCode::RedundantCast => remove_cast(&node),
            DiagnosticCode::FieldMayBeFinal => make_field_final(&node),
            DiagnosticCode::RedundantModifier => remove_modifier(&ctx.root, diagnostic.range),
            _ => None,
        };
        let Some((range, text, label)) = fix else {
            continue;
        };
        out.push(ctx.assist(
            "simplify",
            label,
            AssistKind::QuickFix,
            vec![TextEdit::replace(range, text)],
        ));
    }
}

/// 删掉多余的修饰符和它后面的空白。
fn remove_modifier(root: &SyntaxNode, range: TextRange) -> Option<(TextRange, String, String)> {
    let token = root.covering_element(range).into_token()?;
    let mut end = token.text_range().end();
    if let Some(next) = token
        .next_token()
        .filter(|t| t.kind() == SyntaxKind::WHITESPACE)
    {
        end = next.text_range().end();
    }
    let text = token.text().to_owned();
    Some((
        TextRange::new(token.text_range().start(), end),
        String::new(),
        base_db::tr!("删除多余的 `{text}`", "Remove redundant `{text}`"),
    ))
}

/// 在字段类型前面加 `final`：`private int x;` → `private final int x;`。
fn make_field_final(node: &SyntaxNode) -> Option<(TextRange, String, String)> {
    let field = node.ancestors().find_map(ast::FieldDecl::cast)?;
    let name = field.declarators().next()?.name()?.text();
    let start = field.ty()?.syntax().text_range().start();
    Some((
        TextRange::empty(start),
        "final ".to_owned(),
        base_db::tr!("把 `{name}` 声明为 final", "Make `{name}` final"),
    ))
}

fn simplify_boolean(node: &SyntaxNode) -> Option<(TextRange, String, String)> {
    let binary = node.ancestors().find_map(ast::BinaryExpr::cast)?;
    let (lhs, rhs) = (binary.lhs()?, binary.rhs()?);
    let (value, other) = match (bool_value(&lhs), bool_value(&rhs)) {
        (Some(v), None) => (v, rhs),
        (None, Some(v)) => (v, lhs),
        _ => return None,
    };
    let equal = binary.op_kind()? == SyntaxKind::EQ_EQ;
    let text = if value == equal {
        other.syntax().text().to_string()
    } else {
        negate(&other)
    };
    Some((
        binary.syntax().text_range(),
        text.clone(),
        base_db::tr!("简化为 `{text}`", "Simplify to `{text}`"),
    ))
}

fn use_is_empty(node: &SyntaxNode) -> Option<(TextRange, String, String)> {
    let binary = node.ancestors().find_map(ast::BinaryExpr::cast)?;
    let op = binary.op_kind()?;
    let (lhs, rhs) = (strip_parens(binary.lhs()?), strip_parens(binary.rhs()?));
    let zero = |e: &ast::Expr| e.syntax().text() == "0";
    let (call, flipped) = if zero(&rhs) {
        (lhs, false)
    } else if zero(&lhs) {
        (rhs, true)
    } else {
        return None;
    };
    let negated = match (op, flipped) {
        (SyntaxKind::EQ_EQ, _) => false,
        (SyntaxKind::NEQ, _) | (SyntaxKind::GT, false) | (SyntaxKind::LT, true) => true,
        _ => return None,
    };
    let ast::Expr::MethodCall(call) = call else {
        return None;
    };
    let receiver = call.receiver()?.syntax().text().to_string();
    let text = if negated {
        format!("!{receiver}.isEmpty()")
    } else {
        format!("{receiver}.isEmpty()")
    };
    Some((
        binary.syntax().text_range(),
        text,
        base_db::trs!("换成 `isEmpty()`", "Replace with `isEmpty()`").to_owned(),
    ))
}

fn simplify_if(node: &SyntaxNode) -> Option<(TextRange, String, String)> {
    let if_stmt = node.ancestors().find_map(ast::IfStmt::cast)?;
    let cond = if_stmt.condition()?;
    let then = single_statement(if_stmt.then_branch()?)?;
    let (other, range) = match if_stmt.else_branch() {
        Some(other) => (single_statement(other)?, if_stmt.syntax().text_range()),
        None => {
            let next = if_stmt.syntax().next_sibling().and_then(ast::Stmt::cast)?;
            let range = TextRange::new(
                if_stmt.syntax().text_range().start(),
                next.syntax().text_range().end(),
            );
            (next, range)
        }
    };
    let value_of = |s: &ast::Stmt| -> Option<bool> {
        match s {
            ast::Stmt::ReturnStmt(r) => bool_value(&r.expr()?),
            ast::Stmt::ExprStmt(e) => match e.expr()? {
                ast::Expr::AssignExpr(a) => bool_value(&a.rhs()?),
                _ => None,
            },
            _ => None,
        }
    };
    let positive = value_of(&then)?;
    if value_of(&other)? == positive {
        return None;
    }
    let value = if positive {
        cond.syntax().text().to_string()
    } else {
        negate(&cond)
    };
    let text = match &then {
        ast::Stmt::ReturnStmt(_) => format!("return {value};"),
        ast::Stmt::ExprStmt(e) => {
            let ast::Expr::AssignExpr(a) = e.expr()? else {
                return None;
            };
            format!("{} = {value};", a.lhs()?.syntax().text())
        }
        _ => return None,
    };
    Some((
        range,
        text.clone(),
        base_db::tr!("简化为 `{text}`", "Simplify to `{text}`"),
    ))
}

fn remove_cast(node: &SyntaxNode) -> Option<(TextRange, String, String)> {
    let cast = node.ancestors().find_map(ast::CastExpr::cast)?;
    let operand = cast.expr()?;
    let operand_text = operand.syntax().text().to_string();
    // `((String) s).length()` → `s.length()`：外面那层括号也去掉
    let primary = matches!(
        operand,
        ast::Expr::NameExpr(_)
            | ast::Expr::MethodCall(_)
            | ast::Expr::FieldAccess(_)
            | ast::Expr::Literal(_)
            | ast::Expr::ThisExpr(_)
            | ast::Expr::ParenExpr(_)
            | ast::Expr::ArrayAccess(_)
    );
    let range = match cast
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::PAREN_EXPR)
    {
        Some(paren) if primary => paren.text_range(),
        _ => cast.syntax().text_range(),
    };
    Some((
        range,
        operand_text,
        base_db::trs!("去掉多余的强制转换", "Remove redundant cast").to_owned(),
    ))
}
