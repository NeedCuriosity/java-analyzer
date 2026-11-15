//! 类型的内部表示、替换和展示。

use std::sync::Arc;

use base_db::Db;
use hir_def::{class_data, lookup_class, ClassId, GenericOwner, PrimitiveType, TypeParamId};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Wildcard {
    Unbounded,
    Extends(Arc<Ty>),
    Super(Arc<Ty>),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Ty {
    Primitive(PrimitiveType),
    Void,
    /// `null` 字面量的类型
    Null,
    /// 类类型。实参为空表示非泛型类，或泛型类的原始类型（raw type）。
    Class(ClassId, Arc<[Ty]>),
    TypeParam(TypeParamId),
    Array(Arc<Ty>),
    /// 只出现在类型实参位置
    Wildcard(Wildcard),
    Intersection(Arc<[Ty]>),
    /// 推断不出来。所有上层功能都必须对它容错：它与任何类型相容，也不产生诊断。
    #[default]
    Unknown,
}

impl Ty {
    pub fn class(id: ClassId) -> Ty {
        Ty::Class(id, Arc::from([]))
    }

    pub fn class_with_args(id: ClassId, args: Vec<Ty>) -> Ty {
        Ty::Class(id, Arc::from(args))
    }

    pub fn array(element: Ty) -> Ty {
        Ty::Array(Arc::new(element))
    }

    /// 按二进制名构造非泛型类类型；类不存在时为 `Unknown`。
    pub fn well_known(db: &dyn Db, binary_name: &str) -> Ty {
        lookup_class(db, binary_name).map_or(Ty::Unknown, Ty::class)
    }

    pub fn object(db: &dyn Db) -> Ty {
        Ty::well_known(db, "java/lang/Object")
    }

    pub fn string(db: &dyn Db) -> Ty {
        Ty::well_known(db, "java/lang/String")
    }

    pub const BOOLEAN: Ty = Ty::Primitive(PrimitiveType::Boolean);
    pub const INT: Ty = Ty::Primitive(PrimitiveType::Int);

    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    /// 类型里是否有任何推断不出来的部分。
    pub fn contains_unknown(&self) -> bool {
        match self {
            Ty::Unknown => true,
            Ty::Class(_, args) | Ty::Intersection(args) => args.iter().any(Ty::contains_unknown),
            Ty::Array(element) => element.contains_unknown(),
            Ty::Wildcard(Wildcard::Extends(bound) | Wildcard::Super(bound)) => {
                bound.contains_unknown()
            }
            _ => false,
        }
    }

    pub fn is_primitive(&self) -> bool {
        matches!(self, Ty::Primitive(_))
    }

    pub fn is_reference(&self) -> bool {
        !matches!(self, Ty::Primitive(_) | Ty::Void | Ty::Unknown)
    }

    pub fn as_primitive(&self) -> Option<PrimitiveType> {
        match self {
            Ty::Primitive(p) => Some(*p),
            _ => None,
        }
    }

    pub fn as_class(&self) -> Option<ClassId> {
        match self {
            Ty::Class(id, _) => Some(*id),
            _ => None,
        }
    }

    pub fn type_args(&self) -> &[Ty] {
        match self {
            Ty::Class(_, args) => args,
            _ => &[],
        }
    }

    /// 基本类型装箱；其它类型原样返回。
    pub fn boxed(&self, db: &dyn Db) -> Ty {
        match self {
            Ty::Primitive(p) => Ty::well_known(db, p.boxed_class()),
            other => other.clone(),
        }
    }

    /// 包装类拆箱成基本类型；不是包装类返回 `None`。
    pub fn unboxed(&self, db: &dyn Db) -> Option<PrimitiveType> {
        match self {
            Ty::Primitive(p) => Some(*p),
            Ty::Class(id, _) => PrimitiveType::from_boxed_class(&id.binary_name(db)),
            _ => None,
        }
    }

    /// 通配符取上界（`? extends T` → `T`，`?` / `? super T` → `Object`）；其它类型原样返回。
    pub fn upper_bound(&self, db: &dyn Db) -> Ty {
        match self {
            Ty::Wildcard(Wildcard::Extends(bound)) => (**bound).clone(),
            Ty::Wildcard(_) => Ty::object(db),
            other => other.clone(),
        }
    }

    /// 通配符取"用作值时最有用"的界（`? super T` → `T`）。用于给 lambda 参数定型。
    pub fn wildcard_bound(&self, db: &dyn Db) -> Ty {
        match self {
            Ty::Wildcard(Wildcard::Extends(bound) | Wildcard::Super(bound)) => (**bound).clone(),
            Ty::Wildcard(Wildcard::Unbounded) => Ty::object(db),
            other => other.clone(),
        }
    }

    pub fn subst(&self, subst: &Subst) -> Ty {
        if subst.is_empty() {
            return self.clone();
        }
        match self {
            Ty::TypeParam(id) => subst.get(*id).cloned().unwrap_or_else(|| self.clone()),
            Ty::Class(id, args) if !args.is_empty() => {
                Ty::Class(*id, args.iter().map(|a| a.subst(subst)).collect())
            }
            Ty::Array(element) => Ty::Array(Arc::new(element.subst(subst))),
            Ty::Wildcard(Wildcard::Extends(bound)) => {
                Ty::Wildcard(Wildcard::Extends(Arc::new(bound.subst(subst))))
            }
            Ty::Wildcard(Wildcard::Super(bound)) => {
                Ty::Wildcard(Wildcard::Super(Arc::new(bound.subst(subst))))
            }
            Ty::Intersection(parts) => {
                Ty::Intersection(parts.iter().map(|p| p.subst(subst)).collect())
            }
            _ => self.clone(),
        }
    }

    /// 把残留的、属于 `owner` 的类型参数换成 `Unknown`（推断不出来的方法类型参数）。
    pub fn erase_params_of(&self, owner: GenericOwner) -> Ty {
        match self {
            Ty::TypeParam(id) if id.owner == owner => Ty::Unknown,
            Ty::Class(id, args) if !args.is_empty() => {
                Ty::Class(*id, args.iter().map(|a| a.erase_params_of(owner)).collect())
            }
            Ty::Array(element) => Ty::Array(Arc::new(element.erase_params_of(owner))),
            Ty::Wildcard(Wildcard::Extends(bound)) => {
                Ty::Wildcard(Wildcard::Extends(Arc::new(bound.erase_params_of(owner))))
            }
            Ty::Wildcard(Wildcard::Super(bound)) => {
                Ty::Wildcard(Wildcard::Super(Arc::new(bound.erase_params_of(owner))))
            }
            Ty::Intersection(parts) => {
                Ty::Intersection(parts.iter().map(|p| p.erase_params_of(owner)).collect())
            }
            _ => self.clone(),
        }
    }

    /// 类型里是否提到了属于 `owner` 的类型参数。
    pub fn mentions_params_of(&self, owner: GenericOwner) -> bool {
        match self {
            Ty::TypeParam(id) => id.owner == owner,
            Ty::Class(_, args) | Ty::Intersection(args) => {
                args.iter().any(|a| a.mentions_params_of(owner))
            }
            Ty::Array(element) => element.mentions_params_of(owner),
            Ty::Wildcard(Wildcard::Extends(bound) | Wildcard::Super(bound)) => {
                bound.mentions_params_of(owner)
            }
            _ => false,
        }
    }

    /// 用简单名渲染，例如 `Map<String, List<Integer>>`。
    pub fn display(&self, db: &dyn Db) -> String {
        let mut out = String::new();
        self.render(db, false, &mut out);
        out
    }

    /// 用全限定名渲染。
    pub fn display_qualified(&self, db: &dyn Db) -> String {
        let mut out = String::new();
        self.render(db, true, &mut out);
        out
    }

    fn render(&self, db: &dyn Db, qualified: bool, out: &mut String) {
        match self {
            Ty::Primitive(p) => out.push_str(p.name()),
            Ty::Void => out.push_str("void"),
            Ty::Null => out.push_str("null"),
            Ty::Unknown => out.push('?'),
            Ty::Class(id, args) => {
                if qualified {
                    out.push_str(&id.qualified_name(db));
                } else {
                    // 成员类显示成 `Map.Entry`
                    let binary = id.binary_name(db);
                    let tail = binary.rsplit('/').next().unwrap_or(&binary);
                    out.push_str(&tail.replace('$', "."));
                }
                if !args.is_empty() {
                    out.push('<');
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        arg.render(db, qualified, out);
                    }
                    out.push('>');
                }
            }
            Ty::TypeParam(id) => out.push_str(&type_param_name(db, *id)),
            Ty::Array(element) => {
                element.render(db, qualified, out);
                out.push_str("[]");
            }
            Ty::Wildcard(Wildcard::Unbounded) => out.push('?'),
            Ty::Wildcard(Wildcard::Extends(bound)) => {
                out.push_str("? extends ");
                bound.render(db, qualified, out);
            }
            Ty::Wildcard(Wildcard::Super(bound)) => {
                out.push_str("? super ");
                bound.render(db, qualified, out);
            }
            Ty::Intersection(parts) => {
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        out.push_str(" & ");
                    }
                    part.render(db, qualified, out);
                }
            }
        }
    }
}

pub fn type_param_name(db: &dyn Db, id: TypeParamId) -> String {
    let name = match id.owner {
        GenericOwner::Class(class) => class_data(db, class).and_then(|data| {
            data.type_params
                .get(id.idx as usize)
                .map(|p| p.name.clone())
        }),
        GenericOwner::Method(method) => class_data(db, method.class).and_then(|data| {
            let m = data.methods.get(method.idx as usize)?;
            m.type_params.get(id.idx as usize).map(|p| p.name.clone())
        }),
    };
    name.map_or_else(|| "?".to_owned(), |n| n.to_string())
}

/// 类型参数 → 类型 的替换。条目很少，线性查找即可。
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct Subst {
    entries: Vec<(TypeParamId, Ty)>,
}

impl Subst {
    pub fn empty() -> Subst {
        Subst::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: TypeParamId) -> Option<&Ty> {
        self.entries
            .iter()
            .find(|(p, _)| *p == id)
            .map(|(_, ty)| ty)
    }

    pub fn contains(&self, id: TypeParamId) -> bool {
        self.get(id).is_some()
    }

    /// 已有绑定时不覆盖。
    pub fn insert(&mut self, id: TypeParamId, ty: Ty) {
        if !self.contains(id) {
            self.entries.push((id, ty));
        }
    }

    /// 写入绑定，已有时覆盖。
    pub fn set(&mut self, id: TypeParamId, ty: Ty) {
        match self.entries.iter_mut().find(|(p, _)| *p == id) {
            Some(entry) => entry.1 = ty,
            None => self.entries.push((id, ty)),
        }
    }

    pub fn extend(&mut self, other: &Subst) {
        for (id, ty) in &other.entries {
            self.insert(*id, ty.clone());
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (TypeParamId, &Ty)> {
        self.entries.iter().map(|(id, ty)| (*id, ty))
    }

    /// 对每个绑定的类型做变换。
    pub fn map_types(&self, f: impl Fn(&Ty) -> Ty) -> Subst {
        Subst {
            entries: self.entries.iter().map(|(id, ty)| (*id, f(ty))).collect(),
        }
    }

    /// 类的类型参数 → 给定的实参。实参个数不符（原始类型）时，参数擦除为 `Object`。
    pub fn for_class(db: &dyn Db, class: ClassId, args: &[Ty]) -> Subst {
        let Some(data) = class_data(db, class) else {
            return Subst::empty();
        };
        let n = data.type_params.len();
        if n == 0 {
            return Subst::empty();
        }
        let raw = args.len() != n;
        let object = if raw { Ty::object(db) } else { Ty::Unknown };
        let entries = (0..n)
            .map(|i| {
                let id = TypeParamId {
                    owner: GenericOwner::Class(class),
                    idx: i as u32,
                };
                let ty = if raw { object.clone() } else { args[i].clone() };
                (id, ty)
            })
            .collect();
        Subst { entries }
    }

    /// 接收者类型对应的替换（非类类型为空）。
    pub fn for_receiver(db: &dyn Db, receiver: &Ty) -> Subst {
        match receiver {
            Ty::Class(class, args) => Subst::for_class(db, *class, args),
            _ => Subst::empty(),
        }
    }
}
