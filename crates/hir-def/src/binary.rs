//! class 文件 → [`ClassData`]。

use classfile::{
    access,
    signature::{self, BaseType, JType, TypeArg, TypeParamSig},
    ClassFile,
};
use smol_str::SmolStr;

use crate::data::{
    ClassData, ClassKind, FieldData, MethodData, Modifiers, Name, NullAnnotation, ParamData,
    PrimitiveType, TypeParamData, TypeRef, WildcardBound,
};

pub fn lower_class_file(class: &ClassFile) -> ClassData {
    // 成员类的真实访问标志（static / private / protected）记录在 InnerClasses 属性里
    let self_inner = class.inner_classes.iter().find(|ic| ic.inner == class.name);
    let class_access = self_inner.map_or(class.access, |ic| ic.access);

    let kind = if class_access & access::ANNOTATION != 0 {
        ClassKind::Annotation
    } else if class_access & access::INTERFACE != 0 {
        ClassKind::Interface
    } else if class_access & access::ENUM != 0 {
        ClassKind::Enum
    } else if class.super_name.as_deref() == Some("java/lang/Record") {
        ClassKind::Record
    } else {
        ClassKind::Class
    };

    let mut flags = common_flags(class_access);
    if class.deprecated || has_deprecated(&class.annotations) {
        flags.insert(Modifiers::DEPRECATED);
    }
    if !class.permitted_subclasses.is_empty() {
        flags.insert(Modifiers::SEALED);
    }

    let outer = self_inner.and_then(|ic| ic.outer.clone());
    let is_inner_instance_class = outer.is_some() && class_access & access::STATIC == 0;

    let (type_params, superclass, interfaces) = match class
        .signature
        .as_deref()
        .and_then(signature::parse_class_sig)
    {
        Some(sig) => (
            lower_type_params(&sig.type_params),
            Some(lower_jtype(&sig.super_class)),
            sig.interfaces.iter().map(lower_jtype).collect(),
        ),
        None => (
            Vec::new(),
            class.super_name.as_deref().map(TypeRef::class),
            class
                .interfaces
                .iter()
                .map(|name| TypeRef::class(name))
                .collect(),
        ),
    };
    // 接口在 class 文件里的父类是 Object，源码层面接口没有父类
    let superclass = if matches!(kind, ClassKind::Interface | ClassKind::Annotation) {
        None
    } else {
        superclass
    };

    let name: Name = self_inner
        .and_then(|ic| ic.inner_name.as_deref())
        .map(Name::new)
        .unwrap_or_else(|| Name::new(classfile::simple_name(&class.name)));

    let fields = class
        .fields
        .iter()
        .filter(|f| f.access & access::SYNTHETIC == 0)
        .map(|f| {
            let ty = f
                .signature
                .as_deref()
                .and_then(signature::parse_field_type)
                .or_else(|| signature::parse_field_type(&f.descriptor))
                .map(|t| lower_jtype(&t))
                .unwrap_or(TypeRef::Error);
            let mut flags = common_flags(f.access);
            if f.access & access::ENUM != 0 {
                flags.insert(Modifiers::ENUM_CONSTANT);
            }
            if f.access & access::VOLATILE != 0 {
                flags.insert(Modifiers::VOLATILE);
            }
            if f.access & access::TRANSIENT != 0 {
                flags.insert(Modifiers::TRANSIENT);
            }
            if f.deprecated || has_deprecated(&f.annotations) {
                flags.insert(Modifiers::DEPRECATED);
            }
            flags.insert_null_annotation(NullAnnotation::of(f.annotations.iter().map(|a| &**a)));
            FieldData {
                name: Name::new(&f.name),
                ty,
                flags,
                decl: None,
            }
        })
        .collect();

    let methods = class
        .methods
        .iter()
        .filter(|m| m.access & (access::SYNTHETIC | access::BRIDGE) == 0 && m.name != "<clinit>")
        .filter_map(|m| {
            let is_constructor = m.name == "<init>";
            let generic = m.signature.as_deref().and_then(signature::parse_method_sig);
            let has_generic_sig = generic.is_some();
            let sig = generic.or_else(|| signature::parse_method_sig(&m.descriptor))?;

            let mut param_types: Vec<TypeRef> = sig.params.iter().map(lower_jtype).collect();
            let mut param_names: &[String] = &m.param_names;
            // 没有泛型签名时，内部类构造器的描述符第一个参数是外层实例，源码里看不到
            if is_constructor
                && is_inner_instance_class
                && !has_generic_sig
                && !param_types.is_empty()
            {
                param_types.remove(0);
                if param_names.len() == param_types.len() + 1 {
                    param_names = &param_names[1..];
                }
            }
            let names_usable = param_names.len() == param_types.len();
            // 形参注解的个数可能和参数个数不同（编译器合成的参数），按末尾对齐
            let n_params = param_types.len();
            let n_annotated = m.param_annotations.len();
            let params = param_types
                .into_iter()
                .enumerate()
                .map(|(i, ty)| ParamData {
                    name: names_usable.then(|| Name::new(&param_names[i])),
                    ty,
                    nullness: (i + n_annotated)
                        .checked_sub(n_params)
                        .and_then(|j| m.param_annotations.get(j))
                        .map(|list| NullAnnotation::of(list.iter().map(|a| &**a)))
                        .unwrap_or_default(),
                })
                .collect();

            let mut flags = common_flags(m.access);
            if m.access & access::VARARGS != 0 {
                flags.insert(Modifiers::VARARGS);
            }
            if m.access & access::SYNCHRONIZED != 0 {
                flags.insert(Modifiers::SYNCHRONIZED);
            }
            if m.access & access::NATIVE != 0 {
                flags.insert(Modifiers::NATIVE);
            }
            if m.deprecated || has_deprecated(&m.annotations) {
                flags.insert(Modifiers::DEPRECATED);
            }
            flags.insert_null_annotation(NullAnnotation::of(m.annotations.iter().map(|a| &**a)));
            let is_abstract = m.access & access::ABSTRACT != 0;
            if matches!(kind, ClassKind::Interface)
                && !is_abstract
                && m.access & (access::STATIC | access::PRIVATE) == 0
            {
                flags.insert(Modifiers::DEFAULT);
            }

            let throws = if sig.throws.is_empty() {
                m.exceptions
                    .iter()
                    .map(|name| TypeRef::class(name))
                    .collect()
            } else {
                sig.throws.iter().map(lower_jtype).collect()
            };

            Some(MethodData {
                name: if is_constructor {
                    name.clone()
                } else {
                    Name::new(&m.name)
                },
                is_constructor,
                type_params: lower_type_params(&sig.type_params),
                params,
                ret: sig.ret.as_ref().map(lower_jtype).unwrap_or(TypeRef::Void),
                throws,
                flags,
                has_body: !is_abstract,
                decl: None,
            })
        })
        .collect();

    let nested = class
        .inner_classes
        .iter()
        .filter(|ic| ic.outer.as_deref() == Some(class.name.as_str()))
        .filter_map(|ic| ic.inner_name.as_deref().map(Name::new))
        .collect();

    ClassData {
        name,
        binary_name: SmolStr::new(&class.name),
        kind,
        flags,
        type_params,
        superclass,
        interfaces,
        permits: class
            .permitted_subclasses
            .iter()
            .map(|name| TypeRef::class(name))
            .collect(),
        fields,
        methods,
        nested,
        outer: outer.map(SmolStr::new),
        annotations: class
            .annotations
            .iter()
            .map(|desc| {
                // `Ljava/lang/Deprecated;` → `java/lang/Deprecated`
                SmolStr::new(
                    desc.strip_prefix('L')
                        .and_then(|d| d.strip_suffix(';'))
                        .unwrap_or(desc),
                )
            })
            .collect(),
        decl: None,
        is_source: false,
    }
}

fn has_deprecated(annotations: &[String]) -> bool {
    annotations.iter().any(|a| a == "Ljava/lang/Deprecated;")
}

fn common_flags(access_flags: u16) -> Modifiers {
    let mut flags = Modifiers::default();
    let table: &[(u16, u32)] = &[
        (access::PUBLIC, Modifiers::PUBLIC),
        (access::PROTECTED, Modifiers::PROTECTED),
        (access::PRIVATE, Modifiers::PRIVATE),
        (access::STATIC, Modifiers::STATIC),
        (access::FINAL, Modifiers::FINAL),
        (access::ABSTRACT, Modifiers::ABSTRACT),
    ];
    for (bit, flag) in table {
        if access_flags & bit != 0 {
            flags.insert(*flag);
        }
    }
    flags
}

fn lower_type_params(params: &[TypeParamSig]) -> Vec<TypeParamData> {
    params
        .iter()
        .map(|p| TypeParamData {
            name: Name::new(&p.name),
            bounds: p.bounds.iter().map(lower_jtype).collect(),
        })
        .collect()
}

fn lower_jtype(ty: &JType) -> TypeRef {
    match ty {
        JType::Base(base) => TypeRef::Primitive(match base {
            BaseType::Boolean => PrimitiveType::Boolean,
            BaseType::Byte => PrimitiveType::Byte,
            BaseType::Short => PrimitiveType::Short,
            BaseType::Char => PrimitiveType::Char,
            BaseType::Int => PrimitiveType::Int,
            BaseType::Long => PrimitiveType::Long,
            BaseType::Float => PrimitiveType::Float,
            BaseType::Double => PrimitiveType::Double,
        }),
        JType::Class { name, args } => TypeRef::Class {
            name: SmolStr::new(name),
            args: args
                .iter()
                .map(|arg| match arg {
                    TypeArg::Unbounded => TypeRef::Wildcard(WildcardBound::Unbounded),
                    TypeArg::Extends(t) => {
                        TypeRef::Wildcard(WildcardBound::Extends(Box::new(lower_jtype(t))))
                    }
                    TypeArg::Super(t) => {
                        TypeRef::Wildcard(WildcardBound::Super(Box::new(lower_jtype(t))))
                    }
                    TypeArg::Exact(t) => lower_jtype(t),
                })
                .collect(),
        },
        JType::TypeVar(name) => TypeRef::TypeVar(Name::new(name)),
        JType::Array(element) => TypeRef::Array(Box::new(lower_jtype(element))),
    }
}

#[cfg(test)]
mod tests {
    use classfile::{FieldInfo, InnerClassInfo, MethodInfo};

    use super::*;

    #[test]
    fn lowers_generic_class() {
        let class = ClassFile {
            access: access::PUBLIC | access::INTERFACE | access::ABSTRACT,
            name: "java/util/Map$Entry".into(),
            super_name: Some("java/lang/Object".into()),
            signature: Some("<K:Ljava/lang/Object;V:Ljava/lang/Object;>Ljava/lang/Object;".into()),
            inner_classes: vec![InnerClassInfo {
                inner: "java/util/Map$Entry".into(),
                outer: Some("java/util/Map".into()),
                inner_name: Some("Entry".into()),
                access: access::PUBLIC | access::STATIC | access::INTERFACE | access::ABSTRACT,
            }],
            fields: vec![FieldInfo {
                access: access::PUBLIC | access::STATIC | access::FINAL,
                name: "X".into(),
                descriptor: "I".into(),
                ..FieldInfo::default()
            }],
            methods: vec![
                MethodInfo {
                    access: access::PUBLIC | access::ABSTRACT,
                    name: "getKey".into(),
                    descriptor: "()Ljava/lang/Object;".into(),
                    signature: Some("()TK;".into()),
                    ..MethodInfo::default()
                },
                MethodInfo {
                    access: access::PUBLIC | access::SYNTHETIC | access::BRIDGE,
                    name: "bridge".into(),
                    descriptor: "()V".into(),
                    ..MethodInfo::default()
                },
            ],
            ..ClassFile::default()
        };
        let data = lower_class_file(&class);
        assert_eq!(data.name, "Entry");
        assert_eq!(data.kind, ClassKind::Interface);
        assert!(data.flags.is_static());
        assert_eq!(data.outer.as_deref(), Some("java/util/Map"));
        assert_eq!(data.type_params.len(), 2);
        assert_eq!(data.superclass, None);
        assert_eq!(data.methods.len(), 1, "桥接方法应被过滤");
        assert_eq!(data.methods[0].ret, TypeRef::TypeVar(Name::new("K")));
        assert_eq!(data.fields[0].ty, TypeRef::Primitive(PrimitiveType::Int));
    }
}
