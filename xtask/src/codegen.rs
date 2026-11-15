//! AST 节点声明的代码生成。
//!
//! 输入 `crates/syntax/ast.def`，每行一条声明：
//!
//! ```text
//! node ClassDecl CLASS_DECL
//! enum TypeDecl = ClassDecl | InterfaceDecl
//! ```
//!
//! 输出 `crates/syntax/src/ast/generated.rs`：对 `nodes.rs` 里 `ast_node!` / `ast_enum!`
//! 两个宏的调用。访问器（`fn name()` 之类）不生成，仍然手写在 `nodes.rs` 里。

use std::{collections::HashSet, fs, path::Path};

use anyhow::{bail, Context};

const DEF_PATH: &str = "crates/syntax/ast.def";
const OUT_PATH: &str = "crates/syntax/src/ast/generated.rs";
const KINDS_PATH: &str = "crates/parser/src/syntax_kind.rs";

/// 与 rustfmt 的默认行宽一致：超过就把枚举拆成一行一个变体。
const MAX_WIDTH: usize = 100;

#[derive(Debug, PartialEq, Eq)]
enum Decl {
    Node { name: String, kind: String },
    Enum { name: String, variants: Vec<String> },
}

fn is_ident(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_def(text: &str) -> anyhow::Result<Vec<Decl>> {
    let mut decls = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let lineno = idx + 1;
        if let Some(rest) = line.strip_prefix("node ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            match parts.as_slice() {
                [name, kind] if is_ident(name) && is_ident(kind) => {
                    decls.push(Decl::Node {
                        name: (*name).to_owned(),
                        kind: (*kind).to_owned(),
                    });
                }
                _ => bail!("{DEF_PATH}:{lineno}: 应为 `node <名字> <SyntaxKind>`"),
            }
        } else if let Some(rest) = line.strip_prefix("enum ") {
            let Some((name, variants)) = rest.split_once('=') else {
                bail!("{DEF_PATH}:{lineno}: 应为 `enum <名字> = <A> | <B>`");
            };
            let name = name.trim();
            let variants: Vec<String> = variants.split('|').map(|v| v.trim().to_owned()).collect();
            if !is_ident(name) || variants.iter().any(|v| !is_ident(v)) {
                bail!("{DEF_PATH}:{lineno}: 枚举名或变体名不合法");
            }
            decls.push(Decl::Enum {
                name: name.to_owned(),
                variants,
            });
        } else {
            bail!("{DEF_PATH}:{lineno}: 无法识别的声明 `{line}`");
        }
    }
    Ok(decls)
}

/// 一致性检查：名字不重复；枚举的变体都是已声明的节点；节点的种类都在 `SyntaxKind` 里。
fn validate(decls: &[Decl], kinds_source: &str) -> anyhow::Result<()> {
    let known_kinds: HashSet<&str> = kinds_source
        .lines()
        .filter_map(|line| line.trim().strip_suffix(','))
        .filter(|name| is_ident(name) && name.chars().all(|c| !c.is_ascii_lowercase()))
        .collect();

    let mut names = HashSet::new();
    let mut nodes = HashSet::new();
    for decl in decls {
        let name = match decl {
            Decl::Node { name, kind } => {
                if !known_kinds.contains(kind.as_str()) {
                    bail!("节点 `{name}` 引用了不存在的 SyntaxKind `{kind}`");
                }
                nodes.insert(name.as_str());
                name
            }
            Decl::Enum { name, .. } => name,
        };
        if !names.insert(name.as_str()) {
            bail!("`{name}` 重复声明");
        }
    }
    for decl in decls {
        if let Decl::Enum { name, variants } = decl {
            let mut seen = HashSet::new();
            for variant in variants {
                if !nodes.contains(variant.as_str()) {
                    bail!("枚举 `{name}` 的变体 `{variant}` 不是已声明的节点");
                }
                if !seen.insert(variant.as_str()) {
                    bail!("枚举 `{name}` 的变体 `{variant}` 重复");
                }
            }
        }
    }
    Ok(())
}

fn generate(decls: &[Decl]) -> String {
    let mut out = String::new();
    out.push_str(
        "// 由 `cargo xtask codegen` 根据 `crates/syntax/ast.def` 生成，请勿手工修改。\n\n",
    );
    for decl in decls {
        match decl {
            Decl::Node { name, kind } => {
                out.push_str(&format!("ast_node!({name}, {kind});\n"));
            }
            Decl::Enum { name, variants } => {
                let one_line = format!("ast_enum!({name} {{ {} }});", variants.join(", "));
                if one_line.chars().count() <= MAX_WIDTH {
                    out.push_str(&one_line);
                    out.push('\n');
                } else {
                    out.push_str(&format!("ast_enum!({name} {{\n"));
                    for variant in variants {
                        out.push_str(&format!("    {variant},\n"));
                    }
                    out.push_str("});\n");
                }
            }
        }
    }
    out
}

/// 生成（或在 `check` 模式下只校验）AST 声明文件。
pub fn run(root: &Path, check: bool) -> anyhow::Result<()> {
    let def_path = root.join(DEF_PATH);
    let out_path = root.join(OUT_PATH);
    let def =
        fs::read_to_string(&def_path).with_context(|| format!("读取 {}", def_path.display()))?;
    let kinds =
        fs::read_to_string(root.join(KINDS_PATH)).with_context(|| format!("读取 {KINDS_PATH}"))?;

    let decls = parse_def(&def)?;
    validate(&decls, &kinds)?;
    let generated = generate(&decls);

    let existing = fs::read_to_string(&out_path).unwrap_or_default();
    if existing == generated {
        println!("{OUT_PATH} 已是最新");
        return Ok(());
    }
    if check {
        bail!("{OUT_PATH} 与 {DEF_PATH} 不一致，请运行 `cargo xtask codegen`");
    }
    fs::write(&out_path, generated).with_context(|| format!("写入 {}", out_path.display()))?;
    println!("已更新 {OUT_PATH}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: &str = "pub enum SyntaxKind {\n    CLASS_DECL,\n    ENUM_DECL,\n    BLOCK,\n}\n";

    #[test]
    fn parses_and_generates() {
        let decls = parse_def(
            "# 注释\nnode ClassDecl CLASS_DECL\nnode EnumDecl ENUM_DECL  # 行尾注释\n\nenum TypeDecl = ClassDecl | EnumDecl\n",
        )
        .unwrap();
        assert_eq!(decls.len(), 3);
        validate(&decls, KINDS).unwrap();
        assert_eq!(
            generate(&decls).lines().skip(2).collect::<Vec<_>>(),
            [
                "ast_node!(ClassDecl, CLASS_DECL);",
                "ast_node!(EnumDecl, ENUM_DECL);",
                "ast_enum!(TypeDecl { ClassDecl, EnumDecl });",
            ]
        );
    }

    #[test]
    fn long_enums_are_wrapped() {
        let variants: Vec<String> = (0..12).map(|i| format!("SomeLongNodeName{i}")).collect();
        let text = generate(&[Decl::Enum {
            name: "Big".to_owned(),
            variants,
        }]);
        assert!(text.contains("ast_enum!(Big {\n    SomeLongNodeName0,\n"));
        assert!(text.ends_with("    SomeLongNodeName11,\n});\n"));
    }

    #[test]
    fn rejects_inconsistent_definitions() {
        assert!(parse_def("struct Foo").is_err());
        assert!(parse_def("node Foo").is_err());

        let unknown_kind = parse_def("node Foo MISSING_KIND").unwrap();
        assert!(validate(&unknown_kind, KINDS).is_err());

        let unknown_variant = parse_def("node A BLOCK\nenum E = A | B").unwrap();
        assert!(validate(&unknown_variant, KINDS).is_err());

        let duplicate = parse_def("node A BLOCK\nnode A CLASS_DECL").unwrap();
        assert!(validate(&duplicate, KINDS).is_err());
    }

    /// 仓库里提交的生成文件必须与规格一致。
    #[test]
    fn checked_in_file_is_up_to_date() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        run(root, true).unwrap();
    }
}
