//! 与构建系统无关的工程模型。由 `project-model` 从 Maven / Gradle / 手动配置导入后填充。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LibraryId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceRootKind {
    Main,
    Test,
    /// 注解处理器等生成的代码
    Generated,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceRoot {
    pub path: PathBuf,
    pub kind: SourceRootKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Compile,
    Provided,
    Runtime,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependencyTarget {
    Module(ModuleId),
    /// 模块的测试代码（Maven 的 `test-jar` 依赖）
    ModuleTests(ModuleId),
    Library(LibraryId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dependency {
    pub target: DependencyTarget,
    pub scope: Scope,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModuleData {
    pub name: String,
    pub root: PathBuf,
    pub source_roots: Vec<SourceRoot>,
    /// Java 语言级别（8、11、17、21 …）；未知时为 `None`
    pub language_level: Option<u32>,
    /// 编译时开启了预览特性（`--enable-preview`）
    pub preview: bool,
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LibraryData {
    /// 展示用的名字，通常是 `group:artifact:version`
    pub name: String,
    pub jar: PathBuf,
    pub sources: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JdkInfo {
    pub home: PathBuf,
    /// 特性版本号（8、11、17、21 …）
    pub version: u32,
    pub src_zip: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ProjectGraph {
    pub modules: Vec<ModuleData>,
    pub libraries: Vec<LibraryData>,
    pub jdk: Option<JdkInfo>,
}

impl ProjectGraph {
    pub fn module(&self, id: ModuleId) -> &ModuleData {
        &self.modules[id.0 as usize]
    }

    pub fn library(&self, id: LibraryId) -> &LibraryData {
        &self.libraries[id.0 as usize]
    }

    /// 路径落在哪个模块的哪个源码根下。多个根互相嵌套时取最深的那个。
    pub fn source_root_of(&self, path: &Path) -> Option<(ModuleId, &SourceRoot)> {
        let mut best: Option<(ModuleId, &SourceRoot)> = None;
        for (i, module) in self.modules.iter().enumerate() {
            for root in &module.source_roots {
                if !path.starts_with(&root.path) {
                    continue;
                }
                let deeper = best.is_none_or(|(_, b)| {
                    root.path.components().count() > b.path.components().count()
                });
                if deeper {
                    best = Some((ModuleId(i as u32), root));
                }
            }
        }
        best
    }

    pub fn all_source_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .modules
            .iter()
            .flat_map(|m| m.source_roots.iter().map(|r| r.path.clone()))
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    /// 工程里用到的最高语言级别，用作全局默认。
    pub fn max_language_level(&self) -> Option<u32> {
        self.modules.iter().filter_map(|m| m.language_level).max()
    }

    /// 按库名找它的源码包。
    pub fn library_by_jar(&self, jar: &Path) -> Option<&LibraryData> {
        self.libraries.iter().find(|lib| lib.jar == jar)
    }
}
