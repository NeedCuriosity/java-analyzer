//! 模块可见性（JLS §7.7、§6.6.1）：命名模块里的代码只能用它读得到的模块导出的包。
//!
//! 只在有 `module-info.java` 的工程模块的主源码里检查 import，并且只认得清归属的包：
//! JDK 和带模块描述的 jar 按 `exports` 里列出的包认（只限定导出给别的模块的包也列在里面），
//! 工作区里的类按所在工程模块的 `module-info.java` 认。自动模块（普通 jar）的名字是推出来的，
//! 可能和构建工具用的不一致，不检查；`requires` 了找不到的模块时整个文件都不检查。

use base_db::{Db, FileKind, ModuleId, SourceFile, SourceRootKind};
use hir::Name as SmolStr;
use hir::{ClassId, ClassOrigin, ModuleGraph};
use rustc_hash::FxHashSet;

/// 文件所在的命名模块和它读得到的模块。
pub(crate) struct ModuleAccess {
    own: SmolStr,
    project_module: ModuleId,
    readable: FxHashSet<SmolStr>,
}

/// 工程模块主源码根下的 `module-info.java` 声明的模块。一个工程模块里有好几个
/// `module-info.java`（例如把 JDK 源码当成一个工程打开）时分不清，返回 `None`。
fn named_module(db: &dyn Db, graph: &ModuleGraph, module: ModuleId) -> Option<SmolStr> {
    let mut found = graph.iter().filter(|info| {
        info.source.is_some_and(|file| {
            matches!(
                main_module_of(db, file),
                Some(m) if m == module
            )
        })
    });
    let first = found.next()?;
    if found.next().is_some() {
        return None;
    }
    Some(first.name.clone())
}

/// 主源码根里的文件所属的工程模块。
fn main_module_of(db: &dyn Db, file: SourceFile) -> Option<ModuleId> {
    match base_db::file_entry(db, file.file_id(db))?.kind {
        FileKind::Workspace {
            module: Some(module),
            root: SourceRootKind::Main,
        } => Some(module),
        _ => None,
    }
}

impl ModuleAccess {
    pub(crate) fn for_file(db: &dyn Db, file: SourceFile, graph: &ModuleGraph) -> Option<Self> {
        let project_module = main_module_of(db, file)?;
        let own = named_module(db, graph, project_module)?;
        let info = graph.get(&own)?;
        // 读得到：自己、java.base、requires 的模块，以及它们 requires transitive 带来的模块
        let mut readable: FxHashSet<SmolStr> = FxHashSet::default();
        readable.insert(own.clone());
        readable.insert(SmolStr::new("java.base"));
        let mut stack: Vec<SmolStr> = Vec::new();
        for (required, _) in &info.requires {
            graph.get(required)?;
            stack.push(required.clone());
        }
        let mut visited: FxHashSet<SmolStr> = FxHashSet::default();
        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            readable.insert(current.clone());
            let module = graph.get(&current)?;
            // 自动模块读得到所有模块，经由它能读到什么说不清
            if module.automatic {
                return None;
            }
            for (required, transitive) in &module.requires {
                if *transitive {
                    stack.push(required.clone());
                }
            }
        }
        Some(ModuleAccess {
            own,
            project_module,
            readable,
        })
    }

    /// 包 `package`（斜杠形式）归哪个模块：工作区的类看所在工程模块，类路径上的看谁导出它。
    fn owner_of_package(&self, graph: &ModuleGraph, package: &str) -> Option<SmolStr> {
        let mut owners = graph
            .iter()
            .filter(|info| info.source.is_none())
            .filter(|info| info.exports.iter().any(|(p, _)| p == package));
        let owner = owners.next()?;
        if owner.automatic || owners.next().is_some() {
            return None;
        }
        Some(owner.name.clone())
    }

    fn owner_of_class(&self, db: &dyn Db, graph: &ModuleGraph, class: ClassId) -> Option<SmolStr> {
        let package = package_of(&class.binary_name(db));
        match hir::class_origin(db, class)? {
            ClassOrigin::Source { file, .. } => {
                let module = main_module_of(db, file)?;
                if module == self.project_module {
                    return Some(self.own.clone());
                }
                named_module(db, graph, module)
            }
            ClassOrigin::Binary { .. } => self.owner_of_package(graph, &package),
        }
    }

    /// 用类 `class` 违反模块可见性时的说明。
    pub(crate) fn check_class(
        &self,
        db: &dyn Db,
        graph: &ModuleGraph,
        class: ClassId,
    ) -> Option<String> {
        let package = package_of(&class.binary_name(db));
        let owner = self.owner_of_class(db, graph, class)?;
        self.check(graph, &package, &owner)
    }

    /// 按需导入整个包 `package` 时的检查（包里的类都来自类路径上同一个模块时）。
    pub(crate) fn check_package(&self, graph: &ModuleGraph, package: &str) -> Option<String> {
        let owner = self.owner_of_package(graph, package)?;
        self.check(graph, package, &owner)
    }

    fn check(&self, graph: &ModuleGraph, package: &str, owner: &str) -> Option<String> {
        if package.is_empty() || owner == self.own {
            return None;
        }
        let dotted = package.replace('/', ".");
        let own = &self.own;
        if !self.readable.contains(owner) {
            return Some(base_db::tr!(
                "包 `{dotted}` 不可见：它在模块 `{owner}` 里，而模块 `{own}` 没有 requires 它",
                "Package `{dotted}` is not visible: it is declared in module `{owner}`, but module `{own}` does not read it"
            ));
        }
        let info = graph.get(owner)?;
        let exported = info.exports.iter().any(|(p, targets)| {
            p == package && (targets.is_empty() || targets.iter().any(|t| t == own))
        });
        if exported {
            return None;
        }
        Some(base_db::tr!(
            "包 `{dotted}` 不可见：模块 `{owner}` 没有把它导出给模块 `{own}`",
            "Package `{dotted}` is not visible: module `{owner}` does not export it to module `{own}`"
        ))
    }
}

fn package_of(binary_name: &str) -> String {
    binary_name
        .rsplit_once('/')
        .map_or_else(String::new, |(package, _)| package.to_owned())
}
