//! 补全条目及其构造。

use hir::{
    class_data, is_assignable, lookup_field, lookup_methods, resolve_simple_type, ClassId,
    ClassKind, FieldCandidate, LocalData, LocalKind, MethodCandidate, Ty, TypeNs,
};
use ide_db::{
    imports::{importable_name, insert_import},
    render, SymbolKind, TextEdit,
};
use rustc_hash::FxHashSet;
use smol_str::SmolStr;
use syntax::TextRange;

use crate::context::{CompletionContext, ContextKind, Following, SupertypeKind, TypeLocation};

/// 补全位置所在文件的包名（斜杠形式，默认包为空串）。
fn file_package(ctx: &CompletionContext<'_>) -> String {
    <syntax::ast::CompilationUnit as syntax::ast::AstNode>::cast(ctx.original_root.clone())
        .and_then(|cu| cu.package())
        .and_then(|p| p.name())
        .map(|n| n.text().replace('.', "/"))
        .unwrap_or_default()
}

fn outermost(db: &dyn hir::Db, class: ClassId) -> SmolStr {
    let name = class.binary_name(db);
    match name.split_once('$') {
        Some((outer, _)) => SmolStr::new(outer),
        None => name,
    }
}

/// 类在补全位置能不能访问（JLS 6.6.1）：它自己和它的每一层外层类都要能访问。
/// 包私有的类只在同一个包里能用，私有的嵌套类只在同一个顶层类里能用，
/// 受保护的嵌套类还可以在外层类的子类里用。
fn class_accessible(ctx: &CompletionContext<'_>, class: ClassId) -> bool {
    let db = ctx.db;
    let package = file_package(ctx);
    let mut current = Some(class);
    for _ in 0..32 {
        let Some(c) = current else {
            break;
        };
        let Some(data) = class_data(db, c) else {
            return true;
        };
        let outer = hir::outer_class(db, c);
        let same_package = data.package() == package;
        let accessible = match data.flags.visibility() {
            hir::Visibility::Public => true,
            hir::Visibility::Package => same_package,
            hir::Visibility::Protected => {
                same_package
                    || match (ctx.enclosing_class, outer) {
                        (Some(from), Some(outer)) => {
                            hir::is_subclass_of(db, from, outer)
                                || Some(hir::outermost_class(db, from))
                                    .is_some_and(|top| hir::is_subclass_of(db, top, outer))
                        }
                        _ => false,
                    }
            }
            hir::Visibility::Private => ctx
                .enclosing_class
                .is_some_and(|from| outermost(db, from) == outermost(db, c)),
        };
        if !accessible {
            return false;
        }
        current = outer;
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionItemKind {
    Symbol(SymbolKind),
    Keyword,
    Snippet,
}

/// 延迟解析（`completionItem/resolve`）时用来找回定义的数据。只含可序列化的稳定标识。
/// 类用 `ClassId::key` 表示：主副本就是二进制名，同名类的其他副本还带着来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionResolve {
    Class {
        binary_name: SmolStr,
    },
    Method {
        class: SmolStr,
        idx: u32,
    },
    Field {
        class: SmolStr,
        idx: u32,
    },
    /// Spring 配置文件里的一个键（文档来自配置元数据）
    ConfigKey {
        key: SmolStr,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionItemKind,
    /// 紧跟在标签后面的灰色文字（方法的形参表、类所在的包）
    pub label_detail: Option<String>,
    /// 右侧的说明（类型）
    pub detail: Option<String>,
    /// 用于过滤的文本
    pub lookup: String,
    pub insert_text: String,
    /// `insert_text` 是否是带 `$0` / `${1:x}` 占位的片段
    pub is_snippet: bool,
    /// 被 `insert_text` 替换的范围
    pub source_range: TextRange,
    pub additional_edits: Vec<TextEdit>,
    pub deprecated: bool,
    /// 相关度，越大越靠前
    pub score: i32,
    pub resolve: Option<CompletionResolve>,
}

/// 各类候选的基础分。
pub(crate) mod score {
    pub(crate) const ENUM_CASE: i32 = 120;
    pub(crate) const LOCAL: i32 = 100;
    pub(crate) const MEMBER: i32 = 85;
    pub(crate) const STATIC_IMPORT: i32 = 70;
    pub(crate) const TYPE_IN_SCOPE: i32 = 60;
    pub(crate) const OVERRIDE: i32 = 58;
    pub(crate) const KEYWORD: i32 = 50;
    pub(crate) const POSTFIX: i32 = 45;
    pub(crate) const SNIPPET: i32 = 40;
    pub(crate) const PACKAGE: i32 = 35;
    pub(crate) const TYPE_UNIMPORTED: i32 = 20;

    pub(crate) const EXPECTED_TYPE: i32 = 30;
    /// 以已输入的前缀开头的候选要排在只是模糊匹配上的候选前面（`ret` → `return` 先于 `Deprecated`）
    pub(crate) const EXACT_PREFIX: i32 = 25;
    pub(crate) const DEPRECATED: i32 = -25;
    pub(crate) const OBJECT_MEMBER: i32 = -20;
    pub(crate) const STATIC_VIA_INSTANCE: i32 = -30;
    /// 候选和标识符后面已经写好的内容相容（`(` 前面的方法）/ 不相容（`(` 前面的变量）
    pub(crate) const FOLLOWING: i32 = 30;
    /// 改写已有调用时形参个数和实参个数对得上
    pub(crate) const ARITY: i32 = 8;
}

/// 候选在语法上是哪一类名字，用来和 [`Following`] 比较。
pub(crate) enum Shape<'a> {
    /// 方法；`ret` 是返回类型（不知道时为 `None`）
    Method {
        ret: Option<&'a Ty>,
    },
    Variable {
        ty: &'a Ty,
        assignable: bool,
    },
    Class(Option<ClassId>),
}

/// `x.name` 里的 `name` 在 `x` 的类型上有没有；类型不知道时为 `None`。
fn has_instance_member(db: &dyn hir::Db, ty: &Ty, name: &str) -> Option<bool> {
    match ty {
        Ty::Unknown => None,
        Ty::Primitive(_) | Ty::Void | Ty::Null => Some(false),
        Ty::Array(_) => Some(
            matches!(name, "length" | "clone")
                || !lookup_methods(db, &Ty::object(db), name).is_empty(),
        ),
        _ => Some(lookup_field(db, ty, name).is_some() || !lookup_methods(db, ty, name).is_empty()),
    }
}

/// `Foo.name` 里的 `name` 是不是 `Foo` 的静态成员、成员类型或 `class` / `this`。
fn has_static_member(db: &dyn hir::Db, class: ClassId, name: &str) -> bool {
    if matches!(name, "class" | "this" | "super") {
        return true;
    }
    let ty = Ty::class(class);
    lookup_field(db, &ty, name).is_some()
        || !lookup_methods(db, &ty, name).is_empty()
        || hir::member_type(db, class, name).is_some()
}

/// 标识符后面已经有 `(`、`=`、`.`、`[` 时，按候选能不能出现在这里调整相关度。
/// 只在表达式位置生效：类型位置的 `new Foo(` 里的 `(` 不说明什么。
/// 后面是 `.name` 时还看候选有没有叫 `name` 的成员（改写已有的 `a.name` 里的 `a`）。
pub(crate) fn following_bonus(ctx: &CompletionContext<'_>, shape: Shape<'_>) -> i32 {
    if !matches!(ctx.kind, ContextKind::Expr { .. } | ContextKind::Dot { .. }) {
        return 0;
    }
    let fits = score::FOLLOWING;
    let member_fits = |has: Option<bool>| match has {
        Some(true) => fits / 2,
        Some(false) => -fits,
        None => 0,
    };
    match (ctx.following, shape) {
        (Following::Other, _) => 0,
        // `|(...).name`：调用结果上要有 `name`
        (Following::Paren, Shape::Method { ret }) => {
            fits + match (&ctx.next_member, ret) {
                (Some(name), Some(ret)) => member_fits(has_instance_member(ctx.db, ret, name)),
                _ => 0,
            }
        }
        (Following::Paren, _) => -fits,
        (_, Shape::Method { .. }) => -fits,
        (Following::Assign, Shape::Variable { assignable, .. }) => {
            if assignable {
                fits / 2
            } else {
                0
            }
        }
        (Following::Assign | Following::Bracket, Shape::Class(_)) => -fits,
        (Following::Dot, Shape::Variable { ty, .. }) => {
            if matches!(ty, Ty::Primitive(_)) {
                -fits
            } else {
                match &ctx.next_member {
                    Some(name) => member_fits(has_instance_member(ctx.db, ty, name)),
                    None => 0,
                }
            }
        }
        (Following::Bracket, Shape::Variable { ty, .. }) => {
            if matches!(ty, Ty::Array(_)) {
                fits / 2
            } else if ty.is_unknown() {
                0
            } else {
                -fits
            }
        }
        (Following::Dot, Shape::Class(class)) => match (&ctx.next_member, class) {
            (Some(name), Some(class)) => member_fits(Some(has_static_member(ctx.db, class, name))),
            _ => 0,
        },
    }
}

#[derive(Default)]
pub(crate) struct Completions {
    pub(crate) items: Vec<CompletionItem>,
    seen_classes: FxHashSet<ClassId>,
}

impl Completions {
    fn base(
        ctx: &CompletionContext<'_>,
        label: impl Into<String>,
        kind: CompletionItemKind,
        score: i32,
    ) -> CompletionItem {
        let label = label.into();
        let exact = !ctx.prefix.is_empty() && label.starts_with(&ctx.prefix);
        CompletionItem {
            lookup: label.clone(),
            insert_text: label.clone(),
            label,
            kind,
            label_detail: None,
            detail: None,
            is_snippet: false,
            source_range: ctx.source_range,
            additional_edits: Vec::new(),
            deprecated: false,
            score: score + if exact { score::EXACT_PREFIX } else { 0 },
            resolve: None,
        }
    }

    fn type_bonus(ctx: &CompletionContext<'_>, ty: &Ty) -> i32 {
        let Some(expected) = &ctx.expected_ty else {
            return 0;
        };
        // 这里要一个值：返回 void 的方法用不上
        if *ty == Ty::Void {
            return -score::EXPECTED_TYPE;
        }
        if ty.is_unknown() || *expected == Ty::object(ctx.db) {
            return 0;
        }
        if is_assignable(ctx.db, ty, expected) {
            score::EXPECTED_TYPE
        } else {
            0
        }
    }

    /// 类型位置有期望类型时（`throw new |`、`catch (|`、`List<T> x = new |`），它的子类排在前面。
    fn class_type_bonus(ctx: &CompletionContext<'_>, class: ClassId) -> i32 {
        if !matches!(
            ctx.kind,
            ContextKind::Type { .. } | ContextKind::JavadocType { .. }
        ) {
            return 0;
        }
        let db = ctx.db;
        if let Some(supertype) = ctx.supertype {
            let Some(data) = class_data(db, class) else {
                return 0;
            };
            let fits = match supertype {
                SupertypeKind::Interface => data.kind == ClassKind::Interface,
                SupertypeKind::Class => data.kind == ClassKind::Class && !data.flags.is_final(),
            };
            // 正在声明的类（和它的外层类）不能做自己的父类型
            let declaring = ctx.enclosing_classes().contains(&class);
            return if fits && !declaring {
                score::EXPECTED_TYPE
            } else {
                0
            };
        }
        // 已经写好的变量名像是由这个类型名起的
        let mut bonus = 0;
        if let Some(elements) = &ctx.annotation_elements {
            let has_all = class_data(db, class).is_some_and(|data| {
                elements
                    .iter()
                    .all(|e| data.methods.iter().any(|m| m.name == *e))
            });
            if has_all && !elements.is_empty() {
                bonus += score::EXPECTED_TYPE;
            }
        }
        if let Some(declared) = &ctx.declared_name {
            if class_data(db, class)
                .is_some_and(|data| crate::context::name_suggests_type(declared, &data.name))
            {
                bonus += score::EXPECTED_TYPE / 2;
            }
        }
        // `| x = new Foo(...)`：声明的类型多半就是 `Foo` 或它的父类型
        if let Some(init) = ctx.initializer_class {
            if init == class {
                bonus += score::EXPECTED_TYPE;
            } else if &*class.binary_name(db) != "java/lang/Object"
                && hir::is_subclass_of(db, init, class)
            {
                bonus += score::EXPECTED_TYPE / 2;
            }
        }
        let Some(expected) = ctx.expected_ty.as_ref().and_then(Ty::as_class) else {
            return bonus;
        };
        if &*expected.binary_name(db) == "java/lang/Object" {
            return bonus;
        }
        if hir::is_subclass_of(db, class, expected) {
            bonus += score::EXPECTED_TYPE;
        }
        bonus
    }

    pub(crate) fn matches(ctx: &CompletionContext<'_>, name: &str) -> bool {
        ide_db::fuzzy::matches_prefix(&ctx.prefix, name)
    }

    pub(crate) fn add(&mut self, item: CompletionItem) {
        self.items.push(item);
    }

    pub(crate) fn add_local(&mut self, ctx: &CompletionContext<'_>, local: &LocalData) {
        if !Self::matches(ctx, &local.name) {
            return;
        }
        let kind = match local.kind {
            LocalKind::Param | LocalKind::LambdaParam | LocalKind::CatchParam => SymbolKind::Param,
            LocalKind::LocalClassField => SymbolKind::Field,
            LocalKind::LocalClassMethod => SymbolKind::Method,
            _ => SymbolKind::Local,
        };
        let shape = if local.kind == LocalKind::LocalClassMethod {
            Shape::Method { ret: None }
        } else {
            Shape::Variable {
                ty: &local.ty,
                assignable: true,
            }
        };
        let mut item = Self::base(
            ctx,
            local.name.as_str(),
            CompletionItemKind::Symbol(kind),
            score::LOCAL + Self::type_bonus(ctx, &local.ty) + following_bonus(ctx, shape),
        );
        item.detail = Some(local.ty.display(ctx.db));
        if local.kind == LocalKind::LocalClassMethod && !ctx.followed_by_paren {
            item.insert_text = format!("{}()", local.name);
        }
        self.add(item);
    }

    pub(crate) fn add_field(
        &mut self,
        ctx: &CompletionContext<'_>,
        field: &FieldCandidate,
        base_score: i32,
    ) {
        let db = ctx.db;
        let Some(data) = class_data(db, field.id.class) else {
            return;
        };
        let Some(f) = data.fields.get(field.id.idx as usize) else {
            return;
        };
        if !Self::matches(ctx, &f.name) {
            return;
        }
        let kind = if f.is_enum_constant() {
            SymbolKind::EnumConstant
        } else {
            SymbolKind::Field
        };
        // final 字段只能在构造器和初始化块里赋值
        let initializing = match ctx.body {
            Some(hir::BodyId::Method(m)) => class_data(db, m.class)
                .and_then(|d| d.methods.get(m.idx as usize).map(|m| m.is_constructor))
                .unwrap_or(false),
            Some(hir::BodyId::Initializer { .. }) => true,
            _ => false,
        };
        let shape = Shape::Variable {
            ty: &field.ty,
            assignable: !f.flags.is_final() || initializing,
        };
        let mut item = Self::base(
            ctx,
            f.name.as_str(),
            CompletionItemKind::Symbol(kind),
            base_score + Self::type_bonus(ctx, &field.ty) + following_bonus(ctx, shape),
        );
        // `Color.|` 要的多半是常量，不是 `values()` / `valueOf()`
        if f.is_enum_constant() {
            item.score += 5;
        }
        item.detail = Some(field.ty.display(db));
        item.deprecated = f.flags.is_deprecated();
        if item.deprecated {
            item.score += score::DEPRECATED;
        }
        item.resolve = Some(CompletionResolve::Field {
            class: field.id.class.key(db),
            idx: field.id.idx,
        });
        self.add(item);
    }

    /// `name_only`：只插入方法名（方法引用、静态 import）。
    pub(crate) fn add_method(
        &mut self,
        ctx: &CompletionContext<'_>,
        method: &MethodCandidate,
        base_score: i32,
        name_only: bool,
    ) {
        let db = ctx.db;
        let Some(data) = class_data(db, method.id.class) else {
            return;
        };
        let Some(m) = data.methods.get(method.id.idx as usize) else {
            return;
        };
        if m.is_constructor || !Self::matches(ctx, &m.name) {
            return;
        }
        let ret = method.ret_ty(db);
        // `Object` 的方法不因为返回类型合适而提前：它们总是排在接收者自己的成员后面
        let from_object = &*method.id.class.binary_name(db) == "java/lang/Object";
        let bonus = if from_object {
            0
        } else {
            Self::type_bonus(ctx, &ret)
        };
        // 改写已有调用的方法名：形参个数和已有实参个数对得上的排在前面
        let arity = match ctx.call_args {
            Some(args) if !name_only => {
                let params = m.params.len();
                let fits = if m.is_varargs() {
                    args + 1 >= params
                } else {
                    args == params
                };
                if fits {
                    score::ARITY
                } else {
                    0
                }
            }
            _ => 0,
        };
        let mut item = Self::base(
            ctx,
            m.name.as_str(),
            CompletionItemKind::Symbol(SymbolKind::Method),
            base_score
                + bonus
                + arity
                + if name_only {
                    0
                } else {
                    following_bonus(ctx, Shape::Method { ret: Some(&ret) })
                },
        );
        let params = render::method_params(db, method.id, &method.subst);
        item.label_detail = Some(format!("({})", params.join(", ")));
        item.detail = Some(ret.display(db));
        item.deprecated = m.flags.is_deprecated();
        if item.deprecated {
            item.score += score::DEPRECATED;
        }
        if from_object {
            item.score += score::OBJECT_MEMBER;
        }
        if !name_only && !ctx.followed_by_paren {
            if params.is_empty() || !ctx.config.snippets {
                item.insert_text = format!("{}()", m.name);
            } else {
                item.insert_text = format!("{}($0)", m.name);
                item.is_snippet = true;
            }
        }
        item.resolve = Some(CompletionResolve::Method {
            class: method.id.class.key(db),
            idx: method.id.idx,
        });
        self.add(item);
    }

    /// 加入一个类。`import` 为真时按需附带 import 编辑（或改用全限定名）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_class(
        &mut self,
        ctx: &CompletionContext<'_>,
        class: ClassId,
        name: &str,
        kind: SymbolKind,
        deprecated: bool,
        base_score: i32,
        location: TypeLocation,
        import: bool,
    ) {
        if !hir::is_preferred_from(ctx.db, ctx.file, class) || !class_accessible(ctx, class) {
            return;
        }
        let accepted = match location {
            TypeLocation::Annotation => kind == SymbolKind::Annotation,
            TypeLocation::New => !matches!(kind, SymbolKind::Annotation | SymbolKind::Enum),
            _ => true,
        };
        if !accepted || !Self::matches(ctx, name) || !self.seen_classes.insert(class) {
            return;
        }
        let db = ctx.db;
        // `@Target` 不允许写在这里的注解：被注解的东西确定时不给，否则排到最后
        let mut not_applicable = 0;
        if let Some(site) = ctx
            .annotation_site
            .filter(|_| location == TypeLocation::Annotation && kind == SymbolKind::Annotation)
        {
            let applicable = class_data(db, class)
                .is_none_or(|data| data.annotation_targets().intersects(site.accepts));
            if !applicable {
                if site.certain {
                    return;
                }
                not_applicable = -2 * score::EXPECTED_TYPE;
            }
        }
        // 注解类型只能写在 `@` 后面：表达式里几乎只会以 `Foo.class` 出现，也不能用来声明变量
        let misplaced_annotation = kind == SymbolKind::Annotation
            && match ctx.kind {
                ContextKind::Expr { .. } => ctx.following != Following::Dot,
                ContextKind::Type { .. } => location != TypeLocation::Annotation,
                _ => false,
            };
        let mut item = Self::base(
            ctx,
            name,
            CompletionItemKind::Symbol(kind),
            base_score
                + not_applicable
                + following_bonus(ctx, Shape::Class(Some(class)))
                + Self::class_type_bonus(ctx, class)
                + if misplaced_annotation {
                    -score::FOLLOWING
                } else {
                    0
                },
        );
        let binary_name = class.binary_name(db);
        if let Some(idx) = binary_name.rfind(['/', '$']) {
            item.label_detail = Some(format!(
                " ({})",
                binary_name[..idx].replace(['/', '$'], ".")
            ));
        }
        item.deprecated = deprecated;
        if deprecated {
            item.score += score::DEPRECATED;
        }

        let mut insert = name.to_owned();
        if import {
            match resolve_simple_type(db, &ctx.scope, name) {
                Some(TypeNs::Class(found)) if found == class => {}
                Some(_) => {
                    if let Some(qualified) = importable_name(db, class) {
                        insert = qualified;
                    }
                }
                None => {
                    if let Some(qualified) = importable_name(db, class) {
                        if ctx.config.auto_import {
                            item.additional_edits
                                .push(insert_import(db, ctx.file, &qualified));
                        } else {
                            insert = qualified;
                        }
                    }
                }
            }
        }

        if location == TypeLocation::New && !ctx.followed_by_paren {
            if let Some(data) = class_data(db, class) {
                // 菱形 `<>` 要 Java 7，用在匿名类上要 Java 9；更低的级别能从期望类型推出实参时
                // 写出显式的类型实参，推不出来就不写（原始类型）
                let type_args = |min_level: u32| -> String {
                    if data.type_params.is_empty() {
                        String::new()
                    } else if !ctx.level_below(min_level) {
                        "<>".to_owned()
                    } else {
                        crate::extras::type_args_from_expected(ctx, class)
                            .map(|args| {
                                let args: Vec<String> =
                                    args.iter().map(|arg| arg.display(db)).collect();
                                format!("<{}>", args.join(", "))
                            })
                            .unwrap_or_default()
                    }
                };
                let needs_body = data.kind == ClassKind::Interface || data.flags.is_abstract();
                let has_params = data.constructors().any(|(_, ctor)| !ctor.params.is_empty());
                if !ctx.config.snippets {
                    insert = format!("{insert}{}()", type_args(7));
                } else if needs_body {
                    // 需要实现的抽象方法都生成出来；生成不了时（泛型方法、不知道类型实参）给空的类体
                    match crate::extras::anonymous_class_stubs(ctx, class, &insert) {
                        Some((header, body)) => insert = format!("{header}() {body}"),
                        None => {
                            let indent = &ctx.indent;
                            let diamond = type_args(9);
                            insert = format!("{insert}{diamond}() {{\n{indent}    $0\n{indent}}}");
                        }
                    }
                    item.is_snippet = true;
                } else if has_params {
                    insert = format!("{insert}{}($0)", type_args(7));
                    item.is_snippet = true;
                } else {
                    insert = format!("{insert}{}()", type_args(7));
                }
            }
        }
        item.insert_text = insert;
        item.resolve = Some(CompletionResolve::Class {
            binary_name: class.key(db),
        });
        self.add(item);
    }

    /// 已经加载过签名的类（作用域内的类）。
    pub(crate) fn add_known_class(
        &mut self,
        ctx: &CompletionContext<'_>,
        class: ClassId,
        base_score: i32,
        location: TypeLocation,
        import: bool,
    ) {
        let Some(data) = class_data(ctx.db, class) else {
            return;
        };
        self.add_class(
            ctx,
            class,
            &data.name,
            SymbolKind::of_class(data.kind),
            data.flags.is_deprecated(),
            base_score,
            location,
            import,
        );
    }

    pub(crate) fn add_package(&mut self, ctx: &CompletionContext<'_>, name: &str) {
        if !Self::matches(ctx, name) {
            return;
        }
        let item = Self::base(
            ctx,
            name,
            CompletionItemKind::Symbol(SymbolKind::Package),
            score::PACKAGE,
        );
        self.add(item);
    }

    pub(crate) fn add_keyword(&mut self, ctx: &CompletionContext<'_>, keyword: &str) {
        if !Self::matches(ctx, keyword) {
            return;
        }
        let item = Self::base(ctx, keyword, CompletionItemKind::Keyword, score::KEYWORD);
        self.add(item);
    }

    /// 片段。`body` 里的换行会自动接上当前行的缩进。
    pub(crate) fn add_snippet(
        &mut self,
        ctx: &CompletionContext<'_>,
        label: &str,
        detail: &str,
        body: &str,
        base_score: i32,
    ) -> Option<&mut CompletionItem> {
        if !ctx.config.snippets || !Self::matches(ctx, label) {
            return None;
        }
        let mut item = Self::base(ctx, label, CompletionItemKind::Snippet, base_score);
        item.detail = Some(detail.to_owned());
        item.insert_text = body.replace('\n', &format!("\n{}", ctx.indent));
        item.is_snippet = true;
        self.items.push(item);
        self.items.last_mut()
    }
}
