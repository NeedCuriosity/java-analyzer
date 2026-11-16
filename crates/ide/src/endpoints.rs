//! Web 端点。
//!
//! 服务端：Spring MVC（`@RequestMapping`、`@GetMapping` …）和 JAX-RS（`@Path` + `@GET` …）。
//! 客户端：Spring Cloud OpenFeign 的 `@FeignClient` 接口（方法上是 Spring MVC 的映射注解）和
//! Spring 6 的 HTTP 接口（`@HttpExchange`、`@GetExchange` …）。
//!
//! 只看注解的写法，不依赖 Spring 的类在不在类路径上。服务端端点以 `GET /api/users/{id}` 的形式
//! 出现在工作区符号搜索里：查询里带 `/` 时按路径匹配。客户端方法按 HTTP 方法和路径匹配服务端端点
//! （路径变量 `{id}` / `{name:regex}` 当作通配），用于在两边之间跳转和查找引用。
//! `@FeignClient` 的服务名和某个模块的 `spring.application.name` 相同时，优先那个模块里的端点。

use std::sync::Arc;

use base_db::{workspace, Db, FileId, FileKind, ModuleId, SourceFile};
use hir::Definition;
use rustc_hash::FxHashSet;
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use crate::{
    navigation::{NavFile, NavigationTarget},
    SymbolKind,
};

/// 端点在哪一边。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Side {
    Server,
    /// `service`：`@FeignClient` 的 `name` / `value`，可能是 `${key:default}` 形式的占位符
    Client {
        service: Option<String>,
    },
}

/// 一个方法上的一个端点（同一个方法有几个 HTTP 方法或几条路径时有几个）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub(crate) file_id: FileId,
    pub(crate) module: Option<ModuleId>,
    pub(crate) side: Side,
    /// `GET`、`POST` …；没有限定 HTTP 方法时是 `ANY`
    pub(crate) verb: SmolStr,
    /// 规范化后的完整路径：以 `/` 开头，没有重复和结尾的 `/`
    pub(crate) path: String,
    pub(crate) class_name: Option<String>,
    pub(crate) method_name: SmolStr,
    /// 整个方法声明
    pub(crate) full_range: TextRange,
    /// 方法名
    pub(crate) focus_range: TextRange,
    /// 方法上映射注解里的字符串字面量（光标在这里也算在这个端点上）
    pub(crate) literals: Vec<TextRange>,
}

impl Endpoint {
    pub(crate) fn is_client(&self) -> bool {
        matches!(self.side, Side::Client { .. })
    }

    /// `GET /api/users/{id}`
    pub(crate) fn route(&self) -> String {
        format!("{} {}", self.verb, self.path)
    }

    fn contains(&self, offset: TextSize) -> bool {
        self.focus_range.contains_inclusive(offset)
            || self.literals.iter().any(|r| r.contains_inclusive(offset))
    }

    fn to_nav(&self) -> NavigationTarget {
        NavigationTarget {
            file: NavFile::Source(self.file_id),
            full_range: self.full_range,
            focus_range: self.focus_range,
            name: SmolStr::new(self.route()),
            kind: SymbolKind::Method,
            container: self.class_name.clone(),
            detail: Some(format!("{}()", self.method_name)),
            deprecated: false,
            key: None,
        }
    }

    /// 悬停里的一行：`` `GET /api/users/{id}` → `UserController.get()` ``
    fn describe(&self) -> String {
        match &self.class_name {
            Some(class) => format!("`{}` → `{class}.{}()`", self.route(), self.method_name),
            None => format!("`{}` → `{}()`", self.route(), self.method_name),
        }
    }
}

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

fn literal_ranges(node: &SyntaxNode) -> Vec<TextRange> {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::STRING_LITERAL)
        .map(|t| t.text_range())
        .collect()
}

/// 注解里这些属性（或不带名字的唯一实参）的字符串值。
fn attribute_strings(annotation: &ast::Annotation, keys: &[&str]) -> Vec<String> {
    let Some(args) = annotation.arg_list() else {
        return Vec::new();
    };
    let named: Vec<ast::AnnotationArg> = args.args().collect();
    if named.iter().all(|arg| arg.name_ref().is_none()) {
        return if keys.contains(&"value") {
            string_literals(args.syntax())
        } else {
            Vec::new()
        };
    }
    let mut res = Vec::new();
    for arg in &named {
        let key = arg.name_ref().map(|n| n.text().to_string());
        if key.as_deref().is_some_and(|k| keys.contains(&k)) {
            res.extend(string_literals(arg.syntax()));
        }
    }
    res
}

/// 注解里的路径：`("/x")`、`(value = "/x")`、`(path = {"/x", "/y"})`。没写时是空串。
fn paths_of(annotation: &ast::Annotation, keys: &[&str]) -> Vec<String> {
    let mut paths = attribute_strings(annotation, keys);
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
                .filter(|n| is_verb(n))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn is_verb(name: &str) -> bool {
    matches!(
        name,
        "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
    )
}

/// URL 里的路径部分：`http://host:8080/api` → `/api`；不带协议的原样返回。
/// 含占位符的 URL（`${user.url}`）通常代表协议、主机和端口，不计入路径。
fn url_path(url: &str) -> String {
    if url.contains("${") {
        return String::new();
    }
    match url.split_once("://") {
        Some((_, rest)) => rest
            .find('/')
            .map_or(String::new(), |i| rest[i..].to_owned()),
        None => url.to_owned(),
    }
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

fn exchange_verb(annotation: &str) -> Option<&'static str> {
    Some(match annotation {
        "GetExchange" => "GET",
        "PostExchange" => "POST",
        "PutExchange" => "PUT",
        "DeleteExchange" => "DELETE",
        "PatchExchange" => "PATCH",
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

/// 一个方法上的映射：HTTP 方法、相对路径、注解里的字符串字面量。
#[derive(Default)]
struct Mapping {
    verbs: Vec<String>,
    paths: Vec<String>,
    literals: Vec<TextRange>,
}

impl Mapping {
    fn add(&mut self, verbs: Vec<String>, paths: Vec<String>, annotation: &ast::Annotation) {
        self.verbs.extend(verbs);
        self.paths = paths;
        self.literals.extend(literal_ranges(annotation.syntax()));
    }
}

/// Spring MVC 风格（服务端控制器和 Feign 客户端共用）的方法映射。
fn mvc_mapping(method_annotations: &[(String, ast::Annotation)], jaxrs: bool) -> Mapping {
    let mut mapping = Mapping::default();
    for (name, annotation) in method_annotations {
        if let Some(verb) = spring_verb(name) {
            mapping.add(
                vec![verb.to_owned()],
                paths_of(annotation, &["value", "path"]),
                annotation,
            );
        } else if name == "RequestMapping" {
            let mut verbs = request_methods(annotation);
            if verbs.is_empty() {
                verbs.push("ANY".to_owned());
            }
            mapping.add(verbs, paths_of(annotation, &["value", "path"]), annotation);
        } else if !jaxrs {
            continue;
        } else if let Some(verb) = jaxrs_verb(name) {
            mapping.verbs.push(verb.to_owned());
        } else if name == "Path" {
            mapping.paths = paths_of(annotation, &["value"]);
            mapping.literals.extend(literal_ranges(annotation.syntax()));
        }
    }
    mapping
}

/// HTTP 接口的方法映射：`@GetExchange("/x")`、`@HttpExchange(method = "POST", url = "/x")`。
fn exchange_mapping(method_annotations: &[(String, ast::Annotation)]) -> Mapping {
    let mut mapping = Mapping::default();
    for (name, annotation) in method_annotations {
        if let Some(verb) = exchange_verb(name) {
            mapping.add(
                vec![verb.to_owned()],
                paths_of(annotation, &["value", "url"]),
                annotation,
            );
        } else if name == "HttpExchange" {
            let mut verbs: Vec<String> = attribute_strings(annotation, &["method"])
                .into_iter()
                .map(|m| m.to_uppercase())
                .filter(|m| is_verb(m))
                .collect();
            if verbs.is_empty() {
                verbs.push("ANY".to_owned());
            }
            mapping.add(verbs, paths_of(annotation, &["value", "url"]), annotation);
        }
    }
    mapping
}

fn type_endpoints(decl: &ast::TypeDecl, file_id: FileId, out: &mut Vec<Endpoint>) {
    let class_annotations = annotations(decl.syntax());
    let find = |wanted: &str| {
        class_annotations
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, a)| a)
    };
    let is_interface = decl.syntax().kind() == SyntaxKind::INTERFACE_DECL;
    let request_mapping = find("RequestMapping").map(|a| paths_of(a, &["value", "path"]));
    // Feign 客户端：url 的路径部分 + path 属性 + 类上的 `@RequestMapping`
    let feign = find("FeignClient").map(|feign| {
        let service = attribute_strings(feign, &["value", "name"])
            .into_iter()
            .next();
        let url = attribute_strings(feign, &["url"]).into_iter().next();
        let path = attribute_strings(feign, &["path"]).into_iter().next();
        let base = join_path(
            &url_path(url.as_deref().unwrap_or("")),
            path.as_deref().unwrap_or(""),
        );
        let prefixes: Vec<String> = request_mapping
            .clone()
            .unwrap_or_else(|| vec![String::new()])
            .iter()
            .map(|p| join_path(&base, p))
            .collect();
        (service, prefixes)
    });
    let exchange = find("HttpExchange");
    let exchange_prefixes: Vec<String> = exchange
        .map(|a| paths_of(a, &["value", "url"]))
        .unwrap_or_else(|| vec![String::new()])
        .iter()
        .map(|url| url_path(url))
        .collect();
    let server_prefixes: Vec<String> = request_mapping
        .or_else(|| find("Path").map(|a| paths_of(a, &["value"])))
        .unwrap_or_else(|| vec![String::new()]);
    let class_name = decl.name().map(|n| n.text().to_string());
    let Some(body) = decl.body() else { return };
    for method in body.syntax().children().filter_map(ast::MethodDecl::cast) {
        let Some(name) = method.name() else { continue };
        let method_annotations = annotations(method.syntax());
        let (side, prefixes, mut mapping) = if let Some((service, prefixes)) = &feign {
            let side = Side::Client {
                service: service.clone(),
            };
            (side, prefixes, mvc_mapping(&method_annotations, false))
        } else {
            let mapping = if is_interface || exchange.is_some() {
                exchange_mapping(&method_annotations)
            } else {
                Mapping::default()
            };
            if !mapping.verbs.is_empty() {
                (Side::Client { service: None }, &exchange_prefixes, mapping)
            } else {
                (
                    Side::Server,
                    &server_prefixes,
                    mvc_mapping(&method_annotations, true),
                )
            }
        };
        if mapping.verbs.is_empty() {
            continue;
        }
        if mapping.paths.is_empty() {
            mapping.paths.push(String::new());
        }
        for verb in &mapping.verbs {
            for prefix in prefixes {
                for path in &mapping.paths {
                    out.push(Endpoint {
                        file_id,
                        module: None,
                        side: side.clone(),
                        verb: SmolStr::new(verb),
                        path: join_path(prefix, path),
                        class_name: class_name.clone(),
                        method_name: SmolStr::new(name.text()),
                        full_range: method.syntax().text_range(),
                        focus_range: name.syntax().text_range(),
                        literals: mapping.literals.clone(),
                    });
                }
            }
        }
    }
}

/// 一个文件里的端点（`module` 还没有填）。
#[salsa::tracked]
fn file_endpoints(db: &dyn Db, file: SourceFile) -> Arc<Vec<Endpoint>> {
    let text: &str = file.text(db);
    // 绝大多数文件没有端点：先按文本筛一遍，不解析
    if !text.contains("Mapping") && !text.contains("@Path") && !text.contains("Exchange") {
        return Arc::new(Vec::new());
    }
    let root = base_db::parse(db, file).syntax_node();
    let mut res = Vec::new();
    for decl in root.descendants().filter_map(ast::TypeDecl::cast) {
        type_endpoints(&decl, file.file_id(db), &mut res);
    }
    Arc::new(res)
}

/// 工作区里的全部端点（服务端和客户端）。
#[salsa::tracked]
fn workspace_endpoints(db: &dyn Db) -> Arc<Vec<Endpoint>> {
    let files = workspace(db).files(db);
    let mut res = Vec::new();
    for (file_id, file) in files.workspace_files() {
        db.unwind_if_revision_cancelled();
        let found = file_endpoints(db, file);
        if found.is_empty() {
            continue;
        }
        let module = match files.get(file_id).map(|entry| entry.kind) {
            Some(FileKind::Workspace { module, .. }) => module,
            _ => None,
        };
        res.extend(found.iter().cloned().map(|mut e| {
            e.module = module;
            e
        }));
    }
    Arc::new(res)
}

/// 工作区里的全部服务端端点。
pub(crate) fn endpoints(db: &dyn Db) -> Vec<NavigationTarget> {
    workspace_endpoints(db)
        .iter()
        .filter(|e| !e.is_client())
        .map(Endpoint::to_nav)
        .collect()
}

/// 按路径（或 `GET /path`）搜索服务端端点。
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

fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// 能匹配任意一段的路径段：路径变量、`*`、没法解析的占位符。
fn is_wildcard(segment: &str) -> bool {
    (segment.starts_with('{') && segment.ends_with('}')) || segment == "*" || segment.contains("${")
}

/// 两条路径能不能指向同一个端点。返回匹配的精确程度（越大越精确），不能匹配时返回 `None`。
fn match_score(a: &[&str], b: &[&str]) -> Option<u32> {
    match (a.split_first(), b.split_first()) {
        (None, None) => Some(0),
        // `/**` 匹配剩下的全部
        (Some((&"**", _)), _) | (_, Some((&"**", _))) => Some(0),
        (Some((x, rest_a)), Some((y, rest_b))) => {
            let score = match (is_wildcard(x), is_wildcard(y)) {
                (false, false) if x == y => 2,
                (false, false) => return None,
                (true, true) => 2,
                _ => 1,
            };
            Some(score + match_score(rest_a, rest_b)?)
        }
        _ => None,
    }
}

fn verbs_match(a: &str, b: &str) -> bool {
    a == b || a == "ANY" || b == "ANY"
}

/// 模块自己的 Spring 配置文件（`application*.yml` / `.properties`、`bootstrap*`）里和端点有关的项。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ModuleWebConfig {
    /// `spring.application.name`
    application_name: Option<String>,
    /// `server.servlet.context-path` 等：服务端路径的公共前缀
    context_path: Option<String>,
}

/// 每个模块（按编号）的 [`ModuleWebConfig`]。
#[salsa::tracked]
fn module_web_configs(db: &dyn Db) -> Arc<Vec<ModuleWebConfig>> {
    let ws = workspace(db);
    let mut res = vec![ModuleWebConfig::default(); ws.project(db).modules.len()];
    let name_key = ide_db::spring::canonical("spring.application.name");
    let context_keys: Vec<String> = [
        "server.servlet.context-path",
        "spring.mvc.servlet.path",
        "spring.webflux.base-path",
    ]
    .iter()
    .map(|k| ide_db::spring::canonical(k))
    .collect();
    for resource in ws.resources(db).iter() {
        let spring_file =
            resource.name.starts_with("application") || resource.name.starts_with("bootstrap");
        let Some(slot) = resource.module.and_then(|m| res.get_mut(m.0 as usize)) else {
            continue;
        };
        if !spring_file || !ide_db::spring::is_config_name(&resource.name) {
            continue;
        }
        let mut properties = Vec::new();
        ide_db::spring::parse_resource(resource, &mut properties);
        for property in properties {
            let value = property.value.trim().trim_matches(['"', '\'']).to_owned();
            if value.is_empty() {
                continue;
            }
            let key = ide_db::spring::canonical(&property.key);
            if key == name_key && slot.application_name.is_none() {
                slot.application_name = Some(value);
            } else if context_keys.contains(&key) && slot.context_path.is_none() {
                slot.context_path = Some(value);
            }
        }
    }
    Arc::new(res)
}

/// `${key:default}` 形式的服务名：先在客户端模块的配置里找 `key`，找不到用默认值。
fn resolve_service(db: &dyn Db, client_module: Option<ModuleId>, service: &str) -> String {
    let Some(inner) = service
        .trim()
        .strip_prefix("${")
        .and_then(|s| s.strip_suffix('}'))
    else {
        return service.trim().to_owned();
    };
    let (key, default) = inner.split_once(':').unwrap_or((inner, ""));
    let configured = ide_db::spring::config_properties(db, client_module)
        .into_iter()
        .find(|p| ide_db::spring::canonical(&p.key) == ide_db::spring::canonical(key))
        .map(|p| p.value);
    configured.unwrap_or_else(|| default.to_owned())
}

/// 某个客户端端点匹配的服务端端点，按精确程度只留最好的那些。
fn matching_servers(db: &dyn Db, clients: &[&Endpoint]) -> Vec<Endpoint> {
    let all = workspace_endpoints(db);
    let servers: Vec<&Endpoint> = all.iter().filter(|e| !e.is_client()).collect();
    if servers.is_empty() {
        return Vec::new();
    }
    let configs = module_web_configs(db);
    let mut res: Vec<(u32, Endpoint)> = Vec::new();
    for client in clients {
        let client_path = segments(&client.path);
        let mut found: Vec<(u32, &Endpoint)> = Vec::new();
        for server in &servers {
            if !verbs_match(&client.verb, &server.verb) {
                continue;
            }
            let direct = match_score(&client_path, &segments(&server.path));
            // 服务端配了 context-path 时客户端的路径通常带着它
            let with_context = server
                .module
                .and_then(|m| configs.get(m.0 as usize)?.context_path.as_deref())
                .and_then(|ctx| {
                    match_score(&client_path, &segments(&join_path(ctx, &server.path)))
                });
            if let Some(score) = direct.max(with_context) {
                found.push((score, server));
            }
        }
        // Feign 的服务名对上某个模块的 `spring.application.name` 时只看那个模块（和它依赖的模块）
        if let Side::Client {
            service: Some(service),
        } = &client.side
        {
            let service = resolve_service(db, client.module, service);
            let preferred = service_modules(db, &service);
            if found
                .iter()
                .any(|(_, e)| e.module.is_some_and(|m| preferred.contains(&m)))
            {
                found.retain(|(_, e)| e.module.is_some_and(|m| preferred.contains(&m)));
            }
        }
        res.extend(found.into_iter().map(|(score, e)| (score, e.clone())));
    }
    let best = res.iter().map(|(score, _)| *score).max().unwrap_or(0);
    let mut seen = FxHashSet::default();
    res.into_iter()
        .filter(|(score, _)| *score == best)
        .map(|(_, e)| e)
        .filter(|e| seen.insert((e.file_id, e.focus_range)))
        .collect()
}

/// `spring.application.name` 是 `service` 的模块，加上它们依赖的模块（控制器可能在被依赖的模块里）。
fn service_modules(db: &dyn Db, service: &str) -> Vec<ModuleId> {
    if service.is_empty() {
        return Vec::new();
    }
    let project = workspace(db).project(db);
    let configs = module_web_configs(db);
    let mut res = Vec::new();
    for (idx, module) in project.modules.iter().enumerate() {
        let id = ModuleId(idx as u32);
        let name = configs.get(idx).and_then(|c| c.application_name.as_deref());
        if !name.is_some_and(|n| n.eq_ignore_ascii_case(service)) {
            continue;
        }
        res.push(id);
        for dependency in &module.dependencies {
            if let base_db::DependencyTarget::Module(dep) = dependency.target {
                res.push(dep);
            }
        }
    }
    res
}

/// 匹配某个服务端端点的客户端端点。
fn matching_clients(db: &dyn Db, servers: &[&Endpoint]) -> Vec<Endpoint> {
    let all = workspace_endpoints(db);
    let mut res = Vec::new();
    let mut seen = FxHashSet::default();
    for client in all.iter().filter(|e| e.is_client()) {
        if seen.contains(&(client.file_id, client.focus_range)) {
            continue;
        }
        // 先粗筛：HTTP 方法对得上、路径的段数对得上（或带 `**`）
        let rough = servers.iter().any(|s| {
            verbs_match(&client.verb, &s.verb)
                && (segments(&client.path).len() >= segments(&s.path).len()
                    || s.path.contains("**"))
        });
        if !rough {
            continue;
        }
        // 反过来算一遍客户端的匹配，保证两个方向的结果一致（服务名优先、只留最精确的）
        let matched = matching_servers(db, &[client]);
        let hit = matched.iter().any(|server| {
            servers
                .iter()
                .any(|s| s.file_id == server.file_id && s.focus_range == server.focus_range)
        });
        if hit {
            seen.insert((client.file_id, client.focus_range));
            res.push(client.clone());
        }
    }
    res
}

/// 文件里光标所在的端点（方法名上或映射注解的字符串里）。
fn endpoints_at(db: &dyn Db, file: SourceFile, offset: TextSize) -> Vec<Endpoint> {
    file_endpoints(db, file)
        .iter()
        .filter(|e| e.contains(offset))
        .cloned()
        .collect()
}

/// 方法声明对应的端点。
fn endpoints_of_method(db: &dyn Db, def: &Definition) -> Vec<Endpoint> {
    let Definition::Method(_) = def else {
        return Vec::new();
    };
    let Some(source) = def.source(db) else {
        return Vec::new();
    };
    file_endpoints(db, source.file)
        .iter()
        .filter(|e| e.focus_range == source.focus_range)
        .cloned()
        .collect()
}

/// 光标在客户端方法的名字或映射注解的字符串上时，它匹配的服务端方法。
/// 光标不在客户端方法上、或者没有匹配时返回 `None`。
pub(crate) fn goto_server(
    db: &dyn Db,
    file: SourceFile,
    offset: TextSize,
) -> Option<(TextRange, Vec<NavigationTarget>)> {
    let found = endpoints_at(db, file, offset);
    let clients: Vec<&Endpoint> = found.iter().filter(|e| e.is_client()).collect();
    let first = clients.first()?;
    let range = std::iter::once(first.focus_range)
        .chain(first.literals.iter().copied())
        .find(|r| r.contains_inclusive(offset))?;
    let targets: Vec<NavigationTarget> = matching_servers(db, &clients)
        .iter()
        .map(Endpoint::to_nav)
        .collect();
    (!targets.is_empty()).then_some((range, targets))
}

/// 客户端方法匹配的服务端方法（跳转到实现时用）。
pub(crate) fn servers_of_method(db: &dyn Db, def: &Definition) -> Vec<NavigationTarget> {
    let found = endpoints_of_method(db, def);
    let clients: Vec<&Endpoint> = found.iter().filter(|e| e.is_client()).collect();
    if clients.is_empty() {
        return Vec::new();
    }
    matching_servers(db, &clients)
        .iter()
        .map(Endpoint::to_nav)
        .collect()
}

/// 调用服务端方法的客户端方法：`(文件, 方法名的范围)`。
pub(crate) fn clients_of_method(db: &dyn Db, def: &Definition) -> Vec<(FileId, TextRange)> {
    let found = endpoints_of_method(db, def);
    let servers: Vec<&Endpoint> = found.iter().filter(|e| !e.is_client()).collect();
    if servers.is_empty() {
        return Vec::new();
    }
    matching_clients(db, &servers)
        .iter()
        .map(|e| (e.file_id, e.focus_range))
        .collect()
}

/// 光标在端点方法映射注解的字符串里时，方法名的起点（之后按方法本身处理）。
pub(crate) fn method_name_at_literal(
    db: &dyn Db,
    file: SourceFile,
    offset: TextSize,
) -> Option<TextSize> {
    endpoints_at(db, file, offset)
        .first()
        .map(|e| e.focus_range.start())
}

/// 悬停时附加的端点信息：客户端方法列出匹配的服务端端点，服务端方法列出自己的路由。
pub(crate) fn hover_lines(db: &dyn Db, def: &Definition) -> Option<String> {
    let found = endpoints_of_method(db, def);
    if found.is_empty() {
        return None;
    }
    let clients: Vec<&Endpoint> = found.iter().filter(|e| e.is_client()).collect();
    let mut lines: Vec<String> = Vec::new();
    if clients.is_empty() {
        for endpoint in &found {
            let line = format!("`{}`", endpoint.route());
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
        return Some(lines.join("  \n"));
    }
    let servers = matching_servers(db, &clients);
    if servers.is_empty() {
        for client in &clients {
            lines.push(format!("`{}`", client.route()));
        }
        lines.dedup();
        return Some(lines.join("  \n"));
    }
    for server in &servers {
        lines.push(server.describe());
    }
    Some(lines.join("  \n"))
}

#[cfg(test)]
mod tests {
    use super::{join_path, match_score, segments, url_path};

    #[test]
    fn joins_prefix_and_path() {
        assert_eq!(join_path("", ""), "/");
        assert_eq!(join_path("/api", ""), "/api");
        assert_eq!(join_path("/api/", "/users/{id}"), "/api/users/{id}");
        assert_eq!(join_path("api", "users/"), "/api/users");
        assert_eq!(join_path("", "/x"), "/x");
    }

    #[test]
    fn extracts_path_of_url() {
        assert_eq!(url_path("http://localhost:8080/api"), "/api");
        assert_eq!(url_path("https://users"), "");
        assert_eq!(url_path("${users.url}"), "");
        assert_eq!(url_path("/api"), "/api");
    }

    #[test]
    fn matches_paths_with_variables() {
        let score = |a: &str, b: &str| match_score(&segments(a), &segments(b));
        assert_eq!(score("/api/users/{id}", "/api/users/{userId}"), Some(6));
        assert_eq!(score("/api/users/{id:\\d+}", "/api/users/{id}"), Some(6));
        assert_eq!(score("/api/users/me", "/api/users/{id}"), Some(5));
        assert_eq!(score("/api/users", "/api/users/{id}"), None);
        assert_eq!(score("/api/orders/1", "/api/users/{id}"), None);
        assert_eq!(score("/files/a/b", "/files/**"), Some(2));
        assert_eq!(score("/", "/"), Some(0));
    }
}
