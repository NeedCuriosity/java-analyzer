//! 场景文件的格式与解析。
//!
//! 一个场景是一个 `.java` 文件，里面除了 Java 代码还有三种评测专用的记号，
//! 交给分析器之前都会被去掉：
//!
//! - `//- /路径/文件.java`：多文件场景的文件分隔行（和单元测试的 fixture 相同）。
//!   没有分隔行时整个场景是一个文件，文件名取自第一个顶层类型。
//! - `/*@名字*/`：标记一个位置。名字在整个场景里唯一。
//! - `//? 指令 ...`：一条预期，独占一行。
//!
//! 指令（`m`、`a`、`b` 都是标记的名字）：
//!
//! ```text
//! //? def m -> a                 在 m 处跳转到定义，落在标记 a 上
//! //? def m -> lib java.util.List#add   跳到依赖库 / JDK 里的类（`#成员` 可省略）
//! //? def m -> none              没有定义可跳
//! //? typedef m -> a | lib ...   跳转到类型定义
//! //? impl m -> a b              跳转到实现，恰好是这些位置
//! //? refs m = a b c             查找引用（不含声明），恰好是这些位置；`refs m =` 表示没有引用
//! //? hover m ~ "文本"           悬停的签名里包含这段文本
//! //? complete m has a b         补全候选里有这些标签
//! //? complete m lacks a b       补全候选里没有这些标签
//! //? complete m first a         排在第一位的候选
//! //? sig m ~ "文本" active N    签名帮助：当前签名包含文本，活动形参下标为 N
//! //? diag none                  整个场景没有错误级诊断
//! //? diag 代码 m                m 处有一条该代码的诊断（任何级别）
//! //? diag exact                 错误级诊断只有上面 `diag 代码 m` 列出的那些
//! //? diag only 代码             这个代码的诊断（任何级别）只出现在 `diag 代码 m` 列出的位置
//! //? rename m 新名字 ; has "文本" ; lacks "文本"     重命名后的全部文本满足这些条件
//! //? rename m 新名字 ; fails    重命名应当被拒绝
//! //? assist 标识 m [m2] ; has "文本" ; lacks "文本"  在 m（或 m..m2 选区）应用代码操作
//! //? noassist 标识 m [m2]       这里不应当提供该代码操作
//! //? hint m "标签"              m 处有一条该标签的 inlay hint
//! //? symbol "查询" "名字" m     工作区符号搜索的结果里有这个名字的符号，位置在 m
//! //? xfail 原因                 已知做不到：场景失败记为"预期失败"，通过了反而要提醒
//! ```
//!
//! 字符串用双引号，支持 `\n`、`\"`、`\\`。

use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ScenarioFile {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub file: usize,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Markers(Vec<String>),
    Library {
        class: String,
        member: Option<String>,
    },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextCheck {
    Has(String),
    Lacks(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    Navigate {
        kind: NavKind,
        at: String,
        target: Target,
    },
    Refs {
        at: String,
        expected: Vec<String>,
    },
    Hover {
        at: String,
        contains: String,
    },
    CompleteHas {
        at: String,
        labels: Vec<String>,
    },
    CompleteLacks {
        at: String,
        labels: Vec<String>,
    },
    CompleteFirst {
        at: String,
        label: String,
    },
    Signature {
        at: String,
        contains: String,
        active: Option<usize>,
    },
    DiagNone,
    Diag {
        code: String,
        at: String,
    },
    DiagExact,
    /// 这个代码的诊断（任何级别）只出现在 `diag 代码 m` 列出的位置
    DiagOnly {
        code: String,
    },
    Rename {
        at: String,
        new_name: String,
        checks: Vec<TextCheck>,
        fails: bool,
    },
    Assist {
        id: String,
        from: String,
        to: Option<String>,
        checks: Vec<TextCheck>,
    },
    NoAssist {
        id: String,
        from: String,
        to: Option<String>,
    },
    Hint {
        at: String,
        label: String,
    },
    /// 工作区符号搜索 `query` 的结果里有一个名为 `name`、落在标记 `at` 上的符号
    Symbol {
        query: String,
        name: String,
        at: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKind {
    Definition,
    TypeDefinition,
    Implementation,
}

#[derive(Debug, Clone)]
pub struct Scenario {
    /// `类别/名字`
    pub name: String,
    pub category: String,
    pub files: Vec<ScenarioFile>,
    pub markers: BTreeMap<String, Marker>,
    /// `(原文行号, 原文, 预期)`
    pub expectations: Vec<(usize, String, Expectation)>,
    pub xfail: Option<String>,
}

/// 把一行拆成词：空白分隔，双引号里的内容算一个词（保留一个前导的 `"` 作为"这是字符串"的记号）。
fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut text = String::from("\"");
            loop {
                match chars.next() {
                    None => return Err("字符串没有闭合".to_owned()),
                    Some('"') => break,
                    Some('\\') => match chars.next() {
                        Some('n') => text.push('\n'),
                        Some('t') => text.push('\t'),
                        Some(other) => text.push(other),
                        None => return Err("字符串没有闭合".to_owned()),
                    },
                    Some(other) => text.push(other),
                }
            }
            tokens.push(text);
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                word.push(c);
                chars.next();
            }
            tokens.push(word);
        }
    }
    Ok(tokens)
}

fn string(token: Option<&String>) -> Result<String, String> {
    match token.and_then(|t| t.strip_prefix('"')) {
        Some(text) => Ok(text.to_owned()),
        None => Err("此处应为双引号括起来的字符串".to_owned()),
    }
}

fn word(token: Option<&String>) -> Result<String, String> {
    match token {
        Some(t) if !t.starts_with('"') => Ok(t.clone()),
        _ => Err("缺少参数".to_owned()),
    }
}

/// `; has "..." ; lacks "..."` 这样的尾部。
fn text_checks(parts: &[Vec<String>]) -> Result<(Vec<TextCheck>, bool), String> {
    let mut checks = Vec::new();
    let mut fails = false;
    for part in parts {
        match part.first().map(String::as_str) {
            Some("has") => checks.push(TextCheck::Has(string(part.get(1))?)),
            Some("lacks") => checks.push(TextCheck::Lacks(string(part.get(1))?)),
            Some("fails") => fails = true,
            other => return Err(format!("不认识的条件 {other:?}")),
        }
    }
    Ok((checks, fails))
}

fn parse_target(tokens: &[String]) -> Result<Target, String> {
    match tokens.first().map(String::as_str) {
        None => Err("`->` 后面缺少目标".to_owned()),
        Some("none") => Ok(Target::None),
        Some("lib") => {
            let spec = word(tokens.get(1))?;
            let (class, member) = match spec.split_once('#') {
                Some((class, member)) => (class.to_owned(), Some(member.to_owned())),
                None => (spec, None),
            };
            Ok(Target::Library { class, member })
        }
        Some(_) => Ok(Target::Markers(tokens.to_vec())),
    }
}

fn parse_expectation(line: &str) -> Result<Option<Expectation>, String> {
    // 以 ` ; ` 分成主体和若干条件
    let mut parts: Vec<Vec<String>> = Vec::new();
    for part in split_semicolons(line) {
        parts.push(tokenize(&part)?);
    }
    let head = parts.first().cloned().unwrap_or_default();
    let rest = &parts[1.min(parts.len())..];
    let arg = |i: usize| head.get(i);

    let expectation = match head.first().map(String::as_str) {
        Some(kind @ ("def" | "typedef" | "impl")) => {
            let at = word(arg(1))?;
            if arg(2).map(String::as_str) != Some("->") {
                return Err("格式: def 标记 -> 目标".to_owned());
            }
            let kind = match kind {
                "def" => NavKind::Definition,
                "typedef" => NavKind::TypeDefinition,
                _ => NavKind::Implementation,
            };
            Expectation::Navigate {
                kind,
                at,
                target: parse_target(&head[3..])?,
            }
        }
        Some("refs") => {
            let at = word(arg(1))?;
            if arg(2).map(String::as_str) != Some("=") {
                return Err("格式: refs 标记 = 标记...".to_owned());
            }
            Expectation::Refs {
                at,
                expected: head[3..].to_vec(),
            }
        }
        Some("hover") => {
            if arg(2).map(String::as_str) != Some("~") {
                return Err("格式: hover 标记 ~ \"文本\"".to_owned());
            }
            Expectation::Hover {
                at: word(arg(1))?,
                contains: string(arg(3))?,
            }
        }
        Some("complete") => {
            let at = word(arg(1))?;
            let labels: Vec<String> = head[3.min(head.len())..]
                .iter()
                .map(|l| l.trim_start_matches('"').to_owned())
                .collect();
            match arg(2).map(String::as_str) {
                Some("has") => Expectation::CompleteHas { at, labels },
                Some("lacks") => Expectation::CompleteLacks { at, labels },
                Some("first") => Expectation::CompleteFirst {
                    at,
                    label: labels.first().cloned().ok_or("first 后面缺少标签")?,
                },
                _ => return Err("格式: complete 标记 has|lacks|first 标签...".to_owned()),
            }
        }
        Some("sig") => {
            if arg(2).map(String::as_str) != Some("~") {
                return Err("格式: sig 标记 ~ \"文本\" [active N]".to_owned());
            }
            let active = match arg(4).map(String::as_str) {
                Some("active") => Some(
                    word(arg(5))?
                        .parse::<usize>()
                        .map_err(|_| "active 后面应为数字".to_owned())?,
                ),
                _ => None,
            };
            Expectation::Signature {
                at: word(arg(1))?,
                contains: string(arg(3))?,
                active,
            }
        }
        Some("diag") => match arg(1).map(String::as_str) {
            Some("none") => Expectation::DiagNone,
            Some("exact") => Expectation::DiagExact,
            Some("only") => Expectation::DiagOnly {
                code: word(arg(2))?,
            },
            Some(_) => Expectation::Diag {
                code: word(arg(1))?,
                at: word(arg(2))?,
            },
            None => return Err("格式: diag none | diag exact | diag 代码 标记".to_owned()),
        },
        Some("rename") => {
            let (checks, fails) = text_checks(rest)?;
            Expectation::Rename {
                at: word(arg(1))?,
                new_name: word(arg(2))?,
                checks,
                fails,
            }
        }
        Some("assist") => {
            let (checks, _) = text_checks(rest)?;
            Expectation::Assist {
                id: word(arg(1))?,
                from: word(arg(2))?,
                to: arg(3).cloned(),
                checks,
            }
        }
        Some("noassist") => Expectation::NoAssist {
            id: word(arg(1))?,
            from: word(arg(2))?,
            to: arg(3).cloned(),
        },
        Some("hint") => Expectation::Hint {
            at: word(arg(1))?,
            label: string(arg(2))?,
        },
        Some("symbol") => Expectation::Symbol {
            query: string(arg(1))?,
            name: string(arg(2))?,
            at: word(arg(3))?,
        },
        Some("xfail") => return Ok(None),
        other => return Err(format!("不认识的指令 {other:?}")),
    };
    Ok(Some(expectation))
}

/// 按不在引号里的 `;` 切分。
fn split_semicolons(line: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut in_string = false;
    let mut escaped = false;
    for c in line.chars() {
        if in_string {
            parts.last_mut().unwrap().push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == ';' {
            parts.push(String::new());
        } else {
            if c == '"' {
                in_string = true;
            }
            parts.last_mut().unwrap().push(c);
        }
    }
    parts
}

/// 没有 `//- ` 分隔行时，按 package 和第一个顶层类型的名字给文件起名。
fn default_path(text: &str) -> String {
    let package = text.lines().find_map(|l| {
        l.trim()
            .strip_prefix("package ")
            .map(|p| p.trim_end_matches(';').trim().to_owned())
    });
    let type_name = text.lines().find_map(|line| {
        let mut words = line.split_whitespace().peekable();
        while let Some(w) = words.next() {
            if matches!(w, "class" | "interface" | "enum" | "record" | "@interface") {
                return words.next().map(|n| {
                    n.chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect::<String>()
                });
            }
        }
        None
    });
    let name = type_name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Main".to_owned());
    match package {
        Some(package) if !package.is_empty() => {
            format!("/{}/{name}.java", package.replace('.', "/"))
        }
        _ => format!("/{name}.java"),
    }
}

/// 去掉一段文本里的 `/*@名字*/`，记录每个标记在去掉之后的偏移。
fn strip_markers(text: &str) -> Result<(String, Vec<(String, u32)>), String> {
    let mut out = String::with_capacity(text.len());
    let mut markers = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("/*@") {
        let Some(len) = rest[start..].find("*/") else {
            return Err("标记 `/*@` 没有闭合".to_owned());
        };
        let name = &rest[start + 3..start + len];
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(format!(
                "标记名 `{name}` 不合法（只能是字母、数字、下划线）"
            ));
        }
        out.push_str(&rest[..start]);
        markers.push((name.to_owned(), out.len() as u32));
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    Ok((out, markers))
}

pub fn parse_scenario(name: &str, category: &str, source: &str) -> Result<Scenario, String> {
    let mut raw_files: Vec<(Option<String>, String)> = Vec::new();
    let mut expectations = Vec::new();
    let mut xfail = None;

    for (idx, line) in source.split_inclusive('\n').enumerate() {
        let trimmed = line.trim();
        if let Some(path) = trimmed.strip_prefix("//- ") {
            raw_files.push((Some(path.trim().to_owned()), String::new()));
            continue;
        }
        if let Some(directive) = trimmed.strip_prefix("//?") {
            let directive = directive.trim();
            if let Some(reason) = directive.strip_prefix("xfail") {
                xfail = Some(reason.trim().to_owned());
                continue;
            }
            match parse_expectation(directive) {
                Ok(Some(expectation)) => {
                    expectations.push((idx + 1, directive.to_owned(), expectation))
                }
                Ok(None) => {}
                Err(err) => return Err(format!("第 {} 行: {err}（`{directive}`）", idx + 1)),
            }
            continue;
        }
        match raw_files.last_mut() {
            Some((_, content)) => content.push_str(line),
            None if trimmed.is_empty() => {}
            None => raw_files.push((None, line.to_owned())),
        }
    }
    if raw_files.is_empty() {
        return Err("场景里没有 Java 代码".to_owned());
    }
    if expectations.is_empty() {
        return Err("场景里没有任何 `//?` 预期".to_owned());
    }

    let mut files = Vec::new();
    let mut markers = BTreeMap::new();
    for (file_idx, (path, raw)) in raw_files.into_iter().enumerate() {
        let (text, found) = strip_markers(&raw)?;
        for (marker, offset) in found {
            if markers
                .insert(
                    marker.clone(),
                    Marker {
                        file: file_idx,
                        offset,
                    },
                )
                .is_some()
            {
                return Err(format!("标记 `{marker}` 重复"));
            }
        }
        let path = path.unwrap_or_else(|| default_path(&text));
        files.push(ScenarioFile { path, text });
    }

    Ok(Scenario {
        name: name.to_owned(),
        category: category.to_owned(),
        files,
        markers,
        expectations,
        xfail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markers_and_expectations() {
        let scenario = parse_scenario(
            "nav/basic",
            "nav",
            "package app;\n\nclass Main {\n    int /*@decl*/count;\n    int f() { return /*@use*/count; }\n}\n//? def use -> decl\n//? hover use ~ \"int count\"\n//? diag none\n",
        )
        .unwrap();
        assert_eq!(scenario.files.len(), 1);
        assert_eq!(scenario.files[0].path, "/app/Main.java");
        assert!(!scenario.files[0].text.contains("/*@"));
        assert!(!scenario.files[0].text.contains("//?"));
        let decl = scenario.markers["decl"];
        assert_eq!(
            &scenario.files[0].text[decl.offset as usize..][..5],
            "count"
        );
        assert_eq!(scenario.expectations.len(), 3);
        assert_eq!(
            scenario.expectations[0].2,
            Expectation::Navigate {
                kind: NavKind::Definition,
                at: "use".to_owned(),
                target: Target::Markers(vec!["decl".to_owned()]),
            }
        );
    }

    #[test]
    fn parses_text_checks_with_semicolons_inside_strings() {
        let scenario = parse_scenario(
            "x/y",
            "x",
            "class A { int /*@v*/v; }\n//? rename v total ; has \"int total;\" ; lacks \"v;\"\n//? assist extract_variable v ; has \"a;\\nb\"\n//? xfail 还没实现\n",
        )
        .unwrap();
        assert_eq!(scenario.xfail.as_deref(), Some("还没实现"));
        assert_eq!(
            scenario.expectations[0].2,
            Expectation::Rename {
                at: "v".to_owned(),
                new_name: "total".to_owned(),
                checks: vec![
                    TextCheck::Has("int total;".to_owned()),
                    TextCheck::Lacks("v;".to_owned())
                ],
                fails: false,
            }
        );
        match &scenario.expectations[1].2 {
            Expectation::Assist { checks, .. } => {
                assert_eq!(checks, &[TextCheck::Has("a;\nb".to_owned())]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_bad_scenarios() {
        assert!(
            parse_scenario("a/b", "a", "class A {}\n").is_err(),
            "没有预期"
        );
        assert!(parse_scenario("a/b", "a", "class A { /*@x*/ /*@x*/ }\n//? diag none\n").is_err());
        assert!(parse_scenario("a/b", "a", "class A {}\n//? frobnicate x\n").is_err());
    }

    #[test]
    fn multi_file_scenarios_keep_marker_file() {
        let scenario = parse_scenario(
            "a/b",
            "a",
            "//- /a/Foo.java\npackage a;\npublic class /*@foo*/Foo {}\n//- /b/Main.java\npackage b;\nclass Main { a./*@use*/Foo f; }\n//? def use -> foo\n",
        )
        .unwrap();
        assert_eq!(scenario.files.len(), 2);
        assert_eq!(scenario.markers["foo"].file, 0);
        assert_eq!(scenario.markers["use"].file, 1);
    }
}
