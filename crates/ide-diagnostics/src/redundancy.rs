//! 几种多余或过时的写法，每个都带着修复用的编辑：
//!
//! - C 风格的数组声明：`int a[]`、`String f()[]`（改成 `int[] a`）；
//! - 多余的 `toString()`：接收者本来就是 `String`，或者用在字符串拼接里（另一边已经是 `String`）；
//! - 可以换成模式变量的强制转换（见 `pattern_vars`）；
//! - 可以用 `getOrDefault` / `putIfAbsent` / `computeIfAbsent` 简化的 `Map` 操作（见 `map_api`）；
//! - 可以改成 try-with-resources 的 `try` / `finally`（见 `try_resources`）；
//! - 多余的 `new String(s)` / `new String()`（字段初值除外：那多半是靠对象身份区分的哨兵值）；
//! - 可以用更直接的 API 写的调用（见 `simplifiable_calls`）；
//! - 裸用的泛型类（见 `raw_types`，只有一部分带修复）；
//! - 几种常见的错误写法（见 `bug_patterns`）。

use base_db::{Db, SourceFile};
use hir::{body_syntax, class_data, infer, InferenceResult, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{Diagnostic, DiagnosticCode, Severity};

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

fn c_style_array(node: &SyntaxNode, root: &SyntaxNode, out: &mut Vec<RedundancyFix>) -> Option<()> {
    // 大多数声明没有自己的 `[`，先排除
    let (brackets, dims) = own_brackets(node)?;
    let ty = match node.kind() {
        SyntaxKind::VARIABLE_DECLARATOR => {
            let parent = node.parent()?;
            // `int a, b[];` 改一个类型会影响别的声明，不处理
            let (ty, count) = match parent.kind() {
                SyntaxKind::LOCAL_VAR_DECL => {
                    let decl = ast::LocalVarDecl::cast(parent)?;
                    (decl.ty(), decl.declarators().count())
                }
                SyntaxKind::FIELD_DECL => {
                    let decl = ast::FieldDecl::cast(parent)?;
                    (decl.ty(), decl.declarators().count())
                }
                _ => return None,
            };
            if count != 1 {
                return None;
            }
            ty
        }
        SyntaxKind::PARAM => {
            let param = ast::Param::cast(node.clone())?;
            if param.is_varargs() {
                return None;
            }
            param.ty()
        }
        SyntaxKind::METHOD_DECL => ast::MethodDecl::cast(node.clone())?.return_type(),
        _ => return None,
    }?;
    if ty.syntax().text() == "var" {
        return None;
    }
    let start = prev_significant_end(brackets.start(), root)?;
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
    Some(())
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

/// 需要类型信息才能判断的候选节点。
enum Pending {
    ToString(ast::MethodCall),
    StringConstructor(ast::NewExpr),
    PatternVariable(ast::IfStmt),
    MapApi(SyntaxNode, crate::map_api::Candidate),
    TryWithResources(ast::TryStmt),
    SimplifiableCall(ast::MethodCall),
}

pub fn redundancy_fixes(db: &dyn Db, file: SourceFile) -> Vec<RedundancyFix> {
    let text: &str = file.text(db);
    let root = base_db::parse(db, file).syntax_node();
    let level = crate::language_level(db, file);
    let patterns = level.is_none_or(|l| l >= 16) && text.contains("instanceof");
    let maps =
        level.is_none_or(|l| l >= 8) && (text.contains("containsKey") || text.contains(".put("));
    let strings = text.contains("toString()") || text.contains("new String");
    let arrays = text.contains(']');
    let resources = level.is_none_or(|l| l >= 7) && text.contains("finally");
    let calls =
        level.is_none_or(|l| l >= 8) && crate::simplifiable_calls::may_have_candidates(text);
    let mut out = Vec::new();
    // 一次遍历：语法上就能判断的直接处理，其余的先收集起来
    let mut pending = Vec::new();
    let mut raw_candidates = Vec::new();
    for node in root.descendants() {
        match node.kind() {
            SyntaxKind::CLASS_TYPE if level.is_none_or(|l| l >= 5) => {
                if crate::raw_types::is_candidate(&node) {
                    raw_candidates.push(node);
                }
            }
            SyntaxKind::VARIABLE_DECLARATOR | SyntaxKind::PARAM | SyntaxKind::METHOD_DECL
                if arrays =>
            {
                c_style_array(&node, &root, &mut out);
            }
            SyntaxKind::METHOD_CALL if strings && is_to_string_call(&node) => {
                pending.extend(ast::MethodCall::cast(node).map(Pending::ToString));
            }
            SyntaxKind::METHOD_CALL if calls => {
                pending.extend(
                    ast::MethodCall::cast(node)
                        .filter(crate::simplifiable_calls::is_candidate)
                        .map(Pending::SimplifiableCall),
                );
            }
            SyntaxKind::NEW_EXPR if strings => {
                let Some(new) = ast::NewExpr::cast(node) else {
                    continue;
                };
                let named_string = matches!(
                    new.ty(),
                    Some(ast::Type::ClassType(t)) if t.name_ref().is_some_and(|n| n.text() == "String")
                );
                if named_string {
                    pending.push(Pending::StringConstructor(new));
                }
            }
            SyntaxKind::TRY_STMT if resources => {
                if let Some(try_stmt) =
                    ast::TryStmt::cast(node).filter(crate::try_resources::is_candidate)
                {
                    pending.push(Pending::TryWithResources(try_stmt));
                }
            }
            SyntaxKind::IF_STMT | SyntaxKind::CONDITIONAL_EXPR => {
                if patterns {
                    if let Some(if_stmt) =
                        ast::IfStmt::cast(node.clone()).filter(crate::pattern_vars::is_candidate)
                    {
                        pending.push(Pending::PatternVariable(if_stmt));
                    }
                }
                if maps {
                    if let Some(candidate) = crate::map_api::candidate(&node) {
                        pending.push(Pending::MapApi(node, candidate));
                    }
                }
            }
            _ => {}
        }
    }
    crate::timing::timed("bug_patterns::check", || {
        crate::bug_patterns::check(db, file, &root, &mut out)
    });
    let sema = hir::Semantics::new(db);
    let mut raw_cache = crate::raw_types::Cache::default();
    for node in &raw_candidates {
        crate::raw_types::check(db, &sema, file, node, &mut raw_cache, &mut out);
    }
    if pending.is_empty() {
        return out;
    }
    for item in pending {
        let node = match &item {
            Pending::ToString(call) => call.syntax().clone(),
            Pending::StringConstructor(new) => new.syntax().clone(),
            Pending::PatternVariable(if_stmt) => if_stmt.syntax().clone(),
            Pending::MapApi(node, _) => node.clone(),
            Pending::TryWithResources(try_stmt) => try_stmt.syntax().clone(),
            Pending::SimplifiableCall(call) => call.syntax().clone(),
        };
        let Some(body) = sema.body_of(file, &node) else {
            continue;
        };
        let result = infer(db, body);
        match item {
            Pending::ToString(call) => {
                redundant_to_string(db, &result, &call, &mut out);
            }
            Pending::StringConstructor(new) => {
                redundant_string_constructor(db, &result, &new, &mut out);
            }
            Pending::PatternVariable(if_stmt) => {
                crate::pattern_vars::check(text, &result, &if_stmt, &mut out);
            }
            Pending::TryWithResources(try_stmt) => {
                crate::try_resources::check(db, text, &result, &try_stmt, &mut out);
            }
            Pending::SimplifiableCall(call) => {
                crate::simplifiable_calls::check(db, &result, &call, &mut out);
            }
            Pending::MapApi(node, candidate) => {
                let Some((_, body_node)) = body_syntax(db, body) else {
                    continue;
                };
                crate::map_api::check(db, &result, &body_node, &node, candidate, text, &mut out);
            }
        }
    }
    out
}

pub(crate) fn redundancies(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    out.extend(redundancy_fixes(db, file).into_iter().map(|f| f.diagnostic));
}
