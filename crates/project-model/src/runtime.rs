//! 模块运行时的类路径：调试器启动 JVM（`launch` 不给 `classPaths` 时）和
//! LSP 请求 `java-analyzer/projectClasspath` 都用它，两边结果一致。

use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
};

use base_db::{
    DependencyTarget, LibraryData, LibraryId, ModuleId, ProjectGraph, Scope, SourceRootKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeClassPath {
    /// 模块自己的编译输出目录，`test` 时测试输出在前；按工程模型给出，不一定已经存在
    pub output_dirs: Vec<PathBuf>,
    /// 类路径：模块自己的输出目录、（传递）依赖的模块的输出目录、依赖库的 jar，
    /// 按这个顺序去重；磁盘上不存在的条目（还没编译的输出目录）不在里面
    pub entries: Vec<PathBuf>,
}

/// 模块 `module` 运行时的类路径。`test` 为真时带上测试输出目录和 test scope 的依赖。
///
/// 和 Maven 一样，依赖模块的 test / provided scope 依赖不传递；不是测试时，模块自己的
/// provided 依赖也不在里面（由运行环境提供）。模块的依赖已经是传递闭包时
/// （[`ModuleData::transitive_dependencies`]）只用它自己的，不展开依赖模块的依赖：
/// 闭包里没有的是被排除的或者 optional 的。否则展开依赖模块的依赖，
/// 依赖模块又带进来的同一个构件（`group:artifact` 和 classifier 相同）的其他版本不再加入，
/// 先遇到的（`module` 自己的）为准；工作区模块自己的 jar 在它的输出目录已经在类路径上时也跳过。
///
/// [`ModuleData::transitive_dependencies`]: base_db::ModuleData::transitive_dependencies
pub fn runtime_class_path(graph: &ProjectGraph, module: ModuleId, test: bool) -> RuntimeClassPath {
    runtime_class_path_in(
        graph,
        module,
        test,
        &crate::maven_settings::local_repository(),
    )
}

/// 类路径上的一项：目录或者依赖库，最后再去重。
enum Entry {
    Path(PathBuf),
    Library(LibraryId),
}

fn runtime_class_path_in(
    graph: &ProjectGraph,
    module: ModuleId,
    test: bool,
    maven_repo: &Path,
) -> RuntimeClassPath {
    let outputs = |module: ModuleId, kind: SourceRootKind| -> Vec<PathBuf> {
        graph
            .module(module)
            .output_dirs
            .iter()
            .filter(|dir| dir.kind == kind)
            .map(|dir| dir.path.clone())
            .collect()
    };
    let mut output_dirs = Vec::new();
    if test {
        output_dirs.extend(outputs(module, SourceRootKind::Test));
    }
    output_dirs.extend(outputs(module, SourceRootKind::Main));

    let mut ordered: Vec<Entry> = output_dirs.iter().cloned().map(Entry::Path).collect();
    let closed = graph.module(module).transitive_dependencies;
    // 输出目录在类路径上的模块
    let mut on_path: Vec<ModuleId> = vec![module];
    let mut seen: HashSet<DependencyTarget> = HashSet::new();
    let mut queue: VecDeque<ModuleId> = VecDeque::from([module]);
    let mut visited: HashSet<ModuleId> = HashSet::from([module]);
    while let Some(current) = queue.pop_front() {
        for dep in &graph.module(current).dependencies {
            let wanted = match dep.scope {
                Scope::Compile | Scope::Runtime => true,
                Scope::Test | Scope::Provided => test && current == module,
            };
            if !wanted || !seen.insert(dep.target) {
                continue;
            }
            match dep.target {
                DependencyTarget::Module(target) => {
                    ordered.extend(
                        outputs(target, SourceRootKind::Main)
                            .into_iter()
                            .map(Entry::Path),
                    );
                    on_path.push(target);
                    if visited.insert(target) && !closed {
                        queue.push_back(target);
                    }
                }
                DependencyTarget::ModuleTests(target) => {
                    ordered.extend(
                        outputs(target, SourceRootKind::Test)
                            .into_iter()
                            .map(Entry::Path),
                    );
                    on_path.push(target);
                }
                DependencyTarget::Library(lib) => ordered.push(Entry::Library(lib)),
            }
        }
    }

    let module_coordinates: HashSet<(String, String)> = on_path
        .iter()
        .filter_map(|&m| crate::maven::pom_coordinates(&graph.module(m).root.join("pom.xml")))
        .collect();
    let mut entries: Vec<PathBuf> = Vec::new();
    let mut artifacts: HashSet<String> = HashSet::new();
    for entry in ordered {
        let path = match entry {
            Entry::Path(path) => path,
            Entry::Library(lib) => {
                let library = graph.library(lib);
                if let Some(coord) = library_coordinate(library, maven_repo) {
                    if module_coordinates.contains(&(coord.group.clone(), coord.artifact.clone())) {
                        continue;
                    }
                    let key = format!("{}:{}:{}", coord.group, coord.artifact, coord.classifier);
                    if !artifacts.insert(key) {
                        continue;
                    }
                }
                library.jar.clone()
            }
        };
        if !entries.contains(&path) {
            entries.push(path);
        }
    }
    entries.retain(|e| e.exists());
    RuntimeClassPath {
        output_dirs,
        entries,
    }
}

/// 依赖库的 Maven 坐标（不含版本）。
#[derive(Debug, PartialEq, Eq)]
struct LibraryCoordinate {
    group: String,
    artifact: String,
    /// 没有 classifier 时为空
    classifier: String,
}

/// 从依赖库的名字（`group:artifact:version`）或者 jar 在仓库里的位置推出坐标：
/// Maven 本地仓库的 `<group 路径>/<artifact>/<version>/<artifact>-<version>[-<classifier>].jar`、
/// Gradle 缓存的 `files-2.1/<group>/<artifact>/<version>/<hash>/<文件>`。都推不出时返回 `None`。
fn library_coordinate(library: &LibraryData, maven_repo: &Path) -> Option<LibraryCoordinate> {
    let stem = library
        .jar
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    // 文件名 `<artifact>-<version>[-<classifier>]` 里的 classifier；对不上时为 `None`
    let classifier_in_name = |artifact: &str, version: &str| -> Option<String> {
        let rest = stem
            .strip_prefix(artifact)?
            .strip_prefix('-')?
            .strip_prefix(version)?;
        if rest.is_empty() {
            Some(String::new())
        } else {
            rest.strip_prefix('-').map(str::to_owned)
        }
    };
    if let [group, artifact, version] = library.name.split(':').collect::<Vec<_>>()[..] {
        if [group, artifact, version].iter().all(|p| !p.is_empty()) {
            return Some(LibraryCoordinate {
                group: group.to_owned(),
                artifact: artifact.to_owned(),
                classifier: classifier_in_name(artifact, version).unwrap_or_default(),
            });
        }
    }
    let components: Vec<&str> = library
        .jar
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    let (group, artifact, version) =
        if let Some(pos) = components.iter().rposition(|c| *c == "files-2.1") {
            let [group, artifact, version, _hash, _file] = components.get(pos + 1..)? else {
                return None;
            };
            (group.to_string(), *artifact, *version)
        } else {
            let relative = library.jar.strip_prefix(maven_repo).ok()?;
            let parts: Vec<&str> = relative
                .components()
                .map(|c| c.as_os_str().to_str())
                .collect::<Option<_>>()?;
            let [group @ .., artifact, version, _file] = parts.as_slice() else {
                return None;
            };
            if group.is_empty() {
                return None;
            }
            (group.join("."), *artifact, *version)
        };
    Some(LibraryCoordinate {
        group,
        artifact: artifact.to_owned(),
        classifier: classifier_in_name(artifact, version)?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};

    /// 建出路径对应的空目录（以 `/` 结尾）或空文件，返回绝对路径。
    pub(crate) fn touch(dir: &Path, relative: &str) -> PathBuf {
        let path = dir.join(relative.trim_end_matches('/'));
        if relative.ends_with('/') {
            std::fs::create_dir_all(&path).unwrap();
        } else {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"").unwrap();
        }
        path
    }

    use base_db::{
        Dependency, DependencyTarget, LibraryData, LibraryId, ModuleData, ModuleId, ProjectGraph,
        Scope, SourceRoot, SourceRootKind,
    };

    use super::{library_coordinate, runtime_class_path_in, LibraryCoordinate};

    fn module(root: &Path, dependencies: Vec<(DependencyTarget, Scope)>) -> ModuleData {
        ModuleData {
            name: root.file_name().unwrap().to_string_lossy().into_owned(),
            root: root.to_path_buf(),
            source_roots: Vec::new(),
            language_level: None,
            api_release: None,
            test_language_level: None,
            preview: false,
            encoding: None,
            transitive_dependencies: false,
            dependencies: dependencies
                .into_iter()
                .map(|(target, scope)| Dependency { target, scope })
                .collect(),
            output_dirs: vec![
                SourceRoot {
                    path: touch(root, "target/classes/"),
                    kind: SourceRootKind::Main,
                },
                SourceRoot {
                    path: touch(root, "target/test-classes/"),
                    kind: SourceRootKind::Test,
                },
            ],
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ja-runtime-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn lib(id: u32) -> DependencyTarget {
        DependencyTarget::Library(LibraryId(id))
    }

    #[test]
    fn libraries_are_deduplicated_by_coordinate_and_scope_follows_maven() {
        let dir = temp_dir("dedup");
        let repo = dir.join("repo");
        let jar = |relative: &str| touch(&repo, relative);
        let library = |name: &str, jar: PathBuf| LibraryData {
            name: name.to_owned(),
            jar,
            sources: None,
        };
        let libraries = vec![
            library("g:guava:32", jar("g/guava/32/guava-32.jar")),
            library("g:guava:31", jar("g/guava/31/guava-31.jar")),
            library("g:servlet:1", jar("g/servlet/1/servlet-1.jar")),
            library("g:junit:4", jar("g/junit/4/junit-4.jar")),
            library("g:lombok:1", jar("g/lombok/1/lombok-1.jar")),
            library("g:mockito:5", jar("g/mockito/5/mockito-5.jar")),
            library("g:kit:1", jar("g/kit/1/kit-1.jar")),
            library("g:kit:1", jar("g/kit/1/kit-1-tests.jar")),
            library("g:kit:2", jar("g/kit/2/kit-2-tests.jar")),
            // 只有文件名的库按本地仓库里的位置推坐标
            library("rt-1.jar", jar("org/x/rt/1/rt-1.jar")),
            library("rt-2.jar", jar("org/x/rt/2/rt-2.jar")),
            // 工作区模块 core 的 jar
            library("g:core:1", jar("g/core/1/core-1.jar")),
        ];
        let app_dir = dir.join("app");
        let core_dir = dir.join("core");
        std::fs::create_dir_all(&core_dir).unwrap();
        std::fs::write(
            core_dir.join("pom.xml"),
            "<project><groupId>g</groupId><artifactId>core</artifactId><version>1</version></project>",
        )
        .unwrap();
        let app = module(
            &app_dir,
            vec![
                (DependencyTarget::Module(ModuleId(1)), Scope::Compile),
                (lib(0), Scope::Compile),
                (lib(2), Scope::Provided),
                (lib(3), Scope::Test),
                (lib(6), Scope::Compile),
                (lib(7), Scope::Compile),
                (lib(9), Scope::Runtime),
                (lib(11), Scope::Compile),
            ],
        );
        let core = module(
            &core_dir,
            vec![
                (lib(1), Scope::Compile),
                (lib(4), Scope::Provided),
                (lib(5), Scope::Test),
                (lib(8), Scope::Compile),
                (lib(10), Scope::Compile),
            ],
        );
        let graph = ProjectGraph {
            modules: vec![app, core],
            libraries,
            jdk: None,
        };
        let names = |test: bool| -> Vec<String> {
            runtime_class_path_in(&graph, ModuleId(0), test, &repo)
                .entries
                .iter()
                .map(|p| {
                    p.strip_prefix(&dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect()
        };
        // 运行：app 自己的 provided、test 不在；core 的 guava 31、kit 2 的 tests、rt 2 被
        // app 的版本盖过；core 的 provided、test 不传递；core 自己的 jar 不和输出目录重复
        assert_eq!(
            names(false),
            [
                "app/target/classes",
                "core/target/classes",
                "repo/g/guava/32/guava-32.jar",
                "repo/g/kit/1/kit-1.jar",
                "repo/g/kit/1/kit-1-tests.jar",
                "repo/org/x/rt/1/rt-1.jar",
            ]
        );
        // 测试：app 自己的 provided 和 test 都在，core 的仍然不传递
        assert_eq!(
            names(true),
            [
                "app/target/test-classes",
                "app/target/classes",
                "core/target/classes",
                "repo/g/guava/32/guava-32.jar",
                "repo/g/servlet/1/servlet-1.jar",
                "repo/g/junit/4/junit-4.jar",
                "repo/g/kit/1/kit-1.jar",
                "repo/g/kit/1/kit-1-tests.jar",
                "repo/org/x/rt/1/rt-1.jar",
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn module_jar_is_kept_when_the_module_is_not_on_the_class_path() {
        let dir = temp_dir("modjar");
        let repo = dir.join("repo");
        let core_dir = dir.join("core");
        std::fs::create_dir_all(&core_dir).unwrap();
        std::fs::write(
            core_dir.join("pom.xml"),
            "<project><groupId>g</groupId><artifactId>core</artifactId><version>1</version></project>",
        )
        .unwrap();
        let core_jar = touch(&repo, "g/core/1/core-1.jar");
        let graph = ProjectGraph {
            modules: vec![
                module(&dir.join("app"), vec![(lib(0), Scope::Compile)]),
                module(&core_dir, Vec::new()),
            ],
            libraries: vec![LibraryData {
                name: "g:core:1".to_owned(),
                jar: core_jar.clone(),
                sources: None,
            }],
            jdk: None,
        };
        let entries = runtime_class_path_in(&graph, ModuleId(0), false, &repo).entries;
        assert_eq!(entries, [dir.join("app/target/classes"), core_jar]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_coordinates() {
        let repo = Path::new("/m2");
        let coordinate = |name: &str, jar: &str| {
            library_coordinate(
                &LibraryData {
                    name: name.to_owned(),
                    jar: PathBuf::from(jar),
                    sources: None,
                },
                repo,
            )
        };
        let expected = |group: &str, artifact: &str, classifier: &str| {
            Some(LibraryCoordinate {
                group: group.to_owned(),
                artifact: artifact.to_owned(),
                classifier: classifier.to_owned(),
            })
        };
        assert_eq!(
            coordinate("a.b:c:1.0", "/anywhere/c-1.0-jdk8.jar"),
            expected("a.b", "c", "jdk8")
        );
        assert_eq!(
            coordinate("a.b:c:1.0", "/anywhere/renamed.jar"),
            expected("a.b", "c", "")
        );
        assert_eq!(
            coordinate("c-1.0.jar", "/m2/a/b/c/1.0/c-1.0.jar"),
            expected("a.b", "c", "")
        );
        assert_eq!(
            coordinate("c-1.0-tests.jar", "/m2/a/b/c/1.0/c-1.0-tests.jar"),
            expected("a.b", "c", "tests")
        );
        assert_eq!(
            coordinate(
                "c-1.0.jar",
                "/g/caches/modules-2/files-2.1/a.b/c/1.0/abc123/c-1.0.jar"
            ),
            expected("a.b", "c", "")
        );
        assert_eq!(coordinate("c-1.0.jar", "/elsewhere/lib/c-1.0.jar"), None);
        assert_eq!(coordinate("c-1.0.jar", "/m2/c/1.0/c-1.0.jar"), None);
    }
}
