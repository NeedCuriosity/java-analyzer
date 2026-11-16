//! 几种常见的错误写法：
//!
//! - 整数除法的结果用在浮点数里（`double half = 1 / 2;` 得到 0.0）；
//! - 八进制字面量（`int mode = 010;` 是 8，不是 10）；
//! - 覆写了 `equals` 却没有覆写 `hashCode`（相等的对象哈希值不同，放进 `HashMap` / `HashSet` 就出错）；
//! - 调用 `Thread.run()`：在当前线程里执行，多半本意是 `start()`。

use base_db::{Db, SourceFile};
use hir::{class_data, class_header, ClassKind, InferenceResult, PrimitiveType, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

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

/// 文本里可能有八进制字面量：`0` 后面跟着数字，前面不是标识符或数字的一部分。
fn may_have_octal(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(2).enumerate().any(|(i, w)| {
        w[0] == b'0'
            && (w[1].is_ascii_digit() || w[1] == b'_')
            && (i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric()
                    || matches!(bytes[i - 1], b'_' | b'.' | b'$')))
    })
}

/// `text` 是视图里 `region` 这一段的文本。
fn check_octal(view: &View<'_>, region: TextRange, text: &str, out: &mut Vec<Diagnostic>) {
    let root = &view.root;
    // 八进制字面量以 `0` 开头、第二个字符是数字或 `_`：只看这些位置上开始的词法单元，
    // 不必遍历整棵树
    let bytes = text.as_bytes();
    // 前面紧跟着数字、`_`、`$` 或者（字面量后缀之外的）字母时，这个 `0` 一定和前面连成同一个
    // 词法单元（标识符或者数字），不可能是一个整数字面量的开头
    let glued = |prev: u8| {
        prev.is_ascii_digit()
            || matches!(prev, b'_' | b'$')
            || (prev.is_ascii_alphabetic()
                && !matches!(prev, b'l' | b'L' | b'f' | b'F' | b'd' | b'D'))
    };
    let starts = bytes
        .windows(2)
        .enumerate()
        .filter(|(i, w)| {
            w[0] == b'0'
                && (w[1].is_ascii_digit() || w[1] == b'_')
                && (*i == 0 || !glued(bytes[i - 1]))
        })
        .map(|(i, _)| region.start() + TextSize::from(i as u32));
    // 已经看过的词法单元的终点：同一个词法单元（比如很长的字符串）里的其它位置不再查找
    let mut seen_until = TextSize::from(0);
    for start in starts {
        if start < seen_until {
            continue;
        }
        // `covering_element` 每层二分查找；`token_at_offset` 逐个扫描子节点，在很长的数组
        // 初始化式里会退化成平方
        let Some(token) = root
            .covering_element(TextRange::at(start, TextSize::from(1)))
            .into_token()
        else {
            continue;
        };
        seen_until = token.text_range().end();
        if token.text_range().start() != start
            || !view.covers(token.text_range())
            || !matches!(
                token.kind(),
                SyntaxKind::INT_LITERAL | SyntaxKind::LONG_LITERAL
            )
        {
            continue;
        }
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

fn check_equals_hash_code(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    if !view.file_contains("equals") {
        return;
    }
    for decl in view.nodes().filter_map(ast::TypeDecl::cast) {
        if !matches!(
            decl.syntax().kind(),
            SyntaxKind::CLASS_DECL | SyntaxKind::ENUM_DECL
        ) {
            continue;
        }
        // 先在语法上看类体里有没有直接声明 `equals` 方法，没有就不必做语义查询
        let declares_equals = decl.body().is_some_and(|b| {
            b.syntax()
                .children()
                .filter_map(ast::MethodDecl::cast)
                .any(|m| m.name().is_some_and(|n| n.text() == "equals"))
        });
        if !declares_equals {
            continue;
        }
        let Some(class) = view.sema.class_of_decl(view.file, &decl) else {
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
                    .and_then(|c| class_header(db, c))
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
                class_header(db, c.id.class).is_some_and(|d| {
                    d.kind != ClassKind::Interface
                        && hir::method_data(db, c.id)
                            .is_some_and(|m| m.name == missing && m.params.len() == arity)
                })
            })
            .all(|c| {
                class_header(db, c.id.class).is_some_and(|d| &*d.binary_name == "java/lang/Object")
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

pub(crate) fn probable_bugs(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    let region = view.region();
    let text = &view.text()[region];
    if may_have_octal(text) {
        check_octal(view, region, text, out);
    }
    check_equals_hash_code(view, out);
    for body in &view.bodies {
        // 下面只看除法和 `run()` 调用
        let body_text = body.text(view);
        if !body_text.contains('/') && !body_text.contains("run") {
            continue;
        }
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for n in body.node.descendants() {
            match n.kind() {
                SyntaxKind::BINARY_EXPR => {
                    let Some(binary) = ast::BinaryExpr::cast(n) else {
                        continue;
                    };
                    if binary.op_kind() != Some(SyntaxKind::SLASH) {
                        continue;
                    }
                    let result = result.get_or_insert_with(|| body.result(view));
                    check_division(db, result, &binary, out);
                }
                SyntaxKind::METHOD_CALL => {
                    let Some(call) = ast::MethodCall::cast(n) else {
                        continue;
                    };
                    if call.name_ref().is_none_or(|r| r.text() != "run") {
                        continue;
                    }
                    let result = result.get_or_insert_with(|| body.result(view));
                    check_thread_run(db, result, &call, out);
                }
                _ => {}
            }
        }
    }
}

/// 这些检查的诊断（给快速修复用）。
pub fn probable_bug_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    probable_bugs(&View::file(db, file), &mut out);
    out
}
