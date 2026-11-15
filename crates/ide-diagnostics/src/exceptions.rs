//! 受检异常检查：调用会抛出受检异常的方法 / 构造器，或 `throw new X(...)`，
//! 却既没有被 `try` 捕获，也没有写进所在方法的 `throws`。
//!
//! 只报能确定的情形，下面这些一律跳过：
//! - 匿名类 / 局部类里的代码（要看内部方法的签名）；
//! - 目标函数式接口确定不了的 lambda（类型未知、重载里有多个个数相同的候选、
//!   接口方法的 `throws` 是类型变量）；
//! - 字段初值、初始化块（实例初始化块的异常由构造器的 `throws` 决定）；
//! - `throw e` 这种抛出变量的语句（精确重抛的规则依赖数据流）；
//! - 抛出的类型是类型变量，或者任何一步解析不出来；
//! - 标了 Lombok `@SneakyThrows` 的方法。

use base_db::{Db, SourceFile};
use hir::{
    body_syntax, infer, is_subclass_of, is_subtype, lookup_class, lookup_methods, method_sig,
    BodyId, InferenceResult, Semantics, Ty,
};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{bodies_in_file, Diagnostic, DiagnosticCode, Severity};

struct Checker<'a> {
    db: &'a dyn Db,
    sema: Semantics<'a>,
    file: SourceFile,
    throwable: Ty,
    runtime_exception: Option<Ty>,
    error: Option<Ty>,
}

impl Checker<'_> {
    fn is_checked(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Class(..))
            && is_subtype(self.db, ty, &self.throwable)
            && !self
                .runtime_exception
                .as_ref()
                .is_some_and(|rt| is_subtype(self.db, ty, rt))
            && !self
                .error
                .as_ref()
                .is_some_and(|err| is_subtype(self.db, ty, err))
    }

    /// 语法里写的一个（可能是 `A | B` 的）异常类型能否接住 `thrown`。
    /// 解析不出来的类型按"能接住"处理。
    fn type_covers(&self, ty: &ast::Type, thrown: &Ty) -> bool {
        let alternatives: Vec<ast::Type> = match ty {
            ast::Type::UnionType(union) => union
                .syntax()
                .children()
                .filter_map(ast::Type::cast)
                .collect(),
            other => vec![other.clone()],
        };
        alternatives.iter().any(|alt| {
            let resolved = self.sema.resolve_type(self.file, alt);
            resolved.contains_unknown() || is_subtype(self.db, thrown, &resolved)
        })
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
                // 类体里的代码属于别的方法
                SyntaxKind::CLASS_BODY if ancestor != *callable => return None,
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
                    // 只有 try 块和资源列表里的代码受 catch 保护
                    let guarded = try_stmt.body().is_some_and(|b| *b.syntax() == inner)
                        || try_stmt
                            .resource_list()
                            .is_some_and(|r| *r.syntax() == inner);
                    if guarded {
                        for catch in try_stmt.catch_clauses() {
                            let ty = catch.param().and_then(|p| p.ty())?;
                            if self.type_covers(&ty, thrown) {
                                return Some(true);
                            }
                        }
                    }
                }
                _ => {}
            }
            if ancestor == *callable {
                break;
            }
            inner = ancestor;
        }

        let throws = if let Some(method) = ast::MethodDecl::cast(callable.clone()) {
            method.throws_clause()
        } else {
            let ctor = ast::ConstructorDecl::cast(callable.clone())?;
            ctor.throws_clause()
        };
        Some(throws.is_some_and(|clause| clause.types().any(|ty| self.type_covers(&ty, thrown))))
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
    fn close_throws(&self, resource: &ast::Resource, result: &InferenceResult) -> Vec<Ty> {
        let ty = match resource.declarator().and_then(|d| d.name()) {
            Some(name) => result
                .local_at_decl(name.syntax().text_range())
                .map(|id| result.local(id).ty.clone()),
            None => resource
                .expr()
                .and_then(|e| result.type_of(e.syntax().text_range()).cloned()),
        };
        let Some(ty @ Ty::Class(..)) = ty else {
            return Vec::new();
        };
        if ty.contains_unknown() {
            return Vec::new();
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
        });
        match most_derived {
            Some(candidate) => method_sig(self.db, candidate.id).throws.clone(),
            None => Vec::new(),
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

pub(crate) fn exception_diagnostics(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    for unhandled in unhandled_exceptions(db, file) {
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
}

pub fn unhandled_exceptions(db: &dyn Db, file: SourceFile) -> Vec<UnhandledException> {
    let mut out = Vec::new();
    let Some(throwable) = lookup_class(db, "java/lang/Throwable") else {
        return out;
    };
    let checker = Checker {
        db,
        sema: Semantics::new(db),
        file,
        throwable: Ty::class(throwable),
        runtime_exception: lookup_class(db, "java/lang/RuntimeException").map(Ty::class),
        error: lookup_class(db, "java/lang/Error").map(Ty::class),
    };

    for body in bodies_in_file(db, file) {
        if !matches!(body, BodyId::Method(_)) {
            continue;
        }
        let Some((_, callable)) = body_syntax(db, body) else {
            continue;
        };
        if has_sneaky_throws(&callable) {
            continue;
        }
        let result = infer(db, body);

        // (报告位置, 抛出的类型)
        let mut sites: Vec<(SyntaxNode, TextRange, Ty)> = Vec::new();
        for node in callable.descendants() {
            match node.kind() {
                SyntaxKind::METHOD_CALL
                | SyntaxKind::NEW_EXPR
                | SyntaxKind::EXPLICIT_CONSTRUCTOR_CALL => {
                    let Some(arg_list) = node.children().find_map(ast::ArgList::cast) else {
                        continue;
                    };
                    let Some(method) = result
                        .call_info(arg_list.syntax().text_range())
                        .and_then(|info| info.chosen)
                    else {
                        continue;
                    };
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
                    // 数组的 `clone()` 不抛 `CloneNotSupportedException`（JLS §10.7）
                    let array_clone = ast::MethodCall::cast(node.clone()).is_some_and(|call| {
                        call.name_ref().is_some_and(|n| n.text() == "clone")
                            && call.receiver().is_some_and(|receiver| {
                                matches!(
                                    result.type_of(receiver.syntax().text_range()),
                                    Some(Ty::Array(_))
                                )
                            })
                    });
                    if array_clone {
                        continue;
                    }
                    for thrown in &method_sig(db, method).throws {
                        sites.push((node.clone(), range, thrown.clone()));
                    }
                }
                SyntaxKind::RESOURCE => {
                    let Some(resource) = ast::Resource::cast(node.clone()) else {
                        continue;
                    };
                    for thrown in checker.close_throws(&resource, &result) {
                        sites.push((node.clone(), node.text_range(), thrown));
                    }
                }
                SyntaxKind::THROW_STMT => {
                    let Some(ast::Expr::NewExpr(new_expr)) =
                        ast::ThrowStmt::cast(node.clone()).and_then(|t| t.expr())
                    else {
                        continue;
                    };
                    // 匿名子类的类型不是写出来的那个类，跳过
                    if new_expr.body().is_some() {
                        continue;
                    }
                    let Some(ty) = result.type_of(new_expr.syntax().text_range()) else {
                        continue;
                    };
                    let range = new_expr
                        .ty()
                        .map(|t| t.syntax().text_range())
                        .unwrap_or_else(|| new_expr.syntax().text_range());
                    sites.push((node.clone(), range, ty.clone()));
                }
                _ => {}
            }
        }

        for (site, range, thrown) in sites {
            if thrown.contains_unknown() || !checker.is_checked(&thrown) {
                continue;
            }
            if checker.is_handled(&site, &callable, &result, &thrown) == Some(false) {
                out.push(UnhandledException {
                    range,
                    site: site.text_range(),
                    ty: thrown,
                });
            }
        }
    }
    out
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

impl Checker<'_> {
    /// 方法体里能抛到方法外面的受检异常类型。有确定不了的地方时返回 `None`。
    fn escaping(&self, callable: &SyntaxNode, result: &InferenceResult) -> Option<Vec<Ty>> {
        let mut thrown: Vec<(SyntaxNode, Ty)> = Vec::new();
        for node in callable.descendants() {
            match node.kind() {
                SyntaxKind::METHOD_CALL
                | SyntaxKind::NEW_EXPR
                | SyntaxKind::EXPLICIT_CONSTRUCTOR_CALL => {
                    let arg_list = node.children().find_map(ast::ArgList::cast)?;
                    let method = result.call_info(arg_list.syntax().text_range())?.chosen?;
                    for ty in &method_sig(self.db, method).throws {
                        thrown.push((node.clone(), ty.clone()));
                    }
                }
                SyntaxKind::RESOURCE => {
                    let resource = ast::Resource::cast(node.clone())?;
                    for ty in self.close_throws(&resource, result) {
                        thrown.push((node.clone(), ty));
                    }
                }
                SyntaxKind::THROW_STMT => {
                    let expr = ast::ThrowStmt::cast(node.clone()).and_then(|t| t.expr())?;
                    let ast::Expr::NewExpr(new_expr) = &expr else {
                        // `throw e`：精确重抛依赖数据流，不判断
                        return None;
                    };
                    if new_expr.body().is_some() {
                        return None;
                    }
                    thrown.push((
                        node.clone(),
                        result.type_of(expr.syntax().text_range())?.clone(),
                    ));
                }
                _ => {}
            }
        }
        let mut escaping = Vec::new();
        for (site, ty) in thrown {
            if ty.contains_unknown() || !matches!(ty, Ty::Class(..)) {
                return None;
            }
            if !self.is_checked(&ty) {
                continue;
            }
            // 被 try 接住、在 lambda / 匿名类里的不会抛到方法外
            let mut inner = site.clone();
            let mut caught = false;
            for ancestor in site.ancestors().skip(1) {
                if ancestor == *callable {
                    break;
                }
                match ancestor.kind() {
                    SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY => {
                        caught = true;
                        break;
                    }
                    SyntaxKind::TRY_STMT => {
                        let try_stmt = ast::TryStmt::cast(ancestor.clone())?;
                        let guarded = try_stmt.body().is_some_and(|b| *b.syntax() == inner)
                            || try_stmt
                                .resource_list()
                                .is_some_and(|r| *r.syntax() == inner);
                        if guarded
                            && try_stmt.catch_clauses().any(|c| {
                                c.param()
                                    .and_then(|p| p.ty())
                                    .is_some_and(|t| self.type_covers(&t, &ty))
                            })
                        {
                            caught = true;
                            break;
                        }
                    }
                    _ => {}
                }
                inner = ancestor;
            }
            if !caught {
                escaping.push(ty);
            }
        }
        Some(escaping)
    }
}

/// 不参与覆写的方法和构造器上，`throws` 里声明了、方法体却不会抛出的受检异常。
pub fn redundant_throws(db: &dyn Db, file: SourceFile) -> Vec<RedundantThrows> {
    let mut out = Vec::new();
    let Some(throwable) = lookup_class(db, "java/lang/Throwable") else {
        return out;
    };
    let checker = Checker {
        db,
        sema: Semantics::new(db),
        file,
        throwable: Ty::class(throwable),
        runtime_exception: lookup_class(db, "java/lang/RuntimeException").map(Ty::class),
        error: lookup_class(db, "java/lang/Error").map(Ty::class),
    };
    for body in bodies_in_file(db, file) {
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
        let Some((_, callable)) = body_syntax(db, body) else {
            continue;
        };
        let throws = ast::MethodDecl::cast(callable.clone())
            .and_then(|m| m.throws_clause())
            .or_else(|| {
                ast::ConstructorDecl::cast(callable.clone()).and_then(|c| c.throws_clause())
            });
        let Some(throws) = throws else { continue };
        // 方法体之外（形参注解之类）不看；没有方法体的不判断
        if !callable.children().any(|c| c.kind() == SyntaxKind::BLOCK) {
            continue;
        }
        let declared: Vec<ast::Type> = throws.types().collect();
        let result = infer(db, body);
        let Some(escaping) = checker.escaping(&callable, &result) else {
            continue;
        };
        let all: Vec<TextRange> = declared.iter().map(|t| t.syntax().text_range()).collect();
        for ty in &declared {
            let resolved = checker.sema.resolve_type(file, ty);
            if resolved.contains_unknown() || !checker.is_checked(&resolved) {
                continue;
            }
            // 抛出的异常是它的子类型，或者它是抛出异常的子类型（`throws IOException`
            // 而调用声明了 `Exception`，这时另有"未处理异常"的错误）都算用到了
            let used = escaping
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

pub(crate) fn redundant_throws_diagnostics(
    db: &dyn Db,
    file: SourceFile,
    out: &mut Vec<Diagnostic>,
) {
    for found in redundant_throws(db, file) {
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
