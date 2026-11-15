//! 可以换成模式变量的强制转换（Java 16+）：
//!
//! ```java
//! if (o instanceof Foo && ready) {
//!     Foo foo = (Foo) o;      // 删掉
//!     ...
//! }
//! // →  if (o instanceof Foo foo && ready) { ... }
//! ```
//!
//! 只看 `if` 条件顶层 `&&` 里的 `x instanceof T`（`x` 是局部变量或形参），then 分支的第一条语句是
//! `T v = (T) x;`。`v` 在条件后面出现时不报（模式变量的作用域会覆盖到那里）。

use base_db::{Db, SourceFile};
use hir::{body_syntax, infer, InferenceResult, Resolution};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use crate::{bodies_in_file, redundancy::RedundancyFix, Diagnostic, DiagnosticCode, Severity};

fn normalized(node: &SyntaxNode) -> String {
    node.text()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn conjuncts(expr: &ast::Expr, out: &mut Vec<ast::Expr>) {
    match expr {
        ast::Expr::ParenExpr(paren) if out.is_empty() => {
            if let Some(inner) = paren.expr() {
                conjuncts(&inner, out);
            }
        }
        ast::Expr::BinaryExpr(binary) if binary.op_kind() == Some(SyntaxKind::AMP_AMP) => {
            if let (Some(lhs), Some(rhs)) = (binary.lhs(), binary.rhs()) {
                conjuncts(&lhs, out);
                conjuncts(&rhs, out);
            }
        }
        other => out.push(other.clone()),
    }
}

/// 整行删除一条语句：行首到语句前只有空白、语句后到行尾只有空白时连同换行一起删。
pub(crate) fn statement_line_range(text: &str, range: TextRange) -> TextRange {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
    if text[line_start..start].trim().is_empty() && text[end..line_end].trim().is_empty() {
        TextRange::new(
            TextSize::from(line_start as u32),
            TextSize::from(line_end as u32),
        )
    } else {
        range
    }
}

fn check_if(
    text: &str,
    result: &InferenceResult,
    if_stmt: &ast::IfStmt,
    out: &mut Vec<RedundancyFix>,
) -> Option<()> {
    let condition = if_stmt.condition()?;
    let ast::Stmt::Block(block) = if_stmt.then_branch()? else {
        return None;
    };
    let first = block.statements().next()?;
    let decl = ast::LocalVarDecl::cast(first.syntax().clone())?;
    if decl
        .modifier_list()
        .is_some_and(|m| m.syntax().text_range().len() > 0.into())
    {
        return None;
    }
    let mut declarators = decl.declarators();
    let (Some(declarator), None) = (declarators.next(), declarators.next()) else {
        return None;
    };
    if declarator.extra_dims() > 0 {
        return None;
    }
    let (Some(name), Some(decl_ty)) = (declarator.name(), decl.ty()) else {
        return None;
    };
    let ast::Expr::CastExpr(cast) = declarator.initializer()? else {
        return None;
    };
    let (Some(cast_ty), Some(ast::Expr::NameExpr(operand))) = (cast.ty(), cast.expr()) else {
        return None;
    };
    let ty_text = normalized(cast_ty.syntax());
    if normalized(decl_ty.syntax()) != ty_text {
        return None;
    }
    let mut parts = Vec::new();
    conjuncts(&condition, &mut parts);
    let index = parts.iter().position(|part| {
        let ast::Expr::InstanceofExpr(test) = part else {
            return false;
        };
        test.pattern().is_none()
            && test
                .ty()
                .is_some_and(|t| normalized(t.syntax()) == ty_text)
            && test
                .expr()
                .is_some_and(|e| matches!(&e, ast::Expr::NameExpr(n) if n.syntax().text() == operand.syntax().text()))
    })?;
    let ast::Expr::InstanceofExpr(test) = &parts[index] else {
        return None;
    };
    // 被测试的必须是局部变量或形参（字段可能在条件的其余部分里被改掉）
    let tested = match test.expr()? {
        ast::Expr::NameExpr(n) => n.name_ref()?,
        _ => return None,
    };
    if !matches!(
        result.resolution(tested.syntax().text_range()),
        Some(Resolution::Local(_))
    ) {
        return None;
    }
    let var = name.text().to_string();
    let operand_text = operand.syntax().text().to_string();
    // 条件后面的部分不能用到这个名字，也不能给被测试的变量赋值
    for later in &parts[index + 1..] {
        let clash = later.syntax().descendants().any(|n| {
            ast::NameRef::cast(n.clone()).is_some_and(|r| r.text() == var.as_str())
                || ast::AssignExpr::cast(n)
                    .and_then(|a| a.lhs())
                    .is_some_and(|l| l.syntax().text() == operand_text.as_str())
        });
        if clash {
            return None;
        }
    }
    let ty_end = test.ty()?.syntax().text_range().end();
    out.push(RedundancyFix {
        diagnostic: Diagnostic {
            range: first.syntax().text_range(),
            message: base_db::tr!(
                "变量 `{var}` 可以换成模式变量",
                "Variable `{var}` can be replaced with a pattern variable"
            ),
            severity: Severity::Hint,
            code: DiagnosticCode::PatternVariable,
            unused: true,
        },
        label: base_db::trs!("换成模式变量", "Replace with pattern variable"),
        edits: vec![
            (TextRange::empty(ty_end), format!(" {var}")),
            (
                statement_line_range(text, first.syntax().text_range()),
                String::new(),
            ),
        ],
    });
    Some(())
}

pub(crate) fn pattern_variables(db: &dyn Db, file: SourceFile, out: &mut Vec<RedundancyFix>) {
    if crate::language_level(db, file).is_some_and(|l| l < 16) {
        return;
    }
    let text: &str = file.text(db);
    if !text.contains("instanceof") {
        return;
    }
    for body in bodies_in_file(db, file) {
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for if_stmt in node.descendants().filter_map(ast::IfStmt::cast) {
            let has_test = if_stmt.condition().is_some_and(|c| {
                c.syntax()
                    .descendants()
                    .any(|n| n.kind() == SyntaxKind::INSTANCEOF_EXPR)
            });
            if !has_test {
                continue;
            }
            let result = result.get_or_insert_with(|| infer(db, body));
            check_if(text, result, &if_stmt, out);
        }
    }
}
