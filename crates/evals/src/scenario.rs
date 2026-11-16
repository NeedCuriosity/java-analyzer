//! 场景文件的格式与解析。
//!
//! 一个场景是一个 `.java` 文件，里面除了 Java 代码还有三种评测专用的记号，
//! 交给分析器之前都会被去掉：
//!
//! - `//- /路径/文件.java [属性...]`：多文件场景的文件分隔行（和单元测试的 fixture 相同）。
//!   没有分隔行时整个场景是一个文件，文件名取自第一个顶层类型。属性：
//!   - `root:test`：这个文件在测试源码根下（默认在主源码根下）。
//!   - `lib:名字 [scope:compile|provided|runtime|test]`：这个文件不是工作区源码，而是依赖库
//!     `名字` 的一部分。评测时用 javac 编译、打成 jar 挂到类路径上（不带源码），
//!     依赖的 scope 默认是 `compile`。非 Java 文件（如 `/META-INF/spring-configuration-metadata.json`）
//!     原样放进 jar，路径里 `src/main/resources/` 之前的部分会去掉。
//!   - 不是 `.java` 的工作区文件（`application.yml`、元数据 JSON 等）作为资源文件交给分析器，
//!     里面同样可以有标记，补全、悬停、跳转、诊断的预期都可以落在它们上面。
//!   - `open`：这份源码作为"打开的库源码文件"交给分析器，相当于在编辑器里跳进依赖库之后
//!     打开的文件，里面可以有标记。和 `lib:` 一起用时它同时是那个依赖库的源码；单独用时
//!     它是类路径上已有的类（JDK）的源码，可以只写一部分成员。
//!   - `formatted`：这一段不是输入，而是前面同路径文件格式化之后应当得到的全文。
//!   - `before`：这一段不是输入，而是同路径文件被编辑之前的全文（标记会被去掉）。两段文本之间的
//!     差异当作编辑器里的一次修改交给签名记录，"按新签名更新用法"（`change_signature`）
//!     由此得知旧签名。
//!   - `module:名字`：这个文件属于用 `//? module` 声明的哪个模块（默认是第一个声明的模块）；
//!     `module:none` 表示不在任何模块里的散落文件。
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
//! //? hover m ~ "文本"           悬停的签名里包含这段文本（`!~`：签名里不包含）
//! //? complete m has a b         补全候选里有这些标签
//! //? complete m lacks a b       补全候选里没有这些标签
//! //? complete m first a         排在第一位的候选
//! //? complete m order a b c     这些候选都在，并且按这个先后顺序出现（中间可以隔着别的）
//! //? insert m 标签 ~ "文本" [snippet|plain]   选中该候选时插入的文本包含这段文本，
//!                                以及它是不是带占位符的片段
//! //? stub 类名 ; has "文本" ; lacks "文本"   依赖库里的类生成的签名桩：没有语法错误，
//!                                并且满足这些条件（类名写 `a.b.Outer$Inner`）
//! //? stub 类名 ; refs "文本" = a b   把桩作为库源码文件打开，在"文本"第一次出现处查引用，
//!                                场景文件里的引用应当恰好是这些标记
//! //? sig m ~ "文本" active N    签名帮助：当前签名包含文本，活动形参下标为 N
//! //? diag none                  整个场景没有错误级诊断
//! //? diag 代码 m                m 处有一条该代码的诊断（任何级别）
//! //? diag exact                 错误级诊断只有上面 `diag 代码 m` 列出的那些
//! //? diag only 代码             这个代码的诊断（任何级别）只出现在 `diag 代码 m` 列出的位置
//! //? rename m 新名字 ; has "文本" ; lacks "文本"     重命名后的全部文本满足这些条件
//! //? rename m 新名字 ; fails    重命名应当被拒绝
//! //? assist 标识 m [m2] ; has "文本" ; lacks "文本"  在 m（或 m..m2 选区）应用代码操作
//! //? assist 标识 m ; fails      这里提供该代码操作，但它被拒绝（不可用，附带原因）
//! //? noassist 标识 m [m2]       这里不应当提供该代码操作
//! //? hint m "标签"              m 处有一条该标签的 inlay hint
//! //? symbol "查询" "名字" m     工作区符号搜索的结果里有这个名字的符号，位置在 m
//! //? xfail 原因                 已知做不到：场景失败记为"预期失败"，通过了反而要提醒
//! //? level N [preview]          场景按语言级别为 N（可选开启预览特性）的模块来装
//! //? module 名字 [依赖...]       声明一个模块和它的依赖（按声明顺序编号，传递依赖要自己写全）。
//!                                依赖写成 `模块`、`模块/tests`（那个模块的测试代码）或
//!                                `lib:库名`，后面可以跟 `@compile|provided|runtime|test`
//! ```
//!
//! 没有 `//? module` 时，有依赖库、测试源码根或打开的库源码文件的场景按一个叫 `app` 的模块来装，
//! 依赖库按各自的 `scope:` 挂在它上面。声明了模块时，没有被任何模块列为依赖的库挂在第一个模块上。
//!
//! 字符串用双引号，支持 `\n`、`\"`、`\\`。

use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ScenarioFile {
    pub path: String,
    pub text: String,
    /// 在测试源码根下（`root:test`）
    pub test: bool,
    /// 打开的库源码文件（`lib:名字 open`），不是工作区源码
    pub library: bool,
    /// 所属的模块（`module:名字`）；`None` 表示默认的第一个模块
    pub module: Option<String>,
    /// 编辑之前的全文（`before` 段）
    pub before: Option<String>,
}

/// 用 `//? module` 声明的模块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioModule {
    pub name: String,
    pub deps: Vec<ModuleDep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepTarget {
    Module(String),
    /// 模块的测试代码（`模块/tests`）
    ModuleTests(String),
    Library(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDep {
    pub target: DepTarget,
    pub scope: LibScope,
}

fn parse_module_directive(rest: &str) -> Result<ScenarioModule, String> {
    let mut words = rest.split_whitespace();
    let name = words.next().ok_or("module 后面缺少模块名")?.to_owned();
    let mut deps = Vec::new();
    for word in words {
        let (target, scope) = match word.split_once('@') {
            Some((target, scope)) => (target, scope),
            None => (word, "compile"),
        };
        let scope = match scope {
            "compile" => LibScope::Compile,
            "provided" => LibScope::Provided,
            "runtime" => LibScope::Runtime,
            "test" => LibScope::Test,
            _ => return Err(format!("不认识的 scope `{scope}`")),
        };
        let target = if let Some(lib) = target.strip_prefix("lib:") {
            DepTarget::Library(lib.to_owned())
        } else if let Some(module) = target.strip_suffix("/tests") {
            DepTarget::ModuleTests(module.to_owned())
        } else {
            DepTarget::Module(target.to_owned())
        };
        deps.push(ModuleDep { target, scope });
    }
    Ok(ScenarioModule { name, deps })
}

/// 依赖的 scope（`scope:` 属性）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibScope {
    Compile,
    Provided,
    Runtime,
    Test,
}

/// 依赖库里的一个源文件（`lib:名字`）。
#[derive(Debug, Clone)]
pub struct LibFile {
    pub lib: String,
    pub scope: LibScope,
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
        /// `!~`：签名里不应包含这段文本
        negated: bool,
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
    /// 这些候选都在，并且按给出的先后顺序出现
    CompleteOrder {
        at: String,
        labels: Vec<String>,
    },
    /// 候选 `label` 的插入文本包含 `contains`；`snippet` 给出时还要求它是 / 不是片段
    Insert {
        at: String,
        label: String,
        contains: String,
        snippet: Option<bool>,
    },
    /// 依赖库里的类生成的签名桩
    Stub {
        class: String,
        checks: Vec<TextCheck>,
        /// 把桩作为库源码文件打开，在 `文本` 第一次出现的位置查引用，应当恰好是这些标记
        refs: Vec<(String, Vec<String>)>,
    },
    /// 第 `file` 个文件格式化之后的全文（来自 `formatted` 段）
    Formatted {
        file: usize,
        expected: String,
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
        /// 操作应当被提供但不可用
        fails: bool,
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
    /// 依赖库的源文件；不参与标记和文件编号
    pub libs: Vec<LibFile>,
    pub markers: BTreeMap<String, Marker>,
    /// `(原文行号, 原文, 预期)`
    pub expectations: Vec<(usize, String, Expectation)>,
    pub xfail: Option<String>,
    /// `//? level N [preview]`
    pub level: Option<(u32, bool)>,
    /// `//? module 名字 依赖...`
    pub modules: Vec<ScenarioModule>,
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
            let negated = match arg(2).map(String::as_str) {
                Some("~") => false,
                Some("!~") => true,
                _ => return Err("格式: hover 标记 ~ \"文本\"".to_owned()),
            };
            Expectation::Hover {
                at: word(arg(1))?,
                contains: string(arg(3))?,
                negated,
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
                Some("order") => {
                    if labels.len() < 2 {
                        return Err("order 后面至少要有两个标签".to_owned());
                    }
                    Expectation::CompleteOrder { at, labels }
                }
                _ => return Err("格式: complete 标记 has|lacks|first|order 标签...".to_owned()),
            }
        }
        Some("insert") => {
            if arg(3).map(String::as_str) != Some("~") {
                return Err("格式: insert 标记 标签 ~ \"文本\" [snippet|plain]".to_owned());
            }
            let snippet = match arg(5).map(String::as_str) {
                Some("snippet") => Some(true),
                Some("plain") => Some(false),
                None => None,
                Some(other) => return Err(format!("应为 snippet 或 plain，实际是 `{other}`")),
            };
            Expectation::Insert {
                at: word(arg(1))?,
                // 标签可以用引号括起来（带空格的标签）
                label: arg(2)
                    .map(|l| l.trim_start_matches('"').to_owned())
                    .ok_or("缺少参数")?,
                contains: string(arg(4))?,
                snippet,
            }
        }
        Some("stub") => {
            let (ref_parts, check_parts): (Vec<Vec<String>>, Vec<Vec<String>>) = rest
                .iter()
                .cloned()
                .partition(|part| part.first().map(String::as_str) == Some("refs"));
            let (checks, _) = text_checks(&check_parts)?;
            let mut refs = Vec::new();
            for part in ref_parts {
                if part.get(2).map(String::as_str) != Some("=") {
                    return Err("格式: stub 类名 ; refs \"文本\" = 标记...".to_owned());
                }
                refs.push((string(part.get(1))?, part[3..].to_vec()));
            }
            Expectation::Stub {
                class: word(arg(1))?,
                checks,
                refs,
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
            let (checks, fails) = text_checks(rest)?;
            Expectation::Assist {
                id: word(arg(1))?,
                from: word(arg(2))?,
                to: arg(3).cloned(),
                checks,
                fails,
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

struct RawFile {
    path: Option<String>,
    attrs: Vec<String>,
    /// 分隔行的行号（没有分隔行时是第一行代码）
    line: usize,
    content: String,
}

pub fn parse_scenario(name: &str, category: &str, source: &str) -> Result<Scenario, String> {
    let mut raw_files: Vec<RawFile> = Vec::new();
    let mut expectations = Vec::new();
    let mut xfail = None;
    let mut level = None;
    let mut modules: Vec<ScenarioModule> = Vec::new();

    for (idx, line) in source.split_inclusive('\n').enumerate() {
        let trimmed = line.trim();
        if let Some(header) = trimmed.strip_prefix("//- ") {
            let mut words = header.split_whitespace();
            let path = words
                .next()
                .ok_or_else(|| format!("第 {} 行: 分隔行缺少路径", idx + 1))?;
            raw_files.push(RawFile {
                path: Some(path.to_owned()),
                attrs: words.map(str::to_owned).collect(),
                line: idx + 1,
                content: String::new(),
            });
            continue;
        }
        if let Some(directive) = trimmed.strip_prefix("//?") {
            let directive = directive.trim();
            if let Some(reason) = directive.strip_prefix("xfail") {
                xfail = Some(reason.trim().to_owned());
                continue;
            }
            if let Some(rest) = directive.strip_prefix("level ") {
                let mut words = rest.split_whitespace();
                let n = words
                    .next()
                    .and_then(|w| w.parse::<u32>().ok())
                    .ok_or_else(|| format!("第 {} 行: level 后面应为数字", idx + 1))?;
                level = Some((n, words.next() == Some("preview")));
                continue;
            }
            if let Some(rest) = directive.strip_prefix("module ") {
                let module = parse_module_directive(rest)
                    .map_err(|err| format!("第 {} 行: {err}", idx + 1))?;
                if modules.iter().any(|m| m.name == module.name) {
                    return Err(format!("第 {} 行: 模块 `{}` 重复", idx + 1, module.name));
                }
                modules.push(module);
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
            Some(file) => file.content.push_str(line),
            None if trimmed.is_empty() => {}
            None => raw_files.push(RawFile {
                path: None,
                attrs: Vec::new(),
                line: idx + 1,
                content: line.to_owned(),
            }),
        }
    }

    let mut files: Vec<ScenarioFile> = Vec::new();
    let mut libs = Vec::new();
    let mut markers = BTreeMap::new();
    let mut befores: Vec<(usize, String, String)> = Vec::new();
    for raw in raw_files {
        let mut test = false;
        let mut lib = None;
        let mut scope = LibScope::Compile;
        let mut formatted = false;
        let mut open = false;
        let mut before = false;
        let mut module = None;
        for attr in &raw.attrs {
            match attr.split_once(':') {
                Some(("root", "test")) => test = true,
                Some(("root", "main")) => test = false,
                Some(("lib", name)) if !name.is_empty() => lib = Some(name.to_owned()),
                Some(("scope", "compile")) => scope = LibScope::Compile,
                Some(("scope", "provided")) => scope = LibScope::Provided,
                Some(("scope", "runtime")) => scope = LibScope::Runtime,
                Some(("scope", "test")) => scope = LibScope::Test,
                Some(("module", name)) if !name.is_empty() => module = Some(name.to_owned()),
                None if attr == "formatted" => formatted = true,
                None if attr == "open" => open = true,
                None if attr == "before" => before = true,
                _ => return Err(format!("第 {} 行: 不认识的属性 `{attr}`", raw.line)),
            }
        }
        if formatted {
            let path = raw.path.clone().unwrap_or_default();
            let Some(file) = files.iter().position(|f| f.path == path) else {
                return Err(format!(
                    "第 {} 行: `formatted` 段前面没有路径为 {path} 的文件",
                    raw.line
                ));
            };
            expectations.push((
                raw.line,
                format!("{path} formatted"),
                Expectation::Formatted {
                    file,
                    expected: raw.content,
                },
            ));
            continue;
        }
        if before {
            let (text, _) = strip_markers(&raw.content)?;
            let path = raw.path.clone().unwrap_or_default();
            befores.push((raw.line, path, text));
            continue;
        }
        let (text, found) = strip_markers(&raw.content)?;
        let path = raw.path.unwrap_or_else(|| default_path(&text));
        if let Some(lib) = lib {
            if !open && !found.is_empty() {
                return Err(format!(
                    "依赖库的文件 {path} 里不能有标记（打开的库源码文件要加 `open`）"
                ));
            }
            libs.push(LibFile {
                lib,
                scope,
                path: path.clone(),
                text: text.clone(),
            });
            if !open {
                continue;
            }
        }
        for (marker, offset) in found {
            if markers
                .insert(
                    marker.clone(),
                    Marker {
                        file: files.len(),
                        offset,
                    },
                )
                .is_some()
            {
                return Err(format!("标记 `{marker}` 重复"));
            }
        }
        files.push(ScenarioFile {
            path,
            text,
            test,
            library: open,
            module,
            before: None,
        });
    }
    for (line, path, text) in befores {
        let Some(file) = files.iter_mut().find(|f| f.path == path) else {
            return Err(format!(
                "第 {line} 行: 没有路径为 {path} 的文件，`before` 段对应不上"
            ));
        };
        file.before = Some(text);
    }
    if files.is_empty() {
        return Err("场景里没有 Java 代码".to_owned());
    }
    if expectations.is_empty() {
        return Err("场景里没有任何 `//?` 预期".to_owned());
    }
    expectations.sort_by_key(|(line, _, _)| *line);
    let known = |name: &str| modules.iter().any(|m| m.name == name);
    for file in &files {
        if let Some(name) = &file.module {
            if name != "none" && !known(name) {
                return Err(format!("{} 属于没有声明的模块 `{name}`", file.path));
            }
        }
    }
    for module in &modules {
        for dep in &module.deps {
            if let DepTarget::Module(name) | DepTarget::ModuleTests(name) = &dep.target {
                if !known(name) {
                    return Err(format!(
                        "模块 `{}` 依赖没有声明的模块 `{name}`",
                        module.name
                    ));
                }
            }
        }
    }

    Ok(Scenario {
        name: name.to_owned(),
        category: category.to_owned(),
        files,
        libs,
        markers,
        expectations,
        xfail,
        level,
        modules,
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

    #[test]
    fn parses_file_attributes() {
        let scenario = parse_scenario(
            "a/b",
            "a",
            "//- /org/lib/Tool.java lib:tools scope:test\npackage org.lib;\npublic class Tool {}\n//- /app/Main.java\npackage app;\nclass Main { /*@m*/ }\n//- /app/MainTest.java root:test\npackage app;\nclass MainTest {}\n//? complete m lacks Tool\n//? stub org.lib.Tool ; has \"class Tool\"\n",
        )
        .unwrap();
        assert_eq!(scenario.files.len(), 2);
        assert!(!scenario.files[0].test);
        assert!(scenario.files[1].test);
        assert_eq!(scenario.markers["m"].file, 0);
        assert_eq!(scenario.libs.len(), 1);
        assert_eq!(scenario.libs[0].lib, "tools");
        assert_eq!(scenario.libs[0].scope, LibScope::Test);
        assert_eq!(scenario.libs[0].path, "/org/lib/Tool.java");
        assert!(parse_scenario(
            "a/b",
            "a",
            "//- /A.java root:nowhere\nclass A {}\n//? diag none\n"
        )
        .is_err());
    }

    #[test]
    fn formatted_section_becomes_an_expectation() {
        let scenario = parse_scenario(
            "f/x",
            "f",
            "//- /A.java\nclass A {\nint x;\n}\n//- /A.java formatted\nclass A {\n    int x;\n}\n",
        )
        .unwrap();
        assert_eq!(scenario.files.len(), 1);
        assert_eq!(
            scenario.expectations[0].2,
            Expectation::Formatted {
                file: 0,
                expected: "class A {\n    int x;\n}\n".to_owned(),
            }
        );
        assert!(parse_scenario("f/x", "f", "//- /B.java formatted\nclass B {}\n").is_err());
    }

    #[test]
    fn before_section_is_attached_to_its_file() {
        let scenario = parse_scenario(
            "s/x",
            "s",
            "//- /A.java before\nclass A { void f(int a) {} }\n//- /A.java\nclass A { void /*@m*/f(int a, int b) {} }\n//? assist change_signature m ; fails\n",
        )
        .unwrap();
        assert_eq!(scenario.files.len(), 1);
        assert_eq!(
            scenario.files[0].before.as_deref(),
            Some("class A { void f(int a) {} }\n")
        );
        assert!(matches!(
            scenario.expectations[0].2,
            Expectation::Assist { fails: true, .. }
        ));
        assert!(parse_scenario(
            "s/x",
            "s",
            "//- /B.java before\nclass B {}\n//- /A.java\nclass A {}\n//? diag none\n"
        )
        .is_err());
    }

    #[test]
    fn parses_completion_order_and_insert() {
        let scenario = parse_scenario(
            "c/x",
            "c",
            "class A { void f() { this./*@m*/; } }\n//? complete m order f hashCode\n//? insert m f ~ \"f()\" plain\n//? insert m equals ~ \"equals($0)\" snippet\n",
        )
        .unwrap();
        assert_eq!(
            scenario.expectations[0].2,
            Expectation::CompleteOrder {
                at: "m".to_owned(),
                labels: vec!["f".to_owned(), "hashCode".to_owned()],
            }
        );
        assert_eq!(
            scenario.expectations[2].2,
            Expectation::Insert {
                at: "m".to_owned(),
                label: "equals".to_owned(),
                contains: "equals($0)".to_owned(),
                snippet: Some(true),
            }
        );
    }
}
