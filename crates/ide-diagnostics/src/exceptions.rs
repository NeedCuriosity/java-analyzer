//! 受检异常：
//!
//! - 未处理的异常：调用会抛出受检异常的方法 / 构造器、`throw`、try-with-resources 隐含的
//!   `close()`，既没有被 `try` 捕获，也没有写进所在方法的 `throws`（匿名类 / 局部类的方法
//!   看它自己的 `throws`，lambda 看目标函数式接口方法的 `throws`）；
//! - `catch` 了 try 块不可能抛出的受检异常；
//! - `throws` 里多余的声明（提示）。
//!
//! `throw e` 按 Java 7 的精确重抛处理：`e` 是没有被重新赋值的 catch 形参时，抛出的是 try 块
//! 能抛出、这个 catch 接得住、前面的 catch 又没接住的那些受检异常；方法形参和显式声明类型的
//! 局部变量按声明的类型。
//!
//! 只报能确定的情形，下面这些一律跳过：
//! - 目标函数式接口确定不了的 lambda（类型未知、重载里有多个个数相同的候选、
//!   接口方法的 `throws` 是类型变量）；
//! - 字段初值、初始化块里的代码（实例初始化块的异常由构造器的 `throws` 决定），
//!   以及匿名类 / 局部类的字段初值和初始化块；
//! - 抛出的类型是类型变量，或者任何一步解析不出来；
//! - 标了 Lombok `@SneakyThrows` 的方法。

use std::{cell::RefCell, sync::Arc};

use base_db::{Db, SourceFile};
use hir::{
    class_data, is_subclass_of, is_subtype, lookup_class, lookup_methods, method_sig, BodyId,
    InferenceResult, LocalId, LocalKind, MethodId, Resolution, Ty,
};
use rustc_hash::FxHashMap;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{
    flow::block_completes,
    hierarchy_is_resolved,
    view::{FileBody, View},
    Diagnostic, DiagnosticCode, Severity,
};

/// 精确重抛要算外层 try 块抛出的异常，嵌套的 `throw e` 会继续往外算；限制递归深度。
const MAX_DEPTH: u32 = 8;

/// 一段代码能抛出的受检异常。
struct Thrown {
    types: Vec<Ty>,
    /// 每个抛出点都解析清楚了；为 false 时 `types` 只是其中一部分
    complete: bool,
}

struct Checker<'a> {
    db: &'a dyn Db,
    view: &'a View<'a>,
    throwable: Ty,
    exception: Option<Ty>,
    runtime_exception: Option<Ty>,
    error: Option<Ty>,
    /// 同一个 catch / throws 里的类型对很多抛出点都要判断一遍，结果缓存起来
    covers: RefCell<FxHashMap<(SyntaxNode, Ty), Option<bool>>>,
    resolved: RefCell<FxHashMap<SyntaxNode, Ty>>,
    checked: RefCell<FxHashMap<Ty, Option<bool>>>,
    throws: RefCell<FxHashMap<MethodId, Option<Arc<[Ty]>>>>,
    /// finally 块能否正常结束
    finally_completes: RefCell<FxHashMap<SyntaxNode, Option<bool>>>,
}

impl<'a> Checker<'a> {
    fn new(view: &'a View<'a>) -> Option<Checker<'a>> {
        let db = view.db;
        let throwable = lookup_class(db, "java/lang/Throwable")?;
        Some(Checker {
            db,
            view,
            throwable: Ty::class(throwable),
            exception: lookup_class(db, "java/lang/Exception").map(Ty::class),
            runtime_exception: lookup_class(db, "java/lang/RuntimeException").map(Ty::class),
            error: lookup_class(db, "java/lang/Error").map(Ty::class),
            covers: RefCell::default(),
            resolved: RefCell::default(),
            checked: RefCell::default(),
            throws: RefCell::default(),
            finally_completes: RefCell::default(),
        })
    }

    /// 方法声明抛出的异常类型。大多数方法没有 `throws`，先看声明，免得算出整个签名。
    fn method_throws(&self, method: MethodId) -> Vec<Ty> {
        let declares_throws = |method: MethodId| {
            class_data(self.db, method.class)
                .and_then(|d| {
                    d.methods
                        .get(method.idx as usize)
                        .map(|m| !m.throws.is_empty())
                })
                .unwrap_or(false)
        };
        self.throws
            .borrow_mut()
            .entry(method)
            .or_insert_with(|| {
                declares_throws(method).then(|| method_sig(self.db, method).throws.clone().into())
            })
            .as_deref()
            .map(<[Ty]>::to_vec)
            .unwrap_or_default()
    }

    fn resolve_type(&self, ty: &ast::Type) -> Ty {
        if let Some(resolved) = self.resolved.borrow().get(ty.syntax()) {
            return resolved.clone();
        }
        let resolved = self.view.resolve_type(ty);
        self.resolved
            .borrow_mut()
            .insert(ty.syntax().clone(), resolved.clone());
        resolved
    }

    /// 是不是受检异常。类型未知、不是类类型，或者继承链没有完全解析时为 `None`。
    fn checkedness(&self, ty: &Ty) -> Option<bool> {
        let Ty::Class(class, _) = ty else {
            return None;
        };
        if let Some(checked) = self.checked.borrow().get(ty) {
            return *checked;
        }
        let checked =
            (!ty.contains_unknown() && hierarchy_is_resolved(self.db, *class)).then(|| {
                is_subtype(self.db, ty, &self.throwable)
                    && !self
                        .runtime_exception
                        .as_ref()
                        .is_some_and(|rt| is_subtype(self.db, ty, rt))
                    && !self
                        .error
                        .as_ref()
                        .is_some_and(|err| is_subtype(self.db, ty, err))
            });
        self.checked.borrow_mut().insert(ty.clone(), checked);
        checked
    }

    /// 语法里写的一个（可能是 `A | B` 的）异常类型能否接住 `thrown`。
    /// 有解析不出来的类型、又没有确定接得住的时候为 `None`。
    fn type_covers(&self, ty: &ast::Type, thrown: &Ty) -> Option<bool> {
        let key = (ty.syntax().clone(), thrown.clone());
        if let Some(covers) = self.covers.borrow().get(&key) {
            return *covers;
        }
        let mut covers = Some(false);
        for alt in union_alternatives(ty) {
            let resolved = self.resolve_type(&alt);
            if resolved.contains_unknown() {
                covers = None;
            } else if is_subtype(self.db, thrown, &resolved) {
                covers = Some(true);
                break;
            }
        }
        self.covers.borrow_mut().insert(key, covers);
        covers
    }

    fn finally_completes(&self, finally: &ast::FinallyClause) -> Option<bool> {
        let Some(block) = finally.body() else {
            return Some(true);
        };
        *self
            .finally_completes
            .borrow_mut()
            .entry(finally.syntax().clone())
            .or_insert_with(|| block_completes(&block))
    }

    /// `try_stmt` 能否处理从它的子节点 `inner` 里抛出的 `thrown`：被某个 catch 接住，或者
    /// finally 不能正常结束（异常被丢弃）。拿不准时为 `None`。
    fn try_catches(
        &self,
        try_stmt: &ast::TryStmt,
        inner: &SyntaxNode,
        thrown: &Ty,
    ) -> Option<bool> {
        // 只有 try 块和资源列表里的代码受 catch 保护
        let guarded = try_stmt.body().is_some_and(|b| b.syntax() == inner)
            || try_stmt
                .resource_list()
                .is_some_and(|r| r.syntax() == inner);
        let mut unsure = false;
        if guarded {
            for catch in try_stmt.catch_clauses() {
                match catch
                    .param()
                    .and_then(|p| p.ty())
                    .and_then(|ty| self.type_covers(&ty, thrown))
                {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unsure = true,
                }
            }
        }
        if guarded || inner.kind() == SyntaxKind::CATCH_CLAUSE {
            if let Some(finally) = try_stmt.finally_clause() {
                match self.finally_completes(&finally) {
                    Some(true) => {}
                    Some(false) => return Some(true),
                    None => unsure = true,
                }
            }
        }
        (!unsure).then_some(false)
    }

    /// 方法 / 构造器的 `throws` 能否接住 `thrown`。不是方法 / 构造器（字段初值、初始化块）
    /// 或者拿不准时为 `None`。
    fn declared_covers(&self, callable: &SyntaxNode, thrown: &Ty) -> Option<bool> {
        let throws = if let Some(method) = ast::MethodDecl::cast(callable.clone()) {
            method.throws_clause()
        } else {
            ast::ConstructorDecl::cast(callable.clone())?.throws_clause()
        };
        let mut unsure = false;
        for ty in throws.iter().flat_map(|clause| clause.types()) {
            match self.type_covers(&ty, thrown) {
                Some(true) => return Some(true),
                Some(false) => {}
                None => unsure = true,
            }
        }
        (!unsure).then_some(false)
    }

    /// 从 `site` 往外到 `callable`，`thrown` 有没有被处理。`None` 表示这里不做判断。
    fn is_handled(
        &self,
        site: &SyntaxNode,
        callable: &SyntaxNode,
        result: &InferenceResult,
        thrown: &Ty,
    ) -> Option<bool> {
        let mut inner = site.clone();
        for ancestor in site.ancestors().skip(1) {
            match ancestor.kind() {
                // 匿名类 / 局部类里的方法和构造器看它自己的 `throws`；
                // 它们的字段初值和初始化块不判断
                SyntaxKind::CLASS_BODY => {
                    return match inner.kind() {
                        SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL
                            if !has_sneaky_throws(&inner) =>
                        {
                            self.declared_covers(&inner, thrown)
                        }
                        _ => None,
                    };
                }
                // lambda 体里的异常由函数式接口方法的 `throws` 决定
                SyntaxKind::LAMBDA_EXPR => {
                    let declared = self.lambda_throws(&ancestor, result)?;
                    return Some(
                        declared
                            .iter()
                            .any(|declared| is_subtype(self.db, thrown, declared)),
                    );
                }
                SyntaxKind::TRY_STMT => {
                    let try_stmt = ast::TryStmt::cast(ancestor.clone())?;
                    if self.try_catches(&try_stmt, &inner, thrown)? {
                        return Some(true);
                    }
                }
                _ => {}
            }
            if ancestor == *callable {
                break;
            }
            inner = ancestor;
        }
        self.declared_covers(callable, thrown)
    }
}

/// `A | B` 拆成各个类型；不是并集时就是它自己。
fn union_alternatives(ty: &ast::Type) -> Vec<ast::Type> {
    match ty {
        ast::Type::UnionType(union) => union
            .syntax()
            .children()
            .filter_map(ast::Type::cast)
            .collect(),
        other => vec![other.clone()],
    }
}

fn strip_parens(expr: ast::Expr) -> Option<ast::Expr> {
    match expr {
        ast::Expr::ParenExpr(paren) => strip_parens(paren.expr()?),
        other => Some(other),
    }
}

impl Checker<'_> {
    /// lambda 的目标函数式接口方法声明的 `throws`。确定不了目标时返回 `None`。
    fn lambda_throws(&self, lambda: &SyntaxNode, result: &InferenceResult) -> Option<Vec<Ty>> {
        // 作为实参的 lambda：同名、实参个数相同的候选只有一个时，目标类型才是确定的
        if let Some(arg_list) = lambda.parent().filter(|p| p.kind() == SyntaxKind::ARG_LIST) {
            let n_args = arg_list
                .children()
                .filter(|c| ast::Expr::can_cast(c.kind()))
                .count();
            let info = result.call_info(arg_list.text_range())?;
            let same_arity = info
                .candidates
                .iter()
                .filter(|candidate| {
                    let sig = method_sig(self.db, candidate.id);
                    sig.params.len() == n_args || sig.varargs
                })
                .count();
            if same_arity != 1 {
                return None;
            }
        }
        let target = result.type_of(lambda.text_range())?;
        if target.contains_unknown() {
            return None;
        }
        let method = hir::functional_method(self.db, target)?;
        let sig = method_sig(self.db, method.id);
        if sig
            .throws
            .iter()
            .any(|ty| !matches!(ty, Ty::Class(..)) || ty.contains_unknown())
        {
            return None;
        }
        Some(sig.throws.clone())
    }

    /// try-with-resources 的资源在退出时隐含调用的 `close()` 声明的异常。
    /// 资源的类型或 `close()` 确定不了时为 `None`。
    fn close_throws(&self, resource: &ast::Resource, result: &InferenceResult) -> Option<Vec<Ty>> {
        let ty = match resource.declarator().and_then(|d| d.name()) {
            Some(name) => result
                .local_at_decl(name.syntax().text_range())
                .map(|id| result.local(id).ty.clone()),
            None => resource
                .expr()
                .and_then(|e| result.type_of(e.syntax().text_range()).cloned()),
        };
        let ty @ Ty::Class(..) = ty? else {
            return None;
        };
        if ty.contains_unknown() {
            return None;
        }
        let candidates: Vec<_> = lookup_methods(self.db, &ty, "close")
            .into_iter()
            .filter(|candidate| method_sig(self.db, candidate.id).params.is_empty())
            .collect();
        // 取最派生的那个声明（子类覆写时可以收窄 `throws`）
        let most_derived = candidates.iter().find(|candidate| {
            candidates.iter().all(|other| {
                other.id.class == candidate.id.class
                    || !is_subclass_of(self.db, other.id.class, candidate.id.class)
            })
        })?;
        Some(method_sig(self.db, most_derived.id).throws.clone())
    }

    /// 在匿名类 / 局部类里不带限定符（或用 `this.`）调用的方法，解析结果不可信：类体自己
    /// 声明了同名方法（解析到的可能是父类型里的版本），或者类的继承链没有完全解析（真正
    /// 调用的可能是没解析出来的父类型里的方法，而不是外层类的同名方法）。
    fn unreliable_local_call(&self, call: &ast::MethodCall) -> bool {
        let unqualified = match call.receiver() {
            None => true,
            Some(ast::Expr::ThisExpr(this)) => this.qualifier().is_none(),
            Some(_) => false,
        };
        if !unqualified {
            return false;
        }
        let Some(name) = call.name_ref().map(|n| n.text()) else {
            return true;
        };
        for body in call.syntax().ancestors().filter_map(ast::ClassBody::cast) {
            let Some(owner) = body.syntax().parent() else {
                return true;
            };
            // 遇到具名的成员类就到头了：那里的方法解析是准确的
            let supertypes_resolved = match owner.kind() {
                SyntaxKind::NEW_EXPR => ast::NewExpr::cast(owner.clone())
                    .and_then(|n| n.ty())
                    .is_some_and(|ty| match self.resolve_type(&ty) {
                        Ty::Class(class, _) => hierarchy_is_resolved(self.db, class),
                        _ => false,
                    }),
                SyntaxKind::ENUM_CONSTANT => true,
                _ => {
                    let local = owner.parent().is_some_and(|p| {
                        !matches!(
                            p.kind(),
                            SyntaxKind::CLASS_BODY | SyntaxKind::COMPILATION_UNIT
                        )
                    });
                    if !local {
                        return false;
                    }
                    ast::TypeDecl::cast(owner.clone())
                        .and_then(|decl| self.view.sema.local_class_of_decl(self.view.file, &decl))
                        .is_some_and(|class| hierarchy_is_resolved(self.db, class))
                }
            };
            let declares = body.members().any(|m| match m {
                ast::Member::MethodDecl(method) => method.name().is_some_and(|n| n.text() == name),
                _ => false,
            });
            if declares || !supertypes_resolved {
                return true;
            }
        }
        false
    }

    /// `node` 这一个节点（不含子节点）抛出的受检异常：`(诊断标出的范围, 类型)`。
    /// 第二个值为 false 表示有拿不准的抛出（调用解析不出来、类型未知……），列出的只是一部分。
    fn node_throws(
        &self,
        node: &SyntaxNode,
        result: &InferenceResult,
        depth: u32,
    ) -> (Vec<(TextRange, Ty)>, bool) {
        let mut out = Vec::new();
        let mut complete = true;
        let add = |out: &mut Vec<(TextRange, Ty)>, complete: &mut bool, range, ty: Ty| match self
            .checkedness(&ty)
        {
            Some(true) => out.push((range, ty)),
            Some(false) => {}
            None => *complete = false,
        };
        match node.kind() {
            SyntaxKind::METHOD_CALL
            | SyntaxKind::NEW_EXPR
            | SyntaxKind::EXPLICIT_CONSTRUCTOR_CALL => {
                let Some(arg_list) = node.children().find_map(ast::ArgList::cast) else {
                    // `new int[n]` 这类数组创建没有实参列表，不抛异常
                    return (out, node.kind() == SyntaxKind::NEW_EXPR);
                };
                let Some(method) = result
                    .call_info(arg_list.syntax().text_range())
                    .and_then(|info| info.chosen)
                else {
                    return (out, false);
                };
                if !hierarchy_is_resolved(self.db, method.class) {
                    complete = false;
                }
                let range = match node.kind() {
                    SyntaxKind::METHOD_CALL => ast::MethodCall::cast(node.clone())
                        .and_then(|c| c.name_ref())
                        .map(|n| n.syntax().text_range()),
                    SyntaxKind::NEW_EXPR => ast::NewExpr::cast(node.clone())
                        .and_then(|n| n.ty())
                        .map(|t| t.syntax().text_range()),
                    _ => None,
                }
                .unwrap_or_else(|| node.text_range());
                if let Some(call) = ast::MethodCall::cast(node.clone()) {
                    let receiver_ty = call
                        .receiver()
                        .and_then(|r| result.type_of(r.syntax().text_range()));
                    // 数组的 `clone()` 不抛 `CloneNotSupportedException`（JLS §10.7）
                    if call.name_ref().is_some_and(|n| n.text() == "clone")
                        && matches!(receiver_ty, Some(Ty::Array(_)))
                    {
                        return (out, true);
                    }
                    // 接收者的继承链不全时，真正调用的可能是没解析出来的父类型里的重载
                    if let Some(Ty::Class(class, _)) = receiver_ty {
                        if !hierarchy_is_resolved(self.db, *class) {
                            complete = false;
                        }
                    }
                    if self.unreliable_local_call(&call) {
                        return (out, false);
                    }
                }
                // 匿名类的实例初始化块、字段初值抛出的异常也算在 `new` 上，这里不建模
                if let Some(body) = ast::NewExpr::cast(node.clone()).and_then(|n| n.body()) {
                    let initializes = body.members().any(|m| match m {
                        ast::Member::InitializerBlock(_) => true,
                        ast::Member::FieldDecl(field) => {
                            field.declarators().any(|d| d.initializer().is_some())
                        }
                        _ => false,
                    });
                    if initializes {
                        complete = false;
                    }
                }
                for ty in self.method_throws(method) {
                    add(&mut out, &mut complete, range, ty);
                }
            }
            SyntaxKind::RESOURCE => {
                let Some(resource) = ast::Resource::cast(node.clone()) else {
                    return (out, false);
                };
                match self.close_throws(&resource, result) {
                    Some(types) => {
                        for ty in types {
                            add(&mut out, &mut complete, node.text_range(), ty);
                        }
                    }
                    None => complete = false,
                }
            }
            SyntaxKind::THROW_STMT => {
                let Some(expr) = ast::ThrowStmt::cast(node.clone())
                    .and_then(|t| t.expr())
                    .and_then(strip_parens)
                else {
                    return (out, false);
                };
                match &expr {
                    ast::Expr::NewExpr(new_expr) => {
                        // 匿名子类的类型不是写出来的那个类
                        if new_expr.body().is_some() {
                            return (out, false);
                        }
                        let Some(ty) = result.type_of(new_expr.syntax().text_range()) else {
                            return (out, false);
                        };
                        let range = new_expr
                            .ty()
                            .map(|t| t.syntax().text_range())
                            .unwrap_or_else(|| new_expr.syntax().text_range());
                        add(&mut out, &mut complete, range, ty.clone());
                    }
                    ast::Expr::NameExpr(name) => {
                        let thrown = self.variable_throws(node, name, result, depth);
                        complete &= thrown.complete;
                        for ty in thrown.types {
                            add(&mut out, &mut complete, expr.syntax().text_range(), ty);
                        }
                    }
                    _ => complete = false,
                }
            }
            _ => {}
        }
        (out, complete)
    }

    /// `throw x`：变量 `x` 抛出的异常类型。
    fn variable_throws(
        &self,
        throw: &SyntaxNode,
        name: &ast::NameExpr,
        result: &InferenceResult,
        depth: u32,
    ) -> Thrown {
        let unknown = Thrown {
            types: Vec::new(),
            complete: false,
        };
        let Some(name_ref) = name.name_ref() else {
            return unknown;
        };
        let declared = |ty: &Ty| Thrown {
            types: vec![ty.clone()],
            complete: true,
        };
        match result.resolution(name_ref.syntax().text_range()) {
            Some(Resolution::Local(id)) => {
                let local = result.local(*id);
                let root = throw.ancestors().last().unwrap_or_else(|| throw.clone());
                let decl = match root.covering_element(local.decl_range) {
                    syntax::NodeOrToken::Node(node) => Some(node),
                    syntax::NodeOrToken::Token(token) => token.parent(),
                };
                match local.kind {
                    LocalKind::CatchParam => {
                        let Some(catch) =
                            decl.and_then(|d| d.ancestors().find_map(ast::CatchClause::cast))
                        else {
                            return unknown;
                        };
                        self.rethrown(&catch, *id, result, depth)
                    }
                    LocalKind::Param => declared(&local.ty),
                    // `var` 的类型是推断出来的，不用
                    LocalKind::Local => {
                        let explicit = decl
                            .and_then(|d| d.ancestors().find_map(ast::LocalVarDecl::cast))
                            .and_then(|d| d.ty())
                            .is_some_and(|ty| ty.syntax().text() != "var");
                        if explicit {
                            declared(&local.ty)
                        } else {
                            unknown
                        }
                    }
                    _ => unknown,
                }
            }
            Some(Resolution::Field(_)) => match result.type_of(name.syntax().text_range()) {
                Some(ty) => declared(ty),
                None => unknown,
            },
            _ => unknown,
        }
    }

    /// 在 `catch` 里 `throw` 它的形参（局部变量 `id`）抛出的异常（JLS §11.2.2）。
    fn rethrown(
        &self,
        catch: &ast::CatchClause,
        id: LocalId,
        result: &InferenceResult,
        depth: u32,
    ) -> Thrown {
        let unknown = Thrown {
            types: Vec::new(),
            complete: false,
        };
        let Some(param_ty) = catch.param().and_then(|p| p.ty()) else {
            return unknown;
        };
        let Some(try_stmt) = catch.syntax().parent().and_then(ast::TryStmt::cast) else {
            return unknown;
        };
        let alternatives: Vec<Ty> = union_alternatives(&param_ty)
            .iter()
            .map(|alt| self.resolve_type(alt))
            .collect();
        if alternatives
            .iter()
            .any(|ty| ty.contains_unknown() || !matches!(ty, Ty::Class(..)))
        {
            return unknown;
        }
        // 多重 catch 的形参隐含 final；单个类型的形参被重新赋值过就不是精确重抛，按声明的类型
        let reassigned = alternatives.len() == 1
            && catch.body().is_some_and(|body| {
                body.syntax().descendants().any(|n| {
                    ast::AssignExpr::cast(n)
                        .and_then(|assign| assign.lhs())
                        .and_then(strip_parens)
                        .is_some_and(|lhs| match lhs {
                            ast::Expr::NameExpr(name) => name.name_ref().is_some_and(|r| {
                                result.resolution(r.syntax().text_range())
                                    == Some(&Resolution::Local(id))
                            }),
                            _ => false,
                        })
                })
            });
        if reassigned {
            return Thrown {
                types: alternatives,
                complete: true,
            };
        }
        if depth >= MAX_DEPTH {
            return unknown;
        }
        let roots: Vec<SyntaxNode> = try_stmt
            .resource_list()
            .map(|r| r.syntax().clone())
            .into_iter()
            .chain(try_stmt.body().map(|b| b.syntax().clone()))
            .collect();
        let in_try = self.thrown_in(&roots, result, depth + 1);
        let mut complete = in_try.complete;
        let earlier: Vec<ast::Type> = try_stmt
            .catch_clauses()
            .take_while(|c| c != catch)
            .filter_map(|c| c.param().and_then(|p| p.ty()))
            .collect();
        let mut types: Vec<Ty> = Vec::new();
        for thrown in in_try.types {
            let mut caught_before = false;
            for ty in &earlier {
                match self.type_covers(ty, &thrown) {
                    Some(true) => caught_before = true,
                    Some(false) => {}
                    None => {
                        caught_before = true;
                        complete = false;
                    }
                }
            }
            if caught_before {
                continue;
            }
            for alt in &alternatives {
                let ty = if is_subtype(self.db, &thrown, alt) {
                    thrown.clone()
                } else if is_subtype(self.db, alt, &thrown) {
                    alt.clone()
                } else {
                    continue;
                };
                if !types.contains(&ty) {
                    types.push(ty);
                }
            }
        }
        Thrown { types, complete }
    }

    /// `roots` 里的代码（不进 lambda 和类体）能抛出、又没有在其中被捕获的受检异常。
    fn thrown_in(&self, roots: &[SyntaxNode], result: &InferenceResult, depth: u32) -> Thrown {
        let mut thrown = Thrown {
            types: Vec::new(),
            complete: true,
        };
        for root in roots {
            let mut walk = root.preorder();
            while let Some(event) = walk.next() {
                let syntax::WalkEvent::Enter(node) = event else {
                    continue;
                };
                if matches!(
                    node.kind(),
                    SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY
                ) {
                    walk.skip_subtree();
                    continue;
                }
                let (sites, complete) = self.node_throws(&node, result, depth);
                thrown.complete &= complete;
                for (_, ty) in sites {
                    match self.caught_within(&node, root, &ty) {
                        Some(true) => {}
                        Some(false) => {
                            if !thrown.types.contains(&ty) {
                                thrown.types.push(ty);
                            }
                        }
                        None => thrown.complete = false,
                    }
                }
            }
        }
        thrown
    }

    /// 从 `site` 往外到 `root`（含）之间的 try 有没有处理 `thrown`。
    fn caught_within(&self, site: &SyntaxNode, root: &SyntaxNode, thrown: &Ty) -> Option<bool> {
        if site == root {
            return Some(false);
        }
        let mut inner = site.clone();
        for ancestor in site.ancestors().skip(1) {
            if let Some(try_stmt) = ast::TryStmt::cast(ancestor.clone()) {
                if self.try_catches(&try_stmt, &inner, thrown)? {
                    return Some(true);
                }
            }
            if ancestor == *root {
                break;
            }
            inner = ancestor;
        }
        Some(false)
    }

    fn is_exception_or_throwable(&self, ty: &Ty) -> bool {
        *ty == self.throwable || self.exception.as_ref() == Some(ty)
    }

    /// `catch` 的受检异常类型，try 块（连同资源的 `close()`）不可能抛出它或它的子类
    /// （JLS §11.2.3）。只在 try 块里的每个抛出点都解析清楚时判断。
    fn never_thrown_catches(
        &self,
        body: &FileBody,
        result: &InferenceResult,
        out: &mut Vec<(TextRange, Ty)>,
    ) {
        // 不带限定符的调用要靠所在类的继承链解析
        if !hierarchy_is_resolved(self.db, body.id.class()) {
            return;
        }
        for try_stmt in body.node.descendants().filter_map(ast::TryStmt::cast) {
            if try_stmt.catch_clauses().next().is_none() {
                continue;
            }
            let roots: Vec<SyntaxNode> = try_stmt
                .resource_list()
                .map(|r| r.syntax().clone())
                .into_iter()
                .chain(try_stmt.body().map(|b| b.syntax().clone()))
                .collect();
            let has_errors = result
                .diagnostics()
                .iter()
                .any(|d| roots.iter().any(|r| r.text_range().contains_range(d.range)));
            if has_errors {
                continue;
            }
            let thrown = self.thrown_in(&roots, result, 0);
            if !thrown.complete {
                continue;
            }
            let mut caught: Vec<Ty> = Vec::new();
            'catches: for catch in try_stmt.catch_clauses() {
                let Some(ty) = catch.param().and_then(|p| p.ty()) else {
                    break;
                };
                for alt in union_alternatives(&ty) {
                    let exc = self.resolve_type(&alt);
                    let Some(checked) = self.checkedness(&exc) else {
                        break 'catches;
                    };
                    // 已经被前面的 catch 接住：javac 报的是另一个错误
                    let already = caught.iter().any(|c| is_subtype(self.db, &exc, c));
                    caught.push(exc.clone());
                    if already || !checked || self.is_exception_or_throwable(&exc) {
                        continue;
                    }
                    let may_throw = thrown
                        .types
                        .iter()
                        .any(|t| is_subtype(self.db, t, &exc) || is_subtype(self.db, &exc, t));
                    if !may_throw {
                        out.push((alt.syntax().text_range(), exc));
                    }
                }
            }
        }
    }
}

fn has_sneaky_throws(callable: &SyntaxNode) -> bool {
    callable
        .children()
        .find_map(ast::ModifierList::cast)
        .is_some_and(|mods| {
            mods.annotations().any(|a| {
                a.name()
                    .is_some_and(|n| n.segments().last().is_some_and(|s| s == "SneakyThrows"))
            })
        })
}

/// 一处没有被处理的受检异常。
#[derive(Debug, Clone)]
pub struct UnhandledException {
    /// 诊断标出的范围（方法名、类型名或资源声明）
    pub range: TextRange,
    /// 抛出异常的那个表达式 / 语句节点的范围
    pub site: TextRange,
    pub ty: Ty,
}

pub(crate) fn exception_diagnostics(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    let Some(checker) = Checker::new(view) else {
        return;
    };
    for body in &view.bodies {
        let result = body.result(view);
        let mut unhandled = Vec::new();
        checker.unhandled_in_body(body, &result, &mut unhandled);
        for unhandled in unhandled {
            out.push(Diagnostic {
                range: unhandled.range,
                message: base_db::tr!(
                    "未处理的异常 `{}`：需要捕获，或在方法上声明 throws",
                    "Unhandled exception `{}`: catch it or declare it in the method's throws clause",
                    unhandled.ty.display(db)
                ),
                severity: Severity::Error,
                code: DiagnosticCode::UnhandledException,
                unused: false,
            });
        }

        let mut found = Vec::new();
        checker.never_thrown_catches(body, &result, &mut found);
        for (range, ty) in found {
            out.push(Diagnostic {
                range,
                message: base_db::tr!(
                    "try 块里不会抛出异常 `{}`，这个 catch 永远执行不到",
                    "Exception `{}` is never thrown in the body of the corresponding try statement",
                    ty.display(db)
                ),
                severity: Severity::Error,
                code: DiagnosticCode::ExceptionNeverThrown,
                unused: false,
            });
        }
    }
}

pub fn unhandled_exceptions(db: &dyn Db, file: SourceFile) -> Vec<UnhandledException> {
    let mut out = Vec::new();
    let view = View::file(db, file);
    let Some(checker) = Checker::new(&view) else {
        return out;
    };
    for body in &view.bodies {
        checker.unhandled_in_body(body, &body.result(&view), &mut out);
    }
    out
}

impl Checker<'_> {
    fn unhandled_in_body(
        &self,
        body: &FileBody,
        result: &InferenceResult,
        out: &mut Vec<UnhandledException>,
    ) {
        let callable = &body.node;
        if has_sneaky_throws(callable) {
            return;
        }
        for node in callable.descendants() {
            let (sites, _) = self.node_throws(&node, result, 0);
            for (range, thrown) in sites {
                if self.is_handled(&node, callable, result, &thrown) == Some(false) {
                    out.push(UnhandledException {
                        range,
                        site: node.text_range(),
                        ty: thrown,
                    });
                }
            }
        }
    }
}

/// 选中的这些语句（或一个表达式）能抛出、又没有在其中被捕获的受检异常，供提取方法使用。
/// 有拿不准的抛出点时为 `None`。
pub fn thrown_exceptions(db: &dyn Db, file: SourceFile, nodes: &[SyntaxNode]) -> Option<Vec<Ty>> {
    let view = View::file(db, file);
    let checker = Checker::new(&view)?;
    let body = view.body_of(nodes.first()?)?;
    let result = view.infer(body);
    let thrown = checker.thrown_in(nodes, &result, 0);
    thrown.complete.then_some(thrown.types)
}

/// `throws` 里多余的一个受检异常类型。
#[derive(Debug, Clone)]
pub struct RedundantThrows {
    /// `throws` 子句里这个类型的范围
    pub range: TextRange,
    /// 整个 `throws` 子句里所有类型的范围（删除时用来处理逗号）
    pub all: Vec<TextRange>,
    pub name: String,
}

/// 不参与覆写的方法和构造器上，`throws` 里声明了、方法体却不会抛出的受检异常。
pub fn redundant_throws(db: &dyn Db, file: SourceFile) -> Vec<RedundantThrows> {
    redundant_throws_in(&View::file(db, file))
}

fn redundant_throws_in(view: &View<'_>) -> Vec<RedundantThrows> {
    let db = view.db;
    let mut out = Vec::new();
    let Some(checker) = Checker::new(view) else {
        return out;
    };
    for file_body in &view.bodies {
        let body = file_body.id;
        let BodyId::Method(method) = body else {
            continue;
        };
        let Some(class) = hir::class_data(db, method.class) else {
            continue;
        };
        let Some(method_data) = class.methods.get(method.idx as usize) else {
            continue;
        };
        let flags = method_data.flags;
        // public / protected 的 `throws` 是对外的约定（删掉会让调用方的 catch 编译不过）
        if matches!(
            flags.visibility(),
            hir::Visibility::Public | hir::Visibility::Protected
        ) {
            continue;
        }
        let standalone = method_data.is_constructor
            || flags.is_static()
            || flags.visibility() == hir::Visibility::Private
            || flags.contains(hir::Modifiers::FINAL)
            || class.flags.contains(hir::Modifiers::FINAL)
            || matches!(class.kind, hir::ClassKind::Enum | hir::ClassKind::Record);
        if !standalone
            || flags.contains(hir::Modifiers::ABSTRACT)
            || flags.contains(hir::Modifiers::NATIVE)
        {
            continue;
        }
        // 序列化方法的签名是约定好的
        if matches!(
            method_data.name.as_str(),
            "readObject" | "writeObject" | "readObjectNoData" | "readResolve" | "writeReplace"
        ) {
            continue;
        }
        // 覆写父类型的方法
        if !method_data.is_constructor {
            let overrides = hir::all_super_tys(db, &Ty::class(method.class))
                .iter()
                .filter_map(|t| t.as_class())
                .filter(|c| *c != method.class)
                .any(|c| {
                    hir::class_data(db, c).is_some_and(|d| {
                        d.methods.iter().any(|m| {
                            !m.is_constructor
                                && m.name == method_data.name
                                && m.params.len() == method_data.params.len()
                        })
                    })
                });
            if overrides {
                continue;
            }
        }
        let callable = file_body.node.clone();
        let throws = ast::MethodDecl::cast(callable.clone())
            .and_then(|m| m.throws_clause())
            .or_else(|| {
                ast::ConstructorDecl::cast(callable.clone()).and_then(|c| c.throws_clause())
            });
        let Some(throws) = throws else { continue };
        // 方法体之外（形参注解之类）不看；没有方法体的不判断
        let Some(block) = callable.children().find(|c| c.kind() == SyntaxKind::BLOCK) else {
            continue;
        };
        let declared: Vec<ast::Type> = throws.types().collect();
        let result = file_body.result(view);
        let escaping = checker.thrown_in(&[block], &result, 0);
        if !escaping.complete {
            continue;
        }
        let all: Vec<TextRange> = declared.iter().map(|t| t.syntax().text_range()).collect();
        for ty in &declared {
            let resolved = checker.resolve_type(ty);
            if checker.checkedness(&resolved) != Some(true) {
                continue;
            }
            // 抛出的异常是它的子类型，或者它是抛出异常的子类型（`throws IOException`
            // 而调用声明了 `Exception`，这时另有"未处理异常"的错误）都算用到了
            let used = escaping
                .types
                .iter()
                .any(|t| is_subtype(db, t, &resolved) || is_subtype(db, &resolved, t));
            if !used {
                out.push(RedundantThrows {
                    range: ty.syntax().text_range(),
                    all: all.clone(),
                    name: ty.syntax().text().to_string(),
                });
            }
        }
    }
    out
}

pub(crate) fn redundant_throws_diagnostics(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    for found in redundant_throws_in(view) {
        let name = &found.name;
        out.push(Diagnostic {
            range: found.range,
            message: base_db::tr!(
                "方法不会抛出 `{name}`，`throws` 里的声明是多余的",
                "`{name}` is never thrown by the method; the `throws` declaration is redundant"
            ),
            severity: Severity::Hint,
            code: DiagnosticCode::RedundantThrows,
            unused: true,
        });
    }
}
