//! 几种多余或过时的写法，每个都带着修复用的编辑：
//!
//! - C 风格的数组声明：`int a[]`、`String f()[]`（改成 `int[] a`）；
//! - 多余的 `toString()`：接收者本来就是 `String`，或者用在字符串拼接里（另一边已经是 `String`）；
//! - 可以换成模式变量的强制转换（见 `pattern_vars`）；
//! - 多余的 `new String(s)` / `new String()`（字段初值除外：那多半是靠对象身份区分的哨兵值）。

use base_db::{Db, SourceFile};
use hir::{body_syntax, class_data, infer, InferenceResult, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{bodies_in_file, Diagnostic, DiagnosticCode, Severity};

/// 一个诊断和修复它的编辑（范围 → 新文本）。
pub struct RedundancyFix {
    pub diagnostic: Diagnostic,
    pub label: &'static str,
    pub edits: Vec<(TextRange, String)>,
}

fn is_string(db: &dyn Db, ty: Option<&Ty>) -> bool {
    match ty {
        Some(Ty::Class(class, _)) => {
            class_data(db, *class).is_some_and(|d| d.binary_name == "java/lang/String")
        }
        _ => false,
    }
}

/// 节点自己的（不在子节点里的）`[` `]` token 范围；没有时为 `None`。
fn own_brackets(node: &SyntaxNode) -> Option<(TextRange, usize)> {
    let brackets: Vec<_> = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::L_BRACK | SyntaxKind::R_BRACK))
        .collect();
    let first = brackets.first()?;
    let last = brackets.last()?;
    let dims = brackets
        .iter()
        .filter(|t| t.kind() == SyntaxKind::L_BRACK)
        .count();
    Some((
        TextRange::new(first.text_range().start(), last.text_range().end()),
        dims,
    ))
}

/// 前一个不是空白 / 注释的 token 的结束位置。
fn prev_significant_end(offset: syntax::TextSize, node: &SyntaxNode) -> Option<syntax::TextSize> {
    let mut token = node.token_at_offset(offset).right_biased()?.prev_token();
    while let Some(t) = token {
        if !t.kind().is_trivia() {
            return Some(t.text_range().end());
        }
        token = t.prev_token();
    }
    None
}

fn c_style_arrays(root: &SyntaxNode, out: &mut Vec<RedundancyFix>) {
    for node in root.descendants() {
        let ty = match node.kind() {
            SyntaxKind::VARIABLE_DECLARATOR => {
                let Some(parent) = node.parent() else {
                    continue;
                };
                // `int a, b[];` 改一个类型会影响别的声明，不处理
                let (ty, count) = match parent.kind() {
                    SyntaxKind::LOCAL_VAR_DECL => {
                        let Some(decl) = ast::LocalVarDecl::cast(parent) else {
                            continue;
                        };
                        (decl.ty(), decl.declarators().count())
                    }
                    SyntaxKind::FIELD_DECL => {
                        let Some(decl) = ast::FieldDecl::cast(parent) else {
                            continue;
                        };
                        (decl.ty(), decl.declarators().count())
                    }
                    _ => continue,
                };
                if count != 1 {
                    continue;
                }
                ty
            }
            SyntaxKind::PARAM => {
                let Some(param) = ast::Param::cast(node.clone()) else {
                    continue;
                };
                if param.is_varargs() {
                    continue;
                }
                param.ty()
            }
            SyntaxKind::METHOD_DECL => {
                ast::MethodDecl::cast(node.clone()).and_then(|m| m.return_type())
            }
            _ => continue,
        };
        let Some(ty) = ty else {
            continue;
        };
        if ty.syntax().text() == "var" {
            continue;
        }
        let Some((brackets, dims)) = own_brackets(&node) else {
            continue;
        };
        let Some(start) = prev_significant_end(brackets.start(), root) else {
            continue;
        };
        out.push(RedundancyFix {
            diagnostic: Diagnostic {
                range: brackets,
                message: base_db::trs!(
                    "C 风格的数组声明：把 `[]` 写在类型后面",
                    "C-style array declaration: put `[]` after the type"
                )
                .to_owned(),
                severity: Severity::Hint,
                code: DiagnosticCode::CStyleArray,
                unused: false,
            },
            label: base_db::trs!("把 `[]` 移到类型后面", "Move `[]` to the type"),
            edits: vec![
                (
                    TextRange::empty(ty.syntax().text_range().end()),
                    "[]".repeat(dims),
                ),
                (TextRange::new(start, brackets.end()), String::new()),
            ],
        });
    }
}

/// `.toString()` 的范围（从点号到调用结束）。
fn call_suffix(call: &ast::MethodCall) -> Option<TextRange> {
    let name = call.name_ref()?;
    let dot = name
        .syntax()
        .first_token()?
        .prev_token()
        .filter(|t| t.kind() == SyntaxKind::DOT)?;
    Some(TextRange::new(
        dot.text_range().start(),
        call.syntax().text_range().end(),
    ))
}

fn is_to_string_call(expr: &SyntaxNode) -> bool {
    ast::MethodCall::cast(expr.clone()).is_some_and(|c| {
        c.name_ref().is_some_and(|n| n.text() == "toString")
            && c.arg_list().is_some_and(|a| a.args().next().is_none())
    })
}

fn redundant_to_string(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if !is_to_string_call(call.syntax()) {
        return None;
    }
    let receiver = call.receiver()?;
    if matches!(receiver, ast::Expr::SuperExpr(_)) {
        return None;
    }
    let receiver_ty = result.type_of(receiver.syntax().text_range())?;
    if !matches!(receiver_ty, Ty::Class(..) | Ty::TypeParam(_)) {
        return None;
    }
    let redundant = if is_string(db, Some(receiver_ty)) {
        true
    } else {
        // `"a" + x.toString()`：另一边是 String（而且不是另一个要去掉的 `toString()`）
        let parent = call.syntax().parent()?;
        let binary = ast::BinaryExpr::cast(parent)?;
        if binary.op_kind() != Some(SyntaxKind::PLUS) {
            return None;
        }
        let other = match (binary.lhs(), binary.rhs()) {
            (Some(l), Some(r)) if l.syntax() == call.syntax() => r,
            (Some(l), Some(r)) if r.syntax() == call.syntax() => l,
            _ => return None,
        };
        is_string(db, result.type_of(other.syntax().text_range()))
            && !is_to_string_call(other.syntax())
    };
    if !redundant {
        return None;
    }
    let range = call_suffix(call)?;
    out.push(RedundancyFix {
        diagnostic: Diagnostic {
            range,
            message: base_db::trs!("多余的 `toString()` 调用", "Unnecessary `toString()` call")
                .to_owned(),
            severity: Severity::Hint,
            code: DiagnosticCode::RedundantToString,
            unused: true,
        },
        label: base_db::trs!("删除 `toString()`", "Remove `toString()`"),
        edits: vec![(range, String::new())],
    });
    Some(())
}

fn redundant_string_constructor(
    db: &dyn Db,
    result: &InferenceResult,
    new: &ast::NewExpr,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    if new.body().is_some() || new.qualifier().is_some() {
        return None;
    }
    // 字段初值里的 `new String()` 多半是靠对象身份区分的哨兵值
    let field_initializer = new
        .syntax()
        .parent()
        .filter(|p| p.kind() == SyntaxKind::VARIABLE_DECLARATOR)
        .and_then(|p| p.parent())
        .is_some_and(|p| p.kind() == SyntaxKind::FIELD_DECL);
    if field_initializer {
        return None;
    }
    let ast::Type::ClassType(ty) = new.ty()? else {
        return None;
    };
    if ty.type_arg_list().is_some() || !is_string(db, result.type_of(new.syntax().text_range())) {
        return None;
    }
    let mut args = new.arg_list()?.args();
    let replacement = match (args.next(), args.next()) {
        (None, _) => "\"\"".to_owned(),
        (Some(arg), None) if is_string(db, result.type_of(arg.syntax().text_range())) => {
            let text = arg.syntax().text().to_string();
            let atomic = matches!(
                arg.syntax().kind(),
                SyntaxKind::LITERAL
                    | SyntaxKind::NAME_EXPR
                    | SyntaxKind::FIELD_ACCESS
                    | SyntaxKind::METHOD_CALL
                    | SyntaxKind::PAREN_EXPR
                    | SyntaxKind::ARRAY_ACCESS
                    | SyntaxKind::THIS_EXPR
            );
            let loose_context = new.syntax().parent().is_none_or(|p| {
                !matches!(
                    p.kind(),
                    SyntaxKind::VARIABLE_DECLARATOR
                        | SyntaxKind::ARG_LIST
                        | SyntaxKind::RETURN_STMT
                        | SyntaxKind::ASSIGN_EXPR
                        | SyntaxKind::PAREN_EXPR
                        | SyntaxKind::EXPR_STMT
                )
            });
            if !atomic && loose_context {
                format!("({text})")
            } else {
                text
            }
        }
        _ => return None,
    };
    out.push(RedundancyFix {
        diagnostic: Diagnostic {
            range: new.syntax().text_range(),
            message: base_db::trs!(
                "多余的 `String` 构造器调用",
                "Redundant `String` constructor call"
            )
            .to_owned(),
            severity: Severity::Hint,
            code: DiagnosticCode::RedundantStringConstructor,
            unused: false,
        },
        label: base_db::trs!("删除构造器调用", "Remove constructor call"),
        edits: vec![(new.syntax().text_range(), replacement)],
    });
    Some(())
}

pub fn redundancy_fixes(db: &dyn Db, file: SourceFile) -> Vec<RedundancyFix> {
    let text: &str = file.text(db);
    let root = base_db::parse(db, file).syntax_node();
    let mut out = Vec::new();
    if text.contains(']') {
        c_style_arrays(&root, &mut out);
    }
    crate::pattern_vars::pattern_variables(db, file, &mut out);
    if !text.contains("toString()") && !text.contains("new String") {
        return out;
    }
    for body in bodies_in_file(db, file) {
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for expr in node.descendants() {
            match expr.kind() {
                SyntaxKind::METHOD_CALL if is_to_string_call(&expr) => {
                    let result = result.get_or_insert_with(|| infer(db, body));
                    if let Some(call) = ast::MethodCall::cast(expr) {
                        redundant_to_string(db, result, &call, &mut out);
                    }
                }
                SyntaxKind::NEW_EXPR => {
                    let Some(new) = ast::NewExpr::cast(expr) else {
                        continue;
                    };
                    let named_string = new
                        .ty()
                        .is_some_and(|t| t.syntax().text().to_string().ends_with("String"));
                    if !named_string {
                        continue;
                    }
                    let result = result.get_or_insert_with(|| infer(db, body));
                    redundant_string_constructor(db, result, &new, &mut out);
                }
                _ => {}
            }
        }
    }
    out
}

pub(crate) fn redundancies(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    out.extend(redundancy_fixes(db, file).into_iter().map(|f| f.diagnostic));
}
