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
