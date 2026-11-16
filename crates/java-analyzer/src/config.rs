//! 服务器配置：来自 `initializationOptions` 和 `workspace/didChangeConfiguration`，
//! 以及从客户端能力里推导出来的开关。

use std::path::PathBuf;

use ide::{CompletionConfig, DiagnosticsConfig};
use lsp_types::{ClientCapabilities, PositionEncodingKind};
use project_model::LoadOptions;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
}

/// 用户可以设置的选项。字段名即 JSON 里的键（camelCase）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct UserConfig {
    pub jdk_home: Option<PathBuf>,
    pub maven: MavenConfig,
    pub gradle: GradleConfig,
    pub completion: CompletionUserConfig,
    pub diagnostics: DiagnosticsUserConfig,
    pub code_lens: CodeLensUserConfig,
    /// 打开新建的空 `.java` 文件时填上 package 声明和同名的类；不设置时开启
    pub new_file_template: Option<bool>,
    /// 跳进没有源码的依赖库类时，在后台下载它的 sources jar；不设置时开启
    pub download_sources: Option<bool>,
    /// 没有源码的依赖库类生成桩代码时反编译方法体；不设置时开启
    pub decompiler: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodeLensUserConfig {
    /// 在声明上方显示引用数。客户端自己会显示用法提示时可以关掉
    pub references: bool,
    /// 在类型 / 方法上方显示实现数
    pub implementations: bool,
    /// 点击 code lens 时执行的客户端命令，参数是 `[uri, position, locations]`
    /// （VS Code 的 `editor.action.showReferences` 的约定）。不设置时，VS Code 用这个命令，
    /// 其它客户端不带命令（只显示数量）
    pub command: Option<String>,
}

impl Default for CodeLensUserConfig {
    fn default() -> Self {
        CodeLensUserConfig {
            references: true,
            implementations: true,
            command: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MavenConfig {
    /// 本地仓库缺件时是否调用 `mvn`
    pub use_cli: bool,
}

impl Default for MavenConfig {
    fn default() -> Self {
        MavenConfig { use_cli: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GradleConfig {
    /// 是否允许运行 Gradle 来导入工程
    pub enabled: bool,
}

impl Default for GradleConfig {
    fn default() -> Self {
        GradleConfig { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompletionUserConfig {
    pub auto_import: bool,
    pub unimported_types: bool,
    pub postfix: bool,
    pub limit: usize,
}

impl Default for CompletionUserConfig {
    fn default() -> Self {
        let defaults = CompletionConfig::default();
        CompletionUserConfig {
            auto_import: defaults.auto_import,
            unimported_types: defaults.unimported_types,
            postfix: defaults.postfix,
            limit: defaults.limit,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiagnosticsUserConfig {
    pub semantic: bool,
    pub unused_imports: bool,
    pub unused_symbols: bool,
    pub inspections: bool,
    /// 整个工程的诊断（`workspace/diagnostic`）：`None` 表示工程不太大时才算
    pub workspace: Option<bool>,
}

impl Default for DiagnosticsUserConfig {
    fn default() -> Self {
        DiagnosticsUserConfig {
            semantic: true,
            unused_imports: true,
            unused_symbols: true,
            inspections: true,
            workspace: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub user: UserConfig,
    pub encoding: PositionEncoding,
    pub snippets: bool,
    /// 客户端会主动拉取诊断（3.17 pull 模型）；否则由服务器推送
    pub pull_diagnostics: bool,
    pub work_done_progress: bool,
    pub dynamic_watchers: bool,
    pub hierarchical_symbols: bool,
    pub location_links: bool,
    pub label_details: bool,
    pub markdown_hover: bool,
    /// 客户端支持在 `WorkspaceEdit` 里重命名文件
    pub rename_file_ops: bool,
    /// 客户端支持在 `WorkspaceEdit` 里新建文件
    pub create_file_ops: bool,
    /// 客户端支持 `workspace/diagnostic/refresh`
    pub diagnostic_refresh: bool,
    /// 客户端支持 `workspace/semanticTokens/refresh`
    pub semantic_tokens_refresh: bool,
    /// `initialize` 里客户端报的名字
    pub client_name: Option<String>,
    /// 客户端支持 `workspace/applyEdit`
    pub apply_edit: bool,
}

impl Config {
    pub fn new(caps: &ClientCapabilities, options: Option<serde_json::Value>) -> Config {
        let text_document = caps.text_document.as_ref();
        let completion_item = text_document
            .and_then(|td| td.completion.as_ref())
            .and_then(|c| c.completion_item.as_ref());

        let encodings = caps
            .general
            .as_ref()
            .and_then(|g| g.position_encodings.as_ref());
        let encoding = if encodings.is_some_and(|list| list.contains(&PositionEncodingKind::UTF8)) {
            PositionEncoding::Utf8
        } else {
            PositionEncoding::Utf16
        };

        let mut config = Config {
            user: UserConfig::default(),
            encoding,
            snippets: completion_item
                .and_then(|c| c.snippet_support)
                .unwrap_or(false),
            pull_diagnostics: text_document.is_some_and(|td| td.diagnostic.is_some()),
            work_done_progress: caps
                .window
                .as_ref()
                .and_then(|w| w.work_done_progress)
                .unwrap_or(false),
            dynamic_watchers: caps
                .workspace
                .as_ref()
                .and_then(|w| w.did_change_watched_files.as_ref())
                .and_then(|d| d.dynamic_registration)
                .unwrap_or(false),
            hierarchical_symbols: text_document
                .and_then(|td| td.document_symbol.as_ref())
                .and_then(|d| d.hierarchical_document_symbol_support)
                .unwrap_or(false),
            location_links: text_document
                .and_then(|td| td.definition.as_ref())
                .and_then(|d| d.link_support)
                .unwrap_or(false),
            label_details: completion_item
                .and_then(|c| c.label_details_support)
                .unwrap_or(false),
            markdown_hover: text_document
                .and_then(|td| td.hover.as_ref())
                .and_then(|h| h.content_format.as_ref())
                .is_none_or(|formats| formats.contains(&lsp_types::MarkupKind::Markdown)),
            rename_file_ops: caps
                .workspace
                .as_ref()
                .and_then(|w| w.workspace_edit.as_ref())
                .and_then(|edit| edit.resource_operations.as_ref())
                .is_some_and(|ops| ops.contains(&lsp_types::ResourceOperationKind::Rename)),
            create_file_ops: caps
                .workspace
                .as_ref()
                .and_then(|w| w.workspace_edit.as_ref())
                .and_then(|edit| edit.resource_operations.as_ref())
                .is_some_and(|ops| ops.contains(&lsp_types::ResourceOperationKind::Create)),
            diagnostic_refresh: caps
                .workspace
                .as_ref()
                .and_then(|w| w.diagnostic.as_ref())
                .and_then(|d| d.refresh_support)
                .unwrap_or(false),
            semantic_tokens_refresh: caps
                .workspace
                .as_ref()
                .and_then(|w| w.semantic_tokens.as_ref())
                .and_then(|s| s.refresh_support)
                .unwrap_or(false),
            client_name: None,
            apply_edit: caps
                .workspace
                .as_ref()
                .and_then(|w| w.apply_edit)
                .unwrap_or(false),
        };
        if let Some(options) = options {
            config.update(options);
        }
        config
    }

    /// 点击 code lens 时执行的命令。
    pub fn code_lens_command(&self) -> Option<String> {
        if let Some(command) = &self.user.code_lens.command {
            return (!command.is_empty()).then(|| command.clone());
        }
        self.client_name
            .as_deref()
            .is_some_and(|name| name.contains("Visual Studio Code") || name.contains("VSCodium"))
            .then(|| "editor.action.showReferences".to_owned())
    }

    /// 应用新的用户配置。接受 `{ "java-analyzer": {...} }` 或直接的 `{...}`。
    /// 返回工程导入相关的选项是否变了（需要重新导入）。
    pub fn update(&mut self, value: serde_json::Value) -> bool {
        let value = match value {
            serde_json::Value::Object(mut map) if map.contains_key("java-analyzer") => map
                .remove("java-analyzer")
                .unwrap_or(serde_json::Value::Null),
            other => other,
        };
        if value.is_null() {
            return false;
        }
        match serde_json::from_value::<UserConfig>(value) {
            Ok(user) => {
                let reload = user.jdk_home != self.user.jdk_home
                    || user.maven.use_cli != self.user.maven.use_cli
                    || user.gradle.enabled != self.user.gradle.enabled;
                self.user = user;
                reload
            }
            Err(err) => {
                tracing::warn!("忽略无法解析的配置: {err}");
                false
            }
        }
    }

    pub fn completion(&self) -> CompletionConfig {
        let defaults = CompletionConfig::default();
        CompletionConfig {
            snippets: self.snippets,
            auto_import: self.user.completion.auto_import,
            unimported_types: self.user.completion.unimported_types,
            postfix: self.user.completion.postfix,
            limit: self.user.completion.limit.max(1),
            ..defaults
        }
    }

    pub fn diagnostics(&self) -> DiagnosticsConfig {
        DiagnosticsConfig {
            semantic: self.user.diagnostics.semantic,
            unused_imports: self.user.diagnostics.unused_imports,
            unused_symbols: self.user.diagnostics.unused_symbols,
            inspections: self.user.diagnostics.inspections,
        }
    }

    pub fn load_options(&self) -> LoadOptions {
        LoadOptions {
            jdk_home: self.user.jdk_home.clone(),
            maven_use_cli: self.user.maven.use_cli,
            gradle_allow_run: self.user.gradle.enabled,
        }
    }
}
