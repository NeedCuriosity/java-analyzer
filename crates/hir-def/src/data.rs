//! 统一的签名模型：源码和 class 文件都降为这里的结构，上层不关心符号的来源。

use base_db::Db;
use smol_str::SmolStr;

pub type Name = SmolStr;

/// 一个具名的类 / 接口 / 枚举 / 记录 / 注解类型，用二进制名（`java/util/Map$Entry`）标识。
///
/// 内部是驻留后的字符串 id：复制和比较都是整数操作，并且跨编辑稳定。
/// 局部类和匿名类也有 `ClassId`（`Outer$1Local`、`Outer$1`），但它们的成员仍在外层的体里就地推断。
///
/// 同一个二进制名可能有多份声明（多个模块、源码和 jar、多个 jar 里各有一份）。
/// 按"源码（文件号顺序）→ 类路径（容器顺序）"排在第一位的那份是**主副本**，它的 id 就是二进制名本身；
/// 其余的副本各有一个带来源的 id（见 [`CopyOrigin`]），最高位置 1 以便快速区分。
/// 从某个文件出发按名字解析时选哪一份由 `visibility::lookup_class_from` 决定。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ClassId(u32);

/// 非主副本的来源。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CopyOrigin {
    /// 声明在这个工作区源码文件（文件号）里
    Source(u32),
    /// 来自这个类路径容器
    Container(u32),
}

/// 驻留键里二进制名和来源之间的分隔符：二进制名里不会出现。
const COPY_SEPARATOR: char = '\0';

impl ClassId {
    const COPY_BIT: u32 = 1 << 31;

    /// 由二进制名构造主副本的 id。注意：这并不检查类是否存在，存在性用 `lookup_class`。
    pub fn from_binary_name(db: &dyn Db, binary_name: &str) -> ClassId {
        ClassId(db.interner().intern(binary_name))
    }

    /// 非主副本的 id。
    pub(crate) fn copy(db: &dyn Db, binary_name: &str, origin: CopyOrigin) -> ClassId {
        let key = match origin {
            CopyOrigin::Source(file) => format!("{binary_name}{COPY_SEPARATOR}s{file}"),
            CopyOrigin::Container(idx) => format!("{binary_name}{COPY_SEPARATOR}c{idx}"),
        };
        ClassId(db.interner().intern(&key) | Self::COPY_BIT)
    }

    /// 是不是非主副本（同名的类有多份声明，这是不排在第一位的某一份）。
    pub fn is_copy(self) -> bool {
        self.0 & Self::COPY_BIT != 0
    }

    /// 非主副本的来源；主副本返回 `None`。
    pub fn copy_origin(self, db: &dyn Db) -> Option<CopyOrigin> {
        if !self.is_copy() {
            return None;
        }
        let key = db.interner().lookup(self.0 & !Self::COPY_BIT);
        let (_, origin) = key.split_once(COPY_SEPARATOR)?;
        let (kind, num) = origin.split_at(1);
        let num: u32 = num.parse().ok()?;
        match kind {
            "s" => Some(CopyOrigin::Source(num)),
            "c" => Some(CopyOrigin::Container(num)),
            _ => None,
        }
    }

    /// 能还原成这个 id 的字符串（主副本就是二进制名），用于在协议数据里引用一个类。
    /// 用 [`ClassId::from_key`] 还原。
    pub fn key(self, db: &dyn Db) -> SmolStr {
        db.interner().lookup(self.0 & !Self::COPY_BIT)
    }

    /// [`ClassId::key`] 的逆操作。不检查类是否存在。
    pub fn from_key(db: &dyn Db, key: &str) -> ClassId {
        let id = db.interner().intern(key);
        if key.contains(COPY_SEPARATOR) {
            ClassId(id | Self::COPY_BIT)
        } else {
            ClassId(id)
        }
    }

    pub fn binary_name(self, db: &dyn Db) -> SmolStr {
        if !self.is_copy() {
            return db.interner().lookup(self.0);
        }
        let key = db.interner().lookup(self.0 & !Self::COPY_BIT);
        match key.split_once(COPY_SEPARATOR) {
            Some((name, _)) => SmolStr::new(name),
            None => key,
        }
    }

    /// 点分的规范名：`java.util.Map.Entry`
    pub fn qualified_name(self, db: &dyn Db) -> String {
        self.binary_name(db).replace(['/', '$'], ".")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FieldId {
    pub class: ClassId,
    /// `ClassData::fields` 的下标
    pub idx: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MethodId {
    pub class: ClassId,
    /// `ClassData::methods` 的下标
    pub idx: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GenericOwner {
    Class(ClassId),
    Method(MethodId),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TypeParamId {
    pub owner: GenericOwner,
    pub idx: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClassKind {
    Class,
    Interface,
    Enum,
    Record,
    Annotation,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Visibility {
    Public,
    Protected,
    Package,
    Private,
}

/// 修饰符位集合。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Modifiers(pub u32);

impl Modifiers {
    pub const PUBLIC: u32 = 1 << 0;
    pub const PROTECTED: u32 = 1 << 1;
    pub const PRIVATE: u32 = 1 << 2;
    pub const STATIC: u32 = 1 << 3;
    pub const FINAL: u32 = 1 << 4;
    pub const ABSTRACT: u32 = 1 << 5;
    pub const NATIVE: u32 = 1 << 6;
    pub const SYNCHRONIZED: u32 = 1 << 7;
    pub const TRANSIENT: u32 = 1 << 8;
    pub const VOLATILE: u32 = 1 << 9;
    pub const STRICTFP: u32 = 1 << 10;
    pub const DEFAULT: u32 = 1 << 11;
    pub const SEALED: u32 = 1 << 12;
    pub const NON_SEALED: u32 = 1 << 13;
    pub const VARARGS: u32 = 1 << 14;
    pub const DEPRECATED: u32 = 1 << 15;
    pub const ENUM_CONSTANT: u32 = 1 << 16;
    /// 编译器或本工具合成的成员（记录访问器、枚举的 `values()`、默认构造器 …）
    pub const SYNTHETIC: u32 = 1 << 17;
    /// 字段带 Lombok 的 `@Delegate`：类里要补上转发到它的方法
    pub const LOMBOK_DELEGATE: u32 = 1 << 18;
    /// 局部类（声明在方法体等里面）
    pub const LOCAL_CLASS: u32 = 1 << 19;
    /// 紧凑源文件的隐式类
    pub const IMPLICIT_CLASS: u32 = 1 << 20;
    /// 字段 / 方法返回值标了 `@Nullable` 一类注解
    pub const NULLABLE: u32 = 1 << 21;
    /// 字段 / 方法返回值标了 `@NotNull` 一类注解
    pub const NOT_NULL: u32 = 1 << 22;
    /// 匿名类（`new T() { ... }`、枚举常量的类体）；同时带 `LOCAL_CLASS`
    pub const ANONYMOUS: u32 = 1 << 23;

    /// `flags` 里的所有位是否都已置位。
    pub fn contains(self, flags: u32) -> bool {
        self.0 & flags == flags
    }

    pub fn insert(&mut self, flag: u32) {
        self.0 |= flag;
    }

    pub fn remove(&mut self, flag: u32) {
        self.0 &= !flag;
    }

    pub fn is_static(self) -> bool {
        self.contains(Self::STATIC)
    }

    pub fn is_abstract(self) -> bool {
        self.contains(Self::ABSTRACT)
    }

    pub fn is_final(self) -> bool {
        self.contains(Self::FINAL)
    }

    pub fn is_deprecated(self) -> bool {
        self.contains(Self::DEPRECATED)
    }

    pub fn null_annotation(self) -> NullAnnotation {
        if self.contains(Self::NULLABLE) {
            NullAnnotation::Nullable
        } else if self.contains(Self::NOT_NULL) {
            NullAnnotation::NotNull
        } else {
            NullAnnotation::Unknown
        }
    }

    pub fn insert_null_annotation(&mut self, annotation: NullAnnotation) {
        match annotation {
            NullAnnotation::Nullable => self.insert(Self::NULLABLE),
            NullAnnotation::NotNull => self.insert(Self::NOT_NULL),
            NullAnnotation::Unknown => {}
        }
    }

    pub fn visibility(self) -> Visibility {
        if self.contains(Self::PUBLIC) {
            Visibility::Public
        } else if self.contains(Self::PROTECTED) {
            Visibility::Protected
        } else if self.contains(Self::PRIVATE) {
            Visibility::Private
        } else {
            Visibility::Package
        }
    }

    /// 按 Java 源码的习惯顺序渲染成关键字序列，例如 `public static final`。
    pub fn render(self) -> String {
        const TABLE: &[(u32, &str)] = &[
            (Modifiers::PUBLIC, "public"),
            (Modifiers::PROTECTED, "protected"),
            (Modifiers::PRIVATE, "private"),
            (Modifiers::ABSTRACT, "abstract"),
            (Modifiers::STATIC, "static"),
            (Modifiers::FINAL, "final"),
            (Modifiers::SEALED, "sealed"),
            (Modifiers::NON_SEALED, "non-sealed"),
            (Modifiers::DEFAULT, "default"),
            (Modifiers::SYNCHRONIZED, "synchronized"),
            (Modifiers::NATIVE, "native"),
            (Modifiers::TRANSIENT, "transient"),
            (Modifiers::VOLATILE, "volatile"),
            (Modifiers::STRICTFP, "strictfp"),
        ];
        let mut out = String::new();
        for (flag, text) in TABLE {
            if self.contains(*flag) {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(text);
            }
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PrimitiveType {
    Boolean,
    Byte,
    Short,
    Char,
    Int,
    Long,
    Float,
    Double,
}

impl PrimitiveType {
    pub fn name(self) -> &'static str {
        match self {
            PrimitiveType::Boolean => "boolean",
            PrimitiveType::Byte => "byte",
            PrimitiveType::Short => "short",
            PrimitiveType::Char => "char",
            PrimitiveType::Int => "int",
            PrimitiveType::Long => "long",
            PrimitiveType::Float => "float",
            PrimitiveType::Double => "double",
        }
    }

    /// 对应包装类的二进制名。
    pub fn boxed_class(self) -> &'static str {
        match self {
            PrimitiveType::Boolean => "java/lang/Boolean",
            PrimitiveType::Byte => "java/lang/Byte",
            PrimitiveType::Short => "java/lang/Short",
            PrimitiveType::Char => "java/lang/Character",
            PrimitiveType::Int => "java/lang/Integer",
            PrimitiveType::Long => "java/lang/Long",
            PrimitiveType::Float => "java/lang/Float",
            PrimitiveType::Double => "java/lang/Double",
        }
    }

    pub fn from_boxed_class(binary_name: &str) -> Option<PrimitiveType> {
        Some(match binary_name {
            "java/lang/Boolean" => PrimitiveType::Boolean,
            "java/lang/Byte" => PrimitiveType::Byte,
            "java/lang/Short" => PrimitiveType::Short,
            "java/lang/Character" => PrimitiveType::Char,
            "java/lang/Integer" => PrimitiveType::Int,
            "java/lang/Long" => PrimitiveType::Long,
            "java/lang/Float" => PrimitiveType::Float,
            "java/lang/Double" => PrimitiveType::Double,
            _ => return None,
        })
    }

    pub fn is_numeric(self) -> bool {
        !matches!(self, PrimitiveType::Boolean)
    }

    pub fn is_integral(self) -> bool {
        matches!(
            self,
            PrimitiveType::Byte
                | PrimitiveType::Short
                | PrimitiveType::Char
                | PrimitiveType::Int
                | PrimitiveType::Long
        )
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PathSegment {
    pub name: Name,
    pub args: Vec<TypeRef>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum WildcardBound {
    Unbounded,
    Extends(Box<TypeRef>),
    Super(Box<TypeRef>),
}

/// 签名里出现的类型，尚未绑定到具体的类型系统对象。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TypeRef {
    Primitive(PrimitiveType),
    Void,
    /// 源码里写的类型路径（`List<String>`、`java.util.Map.Entry<K, V>`），需要按作用域解析
    Path(Vec<PathSegment>),
    /// 来自 class 文件：二进制名已知，不需要按作用域解析
    Class {
        name: SmolStr,
        args: Vec<TypeRef>,
    },
    /// 来自 class 文件的类型变量引用
    TypeVar(Name),
    Array(Box<TypeRef>),
    Wildcard(WildcardBound),
    Intersection(Vec<TypeRef>),
    Error,
}

impl TypeRef {
    pub fn class(binary_name: &str) -> TypeRef {
        TypeRef::Class {
            name: SmolStr::new(binary_name),
            args: Vec::new(),
        }
    }

    pub fn array_of(self, dims: usize) -> TypeRef {
        let mut ty = self;
        for _ in 0..dims {
            ty = TypeRef::Array(Box::new(ty));
        }
        ty
    }

    /// 是否是源码里写的 `var`
    pub fn is_var(&self) -> bool {
        matches!(self, TypeRef::Path(segments)
            if segments.len() == 1 && segments[0].name == "var" && segments[0].args.is_empty())
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TypeParamData {
    pub name: Name,
    pub bounds: Vec<TypeRef>,
}

/// 声明在所属文件的 `SourceMap` 里的下标；二进制成员和合成成员没有。
pub type DeclIdx = u32;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FieldData {
    pub name: Name,
    pub ty: TypeRef,
    pub flags: Modifiers,
    pub decl: Option<DeclIdx>,
    /// class 文件里的常量值（`ConstantValue` 属性）；源码字段的常量值按需求值
    pub constant: Option<FieldConstant>,
}

/// class 文件里字段的常量值。浮点数按位存放，以便比较和哈希。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum FieldConstant {
    Int(i32),
    Long(i64),
    Float(u32),
    Double(u64),
    Str(SmolStr),
}

impl FieldData {
    pub fn is_enum_constant(&self) -> bool {
        self.flags.contains(Modifiers::ENUM_CONSTANT)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ParamData {
    /// 二进制方法可能拿不到参数名
    pub name: Option<Name>,
    pub ty: TypeRef,
    pub nullness: NullAnnotation,
}

/// 空值注解（JetBrains、JSR-305、JSpecify、Checker Framework、Android、Jakarta、Lombok …）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum NullAnnotation {
    #[default]
    Unknown,
    Nullable,
    NotNull,
}

impl NullAnnotation {
    /// 按注解名（简单名、限定名或类型描述符）判断是不是空值注解。
    pub fn from_name(name: &str) -> NullAnnotation {
        let name = name.strip_suffix(';').unwrap_or(name);
        let simple = name.rsplit(['/', '.', '$']).next().unwrap_or(name);
        match simple {
            "Nullable" | "CheckForNull" | "NullableDecl" | "NullableType" | "RecentlyNullable" => {
                NullAnnotation::Nullable
            }
            "NotNull" | "NonNull" | "Nonnull" | "NonNullDecl" | "NonNullType"
            | "RecentlyNonNull" => NullAnnotation::NotNull,
            _ => NullAnnotation::Unknown,
        }
    }

    /// 一组注解里的空值注解。
    pub fn of<'a>(names: impl IntoIterator<Item = &'a str>) -> NullAnnotation {
        names
            .into_iter()
            .map(NullAnnotation::from_name)
            .find(|a| *a != NullAnnotation::Unknown)
            .unwrap_or_default()
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct MethodData {
    /// 构造器的名字是所在类的简单名
    pub name: Name,
    pub is_constructor: bool,
    pub type_params: Vec<TypeParamData>,
    pub params: Vec<ParamData>,
    pub ret: TypeRef,
    pub throws: Vec<TypeRef>,
    pub flags: Modifiers,
    pub has_body: bool,
    pub decl: Option<DeclIdx>,
}

impl MethodData {
    pub fn is_varargs(&self) -> bool {
        self.flags.contains(Modifiers::VARARGS)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ClassData {
    pub name: Name,
    pub binary_name: SmolStr,
    pub kind: ClassKind,
    pub flags: Modifiers,
    pub type_params: Vec<TypeParamData>,
    pub superclass: Option<TypeRef>,
    pub interfaces: Vec<TypeRef>,
    pub permits: Vec<TypeRef>,
    pub fields: Vec<FieldData>,
    pub methods: Vec<MethodData>,
    /// 直接成员类型的简单名
    pub nested: Vec<Name>,
    /// 外层类的二进制名
    pub outer: Option<SmolStr>,
    /// 注解名（按源码里写的样子，或 class 文件里的二进制名）
    pub annotations: Vec<SmolStr>,
    /// 注解类型上 `@Target` 写明的适用位置；没写 `@Target`（或不是注解类型）时为 `None`
    pub targets: Option<AnnotationTargets>,
    pub decl: Option<DeclIdx>,
    pub is_source: bool,
}

impl ClassData {
    pub fn is_interface(&self) -> bool {
        matches!(self.kind, ClassKind::Interface | ClassKind::Annotation)
    }

    /// 包名（斜杠形式），默认包为空串。
    pub fn package(&self) -> &str {
        classfile::package_of(&self.binary_name)
    }

    pub fn constructors(&self) -> impl Iterator<Item = (u32, &MethodData)> {
        self.methods
            .iter()
            .enumerate()
            .filter(|(_, m)| m.is_constructor)
            .map(|(i, m)| (i as u32, m))
    }

    pub fn methods_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = (u32, &'a MethodData)> + 'a {
        self.methods
            .iter()
            .enumerate()
            .filter(move |(_, m)| !m.is_constructor && m.name == name)
            .map(|(i, m)| (i as u32, m))
    }

    pub fn field_named(&self, name: &str) -> Option<(u32, &FieldData)> {
        self.fields
            .iter()
            .enumerate()
            .find(|(_, f)| f.name == name)
            .map(|(i, f)| (i as u32, f))
    }

    pub fn has_annotation(&self, simple_name: &str) -> bool {
        self.annotations
            .iter()
            .any(|a| a.rsplit(['.', '/', '$']).next() == Some(simple_name))
    }

    /// 注解类型能用在哪些位置：`@Target` 写明的，没写时是 JLS 9.6.4.1 的默认（全部声明位置）。
    pub fn annotation_targets(&self) -> AnnotationTargets {
        self.targets.unwrap_or(AnnotationTargets::DEFAULT)
    }
}

/// 注解的适用位置（`java.lang.annotation.ElementType` 的位集合）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct AnnotationTargets(pub u16);

impl AnnotationTargets {
    pub const TYPE: u16 = 1 << 0;
    pub const FIELD: u16 = 1 << 1;
    pub const METHOD: u16 = 1 << 2;
    pub const PARAMETER: u16 = 1 << 3;
    pub const CONSTRUCTOR: u16 = 1 << 4;
    pub const LOCAL_VARIABLE: u16 = 1 << 5;
    pub const ANNOTATION_TYPE: u16 = 1 << 6;
    pub const PACKAGE: u16 = 1 << 7;
    pub const TYPE_PARAMETER: u16 = 1 << 8;
    pub const TYPE_USE: u16 = 1 << 9;
    pub const MODULE: u16 = 1 << 10;
    pub const RECORD_COMPONENT: u16 = 1 << 11;

    /// 没写 `@Target` 时：所有声明位置，不含类型位置（JLS 9.6.4.1）
    pub const DEFAULT: AnnotationTargets = AnnotationTargets(
        Self::TYPE
            | Self::FIELD
            | Self::METHOD
            | Self::PARAMETER
            | Self::CONSTRUCTOR
            | Self::LOCAL_VARIABLE
            | Self::ANNOTATION_TYPE
            | Self::PACKAGE
            | Self::TYPE_PARAMETER
            | Self::MODULE
            | Self::RECORD_COMPONENT,
    );
    /// 全部位置：`@Target` 的取值认不出来时按它处理，不做任何限制
    pub const ALL: AnnotationTargets = AnnotationTargets(Self::DEFAULT.0 | Self::TYPE_USE);

    /// `ElementType` 的常量名对应的位。
    pub fn from_name(name: &str) -> Option<u16> {
        Some(match name {
            "TYPE" => Self::TYPE,
            "FIELD" => Self::FIELD,
            "METHOD" => Self::METHOD,
            "PARAMETER" => Self::PARAMETER,
            "CONSTRUCTOR" => Self::CONSTRUCTOR,
            "LOCAL_VARIABLE" => Self::LOCAL_VARIABLE,
            "ANNOTATION_TYPE" => Self::ANNOTATION_TYPE,
            "PACKAGE" => Self::PACKAGE,
            "TYPE_PARAMETER" => Self::TYPE_PARAMETER,
            "TYPE_USE" => Self::TYPE_USE,
            "MODULE" => Self::MODULE,
            "RECORD_COMPONENT" => Self::RECORD_COMPONENT,
            _ => return None,
        })
    }

    /// 由 `ElementType` 常量名构造；有认不出来的名字时退回 [`Self::ALL`]。
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> AnnotationTargets {
        let mut bits = 0;
        for name in names {
            match Self::from_name(name) {
                Some(bit) => bits |= bit,
                None => return Self::ALL,
            }
        }
        AnnotationTargets(bits)
    }

    /// 写明的位置里有 `site` 的任何一位。
    pub fn intersects(self, site: u16) -> bool {
        self.0 & site != 0
    }

    /// 各位对应的 `ElementType` 常量名（按声明顺序）。
    pub fn names(self) -> impl Iterator<Item = &'static str> {
        const NAMES: [&str; 12] = [
            "TYPE",
            "FIELD",
            "METHOD",
            "PARAMETER",
            "CONSTRUCTOR",
            "LOCAL_VARIABLE",
            "ANNOTATION_TYPE",
            "PACKAGE",
            "TYPE_PARAMETER",
            "TYPE_USE",
            "MODULE",
            "RECORD_COMPONENT",
        ];
        NAMES
            .into_iter()
            .enumerate()
            .filter(move |(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, name)| name)
    }
}

/// 一条 import 声明。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Import {
    pub segments: Vec<Name>,
    pub is_static: bool,
    pub is_on_demand: bool,
}

/// 一个源码文件的签名级摘要。**不包含**方法体和任何位置信息，
/// 所以在方法体里编辑不会改变它，依赖它的全局查询也就不会失效。
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct ItemTree {
    /// 包名（斜杠形式），默认包为空串
    pub package: SmolStr,
    pub imports: Vec<Import>,
    /// 文件里所有的类（含成员类、方法体里的局部类和匿名类），按先序排列
    pub classes: Vec<std::sync::Arc<ClassData>>,
    /// 局部类 / 记录 / 接口 / 枚举和匿名类：只在声明它的那个成员的体里可见
    pub local_classes: Vec<LocalClass>,
    /// `import module M;` 的模块名（点分形式）。不放进 `imports`：它们不是按类名导入的
    pub module_imports: Vec<SmolStr>,
    /// `module-info.java` 里的模块声明
    pub module: Option<ModuleDeclData>,
}

/// 源码里的模块声明。包名是斜杠形式，模块名是点分形式。
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct ModuleDeclData {
    pub name: SmolStr,
    /// `(模块, 是否 transitive)`
    pub requires: Vec<(SmolStr, bool)>,
    /// `(包, 限定导出的目标模块)`
    pub exports: Vec<(SmolStr, Vec<SmolStr>)>,
    pub opens: Vec<(SmolStr, Vec<SmolStr>)>,
}

/// 声明在方法体（或字段初始化式、初始化块）里的类型，以及匿名类。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct LocalClass {
    /// 匿名类为空串
    pub name: Name,
    pub binary_name: SmolStr,
    /// 直接外层的具名类（可能本身也是局部类）
    pub outer: SmolStr,
    /// 声明在外层类的哪个方法 / 构造器里（`ClassData::methods` 的下标）；
    /// 在字段初始化式、初始化块里时为 `None`
    pub owner: Option<u32>,
    /// 类型声明节点；匿名类是它的 `CLASS_BODY`
    pub decl: DeclIdx,
}

impl ItemTree {
    pub fn class_idx(&self, binary_name: &str) -> Option<u32> {
        self.classes
            .iter()
            .position(|c| c.binary_name == binary_name)
            .map(|i| i as u32)
    }

    pub fn top_level_classes(&self) -> impl Iterator<Item = &ClassData> {
        self.classes
            .iter()
            .filter(|c| c.outer.is_none())
            .map(|c| &**c)
    }
}
