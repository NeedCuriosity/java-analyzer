//! 把一个场景装进分析器，逐条核对预期。

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use anyhow::Context;
use base_db::{
    Dependency, DependencyTarget, LibraryData, LibraryId, ModuleData, ModuleId, ProjectGraph,
    Scope, SourceRoot, SourceRootKind,
};
use classfile::ClassPath;
use ide::{
    AnalysisHost, Change, CompletionConfig, DiagnosticsConfig, FileChange, FileId, FileKind,
    FilePosition, FileRange, NavFile, NavigationTarget, Severity, SourceChange,
};
use syntax::{TextRange, TextSize};

use crate::scenario::{
    Expectation, LibScope, Marker, NavKind, Scenario, ScenarioFile, Target, TextCheck,
};

/// 所有场景共用的环境：JDK 的类路径只建一次。
pub struct Env {
    graph: Arc<ProjectGraph>,
    class_path: Arc<ClassPath>,
    jdk_home: PathBuf,
    pub jdk: String,
}

/// 场景装好之后的分析器，以及为它临时生成的文件（依赖库的 jar），用完删掉。
pub struct Loaded {
    pub host: AnalysisHost,
    project: Arc<ProjectGraph>,
    class_path: Arc<ClassPath>,
    scratch: Option<PathBuf>,
}

impl Drop for Loaded {
    fn drop(&mut self) {
        if let Some(dir) = &self.scratch {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// 场景文件交给分析器时的种类。`in_module`：场景是按一个模块的工程装的。
fn scenario_file_kind(file: &ScenarioFile, in_module: bool) -> FileKind {
    if file.library {
        return FileKind::Library;
    }
    FileKind::Workspace {
        module: in_module.then_some(ModuleId(0)),
        root: if file.test {
            SourceRootKind::Test
        } else {
            SourceRootKind::Main
        },
    }
}

fn run_tool(binary: &Path, args: &[String]) -> Result<(), String> {
    let output = Command::new(binary)
        .args(args)
        .output()
        .map_err(|err| format!("无法运行 {}: {err}", binary.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} 失败:\n{}",
            binary.display(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

impl Env {
    pub fn new() -> anyhow::Result<Env> {
        let jdk = project_model::discover_jdk(None)
            .context("没有找到 JDK：评测里的代码要用到真实的标准库，请设置 JAVA_HOME")?;
        let description = format!("{} ({})", jdk.version, jdk.home.display());
        let jdk_home = jdk.home.clone();
        let graph = ProjectGraph {
            modules: Vec::new(),
            libraries: Vec::new(),
            jdk: Some(jdk),
        };
        let class_path = project_model::build_class_path(&graph);
        Ok(Env {
            graph: Arc::new(graph),
            class_path: Arc::new(class_path),
            jdk_home,
            jdk: description,
        })
    }

    /// 把依赖库的源文件编译并打成 jar（不带源码，和没有 sources jar 的第三方库一样）。
    fn build_library(
        &self,
        dir: &Path,
        files: &[&crate::scenario::LibFile],
    ) -> Result<PathBuf, String> {
        let io = |err: std::io::Error| format!("无法写入 {}: {err}", dir.display());
        let classes = dir.join("classes");
        std::fs::create_dir_all(&classes).map_err(io)?;
        let mut args: Vec<String> = ["-proc:none", "-Xlint:none", "-nowarn", "-d"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        args.push(classes.display().to_string());
        for file in files {
            let path = dir.join("src").join(file.path.trim_start_matches('/'));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            std::fs::write(&path, &file.text).map_err(io)?;
            args.push(path.display().to_string());
        }
        run_tool(&self.jdk_home.join("bin/javac"), &args)?;
        let jar = dir.join("lib.jar");
        run_tool(
            &self.jdk_home.join("bin/jar"),
            &[
                "--create".to_owned(),
                "--file".to_owned(),
                jar.display().to_string(),
                "-C".to_owned(),
                classes.display().to_string(),
                ".".to_owned(),
            ],
        )?;
        Ok(jar)
    }

    /// 装入一个场景。有依赖库、测试源码根或打开的库源码文件时，按一个模块的工程来装：
    /// 主 / 测试两个源码根，依赖按各自的 scope 挂在模块上。
    pub fn load_scenario(&self, scenario: &Scenario) -> Result<Loaded, String> {
        let needs_project = !scenario.libs.is_empty()
            || scenario.level.is_some()
            || scenario.files.iter().any(|f| f.test || f.library);
        if !needs_project {
            let files: Vec<(String, String)> = scenario
                .files
                .iter()
                .map(|f| (f.path.clone(), f.text.clone()))
                .collect();
            return Ok(Loaded {
                host: self.load(&files),
                project: self.graph.clone(),
                class_path: self.class_path.clone(),
                scratch: None,
            });
        }

        let mut graph = ProjectGraph {
            modules: Vec::new(),
            libraries: Vec::new(),
            jdk: self.graph.jdk.clone(),
        };
        let mut dependencies = Vec::new();
        let mut scratch = None;
        if !scenario.libs.is_empty() {
            let dir = std::env::temp_dir().join(format!(
                "ja-eval-{}-{}",
                std::process::id(),
                scenario.name.replace(['/', '\\'], "-")
            ));
            scratch = Some(dir.clone());
            let mut names: Vec<&str> = scenario.libs.iter().map(|f| f.lib.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            for name in names {
                let files: Vec<_> = scenario.libs.iter().filter(|f| f.lib == name).collect();
                let built = self.build_library(&dir.join(name), &files);
                let jar = match built {
                    Ok(jar) => jar,
                    Err(err) => {
                        let _ = std::fs::remove_dir_all(&dir);
                        return Err(format!("依赖库 `{name}` 构建失败: {err}"));
                    }
                };
                let scope = match files[0].scope {
                    LibScope::Compile => Scope::Compile,
                    LibScope::Provided => Scope::Provided,
                    LibScope::Runtime => Scope::Runtime,
                    LibScope::Test => Scope::Test,
                };
                dependencies.push(Dependency {
                    target: DependencyTarget::Library(LibraryId(graph.libraries.len() as u32)),
                    scope,
                });
                graph.libraries.push(LibraryData {
                    name: name.to_owned(),
                    jar,
                    sources: None,
                });
            }
        }
        graph.modules.push(ModuleData {
            name: "app".to_owned(),
            root: PathBuf::from("/"),
            source_roots: vec![
                SourceRoot {
                    path: PathBuf::from("/src/main/java"),
                    kind: SourceRootKind::Main,
                },
                SourceRoot {
                    path: PathBuf::from("/src/test/java"),
                    kind: SourceRootKind::Test,
                },
            ],
            language_level: scenario.level.map(|(level, _)| level),
            api_release: scenario.level.map(|(level, _)| level),
            preview: scenario.level.is_some_and(|(_, preview)| preview),
            dependencies,
        });
        let class_path = if graph.libraries.is_empty() {
            self.class_path.clone()
        } else {
            Arc::new(project_model::build_class_path(&graph))
        };

        let project = Arc::new(graph);
        let mut change = Change {
            project: Some(project.clone()),
            class_path: Some(class_path.clone()),
            ..Change::default()
        };
        let mut resources = Vec::new();
        for (idx, file) in scenario.files.iter().enumerate() {
            // Spring 配置文件等非 Java 文件作为资源文件
            if !file.path.ends_with(".java") {
                resources.push(base_db::ResourceFile {
                    file_id: FileId(idx as u32),
                    name: file.path.rsplit('/').next().unwrap_or(&file.path).into(),
                    module: Some(ModuleId(0)),
                    text: Arc::from(file.text.as_str()),
                });
                continue;
            }
            change.files.push((
                FileId(idx as u32),
                Some(FileChange {
                    text: Arc::from(file.text.as_str()),
                    kind: scenario_file_kind(file, true),
                    name: base_db::file_stem(&file.path),
                }),
            ));
        }
        change.resources = Some(Arc::new(resources));
        let mut host = AnalysisHost::new();
        host.apply_change(change);
        Ok(Loaded {
            host,
            project,
            class_path,
            scratch,
        })
    }

    /// 装入一组 `(路径, 文本)`，文件号就是下标。
    pub fn load(&self, files: &[(String, String)]) -> AnalysisHost {
        let mut change = Change {
            project: Some(self.graph.clone()),
            class_path: Some(self.class_path.clone()),
            ..Change::default()
        };
        let mut resources = Vec::new();
        for (idx, (path, text)) in files.iter().enumerate() {
            // Spring 配置文件等非 Java 文件作为资源文件
            if !path.ends_with(".java") {
                resources.push(base_db::ResourceFile {
                    file_id: FileId(idx as u32),
                    name: path.rsplit('/').next().unwrap_or(path).into(),
                    module: None,
                    text: Arc::from(text.as_str()),
                });
                continue;
            }
            change.files.push((
                FileId(idx as u32),
                Some(FileChange {
                    name: base_db::file_stem(path),
                    text: Arc::from(text.as_str()),
                    kind: FileKind::Workspace {
                        module: None,
                        root: SourceRootKind::Main,
                    },
                }),
            ));
        }
        change.resources = Some(Arc::new(resources));
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
    loaded: &'a Loaded,
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
        // 新建的文件以 `//- 文件名` 开头接在后面
        for (_, name, text) in &change.file_creates {
            texts.push(format!("//- {name}\n{text}"));
        }
        texts.join("\n")
    }

    /// 把桩作为打开的库源码文件加进场景，在 `needle` 第一次出现的位置查引用（只看场景文件里的）。
    fn check_stub_refs(&self, stub: &str, needle: &str, expected: &[String]) -> Result<(), String> {
        let offset = stub
            .find(needle)
            .ok_or_else(|| format!("桩里没有 {needle:?}。实际:\n{stub}"))?;
        let in_module = !self.loaded.project.modules.is_empty();
        let mut files: Vec<(FileId, Option<FileChange>)> = self
            .scenario
            .files
            .iter()
            .enumerate()
            .map(|(idx, file)| {
                (
                    FileId(idx as u32),
                    Some(FileChange {
                        text: Arc::from(file.text.as_str()),
                        kind: scenario_file_kind(file, in_module),
                        name: base_db::file_stem(&file.path),
                    }),
                )
            })
            .collect();
        let stub_id = FileId(files.len() as u32);
        files.push((
            stub_id,
            Some(FileChange {
                text: Arc::from(stub),
                kind: FileKind::Library,
                name: base_db::file_stem("Stub.java"),
            }),
        ));
        let mut host = AnalysisHost::new();
        host.apply_change(Change {
            project: Some(self.loaded.project.clone()),
            class_path: Some(self.loaded.class_path.clone()),
            files,
            ..Change::default()
        });
        let found = host
            .analysis()
            .find_all_refs(
                FilePosition {
                    file_id: stub_id,
                    offset: TextSize::from(offset as u32),
                },
                false,
            )
            .map_err(|_| "分析被取消")?
            .unwrap_or_default();
        // 桩自己里面的引用（返回类型、形参类型）不计，只看场景文件里的
        let found: Vec<_> = found.into_iter().filter(|r| r.file_id != stub_id).collect();
        let actual: Vec<String> = found
            .iter()
            .map(|r| {
                let path = self
                    .scenario
                    .files
                    .get(r.file_id.0 as usize)
                    .map_or("<桩>", |f| f.path.as_str());
                format!("{path}@{}", u32::from(r.range.start()))
            })
            .collect();
        for name in expected {
            let marker = self.marker(name)?;
            let hit = found.iter().any(|r| {
                r.file_id == FileId(marker.file as u32) && contains(r.range, marker.offset)
            });
            if !hit {
                return Err(format!(
                    "桩里 {needle:?} 的引用里没有标记 `{name}`，实际: {actual:?}"
                ));
            }
        }
        if found.len() != expected.len() {
            return Err(format!(
                "桩里 {needle:?} 应当恰好有 {} 处引用，实际: {actual:?}",
                expected.len()
            ));
        }
        Ok(())
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
            Expectation::Hover {
                at,
                contains,
                negated,
            } => {
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
                if *negated {
                    if hover.signature.contains(contains.as_str()) {
                        Err(format!("悬停签名里不应有 {contains:?}，实际: {text:?}"))
                    } else {
                        Ok(())
                    }
                } else if text.contains(contains.as_str()) {
                    Ok(())
                } else {
                    Err(format!("悬停内容里没有 {contains:?}，实际: {text:?}"))
                }
            }
            Expectation::CompleteHas { at, .. }
            | Expectation::CompleteLacks { at, .. }
            | Expectation::CompleteFirst { at, .. }
            | Expectation::CompleteOrder { at, .. }
            | Expectation::Insert { at, .. } => {
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
                    Expectation::CompleteOrder {
                        labels: ordered, ..
                    } => {
                        // 同名的候选（重载）取最靠前的那个
                        let mut previous: Option<(&String, usize)> = None;
                        for label in ordered {
                            let Some(index) = items
                                .iter()
                                .position(|i| i.label == *label || i.lookup == *label)
                            else {
                                return Err(format!(
                                    "候选里没有 `{label}`（共 {} 条，前几条: {preview:?}）",
                                    items.len()
                                ));
                            };
                            if let Some((before, before_index)) = previous {
                                if index <= before_index {
                                    return Err(format!(
                                        "`{before}`（第 {} 条）应当排在 `{label}`（第 {} 条）前面，实际前几条: {preview:?}",
                                        before_index + 1,
                                        index + 1
                                    ));
                                }
                            }
                            previous = Some((label, index));
                        }
                        Ok(())
                    }
                    Expectation::Insert {
                        label,
                        contains,
                        snippet,
                        ..
                    } => {
                        let candidates: Vec<_> = items
                            .iter()
                            .filter(|i| i.label == *label || i.lookup == *label)
                            .collect();
                        if candidates.is_empty() {
                            return Err(format!(
                                "候选里没有 `{label}`（共 {} 条，前几条: {preview:?}）",
                                items.len()
                            ));
                        }
                        let hit = candidates.iter().any(|i| {
                            i.insert_text.contains(contains.as_str())
                                && snippet.is_none_or(|s| i.is_snippet == s)
                        });
                        if hit {
                            Ok(())
                        } else {
                            let actual: Vec<String> = candidates
                                .iter()
                                .map(|i| {
                                    format!(
                                        "{:?}（{}）",
                                        i.insert_text,
                                        if i.is_snippet { "snippet" } else { "plain" }
                                    )
                                })
                                .collect();
                            Err(format!(
                                "`{label}` 的插入文本应当包含 {contains:?}{}，实际: {actual:?}",
                                match snippet {
                                    Some(true) => "（snippet）",
                                    Some(false) => "（plain）",
                                    None => "",
                                }
                            ))
                        }
                    }
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
                // `id#n`：同一个 id 的第 n 个（从 1 数），比如上移到更远的父类
                let (id_text, nth) = match id.split_once('#') {
                    Some((base, n)) => (base, n.parse::<usize>().unwrap_or(1).max(1)),
                    None => (id.as_str(), 1),
                };
                let Some(assist) = assists.iter().filter(|a| a.id == id_text).nth(nth - 1) else {
                    let offered: Vec<&str> = assists.iter().map(|a| a.id).collect();
                    return Err(format!("这里没有提供 `{id}`，提供的是: {offered:?}"));
                };
                self.check_text(&self.apply(&assist.change), checks)
            }
            Expectation::NoAssist { id, from, to } => {
                let range = self.range(from, to.as_ref())?;
                let assists = self.analysis.assists(range).map_err(|_| CANCELLED)?;
                let (id_text, nth) = match id.split_once('#') {
                    Some((base, n)) => (base, n.parse::<usize>().unwrap_or(1).max(1)),
                    None => (id.as_str(), 1),
                };
                if assists.iter().filter(|a| a.id == id_text).count() >= nth {
                    Err(format!("这里不该提供 `{id}`"))
                } else {
                    Ok(())
                }
            }
            Expectation::Stub {
                class,
                checks,
                refs,
            } => {
                let binary_name = class.replace('.', "/");
                let stub = self
                    .analysis
                    .class_stub(&binary_name)
                    .map_err(|_| CANCELLED)?
                    .ok_or_else(|| format!("没有为 {class} 生成桩"))?;
                let parse = syntax::ast::CompilationUnit::parse(&stub);
                if let Some(error) = parse.errors().first() {
                    return Err(format!("桩有语法错误: {error:?}。实际:\n{stub}"));
                }
                // 桩是要在编辑器里打开的：单独拿出来看，里面用到的类型都得能解析
                let mut host = AnalysisHost::new();
                host.apply_change(Change {
                    project: Some(self.loaded.project.clone()),
                    class_path: Some(self.loaded.class_path.clone()),
                    files: vec![(
                        FileId(0),
                        Some(FileChange {
                            text: Arc::from(stub.as_str()),
                            name: base_db::file_stem("Stub.java"),
                            kind: FileKind::Workspace {
                                module: None,
                                root: SourceRootKind::Main,
                            },
                        }),
                    )],
                    ..Change::default()
                });
                let unresolved: Vec<String> = host
                    .analysis()
                    .diagnostics(FileId(0), &DiagnosticsConfig::default())
                    .map_err(|_| CANCELLED)?
                    .into_iter()
                    .filter(|d| matches!(d.code.as_str(), "unresolved-type" | "unresolved-import"))
                    .map(|d| d.message)
                    .collect();
                if !unresolved.is_empty() {
                    return Err(format!(
                        "桩里有解析不了的类型: {unresolved:?}。实际:\n{stub}"
                    ));
                }
                self.check_text(&stub, checks)?;
                for (needle, expected) in refs {
                    self.check_stub_refs(&stub, needle, expected)?;
                }
                Ok(())
            }
            Expectation::Formatted { file, expected } => {
                let text = &self.scenario.files[*file].text;
                let edits = self
                    .analysis
                    .format(FileId(*file as u32), None, &ide::FormatOptions::default())
                    .map_err(|_| CANCELLED)?;
                let actual =
                    crate::format_check::apply(text, edits).ok_or("格式化给出的修改互相重叠")?;
                let (actual, expected) = (actual.trim_end(), expected.trim_end());
                if actual == expected {
                    return Ok(());
                }
                let line = actual
                    .lines()
                    .zip(expected.lines())
                    .position(|(a, e)| a != e)
                    .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()));
                Err(format!(
                    "格式化结果和预期不一样，从第 {} 行开始:\n  预期: {:?}\n  实际: {:?}\n完整结果:\n{actual}",
                    line + 1,
                    expected.lines().nth(line).unwrap_or("<没有这一行>"),
                    actual.lines().nth(line).unwrap_or("<没有这一行>"),
                ))
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
    let loaded = match env.load_scenario(scenario) {
        Ok(loaded) => loaded,
        Err(err) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                category: scenario.category.clone(),
                checks: scenario.expectations.len(),
                failures: vec![err],
                xfail: scenario.xfail.clone(),
            }
        }
    };
    let checker = Checker {
        scenario,
        loaded: &loaded,
        analysis: loaded.host.analysis(),
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
