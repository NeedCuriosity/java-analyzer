//! 类型名字解析（JLS §6.4.1、§6.5）。
//!
//! 简单名的查找顺序：
//! 1. 方法的类型参数；
//! 2. 由内到外每一层类的类型参数、成员类型（含继承来的）；
//! 3. 单类型 import；
//! 4. 同一文件的顶层类型、同包类型；
//! 5. 按需 import（`import a.b.*`）；
//! 6. `java.lang`。

use base_db::{Db, SourceFile};
use rustc_hash::FxHashSet;

use crate::{
    data::{ClassId, GenericOwner, MethodId, Name, TypeParamId, TypeRef},
    index::{class_data, class_decl_file, direct_supers, item_tree, lookup_class},
};

/// 解析类型名时所处的位置。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TypeScope {
    pub file: Option<SourceFile>,
    pub class: Option<ClassId>,
    pub method: Option<MethodId>,
    /// 正在解析 `class` 自己的头部（extends / implements / 类型参数约束）。
    /// 此时 `class` 自身的成员类型不在作用域内，否则会循环依赖。
    pub in_class_header: bool,
}

impl TypeScope {
    pub fn file(file: SourceFile) -> TypeScope {
        TypeScope {
            file: Some(file),
            class: None,
            method: None,
            in_class_header: false,
        }
    }

    pub fn class(db: &dyn Db, class: ClassId) -> TypeScope {
        TypeScope {
            file: class_decl_file(db, class),
            class: Some(class),
            method: None,
            in_class_header: false,
        }
    }

    pub fn class_header(db: &dyn Db, class: ClassId) -> TypeScope {
        TypeScope {
            in_class_header: true,
            ..TypeScope::class(db, class)
        }
    }

    pub fn method(db: &dyn Db, method: MethodId) -> TypeScope {
        TypeScope {
            method: Some(method),
            ..TypeScope::class(db, method.class)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TypeNs {
    Class(ClassId),
    TypeParam(TypeParamId),
}

fn nested_name(outer: &str, name: &str) -> String {
    format!("{outer}${name}")
}

/// `class` 的成员类型 `name`：先看自己声明的，再看继承来的。
pub fn member_type(db: &dyn Db, class: ClassId, name: &str) -> Option<ClassId> {
    if let Some(found) = lookup_class(db, &nested_name(&class.binary_name(db), name)) {
        return Some(found);
    }
    let mut seen = FxHashSet::default();
    seen.insert(class);
    direct_supers(db, class)
        .iter()
        .find_map(|sup| inherited_member_type(db, *sup, name, &mut seen))
}

/// 子类能从 `class` 继承到的成员类型 `name`。
///
/// 父类型自己声明了同名的成员类型时，它隐藏更上层的同名类型：如果这个声明是私有的，
/// 沿这条路径就什么也继承不到（`WeakHashMap` 的私有 `Entry` 挡住了 `Map.Entry`）。
fn inherited_member_type(
    db: &dyn Db,
    class: ClassId,
    name: &str,
    seen: &mut FxHashSet<ClassId>,
) -> Option<ClassId> {
    if !seen.insert(class) {
        return None;
    }
    if let Some(found) = lookup_class(db, &nested_name(&class.binary_name(db), name)) {
        let private = class_data(db, found)
            .is_some_and(|data| data.flags.visibility() == crate::Visibility::Private);
        return (!private).then_some(found);
    }
    direct_supers(db, class)
        .iter()
        .find_map(|sup| inherited_member_type(db, *sup, name, seen))
}

/// 只在类型参数里找（方法的 → 各层类的）。
pub fn resolve_type_param(db: &dyn Db, scope: &TypeScope, name: &str) -> Option<TypeParamId> {
    if let Some(method) = scope.method {
        if let Some(data) = class_data(db, method.class) {
            if let Some(m) = data.methods.get(method.idx as usize) {
                if let Some(idx) = m.type_params.iter().position(|p| p.name == name) {
                    return Some(TypeParamId {
                        owner: GenericOwner::Method(method),
                        idx: idx as u32,
                    });
                }
            }
        }
    }
    let mut current = scope.class;
    while let Some(class) = current {
        let data = class_data(db, class)?;
        if let Some(idx) = data.type_params.iter().position(|p| p.name == name) {
            return Some(TypeParamId {
                owner: GenericOwner::Class(class),
                idx: idx as u32,
            });
        }
        if data.flags.contains(crate::Modifiers::LOCAL_CLASS) {
            let found = local_class_owner(db, class)
                .and_then(|(outer, owner)| owner_method_type_param(db, outer, owner, name));
            if found.is_some() {
                return found;
            }
        }
        current = data
            .outer
            .as_deref()
            .and_then(|outer| lookup_class(db, outer));
    }
    None
}

/// `outer` 的成员 `owner`（方法下标；`None` 是字段初始化式和初始化块）里声明的局部类 `name`。
fn local_class_in(db: &dyn Db, outer: ClassId, owner: Option<u32>, name: &str) -> Option<ClassId> {
    let file = class_decl_file(db, outer)?;
    let tree = item_tree(db, file);
    if tree.local_classes.is_empty() {
        return None;
    }
    let outer_name = outer.binary_name(db);
    tree.local_classes
        .iter()
        .find(|l| l.outer == outer_name && l.owner == owner && l.name == name)
        .map(|l| ClassId::from_binary_name(db, &l.binary_name))
}

/// 局部类（含匿名类）声明在哪里：`(直接外层的类, 所在方法的下标)`；在字段初始化式、
/// 初始化块里时方法下标为 `None`。
pub fn local_class_owner(db: &dyn Db, class: ClassId) -> Option<(ClassId, Option<u32>)> {
    let file = class_decl_file(db, class)?;
    let tree = item_tree(db, file);
    let binary = class.binary_name(db);
    let info = tree
        .local_classes
        .iter()
        .find(|l| l.binary_name == binary)?;
    Some((lookup_class(db, &info.outer)?, info.owner))
}

/// 局部类 `class` 所在的方法的类型参数 `name`（`<T> void m() { class L { T t; } }`）。
fn owner_method_type_param(
    db: &dyn Db,
    outer: ClassId,
    owner: Option<u32>,
    name: &str,
) -> Option<TypeParamId> {
    let method = MethodId {
        class: outer,
        idx: owner?,
    };
    let data = class_data(db, outer)?;
    let idx = data
        .methods
        .get(method.idx as usize)?
        .type_params
        .iter()
        .position(|p| p.name == name)?;
    Some(TypeParamId {
        owner: GenericOwner::Method(method),
        idx: idx as u32,
    })
}

/// 解析一个简单类型名。
pub fn resolve_simple_type(db: &dyn Db, scope: &TypeScope, name: &str) -> Option<TypeNs> {
    if let Some(method) = scope.method {
        if let Some(data) = class_data(db, method.class) {
            if let Some(m) = data.methods.get(method.idx as usize) {
                if let Some(idx) = m.type_params.iter().position(|p| p.name == name) {
                    return Some(TypeNs::TypeParam(TypeParamId {
                        owner: GenericOwner::Method(method),
                        idx: idx as u32,
                    }));
                }
            }
        }
    }

    // 当前体里声明的局部类遮蔽外层的同名类型
    if let Some(class) = scope.class {
        let owner = scope.method.filter(|m| m.class == class).map(|m| m.idx);
        if let Some(found) = local_class_in(db, class, owner, name) {
            return Some(TypeNs::Class(found));
        }
    }

    let mut current = scope.class;
    while let Some(class) = current {
        let Some(data) = class_data(db, class) else {
            break;
        };
        if let Some(idx) = data.type_params.iter().position(|p| p.name == name) {
            return Some(TypeNs::TypeParam(TypeParamId {
                owner: GenericOwner::Class(class),
                idx: idx as u32,
            }));
        }
        let skip_members = scope.in_class_header && Some(class) == scope.class;
        if !skip_members {
            if let Some(found) = member_type(db, class, name) {
                return Some(TypeNs::Class(found));
            }
        }
        // 局部类：和它声明在同一个体里的局部类（包括它自己），然后是所在方法的类型参数
        if data.flags.contains(crate::Modifiers::LOCAL_CLASS) {
            if let Some((outer, owner)) = local_class_owner(db, class) {
                if let Some(found) = local_class_in(db, outer, owner, name) {
                    return Some(TypeNs::Class(found));
                }
                if let Some(param) = owner_method_type_param(db, outer, owner, name) {
                    return Some(TypeNs::TypeParam(param));
                }
            }
        }
        current = data
            .outer
            .as_deref()
            .and_then(|outer| lookup_class(db, outer));
    }

    if let Some(file) = scope.file {
        if let Some(found) = resolve_in_file(db, file, name) {
            return Some(TypeNs::Class(found));
        }
    } else if scope.class.is_none() {
        return lookup_class(db, &format!("java/lang/{name}")).map(TypeNs::Class);
    }
    None
}

fn resolve_in_file(db: &dyn Db, file: SourceFile, name: &str) -> Option<ClassId> {
    // 主源码根里的代码看不到测试源码根和 test scope 依赖里的类
    resolve_in_file_unscoped(db, file, name)
        .filter(|class| crate::is_visible_from(db, Some(file), *class))
}

fn resolve_in_file_unscoped(db: &dyn Db, file: SourceFile, name: &str) -> Option<ClassId> {
    let tree = item_tree(db, file);

    for import in tree.imports.iter().filter(|i| !i.is_on_demand) {
        if import.segments.last().is_some_and(|last| last == name) {
            let found = if import.is_static {
                let (_, qualifier) = import.segments.split_last()?;
                resolve_qualified_class(db, qualifier).and_then(|c| member_type(db, c, name))
            } else {
                resolve_qualified_class(db, &import.segments)
            };
            if found.is_some() {
                return found;
            }
        }
    }

    // 紧凑源文件的隐式类不能按名字引用
    let implicit =
        |class: &crate::ClassData| class.flags.contains(crate::Modifiers::IMPLICIT_CLASS);
    if let Some(class) = tree
        .top_level_classes()
        .find(|c| c.name == name && !implicit(c))
    {
        return Some(ClassId::from_binary_name(db, &class.binary_name));
    }

    let in_package = if tree.package.is_empty() {
        name.to_owned()
    } else {
        format!("{}/{}", tree.package, name)
    };
    if let Some(found) = lookup_class(db, &in_package)
        .filter(|found| class_data(db, *found).is_none_or(|data| !implicit(&data)))
    {
        return Some(found);
    }

    for import in tree.imports.iter().filter(|i| i.is_on_demand) {
        if !import.is_static {
            let package = import.segments.join("/");
            // 按需 import 只带进别的包里的 public 类（`java.io.*` 不会带进包私有的 `java.io.FileSystem`）
            let found = lookup_class(db, &format!("{package}/{name}")).filter(|found| {
                class_data(db, *found)
                    .is_none_or(|data| data.flags.visibility() == crate::Visibility::Public)
            });
            if found.is_some() {
                return found;
            }
        }
        // `import a.b.Outer.*` / `import static a.b.Outer.*` 都能带进成员类型。
        // 和 javac 一致：非静态的只带进 `Outer` 自己声明的，静态的还包括继承来的。
        let found = resolve_qualified_class(db, &import.segments).and_then(|outer| {
            if import.is_static {
                member_type(db, outer, name)
            } else {
                lookup_class(db, &nested_name(&outer.binary_name(db), name))
            }
        });
        if found.is_some() {
            return found;
        }
    }

    if let Some(found) = lookup_class(db, &format!("java/lang/{name}")) {
        return Some(found);
    }

    // 模块导入的优先级最低；两个模块导入带进同名的类时有歧义，不选
    match crate::modules::module_import_candidates(db, file, name).as_slice() {
        [single] => Some(*single),
        _ => None,
    }
}

/// 解析完全限定名 `a.b.Outer.Inner`：最短的包前缀优先（类型遮蔽包）。
pub fn resolve_qualified_class(db: &dyn Db, segments: &[Name]) -> Option<ClassId> {
    for class_pos in 0..segments.len() {
        let mut binary_name = segments[..class_pos].join("/");
        if !binary_name.is_empty() {
            binary_name.push('/');
        }
        binary_name.push_str(&segments[class_pos]);
        if let Some(mut class) = lookup_class(db, &binary_name) {
            for nested in &segments[class_pos + 1..] {
                class = member_type(db, class, nested)?;
            }
            return Some(class);
        }
    }
    None
}

/// 解析一条类型路径：第一段按简单名解析，失败则把前缀当包名；后续各段是成员类型。
pub fn resolve_type_path<S: AsRef<str>>(
    db: &dyn Db,
    scope: &TypeScope,
    segments: &[S],
) -> Option<TypeNs> {
    let (first, rest) = segments.split_first()?;
    if let Some(found) = resolve_simple_type(db, scope, first.as_ref()) {
        return match found {
            TypeNs::Class(mut class) => {
                for nested in rest {
                    class = member_type(db, class, nested.as_ref())?;
                }
                Some(TypeNs::Class(class))
            }
            TypeNs::TypeParam(_) if rest.is_empty() => Some(found),
            TypeNs::TypeParam(_) => None,
        };
    }
    let names: Vec<Name> = segments.iter().map(|s| Name::new(s.as_ref())).collect();
    resolve_qualified_class(db, &names)
        .filter(|class| crate::is_visible_from(db, scope.file, *class))
        .map(TypeNs::Class)
}

/// 把签名里的类型引用解析到它的（擦除后的）类。类型变量、基本类型、数组返回 `None`。
pub fn resolve_type_ref_to_class(db: &dyn Db, scope: &TypeScope, ty: &TypeRef) -> Option<ClassId> {
    match ty {
        TypeRef::Path(segments) => {
            let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
            match resolve_type_path(db, scope, &names)? {
                TypeNs::Class(class) => Some(class),
                TypeNs::TypeParam(_) => None,
            }
        }
        TypeRef::Class { name, .. } => lookup_class(db, name),
        _ => None,
    }
}
