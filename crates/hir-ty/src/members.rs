//! 成员查找：字段、方法（含继承）、构造器、函数式接口的抽象方法。

use base_db::Db;
use hir_def::{class_data, ClassId, ClassKind, FieldId, MethodId, Modifiers, Name};

use crate::{
    lower::{field_ty, method_sig},
    subtype::all_super_tys,
    ty::{Subst, Ty},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldCandidate {
    pub id: FieldId,
    /// 按接收者类型代换后的字段类型
    pub ty: Ty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodCandidate {
    pub id: MethodId,
    /// 声明该方法的类的类型参数 → 接收者处的实参
    pub subst: Subst,
}

impl MethodCandidate {
    /// 代换后的形参类型。
    pub fn param_tys(&self, db: &dyn Db) -> Vec<Ty> {
        method_sig(db, self.id)
            .params
            .iter()
            .map(|p| p.subst(&self.subst))
            .collect()
    }

    pub fn ret_ty(&self, db: &dyn Db) -> Ty {
        method_sig(db, self.id).ret.subst(&self.subst)
    }
}

/// 在接收者类型及其父类型里找字段。
pub fn lookup_field(db: &dyn Db, receiver: &Ty, name: &str) -> Option<FieldCandidate> {
    for ty in all_super_tys(db, receiver) {
        let Ty::Class(class, args) = &ty else {
            continue;
        };
        let Some(data) = class_data(db, *class) else {
            continue;
        };
        if let Some((idx, _)) = data.field_named(name) {
            let id = FieldId { class: *class, idx };
            let subst = Subst::for_class(db, *class, args);
            return Some(FieldCandidate {
                id,
                ty: field_ty(db, id).subst(&subst),
            });
        }
    }
    None
}

/// 接收者类型上名为 `name` 的全部方法（含继承来的重载）。被子类覆写的版本不重复出现。
pub fn lookup_methods(db: &dyn Db, receiver: &Ty, name: &str) -> Vec<MethodCandidate> {
    let mut res: Vec<MethodCandidate> = Vec::new();
    let mut seen: Vec<Vec<Ty>> = Vec::new();
    for ty in all_super_tys(db, receiver) {
        let Ty::Class(class, args) = &ty else {
            continue;
        };
        let Some(data) = class_data(db, *class) else {
            continue;
        };
        let subst = Subst::for_class(db, *class, args);
        for (idx, _) in data.methods_named(name) {
            let candidate = MethodCandidate {
                id: MethodId { class: *class, idx },
                subst: subst.clone(),
            };
            let params = candidate.param_tys(db);
            if seen.contains(&params) {
                continue;
            }
            seen.push(params);
            res.push(candidate);
        }
    }
    res
}

pub fn constructors(db: &dyn Db, class: ClassId, subst: &Subst) -> Vec<MethodCandidate> {
    let Some(data) = class_data(db, class) else {
        return Vec::new();
    };
    data.constructors()
        .map(|(idx, _)| MethodCandidate {
            id: MethodId { class, idx },
            subst: subst.clone(),
        })
        .collect()
}

/// 接收者类型上所有可见的字段（子类的同名字段遮蔽父类的）。
pub fn all_fields(db: &dyn Db, receiver: &Ty) -> Vec<FieldCandidate> {
    let mut res: Vec<FieldCandidate> = Vec::new();
    let mut seen: Vec<Name> = Vec::new();
    for ty in all_super_tys(db, receiver) {
        let Ty::Class(class, args) = &ty else {
            continue;
        };
        let Some(data) = class_data(db, *class) else {
            continue;
        };
        let subst = Subst::for_class(db, *class, args);
        for (idx, field) in data.fields.iter().enumerate() {
            if seen.contains(&field.name) {
                continue;
            }
            seen.push(field.name.clone());
            let id = FieldId {
                class: *class,
                idx: idx as u32,
            };
            res.push(FieldCandidate {
                id,
                ty: field_ty(db, id).subst(&subst),
            });
        }
    }
    res
}

/// 接收者类型上所有的方法（不含构造器），覆写关系已去重。
pub fn all_methods(db: &dyn Db, receiver: &Ty) -> Vec<MethodCandidate> {
    let mut res: Vec<MethodCandidate> = Vec::new();
    let mut seen: Vec<(Name, Vec<Ty>)> = Vec::new();
    for ty in all_super_tys(db, receiver) {
        let Ty::Class(class, args) = &ty else {
            continue;
        };
        let Some(data) = class_data(db, *class) else {
            continue;
        };
        let subst = Subst::for_class(db, *class, args);
        for (idx, method) in data.methods.iter().enumerate() {
            if method.is_constructor {
                continue;
            }
            let candidate = MethodCandidate {
                id: MethodId {
                    class: *class,
                    idx: idx as u32,
                },
                subst: subst.clone(),
            };
            let key = (method.name.clone(), candidate.param_tys(db));
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            res.push(candidate);
        }
    }
    res
}

const OBJECT_METHODS: &[(&str, usize)] = &[("equals", 1), ("hashCode", 0), ("toString", 0)];

/// 函数式接口的那个唯一的抽象方法（JLS §9.8）。
pub fn functional_method(db: &dyn Db, ty: &Ty) -> Option<MethodCandidate> {
    let class = ty.as_class()?;
    let data = class_data(db, class)?;
    if data.kind != ClassKind::Interface {
        return None;
    }

    let mut found: Option<MethodCandidate> = None;
    let mut found_key: Option<(Name, usize)> = None;
    // 被 default 方法实现掉的抽象方法不算
    let mut implemented: Vec<(Name, usize)> = Vec::new();

    for sup in all_super_tys(db, ty) {
        let Ty::Class(sup_class, args) = &sup else {
            continue;
        };
        let Some(sup_data) = class_data(db, *sup_class) else {
            continue;
        };
        if !sup_data.is_interface() {
            continue;
        }
        let subst = Subst::for_class(db, *sup_class, args);
        for (idx, method) in sup_data.methods.iter().enumerate() {
            let key = (method.name.clone(), method.params.len());
            if method.flags.is_static() {
                continue;
            }
            if !method.flags.is_abstract() {
                implemented.push(key);
                continue;
            }
            if OBJECT_METHODS.contains(&(method.name.as_str(), method.params.len()))
                || implemented.contains(&key)
            {
                continue;
            }
            match &found_key {
                Some(existing) if *existing == key => {}
                Some(_) => return None,
                None => {
                    found_key = Some(key);
                    found = Some(MethodCandidate {
                        id: MethodId {
                            class: *sup_class,
                            idx: idx as u32,
                        },
                        subst: subst.clone(),
                    });
                }
            }
        }
    }
    found
}

/// 成员是否可以从 `from_class`（及其所在的包）访问。`from_class` 为 `None` 时只看得见 public。
pub fn is_accessible(
    db: &dyn Db,
    flags: Modifiers,
    declaring_class: ClassId,
    from_class: Option<ClassId>,
) -> bool {
    use hir_def::Visibility;
    let Some(from) = from_class else {
        return flags.visibility() == Visibility::Public;
    };
    match flags.visibility() {
        Visibility::Public => true,
        Visibility::Private => outermost_class(db, declaring_class) == outermost_class(db, from),
        Visibility::Package => same_package(db, declaring_class, from),
        Visibility::Protected => {
            same_package(db, declaring_class, from)
                || hir_def::is_subclass_of(db, from, declaring_class)
                || hir_def::is_subclass_of(db, outermost_class(db, from), declaring_class)
        }
    }
}

fn same_package(db: &dyn Db, a: ClassId, b: ClassId) -> bool {
    let a = a.binary_name(db);
    let b = b.binary_name(db);
    a.rsplit_once('/').map(|(p, _)| p) == b.rsplit_once('/').map(|(p, _)| p)
}

fn outermost_class(db: &dyn Db, class: ClassId) -> ClassId {
    let name = class.binary_name(db);
    match name.split_once('$') {
        Some((outer, _)) => ClassId::from_binary_name(db, outer),
        None => class,
    }
}
