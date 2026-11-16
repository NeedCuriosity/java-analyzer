//! 代码检查：使用已过时的 API，以及几种可以简化的写法（和 `true` / `false` 比较、
//! 直接返回条件的 if、`size() == 0`、多余的强制转换）。

use base_db::Db;
use hir::{class_data, BodyId, ClassId, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

fn outermost(db: &dyn Db, class: ClassId) -> String {
    let binary = class.binary_name(db);
    binary.split('$').next().unwrap_or(&binary).to_owned()
}

/// 体所在的成员（或它所在的类）本身已过时：里面的使用不提示（和 javac 一致）。
fn in_deprecated_context(db: &dyn Db, body: BodyId) -> bool {
    let class = body.class();
    let mut current = Some(class);
    while let Some(c) = current {
        let Some(data) = class_data(db, c) else {
            break;
        };
        if data.flags.is_deprecated() {
            return true;
        }
        current = hir::outer_class(db, c);
    }
    let Some(data) = class_data(db, class) else {
        return false;
    };
    match body {
        BodyId::Method(m) => data
            .methods
            .get(m.idx as usize)
            .is_some_and(|m| m.flags.is_deprecated()),
        BodyId::Field(f) => data
            .fields
            .get(f.idx as usize)
            .is_some_and(|f| f.flags.is_deprecated()),
        BodyId::Initializer { .. } => false,
    }
}

/// 名字 `name` 被标成已过时的诊断。
fn deprecated(range: TextRange, what: &str) -> Diagnostic {
    Diagnostic {
        range,
        message: base_db::tr!("`{what}` 已过时", "`{what}` is deprecated"),
        severity: Severity::Warning,
        code: DiagnosticCode::DeprecatedUsage,
        unused: false,
    }
}

/// 使用了标注 `@Deprecated` 的类、字段、方法或构造器。同一个顶层类里的使用不提示。
pub(crate) fn deprecated_usages(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    for file_body in &view.bodies {
        let body = file_body.id;
        if in_deprecated_context(db, body) {
            continue;
        }
        let here = outermost(db, body.class());
        let result = file_body.result(view);
        for (range, resolution) in result.resolutions() {
            let (owner, flags, name) = match resolution {
                Resolution::Field(field) => {
                    let Some(data) = class_data(db, field.class) else {
                        continue;
                    };
                    let Some(f) = data.fields.get(field.idx as usize) else {
                        continue;
                    };
                    (field.class, f.flags, f.name.to_string())
                }
                Resolution::Method(method) => {
                    let Some(data) = class_data(db, method.class) else {
                        continue;
                    };
                    let Some(m) = data.methods.get(method.idx as usize) else {
                        continue;
                    };
                    (method.class, m.flags, m.name.to_string())
                }
                Resolution::Class(class) => {
                    let Some(data) = class_data(db, *class) else {
                        continue;
                    };
                    (*class, data.flags, data.name.to_string())
                }
                _ => continue,
            };
            if flags.is_deprecated() && outermost(db, owner) != here {
                out.push(deprecated(range, &name));
            }
        }
        // 已过时的构造器：`new Date(2020, 1, 1)`
        for new in file_body.node.descendants().filter_map(ast::NewExpr::cast) {
            let (Some(args), Some(ast::Type::ClassType(ty))) = (new.arg_list(), new.ty()) else {
                continue;
            };
            let Some(info) = result.call_info(args.syntax().text_range()) else {
                continue;
            };
            let Some(ctor) = info.chosen else {
                continue;
            };
            let Some(data) = class_data(db, ctor.class) else {
                continue;
            };
            let ctor_deprecated = data
                .methods
                .get(ctor.idx as usize)
                .is_some_and(|m| m.flags.is_deprecated());
            // 类本身已过时的情形上面已经报过了
            if ctor_deprecated && !data.flags.is_deprecated() && outermost(db, ctor.class) != here {
                if let Some(name) = ty.name_ref() {
                    out.push(deprecated(name.syntax().text_range(), &data.name));
                }
            }
        }
    }
}

fn strip_parens(mut expr: ast::Expr) -> ast::Expr {
    while let ast::Expr::ParenExpr(p) = &expr {
        match p.expr() {
            Some(inner) => expr = inner,
            None => break,
        }
    }
    expr
}

fn bool_literal(expr: &ast::Expr) -> Option<bool> {
    match strip_parens(expr.clone()) {
        ast::Expr::Literal(l) => match l.kind()? {
            SyntaxKind::TRUE_KW => Some(true),
            SyntaxKind::FALSE_KW => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn weak(range: TextRange, code: DiagnosticCode, message: String) -> Diagnostic {
    Diagnostic {
        range,
        message,
        severity: Severity::Warning,
        code,
        unused: false,
    }
}

/// 只有一条语句的块返回那条语句。
fn single_statement(stmt: ast::Stmt) -> Option<ast::Stmt> {
    match stmt {
        ast::Stmt::Block(block) => {
            let mut stmts = block.statements();
            match (stmts.next(), stmts.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
        other => Some(other),
    }
}

/// `return true;` / `return false;` 返回的布尔字面量。
fn returned_bool(stmt: &ast::Stmt) -> Option<bool> {
    match stmt {
        ast::Stmt::ReturnStmt(r) => bool_literal(&r.expr()?),
        _ => None,
    }
}

/// `if (c) return true; else return false;`（或者 if 后面紧跟着 `return false;`）：
/// 返回的就是条件本身。也覆盖给同一个变量赋 `true` / `false` 的写法。
pub(crate) fn redundant_if(if_stmt: &ast::IfStmt) -> bool {
    let Some(then) = if_stmt.then_branch().and_then(single_statement) else {
        return false;
    };
    let other = match if_stmt.else_branch() {
        Some(other) => single_statement(other),
        None => {
            // `if (c) return true; return false;`
            let next = if_stmt.syntax().next_sibling().and_then(ast::Stmt::cast);
            match (&then, next) {
                (ast::Stmt::ReturnStmt(_), Some(next @ ast::Stmt::ReturnStmt(_))) => Some(next),
                _ => None,
            }
        }
    };
    let Some(other) = other else {
        return false;
    };
    if let (Some(a), Some(b)) = (returned_bool(&then), returned_bool(&other)) {
        return a != b;
    }
    // `if (c) x = true; else x = false;`
    if if_stmt.else_branch().is_none() {
        return false;
    }
    let assign = |s: &ast::Stmt| -> Option<(String, bool)> {
        let ast::Stmt::ExprStmt(e) = s else {
            return None;
        };
        let ast::Expr::AssignExpr(a) = e.expr()? else {
            return None;
        };
        if a.op_token()?.text() != "=" {
            return None;
        }
        Some((
            a.lhs()?.syntax().text().to_string(),
            bool_literal(&a.rhs()?)?,
        ))
    };
    matches!((assign(&then), assign(&other)), (Some((l1, v1)), Some((l2, v2))) if l1 == l2 && v1 != v2)
}

/// `x.size() == 0` 这类比较的接收者和"是否取反"（`!= 0` / `> 0` 取反）。
pub(crate) fn size_comparison(binary: &ast::BinaryExpr) -> Option<(ast::Expr, bool)> {
    let op = binary.op_kind()?;
    let (lhs, rhs) = (strip_parens(binary.lhs()?), strip_parens(binary.rhs()?));
    let is_zero = |e: &ast::Expr| matches!(e, ast::Expr::Literal(l) if l.syntax().text() == "0");
    // 把 `0 == x.size()` 统一成 `x.size() == 0` 的方向
    let (call, op) = if is_zero(&rhs) {
        (lhs, op)
    } else if is_zero(&lhs) {
        let mirrored = match op {
            SyntaxKind::LT => SyntaxKind::GT,
            SyntaxKind::GT => SyntaxKind::LT,
            other => other,
        };
        (rhs, mirrored)
    } else {
        return None;
    };
    let negated = match op {
        SyntaxKind::EQ_EQ => false,
        SyntaxKind::NEQ | SyntaxKind::GT => true,
        _ => return None,
    };
    let ast::Expr::MethodCall(call) = call else {
        return None;
    };
    let name = call.name_ref()?.text();
    if !(name == "size" || name == "length") || call.arg_list()?.args().next().is_some() {
        return None;
    }
    Some((call.receiver()?, negated))
}

/// 类型有没有无参的 `isEmpty()` 方法。`CharSequence.isEmpty()` 是 Java 15 才加的，
/// 语言级别更低时不算。
fn has_is_empty(db: &dyn Db, ty: &Ty, level: Option<u32>) -> bool {
    hir::lookup_methods(db, ty, "isEmpty").iter().any(|m| {
        if !hir::method_sig(db, m.id).params.is_empty() {
            return false;
        }
        let from_char_sequence = hir::class_data(db, m.id.class)
            .is_some_and(|d| &*d.binary_name == "java/lang/CharSequence");
        !(from_char_sequence && level.is_some_and(|l| l < 15))
    })
}

/// 强制转换的目标类型和操作数的类型完全相同时返回目标类型。目标类型（`resolve_target`）
/// 到最后才解析。
pub(crate) fn redundant_cast_target(
    db: &dyn Db,
    cast: &ast::CastExpr,
    operand_ty: Option<&Ty>,
    resolve_target: impl FnOnce() -> Ty,
    result_type_of: impl Fn(TextRange) -> Option<Ty>,
) -> Option<Ty> {
    let operand_ty = operand_ty?;
    let operand = cast.expr()?;
    // lambda / 方法引用靠转换提供目标类型；实参位置的转换可能用来选重载
    if matches!(
        strip_parens(operand.clone()),
        ast::Expr::LambdaExpr(_) | ast::Expr::MethodRef(_) | ast::Expr::ConditionalExpr(_)
    ) {
        return None;
    }
    // 签名多态的调用（`MethodHandle.invokeExact`、`VarHandle.compareAndSet`）靠转换决定签名
    if let ast::Expr::MethodCall(call) = strip_parens(operand.clone()) {
        if let Some(Ty::Class(class, _)) = call
            .receiver()
            .and_then(|r| result_type_of(r.syntax().text_range()))
        {
            let name = class.binary_name(db);
            if &*name == "java/lang/invoke/MethodHandle" || &*name == "java/lang/invoke/VarHandle" {
                return None;
            }
        }
    }
    let mut parent = cast.syntax().parent();
    while let Some(p) = parent
        .clone()
        .filter(|p| p.kind() == SyntaxKind::PAREN_EXPR)
    {
        parent = p.parent();
    }
    if parent.is_some_and(|p| p.kind() == SyntaxKind::ARG_LIST) {
        return None;
    }
    let plain = |ty: &Ty| match ty {
        Ty::Primitive(_) => true,
        Ty::Class(_, args) => args.is_empty(),
        _ => false,
    };
    if !plain(operand_ty) {
        return None;
    }
    let target = resolve_target();
    let redundant = plain(&target)
        && *operand_ty == target
        && !target.contains_unknown()
        && db_is_class_or_primitive(db, &target);
    redundant.then_some(target)
}

fn db_is_class_or_primitive(db: &dyn Db, ty: &Ty) -> bool {
    match ty {
        Ty::Primitive(_) => true,
        Ty::Class(class, _) => class_data(db, *class).is_some_and(|d| d.type_params.is_empty()),
        _ => false,
    }
}

/// 可以简化的写法。
pub(crate) fn simplifications(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    for body in &view.bodies {
        let result = body.result(view);
        check_body(view, &body.node, &result, out);
    }
}

fn check_body(
    view: &View<'_>,
    node: &SyntaxNode,
    result: &hir::InferenceResult,
    out: &mut Vec<Diagnostic>,
) {
    let db = view.db;
    let file = view.file;
    for n in node.descendants() {
        match n.kind() {
            SyntaxKind::BINARY_EXPR => {
                let Some(binary) = ast::BinaryExpr::cast(n.clone()) else {
                    continue;
                };
                // `x == true` / `x != false`
                if matches!(binary.op_kind(), Some(SyntaxKind::EQ_EQ | SyntaxKind::NEQ)) {
                    let (Some(lhs), Some(rhs)) = (binary.lhs(), binary.rhs()) else {
                        continue;
                    };
                    let other = match (bool_literal(&lhs), bool_literal(&rhs)) {
                        (Some(_), None) => Some(rhs),
                        (None, Some(_)) => Some(lhs),
                        _ => None,
                    };
                    let primitive_boolean = other.as_ref().is_some_and(|o| {
                        result.type_of(o.syntax().text_range()) == Some(&Ty::BOOLEAN)
                    });
                    if primitive_boolean {
                        out.push(weak(
                            binary.syntax().text_range(),
                            DiagnosticCode::PointlessBoolean,
                            base_db::tr!(
                                "和布尔字面量比较是多余的",
                                "Comparison with a boolean literal is redundant"
                            ),
                        ));
                        continue;
                    }
                }
                // `isEmpty()` 自己的实现里不提示
                let in_is_empty = || {
                    n.ancestors()
                        .find_map(ast::MethodDecl::cast)
                        .and_then(|m| m.name())
                        .is_some_and(|name| name.text() == "isEmpty")
                };
                if let Some((receiver, _)) = size_comparison(&binary).filter(|_| !in_is_empty()) {
                    let receiver_ty = result.type_of(receiver.syntax().text_range());
                    let level = crate::language_level::language_level(db, file);
                    if receiver_ty.is_some_and(|ty| has_is_empty(db, ty, level)) {
                        out.push(weak(
                            binary.syntax().text_range(),
                            DiagnosticCode::UseIsEmpty,
                            base_db::tr!(
                                "可以换成 `isEmpty()`",
                                "Can be replaced with `isEmpty()`"
                            ),
                        ));
                    }
                }
            }
            SyntaxKind::IF_STMT => {
                let Some(if_stmt) = ast::IfStmt::cast(n.clone()) else {
                    continue;
                };
                // `else if` 链里的 if 不单独提示
                if n.parent().is_some_and(|p| p.kind() == SyntaxKind::IF_STMT) {
                    continue;
                }
                if redundant_if(&if_stmt) {
                    let keyword = n
                        .first_token()
                        .map_or_else(|| n.text_range(), |t| t.text_range());
                    out.push(weak(
                        keyword,
                        DiagnosticCode::RedundantIf,
                        base_db::tr!(
                            "这个 if 语句可以简化为直接使用条件",
                            "This `if` statement can be simplified to use the condition directly"
                        ),
                    ));
                }
            }
            SyntaxKind::CAST_EXPR => {
                let Some(cast) = ast::CastExpr::cast(n.clone()) else {
                    continue;
                };
                let (Some(ty), Some(operand)) = (cast.ty(), cast.expr()) else {
                    continue;
                };
                let inner = strip_parens(operand.clone());
                // 变量取声明的类型：`instanceof` 之后推断出的收窄类型在 Java 里并不存在
                let declared = match &inner {
                    ast::Expr::NameExpr(name) => name
                        .name_ref()
                        .and_then(|r| result.resolution(r.syntax().text_range()))
                        .and_then(|res| match res {
                            // multi-catch 的形参是几个类型的并，记下的只是其中一个
                            Resolution::Local(id)
                                if !matches!(
                                    result.local(*id).kind,
                                    hir::LocalKind::CatchParam | hir::LocalKind::LambdaParam
                                ) =>
                            {
                                Some(result.local(*id).ty.clone())
                            }
                            Resolution::Field(field) => Some(hir::field_ty(db, *field)),
                            _ => None,
                        }),
                    _ => None,
                };
                // 泛型方法的返回类型可能是从转换的目标类型推断出来的：`(Foo) get()`
                if let ast::Expr::MethodCall(call) = &inner {
                    let generic = call
                        .arg_list()
                        .and_then(|args| result.call_info(args.syntax().text_range()))
                        .and_then(|info| info.chosen)
                        .is_none_or(|m| hir::method_sig(db, m).n_type_params > 0);
                    if generic {
                        continue;
                    }
                }
                let operand_ty = match &inner {
                    ast::Expr::NameExpr(_) => declared.as_ref(),
                    _ => result.type_of(operand.syntax().text_range()),
                };
                let type_of = |range| result.type_of(range).cloned();
                let resolve_target = || view.resolve_type(&ty);
                if let Some(target) =
                    redundant_cast_target(db, &cast, operand_ty, resolve_target, type_of)
                {
                    // 范围是 `(类型)` 这一段
                    let range = TextRange::new(
                        cast.syntax().text_range().start(),
                        operand.syntax().text_range().start(),
                    );
                    out.push(Diagnostic {
                        range,
                        message: base_db::tr!(
                            "转换成 `{}` 是多余的",
                            "Casting to `{}` is redundant",
                            target.display(db)
                        ),
                        severity: Severity::Warning,
                        code: DiagnosticCode::RedundantCast,
                        unused: true,
                    });
                }
            }
            SyntaxKind::LOCAL_VAR_DECL => {
                if let Some(name) = immediately_returned(&n, result) {
                    out.push(weak(
                        name.text_range(),
                        DiagnosticCode::RedundantLocal,
                        base_db::tr!(
                            "局部变量 `{}` 是多余的：声明后立即被返回",
                            "Local variable `{}` is redundant: it is returned right after its declaration",
                            name.text()
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
}

/// `T x = expr; return x;`（或 `throw x;`）里只用了这一次的 `x`，返回它的名字节点。
/// 初值是数组初始化式、lambda、方法引用时内联后需要目标类型，不提示。
pub(crate) fn immediately_returned(
    decl: &SyntaxNode,
    result: &hir::InferenceResult,
) -> Option<SyntaxNode> {
    let local_decl = ast::LocalVarDecl::cast(decl.clone())?;
    if local_decl
        .modifier_list()
        .is_some_and(|m| m.annotations().next().is_some())
    {
        return None;
    }
    let mut declarators = local_decl.declarators();
    let declarator = declarators.next()?;
    if declarators.next().is_some() || declarator.extra_dims() > 0 {
        return None;
    }
    let init = declarator.initializer()?;
    if matches!(
        init.syntax().kind(),
        SyntaxKind::ARRAY_INITIALIZER | SyntaxKind::LAMBDA_EXPR | SyntaxKind::METHOD_REF
    ) {
        return None;
    }
    let name = declarator.name()?;
    let id = result.local_at_decl(name.syntax().text_range())?;
    // 涉及基本类型的转换（`long x = intValue;`、装箱）时内联可能改变语义
    let init_ty = result.type_of(init.syntax().text_range())?;
    let local_ty = &result.local(id).ty;
    let primitive =
        matches!(init_ty, hir::Ty::Primitive(_)) || matches!(local_ty, hir::Ty::Primitive(_));
    if (primitive && init_ty != local_ty) || matches!(init_ty, hir::Ty::Unknown) {
        return None;
    }
    // 紧跟着的下一条语句（中间不能有注释）
    let mut next = decl.next_sibling_or_token();
    while let Some(element) = &next {
        match element {
            syntax::NodeOrToken::Token(t) if t.kind() == SyntaxKind::WHITESPACE => {
                next = element.next_sibling_or_token();
            }
            _ => break,
        }
    }
    let stmt = next?.into_node()?;
    if !matches!(
        stmt.kind(),
        SyntaxKind::RETURN_STMT | SyntaxKind::THROW_STMT
    ) {
        return None;
    }
    let value = stmt.children().find(|c| ast::Expr::can_cast(c.kind()))?;
    if value.kind() != SyntaxKind::NAME_EXPR {
        return None;
    }
    let value_ref = value
        .children()
        .find(|c| c.kind() == SyntaxKind::NAME_REF)?;
    if result.resolution(value_ref.text_range()) != Some(&hir::Resolution::Local(id)) {
        return None;
    }
    let uses = result
        .resolutions()
        .filter(|(_, r)| **r == hir::Resolution::Local(id))
        .count();
    (uses == 1).then(|| name.syntax().clone())
}
