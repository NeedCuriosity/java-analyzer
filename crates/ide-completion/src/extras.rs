//! 不来自符号的候选：关键字、片段、postfix 模板、覆写方法生成。

use hir::{
    all_methods, class_data, is_accessible, this_ty, GenericOwner, Modifiers, PrimitiveType, Ty,
    TypeParamId, Visibility,
};
use ide_db::{render, SymbolKind, TextEdit};
use syntax::{SyntaxKind, TextRange};

use crate::{
    context::{CompletionContext, TypeLocation},
    item::{score, CompletionItem, CompletionItemKind, Completions},
};

const PRIMITIVES: &[&str] = &[
    "boolean", "byte", "char", "short", "int", "long", "float", "double",
];

const EXPR_KEYWORDS: &[&str] = &["new", "this", "super", "null", "true", "false", "switch"];

const STMT_KEYWORDS: &[&str] = &[
    "if",
    "for",
    "while",
    "do",
    "switch",
    "try",
    "return",
    "throw",
    "break",
    "continue",
    "synchronized",
    "assert",
    "final",
    "var",
    "yield",
    "class",
    "record",
    "interface",
    "enum",
];

const MEMBER_KEYWORDS: &[&str] = &[
    "public",
    "protected",
    "private",
    "static",
    "final",
    "abstract",
    "synchronized",
    "native",
    "transient",
    "volatile",
    "default",
    "strictfp",
    "sealed",
    "non-sealed",
    "class",
    "interface",
    "enum",
    "record",
    "void",
];

const TOP_LEVEL_KEYWORDS: &[&str] = &[
    "package",
    "import",
    "public",
    "abstract",
    "final",
    "sealed",
    "non-sealed",
    "strictfp",
    "class",
    "interface",
    "enum",
    "record",
];

pub(crate) fn expr_keywords(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    statement_start: bool,
) {
    for kw in EXPR_KEYWORDS {
        acc.add_keyword(ctx, kw);
    }
    if !statement_start {
        return;
    }
    for kw in STMT_KEYWORDS.iter().chain(PRIMITIVES) {
        acc.add_keyword(ctx, kw);
    }
    if ctx.prev_token == Some(SyntaxKind::R_CURLY) {
        for kw in ["else", "catch", "finally"] {
            acc.add_keyword(ctx, kw);
        }
    }
}

pub(crate) fn type_keywords(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    location: TypeLocation,
) {
    match location {
        TypeLocation::Member => {
            for kw in MEMBER_KEYWORDS.iter().chain(PRIMITIVES) {
                acc.add_keyword(ctx, kw);
            }
        }
        TypeLocation::TopLevel => {
            for kw in TOP_LEVEL_KEYWORDS {
                acc.add_keyword(ctx, kw);
            }
        }
        TypeLocation::Other => {
            for kw in PRIMITIVES.iter().chain(&["final", "var", "void"]) {
                acc.add_keyword(ctx, kw);
            }
        }
        TypeLocation::New => {
            for kw in PRIMITIVES {
                acc.add_keyword(ctx, kw);
            }
        }
        TypeLocation::Annotation => {
            acc.add_keyword(ctx, "interface");
        }
    }
}

pub(crate) fn statement_snippets(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let snippets: &[(&str, &str, &str)] = &[
        ("sout", "System.out.println()", "System.out.println($0);"),
        ("serr", "System.err.println()", "System.err.println($0);"),
        (
            "fori",
            "for (int i = 0; i < n; i++)",
            "for (int ${1:i} = 0; $1 < ${2:n}; $1++) {\n    $0\n}",
        ),
        (
            "foreach",
            "for (var item : items)",
            "for (${1:var} ${2:item} : ${3:items}) {\n    $0\n}",
        ),
        (
            "ifn",
            "if (x == null)",
            "if (${1:value} == null) {\n    $0\n}",
        ),
        (
            "inn",
            "if (x != null)",
            "if (${1:value} != null) {\n    $0\n}",
        ),
        (
            "trycatch",
            "try { } catch (Exception e) { }",
            "try {\n    $0\n} catch (${1:Exception} ${2:e}) {\n    \n}",
        ),
    ];
    for (label, detail, body) in snippets {
        acc.add_snippet(ctx, label, detail, body, score::SNIPPET);
    }
}

pub(crate) fn member_snippets(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    for label in ["psvm", "main"] {
        acc.add_snippet(
            ctx,
            label,
            "public static void main(String[] args)",
            "public static void main(String[] args) {\n    $0\n}",
            score::SNIPPET,
        );
    }
}

fn is_iterable(ctx: &CompletionContext<'_>, ty: &Ty) -> bool {
    if matches!(ty, Ty::Array(_)) {
        return true;
    }
    let Some(iterable) = hir::lookup_class(ctx.db, "java/lang/Iterable") else {
        return false;
    };
    hir::find_super(ctx.db, ty, iterable).is_some()
}

/// `expr.var`、`expr.if` 之类的后缀模板。
pub(crate) fn postfix_templates(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    receiver_range: TextRange,
    receiver_ty: Option<&Ty>,
    statement_level: bool,
) {
    if !ctx.config.postfix || !ctx.config.snippets {
        return;
    }
    let text: &str = ctx.file.text(ctx.db);
    let Some(receiver) =
        text.get(usize::from(receiver_range.start())..usize::from(receiver_range.end()))
    else {
        return;
    };
    // 主编辑必须是单行的；跨行的接收者不提供模板
    if receiver.contains('\n') || receiver_range.end() > ctx.source_range.start() {
        return;
    }
    let delete_receiver = TextEdit::replace(
        TextRange::new(receiver_range.start(), ctx.source_range.start()),
        String::new(),
    );

    let unknown = receiver_ty.is_none_or(|ty| ty.is_unknown());
    let is_boolean = receiver_ty
        .is_some_and(|ty| *ty == Ty::BOOLEAN || ty.unboxed(ctx.db) == Some(PrimitiveType::Boolean));
    let is_reference = receiver_ty.is_some_and(|ty| ty.is_reference());
    let is_int = receiver_ty.is_some_and(|ty| *ty == Ty::INT);
    let iterable = receiver_ty.is_some_and(|ty| is_iterable(ctx, ty));
    let is_void = receiver_ty == Some(&Ty::Void);
    let is_throwable = receiver_ty.is_some_and(|ty| {
        hir::lookup_class(ctx.db, "java/lang/Throwable")
            .is_some_and(|t| hir::find_super(ctx.db, ty, t).is_some())
    });

    let mut add = |label: &str, detail: &str, body: String| {
        if let Some(item) = acc.add_snippet(ctx, label, detail, &body, score::POSTFIX) {
            item.additional_edits.push(delete_receiver.clone());
        }
    };

    if !is_void {
        add("not", "!expr", format!("!{receiver}"));
        add("par", "(expr)", format!("({receiver})"));
        add(
            "cast",
            "((Type) expr)",
            format!("((${{1:Type}}) {receiver})$0"),
        );
    }
    if !statement_level {
        return;
    }
    if !is_void {
        add(
            "var",
            "var name = expr;",
            format!("var ${{1:name}} = {receiver};$0"),
        );
        add("return", "return expr;", format!("return {receiver};"));
        add(
            "sout",
            "System.out.println(expr);",
            format!("System.out.println({receiver});"),
        );
    }
    if is_boolean || unknown {
        add(
            "if",
            "if (expr) { }",
            format!("if ({receiver}) {{\n    $0\n}}"),
        );
        add(
            "while",
            "while (expr) { }",
            format!("while ({receiver}) {{\n    $0\n}}"),
        );
    }
    if is_reference || unknown {
        add(
            "nn",
            "if (expr != null) { }",
            format!("if ({receiver} != null) {{\n    $0\n}}"),
        );
        add(
            "null",
            "if (expr == null) { }",
            format!("if ({receiver} == null) {{\n    $0\n}}"),
        );
    }
    if iterable || unknown {
        add(
            "for",
            "for (var item : expr) { }",
            format!("for (var ${{1:item}} : {receiver}) {{\n    $0\n}}"),
        );
    }
    if is_int || unknown {
        add(
            "fori",
            "for (int i = 0; i < expr; i++) { }",
            format!("for (int ${{1:i}} = 0; $1 < {receiver}; $1++) {{\n    $0\n}}"),
        );
    }
    if is_throwable {
        add("throw", "throw expr;", format!("throw {receiver};"));
    }
}

/// 在类体里输入方法名时，生成覆写 / 实现父类型方法的整段代码。
pub(crate) fn override_methods(acc: &mut Completions, ctx: &CompletionContext<'_>) {
    let db = ctx.db;
    let Some(class) = ctx.enclosing_class else {
        return;
    };
    // 已经写了修饰符或返回类型时不再生成整段声明
    if ctx.prev_token.is_some_and(|kind| {
        !matches!(
            kind,
            SyntaxKind::L_CURLY | SyntaxKind::R_CURLY | SyntaxKind::SEMICOLON
        )
    }) {
        return;
    }
    let Some(data) = class_data(db, class) else {
        return;
    };
    let declared: Vec<(&str, usize)> = data
        .methods
        .iter()
        .map(|m| (m.name.as_str(), m.params.len()))
        .collect();

    for candidate in all_methods(db, &this_ty(db, class)) {
        if candidate.id.class == class {
            continue;
        }
        let Some(owner) = class_data(db, candidate.id.class) else {
            continue;
        };
        let Some(method) = owner.methods.get(candidate.id.idx as usize) else {
            continue;
        };
        let flags = method.flags;
        if method.is_constructor
            || flags.is_static()
            || flags.is_final()
            || flags.contains(Modifiers::SYNTHETIC)
            || flags.visibility() == Visibility::Private
            || !is_accessible(db, flags, candidate.id.class, Some(class))
            || declared.contains(&(method.name.as_str(), method.params.len()))
            || !Completions::matches(ctx, &method.name)
        {
            continue;
        }

        let params = render::method_params(db, candidate.id, &candidate.subst);
        let ret = candidate.ret_ty(db);
        let visibility = if flags.visibility() == Visibility::Public || owner.is_interface() {
            "public "
        } else if flags.visibility() == Visibility::Protected {
            "protected "
        } else {
            ""
        };
        let type_params = if method.type_params.is_empty() {
            String::new()
        } else {
            let decls: Vec<String> = (0..method.type_params.len())
                .map(|idx| {
                    render::type_param_decl(
                        db,
                        TypeParamId {
                            owner: GenericOwner::Method(candidate.id),
                            idx: idx as u32,
                        },
                    )
                })
                .collect();
            format!("<{}> ", decls.join(", "))
        };
        let sig = hir::method_sig(db, candidate.id);
        let throws = if sig.throws.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = sig
                .throws
                .iter()
                .map(|t| t.subst(&candidate.subst).display(db))
                .collect();
            format!(" throws {}", names.join(", "))
        };

        let args: Vec<String> = method
            .params
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                p.name
                    .as_ref()
                    .map_or_else(|| format!("arg{idx}"), |n| n.to_string())
            })
            .collect();
        let body = if flags.is_abstract() {
            "$0".to_owned()
        } else {
            let receiver = if owner.is_interface() {
                format!("{}.super", owner.name)
            } else {
                "super".to_owned()
            };
            let call = format!("{receiver}.{}({});", method.name, args.join(", "));
            if ret == Ty::Void {
                format!("{call}$0")
            } else {
                format!("return {call}$0")
            }
        };

        let indent = &ctx.indent;
        let text = format!(
            "@Override\n{indent}{visibility}{type_params}{} {}({}){throws} {{\n{indent}    {body}\n{indent}}}",
            ret.display(db),
            method.name,
            params.join(", "),
        );
        let (insert_text, is_snippet) = if ctx.config.snippets {
            (text, true)
        } else {
            (text.replace("$0", ""), false)
        };
        acc.add(CompletionItem {
            label: format!("{}({})", method.name, params.join(", ")),
            kind: CompletionItemKind::Symbol(SymbolKind::Method),
            label_detail: None,
            detail: Some(format!("覆写 {}.{}", owner.name, method.name)),
            lookup: method.name.to_string(),
            insert_text,
            is_snippet,
            source_range: ctx.source_range,
            additional_edits: Vec::new(),
            deprecated: flags.is_deprecated(),
            score: score::OVERRIDE + if flags.is_abstract() { 10 } else { 0 },
            resolve: None,
        });
    }
}

const JAVADOC_BLOCK_TAGS: &[(&str, &str)] = &[
    ("param", "形参或类型参数的说明"),
    ("return", "返回值的说明"),
    ("throws", "可能抛出的异常"),
    ("see", "参见"),
    ("since", "从哪个版本开始提供"),
    ("deprecated", "已弃用及替代方案"),
    ("author", "作者"),
    ("version", "版本"),
    ("apiNote", "API 使用说明"),
    ("implSpec", "实现规约"),
    ("implNote", "实现说明"),
    ("serial", "序列化字段说明"),
    ("hidden", "不出现在生成的文档里"),
];

const JAVADOC_INLINE_TAGS: &[(&str, &str)] = &[
    ("code", "等宽的代码文本"),
    ("link", "指向类或成员的链接"),
    ("linkplain", "普通字体的链接"),
    ("literal", "不解释 HTML 的文本"),
    ("value", "常量的值"),
    ("inheritDoc", "继承父类型的文档"),
    ("snippet", "代码片段"),
    ("summary", "摘要"),
    ("index", "索引项"),
    ("systemProperty", "系统属性"),
];

pub(crate) fn javadoc_tags(acc: &mut Completions, ctx: &CompletionContext<'_>, inline: bool) {
    let tags = if inline {
        JAVADOC_INLINE_TAGS
    } else {
        JAVADOC_BLOCK_TAGS
    };
    for (rank, (tag, detail)) in tags.iter().enumerate() {
        if !Completions::matches(ctx, tag) {
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

pub(crate) fn javadoc_params(
    acc: &mut Completions,
    ctx: &CompletionContext<'_>,
    names: &[smol_str::SmolStr],
) {
    let text: &str = ctx.file.text(ctx.db);
    for (rank, name) in names.iter().enumerate() {
        if !Completions::matches(ctx, name.trim_start_matches('<')) {
            continue;
        }
        // 已经写过说明的形参排到后面
        let documented = text.contains(&format!("@param {name}"));
        acc.add(CompletionItem {
            label: name.to_string(),
            kind: CompletionItemKind::Symbol(SymbolKind::Param),
            label_detail: None,
            detail: None,
            lookup: name.trim_start_matches('<').to_owned(),
            insert_text: name.to_string(),
            is_snippet: false,
            source_range: ctx.source_range,
            additional_edits: Vec::new(),
            deprecated: false,
            // 间隔取 10，保证声明顺序不会被使用频度的加成打乱
            score: score::LOCAL - rank as i32 * 10 - if documented { 200 } else { 0 },
            resolve: None,
        });
    }
}
