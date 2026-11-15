//! 在别的类里创建成员：
//!
//! - `other.foo(x)` / `Util.foo(x)` 调用了不存在的方法：在 `other` 的类（或 `Util`）里创建它；
//! - `new Foo(a, b)` 没有能接受这些实参的构造器：在 `Foo` 里创建构造器。
//!
//! 目标类必须是工作区里的源码类。修改落在目标类所在的文件里，需要的 import 一并加上。

use hir::{class_data, ClassId, ClassKind, Definition, InferDiagnosticKind, Ty};
use ide_db::imports::ImportAction;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind,
};

use super::{
    fixes::{default_value, expected_type_text, name_for_type, ty_text, unresolved_name_at_cursor},
    insert_members_in, Assist, AssistCtx, AssistKind,
};
use crate::SourceChange;

/// 由实参生成形参（`类型 名字`），名字不重复。同时返回用到的类。
fn params_for(ctx: &AssistCtx<'_>, args: &[ast::Expr]) -> (Vec<String>, Vec<ClassId>) {
    let mut used: Vec<String> = Vec::new();
    let mut classes = Vec::new();
    let params = args
        .iter()
        .map(|arg| {
            let arg_ty = ctx.sema.type_of_expr(ctx.file, arg);
            if let Some(ty) = &arg_ty {
                collect_classes(ty, &mut classes);
            }
            let ty = ty_text(ctx, arg_ty.as_ref());
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
    (params, classes)
}

fn collect_classes(ty: &Ty, out: &mut Vec<ClassId>) {
    match ty {
        Ty::Class(class, args) => {
            if !out.contains(class) {
                out.push(*class);
            }
            for arg in args.iter() {
                collect_classes(arg, out);
            }
        }
        Ty::Array(elem) => collect_classes(elem, out),
        Ty::Wildcard(hir::Wildcard::Extends(bound) | hir::Wildcard::Super(bound)) => {
            collect_classes(bound, out)
        }
        _ => {}
    }
}

fn package_of(binary_name: &str) -> &str {
    binary_name.rsplit_once('/').map_or("", |(p, _)| p)
}

/// 目标类在工作区源码里的文件和声明节点。
fn target_decl(
    ctx: &AssistCtx<'_>,
    class: ClassId,
) -> Option<(base_db::SourceFile, ast::TypeDecl)> {
    let source = Definition::Class(class).source(ctx.db)?;
    let entry = base_db::file_entry(ctx.db, source.file.file_id(ctx.db))?;
    if !matches!(entry.kind, base_db::FileKind::Workspace { .. }) {
        return None;
    }
    let root = base_db::parse(ctx.db, source.file).syntax_node();
    let decl = root
        .covering_element(source.full_range)
        .into_node()?
        .ancestors()
        .find(|n| n.text_range() == source.full_range)
        .and_then(ast::TypeDecl::cast)?;
    Some((source.file, decl))
}

/// 在目标文件里插入成员，并给用到的类加上 import。
fn change_for(
    ctx: &AssistCtx<'_>,
    file: base_db::SourceFile,
    decl: &ast::TypeDecl,
    lines: Vec<String>,
    classes: &[ClassId],
) -> Option<SourceChange> {
    let text: &str = file.text(ctx.db);
    let mut edits = vec![insert_members_in(text, decl, &[lines])?];
    let scope = ctx.sema.type_scope(file, decl.syntax());
    for class in classes {
        if let ImportAction::Add(edit) =
            ide_db::imports::import_action(ctx.db, file, &scope, *class)
        {
            if !edits.contains(&edit) {
                edits.push(edit);
            }
        }
    }
    edits.sort_by_key(|e| e.range.start());
    Some(SourceChange::single_file(file.file_id(ctx.db), edits))
}

pub(super) fn create_method_in_class(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(name_ref) = unresolved_name_at_cursor(ctx) else {
        return;
    };
    let Some(call) = name_ref.syntax().parent().and_then(ast::MethodCall::cast) else {
        return;
    };
    let Some(receiver) = call.receiver() else {
        return;
    };
    if matches!(receiver, ast::Expr::ThisExpr(_) | ast::Expr::SuperExpr(_)) {
        return;
    }
    // `Util.foo()`：静态方法；`obj.foo()`：实例方法
    let static_target = match &receiver {
        ast::Expr::NameExpr(n) => {
            n.name_ref()
                .and_then(|r| match ctx.sema.classify_name_ref(ctx.file, &r) {
                    Some(Definition::Class(class)) => Some(class),
                    _ => None,
                })
        }
        _ => None,
    };
    let (class, is_static) = match static_target {
        Some(class) => (class, true),
        None => match ctx.sema.type_of_expr(ctx.file, &receiver) {
            Some(Ty::Class(class, _)) => (class, false),
            _ => return,
        },
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    if !matches!(
        data.kind,
        ClassKind::Class | ClassKind::Interface | ClassKind::Enum | ClassKind::Record
    ) {
        return;
    }
    let Some((file, decl)) = target_decl(ctx, class) else {
        return;
    };
    let args: Vec<ast::Expr> = call
        .arg_list()
        .map(|l| l.args().collect())
        .unwrap_or_default();
    let (params, classes) = params_for(ctx, &args);
    let ret = expected_type_text(ctx, call.syntax()).unwrap_or_else(|| "Object".to_owned());
    let same_package = ctx
        .enclosing_class()
        .and_then(|(c, _)| class_data(ctx.db, c))
        .is_some_and(|d| package_of(&d.binary_name) == package_of(&data.binary_name));
    let interface = data.kind == ClassKind::Interface;
    let visibility = if interface || same_package {
        ""
    } else {
        "public "
    };
    let signature = format!(
        "{visibility}{}{ret} {}({})",
        if is_static { "static " } else { "" },
        name_ref.text(),
        params.join(", ")
    );
    let lines = if interface && !is_static {
        vec![format!("{signature};")]
    } else {
        let mut lines = vec![format!("{signature} {{")];
        if ret != "void" {
            lines.push(format!("    return {};", default_value(&ret)));
        }
        lines.push("}".to_owned());
        lines
    };
    let Some(change) = change_for(ctx, file, &decl, lines, &classes) else {
        return;
    };
    out.push(Assist {
        id: "create_method_in_class",
        label: base_db::tr!(
            "在 `{}` 中创建方法 {}",
            "Create method {1} in `{0}`",
            data.name,
            name_ref.text()
        ),
        kind: AssistKind::QuickFix,
        change,
    });
}

pub(super) fn create_constructor(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(new) = token
        .parent_ancestors()
        .take_while(|n| n.kind() != SyntaxKind::ARG_LIST)
        .find_map(ast::NewExpr::cast)
    else {
        return;
    };
    if new.body().is_some() {
        return;
    }
    let Some(args) = new.arg_list() else {
        return;
    };
    // 光标在类名上
    let Some(ast::Type::ClassType(class_ty)) = new.ty() else {
        return;
    };
    if !class_ty
        .syntax()
        .text_range()
        .contains_inclusive(ctx.range.start())
    {
        return;
    }
    // 只在没有可用的构造器时提供
    let Some(body) = ctx.sema.body_of(ctx.file, new.syntax()) else {
        return;
    };
    let result = hir::infer(ctx.db, body);
    let new_range = new.syntax().text_range();
    let arg_ranges: Vec<_> = args.args().map(|a| a.syntax().text_range()).collect();
    let mismatch = result.diagnostics().iter().any(|d| {
        matches!(
            d.kind,
            InferDiagnosticKind::ArgumentMismatch { .. }
                | InferDiagnosticKind::UnresolvedMethod { .. }
        ) && new_range.contains_range(d.range)
            && !arg_ranges.iter().any(|a| a.contains_range(d.range))
    });
    if !mismatch {
        return;
    }
    let Some(Ty::Class(class, _)) = ctx
        .sema
        .type_of_expr(ctx.file, &ast::Expr::NewExpr(new.clone()))
    else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    if data.kind != ClassKind::Class {
        return;
    }
    let Some((file, decl)) = target_decl(ctx, class) else {
        return;
    };
    let args: Vec<ast::Expr> = args.args().collect();
    let (params, classes) = params_for(ctx, &args);
    let lines = vec![
        format!("public {}({}) {{", data.name, params.join(", ")),
        "}".to_owned(),
    ];
    let Some(change) = change_for(ctx, file, &decl, lines, &classes) else {
        return;
    };
    out.push(Assist {
        id: "create_constructor",
        label: base_db::tr!(
            "在 `{}` 中创建构造器",
            "Create constructor in `{}`",
            data.name
        ),
        kind: AssistKind::QuickFix,
        change,
    });
}
