//! 看上下文的代码模板：
//!
//! - `soutv`：打印离光标最近的变量 `System.out.println("x = " + x);`
//! - `soutp`：打印所在方法的全部形参；`soutm`：打印 `类名.方法名`；`souf`：`printf`
//! - `iter`：遍历最近的可迭代变量（数组或 `Iterable`），元素类型和名字都推出来
//! - `itar` / `ritar`：按下标正序 / 倒序遍历最近的数组变量
//! - `thr`：`throw new `
//! - 类体里：`psf` `psfs` `psfi` `prsf` 常量声明

use hir::{class_data, LocalData, LocalKind, Ty, TypeNs};
use syntax::{ast, ast::AstNode, SyntaxKind};

use crate::{
    context::CompletionContext,
    item::{score, Completions},
};

const STATEMENT_LABELS: &[&str] = &[
    "soutv", "soutp", "soutm", "souf", "iter", "itar", "ritar", "thr",
];

/// 片段里的 `$`、`}`、`\` 要转义。
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}

/// 类型里提到的类是否都能用简单名直接写出来。
fn writable(ctx: &CompletionContext<'_>, ty: &Ty) -> bool {
    match ty {
        Ty::Primitive(_) => true,
        Ty::Array(element) => writable(ctx, element),
        Ty::Class(class, args) => {
            let Some(data) = class_data(ctx.db, *class) else {
                return false;
            };
            // 成员类写成 `Outer.Inner`，只检查最外层的名字
            let binary = class.binary_name(ctx.db);
            let tail = binary.rsplit('/').next().unwrap_or(&binary);
            let first = tail.split('$').next().unwrap_or(tail);
            let resolved = match hir::resolve_simple_type(ctx.db, &ctx.scope, first) {
                Some(TypeNs::Class(found)) => {
                    found.binary_name(ctx.db).rsplit('/').next() == Some(first)
                        && (data.outer.is_some() || found == *class)
                }
                _ => false,
            };
            resolved && args.iter().all(|a| writable(ctx, a))
        }
        _ => false,
    }
}

/// 写在变量声明里的类型：能用简单名写出来时写类型，否则用 `var`。
fn type_text(ctx: &CompletionContext<'_>, ty: &Ty) -> String {
    if writable(ctx, ty) {
        ty.display(ctx.db)
    } else {
        "var".to_owned()
    }
}

/// 可迭代变量的元素类型。
fn element_type(ctx: &CompletionContext<'_>, ty: &Ty) -> Option<Ty> {
    if let Ty::Array(element) = ty {
        return Some((**element).clone());
    }
    let iterable = hir::lookup_class(ctx.db, "java/lang/Iterable")?;
    match hir::find_super(ctx.db, ty, iterable)? {
        Ty::Class(_, args) => Some(
            args.first()
                .map(|a| a.wildcard_bound(ctx.db))
                .unwrap_or_else(|| {
                    Ty::class(hir::lookup_class(ctx.db, "java/lang/Object").unwrap_or(iterable))
                }),
        ),
        _ => None,
    }
}

/// 元素变量名：`names` → `name`，`entries` → `entry`，否则由元素类型起名。
fn element_name(
    ctx: &CompletionContext<'_>,
    collection: &str,
    element: &Ty,
    taken: &[String],
) -> String {
    let base = if let Some(stem) = collection.strip_suffix("ies").filter(|s| !s.is_empty()) {
        format!("{stem}y")
    } else if let Some(stem) = collection
        .strip_suffix('s')
        .filter(|s| !s.is_empty() && !s.ends_with('s'))
    {
        stem.to_owned()
    } else {
        match element {
            Ty::Primitive(p) => p.name().chars().next().unwrap_or('x').to_string(),
            Ty::Class(class, _) => {
                let name = class_data(ctx.db, *class)
                    .map(|d| d.name.to_string())
                    .unwrap_or_default();
                let mut chars = name.chars();
                match chars.next() {
                    Some(c) => c.to_lowercase().chain(chars).collect(),
                    None => "item".to_owned(),
                }
            }
            _ => "item".to_owned(),
        }
    };
    let keywordish = matches!(
        base.as_str(),
        "int"
            | "char"
            | "byte"
            | "long"
            | "short"
            | "double"
            | "float"
            | "boolean"
            | "class"
            | "new"
    );
    let base = if keywordish {
        format!("{base}Value")
    } else {
        base
    };
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name) || name == collection {
        name = format!("{base}{n}");
        n += 1;
    }
    name
}

fn is_iterable(ctx: &CompletionContext<'_>, local: &LocalData) -> bool {
    !matches!(
        local.kind,
        LocalKind::LocalClassField | LocalKind::LocalClassMethod
    ) && element_type(ctx, &local.ty).is_some()
}

pub(crate) fn statement_templates(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    if !ctx.config.snippets
        || !STATEMENT_LABELS
            .iter()
            .any(|l| Completions::matches(ctx, l))
    {
        return;
    }
    let db = ctx.db;
    // 离光标最近的在前
    let locals: Vec<LocalData> = crate::completions::visible_locals(ctx)
        .into_iter()
        .filter(|l| {
            !matches!(
                l.kind,
                LocalKind::LocalClassField | LocalKind::LocalClassMethod
            )
        })
        .collect();
    let taken: Vec<String> = locals.iter().map(|l| l.name.to_string()).collect();
    let add = |acc: &mut Completions, label: &str, detail: &str, body: String| {
        acc.add_snippet(ctx, label, detail, &body, score::SNIPPET);
    };

    if let Some(nearest) = locals.first() {
        let name = esc(&nearest.name);
        add(
            acc,
            "soutv",
            &format!("System.out.println(\"{0} = \" + {0})", nearest.name),
            format!("System.out.println(\"{name} = \" + {name});$0"),
        );
    }

    // 所在方法的形参（按声明顺序）
    let method = ctx
        .original_root
        .token_at_offset(ctx.offset)
        .left_biased()
        .and_then(|t| t.parent_ancestors().find_map(ast::MethodDecl::cast));
    if let Some(method) = &method {
        let params_range = method.param_list().map(|p| p.syntax().text_range());
        let mut params: Vec<&LocalData> = locals
            .iter()
            .filter(|l| {
                l.kind == LocalKind::Param
                    && params_range.is_some_and(|r| r.contains_range(l.decl_range))
            })
            .collect();
        params.sort_by_key(|l| l.decl_range.start());
        if !params.is_empty() {
            let parts: Vec<String> = params
                .iter()
                .map(|p| format!("\"{0} = \" + {0}", esc(&p.name)))
                .collect();
            add(
                acc,
                "soutp",
                "System.out.println(\"a = \" + a + \", b = \" + b)",
                format!("System.out.println({});$0", parts.join(" + \", \" + ")),
            );
        }
        let class_name = ctx
            .enclosing_class
            .and_then(|c| class_data(db, c))
            .map(|d| d.name.to_string())
            .unwrap_or_default();
        if let Some(name) = method.name() {
            let text = format!("{class_name}.{}", name.text());
            add(
                acc,
                "soutm",
                &format!("System.out.println(\"{text}\")"),
                format!("System.out.println(\"{}\");$0", esc(&text)),
            );
        }
    }
    add(
        acc,
        "souf",
        "System.out.printf(\"\")",
        "System.out.printf(\"$1%n\"$2);$0".to_owned(),
    );
    add(acc, "thr", "throw new", "throw new $0".to_owned());

    // 遍历最近的可迭代变量 / 数组变量
    if let Some(collection) = locals.iter().find(|l| is_iterable(ctx, l)) {
        if let Some(element) = element_type(ctx, &collection.ty) {
            let elem_name = element_name(ctx, &collection.name, &element, &taken);
            let ty = type_text(ctx, &element);
            add(
                acc,
                "iter",
                &format!("for ({ty} {elem_name} : {})", collection.name),
                format!(
                    "for (${{1:{}}} ${{2:{}}} : {}) {{\n    $0\n}}",
                    esc(&ty),
                    esc(&elem_name),
                    esc(&collection.name)
                ),
            );
        }
    }
    if let Some(array) = locals.iter().find(|l| matches!(l.ty, Ty::Array(_))) {
        if let Ty::Array(element) = &array.ty {
            let elem_name = element_name(ctx, &array.name, element, &taken);
            let ty = esc(&type_text(ctx, element));
            let arr = esc(&array.name);
            let elem = esc(&elem_name);
            let index = if taken.iter().any(|t| t == "i") {
                "j"
            } else {
                "i"
            };
            add(
                acc,
                "itar",
                &format!("for (int {index} = 0; {index} < {}.length; {index}++)", array.name),
                format!(
                    "for (int ${{1:{index}}} = 0; $1 < {arr}.length; $1++) {{\n    {ty} ${{2:{elem}}} = {arr}[$1];\n    $0\n}}"
                ),
            );
            add(
                acc,
                "ritar",
                &format!("for (int {index} = {}.length - 1; {index} >= 0; {index}--)", array.name),
                format!(
                    "for (int ${{1:{index}}} = {arr}.length - 1; $1 >= 0; $1--) {{\n    {ty} ${{2:{elem}}} = {arr}[$1];\n    $0\n}}"
                ),
            );
        }
    }
}

/// 类体里的常量声明模板。
pub(crate) fn member_templates(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    // 已经写了修饰符时不提供
    if ctx.prev_token.is_some_and(|kind| {
        !matches!(
            kind,
            SyntaxKind::L_CURLY | SyntaxKind::R_CURLY | SyntaxKind::SEMICOLON
        )
    }) {
        return;
    }
    let templates: &[(&str, &str, &str)] = &[
        ("psf", "public static final", "public static final $0"),
        (
            "psfs",
            "public static final String",
            "public static final String ${1:NAME} = \"$2\";$0",
        ),
        (
            "psfi",
            "public static final int",
            "public static final int ${1:NAME} = $2;$0",
        ),
        ("prsf", "private static final", "private static final $0"),
    ];
    for (label, detail, body) in templates {
        acc.add_snippet(ctx, label, detail, body, score::SNIPPET);
    }
}
