//! Lombok 注解生成的成员。
//!
//! Lombok 在编译期往类里加方法，源码里看不到。这里按注解把对应的签名合成进 [`ClassData`]，
//! 上层（补全、跳转、类型推断）就能像对待手写成员一样对待它们。合成成员带 `SYNTHETIC`
//! 标志，声明位置指向触发它的字段（或类），所以"跳转到定义"会落在字段上。
//!
//! 只按注解的简单名识别（`@Getter` 和 `@lombok.Getter` 等价）。注解参数只看
//! `@Accessors(chain = true / fluent = true)`；`AccessLevel`、`staticName` 等定制一律按默认行为处理。
//!
//! 需要别的类的签名才能合成的成员（`@Delegate` 转发的方法、`@SuperBuilder` 子类构建器继承的
//! 设置方法）不在这里，见 [`crate::lombok_expand`]。

use smol_str::SmolStr;
use syntax::ast::{self, AstNode};

use crate::data::{
    ClassData, ClassKind, FieldData, MethodData, Modifiers, Name, ParamData, PrimitiveType, TypeRef,
};

fn simple(annotation: &str) -> &str {
    annotation
        .rsplit(['.', '/', '$'])
        .next()
        .unwrap_or(annotation)
}

fn has(annotations: &[SmolStr], name: &str) -> bool {
    annotations.iter().any(|a| simple(a) == name)
}

/// `@Accessors(...)` 的参数，以伪注解名的形式跟在 `Accessors` 后面记进注解列表。
const ACCESSORS_CHAIN: &str = "@Accessors(chain)";
const ACCESSORS_FLUENT: &str = "@Accessors(fluent)";

pub(crate) fn accessors_options(annotation: &ast::Annotation) -> Vec<SmolStr> {
    let mut res = Vec::new();
    let Some(args) = annotation.arg_list() else {
        return res;
    };
    for arg in args.args() {
        let enabled = arg
            .value_expr()
            .is_some_and(|value| value.syntax().text() == "true");
        match arg.name_ref().map(|n| n.text()) {
            Some(name) if enabled && name == "chain" => res.push(SmolStr::new(ACCESSORS_CHAIN)),
            Some(name) if enabled && name == "fluent" => res.push(SmolStr::new(ACCESSORS_FLUENT)),
            _ => {}
        }
    }
    res
}

/// 访问器的风格：`fluent` 时访问器与字段同名，`chain` 时设置方法返回 `this`。
#[derive(Clone, Copy, Default)]
struct AccessorStyle {
    chain: bool,
    fluent: bool,
}

impl AccessorStyle {
    fn of(annotations: &[SmolStr]) -> Option<AccessorStyle> {
        if !has(annotations, "Accessors") {
            return None;
        }
        let fluent = annotations.iter().any(|a| a == ACCESSORS_FLUENT);
        let chain = fluent || annotations.iter().any(|a| a == ACCESSORS_CHAIN);
        Some(AccessorStyle { chain, fluent })
    }
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn is_boolean(ty: &TypeRef) -> bool {
    matches!(ty, TypeRef::Primitive(PrimitiveType::Boolean))
}

/// `isActive` 这种已经带 `is` 前缀的 boolean 字段，去掉前缀后的属性名。
fn boolean_property(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("is")?;
    rest.starts_with(|c: char| c.is_uppercase()).then_some(rest)
}

pub(crate) fn getter_name(field: &FieldData) -> Name {
    if is_boolean(&field.ty) {
        match boolean_property(&field.name) {
            Some(_) => field.name.clone(),
            None => Name::new(format!("is{}", capitalize(&field.name))),
        }
    } else {
        Name::new(format!("get{}", capitalize(&field.name)))
    }
}

pub(crate) fn setter_name(field: &FieldData) -> Name {
    let property = match boolean_property(&field.name) {
        Some(rest) if is_boolean(&field.ty) => rest.to_owned(),
        _ => capitalize(&field.name),
    };
    Name::new(format!("set{property}"))
}

fn with_name(field: &FieldData) -> Name {
    Name::new(format!("with{}", capitalize(&field.name)))
}

/// 从字段合成出来的访问器的种类（名字怎样由字段名推出来）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AccessorKind {
    /// `getName()` / `isActive()`
    Getter,
    /// `setName(..)`
    Setter,
    /// `withName(..)`
    Wither,
    /// 与字段同名：构建器上的 `name(..)`、记录的访问器 `name()`
    SameName,
}

impl AccessorKind {
    /// 合成方法 `method` 是字段 `field` 的哪种访问器。
    pub fn of(field: &FieldData, method: &MethodData) -> Option<AccessorKind> {
        let arity = method.params.len();
        if arity == 0 && method.name == getter_name(field) {
            Some(AccessorKind::Getter)
        } else if arity == 1 && method.name == setter_name(field) {
            Some(AccessorKind::Setter)
        } else if arity == 1 && method.name == with_name(field) {
            Some(AccessorKind::Wither)
        } else if arity <= 1 && method.name == field.name {
            Some(AccessorKind::SameName)
        } else {
            None
        }
    }

    /// 字段（按它的名字和类型）对应的这种访问器的名字。
    pub fn name_for(self, field: &FieldData) -> Name {
        match self {
            AccessorKind::Getter => getter_name(field),
            AccessorKind::Setter => setter_name(field),
            AccessorKind::Wither => with_name(field),
            AccessorKind::SameName => field.name.clone(),
        }
    }

    /// 反推：要让这种访问器叫 `accessor`，字段应当叫什么。推不出来（前缀不对、
    /// 剩下的部分为空）时返回 `None`。`field` 提供类型和原来的命名风格。
    pub fn field_name_for(self, field: &FieldData, accessor: &str) -> Option<Name> {
        let rest = match self {
            AccessorKind::SameName => return Some(Name::new(accessor)),
            AccessorKind::Getter => accessor
                .strip_prefix("get")
                .filter(|_| !is_boolean(&field.ty))
                .or_else(|| {
                    accessor
                        .strip_prefix("is")
                        .filter(|_| is_boolean(&field.ty))
                }),
            AccessorKind::Setter => accessor.strip_prefix("set"),
            AccessorKind::Wither => accessor.strip_prefix("with"),
        }?;
        if rest.is_empty() {
            return None;
        }
        // 原字段名以大写开头（`Effect`）就保留大小写，否则首字母小写
        let plain = if field.name.starts_with(|c: char| c.is_uppercase()) {
            rest.to_owned()
        } else {
            let mut chars = rest.chars();
            let first = chars.next()?;
            first.to_lowercase().chain(chars).collect()
        };
        // boolean 字段原来就带 `is` 前缀时，优先保留这个风格
        let prefixed = format!("is{}", capitalize(rest));
        let candidates = if boolean_property(&field.name).is_some() {
            [prefixed, plain]
        } else {
            [plain, prefixed]
        };
        candidates.into_iter().map(Name::new).find(|candidate| {
            let renamed = FieldData {
                name: candidate.clone(),
                ..field.clone()
            };
            self.name_for(&renamed) == accessor
        })
    }
}

const SYNTHETIC_PUBLIC: Modifiers = Modifiers(Modifiers::PUBLIC | Modifiers::SYNTHETIC);

fn declared(data: &ClassData, name: &str, arity: usize) -> bool {
    data.methods
        .iter()
        .any(|m| !m.is_constructor && m.name == name && m.params.len() == arity)
}

fn method(
    name: Name,
    params: Vec<ParamData>,
    ret: TypeRef,
    flags: Modifiers,
    decl: Option<u32>,
) -> MethodData {
    MethodData {
        name,
        is_constructor: false,
        type_params: Vec::new(),
        params,
        ret,
        throws: Vec::new(),
        flags,
        has_body: true,
        decl,
    }
}

fn add_constructor(data: &mut ClassData, params: Vec<ParamData>) {
    let exists = data
        .methods
        .iter()
        .any(|m| m.is_constructor && m.params.len() == params.len());
    if exists {
        return;
    }
    data.methods.push(MethodData {
        name: data.name.clone(),
        is_constructor: true,
        type_params: Vec::new(),
        params,
        ret: TypeRef::Void,
        throws: Vec::new(),
        flags: SYNTHETIC_PUBLIC,
        has_body: true,
        decl: data.decl,
    });
}

/// 日志注解 → 日志器类型。
fn logger_type(annotations: &[SmolStr]) -> Option<&'static str> {
    const LOGGERS: &[(&str, &str)] = &[
        ("Slf4j", "org/slf4j/Logger"),
        ("XSlf4j", "org/slf4j/ext/XLogger"),
        ("Log4j2", "org/apache/logging/log4j/Logger"),
        ("Log4j", "org/apache/log4j/Logger"),
        ("Log", "java/util/logging/Logger"),
        ("CommonsLog", "org/apache/commons/logging/Log"),
        ("JBossLog", "org/jboss/logging/Logger"),
        ("Flogger", "com/google/common/flogger/FluentLogger"),
    ];
    LOGGERS
        .iter()
        .find(|(name, _)| has(annotations, name))
        .map(|(_, ty)| *ty)
}

/// 按类和字段上的 Lombok 注解合成成员。`@Builder` 还会产生一个嵌套的构建器类，作为返回值交给调用方登记。
pub(crate) fn apply(
    data: &mut ClassData,
    field_annotations: &[(usize, Vec<SmolStr>)],
    self_ty: &TypeRef,
) -> Option<ClassData> {
    let class_annotations = data.annotations.clone();
    let any_field_annotation = field_annotations.iter().any(|(_, list)| {
        ["Getter", "Setter", "With", "Delegate"]
            .iter()
            .any(|name| has(list, name))
    });
    const CLASS_LEVEL: &[&str] = &[
        "Getter",
        "Setter",
        "Data",
        "Value",
        "With",
        "Builder",
        "SuperBuilder",
        "NoArgsConstructor",
        "AllArgsConstructor",
        "RequiredArgsConstructor",
        "UtilityClass",
    ];
    let any_class_annotation = CLASS_LEVEL.iter().any(|name| has(&class_annotations, name));
    let logger = logger_type(&class_annotations);
    if !any_class_annotation && !any_field_annotation && logger.is_none() {
        return None;
    }
    if !matches!(
        data.kind,
        ClassKind::Class | ClassKind::Enum | ClassKind::Record
    ) {
        return None;
    }

    let is_data = has(&class_annotations, "Data");
    let is_value = has(&class_annotations, "Value");
    let class_getter = is_data || is_value || has(&class_annotations, "Getter");
    let class_setter = is_data || has(&class_annotations, "Setter");
    let class_with = has(&class_annotations, "With");
    let class_style = AccessorStyle::of(&class_annotations).unwrap_or_default();
    let annotations_of = |idx: usize| -> &[SmolStr] {
        field_annotations
            .iter()
            .find(|(field, _)| *field == idx)
            .map(|(_, list)| list.as_slice())
            .unwrap_or(&[])
    };

    // `@UtilityClass`：类变成 final，所有成员变成静态的，构造器私有
    if has(&class_annotations, "UtilityClass") && data.kind == ClassKind::Class {
        data.flags.insert(Modifiers::FINAL);
        for field in &mut data.fields {
            field.flags.insert(Modifiers::STATIC);
        }
        for method in data.methods.iter_mut().filter(|m| !m.is_constructor) {
            method.flags.insert(Modifiers::STATIC);
        }
        if !data.methods.iter().any(|m| m.is_constructor) {
            data.methods.push(MethodData {
                name: data.name.clone(),
                is_constructor: true,
                type_params: Vec::new(),
                params: Vec::new(),
                ret: TypeRef::Void,
                throws: Vec::new(),
                flags: Modifiers(Modifiers::PRIVATE | Modifiers::SYNTHETIC),
                has_body: true,
                decl: data.decl,
            });
        }
    }

    // `@Delegate` 的字段：转发的方法要看字段类型的成员，留给 `lombok_expand` 补
    for (idx, list) in field_annotations {
        if has(list, "Delegate") {
            if let Some(field) = data.fields.get_mut(*idx) {
                field.flags.insert(Modifiers::LOMBOK_DELEGATE);
            }
        }
    }

    // `@Value` 让所有实例字段变成 private final
    if is_value {
        for field in data.fields.iter_mut().filter(|f| !f.flags.is_static()) {
            field.flags.remove(Modifiers::PUBLIC | Modifiers::PROTECTED);
            field.flags.insert(Modifiers::PRIVATE | Modifiers::FINAL);
        }
    }

    // 访问器
    let fields: Vec<(usize, FieldData)> = data.fields.iter().cloned().enumerate().collect();
    for (idx, field) in &fields {
        if field.is_enum_constant() {
            continue;
        }
        let annotations = annotations_of(*idx);
        let is_static = field.flags.is_static();
        let mut flags = SYNTHETIC_PUBLIC;
        if is_static {
            flags.insert(Modifiers::STATIC);
        }

        let style = AccessorStyle::of(annotations).unwrap_or(class_style);

        // 类级注解不作用于静态字段；字段级注解可以
        if has(annotations, "Getter") || (class_getter && !is_static) {
            let name = if style.fluent {
                field.name.clone()
            } else {
                getter_name(field)
            };
            if !declared(data, &name, 0) {
                data.methods.push(method(
                    name,
                    Vec::new(),
                    field.ty.clone(),
                    flags,
                    field.decl,
                ));
            }
        }
        let settable = !field.flags.is_final();
        if settable && (has(annotations, "Setter") || (class_setter && !is_static)) {
            let name = if style.fluent {
                field.name.clone()
            } else {
                setter_name(field)
            };
            if !declared(data, &name, 1) {
                let param = ParamData {
                    name: Some(field.name.clone()),
                    ty: field.ty.clone(),
                    nullness: field.flags.null_annotation(),
                };
                // 链式设置方法返回 `this`；静态字段的设置方法没有 `this` 可返回
                let ret = if style.chain && !is_static {
                    self_ty.clone()
                } else {
                    TypeRef::Void
                };
                data.methods
                    .push(method(name, vec![param], ret, flags, field.decl));
            }
        }
        if !is_static && (has(annotations, "With") || class_with) {
            let name = with_name(field);
            if !declared(data, &name, 1) {
                let param = ParamData {
                    name: Some(field.name.clone()),
                    ty: field.ty.clone(),
                    nullness: field.flags.null_annotation(),
                };
                data.methods.push(method(
                    name,
                    vec![param],
                    self_ty.clone(),
                    SYNTHETIC_PUBLIC,
                    field.decl,
                ));
            }
        }
    }

    // 日志器字段
    if let Some(logger) = logger {
        if data.field_named("log").is_none() {
            data.fields.push(FieldData {
                name: Name::new("log"),
                ty: TypeRef::class(logger),
                flags: Modifiers(
                    Modifiers::PRIVATE
                        | Modifiers::STATIC
                        | Modifiers::FINAL
                        | Modifiers::SYNTHETIC,
                ),
                decl: data.decl,
            });
        }
    }

    // 构造器
    let instance_fields: Vec<&(usize, FieldData)> = fields
        .iter()
        .filter(|(_, f)| !f.flags.is_static() && !f.is_enum_constant())
        .collect();
    let to_params = |list: &[&(usize, FieldData)]| -> Vec<ParamData> {
        list.iter()
            .map(|(_, f)| ParamData {
                name: Some(f.name.clone()),
                ty: f.ty.clone(),
                nullness: f.flags.null_annotation(),
            })
            .collect()
    };
    let required: Vec<&(usize, FieldData)> = instance_fields
        .iter()
        .copied()
        .filter(|(idx, f)| f.flags.is_final() || is_value || has(annotations_of(*idx), "NonNull"))
        .collect();
    let has_explicit_ctor = data.methods.iter().any(|m| m.is_constructor);
    let wants_builder =
        has(&class_annotations, "Builder") || has(&class_annotations, "SuperBuilder");

    if data.kind == ClassKind::Class {
        if has(&class_annotations, "NoArgsConstructor") {
            add_constructor(data, Vec::new());
        }
        if has(&class_annotations, "RequiredArgsConstructor") || (is_data && !has_explicit_ctor) {
            add_constructor(data, to_params(&required));
        }
        // `@Builder` 在没有显式构造器时隐含一个全参构造器
        if has(&class_annotations, "AllArgsConstructor")
            || (is_value && !has_explicit_ctor)
            || (wants_builder && !has_explicit_ctor)
        {
            add_constructor(data, to_params(&instance_fields));
        }
    }

    // 构建器
    if !wants_builder || data.kind == ClassKind::Enum {
        return None;
    }
    let builder_name = Name::new(format!("{}Builder", data.name));
    if data.nested.contains(&builder_name) {
        // 用户自己写了构建器类（常见的定制方式）：只补 `builder()`
        if !declared(data, "builder", 0) {
            let ty = TypeRef::class(&format!("{}${}", data.binary_name, builder_name));
            let flags = Modifiers(Modifiers::PUBLIC | Modifiers::STATIC | Modifiers::SYNTHETIC);
            data.methods.push(method(
                Name::new("builder"),
                Vec::new(),
                ty,
                flags,
                data.decl,
            ));
        }
        return None;
    }
    let builder_binary: SmolStr = format!("{}${}", data.binary_name, builder_name).into();
    let builder_ty = TypeRef::class(&builder_binary);

    let mut builder = ClassData {
        name: builder_name,
        binary_name: builder_binary,
        kind: ClassKind::Class,
        flags: Modifiers(Modifiers::PUBLIC | Modifiers::STATIC | Modifiers::SYNTHETIC),
        type_params: Vec::new(),
        superclass: Some(TypeRef::class("java/lang/Object")),
        interfaces: Vec::new(),
        permits: Vec::new(),
        fields: Vec::new(),
        methods: Vec::new(),
        nested: Vec::new(),
        outer: Some(data.binary_name.clone()),
        annotations: Vec::new(),
        decl: data.decl,
        is_source: true,
    };
    for (_, field) in &instance_fields {
        let param = ParamData {
            name: Some(field.name.clone()),
            ty: field.ty.clone(),
            nullness: field.flags.null_annotation(),
        };
        builder.methods.push(method(
            field.name.clone(),
            vec![param],
            builder_ty.clone(),
            SYNTHETIC_PUBLIC,
            field.decl,
        ));
    }
    builder.methods.push(method(
        Name::new("build"),
        Vec::new(),
        self_ty.clone(),
        SYNTHETIC_PUBLIC,
        data.decl,
    ));

    let static_flags = Modifiers(Modifiers::PUBLIC | Modifiers::STATIC | Modifiers::SYNTHETIC);
    if !declared(data, "builder", 0) {
        data.methods.push(method(
            Name::new("builder"),
            Vec::new(),
            builder_ty.clone(),
            static_flags,
            data.decl,
        ));
    }
    if !declared(data, "toBuilder", 0) && has(&class_annotations, "SuperBuilder") {
        data.methods.push(method(
            Name::new("toBuilder"),
            Vec::new(),
            builder_ty,
            SYNTHETIC_PUBLIC,
            data.decl,
        ));
    }
    Some(builder)
}

#[cfg(test)]
mod tests {
    use syntax::ast::CompilationUnit;

    use crate::lower::lower_file;

    fn methods_of(text: &str, class: &str) -> Vec<String> {
        let (tree, _) = lower_file(&CompilationUnit::parse(text).tree());
        let data = tree
            .classes
            .iter()
            .find(|c| c.name == class)
            .unwrap_or_else(|| panic!("没有类 {class}"));
        let mut names: Vec<String> = data
            .methods
            .iter()
            .map(|m| {
                if m.is_constructor {
                    format!("<init>/{}", m.params.len())
                } else {
                    format!("{}/{}", m.name, m.params.len())
                }
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn data_generates_accessors_and_required_constructor() {
        let methods = methods_of(
            "import lombok.Data;\n@Data class User { private final long id; private String name; private boolean active; private boolean isAdmin; static int count; }",
            "User",
        );
        assert_eq!(
            methods,
            [
                "<init>/1",
                "getId/0",
                "getName/0",
                "isActive/0",
                "isAdmin/0",
                "setActive/1",
                "setAdmin/1",
                "setName/1",
            ]
        );
    }

    #[test]
    fn field_level_annotations_and_existing_methods() {
        let methods = methods_of(
            "class A { @lombok.Getter @lombok.Setter int x; @lombok.Getter int y; int z; int getY() { return 1; } }",
            "A",
        );
        // 已有的 getY 不重复生成；没有注解的 z 不生成；还有隐式无参构造器
        assert_eq!(methods, ["<init>/0", "getX/0", "getY/0", "setX/1"]);
    }

    #[test]
    fn builder_creates_nested_class() {
        let text = "@lombok.Builder class Point { int x; int y; }";
        assert_eq!(methods_of(text, "Point"), ["<init>/2", "builder/0"]);
        assert_eq!(methods_of(text, "PointBuilder"), ["build/0", "x/1", "y/1"]);
        let (tree, _) = lower_file(&CompilationUnit::parse(text).tree());
        let builder = tree
            .classes
            .iter()
            .find(|c| c.name == "PointBuilder")
            .unwrap();
        assert_eq!(builder.binary_name, "Point$PointBuilder");
        assert_eq!(builder.outer.as_deref(), Some("Point"));
    }

    #[test]
    fn value_and_logger() {
        let text = "@lombok.Value @lombok.extern.slf4j.Slf4j class V { String a; int b; }";
        assert_eq!(methods_of(text, "V"), ["<init>/2", "getA/0", "getB/0"]);
        let (tree, _) = lower_file(&CompilationUnit::parse(text).tree());
        let data = tree.classes.iter().find(|c| c.name == "V").unwrap();
        assert!(data
            .fields
            .iter()
            .filter(|f| f.name != "log")
            .all(|f| f.flags.is_final()));
        assert!(data.field_named("log").is_some());
    }

    #[test]
    fn accessor_names_map_back_to_field_names() {
        use super::AccessorKind;
        use crate::data::{FieldData, Modifiers, PrimitiveType, TypeRef};

        let field = |name: &str, ty: TypeRef| FieldData {
            name: name.into(),
            ty,
            flags: Modifiers::default(),
            decl: None,
        };
        let string = field("name", TypeRef::class("java/lang/String"));
        let flag = field("active", TypeRef::Primitive(PrimitiveType::Boolean));
        let prefixed = field("isAdmin", TypeRef::Primitive(PrimitiveType::Boolean));
        let upper = field("Effect", TypeRef::class("java/lang/String"));

        let back = |kind: AccessorKind, f: &FieldData, accessor: &str| {
            kind.field_name_for(f, accessor).map(|n| n.to_string())
        };
        assert_eq!(
            back(AccessorKind::Getter, &string, "getTitle").as_deref(),
            Some("title")
        );
        assert_eq!(
            back(AccessorKind::Setter, &string, "setTitle").as_deref(),
            Some("title")
        );
        assert_eq!(
            back(AccessorKind::SameName, &string, "title").as_deref(),
            Some("title")
        );
        assert_eq!(back(AccessorKind::Getter, &string, "title"), None);
        assert_eq!(back(AccessorKind::Getter, &string, "isTitle"), None);
        assert_eq!(
            back(AccessorKind::Getter, &flag, "isOn").as_deref(),
            Some("on")
        );
        assert_eq!(
            back(AccessorKind::Setter, &prefixed, "setRoot").as_deref(),
            Some("isRoot")
        );
        assert_eq!(
            back(AccessorKind::Getter, &upper, "getAction").as_deref(),
            Some("Action")
        );
    }

    #[test]
    fn plain_classes_are_untouched() {
        assert_eq!(methods_of("class P { int x; }", "P"), ["<init>/0"]);
    }
}
