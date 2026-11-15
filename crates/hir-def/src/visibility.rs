//! 源码根和依赖 scope 带来的可见性：主源码根里的代码看不到测试源码根里的类，
//! 也看不到所在模块只以 test scope 依赖的库。
//!
//! 类的名字空间仍然是全局的（模块之间不隔离）；这里只处理 main / test 这一层。

use std::{path::PathBuf, sync::Arc};

use base_db::{
    workspace, Db, DependencyTarget, FileKind, ModuleId, Scope, SourceFile, SourceRootKind,
    Workspace,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{class_origin, ClassId, ClassOrigin};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScopeIndex {
    /// 测试源码根下的文件
    test_files: FxHashSet<SourceFile>,
    /// 主源码根下的文件所在的模块
    main_files: FxHashMap<SourceFile, ModuleId>,
    /// 每个模块只以 test scope 依赖的 jar
    test_only_jars: FxHashMap<ModuleId, FxHashSet<PathBuf>>,
}

#[salsa::tracked]
fn scope_index(db: &dyn Db, ws: Workspace) -> Arc<ScopeIndex> {
    let mut index = ScopeIndex::default();
    for (_, entry) in ws.files(db).iter() {
        match entry.kind {
            FileKind::Workspace {
                root: SourceRootKind::Test,
                ..
            } => {
                index.test_files.insert(entry.source);
            }
            FileKind::Workspace {
                module: Some(module),
                ..
            } => {
                index.main_files.insert(entry.source, module);
            }
            _ => {}
        }
    }
    let project = ws.project(db);
    for (idx, module) in project.modules.iter().enumerate() {
        let jars_with = |test: bool| -> FxHashSet<PathBuf> {
            module
                .dependencies
                .iter()
                .filter(|dep| (dep.scope == Scope::Test) == test)
                .filter_map(|dep| match dep.target {
                    DependencyTarget::Library(id) => {
                        project.libraries.get(id.0 as usize).map(|l| l.jar.clone())
                    }
                    DependencyTarget::Module(_) => None,
                })
                .collect()
        };
        let main = jars_with(false);
        let test_only: FxHashSet<PathBuf> = jars_with(true)
            .into_iter()
            .filter(|jar| !main.contains(jar))
            .collect();
        if !test_only.is_empty() {
            index.test_only_jars.insert(ModuleId(idx as u32), test_only);
        }
    }
    Arc::new(index)
}

/// `class` 在文件 `from` 里是否可见。`from` 未知（库里的签名等）时不做限制。
pub fn is_visible_from(db: &dyn Db, from: Option<SourceFile>, class: ClassId) -> bool {
    let Some(from) = from else {
        return true;
    };
    let index = scope_index(db, workspace(db));
    if (index.test_files.is_empty() && index.test_only_jars.is_empty())
        || index.test_files.contains(&from)
    {
        return true;
    }
    match class_origin(db, class) {
        Some(ClassOrigin::Source { file, .. }) => !index.test_files.contains(&file),
        Some(ClassOrigin::Binary) => {
            let Some(jars) = index
                .main_files
                .get(&from)
                .and_then(|module| index.test_only_jars.get(module))
            else {
                return true;
            };
            let name = class.binary_name(db);
            workspace(db)
                .class_path(db)
                .container_path(&name)
                .is_none_or(|jar| !jars.contains(jar))
        }
        None => true,
    }
}
