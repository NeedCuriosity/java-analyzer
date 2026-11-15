//! Maven 工程导入：直接解析 POM，在本地仓库里解析依赖，不启动 JVM。
//!
//! 实现的部分：父 POM 继承、属性插值、`dependencyManagement`、BOM import、scope、
//! exclusion、最近者优先的版本仲裁、`activeByDefault` 的 profile、多模块 reactor。
//!
//! 没有实现的部分（遇到时依赖会缺失，由调用方决定是否回退到 `mvn` 命令行）：
//! 版本区间的完整语义、按条件激活的 profile、远程仓库下载、改变源码目录的插件。

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use base_db::{
    Dependency, DependencyTarget, LibraryData, LibraryId, ModuleData, ModuleId, Scope, SourceRoot,
    SourceRootKind,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    jdk::parse_feature_version,
    xml::{self, XmlNode},
};

#[derive(Default)]
pub struct MavenImport {
    pub modules: Vec<ModuleData>,
    pub libraries: Vec<LibraryData>,
    pub messages: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Coord {
    group: String,
    artifact: String,
}

#[derive(Clone, Debug, Default)]
struct RawDep {
    group: String,
    artifact: String,
    version: Option<String>,
    scope: Option<String>,
    kind: Option<String>,
    classifier: Option<String>,
    optional: bool,
    exclusions: Vec<Coord>,
}

impl RawDep {
    fn coord(&self) -> Coord {
        Coord {
            group: self.group.clone(),
            artifact: self.artifact.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RawParent {
    group: String,
    artifact: String,
    version: String,
    relative_path: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct RawPom {
    parent: Option<RawParent>,
    group: Option<String>,
    artifact: String,
    version: Option<String>,
    properties: Vec<(String, String)>,
    modules: Vec<String>,
    deps: Vec<RawDep>,
    managed: Vec<RawDep>,
    source_dir: Option<String>,
    test_source_dir: Option<String>,
    compiler_level: Option<String>,
}

fn text(node: &XmlNode, name: &str) -> Option<String> {
    node.child_text(name).map(str::to_owned)
}

fn read_dep(node: &XmlNode) -> Option<RawDep> {
    Some(RawDep {
        group: text(node, "groupId")?,
        artifact: text(node, "artifactId")?,
        version: text(node, "version"),
        scope: text(node, "scope"),
        kind: text(node, "type"),
        classifier: text(node, "classifier"),
        optional: node.child_text("optional") == Some("true"),
        exclusions: node
            .child("exclusions")
            .map(|e| {
                e.children_named("exclusion")
                    .filter_map(|x| {
                        Some(Coord {
                            group: text(x, "groupId")?,
                            artifact: text(x, "artifactId")?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// 读取 `<project>` 或 `<profile>` 里通用的那几节。
fn read_common(node: &XmlNode, pom: &mut RawPom) {
    if let Some(properties) = node.child("properties") {
        for prop in &properties.children {
            pom.properties.push((prop.name.clone(), prop.text.clone()));
        }
    }
    if let Some(modules) = node.child("modules") {
        pom.modules
            .extend(modules.children_named("module").map(|m| m.text.clone()));
    }
    if let Some(deps) = node.child("dependencies") {
        pom.deps
            .extend(deps.children_named("dependency").filter_map(read_dep));
    }
    if let Some(deps) = node.path(&["dependencyManagement", "dependencies"]) {
        pom.managed
            .extend(deps.children_named("dependency").filter_map(read_dep));
    }
    if let Some(build) = node.child("build") {
        if let Some(dir) = text(build, "sourceDirectory") {
            pom.source_dir = Some(dir);
        }
        if let Some(dir) = text(build, "testSourceDirectory") {
            pom.test_source_dir = Some(dir);
        }
        let plugin_lists = [
            build.child("plugins"),
            build.path(&["pluginManagement", "plugins"]),
        ];
        for plugins in plugin_lists.into_iter().flatten() {
            for plugin in plugins.children_named("plugin") {
                if plugin.child_text("artifactId") != Some("maven-compiler-plugin") {
                    continue;
                }
                if let Some(config) = plugin.child("configuration") {
                    let level = text(config, "release").or_else(|| text(config, "source"));
                    if level.is_some() {
                        pom.compiler_level = level;
                    }
                }
            }
        }
    }
}

fn read_raw_pom(path: &Path) -> Option<RawPom> {
    let content = std::fs::read_to_string(path).ok()?;
    let root = xml::parse(&content)?;
    if root.name != "project" {
        return None;
    }
    let mut pom = RawPom {
        group: text(&root, "groupId"),
        artifact: text(&root, "artifactId")?,
        version: text(&root, "version"),
        ..RawPom::default()
    };
    if let Some(parent) = root.child("parent") {
        pom.parent = Some(RawParent {
            group: text(parent, "groupId")?,
            artifact: text(parent, "artifactId")?,
            version: text(parent, "version").unwrap_or_default(),
            relative_path: parent.child("relativePath").map(|n| n.text.clone()),
        });
    }
    read_common(&root, &mut pom);
    if let Some(profiles) = root.child("profiles") {
        for profile in profiles.children_named("profile") {
            let active = profile
                .path(&["activation", "activeByDefault"])
                .is_some_and(|n| n.text == "true");
            if active {
                read_common(profile, &mut pom);
            }
        }
    }
    Some(pom)
}

#[derive(Clone, Debug, Default)]
struct Managed {
    version: Option<String>,
    scope: Option<String>,
    exclusions: Vec<Coord>,
}

type ManagedKey = (String, String, Option<String>);

/// 继承、插值之后的有效 POM。
#[derive(Clone, Debug, Default)]
struct Effective {
    group: String,
    artifact: String,
    version: String,
    dir: PathBuf,
    properties: FxHashMap<String, String>,
    deps: Vec<RawDep>,
    managed: FxHashMap<ManagedKey, Managed>,
    source_dir: Option<String>,
    test_source_dir: Option<String>,
    level: Option<u32>,
}

fn interpolate(value: &str, properties: &FxHashMap<String, String>) -> String {
    let mut current = value.to_owned();
    // 属性可以互相引用；限制轮数以防循环定义
    for _ in 0..10 {
        let Some(start) = current.find("${") else {
            break;
        };
        let Some(len) = current[start..].find('}') else {
            break;
        };
        let key = &current[start + 2..start + len];
        let Some(replacement) = properties.get(key) else {
            break;
        };
        current = format!(
            "{}{}{}",
            &current[..start],
            replacement,
            &current[start + len + 1..]
        );
    }
    current
}

fn interpolate_dep(dep: &RawDep, properties: &FxHashMap<String, String>) -> RawDep {
    let i = |s: &String| interpolate(s, properties);
    RawDep {
        group: i(&dep.group),
        artifact: i(&dep.artifact),
        version: dep.version.as_ref().map(i),
        scope: dep.scope.as_ref().map(i),
        kind: dep.kind.clone(),
        classifier: dep.classifier.as_ref().map(i),
        optional: dep.optional,
        exclusions: dep.exclusions.clone(),
    }
}

/// 版本区间 `[1.0,2.0)` 没有完整实现：取其中出现的第一个版本号。
fn normalize_version(version: &str) -> String {
    if version.starts_with('[') || version.starts_with('(') {
        version
            .trim_matches(|c| matches!(c, '[' | ']' | '(' | ')'))
            .split(',')
            .map(str::trim)
            .find(|part| !part.is_empty())
            .unwrap_or_default()
            .to_owned()
    } else {
        version.to_owned()
    }
}

fn parse_scope(scope: Option<&str>) -> Scope {
    match scope {
        Some("test") => Scope::Test,
        Some("provided") | Some("system") => Scope::Provided,
        Some("runtime") => Scope::Runtime,
        _ => Scope::Compile,
    }
}

/// 传递依赖的 scope 合成规则。返回 `None` 表示该传递依赖不进入 classpath。
fn combine_scope(parent: Scope, child: Scope) -> Option<Scope> {
    if matches!(child, Scope::Test | Scope::Provided) {
        return None;
    }
    Some(match (parent, child) {
        (Scope::Test, _) => Scope::Test,
        (Scope::Provided, _) => Scope::Provided,
        (Scope::Runtime, _) => Scope::Runtime,
        (Scope::Compile, Scope::Runtime) => Scope::Runtime,
        (Scope::Compile, _) => Scope::Compile,
    })
}

fn is_excluded(exclusions: &[Coord], coord: &Coord) -> bool {
    exclusions.iter().any(|e| {
        (e.group == "*" || e.group == coord.group)
            && (e.artifact == "*" || e.artifact == coord.artifact)
    })
}

fn local_repository() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let default = home.join(".m2/repository");
    let Ok(settings) = std::fs::read_to_string(home.join(".m2/settings.xml")) else {
        return default;
    };
    xml::parse(&settings)
        .and_then(|root| root.child_text("localRepository").map(PathBuf::from))
        .unwrap_or(default)
}

struct Resolver {
    repo: PathBuf,
    /// reactor 里的模块：坐标 → pom.xml 路径
    reactor: FxHashMap<Coord, PathBuf>,
    cache: FxHashMap<PathBuf, Option<Arc<Effective>>>,
    messages: Vec<String>,
    missing: FxHashSet<String>,
}

impl Resolver {
    fn artifact_dir(&self, group: &str, artifact: &str, version: &str) -> PathBuf {
        self.repo
            .join(group.replace('.', "/"))
            .join(artifact)
            .join(version)
    }

    fn repo_pom(&self, group: &str, artifact: &str, version: &str) -> PathBuf {
        self.artifact_dir(group, artifact, version)
            .join(format!("{artifact}-{version}.pom"))
    }

    fn pom_for(&self, coord: &Coord, version: &str) -> PathBuf {
        match self.reactor.get(coord) {
            Some(path) => path.clone(),
            None => self.repo_pom(&coord.group, &coord.artifact, version),
        }
    }

    fn effective(&mut self, pom_path: &Path, depth: u32) -> Option<Arc<Effective>> {
        if let Some(cached) = self.cache.get(pom_path) {
            return cached.clone();
        }
        // 先占位，遇到循环的父子关系时直接返回 None
        self.cache.insert(pom_path.to_path_buf(), None);
        let result = self.compute_effective(pom_path, depth).map(Arc::new);
        self.cache.insert(pom_path.to_path_buf(), result.clone());
        result
    }

    fn compute_effective(&mut self, pom_path: &Path, depth: u32) -> Option<Effective> {
        if depth > 32 {
            return None;
        }
        let raw = read_raw_pom(pom_path)?;
        let dir = pom_path.parent().unwrap_or(Path::new(".")).to_path_buf();

        let parent = raw.parent.as_ref().and_then(|p| {
            let relative = p.relative_path.as_deref().unwrap_or("../pom.xml");
            let mut local = dir.join(relative);
            if local.is_dir() {
                local = local.join("pom.xml");
            }
            let local_matches = !relative.is_empty()
                && local.is_file()
                && read_raw_pom(&local).is_some_and(|candidate| candidate.artifact == p.artifact);
            let path = if local_matches {
                local
            } else {
                self.repo_pom(&p.group, &p.artifact, &p.version)
            };
            let parent = self.effective(&path, depth + 1);
            if parent.is_none() {
                self.messages.push(base_db::tr!(
                    "找不到父 POM {}:{}:{}（{} 需要它）",
                    "Parent POM {}:{}:{} not found (required by {})",
                    p.group,
                    p.artifact,
                    p.version,
                    pom_path.display()
                ));
            }
            parent
        });

        let mut eff = Effective {
            dir,
            artifact: raw.artifact.clone(),
            ..Effective::default()
        };
        if let Some(parent) = &parent {
            eff.properties = parent.properties.clone();
            eff.managed = parent.managed.clone();
            eff.deps = parent.deps.clone();
            eff.source_dir = parent.source_dir.clone();
            eff.test_source_dir = parent.test_source_dir.clone();
            eff.level = parent.level;
        }
        eff.group = raw
            .group
            .clone()
            .or_else(|| raw.parent.as_ref().map(|p| p.group.clone()))
            .unwrap_or_default();
        eff.version = raw
            .version
            .clone()
            .or_else(|| raw.parent.as_ref().map(|p| p.version.clone()))
            .unwrap_or_default();

        for (key, value) in &raw.properties {
            eff.properties.insert(key.clone(), value.clone());
        }
        eff.version = interpolate(&eff.version, &eff.properties);
        eff.group = interpolate(&eff.group, &eff.properties);
        for prefix in ["project", "pom"] {
            eff.properties
                .insert(format!("{prefix}.groupId"), eff.group.clone());
            eff.properties
                .insert(format!("{prefix}.artifactId"), eff.artifact.clone());
            eff.properties
                .insert(format!("{prefix}.version"), eff.version.clone());
        }
        if let Some(p) = &raw.parent {
            eff.properties
                .insert("project.parent.version".into(), p.version.clone());
            eff.properties
                .insert("project.parent.groupId".into(), p.group.clone());
        }
        eff.properties.insert(
            "project.basedir".into(),
            eff.dir.to_string_lossy().into_owned(),
        );

        // 自己声明的 dependencyManagement 覆盖继承来的；BOM 只补缺
        let mut boms: Vec<RawDep> = Vec::new();
        for dep in &raw.managed {
            let dep = interpolate_dep(dep, &eff.properties);
            if dep.scope.as_deref() == Some("import") && dep.kind.as_deref() == Some("pom") {
                boms.push(dep);
                continue;
            }
            eff.managed.insert(
                (
                    dep.group.clone(),
                    dep.artifact.clone(),
                    dep.classifier.clone(),
                ),
                Managed {
                    version: dep.version,
                    scope: dep.scope,
                    exclusions: dep.exclusions,
                },
            );
        }
        for bom in boms {
            let Some(version) = bom.version.as_deref().map(normalize_version) else {
                continue;
            };
            let path = self.pom_for(&bom.coord(), &version);
            match self.effective(&path, depth + 1) {
                Some(imported) => {
                    for (key, managed) in &imported.managed {
                        eff.managed
                            .entry(key.clone())
                            .or_insert_with(|| managed.clone());
                    }
                }
                None => self.note_missing(&bom.group, &bom.artifact, &version, "pom"),
            }
        }

        for dep in &raw.deps {
            eff.deps.push(interpolate_dep(dep, &eff.properties));
        }
        if let Some(dir) = &raw.source_dir {
            eff.source_dir = Some(interpolate(dir, &eff.properties));
        }
        if let Some(dir) = &raw.test_source_dir {
            eff.test_source_dir = Some(interpolate(dir, &eff.properties));
        }

        let level = raw
            .compiler_level
            .as_deref()
            .map(|l| interpolate(l, &eff.properties))
            .or_else(|| eff.properties.get("maven.compiler.release").cloned())
            .or_else(|| eff.properties.get("maven.compiler.source").cloned())
            .or_else(|| eff.properties.get("java.version").cloned())
            .and_then(|l| parse_feature_version(&interpolate(&l, &eff.properties)));
        if level.is_some() {
            eff.level = level;
        }
        Some(eff)
    }

    fn note_missing(&mut self, group: &str, artifact: &str, version: &str, kind: &str) {
        let key = format!("{group}:{artifact}:{version} ({kind})");
        if self.missing.insert(key.clone()) {
            self.messages.push(base_db::tr!(
                "本地仓库里没有 {key}",
                "{key} is not in the local repository"
            ));
        }
    }

    /// 解析一个模块的传递依赖闭包（广度优先，最近者优先）。
    fn resolve_dependencies(&mut self, root: &Effective) -> Vec<ResolvedDep> {
        struct Pending {
            dep: RawDep,
            version: String,
            scope: Scope,
            exclusions: Arc<Vec<Coord>>,
        }

        let mut result = Vec::new();
        let mut seen: FxHashSet<ManagedKey> = FxHashSet::default();
        let mut queue: VecDeque<Pending> = VecDeque::new();

        for dep in &root.deps {
            let key = (
                dep.group.clone(),
                dep.artifact.clone(),
                dep.classifier.clone(),
            );
            let managed = root.managed.get(&key);
            let Some(version) = dep
                .version
                .clone()
                .or_else(|| managed.and_then(|m| m.version.clone()))
                .map(|v| normalize_version(&interpolate(&v, &root.properties)))
            else {
                self.messages.push(base_db::tr!(
                    "{}:{} 没有版本号（{}）",
                    "{}:{} has no version ({})",
                    dep.group,
                    dep.artifact,
                    root.artifact
                ));
                continue;
            };
            let scope = parse_scope(
                dep.scope
                    .as_deref()
                    .or_else(|| managed.and_then(|m| m.scope.as_deref())),
            );
            let mut exclusions = dep.exclusions.clone();
            if let Some(managed) = managed {
                exclusions.extend(managed.exclusions.iter().cloned());
            }
            queue.push_back(Pending {
                dep: dep.clone(),
                version,
                scope,
                exclusions: Arc::new(exclusions),
            });
        }

        while let Some(pending) = queue.pop_front() {
            let coord = pending.dep.coord();
            let key = (
                coord.group.clone(),
                coord.artifact.clone(),
                pending.dep.classifier.clone(),
            );
            if !seen.insert(key) {
                continue;
            }

            let kind = pending.dep.kind.as_deref().unwrap_or("jar");
            if let Some(pom) = self.reactor.get(&coord).cloned() {
                result.push(ResolvedDep::Module {
                    pom,
                    scope: pending.scope,
                });
            } else if kind != "pom" {
                let dir = self.artifact_dir(&coord.group, &coord.artifact, &pending.version);
                let classifier = match (&pending.dep.classifier, kind) {
                    (Some(c), _) => format!("-{c}"),
                    (None, "test-jar") => "-tests".to_owned(),
                    _ => String::new(),
                };
                let stem = format!("{}-{}", coord.artifact, pending.version);
                let jar = dir.join(format!("{stem}{classifier}.jar"));
                if jar.is_file() {
                    let sources = dir.join(format!("{stem}-sources.jar"));
                    result.push(ResolvedDep::Library {
                        name: format!("{}:{}:{}", coord.group, coord.artifact, pending.version),
                        jar,
                        sources: sources.is_file().then_some(sources),
                        scope: pending.scope,
                    });
                } else {
                    self.note_missing(&coord.group, &coord.artifact, &pending.version, "jar");
                }
            }

            // 传递依赖
            let pom_path = self.pom_for(&coord, &pending.version);
            let Some(dep_pom) = self.effective(&pom_path, 0) else {
                continue;
            };
            for transitive in &dep_pom.deps {
                if transitive.optional {
                    continue;
                }
                let t_coord = transitive.coord();
                if is_excluded(&pending.exclusions, &t_coord) {
                    continue;
                }
                let key = (
                    transitive.group.clone(),
                    transitive.artifact.clone(),
                    transitive.classifier.clone(),
                );
                let own_managed = dep_pom.managed.get(&key);
                let root_managed = root.managed.get(&key);
                let child_scope = parse_scope(
                    transitive
                        .scope
                        .as_deref()
                        .or_else(|| own_managed.and_then(|m| m.scope.as_deref())),
                );
                let Some(scope) = combine_scope(pending.scope, child_scope) else {
                    continue;
                };
                // 根模块的 dependencyManagement 对传递依赖的版本同样生效
                let version = root_managed
                    .and_then(|m| m.version.clone())
                    .map(|v| interpolate(&v, &root.properties))
                    .or_else(|| transitive.version.clone())
                    .or_else(|| own_managed.and_then(|m| m.version.clone()))
                    .map(|v| normalize_version(&interpolate(&v, &dep_pom.properties)));
                let Some(version) = version else { continue };

                let mut exclusions = (*pending.exclusions).clone();
                exclusions.extend(transitive.exclusions.iter().cloned());
                if let Some(managed) = root_managed {
                    exclusions.extend(managed.exclusions.iter().cloned());
                }
                queue.push_back(Pending {
                    dep: transitive.clone(),
                    version,
                    scope,
                    exclusions: Arc::new(exclusions),
                });
            }
        }
        result
    }
}

enum ResolvedDep {
    Module {
        pom: PathBuf,
        scope: Scope,
    },
    Library {
        name: String,
        jar: PathBuf,
        sources: Option<PathBuf>,
        scope: Scope,
    },
}

/// 收集 reactor 里所有模块的 pom.xml（根在前）。
fn collect_reactor(pom_path: &Path, out: &mut Vec<PathBuf>) {
    if out.iter().any(|p| p == pom_path) || out.len() > 2000 {
        return;
    }
    let Some(raw) = read_raw_pom(pom_path) else {
        return;
    };
    out.push(pom_path.to_path_buf());
    let dir = pom_path.parent().unwrap_or(Path::new("."));
    for module in &raw.modules {
        let mut path = dir.join(module);
        if path.is_dir() {
            path = path.join("pom.xml");
        }
        if path.is_file() {
            collect_reactor(&path, out);
        }
    }
}

fn source_roots(eff: &Effective) -> Vec<SourceRoot> {
    let resolve = |configured: &Option<String>, default: &str| {
        let path = PathBuf::from(configured.as_deref().unwrap_or(default));
        if path.is_absolute() {
            path
        } else {
            eff.dir.join(path)
        }
    };
    let mut roots = vec![
        SourceRoot {
            path: resolve(&eff.source_dir, "src/main/java"),
            kind: SourceRootKind::Main,
        },
        SourceRoot {
            path: resolve(&eff.test_source_dir, "src/test/java"),
            kind: SourceRootKind::Test,
        },
    ];
    // 注解处理器等生成的源码
    for generated in ["target/generated-sources", "target/generated-test-sources"] {
        if let Ok(entries) = std::fs::read_dir(eff.dir.join(generated)) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    roots.push(SourceRoot {
                        path: entry.path(),
                        kind: SourceRootKind::Generated,
                    });
                }
            }
        }
    }
    roots.retain(|root| root.path.is_dir());
    roots
}

/// 让 Maven 自己算 classpath（会联网下载缺失的依赖）。
fn cli_classpath(module_dir: &Path) -> Option<Vec<PathBuf>> {
    let wrapper = module_dir
        .ancestors()
        .map(|d| d.join("mvnw"))
        .find(|p| p.is_file());
    let program = wrapper.unwrap_or_else(|| PathBuf::from("mvn"));
    let out_file =
        std::env::temp_dir().join(format!("java-analyzer-cp-{}.txt", std::process::id()));
    let status = Command::new(program)
        .current_dir(module_dir)
        .arg("-q")
        .arg("dependency:build-classpath")
        .arg("-Dmdep.includeScope=test")
        .arg(format!("-Dmdep.outputFile={}", out_file.display()))
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let content = std::fs::read_to_string(&out_file).ok()?;
    let _ = std::fs::remove_file(&out_file);
    let separator = if cfg!(windows) { ';' } else { ':' };
    Some(
        content
            .trim()
            .split(separator)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .collect(),
    )
}

/// 导入以 `root_pom` 为根的 Maven 工程。`use_cli` 为真时，本地仓库缺件的模块回退到 `mvn`。
pub fn import(root_pom: &Path, use_cli: bool) -> Option<MavenImport> {
    let mut poms = Vec::new();
    collect_reactor(root_pom, &mut poms);
    if poms.is_empty() {
        return None;
    }

    let mut resolver = Resolver {
        repo: local_repository(),
        reactor: FxHashMap::default(),
        cache: FxHashMap::default(),
        messages: Vec::new(),
        missing: FxHashSet::default(),
    };
    for pom in &poms {
        if let Some(raw) = read_raw_pom(pom) {
            let group = raw
                .group
                .or_else(|| raw.parent.map(|p| p.group))
                .unwrap_or_default();
            resolver.reactor.insert(
                Coord {
                    group,
                    artifact: raw.artifact,
                },
                pom.clone(),
            );
        }
    }

    let module_index: FxHashMap<&Path, ModuleId> = poms
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_path(), ModuleId(i as u32)))
        .collect();

    let mut import = MavenImport::default();
    let mut library_ids: FxHashMap<PathBuf, LibraryId> = FxHashMap::default();
    let mut add_library = |import: &mut MavenImport, name: String, jar: PathBuf, sources| {
        *library_ids.entry(jar.clone()).or_insert_with(|| {
            import.libraries.push(LibraryData { name, jar, sources });
            LibraryId((import.libraries.len() - 1) as u32)
        })
    };

    for pom in &poms {
        let Some(eff) = resolver.effective(pom, 0) else {
            // 保持模块下标与 poms 对齐
            import.modules.push(ModuleData {
                name: pom.display().to_string(),
                root: pom.parent().unwrap_or(Path::new(".")).to_path_buf(),
                source_roots: Vec::new(),
                language_level: None,
                dependencies: Vec::new(),
            });
            continue;
        };
        let missing_before = resolver.missing.len();
        let mut dependencies = Vec::new();
        for dep in resolver.resolve_dependencies(&eff) {
            match dep {
                ResolvedDep::Module { pom, scope } => {
                    if let Some(id) = module_index.get(pom.as_path()) {
                        dependencies.push(Dependency {
                            target: DependencyTarget::Module(*id),
                            scope,
                        });
                    }
                }
                ResolvedDep::Library {
                    name,
                    jar,
                    sources,
                    scope,
                } => {
                    let id = add_library(&mut import, name, jar, sources);
                    dependencies.push(Dependency {
                        target: DependencyTarget::Library(id),
                        scope,
                    });
                }
            }
        }

        if use_cli && resolver.missing.len() > missing_before {
            match cli_classpath(&eff.dir) {
                Some(jars) => {
                    for jar in jars.into_iter().filter(|j| j.is_file()) {
                        let name = jar
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let id = add_library(&mut import, name, jar, None);
                        let dep = Dependency {
                            target: DependencyTarget::Library(id),
                            scope: Scope::Compile,
                        };
                        if !dependencies
                            .iter()
                            .any(|d: &Dependency| d.target == dep.target)
                        {
                            dependencies.push(dep);
                        }
                    }
                }
                None => resolver.messages.push(base_db::tr!(
                    "调用 mvn 解析 {} 的依赖失败",
                    "Running mvn to resolve the dependencies of {} failed",
                    eff.artifact
                )),
            }
        }

        import.modules.push(ModuleData {
            name: eff.artifact.clone(),
            root: eff.dir.clone(),
            source_roots: source_roots(&eff),
            language_level: eff.level,
            dependencies,
        });
    }

    import.messages = resolver.messages;
    Some(import)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation() {
        let mut props = FxHashMap::default();
        props.insert("a".to_owned(), "1".to_owned());
        props.insert("b".to_owned(), "${a}.2".to_owned());
        assert_eq!(interpolate("v${b}-x", &props), "v1.2-x");
        assert_eq!(interpolate("${missing}", &props), "${missing}");
        assert_eq!(interpolate("plain", &props), "plain");
    }

    #[test]
    fn scopes() {
        assert_eq!(
            combine_scope(Scope::Compile, Scope::Compile),
            Some(Scope::Compile)
        );
        assert_eq!(
            combine_scope(Scope::Compile, Scope::Runtime),
            Some(Scope::Runtime)
        );
        assert_eq!(
            combine_scope(Scope::Test, Scope::Compile),
            Some(Scope::Test)
        );
        assert_eq!(combine_scope(Scope::Compile, Scope::Test), None);
        assert_eq!(combine_scope(Scope::Compile, Scope::Provided), None);
    }

    #[test]
    fn version_ranges_and_exclusions() {
        assert_eq!(normalize_version("[1.2,2.0)"), "1.2");
        assert_eq!(normalize_version("1.2.3"), "1.2.3");
        let ex = vec![Coord {
            group: "g".into(),
            artifact: "*".into(),
        }];
        assert!(is_excluded(
            &ex,
            &Coord {
                group: "g".into(),
                artifact: "a".into()
            }
        ));
        assert!(!is_excluded(
            &ex,
            &Coord {
                group: "h".into(),
                artifact: "a".into()
            }
        ));
    }
}
