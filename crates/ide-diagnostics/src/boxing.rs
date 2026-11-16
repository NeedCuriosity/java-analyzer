//! 不必要的装箱和拆箱（编译器会自动做）：
//!
//! - `Integer.valueOf(i)` / `new Integer(i)`（实参正好是对应的基本类型）用在需要 `Integer` 的地方；
//! - `n.intValue()`（`n` 是 `Integer`）用在需要 `int` 的地方。
//!
//! 只看初值、赋值右边、`return` 和运算符的操作数这些不影响重载选择的位置；方法实参不报。

use base_db::{Db, SourceFile};
use hir::{class_header, InferenceResult, PrimitiveType, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

/// 跳过外面的括号。
fn outermost(node: &SyntaxNode) -> SyntaxNode {
    let mut node = node.clone();
    while let Some(parent) = node.parent().filter(|p| p.kind() == SyntaxKind::PAREN_EXPR) {
        node = parent;
    }
    node
}

/// 表达式作为初值 / 赋值右边 / 返回值时要求的类型。`compound` 为真时也接受 `+=` 这类复合赋值。
pub(crate) fn target_type(
    view: &View<'_>,
    result: &InferenceResult,
    expr: &SyntaxNode,
    compound: bool,
) -> Option<Ty> {
    let node = outermost(expr);
    let parent = node.parent()?;
    match parent.kind() {
        SyntaxKind::VARIABLE_DECLARATOR => {
            let declarator = ast::VariableDeclarator::cast(parent.clone())?;
            let name = declarator.name()?;
            // `var x = …` 的类型来自初值本身，没有目标类型
            let is_var = parent
                .parent()
                .and_then(ast::LocalVarDecl::cast)
                .and_then(|d| d.ty())
                .is_some_and(|t| t.syntax().text() == "var");
            if is_var {
                return None;
            }
            if let Some(id) = result.local_at_decl(name.syntax().text_range()) {
                return Some(result.local(id).ty.clone());
            }
            // 字段的初值
            let field = parent.parent().and_then(ast::FieldDecl::cast)?;
            if declarator.extra_dims() > 0 {
                return None;
            }
            Some(view.resolve_type(&field.ty()?))
        }
        SyntaxKind::ASSIGN_EXPR => {
            let assign = ast::AssignExpr::cast(parent)?;
            if assign.rhs()?.syntax() != &node {
                return None;
            }
            let plain = assign.op_token()?.kind() == SyntaxKind::EQ;
            if !plain && !compound {
                return None;
            }
            result.type_of(assign.lhs()?.syntax().text_range()).cloned()
        }
        SyntaxKind::RETURN_STMT => {
            let method = parent
                .ancestors()
                .find(|a| matches!(a.kind(), SyntaxKind::METHOD_DECL | SyntaxKind::LAMBDA_EXPR))
                .and_then(ast::MethodDecl::cast)?;
            let ret = method.return_type()?;
            Some(view.resolve_type(&ret))
        }
        _ => None,
    }
}

/// 拆箱后的值用作运算符的操作数，去掉 `xxxValue()` 编译器也会拆箱。
fn unboxed_operand(result: &InferenceResult, expr: &SyntaxNode) -> bool {
    let node = outermost(expr);
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        SyntaxKind::BINARY_EXPR => {
            let Some(binary) = ast::BinaryExpr::cast(parent.clone()) else {
                return false;
            };
            let other = match (binary.lhs(), binary.rhs()) {
                (Some(l), Some(r)) if l.syntax() == &node => r,
                (Some(l), Some(r)) if r.syntax() == &node => l,
                _ => return false,
            };
            let other_ty = result.type_of(other.syntax().text_range());
            match binary.op_kind() {
                Some(
                    SyntaxKind::MINUS
                    | SyntaxKind::STAR
                    | SyntaxKind::SLASH
                    | SyntaxKind::PERCENT
                    | SyntaxKind::LT
                    | SyntaxKind::GT
                    | SyntaxKind::LT_EQ
                    | SyntaxKind::GT_EQ
                    | SyntaxKind::AMP_AMP
                    | SyntaxKind::PIPE_PIPE,
                ) => true,
                // `+` 可能是字符串拼接（`null` 时结果不同）；`==` 两边都是包装类时比较的是引用
                Some(SyntaxKind::PLUS) => matches!(other_ty, Some(Ty::Primitive(_))),
                // 另一边也是 `xxxValue()` 时两边都去掉就成了引用比较
                Some(SyntaxKind::EQ_EQ | SyntaxKind::NEQ) => {
                    let other_unboxes = ast::MethodCall::cast(other.syntax().clone())
                        .and_then(|c| c.name_ref())
                        .is_some_and(|n| n.text().ends_with("Value"));
                    matches!(other_ty, Some(Ty::Primitive(_))) && !other_unboxes
                }
                _ => false,
            }
        }
        SyntaxKind::UNARY_EXPR => ast::UnaryExpr::cast(parent).is_some_and(|u| {
            u.op_token().is_some_and(|t| {
                matches!(
                    t.kind(),
                    SyntaxKind::BANG | SyntaxKind::MINUS | SyntaxKind::TILDE
                )
            })
        }),
        _ => false,
    }
}

fn boxed_class(db: &dyn Db, class: hir::ClassId) -> Option<PrimitiveType> {
    PrimitiveType::from_boxed_class(&class_header(db, class)?.binary_name)
}

/// `Integer.valueOf(i)` / `new Integer(i)`：返回实参和对应的基本类型。
fn boxing_call(db: &dyn Db, result: &InferenceResult, expr: &SyntaxNode) -> Option<ast::Expr> {
    let (name_range, args) = match expr.kind() {
        SyntaxKind::METHOD_CALL => {
            let call = ast::MethodCall::cast(expr.clone())?;
            let name = call.name_ref()?;
            if name.text() != "valueOf" {
                return None;
            }
            (name.syntax().text_range(), call.arg_list()?)
        }
        SyntaxKind::NEW_EXPR => {
            let new = ast::NewExpr::cast(expr.clone())?;
            if new.body().is_some() {
                return None;
            }
            let ast::Type::ClassType(ty) = new.ty()? else {
                return None;
            };
            let name = ty
                .syntax()
                .descendants()
                .filter_map(ast::NameRef::cast)
                .last()?;
            (name.syntax().text_range(), new.arg_list()?)
        }
        _ => return None,
    };
    let Resolution::Method(method) = result.resolution(name_range)? else {
        return None;
    };
    let primitive = boxed_class(db, method.class)?;
    let mut args = args.args();
    let (Some(arg), None) = (args.next(), args.next()) else {
        return None;
    };
    if result.type_of(arg.syntax().text_range()) != Some(&Ty::Primitive(primitive)) {
        return None;
    }
    // 结果正好是这个包装类
    let ty = result.type_of(expr.text_range())?;
    matches!(ty, Ty::Class(c, _) if *c == method.class).then_some(arg)
}

/// `n.intValue()`：返回方法名开始到调用结束的范围（`.intValue()`，包括点号）。
fn unboxing_call(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
) -> Option<TextRange> {
    let name = call.name_ref()?;
    if !name.text().ends_with("Value") || call.arg_list()?.args().next().is_some() {
        return None;
    }
    let receiver = call.receiver()?;
    let Some(Ty::Class(class, _)) = result.type_of(receiver.syntax().text_range()) else {
        return None;
    };
    let primitive = boxed_class(db, *class)?;
    if name.text() != format!("{}Value", primitive.name()) {
        return None;
    }
    let Resolution::Method(method) = result.resolution(name.syntax().text_range())? else {
        return None;
    };
    if method.class != *class {
        return None;
    }
    let start = name
        .syntax()
        .first_token()
        .and_then(|t| t.prev_token())
        .filter(|t| t.kind() == SyntaxKind::DOT)
        .map_or(name.syntax().text_range().start(), |t| {
            t.text_range().start()
        });
    Some(TextRange::new(start, call.syntax().text_range().end()))
}

/// `class` 就是 `target`，或者嵌套在 `target` 里。
fn inside_class(db: &dyn Db, class: hir::ClassId, target: hir::ClassId) -> bool {
    let mut current = Some(class);
    while let Some(c) = current {
        if c == target {
            return true;
        }
        current = hir::outer_class(db, c);
    }
    false
}

pub(crate) fn unnecessary_boxing(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    if !view.file_contains("valueOf")
        && !view.file_contains("Value()")
        && !view.file_contains("new ")
    {
        return;
    }
    for file_body in &view.bodies {
        let body = file_body.id;
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for expr in file_body.node.descendants() {
            match expr.kind() {
                SyntaxKind::METHOD_CALL | SyntaxKind::NEW_EXPR => {}
                _ => continue,
            }
            // 快速过滤：只看可能相关的名字
            let relevant = match expr.kind() {
                SyntaxKind::METHOD_CALL => ast::MethodCall::cast(expr.clone())
                    .and_then(|c| c.name_ref())
                    .is_some_and(|n| n.text() == "valueOf" || n.text().ends_with("Value")),
                _ => true,
            };
            if !relevant {
                continue;
            }
            let result = result.get_or_insert_with(|| file_body.result(view));
            if boxing_call(db, result, &expr).is_some() {
                // 包装类自己（包括它的嵌套类，比如缓存）里的自动装箱会调用正在初始化的 `valueOf`
                if let Some(Ty::Class(boxed, _)) = result.type_of(expr.text_range()) {
                    if inside_class(db, body.class(), *boxed) {
                        continue;
                    }
                }
                let wanted = target_type(view, result, &expr, false);
                if wanted.as_ref() == result.type_of(expr.text_range()) {
                    out.push(Diagnostic {
                        range: expr.text_range(),
                        message: base_db::trs!("不必要的装箱", "Unnecessary boxing").to_owned(),
                        severity: Severity::Hint,
                        code: DiagnosticCode::UnnecessaryBoxing,
                        unused: false,
                    });
                }
                continue;
            }
            let Some(call) = ast::MethodCall::cast(expr.clone()) else {
                continue;
            };
            let Some(range) = unboxing_call(db, result, &call) else {
                continue;
            };
            let call_ty = result.type_of(expr.text_range()).cloned();
            let redundant = match target_type(view, result, &expr, true) {
                Some(wanted) => call_ty.as_ref() == Some(&wanted),
                None => unboxed_operand(result, &expr),
            };
            if redundant {
                out.push(Diagnostic {
                    range,
                    message: base_db::trs!("不必要的拆箱", "Unnecessary unboxing").to_owned(),
                    severity: Severity::Hint,
                    code: DiagnosticCode::UnnecessaryBoxing,
                    unused: true,
                });
            }
        }
    }
}

/// 给快速修复用：诊断和要替换成的文本（装箱换成实参，拆箱删掉 `.xxxValue()`）。
pub fn unnecessary_boxing_fixes(db: &dyn Db, file: SourceFile) -> Vec<(TextRange, String)> {
    let mut diagnostics = Vec::new();
    let view = View::file(db, file);
    unnecessary_boxing(&view, &mut diagnostics);
    diagnostics
        .into_iter()
        .filter_map(|d| {
            if d.unused {
                return Some((d.range, String::new()));
            }
            let node = view
                .root
                .covering_element(d.range)
                .into_node()?
                .ancestors()
                .find(|n| n.text_range() == d.range)?;
            let body = view.body_of(&node)?;
            let result = view.infer(body);
            let arg = boxing_call(db, &result, &node)?;
            Some((d.range, arg.syntax().text().to_string()))
        })
        .collect()
}
