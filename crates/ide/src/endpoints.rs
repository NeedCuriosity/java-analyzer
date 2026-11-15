//! Web 端点：Spring MVC（`@RequestMapping`、`@GetMapping` …）和 JAX-RS（`@Path` + `@GET` …）。
//!
//! 只看注解的写法，不依赖 Spring 的类在不在类路径上。端点以 `GET /api/users/{id}` 的形式
//! 出现在工作区符号搜索里：查询里带 `/` 时按路径匹配。

use base_db::{workspace, Db};
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode,
};

use crate::{
    navigation::{NavFile, NavigationTarget},
    SymbolKind,
};

/// 注解的简单名和它的实参。
fn annotations(node: &SyntaxNode) -> Vec<(String, ast::Annotation)> {
    node.children()
        .find_map(ast::ModifierList::cast)
        .map(|list| {
            list.annotations()
                .filter_map(|a| {
                    let name = a.name()?.segments().last()?.to_string();
                    Some((name, a))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_literals(node: &SyntaxNode) -> Vec<String> {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::STRING_LITERAL)
        .map(|t| t.text().trim_matches('"').to_owned())
        .collect()
}

/// 注解里的路径：`("/x")`、`(value = "/x")`、`(path = {"/x", "/y"})`。没写时是空串。
fn paths_of(annotation: &ast::Annotation) -> Vec<String> {
    let Some(args) = annotation.arg_list() else {
        return vec![String::new()];
    };
    let named: Vec<ast::AnnotationArg> = args.args().collect();
    let mut paths = Vec::new();
    if named.iter().all(|arg| arg.name_ref().is_none()) {
        paths = string_literals(args.syntax());
    } else {
        for arg in &named {
            let key = arg.name_ref().map(|n| n.text().to_string());
            if matches!(key.as_deref(), Some("value" | "path")) {
                paths.extend(string_literals(arg.syntax()));
            }
        }
    }
    if paths.is_empty() {
        paths.push(String::new());
    }
    paths
}

/// `@RequestMapping(method = RequestMethod.POST)` 里的 HTTP 方法。
fn request_methods(annotation: &ast::Annotation) -> Vec<String> {
    let Some(args) = annotation.arg_list() else {
        return Vec::new();
    };
    args.args()
        .filter(|arg| arg.name_ref().is_some_and(|n| n.text() == "method"))
        .flat_map(|arg| {
            arg.syntax()
                .descendants()
                .filter_map(ast::NameRef::cast)
                .map(|n| n.text().to_string())
                .filter(|n| {
                    matches!(
                        n.as_str(),
                        "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn join_path(prefix: &str, path: &str) -> String {
    let joined = format!("/{}/{}", prefix.trim_matches('/'), path.trim_matches('/'));
    let mut out = String::new();
    for ch in joined.chars() {
        if ch == '/' && out.ends_with('/') {
            continue;
        }
        out.push(ch);
    }
    if out.len() > 1 {
        out.truncate(out.trim_end_matches('/').len());
    }
    out
}

fn spring_verb(annotation: &str) -> Option<&'static str> {
    Some(match annotation {
        "GetMapping" => "GET",
        "PostMapping" => "POST",
        "PutMapping" => "PUT",
        "DeleteMapping" => "DELETE",
        "PatchMapping" => "PATCH",
        _ => return None,
    })
}

fn jaxrs_verb(annotation: &str) -> Option<&'static str> {
    Some(match annotation {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "DELETE" => "DELETE",
        "PATCH" => "PATCH",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => return None,
    })
}

/// 工作区里的全部端点。
pub(crate) fn endpoints(db: &dyn Db) -> Vec<NavigationTarget> {
    let mut res = Vec::new();
    for (file_id, file) in workspace(db).files(db).workspace_files() {
        let text: &str = file.text(db);
        // 绝大多数文件没有端点：先按文本筛一遍，不解析
        if !text.contains("Mapping") && !text.contains("@Path") {
            continue;
        }
        db.unwind_if_revision_cancelled();
        let root = base_db::parse(db, file).syntax_node();
        for decl in root.descendants().filter_map(ast::TypeDecl::cast) {
            let class_annotations = annotations(decl.syntax());
            let prefixes: Vec<String> = class_annotations
                .iter()
                .find(|(name, _)| name == "RequestMapping" || name == "Path")
                .map(|(_, a)| paths_of(a))
                .unwrap_or_else(|| vec![String::new()]);
            let class_name = decl.name().map(|n| n.text().to_string());
            let Some(body) = decl.body() else { continue };
            for method in body.syntax().children().filter_map(ast::MethodDecl::cast) {
                let Some(name) = method.name() else { continue };
                let method_annotations = annotations(method.syntax());
                let mut verbs: Vec<String> = Vec::new();
                let mut paths: Vec<String> = Vec::new();
                for (annotation_name, annotation) in &method_annotations {
                    if let Some(verb) = spring_verb(annotation_name) {
                        verbs.push(verb.to_owned());
                        paths = paths_of(annotation);
                    } else if annotation_name == "RequestMapping" {
                        let methods = request_methods(annotation);
                        if methods.is_empty() {
                            verbs.push("ANY".to_owned());
                        } else {
                            verbs.extend(methods);
                        }
                        paths = paths_of(annotation);
                    } else if let Some(verb) = jaxrs_verb(annotation_name) {
                        verbs.push(verb.to_owned());
                    } else if annotation_name == "Path" {
                        paths = paths_of(annotation);
                    }
                }
                if verbs.is_empty() {
                    continue;
                }
                if paths.is_empty() {
                    paths.push(String::new());
                }
                for verb in &verbs {
                    for prefix in &prefixes {
                        for path in &paths {
                            res.push(NavigationTarget {
                                file: NavFile::Source(file_id),
                                full_range: method.syntax().text_range(),
                                focus_range: name.syntax().text_range(),
                                name: SmolStr::new(format!("{verb} {}", join_path(prefix, path))),
                                kind: SymbolKind::Method,
                                container: class_name.clone(),
                                detail: Some(format!("{}()", name.text())),
                                deprecated: false,
                                key: None,
                            });
                        }
                    }
                }
            }
        }
    }
    res
}

/// 按路径（或 `GET /path`）搜索端点。
pub(crate) fn search(db: &dyn Db, query: &str, limit: usize) -> Vec<NavigationTarget> {
    let needle = query.trim().to_lowercase();
    let mut found: Vec<NavigationTarget> = endpoints(db)
        .into_iter()
        .filter(|endpoint| endpoint.name.to_lowercase().contains(&needle))
        .collect();
    found.sort_by(|a, b| (a.name.len(), &a.name).cmp(&(b.name.len(), &b.name)));
    found.truncate(limit);
    found
}

#[cfg(test)]
mod tests {
    use super::join_path;

    #[test]
    fn joins_prefix_and_path() {
        assert_eq!(join_path("", ""), "/");
        assert_eq!(join_path("/api", ""), "/api");
        assert_eq!(join_path("/api/", "/users/{id}"), "/api/users/{id}");
        assert_eq!(join_path("api", "users/"), "/api/users");
        assert_eq!(join_path("", "/x"), "/x");
    }
}
