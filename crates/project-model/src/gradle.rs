//! Gradle 工程导入。
//!
//! Gradle 的构建脚本是可执行代码，无法静态解析。这里用 `--init-script` 注入一段脚本，
//! 在配置阶段结束时把每个子工程的 source set、编译 classpath 和语言级别写成 JSON。
//! 结果按"构建文件内容的哈希"缓存：构建文件没变时启动不需要调用 Gradle。

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
};

use base_db::{
    Dependency, DependencyTarget, LibraryData, LibraryId, ModuleData, ModuleId, Scope, SourceRoot,
    SourceRootKind,
};
use rustc_hash::FxHashMap;
use serde::Deserialize;

const INIT_SCRIPT: &str = r#"
// 由 java-analyzer 注入：导出工程模型，不执行任何构建任务。
// 每个子工程注册一个自己的任务，classpath 在任务执行时解析：Gradle 不允许在别的工程
// （或构建级的回调）里解析一个工程的配置。每个工程写一个 JSON 文件到输出目录。
def javaAnalyzerOutput = new File(System.getProperty('javaAnalyzer.output'))
allprojects { p ->
    p.afterEvaluate {
        def sourceSets = p.extensions.findByName('sourceSets')
        if (sourceSets == null) {
            return
        }
        def javaExtension = p.extensions.findByName('java')
        def compileTasks = p.tasks.withType(JavaCompile)
        def projectName = p.name
        def projectDir = p.projectDir.absolutePath
        def buildDir = p.layout.buildDirectory.get().asFile.absolutePath
        def fileName = (p.path == ':' ? 'root' : p.path.substring(1).replace(':', '.')) + '.json'
        p.tasks.register('javaAnalyzerModel') {
            doLast {
                def sets = []
                sourceSets.each { ss ->
                    def classpath = []
                    try {
                        ss.compileClasspath.files.each { classpath << it.absolutePath }
                    } catch (Throwable ignored) {
                    }
                    sets << [
                        name      : ss.name,
                        srcDirs   : ss.java.srcDirs.collect { it.absolutePath },
                        classpath : classpath,
                        outputDirs: ss.output.classesDirs.files.collect { it.absolutePath },
                    ]
                }
                def level = null
                try {
                    level = javaExtension?.sourceCompatibility?.majorVersion
                } catch (Throwable ignored) {
                }
                def preview = false
                def release = null
                try {
                    compileTasks.each { task ->
                        if (task.options.compilerArgs.contains('--enable-preview')) {
                            preview = true
                        }
                        def r = task.options.release.getOrNull()
                        if (r != null && (release == null || r > release)) {
                            release = r
                        }
                    }
                } catch (Throwable ignored) {
                }
                javaAnalyzerOutput.mkdirs()
                new File(javaAnalyzerOutput, fileName).text = groovy.json.JsonOutput.toJson([
                    name         : projectName,
                    dir          : projectDir,
                    buildDir     : buildDir,
                    languageLevel: level,
                    preview      : preview,
                    release      : release,
                    sourceSets   : sets,
                ])
            }
        }
    }
}
"#;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GradleSourceSet {
    name: String,
    #[serde(default)]
    src_dirs: Vec<PathBuf>,
    #[serde(default)]
    classpath: Vec<PathBuf>,
    #[serde(default)]
    output_dirs: Vec<PathBuf>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GradleProject {
    name: String,
    dir: PathBuf,
    build_dir: PathBuf,
    #[serde(default)]
    language_level: Option<String>,
    #[serde(default)]
    preview: bool,
    /// `JavaCompile.options.release`
    #[serde(default)]
    release: Option<u32>,
    #[serde(default)]
    source_sets: Vec<GradleSourceSet>,
}

#[derive(Default)]
pub struct GradleImport {
    pub modules: Vec<ModuleData>,
    pub libraries: Vec<LibraryData>,
    pub messages: Vec<String>,
}

const BUILD_FILE_NAMES: &[&str] = &[
    "settings.gradle",
    "settings.gradle.kts",
    "build.gradle",
    "build.gradle.kts",
    "gradle.properties",
];

pub fn is_gradle_root(dir: &Path) -> bool {
    [
        "settings.gradle",
        "settings.gradle.kts",
        "build.gradle",
        "build.gradle.kts",
    ]
    .iter()
    .any(|name| dir.join(name).is_file())
}

/// 所有可能影响工程模型的构建文件（根目录和两层以内的子目录，外加版本目录）。
pub fn build_files(root: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        for name in BUILD_FILE_NAMES {
            let path = dir.join(name);
            if path.is_file() {
                out.push(path);
            }
        }
        if depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() && !name.starts_with('.') && name != "build" && name != "node_modules"
            {
                visit(&path, depth - 1, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(root, 2, &mut out);
    let catalog = root.join("gradle/libs.versions.toml");
    if catalog.is_file() {
        out.push(catalog);
    }
    let wrapper = root.join("gradle/wrapper/gradle-wrapper.properties");
    if wrapper.is_file() {
        out.push(wrapper);
    }
    out.sort();
    out
}

fn cache_key(root: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    INIT_SCRIPT.hash(&mut hasher);
    for file in build_files(root) {
        file.hash(&mut hasher);
        if let Ok(content) = std::fs::read(&file) {
            content.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn cache_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("java-analyzer").join("gradle"))
}

/// 运行 Gradle 导出模型，返回 JSON 文本（各工程的模型组成的数组）。
fn run_gradle(root: &Path) -> Result<String, String> {
    let tmp = std::env::temp_dir();
    let pid = std::process::id();
    let script = tmp.join(format!("java-analyzer-init-{pid}.gradle"));
    let output = tmp.join(format!("java-analyzer-model-{pid}"));
    let _ = std::fs::remove_dir_all(&output);
    std::fs::write(&script, INIT_SCRIPT).map_err(|e| {
        base_db::tr!(
            "写入 init script 失败: {e}",
            "Failed to write the init script: {e}"
        )
    })?;

    let wrapper = root.join(if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    });
    let program = if wrapper.is_file() {
        wrapper
    } else {
        PathBuf::from("gradle")
    };
    let result = Command::new(&program)
        .current_dir(root)
        .arg("--init-script")
        .arg(&script)
        .arg(format!("-DjavaAnalyzer.output={}", output.display()))
        // 导出任务在执行时读工程模型，和配置缓存不兼容；旧版本 Gradle 不认识的系统属性会被忽略
        .arg("-Dorg.gradle.configuration-cache=false")
        .arg("-Dorg.gradle.unsafe.configuration-cache=false")
        .args(["-q", "--console=plain", "javaAnalyzerModel"])
        .output();
    let _ = std::fs::remove_file(&script);

    let result = result.map_err(|e| {
        base_db::tr!(
            "无法启动 {}: {e}",
            "Failed to start {}: {e}",
            program.display()
        )
    })?;
    if !result.status.success() {
        let _ = std::fs::remove_dir_all(&output);
        let stderr = String::from_utf8_lossy(&result.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(15).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(base_db::tr!(
            "Gradle 导入失败:\n{}",
            "Gradle import failed:\n{}",
            tail.join("\n")
        ));
    }
    let json = read_model_dir(&output).map_err(|e| {
        base_db::tr!(
            "读取导出的模型失败: {e}",
            "Failed to read the exported model: {e}"
        )
    });
    let _ = std::fs::remove_dir_all(&output);
    json
}

/// 输出目录里每个工程一个 JSON 对象，按文件名排序后拼成一个数组。
fn read_model_dir(dir: &Path) -> std::io::Result<String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    let mut parts = Vec::with_capacity(files.len());
    for file in files {
        parts.push(std::fs::read_to_string(file)?);
    }
    Ok(format!("[{}]", parts.join(",")))
}

/// Gradle 缓存里与 jar 相邻的 sources jar：
/// `files-2.1/<group>/<name>/<version>/<hash>/<name>-<version>.jar`，sources 在另一个 hash 目录下。
fn find_sources_jar(jar: &Path) -> Option<PathBuf> {
    let stem = jar.file_stem()?.to_string_lossy().into_owned();
    let version_dir = jar.parent()?.parent()?;
    let wanted = format!("{stem}-sources.jar");
    for hash_dir in std::fs::read_dir(version_dir).ok()?.flatten() {
        let candidate = hash_dir.path().join(&wanted);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

enum TestOutput {
    No,
    Yes,
    Maybe,
}

/// classpath 里指向工程 `project` 产物的条目是不是它的测试代码：测试类源码集（名字里带 test，
/// 包括 testFixtures）的输出目录是；名字里带 test 的 jar 可能是。
fn test_output(project: &GradleProject, entry: &Path) -> TestOutput {
    let test_set = |set: &GradleSourceSet| set.name.to_lowercase().contains("test");
    if let Some(set) = project
        .source_sets
        .iter()
        .find(|set| set.output_dirs.iter().any(|d| d == entry))
    {
        return if test_set(set) {
            TestOutput::Yes
        } else {
            TestOutput::No
        };
    }
    let test_jar = entry.extension().is_some_and(|ext| ext == "jar")
        && entry
            .file_stem()
            .is_some_and(|stem| stem.to_string_lossy().to_lowercase().contains("test"));
    if test_jar {
        TestOutput::Maybe
    } else {
        TestOutput::No
    }
}

fn convert(projects: Vec<GradleProject>) -> GradleImport {
    let mut import = GradleImport::default();
    let mut library_ids: FxHashMap<PathBuf, LibraryId> = FxHashMap::default();

    // 先排好模块下标，classpath 里指向别的子工程产物的条目要转成模块依赖
    let project_ids: Vec<ModuleId> = (0..projects.len()).map(|i| ModuleId(i as u32)).collect();
    let owner_of = |path: &Path| -> Option<ModuleId> {
        projects
            .iter()
            .zip(&project_ids)
            .find(|(p, _)| path.starts_with(&p.build_dir))
            .map(|(_, id)| *id)
    };

    for (idx, project) in projects.iter().enumerate() {
        let mut source_roots = Vec::new();
        let mut dependencies: Vec<Dependency> = Vec::new();

        for set in &project.source_sets {
            let is_test = set.name.to_lowercase().contains("test");
            let kind = if is_test {
                SourceRootKind::Test
            } else {
                SourceRootKind::Main
            };
            for dir in &set.src_dirs {
                if dir.is_dir() {
                    let generated = dir.starts_with(&project.build_dir);
                    source_roots.push(SourceRoot {
                        path: dir.clone(),
                        kind: if generated {
                            SourceRootKind::Generated
                        } else {
                            kind
                        },
                    });
                }
            }
            let scope = if is_test { Scope::Test } else { Scope::Compile };
            for entry in &set.classpath {
                // 本工程自己的输出目录（test 依赖 main）不算依赖
                if set.output_dirs.contains(entry) || entry.starts_with(&project.build_dir) {
                    continue;
                }
                let targets: Vec<DependencyTarget> = match owner_of(entry) {
                    Some(module) if module == project_ids[idx] => continue,
                    Some(module) => match test_output(&projects[module.0 as usize], entry) {
                        TestOutput::No => vec![DependencyTarget::Module(module)],
                        TestOutput::Yes => vec![DependencyTarget::ModuleTests(module)],
                        // 名字里带 test 的 jar 分不清是测试代码还是主代码，两样都算
                        TestOutput::Maybe => vec![
                            DependencyTarget::Module(module),
                            DependencyTarget::ModuleTests(module),
                        ],
                    },
                    None => {
                        if !entry.is_file() {
                            continue;
                        }
                        let id = *library_ids.entry(entry.clone()).or_insert_with(|| {
                            import.libraries.push(LibraryData {
                                name: entry
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                jar: entry.clone(),
                                sources: find_sources_jar(entry),
                            });
                            LibraryId((import.libraries.len() - 1) as u32)
                        });
                        vec![DependencyTarget::Library(id)]
                    }
                };
                for target in targets {
                    // 同一个依赖同时出现在 main 和 test 里时保留 Compile
                    match dependencies.iter_mut().find(|d| d.target == target) {
                        Some(existing) => {
                            if scope == Scope::Compile {
                                existing.scope = Scope::Compile;
                            }
                        }
                        None => dependencies.push(Dependency { target, scope }),
                    }
                }
            }
        }

        // 注解处理器生成的源码目录
        for generated in [
            "generated/sources/annotationProcessor/java/main",
            "generated/sources/annotationProcessor/java/test",
        ] {
            let dir = project.build_dir.join(generated);
            if dir.is_dir() && !source_roots.iter().any(|r| r.path == dir) {
                source_roots.push(SourceRoot {
                    path: dir,
                    kind: SourceRootKind::Generated,
                });
            }
        }

        import.modules.push(ModuleData {
            name: project.name.clone(),
            root: project.dir.clone(),
            source_roots,
            language_level: project
                .language_level
                .as_deref()
                .and_then(crate::jdk::parse_feature_version),
            api_release: project.release,
            preview: project.preview,
            dependencies,
        });
    }
    import
}

/// `root` 处的 Gradle 工程当前对应的缓存文件。构建文件的内容变了，路径也会变。
/// 也用于测试：预先写入导出的模型，就能在不运行 Gradle 的情况下导入。
pub fn cache_file(root: &Path) -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join(format!("{:016x}.json", cache_key(root))))
}

/// 导入 Gradle 工程。`allow_run` 为假时只使用缓存，不调用 Gradle。
pub fn import(root: &Path, allow_run: bool) -> Option<GradleImport> {
    let cache_file = cache_file(root);

    let cached = cache_file
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok());
    let (json, from_cache) = match cached {
        Some(json) => (json, true),
        None if allow_run => match run_gradle(root) {
            Ok(json) => (json, false),
            Err(message) => {
                return Some(GradleImport {
                    messages: vec![message],
                    ..GradleImport::default()
                })
            }
        },
        None => return None,
    };

    let projects: Vec<GradleProject> = match serde_json::from_str(&json) {
        Ok(projects) => projects,
        Err(err) => {
            if let Some(path) = &cache_file {
                let _ = std::fs::remove_file(path);
            }
            return Some(GradleImport {
                messages: vec![base_db::tr!(
                    "无法解析 Gradle 导出的模型: {err}",
                    "Cannot parse the model exported by Gradle: {err}"
                )],
                ..GradleImport::default()
            });
        }
    };

    if !from_cache {
        if let Some(path) = &cache_file {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, &json);
        }
    }
    Some(convert(projects))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_files_are_joined_into_one_array() {
        let dir = std::env::temp_dir().join(format!("ja-gradle-model-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("core.json"),
            r#"{"name":"core","dir":"/p/core","buildDir":"/p/core/build"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("app.json"),
            r#"{"name":"app","dir":"/p/app","buildDir":"/p/app/build"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("ignored.txt"), "x").unwrap();
        let json = read_model_dir(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let projects: Vec<GradleProject> = serde_json::from_str(&json).unwrap();
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["app", "core"]);
    }

    #[test]
    fn converts_exported_model() {
        let json = r#"[
            {"name":"app","dir":"/w/app","buildDir":"/w/app/build","languageLevel":"17",
             "sourceSets":[{"name":"main","srcDirs":["/w/app/src/main/java"],
                            "classpath":["/w/lib/build/libs/lib.jar"],"outputDirs":[]}]},
            {"name":"lib","dir":"/w/lib","buildDir":"/w/lib/build","languageLevel":null,"sourceSets":[]}
        ]"#;
        let projects: Vec<GradleProject> = serde_json::from_str(json).unwrap();
        let import = convert(projects);
        assert_eq!(import.modules.len(), 2);
        assert_eq!(import.modules[0].language_level, Some(17));
        assert_eq!(
            import.modules[0].dependencies,
            vec![Dependency {
                target: DependencyTarget::Module(ModuleId(1)),
                scope: Scope::Compile
            }]
        );
        assert!(import.libraries.is_empty());
    }

    #[test]
    fn test_outputs_of_other_projects_are_module_tests() {
        let json = r#"[
            {"name":"app","dir":"/w/app","buildDir":"/w/app/build","sourceSets":[
                {"name":"main","srcDirs":[],"classpath":["/w/core/build/classes/java/main"],
                 "outputDirs":["/w/app/build/classes/java/main"]},
                {"name":"test","srcDirs":[],"classpath":[
                    "/w/app/build/classes/java/main",
                    "/w/core/build/classes/java/main",
                    "/w/core/build/classes/java/testFixtures",
                    "/w/kit/build/libs/kit-test-utils.jar"],
                 "outputDirs":["/w/app/build/classes/java/test"]}]},
            {"name":"core","dir":"/w/core","buildDir":"/w/core/build","sourceSets":[
                {"name":"main","srcDirs":[],"classpath":[],"outputDirs":["/w/core/build/classes/java/main"]},
                {"name":"testFixtures","srcDirs":[],"classpath":[],
                 "outputDirs":["/w/core/build/classes/java/testFixtures"]}]},
            {"name":"kit","dir":"/w/kit","buildDir":"/w/kit/build","sourceSets":[]}
        ]"#;
        let projects: Vec<GradleProject> = serde_json::from_str(json).unwrap();
        let import = convert(projects);
        let dep = |target, scope| Dependency { target, scope };
        assert_eq!(
            import.modules[0].dependencies,
            vec![
                dep(DependencyTarget::Module(ModuleId(1)), Scope::Compile),
                dep(DependencyTarget::ModuleTests(ModuleId(1)), Scope::Test),
                // 名字里带 test 的 jar 分不清，主代码和测试代码都算
                dep(DependencyTarget::Module(ModuleId(2)), Scope::Test),
                dep(DependencyTarget::ModuleTests(ModuleId(2)), Scope::Test),
            ]
        );
    }
}
