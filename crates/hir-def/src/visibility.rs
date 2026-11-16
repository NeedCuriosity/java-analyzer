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
//! 同名的类出现在多个模块、源码和 jar 或多个 jar 里时，每一份是一个独立的 `ClassId`。
//! 从文件出发按名字解析时（[`lookup_class_from`]）按 javac 类路径的顺序挑看得到的那一份：
//! 本模块（测试代码先看本模块的测试代码）→ 依赖（按依赖列表的顺序，模块和依赖库交错）→
//! 散落文件 → 谁都能看到的类路径容器（JDK 等）。

use std::{path::Path, sync::Arc};

use base_db::{
    workspace, Db, DependencyTarget, FileKind, ModuleId, Scope, SourceFile, SourceRootKind,
    Workspace,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    class_in_origin, has_copies, lookup_class, package_classes, package_exists, source_class_index,
    ClassId, ClassOrigin,
};

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
    /// `(模块, 是否测试代码)` → 它在依赖列表里的位置（同名类挑副本时的先后）
    module_ranks: FxHashMap<(u32, bool), u32>,
    /// 依赖库的类路径容器 → 它在依赖列表里的位置
    container_ranks: FxHashMap<u32, u32>,
}

/// 同名类各副本的先后：本模块的测试代码、本模块的主代码、依赖（按列表顺序）、散落文件、公共容器。
const RANK_OWN_TESTS: u32 = 0;
const RANK_OWN_MAIN: u32 = 1;
const RANK_DEPENDENCY: u32 = 2;
const RANK_UNRESTRICTED: u32 = 1 << 24;
const RANK_SHARED: u32 = 1 << 25;

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
                module_ranks: FxHashMap::default(),
                container_ranks: FxHashMap::default(),
            };
            sight.main.insert(idx);
            if test {
                sight.tests.insert(idx);
            }
            for (pos, dep) in module.dependencies.iter().enumerate() {
                // 主代码的编译类路径不含 test / runtime scope 的依赖
                if !test && matches!(dep.scope, Scope::Test | Scope::Runtime) {
                    continue;
                }
                let pos = pos as u32;
                match dep.target {
                    DependencyTarget::Module(id) => {
                        sight.main.insert(id.0 as usize);
                        sight.module_ranks.entry((id.0, false)).or_insert(pos);
                    }
                    DependencyTarget::ModuleTests(id) => {
                        sight.tests.insert(id.0 as usize);
                        sight.module_ranks.entry((id.0, true)).or_insert(pos);
                    }
                    DependencyTarget::Library(id) => {
                        let jar = project.libraries.get(id.0 as usize).map(|lib| &lib.jar);
                        for &container in jar
                            .and_then(|jar| jar_containers.get(jar.as_path()))
                            .into_iter()
                            .flatten()
                        {
                            sight.containers.insert(container);
                            sight.container_ranks.entry(container as u32).or_insert(pos);
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

/// 文件所在的模块和是不是测试代码（生成代码按测试代码宽松处理）。散落文件、库源码文件为 `None`。
fn file_place(db: &dyn Db, file: SourceFile) -> Option<(ModuleId, bool)> {
    match file_kind_of(db, file)? {
        FileKind::Workspace {
            module: Some(module),
            root,
        } => Some((module, root != SourceRootKind::Main)),
        _ => None,
    }
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
    if file_sight(db, from).is_none() {
        return true;
    }
    let names = package_classes(db, package);
    names.is_empty()
        || names.iter().any(|name| {
            let binary = format!("{package}/{name}");
            lookup_class(db, &binary)
                .is_none_or(|_| lookup_class_from(db, Some(from), &binary).is_some())
        })
}

/// 在文件 `from` 里，二进制名 `binary_name` 指向哪一份类：只考虑这个文件看得到的副本，
/// 有多份时按类路径的先后挑（见模块文档）。看不到任何一份时返回 `None`。
/// `from` 未知（库里的签名等）时就是主副本。
pub fn lookup_class_from(
    db: &dyn Db,
    from: Option<SourceFile>,
    binary_name: &str,
) -> Option<ClassId> {
    // 这里在名字解析的热路径上：索引只取一次，副本的判断和 `has_copies` 相同
    let index = source_class_index(db);
    let class_path = workspace(db).class_path(db);
    let source = index.get_with_kind(binary_name);
    let in_class_path = class_path.contains(binary_name);
    if source.is_none() && !in_class_path {
        // 只出现在打开的库源码文件里的类不受限制
        return crate::library_decls::library_source_index(db)
            .get(binary_name)
            .map(|_| ClassId::from_binary_name(db, binary_name));
    }
    let primary = ClassId::from_binary_name(db, binary_name);
    let Some(from) = from else {
        return Some(primary);
    };
    let copies = match source {
        Some(_) => in_class_path || index.is_shadowed(binary_name),
        None => class_path.is_shadowed(binary_name),
    };
    if !copies {
        return is_visible_from(db, Some(from), primary).then_some(primary);
    }
    let Some((module, test)) = file_place(db, from) else {
        // 散落文件、库源码文件：自己声明的那份优先；库源码文件里引用的是类路径上的类；否则主副本
        let declared = class_in_origin(db, binary_name, ClassOrigin::Source { file: from, idx: 0 });
        let library = || {
            matches!(file_kind_of(db, from), Some(FileKind::Library))
                .then(|| crate::binary_class(db, binary_name))
                .flatten()
        };
        return Some(declared.or_else(library).unwrap_or(primary));
    };
    // 最常见的情况：主副本就在本模块里，而且没有更靠前的副本
    if let Some((
        _,
        _,
        FileKind::Workspace {
            module: Some(owner),
            root,
        },
    )) = source
    {
        let own_tests = root == SourceRootKind::Test;
        if owner == module && own_tests == test {
            return Some(primary);
        }
    }
    let visibility = visibility_index(db, workspace(db));
    let sight = &visibility.modules.get(module.0 as usize)?[test as usize];
    let origins = index
        .declarations(binary_name)
        .map(|(file, idx)| ClassOrigin::Source { file, idx })
        .chain(
            class_path
                .containers_of(binary_name)
                .map(|container| ClassOrigin::Binary { container }),
        );
    let (_, best) = origins
        .filter_map(|origin| Some((sight.rank(db, module, test, origin)?, origin)))
        .min_by_key(|(rank, _)| *rank)?;
    class_in_origin(db, binary_name, best)
}

impl Sight {
    /// 类的这一份（按 `ClassId` 区分副本）是否看得到。
    fn sees(&self, db: &dyn Db, class: ClassId) -> bool {
        match crate::class_origin(db, class) {
            Some(ClassOrigin::Source { file, .. }) => match file_kind_of(db, file) {
                Some(FileKind::Workspace {
                    module: Some(ModuleId(owner)),
                    root,
                }) => {
                    let visible = match root {
                        SourceRootKind::Test => &self.tests,
                        SourceRootKind::Main | SourceRootKind::Generated => &self.main,
                    };
                    visible.contains(owner as usize)
                }
                // 散落文件和库源码里的类不受限制
                _ => true,
            },
            Some(ClassOrigin::Binary { container }) => self.containers.contains(container as usize),
            None => true,
        }
    }

    /// 同名类的某一份对模块 `module`（`test`：测试代码）的先后，越小越靠前；看不到时为 `None`。
    fn rank(&self, db: &dyn Db, module: ModuleId, test: bool, origin: ClassOrigin) -> Option<u32> {
        match origin {
            ClassOrigin::Source { file, .. } => match file_kind_of(db, file) {
                Some(FileKind::Workspace {
                    module: Some(owner),
                    root,
                }) => {
                    let test_code = root == SourceRootKind::Test;
                    if owner == module {
                        return match (test_code, test) {
                            (true, true) => Some(RANK_OWN_TESTS),
                            (true, false) => None,
                            (false, _) => Some(RANK_OWN_MAIN),
                        };
                    }
                    self.module_ranks
                        .get(&(owner.0, test_code))
                        .map(|pos| RANK_DEPENDENCY + pos)
                }
                _ => Some(RANK_UNRESTRICTED),
            },
            ClassOrigin::Binary { container } => match self.container_ranks.get(&container) {
                Some(pos) => Some(RANK_DEPENDENCY + pos),
                None => self
                    .containers
                    .contains(container as usize)
                    .then_some(RANK_SHARED + container),
            },
        }
    }
}

/// `class` 是不是文件 `from` 里按它的名字会解析到的那一份：看得到，并且同名的类有多份时
/// 正好是被挑中的那一份。补全、导入建议用它避免同一个名字出现多次。
pub fn is_preferred_from(db: &dyn Db, from: SourceFile, class: ClassId) -> bool {
    let name = class.binary_name(db);
    if !class.is_copy() && !has_copies(db, &name) {
        return is_visible_from(db, Some(from), class);
    }
    lookup_class_from(db, Some(from), &name) == Some(class)
}
