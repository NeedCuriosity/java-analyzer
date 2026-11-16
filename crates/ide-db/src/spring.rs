//! Spring 配置：`application*.properties` / `.yml` 里的配置项，以及 Java 代码里引用它们的地方
//! （`@Value("${key:default}")`、`environment.getProperty("key")`）。
//!
//! 配置项按 Spring 的宽松绑定比较：忽略大小写、`-` 和 `_`（`app.max-size` 等同于 `app.maxSize`）。

use base_db::{workspace, Db, FileId, ModuleId, ResourceFile};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxToken, TextRange, TextSize,
};

/// 配置文件里的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProperty {
    /// 展开后的完整键：`spring.datasource.url`、`app.servers[0]`
    pub key: String,
    pub value: String,
    pub file_id: FileId,
    /// 配置文件名，如 `application-dev.yml`
    pub file_name: smol_str::SmolStr,
    /// 键在文件里的范围（YAML 里是最后一段）
    pub key_range: TextRange,
}

/// 宽松绑定下的规范形式。
pub fn canonical(key: &str) -> String {
    key.chars()
        .filter(|c| *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// 一个模块能看到的配置文件：它自己的、它依赖的模块的，以及不属于任何模块的。
/// `module` 为 `None` 时全部可见。
pub fn visible_resources(db: &dyn Db, module: Option<ModuleId>) -> Vec<ResourceFile> {
    let ws = workspace(db);
    // 资源文件里还有 `lombok.config` 等，只要 Spring 的配置文件
    let resources: Vec<&ResourceFile> = ws
        .resources(db)
        .iter()
        .filter(|r| {
            [".properties", ".yml", ".yaml"]
                .iter()
                .any(|ext| r.name.ends_with(ext))
        })
        .collect();
    let Some(module) = module else {
        return resources.into_iter().cloned().collect();
    };
    let project = ws.project(db);
    let mut visible = vec![module];
    if let Some(data) = project.modules.get(module.0 as usize) {
        for dependency in &data.dependencies {
            match dependency.target {
                base_db::DependencyTarget::Module(id)
                | base_db::DependencyTarget::ModuleTests(id) => visible.push(id),
                base_db::DependencyTarget::Library(_) => {}
            }
        }
    }
    resources
        .into_iter()
        .filter(|r| r.module.is_none_or(|m| visible.contains(&m)))
        .cloned()
        .collect()
}

/// 模块能看到的全部配置项。
pub fn config_properties(db: &dyn Db, module: Option<ModuleId>) -> Vec<ConfigProperty> {
    let mut out = Vec::new();
    for resource in visible_resources(db, module) {
        parse_resource(&resource, &mut out);
    }
    out
}

pub fn parse_resource(resource: &ResourceFile, out: &mut Vec<ConfigProperty>) {
    let mut push = |key: String, value: String, range: TextRange| {
        out.push(ConfigProperty {
            key,
            value,
            file_id: resource.file_id,
            file_name: resource.name.clone(),
            key_range: range,
        })
    };
    if resource.name.ends_with(".properties") {
        parse_properties(&resource.text, &mut push);
    } else {
        parse_yaml(&resource.text, &mut push);
    }
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32))
}

/// `.properties`：`key=value`、`key: value`、`key value`，`#` / `!` 开头是注解，
/// 行尾的 `\` 表示续行。
pub fn parse_properties(text: &str, push: &mut impl FnMut(String, String, TextRange)) {
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(raw) = lines.next() {
        let line_start = offset;
        offset += raw.len();
        let line = raw.trim_end_matches(['\n', '\r']);
        let indent = line.len() - line.trim_start().len();
        let content = &line[indent..];
        if content.is_empty() || content.starts_with('#') || content.starts_with('!') {
            continue;
        }
        // 键在第一个没有转义的 `=`、`:` 或空白处结束
        let bytes = content.as_bytes();
        let mut end = 0;
        while end < bytes.len() {
            match bytes[end] {
                b'\\' => end += 2,
                b'=' | b':' | b' ' | b'\t' => break,
                _ => end += 1,
            }
        }
        let end = end.min(bytes.len());
        let key = content[..end].replace('\\', "");
        let mut value = content[end..]
            .trim_start_matches([' ', '\t'])
            .trim_start_matches(['=', ':'])
            .trim_start()
            .to_owned();
        // 续行
        while value.ends_with('\\') {
            value.pop();
            match lines.next() {
                Some(next) => {
                    offset += next.len();
                    value.push_str(next.trim());
                }
                None => break,
            }
        }
        if key.is_empty() {
            continue;
        }
        let start = line_start + indent;
        push(key, value, range(start, start + end));
    }
}

/// YAML 的常见子集：按缩进嵌套的映射、`- ` 列表项（展开成 `key[0]`）、`---` 分隔的多个文档、
/// `|` / `>` 块标量。锚点、流式写法（`{a: 1}`）不展开。
pub fn parse_yaml(text: &str, push: &mut impl FnMut(String, String, TextRange)) {
    // (缩进, 键)
    let mut stack: Vec<(usize, String)> = Vec::new();
    // 每个父键下已经出现的列表项个数
    let mut list_counts: Vec<(String, usize)> = Vec::new();
    let mut block_indent: Option<usize> = None;
    let mut offset = 0;
    for raw in text.split_inclusive('\n') {
        let line_start = offset;
        offset += raw.len();
        let line = raw.trim_end_matches(['\n', '\r']);
        let indent = line.len() - line.trim_start().len();
        let content = line[indent..].trim_end();
        if let Some(block) = block_indent {
            if content.is_empty() || indent > block {
                continue;
            }
            block_indent = None;
        }
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        if content == "---" || content == "..." {
            stack.clear();
            list_counts.clear();
            continue;
        }
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let parent = stack.last().map(|(_, k)| k.clone()).unwrap_or_default();

        let (mut key_start, mut entry) = (line_start + indent, content);
        let mut prefix = parent.clone();
        let mut item_indent = indent;
        if let Some(rest) =
            content
                .strip_prefix("- ")
                .or(if content == "-" { Some("") } else { None })
        {
            // 列表项：`parent[i]`
            let idx = match list_counts.iter_mut().find(|(k, _)| *k == parent) {
                Some((_, n)) => {
                    *n += 1;
                    *n - 1
                }
                None => {
                    list_counts.push((parent.clone(), 1));
                    0
                }
            };
            prefix = format!("{parent}[{idx}]");
            let skipped = content.len() - rest.trim_start().len();
            key_start += skipped;
            item_indent = indent + skipped;
            entry = rest.trim_start();
            match split_yaml_entry(entry) {
                Some(_) => {
                    // `- name: x`：后面同缩进的行属于这一项
                    stack.push((indent, prefix.clone()));
                }
                None => {
                    if !entry.is_empty() {
                        let value = yaml_scalar(entry);
                        push(prefix, value, range(key_start, key_start + entry.len()));
                    } else {
                        stack.push((indent, prefix));
                    }
                    continue;
                }
            }
        }
        let Some((key, value)) = split_yaml_entry(entry) else {
            continue;
        };
        let full = if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        };
        let key_offset = entry.find(key).unwrap_or(0);
        let key_range = range(key_start + key_offset, key_start + key_offset + key.len());
        let value = value.trim();
        if value.is_empty() {
            stack.push((item_indent, full));
        } else if value.starts_with('|') || value.starts_with('>') {
            block_indent = Some(item_indent);
            push(full, String::new(), key_range);
        } else {
            push(full, yaml_scalar(value), key_range);
        }
    }
}

/// `key: value` 拆成键和值；键可以带引号。不是映射项时返回 `None`。
fn split_yaml_entry(entry: &str) -> Option<(&str, &str)> {
    let (key, value) = if let Some(rest) = entry.strip_prefix(['"', '\'']) {
        let quote = entry.as_bytes()[0] as char;
        let close = rest.find(quote)?;
        let after = rest[close + 1..].trim_start().strip_prefix(':')?;
        (&rest[..close], after)
    } else {
        // `: ` 或行尾的 `:` 才是分隔符（`http://x` 里的不是）
        let idx = entry
            .match_indices(':')
            .find(|(i, _)| entry[i + 1..].is_empty() || entry[i + 1..].starts_with([' ', '\t']))?
            .0;
        (entry[..idx].trim_end(), &entry[idx + 1..])
    };
    (!key.is_empty()).then_some((key, value))
}

fn yaml_scalar(value: &str) -> String {
    let value = value.trim();
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
    {
        return inner.to_owned();
    }
    // 去掉行尾注释
    match value.find(" #") {
        Some(idx) => value[..idx].trim_end().to_owned(),
        None => value.to_owned(),
    }
}

/// 字符串字面量里的一个 `${key}` / `${key:default}` 占位符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub key: String,
    /// 键在文件里的范围
    pub key_range: TextRange,
    pub has_default: bool,
}

/// 字面量（含引号的原文）里的占位符。`${` 后面还没写完（没有 `}`）的也算，键到字面量末尾为止。
pub fn placeholders(literal: &SyntaxToken) -> Vec<Placeholder> {
    let text = literal.text();
    let base = usize::from(literal.text_range().start());
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(found) = text[search..].find("${") {
        let key_start = search + found + 2;
        let rest = &text[key_start..];
        let key_len = rest.find([':', '}', '"', '$', '\n']).unwrap_or(rest.len());
        let has_default = rest[key_len..].starts_with(':');
        let key = rest[..key_len].trim();
        out.push(Placeholder {
            key: key.to_owned(),
            key_range: range(base + key_start, base + key_start + key_len),
            has_default,
        });
        search = key_start + key_len;
    }
    out
}

/// 字符串字面量所在的配置引用：`@Value("...")` 的值（返回其中的占位符），
/// 或者 `xxx.getProperty("key")` 的第一个实参（整个字面量就是键）。
pub enum PropertyUse {
    /// `@Value`：里面的 `${...}`
    Value(Vec<Placeholder>),
    /// `getProperty("key")`：键的范围（引号里面）
    Lookup(Placeholder),
}

pub fn property_use(
    literal: &SyntaxToken,
    receiver_is_environment: impl Fn(&ast::Expr) -> bool,
) -> Option<PropertyUse> {
    if literal.kind() != SyntaxKind::STRING_LITERAL {
        return None;
    }
    let expr = literal.parent()?;
    let parent = expr.parent()?;
    // `@Value("...")` 或 `@Value(value = "...")`
    let annotation = ast::AnnotationArg::cast(parent.clone())
        .filter(|arg| arg.name_ref().is_none_or(|n| n.text() == "value"))
        .and_then(|arg| arg.syntax().parent()?.parent())
        .and_then(ast::Annotation::cast);
    if let Some(annotation) = annotation {
        let name = annotation.name()?.segments().last()?.to_string();
        if name != "Value" {
            return None;
        }
        return Some(PropertyUse::Value(placeholders(literal)));
    }
    // `env.getProperty("key", ...)`
    if parent.kind() == SyntaxKind::ARG_LIST && parent.children().next().as_ref() == Some(&expr) {
        let call = ast::MethodCall::cast(parent.parent()?)?;
        let name = call.name_ref()?.text();
        if !matches!(
            name.as_str(),
            "getProperty" | "getRequiredProperty" | "containsProperty"
        ) {
            return None;
        }
        if !call.receiver().is_some_and(|r| receiver_is_environment(&r)) {
            return None;
        }
        let text = literal.text();
        let inner = text.strip_prefix('"')?;
        let len = inner.strip_suffix('"').map_or(inner.len(), str::len);
        let start = usize::from(literal.text_range().start()) + 1;
        return Some(PropertyUse::Lookup(Placeholder {
            key: inner[..len].to_owned(),
            key_range: range(start, start + len),
            has_default: true,
        }));
    }
    None
}

/// 接收者的类型是 Spring 的 `Environment` / `PropertyResolver` 一族。
pub fn is_environment(
    db: &dyn Db,
    sema: &hir::Semantics<'_>,
    file: base_db::SourceFile,
    expr: &ast::Expr,
) -> bool {
    let Some(hir::Ty::Class(class, _)) = sema.type_of_expr(file, expr) else {
        return false;
    };
    let is_env = |c: hir::ClassId| {
        hir::class_data(db, c).is_some_and(|d| {
            d.name.ends_with("Environment") || d.name.ends_with("PropertyResolver")
        })
    };
    is_env(class) || hir::all_supers(db, class).into_iter().any(is_env)
}

/// 光标处引用的配置项：占位符（或 `getProperty` 的键）以及文件所在的模块。
pub fn property_at(
    db: &dyn Db,
    file: base_db::SourceFile,
    offset: TextSize,
) -> Option<(Placeholder, Option<ModuleId>)> {
    let root = base_db::parse(db, file).syntax_node();
    let token = root
        .token_at_offset(offset)
        .find(|t| t.kind() == SyntaxKind::STRING_LITERAL)?;
    let sema = hir::Semantics::new(db);
    let found = match property_use(&token, |r| is_environment(db, &sema, file, r))? {
        PropertyUse::Value(list) => list
            .into_iter()
            .find(|p| p.key_range.contains_inclusive(offset))?,
        PropertyUse::Lookup(p) => p.key_range.contains_inclusive(offset).then_some(p)?,
    };
    let module = match base_db::file_entry(db, file.file_id(db))?.kind {
        base_db::FileKind::Workspace { module, .. } => module,
        base_db::FileKind::Library => return None,
    };
    Some((found, module))
}

/// 和 `key` 匹配的配置项（宽松绑定）。
pub fn matching_properties(
    db: &dyn Db,
    module: Option<ModuleId>,
    key: &str,
) -> Vec<ConfigProperty> {
    let wanted = canonical(key);
    config_properties(db, module)
        .into_iter()
        .filter(|p| canonical(&p.key) == wanted)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(text: &str, yaml: bool) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        let mut push = |k: String, v: String, r: TextRange| {
            let span = text[usize::from(r.start())..usize::from(r.end())].to_owned();
            out.push((k, v, span));
        };
        if yaml {
            parse_yaml(text, &mut push);
        } else {
            parse_properties(text, &mut push);
        }
        out
    }

    #[test]
    fn properties() {
        let got = collect(
            "# comment\nserver.port=8080\napp.name : demo\napp.list a,\\\n  b\n\n! bang\nurl=http://x:1\n",
            false,
        );
        let keys: Vec<(&str, &str)> = got
            .iter()
            .map(|(k, v, _)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("server.port", "8080"),
                ("app.name", "demo"),
                ("app.list", "a,b"),
                ("url", "http://x:1"),
            ]
        );
        assert_eq!(got[0].2, "server.port");
    }

    #[test]
    fn yaml() {
        let text = "server:\n  port: 8080 # web\nspring:\n  datasource:\n    url: \"jdbc:h2:mem\"\n  profiles:\n    active: dev\napp:\n  servers:\n    - a.example.com\n    - b.example.com\n  users:\n    - name: tom\n      role: admin\n  banner: |\n    hello\n    world\n  after: 1\n---\nother: 2\n";
        let got = collect(text, true);
        let keys: Vec<(&str, &str)> = got
            .iter()
            .map(|(k, v, _)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("server.port", "8080"),
                ("spring.datasource.url", "jdbc:h2:mem"),
                ("spring.profiles.active", "dev"),
                ("app.servers[0]", "a.example.com"),
                ("app.servers[1]", "b.example.com"),
                ("app.users[0].name", "tom"),
                ("app.users[0].role", "admin"),
                ("app.banner", ""),
                ("app.after", "1"),
                ("other", "2"),
            ]
        );
        assert_eq!(got[0].2, "port");
        assert_eq!(got[5].2, "name");
    }

    #[test]
    fn relaxed_binding() {
        assert_eq!(canonical("app.max-size"), canonical("app.maxSize"));
        assert_eq!(
            canonical("APP_MAXSIZE"),
            canonical("app.max-size").replace('.', "")
        );
    }
}
