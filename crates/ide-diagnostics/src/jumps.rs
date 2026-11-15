//! 多余的跳转：在方法（构造器、lambda）最后执行的 `return;`，在循环体最后执行的
//! 不带标签的 `continue;`。删掉它们不改变行为。

use syntax::{ast::AstNode, SyntaxKind, SyntaxNode};

use crate::{Diagnostic, DiagnosticCode, Severity};

const LOOPS: &[SyntaxKind] = &[
    SyntaxKind::WHILE_STMT,
    SyntaxKind::DO_WHILE_STMT,
    SyntaxKind::FOR_STMT,
    SyntaxKind::FOREACH_STMT,
];

/// `stmt` 执行完之后，控制流直接到达 `stop` 指定的结构（方法 / 循环）的末尾。
fn falls_to_end(stmt: &SyntaxNode, stop: &dyn Fn(&SyntaxNode) -> bool) -> bool {
    let Some(parent) = stmt.parent() else {
        return false;
    };
    match parent.kind() {
        SyntaxKind::BLOCK => {
            // 必须是块里的最后一条语句
            let last = parent
                .children()
                .filter(|c| c.kind() != SyntaxKind::EMPTY_STMT)
                .last();
            if last.as_ref() != Some(stmt) {
                return false;
            }
            let Some(owner) = parent.parent() else {
                return false;
            };
            if stop(&owner) {
                return true;
            }
            match owner.kind() {
                // 嵌套的块语句
                SyntaxKind::BLOCK => falls_to_end(&parent, stop),
                // if 的分支、synchronized 的体
                SyntaxKind::IF_STMT | SyntaxKind::SYNCHRONIZED_STMT => falls_to_end(&owner, stop),
                _ => false,
            }
        }
        // `else if` 链里作为分支的 if
        SyntaxKind::IF_STMT => falls_to_end(&parent, stop),
        _ => false,
    }
}

pub(crate) fn redundant_jumps(root: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    for node in root.descendants() {
        let (is_redundant, message) = match node.kind() {
            SyntaxKind::RETURN_STMT => {
                // 不带返回值
                if node
                    .children()
                    .any(|c| syntax::ast::Expr::can_cast(c.kind()))
                {
                    continue;
                }
                let at_end = falls_to_end(&node, &|owner| {
                    matches!(
                        owner.kind(),
                        SyntaxKind::METHOD_DECL
                            | SyntaxKind::CONSTRUCTOR_DECL
                            | SyntaxKind::LAMBDA_EXPR
                    )
                });
                (
                    at_end,
                    base_db::trs!("多余的 `return`", "Unnecessary `return` statement"),
                )
            }
            SyntaxKind::CONTINUE_STMT => {
                if node.children().any(|c| c.kind() == SyntaxKind::NAME_REF) {
                    continue;
                }
                let at_end = falls_to_end(&node, &|owner| LOOPS.contains(&owner.kind()));
                (
                    at_end,
                    base_db::trs!("多余的 `continue`", "Unnecessary `continue` statement"),
                )
            }
            _ => continue,
        };
        // 直接写在块里（`if (x) return;` 删掉后就不成语句了）
        if !is_redundant || node.parent().is_none_or(|p| p.kind() != SyntaxKind::BLOCK) {
            continue;
        }
        // try / finally 里的 return 会影响 finally 的执行顺序，保守起见不报
        if node
            .ancestors()
            .take_while(|a| {
                !matches!(
                    a.kind(),
                    SyntaxKind::METHOD_DECL
                        | SyntaxKind::CONSTRUCTOR_DECL
                        | SyntaxKind::LAMBDA_EXPR
                )
            })
            .any(|a| a.kind() == SyntaxKind::TRY_STMT)
        {
            continue;
        }
        out.push(Diagnostic {
            range: node.text_range(),
            message: message.to_owned(),
            severity: Severity::Hint,
            code: DiagnosticCode::RedundantJump,
            unused: true,
        });
    }
}
