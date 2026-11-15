//! 工程模型的导入：把 Maven / Gradle / 手动配置统一成 `base_db::ProjectGraph`。

mod eclipse;
mod gradle;
mod jdk;
mod maven;
mod xml;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base_db::{
    Dependency, DependencyTarget, LibraryData, LibraryId, ModuleData, ProjectGraph, Scope,
    SourceRoot, SourceRootKind,
};
use classfile::{ClassContainer, ClassPath, JarContainer};
use serde::Deserialize;

pub use crate::{
    gradle::cache_file as gradle_cache_file,
    jdk::{discover_jdk, jdk_containers, parse_feature_version},
};

pub const CONFIG_FILE_NAME: &str = "java-analyzer.toml";

/// 工作区根目录下 `java-analyzer.toml` 的内容。
///
/// ```toml
/// [jdk]
/// home = "/path/to/jdk"
///
/// [[module]]
/// name = "app"
/// root = "."
/// sources = ["src/main/java"]
/// test_sources = ["src/test/java"]
/// classpath = ["lib/*.jar", "/abs/path/dep.jar"]
/// language_level = 17
/// preview = false
/// ```
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(default)]
pub struct ManualConfig {
    pub jdk: Option<JdkConfig>,
    #[serde(rename = "module")]
    pub modules: Vec<ManualModule>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct JdkConfig {
    pub home: PathBuf,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(default)]
pub struct ManualModule {
    pub name: Option<String>,
    pub root: Option<PathBuf>,
    pub sources: Vec<PathBuf>,
    pub test_sources: Vec<PathBuf>,
    pub classpath: Vec<String>,
    pub language_level: Option<u32>,
    /// 开启预览特性（相当于 `javac --enable-preview`）
    pub preview: bool,
}

/// 来自编辑器配置的导入选项。
#[derive(Debug, Clone)]
pub struct LoadOptions {
    pub jdk_home: Option<PathBuf>,
    /// 本地仓库缺件时是否调用 `mvn` 兜底
    pub maven_use_cli: bool,
    /// 是否允许调用 Gradle（关闭后只使用缓存的导入结果）
    pub gradle_allow_run: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions {
            jdk_home: None,
            maven_use_cli: true,
            gradle_allow_run: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    Manual,
    Maven,
    Gradle,
    /// 没有识别出构建系统，按目录约定猜的源码根
    Plain,
}

#[derive(Debug)]
pub struct LoadedProject {
    pub kind: ProjectKind,
    pub graph: ProjectGraph,
    /// 导入过程中需要告诉用户的问题（缺失的依赖、失败的命令 …）
    pub messages: Vec<String>,
    /// 这些文件变化后需要重新导入
    pub build_files: Vec<PathBuf>,
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// 展开 classpath 条目：jar 文件、目录（取其中的 jar）、`dir/*` 或 `dir/*.jar`。
fn expand_classpath_entry(base: &Path, entry: &str) -> Vec<PathBuf> {
    let (dir, wildcard) = match entry
        .strip_suffix("/*.jar")
        .or_else(|| entry.strip_suffix("/*"))
    {
        Some(dir) => (absolutize(base, Path::new(dir)), true),
        None => (absolutize(base, Path::new(entry)), false),
    };
    if wildcard || dir.is_dir() {
        let mut jars: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|ext| ext == "jar"))
                    .collect()
            })
            .unwrap_or_default();
        jars.sort();
        jars
    } else if dir.is_file() {
        vec![dir]
    } else {
        Vec::new()
    }
}

fn read_manual_config(root: &Path, messages: &mut Vec<String>) -> ManualConfig {
    let path = root.join(CONFIG_FILE_NAME);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return ManualConfig::default();
    };
    match toml::from_str(&content) {
        Ok(config) => config,
        Err(err) => {
            messages.push(base_db::tr!(
                "{} 解析失败: {err}",
                "Failed to parse {}: {err}",
                path.display()
            ));
            ManualConfig::default()
        }
    }
}

fn manual_graph(root: &Path, config: &ManualConfig, messages: &mut Vec<String>) -> ProjectGraph {
    let mut graph = ProjectGraph::default();
    for (idx, module) in config.modules.iter().enumerate() {
        let module_root = absolutize(root, module.root.as_deref().unwrap_or(Path::new(".")));
        let mut source_roots = Vec::new();
        for (dirs, kind) in [
            (&module.sources, SourceRootKind::Main),
            (&module.test_sources, SourceRootKind::Test),
        ] {
            for dir in dirs {
                let path = absolutize(&module_root, dir);
                if path.is_dir() {
                    source_roots.push(SourceRoot { path, kind });
                } else {
                    messages.push(base_db::tr!(
                        "源码目录不存在: {}",
                        "Source directory does not exist: {}",
                        path.display()
                    ));
                }
            }
        }
        let mut dependencies = Vec::new();
        for entry in &module.classpath {
            let jars = expand_classpath_entry(&module_root, entry);
            if jars.is_empty() {
                messages.push(base_db::tr!(
                    "classpath 条目没有匹配到任何 jar: {entry}",
                    "Classpath entry matches no jar: {entry}"
                ));
            }
            for jar in jars {
                let id = match graph.libraries.iter().position(|lib| lib.jar == jar) {
                    Some(i) => LibraryId(i as u32),
                    None => {
                        let name = jar
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let sources = jar
                            .to_str()
                            .and_then(|s| s.strip_suffix(".jar"))
                            .map(|stem| PathBuf::from(format!("{stem}-sources.jar")))
                            .filter(|p| p.is_file());
                        graph.libraries.push(LibraryData { name, jar, sources });
                        LibraryId((graph.libraries.len() - 1) as u32)
                    }
                };
                dependencies.push(Dependency {
                    target: DependencyTarget::Library(id),
                    scope: Scope::Compile,
                });
            }
        }
        graph.modules.push(ModuleData {
            name: module
                .name
                .clone()
                .unwrap_or_else(|| format!("module{idx}")),
            root: module_root,
            source_roots,
            language_level: module.language_level,
            api_release: module.language_level,
            preview: module.preview,
            dependencies,
        });
    }
    graph
}

/// 没有构建文件时按目录约定猜源码根。
fn plain_graph(root: &Path) -> ProjectGraph {
    let conventional = [
        ("src/main/java", SourceRootKind::Main),
        ("src/test/java", SourceRootKind::Test),
    ];
    let mut source_roots: Vec<SourceRoot> = conventional
        .iter()
        .map(|(dir, kind)| SourceRoot {
            path: root.join(dir),
            kind: *kind,
        })
        .filter(|r| r.path.is_dir())
        .collect();
    if source_roots.is_empty() {
        let src = root.join("src");
        let path = if src.is_dir() {
            src
        } else {
            root.to_path_buf()
        };
        source_roots.push(SourceRoot {
            path,
            kind: SourceRootKind::Main,
        });
    }
    ProjectGraph {
        modules: vec![ModuleData {
            name: root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "root".to_owned()),
            root: root.to_path_buf(),
            source_roots,
            language_level: None,
            api_release: None,
            preview: false,
            dependencies: Vec::new(),
        }],
        libraries: Vec::new(),
        jdk: None,
    }
}

/// 导入 `root` 处的工程。优先级：手动配置里的模块 → Maven → Gradle → 目录约定。
pub fn load_project(root: &Path, options: &LoadOptions) -> LoadedProject {
    let mut messages = Vec::new();
    let config = read_manual_config(root, &mut messages);
    let mut build_files = vec![root.join(CONFIG_FILE_NAME)];

    let pom = root.join("pom.xml");
    let (kind, mut graph) = if !config.modules.is_empty() {
        (
            ProjectKind::Manual,
            manual_graph(root, &config, &mut messages),
        )
    } else if pom.is_file() {
        match maven::import(&pom, options.maven_use_cli) {
            Some(import) => {
                messages.extend(import.messages);
                for module in &import.modules {
                    build_files.push(module.root.join("pom.xml"));
                }
                let graph = ProjectGraph {
                    modules: import.modules,
                    libraries: import.libraries,
                    jdk: None,
                };
                (ProjectKind::Maven, graph)
            }
            None => {
                messages.push(base_db::tr!(
                    "无法解析 {}",
                    "Cannot parse {}",
                    pom.display()
                ));
                build_files.push(pom);
                (ProjectKind::Plain, plain_graph(root))
            }
        }
    } else if gradle::is_gradle_root(root) {
        build_files.extend(gradle::build_files(root));
        match gradle::import(root, options.gradle_allow_run) {
            Some(import) if !import.modules.is_empty() => {
                messages.extend(import.messages);
                let graph = ProjectGraph {
                    modules: import.modules,
                    libraries: import.libraries,
                    jdk: None,
                };
                (ProjectKind::Gradle, graph)
            }
            Some(import) => {
                messages.extend(import.messages);
                (ProjectKind::Plain, plain_graph(root))
            }
            None => {
                messages.push(base_db::tr!("Gradle 导入已被禁用且没有可用的缓存，按目录约定加载源码", "Gradle import is disabled and no cache is available; loading sources by directory convention"));
                (ProjectKind::Plain, plain_graph(root))
            }
        }
    } else {
        (ProjectKind::Plain, plain_graph(root))
    };

    let jdk_home = config
        .jdk
        .as_ref()
        .map(|j| j.home.clone())
        .or_else(|| options.jdk_home.clone());
    graph.jdk = discover_jdk(jdk_home.as_deref());
    if graph.jdk.is_none() {
        messages.push(
            base_db::tr!("没有找到 JDK：标准库的类型将无法解析。请设置 JAVA_HOME，或在 java-analyzer.toml 里配置 [jdk] home", "No JDK found: standard library types cannot be resolved. Set JAVA_HOME, or configure [jdk] home in java-analyzer.toml"),
        );
    }

    build_files.sort();
    build_files.dedup();
    LoadedProject {
        kind,
        graph,
        messages,
        build_files,
    }
}

/// 导入多个工作区根并合并成一张工程图。
///
/// 每个根各自按 [`load_project`] 的规则导入；合并时模块编号顺延，同一个 jar 只保留一份。
/// JDK 取第一个找到的。根之间不会自动建立模块依赖（它们是各自独立的构建）。
pub fn load_projects(roots: &[PathBuf], options: &LoadOptions) -> LoadedProject {
    let mut loaded = roots.iter().map(|root| load_project(root, options));
    let Some(mut merged) = loaded.next() else {
        return LoadedProject {
            kind: ProjectKind::Plain,
            graph: ProjectGraph::default(),
            messages: Vec::new(),
            build_files: Vec::new(),
        };
    };
    for project in loaded {
        merge_graph(&mut merged.graph, project.graph);
        for message in project.messages {
            if !merged.messages.contains(&message) {
                merged.messages.push(message);
            }
        }
        merged.build_files.extend(project.build_files);
    }
    merged.build_files.sort();
    merged.build_files.dedup();
    merged
}

fn merge_graph(into: &mut ProjectGraph, other: ProjectGraph) {
    let module_offset = into.modules.len() as u32;
    // 另一张图里的库编号 → 合并后的编号
    let library_map: Vec<LibraryId> = other
        .libraries
        .into_iter()
        .map(|lib| {
            match into
                .libraries
                .iter()
                .position(|existing| existing.jar == lib.jar)
            {
                Some(idx) => LibraryId(idx as u32),
                None => {
                    into.libraries.push(lib);
                    LibraryId((into.libraries.len() - 1) as u32)
                }
            }
        })
        .collect();
    for mut module in other.modules {
        // 嵌套的工作区根（一个根在另一个根里面）会把同一个模块导入两次
        if into
            .modules
            .iter()
            .any(|existing| existing.root == module.root)
        {
            continue;
        }
        for dependency in &mut module.dependencies {
            dependency.target = match dependency.target {
                DependencyTarget::Module(id) => {
                    DependencyTarget::Module(base_db::ModuleId(id.0 + module_offset))
                }
                DependencyTarget::ModuleTests(id) => {
                    DependencyTarget::ModuleTests(base_db::ModuleId(id.0 + module_offset))
                }
                DependencyTarget::Library(id) => {
                    DependencyTarget::Library(library_map[id.0 as usize])
                }
            };
        }
        into.modules.push(module);
    }
    if into.jdk.is_none() {
        into.jdk = other.jdk;
    }
}

/// 为工程图建立二进制类路径索引：依赖库在前，JDK 在后。
pub fn build_class_path(graph: &ProjectGraph) -> ClassPath {
    let mut containers: Vec<Arc<dyn ClassContainer>> = graph
        .libraries
        .iter()
        .map(|lib| Arc::new(JarContainer::new(lib.jar.clone())) as Arc<dyn ClassContainer>)
        .collect();
    if let Some(jdk) = &graph.jdk {
        containers.extend(jdk_containers(jdk, graph.api_release()));
    }
    ClassPath::build(containers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manual_config() {
        let config: ManualConfig = toml::from_str(
            r#"
            [jdk]
            home = "/opt/jdk"

            [[module]]
            name = "app"
            sources = ["src"]
            classpath = ["lib/*.jar"]
            language_level = 17
            preview = true
            "#,
        )
        .unwrap();
        assert_eq!(config.jdk.unwrap().home, PathBuf::from("/opt/jdk"));
        assert_eq!(config.modules.len(), 1);
        assert_eq!(config.modules[0].language_level, Some(17));
        assert!(config.modules[0].preview);
        assert!(config.modules[0].test_sources.is_empty());
    }

    #[test]
    fn empty_config_is_valid() {
        let config: ManualConfig = toml::from_str("").unwrap();
        assert!(config.jdk.is_none());
        assert!(config.modules.is_empty());
    }

    #[test]
    fn merge_graph_renumbers_modules_and_dedups_libraries() {
        let lib = |jar: &str| LibraryData {
            name: jar.to_owned(),
            jar: PathBuf::from(jar),
            sources: None,
        };
        let module = |root: &str, dependencies: Vec<Dependency>| ModuleData {
            name: root.to_owned(),
            root: PathBuf::from(root),
            source_roots: Vec::new(),
            language_level: None,
            api_release: None,
            preview: false,
            dependencies,
        };
        let dep = |target| Dependency {
            target,
            scope: Scope::Compile,
        };

        let mut first = ProjectGraph {
            modules: vec![module(
                "/a",
                vec![dep(DependencyTarget::Library(LibraryId(0)))],
            )],
            libraries: vec![lib("/m2/guava.jar")],
            jdk: None,
        };
        let second = ProjectGraph {
            modules: vec![
                module(
                    "/b/core",
                    vec![dep(DependencyTarget::Library(LibraryId(1)))],
                ),
                module(
                    "/b/app",
                    vec![
                        dep(DependencyTarget::Module(base_db::ModuleId(0))),
                        dep(DependencyTarget::Library(LibraryId(0))),
                    ],
                ),
            ],
            libraries: vec![lib("/m2/slf4j.jar"), lib("/m2/guava.jar")],
            jdk: None,
        };
        merge_graph(&mut first, second);

        assert_eq!(first.modules.len(), 3);
        assert_eq!(first.libraries.len(), 2, "guava 只保留一份");
        // /b/core 依赖 guava：合并后是 0 号库
        assert_eq!(
            first.modules[1].dependencies[0].target,
            DependencyTarget::Library(LibraryId(0))
        );
        // /b/app 依赖 /b/core（顺延为 1 号模块）和 slf4j（1 号库）
        assert_eq!(
            first.modules[2].dependencies[0].target,
            DependencyTarget::Module(base_db::ModuleId(1))
        );
        assert_eq!(
            first.modules[2].dependencies[1].target,
            DependencyTarget::Library(LibraryId(1))
        );
    }
}
