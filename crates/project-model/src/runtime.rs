//! 模块运行时的类路径：调试器启动 JVM（`launch` 不给 `classPaths` 时）和
//! LSP 请求 `java-analyzer/projectClasspath` 都用它，两边结果一致。

use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
};

use base_db::{DependencyTarget, ModuleId, ProjectGraph, Scope, SourceRootKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeClassPath {
    /// 模块自己的编译输出目录，`test` 时测试输出在前；按工程模型给出，不一定已经存在
    pub output_dirs: Vec<PathBuf>,
    /// 类路径：模块自己的输出目录、（传递）依赖的模块的输出目录、依赖库的 jar，
    /// 按这个顺序去重；磁盘上不存在的条目（还没编译的输出目录）不在里面
    pub entries: Vec<PathBuf>,
}

/// 模块 `module` 运行时的类路径。`test` 为真时带上测试输出目录和 test scope 的依赖
/// （依赖模块的 test scope 依赖不传递）。
pub fn runtime_class_path(graph: &ProjectGraph, module: ModuleId, test: bool) -> RuntimeClassPath {
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

    let mut entries: Vec<PathBuf> = Vec::new();
    let push = |path: &Path, entries: &mut Vec<PathBuf>| {
        if !entries.iter().any(|e| e == path) {
            entries.push(path.to_path_buf());
        }
    };
    for dir in &output_dirs {
        push(dir, &mut entries);
    }

    let scope_ok = |scope: Scope| test || scope != Scope::Test;
    let mut seen: HashSet<DependencyTarget> = HashSet::new();
    let mut queue: VecDeque<ModuleId> = VecDeque::from([module]);
    let mut visited: HashSet<ModuleId> = HashSet::from([module]);
    while let Some(current) = queue.pop_front() {
        for dep in &graph.module(current).dependencies {
            // 依赖模块的 test scope 依赖不会传递过来
            if !scope_ok(dep.scope) || (current != module && dep.scope == Scope::Test) {
                continue;
            }
            if !seen.insert(dep.target) {
                continue;
            }
            match dep.target {
                DependencyTarget::Module(target) => {
                    for dir in outputs(target, SourceRootKind::Main) {
                        push(&dir, &mut entries);
                    }
                    if visited.insert(target) {
                        queue.push_back(target);
                    }
                }
                DependencyTarget::ModuleTests(target) => {
                    for dir in outputs(target, SourceRootKind::Test) {
                        push(&dir, &mut entries);
                    }
                }
                DependencyTarget::Library(lib) => push(&graph.library(lib).jar, &mut entries),
            }
        }
    }
    entries.retain(|e| e.exists());
    RuntimeClassPath {
        output_dirs,
        entries,
    }
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
}
