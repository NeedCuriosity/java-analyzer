//! 在别的类里创建成员：
//!
//! - `other.foo(x)` / `Util.foo(x)` 调用了不存在的方法：在 `other` 的类（或 `Util`）里创建它；
//! - `other.count` / `Util.LIMIT` 访问了不存在的字段：在那个类里创建它；
//! - `new Foo(a, b)` 没有能接受这些实参的构造器：在 `Foo` 里创建构造器；
//! - 调用比方法多了一个实参：删掉它，或者给方法加上对应的形参（其它调用处补上默认值）。
//!
//! 目标类必须是工作区里的源码类。修改落在目标类所在的文件里，需要的 import 一并加上。

use hir::{class_data, ClassId, ClassKind, Definition, InferDiagnosticKind, Ty};
use ide_db::imports::ImportAction;
use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange, TextSize,
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

/// 接收者指向的类：`Util.x` 是类名（静态成员），`obj.x` 取它的类型（实例成员）。
fn receiver_class(ctx: &AssistCtx<'_>, receiver: &ast::Expr) -> Option<(ClassId, bool)> {
    if matches!(receiver, ast::Expr::ThisExpr(_) | ast::Expr::SuperExpr(_)) {
        return None;
    }
    let static_target = match receiver {
        ast::Expr::NameExpr(n) => {
            n.name_ref()
                .and_then(|r| match ctx.sema.classify_name_ref(ctx.file, &r) {
                    Some(Definition::Class(class)) => Some(class),
                    _ => None,
                })
        }
        _ => None,
    };
    match static_target {
        Some(class) => Some((class, true)),
        None => match ctx.sema.type_of_expr(ctx.file, receiver) {
            Some(Ty::Class(class, _)) => Some((class, false)),
            _ => None,
        },
    }
}

fn same_package(ctx: &AssistCtx<'_>, data: &hir::ClassData) -> bool {
    ctx.enclosing_class()
        .and_then(|(c, _)| class_data(ctx.db, c))
        .is_some_and(|d| package_of(&d.binary_name) == package_of(&data.binary_name))
}

/// `obj.count` / `Util.LIMIT` 访问了不存在的字段：在那个类里创建它。接口里只能是常量。
pub(super) fn create_field_in_class(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(name_ref) = unresolved_name_at_cursor(ctx) else {
        return;
    };
    let Some(access) = name_ref.syntax().parent().and_then(ast::FieldAccess::cast) else {
        return;
    };
    let Some(receiver) = access.receiver() else {
        return;
    };
    let Some((class, is_static)) = receiver_class(ctx, &receiver) else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    let interface = data.kind == ClassKind::Interface;
    if !matches!(data.kind, ClassKind::Class | ClassKind::Enum) && !(interface && is_static) {
        return;
    }
    let Some((file, decl)) = target_decl(ctx, class) else {
        return;
    };
    // 字段类型里用到的类，目标文件可能要 import
    let mut classes = Vec::new();
    let parent = access.syntax().parent();
    let target_ty = match parent.as_ref().map(|p| p.kind()) {
        Some(SyntaxKind::VARIABLE_DECLARATOR) => parent
            .and_then(|p| p.parent())
            .and_then(|d| d.children().find_map(ast::Type::cast))
            .map(|t| ctx.sema.resolve_type(ctx.file, &t)),
        Some(SyntaxKind::ASSIGN_EXPR) => parent
            .and_then(ast::AssignExpr::cast)
            .filter(|a| a.lhs().is_some_and(|l| l.syntax() == access.syntax()))
            .and_then(|a| a.rhs())
            .and_then(|rhs| ctx.sema.type_of_expr(ctx.file, &rhs)),
        _ => None,
    };
    if let Some(ty) = &target_ty {
        collect_classes(ty, &mut classes);
    }
    let ty = expected_type_text(ctx, access.syntax())
        .filter(|t| t != "void")
        .unwrap_or_else(|| "Object".to_owned());
    let name = name_ref.text().to_string();
    let constant = name
        .chars()
        .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit());
    let visibility = if interface || same_package(ctx, &data) {
        ""
    } else {
        "public "
    };
    let line = if interface {
        format!("{ty} {name} = {};", default_value(&ty))
    } else if is_static && constant {
        format!(
            "{visibility}static final {ty} {name} = {};",
            default_value(&ty)
        )
    } else {
        format!(
            "{visibility}{}{ty} {name};",
            if is_static { "static " } else { "" }
        )
    };
    let Some(change) = change_for(ctx, file, &decl, vec![line], &classes) else {
        return;
    };
    out.push(Assist {
        id: "create_field_in_class",
        label: base_db::tr!(
            "在 `{}` 中创建字段 {}",
            "Create field {1} in `{0}`",
            data.name,
            name
        ),
        kind: AssistKind::QuickFix,
        change,
    });
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
    let Some((class, is_static)) = receiver_class(ctx, &receiver) else {
        return;
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
    let interface = data.kind == ClassKind::Interface;
    let visibility = if interface || same_package(ctx, &data) {
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

/// 在列表的第 `idx` 个位置插入一项：`ranges` 是现有各项的范围，`open_end` 是左括号的结束位置。
fn list_insertion(ranges: &[TextRange], idx: usize, open_end: TextSize, item: &str) -> TextEdit {
    match ranges.get(idx) {
        Some(at) => TextEdit::insert(at.start(), format!("{item}, ")),
        None => match ranges.last() {
            Some(last) => TextEdit::insert(last.end(), format!(", {item}")),
            None => TextEdit::insert(open_end, item.to_owned()),
        },
    }
}

/// 光标所在的、报了实参不匹配的最内层方法调用。
fn mismatched_call(
    ctx: &AssistCtx<'_>,
) -> Option<(
    ast::MethodCall,
    ast::ArgList,
    std::sync::Arc<hir::InferenceResult>,
)> {
    let token = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start())?;
    let body = ctx.sema.body_of(ctx.file, &token.parent()?)?;
    let result = hir::infer(ctx.db, body);
    let (call, arg_list) = token.parent_ancestors().find_map(|n| {
        let call = ast::MethodCall::cast(n)?;
        let arg_list = call.arg_list()?;
        let name_range = call.name_ref()?.syntax().text_range();
        let arg_ranges: Vec<TextRange> = arg_list.args().map(|a| a.syntax().text_range()).collect();
        let mismatch = result.diagnostics().iter().any(|d| match d.kind {
            InferDiagnosticKind::ArgumentMismatch { .. } => {
                d.range == name_range || d.range == arg_list.syntax().text_range()
            }
            // 只有一个候选时报在实参上
            InferDiagnosticKind::TypeMismatch { .. } => arg_ranges.contains(&d.range),
            _ => false,
        });
        mismatch.then_some((call, arg_list))
    })?;
    Some((call, arg_list, result))
}

/// 调用比方法多了一个实参：删掉这个实参，或者给方法加上对应的形参（其它调用处补上默认值）。
pub(super) fn fix_extra_argument(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some((call, arg_list, result)) = mismatched_call(ctx) else {
        return;
    };
    let args: Vec<ast::Expr> = arg_list.args().collect();
    let Some(info) = result.call_info(arg_list.syntax().text_range()) else {
        return;
    };
    let arg_tys: Vec<Option<Ty>> = args
        .iter()
        .map(|a| result.type_of(a.syntax().text_range()).cloned())
        .collect();
    // 去掉第 idx 个实参后其余的都能传给某个重载
    let mut found = None;
    'candidates: for candidate in &info.candidates {
        let sig = hir::method_sig(ctx.db, candidate.id);
        if sig.varargs || sig.n_type_params > 0 || sig.params.len() + 1 != args.len() {
            continue;
        }
        let params = candidate.param_tys(ctx.db);
        for idx in 0..args.len() {
            let fits = arg_tys
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != idx)
                .zip(&params)
                .all(|((_, arg), param)| {
                    arg.as_ref().is_some_and(|a| {
                        !a.contains_unknown() && hir::is_assignable(ctx.db, a, param)
                    })
                });
            if fits {
                found = Some((candidate.id, idx));
                break 'candidates;
            }
        }
    }
    let Some((method, idx)) = found else {
        return;
    };
    let ordinal = idx + 1;
    let arg_ranges: Vec<TextRange> = args.iter().map(|a| a.syntax().text_range()).collect();
    if let Some(removal) = super::refactor::list_element_removal(&arg_ranges, idx) {
        out.push(ctx.assist(
            "remove_argument",
            base_db::tr!("删除第 {ordinal} 个实参", "Remove argument {ordinal}"),
            AssistKind::QuickFix,
            vec![TextEdit::replace(removal, "")],
        ));
    }
    add_parameter(
        ctx,
        &call,
        method,
        idx,
        &args[idx],
        arg_tys[idx].as_ref(),
        out,
    );
}

/// 不参与覆写、声明在工作区源码里的方法的声明节点。
fn standalone_method_decl(
    ctx: &AssistCtx<'_>,
    method: hir::MethodId,
) -> Option<(base_db::SourceFile, ast::MethodDecl)> {
    let db = ctx.db;
    let data = class_data(db, method.class)?;
    let method_data = data.methods.get(method.idx as usize)?;
    let standalone =
        method_data.flags.is_static() || method_data.flags.visibility() == hir::Visibility::Private;
    if !standalone || method_data.is_constructor {
        return None;
    }
    let source = Definition::Method(method).source(db)?;
    let entry = base_db::file_entry(db, source.file.file_id(db))?;
    if !matches!(entry.kind, base_db::FileKind::Workspace { .. }) {
        return None;
    }
    let root = base_db::parse(db, source.file).syntax_node();
    let decl = root
        .covering_element(source.full_range)
        .into_node()?
        .ancestors()
        .find(|n| n.text_range() == source.full_range)
        .and_then(ast::MethodDecl::cast)?;
    Some((source.file, decl))
}

/// 实参个数对、只有一个位置的类型不对：把方法的这个形参改成实参的类型（其它调用处的实参
/// 也要能传给新类型）。
pub(super) fn change_parameter_type(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some((call, arg_list, result)) = mismatched_call(ctx) else {
        return;
    };
    let args: Vec<ast::Expr> = arg_list.args().collect();
    let Some(info) = result.call_info(arg_list.syntax().text_range()) else {
        return;
    };
    let arg_tys: Vec<Option<Ty>> = args
        .iter()
        .map(|a| result.type_of(a.syntax().text_range()).cloned())
        .collect();
    if arg_tys.iter().any(|t| {
        t.as_ref()
            .is_none_or(|t| t.contains_unknown() || matches!(t, Ty::Null))
    }) {
        return;
    }
    let mut found = None;
    for candidate in &info.candidates {
        let sig = hir::method_sig(ctx.db, candidate.id);
        if sig.varargs || sig.n_type_params > 0 || sig.params.len() != args.len() {
            continue;
        }
        let params = candidate.param_tys(ctx.db);
        let wrong: Vec<usize> = (0..args.len())
            .filter(|i| {
                !arg_tys[*i]
                    .as_ref()
                    .is_some_and(|a| hir::is_assignable(ctx.db, a, &params[*i]))
            })
            .collect();
        if let [idx] = wrong.as_slice() {
            found = Some((candidate.id, *idx));
            break;
        }
    }
    let Some((method, idx)) = found else {
        return;
    };
    let Some(new_ty) = arg_tys[idx].clone() else {
        return;
    };
    let db = ctx.db;
    let Some((file, decl)) = standalone_method_decl(ctx, method) else {
        return;
    };
    let Some(param) = decl.param_list().and_then(|l| l.params().nth(idx)) else {
        return;
    };
    let Some(param_ty) = param.ty() else {
        return;
    };
    if param.extra_dims() > 0 {
        return;
    }
    // 其它调用处这个位置的实参都能传给新类型
    for reference in ide_db::search::find_references(db, &Definition::Method(method)) {
        let Some(other_file) = base_db::source_file(db, reference.file_id) else {
            return;
        };
        let root = base_db::parse(db, other_file).syntax_node();
        let Some(other) = root
            .covering_element(reference.range)
            .ancestors()
            .take(3)
            .find_map(ast::MethodCall::cast)
        else {
            return;
        };
        if other.syntax() == call.syntax() {
            continue;
        }
        let Some(other_arg) = other.arg_list().and_then(|l| l.args().nth(idx)) else {
            return;
        };
        let fits = ctx
            .sema
            .type_of_expr(other_file, &other_arg)
            .is_some_and(|t| !t.contains_unknown() && hir::is_assignable(db, &t, &new_ty));
        if !fits {
            return;
        }
    }
    let ty = ty_text(ctx, Some(&new_ty));
    let mut edits = vec![TextEdit::replace(
        param_ty.syntax().text_range(),
        ty.clone(),
    )];
    let mut classes = Vec::new();
    collect_classes(&new_ty, &mut classes);
    let scope = ctx.sema.type_scope(file, decl.syntax());
    for class in classes {
        if let ImportAction::Add(edit) = ide_db::imports::import_action(db, file, &scope, class) {
            if !edits.contains(&edit) {
                edits.push(edit);
            }
        }
    }
    edits.sort_by_key(|e| e.range.start());
    let ordinal = idx + 1;
    let name = decl
        .name()
        .map(|n| n.text().to_string())
        .unwrap_or_default();
    out.push(Assist {
        id: "change_parameter_type",
        label: base_db::tr!(
            "把方法 `{name}` 的第 {ordinal} 个形参改成 `{ty}`",
            "Change parameter {ordinal} of method `{name}` to `{ty}`"
        ),
        kind: AssistKind::QuickFix,
        change: SourceChange::single_file(file.file_id(db), edits),
    });
}

fn add_parameter(
    ctx: &AssistCtx<'_>,
    call: &ast::MethodCall,
    method: hir::MethodId,
    idx: usize,
    arg: &ast::Expr,
    arg_ty: Option<&Ty>,
    out: &mut Vec<Assist>,
) -> Option<()> {
    let db = ctx.db;
    let (decl_file, decl) = standalone_method_decl(ctx, method)?;
    let method_name = decl.name()?.text().to_string();
    let param_list = decl.param_list()?;
    let param_ranges: Vec<TextRange> = param_list
        .params()
        .map(|p| p.syntax().text_range())
        .collect();
    let existing: Vec<String> = param_list
        .params()
        .filter_map(|p| p.name().map(|n| n.text().to_string()))
        .collect();
    let ty = ty_text(ctx, arg_ty);
    let base = match arg {
        ast::Expr::NameExpr(name) => name.syntax().text().to_string(),
        _ => name_for_type(&ty),
    };
    let mut name = base.clone();
    let mut n = 2;
    while existing.contains(&name) {
        name = format!("{base}{n}");
        n += 1;
    }
    let open_end = param_list
        .syntax()
        .first_token()
        .filter(|t| t.kind() == SyntaxKind::L_PAREN)?
        .text_range()
        .end();
    let mut edits: Vec<(crate::FileId, TextEdit)> = vec![(
        decl_file.file_id(db),
        list_insertion(&param_ranges, idx, open_end, &format!("{ty} {name}")),
    )];
    // 形参类型需要的 import
    if let Some(class) = arg_ty.and_then(|t| t.as_class()) {
        let scope = ctx.sema.type_scope(decl_file, decl.syntax());
        if let ImportAction::Add(edit) =
            ide_db::imports::import_action(db, decl_file, &scope, class)
        {
            edits.push((decl_file.file_id(db), edit));
        }
    }
    // 其它调用处在同样的位置补上默认值
    let default = default_value(&ty);
    for reference in ide_db::search::find_references(db, &Definition::Method(method)) {
        let file = base_db::source_file(db, reference.file_id)?;
        let root = base_db::parse(db, file).syntax_node();
        let other = root
            .covering_element(reference.range)
            .ancestors()
            .take(3)
            .find_map(ast::MethodCall::cast)?;
        if other.syntax() == call.syntax() {
            continue;
        }
        let other_args = other.arg_list()?;
        let ranges: Vec<TextRange> = other_args.args().map(|a| a.syntax().text_range()).collect();
        if ranges.len() != param_ranges.len() {
            return None;
        }
        let open = other_args.syntax().first_token()?.text_range().end();
        edits.push((
            reference.file_id,
            list_insertion(&ranges, idx, open, default),
        ));
    }
    let mut change = SourceChange::default();
    edits.sort_by_key(|(file_id, edit)| (*file_id, edit.range.start()));
    for (file_id, edit) in edits {
        match change.edits.last_mut() {
            Some((last, list)) if *last == file_id => list.push(edit),
            _ => change.edits.push((file_id, vec![edit])),
        }
    }
    out.push(Assist {
        id: "add_parameter",
        label: base_db::tr!(
            "给方法 `{}` 加上形参 `{ty}`",
            "Add parameter `{ty}` to method `{}`",
            method_name
        ),
        kind: AssistKind::QuickFix,
        change,
    });
    Some(())
}
