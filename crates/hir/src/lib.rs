//! 语义层对外的门面。
//!
//! IDE 功能只通过这里和语法树打交道：给一个语法节点，问"它是什么"（[`Semantics`]），
//! 或者给一个定义，问"它在哪里声明"（[`Definition::source`]）。

mod definition;
mod semantics;

pub use base_db::{Db, FileId, SourceFile};
pub use hir_def::{
    all_supers, class_data, class_decl_file, class_origin, class_source_file, direct_supers,
    is_compact_source, is_subclass_of, is_visible_from, item_tree, library_class_source,
    library_method_source, lookup_class, member_type, method_param_names, module_graph,
    module_import_candidates, module_imports, package_classes, package_exists,
    permitted_subclasses, resolve_qualified_class, resolve_simple_type, resolve_type_path,
    source_class_index, source_map, source_subclasses, subpackages, AccessorKind, ClassData,
    ClassId, ClassKind, ClassOrigin, FieldData, FieldId, GenericOwner, Import, ItemTree,
    MethodData, MethodId, Modifiers, ModuleDeclData, ModuleGraph, ModuleInfo, Name, ParamData,
    PrimitiveType, TypeNs, TypeParamId, TypeRef, TypeScope, Visibility,
};
pub use hir_ty::{
    all_fields, all_methods, all_super_tys, body_syntax, class_supertypes, constructors, field_ty,
    find_super, functional_method, infer, is_accessible, is_accessible_via, is_assignable,
    is_subtype, lookup_field, lookup_methods, lower_type_ref, method_sig, this_ty,
    type_param_bounds, type_param_name, BodyId, CallInfo, FieldCandidate, InferDiagnostic,
    InferDiagnosticKind, InferenceResult, LocalData, LocalId, LocalKind, MethodCandidate,
    MethodSig, Resolution, Subst, Ty, Wildcard,
};

pub use crate::{
    definition::{DeclSource, Definition, LocalDef},
    semantics::Semantics,
};
