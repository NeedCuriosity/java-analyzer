//! 重载决议（JLS §15.12.2）与方法类型参数的推断。
//!
//! 三个阶段依次尝试：严格调用（只允许子类型）→ 宽松调用（允许装箱 / 拆箱）→ 可变参数调用。
//! 某个阶段有适用的候选就在该阶段内选最具体的那个。
//!
//! 类型参数推断只实现了 JLS §18 的常见形态：由实参向形参做结构化的合一，
//! lambda / 方法引用的实参延后处理，最后用期望类型补全返回类型里剩下的变量。
//! 推断不出来的变量变成 `Unknown`，不报错。由实参推出的绑定确定违反声明的上界时，候选不适用。

use std::sync::Arc;

use base_db::Db;
use hir_def::{GenericOwner, MethodId, TypeParamId};

use crate::{
    lower::{method_sig, type_param_bounds, MethodSig},
    members::{functional_method, MethodCandidate},
    subtype::{find_super, is_assignable, is_subtype},
    ty::{Subst, Ty},
};

/// 一个实参在重载决议时提供的信息。
#[derive(Clone, Debug)]
pub enum Arg {
    Ty(Ty),
    /// lambda：只知道形参个数，类型要等选定方法之后才能推
    Lambda {
        arity: usize,
        /// 形参都没写类型（`x -> ...`）。零个形参的 lambda 按显式类型算
        implicit: bool,
    },
    /// 方法引用
    MethodRef,
}

/// 合一的上下文：哪些类型参数是待推断的，以及目前推出的绑定。
#[derive(Clone, Debug)]
pub struct Inference {
    pub owners: Vec<GenericOwner>,
    pub bindings: Subst,
    /// 由通配符实参（`Class<?>`、`List<? extends T>`）近似得到的绑定：真实的类型是捕获变量，
    /// 不能拿来下"违反上界"之类的结论
    pub approximate: Vec<TypeParamId>,
}

impl Inference {
    pub fn new(owners: Vec<GenericOwner>) -> Inference {
        Inference {
            owners,
            bindings: Subst::empty(),
            approximate: Vec::new(),
        }
    }

    fn is_inferable(&self, param: TypeParamId) -> bool {
        self.owners.contains(&param.owner) && !self.bindings.contains(param)
    }

    /// 实参给出的绑定能否写入：还没有绑定，或者现有的绑定只是 `Object`
    /// （多半来自 `Class<?>` 这类不带信息的实参）或者还不完整，后面更具体的实参可以覆盖它。
    fn can_bind_from_arg(&self, db: &dyn Db, param: TypeParamId) -> bool {
        self.owners.contains(&param.owner)
            && self
                .bindings
                .get(param)
                .is_none_or(|bound| *bound == Ty::object(db) || bound.contains_unknown())
    }

    /// 让形参类型 `formal` 去匹配实参类型 `actual`，记录由此得到的类型参数绑定。
    pub fn unify(&mut self, db: &dyn Db, formal: &Ty, actual: &Ty) {
        if matches!(actual, Ty::Unknown | Ty::Null) {
            return;
        }
        match formal {
            Ty::TypeParam(param) => {
                if self.can_bind_from_arg(db, *param) {
                    // 类型实参不能是基本类型；通配符取它的界来近似捕获转换
                    self.bindings
                        .set(*param, actual.wildcard_bound(db).boxed(db));
                    if matches!(actual, Ty::Wildcard(_)) {
                        if !self.approximate.contains(param) {
                            self.approximate.push(*param);
                        }
                    } else {
                        self.approximate.retain(|p| p != param);
                    }
                }
            }
            Ty::Wildcard(_) => {
                // `?` 不提供任何信息
                if matches!(actual, Ty::Wildcard(crate::ty::Wildcard::Unbounded)) {
                    return;
                }
                let bound = formal.wildcard_bound(db);
                self.unify(db, &bound, &actual.wildcard_bound(db));
            }
            Ty::Array(formal_element) => {
                if let Ty::Array(actual_element) = actual {
                    self.unify(db, formal_element, actual_element);
                }
            }
            Ty::Class(class, formal_args) if !formal_args.is_empty() => {
                let actual = actual.boxed(db);
                if let Some(found) = find_super(db, &actual, *class) {
                    for (formal_arg, actual_arg) in formal_args.iter().zip(found.type_args()) {
                        self.unify(db, formal_arg, actual_arg);
                    }
                }
            }
            _ => {}
        }
    }

    /// 反方向：用期望类型去确定 `ty`（通常是返回类型）里还没绑定的变量。
    pub fn unify_with_expected(&mut self, db: &dyn Db, ty: &Ty, expected: &Ty) {
        if matches!(expected, Ty::Unknown) {
            return;
        }
        match ty {
            Ty::TypeParam(param) => {
                let bound = expected.wildcard_bound(db);
                if self.is_inferable(*param) && !bound.contains_unknown() {
                    self.bindings.insert(*param, bound.boxed(db));
                }
            }
            Ty::Array(element) => {
                if let Ty::Array(expected_element) = expected {
                    self.unify_with_expected(db, element, expected_element);
                }
            }
            Ty::Class(class, args) if !args.is_empty() => {
                // 把 `ty` 向上转成期望类型的那个类，再逐个实参对齐。
                // 此时 `ty` 的实参里还带着未绑定的类型参数，代换会原样保留它们。
                if let Ty::Class(expected_class, expected_args) = expected {
                    let lifted = if class == expected_class {
                        Some(ty.clone())
                    } else {
                        find_super(db, ty, *expected_class)
                    };
                    if let Some(lifted) = lifted {
                        for (arg, expected_arg) in
                            lifted.type_args().iter().zip(expected_args.iter())
                        {
                            self.unify_with_expected(db, arg, expected_arg);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// 重载决议的结果。
#[derive(Clone, Debug)]
pub struct Call {
    pub method: MethodId,
    pub sig: Arc<MethodSig>,
    /// 接收者带来的代换（声明类的类型参数）
    pub receiver_subst: Subst,
    /// 方法（以及菱形构造时的类）的类型参数推断状态
    pub inference: Inference,
    /// 是否按可变参数展开的形式调用
    pub varargs_expanded: bool,
    /// 没有任何候选适用，这是按参数个数挑的"最像"的一个
    pub applicable: bool,
    /// 能确定调用有歧义时，另一个同样适用、又不比选中者更不具体的候选
    pub ambiguous: Option<MethodId>,
}

impl Call {
    /// 第 `arg_idx` 个实参对应的形参类型（已应用目前所有的代换）。
    pub fn param_ty(&self, arg_idx: usize) -> Ty {
        let n = self.sig.params.len();
        if n == 0 {
            return Ty::Unknown;
        }
        let raw = if self.varargs_expanded && arg_idx >= n - 1 {
            match &self.sig.params[n - 1] {
                Ty::Array(element) => (**element).clone(),
                other => other.clone(),
            }
        } else {
            match self.sig.params.get(arg_idx) {
                Some(ty) => ty.clone(),
                None => return Ty::Unknown,
            }
        };
        raw.subst(&self.receiver_subst)
            .subst(&self.inference.bindings)
    }

    /// 第 `arg_idx` 个实参对应的形参类型：只代换接收者带来的类型参数，方法自己的类型参数原样保留。
    pub fn formal_param_ty(&self, arg_idx: usize) -> Ty {
        raw_param(self, arg_idx).subst(&self.receiver_subst)
    }

    /// 第 `arg_idx` 个实参的类型在选定方法之后才变得更精确时，用它重新约束类型参数。
    pub fn refine_arg(&mut self, db: &dyn Db, arg_idx: usize, actual: &Ty) {
        let formal = raw_param(self, arg_idx).subst(&self.receiver_subst);
        self.inference.unify(db, &formal, actual);
    }

    /// 返回类型。`expected` 用来确定只出现在返回类型里的类型参数；仍未确定的变成 `Unknown`。
    pub fn return_ty(&mut self, db: &dyn Db, expected: Option<&Ty>) -> Ty {
        let ret = self.sig.ret.subst(&self.receiver_subst);
        self.finish_ty(db, &ret, expected)
    }

    /// 用当前的推断状态把任意一个类型定下来（菱形构造用它来算 `new Foo<>()` 的类型）。
    pub fn finish_ty(&mut self, db: &dyn Db, ty: &Ty, expected: Option<&Ty>) -> Ty {
        if let Some(expected) = expected {
            let partially = ty.subst(&self.inference.bindings);
            self.inference.unify_with_expected(db, &partially, expected);
        }
        let mut res = ty.subst(&self.inference.bindings);
        for owner in &self.inference.owners {
            res = res.erase_params_of(*owner);
        }
        res
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Strict,
    Loose,
    Varargs,
}

struct Scored {
    call: Call,
    phase: Phase,
    /// 代换后的形参类型，用来比较"谁更具体"
    params: Vec<Ty>,
}

/// 在候选里选出要调用的方法。
///
/// - `extra_owners`：除方法自身之外也待推断的类型参数所有者（菱形构造时是被构造的类）。
/// - `explicit_type_args`：`obj.<String>foo()` 里显式给出的类型实参。
pub fn resolve_call(
    db: &dyn Db,
    candidates: &[MethodCandidate],
    args: &[Arg],
    explicit_type_args: &[Ty],
    extra_owners: &[GenericOwner],
) -> Option<Call> {
    let mut scored: Vec<Scored> = Vec::new();
    let mut fallback: Option<Call> = None;
    let mut fallback_arity_matches = false;

    for candidate in candidates {
        let sig = method_sig(db, candidate.id);
        let method_owner = GenericOwner::Method(candidate.id);
        let mut owners = vec![method_owner];
        owners.extend_from_slice(extra_owners);

        let n = sig.params.len();
        let fixed_arity = args.len() == n;
        let variable_arity = sig.varargs && n > 0 && args.len() >= n - 1;

        let make_call = |varargs_expanded: bool| -> Call {
            let mut inference = Inference::new(owners.clone());
            if explicit_type_args.len() == sig.n_type_params as usize {
                for (idx, ty) in explicit_type_args.iter().enumerate() {
                    inference.bindings.insert(
                        TypeParamId {
                            owner: method_owner,
                            idx: idx as u32,
                        },
                        ty.clone(),
                    );
                }
            }
            let mut call = Call {
                method: candidate.id,
                sig: sig.clone(),
                receiver_subst: candidate.subst.clone(),
                inference,
                varargs_expanded,
                applicable: true,
                ambiguous: None,
            };
            for (idx, arg) in args.iter().enumerate() {
                if let Arg::Ty(arg_ty) = arg {
                    let formal = raw_param(&call, idx).subst(&call.receiver_subst);
                    call.inference.unify(db, &formal, arg_ty);
                }
            }
            call
        };

        // 兜底候选：优先选参数个数对得上的第一个
        let arity_matches = fixed_arity || variable_arity;
        if fallback.is_none() || (arity_matches && !fallback_arity_matches) {
            fallback = Some(make_call(variable_arity && !fixed_arity));
            fallback_arity_matches = arity_matches;
        }

        if fixed_arity {
            let call = make_call(false);
            let bounds_ok = bounds_satisfied(db, &call);
            let strict = bounds_ok && args_applicable(db, &call, args, false);
            let loose = strict || (bounds_ok && args_applicable(db, &call, args, true));
            if loose {
                let params = (0..args.len()).map(|i| call.param_ty(i)).collect();
                let phase = if strict { Phase::Strict } else { Phase::Loose };
                scored.push(Scored {
                    call,
                    phase,
                    params,
                });
                continue;
            }
        }
        if variable_arity {
            let call = make_call(true);
            if bounds_satisfied(db, &call) && args_applicable(db, &call, args, true) {
                let params = (0..args.len().max(n)).map(|i| call.param_ty(i)).collect();
                scored.push(Scored {
                    call,
                    phase: Phase::Varargs,
                    params,
                });
            }
        }
    }

    let best_phase = scored.iter().map(|s| s.phase).min();
    if let Some(best_phase) = best_phase {
        let in_phase: Vec<&Scored> = scored.iter().filter(|s| s.phase == best_phase).collect();
        let most_specific = in_phase.iter().find(|candidate| {
            in_phase.iter().all(|other| {
                std::ptr::eq(**candidate, *other)
                    || candidate.call.method == other.call.method
                    || more_specific(db, candidate, other)
            })
        });
        return Some(match most_specific {
            Some(best) => best.call.clone(),
            None => {
                let mut call = in_phase[0].call.clone();
                call.ambiguous = certain_ambiguity(db, &in_phase, args, best_phase);
                if let Some(other) = call.ambiguous {
                    // 报告时把两个互不更具体的候选都点出来：选中的那个得是极大的
                    if let Some(first) = maximal(db, &in_phase)
                        .into_iter()
                        .find(|s| s.call.method != other)
                    {
                        call = first.call.clone();
                        call.ambiguous = Some(other);
                    }
                }
                call
            }
        });
    }

    // 没有适用的候选：返回参数个数最接近的一个，让跳转、悬停等功能仍然可用
    fallback.map(|call| Call {
        applicable: false,
        ..call
    })
}

pub(crate) fn mentions_type_param(ty: &Ty) -> bool {
    match ty {
        Ty::TypeParam(_) => true,
        Ty::Class(_, args) | Ty::Intersection(args) => args.iter().any(mentions_type_param),
        Ty::Array(element) => mentions_type_param(element),
        Ty::Wildcard(crate::ty::Wildcard::Extends(bound) | crate::ty::Wildcard::Super(bound)) => {
            mentions_type_param(bound)
        }
        _ => false,
    }
}

/// 方法自己的类型参数目前的绑定是否满足声明的上界（`<T extends Number>`）。
///
/// 绑定或上界里有推断不出来的部分、绑定本身是（外层的）类型变量、绑定是由通配符实参近似
/// 得到的时候从宽，视为满足。
fn bounds_satisfied(db: &dyn Db, call: &Call) -> bool {
    bound_violation(db, call).is_none()
}

/// 第一个确定违反上界的类型参数：`(绑定, 代换后的上界)`。
fn bound_violation(db: &dyn Db, call: &Call) -> Option<(Ty, Ty)> {
    let owner = GenericOwner::Method(call.method);
    (0..call.sig.n_type_params).find_map(|idx| {
        let param = TypeParamId { owner, idx };
        if call.inference.approximate.contains(&param) {
            return None;
        }
        let binding = call.inference.bindings.get(param)?;
        if binding.contains_unknown() || mentions_type_param(binding) {
            return None;
        }
        type_param_bounds(db, param).iter().find_map(|bound| {
            let bound = bound
                .subst(&call.receiver_subst)
                .subst(&call.inference.bindings);
            let unclear = bound.contains_unknown()
                || call
                    .inference
                    .owners
                    .iter()
                    .any(|owner| bound.mentions_params_of(*owner));
            (!unclear && !is_subtype(db, binding, &bound)).then(|| (binding.clone(), bound))
        })
    })
}

/// 按实参推出的类型参数确定违反了上界，因而这个候选确定不适用：
/// 参数个数固定且对得上，实参都是已知类型，违反上界的绑定和上界里都没有类型变量。
pub fn certainly_violates_bounds(db: &dyn Db, candidate: &MethodCandidate, args: &[Arg]) -> bool {
    let sig = method_sig(db, candidate.id);
    if sig.n_type_params == 0 || sig.params.len() != args.len() {
        return false;
    }
    let mut call = Call {
        method: candidate.id,
        sig: sig.clone(),
        receiver_subst: candidate.subst.clone(),
        inference: Inference::new(vec![GenericOwner::Method(candidate.id)]),
        varargs_expanded: false,
        applicable: true,
        ambiguous: None,
    };
    for (idx, arg) in args.iter().enumerate() {
        match arg {
            Arg::Ty(ty) if !ty.contains_unknown() && !matches!(ty, Ty::Null) => {
                let formal = raw_param(&call, idx).subst(&call.receiver_subst);
                call.inference.unify(db, &formal, ty);
            }
            _ => return false,
        }
    }
    bound_violation(db, &call).is_some_and(|(binding, bound)| {
        !mentions_type_param(&binding) && !mentions_type_param(&bound)
    })
}

/// 未代换的形参类型（考虑可变参数展开）。
fn raw_param(call: &Call, arg_idx: usize) -> Ty {
    let n = call.sig.params.len();
    if n == 0 {
        return Ty::Unknown;
    }
    if call.varargs_expanded && arg_idx >= n - 1 {
        match &call.sig.params[n - 1] {
            Ty::Array(element) => (**element).clone(),
            other => other.clone(),
        }
    } else {
        call.sig.params.get(arg_idx).cloned().unwrap_or(Ty::Unknown)
    }
}

fn args_applicable(db: &dyn Db, call: &Call, args: &[Arg], allow_boxing: bool) -> bool {
    args.iter().enumerate().all(|(idx, arg)| {
        let param = call.param_ty(idx);
        match arg {
            Arg::Ty(arg_ty) => {
                if allow_boxing {
                    is_assignable(db, arg_ty, &param)
                } else {
                    is_subtype(db, arg_ty, &param)
                }
            }
            Arg::Lambda { arity, .. } => match &param {
                Ty::Class(..) => match functional_method(db, &param) {
                    Some(method) => method_sig(db, method.id).params.len() == *arity,
                    None => false,
                },
                Ty::TypeParam(_) | Ty::Unknown => true,
                _ => false,
            },
            Arg::MethodRef => match &param {
                Ty::Class(..) => functional_method(db, &param).is_some(),
                Ty::TypeParam(_) | Ty::Unknown => true,
                _ => false,
            },
        }
    })
}

/// 没有谁比别人都具体的候选里，没有别的候选比它更具体的那些（按方法去重）。
fn maximal<'a>(db: &dyn Db, in_phase: &[&'a Scored]) -> Vec<&'a Scored> {
    let mut res: Vec<&Scored> = Vec::new();
    for candidate in in_phase {
        let dominated = in_phase.iter().any(|other| {
            other.call.method != candidate.call.method && more_specific(db, other, candidate)
        });
        if !dominated && !res.iter().any(|r| r.call.method == candidate.call.method) {
            res.push(candidate);
        }
    }
    res
}

/// 能确定调用有歧义时，返回第二个极大的候选。
///
/// 只在规则简单、不会误报的情形下判断：没有可变参数展开，实参都是已知类型或隐式类型的 lambda
/// （显式类型的 lambda、方法引用有额外的"更具体"规则），候选都不是泛型方法、形参类型都已知。
/// 形参类型完全相同的两个候选只在都是静态方法、来自互不相关的类（两个静态导入撞名）时才算歧义，
/// 别的情形多半是覆写或者继承来的同一签名。
fn certain_ambiguity(
    db: &dyn Db,
    in_phase: &[&Scored],
    args: &[Arg],
    phase: Phase,
) -> Option<MethodId> {
    if phase == Phase::Varargs {
        return None;
    }
    let args_clear = args.iter().all(|arg| match arg {
        Arg::Ty(ty) => !ty.contains_unknown() && !mentions_type_param(ty),
        Arg::Lambda { implicit, .. } => *implicit,
        Arg::MethodRef => false,
    });
    let candidates_clear = in_phase.iter().all(|s| {
        s.call.sig.n_type_params == 0
            && s.params
                .iter()
                .all(|p| !p.contains_unknown() && !mentions_type_param(p))
    });
    if !args_clear || !candidates_clear {
        return None;
    }
    let maximal = maximal(db, in_phase);
    let [first, second, ..] = maximal.as_slice() else {
        return None;
    };
    if first.params != second.params {
        return Some(second.call.method);
    }
    let unrelated_statics =
        maximal.iter().all(|s| {
            hir_def::class_data(db, s.call.method.class)
                .and_then(|data| {
                    data.methods
                        .get(s.call.method.idx as usize)
                        .map(|m| m.flags.is_static())
                })
                .unwrap_or(false)
        }) && !hir_def::is_subclass_of(db, first.call.method.class, second.call.method.class)
            && !hir_def::is_subclass_of(db, second.call.method.class, first.call.method.class);
    unrelated_statics.then_some(second.call.method)
}

/// `a` 是否比 `b` 更具体：`a` 的每个形参类型都是 `b` 对应形参类型的子类型。
///
/// 代换后形参完全相同时，非泛型方法比泛型方法更具体：`f(StringBuilder)` 比
/// `<A extends Appendable> f(A)` 更具体，因为后者声明的形参是类型变量，不是前者的子类型。
fn more_specific(db: &dyn Db, a: &Scored, b: &Scored) -> bool {
    if a.params.len() != b.params.len()
        || !a
            .params
            .iter()
            .zip(&b.params)
            .all(|(pa, pb)| is_subtype(db, pa, pb))
    {
        return false;
    }
    a.params != b.params || (a.call.sig.n_type_params == 0 && b.call.sig.n_type_params > 0)
}
