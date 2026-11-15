//! 引入形参：把方法里选中的表达式变成方法的新形参，所有调用处传入原来的表达式。
//!
//! 只处理不参与覆写的方法（`private` / `static`）。表达式里能用的名字：本方法的形参
//! （调用处换成对应的实参，形参在方法里不能被重新赋值）、静态字段 / 静态方法、类名和字面量；
//! 用到局部变量、实例成员、`this` 或 lambda 时不提供。

use hir::{class_data, Definition, LocalKind, Ty, Visibility};
use ide_db::{search::find_references, TextEdit};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use super::{
    refactor::{group_edits, is_atomic, is_pure_argument},
    Assist, AssistCtx, AssistKind,
};
use crate::FileId;

/// 去掉选区两端的空白。
fn trimmed_selection(ctx: &AssistCtx<'_>) -> Option<TextRange> {
    if ctx.range.is_empty() {
        return None;
    }
    let selected = &ctx.text[usize::from(ctx.range.start())..usize::from(ctx.range.end())];
    let leading = selected.len() - selected.trim_start().len();
    let trailing = selected.len() - selected.trim_end().len();
    if leading + trailing >= selected.len() {
        return None;
    }
    Some(TextRange::new(
        ctx.range.start() + TextSize::from(leading as u32),
        ctx.range.end() - TextSize::from(trailing as u32),
    ))
}

/// 表达式里用到的名字。
enum Use {
    /// 第几个形参
    Param(usize, TextRange),
    /// 只能在同一个文件里写出来的名字（类名、不带限定的静态成员、私有静态成员）
    FileLocal,
    /// 在哪儿都能写的名字（带限定的成员）
    Anywhere,
}

fn classify_use(
    ctx: &AssistCtx<'_>,
    name_ref: &ast::NameRef,
    param_decls: &[TextRange],
) -> Option<Use> {
    let db = ctx.db;
    let parent = name_ref.syntax().parent()?;
    let qualified = match parent.kind() {
        SyntaxKind::METHOD_CALL => ast::MethodCall::cast(parent.clone())?.receiver().is_some(),
        SyntaxKind::FIELD_ACCESS => true,
        _ => false,
    };
    match ctx.sema.classify_name_ref(ctx.file, name_ref)? {
        Definition::Local(local) => {
            let data = hir::infer(db, local.body).local(local.id).clone();
            if data.kind != LocalKind::Param {
                return None;
            }
            let index = param_decls
                .iter()
                .position(|r| r.contains_range(data.decl_range))?;
            Some(Use::Param(index, name_ref.syntax().text_range()))
        }
        Definition::Field(field) => {
            let data = class_data(db, field.class)?;
            let field_data = data.fields.get(field.idx as usize)?;
            if !field_data.flags.is_static() && !qualified {
                return None;
            }
            if !qualified || field_data.flags.visibility() == Visibility::Private {
                Some(Use::FileLocal)
            } else {
                Some(Use::Anywhere)
            }
        }
        Definition::Method(method) => {
            let data = class_data(db, method.class)?;
            let method_data = data.methods.get(method.idx as usize)?;
            if !method_data.flags.is_static() && !qualified {
                return None;
            }
            if !qualified || method_data.flags.visibility() == Visibility::Private {
                Some(Use::FileLocal)
            } else {
                Some(Use::Anywhere)
            }
        }
        // `java.lang` 里的类在哪儿都能直接写
        Definition::Class(class) => {
            let binary = class.binary_name(db);
            let in_java_lang = binary
                .strip_prefix("java/lang/")
                .is_some_and(|rest| !rest.contains('/'));
            Some(if in_java_lang {
                Use::Anywhere
            } else {
                Use::FileLocal
            })
        }
        Definition::Package(_) => Some(Use::FileLocal),
        Definition::TypeParam(_) => None,
    }
}

/// 由表达式起形参名：名字 / 成员访问取最后的名字，`getFoo()` 取 `foo`，否则按类型起名。
fn suggest_name(ctx: &AssistCtx<'_>, expr: &ast::Expr, ty: &Ty) -> String {
    let lower_camel = |s: &str| -> String {
        if s.chars()
            .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
        {
            // CONSTANT_CASE → constantCase
            let mut out = String::new();
            for (i, part) in s.split('_').filter(|p| !p.is_empty()).enumerate() {
                let lower = part.to_lowercase();
                if i == 0 {
                    out.push_str(&lower);
                } else {
                    out.push_str(&super::capitalize(&lower));
                }
            }
            out
        } else {
            let mut chars = s.chars();
            match chars.next() {
                Some(c) => c.to_lowercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    };
    let from_expr = match expr {
        ast::Expr::NameExpr(e) => e.name_ref().map(|n| n.text().to_string()),
        ast::Expr::FieldAccess(a) => a.name_ref().map(|n| n.text().to_string()),
        ast::Expr::MethodCall(c) => c.name_ref().map(|n| {
            let name = n.text().to_string();
            for prefix in ["get", "is", "to"] {
                if let Some(rest) = name.strip_prefix(prefix) {
                    if rest.starts_with(|c: char| c.is_uppercase()) {
                        return rest.to_owned();
                    }
                }
            }
            name
        }),
        _ => None,
    };
    let base = from_expr
        .map(|n| lower_camel(&n))
        .unwrap_or_else(|| match ty {
            Ty::Primitive(p) => match p.name() {
                "boolean" => "flag".to_owned(),
                "char" => "c".to_owned(),
                "double" | "float" => "value".to_owned(),
                _ => "n".to_owned(),
            },
            Ty::Class(class, _) => match class_data(ctx.db, *class) {
                Some(d) if &*d.binary_name == "java/lang/String" => "text".to_owned(),
                Some(d) => lower_camel(&d.name),
                None => "value".to_owned(),
            },
            Ty::Array(_) => "values".to_owned(),
            _ => "value".to_owned(),
        });
    let keyword = matches!(
        base.as_str(),
        "int"
            | "long"
            | "short"
            | "byte"
            | "char"
            | "float"
            | "double"
            | "boolean"
            | "class"
            | "new"
            | "default"
            | "string"
            | "object"
    );
    if keyword || base.is_empty() {
        "value".to_owned()
    } else {
        base
    }
}

/// 类型里的类都能在 `node` 处用简单名写出来。
fn writable(ctx: &AssistCtx<'_>, ty: &Ty, node: &SyntaxNode) -> bool {
    match ty {
        Ty::Primitive(_) => true,
        Ty::Array(element) => writable(ctx, element, node),
        Ty::Class(class, args) => {
            let binary = class.binary_name(ctx.db);
            let tail = binary.rsplit('/').next().unwrap_or(&binary);
            if tail.contains('$') {
                return false;
            }
            let scope = ctx.sema.type_scope(ctx.file, node);
            matches!(
                hir::resolve_simple_type(ctx.db, &scope, tail),
                Some(hir::TypeNs::Class(found)) if found == *class
            ) && args.iter().all(|a| writable(ctx, a, node))
        }
        _ => false,
    }
}

/// 在 `(…)` 列表末尾追加一项。
fn append_to_list(list: &SyntaxNode, has_items: bool, text: &str) -> Option<TextEdit> {
    let close = list
        .last_token()
        .filter(|t| t.kind() == SyntaxKind::R_PAREN)?;
    let insert = if has_items {
        format!(", {text}")
    } else {
        text.to_owned()
    };
    Some(TextEdit::insert(close.text_range().start(), insert))
}

pub(super) fn introduce_parameter(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(selection) = trimmed_selection(ctx) else {
        return;
    };
    let Some(expr) = syntax::algo::find_node_at_range::<ast::Expr>(&ctx.root, selection) else {
        return;
    };
    if expr.syntax().text_range() != selection {
        return;
    }
    let Some(parent) = expr.syntax().parent() else {
        return;
    };
    // 语句级的调用、赋值左侧不能换成形参
    if parent.kind() == SyntaxKind::EXPR_STMT
        || (parent.kind() == SyntaxKind::ASSIGN_EXPR
            && parent.first_child().as_ref() == Some(expr.syntax()))
    {
        return;
    }
    if expr.syntax().descendants().any(|n| {
        matches!(
            n.kind(),
            SyntaxKind::THIS_EXPR
                | SyntaxKind::SUPER_EXPR
                | SyntaxKind::LAMBDA_EXPR
                | SyntaxKind::METHOD_REF
                | SyntaxKind::CLASS_BODY
                | SyntaxKind::ASSIGN_EXPR
                | SyntaxKind::POSTFIX_EXPR
        )
    }) {
        return;
    }
    // 直接所在的方法（不能在 lambda / 局部类 / 匿名类里面）
    let Some(method_decl) = expr
        .syntax()
        .ancestors()
        .find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::METHOD_DECL | SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY
            )
        })
        .and_then(ast::MethodDecl::cast)
    else {
        return;
    };
    let (Some(param_list), Some(_)) = (method_decl.param_list(), method_decl.body()) else {
        return;
    };
    let Some(method) = ctx.sema.method_of_decl(ctx.file, method_decl.syntax()) else {
        return;
    };
    let Some(data) = class_data(db, method.class) else {
        return;
    };
    let Some(method_data) = data.methods.get(method.idx as usize) else {
        return;
    };
    if !(method_data.flags.is_static() || method_data.flags.visibility() == Visibility::Private) {
        return;
    }
    let params: Vec<ast::Param> = param_list.params().collect();
    if params.iter().any(|p| p.is_varargs()) {
        return;
    }
    let param_decls: Vec<TextRange> = params.iter().map(|p| p.syntax().text_range()).collect();

    // 表达式里的名字
    let mut uses: Vec<Use> = Vec::new();
    for name_ref in expr.syntax().descendants().filter_map(ast::NameRef::cast) {
        let Some(found) = classify_use(ctx, &name_ref, &param_decls) else {
            return;
        };
        uses.push(found);
    }
    // 用到的形参不能在方法里被重新赋值
    let body = method_decl.body().map(|b| b.syntax().clone());
    for used in &uses {
        let Use::Param(index, _) = used else { continue };
        let Some(name) = params[*index].name().map(|n| n.text().to_string()) else {
            return;
        };
        let reassigned = body.iter().flat_map(|b| b.descendants()).any(|n| {
            let target = match n.kind() {
                SyntaxKind::ASSIGN_EXPR => ast::AssignExpr::cast(n).and_then(|a| a.lhs()),
                SyntaxKind::POSTFIX_EXPR | SyntaxKind::UNARY_EXPR => {
                    n.children().find_map(ast::Expr::cast)
                }
                _ => None,
            };
            matches!(target, Some(ast::Expr::NameExpr(e)) if e.name_ref().is_some_and(|r| r.text() == name.as_str()))
        });
        if reassigned {
            return;
        }
    }
    let file_local = uses.iter().any(|u| matches!(u, Use::FileLocal));

    let ty = match ctx.sema.type_of_expr(ctx.file, &expr) {
        Some(ty) if !ty.contains_unknown() && !matches!(ty, Ty::Null | Ty::Void) => ty,
        _ => return,
    };
    let ty_text = if writable(ctx, &ty, method_decl.syntax()) {
        ty.display(db)
    } else {
        ty.display_qualified(db)
    };

    // 新形参的名字不能和方法里声明的名字、当作变量用的名字冲突（方法名、成员名不算）
    let taken: Vec<String> = method_decl
        .syntax()
        .descendants()
        .filter(|n| {
            n.kind() == SyntaxKind::NAME
                || (n.kind() == SyntaxKind::NAME_REF
                    && n.parent()
                        .is_some_and(|p| p.kind() == SyntaxKind::NAME_EXPR))
        })
        .map(|n| n.text().to_string())
        .collect();
    let base = suggest_name(ctx, &expr, &ty);
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}{n}");
        n += 1;
    }

    let expr_range = expr.syntax().text_range();
    let expr_text = expr.syntax().text().to_string();
    let mut edits: Vec<(FileId, TextEdit)> =
        vec![(ctx.file_id, TextEdit::replace(selection, name.as_str()))];
    let Some(decl_edit) = append_to_list(
        param_list.syntax(),
        !params.is_empty(),
        &format!("{ty_text} {name}"),
    ) else {
        return;
    };
    edits.push((ctx.file_id, decl_edit));

    for reference in find_references(db, &Definition::Method(method)) {
        if file_local && reference.file_id != ctx.file_id {
            return;
        }
        let Some(file) = base_db::source_file(db, reference.file_id) else {
            return;
        };
        let root = base_db::parse(db, file).syntax_node();
        let Some(call) = root
            .covering_element(reference.range)
            .ancestors()
            .take(3)
            .find_map(ast::MethodCall::cast)
        else {
            // 方法引用之类的用法：形参个数是隐含的
            return;
        };
        if reference.file_id == ctx.file_id && expr_range.contains_range(call.syntax().text_range())
        {
            return;
        }
        let Some(arg_list) = call.arg_list() else {
            return;
        };
        let args: Vec<ast::Expr> = arg_list.args().collect();
        if args.len() != params.len() {
            return;
        }
        // 调用处的实参：把用到的形参换成对应的实参
        let mut replacements: Vec<(TextRange, String)> = Vec::new();
        for used in &uses {
            let Use::Param(index, range) = used else {
                continue;
            };
            let arg = &args[*index];
            if !is_pure_argument(arg) {
                return;
            }
            let text = arg.syntax().text().to_string();
            let text = if is_atomic(arg) {
                text
            } else {
                format!("({text})")
            };
            replacements.push((*range, text));
        }
        replacements.sort_by_key(|(r, _)| std::cmp::Reverse(r.start()));
        let mut arg_text = expr_text.clone();
        for (range, text) in replacements {
            let start = usize::from(range.start() - expr_range.start());
            let end = usize::from(range.end() - expr_range.start());
            arg_text.replace_range(start..end, &text);
        }
        let Some(edit) = append_to_list(arg_list.syntax(), !args.is_empty(), &arg_text) else {
            return;
        };
        edits.push((reference.file_id, edit));
    }

    out.push(Assist {
        id: "introduce_parameter",
        label: base_db::tr!("引入形参 `{name}`", "Introduce parameter `{name}`"),
        kind: AssistKind::RefactorExtract,
        change: group_edits(edits),
    });
}
