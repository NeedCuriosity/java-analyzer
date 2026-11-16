//! 类型层：类型表示、子类型、成员查找、重载决议、方法体推断。

mod body_tree;
mod infer;
mod lower;
mod members;
mod method_resolution;
mod subtype;
mod ty;

pub use crate::{
    body_tree::{body_member, body_tree, BodyTree},
    infer::{
        body_syntax, infer, BodyId, CallInfo, InferDiagnostic, InferDiagnosticKind,
        InferenceResult, LocalData, LocalId, LocalKind, Resolution,
    },
    lower::{
        class_supertypes, field_ty, lower_type_ref, method_sig, this_ty, type_param_bounds,
        MethodSig,
    },
    members::{
        all_fields, all_methods, constructors, functional_method, is_accessible, is_accessible_via,
        lookup_field, lookup_methods, FieldCandidate, MethodCandidate,
    },
    method_resolution::{resolve_call, Arg, Call, Inference},
    subtype::{
        all_super_tys, binary_numeric_promotion, find_super, find_super_keeping_raw, is_assignable,
        is_subtype, primitive_widens,
    },
    ty::{type_param_name, Subst, Ty, Wildcard},
};
