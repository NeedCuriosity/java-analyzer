//! 未经检查的操作（`unchecked` 警告）：
//!
//! - 未经检查的转换：原始类型的值（或父类型链上经过原始类型的值）赋给参数化类型的变量、
//!   字段、返回值或形参（`List<String> xs = new ArrayList();`）；
//! - 未经检查的方法调用：调用时某个实参要经过未经检查的转换才适用（`Collections.sort(raw)`），
//!   报在方法名上，实参上另报未经检查的转换；
//! - 未经检查的调用：在原始类型的接收者上调用实例方法（或用原始类型 `new` 对象），而方法的
//!   形参类型在擦除之后会改变（`raw.add(x)`、`new ArrayList(coll)`）；
//! - 未经检查的强制转换：转换成类型变量（`(T) obj`）、元素是类型变量的数组（`(T[]) objs`），
//!   或者从 `Object`、原始类型、非泛型的父类型向下转换成参数化类型（`(List<String>) obj`）。
//!
//! 只报确定的：参与判断的类型都要"可靠"——来自声明（局部变量、形参、字段、方法的返回类型）、
//! 写出来的 `new` / 强制转换，或者经过可靠的接收者代换得到；`var`、菱形、需要推断的泛型方法
//! 调用、lambda 的形参、匿名类里不带限定符的成员访问一律不下结论。参数化类型的类型实参全是
//! 无界通配符时不算（`List<?>`）。

use base_db::{Db, SourceFile};
use std::cell::RefCell;

use hir::{
    class_data, field_ty, find_super_keeping_raw, method_sig, this_ty, ClassId, FieldId,
    GenericOwner, InferenceResult, LocalKind, MethodId, Resolution, Semantics, Subst, Ty,
    TypeParamId, Wildcard,
};
use rustc_hash::FxHashMap;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{hierarchy_is_resolved, view::View, Diagnostic, DiagnosticCode, Severity};

/// 递归求可靠类型的深度上限（长调用链）。
const MAX_DEPTH: u32 = 32;

fn warning(range: TextRange, message: String) -> Diagnostic {
    Diagnostic {
        range,
        message,
        severity: Severity::Warning,
        code: DiagnosticCode::Unchecked,
        unused: false,
    }
}

fn strip_parens(expr: ast::Expr) -> Option<ast::Expr> {
    match expr {
        ast::Expr::ParenExpr(paren) => strip_parens(paren.expr()?),
        other => Some(other),
    }
}

fn is_generic(db: &dyn Db, class: ClassId) -> bool {
    class_data(db, class).is_some_and(|d| !d.type_params.is_empty())
}

fn is_raw(db: &dyn Db, ty: &Ty) -> bool {
    matches!(ty, Ty::Class(class, args) if args.is_empty() && is_generic(db, *class))
}

/// 类型和它的擦除不同：提到了类型变量，或者有参数化类型。
fn differs_from_erasure(ty: &Ty) -> bool {
    match ty {
        Ty::TypeParam(_) => true,
        Ty::Class(_, args) => !args.is_empty(),
        Ty::Array(element) => differs_from_erasure(element),
        Ty::Intersection(_) | Ty::Wildcard(_) => true,
        _ => false,
    }
}

/// 写出来的类型实参全是无界通配符（`List<?>`、`Map<?, ? extends Object>`）：这样的参数化类型
/// 和原始类型之间的转换不是未经检查的（JLS §5.1.9）。拿不准的（含未知类型、`? extends X`
/// 而类型参数本身有上界）也算作无界，不报。
fn is_unbounded(db: &dyn Db, ty: &Ty) -> bool {
    let Ty::Class(class, args) = ty else {
        return true;
    };
    let Some(data) = class_data(db, *class) else {
        return true;
    };
    if args.is_empty() || args.len() != data.type_params.len() {
        return true;
    }
    args.iter().enumerate().all(|(idx, arg)| match arg {
        Ty::Wildcard(Wildcard::Unbounded) | Ty::Unknown => true,
        Ty::Wildcard(Wildcard::Super(_)) => false,
        Ty::Wildcard(Wildcard::Extends(bound)) => {
            let is_object = |ty: &Ty| {
                ty.as_class()
                    .is_some_and(|c| c.binary_name(db) == "java/lang/Object")
            };
            // class 文件里没写上界的类型参数也记着 `Object`
            let declared_bound = hir::type_param_bounds(
                db,
                TypeParamId {
                    owner: GenericOwner::Class(*class),
                    idx: idx as u32,
                },
            )
            .iter()
            .any(|b| !is_object(b));
            bound.contains_unknown() || declared_bound || is_object(bound)
        }
        other => other.contains_unknown() && !matches!(other, Ty::Class(..)),
    })
}

/// 可具体化的类型（JLS §4.7）：运行时能完整检查的类型。拿不准的按可具体化处理。
fn reifiable(db: &dyn Db, ty: &Ty) -> bool {
    match ty {
        Ty::TypeParam(_) | Ty::Intersection(_) => false,
        Ty::Class(..) => is_unbounded(db, ty),
        Ty::Array(element) => reifiable(db, element),
        _ => true,
    }
}

/// 参数化的目标类型：泛型类带着（不全是无界通配符的）类型实参。非静态内部类不看
/// （外层类的类型实参也算在内，这里不建模）。
fn parameterized_target(db: &dyn Db, ty: &Ty) -> Option<ClassId> {
    let Ty::Class(class, args) = ty else {
        return None;
    };
    if args.is_empty() || is_inner(db, *class) || is_unbounded(db, ty) {
        return None;
    }
    Some(*class)
}

/// 非静态的内部类（外层类的类型参数可能参与它的类型）。
fn is_inner(db: &dyn Db, class: ClassId) -> bool {
    let Some(data) = class_data(db, class) else {
        return true;
    };
    data.outer.is_some()
        && !data.flags.is_static()
        && !data.is_interface()
        && !matches!(data.kind, hir::ClassKind::Enum | hir::ClassKind::Record)
}

fn method_is_static(db: &dyn Db, method: MethodId) -> Option<bool> {
    class_data(db, method.class)?
        .methods
        .get(method.idx as usize)
        .map(|m| m.flags.is_static())
}

fn field_is_static(db: &dyn Db, field: FieldId) -> Option<bool> {
    class_data(db, field.class)?
        .fields
        .get(field.idx as usize)
        .map(|f| f.flags.is_static())
}

/// `Foo(int, List<E>)` / `add(E)` 这样的签名文本。
fn signature(db: &dyn Db, method: MethodId) -> String {
    let Some(data) = class_data(db, method.class) else {
        return String::new();
    };
    let Some(m) = data.methods.get(method.idx as usize) else {
        return String::new();
    };
    let name = if m.is_constructor {
        data.name.to_string()
    } else {
        m.name.to_string()
    };
    let params: Vec<String> = method_sig(db, method)
        .params
        .iter()
        .map(|p| p.display(db))
        .collect();
    format!("{name}({})", params.join(", "))
}

struct Checker<'a> {
    db: &'a dyn Db,
    sema: &'a Semantics<'a>,
    file: SourceFile,
    root: SyntaxNode,
    result: &'a InferenceResult,
    /// 表达式范围 → 可靠类型
    reliable: RefCell<FxHashMap<TextRange, Option<Ty>>>,
}

/// 接收者类型看作 `owner` 时的样子：`Some(原始类型)` 表示经过了原始类型。
struct MemberView {
    ty: Ty,
}

impl MemberView {
    fn is_raw(&self, db: &dyn Db) -> bool {
        is_raw(db, &self.ty)
    }

    /// 声明在 `owner` 里、类型为 `declared` 的实例成员在这个接收者上的类型。
    fn member_ty(&self, db: &dyn Db, owner: ClassId, declared: &Ty) -> Option<Ty> {
        if self.is_raw(db) {
            return Some(declared.erasure(db));
        }
        let own = GenericOwner::Class(owner);
        if declared.mentions_params_where(&|p| p.owner != own) {
            return None;
        }
        if !declared.mentions_params_of(own) {
            return Some(declared.clone());
        }
        let Ty::Class(_, args) = &self.ty else {
            return None;
        };
        if args.is_empty() || args.iter().any(|a| a.is_wildcard() || a.contains_unknown()) {
            return None;
        }
        Some(declared.subst(&Subst::for_class(db, owner, args)))
    }
}

impl Checker<'_> {
    fn view(&self, site: &Ty, owner: ClassId) -> Option<MemberView> {
        if let Ty::Class(class, _) = site {
            if !hierarchy_is_resolved(self.db, *class) {
                return None;
            }
        }
        if site.contains_unknown() {
            return None;
        }
        let ty = find_super_keeping_raw(self.db, site, owner)?;
        Some(MemberView { ty })
    }

    /// `node` 所在的最内层类体属于具名类（含局部类）时返回这个类；匿名类返回 `None`。
    fn innermost_named_class(&self, node: &SyntaxNode) -> Option<ClassId> {
        let body = node
            .ancestors()
            .find(|a| a.kind() == SyntaxKind::CLASS_BODY)?;
        self.named_class_of_body(&body)
    }

    fn named_class_of_body(&self, body: &SyntaxNode) -> Option<ClassId> {
        let decl = ast::TypeDecl::cast(body.parent()?)?;
        self.sema
            .class_of_decl(self.file, &decl)
            .or_else(|| self.sema.local_class_of_decl(self.file, &decl))
    }

    /// 不带限定符访问 `owner` 的实例成员时的接收者：由内往外第一个继承了 `owner` 的具名类。
    /// 中途经过匿名类时拿不准。
    fn implicit_view(&self, node: &SyntaxNode, owner: ClassId) -> Option<MemberView> {
        for body in node
            .ancestors()
            .filter(|a| a.kind() == SyntaxKind::CLASS_BODY)
        {
            let class = self.named_class_of_body(&body)?;
            if !hierarchy_is_resolved(self.db, class) {
                return None;
            }
            if let Some(ty) = find_super_keeping_raw(self.db, &this_ty(self.db, class), owner) {
                return Some(MemberView { ty });
            }
        }
        None
    }

    /// 接收者是 `receiver`（`None` 表示不带限定符）时，访问 `owner` 的实例成员看到的样子。
    fn member_view(
        &self,
        receiver: Option<ast::Expr>,
        at: &SyntaxNode,
        owner: ClassId,
        depth: u32,
    ) -> Option<MemberView> {
        match receiver {
            None => self.implicit_view(at, owner),
            Some(receiver) => {
                let site = self.reliable_type(&receiver, depth + 1)?;
                self.view(&site, owner)
            }
        }
    }

    /// 局部变量的类型来自写出来的声明（不是 `var`、不是没写类型的 lambda 形参）。
    fn local_declared_ty(&self, id: hir::LocalId) -> Option<Ty> {
        let local = self.result.local(id);
        match local.kind {
            LocalKind::Param
            | LocalKind::Local
            | LocalKind::CatchParam
            | LocalKind::Pattern
            | LocalKind::Resource
            | LocalKind::ForeachVar => {}
            _ => return None,
        }
        let start = match self.root.covering_element(local.decl_range) {
            syntax::NodeOrToken::Node(node) => node,
            syntax::NodeOrToken::Token(token) => token.parent()?,
        };
        // 名字往上几层之内的声明节点上写着类型
        let ty = start
            .ancestors()
            .take(4)
            .find_map(|n| n.children().find_map(ast::Type::cast))?;
        if ty.syntax().text() == "var" {
            return None;
        }
        (!local.ty.contains_unknown()).then(|| local.ty.clone())
    }

    fn field_access_ty(
        &self,
        field: FieldId,
        receiver: Option<ast::Expr>,
        at: &SyntaxNode,
        depth: u32,
    ) -> Option<Ty> {
        let declared = field_ty(self.db, field);
        if declared.contains_unknown() {
            return None;
        }
        if field_is_static(self.db, field)? {
            return Some(declared);
        }
        // `Foo.this.x` 之类拿不准
        if matches!(&receiver, Some(ast::Expr::ThisExpr(this)) if this.qualifier().is_some()) {
            return None;
        }
        self.member_view(receiver, at, field.class, depth)?
            .member_ty(self.db, field.class, &declared)
    }

    /// 表达式的类型，只在能确定和 javac 一致时给出（见模块注释）。
    fn reliable_type(&self, expr: &ast::Expr, depth: u32) -> Option<Ty> {
        let range = expr.syntax().text_range();
        if let Some(known) = self.reliable.borrow().get(&range) {
            return known.clone();
        }
        if depth > MAX_DEPTH {
            return None;
        }
        let ty = self.compute_reliable_type(expr, depth);
        self.reliable.borrow_mut().insert(range, ty.clone());
        ty
    }

    fn compute_reliable_type(&self, expr: &ast::Expr, depth: u32) -> Option<Ty> {
        let ty = match strip_parens(expr.clone())? {
            ast::Expr::NameExpr(name) => {
                let name_ref = name.name_ref()?;
                match self.result.resolution(name_ref.syntax().text_range())? {
                    Resolution::Local(id) => self.local_declared_ty(*id)?,
                    Resolution::Field(field) => {
                        self.field_access_ty(*field, None, name.syntax(), depth)?
                    }
                    _ => return None,
                }
            }
            ast::Expr::FieldAccess(access) => {
                let name_ref = access.name_ref()?;
                match self.result.resolution(name_ref.syntax().text_range())? {
                    Resolution::Field(field) => {
                        let receiver = access.receiver()?;
                        self.field_access_ty(*field, Some(receiver), access.syntax(), depth)?
                    }
                    _ => return None,
                }
            }
            ast::Expr::MethodCall(call) => self.call_result_ty(&call, depth)?,
            ast::Expr::NewExpr(new_expr) => {
                if new_expr.body().is_some() || new_expr.qualifier().is_some() {
                    return None;
                }
                let ast::Type::ClassType(class_ty) = new_expr.ty()? else {
                    return None;
                };
                // 菱形要靠推断
                if class_ty
                    .type_arg_list()
                    .is_some_and(|args| args.args().next().is_none())
                {
                    return None;
                }
                let ty = self
                    .sema
                    .resolve_type(self.file, &ast::Type::ClassType(class_ty));
                match &ty {
                    Ty::Class(class, _) if !is_inner(self.db, *class) => ty,
                    _ => return None,
                }
            }
            ast::Expr::CastExpr(cast) => match cast.ty()? {
                ast::Type::IntersectionType(_) => return None,
                ty => self.sema.resolve_type(self.file, &ty),
            },
            ast::Expr::ArrayCreationExpr(creation) => {
                self.result.type_of(creation.syntax().text_range())?.clone()
            }
            ast::Expr::ArrayAccess(access) => {
                match self.reliable_type(&access.array()?, depth + 1)? {
                    Ty::Array(element) => (*element).clone(),
                    _ => return None,
                }
            }
            ast::Expr::ThisExpr(this) => {
                if this.qualifier().is_some() {
                    return None;
                }
                this_ty(self.db, self.innermost_named_class(this.syntax())?)
            }
            ast::Expr::Literal(literal) => {
                if literal.kind() != Some(SyntaxKind::STRING_LITERAL) {
                    return None;
                }
                Ty::string(self.db)
            }
            _ => return None,
        };
        (!ty.contains_unknown() && ty.is_reference()).then_some(ty)
    }

    /// 方法调用的结果类型（JLS §15.12.2.6）：调用要经过未经检查的转换才适用时是返回类型的擦除；
    /// 原始类型上的实例方法也是擦除；否则是按接收者代换的声明的返回类型（不提到方法自己的
    /// 类型参数时）。
    fn call_result_ty(&self, call: &ast::MethodCall, depth: u32) -> Option<Ty> {
        let method = self.chosen(call.arg_list()?)?;
        let ret = method_sig(self.db, method).ret.clone();
        if ret.contains_unknown() || matches!(ret, Ty::Void) {
            return None;
        }
        if !self.unchecked_args(call.syntax(), depth).is_empty() {
            return Some(ret.erasure(self.db));
        }
        let declared = if method_is_static(self.db, method)? {
            ret
        } else {
            if matches!(call.receiver(), Some(ast::Expr::ThisExpr(this)) if this.qualifier().is_some())
                || matches!(call.receiver(), Some(ast::Expr::SuperExpr(_)))
            {
                return None;
            }
            let view = self.member_view(call.receiver(), call.syntax(), method.class, depth)?;
            if view.is_raw(self.db) {
                return Some(ret.erasure(self.db));
            }
            if ret.mentions_params_of(GenericOwner::Method(method)) {
                return None;
            }
            view.member_ty(self.db, method.class, &ret)?
        };
        if declared.mentions_params_of(GenericOwner::Method(method)) {
            return None;
        }
        Some(declared)
    }

    fn chosen(&self, args: ast::ArgList) -> Option<MethodId> {
        let method = self.result.call_info(args.syntax().text_range())?.chosen?;
        hierarchy_is_resolved(self.db, method.class).then_some(method)
    }

    /// 调用（方法调用或 `new`）里要经过未经检查的转换才能传给形参的实参：(实参, 形参类型)。
    fn unchecked_args(&self, call: &SyntaxNode, depth: u32) -> Vec<(ast::Expr, Ty)> {
        let mut res = Vec::new();
        let Some((method, args, site_raw)) = self.call_parts(call, depth) else {
            return res;
        };
        if site_raw {
            // 原始类型的成员：形参类型整个被擦除
            return res;
        }
        let sig = method_sig(self.db, method);
        let fixed = if sig.varargs {
            sig.params.len().saturating_sub(1)
        } else {
            sig.params.len()
        };
        let args: Vec<ast::Expr> = args.args().collect();
        if !sig.varargs && args.len() != sig.params.len() {
            return res;
        }
        for (arg, param) in args.into_iter().zip(sig.params.iter()).take(fixed) {
            if param.contains_unknown() {
                continue;
            }
            if self.unchecked_conversion(&arg, param, depth).is_some() {
                res.push((arg, param.clone()));
            }
        }
        res
    }

    /// 调用的方法、实参列表，以及接收者是不是原始类型（静态方法为 false）。
    fn call_parts(&self, call: &SyntaxNode, depth: u32) -> Option<(MethodId, ast::ArgList, bool)> {
        if let Some(call) = ast::MethodCall::cast(call.clone()) {
            let args = call.arg_list()?;
            let method = self.chosen(args.clone())?;
            if method_is_static(self.db, method)? {
                return Some((method, args, false));
            }
            if matches!(call.receiver(), Some(ast::Expr::ThisExpr(this)) if this.qualifier().is_some())
                || matches!(call.receiver(), Some(ast::Expr::SuperExpr(_)))
            {
                return None;
            }
            let view = self.member_view(call.receiver(), call.syntax(), method.class, depth)?;
            return Some((method, args, view.is_raw(self.db)));
        }
        let new_expr = ast::NewExpr::cast(call.clone())?;
        if new_expr.qualifier().is_some() {
            return None;
        }
        let args = new_expr.arg_list()?;
        let method = self.chosen(args.clone())?;
        let ast::Type::ClassType(class_ty) = new_expr.ty()? else {
            return None;
        };
        let raw = class_ty.type_arg_list().is_none() && is_generic(self.db, method.class);
        if is_inner(self.db, method.class) {
            return None;
        }
        Some((method, args, raw))
    }

    /// `expr` 传给类型为 `target` 的位置时是否是未经检查的转换；是的话返回原始的源类型。
    fn unchecked_conversion(&self, expr: &ast::Expr, target: &Ty, depth: u32) -> Option<Ty> {
        let target_class = parameterized_target(self.db, target)?;
        if !is_generic(self.db, target_class) || !hierarchy_is_resolved(self.db, target_class) {
            return None;
        }
        let source = self.reliable_type(expr, depth + 1)?;
        if let Ty::Class(class, _) = &source {
            if !hierarchy_is_resolved(self.db, *class) {
                return None;
            }
        }
        if !matches!(source, Ty::Class(..) | Ty::TypeParam(_)) {
            return None;
        }
        let view = find_super_keeping_raw(self.db, &source, target_class)?;
        is_raw(self.db, &view).then_some(source)
    }

    fn conversion_warning(&self, expr: &ast::Expr, target: &Ty, out: &mut Vec<Diagnostic>) {
        if let Some(source) = self.unchecked_conversion(expr, target, 0) {
            out.push(warning(
                expr.syntax().text_range(),
                base_db::tr!(
                    "未经检查的转换：`{}` 转换成 `{}`",
                    "Unchecked conversion from `{}` to `{}`",
                    source.display(self.db),
                    target.display(self.db)
                ),
            ));
        }
    }

    /// 赋值上下文里的目标类型：声明的局部变量 / 字段、`=` 左边的变量、方法的返回类型。
    fn assignment_checks(&self, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
        match node.kind() {
            SyntaxKind::VARIABLE_DECLARATOR => {
                let Some(declarator) = ast::VariableDeclarator::cast(node.clone()) else {
                    return;
                };
                let Some(init) = declarator.initializer() else {
                    return;
                };
                if declarator.extra_dims() > 0 {
                    return;
                }
                let Some(parent) = node.parent() else {
                    return;
                };
                let target = match parent.kind() {
                    SyntaxKind::LOCAL_VAR_DECL => {
                        let Some(name) = declarator.name() else {
                            return;
                        };
                        let Some(id) = self.result.local_at_decl(name.syntax().text_range()) else {
                            return;
                        };
                        match self.local_declared_ty(id) {
                            Some(ty) => ty,
                            None => return,
                        }
                    }
                    SyntaxKind::FIELD_DECL => match self.sema.field_of_decl(self.file, node) {
                        Some(field) => field_ty(self.db, field),
                        None => return,
                    },
                    _ => return,
                };
                self.conversion_warning(&init, &target, out);
            }
            SyntaxKind::ASSIGN_EXPR => {
                let Some(assign) = ast::AssignExpr::cast(node.clone()) else {
                    return;
                };
                if assign.op_token().map(|t| t.kind()) != Some(SyntaxKind::EQ) {
                    return;
                }
                let (Some(lhs), Some(rhs)) = (assign.lhs(), assign.rhs()) else {
                    return;
                };
                let Some(target) = self.reliable_type(&lhs, 0) else {
                    return;
                };
                self.conversion_warning(&rhs, &target, out);
            }
            SyntaxKind::RETURN_STMT => {
                let Some(value) = ast::ReturnStmt::cast(node.clone()).and_then(|r| r.expr()) else {
                    return;
                };
                let Some(method) = node
                    .ancestors()
                    .find(|a| {
                        matches!(
                            a.kind(),
                            SyntaxKind::METHOD_DECL
                                | SyntaxKind::LAMBDA_EXPR
                                | SyntaxKind::CLASS_BODY
                                | SyntaxKind::CONSTRUCTOR_DECL
                        )
                    })
                    .and_then(ast::MethodDecl::cast)
                else {
                    return;
                };
                let Some(ret) = method.return_type() else {
                    return;
                };
                if method.extra_dims() > 0 {
                    return;
                }
                let target = self.sema.resolve_type(self.file, &ret);
                if target.contains_unknown() {
                    return;
                }
                self.conversion_warning(&value, &target, out);
            }
            _ => {}
        }
    }

    /// 方法调用 / `new` 上的检查：实参的未经检查的转换、原始类型上的调用。
    fn call_checks(&self, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
        let anchor = match ast::Expr::cast(node.clone()) {
            Some(ast::Expr::MethodCall(call)) => call.name_ref().map(|n| n.syntax().text_range()),
            Some(ast::Expr::NewExpr(new_expr)) => new_expr.ty().map(|t| t.syntax().text_range()),
            _ => return,
        };
        let Some(anchor) = anchor else {
            return;
        };
        let Some((method, _, site_raw)) = self.call_parts(node, 0) else {
            return;
        };
        if site_raw {
            let sig = method_sig(self.db, method);
            if sig.params.iter().any(|p| p.contains_unknown()) {
                return;
            }
            if sig.params.iter().any(differs_from_erasure) {
                let owner = Ty::class(method.class).display(self.db);
                out.push(warning(
                    anchor,
                    base_db::tr!(
                        "未经检查的调用：`{}` 是原始类型 `{owner}` 的成员",
                        "Unchecked call to `{}` as a member of the raw type `{owner}`",
                        signature(self.db, method)
                    ),
                ));
            }
            return;
        }
        let unchecked = self.unchecked_args(node, 0);
        if unchecked.is_empty() {
            return;
        }
        let args: Vec<String> = unchecked
            .iter()
            .map(|(arg, _)| {
                self.reliable_type(arg, 0)
                    .map_or_else(|| "?".to_owned(), |ty| ty.display(self.db))
            })
            .collect();
        out.push(warning(
            anchor,
            base_db::tr!(
                "未经检查的方法调用：`{}` 用在了 ({}) 上",
                "Unchecked method invocation: `{}` is applied to ({})",
                signature(self.db, method),
                args.join(", ")
            ),
        ));
        for (arg, param) in unchecked {
            self.conversion_warning(&arg, &param, out);
        }
    }

    /// 强制转换：转换成类型变量、不可具体化的数组，或从没有类型实参信息的类型向下转换成参数化类型。
    fn cast_check(&self, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
        let Some(cast) = ast::CastExpr::cast(node.clone()) else {
            return;
        };
        let Some(ty_node) = cast.ty() else {
            return;
        };
        if matches!(ty_node, ast::Type::IntersectionType(_)) {
            return;
        }
        let target = self.sema.resolve_type(self.file, &ty_node);
        if target.contains_unknown() || !differs_from_erasure(&target) {
            return;
        }
        let Some(operand) = cast.expr() else {
            return;
        };
        let Some(source) = self.reliable_type(&operand, 0) else {
            return;
        };
        if self.unchecked_cast(&source, &target) {
            out.push(warning(
                ty_node.syntax().text_range(),
                base_db::tr!(
                    "未经检查的强制转换：`{}` 转换成 `{}`",
                    "Unchecked cast from `{}` to `{}`",
                    source.display(self.db),
                    target.display(self.db)
                ),
            ));
        }
    }

    fn unchecked_cast(&self, source: &Ty, target: &Ty) -> bool {
        match target {
            Ty::TypeParam(param) => source.is_reference() && !self.is_below(source, *param),
            Ty::Array(_) if reifiable(self.db, target) => false,
            Ty::Array(target_element) => match source {
                Ty::Array(source_element) => {
                    !source_element.is_primitive()
                        && self.unchecked_cast(source_element, target_element)
                }
                Ty::Class(class, _) => matches!(
                    &*class.binary_name(self.db),
                    "java/lang/Object" | "java/lang/Cloneable" | "java/io/Serializable"
                ),
                _ => false,
            },
            Ty::Class(target_class, _) => {
                if parameterized_target(self.db, target).is_none()
                    || !hierarchy_is_resolved(self.db, *target_class)
                {
                    return false;
                }
                let Ty::Class(source_class, _) = source else {
                    return false;
                };
                if !hierarchy_is_resolved(self.db, *source_class) {
                    return false;
                }
                if source_class.binary_name(self.db) == "java/lang/Object" {
                    return true;
                }
                if is_raw(self.db, source) {
                    return hir::is_subclass_of(self.db, *source_class, *target_class)
                        || hir::is_subclass_of(self.db, *target_class, *source_class);
                }
                // 非泛型的父类型没有类型实参可以传给目标类型
                !is_generic(self.db, *source_class)
                    && *source_class != *target_class
                    && hir::is_subclass_of(self.db, *target_class, *source_class)
            }
            _ => false,
        }
    }

    /// `ty` 是类型变量 `param` 的子类型（JLS §4.10.2：它自己、以它为上界的类型变量、null）。
    /// 上界解析不出来时也算作是，不报。
    fn is_below(&self, ty: &Ty, param: TypeParamId) -> bool {
        self.is_below_inner(ty, param, 0)
    }

    fn is_below_inner(&self, ty: &Ty, param: TypeParamId, depth: u32) -> bool {
        if depth > 8 {
            return true;
        }
        match ty {
            Ty::Null | Ty::Unknown | Ty::Wildcard(_) => true,
            // 同名的类型变量也算同一个：局部类 / 匿名类里两边的作用域可能对不上
            Ty::TypeParam(p)
                if *p == param
                    || hir::type_param_name(self.db, *p)
                        == hir::type_param_name(self.db, param) =>
            {
                true
            }
            Ty::TypeParam(p) => {
                let bounds = hir::type_param_bounds(self.db, *p);
                bounds.iter().any(|b| b.contains_unknown())
                    || bounds
                        .iter()
                        .any(|b| self.is_below_inner(b, param, depth + 1))
            }
            Ty::Intersection(parts) => parts
                .iter()
                .any(|p| self.is_below_inner(p, param, depth + 1)),
            _ => false,
        }
    }
}

pub(crate) fn unchecked_diagnostics(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let (db, file, sema) = (view.db, view.file, &view.sema);
    for body in &view.bodies {
        let result = body.result(view);
        let checker = Checker {
            db,
            sema,
            file,
            root: view.root.clone(),
            result: &result,
            reliable: RefCell::default(),
        };
        let mut walk = body.node.preorder();
        while let Some(event) = walk.next() {
            let syntax::WalkEvent::Enter(node) = event else {
                continue;
            };
            // 没有登记成类的匿名类（菱形、`outer.new Inner() {}`）：里面的类型参数作用域拿不准
            if node.kind() == SyntaxKind::CLASS_BODY
                && node.parent().is_some_and(|p| {
                    matches!(p.kind(), SyntaxKind::NEW_EXPR | SyntaxKind::ENUM_CONSTANT)
                })
                && sema.anonymous_class_of_body(file, &node).is_none()
            {
                walk.skip_subtree();
                continue;
            }
            match node.kind() {
                SyntaxKind::VARIABLE_DECLARATOR
                | SyntaxKind::ASSIGN_EXPR
                | SyntaxKind::RETURN_STMT => checker.assignment_checks(&node, out),
                SyntaxKind::METHOD_CALL | SyntaxKind::NEW_EXPR => checker.call_checks(&node, out),
                SyntaxKind::CAST_EXPR => checker.cast_check(&node, out),
                _ => {}
            }
        }
    }
}
