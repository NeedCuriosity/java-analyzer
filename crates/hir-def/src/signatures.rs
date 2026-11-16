//! 比 [`class_data`] 更细的签名查询。
//!
//! `class_data` 是一个类的全部签名：类里任何一个成员的签名变了（加了形参、改了字段的类型），
//! 它就变了，直接读它的查询（各个体的推断和检查）也就全都要重算。一个体通常只用到类的头部和
//! 少数几个成员，这里把类的签名切成两层：类的头部（含各个成员的名字），以及按成员缓存的单个
//! 成员的签名（类型层的 `method_sig` / `field_sig`，见 [`MethodRef`]）。`class_data` 变化后
//! 头部会重新执行（不复制数据，很便宜），但只要成员的名字没变，结果就和上次相等，salsa 不会
//! 让只依赖头部的查询重算。
//!
//! 每个缓存和每条依赖边都占内存：切分只做到够用的粒度。按名字查成员不另外缓存（每个类或每个
//! 名字一份时，每个体依赖的查询成倍增加，依赖边占的内存比省下的重算更多），直接在头部里找。
//!
//! 这些结果都不比较声明下标（[`crate::DeclIdx`]）：前面新增一个声明会让后面所有声明的下标
//! 挪动，比较它就会让无关的成员看起来变了。要找声明的地方仍然用 `class_data`。

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use base_db::{Db, SourceFile};
use smol_str::SmolStr;

use crate::{
    data::{
        ClassData, ClassId, DeclIdx, FieldData, Import, ItemTree, MethodData, MethodId, Modifiers,
        Name, TypeParamData,
    },
    index::{class_data, item_tree},
};

/// 类的头部：修饰符、类型参数、父类型、外层类、注解等，再加上各个成员的名字和泛型方法的
/// 类型参数（按名字查成员、解析方法里的类型名要用）。不复制数据，和类的签名共用一份。
///
/// 相等只比较这些部分（不比较成员的类型、修饰符和声明下标）：作为查询结果时，改成员的签名
/// 不会让它看起来变了。通过它只应该读这些部分；成员的签名要通过按成员缓存的查询读。
#[derive(Clone)]
pub struct ClassHeader(Arc<ClassData>);

impl std::ops::Deref for ClassHeader {
    type Target = ClassData;

    fn deref(&self) -> &ClassData {
        &self.0
    }
}

impl ClassHeader {
    /// 各个方法的名字、是不是构造器和类型参数，按下标。
    fn method_names(&self) -> impl Iterator<Item = (&Name, bool, &[TypeParamData])> + '_ {
        self.0
            .methods
            .iter()
            .map(|m| (&m.name, m.is_constructor, m.type_params.as_slice()))
    }

    /// 各个字段的名字，按下标。
    fn field_names(&self) -> impl Iterator<Item = &Name> + '_ {
        self.0.fields.iter().map(|f| &f.name)
    }
}

impl PartialEq for ClassHeader {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&*self.0, &*other.0);
        let ClassData {
            name,
            binary_name,
            kind,
            flags,
            type_params,
            superclass,
            interfaces,
            permits,
            fields: _,
            methods: _,
            nested,
            outer,
            annotations,
            targets,
            decl: _,
            is_source,
        } = a;
        *name == b.name
            && *binary_name == b.binary_name
            && *kind == b.kind
            && *flags == b.flags
            && *type_params == b.type_params
            && *superclass == b.superclass
            && *interfaces == b.interfaces
            && *permits == b.permits
            && *nested == b.nested
            && *outer == b.outer
            && *annotations == b.annotations
            && *targets == b.targets
            && *is_source == b.is_source
            && self.method_names().eq(other.method_names())
            && self.field_names().eq(other.field_names())
    }
}

impl Eq for ClassHeader {}

impl std::fmt::Debug for ClassHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClassHeader")
            .field("binary_name", &self.0.binary_name)
            .finish_non_exhaustive()
    }
}

/// 类的头部（类型参数、修饰符、父类型、外层类、成员的名字……），不含成员的签名。
pub fn class_header(db: &dyn Db, id: ClassId) -> Option<ClassHeader> {
    class_header_tracked(db, base_db::workspace(db), id)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked]
fn class_header_tracked(db: &dyn Db, _ws: base_db::Workspace, id: ClassId) -> Option<ClassHeader> {
    class_data(db, id).map(ClassHeader)
}

// ---------------------------------------------------------------------------
// 单个成员
// ---------------------------------------------------------------------------

/// 类里的一个方法或构造器的签名。不复制数据，和类的签名共用一份。
///
/// 相等只比较这个方法自己（不比较声明下标）：作为按成员缓存的查询结果时，类里别的成员变化
/// 不会让它看起来变了。通过它读到的 `decl` 是当前的值，但别的声明挪动下标时依赖它的查询
/// 不会因此重算，不要拿 `decl` 做缓存的结果。
#[derive(Clone)]
pub struct MethodRef {
    class: Arc<ClassData>,
    idx: u32,
}

impl MethodRef {
    pub fn new(class: Arc<ClassData>, idx: u32) -> Option<MethodRef> {
        class.methods.get(idx as usize)?;
        Some(MethodRef { class, idx })
    }
}

impl std::ops::Deref for MethodRef {
    type Target = MethodData;

    fn deref(&self) -> &MethodData {
        &self.class.methods[self.idx as usize]
    }
}

impl PartialEq for MethodRef {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&**self, &**other);
        let MethodData {
            name,
            is_constructor,
            type_params,
            params,
            ret,
            throws,
            flags,
            has_body,
            decl: _,
        } = a;
        *name == b.name
            && *is_constructor == b.is_constructor
            && *type_params == b.type_params
            && *params == b.params
            && *ret == b.ret
            && *throws == b.throws
            && *flags == b.flags
            && *has_body == b.has_body
    }
}

impl Eq for MethodRef {}

impl Hash for MethodRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.params.len().hash(state);
    }
}

impl std::fmt::Debug for MethodRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

/// 类里的一个字段的签名，同 [`MethodRef`]。
#[derive(Clone)]
pub struct FieldRef {
    class: Arc<ClassData>,
    idx: u32,
}

impl FieldRef {
    pub fn new(class: Arc<ClassData>, idx: u32) -> Option<FieldRef> {
        class.fields.get(idx as usize)?;
        Some(FieldRef { class, idx })
    }
}

impl std::ops::Deref for FieldRef {
    type Target = FieldData;

    fn deref(&self) -> &FieldData {
        &self.class.fields[self.idx as usize]
    }
}

impl PartialEq for FieldRef {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&**self, &**other);
        let FieldData {
            name,
            ty,
            flags,
            decl: _,
            constant,
        } = a;
        *name == b.name && *ty == b.ty && *flags == b.flags && *constant == b.constant
    }
}

impl Eq for FieldRef {}

impl Hash for FieldRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl std::fmt::Debug for FieldRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

/// 方法 `method` 的类型参数里名为 `name` 的那个的下标。
pub fn method_type_param(db: &dyn Db, method: MethodId, name: &str) -> Option<u32> {
    // 类的头部带着各个泛型方法的类型参数
    let header = class_header(db, method.class)?;
    let m = header.methods.get(method.idx as usize)?;
    m.type_params
        .iter()
        .position(|p| p.name == name)
        .map(|i| i as u32)
}

// ---------------------------------------------------------------------------
// 按名字查成员
// ---------------------------------------------------------------------------

/// `class` 自己声明的名为 `name` 的方法（不含构造器）在 `ClassData::methods` 里的下标。
pub fn methods_named(db: &dyn Db, class: ClassId, name: &str) -> Vec<u32> {
    class_header(db, class)
        .map(|header| header.methods_named(name).map(|(idx, _)| idx).collect())
        .unwrap_or_default()
}

/// `class` 自己声明的名为 `name` 的字段在 `ClassData::fields` 里的下标。
pub fn field_named(db: &dyn Db, class: ClassId, name: &str) -> Option<u32> {
    class_header(db, class)?
        .field_named(name)
        .map(|(idx, _)| idx)
}

/// `class` 自己声明的构造器在 `ClassData::methods` 里的下标。
pub fn constructor_indices(db: &dyn Db, class: ClassId) -> Vec<u32> {
    class_header(db, class)
        .map(|header| header.constructors().map(|(idx, _)| idx).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 文件
// ---------------------------------------------------------------------------

/// 文件的包名、import 和其中的类：解析类型名、判断有没有 Lombok 时用。改成员的签名、新增成员
/// 都不改变它（新增 import 会）。不复制数据，和文件的签名摘要共用一份；相等只比较这些部分。
#[derive(Clone)]
pub struct FileScope(Arc<ItemTree>);

impl FileScope {
    /// 包名（斜杠形式），默认包为空串。
    pub fn package(&self) -> &str {
        &self.0.package
    }

    pub fn imports(&self) -> &[Import] {
        &self.0.imports
    }

    /// `import module M;` 的模块名（点分形式）。
    pub fn module_imports(&self) -> &[SmolStr] {
        &self.0.module_imports
    }

    /// 能按简单名引用的顶层类型：`(简单名, 二进制名)`，不含紧凑源文件的隐式类。
    pub fn top_level(&self) -> impl Iterator<Item = (&Name, &SmolStr)> {
        self.0
            .top_level_classes()
            .filter(|c| !c.flags.contains(Modifiers::IMPLICIT_CLASS))
            .map(|c| (&c.name, &c.binary_name))
    }

    /// 紧凑源文件的隐式类。
    pub fn implicit(&self) -> Option<&SmolStr> {
        self.0
            .classes
            .iter()
            .find(|c| c.flags.contains(Modifiers::IMPLICIT_CLASS))
            .map(|c| &c.binary_name)
    }

    /// 文件里有没有这个二进制名的类（含成员类、局部类）。
    pub fn contains(&self, binary_name: &str) -> bool {
        self.0.classes.iter().any(|c| c.binary_name == binary_name)
    }

    /// 文件是否 import 了 Lombok 的东西（`import lombok.*`、`import lombok.Getter` 等）：
    /// 这样的文件里的类可能有凭空生成的成员。
    pub fn imports_lombok(&self) -> bool {
        self.0
            .imports
            .iter()
            .any(|i| i.segments.first().is_some_and(|s| s == "lombok"))
    }

    pub fn static_imports(&self) -> impl Iterator<Item = &Import> {
        self.0.imports.iter().filter(|i| i.is_static)
    }

    /// 各个类的名字、是否顶层、是否隐式类，按 `ItemTree::classes` 的顺序。
    fn classes(&self) -> impl Iterator<Item = (&Name, &SmolStr, bool, bool)> {
        self.0.classes.iter().map(|c| {
            (
                &c.name,
                &c.binary_name,
                c.outer.is_none(),
                c.flags.contains(Modifiers::IMPLICIT_CLASS),
            )
        })
    }
}

impl PartialEq for FileScope {
    fn eq(&self, other: &Self) -> bool {
        self.0.package == other.0.package
            && self.0.imports == other.0.imports
            && self.0.module_imports == other.0.module_imports
            && self.classes().eq(other.classes())
    }
}

impl Eq for FileScope {}

impl std::fmt::Debug for FileScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileScope")
            .field("package", &self.0.package)
            .finish_non_exhaustive()
    }
}

#[salsa::tracked]
pub fn file_scope(db: &dyn Db, file: SourceFile) -> FileScope {
    FileScope(item_tree(db, file))
}

/// 文件里各个类（含局部类、匿名类）的声明下标和二进制名，按声明下标排序。
#[salsa::tracked]
pub fn class_decls(db: &dyn Db, file: SourceFile) -> Arc<Vec<(DeclIdx, SmolStr)>> {
    let mut res: Vec<(DeclIdx, SmolStr)> = item_tree(db, file)
        .classes
        .iter()
        .filter_map(|c| Some((c.decl?, c.binary_name.clone())))
        .collect();
    res.sort();
    Arc::new(res)
}

/// 文件里声明在 `decl` 处的类的二进制名。
pub fn class_at_decl(db: &dyn Db, file: SourceFile, decl: DeclIdx) -> Option<SmolStr> {
    let decls = class_decls(db, file);
    let at = decls.binary_search_by_key(&decl, |(idx, _)| *idx).ok()?;
    Some(decls[at].1.clone())
}
