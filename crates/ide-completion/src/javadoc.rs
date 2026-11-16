//! 文档注释里的补全：标签名、`@param` 的形参名、`{@link}` / `@see` / `@throws` 引用的类型和成员。
//! 引用里的类型和代码里的类型一样补全（作用域里的、可以自动导入的），见 `lib.rs` 的分发。

use hir::{
    all_fields, all_methods, class_data, method_sig, resolve_type_path, ClassId, Modifiers, Ty,
    TypeNs,
};
use ide_db::{doc_links::doc_param_type, SymbolKind};
use rustc_hash::FxHashSet;
use smol_str::SmolStr;

use crate::{
    context::{CompletionContext, DocOwner, JavadocParams},
    item::{score, CompletionItem, CompletionItemKind, CompletionResolve, Completions},
};

const JAVADOC_BLOCK_TAGS: &[(&str, &str, &str)] = &[
    (
        "param",
        "形参或类型参数的说明",
        "Describes a parameter or type parameter",
    ),
    ("return", "返回值的说明", "Describes the return value"),
    (
        "throws",
        "可能抛出的异常",
        "An exception that may be thrown",
    ),
    ("see", "参见", "See also"),
    (
        "since",
        "从哪个版本开始提供",
        "The version this was introduced in",
    ),
    (
        "deprecated",
        "已弃用及替代方案",
        "Deprecated, with the replacement",
    ),
    ("author", "作者", "Author"),
    ("version", "版本", "Version"),
    ("apiNote", "API 使用说明", "API usage note"),
    ("implSpec", "实现规约", "Implementation specification"),
    ("implNote", "实现说明", "Implementation note"),
    ("serial", "序列化字段说明", "Describes a serialized field"),
    (
        "hidden",
        "不出现在生成的文档里",
        "Hidden from the generated documentation",
    ),
];

const JAVADOC_INLINE_TAGS: &[(&str, &str, &str)] = &[
    ("code", "等宽的代码文本", "Text in code font"),
    ("link", "指向类或成员的链接", "Link to a class or member"),
    ("linkplain", "普通字体的链接", "Link in plain font"),
    (
        "literal",
        "不解释 HTML 的文本",
        "Text without HTML interpretation",
    ),
    ("value", "常量的值", "The value of a constant"),
    (
        "inheritDoc",
        "继承父类型的文档",
        "Inherit the documentation of the supertype",
    ),
    ("snippet", "代码片段", "Code snippet"),
    ("summary", "摘要", "Summary"),
    ("index", "索引项", "Index entry"),
    ("systemProperty", "系统属性", "System property"),
];

/// 标签能不能写在这种声明的文档里（`Other` 不确定，都给）。
fn tag_applies(tag: &str, owner: DocOwner) -> bool {
    use DocOwner::*;
    match tag {
        "param" => matches!(
            owner,
            Method { .. } | Constructor | Type { params: true } | Other
        ),
        "return" => matches!(
            owner,
            Method {
                returns_value: true
            } | Other
        ),
        "throws" => matches!(owner, Method { .. } | Constructor | Other),
        "author" | "version" => matches!(owner, Type { .. } | Other),
        "serial" => matches!(owner, Field | Type { .. } | Other),
        "inheritDoc" => matches!(owner, Method { .. } | Other),
        _ => true,
    }
}

pub(crate) fn javadoc_tags(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    inline: bool,
    owner: DocOwner,
) {
    let tags = if inline {
        JAVADOC_INLINE_TAGS
    } else {
        JAVADOC_BLOCK_TAGS
    };
    for (rank, (tag, zh, en)) in tags.iter().enumerate() {
        let detail = match base_db::lang() {
            base_db::Lang::Zh => zh,
            base_db::Lang::En => en,
        };
        if !Completions::matches(ctx, tag) || !tag_applies(tag, owner) {
            continue;
        }
        acc.add(CompletionItem {
            label: (*tag).to_owned(),
            kind: CompletionItemKind::Keyword,
            label_detail: None,
            detail: Some((*detail).to_owned()),
            lookup: (*tag).to_owned(),
            insert_text: (*tag).to_owned(),
            is_snippet: false,
            source_range: ctx.source_range,
            additional_edits: Vec::new(),
            deprecated: false,
            // 保持上面列出的顺序（常用的在前）
            score: score::KEYWORD - rank as i32,
            resolve: None,
        });
    }
}

/// `@param |`：`names` 是形参名和 `<T>`，以及这条注释里是否已经写过它。
pub(crate) fn javadoc_params(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    names: &[(SmolStr, bool)],
) {
    for (rank, (name, documented)) in names.iter().enumerate() {
        let bare = name.trim_start_matches('<').trim_end_matches('>');
        if !Completions::matches(ctx, bare) {
            continue;
        }
        // `@param <|` 时插入的是 `T>`，标签仍然写成 `<T>`
        let label = if name.starts_with('<') || !name.ends_with('>') {
            name.to_string()
        } else {
            format!("<{name}")
        };
        acc.add(CompletionItem {
            label,
            kind: CompletionItemKind::Symbol(SymbolKind::Param),
            label_detail: None,
            detail: None,
            lookup: bare.to_owned(),
            insert_text: name.to_string(),
            is_snippet: false,
            source_range: ctx.source_range,
            additional_edits: Vec::new(),
            deprecated: false,
            // 已经写过说明的排到后面；间隔取 10，保证声明顺序不会被使用频度的加成打乱
            score: score::LOCAL - rank as i32 * 10 - if *documented { 200 } else { 0 },
            resolve: None,
        });
    }
}

/// `@throws |`：方法 `throws` 子句里声明的异常排在最前面。
pub(crate) fn javadoc_declared_throws(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let Some(hir::BodyId::Method(method)) = ctx.body else {
        return;
    };
    let db = ctx.db;
    let declared: Vec<SmolStr> = method_sig(db, method)
        .throws
        .iter()
        .filter_map(Ty::as_class)
        .map(|class| class.key(db))
        .collect();
    for item in &mut acc.items {
        if let Some(CompletionResolve::Class { binary_name }) = &item.resolve {
            if declared.contains(binary_name) {
                item.score += score::EXPECTED_TYPE;
            }
        }
    }
}

/// 文档引用里 `#` 前面的类型：为空时是被注释的类自己，再往外层类找（和解析文档引用时一样）。
fn referenced_classes(ctx: &CompletionContext<'_>, ty: &[SmolStr]) -> Vec<ClassId> {
    if ty.is_empty() {
        ctx.enclosing_classes()
    } else {
        match resolve_type_path(ctx.db, &ctx.scope, ty) {
            Some(TypeNs::Class(class)) => vec![class],
            _ => Vec::new(),
        }
    }
}

/// 方法在文档引用里的形参列表：`int, java.util.List, String...`。
fn doc_signature(db: &dyn hir::Db, id: hir::MethodId) -> Vec<String> {
    let sig = method_sig(db, id);
    let mut params: Vec<String> = sig.params.iter().map(|ty| doc_param_type(db, ty)).collect();
    if sig.varargs {
        if let Some(last) = params.last_mut() {
            if let Some(element) = last.strip_suffix("[]") {
                *last = format!("{element}...");
            }
        }
    }
    params
}

/// `{@link #foo(|}`：`foo` 每个重载的整组形参类型，排在单个类型的前面。
pub(crate) fn javadoc_overload_params(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    params: &JavadocParams,
) {
    let db = ctx.db;
    let Some(&class) = referenced_classes(ctx, &params.ty).first() else {
        return;
    };
    let class_name = class_data(db, class).map(|d| d.name.clone());
    let constructor = class_name.as_deref() == Some(params.name.as_str());
    let candidates: Vec<hir::MethodId> = if constructor {
        class_data(db, class)
            .map(|data| {
                data.constructors()
                    .map(|(idx, _)| hir::MethodId { class, idx })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        all_methods(db, &Ty::class(class))
            .into_iter()
            .map(|m| m.id)
            .collect()
    };
    let mut seen: FxHashSet<String> = FxHashSet::default();
    for (rank, id) in candidates.into_iter().enumerate() {
        let Some(data) = class_data(db, id.class) else {
            continue;
        };
        let Some(m) = data.methods.get(id.idx as usize) else {
            continue;
        };
        if m.is_constructor != constructor
            || (!constructor && m.name != params.name)
            || m.flags.contains(Modifiers::SYNTHETIC)
            || !member_visible(m.flags, id.class, class)
        {
            continue;
        }
        let list = doc_signature(db, id).join(", ");
        if list.is_empty() || !Completions::matches(ctx, &list) || !seen.insert(list.clone()) {
            continue;
        }
        let close = if params.closed { "" } else { ")" };
        acc.add(CompletionItem {
            label: format!("{list})"),
            kind: CompletionItemKind::Symbol(if constructor {
                SymbolKind::Constructor
            } else {
                SymbolKind::Method
            }),
            label_detail: None,
            detail: None,
            lookup: list.clone(),
            insert_text: format!("{list}{close}"),
            is_snippet: false,
            source_range: ctx.source_range,
            additional_edits: Vec::new(),
            deprecated: m.flags.is_deprecated(),
            score: score::EXPECTED_TYPE + score::LOCAL - rank.min(50) as i32,
            resolve: Some(CompletionResolve::Method {
                class: id.class.key(db),
                idx: id.idx,
            }),
        });
    }
}

/// `{@link #|}`、`{@link Foo#|}`：类的字段、方法和构造器。方法插入 `name(形参类型, ...)`。
pub(crate) fn javadoc_members(acc: &mut Completions, ctx: &CompletionContext<'_>, ty: &[SmolStr]) {
    let db = ctx.db;
    let classes = referenced_classes(ctx, ty);
    let mut seen: FxHashSet<String> = FxHashSet::default();
    for (depth, &class) in classes.iter().enumerate() {
        let base = score::MEMBER - depth as i32 * 3;
        let this = Ty::class(class);
        for field in all_fields(db, &this) {
            let Some(data) = class_data(db, field.id.class) else {
                continue;
            };
            let Some(f) = data.fields.get(field.id.idx as usize) else {
                continue;
            };
            if !member_visible(f.flags, field.id.class, class)
                || !Completions::matches(ctx, &f.name)
                || !seen.insert(f.name.to_string())
            {
                continue;
            }
            let kind = if f.is_enum_constant() {
                SymbolKind::EnumConstant
            } else {
                SymbolKind::Field
            };
            let inherited = field.id.class != class;
            acc.add(CompletionItem {
                label: f.name.to_string(),
                kind: CompletionItemKind::Symbol(kind),
                label_detail: None,
                detail: Some(field.ty.display(db)),
                lookup: f.name.to_string(),
                insert_text: f.name.to_string(),
                is_snippet: false,
                source_range: ctx.source_range,
                additional_edits: Vec::new(),
                deprecated: f.flags.is_deprecated(),
                score: base - if inherited { 5 } else { 0 },
                resolve: Some(CompletionResolve::Field {
                    class: field.id.class.key(db),
                    idx: field.id.idx,
                }),
            });
        }
        let methods = all_methods(db, &this)
            .into_iter()
            .map(|m| m.id)
            .filter(|m| {
                class_data(db, m.class)
                    .and_then(|d| d.methods.get(m.idx as usize).map(|m| !m.is_constructor))
                    .unwrap_or(false)
            });
        // 构造器（`#Foo(int)`）只取这个类自己的
        let constructors = class_data(db, class)
            .map(|data| {
                data.constructors()
                    .map(|(idx, _)| hir::MethodId { class, idx })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for id in methods.chain(constructors) {
            let Some(data) = class_data(db, id.class) else {
                continue;
            };
            let Some(m) = data.methods.get(id.idx as usize) else {
                continue;
            };
            let name = if m.is_constructor {
                &data.name
            } else {
                &m.name
            };
            if m.flags.contains(Modifiers::SYNTHETIC)
                || !member_visible(m.flags, id.class, class)
                || !Completions::matches(ctx, name)
            {
                continue;
            }
            let sig = method_sig(db, id);
            let signature = format!("({})", doc_signature(db, id).join(", "));
            if !seen.insert(format!("{name}{signature}")) {
                continue;
            }
            let inherited = id.class != class;
            let from_object = &*id.class.binary_name(db) == "java/lang/Object";
            acc.add(CompletionItem {
                label: name.to_string(),
                kind: CompletionItemKind::Symbol(if m.is_constructor {
                    SymbolKind::Constructor
                } else {
                    SymbolKind::Method
                }),
                label_detail: Some(signature.clone()),
                detail: (!m.is_constructor).then(|| sig.ret.display(db)),
                lookup: name.to_string(),
                insert_text: format!("{name}{signature}"),
                is_snippet: false,
                source_range: ctx.source_range,
                additional_edits: Vec::new(),
                deprecated: m.flags.is_deprecated(),
                score: base - if inherited { 5 } else { 0 } - if m.is_constructor { 2 } else { 0 }
                    + if from_object { score::OBJECT_MEMBER } else { 0 },
                resolve: Some(CompletionResolve::Method {
                    class: id.class.key(db),
                    idx: id.idx,
                }),
            });
        }
    }
}

/// 文档能引用的成员：类自己的都行，继承来的不含私有的。
fn member_visible(flags: Modifiers, declaring: ClassId, class: ClassId) -> bool {
    declaring == class || flags.visibility() != hir::Visibility::Private
}
