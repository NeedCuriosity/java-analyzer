//! 各种位置上的候选来源：成员访问、表达式、类型、import。

use hir::{
    all_fields, all_methods, class_data, is_accessible, is_accessible_via, item_tree, lookup_class,
    lookup_field, lookup_methods, member_type, package_classes, package_exists,
    resolve_qualified_class, resolve_type_path, subpackages, this_ty, ClassId, Definition,
    FieldCandidate, LocalData, LocalKind, MethodCandidate, Modifiers, Ty, TypeNs,
};
use ide_db::{
    symbol_index::{binary_class_index, source_symbol_index},
    SymbolKind,
};
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextSize,
};

use crate::{
    context::{CompletionContext, ContextKind, TypeLocation},
    item::{score, CompletionItemKind, Completions},
};

fn field_flags(ctx: &CompletionContext<'_>, field: &FieldCandidate) -> Modifiers {
    class_data(ctx.db, field.id.class)
        .and_then(|d| d.fields.get(field.id.idx as usize).map(|f| f.flags))
        .unwrap_or_default()
}

fn method_flags(ctx: &CompletionContext<'_>, method: &MethodCandidate) -> Modifiers {
    class_data(ctx.db, method.id.class)
        .and_then(|d| d.methods.get(method.id.idx as usize).map(|m| m.flags))
        .unwrap_or_default()
}

fn visible(ctx: &CompletionContext<'_>, flags: Modifiers, declaring: ClassId) -> bool {
    is_accessible(ctx.db, flags, declaring, ctx.enclosing_class)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberFilter {
    /// 通过实例访问：静态成员也列出，但排在后面
    Instance,
    /// 通过类名访问：只要静态成员
    Static,
    /// 不加限定地使用（当前类及外层类的成员）
    Unqualified { static_only: bool },
}

fn members_of(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    ty: &Ty,
    filter: MemberFilter,
    methods_only: bool,
    name_only: bool,
) {
    let db = ctx.db;
    let score_for = |is_static: bool| match filter {
        MemberFilter::Instance if is_static => score::MEMBER + score::STATIC_VIA_INSTANCE,
        _ => score::MEMBER,
    };
    // `super.clone()` 是允许的；别的表达式上的 protected 成员要看表达式的类型
    let via_super = matches!(
        &ctx.kind,
        ContextKind::Dot { receiver: Some(receiver), .. } if receiver.syntax().text() == "super"
    );
    let visible = |flags: Modifiers, declaring: ClassId| {
        if filter == MemberFilter::Instance && !via_super {
            is_accessible_via(db, flags, declaring, ctx.enclosing_class, ty)
        } else {
            visible(ctx, flags, declaring)
        }
    };
    let keep = |is_static: bool| match filter {
        MemberFilter::Instance => true,
        MemberFilter::Static => is_static,
        MemberFilter::Unqualified { static_only } => is_static || !static_only,
    };
    if !methods_only {
        for field in all_fields(db, ty) {
            let flags = field_flags(ctx, &field);
            if keep(flags.is_static()) && visible(flags, field.id.class) {
                acc.add_field(ctx, &field, score_for(flags.is_static()));
            }
        }
    }
    for method in all_methods(db, ty) {
        let flags = method_flags(ctx, &method);
        if keep(flags.is_static()) && visible(flags, method.id.class) {
            acc.add_method(ctx, &method, score_for(flags.is_static()), name_only);
        }
    }
}

fn nested_classes(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    class: ClassId,
    location: TypeLocation,
) {
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    for name in &data.nested {
        if let Some(nested) = member_type(ctx.db, class, name) {
            acc.add_known_class(ctx, nested, score::MEMBER, location, false);
        }
    }
}

fn package_members(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    package: &str,
    location: TypeLocation,
) {
    let db = ctx.db;
    for name in subpackages(db, package) {
        acc.add_package(ctx, &name);
    }
    for name in package_classes(db, package) {
        if !Completions::matches(ctx, &name) {
            continue;
        }
        let binary_name = if package.is_empty() {
            name.to_string()
        } else {
            format!("{package}/{name}")
        };
        if let Some(class) = lookup_class(db, &binary_name) {
            acc.add_known_class(ctx, class, score::TYPE_IN_SCOPE, location, false);
        }
    }
}

fn static_members(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    class: ClassId,
    method_ref: bool,
) {
    members_of(
        acc,
        ctx,
        &Ty::class(class),
        MemberFilter::Static,
        method_ref,
        method_ref,
    );
    if method_ref {
        // `Foo::bar` 也可以引用实例方法
        members_of(
            acc,
            ctx,
            &Ty::class(class),
            MemberFilter::Instance,
            true,
            true,
        );
        acc.add_keyword(ctx, "new");
    } else {
        nested_classes(acc, ctx, class, TypeLocation::Other);
        acc.add_keyword(ctx, "class");
        acc.add_keyword(ctx, "this");
    }
}

fn instance_members(acc: &mut Completions, ctx: &CompletionContext<'_>, ty: &Ty, method_ref: bool) {
    match ty {
        Ty::Primitive(_) | Ty::Void | Ty::Null | Ty::Unknown => {}
        Ty::Array(_) => {
            if !method_ref && Completions::matches(ctx, "length") {
                let mut item = crate::item::CompletionItem {
                    label: "length".to_owned(),
                    kind: CompletionItemKind::Symbol(SymbolKind::Field),
                    label_detail: None,
                    detail: Some("int".to_owned()),
                    lookup: "length".to_owned(),
                    insert_text: "length".to_owned(),
                    is_snippet: false,
                    source_range: ctx.source_range,
                    additional_edits: Vec::new(),
                    deprecated: false,
                    score: score::MEMBER,
                    resolve: None,
                };
                if ctx.expected_ty.as_ref() == Some(&Ty::INT) {
                    item.score += score::EXPECTED_TYPE;
                }
                acc.add(item);
            }
            members_of(
                acc,
                ctx,
                &Ty::object(ctx.db),
                MemberFilter::Instance,
                true,
                method_ref,
            );
        }
        _ => members_of(acc, ctx, ty, MemberFilter::Instance, method_ref, method_ref),
    }
}

/// `receiver.|` / `receiver::|`
pub(crate) fn complete_dot(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    receiver: Option<&ast::Expr>,
    receiver_path: Option<&[SmolStr]>,
    method_ref: bool,
) -> Option<Ty> {
    let db = ctx.db;
    if let Some(expr) = receiver {
        let name_ref = match expr {
            ast::Expr::NameExpr(name) => name.name_ref(),
            ast::Expr::FieldAccess(access) => access.name_ref(),
            _ => None,
        };
        match name_ref.and_then(|n| ctx.sema.classify_name_ref(ctx.file, &n)) {
            Some(Definition::Class(class)) => {
                static_members(acc, ctx, class, method_ref);
                return None;
            }
            Some(Definition::Package(package)) => {
                package_members(acc, ctx, &package, TypeLocation::Other);
                return None;
            }
            _ => {}
        }
        if let Some(ty) = ctx
            .sema
            .type_of_expr(ctx.file, expr)
            .filter(|ty| !ty.is_unknown())
        {
            instance_members(acc, ctx, &ty, method_ref);
            return Some(ty);
        }
        if matches!(expr, ast::Expr::ThisExpr(_)) {
            if let Some(class) = ctx.enclosing_class {
                let ty = this_ty(db, class);
                instance_members(acc, ctx, &ty, method_ref);
                return Some(ty);
            }
        }
    }

    // 推断没有覆盖到（代码残缺）时按名字路径兜底
    let path = receiver_path?;
    if let [name] = path {
        if let Some(local) = visible_locals(ctx).into_iter().find(|l| l.name == *name) {
            instance_members(acc, ctx, &local.ty, method_ref);
            return Some(local.ty);
        }
        for class in ctx.enclosing_classes() {
            if let Some(field) = lookup_field(db, &this_ty(db, class), name) {
                instance_members(acc, ctx, &field.ty, method_ref);
                return Some(field.ty);
            }
        }
    }
    match resolve_type_path(db, &ctx.scope, path) {
        Some(TypeNs::Class(class)) => static_members(acc, ctx, class, method_ref),
        Some(TypeNs::TypeParam(_)) => {}
        None => {
            let package = path.join("/");
            if package_exists(db, &package) {
                package_members(acc, ctx, &package, TypeLocation::Other);
            }
        }
    }
    None
}

/// `Type::|`
pub(crate) fn complete_type_method_ref(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    ty: &ast::Type,
) {
    let ty = ctx.sema.resolve_type(ctx.file, ty);
    match &ty {
        Ty::Array(_) => {}
        _ => members_of(acc, ctx, &ty, MemberFilter::Instance, true, true),
    }
    acc.add_keyword(ctx, "new");
}

const SCOPE_KINDS: &[SyntaxKind] = &[
    SyntaxKind::BLOCK,
    SyntaxKind::SWITCH_CASE,
    SyntaxKind::FOR_STMT,
    SyntaxKind::FOREACH_STMT,
    SyntaxKind::TRY_STMT,
    SyntaxKind::CATCH_CLAUSE,
    SyntaxKind::LAMBDA_EXPR,
    SyntaxKind::METHOD_DECL,
    SyntaxKind::CONSTRUCTOR_DECL,
    SyntaxKind::CLASS_BODY,
    SyntaxKind::INITIALIZER_BLOCK,
];

fn local_in_scope(root: &SyntaxNode, local: &LocalData, offset: TextSize) -> bool {
    let is_member = matches!(
        local.kind,
        LocalKind::LocalClassField | LocalKind::LocalClassMethod
    );
    if !is_member && local.decl_range.end() > offset {
        return false;
    }
    if !root.text_range().contains_range(local.decl_range) {
        return false;
    }
    let name_node = match root.covering_element(local.decl_range) {
        syntax::NodeOrToken::Node(node) => node,
        syntax::NodeOrToken::Token(token) => match token.parent() {
            Some(node) => node,
            None => return false,
        },
    };
    name_node
        .ancestors()
        .skip(1)
        .find(|n| SCOPE_KINDS.contains(&n.kind()))
        .is_some_and(|scope| scope.text_range().contains_inclusive(offset))
}

/// 光标处可见的局部符号；内层的同名声明遮蔽外层的。
pub(crate) fn visible_locals(ctx: &CompletionContext<'_>) -> Vec<LocalData> {
    let Some(body) = ctx.body else {
        return Vec::new();
    };
    let result = hir::infer(ctx.db, body);
    let mut res: Vec<LocalData> = Vec::new();
    // 靠后声明的先处理，这样同名时保留离光标最近的
    let mut locals: Vec<&LocalData> = result
        .locals()
        .iter()
        .filter(|local| local_in_scope(&ctx.original_root, local, ctx.offset))
        .collect();
    locals.sort_by_key(|local| std::cmp::Reverse(local.decl_range.start()));
    for local in locals {
        if !res.iter().any(|seen| seen.name == local.name) {
            res.push(local.clone());
        }
    }
    res
}

/// 静态 import 带进来的成员。
fn static_imports(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let db = ctx.db;
    let tree = item_tree(db, ctx.file);
    for import in tree.imports.iter().filter(|i| i.is_static) {
        if import.is_on_demand {
            if let Some(class) = resolve_qualified_class(db, &import.segments) {
                let ty = Ty::class(class);
                for field in all_fields(db, &ty) {
                    let flags = field_flags(ctx, &field);
                    if flags.is_static() && visible(ctx, flags, field.id.class) {
                        acc.add_field(ctx, &field, score::STATIC_IMPORT);
                    }
                }
                for method in all_methods(db, &ty) {
                    let flags = method_flags(ctx, &method);
                    if flags.is_static() && visible(ctx, flags, method.id.class) {
                        acc.add_method(ctx, &method, score::STATIC_IMPORT, false);
                    }
                }
            }
        } else if let Some((member, qualifier)) = import.segments.split_last() {
            if !Completions::matches(ctx, member) {
                continue;
            }
            let Some(class) = resolve_qualified_class(db, qualifier) else {
                continue;
            };
            let ty = Ty::class(class);
            if let Some(field) = lookup_field(db, &ty, member) {
                acc.add_field(ctx, &field, score::STATIC_IMPORT);
            }
            for method in lookup_methods(db, &ty, member) {
                acc.add_method(ctx, &method, score::STATIC_IMPORT, false);
            }
        }
    }
}

/// 不带限定符的类型名：作用域内的类型，加上按名字搜到的未导入类型。
pub(crate) fn types_in_scope(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    location: TypeLocation,
) {
    let db = ctx.db;

    // 类型参数
    if location != TypeLocation::Annotation && location != TypeLocation::New {
        let mut add_params = |names: Vec<SmolStr>| {
            for name in names {
                if Completions::matches(ctx, &name) {
                    let mut item = crate::item::CompletionItem {
                        label: name.to_string(),
                        kind: CompletionItemKind::Symbol(SymbolKind::TypeParam),
                        label_detail: None,
                        detail: None,
                        lookup: name.to_string(),
                        insert_text: name.to_string(),
                        is_snippet: false,
                        source_range: ctx.source_range,
                        additional_edits: Vec::new(),
                        deprecated: false,
                        score: score::LOCAL,
                        resolve: None,
                    };
                    item.detail = Some(base_db::tr!("类型参数", "type parameter"));
                    acc.add(item);
                }
            }
        };
        if let Some(method) = ctx.scope.method {
            if let Some(data) = class_data(db, method.class) {
                if let Some(m) = data.methods.get(method.idx as usize) {
                    add_params(m.type_params.iter().map(|p| p.name.clone()).collect());
                }
            }
        }
        for class in ctx.enclosing_classes() {
            if let Some(data) = class_data(db, class) {
                add_params(data.type_params.iter().map(|p| p.name.clone()).collect());
            }
        }
    }

    // 外层类及其父类的成员类型
    for class in ctx.enclosing_classes() {
        let anonymous = class_data(db, class)
            .is_some_and(|data| data.flags.contains(hir::Modifiers::ANONYMOUS));
        if !anonymous {
            acc.add_known_class(ctx, class, score::TYPE_IN_SCOPE, location, false);
        }
        nested_classes(acc, ctx, class, location);
        for sup in hir::all_supers(db, class) {
            nested_classes(acc, ctx, sup, location);
        }
    }

    let tree = item_tree(db, ctx.file);
    // 单类型 import
    for import in tree
        .imports
        .iter()
        .filter(|i| !i.is_on_demand && !i.is_static)
    {
        let Some(last) = import.segments.last() else {
            continue;
        };
        if !Completions::matches(ctx, last) {
            continue;
        }
        if let Some(class) = resolve_qualified_class(db, &import.segments) {
            acc.add_known_class(ctx, class, score::TYPE_IN_SCOPE + 2, location, false);
        }
    }
    // 本文件的顶层类型、同包类型
    for class in tree.top_level_classes() {
        let id = ClassId::from_binary_name(db, &class.binary_name);
        acc.add_known_class(ctx, id, score::TYPE_IN_SCOPE + 2, location, false);
    }
    let mut packages: Vec<String> = vec![tree.package.to_string()];
    // 按需 import 的包、`java.lang`
    for import in tree
        .imports
        .iter()
        .filter(|i| i.is_on_demand && !i.is_static)
    {
        packages.push(import.segments.join("/"));
    }
    packages.push("java/lang".to_owned());
    // 模块导入带进来的包
    let graph = hir::module_graph(db);
    for module in &tree.module_imports {
        for package in graph.imported_packages(module).unwrap_or_default() {
            packages.push(package.to_string());
        }
    }
    // 前缀为空时不展开整个包，候选太多没有意义
    if !ctx.prefix.is_empty() {
        for package in &packages {
            for name in package_classes(db, package) {
                if !Completions::matches(ctx, &name) {
                    continue;
                }
                let binary_name = if package.is_empty() {
                    name.to_string()
                } else {
                    format!("{package}/{name}")
                };
                if let Some(class) = lookup_class(db, &binary_name) {
                    acc.add_known_class(ctx, class, score::TYPE_IN_SCOPE, location, true);
                }
            }
        }
    }

    // 未导入的类型：按名字在符号索引里搜
    if ctx.prefix.is_empty() || !ctx.config.unimported_types {
        return;
    }
    let limit = ctx.config.unimported_limit;
    let source = source_symbol_index(db);
    let binary = binary_class_index(db);
    let hits = source
        .search(&ctx.prefix, limit * 2)
        .into_iter()
        .chain(binary.search(&ctx.prefix, limit));
    for (fuzzy, name, entry) in hits {
        let Definition::Class(class) = &entry.def else {
            continue;
        };
        // 索引里的打分反映名字的匹配质量，折算进相关度
        let bonus = (fuzzy / 8).clamp(0, 10);
        acc.add_class(
            ctx,
            *class,
            name,
            entry.kind,
            entry.deprecated,
            score::TYPE_UNIMPORTED + bonus,
            location,
            true,
        );
    }
}

/// 带限定符的类型名 `a.b.|`。
pub(crate) fn complete_qualified_type(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    qualifier: &[SmolStr],
    location: TypeLocation,
) {
    let db = ctx.db;
    match resolve_type_path(db, &ctx.scope, qualifier) {
        Some(TypeNs::Class(class)) => nested_classes(acc, ctx, class, location),
        Some(TypeNs::TypeParam(_)) => {}
        None => {
            let package = qualifier.join("/");
            if package_exists(db, &package) {
                package_members(acc, ctx, &package, location);
            }
        }
    }
}

/// `import a.b.|` / `import static a.b.C.|`
pub(crate) fn complete_import(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    qualifier: &[SmolStr],
    is_static: bool,
) {
    let db = ctx.db;
    if let Some(class) = resolve_qualified_class(db, qualifier) {
        nested_classes(acc, ctx, class, TypeLocation::Other);
        if is_static {
            members_of(
                acc,
                ctx,
                &Ty::class(class),
                MemberFilter::Static,
                false,
                true,
            );
        }
        return;
    }
    let package = qualifier.join("/");
    if qualifier.is_empty() || package_exists(db, &package) {
        package_members(acc, ctx, &package, TypeLocation::Other);
    }
}

/// 模块名：已经写出的前几段是 `qualifier`，补出剩下的部分（`java.|` → `sql`、`net.http`）。
pub(crate) fn complete_module_name(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    qualifier: &[SmolStr],
) {
    let graph = hir::module_graph(ctx.db);
    for name in graph.names() {
        let segments: Vec<&str> = name.split('.').collect();
        if segments.len() <= qualifier.len()
            || !segments
                .iter()
                .zip(qualifier)
                .all(|(a, b)| *a == b.as_str())
        {
            continue;
        }
        acc.add_package(ctx, &segments[qualifier.len()..].join("."));
    }
}

/// `exports a.|` / `opens a.|`：工作区源码里的包。
pub(crate) fn complete_module_package(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    qualifier: &[SmolStr],
) {
    let index = hir::source_class_index(ctx.db);
    let prefix = qualifier.join("/");
    let mut names: Vec<String> = index
        .packages()
        .filter_map(|package| {
            let rest = if prefix.is_empty() {
                package
            } else {
                package.strip_prefix(prefix.as_str())?.strip_prefix('/')?
            };
            (!rest.is_empty()).then(|| rest.split('/').next().unwrap_or(rest).to_owned())
        })
        .collect();
    names.sort();
    names.dedup();
    for name in names {
        acc.add_package(ctx, &name);
    }
}

/// `package a.b.|`
pub(crate) fn complete_package(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    qualifier: &[SmolStr],
) {
    for name in subpackages(ctx.db, &qualifier.join("/")) {
        acc.add_package(ctx, &name);
    }
}

/// 表达式 / 语句开头：局部符号、当前类及外层类的成员、静态 import、类型。
pub(crate) fn complete_expr(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    switch_scrutinee: Option<&Ty>,
) {
    let db = ctx.db;

    // `case` 标签里优先给出枚举常量
    if let Some(class) = switch_scrutinee.and_then(|ty| ty.as_class()) {
        if class_data(db, class).is_some_and(|d| d.kind == hir::ClassKind::Enum) {
            for field in all_fields(db, &Ty::class(class)) {
                let is_constant = class_data(db, field.id.class)
                    .and_then(|d| {
                        d.fields
                            .get(field.id.idx as usize)
                            .map(|f| f.is_enum_constant())
                    })
                    .unwrap_or(false);
                if is_constant {
                    acc.add_field(ctx, &field, score::ENUM_CASE);
                }
            }
        }
    }

    // `T x = |`：正在声明的变量自己还不能用
    let declaring = ctx
        .original_root
        .token_at_offset(ctx.offset)
        .left_biased()
        .and_then(|t| t.parent_ancestors().find_map(ast::LocalVarDecl::cast))
        .and_then(|decl| {
            decl.declarators()
                .filter(|d| d.syntax().text_range().start() <= ctx.offset)
                .last()
        })
        .and_then(|d| d.name())
        .map(|n| n.syntax().text_range())
        .filter(|name| name.end() <= ctx.offset);
    for local in visible_locals(ctx) {
        if Some(local.decl_range) == declaring {
            continue;
        }
        acc.add_local(ctx, &local);
    }

    for (depth, class) in ctx.enclosing_classes().into_iter().enumerate() {
        // 静态上下文说的是体所属的具名类；它里面的局部类 / 匿名类有自己的实例
        let filter = MemberFilter::Unqualified {
            static_only: Some(class) == ctx.enclosing_class && ctx.in_static_context,
        };
        let before = acc.items.len();
        members_of(acc, ctx, &this_ty(db, class), filter, false, false);
        // 外层类的成员略微靠后
        for item in &mut acc.items[before..] {
            item.score -= depth as i32 * 3;
        }
    }

    static_imports(acc, ctx);
    types_in_scope(acc, ctx, TypeLocation::Other);
}

/// `@Foo(|)`：注解的元素名。
pub(crate) fn complete_annotation_arg(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    annotation: ClassId,
) {
    let Some(data) = class_data(ctx.db, annotation) else {
        return;
    };
    for method in data
        .methods
        .iter()
        .filter(|m| !m.is_constructor && m.params.is_empty())
    {
        if !Completions::matches(ctx, &method.name) {
            continue;
        }
        let item = crate::item::CompletionItem {
            label: method.name.to_string(),
            kind: CompletionItemKind::Symbol(SymbolKind::Method),
            label_detail: None,
            detail: None,
            lookup: method.name.to_string(),
            insert_text: format!("{} = ", method.name),
            is_snippet: false,
            source_range: ctx.source_range,
            additional_edits: Vec::new(),
            deprecated: method.flags.is_deprecated(),
            score: score::MEMBER,
            resolve: None,
        };
        acc.add(item);
    }
}
