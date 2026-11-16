//! 提取接口：把顶层类的公开实例方法提到一个新接口里（同一个包、同一个目录），
//! 类加上 `implements`，被提取的方法标上 `@Override`。
//!
//! 接口名取类名去掉 `Impl` 后缀，没有这个后缀时在前面加 `I`；同名的类已经存在时不提供。
//! 类的类型参数原样带到接口上。已经标了 `@Override` 的方法属于别的契约，不提取。

use hir::{class_data, ClassKind};
use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind,
};

use super::{indent_in, Assist, AssistCtx, AssistKind};
use crate::SourceChange;

fn annotation_name(annotation: &ast::Annotation) -> Option<String> {
    annotation.name()?.segments().last().map(|s| s.to_string())
}

fn interface_name(class_name: &str) -> String {
    match class_name.strip_suffix("Impl") {
        Some(base) if !base.is_empty() => base.to_owned(),
        _ => format!("I{class_name}"),
    }
}

/// 一个被提取的方法：接口里的声明各行，以及要插 `@Override` 的位置和缩进。
struct Extracted {
    lines: Vec<String>,
    override_at: syntax::TextSize,
    indent: String,
}

fn extract(text: &str, method: &ast::MethodDecl) -> Option<Extracted> {
    let mods = method.modifier_list()?;
    if !mods.has(SyntaxKind::PUBLIC_KW) || mods.has(SyntaxKind::STATIC_KW) {
        return None;
    }
    let annotations: Vec<ast::Annotation> = mods.annotations().collect();
    if annotations
        .iter()
        .any(|a| annotation_name(a).as_deref() == Some("Override"))
    {
        return None;
    }
    let name = method.name()?;
    let ret = method.return_type()?;
    let params = method.param_list()?;
    let override_at = mods.syntax().text_range().start();
    let indent = indent_in(text, override_at);

    let mut lines = Vec::new();
    if let Some(doc) = method.doc_comment() {
        // 续行去掉原来的缩进，星号和 `/**` 对齐
        for (i, line) in doc.text().lines().enumerate() {
            let line = line.trim_start();
            lines.push(if i > 0 && line.starts_with('*') {
                format!(" {line}")
            } else {
                line.to_owned()
            });
        }
    }
    // 留下注解（除了 `@Override` / `@SuppressWarnings` 这种只对实现有意义的）
    let kept: Vec<String> = annotations
        .iter()
        .filter(|a| {
            !matches!(
                annotation_name(a).as_deref(),
                Some("Override" | "SuppressWarnings")
            )
        })
        .map(|a| a.syntax().text().to_string())
        .collect();
    let mut signature = String::new();
    for annotation in &kept {
        signature.push_str(annotation);
        signature.push(' ');
    }
    if let Some(type_params) = method.type_param_list() {
        signature.push_str(&type_params.syntax().text().to_string());
        signature.push(' ');
    }
    signature.push_str(&format!(
        "{}{} {}{}",
        ret.syntax().text(),
        "[]".repeat(method.extra_dims()),
        name.text(),
        params.syntax().text()
    ));
    if let Some(throws) = method.throws_clause() {
        signature.push(' ');
        signature.push_str(&throws.syntax().text().to_string());
    }
    signature.push(';');
    lines.push(signature);
    Some(Extracted {
        lines,
        override_at,
        indent,
    })
}

/// 标识符 `name` 是否作为一个完整的词出现在 `text` 里。
fn mentions(text: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text.match_indices(name).any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + name.len()..].chars().next().is_some_and(is_ident)
    })
}

pub(super) fn extract_interface(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(decl) = token
        .parent()
        .and_then(ast::Name::cast)
        .and_then(|n| n.syntax().parent())
        .and_then(ast::TypeDecl::cast)
    else {
        return;
    };
    // 只处理顶层类型：嵌套类的方法签名可能用到外层类的成员类型
    if decl
        .syntax()
        .parent()
        .is_none_or(|p| p.kind() != SyntaxKind::COMPILATION_UNIT)
    {
        return;
    }
    let Some(class) = ctx.sema.class_of_decl(ctx.file, &decl) else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    if !matches!(
        data.kind,
        ClassKind::Class | ClassKind::Enum | ClassKind::Record
    ) {
        return;
    }
    let Some(body) = decl.body() else {
        return;
    };
    let extracted: Vec<Extracted> = body
        .members()
        .filter_map(|m| match m {
            ast::Member::MethodDecl(method) => extract(ctx.text, &method),
            _ => None,
        })
        .collect();
    if extracted.is_empty() {
        return;
    }

    let name = interface_name(&data.name);
    let package = data
        .binary_name
        .rsplit_once('/')
        .map_or("", |(p, _)| p)
        .to_owned();
    let binary = if package.is_empty() {
        name.clone()
    } else {
        format!("{package}/{name}")
    };
    if hir::lookup_class(ctx.db, &binary).is_some() {
        return;
    }

    let type_params = decl
        .type_param_list()
        .map(|t| t.syntax().text().to_string())
        .unwrap_or_default();
    let type_args = decl
        .type_param_list()
        .map(|list| {
            let names: Vec<String> = list
                .type_params()
                .filter_map(|p| p.name().map(|n| n.text().to_string()))
                .collect();
            format!("<{}>", names.join(", "))
        })
        .unwrap_or_default();

    let mut members_text = Vec::new();
    for item in &extracted {
        let lines: Vec<String> = item.lines.iter().map(|l| format!("    {l}")).collect();
        members_text.push(lines.join("\n"));
    }
    let body_text = members_text.join("\n\n");

    // 签名里用到的类型，照搬原文件的 import
    let mut imports = Vec::new();
    if let Some(unit) = ast::CompilationUnit::cast(ctx.root.clone()) {
        for import in unit.imports() {
            if import.is_static() || import.is_module() {
                continue;
            }
            let Some(qualified) = import.name() else {
                continue;
            };
            let qualified = qualified.syntax().text().to_string();
            let used = import.is_on_demand()
                || qualified.rsplit('.').next().is_some_and(|simple| {
                    mentions(&body_text, simple) || mentions(&type_params, simple)
                });
            if used {
                imports.push(import.syntax().text().to_string().trim().to_owned());
            }
        }
    }

    let mut file_text = String::new();
    if !package.is_empty() {
        file_text.push_str(&format!("package {};\n\n", package.replace('/', ".")));
    }
    if !imports.is_empty() {
        for import in &imports {
            file_text.push_str(import);
            file_text.push('\n');
        }
        file_text.push('\n');
    }
    file_text.push_str(&format!(
        "public interface {name}{type_params} {{\n{body_text}\n}}\n"
    ));

    // 类头加上 implements，方法前加 `@Override`
    let mut edits = Vec::new();
    let implemented = format!("{name}{type_args}");
    match decl.implements_clause() {
        Some(clause) => edits.push(TextEdit::insert(
            clause.syntax().text_range().end(),
            format!(", {implemented}"),
        )),
        None => {
            let anchor = decl
                .permits_clause()
                .map(|c| c.syntax().text_range().start())
                .map(|start| (start, format!("implements {implemented} ")))
                .or_else(|| {
                    let end = decl
                        .extends_clause()
                        .map(|c| c.syntax().text_range().end())
                        .or_else(|| {
                            decl.record_components()
                                .map(|c| c.syntax().text_range().end())
                        })
                        .or_else(|| {
                            decl.type_param_list()
                                .map(|c| c.syntax().text_range().end())
                        })
                        .or_else(|| decl.name().map(|n| n.syntax().text_range().end()))?;
                    Some((end, format!(" implements {implemented}")))
                });
            let Some((at, text)) = anchor else {
                return;
            };
            edits.push(TextEdit::insert(at, text));
        }
    }
    for item in &extracted {
        edits.push(TextEdit::insert(
            item.override_at,
            format!("@Override\n{}", item.indent),
        ));
    }
    edits.sort_by_key(|e| e.range.start());

    out.push(Assist {
        id: "extract_interface",
        label: base_db::tr!("提取接口 `{name}`", "Extract interface `{name}`"),
        kind: AssistKind::RefactorExtract,
        change: SourceChange {
            edits: vec![(ctx.file_id, edits)],
            file_renames: Vec::new(),
            file_creates: vec![(ctx.file_id, format!("{name}.java"), file_text)],
        },
    });
}
