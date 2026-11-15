//! 签名层：把源码和 class 文件统一成 [`ClassData`]，建立全局类索引，解析类型名。
//!
//! 这一层完全不看方法体。所有查询的结果只取决于声明的"形状"，
//! 因此在方法体里敲字不会让任何全局结果失效。

mod binary;
mod data;
mod index;
mod lombok;
mod lower;
mod resolver;

pub use crate::{
    binary::lower_class_file,
    data::{
        ClassData, ClassId, ClassKind, DeclIdx, FieldData, FieldId, GenericOwner, Import, ItemTree,
        MethodData, MethodId, Modifiers, Name, ParamData, PathSegment, PrimitiveType,
        TypeParamData, TypeParamId, TypeRef, Visibility, WildcardBound,
    },
    index::{
        all_supers, class_data, class_origin, class_source_file, direct_supers, is_subclass_of,
        item_tree, item_tree_and_map, lookup_class, package_classes, package_exists,
        permitted_subclasses, source_class_index, source_map, subpackages, ClassOrigin,
        SourceClassIndex,
    },
    lower::{lower_file, lower_param_type, lower_type, SourceMap},
    resolver::{
        member_type, resolve_qualified_class, resolve_simple_type, resolve_type_param,
        resolve_type_path, resolve_type_ref_to_class, TypeNs, TypeScope,
    },
};
