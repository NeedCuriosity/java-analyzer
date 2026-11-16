//! 类的可见性按工程图里的模块依赖来定，和 javac 编译一个模块时的类路径一致：
//!
//! - 主源码根里的代码看到：本模块的主代码（含生成代码）、以非 test / runtime scope 依赖的
//!   模块的主代码和依赖库、以 `test-jar` 依赖的模块的测试代码；
//! - 测试源码根里的代码另外看到本模块的测试代码和所有 scope 的依赖；
//! - JDK 和不属于任何依赖库的类路径容器谁都能看到。
//!
//! 导入器（Maven / Gradle）已经把传递依赖展开在 `ModuleData::dependencies` 里，这里只看直接列出的依赖。
//! 不在任何模块里的散落文件、库源码文件以及生成代码不受限制（生成代码分不清主 / 测试，宽松处理）。
//!
//! 类按二进制名标识：同名的类出现在多个模块或多个 jar 里时，只要其中一份可见就算可见。

use std::{path::Path, sync::Arc};

use base_db::{
    workspace, Db, DependencyTarget, FileKind, ModuleId, Scope, SourceFile, SourceRootKind,
    Workspace,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{lookup_class, package_classes, package_exists, source_class_index, ClassId};

/// 定长位集，下标是模块号或类路径容器号。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    fn with_len(len: usize) -> BitSet {
        BitSet {
            words: vec![0; len.div_ceil(64)],
        }
    }

    fn insert(&mut self, idx: usize) {
        if let Some(word) = self.words.get_mut(idx / 64) {
            *word |= 1 << (idx % 64);
        }
    }

    fn contains(&self, idx: usize) -> bool {
        self.words
            .get(idx / 64)
            .is_some_and(|word| word & (1 << (idx % 64)) != 0)
    }

    fn is_superset(&self, other: &BitSet) -> bool {
        self.words
            .iter()
            .zip(&other.words)
            .all(|(mine, theirs)| theirs & !mine == 0)
    }
}

/// 一个模块里的一种代码（主代码或测试代码）能看到的范围。
#[derive(Debug, Default, PartialEq, Eq)]
struct Sight {
    /// 能看到工作区和类路径上的全部类，不必逐个检查
    all: bool,
    /// 能看到主代码（含生成代码）的模块
    main: BitSet,
    /// 能看到测试代码的模块
    tests: BitSet,
    /// 能看到的类路径容器
    containers: BitSet,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct VisibilityIndex {
    /// 每个模块两份：`[主代码, 测试代码]`
    modules: Vec<[Sight; 2]>,
}

/// 每个模块能看到什么。取决于工程图、类路径和哪些模块里有文件；文件增删后结果通常不变，
/// 依赖它的查询不会失效。
#[salsa::tracked]
fn visibility_index(db: &dyn Db, ws: Workspace) -> Arc<VisibilityIndex> {
    let project = ws.project(db);
    let class_path = ws.class_path(db);
    let module_count = project.modules.len();
    let container_count = class_path.container_count();

    // jar → 类路径容器；不属于任何依赖库的容器（JDK 等）谁都能看到
    let mut jar_containers: FxHashMap<&Path, Vec<usize>> = FxHashMap::default();
    for idx in 0..container_count {
        if let Some(path) = class_path.container_path_at(idx) {
            jar_containers.entry(path).or_default().push(idx);
        }
    }
    let library_jars: FxHashSet<&Path> =
        project.libraries.iter().map(|l| l.jar.as_path()).collect();
    let mut shared = BitSet::with_len(container_count);
    for idx in 0..container_count {
        let owned = class_path
            .container_path_at(idx)
            .is_some_and(|path| library_jars.contains(path));
        if !owned {
            shared.insert(idx);
        }
    }
    // 实际有主代码 / 测试代码文件的模块：能看到它们全部的代码不必逐个检查
    let mut all_main = BitSet::with_len(module_count);
    let mut all_tests = BitSet::with_len(module_count);
    for (_, entry) in ws.files(db).iter() {
        if let FileKind::Workspace {
            module: Some(module),
            root,
        } = entry.kind
        {
            match root {
                SourceRootKind::Test => all_tests.insert(module.0 as usize),
                SourceRootKind::Main | SourceRootKind::Generated => {
                    all_main.insert(module.0 as usize)
                }
            }
        }
    }
    let mut all_containers = BitSet::with_len(container_count);
    for idx in 0..container_count {
        all_containers.insert(idx);
    }

    let mut index = VisibilityIndex::default();
    for (idx, module) in project.modules.iter().enumerate() {
        let sight = |test: bool| {
            let mut sight = Sight {
                all: false,
                main: BitSet::with_len(module_count),
                tests: BitSet::with_len(module_count),
                containers: shared.clone(),
            };
            sight.main.insert(idx);
            if test {
                sight.tests.insert(idx);
            }
            for dep in &module.dependencies {
                // 主代码的编译类路径不含 test / runtime scope 的依赖
                if !test && matches!(dep.scope, Scope::Test | Scope::Runtime) {
                    continue;
                }
                match dep.target {
                    DependencyTarget::Module(id) => sight.main.insert(id.0 as usize),
                    DependencyTarget::ModuleTests(id) => sight.tests.insert(id.0 as usize),
                    DependencyTarget::Library(id) => {
                        let jar = project.libraries.get(id.0 as usize).map(|lib| &lib.jar);
                        for &container in jar
                            .and_then(|jar| jar_containers.get(jar.as_path()))
                            .into_iter()
                            .flatten()
                        {
                            sight.containers.insert(container);
                        }
                    }
                }
            }
            sight.all = sight.main.is_superset(&all_main)
                && sight.tests.is_superset(&all_tests)
                && sight.containers.is_superset(&all_containers);
            sight
        };
        index.modules.push([sight(false), sight(true)]);
    }
    Arc::new(index)
}

/// 文件在工程里的角色。单独做成查询：文件集合变化时只要这个文件的角色没变，依赖它的结果就不失效。
#[salsa::tracked]
fn file_kind_of(db: &dyn Db, file: SourceFile) -> Option<FileKind> {
    workspace(db)
        .files(db)
        .get(file.file_id(db))
        .map(|e| e.kind)
}

/// 文件能看到的范围：模块号和是不是测试代码。散落文件、库源码文件，以及能看到全部类的文件为 `None`。
/// 按文件缓存：最常见的"什么都看得到"只要一次查询就能确定。
#[salsa::tracked]
fn file_sight(db: &dyn Db, file: SourceFile) -> Option<(ModuleId, bool)> {
    let (module, test) = match file_kind_of(db, file)? {
        FileKind::Workspace {
            module: Some(module),
            root,
        } => (module, root != SourceRootKind::Main),
        _ => return None,
    };
    let index = visibility_index(db, workspace(db));
    let sight = index.modules.get(module.0 as usize)?;
    (!sight[test as usize].all).then_some((module, test))
}

fn with_sight<R>(db: &dyn Db, from: SourceFile, f: impl FnOnce(&Sight) -> R) -> Option<R> {
    let (module, test) = file_sight(db, from)?;
    let index = visibility_index(db, workspace(db));
    let sight = index.modules.get(module.0 as usize)?;
    Some(f(&sight[test as usize]))
}

/// `class` 在文件 `from` 里是否可见。`from` 未知（库里的签名等）时不做限制。
pub fn is_visible_from(db: &dyn Db, from: Option<SourceFile>, class: ClassId) -> bool {
    let Some(from) = from else {
        return true;
    };
    with_sight(db, from, |sight| sight.sees(db, class)).unwrap_or(true)
}

/// 包 `package`（斜杠形式）在文件 `from` 里是否存在：包里至少有一个这个文件看得到的顶层类。
/// 包里没有类（只有子包）时只看包是否存在。
pub fn package_visible_from(db: &dyn Db, from: SourceFile, package: &str) -> bool {
    if !package_exists(db, package) {
        return false;
    }
    with_sight(db, from, |sight| {
        let names = package_classes(db, package);
        names.is_empty()
            || names.iter().any(|name| {
                lookup_class(db, &format!("{package}/{name}"))
                    .is_none_or(|class| sight.sees(db, class))
            })
    })
    .unwrap_or(true)
}

impl Sight {
    fn sees(&self, db: &dyn Db, class: ClassId) -> bool {
        let name = class.binary_name(db);
        let mut known = false;
        for file in source_class_index(db).declarations(&name) {
            known = true;
            match file_kind_of(db, file) {
                Some(FileKind::Workspace {
                    module: Some(ModuleId(owner)),
                    root,
                }) => {
                    let visible = match root {
                        SourceRootKind::Test => &self.tests,
                        SourceRootKind::Main | SourceRootKind::Generated => &self.main,
                    };
                    if visible.contains(owner as usize) {
                        return true;
                    }
                }
                // 散落文件和库源码里的类不受限制
                _ => return true,
            }
        }
        for container in workspace(db).class_path(db).containers_of(&name) {
            known = true;
            if self.containers.contains(container as usize) {
                return true;
            }
        }
        // 只出现在打开的库源码文件里的类（私有嵌套类等）
        !known
    }
}
