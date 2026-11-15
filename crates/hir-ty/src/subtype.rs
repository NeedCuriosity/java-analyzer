//! 子类型、可赋值性与父类型遍历（JLS §4.10、§5）。
//!
//! 这里的判断在遇到 `Unknown` 或尚未推断出的类型参数时一律从宽：
//! 宁可漏报，不可误报。

use std::collections::VecDeque;

use base_db::Db;
use hir_def::{ClassId, PrimitiveType};
use rustc_hash::FxHashSet;

use crate::{
    lower::{class_supertypes, type_param_bounds},
    ty::{Subst, Ty, Wildcard},
};

/// `ty` 自身（如果是类类型）及其全部父类型，类型实参已按继承链代换。
/// 广度优先，`Object` 在最后。
pub fn all_super_tys(db: &dyn Db, ty: &Ty) -> Vec<Ty> {
    let mut res = Vec::new();
    let mut seen: FxHashSet<ClassId> = FxHashSet::default();
    let mut queue: VecDeque<Ty> = VecDeque::new();
    queue.push_back(ty.clone());

    while let Some(current) = queue.pop_front() {
        match &current {
            Ty::Class(class, args) => {
                if !seen.insert(*class) {
                    continue;
                }
                let subst = Subst::for_class(db, *class, args);
                for sup in class_supertypes(db, *class).iter() {
                    queue.push_back(sup.subst(&subst));
                }
                res.push(current);
            }
            Ty::TypeParam(param) => {
                for bound in type_param_bounds(db, *param).iter() {
                    queue.push_back(bound.clone());
                }
            }
            Ty::Intersection(parts) => queue.extend(parts.iter().cloned()),
            Ty::Wildcard(_) => queue.push_back(current.upper_bound(db)),
            Ty::Array(_) => {
                queue.push_back(Ty::well_known(db, "java/lang/Cloneable"));
                queue.push_back(Ty::well_known(db, "java/io/Serializable"));
            }
            _ => {}
        }
    }

    // Object 放在最后：接口、类型参数、数组的成员查找都要落到它上面
    let object = Ty::object(db);
    if let Ty::Class(object_id, _) = &object {
        if ty.is_reference() && !matches!(ty, Ty::Null) {
            res.retain(|t| t.as_class() != Some(*object_id));
            res.push(object);
        }
    }
    res
}

/// `ty` 作为 `target` 的子类型时，`target` 的实例化形式（带代换后的类型实参）。
pub fn find_super(db: &dyn Db, ty: &Ty, target: ClassId) -> Option<Ty> {
    if let Ty::Class(class, _) = ty {
        if *class == target {
            return Some(ty.clone());
        }
    }
    all_super_tys(db, ty)
        .into_iter()
        .find(|t| t.as_class() == Some(target))
}

fn primitive_rank(p: PrimitiveType) -> u8 {
    match p {
        PrimitiveType::Boolean => 0,
        PrimitiveType::Byte => 1,
        PrimitiveType::Short | PrimitiveType::Char => 2,
        PrimitiveType::Int => 3,
        PrimitiveType::Long => 4,
        PrimitiveType::Float => 5,
        PrimitiveType::Double => 6,
    }
}

/// 基本类型的拓宽转换（含恒等）。
pub fn primitive_widens(from: PrimitiveType, to: PrimitiveType) -> bool {
    if from == to {
        return true;
    }
    if from == PrimitiveType::Boolean || to == PrimitiveType::Boolean {
        return false;
    }
    // byte / short 不能拓宽到 char，char 也不能到 short
    if to == PrimitiveType::Char || (from == PrimitiveType::Char && to == PrimitiveType::Short) {
        return false;
    }
    primitive_rank(from) < primitive_rank(to)
}

/// 二元数值提升的结果类型。
pub fn binary_numeric_promotion(a: PrimitiveType, b: PrimitiveType) -> PrimitiveType {
    if a == PrimitiveType::Double || b == PrimitiveType::Double {
        PrimitiveType::Double
    } else if a == PrimitiveType::Float || b == PrimitiveType::Float {
        PrimitiveType::Float
    } else if a == PrimitiveType::Long || b == PrimitiveType::Long {
        PrimitiveType::Long
    } else {
        PrimitiveType::Int
    }
}

fn is_object(db: &dyn Db, class: ClassId) -> bool {
    class.binary_name(db) == "java/lang/Object"
}

pub fn is_subtype(db: &dyn Db, sub: &Ty, sup: &Ty) -> bool {
    if sub == sup {
        return true;
    }
    match (sub, sup) {
        (Ty::Unknown, _) | (_, Ty::Unknown) => true,
        (Ty::Void, _) | (_, Ty::Void) => false,
        (Ty::Primitive(a), Ty::Primitive(b)) => primitive_widens(*a, *b),
        (Ty::Primitive(_), _) | (_, Ty::Primitive(_)) => false,
        (Ty::Null, _) => true,
        // 还没推断出来的类型参数：从宽
        (_, Ty::TypeParam(_)) => true,
        (_, Ty::Intersection(parts)) => parts.iter().all(|p| is_subtype(db, sub, p)),
        (_, Ty::Wildcard(Wildcard::Unbounded)) => true,
        (_, Ty::Wildcard(Wildcard::Extends(bound))) => is_subtype(db, sub, bound),
        (_, Ty::Wildcard(Wildcard::Super(bound))) => is_subtype(db, bound, sub),
        (_, Ty::Class(class, _)) if is_object(db, *class) => true,
        (Ty::Array(a), Ty::Array(b)) => {
            if a.is_primitive() || b.is_primitive() {
                a == b
            } else {
                is_subtype(db, a, b)
            }
        }
        (Ty::Array(_), Ty::Class(class, _)) => {
            matches!(
                &*class.binary_name(db),
                "java/lang/Cloneable" | "java/io/Serializable"
            )
        }
        (_, Ty::Class(target, sup_args)) => match find_super(db, sub, *target) {
            Some(found) => type_args_contained(db, found.type_args(), sup_args),
            None => false,
        },
        _ => false,
    }
}

fn type_args_contained(db: &dyn Db, sub_args: &[Ty], sup_args: &[Ty]) -> bool {
    // 任一侧是原始类型：未经检查的转换，允许
    if sub_args.is_empty() || sup_args.is_empty() || sub_args.len() != sup_args.len() {
        return true;
    }
    sub_args
        .iter()
        .zip(sup_args)
        .all(|(sub, sup)| type_arg_contains(db, sup, sub))
}

/// 类型实参的包含关系（JLS §4.5.1）：`sup` 是否包含 `sub`。
fn type_arg_contains(db: &dyn Db, sup: &Ty, sub: &Ty) -> bool {
    match sup {
        Ty::Unknown | Ty::Wildcard(Wildcard::Unbounded) => true,
        Ty::Wildcard(Wildcard::Extends(bound)) => is_subtype(db, &sub.upper_bound(db), bound),
        Ty::Wildcard(Wildcard::Super(bound)) => match sub {
            Ty::Wildcard(Wildcard::Super(sub_bound)) => is_subtype(db, bound, sub_bound),
            Ty::Wildcard(_) => false,
            _ => is_subtype(db, bound, sub),
        },
        _ => {
            sub == sup
                || sub.contains_unknown()
                || sup.contains_unknown()
                || matches!(sub, Ty::TypeParam(_))
                || matches!(sup, Ty::TypeParam(_))
        }
    }
}

/// 赋值上下文的相容性：子类型，或经装箱 / 拆箱后相容。
pub fn is_assignable(db: &dyn Db, from: &Ty, to: &Ty) -> bool {
    if is_subtype(db, from, to) {
        return true;
    }
    match (from, to) {
        (Ty::Primitive(_), _) if to.is_reference() => is_subtype(db, &from.boxed(db), to),
        (_, Ty::Primitive(target)) => from
            .unboxed(db)
            .is_some_and(|unboxed| primitive_widens(unboxed, *target)),
        _ => false,
    }
}
