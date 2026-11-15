//! 集合的内容只写不读 / 只读不写（局部变量）：
//!
//! - 往集合里添加、删除，却从来不读它的内容：这些修改都白做了；
//! - 用一个新建的空集合，却从来不往里加东西：查询的结果永远是空的。
//!
//! 只看初值是新建集合（`new ArrayList<>(…)`）的变量。每次使用按方法名归类；集合被当作实参
//! 传出去、被返回、被赋给别的变量、被重新赋值、在匿名类 / 局部类里用到，或者调用了不认识的
//! 方法时，就当作"逃逸"，不再判断。

use base_db::{Db, SourceFile};
use hir::{body_syntax, infer, InferenceResult, LocalKind, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode,
};

use crate::{bodies_in_file, Diagnostic, DiagnosticCode, Severity};

const UPDATES: &[&str] = &[
    "add",
    "addAll",
    "addFirst",
    "addLast",
    "put",
    "putAll",
    "putIfAbsent",
    "removeAll",
    "removeIf",
    "retainAll",
    "clear",
    "set",
    "offer",
    "offerFirst",
    "offerLast",
    "push",
    "compute",
    "computeIfAbsent",
    "computeIfPresent",
    "merge",
    "replaceAll",
    "sort",
];

const QUERIES: &[&str] = &[
    "get",
    "getOrDefault",
    "contains",
    "containsAll",
    "containsKey",
    "containsValue",
    "size",
    "isEmpty",
    "iterator",
    "listIterator",
    "stream",
    "parallelStream",
    "forEach",
    "toArray",
    "indexOf",
    "lastIndexOf",
    "getFirst",
    "getLast",
    "peek",
    "peekFirst",
    "peekLast",
    "element",
    "first",
    "last",
    "equals",
    "hashCode",
    "toString",
];

#[derive(Default)]
struct Usage {
    updated: bool,
    queried: bool,
    escaped: bool,
}

fn is_collection(db: &dyn Db, ty: &Ty) -> bool {
    ["java/util/Collection", "java/util/Map"]
        .iter()
        .any(|name| {
            hir::lookup_class(db, name).is_some_and(|c| hir::find_super(db, ty, c).is_some())
        })
}

/// 初值是新建的空集合（`new ArrayList<>()`、`new HashMap<>(16)`）。
fn fresh_empty(result: &InferenceResult, init: &ast::Expr) -> bool {
    let ast::Expr::NewExpr(new) = init else {
        return false;
    };
    if new
        .syntax()
        .children()
        .any(|c| c.kind() == SyntaxKind::CLASS_BODY)
    {
        return false;
    }
    let Some(args) = new.arg_list() else {
        return false;
    };
    args.args().all(|a| {
        matches!(
            result.type_of(a.syntax().text_range()),
            Some(Ty::Primitive(_))
        )
    })
}

fn classify(expr: &SyntaxNode, usage: &mut Usage) {
    let Some(parent) = expr.parent() else {
        usage.escaped = true;
        return;
    };
    match parent.kind() {
        SyntaxKind::METHOD_CALL => {
            let Some(call) = ast::MethodCall::cast(parent.clone()) else {
                return;
            };
            let is_receiver = call.receiver().map(|r| r.syntax().clone()).as_ref() == Some(expr);
            if !is_receiver {
                usage.escaped = true;
                return;
            }
            let name = call
                .name_ref()
                .map(|n| n.text().to_string())
                .unwrap_or_default();
            // `remove` 的返回值有用时也算读
            let value_used = parent
                .parent()
                .is_none_or(|p| p.kind() != SyntaxKind::EXPR_STMT);
            if name == "remove" {
                usage.updated = true;
                usage.queried |= value_used;
            } else if UPDATES.contains(&name.as_str()) {
                usage.updated = true;
                // 返回值（`if (seen.add(x))`、`put` 返回的旧值）被用到也是一种读
                usage.queried |= value_used;
            } else if QUERIES.contains(&name.as_str()) {
                usage.queried = true;
            } else {
                usage.escaped = true;
            }
        }
        // `for (x : list)`
        SyntaxKind::FOREACH_STMT => {
            let is_iterable = ast::ForeachStmt::cast(parent)
                .and_then(|f| f.iterable())
                .is_some_and(|i| i.syntax() == expr);
            if is_iterable {
                usage.queried = true;
            } else {
                usage.escaped = true;
            }
        }
        // `list == null` / `list != null` 不涉及内容
        SyntaxKind::BINARY_EXPR
            if ast::BinaryExpr::cast(parent.clone()).is_some_and(|b| {
                matches!(b.op_kind(), Some(SyntaxKind::EQ_EQ | SyntaxKind::NEQ))
            }) => {}
        _ => usage.escaped = true,
    }
}

fn check_body(db: &dyn Db, result: &InferenceResult, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
    for (index, local) in result.locals().iter().enumerate() {
        if local.kind != LocalKind::Local || !is_collection(db, &local.ty) {
            continue;
        }
        let Some(name) = node
            .covering_element(local.decl_range)
            .into_node()
            .and_then(ast::Name::cast)
            .or_else(|| {
                node.covering_element(local.decl_range)
                    .parent()
                    .and_then(ast::Name::cast)
            })
        else {
            continue;
        };
        let Some(declarator) = name
            .syntax()
            .parent()
            .and_then(ast::VariableDeclarator::cast)
        else {
            continue;
        };
        if declarator
            .syntax()
            .parent()
            .is_none_or(|p| p.kind() != SyntaxKind::LOCAL_VAR_DECL)
        {
            continue;
        }
        let text = name.text().to_string();
        // 匿名类 / 局部类里的使用不在这个体里，看不到
        if node
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::CLASS_BODY)
            .any(|b| b.text().to_string().contains(&text))
        {
            continue;
        }
        let mut usage = Usage::default();
        // 初值必须是新建的集合：从别处拿来的集合，修改在别处看得到
        let Some(ast::Expr::NewExpr(new)) = declarator.initializer() else {
            continue;
        };
        if new
            .syntax()
            .children()
            .any(|c| c.kind() == SyntaxKind::CLASS_BODY)
        {
            continue;
        }
        let fresh = fresh_empty(result, &ast::Expr::NewExpr(new));
        let mut uses = 0;
        for expr in node.descendants().filter_map(ast::NameExpr::cast) {
            let Some(name_ref) = expr.name_ref() else {
                continue;
            };
            if name_ref.text() != text.as_str() {
                continue;
            }
            let Some(Resolution::Local(id)) = result.resolution(name_ref.syntax().text_range())
            else {
                continue;
            };
            if *id as usize != index {
                continue;
            }
            uses += 1;
            classify(expr.syntax(), &mut usage);
            if usage.escaped {
                break;
            }
        }
        if usage.escaped || uses == 0 {
            continue;
        }
        let message = if usage.updated && !usage.queried {
            base_db::tr!(
                "集合 `{text}` 的内容只被修改，从来没有被读取",
                "Contents of collection `{text}` are updated, but never queried"
            )
        } else if usage.queried && !usage.updated && fresh {
            base_db::tr!(
                "集合 `{text}` 从来没有添加过内容，查询的结果总是空的",
                "Contents of collection `{text}` are queried, but never updated"
            )
        } else {
            continue;
        };
        out.push(Diagnostic {
            range: name.syntax().text_range(),
            message,
            severity: Severity::Warning,
            code: DiagnosticCode::MismatchedCollection,
            unused: false,
        });
    }
}

pub(crate) fn mismatched_collections(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    for body in bodies_in_file(db, file) {
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let result = infer(db, body);
        if !result.locals().iter().any(|l| l.kind == LocalKind::Local) {
            continue;
        }
        check_body(db, &result, &node, out);
    }
}

/// 私有字段：初值是新建的集合，只在这个文件里用到。
pub(crate) fn mismatched_collection_fields(
    db: &dyn Db,
    file: SourceFile,
    out: &mut Vec<Diagnostic>,
) {
    let sema = hir::Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let mut candidates: Vec<(hir::FieldId, ast::Name, bool)> = Vec::new();
    for field_decl in root.descendants().filter_map(ast::FieldDecl::cast) {
        let declarators: Vec<_> = field_decl.declarators().collect();
        let [declarator] = declarators.as_slice() else {
            continue;
        };
        let Some(mods) = field_decl.modifier_list() else {
            continue;
        };
        if !mods.has(SyntaxKind::PRIVATE_KW) || mods.annotations().next().is_some() {
            continue;
        }
        let Some(ast::Expr::NewExpr(new)) = declarator.initializer() else {
            continue;
        };
        if new
            .syntax()
            .children()
            .any(|c| c.kind() == SyntaxKind::CLASS_BODY)
        {
            continue;
        }
        // 带注解的类（框架、Lombok 会读字段）、可序列化的类都不看
        let annotated_class = field_decl.syntax().ancestors().any(|a| {
            ast::TypeDecl::cast(a)
                .and_then(|d| d.modifier_list())
                .is_some_and(|m| m.annotations().next().is_some())
        });
        if annotated_class {
            continue;
        }
        let Some(hir::BodyId::Field(field)) = sema.body_of(file, declarator.syntax()) else {
            continue;
        };
        let serializable = hir::lookup_class(db, "java/io/Serializable")
            .is_some_and(|s| hir::find_super(db, &Ty::class(field.class), s).is_some());
        if serializable {
            continue;
        }
        let ty = hir::field_ty(db, field);
        if !is_collection(db, &ty) {
            continue;
        }
        let Some(name) = declarator.name() else {
            continue;
        };
        let result = infer(db, hir::BodyId::Field(field));
        let fresh = fresh_empty(&result, &ast::Expr::NewExpr(new));
        candidates.push((field, name, fresh));
    }
    if candidates.is_empty() {
        return;
    }
    let mut usages: Vec<Usage> = candidates.iter().map(|_| Usage::default()).collect();
    let mut uses: Vec<usize> = vec![0; candidates.len()];
    for body in bodies_in_file(db, file) {
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for name_ref in node.descendants().filter_map(ast::NameRef::cast) {
            let text = name_ref.text();
            let Some(index) = candidates
                .iter()
                .position(|(_, n, _)| n.text() == text.as_str())
            else {
                continue;
            };
            let result = result.get_or_insert_with(|| infer(db, body));
            let Some(Resolution::Field(found)) = result.resolution(name_ref.syntax().text_range())
            else {
                continue;
            };
            if *found != candidates[index].0 {
                continue;
            }
            let Some(parent) = name_ref.syntax().parent() else {
                continue;
            };
            // `list` 或者 `this.list` / `other.list`
            let expr = match parent.kind() {
                SyntaxKind::NAME_EXPR | SyntaxKind::FIELD_ACCESS => parent,
                _ => {
                    usages[index].escaped = true;
                    continue;
                }
            };
            uses[index] += 1;
            classify(&expr, &mut usages[index]);
        }
    }
    for (((_, name, fresh), usage), count) in candidates.iter().zip(&usages).zip(&uses) {
        if usage.escaped || *count == 0 {
            continue;
        }
        let text = name.text().to_string();
        let message = if usage.updated && !usage.queried {
            base_db::tr!(
                "集合 `{text}` 的内容只被修改，从来没有被读取",
                "Contents of collection `{text}` are updated, but never queried"
            )
        } else if usage.queried && !usage.updated && *fresh {
            base_db::tr!(
                "集合 `{text}` 从来没有添加过内容，查询的结果总是空的",
                "Contents of collection `{text}` are queried, but never updated"
            )
        } else {
            continue;
        };
        out.push(Diagnostic {
            range: name.syntax().text_range(),
            message,
            severity: Severity::Warning,
            code: DiagnosticCode::MismatchedCollection,
            unused: false,
        });
    }
}
