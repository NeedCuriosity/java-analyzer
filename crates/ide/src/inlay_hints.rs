//! 内嵌提示：`var` 的推断类型、字面量实参对应的形参名。

use base_db::{Db, SourceFile};
use hir::{class_data, Definition, Semantics};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange, TextSize,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlayKind {
    /// 显示在名字后面的类型，例如 `var list`**`: List<String>`**
    Type,
    /// 显示在实参前面的形参名，例如 `foo(`**`timeout:`**` 30)`
    Parameter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub offset: TextSize,
    pub label: String,
    pub kind: InlayKind,
}

fn is_var(ty: &ast::Type) -> bool {
    matches!(ty, ast::Type::ClassType(class_ty)
        if class_ty.qualifier().is_none() && class_ty.name_ref().is_some_and(|n| n.text() == "var"))
}

/// 类型在初值里已经一目了然（`var x = new Foo()`、`var s = "..."`、强制转换）时不再提示。
fn type_is_obvious(initializer: Option<ast::Expr>) -> bool {
    matches!(
        initializer,
        Some(
            ast::Expr::NewExpr(_)
                | ast::Expr::Literal(_)
                | ast::Expr::CastExpr(_)
                | ast::Expr::ArrayCreationExpr(_)
        )
    )
}

fn type_hint(sema: &Semantics<'_>, file: SourceFile, name: &ast::Name, out: &mut Vec<InlayHint>) {
    let Some(Definition::Local(local)) = sema.classify_name(file, name) else {
        return;
    };
    let ty = hir::infer(sema.db, local.body).local(local.id).ty.clone();
    if ty.contains_unknown() {
        return;
    }
    out.push(InlayHint {
        offset: name.syntax().text_range().end(),
        label: format!(": {}", ty.display(sema.db)),
        kind: InlayKind::Type,
    });
}

/// 实参是否"看不出含义"：字面量（含负数）、`null`、`true` / `false`。
fn is_opaque_argument(arg: &ast::Expr) -> bool {
    match arg {
        ast::Expr::Literal(_) => true,
        ast::Expr::UnaryExpr(unary) => matches!(unary.operand(), Some(ast::Expr::Literal(_))),
        _ => false,
    }
}

fn parameter_hints(
    db: &dyn Db,
    sema: &Semantics<'_>,
    file: SourceFile,
    arg_list: &ast::ArgList,
    out: &mut Vec<InlayHint>,
) {
    let args: Vec<ast::Expr> = arg_list.args().collect();
    if !args.iter().any(is_opaque_argument) {
        return;
    }
    let Some(body) = sema.body_of(file, arg_list.syntax()) else {
        return;
    };
    let result = hir::infer(db, body);
    let Some(method) = result
        .call_info(arg_list.syntax().text_range())
        .and_then(|c| c.chosen)
    else {
        return;
    };
    let Some(data) = class_data(db, method.class) else {
        return;
    };
    let Some(m) = data.methods.get(method.idx as usize) else {
        return;
    };
    // 只有一个形参的方法（`setName("x")`、`add(1)`）含义通常已经在方法名里了
    if m.params.len() < 2 {
        return;
    }
    let last = m.params.len() - 1;
    for (idx, arg) in args.iter().enumerate() {
        if !is_opaque_argument(arg) {
            continue;
        }
        // 变长参数：多出来的实参都属于最后一个形参，只在第一个上提示
        if idx > last && !m.is_varargs() {
            break;
        }
        if idx > last {
            continue;
        }
        let Some(name) = m.params[idx].name.as_ref() else {
            continue;
        };
        // class 文件没有保留形参名时得到的是 `arg0` 之类的占位名
        let placeholder = name
            .strip_prefix("arg")
            .or_else(|| name.strip_prefix("p"))
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
        if placeholder {
            continue;
        }
        let label = if idx == last && m.is_varargs() {
            format!("...{name}:")
        } else {
            format!("{name}:")
        };
        out.push(InlayHint {
            offset: arg.syntax().text_range().start(),
            label,
            kind: InlayKind::Parameter,
        });
    }
}

pub(crate) fn inlay_hints(
    db: &dyn Db,
    file: SourceFile,
    range: Option<TextRange>,
) -> Vec<InlayHint> {
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let mut out = Vec::new();

    for (count, node) in root.descendants().enumerate() {
        if count % 1024 == 0 {
            db.unwind_if_revision_cancelled();
        }
        if range.is_some_and(|r| r.intersect(node.text_range()).is_none()) {
            continue;
        }
        match node.kind() {
            SyntaxKind::LOCAL_VAR_DECL => {
                let Some(decl) = ast::LocalVarDecl::cast(node) else {
                    continue;
                };
                if !decl.ty().is_some_and(|ty| is_var(&ty)) {
                    continue;
                }
                for declarator in decl.declarators() {
                    if type_is_obvious(declarator.initializer()) {
                        continue;
                    }
                    if let Some(name) = declarator.name() {
                        type_hint(&sema, file, &name, &mut out);
                    }
                }
            }
            // `for (var x : xs)`、`try (var r = ...)`
            SyntaxKind::PARAM => {
                let Some(param) = ast::Param::cast(node.clone()) else {
                    continue;
                };
                let in_foreach = node
                    .parent()
                    .is_some_and(|p| p.kind() == SyntaxKind::FOREACH_STMT);
                if in_foreach && param.ty().is_some_and(|ty| is_var(&ty)) {
                    if let Some(name) = param.name() {
                        type_hint(&sema, file, &name, &mut out);
                    }
                }
            }
            SyntaxKind::ARG_LIST => {
                if let Some(arg_list) = ast::ArgList::cast(node) {
                    parameter_hints(db, &sema, file, &arg_list, &mut out);
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|hint| hint.offset);
    out
}
