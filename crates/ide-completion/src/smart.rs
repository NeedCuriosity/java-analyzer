//! 按期望类型给出的候选：`new` 之后期望类型的实现类、函数式接口位置的 lambda、
//! 期望某个类时它自己的常量（枚举常量、`BigDecimal.ZERO`）。

use hir::{all_fields, class_data, is_assignable, ClassId, ClassKind, Ty};
use ide_db::{imports::importable_name, imports::insert_import, SymbolKind};

use crate::{
    context::{CompletionContext, TypeLocation},
    item::{score, CompletionItemKind, Completions},
};

/// 常用接口 / 抽象类的标准实现。
const KNOWN_IMPLEMENTATIONS: &[(&str, &[&str])] = &[
    (
        "java/util/List",
        &["java/util/ArrayList", "java/util/LinkedList"],
    ),
    (
        "java/util/Collection",
        &["java/util/ArrayList", "java/util/HashSet"],
    ),
    ("java/lang/Iterable", &["java/util/ArrayList"]),
    (
        "java/util/Set",
        &[
            "java/util/HashSet",
            "java/util/LinkedHashSet",
            "java/util/TreeSet",
        ],
    ),
    ("java/util/SortedSet", &["java/util/TreeSet"]),
    ("java/util/NavigableSet", &["java/util/TreeSet"]),
    (
        "java/util/Map",
        &[
            "java/util/HashMap",
            "java/util/LinkedHashMap",
            "java/util/TreeMap",
            "java/util/concurrent/ConcurrentHashMap",
        ],
    ),
    ("java/util/SortedMap", &["java/util/TreeMap"]),
    ("java/util/NavigableMap", &["java/util/TreeMap"]),
    (
        "java/util/concurrent/ConcurrentMap",
        &["java/util/concurrent/ConcurrentHashMap"],
    ),
    (
        "java/util/Queue",
        &[
            "java/util/ArrayDeque",
            "java/util/LinkedList",
            "java/util/PriorityQueue",
        ],
    ),
    (
        "java/util/Deque",
        &["java/util/ArrayDeque", "java/util/LinkedList"],
    ),
    (
        "java/util/concurrent/BlockingQueue",
        &[
            "java/util/concurrent/LinkedBlockingQueue",
            "java/util/concurrent/ArrayBlockingQueue",
        ],
    ),
    ("java/lang/CharSequence", &["java/lang/StringBuilder"]),
    ("java/lang/Appendable", &["java/lang/StringBuilder"]),
    (
        "java/util/concurrent/locks/Lock",
        &["java/util/concurrent/locks/ReentrantLock"],
    ),
];

/// `new` 之后：期望类型本身（是具体类时）和它的实现类排在最前面。
pub(crate) fn new_of_expected_type(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let db = ctx.db;
    let Some(expected) = ctx.expected_ty.as_ref().and_then(Ty::as_class) else {
        return;
    };
    let Some(data) = class_data(db, expected) else {
        return;
    };
    if &*data.binary_name == "java/lang/Object" {
        return;
    }
    let boost = score::TYPE_IN_SCOPE + score::EXPECTED_TYPE;
    let mut candidates: Vec<ClassId> = Vec::new();
    let concrete = data.kind == ClassKind::Class && !data.flags.is_abstract();
    if concrete {
        candidates.push(expected);
    }
    if let Some((_, impls)) = KNOWN_IMPLEMENTATIONS
        .iter()
        .find(|(iface, _)| *iface == &*data.binary_name)
    {
        for name in *impls {
            if let Some(class) = hir::lookup_class(db, name) {
                candidates.push(class);
            }
        }
    }
    // 工作区里的实现类（只看源码，不扫描依赖库）
    {
        let mut found = 0;
        for class in hir::source_subclasses(db, expected) {
            if found >= 20 {
                break;
            }
            let Some(sub) = class_data(db, class) else {
                continue;
            };
            // 局部类、匿名类和抽象类不能直接 new
            if sub.kind != ClassKind::Class
                || sub.flags.is_abstract()
                || sub.binary_name.contains("$1")
                || sub.flags.contains(hir::Modifiers::LOCAL_CLASS)
            {
                continue;
            }
            candidates.push(class);
            found += 1;
        }
    }
    for (rank, class) in candidates.into_iter().enumerate() {
        // 先给出的排在前面
        acc.add_known_class(
            ctx,
            class,
            boost - rank.min(10) as i32,
            TypeLocation::New,
            true,
        );
    }
    // 接口 / 抽象类本身（匿名类）排在实现类后面
    if !concrete {
        acc.add_known_class(ctx, expected, boost - 11, TypeLocation::New, true);
    }
}

/// 由类型起一个 lambda 形参名：`String` → `s`，`Map.Entry<K, V>` → `entry`。
fn lambda_param_name(db: &dyn base_db::Db, ty: &Ty, taken: &[String]) -> String {
    let base = match ty {
        Ty::Primitive(_) => "v".to_owned(),
        Ty::Class(class, _) => {
            let name = class_data(db, *class)
                .map(|d| d.name.to_string())
                .unwrap_or_default();
            match name.as_str() {
                "String" => "s".to_owned(),
                "Integer" | "Long" | "Short" | "Byte" => "i".to_owned(),
                "Double" | "Float" => "d".to_owned(),
                "Object" | "" => "o".to_owned(),
                _ => {
                    let mut chars = name.chars();
                    chars
                        .next()
                        .map(|c| c.to_lowercase().chain(chars).collect())
                        .unwrap_or_else(|| "x".to_owned())
                }
            }
        }
        _ => "x".to_owned(),
    };
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}{n}");
        n += 1;
    }
    name
}

/// 期望类型是函数式接口：给出 lambda 片段 `(a, b) -> `。
pub(crate) fn lambda_for_expected(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let db = ctx.db;
    if !ctx.prefix.is_empty() || !ctx.config.snippets {
        return;
    }
    let Some(expected) = &ctx.expected_ty else {
        return;
    };
    let Some(method) = hir::functional_method(db, expected) else {
        return;
    };
    let params = method.param_tys(db);
    // 已经在作用域里的名字不拿来当形参名
    let mut taken: Vec<String> = crate::completions::visible_locals(ctx)
        .into_iter()
        .map(|l| l.name.to_string())
        .collect();
    let mut names: Vec<String> = Vec::new();
    for ty in &params {
        let name = lambda_param_name(db, &ty.wildcard_bound(db), &taken);
        taken.push(name.clone());
        names.push(name);
    }
    let head = match names.as_slice() {
        [one] => one.clone(),
        _ => format!("({})", names.join(", ")),
    };
    let void = method.ret_ty(db) == Ty::Void;
    let (label, body) = if void && params.is_empty() {
        (
            format!("{head} -> {{}}"),
            format!("{head} -> {{\n    $0\n}}"),
        )
    } else {
        (format!("{head} -> "), format!("{head} -> $0"))
    };
    let detail = expected.display(db);
    acc.add_snippet(
        ctx,
        &label,
        &detail,
        &body,
        score::LOCAL + score::EXPECTED_TYPE + 5,
    );
}

/// 期望类型是某个类时，它自己的静态常量：`TimeUnit.SECONDS`、`BigDecimal.ZERO`。
pub(crate) fn constants_of_expected(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let db = ctx.db;
    let Some(expected_ty) = &ctx.expected_ty else {
        return;
    };
    let Some(class) = expected_ty.as_class() else {
        return;
    };
    let Some(data) = class_data(db, class) else {
        return;
    };
    // `String` / 包装类 / `Object` 的常量（`String.CASE_INSENSITIVE_ORDER` 之类）没有意义
    let binary: &str = &data.binary_name;
    if binary == "java/lang/Object"
        || binary == "java/lang/String"
        || expected_ty.unboxed(db).is_some()
    {
        return;
    }
    let is_enum = data.kind == ClassKind::Enum;
    let class_ty = Ty::class(class);
    let mut import = None;
    let qualifier = match hir::resolve_simple_type(db, &ctx.scope, &data.name) {
        Some(hir::TypeNs::Class(found)) if found == class => data.name.to_string(),
        Some(_) => match importable_name(db, class) {
            Some(q) => q,
            None => return,
        },
        None => {
            let Some(q) = importable_name(db, class) else {
                return;
            };
            if ctx.config.auto_import {
                import = Some(insert_import(db, ctx.file, &q));
                data.name.to_string()
            } else {
                q
            }
        }
    };
    let mut added = 0;
    for field in all_fields(db, &class_ty) {
        if added >= 40 {
            break;
        }
        let Some(owner) = class_data(db, field.id.class) else {
            continue;
        };
        let Some(f) = owner.fields.get(field.id.idx as usize) else {
            continue;
        };
        if !f.flags.is_static()
            || !hir::is_accessible(db, f.flags, field.id.class, ctx.enclosing_class)
            || !is_assignable(db, &field.ty, expected_ty)
            || !Completions::matches(ctx, &f.name)
        {
            continue;
        }
        let label = format!("{qualifier}.{}", f.name);
        let mut item = crate::item::CompletionItem {
            label: label.clone(),
            kind: CompletionItemKind::Symbol(if f.is_enum_constant() {
                SymbolKind::EnumConstant
            } else {
                SymbolKind::Field
            }),
            label_detail: None,
            detail: Some(field.ty.display(db)),
            lookup: f.name.to_string(),
            insert_text: label,
            is_snippet: false,
            source_range: ctx.source_range,
            additional_edits: import.clone().into_iter().collect(),
            deprecated: f.flags.is_deprecated(),
            score: score::MEMBER
                + score::EXPECTED_TYPE
                + if is_enum { 5 } else { 0 }
                + if f.flags.is_deprecated() {
                    score::DEPRECATED
                } else {
                    0
                },
            resolve: Some(crate::item::CompletionResolve::Field {
                class: field.id.class.binary_name(db),
                idx: field.id.idx,
            }),
        };
        if !ctx.prefix.is_empty() && f.name.starts_with(&ctx.prefix) {
            item.score += score::EXACT_PREFIX;
        }
        acc.add(item);
        added += 1;
    }
}
