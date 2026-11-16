//! `TypeRef` → `Ty`，以及签名级的类型查询（父类型、字段类型、方法签名、类型参数约束）。

use std::sync::Arc;

use base_db::Db;
use hir_def::{
    class_data, class_header, lookup_class_in_scope, resolve_type_param, resolve_type_path,
    ClassId, FieldId, FieldRef, GenericOwner, MethodId, MethodRef, Modifiers, TypeNs, TypeParamId,
    TypeRef, TypeScope, WildcardBound,
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
        TypeRef::Class { name, args } => match lookup_class_in_scope(db, scope, name) {
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
    let n = class_header(db, class).map_or(0, |data| data.type_params.len());
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
    let Some(data) = class_header(db, class) else {
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
    field_sig(db, field).ty.clone()
}

/// 字段的签名：声明的类型和声明里写的样子。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FieldSig {
    pub ty: Ty,
    pub data: Option<FieldRef>,
}

/// 字段的签名。按字段缓存：类里别的成员变化时结果不变，用到它的查询不必重算。
pub fn field_sig(db: &dyn Db, field: FieldId) -> &FieldSig {
    field_sig_tracked(db, base_db::workspace(db), field)
}

/// 字段声明里写的样子（类型、修饰符、常量值……）。只依赖这个字段自己的签名。
pub fn field_data(db: &dyn Db, field: FieldId) -> Option<FieldRef> {
    field_sig(db, field).data.clone()
}

pub fn field_flags(db: &dyn Db, field: FieldId) -> Option<Modifiers> {
    field_sig(db, field).data.as_ref().map(|f| f.flags)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked(returns(ref))]
fn field_sig_tracked(db: &dyn Db, _ws: base_db::Workspace, field: FieldId) -> FieldSig {
    let data = class_data(db, field.class).and_then(|class| FieldRef::new(class, field.idx));
    let ty = match &data {
        Some(f) => lower_type_ref(db, &TypeScope::class(db, field.class), &f.ty),
        None => Ty::Unknown,
    };
    FieldSig { ty, data }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct MethodSig {
    pub params: Vec<Ty>,
    /// 构造器为 `Void`
    pub ret: Ty,
    pub throws: Vec<Ty>,
    pub varargs: bool,
    pub n_type_params: u32,
    /// 方法声明里写的样子（名字、修饰符、形参……）
    pub data: Option<MethodRef>,
}

/// 方法的签名。按方法缓存：类里别的成员变化时结果不变，用到它的查询不必重算。
pub fn method_sig(db: &dyn Db, method: MethodId) -> Arc<MethodSig> {
    method_sig_tracked(db, base_db::workspace(db), method)
}

/// 方法声明里写的样子（名字、修饰符、形参……）。只依赖这个方法自己的签名。
pub fn method_data(db: &dyn Db, method: MethodId) -> Option<MethodRef> {
    method_sig(db, method).data.clone()
}

pub fn method_flags(db: &dyn Db, method: MethodId) -> Option<Modifiers> {
    method_sig(db, method).data.as_ref().map(|m| m.flags)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked]
fn method_sig_tracked(db: &dyn Db, _ws: base_db::Workspace, method: MethodId) -> Arc<MethodSig> {
    let Some(m) = class_data(db, method.class).and_then(|class| MethodRef::new(class, method.idx))
    else {
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
        data: Some(m),
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
            let Some(data) = class_header(db, class) else {
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
