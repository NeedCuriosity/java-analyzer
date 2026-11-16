//! 封装字段：把一个非私有的实例字段改成 `private`，生成 getter / setter（已有同名的就直接用），
//! 并把类外对它的读写改成调用访问器。
//!
//! 写操作只在语句位置上改写（`o.f = v;` → `o.setF(v);`，`o.f += v;` / `o.f++;` →
//! `o.setF(o.getF() + v);`，后两种要求接收者重复求值没有区别）；遇到改写不了的用法时不提供。

use hir::{class_data, ClassKind, Definition};
use ide_db::{search::find_references, TextEdit};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use super::{capitalize, Assist, AssistCtx, AssistKind};
use crate::{FileId, SourceChange};

fn simple_receiver(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::NameExpr(_) | ast::Expr::ThisExpr(_) | ast::Expr::SuperExpr(_) => true,
        ast::Expr::FieldAccess(a) => a.receiver().is_some_and(|r| simple_receiver(&r)),
        _ => false,
    }
}

fn is_statement(node: &SyntaxNode) -> bool {
    node.parent()
        .is_some_and(|p| p.kind() == SyntaxKind::EXPR_STMT)
}

/// 一处引用的改写（几处互不重叠的编辑）；`None` 表示改不了。赋值只改左边和右边的外围，
/// 右边里面对同一个字段的读另有自己的改写。
fn rewrite(reference: &SyntaxNode, getter: &str, setter: Option<&str>) -> Option<Vec<TextEdit>> {
    let name_ref = reference.ancestors().find_map(ast::NameRef::cast)?;
    let parent = name_ref.syntax().parent()?;
    // `recv.f` 或者简单名 `f`
    let (expr, prefix, receiver) = match parent.kind() {
        SyntaxKind::FIELD_ACCESS => {
            let access = ast::FieldAccess::cast(parent.clone())?;
            let receiver = access.receiver()?;
            (
                parent,
                format!("{}.", receiver.syntax().text()),
                Some(receiver),
            )
        }
        SyntaxKind::NAME_EXPR => (parent, String::new(), None),
        _ => return None,
    };
    let read = format!("{prefix}{getter}()");
    let Some(outer) = expr.parent() else {
        return Some(vec![TextEdit::replace(expr.text_range(), read)]);
    };
    let repeatable = receiver.as_ref().is_none_or(simple_receiver);
    match outer.kind() {
        SyntaxKind::ASSIGN_EXPR => {
            let assign = ast::AssignExpr::cast(outer.clone())?;
            if assign.lhs()?.syntax() != &expr {
                return Some(vec![TextEdit::replace(expr.text_range(), read)]);
            }
            let setter = setter?;
            if !is_statement(&outer) {
                return None;
            }
            let rhs = assign.rhs()?;
            let op = assign.op_token()?;
            // 右边不是简单表达式时加括号：`o.setF(o.getF() * (a + b))`
            let atomic = matches!(
                rhs,
                ast::Expr::Literal(_)
                    | ast::Expr::NameExpr(_)
                    | ast::Expr::FieldAccess(_)
                    | ast::Expr::MethodCall(_)
                    | ast::Expr::ParenExpr(_)
            );
            let (open, close) = if atomic { ("", "") } else { ("(", ")") };
            let head = if op.kind() == SyntaxKind::EQ {
                format!("{prefix}{setter}(")
            } else {
                if !repeatable {
                    return None;
                }
                let binary = op.text().strip_suffix('=')?;
                format!("{prefix}{setter}({read} {binary} {open}")
            };
            let tail = if op.kind() == SyntaxKind::EQ {
                ")".to_owned()
            } else {
                format!("{close})")
            };
            Some(vec![
                TextEdit::replace(
                    TextRange::new(
                        outer.text_range().start(),
                        rhs.syntax().text_range().start(),
                    ),
                    head,
                ),
                TextEdit::insert(rhs.syntax().text_range().end(), tail),
            ])
        }
        SyntaxKind::POSTFIX_EXPR | SyntaxKind::UNARY_EXPR => {
            let op = outer
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| matches!(t.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS));
            let Some(op) = op else {
                // `-o.f`、`!o.f` 之类的读
                return Some(vec![TextEdit::replace(expr.text_range(), read)]);
            };
            let setter = setter?;
            if !is_statement(&outer) || !repeatable {
                return None;
            }
            let sign = if op.kind() == SyntaxKind::PLUS_PLUS {
                "+"
            } else {
                "-"
            };
            Some(vec![TextEdit::replace(
                outer.text_range(),
                format!("{prefix}{setter}({read} {sign} 1)"),
            )])
        }
        _ => Some(vec![TextEdit::replace(expr.text_range(), read)]),
    }
}

pub(super) fn encapsulate_field(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(name) = token.parent().and_then(ast::Name::cast) else {
        return;
    };
    let Some(declarator) = name
        .syntax()
        .parent()
        .and_then(ast::VariableDeclarator::cast)
    else {
        return;
    };
    let Some(field_decl) = declarator.syntax().parent().and_then(ast::FieldDecl::cast) else {
        return;
    };
    if field_decl.declarators().count() != 1 {
        return;
    }
    let mods = field_decl.modifier_list();
    let has = |kind| mods.as_ref().is_some_and(|m| m.has(kind));
    if has(SyntaxKind::PRIVATE_KW) || has(SyntaxKind::STATIC_KW) {
        return;
    }
    let is_final = has(SyntaxKind::FINAL_KW);
    let Some(decl) = field_decl
        .syntax()
        .ancestors()
        .find_map(ast::TypeDecl::cast)
    else {
        return;
    };
    let Some(class) = ctx.sema.class_of_decl(ctx.file, &decl) else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    if !matches!(data.kind, ClassKind::Class | ClassKind::Enum) {
        return;
    }
    let Some(field) = ctx.sema.field_of_decl(ctx.file, declarator.syntax()) else {
        return;
    };
    let Some(ty) = field_decl.ty() else {
        return;
    };
    let ty_text = format!(
        "{}{}",
        ty.syntax().text(),
        "[]".repeat(declarator.extra_dims())
    );
    let field_name = name.text().to_string();
    let property = capitalize(&field_name);
    let getter = if ty_text == "boolean" {
        format!("is{property}")
    } else {
        format!("get{property}")
    };
    let setter = format!("set{property}");
    let has_method = |name: &str, arity: usize| {
        data.methods
            .iter()
            .any(|m| !m.is_constructor && m.name == name && m.params.len() == arity)
    };

    // 类外的引用全部改成调用访问器
    let class_range = decl.syntax().text_range();
    let mut edits: Vec<(FileId, TextEdit)> = Vec::new();
    for reference in find_references(ctx.db, &Definition::Field(field)) {
        if reference.file_id == ctx.file_id && class_range.contains_range(reference.range) {
            continue;
        }
        let Some(file) = base_db::source_file(ctx.db, reference.file_id) else {
            return;
        };
        let root = base_db::parse(ctx.db, file).syntax_node();
        let node = match root.covering_element(reference.range) {
            syntax::NodeOrToken::Node(n) => n,
            syntax::NodeOrToken::Token(t) => match t.parent() {
                Some(n) => n,
                None => return,
            },
        };
        let setter_name = (!is_final).then_some(setter.as_str());
        match rewrite(&node, &getter, setter_name) {
            Some(list) => edits.extend(list.into_iter().map(|e| (reference.file_id, e))),
            None => return,
        }
    }

    // 字段改成 private
    let visibility = mods.as_ref().and_then(|m| {
        m.keywords()
            .find(|t| matches!(t.kind(), SyntaxKind::PUBLIC_KW | SyntaxKind::PROTECTED_KW))
    });
    let first_keyword = mods.as_ref().and_then(|m| m.keywords().next());
    let privatize = match (visibility, first_keyword) {
        (Some(v), _) => TextEdit::replace(v.text_range(), "private"),
        (None, Some(k)) => TextEdit::insert(k.text_range().start(), "private "),
        (None, None) => TextEdit::insert(ty.syntax().text_range().start(), "private "),
    };
    let mut members = Vec::new();
    if !has_method(&getter, 0) {
        members.push(vec![
            format!("public {ty_text} {getter}() {{"),
            format!("    return {field_name};"),
            "}".to_owned(),
        ]);
    }
    if !is_final && !has_method(&setter, 1) {
        members.push(vec![
            format!("public void {setter}({ty_text} {field_name}) {{"),
            format!("    this.{field_name} = {field_name};"),
            "}".to_owned(),
        ]);
    }
    edits.push((ctx.file_id, privatize));
    if !members.is_empty() {
        let Some(insert) = ctx.insert_members(&decl, &members) else {
            return;
        };
        edits.push((ctx.file_id, insert));
    }
    let mut change = SourceChange::default();
    edits.sort_by_key(|(file_id, edit)| (*file_id, edit.range.start()));
    for (file_id, edit) in edits {
        match change.edits.last_mut() {
            Some((last, list)) if *last == file_id => list.push(edit),
            _ => change.edits.push((file_id, vec![edit])),
        }
    }
    out.push(Assist {
        id: "encapsulate_field",
        label: base_db::tr!(
            "封装字段 `{field_name}`",
            "Encapsulate field `{field_name}`"
        ),
        kind: AssistKind::RefactorRewrite,
        change,
    });
}
