//! 把一个场景装进分析器，逐条核对预期。

use std::sync::Arc;

use anyhow::Context;
use base_db::{ProjectGraph, SourceRootKind};
use classfile::ClassPath;
use ide::{
    AnalysisHost, Change, CompletionConfig, DiagnosticsConfig, FileChange, FileId, FileKind,
    FilePosition, FileRange, NavFile, NavigationTarget, Severity, SourceChange,
};
use syntax::{TextRange, TextSize};

use crate::scenario::{Expectation, Marker, NavKind, Scenario, Target, TextCheck};

/// 所有场景共用的环境：JDK 的类路径只建一次。
pub struct Env {
    graph: Arc<ProjectGraph>,
    class_path: Arc<ClassPath>,
    pub jdk: String,
}

impl Env {
    pub fn new() -> anyhow::Result<Env> {
        let jdk = project_model::discover_jdk(None)
            .context("没有找到 JDK：评测里的代码要用到真实的标准库，请设置 JAVA_HOME")?;
        let description = format!("{} ({})", jdk.version, jdk.home.display());
        let graph = ProjectGraph {
            modules: Vec::new(),
            libraries: Vec::new(),
            jdk: Some(jdk),
        };
        let class_path = project_model::build_class_path(&graph);
        Ok(Env {
            graph: Arc::new(graph),
            class_path: Arc::new(class_path),
            jdk: description,
        })
    }

    /// 装入一组 `(路径, 文本)`，文件号就是下标。
    pub fn load(&self, files: &[(String, String)]) -> AnalysisHost {
        let mut change = Change {
            project: Some(self.graph.clone()),
            class_path: Some(self.class_path.clone()),
            ..Change::default()
        };
        for (idx, (_, text)) in files.iter().enumerate() {
            change.files.push((
                FileId(idx as u32),
                Some(FileChange {
                    text: Arc::from(text.as_str()),
                    kind: FileKind::Workspace {
                        module: None,
                        root: SourceRootKind::Main,
                    },
                }),
            ));
        }
        let mut host = AnalysisHost::new();
        host.apply_change(change);
        host
    }
}

#[derive(Debug, Clone)]
pub struct ScenarioResult {
    pub name: String,
    pub category: String,
    pub checks: usize,
    /// 每条失败的预期：`第 N 行 <指令>: 原因`
    pub failures: Vec<String>,
    pub xfail: Option<String>,
}

impl ScenarioResult {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

struct Checker<'a> {
    scenario: &'a Scenario,
    analysis: ide::Analysis,
}

fn contains(range: TextRange, offset: u32) -> bool {
    u32::from(range.start()) <= offset && offset <= u32::from(range.end())
}

fn describe_target(scenario: &Scenario, target: &NavigationTarget) -> String {
    match &target.file {
        NavFile::Source(file) => {
            let path = scenario
                .files
                .get(file.0 as usize)
                .map_or("?", |f| f.path.as_str());
            format!(
                "{path}@{}（{}）",
                u32::from(target.focus_range.start()),
                target.name
            )
        }
        NavFile::Library { class, member } => match member {
            Some(member) => format!("lib {}#{}", class.replace(['/', '$'], "."), member.name),
            None => format!("lib {}", class.replace(['/', '$'], ".")),
        },
    }
}

impl Checker<'_> {
    fn marker(&self, name: &str) -> Result<Marker, String> {
        self.scenario
            .markers
            .get(name)
            .copied()
            .ok_or_else(|| format!("没有名为 `{name}` 的标记"))
    }

    fn position(&self, name: &str) -> Result<FilePosition, String> {
        let marker = self.marker(name)?;
        Ok(FilePosition {
            file_id: FileId(marker.file as u32),
            offset: TextSize::from(marker.offset),
        })
    }

    fn range(&self, from: &str, to: Option<&String>) -> Result<FileRange, String> {
        let start = self.marker(from)?;
        let end = match to {
            Some(to) => self.marker(to)?,
            None => start,
        };
        if start.file != end.file || end.offset < start.offset {
            return Err("选区的两个标记必须在同一个文件里，并且先后有序".to_owned());
        }
        Ok(FileRange {
            file_id: FileId(start.file as u32),
            range: TextRange::new(TextSize::from(start.offset), TextSize::from(end.offset)),
        })
    }

    /// 应用修改后所有文件的文本（按文件顺序拼接）。
    fn apply(&self, change: &SourceChange) -> String {
        let mut texts: Vec<String> = self.scenario.files.iter().map(|f| f.text.clone()).collect();
        for (file_id, edits) in &change.edits {
            let Some(text) = texts.get_mut(file_id.0 as usize) else {
                continue;
            };
            let mut edits: Vec<_> = edits.iter().collect();
            edits.sort_by_key(|e| (e.range.start(), e.range.end()));
            for edit in edits.into_iter().rev() {
                let range = usize::from(edit.range.start())..usize::from(edit.range.end());
                if range.end <= text.len() {
                    text.replace_range(range, &edit.new_text);
                }
            }
        }
        texts.join("\n")
    }

    fn check_text(&self, text: &str, checks: &[TextCheck]) -> Result<(), String> {
        for check in checks {
            match check {
                TextCheck::Has(needle) if !text.contains(needle.as_str()) => {
                    return Err(format!("修改后的文本里没有 {needle:?}。实际:\n{text}"));
                }
                TextCheck::Lacks(needle) if text.contains(needle.as_str()) => {
                    return Err(format!("修改后的文本里不该有 {needle:?}。实际:\n{text}"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn all_diagnostics(&self) -> Result<Vec<(usize, ide::Diagnostic)>, String> {
        let config = DiagnosticsConfig::default();
        let mut res = Vec::new();
        for idx in 0..self.scenario.files.len() {
            let diagnostics = self
                .analysis
                .diagnostics(FileId(idx as u32), &config)
                .map_err(|_| "分析被取消")?;
            res.extend(diagnostics.into_iter().map(|d| (idx, d)));
        }
        Ok(res)
    }

    fn describe_diagnostic(&self, file: usize, d: &ide::Diagnostic) -> String {
        let text = &self.scenario.files[file].text;
        let start = usize::from(d.range.start()).min(text.len());
        let line = text[..start].matches('\n').count() + 1;
        format!(
            "{}:{line} [{}] {}",
            self.scenario.files[file].path,
            d.code.as_str(),
            d.message
        )
    }

    fn check(&self, expectation: &Expectation) -> Result<(), String> {
        const CANCELLED: &str = "分析被取消";
        match expectation {
            Expectation::Navigate { kind, at, target } => {
                let position = self.position(at)?;
                let found = match kind {
                    NavKind::Definition => self.analysis.goto_definition(position),
                    NavKind::TypeDefinition => self.analysis.goto_type_definition(position),
                    NavKind::Implementation => self.analysis.goto_implementation(position),
                }
                .map_err(|_| CANCELLED)?
                .map(|info| info.info)
                .unwrap_or_default();
                let actual: Vec<String> = found
                    .iter()
                    .map(|t| describe_target(self.scenario, t))
                    .collect();
                match target {
                    Target::None => {
                        if !found.is_empty() {
                            return Err(format!("不该有结果，实际: {actual:?}"));
                        }
                    }
                    Target::Library { class, member } => {
                        let hit = found.iter().any(|t| match &t.file {
                            NavFile::Library {
                                class: found_class,
                                member: found_member,
                            } => {
                                found_class.replace(['/', '$'], ".") == *class
                                    && member.as_ref().is_none_or(|m| {
                                        found_member.as_ref().is_some_and(|f| f.name == m.as_str())
                                    })
                            }
                            NavFile::Source(_) => false,
                        });
                        if !hit {
                            return Err(format!("没有跳到库里的 {class}，实际: {actual:?}"));
                        }
                    }
                    Target::Markers(names) => {
                        for name in names {
                            let marker = self.marker(name)?;
                            let hit = found.iter().any(|t| {
                                t.file == NavFile::Source(FileId(marker.file as u32))
                                    && contains(t.focus_range, marker.offset)
                            });
                            if !hit {
                                return Err(format!("结果里没有标记 `{name}`，实际: {actual:?}"));
                            }
                        }
                        if found.len() != names.len() {
                            return Err(format!(
                                "应当恰好有 {} 个结果，实际: {actual:?}",
                                names.len()
                            ));
                        }
                    }
                }
                Ok(())
            }
            Expectation::Refs { at, expected } => {
                let position = self.position(at)?;
                let found = self
                    .analysis
                    .find_all_refs(position, false)
                    .map_err(|_| CANCELLED)?
                    .unwrap_or_default();
                let actual: Vec<String> = found
                    .iter()
                    .map(|r| {
                        let path = self
                            .scenario
                            .files
                            .get(r.file_id.0 as usize)
                            .map_or("?", |f| f.path.as_str());
                        format!("{path}@{}", u32::from(r.range.start()))
                    })
                    .collect();
                for name in expected {
                    let marker = self.marker(name)?;
                    let hit = found.iter().any(|r| {
                        r.file_id == FileId(marker.file as u32) && contains(r.range, marker.offset)
                    });
                    if !hit {
                        return Err(format!("引用里没有标记 `{name}`，实际: {actual:?}"));
                    }
                }
                if found.len() != expected.len() {
                    return Err(format!(
                        "应当恰好有 {} 处引用，实际: {actual:?}",
                        expected.len()
                    ));
                }
                Ok(())
            }
            Expectation::Hover { at, contains } => {
                let hover = self
                    .analysis
                    .hover(self.position(at)?)
                    .map_err(|_| CANCELLED)?
                    .ok_or("没有悬停信息")?
                    .info;
                let text = format!(
                    "{}\n{}\n{}",
                    hover.signature,
                    hover.container.unwrap_or_default(),
                    hover.docs.unwrap_or_default()
                );
                if text.contains(contains.as_str()) {
                    Ok(())
                } else {
                    Err(format!("悬停内容里没有 {contains:?}，实际: {text:?}"))
                }
            }
            Expectation::CompleteHas { at, .. }
            | Expectation::CompleteLacks { at, .. }
            | Expectation::CompleteFirst { at, .. } => {
                let items = self
                    .analysis
                    .completions(self.position(at)?, &CompletionConfig::default())
                    .map_err(|_| CANCELLED)?
                    .items;
                let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
                let has = |label: &String| {
                    items
                        .iter()
                        .any(|i| i.label == *label || i.lookup == *label)
                };
                let preview: Vec<&str> = labels.iter().take(15).copied().collect();
                match expectation {
                    Expectation::CompleteHas { labels: wanted, .. } => {
                        match wanted.iter().find(|l| !has(l)) {
                            Some(missing) => Err(format!(
                                "候选里没有 `{missing}`（共 {} 条，前几条: {preview:?}）",
                                items.len()
                            )),
                            None => Ok(()),
                        }
                    }
                    Expectation::CompleteLacks {
                        labels: unwanted, ..
                    } => match unwanted.iter().find(|l| has(l)) {
                        Some(present) => Err(format!("候选里不该有 `{present}`")),
                        None => Ok(()),
                    },
                    Expectation::CompleteFirst { label, .. } => match items.first() {
                        Some(first) if first.label == *label || first.lookup == *label => Ok(()),
                        _ => Err(format!("第一条应当是 `{label}`，实际前几条: {preview:?}")),
                    },
                    _ => unreachable!(),
                }
            }
            Expectation::Signature {
                at,
                contains,
                active,
            } => {
                let help = self
                    .analysis
                    .signature_help(self.position(at)?)
                    .map_err(|_| CANCELLED)?
                    .ok_or("没有签名帮助")?;
                let label = help
                    .signatures
                    .get(help.active_signature)
                    .map(|s| s.label.clone())
                    .unwrap_or_default();
                if !label.contains(contains.as_str()) {
                    return Err(format!("当前签名里没有 {contains:?}，实际: {label:?}"));
                }
                if active.is_some() && help.active_parameter != *active {
                    return Err(format!(
                        "活动形参应为 {active:?}，实际: {:?}",
                        help.active_parameter
                    ));
                }
                Ok(())
            }
            Expectation::DiagNone => {
                let errors: Vec<String> = self
                    .all_diagnostics()?
                    .iter()
                    .filter(|(_, d)| d.severity == Severity::Error)
                    .map(|(file, d)| self.describe_diagnostic(*file, d))
                    .collect();
                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(format!("不该有错误级诊断，实际: {errors:#?}"))
                }
            }
            Expectation::Diag { code, at } => {
                let marker = self.marker(at)?;
                let all = self.all_diagnostics()?;
                let hit = all.iter().any(|(file, d)| {
                    *file == marker.file
                        && d.code.as_str() == code
                        && contains(d.range, marker.offset)
                });
                if hit {
                    Ok(())
                } else {
                    let actual: Vec<String> = all
                        .iter()
                        .map(|(file, d)| self.describe_diagnostic(*file, d))
                        .collect();
                    Err(format!(
                        "标记 `{at}` 处没有 `{code}`，实际的诊断: {actual:#?}"
                    ))
                }
            }
            Expectation::DiagOnly { code: only } => {
                let expected: Vec<Marker> = self
                    .scenario
                    .expectations
                    .iter()
                    .filter_map(|(_, _, e)| match e {
                        Expectation::Diag { code, at } if code == only => self.marker(at).ok(),
                        _ => None,
                    })
                    .collect();
                let unexpected: Vec<String> = self
                    .all_diagnostics()?
                    .iter()
                    .filter(|(_, d)| d.code.as_str() == only.as_str())
                    .filter(|(file, d)| {
                        !expected
                            .iter()
                            .any(|marker| marker.file == *file && contains(d.range, marker.offset))
                    })
                    .map(|(file, d)| self.describe_diagnostic(*file, d))
                    .collect();
                if unexpected.is_empty() {
                    Ok(())
                } else {
                    Err(format!("`{only}` 出现在了预期之外的位置: {unexpected:#?}"))
                }
            }
            Expectation::DiagExact => {
                let expected: Vec<(&String, Marker)> = self
                    .scenario
                    .expectations
                    .iter()
                    .filter_map(|(_, _, e)| match e {
                        Expectation::Diag { code, at } => {
                            self.marker(at).ok().map(|marker| (code, marker))
                        }
                        _ => None,
                    })
                    .collect();
                let unexpected: Vec<String> = self
                    .all_diagnostics()?
                    .iter()
                    .filter(|(_, d)| d.severity == Severity::Error)
                    .filter(|(file, d)| {
                        !expected.iter().any(|(code, marker)| {
                            marker.file == *file
                                && d.code.as_str() == code.as_str()
                                && contains(d.range, marker.offset)
                        })
                    })
                    .map(|(file, d)| self.describe_diagnostic(*file, d))
                    .collect();
                if unexpected.is_empty() {
                    Ok(())
                } else {
                    Err(format!("有预期之外的错误级诊断: {unexpected:#?}"))
                }
            }
            Expectation::Rename {
                at,
                new_name,
                checks,
                fails,
            } => {
                let result = self
                    .analysis
                    .rename(self.position(at)?, new_name)
                    .map_err(|_| CANCELLED)?;
                match (result, fails) {
                    (Err(_), true) => Ok(()),
                    (Ok(_), true) => Err("重命名应当被拒绝，但成功了".to_owned()),
                    (Err(message), false) => Err(format!("重命名被拒绝: {message}")),
                    (Ok(change), false) => self.check_text(&self.apply(&change), checks),
                }
            }
            Expectation::Assist {
                id,
                from,
                to,
                checks,
            } => {
                let range = self.range(from, to.as_ref())?;
                let assists = self.analysis.assists(range).map_err(|_| CANCELLED)?;
                let Some(assist) = assists.iter().find(|a| a.id == id.as_str()) else {
                    let offered: Vec<&str> = assists.iter().map(|a| a.id).collect();
                    return Err(format!("这里没有提供 `{id}`，提供的是: {offered:?}"));
                };
                self.check_text(&self.apply(&assist.change), checks)
            }
            Expectation::NoAssist { id, from, to } => {
                let range = self.range(from, to.as_ref())?;
                let assists = self.analysis.assists(range).map_err(|_| CANCELLED)?;
                if assists.iter().any(|a| a.id == id.as_str()) {
                    Err(format!("这里不该提供 `{id}`"))
                } else {
                    Ok(())
                }
            }
            Expectation::Symbol { query, name, at } => {
                let marker = self.marker(at)?;
                let found = self
                    .analysis
                    .workspace_symbols(query, 64)
                    .map_err(|_| CANCELLED)?;
                let hit = found.iter().any(|target| {
                    target.name.as_str() == name.as_str()
                        && matches!(&target.file, NavFile::Source(file) if file.0 as usize == marker.file)
                        && contains(target.focus_range, marker.offset)
                });
                if hit {
                    Ok(())
                } else {
                    let actual: Vec<String> = found
                        .iter()
                        .map(|t| format!("{} @ {}", t.name, describe_target(self.scenario, t)))
                        .collect();
                    Err(format!(
                        "搜索 {query:?} 没有找到 {name:?}（标记 `{at}`），实际: {actual:?}"
                    ))
                }
            }
            Expectation::Hint { at, label } => {
                let marker = self.marker(at)?;
                let hints = self
                    .analysis
                    .inlay_hints(FileId(marker.file as u32), None)
                    .map_err(|_| CANCELLED)?;
                let hit = hints.iter().any(|h| {
                    u32::from(h.offset) == marker.offset && h.label.trim() == label.trim()
                });
                if hit {
                    Ok(())
                } else {
                    let actual: Vec<String> = hints
                        .iter()
                        .map(|h| format!("@{} {:?}", u32::from(h.offset), h.label))
                        .collect();
                    Err(format!(
                        "标记 `{at}` 处没有提示 {label:?}，实际: {actual:?}"
                    ))
                }
            }
        }
    }
}

pub fn run_scenario(env: &Env, scenario: &Scenario) -> ScenarioResult {
    let files: Vec<(String, String)> = scenario
        .files
        .iter()
        .map(|f| (f.path.clone(), f.text.clone()))
        .collect();
    let host = env.load(&files);
    let checker = Checker {
        scenario,
        analysis: host.analysis(),
    };

    let mut failures = Vec::new();
    for (line, source, expectation) in &scenario.expectations {
        // 分析器里的 panic 也算这条预期失败，不让它中断整轮评测
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| checker.check(expectation)));
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => failures.push(format!("第 {line} 行 `{source}`: {reason}")),
            Err(_) => failures.push(format!("第 {line} 行 `{source}`: 分析器 panic")),
        }
    }
    ScenarioResult {
        name: scenario.name.clone(),
        category: scenario.category.clone(),
        checks: scenario.expectations.len(),
        failures,
        xfail: scenario.xfail.clone(),
    }
}
