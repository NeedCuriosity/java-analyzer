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
    // 已经收下的签名 → 在 `res` 里的下标
    let mut seen: Vec<(Vec<Ty>, usize)> = Vec::new();
    for ty in all_super_tys(db, receiver) {
        let Ty::Class(class, args) = &ty else {
            continue;
        };
        let Some(data) = class_data(db, *class) else {
            continue;
        };
        let subst = Subst::for_class(db, *class, args);
        for (idx, method) in data.methods_named(name) {
            if inherited_interface_static(&data, method, *class, receiver) {
                continue;
            }
            let candidate = MethodCandidate {
                id: MethodId { class: *class, idx },
                subst: subst.clone(),
            };
            let params = candidate.param_tys(db);
            if let Some((_, at)) = seen.iter().find(|(p, _)| *p == params) {
                if class_method_wins(db, res[*at].id.class, *class) {
                    res[*at] = candidate;
                }
                continue;
            }
            seen.push((params, res.len()));
            res.push(candidate);
        }
    }
    res
}

/// 接口的静态方法不会被实现类或子接口继承（JLS §8.4.8、§9.4.1）：只有接收者就是这个接口
/// 本身时才能找到（`Map.of()` 可以，`HashMap.of()` / `Headers.of()` 找到的不是它）。
fn inherited_interface_static(
    data: &hir_def::ClassData,
    method: &hir_def::MethodData,
    class: ClassId,
    receiver: &Ty,
) -> bool {
    data.is_interface() && method.flags.is_static() && receiver.as_class() != Some(class)
}

/// 同一个签名先在接口里、后在父类里找到时，父类的方法才是继承下来的那个（JLS §8.4.8）：
/// `ClassSymbol` 同时从 `Symbol` 继承 `Type asType()`、从 `TypeElement` 继承 `TypeMirror asType()`，
/// 调用得到的是前者。`Object` 的方法不算（接口里重新声明的 `equals` 等保留接口的版本）。
fn class_method_wins(db: &dyn Db, existing: ClassId, found: ClassId) -> bool {
    let is_interface = |class: ClassId| class_data(db, class).is_some_and(|d| d.is_interface());
    is_interface(existing) && !is_interface(found) && found.binary_name(db) != "java/lang/Object"
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
    let mut seen: Vec<((Name, Vec<Ty>), usize)> = Vec::new();
    for ty in all_super_tys(db, receiver) {
        let Ty::Class(class, args) = &ty else {
            continue;
        };
        let Some(data) = class_data(db, *class) else {
            continue;
        };
        let subst = Subst::for_class(db, *class, args);
        for (idx, method) in data.methods.iter().enumerate() {
            if method.is_constructor || inherited_interface_static(&data, method, *class, receiver)
            {
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
            if let Some((_, at)) = seen.iter().find(|(k, _)| *k == key) {
                if class_method_wins(db, res[*at].id.class, *class) {
                    res[*at] = candidate;
                }
                continue;
            }
            seen.push((key, res.len()));
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

/// 通过类型为 `receiver` 的表达式访问实例成员时的可访问性。在 [`is_accessible`] 之上多一条
/// protected 的规则（JLS 6.6.2.1）：不在同一个包里时，只能通过"访问发生处的类或其子类"
/// 类型的表达式访问——`Object.clone()` 不能在任意对象上调用。
pub fn is_accessible_via(
    db: &dyn Db,
    flags: Modifiers,
    declaring_class: ClassId,
    from_class: Option<ClassId>,
    receiver: &Ty,
) -> bool {
    if !is_accessible(db, flags, declaring_class, from_class) {
        return false;
    }
    let (Some(from), Ty::Class(receiver, _)) = (from_class, receiver) else {
        return true;
    };
    if flags.visibility() != hir_def::Visibility::Protected
        || flags.is_static()
        || same_package(db, declaring_class, from)
    {
        return true;
    }
    let outer = outermost_class(db, from);
    [from, outer]
        .into_iter()
        .any(|class| *receiver == class || hir_def::is_subclass_of(db, *receiver, class))
}

fn same_package(db: &dyn Db, a: ClassId, b: ClassId) -> bool {
    let a = a.binary_name(db);
    let b = b.binary_name(db);
    a.rsplit_once('/').map(|(p, _)| p) == b.rsplit_once('/').map(|(p, _)| p)
}

fn outermost_class(db: &dyn Db, class: ClassId) -> ClassId {
    hir_def::outermost_class(db, class)
}
