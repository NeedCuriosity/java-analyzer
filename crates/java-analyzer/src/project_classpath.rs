//! 扩展请求 `java-analyzer/projectClasspath`：模块运行 / 测试时的类路径和 JDK，
//! 客户端拼 `java` 命令时用。和调试器 `launch` 不给 `classPaths` 时算的一样
//! （都用 [`project_model::runtime_class_path`]）。
//!
//! 请求要用到导入好的工程模型：导入还没完成时由主循环先存起来，导入完再回复。

use std::path::Path;

use base_db::{ModuleId, ProjectGraph, SourceRootKind};
use lsp_server::ErrorCode;
use lsp_types::Url;
use serde::{Deserialize, Serialize};

pub const METHOD: &str = "java-analyzer/projectClasspath";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Params {
    /// 源文件，或者模块里的任意路径（模块目录本身也行）
    uri: Url,
    /// `runtime` 或 `test`；不写时看文件在不在测试源码根里
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ProjectClasspath {
    module: String,
    project_root: String,
    scope: &'static str,
    class_paths: Vec<String>,
    /// 工程模型里没有模块路径的概念：总是空的，留给以后
    module_paths: Vec<String>,
    output_dirs: Vec<String>,
    jdk_home: Option<String>,
    /// `jdkHome` 的特性版本号
    jdk_version: Option<u32>,
    language_level: Option<u32>,
    /// 源文件编码（`project.build.sourceEncoding` 等）；工程没有设置时为 `null`。
    /// 客户端拼 `java` 命令时可以据此加 `-Dfile.encoding=`
    encoding: Option<String>,
    /// JDK 版本低于这个模块（按 `scope`）的语言级别时给出的提示，客户端可以展示给用户；
    /// 没有问题时为 `null`
    jdk_warning: Option<String>,
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// 路径所在的模块和它是不是测试代码：先看源码根（嵌套时取最深的），
/// 不在任何源码根下时取模块根是它最长前缀的那个模块。
fn module_at(graph: &ProjectGraph, path: &Path) -> Option<(ModuleId, bool)> {
    if let Some((module, root)) = graph.source_root_of(path) {
        return Some((module, root.kind == SourceRootKind::Test));
    }
    graph
        .modules
        .iter()
        .enumerate()
        .filter(|(_, module)| path.starts_with(&module.root))
        .max_by_key(|(_, module)| module.root.components().count())
        .map(|(i, _)| (ModuleId(i as u32), false))
}

/// 处理请求。出错时返回错误码和给用户看的消息。
pub fn handle(
    graph: &ProjectGraph,
    params: serde_json::Value,
) -> Result<serde_json::Value, (ErrorCode, String)> {
    let params: Params =
        serde_json::from_value(params).map_err(|e| (ErrorCode::InvalidParams, e.to_string()))?;
    let path = params.uri.to_file_path().map_err(|()| {
        (
            ErrorCode::InvalidParams,
            base_db::tr!("不是文件 URI: {}", "Not a file URI: {}", params.uri),
        )
    })?;
    let (module, in_tests) = module_at(graph, &path)
        .or_else(|| {
            // 客户端给的路径可能没有解析符号链接（macOS 的 /var → /private/var）
            let canonical = project_model::canonicalize(&path).ok()?;
            module_at(graph, &canonical)
        })
        .ok_or_else(|| {
            (
                ErrorCode::RequestFailed,
                base_db::tr!(
                    "{} 不属于工程里的任何模块",
                    "{} does not belong to any module of the project",
                    path.display()
                ),
            )
        })?;
    let test = match params.scope.as_deref() {
        None => in_tests,
        Some("runtime") => false,
        Some("test") => true,
        Some(other) => {
            return Err((
                ErrorCode::InvalidParams,
                base_db::tr!(
                    "scope 只能是 runtime 或 test，收到的是 {other}",
                    "scope must be runtime or test, got {other}"
                ),
            ))
        }
    };

    let class_path = project_model::runtime_class_path(graph, module, test);
    let data = graph.module(module);
    let jdk = graph
        .jdk
        .clone()
        .or_else(|| project_model::discover_jdk(None));
    let level = if test {
        data.test_language_level.or(data.language_level)
    } else {
        data.language_level
    };
    let jdk_warning = match (&jdk, level) {
        (Some(jdk), Some(level)) if jdk.version < level => Some(base_db::tr!(
            "模块 {} 的语言级别是 {level}，但 JDK {} 的版本是 {}",
            "Module {} has language level {level}, but the JDK at {} is version {}",
            data.name,
            jdk.home.display(),
            jdk.version
        )),
        _ => None,
    };
    let result = ProjectClasspath {
        module: data.name.clone(),
        project_root: path_string(&data.root),
        scope: if test { "test" } else { "runtime" },
        class_paths: class_path.entries.iter().map(|p| path_string(p)).collect(),
        module_paths: Vec::new(),
        output_dirs: class_path
            .output_dirs
            .iter()
            .map(|p| path_string(p))
            .collect(),
        jdk_home: jdk.as_ref().map(|jdk| path_string(&jdk.home)),
        jdk_version: jdk.as_ref().map(|jdk| jdk.version),
        language_level: data.language_level,
        encoding: data.encoding.clone(),
        jdk_warning,
    };
    serde_json::to_value(result).map_err(|e| (ErrorCode::InternalError, e.to_string()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use base_db::{
        Dependency, DependencyTarget, JdkInfo, LibraryData, LibraryId, ModuleData, Scope,
        SourceRoot,
    };
    use serde_json::json;

    use super::*;

    fn touch(dir: &Path, relative: &str) -> PathBuf {
        let path = dir.join(relative.trim_end_matches('/'));
        if relative.ends_with('/') {
            std::fs::create_dir_all(&path).unwrap();
        } else {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"").unwrap();
        }
        path
    }

    fn module(dir: &Path, name: &str, dependencies: Vec<Dependency>) -> ModuleData {
        let root = dir.join(name);
        ModuleData {
            name: name.into(),
            source_roots: vec![
                SourceRoot {
                    path: root.join("src/main/java"),
                    kind: SourceRootKind::Main,
                },
                SourceRoot {
                    path: root.join("src/test/java"),
                    kind: SourceRootKind::Test,
                },
            ],
            language_level: Some(17),
            api_release: None,
            test_language_level: None,
            preview: false,
            dependencies,
            output_dirs: vec![
                SourceRoot {
                    path: root.join("target/classes"),
                    kind: SourceRootKind::Main,
                },
                SourceRoot {
                    path: root.join("target/test-classes"),
                    kind: SourceRootKind::Test,
                },
            ],
            encoding: (name == "app").then(|| "UTF-8".to_owned()),
            root,
        }
    }

    /// 和调试器按主类算出来的类路径一致；scope 不写时按文件所在的源码根推断。
    #[test]
    fn matches_the_debugger_class_path() {
        let dir = std::env::temp_dir().join(format!("ja-projcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        touch(&dir, "app/target/classes/demo/Main.class");
        touch(&dir, "app/target/test-classes/demo/MainTest.class");
        touch(&dir, "core/target/classes/");
        let lib = touch(&dir, "lib.jar");
        let junit = touch(&dir, "junit.jar");
        let graph = ProjectGraph {
            modules: vec![
                module(
                    &dir,
                    "app",
                    vec![
                        Dependency {
                            target: DependencyTarget::Module(ModuleId(1)),
                            scope: Scope::Compile,
                        },
                        Dependency {
                            target: DependencyTarget::Library(LibraryId(1)),
                            scope: Scope::Test,
                        },
                    ],
                ),
                module(
                    &dir,
                    "core",
                    vec![Dependency {
                        target: DependencyTarget::Library(LibraryId(0)),
                        scope: Scope::Compile,
                    }],
                ),
            ],
            libraries: vec![
                LibraryData {
                    name: "lib".into(),
                    jar: lib,
                    sources: None,
                },
                LibraryData {
                    name: "junit".into(),
                    jar: junit,
                    sources: None,
                },
            ],
            jdk: Some(JdkInfo {
                home: PathBuf::from("/opt/jdk"),
                version: 21,
                src_zip: None,
            }),
        };
        let request = |relative: &str, scope: Option<&str>| {
            let uri = Url::from_file_path(dir.join(relative)).unwrap();
            handle(&graph, json!({ "uri": uri, "scope": scope })).unwrap()
        };
        let strings =
            |paths: Vec<PathBuf>| -> Vec<String> { paths.iter().map(|p| path_string(p)).collect() };

        let main = request("app/src/main/java/demo/Main.java", None);
        assert_eq!(main["scope"], "runtime");
        assert_eq!(main["module"], "app");
        assert_eq!(main["projectRoot"], path_string(&dir.join("app")));
        assert_eq!(main["jdkHome"], "/opt/jdk");
        assert_eq!(main["languageLevel"], 17);
        assert_eq!(main["encoding"], "UTF-8");
        assert_eq!(main["jdkVersion"], 21);
        assert_eq!(main["jdkWarning"], serde_json::Value::Null);
        assert_eq!(main["modulePaths"], json!([]));
        assert_eq!(
            main["outputDirs"],
            json!([path_string(&dir.join("app").join("target/classes"))])
        );
        assert_eq!(
            main["classPaths"],
            json!(strings(
                dap::runtime_class_path(&graph, "demo.Main").unwrap()
            ))
        );

        let test = request("app/src/test/java/demo/MainTest.java", None);
        assert_eq!(test["scope"], "test");
        assert_eq!(
            test["classPaths"],
            json!(strings(
                dap::runtime_class_path(&graph, "demo.MainTest").unwrap()
            ))
        );
        assert_eq!(test["classPaths"].as_array().unwrap().len(), 5);

        // 模块目录 + 显式 scope
        assert_eq!(
            request("app", Some("test"))["classPaths"],
            test["classPaths"]
        );
        assert_eq!(
            request("app", Some("runtime"))["classPaths"],
            main["classPaths"]
        );
        // 没有设置编码的模块给 null
        assert_eq!(request("core", None)["encoding"], json!(null));
        // core 自己的类路径里没有 app，也没有 app 的 test 依赖
        assert_eq!(
            request("core/pom.xml", Some("test"))["classPaths"],
            json!(strings(vec![
                dir.join("core").join("target/classes"),
                dir.join("lib.jar")
            ]))
        );

        // JDK 比模块的语言级别低：给出提示
        let mut old_jdk = graph.clone();
        old_jdk.jdk.as_mut().unwrap().version = 11;
        let uri = Url::from_file_path(dir.join("app")).unwrap();
        let warned = handle(&old_jdk, json!({ "uri": uri })).unwrap();
        assert_eq!(warned["jdkVersion"], 11);
        let warning = warned["jdkWarning"].as_str().unwrap();
        assert!(
            warning.contains("17") && warning.contains("11"),
            "{warning}"
        );

        let err = handle(&graph, json!({ "uri": uri, "scope": "debug" })).unwrap_err();
        assert_eq!(err.0 as i32, ErrorCode::InvalidParams as i32);
        let uri = Url::from_file_path(std::env::temp_dir().join("elsewhere")).unwrap();
        let err = handle(&graph, json!({ "uri": uri })).unwrap_err();
        assert_eq!(err.0 as i32, ErrorCode::RequestFailed as i32);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
