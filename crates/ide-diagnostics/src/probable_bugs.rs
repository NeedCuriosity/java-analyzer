//! 几种常见的错误写法：
//!
//! - 整数除法的结果用在浮点数里（`double half = 1 / 2;` 得到 0.0）；
//! - 八进制字面量（`int mode = 010;` 是 8，不是 10）；
//! - 覆写了 `equals` 却没有覆写 `hashCode`（相等的对象哈希值不同，放进 `HashMap` / `HashSet` 就出错）；
//! - 调用 `Thread.run()`：在当前线程里执行，多半本意是 `start()`。

use base_db::{Db, SourceFile};
use hir::{
    body_syntax, class_data, infer, ClassKind, InferenceResult, PrimitiveType, Resolution, Ty,
};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode,
};

use crate::{bodies_in_file, Diagnostic, DiagnosticCode, Severity};

fn primitive(db: &dyn Db, ty: Option<&Ty>) -> Option<PrimitiveType> {
    match ty? {
        Ty::Primitive(p) => Some(*p),
        other => other.unboxed(db),
    }
}

fn is_integral(p: Option<PrimitiveType>) -> bool {
    matches!(
        p,
        Some(
            PrimitiveType::Byte
                | PrimitiveType::Short
                | PrimitiveType::Char
                | PrimitiveType::Int
                | PrimitiveType::Long
        )
    )
}

fn is_floating(p: Option<PrimitiveType>) -> bool {
    matches!(p, Some(PrimitiveType::Float | PrimitiveType::Double))
}

/// 整数除法的值被当成浮点数用：外层（跳过括号）是浮点运算、转成浮点的强制转换、
/// 浮点变量的初值或赋值。
fn used_as_floating(db: &dyn Db, result: &InferenceResult, division: &SyntaxNode) -> bool {
    let mut node = division.clone();
    while let Some(parent) = node.parent() {
        if parent.kind() != SyntaxKind::PAREN_EXPR {
            break;
        }
        node = parent;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    let type_of = |n: &SyntaxNode| primitive(db, result.type_of(n.text_range()));
    match parent.kind() {
        SyntaxKind::BINARY_EXPR => {
            let arithmetic = ast::BinaryExpr::cast(parent.clone()).is_some_and(|b| {
                matches!(
                    b.op_kind(),
                    Some(
                        SyntaxKind::PLUS | SyntaxKind::MINUS | SyntaxKind::STAR | SyntaxKind::SLASH
                    )
                )
            });
            arithmetic && is_floating(type_of(&parent))
        }
        SyntaxKind::CAST_EXPR => is_floating(type_of(&parent)),
        SyntaxKind::ASSIGN_EXPR => {
            let assign = ast::AssignExpr::cast(parent.clone());
            let is_rhs = assign
                .as_ref()
                .and_then(|a| a.rhs())
                .is_some_and(|r| r.syntax() == &node);
            let plain = assign
                .and_then(|a| a.op_token())
                .is_some_and(|t| t.kind() == SyntaxKind::EQ);
            is_rhs && plain && is_floating(type_of(&parent))
        }
        SyntaxKind::VARIABLE_DECLARATOR => {
            let Some(name) = ast::VariableDeclarator::cast(parent).and_then(|d| d.name()) else {
                return false;
            };
            result
                .local_at_decl(name.syntax().text_range())
                .is_some_and(|id| is_floating(primitive(db, Some(&result.local(id).ty))))
        }
        _ => false,
    }
}

fn check_division(
    db: &dyn Db,
    result: &InferenceResult,
    binary: &ast::BinaryExpr,
    out: &mut Vec<Diagnostic>,
) {
    if binary.op_kind() != Some(SyntaxKind::SLASH) {
        return;
    }
    let (Some(lhs), Some(rhs)) = (binary.lhs(), binary.rhs()) else {
        return;
    };
    let integral =
        |e: &ast::Expr| is_integral(primitive(db, result.type_of(e.syntax().text_range())));
    if !integral(&lhs) || !integral(&rhs) {
        return;
    }
    if used_as_floating(db, result, binary.syntax()) {
        out.push(Diagnostic {
            range: binary.syntax().text_range(),
            message: base_db::trs!(
                "整数除法的结果用在浮点数里：小数部分已经被舍掉了",
                "Integer division in a floating-point context: the fractional part is discarded"
            )
            .to_owned(),
            severity: Severity::Warning,
            code: DiagnosticCode::IntegerDivision,
            unused: false,
        });
    }
}

fn check_thread_run(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<Diagnostic>,
) -> Option<()> {
    let name = call.name_ref()?;
    if name.text() != "run" || call.arg_list()?.args().next().is_some() {
        return None;
    }
    // `super.run()` 是正常的
    let receiver = call.receiver()?;
    if matches!(receiver, ast::Expr::SuperExpr(_) | ast::Expr::ThisExpr(_)) {
        return None;
    }
    let Resolution::Method(_) = result.resolution(name.syntax().text_range())? else {
        return None;
    };
    let ty = result.type_of(receiver.syntax().text_range())?;
    let thread = hir::lookup_class(db, "java/lang/Thread")?;
    hir::find_super(db, ty, thread)?;
    out.push(Diagnostic {
        range: name.syntax().text_range(),
        message: base_db::trs!(
            "`Thread.run()` 在当前线程里执行，要启动新线程应当调用 `start()`",
            "`Thread.run()` runs in the current thread; call `start()` to start a new thread"
        )
        .to_owned(),
        severity: Severity::Warning,
        code: DiagnosticCode::ThreadRun,
        unused: false,
    });
    Some(())
}

fn check_octal(root: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    for token in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::INT_LITERAL | SyntaxKind::LONG_LITERAL))
    {
        let text = token.text();
        let digits = text.trim_end_matches(['l', 'L']).replace('_', "");
        let octal = digits.len() > 1
            && digits.starts_with('0')
            && digits[1..].chars().all(|c| c.is_ascii_digit());
        if octal {
            out.push(Diagnostic {
                range: token.text_range(),
                message: base_db::tr!("`{text}` 是八进制字面量", "`{text}` is an octal literal"),
                severity: Severity::Warning,
                code: DiagnosticCode::OctalLiteral,
                unused: false,
            });
        }
    }
}

fn check_equals_hash_code(
    db: &dyn Db,
    file: SourceFile,
    root: &SyntaxNode,
    out: &mut Vec<Diagnostic>,
) {
    let sema = hir::Semantics::new(db);
    for decl in root.descendants().filter_map(ast::TypeDecl::cast) {
        if !matches!(
            decl.syntax().kind(),
            SyntaxKind::CLASS_DECL | SyntaxKind::ENUM_DECL
        ) {
            continue;
        }
        let Some(class) = sema.class_of_decl(file, &decl) else {
            continue;
        };
        let Some(data) = class_data(db, class) else {
            continue;
        };
        let object = |name: &str, params: usize| {
            data.methods.iter().enumerate().any(|(idx, m)| {
                if m.is_constructor
                    || m.name != name
                    || m.params.len() != params
                    || m.flags.is_static()
                {
                    return false;
                }
                // `equals(Object)`，不是 `equals(Point)` 这种重载
                params == 0
                    || hir::method_sig(
                        db,
                        hir::MethodId {
                            class,
                            idx: idx as u32,
                        },
                    )
                    .params
                    .first()
                    .and_then(Ty::as_class)
                    .and_then(|c| class_data(db, c))
                    .is_some_and(|d| &*d.binary_name == "java/lang/Object")
            })
        };
        let equals = object("equals", 1);
        let hash_code = object("hashCode", 0);
        // 只覆写 `hashCode` 而保留按引用比较的 `equals` 并不违反约定，不报
        if !equals || hash_code {
            continue;
        }
        // `hashCode` 要是从 `Object` 继承来的才有问题（父类可能已经按内容实现了）；
        // 接口里的抽象声明不算
        let (missing, arity) = ("hashCode", 0);
        let inherited_from_object = hir::all_methods(db, &Ty::class(class))
            .iter()
            .filter(|c| {
                class_data(db, c.id.class).is_some_and(|d| {
                    d.kind != ClassKind::Interface
                        && d.methods
                            .get(c.id.idx as usize)
                            .is_some_and(|m| m.name == missing && m.params.len() == arity)
                })
            })
            .all(|c| {
                class_data(db, c.id.class).is_some_and(|d| &*d.binary_name == "java/lang/Object")
            });
        if !inherited_from_object {
            continue;
        }
        let Some(name) = decl.name() else { continue };
        let class_name = name.text().to_string();
        let message = base_db::tr!(
            "`{class_name}` 覆写了 `equals()` 却没有覆写 `hashCode()`",
            "`{class_name}` overrides `equals()` but not `hashCode()`"
        );
        out.push(Diagnostic {
            range: name.syntax().text_range(),
            message,
            severity: Severity::Warning,
            code: DiagnosticCode::EqualsHashCode,
            unused: false,
        });
    }
}

pub(crate) fn probable_bugs(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    let root = base_db::parse(db, file).syntax_node();
    check_octal(&root, out);
    check_equals_hash_code(db, file, &root, out);
    for body in bodies_in_file(db, file) {
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for n in node.descendants() {
            match n.kind() {
                SyntaxKind::BINARY_EXPR => {
                    let Some(binary) = ast::BinaryExpr::cast(n) else {
                        continue;
                    };
                    if binary.op_kind() != Some(SyntaxKind::SLASH) {
                        continue;
                    }
                    let result = result.get_or_insert_with(|| infer(db, body));
                    check_division(db, result, &binary, out);
                }
                SyntaxKind::METHOD_CALL => {
                    let Some(call) = ast::MethodCall::cast(n) else {
                        continue;
                    };
                    if call.name_ref().is_none_or(|r| r.text() != "run") {
                        continue;
                    }
                    let result = result.get_or_insert_with(|| infer(db, body));
                    check_thread_run(db, result, &call, out);
                }
                _ => {}
            }
        }
    }
}
