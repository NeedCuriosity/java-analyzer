//! `java-analyzer`：Java 语言服务器。
//!
//! 不带参数（或 `lsp`）时在标准输入输出上说 LSP；`dap` 在标准输入输出上运行调试适配器；
//! 另有两个诊断用的子命令：
//! - `java-analyzer parse <路径>`：解析文件或目录，报告语法错误数和耗时；
//! - `java-analyzer analysis-stats <工程路径>`：导入工程并做全量分析，输出各阶段耗时和类型已知率；
//! - `java-analyzer decompile <jar> [类名]`：反编译 jar（或 JDK 的 `lib/modules`）里的类。

mod cli;
mod config;
mod global_state;
mod handlers;
mod library_files;
mod proto;
mod task_pool;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use lsp_server::Connection;
use lsp_types as lsp;

use crate::{
    config::{Config, PositionEncoding},
    global_state::GlobalState,
};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const USAGE_ZH: &str = "\
java-analyzer - Java 语言服务器

用法:
  java-analyzer [lsp]                 在标准输入输出上运行语言服务器
  java-analyzer dap                   在标准输入输出上运行调试适配器（Debug Adapter Protocol）
  java-analyzer parse <路径>          解析文件或目录，报告语法错误
  java-analyzer analysis-stats <路径>... 导入并分析整个工程（多个路径按多根工作区合并），输出统计
  java-analyzer decompile <jar> [类名或包前缀]
                                      反编译 jar（或 JDK 的 lib/modules）：给完整类名时输出源码，
                                      否则统计（前缀下）全部类的反编译成功率
  java-analyzer --version
  java-analyzer --help

环境变量:
  JA_LOG   日志过滤（tracing 语法，例如 `info` 或 `java_analyzer=debug`），日志写到标准错误
  LANG     命令行输出的语言：以 `zh` 开头时输出中文，否则输出英文（也看 LC_ALL / LC_MESSAGES）
";

const USAGE_EN: &str = "\
java-analyzer - a Java language server

Usage:
  java-analyzer [lsp]                 run the language server over stdio
  java-analyzer dap                   run the debug adapter (Debug Adapter Protocol) over stdio
  java-analyzer parse <path>          parse a file or directory and report syntax errors
  java-analyzer analysis-stats <path>... import and analyze a whole project (several paths form a multi-root workspace), print statistics
  java-analyzer decompile <jar> [class or package prefix]
                                      decompile a jar (or the JDK's lib/modules): print the source of one
                                      class, or report the success rate over all classes (under the prefix)
  java-analyzer --version
  java-analyzer --help

Environment:
  JA_LOG   log filter (tracing syntax, e.g. `info` or `java_analyzer=debug`); logs go to stderr
  LANG     language of command-line output: Chinese when it starts with `zh`, English otherwise
           (LC_ALL / LC_MESSAGES are honored too)
";

fn usage() -> &'static str {
    match base_db::lang() {
        base_db::Lang::Zh => USAGE_ZH,
        base_db::Lang::En => USAGE_EN,
    }
}

/// 命令行输出的语言，按 POSIX 的优先级取 `LC_ALL`、`LC_MESSAGES`、`LANG` 里第一个非空的。
/// 语言服务器在 `initialize` 时还会按客户端的 `locale` 再定一次。
fn lang_from_env() -> base_db::Lang {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty());
    match locale {
        Some(value) if !value.starts_with("zh") => base_db::Lang::En,
        _ => base_db::Lang::Zh,
    }
}

fn main() {
    // 日志只能写到 stderr：stdout 是 LSP / DAP 的传输通道
    let filter = std::env::var("JA_LOG").unwrap_or_else(|_| "warn".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    base_db::set_lang(lang_from_env());
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("lsp") => run_server(),
        Some("dap") => dap::run_stdio(),
        Some("--version") | Some("-V") => {
            println!("java-analyzer {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("-h") => {
            print!("{}", usage());
            Ok(())
        }
        Some("parse") => match args.get(1) {
            Some(path) => cli::parse(&PathBuf::from(path)),
            None => Err(anyhow::anyhow!(base_db::trs!(
                "parse 需要一个路径参数",
                "parse needs a path argument"
            ))),
        },
        Some("analysis-stats") => match &args[1..] {
            [] => Err(anyhow::anyhow!(base_db::trs!(
                "analysis-stats 需要一个路径参数",
                "analysis-stats needs a path argument"
            ))),
            paths => cli::analysis_stats(&paths.iter().map(PathBuf::from).collect::<Vec<_>>()),
        },
        Some("decompile") => match args.get(1) {
            Some(path) => cli::decompile(&PathBuf::from(path), args.get(2).map(String::as_str)),
            None => Err(anyhow::anyhow!(base_db::trs!(
                "decompile 需要一个 jar 路径参数",
                "decompile needs a jar path argument"
            ))),
        },
        Some(other) => Err(anyhow::anyhow!(base_db::tr!(
            "未知的子命令 `{other}`\n\n{}",
            "Unknown subcommand `{other}`\n\n{}",
            usage()
        ))),
    };
    if let Err(err) = result {
        eprintln!("{}{err:#}", base_db::trs!("错误: ", "error: "));
        std::process::exit(1);
    }
}

/// 工作区的根目录。多根工作区的每个文件夹各自作为一个根导入。
#[allow(deprecated)] // `root_uri` 已被 `workspace_folders` 取代，但旧客户端只发前者
fn workspace_roots(params: &lsp::InitializeParams) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = params
        .workspace_folders
        .iter()
        .flatten()
        .filter_map(|folder| folder.uri.to_file_path().ok())
        .collect();
    if roots.is_empty() {
        roots.extend(
            params
                .root_uri
                .as_ref()
                .and_then(|uri| uri.to_file_path().ok()),
        );
    }
    if roots.is_empty() {
        roots.push(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    }
    roots.dedup();
    roots
}

fn server_capabilities(config: &Config) -> anyhow::Result<serde_json::Value> {
    let legend = lsp::SemanticTokensLegend {
        token_types: proto::TOKEN_TYPES
            .iter()
            .map(|name| lsp::SemanticTokenType::new(name))
            .collect(),
        token_modifiers: proto::TOKEN_MODIFIERS
            .iter()
            .map(|name| lsp::SemanticTokenModifier::new(name))
            .collect(),
    };
    let capabilities = lsp::ServerCapabilities {
        position_encoding: Some(match config.encoding {
            PositionEncoding::Utf8 => lsp::PositionEncodingKind::UTF8,
            PositionEncoding::Utf16 => lsp::PositionEncodingKind::UTF16,
        }),
        text_document_sync: Some(lsp::TextDocumentSyncCapability::Options(
            lsp::TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(lsp::TextDocumentSyncKind::INCREMENTAL),
                will_save: None,
                will_save_wait_until: None,
                save: Some(lsp::TextDocumentSyncSaveOptions::Supported(true)),
            },
        )),
        completion_provider: Some(lsp::CompletionOptions {
            resolve_provider: Some(true),
            trigger_characters: Some(vec![
                ".".to_owned(),
                "@".to_owned(),
                ":".to_owned(),
                // 文档注释里的 `{@link Foo#`
                "#".to_owned(),
            ]),
            ..Default::default()
        }),
        signature_help_provider: Some(lsp::SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
        hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
        definition_provider: Some(lsp::OneOf::Left(true)),
        declaration_provider: Some(lsp::DeclarationCapability::Simple(true)),
        type_definition_provider: Some(lsp::TypeDefinitionProviderCapability::Simple(true)),
        implementation_provider: Some(lsp::ImplementationProviderCapability::Simple(true)),
        references_provider: Some(lsp::OneOf::Left(true)),
        document_highlight_provider: Some(lsp::OneOf::Left(true)),
        document_symbol_provider: Some(lsp::OneOf::Left(true)),
        workspace_symbol_provider: Some(lsp::OneOf::Left(true)),
        workspace: Some(lsp::WorkspaceServerCapabilities {
            workspace_folders: Some(lsp::WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(lsp::OneOf::Left(true)),
            }),
            // 移动 / 重命名 `.java` 文件或目录时同步 package、import 和类名
            file_operations: Some(lsp::WorkspaceFileOperationsServerCapabilities {
                will_rename: Some(lsp::FileOperationRegistrationOptions {
                    filters: vec![
                        lsp::FileOperationFilter {
                            scheme: Some("file".to_owned()),
                            pattern: lsp::FileOperationPattern {
                                glob: "**/*.java".to_owned(),
                                matches: Some(lsp::FileOperationPatternKind::File),
                                options: None,
                            },
                        },
                        // 目录（包）：其下的源文件一起换包
                        lsp::FileOperationFilter {
                            scheme: Some("file".to_owned()),
                            pattern: lsp::FileOperationPattern {
                                glob: "**/*".to_owned(),
                                matches: Some(lsp::FileOperationPatternKind::Folder),
                                options: None,
                            },
                        },
                    ],
                }),
                ..Default::default()
            }),
        }),
        folding_range_provider: Some(lsp::FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(lsp::SelectionRangeProviderCapability::Simple(true)),
        call_hierarchy_provider: Some(lsp::CallHierarchyServerCapability::Simple(true)),
        rename_provider: Some(lsp::OneOf::Right(lsp::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        code_action_provider: Some(lsp::CodeActionProviderCapability::Options(
            lsp::CodeActionOptions {
                code_action_kinds: Some(vec![
                    lsp::CodeActionKind::QUICKFIX,
                    lsp::CodeActionKind::REFACTOR,
                    lsp::CodeActionKind::REFACTOR_EXTRACT,
                    lsp::CodeActionKind::REFACTOR_REWRITE,
                    lsp::CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                    lsp::CodeActionKind::SOURCE_FIX_ALL,
                    lsp::CodeActionKind::new("source.generate"),
                ]),
                resolve_provider: None,
                work_done_progress_options: Default::default(),
            },
        )),
        execute_command_provider: Some(lsp::ExecuteCommandOptions {
            commands: vec![handlers::ACCEPT_SIGNATURE_COMMAND.to_owned()],
            work_done_progress_options: Default::default(),
        }),
        document_formatting_provider: Some(lsp::OneOf::Left(true)),
        document_range_formatting_provider: Some(lsp::OneOf::Left(true)),
        document_on_type_formatting_provider: Some(lsp::DocumentOnTypeFormattingOptions {
            first_trigger_character: "}".to_owned(),
            more_trigger_character: Some(vec![";".to_owned(), "\n".to_owned()]),
        }),
        inlay_hint_provider: Some(lsp::OneOf::Left(true)),
        code_lens_provider: Some(lsp::CodeLensOptions {
            resolve_provider: Some(true),
        }),
        semantic_tokens_provider: Some(
            lsp::SemanticTokensServerCapabilities::SemanticTokensOptions(
                lsp::SemanticTokensOptions {
                    legend,
                    full: Some(lsp::SemanticTokensFullOptions::Delta { delta: Some(true) }),
                    range: Some(true),
                    work_done_progress_options: Default::default(),
                },
            ),
        ),
        diagnostic_provider: Some(lsp::DiagnosticServerCapabilities::Options(
            lsp::DiagnosticOptions {
                identifier: Some("java-analyzer".to_owned()),
                inter_file_dependencies: true,
                workspace_diagnostics: true,
                work_done_progress_options: Default::default(),
            },
        )),
        ..Default::default()
    };
    let mut value = serde_json::to_value(capabilities)?;
    // `ServerCapabilities` 结构体里不一定有这个字段，直接写进 JSON
    if let Some(map) = value.as_object_mut() {
        map.insert(
            "typeHierarchyProvider".to_owned(),
            serde_json::Value::Bool(true),
        );
    }
    Ok(value)
}

fn run_server() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    serve(connection)?;
    io_threads.join()?;
    Ok(())
}

/// 在一条已建立的连接上完成握手并运行主循环，直到客户端发来 `exit`。
fn serve(connection: Connection) -> anyhow::Result<()> {
    let (id, params) = connection.initialize_start()?;
    let params: lsp::InitializeParams = serde_json::from_value(params)?;
    let mut config = Config::new(&params.capabilities, params.initialization_options.clone());
    config.client_name = params.client_info.as_ref().map(|info| info.name.clone());
    // 界面文案的语言跟随客户端：`zh` 开头的 locale 用中文，其余（包括没有声明的）用英文
    let chinese = params
        .locale
        .as_deref()
        .is_some_and(|locale| locale.to_ascii_lowercase().starts_with("zh"));
    base_db::set_lang(if chinese {
        base_db::Lang::Zh
    } else {
        base_db::Lang::En
    });
    let roots = workspace_roots(&params);
    tracing::info!("工作区根目录: {roots:?}");

    let result = serde_json::json!({
        "capabilities": server_capabilities(&config)?,
        "serverInfo": { "name": "java-analyzer", "version": env!("CARGO_PKG_VERSION") },
    });
    connection.initialize_finish(id, result)?;

    let state = GlobalState::new(connection.sender.clone(), config, roots);
    state.run(connection)
}
