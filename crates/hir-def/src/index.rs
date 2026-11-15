//! 全局类索引：二进制名 → 类的来源与签名；包 → 内容。
//!
//! 当前实现把"工作区源码 + 全部依赖 + JDK"视为一个统一的命名空间：同名类按
//! 源码 > 依赖 > JDK 的顺序取第一个。按模块隔离 classpath 是后续工作。

use std::{cell::RefCell, sync::Arc};

use base_db::{workspace, Db, SourceFile};
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use crate::{
    binary::lower_class_file,
    data::{ClassData, ClassId, ItemTree, Name},
    library_decls::{
        library_only_class, library_source_index, merge_library_source, open_library_source,
    },
    lower::{lower_file, SourceMap},
    resolver::{resolve_type_ref_to_class, TypeScope},
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClassOrigin {
    Source {
        file: SourceFile,
        /// `ItemTree::classes` 的下标
        idx: u32,
    },
    Binary,
}

#[salsa::tracked]
pub fn item_tree_and_map(db: &dyn Db, file: SourceFile) -> (Arc<ItemTree>, Arc<SourceMap>) {
    let parse = base_db::parse(db, file);
    let (tree, map) = lower_file(&parse.tree());
    (Arc::new(tree), Arc::new(map))
}

/// 文件的签名级摘要。方法体内的编辑不会改变返回值，salsa 因此不会让下游失效。
#[salsa::tracked]
pub fn item_tree(db: &dyn Db, file: SourceFile) -> Arc<ItemTree> {
    item_tree_and_map(db, file).0
}

#[salsa::tracked]
pub fn source_map(db: &dyn Db, file: SourceFile) -> Arc<SourceMap> {
    item_tree_and_map(db, file).1
}

/// 工作区源码里所有具名类的索引。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SourceClassIndex {
    classes: FxHashMap<SmolStr, (SourceFile, u32)>,
    package_classes: FxHashMap<SmolStr, Vec<Name>>,
    subpackages: FxHashMap<SmolStr, Vec<Name>>,
    packages: FxHashSet<SmolStr>,
}

impl SourceClassIndex {
    pub fn get(&self, binary_name: &str) -> Option<(SourceFile, u32)> {
        self.classes.get(binary_name).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SmolStr, SourceFile, u32)> {
        self.classes
            .iter()
            .map(|(name, (file, idx))| (name, *file, *idx))
    }

    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    fn register_package(&mut self, package: &str) {
        let mut current = package;
        while !current.is_empty() && self.packages.insert(SmolStr::new(current)) {
            let (parent, last) = match current.rfind('/') {
                Some(i) => (&current[..i], &current[i + 1..]),
                None => ("", current),
            };
            self.subpackages
                .entry(SmolStr::new(parent))
                .or_default()
                .push(Name::new(last));
            current = parent;
        }
    }
}

#[salsa::tracked]
pub fn source_class_index(db: &dyn Db) -> Arc<SourceClassIndex> {
    let mut index = SourceClassIndex::default();
    for (_, file) in workspace(db).files(db).workspace_files() {
        let tree = item_tree(db, file);
        index.register_package(&tree.package);
        for (idx, class) in tree.classes.iter().enumerate() {
            if index.classes.contains_key(&class.binary_name) {
                continue;
            }
            index
                .classes
                .insert(class.binary_name.clone(), (file, idx as u32));
            if class.outer.is_none() {
                index
                    .package_classes
                    .entry(tree.package.clone())
                    .or_default()
                    .push(class.name.clone());
            }
        }
    }
    for names in index.package_classes.values_mut() {
        names.sort_unstable();
    }
    for names in index.subpackages.values_mut() {
        names.sort_unstable();
        names.dedup();
    }
    Arc::new(index)
}

/// 按二进制名查找类；不存在返回 `None`。
pub fn lookup_class(db: &dyn Db, binary_name: &str) -> Option<ClassId> {
    if source_class_index(db).classes.contains_key(binary_name)
        || workspace(db).class_path(db).contains(binary_name)
        || library_source_index(db).get(binary_name).is_some()
    {
        Some(ClassId::from_binary_name(db, binary_name))
    } else {
        None
    }
}

pub fn class_origin(db: &dyn Db, id: ClassId) -> Option<ClassOrigin> {
    let name = id.binary_name(db);
    if let Some((file, idx)) = source_class_index(db).get(&name) {
        return Some(ClassOrigin::Source { file, idx });
    }
    if workspace(db).class_path(db).contains(&name) {
        return Some(ClassOrigin::Binary);
    }
    // 只出现在打开的库源码文件里的类（私有嵌套类等）
    let (file, idx) = open_library_source(db, id)?;
    Some(ClassOrigin::Source { file, idx })
}

/// 类的声明所在的文件：工作区源码，或者（依赖库的类）打开的库源码文件。
/// `ClassData` 里的 `decl` 都是这个文件的 `SourceMap` 下标。
pub fn class_decl_file(db: &dyn Db, id: ClassId) -> Option<SourceFile> {
    match class_origin(db, id)? {
        ClassOrigin::Source { file, .. } => Some(file),
        ClassOrigin::Binary => open_library_source(db, id).map(|(file, _)| file),
    }
}

/// 类所在的源码文件（二进制类返回 `None`）。
pub fn class_source_file(db: &dyn Db, id: ClassId) -> Option<SourceFile> {
    match class_origin(db, id)? {
        ClassOrigin::Source { file, .. } => Some(file),
        ClassOrigin::Binary => None,
    }
}

/// 类的签名。源码类取自 `ItemTree`，二进制类按需读取 class 文件。
pub fn class_data(db: &dyn Db, id: ClassId) -> Option<Arc<ClassData>> {
    class_data_tracked(db, base_db::workspace(db), id)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked]
fn class_data_tracked(db: &dyn Db, _ws: base_db::Workspace, id: ClassId) -> Option<Arc<ClassData>> {
    match class_origin(db, id)? {
        ClassOrigin::Source { file, idx } => {
            let data = item_tree(db, file).classes.get(idx as usize).cloned()?;
            let is_library = workspace(db)
                .files(db)
                .get(file.file_id(db))
                .is_some_and(|entry| matches!(entry.kind, base_db::FileKind::Library));
            if is_library {
                Some(Arc::new(library_only_class(&data)))
            } else {
                Some(data)
            }
        }
        ClassOrigin::Binary => {
            let name = id.binary_name(db);
            let class_file = workspace(db).class_path(db).load(&name)?;
            let mut data = lower_class_file(&class_file);
            if let Some((file, idx)) = open_library_source(db, id) {
                if let Some(source) = item_tree(db, file).classes.get(idx as usize) {
                    merge_library_source(&mut data, source);
                }
            }
            Some(Arc::new(data))
        }
    }
}

pub fn package_exists(db: &dyn Db, package: &str) -> bool {
    source_class_index(db).packages.contains(package)
        || workspace(db).class_path(db).has_package(package)
}

/// 包下的顶层类简单名（源码与二进制合并、去重、排序）。
pub fn package_classes(db: &dyn Db, package: &str) -> Vec<Name> {
    let index = source_class_index(db);
    let mut res: Vec<Name> = index
        .package_classes
        .get(package)
        .cloned()
        .unwrap_or_default();
    res.extend(
        workspace(db)
            .class_path(db)
            .package_classes(package)
            .iter()
            .cloned(),
    );
    res.sort_unstable();
    res.dedup();
    res
}

/// 直接子包的最后一段名字；`""` 表示根。
pub fn subpackages(db: &dyn Db, package: &str) -> Vec<Name> {
    let index = source_class_index(db);
    let mut res: Vec<Name> = index.subpackages.get(package).cloned().unwrap_or_default();
    res.extend(
        workspace(db)
            .class_path(db)
            .subpackages(package)
            .iter()
            .cloned(),
    );
    res.sort_unstable();
    res.dedup();
    res
}

// ---------------------------------------------------------------------------
// 父类型
// ---------------------------------------------------------------------------

thread_local! {
    /// 正在解析父类型的类。只用于在非法的循环继承上截断递归，避免 salsa 报循环。
    static RESOLVING_SUPERS: RefCell<Vec<ClassId>> = const { RefCell::new(Vec::new()) };
}

fn direct_supers_query(db: &dyn Db, id: ClassId) -> Arc<Vec<ClassId>> {
    direct_supers_query_tracked(db, base_db::workspace(db), id)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked]
fn direct_supers_query_tracked(
    db: &dyn Db,
    _ws: base_db::Workspace,
    id: ClassId,
) -> Arc<Vec<ClassId>> {
    let Some(data) = class_data(db, id) else {
        return Arc::default();
    };
    let scope = TypeScope::class_header(db, id);
    let mut res = Vec::new();
    for ty in data.superclass.iter().chain(data.interfaces.iter()) {
        if let Some(class) = resolve_type_ref_to_class(db, &scope, ty) {
            if class != id && !res.contains(&class) {
                res.push(class);
            }
        }
    }
    Arc::new(res)
}

/// 直接父类型（父类在前，接口在后），已擦除类型实参。
pub fn direct_supers(db: &dyn Db, id: ClassId) -> Arc<Vec<ClassId>> {
    let cyclic = RESOLVING_SUPERS.with(|stack| stack.borrow().contains(&id));
    if cyclic {
        return Arc::default();
    }
    RESOLVING_SUPERS.with(|stack| stack.borrow_mut().push(id));
    let res = direct_supers_query(db, id);
    RESOLVING_SUPERS.with(|stack| {
        stack.borrow_mut().pop();
    });
    res
}

/// 所有祖先类型（不含自身），广度优先；`java.lang.Object` 总在其中（如果存在）。
pub fn all_supers(db: &dyn Db, id: ClassId) -> Vec<ClassId> {
    let mut res: Vec<ClassId> = Vec::new();
    let mut seen: FxHashSet<ClassId> = FxHashSet::default();
    seen.insert(id);
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(id);
    while let Some(current) = queue.pop_front() {
        for sup in direct_supers(db, current).iter() {
            if seen.insert(*sup) {
                res.push(*sup);
                queue.push_back(*sup);
            }
        }
    }
    // 接口没有父类，但 Object 的成员对它同样可用
    if let Some(object) = lookup_class(db, "java/lang/Object") {
        if object != id && seen.insert(object) {
            res.push(object);
        }
    }
    res
}

/// `sub` 是否等于 `sup` 或是它的子类型（按擦除后的类判断）。
/// `sealed` 类型允许的直接子类型。没写 `permits` 时是同一个编译单元里直接继承它的类型。
/// 不是 sealed、或者有解析不了的子类型时返回 `None`。
pub fn permitted_subclasses(db: &dyn Db, id: ClassId) -> Option<Vec<ClassId>> {
    let data = class_data(db, id)?;
    if !data.permits.is_empty() {
        let scope = TypeScope::class_header(db, id);
        return data
            .permits
            .iter()
            .map(|ty| resolve_type_ref_to_class(db, &scope, ty))
            .collect();
    }
    if !data.flags.contains(crate::data::Modifiers::SEALED) {
        return None;
    }
    let file = class_source_file(db, id)?;
    let tree = item_tree(db, file);
    Some(
        tree.classes
            .iter()
            .map(|class| ClassId::from_binary_name(db, &class.binary_name))
            .filter(|class| *class != id && direct_supers(db, *class).contains(&id))
            .collect(),
    )
}

pub fn is_subclass_of(db: &dyn Db, sub: ClassId, sup: ClassId) -> bool {
    sub == sup || all_supers(db, sub).contains(&sup)
}
