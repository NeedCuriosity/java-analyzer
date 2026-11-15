//! `TypeRef` → `Ty`，以及签名级的类型查询（父类型、字段类型、方法签名、类型参数约束）。

use std::sync::Arc;

use base_db::Db;
use hir_def::{
    class_data, lookup_class, resolve_type_param, resolve_type_path, ClassId, FieldId,
    GenericOwner, MethodId, TypeNs, TypeParamId, TypeRef, TypeScope, WildcardBound,
};

use crate::ty::{Ty, Wildcard};

pub fn lower_type_ref(db: &dyn Db, scope: &TypeScope, ty: &TypeRef) -> Ty {
    match ty {
        TypeRef::Primitive(p) => Ty::Primitive(*p),
        TypeRef::Void => Ty::Void,
        TypeRef::Error => Ty::Unknown,
        TypeRef::Path(segments) => {
            let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
            match resolve_type_path(db, scope, &names) {
                Some(TypeNs::Class(class)) => {
                    let args = segments.last().map(|s| s.args.as_slice()).unwrap_or(&[]);
                    Ty::Class(
                        class,
                        args.iter().map(|a| lower_type_ref(db, scope, a)).collect(),
                    )
                }
                Some(TypeNs::TypeParam(param)) => Ty::TypeParam(param),
                None => Ty::Unknown,
            }
        }
        TypeRef::Class { name, args } => match lookup_class(db, name) {
            Some(class) => Ty::Class(
                class,
                args.iter().map(|a| lower_type_ref(db, scope, a)).collect(),
            ),
            None => Ty::Unknown,
        },
        TypeRef::TypeVar(name) => {
            resolve_type_param(db, scope, name).map_or(Ty::Unknown, Ty::TypeParam)
        }
        TypeRef::Array(element) => Ty::Array(Arc::new(lower_type_ref(db, scope, element))),
        TypeRef::Wildcard(bound) => Ty::Wildcard(match bound {
            WildcardBound::Unbounded => Wildcard::Unbounded,
            WildcardBound::Extends(b) => Wildcard::Extends(Arc::new(lower_type_ref(db, scope, b))),
            WildcardBound::Super(b) => Wildcard::Super(Arc::new(lower_type_ref(db, scope, b))),
        }),
        TypeRef::Intersection(parts) => {
            Ty::Intersection(parts.iter().map(|p| lower_type_ref(db, scope, p)).collect())
        }
    }
}

/// 类在自己内部看到的类型：实参就是它自己的类型参数（`List<E>`）。
pub fn this_ty(db: &dyn Db, class: ClassId) -> Ty {
    let n = class_data(db, class).map_or(0, |data| data.type_params.len());
    let args = (0..n)
        .map(|i| {
            Ty::TypeParam(TypeParamId {
                owner: GenericOwner::Class(class),
                idx: i as u32,
            })
        })
        .collect();
    Ty::Class(class, args)
}

/// 直接父类型（父类在前，接口在后），类型实参用本类的类型参数表示。
pub fn class_supertypes(db: &dyn Db, class: ClassId) -> Arc<Vec<Ty>> {
    class_supertypes_tracked(db, base_db::workspace(db), class)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked]
fn class_supertypes_tracked(db: &dyn Db, _ws: base_db::Workspace, class: ClassId) -> Arc<Vec<Ty>> {
    let Some(data) = class_data(db, class) else {
        return Arc::default();
    };
    let scope = TypeScope::class_header(db, class);
    let res = data
        .superclass
        .iter()
        .chain(data.interfaces.iter())
        .map(|ty| lower_type_ref(db, &scope, ty))
        .filter(|ty| matches!(ty, Ty::Class(sup, _) if *sup != class))
        .collect();
    Arc::new(res)
}

pub fn field_ty(db: &dyn Db, field: FieldId) -> Ty {
    field_ty_tracked(db, base_db::workspace(db), field)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked]
fn field_ty_tracked(db: &dyn Db, _ws: base_db::Workspace, field: FieldId) -> Ty {
    let Some(data) = class_data(db, field.class) else {
        return Ty::Unknown;
    };
    let Some(field_data) = data.fields.get(field.idx as usize) else {
        return Ty::Unknown;
    };
    lower_type_ref(db, &TypeScope::class(db, field.class), &field_data.ty)
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct MethodSig {
    pub params: Vec<Ty>,
    /// 构造器为 `Void`
    pub ret: Ty,
    pub throws: Vec<Ty>,
    pub varargs: bool,
    pub n_type_params: u32,
}

pub fn method_sig(db: &dyn Db, method: MethodId) -> Arc<MethodSig> {
    method_sig_tracked(db, base_db::workspace(db), method)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked]
fn method_sig_tracked(db: &dyn Db, _ws: base_db::Workspace, method: MethodId) -> Arc<MethodSig> {
    let Some(data) = class_data(db, method.class) else {
        return Arc::default();
    };
    let Some(m) = data.methods.get(method.idx as usize) else {
        return Arc::default();
    };
    let scope = TypeScope::method(db, method);
    Arc::new(MethodSig {
        params: m
            .params
            .iter()
            .map(|p| lower_type_ref(db, &scope, &p.ty))
            .collect(),
        ret: lower_type_ref(db, &scope, &m.ret),
        throws: m
            .throws
            .iter()
            .map(|t| lower_type_ref(db, &scope, t))
            .collect(),
        varargs: m.is_varargs(),
        n_type_params: m.type_params.len() as u32,
    })
}

/// 类型参数的上界；没写约束时为空（即 `Object`）。
pub fn type_param_bounds(db: &dyn Db, param: TypeParamId) -> Arc<Vec<Ty>> {
    type_param_bounds_tracked(db, base_db::workspace(db), param)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked]
fn type_param_bounds_tracked(
    db: &dyn Db,
    _ws: base_db::Workspace,
    param: TypeParamId,
) -> Arc<Vec<Ty>> {
    let (scope, bounds) = match param.owner {
        GenericOwner::Class(class) => {
            let Some(data) = class_data(db, class) else {
                return Arc::default();
            };
            let Some(p) = data.type_params.get(param.idx as usize) else {
                return Arc::default();
            };
            (TypeScope::class_header(db, class), p.bounds.clone())
        }
        GenericOwner::Method(method) => {
            let Some(data) = class_data(db, method.class) else {
                return Arc::default();
            };
            let Some(p) = data
                .methods
                .get(method.idx as usize)
                .and_then(|m| m.type_params.get(param.idx as usize))
            else {
                return Arc::default();
            };
            (TypeScope::method(db, method), p.bounds.clone())
        }
    };
    Arc::new(
        bounds
            .iter()
            .map(|b| lower_type_ref(db, &scope, b))
            .collect(),
    )
}
