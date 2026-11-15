//! 声明层面和上下文相关的类型检查：
//!
//! - 给 `final` 变量 / 字段赋值；
//! - 带限定符访问别的类的私有成员；
//! - `void` 方法里 `return 值;`、非 `void` 方法里空的 `return;`；
//! - 静态上下文里用简单名引用实例成员；
//! - 非抽象类没有实现继承来的抽象方法；
//! - 覆写时收窄了访问级别、覆写了 `final` 方法、返回类型不兼容；
//! - 继承 sealed 类型却不在它允许的子类型里（包括匿名类和局部类）；
//! - 对枚举的 switch 表达式没有覆盖所有常量；
//! - 泛型类的类型实参不在类型参数的上界之内（`Box<String>`，而 `Box<T extends Number>`）。
//!
//! 和其它语义诊断一样只报确定的：涉及泛型、解析不全的继承链、Lombok 的地方一律跳过。

use base_db::{Db, SourceFile};
use hir::{
    all_super_tys, body_syntax, class_data, infer, is_accessible, is_subtype, method_sig, BodyId,
    ClassId, ClassKind, InferenceResult, MethodId, Modifiers, PrimitiveType, Resolution, Semantics,
    Ty, Visibility,
};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{bodies_in_file, hierarchy_is_resolved, Diagnostic, DiagnosticCode, Severity};

fn error(range: TextRange, code: DiagnosticCode, message: String) -> Diagnostic {
    Diagnostic {
        range,
        message,
        severity: Severity::Error,
        code,
        unused: false,
    }
}

fn uses_lombok(db: &dyn Db, class: ClassId) -> bool {
    class_data(db, class).is_some_and(|data| data.annotations.iter().any(|a| a.contains("lombok")))
}

fn has_final(node: &SyntaxNode) -> bool {
    node.children()
        .find_map(ast::ModifierList::cast)
        .is_some_and(|list| list.has(SyntaxKind::FINAL_KW))
}

/// 声明的类型是 Lombok 的 `val`（隐含 `final`）。
fn is_lombok_val(db: &dyn Db, decl: &SyntaxNode) -> bool {
    let Some(ty) = decl.children().find_map(ast::Type::cast) else {
        return false;
    };
    let text = ty.syntax().text().to_string();
    matches!(text.trim(), "val" | "lombok.val") && hir::lookup_class(db, "lombok/val").is_some()
}

fn strip_parens(mut node: SyntaxNode) -> SyntaxNode {
    while node.kind() == SyntaxKind::PAREN_EXPR {
        match node.children().find(|c| ast::Expr::can_cast(c.kind())) {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

/// 名字表达式或字段访问里被引用的那个名字节点，以及它是否带限定符。
fn referenced_name(expr: &SyntaxNode) -> Option<(SyntaxNode, bool)> {
    let qualified = match expr.kind() {
        SyntaxKind::NAME_EXPR => false,
        SyntaxKind::FIELD_ACCESS => true,
        _ => return None,
    };
    let name = expr
        .children()
        .filter(|c| c.kind() == SyntaxKind::NAME_REF)
        .last()?;
    Some((name, qualified))
}

fn field_flags(db: &dyn Db, field: hir::FieldId) -> Option<Modifiers> {
    class_data(db, field.class)?
        .fields
        .get(field.idx as usize)
        .map(|f| f.flags)
}

fn method_flags(db: &dyn Db, method: MethodId) -> Option<(Modifiers, bool)> {
    class_data(db, method.class)?
        .methods
        .get(method.idx as usize)
        .map(|m| (m.flags, m.is_constructor))
}

/// 被赋值的表达式（`x = ..`、`x += ..`、`x++`、`--x` 的 `x`）。
fn assignment_targets(body: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut res = Vec::new();
    for node in body.descendants() {
        let target = match node.kind() {
            SyntaxKind::ASSIGN_EXPR => node.children().find(|c| ast::Expr::can_cast(c.kind())),
            SyntaxKind::UNARY_EXPR | SyntaxKind::POSTFIX_EXPR => {
                let mutates = node
                    .children_with_tokens()
                    .any(|e| matches!(e.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS));
                if mutates {
                    node.children().find(|c| ast::Expr::can_cast(c.kind()))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(target) = target {
            res.push(strip_parens(target));
        }
    }
    res
}

fn final_assignments(
    db: &dyn Db,
    body: &SyntaxNode,
    result: &InferenceResult,
    out: &mut Vec<Diagnostic>,
) {
    for target in assignment_targets(body) {
        let Some((name, _)) = referenced_name(&target) else {
            continue;
        };
        let range = name.text_range();
        match result.resolution(range) {
            Some(Resolution::Local(id)) => {
                let local = result.local(*id);
                if !body.text_range().contains_range(local.decl_range) {
                    continue;
                }
                let decl = match body.covering_element(local.decl_range) {
                    syntax::NodeOrToken::Node(node) => Some(node),
                    syntax::NodeOrToken::Token(token) => token.parent(),
                };
                let Some(declarator) = decl.and_then(|n| {
                    n.ancestors().find(|a| {
                        matches!(
                            a.kind(),
                            SyntaxKind::VARIABLE_DECLARATOR
                                | SyntaxKind::PARAM
                                | SyntaxKind::RESOURCE
                        )
                    })
                }) else {
                    continue;
                };
                let is_final = match declarator.kind() {
                    // try-with-resources 的资源隐含 final
                    SyntaxKind::RESOURCE => true,
                    SyntaxKind::PARAM => has_final(&declarator) || is_lombok_val(db, &declarator),
                    // `final int x;` 之后的第一次赋值是合法的，需要"确定未赋值"分析，这里只看带初值的
                    _ => {
                        let has_initializer = ast::VariableDeclarator::cast(declarator.clone())
                            .is_some_and(|d| d.initializer().is_some());
                        has_initializer
                            && declarator
                                .parent()
                                .is_some_and(|p| has_final(&p) || is_lombok_val(db, &p))
                    }
                };
                if is_final {
                    out.push(error(
                        range,
                        DiagnosticCode::AssignToFinal,
                        base_db::tr!(
                            "不能给 final 变量 `{}` 赋值",
                            "Cannot assign a value to final variable `{}`",
                            local.name
                        ),
                    ));
                }
            }
            Some(Resolution::Field(field)) => {
                if !field_flags(db, *field).is_some_and(|f| f.contains(Modifiers::FINAL)) {
                    continue;
                }
                // 构造器和初始化块里可以给 final 字段赋初值
                let in_initializer = target.ancestors().any(|a| {
                    matches!(
                        a.kind(),
                        SyntaxKind::CONSTRUCTOR_DECL | SyntaxKind::INITIALIZER_BLOCK
                    )
                });
                if in_initializer {
                    continue;
                }
                out.push(error(
                    range,
                    DiagnosticCode::AssignToFinal,
                    base_db::tr!(
                        "不能给 final 字段 `{}` 赋值",
                        "Cannot assign a value to final field `{}`",
                        name.text()
                    ),
                ));
            }
            _ => {}
        }
    }
}

/// `obj.secret` / `obj.secret()`：成员是别的顶层类的私有成员。
fn private_access(
    db: &dyn Db,
    body: BodyId,
    node: &SyntaxNode,
    result: &InferenceResult,
    out: &mut Vec<Diagnostic>,
) {
    for access in node.descendants() {
        let name = match access.kind() {
            SyntaxKind::FIELD_ACCESS => access
                .children()
                .filter(|c| c.kind() == SyntaxKind::NAME_REF)
                .last(),
            SyntaxKind::METHOD_CALL => {
                let call = ast::MethodCall::cast(access.clone());
                call.filter(|c| c.receiver().is_some())
                    .and_then(|c| c.name_ref())
                    .map(|n| n.syntax().clone())
            }
            _ => None,
        };
        let Some(name) = name else { continue };
        let (flags, declaring) = match result.resolution(name.text_range()) {
            Some(Resolution::Field(field)) => match field_flags(db, *field) {
                Some(flags) => (flags, field.class),
                None => continue,
            },
            Some(Resolution::Method(method)) => match method_flags(db, *method) {
                Some((flags, false)) => (flags, method.class),
                _ => continue,
            },
            _ => continue,
        };
        if flags.visibility() == Visibility::Private
            && !is_accessible(db, flags, declaring, Some(body.class()))
        {
            let owner = class_data(db, declaring)
                .map(|d| d.name.to_string())
                .unwrap_or_default();
            out.push(error(
                name.text_range(),
                DiagnosticCode::InaccessibleMember,
                base_db::tr!(
                    "`{}` 是 `{owner}` 的私有成员，这里不能访问",
                    "`{}` has private access in `{owner}`",
                    name.text()
                ),
            ));
        }
    }
}

/// 静态上下文里用简单名引用实例字段 / 实例方法。
fn static_context(
    db: &dyn Db,
    body: BodyId,
    node: &SyntaxNode,
    result: &InferenceResult,
    out: &mut Vec<Diagnostic>,
) {
    let is_static = match body {
        BodyId::Method(method) => {
            method_flags(db, method).is_some_and(|(flags, ctor)| flags.is_static() && !ctor)
        }
        BodyId::Field(field) => field_flags(db, field).is_some_and(|f| f.is_static()),
        BodyId::Initializer { .. } => node
            .children_with_tokens()
            .any(|e| e.kind() == SyntaxKind::STATIC_KW),
    };
    if !is_static {
        return;
    }
    // 匿名类 / 局部类的类体有自己的 this
    let in_nested_class = |n: &SyntaxNode| {
        n.ancestors()
            .take_while(|a| a != node)
            .any(|a| a.kind() == SyntaxKind::CLASS_BODY || ast::TypeDecl::can_cast(a.kind()))
    };
    for this in node.descendants().filter(|n| {
        matches!(n.kind(), SyntaxKind::THIS_EXPR | SyntaxKind::SUPER_EXPR)
            // `Outer.this` / `Iface.super` 带限定符的形式不在这里判断
            && n.children().next().is_none()
    }) {
        if !in_nested_class(&this) {
            out.push(error(
                this.text_range(),
                DiagnosticCode::StaticContext,
                base_db::tr!(
                    "不能在静态上下文里使用 `{}`",
                    "Cannot use `{}` in a static context",
                    this.text()
                ),
            ));
        }
    }
    for name in node
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::NAME_REF)
    {
        let Some(parent) = name.parent() else {
            continue;
        };
        let unqualified = match parent.kind() {
            SyntaxKind::NAME_EXPR => true,
            SyntaxKind::METHOD_CALL => {
                ast::MethodCall::cast(parent.clone()).is_some_and(|c| c.receiver().is_none())
            }
            _ => false,
        };
        if !unqualified {
            continue;
        }
        if in_nested_class(&name) {
            continue;
        }
        let (flags, declaring, what) = match result.resolution(name.text_range()) {
            Some(Resolution::Field(field)) => match field_flags(db, *field) {
                Some(flags) => (flags, field.class, base_db::trs!("字段", "field")),
                None => continue,
            },
            Some(Resolution::Method(method)) => match method_flags(db, *method) {
                Some((flags, false)) => (flags, method.class, base_db::trs!("方法", "method")),
                _ => continue,
            },
            _ => continue,
        };
        // 接口里的字段隐含 static
        let Some(owner) = class_data(db, declaring) else {
            continue;
        };
        if flags.is_static() || (what == base_db::trs!("字段", "field") && owner.is_interface()) {
            continue;
        }
        out.push(error(
            name.text_range(),
            DiagnosticCode::StaticContext,
            base_db::tr!(
                "不能在静态上下文里引用实例{what} `{}`",
                "Cannot reference instance {what} `{}` from a static context",
                name.text()
            ),
        ));
    }
}

/// 直接属于 `callable` 的 `return` 语句（不进入 lambda 和类体）。
fn own_returns(callable: &SyntaxNode) -> Vec<ast::ReturnStmt> {
    fn walk(node: &SyntaxNode, out: &mut Vec<ast::ReturnStmt>) {
        for child in node.children() {
            if matches!(
                child.kind(),
                SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY | SyntaxKind::SWITCH_EXPR
            ) || ast::TypeDecl::can_cast(child.kind())
            {
                continue;
            }
            if let Some(ret) = ast::ReturnStmt::cast(child.clone()) {
                out.push(ret);
            }
            walk(&child, out);
        }
    }
    let mut out = Vec::new();
    walk(callable, &mut out);
    out
}

fn return_values(root: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    for node in root.descendants() {
        let returns_value = match node.kind() {
            SyntaxKind::METHOD_DECL => {
                let Some(method) = ast::MethodDecl::cast(node.clone()) else {
                    continue;
                };
                if method.body().is_none() {
                    continue;
                }
                match method.return_type() {
                    Some(ty) => ty.syntax().text() != "void",
                    None => continue,
                }
            }
            SyntaxKind::CONSTRUCTOR_DECL | SyntaxKind::INITIALIZER_BLOCK => false,
            _ => continue,
        };
        for ret in own_returns(&node) {
            let has_value = ret.expr().is_some();
            if has_value && !returns_value {
                out.push(error(
                    ret.syntax().text_range(),
                    DiagnosticCode::TypeMismatch,
                    base_db::tr!(
                        "这里不能返回值：所在的方法没有返回类型",
                        "Cannot return a value here: the enclosing method has no return type"
                    ),
                ));
            } else if !has_value && returns_value {
                out.push(error(
                    ret.syntax().text_range(),
                    DiagnosticCode::TypeMismatch,
                    base_db::tr!("缺少返回值", "Missing return value"),
                ));
            }
        }
    }
}

/// `Object` 的公有方法：接口里重新声明它们不算抽象方法。
fn is_object_method(name: &str, arity: usize) -> bool {
    matches!(
        (name, arity),
        ("equals", 1) | ("hashCode", 0) | ("toString", 0)
    )
}

fn unimplemented_abstract_methods(db: &dyn Db, class: ClassId) -> Vec<String> {
    let Some(data) = class_data(db, class) else {
        return Vec::new();
    };
    if data.kind != ClassKind::Class && data.kind != ClassKind::Record {
        return Vec::new();
    }
    if data.flags.is_abstract() || uses_lombok(db, class) || !hierarchy_is_resolved(db, class) {
        return Vec::new();
    }
    // 直接父类不是抽象类时，父类那条继承链上的抽象方法由父类负责（javac 也只在父类上报）
    let settled_by_superclass: Vec<ClassId> = hir::direct_supers(db, class)
        .iter()
        .copied()
        .filter(|sup| {
            class_data(db, *sup).is_some_and(|d| !d.is_interface() && !d.flags.is_abstract())
        })
        .flat_map(|sup| std::iter::once(sup).chain(hir::all_supers(db, sup)))
        .collect();
    // (名字, 形参个数)：继承链上任何地方有非抽象的同名同参数个数的方法，就认为实现了
    let mut concrete: Vec<(hir::Name, usize)> = Vec::new();
    let mut abstracts: Vec<(hir::Name, usize, ClassId, Modifiers)> = Vec::new();
    for ty in all_super_tys(db, &hir::this_ty(db, class)) {
        let Ty::Class(owner, _) = ty else { continue };
        let Some(owner_data) = class_data(db, owner) else {
            return Vec::new();
        };
        if uses_lombok(db, owner) {
            return Vec::new();
        }
        for method in owner_data.methods.iter() {
            if method.is_constructor {
                continue;
            }
            let key = (method.name.clone(), method.params.len());
            // 同名的静态方法是另一种错误（静态方法不能隐藏实例方法），这里按"有实现"处理
            if method.flags.is_abstract() && !method.flags.is_static() {
                if settled_by_superclass.contains(&owner) {
                    continue;
                }
                abstracts.push((key.0, key.1, owner, method.flags));
            } else {
                concrete.push(key);
            }
        }
    }
    let mut missing: Vec<String> = abstracts
        .into_iter()
        .filter(|(name, arity, owner, flags)| {
            !is_object_method(name, *arity)
                && is_accessible(db, *flags, *owner, Some(class))
                && !concrete.iter().any(|(n, a)| n == name && a == arity)
        })
        .map(|(name, ..)| name.to_string())
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

fn visibility_rank(visibility: Visibility) -> u8 {
    match visibility {
        Visibility::Private => 0,
        Visibility::Package => 1,
        Visibility::Protected => 2,
        Visibility::Public => 3,
    }
}

/// 类型里不含类型参数、通配符和未知部分，可以直接比较。
fn is_plain(db: &dyn Db, ty: &Ty) -> bool {
    match ty {
        Ty::Primitive(_) | Ty::Void => true,
        Ty::Class(class, args) => {
            args.is_empty() && class_data(db, *class).is_some_and(|d| d.type_params.is_empty())
        }
        Ty::Array(element) => is_plain(db, element),
        _ => false,
    }
}

fn override_checks(
    db: &dyn Db,
    method: MethodId,
    name_range: TextRange,
    out: &mut Vec<Diagnostic>,
) {
    let Some(data) = class_data(db, method.class) else {
        return;
    };
    let Some(decl) = data.methods.get(method.idx as usize) else {
        return;
    };
    if decl.is_constructor || !decl.type_params.is_empty() || uses_lombok(db, method.class) {
        return;
    }
    let sig = method_sig(db, method);
    if !sig.params.iter().all(|p| is_plain(db, p)) {
        return;
    }
    // 接口里的方法隐含 public
    let own_rank = if data.is_interface() {
        3
    } else {
        visibility_rank(decl.flags.visibility())
    };

    for owner in hir::all_supers(db, method.class) {
        let Some(owner_data) = class_data(db, owner) else {
            continue;
        };
        for (idx, candidate) in owner_data.methods.iter().enumerate() {
            if candidate.is_constructor
                || candidate.name != decl.name
                || candidate.params.len() != decl.params.len()
                || !candidate.type_params.is_empty()
            {
                continue;
            }
            let super_rank = if owner_data.is_interface() {
                3
            } else {
                visibility_rank(candidate.flags.visibility())
            };
            // 接口的静态方法不被继承
            if candidate.flags.is_static() && owner_data.is_interface() {
                continue;
            }
            // 私有方法不被继承；包私有的只在同一个包里才被继承
            let same_package = {
                let package = |name: &str| name.rsplit_once('/').map(|(p, _)| p.to_owned());
                package(&owner_data.binary_name) == package(&data.binary_name)
            };
            if super_rank == 0 || (super_rank == 1 && !same_package) {
                continue;
            }
            let super_sig = method_sig(
                db,
                MethodId {
                    class: owner,
                    idx: idx as u32,
                },
            );
            if super_sig.params != sig.params {
                continue;
            }
            let owner_name = &owner_data.name;
            if candidate.flags.is_static() != decl.flags.is_static() {
                let message = if decl.flags.is_static() {
                    base_db::tr!(
                        "静态方法 `{}` 不能隐藏 `{owner_name}` 里的实例方法",
                        "Static method `{}` cannot hide the instance method in `{owner_name}`",
                        decl.name
                    )
                } else {
                    base_db::tr!(
                        "实例方法 `{}` 不能覆写 `{owner_name}` 里的静态方法",
                        "Instance method `{}` cannot override the static method in `{owner_name}`",
                        decl.name
                    )
                };
                out.push(error(name_range, DiagnosticCode::InvalidOverride, message));
                return;
            }
            if candidate.flags.contains(Modifiers::FINAL) {
                out.push(error(
                    name_range,
                    DiagnosticCode::InvalidOverride,
                    base_db::tr!(
                        "不能覆写 `{owner_name}` 里的 final 方法 `{}`",
                        "Cannot override final method `{}` in `{owner_name}`",
                        decl.name
                    ),
                ));
                return;
            }
            if own_rank < super_rank {
                out.push(error(
                    name_range,
                    DiagnosticCode::InvalidOverride,
                    base_db::tr!(
                        "覆写 `{owner_name}.{}` 时不能收窄访问级别",
                        "Cannot reduce the visibility when overriding `{owner_name}.{}`",
                        decl.name
                    ),
                ));
                return;
            }
            if is_plain(db, &sig.ret) && is_plain(db, &super_sig.ret) {
                let compatible = match (&sig.ret, &super_sig.ret) {
                    (Ty::Void, Ty::Void) => true,
                    (Ty::Void, _) | (_, Ty::Void) => false,
                    (Ty::Primitive(a), Ty::Primitive(b)) => a == b,
                    (Ty::Primitive(_), _) | (_, Ty::Primitive(_)) => false,
                    (own, sup) => is_subtype(db, own, sup),
                };
                if !compatible {
                    out.push(error(
                        name_range,
                        DiagnosticCode::InvalidOverride,
                        base_db::tr!(
                            "返回类型 `{}` 和被覆写的 `{owner_name}.{}` 的返回类型 `{}` 不兼容", "Return type `{}` is incompatible with overridden `{owner_name}.{}`, which returns `{}`",
                            sig.ret.display(db),
                            decl.name,
                            super_sig.ret.display(db)
                        ),
                    ));
                    return;
                }
            }
        }
    }
}

/// 基本类型之间的转换是否无条件精确：恒等，或者不丢信息的拓宽（`int` → `long`、`int` → `double`，
/// 但 `int` → `float` 不算）。
fn exact_primitive(from: PrimitiveType, to: PrimitiveType) -> bool {
    use PrimitiveType::*;
    from == to
        || matches!(
            (from, to),
            (Byte, Short | Int | Long | Float | Double)
                | (Short | Char, Int | Long | Float | Double)
                | (Int, Long | Double)
                | (Float, Double)
        )
}

/// 类型为 `pattern` 的类型模式是否无条件匹配 `ty` 的所有（非 null）值。引用类型只比较擦除后的类，
/// 涉及类型变量等说不准的情形返回 `None`。
fn is_unconditional(db: &dyn Db, ty: &Ty, pattern: &Ty) -> Option<bool> {
    match (ty, pattern) {
        (Ty::Primitive(a), Ty::Primitive(b)) => Some(exact_primitive(*a, *b)),
        // 装箱之后再做引用类型的拓宽：`int` 对 `Integer` / `Number` / `Object`
        (Ty::Primitive(_), Ty::Class(..)) => Some(is_subtype(db, &ty.boxed(db), pattern)),
        // 拆箱之后再做精确的基本类型转换：`Integer` 对 `int` / `long`
        (Ty::Class(..), Ty::Primitive(b)) => {
            Some(ty.unboxed(db).is_some_and(|a| exact_primitive(a, *b)))
        }
        (Ty::Class(c, _), Ty::Class(p, _)) => Some(hir::is_subclass_of(db, *c, *p)),
        _ => None,
    }
}

/// 涉及基本类型时，类型为 `ty` 的值能否匹配类型为 `pattern` 的模式（`String` 永远匹配不了
/// `int`）。两边都是引用类型、或者说不准时返回 `None`。
fn primitive_pattern_applicable(db: &dyn Db, ty: &Ty, pattern: &Ty) -> Option<bool> {
    let is_boolean = |p: PrimitiveType| p == PrimitiveType::Boolean;
    match (ty, pattern) {
        (Ty::Primitive(a), Ty::Primitive(b)) => Some(is_boolean(*a) == is_boolean(*b)),
        (Ty::Primitive(_), Ty::Class(_, args)) if args.is_empty() => {
            Some(is_subtype(db, &ty.boxed(db), pattern))
        }
        (Ty::Class(_, args), Ty::Primitive(b)) if args.is_empty() => match ty.unboxed(db) {
            Some(a) => Some(is_boolean(a) == is_boolean(*b)),
            None => Some(is_subtype(db, &pattern.boxed(db), ty)),
        },
        _ => None,
    }
}

/// `ty` 的所有值是否都被 `patterns`（不带守卫的模式的类型）或 `constants`（枚举常量名）覆盖。
fn is_covered(
    db: &dyn Db,
    class: ClassId,
    patterns: &[Ty],
    constants: &[String],
    depth: u32,
) -> bool {
    let ty = Ty::class(class);
    if patterns.iter().any(|pattern| match pattern {
        Ty::Class(p, _) => hir::is_subclass_of(db, class, *p),
        _ => is_subtype(db, &ty, pattern),
    }) {
        return true;
    }
    let Some(data) = class_data(db, class) else {
        return false;
    };
    if data.kind == ClassKind::Enum {
        return data
            .fields
            .iter()
            .filter(|f| f.is_enum_constant())
            .all(|f| constants.iter().any(|c| c == f.name.as_str()));
    }
    // sealed 的抽象类 / 接口：每个允许的子类型都被覆盖
    if depth > 8 || !(data.is_interface() || data.flags.is_abstract()) {
        return false;
    }
    match hir::permitted_subclasses(db, class) {
        Some(subs) if !subs.is_empty() => subs
            .iter()
            .all(|sub| is_covered(db, *sub, patterns, constants, depth + 1)),
        _ => false,
    }
}

/// 记录模式是否无条件匹配它的类型：每个子模式都是 `var` / 不带类型限制的绑定，这里只认
/// "子模式的类型正好是组件的声明类型"这一种，其它情形按不确定处理。
fn record_pattern_is_total(
    sema: &Semantics<'_>,
    file: SourceFile,
    pattern: &ast::RecordPattern,
) -> Option<bool> {
    let ty = sema.resolve_type(file, &pattern.ty()?);
    let Ty::Class(class, _) = &ty else {
        return None;
    };
    let data = class_data(sema.db, *class)?;
    let components: Vec<_> = data
        .fields
        .iter()
        .filter(|f| !f.flags.is_static())
        .collect();
    let subs: Vec<ast::Pattern> = pattern.patterns().collect();
    if components.len() != subs.len() {
        return None;
    }
    for (idx, sub) in subs.iter().enumerate() {
        match sub {
            ast::Pattern::UnnamedPattern(_) => {}
            ast::Pattern::TypePattern(type_pattern) => {
                let written = type_pattern.ty()?;
                if written.syntax().text() == "var" {
                    continue;
                }
                let sub_ty = sema.resolve_type(file, &written);
                let component_ty = hir::field_ty(
                    sema.db,
                    hir::FieldId {
                        class: *class,
                        idx: data
                            .fields
                            .iter()
                            .position(|f| std::ptr::eq(f, components[idx]))?
                            as u32,
                    },
                );
                if sub_ty.contains_unknown() || component_ty.contains_unknown() {
                    return None;
                }
                // 基本类型的组件：子模式的类型只要是精确转换的目标就无条件匹配（`double x` 对 `int x`）
                if matches!(sub_ty, Ty::Primitive(_)) || matches!(component_ty, Ty::Primitive(_)) {
                    match is_unconditional(sema.db, &component_ty, &sub_ty) {
                        Some(true) => continue,
                        other => return other,
                    }
                }
                // 子模式的类型不比组件类型宽时可能匹配不上；涉及泛型时不下结论
                if sub_ty != component_ty && !is_subtype(sema.db, &component_ty, &sub_ty) {
                    return if is_plain(sema.db, &sub_ty) && is_plain(sema.db, &component_ty) {
                        Some(false)
                    } else {
                        None
                    };
                }
            }
            ast::Pattern::RecordPattern(_) => return None,
        }
    }
    Some(true)
}

/// switch 表达式（以及带模式的 switch 语句）没有 `default` 时必须穷尽。
fn switch_exhaustiveness(
    db: &dyn Db,
    sema: &Semantics<'_>,
    file: SourceFile,
    node: &SyntaxNode,
    result: &InferenceResult,
    out: &mut Vec<Diagnostic>,
) {
    for switch in node.descendants() {
        let is_expr = switch.kind() == SyntaxKind::SWITCH_EXPR;
        if !is_expr && switch.kind() != SyntaxKind::SWITCH_STMT {
            continue;
        }
        let Some(scrutinee) = switch.children().find_map(ast::Expr::cast) else {
            continue;
        };
        let Some(block) = switch.children().find_map(ast::SwitchBlock::cast) else {
            continue;
        };
        let Some(scrutinee_ty) = result.type_of(scrutinee.syntax().text_range()) else {
            continue;
        };
        if scrutinee_ty.contains_unknown() {
            continue;
        }

        let mut constants: Vec<String> = Vec::new();
        let mut patterns: Vec<Ty> = Vec::new();
        let mut has_pattern = false;
        let mut certain = true;
        let mut has_default = false;
        for case in block.cases() {
            let Some(label) = case.label() else {
                certain = false;
                continue;
            };
            // `default` 和 `case null, default` 两种写法
            if label
                .syntax()
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::DEFAULT_KW)
            {
                has_default = true;
            }
            let guarded = label.guard().is_some();
            for pattern in label.patterns() {
                has_pattern = true;
                if guarded {
                    continue;
                }
                match &pattern {
                    ast::Pattern::TypePattern(type_pattern) => match type_pattern.ty() {
                        Some(ty) => patterns.push(sema.resolve_type(file, &ty)),
                        None => certain = false,
                    },
                    ast::Pattern::RecordPattern(record) => {
                        match (record_pattern_is_total(sema, file, record), record.ty()) {
                            (Some(true), Some(ty)) => patterns.push(sema.resolve_type(file, &ty)),
                            (Some(false), _) => {}
                            _ => certain = false,
                        }
                    }
                    ast::Pattern::UnnamedPattern(_) => certain = false,
                }
            }
            for expr in label.exprs() {
                if let Some((name, _)) = referenced_name(expr.syntax()) {
                    constants.push(name.text().to_string());
                } else if let ast::Expr::Literal(literal) = &expr {
                    let text = literal.syntax().text().to_string();
                    if text == "true" || text == "false" {
                        constants.push(text);
                    }
                }
            }
        }
        // 普通的 switch 语句不要求穷尽
        if has_default || !certain || (!is_expr && !has_pattern) {
            continue;
        }
        if patterns.iter().any(Ty::contains_unknown) {
            continue;
        }

        let exhaustive = match scrutinee_ty {
            Ty::Class(class, _) => {
                let boxed_or_string = scrutinee_ty.unboxed(db).is_some()
                    || class_data(db, *class).is_some_and(|d| d.binary_name == "java/lang/String");
                if boxed_or_string && !has_pattern {
                    false
                } else {
                    // 包装类也可以被基本类型的模式覆盖：`Integer` 对 `case int i`
                    patterns.iter().any(|p| {
                        matches!(p, Ty::Primitive(_))
                            && is_unconditional(db, scrutinee_ty, p) == Some(true)
                    }) || is_covered(db, *class, &patterns, &constants, 0)
                }
            }
            // 基本类型：无条件的模式（精确转换或装箱），或者 boolean 的 `true` 和 `false` 都有
            Ty::Primitive(p) => {
                patterns
                    .iter()
                    .any(|pattern| is_unconditional(db, scrutinee_ty, pattern) == Some(true))
                    || (*p == PrimitiveType::Boolean
                        && ["true", "false"]
                            .iter()
                            .all(|b| constants.iter().any(|c| c == b)))
            }
            _ => continue,
        };
        if exhaustive {
            continue;
        }
        let range = switch
            .first_token()
            .map_or_else(|| switch.text_range(), |t| t.text_range());
        let what = if is_expr {
            base_db::trs!("switch 表达式", "switch expression")
        } else {
            base_db::trs!("switch 语句", "switch statement")
        };
        out.push(error(
            range,
            DiagnosticCode::NonExhaustiveSwitch,
            base_db::tr!(
                "{what}没有覆盖所有可能的输入值",
                "The {what} does not cover all possible input values"
            ),
        ));
    }
}

/// switch 标签的支配关系：前面不带守卫的类型模式能匹配的值，后面的标签就永远轮不到；
/// 无条件的模式和 `default` 不能同时出现。
fn switch_dominance(
    db: &dyn Db,
    sema: &Semantics<'_>,
    file: SourceFile,
    node: &SyntaxNode,
    result: &InferenceResult,
    out: &mut Vec<Diagnostic>,
) {
    let dominated = |range: TextRange| {
        error(
            range,
            DiagnosticCode::DominatedCase,
            base_db::trs!(
                "这个 case 标签被前面的 case 标签支配，永远不会匹配到",
                "This case label is dominated by a preceding case label"
            )
            .to_string(),
        )
    };
    for switch in node.descendants() {
        if !matches!(
            switch.kind(),
            SyntaxKind::SWITCH_EXPR | SyntaxKind::SWITCH_STMT
        ) {
            continue;
        }
        let Some(scrutinee) = switch.children().find_map(ast::Expr::cast) else {
            continue;
        };
        let Some(block) = switch.children().find_map(ast::SwitchBlock::cast) else {
            continue;
        };
        let Some(scrutinee_ty) = result.type_of(scrutinee.syntax().text_range()) else {
            continue;
        };
        if scrutinee_ty.contains_unknown() {
            continue;
        }
        // 前面不带守卫的类型模式的类型
        let mut dominators: Vec<Ty> = Vec::new();
        let mut default_label: Option<TextRange> = None;
        let mut unconditional: Option<TextRange> = None;
        for case in block.cases() {
            let Some(label) = case.label() else {
                break;
            };
            let guarded = label.guard().is_some();
            if label
                .syntax()
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::DEFAULT_KW)
            {
                let range = label.syntax().text_range();
                if unconditional.is_some() {
                    out.push(error(
                        range,
                        DiagnosticCode::DominatedCase,
                        base_db::trs!(
                            "switch 里同时有无条件的模式和 `default` 标签",
                            "The switch has both an unconditional pattern and a `default` label"
                        )
                        .to_string(),
                    ));
                }
                default_label.get_or_insert(range);
            }
            for pattern in label.patterns() {
                let (ty, type_pattern) = match &pattern {
                    ast::Pattern::TypePattern(it) => match it.ty() {
                        Some(t) if t.syntax().text() != "var" => {
                            (sema.resolve_type(file, &t), true)
                        }
                        _ => continue,
                    },
                    ast::Pattern::RecordPattern(it) => match it.ty() {
                        Some(t) => (sema.resolve_type(file, &t), false),
                        None => continue,
                    },
                    ast::Pattern::UnnamedPattern(_) => continue,
                };
                if ty.contains_unknown() {
                    continue;
                }
                let range = pattern.syntax().text_range();
                if dominators
                    .iter()
                    .any(|d| is_unconditional(db, &ty, d) == Some(true))
                {
                    out.push(dominated(range));
                    continue;
                }
                if type_pattern && !guarded {
                    if is_unconditional(db, scrutinee_ty, &ty) == Some(true) {
                        if default_label.is_some() || unconditional.is_some() {
                            out.push(error(
                                range,
                                DiagnosticCode::DominatedCase,
                                base_db::trs!(
                                    "switch 里同时有无条件的模式和 `default` 标签",
                                    "The switch has both an unconditional pattern and a `default` label"
                                )
                                .to_string(),
                            ));
                        }
                        unconditional.get_or_insert(range);
                    }
                    dominators.push(ty);
                }
            }
            for expr in label.exprs() {
                if expr.syntax().text() == "null" {
                    continue;
                }
                // 枚举常量的简单名不一定有类型记录，它的类型就是选择器的类型
                let ty = match result.type_of(expr.syntax().text_range()) {
                    Some(ty) => ty.clone(),
                    None if expr.syntax().kind() == SyntaxKind::NAME_EXPR => scrutinee_ty.clone(),
                    None => continue,
                };
                if ty.contains_unknown() {
                    continue;
                }
                if dominators
                    .iter()
                    .any(|d| is_unconditional(db, &ty, d) == Some(true))
                {
                    out.push(dominated(expr.syntax().text_range()));
                }
            }
        }
    }
}

/// 涉及基本类型的模式（`instanceof` 和 switch 的标签）：值的类型和模式的类型必须可以互相转换。
fn primitive_pattern_applicability(
    db: &dyn Db,
    sema: &Semantics<'_>,
    file: SourceFile,
    node: &SyntaxNode,
    result: &InferenceResult,
    out: &mut Vec<Diagnostic>,
) {
    let mut check = |operand: &Ty, written: &ast::Type, range: TextRange| {
        if written.syntax().text() == "var" || operand.contains_unknown() {
            return;
        }
        let ty = sema.resolve_type(file, written);
        if ty.contains_unknown() {
            return;
        }
        if primitive_pattern_applicable(db, operand, &ty) == Some(false) {
            out.push(error(
                range,
                DiagnosticCode::TypeMismatch,
                base_db::tr!(
                    "`{}` 类型的值不可能匹配 `{}`",
                    "A value of type `{}` can never match `{}`",
                    operand.display(db),
                    ty.display(db)
                ),
            ));
        }
    };
    for n in node.descendants() {
        match n.kind() {
            SyntaxKind::INSTANCEOF_EXPR => {
                let Some(operand) = n
                    .children()
                    .find_map(ast::Expr::cast)
                    .and_then(|e| result.type_of(e.syntax().text_range()))
                else {
                    continue;
                };
                for child in n.children() {
                    if let Some(written) = ast::Type::cast(child.clone()) {
                        check(operand, &written, child.text_range());
                    } else if let Some(ast::Pattern::TypePattern(p)) = ast::Pattern::cast(child) {
                        if let Some(written) = p.ty() {
                            check(operand, &written, p.syntax().text_range());
                        }
                    }
                }
            }
            SyntaxKind::SWITCH_EXPR | SyntaxKind::SWITCH_STMT => {
                let Some(operand) = n
                    .children()
                    .find_map(ast::Expr::cast)
                    .and_then(|e| result.type_of(e.syntax().text_range()))
                else {
                    continue;
                };
                let Some(block) = n.children().find_map(ast::SwitchBlock::cast) else {
                    continue;
                };
                for label in block.cases().filter_map(|c| c.label()) {
                    for pattern in label.patterns() {
                        if let ast::Pattern::TypePattern(p) = pattern {
                            if let Some(written) = p.ty() {
                                check(operand, &written, p.syntax().text_range());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// `sub`（`None` 表示匿名类或局部类）直接继承 `sup` 时，如果 `sup` 是 sealed 的而 `sub`
/// 不在它允许的子类型里，返回诊断文本。允许的子类型解析不全时不下结论。
fn sealed_violation(db: &dyn Db, sup: ClassId, sub: Option<ClassId>) -> Option<String> {
    let data = class_data(db, sup)?;
    if !data.flags.contains(Modifiers::SEALED) {
        return None;
    }
    let permitted = hir::permitted_subclasses(db, sup)?;
    if sub.is_some_and(|sub| permitted.contains(&sub)) {
        return None;
    }
    let what = if data.is_interface() {
        base_db::trs!("接口", "interface")
    } else {
        base_db::trs!("类", "class")
    };
    Some(match sub.and_then(|sub| class_data(db, sub)) {
        Some(sub) => base_db::tr!(
            "`{}` 不在 sealed {what} `{}` 允许的子类型里",
            "`{}` is not allowed to extend the sealed {what} `{}`",
            sub.name,
            data.name
        ),
        None => base_db::tr!(
            "匿名类和局部类不能继承 sealed {what} `{}`",
            "Anonymous and local classes cannot extend the sealed {what} `{}`",
            data.name
        ),
    })
}

fn mentions_type_param(ty: &Ty) -> bool {
    match ty {
        Ty::TypeParam(_) => true,
        Ty::Class(_, args) | Ty::Intersection(args) => args.iter().any(mentions_type_param),
        Ty::Array(element) => mentions_type_param(element),
        Ty::Wildcard(_) => true,
        _ => false,
    }
}

/// `Box<String>`：写明的类型实参是否在类型参数的上界之内。
///
/// 只看实参和（代换后的）上界都是已知的具体类型的情形；通配符、类型变量、继承链解析不全的跳过。
fn type_arg_bounds(
    db: &dyn Db,
    sema: &Semantics<'_>,
    file: SourceFile,
    class_ty: &ast::ClassType,
    out: &mut Vec<Diagnostic>,
) {
    let Some(arg_nodes) = class_ty
        .type_arg_list()
        .map(|l| l.args().collect::<Vec<_>>())
    else {
        return;
    };
    if arg_nodes.is_empty() {
        return;
    }
    let Ty::Class(class, args) = sema.resolve_type(file, &ast::Type::ClassType(class_ty.clone()))
    else {
        return;
    };
    if args.len() != arg_nodes.len() {
        return;
    }
    let subst = hir::Subst::for_class(db, class, &args);
    for (idx, (arg, node)) in args.iter().zip(&arg_nodes).enumerate() {
        let clear = matches!(arg, Ty::Class(..) | Ty::Array(_))
            && !arg.contains_unknown()
            && !mentions_type_param(arg);
        if !clear {
            continue;
        }
        if let Ty::Class(arg_class, _) = arg {
            if !hierarchy_is_resolved(db, *arg_class) {
                continue;
            }
        }
        let param = hir::TypeParamId {
            owner: hir::GenericOwner::Class(class),
            idx: idx as u32,
        };
        for bound in hir::type_param_bounds(db, param).iter() {
            let bound = bound.subst(&subst);
            if bound.contains_unknown() || mentions_type_param(&bound) {
                continue;
            }
            if !is_subtype(db, arg, &bound) {
                out.push(error(
                    node.syntax().text_range(),
                    DiagnosticCode::TypeArgOutOfBounds,
                    base_db::tr!(
                        "类型实参 `{}` 不在类型变量 `{}` 的上界 `{}` 之内",
                        "Type argument `{}` is not within the bounds of type variable `{}` (`{}`)",
                        arg.display(db),
                        hir::type_param_name(db, param),
                        bound.display(db)
                    ),
                ));
                break;
            }
        }
    }
}

pub(crate) fn typeck_diagnostics(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();

    return_values(&root, out);

    for node in root.descendants() {
        if let Some(class_ty) = ast::ClassType::cast(node.clone()) {
            type_arg_bounds(db, &sema, file, &class_ty, out);
            continue;
        }
        if let Some(new_expr) = ast::NewExpr::cast(node.clone()) {
            // 匿名类不能继承 sealed 类型
            if new_expr.body().is_some() {
                if let Some(ty) = new_expr.ty() {
                    if let Ty::Class(sup, _) = sema.resolve_type(file, &ty) {
                        if let Some(message) = sealed_violation(db, sup, None) {
                            out.push(error(
                                ty.syntax().text_range(),
                                DiagnosticCode::SealedSubtype,
                                message,
                            ));
                        }
                    }
                }
            }
            continue;
        }
        if let Some(decl) = ast::TypeDecl::cast(node.clone()) {
            // 局部类不在全局索引里，这里拿到的会是外层的类
            let is_local = node
                .ancestors()
                .skip(1)
                .any(|a| a.kind() == SyntaxKind::BLOCK);
            if is_local {
                // 局部类不可能是 sealed 类型允许的子类型
                let supertypes = node
                    .children()
                    .filter(|n| {
                        matches!(
                            n.kind(),
                            SyntaxKind::EXTENDS_CLAUSE | SyntaxKind::IMPLEMENTS_CLAUSE
                        )
                    })
                    .flat_map(|clause| clause.children().filter_map(ast::Type::cast));
                for ty in supertypes {
                    if let Ty::Class(sup, _) = sema.resolve_type(file, &ty) {
                        if let Some(message) = sealed_violation(db, sup, None) {
                            out.push(error(
                                ty.syntax().text_range(),
                                DiagnosticCode::SealedSubtype,
                                message,
                            ));
                        }
                    }
                }
                continue;
            }
            let Some(class) = sema.class_of_decl(file, &decl) else {
                continue;
            };
            if let Some(name) = node.children().find_map(ast::Name::cast) {
                for sup in hir::direct_supers(db, class).iter() {
                    if let Some(message) = sealed_violation(db, *sup, Some(class)) {
                        out.push(error(
                            name.syntax().text_range(),
                            DiagnosticCode::SealedSubtype,
                            message,
                        ));
                    }
                }
            }
            let missing = unimplemented_abstract_methods(db, class);
            if let (Some(first), Some(name)) =
                (missing.first(), node.children().find_map(ast::Name::cast))
            {
                out.push(error(
                    name.syntax().text_range(),
                    DiagnosticCode::MissingImplementation,
                    base_db::tr!(
                        "`{}` 不是抽象类，却没有实现抽象方法 `{first}`{}",
                        "`{}` is not abstract and does not implement abstract method `{first}`{}",
                        name.text(),
                        if missing.len() > 1 {
                            base_db::tr!(" 等 {} 个方法", " and {} more", missing.len())
                        } else {
                            String::new()
                        }
                    ),
                ));
            }
        } else if let Some(method) = ast::MethodDecl::cast(node.clone()) {
            let in_local_class = node
                .ancestors()
                .skip(1)
                .any(|a| a.kind() == SyntaxKind::BLOCK || a.kind() == SyntaxKind::NEW_EXPR);
            if in_local_class {
                continue;
            }
            if let (Some(id), Some(name)) = (sema.method_of_decl(file, &node), method.name()) {
                override_checks(db, id, name.syntax().text_range(), out);
            }
        }
    }

    for body in bodies_in_file(db, file) {
        db.unwind_if_revision_cancelled();
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let result = infer(db, body);
        final_assignments(db, &node, &result, out);
        private_access(db, body, &node, &result, out);
        static_context(db, body, &node, &result, out);
        switch_exhaustiveness(db, &sema, file, &node, &result, out);
        switch_dominance(db, &sema, file, &node, &result, out);
        primitive_pattern_applicability(db, &sema, file, &node, &result, out);
    }
}
