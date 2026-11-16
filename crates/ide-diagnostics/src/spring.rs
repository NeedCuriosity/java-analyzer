//! Spring 配置项：`@Value("${key}")` 引用的键在模块能看到的配置文件里都没有定义，也没有默认值。
//!
//! 只在模块确实有配置文件时检查（没有配置文件的模块多半靠环境变量或配置中心）。
//! 全大写的键（环境变量）、JVM 系统属性、`random.*`、`local.*` 这类运行时才有的键不报。

use base_db::{Db, SourceFile};
use ide_db::spring;
use syntax::SyntaxKind;

use crate::{Diagnostic, DiagnosticCode, Severity};

fn provided_at_runtime(key: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "random.", "local.", "user.", "java.", "os.", "file.", "line.", "path.", "sun.",
    ];
    key.is_empty()
        || key.contains("#{")
        || !key.chars().any(|c| c.is_lowercase())
        || PREFIXES.iter().any(|p| key.starts_with(p))
}

pub(crate) fn unresolved_properties(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    if base_db::workspace(db).resources(db).is_empty() || !file.text(db).contains("${") {
        return;
    }
    let module = match base_db::file_entry(db, file.file_id(db)).map(|e| e.kind) {
        Some(base_db::FileKind::Workspace { module, .. }) => module,
        _ => return,
    };
    if spring::visible_resources(db, module).is_empty() {
        return;
    }
    let defined: rustc_hash::FxHashSet<String> = spring::config_properties(db, module)
        .iter()
        .map(|p| spring::canonical(&p.key))
        .collect();
    let root = base_db::parse(db, file).syntax_node();
    for token in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::STRING_LITERAL && t.text().contains("${"))
    {
        let Some(spring::PropertyUse::Value(placeholders)) =
            spring::property_use(&token, |_| false)
        else {
            continue;
        };
        for placeholder in placeholders {
            if placeholder.has_default
                || provided_at_runtime(&placeholder.key)
                || defined.contains(&spring::canonical(&placeholder.key))
            {
                continue;
            }
            out.push(Diagnostic {
                range: placeholder.key_range,
                message: base_db::tr!(
                    "配置文件里没有定义 `{}`，也没有默认值",
                    "`{}` is not defined in any configuration file and has no default",
                    placeholder.key
                ),
                severity: Severity::Warning,
                code: DiagnosticCode::UnresolvedConfigProperty,
                unused: false,
            });
        }
    }
}

/// Spring 配置文件本身的检查：已弃用的配置项，以及已知分组下不存在的键。
///
/// 只有键的前缀属于某个已知分组（来自配置元数据或 `@ConfigurationProperties`）时才报未知的键：
/// 完全自定义的键（通过 `@Value` 使用）不报。
pub fn config_file_diagnostics(db: &dyn Db, resource: &base_db::ResourceFile) -> Vec<Diagnostic> {
    use ide_db::spring_metadata::{KeyMatch, MetadataSet};

    let mut out = Vec::new();
    if !spring::is_config_name(&resource.name) {
        return out;
    }
    let set = MetadataSet::load(db);
    if set.is_empty() {
        return out;
    }
    let mut keys = Vec::new();
    spring::parse_resource(resource, &mut keys);
    for property in keys {
        match set.resolve(&property.key) {
            KeyMatch::Property(meta) => {
                let Some(deprecation) = &meta.deprecation else {
                    continue;
                };
                let mut message = if deprecation.error {
                    base_db::tr!(
                        "配置项 `{}` 已不再支持",
                        "Configuration property `{}` is no longer supported",
                        meta.name
                    )
                } else {
                    base_db::tr!(
                        "配置项 `{}` 已弃用",
                        "Configuration property `{}` is deprecated",
                        meta.name
                    )
                };
                if let Some(replacement) = &deprecation.replacement {
                    message.push_str(&base_db::tr!(
                        "，请改用 `{}`",
                        ", use `{}` instead",
                        replacement
                    ));
                }
                if let Some(reason) = &deprecation.reason {
                    message.push_str(&base_db::tr!("：{}", ": {}", reason));
                }
                out.push(Diagnostic {
                    range: property.key_range,
                    message,
                    severity: Severity::Warning,
                    code: DiagnosticCode::DeprecatedConfigProperty,
                    unused: false,
                });
            }
            KeyMatch::Unknown { group: Some(group) } => out.push(Diagnostic {
                range: property.key_range,
                message: base_db::tr!(
                    "未知的配置项 `{}`（`{}` 下没有这个键）",
                    "Unknown configuration property `{}` (not defined under `{}`)",
                    property.key,
                    group.name
                ),
                severity: Severity::Warning,
                code: DiagnosticCode::UnknownConfigProperty,
                unused: false,
            }),
            _ => {}
        }
    }
    out
}
