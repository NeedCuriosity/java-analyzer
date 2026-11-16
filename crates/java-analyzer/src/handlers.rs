//! 各个 LSP 请求的处理函数。都运行在线程池上，输入是快照和已解析的参数。
//!
//! 返回 `Err(Cancelled)`（经 `?` 转成 `anyhow::Error`）表示查询被新的编辑打断，
//! 分发器会把它变成 `ContentModified`。

use anyhow::Context;
use ide::{FileId, FilePosition, NavigationTarget, RangeInfo};
use lsp_types::{self as lsp, Url};

use crate::{
    global_state::Snapshot,
    proto::{self, Doc},
};

/// 一次请求里允许物化的依赖库文件个数上限（每个都可能要解压）。
const LIBRARY_BUDGET: usize = 20;

fn position(
    snap: &Snapshot,
    params: &lsp::TextDocumentPositionParams,
) -> anyhow::Result<(FilePosition, Doc)> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let offset = doc.offset(params.position);
    Ok((FilePosition { file_id, offset }, doc))
}

fn key_from_data(data: Option<&serde_json::Value>) -> anyhow::Result<ide::DefKey> {
    data.and_then(proto::def_key_from_json)
        .context(base_db::trs!(
            "条目缺少有效的 data 字段",
            "The item has no valid data field"
        ))
}

// ---------------------------------------------------------------------------
// 补全
// ---------------------------------------------------------------------------

pub fn completion(
    snap: Snapshot,
    params: lsp::CompletionParams,
) -> anyhow::Result<Option<lsp::CompletionResponse>> {
    let (position, doc) = position(&snap, &params.text_document_position)?;
    let config = snap.config.completion();
    let list = snap.analysis.completions(position, &config)?;

    let items =
        list.items
            .iter()
            .enumerate()
            .map(|(rank, item)| {
                let additional: Vec<lsp::TextEdit> = item
                    .additional_edits
                    .iter()
                    .map(|edit| proto::text_edit(&doc, edit))
                    .collect();
                lsp::CompletionItem {
                    label: item.label.clone(),
                    label_details: snap.config.label_details.then(|| {
                        lsp::CompletionItemLabelDetails {
                            detail: item.label_detail.clone(),
                            description: item.detail.clone(),
                        }
                    }),
                    kind: Some(proto::completion_item_kind(item.kind)),
                    detail: item.detail.clone(),
                    // 列表已按相关度排好，用名次保证客户端不打乱
                    sort_text: Some(format!("{rank:05}")),
                    filter_text: Some(item.lookup.clone()),
                    insert_text_format: Some(if item.is_snippet {
                        lsp::InsertTextFormat::SNIPPET
                    } else {
                        lsp::InsertTextFormat::PLAIN_TEXT
                    }),
                    text_edit: Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit {
                        range: doc.range(item.source_range),
                        new_text: item.insert_text.clone(),
                    })),
                    additional_text_edits: (!additional.is_empty()).then_some(additional),
                    tags: item
                        .deprecated
                        .then(|| vec![lsp::CompletionItemTag::DEPRECATED]),
                    data: item.resolve.as_ref().map(proto::def_key_to_json),
                    ..Default::default()
                }
            })
            .collect();
    Ok(Some(lsp::CompletionResponse::List(lsp::CompletionList {
        is_incomplete: list.is_incomplete,
        items,
    })))
}

pub fn completion_resolve(
    snap: Snapshot,
    mut item: lsp::CompletionItem,
) -> anyhow::Result<lsp::CompletionItem> {
    let Some(key) = item.data.as_ref().and_then(proto::def_key_from_json) else {
        return Ok(item);
    };
    if let Some(info) = snap.analysis.resolve_completion(&key)? {
        if !info.signature.is_empty() {
            item.detail = Some(info.signature.clone());
        }
        if let Some(docs) = info.docs {
            item.documentation = Some(lsp::Documentation::MarkupContent(lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: docs,
            }));
        }
    }
    Ok(item)
}

pub fn signature_help(
    snap: Snapshot,
    params: lsp::SignatureHelpParams,
) -> anyhow::Result<Option<lsp::SignatureHelp>> {
    let (position, _) = position(&snap, &params.text_document_position_params)?;
    let Some(help) = snap.analysis.signature_help(position)? else {
        return Ok(None);
    };
    let signatures = help
        .signatures
        .into_iter()
        .map(|sig| {
            let parameters = sig
                .params
                .iter()
                .map(|(start, end)| lsp::ParameterInformation {
                    label: lsp::ParameterLabel::LabelOffsets([
                        proto::utf16_offset(&sig.label, *start),
                        proto::utf16_offset(&sig.label, *end),
                    ]),
                    documentation: None,
                })
                .collect();
            lsp::SignatureInformation {
                documentation: sig.docs.map(|docs| {
                    lsp::Documentation::MarkupContent(lsp::MarkupContent {
                        kind: lsp::MarkupKind::Markdown,
                        value: docs,
                    })
                }),
                parameters: Some(parameters),
                active_parameter: None,
                label: sig.label,
            }
        })
        .collect();
    Ok(Some(lsp::SignatureHelp {
        signatures,
        active_signature: Some(help.active_signature as u32),
        active_parameter: help.active_parameter.map(|p| p as u32),
    }))
}

// ---------------------------------------------------------------------------
// 悬停与跳转
// ---------------------------------------------------------------------------

pub fn hover(snap: Snapshot, params: lsp::HoverParams) -> anyhow::Result<Option<lsp::Hover>> {
    let (position, doc) = position(&snap, &params.text_document_position_params)?;
    let Some(RangeInfo { range, info }) = snap.analysis.hover(position)? else {
        return Ok(None);
    };
    let contents = if snap.config.markdown_hover {
        lsp::HoverContents::Markup(lsp::MarkupContent {
            kind: lsp::MarkupKind::Markdown,
            value: info.to_markdown(),
        })
    } else {
        let mut value = info.signature.clone();
        if let Some(docs) = &info.docs {
            value.push_str("\n\n");
            value.push_str(docs);
        }
        lsp::HoverContents::Markup(lsp::MarkupContent {
            kind: lsp::MarkupKind::PlainText,
            value,
        })
    };
    Ok(Some(lsp::Hover {
        contents,
        range: Some(doc.range(range)),
    }))
}

fn goto_response(
    snap: &Snapshot,
    doc: &Doc,
    result: Option<RangeInfo<Vec<NavigationTarget>>>,
) -> anyhow::Result<Option<lsp::GotoDefinitionResponse>> {
    let Some(RangeInfo {
        range,
        info: targets,
    }) = result
    else {
        return Ok(None);
    };
    let locations = proto::nav_locations(snap, &targets, LIBRARY_BUDGET)?;
    // 跳进了没有源码的库类：后台去下载它的 sources jar
    if snap.config.user.download_sources.unwrap_or(true) {
        for target in &targets {
            if let ide::NavFile::Library { class, .. } = &target.file {
                snap.libraries.want_sources(&snap.analysis, class)?;
            }
        }
    }
    if snap.config.location_links {
        let origin = doc.range(range);
        let links = locations
            .into_iter()
            .map(|(_, location)| lsp::LocationLink {
                origin_selection_range: Some(origin),
                target_uri: location.url,
                target_range: location.full_range,
                target_selection_range: location.focus_range,
            })
            .collect();
        Ok(Some(lsp::GotoDefinitionResponse::Link(links)))
    } else {
        let plain = locations
            .into_iter()
            .map(|(_, location)| lsp::Location {
                uri: location.url,
                range: location.focus_range,
            })
            .collect();
        Ok(Some(lsp::GotoDefinitionResponse::Array(plain)))
    }
}

pub fn goto_definition(
    snap: Snapshot,
    params: lsp::GotoDefinitionParams,
) -> anyhow::Result<Option<lsp::GotoDefinitionResponse>> {
    let (position, doc) = position(&snap, &params.text_document_position_params)?;
    let result = snap.analysis.goto_definition(position)?;
    goto_response(&snap, &doc, result)
}

pub fn goto_declaration(
    snap: Snapshot,
    params: lsp::GotoDefinitionParams,
) -> anyhow::Result<Option<lsp::GotoDefinitionResponse>> {
    let (position, doc) = position(&snap, &params.text_document_position_params)?;
    let result = snap.analysis.goto_declaration(position)?;
    goto_response(&snap, &doc, result)
}

pub fn goto_type_definition(
    snap: Snapshot,
    params: lsp::GotoDefinitionParams,
) -> anyhow::Result<Option<lsp::GotoDefinitionResponse>> {
    let (position, doc) = position(&snap, &params.text_document_position_params)?;
    let result = snap.analysis.goto_type_definition(position)?;
    goto_response(&snap, &doc, result)
}

pub fn goto_implementation(
    snap: Snapshot,
    params: lsp::GotoDefinitionParams,
) -> anyhow::Result<Option<lsp::GotoDefinitionResponse>> {
    let (position, doc) = position(&snap, &params.text_document_position_params)?;
    let result = snap.analysis.goto_implementation(position)?;
    goto_response(&snap, &doc, result)
}

pub fn references(
    snap: Snapshot,
    params: lsp::ReferenceParams,
) -> anyhow::Result<Option<Vec<lsp::Location>>> {
    let (position, _) = position(&snap, &params.text_document_position)?;
    let Some(refs) = snap
        .analysis
        .find_all_refs(position, params.context.include_declaration)?
    else {
        return Ok(None);
    };
    let mut res = Vec::with_capacity(refs.len());
    // 引用按文件排好了序：同一个文件只取一次文本和行索引
    let mut current: Option<(FileId, Url, Doc)> = None;
    for item in refs {
        if current
            .as_ref()
            .is_none_or(|(file_id, _, _)| *file_id != item.file_id)
        {
            let Some(url) = snap.url(item.file_id) else {
                continue;
            };
            current = Some((item.file_id, url, snap.doc(item.file_id)?));
        }
        if let Some((_, url, doc)) = &current {
            res.push(lsp::Location {
                uri: url.clone(),
                range: doc.range(item.range),
            });
        }
    }
    Ok(Some(res))
}

pub fn document_highlight(
    snap: Snapshot,
    params: lsp::DocumentHighlightParams,
) -> anyhow::Result<Option<Vec<lsp::DocumentHighlight>>> {
    let (position, doc) = position(&snap, &params.text_document_position_params)?;
    let Some(highlights) = snap.analysis.highlight_related(position)? else {
        return Ok(None);
    };
    Ok(Some(
        highlights
            .into_iter()
            .map(|hl| lsp::DocumentHighlight {
                range: doc.range(hl.range),
                kind: Some(if hl.is_write {
                    lsp::DocumentHighlightKind::WRITE
                } else {
                    lsp::DocumentHighlightKind::READ
                }),
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// 符号与结构
// ---------------------------------------------------------------------------

pub fn document_symbol(
    snap: Snapshot,
    params: lsp::DocumentSymbolParams,
) -> anyhow::Result<Option<lsp::DocumentSymbolResponse>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let structure = snap.analysis.file_structure(file_id)?;
    if snap.config.hierarchical_symbols {
        let symbols = structure
            .iter()
            .map(|node| proto::document_symbol(&doc, node))
            .collect();
        Ok(Some(lsp::DocumentSymbolResponse::Nested(symbols)))
    } else {
        let mut flat = Vec::new();
        proto::flat_symbols(&doc, &params.text_document.uri, &structure, None, &mut flat);
        Ok(Some(lsp::DocumentSymbolResponse::Flat(flat)))
    }
}

pub fn workspace_symbol(
    snap: Snapshot,
    params: lsp::WorkspaceSymbolParams,
) -> anyhow::Result<Option<lsp::WorkspaceSymbolResponse>> {
    let navs = snap.analysis.workspace_symbols(&params.query, 128)?;
    // 依赖库里的类要物化成文件才有位置（有缓存），每次最多物化排在前面的这么多个
    const LIBRARY_SYMBOLS: usize = 20;
    let symbols = proto::nav_locations(&snap, &navs, LIBRARY_SYMBOLS)?
        .into_iter()
        .map(|(idx, location)| proto::symbol_information(&navs[idx], location))
        .collect();
    Ok(Some(lsp::WorkspaceSymbolResponse::Flat(symbols)))
}

pub fn folding_range(
    snap: Snapshot,
    params: lsp::FoldingRangeParams,
) -> anyhow::Result<Option<Vec<lsp::FoldingRange>>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let folds = snap.analysis.folding_ranges(file_id)?;
    Ok(Some(
        folds
            .iter()
            .filter_map(|fold| proto::folding_range(&doc, fold))
            .collect(),
    ))
}

pub fn selection_range(
    snap: Snapshot,
    params: lsp::SelectionRangeParams,
) -> anyhow::Result<Option<Vec<lsp::SelectionRange>>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let mut res = Vec::with_capacity(params.positions.len());
    for lsp_position in params.positions {
        let offset = doc.offset(lsp_position);
        let ranges = snap
            .analysis
            .selection_ranges(FilePosition { file_id, offset })?;
        // 由外到内串成链表
        let mut current: Option<lsp::SelectionRange> = None;
        for range in ranges.into_iter().rev() {
            current = Some(lsp::SelectionRange {
                range: doc.range(range),
                parent: current.map(Box::new),
            });
        }
        res.push(current.unwrap_or(lsp::SelectionRange {
            range: lsp::Range {
                start: lsp_position,
                end: lsp_position,
            },
            parent: None,
        }));
    }
    Ok(Some(res))
}

// ---------------------------------------------------------------------------
// 语义高亮
// ---------------------------------------------------------------------------

pub fn semantic_tokens_full(
    snap: Snapshot,
    params: lsp::SemanticTokensParams,
) -> anyhow::Result<Option<lsp::SemanticTokensResult>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let ranges = snap.analysis.highlight(file_id, None)?;
    let data = proto::semantic_tokens(&doc, ranges);
    let result_id = snap
        .semantic_tokens
        .store(params.text_document.uri, data.clone());
    Ok(Some(lsp::SemanticTokensResult::Tokens(
        lsp::SemanticTokens {
            result_id: Some(result_id),
            data,
        },
    )))
}

/// 增量语义高亮：只返回相对上一次结果变化的那一段。
pub fn semantic_tokens_full_delta(
    snap: Snapshot,
    params: lsp::SemanticTokensDeltaParams,
) -> anyhow::Result<Option<lsp::SemanticTokensFullDeltaResult>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let ranges = snap.analysis.highlight(file_id, None)?;
    let data = proto::semantic_tokens(&doc, ranges);

    let previous = snap
        .semantic_tokens
        .get(&params.text_document.uri, &params.previous_result_id);
    let result_id = snap
        .semantic_tokens
        .store(params.text_document.uri, data.clone());
    Ok(Some(match previous {
        Some(previous) => {
            lsp::SemanticTokensFullDeltaResult::TokensDelta(lsp::SemanticTokensDelta {
                result_id: Some(result_id),
                edits: proto::semantic_tokens_edits(&previous, &data),
            })
        }
        // 客户端手里的版本我们已经不记得了：退回全量
        None => lsp::SemanticTokensFullDeltaResult::Tokens(lsp::SemanticTokens {
            result_id: Some(result_id),
            data,
        }),
    }))
}

pub fn semantic_tokens_range(
    snap: Snapshot,
    params: lsp::SemanticTokensRangeParams,
) -> anyhow::Result<Option<lsp::SemanticTokensRangeResult>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let range = doc.text_range(params.range);
    let ranges = snap.analysis.highlight(file_id, Some(range))?;
    Ok(Some(lsp::SemanticTokensRangeResult::Tokens(
        lsp::SemanticTokens {
            result_id: None,
            data: proto::semantic_tokens(&doc, ranges),
        },
    )))
}

// ---------------------------------------------------------------------------
// 诊断
// ---------------------------------------------------------------------------

/// 计算一个文件的诊断（push 与 pull 共用）。文件已不存在时返回 `None`。
pub fn compute_diagnostics(
    snap: &Snapshot,
    file_id: FileId,
) -> anyhow::Result<Option<(Url, Vec<lsp::Diagnostic>)>> {
    let Some(url) = snap.url(file_id) else {
        return Ok(None);
    };
    let doc = snap.doc(file_id)?;
    let config = snap.config.diagnostics();
    let diagnostics = snap.analysis.diagnostics(file_id, &config)?;
    Ok(Some((
        url,
        diagnostics
            .iter()
            .map(|d| proto::diagnostic(&doc, d))
            .collect(),
    )))
}

pub fn document_diagnostic(
    snap: Snapshot,
    params: lsp::DocumentDiagnosticParams,
) -> anyhow::Result<lsp::DocumentDiagnosticReportResult> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let items = compute_diagnostics(&snap, file_id)?
        .map(|(_, items)| items)
        .unwrap_or_default();
    Ok(lsp::DocumentDiagnosticReportResult::Report(
        lsp::DocumentDiagnosticReport::Full(lsp::RelatedFullDocumentDiagnosticReport {
            related_documents: None,
            full_document_diagnostic_report: lsp::FullDocumentDiagnosticReport {
                result_id: None,
                items,
            },
        }),
    ))
}

/// 不设置 `diagnostics.workspace` 时，工程的源文件不超过这个数才计算整个工程的诊断。
const WORKSPACE_DIAGNOSTICS_AUTO_LIMIT: usize = 3000;

/// `workspace/diagnostic`：整个工程里的错误和警告（提示级别的不算），供客户端的问题列表使用。
/// 和上次结果相同的文件只回 `unchanged`。
pub fn workspace_diagnostic(
    snap: Snapshot,
    params: lsp::WorkspaceDiagnosticParams,
) -> anyhow::Result<lsp::WorkspaceDiagnosticReportResult> {
    use std::hash::{Hash, Hasher};

    let empty = || {
        lsp::WorkspaceDiagnosticReportResult::Report(lsp::WorkspaceDiagnosticReport {
            items: Vec::new(),
        })
    };
    let files = snap.analysis.workspace_files()?;
    let enabled = snap
        .config
        .user
        .diagnostics
        .workspace
        .unwrap_or(files.len() <= WORKSPACE_DIAGNOSTICS_AUTO_LIMIT);
    if !enabled {
        return Ok(empty());
    }
    let previous: std::collections::HashMap<Url, String> = params
        .previous_result_ids
        .into_iter()
        .map(|p| (p.uri, p.value))
        .collect();
    let config = snap.config.diagnostics();
    let mut items = Vec::new();
    for file_id in files {
        let Some(url) = snap.url(file_id) else {
            continue;
        };
        let diagnostics: Vec<ide::Diagnostic> = snap
            .analysis
            .diagnostics(file_id, &config)?
            .into_iter()
            .filter(|d| d.severity != ide::Severity::Hint)
            .collect();
        // 没有问题、上次也没报过的文件不必出现在结果里
        if diagnostics.is_empty() && !previous.contains_key(&url) {
            continue;
        }
        let doc = snap.doc(file_id)?;
        let lsp_items: Vec<lsp::Diagnostic> = diagnostics
            .iter()
            .map(|d| proto::diagnostic(&doc, d))
            .collect();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(&lsp_items)?.hash(&mut hasher);
        let result_id = format!("{:016x}", hasher.finish());
        let report = if previous.get(&url) == Some(&result_id) {
            lsp::WorkspaceDocumentDiagnosticReport::Unchanged(
                lsp::WorkspaceUnchangedDocumentDiagnosticReport {
                    uri: url,
                    version: None,
                    unchanged_document_diagnostic_report: lsp::UnchangedDocumentDiagnosticReport {
                        result_id,
                    },
                },
            )
        } else {
            lsp::WorkspaceDocumentDiagnosticReport::Full(
                lsp::WorkspaceFullDocumentDiagnosticReport {
                    uri: url,
                    version: None,
                    full_document_diagnostic_report: lsp::FullDocumentDiagnosticReport {
                        result_id: Some(result_id),
                        items: lsp_items,
                    },
                },
            )
        };
        items.push(report);
    }
    Ok(lsp::WorkspaceDiagnosticReportResult::Report(
        lsp::WorkspaceDiagnosticReport { items },
    ))
}

// ---------------------------------------------------------------------------
// 调用层级
// ---------------------------------------------------------------------------

pub fn call_hierarchy_prepare(
    snap: Snapshot,
    params: lsp::CallHierarchyPrepareParams,
) -> anyhow::Result<Option<Vec<lsp::CallHierarchyItem>>> {
    let (position, _) = position(&snap, &params.text_document_position_params)?;
    let Some(navs) = snap.analysis.prepare_call_hierarchy(position)? else {
        return Ok(None);
    };
    let items = proto::nav_locations(&snap, &navs, LIBRARY_BUDGET)?
        .into_iter()
        .map(|(idx, location)| proto::call_hierarchy_item(&navs[idx], location))
        .collect();
    Ok(Some(items))
}

pub fn incoming_calls(
    snap: Snapshot,
    params: lsp::CallHierarchyIncomingCallsParams,
) -> anyhow::Result<Option<Vec<lsp::CallHierarchyIncomingCall>>> {
    let key = key_from_data(params.item.data.as_ref())?;
    let calls = snap.analysis.incoming_calls(&key)?;
    let mut res = Vec::with_capacity(calls.len());
    for call in calls {
        // 调用点位于调用者的文件里
        let ide::NavFile::Source(caller_file) = call.target.file else {
            continue;
        };
        let doc = snap.doc(caller_file)?;
        let Some(location) = proto::nav_location(&snap, &call.target)? else {
            continue;
        };
        res.push(lsp::CallHierarchyIncomingCall {
            from: proto::call_hierarchy_item(&call.target, location),
            from_ranges: call.ranges.iter().map(|range| doc.range(*range)).collect(),
        });
    }
    Ok(Some(res))
}

pub fn outgoing_calls(
    snap: Snapshot,
    params: lsp::CallHierarchyOutgoingCallsParams,
) -> anyhow::Result<Option<Vec<lsp::CallHierarchyOutgoingCall>>> {
    let key = key_from_data(params.item.data.as_ref())?;
    // 调用点位于被查询方法自己的文件里
    let file_id = snap.file_id(&params.item.uri)?;
    let doc = snap.doc(file_id)?;
    let calls = snap.analysis.outgoing_calls(&key)?;
    let mut res = Vec::with_capacity(calls.len());
    let mut budget = LIBRARY_BUDGET;
    for call in calls {
        if matches!(call.target.file, ide::NavFile::Library { .. }) {
            if budget == 0 {
                continue;
            }
            budget -= 1;
        }
        let Some(location) = proto::nav_location(&snap, &call.target)? else {
            continue;
        };
        res.push(lsp::CallHierarchyOutgoingCall {
            to: proto::call_hierarchy_item(&call.target, location),
            from_ranges: call.ranges.iter().map(|range| doc.range(*range)).collect(),
        });
    }
    Ok(Some(res))
}

// ---------------------------------------------------------------------------
// 类型层级
// ---------------------------------------------------------------------------

fn type_items(
    snap: &Snapshot,
    navs: Vec<NavigationTarget>,
) -> anyhow::Result<Vec<lsp::TypeHierarchyItem>> {
    Ok(proto::nav_locations(snap, &navs, LIBRARY_BUDGET)?
        .into_iter()
        .map(|(idx, location)| proto::type_hierarchy_item(&navs[idx], location))
        .collect())
}

pub fn type_hierarchy_prepare(
    snap: Snapshot,
    params: lsp::TypeHierarchyPrepareParams,
) -> anyhow::Result<Option<Vec<lsp::TypeHierarchyItem>>> {
    let (position, _) = position(&snap, &params.text_document_position_params)?;
    let Some(navs) = snap.analysis.prepare_type_hierarchy(position)? else {
        return Ok(None);
    };
    Ok(Some(type_items(&snap, navs)?))
}

pub fn supertypes(
    snap: Snapshot,
    params: lsp::TypeHierarchySupertypesParams,
) -> anyhow::Result<Option<Vec<lsp::TypeHierarchyItem>>> {
    let key = key_from_data(params.item.data.as_ref())?;
    let navs = snap.analysis.supertypes(&key)?;
    Ok(Some(type_items(&snap, navs)?))
}

pub fn subtypes(
    snap: Snapshot,
    params: lsp::TypeHierarchySubtypesParams,
) -> anyhow::Result<Option<Vec<lsp::TypeHierarchyItem>>> {
    let key = key_from_data(params.item.data.as_ref())?;
    let navs = snap.analysis.subtypes(&key)?;
    Ok(Some(type_items(&snap, navs)?))
}

// ---------------------------------------------------------------------------
// 修改代码：重命名、代码操作、格式化
// ---------------------------------------------------------------------------

/// 用户可见的操作失败（名字冲突、目标在依赖库里 …）：以 `RequestFailed` 回复，
/// 编辑器会把消息直接展示给用户。
#[derive(Debug)]
pub struct UserError(pub String);

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UserError {}

pub fn prepare_rename(
    snap: Snapshot,
    params: lsp::TextDocumentPositionParams,
) -> anyhow::Result<Option<lsp::PrepareRenameResponse>> {
    let (position, doc) = position(&snap, &params)?;
    match snap.analysis.prepare_rename(position)? {
        Ok(info) => Ok(Some(lsp::PrepareRenameResponse::RangeWithPlaceholder {
            range: doc.range(info.range),
            placeholder: info.info,
        })),
        Err(message) => Err(UserError(message).into()),
    }
}

pub fn rename(
    snap: Snapshot,
    params: lsp::RenameParams,
) -> anyhow::Result<Option<lsp::WorkspaceEdit>> {
    let (position, _) = position(&snap, &params.text_document_position)?;
    match snap.analysis.rename(position, &params.new_name)? {
        Ok(change) => Ok(Some(proto::workspace_edit(&snap, &change)?)),
        Err(message) => Err(UserError(message).into()),
    }
}

/// 按新签名更新完用法之后由客户端执行：结束这个声明的签名记录。
pub const ACCEPT_SIGNATURE_COMMAND: &str = "java-analyzer.acceptSignature";

/// 光标处的声明签名被改过时，"按新签名更新用法"的操作。
fn change_signature_action(
    snap: &Snapshot,
    uri: &lsp::Url,
    range: ide::FileRange,
) -> anyhow::Result<Option<lsp::CodeAction>> {
    let tracker = {
        let signatures = snap.signatures.read().unwrap_or_else(|e| e.into_inner());
        match signatures.get(&range.file_id) {
            Some(tracker) if !tracker.is_empty() => tracker.clone(),
            _ => return Ok(None),
        }
    };
    let since = ide::SignatureTracker::next_id();
    let Some(update) = snap.analysis.change_signature(range, &tracker)? else {
        return Ok(None);
    };
    let kind = lsp::CodeActionKind::REFACTOR;
    Ok(match update.change {
        Ok(change) => Some(lsp::CodeAction {
            title: update.label.clone(),
            kind: Some(kind),
            edit: Some(proto::workspace_edit(snap, &change)?),
            command: Some(lsp::Command {
                title: update.label,
                command: ACCEPT_SIGNATURE_COMMAND.to_owned(),
                arguments: Some(vec![
                    serde_json::json!(uri),
                    serde_json::json!(update.entry),
                    serde_json::json!(since),
                ]),
            }),
            ..Default::default()
        }),
        Err(reason) if snap.config.code_action_disabled => Some(lsp::CodeAction {
            title: update.label,
            kind: Some(kind),
            disabled: Some(lsp::CodeActionDisabled { reason }),
            ..Default::default()
        }),
        Err(_) => None,
    })
}

pub fn code_action(
    snap: Snapshot,
    params: lsp::CodeActionParams,
) -> anyhow::Result<Option<lsp::CodeActionResponse>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let range = doc.text_range(params.range);
    let mut assists = snap.analysis.assists(ide::FileRange { file_id, range })?;

    // 客户端可以只要某几类操作（例如保存时只要 `source.organizeImports`）
    let wanted = params.context.only.as_ref();
    // 整个文件的修复要把所有检查跑一遍：只在客户端明确要 source 类操作时才算
    // （"Source Action…" 菜单、保存时的 `source.fixAll`），不跟着每次光标移动
    let wants_fix_all = wanted.is_some_and(|only| {
        only.iter()
            .any(|kind| matches!(kind.as_str(), "source" | "source.fixAll"))
    });
    if wants_fix_all {
        assists.extend(snap.analysis.fix_all(file_id)?);
    }
    let mut actions = Vec::with_capacity(assists.len() + 1);
    let wants_refactor =
        wanted.is_none_or(|only| only.iter().any(|kind| kind.as_str() == "refactor"));
    if wants_refactor {
        let file_range = ide::FileRange { file_id, range };
        if let Some(action) = change_signature_action(&snap, &params.text_document.uri, file_range)?
        {
            actions.push(lsp::CodeActionOrCommand::CodeAction(action));
        }
    }
    for assist in assists {
        let kind = proto::code_action_kind(assist.kind);
        let accepted = wanted.is_none_or(|only| {
            only.iter().any(|prefix| {
                let (kind, prefix) = (kind.as_str(), prefix.as_str());
                kind == prefix
                    || kind
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with('.'))
            })
        });
        // 新建文件的操作需要客户端支持
        if !accepted || (!assist.change.file_creates.is_empty() && !snap.config.create_file_ops) {
            continue;
        }
        let is_fix = assist.kind == ide::AssistKind::QuickFix;
        actions.push(lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
            title: assist.label,
            kind: Some(kind),
            // 快速修复关联到光标处的诊断，编辑器在诊断的悬停里直接列出它们
            diagnostics: (is_fix && !params.context.diagnostics.is_empty())
                .then(|| params.context.diagnostics.clone()),
            edit: Some(proto::workspace_edit(&snap, &assist.change)?),
            ..Default::default()
        }));
    }
    Ok(Some(actions))
}

/// 编辑器给的格式化选项；`.editorconfig` 里设置了的项在分析层覆盖它们。
fn format_options(options: &lsp::FormattingOptions) -> ide::FormatOptions {
    let size = options.tab_size.max(1) as usize;
    ide::FormatOptions {
        indent_size: size,
        tab_width: size,
        use_tabs: !options.insert_spaces,
        insert_final_newline: options.insert_final_newline == Some(true),
        ..ide::FormatOptions::default()
    }
}

pub fn formatting(
    snap: Snapshot,
    params: lsp::DocumentFormattingParams,
) -> anyhow::Result<Option<Vec<lsp::TextEdit>>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let edits = snap
        .analysis
        .format(file_id, None, &format_options(&params.options))?;
    Ok(Some(
        edits
            .iter()
            .map(|edit| proto::text_edit(&doc, edit))
            .collect(),
    ))
}

pub fn range_formatting(
    snap: Snapshot,
    params: lsp::DocumentRangeFormattingParams,
) -> anyhow::Result<Option<Vec<lsp::TextEdit>>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let range = doc.text_range(params.range);
    let edits = snap
        .analysis
        .format(file_id, Some(range), &format_options(&params.options))?;
    Ok(Some(
        edits
            .iter()
            .map(|edit| proto::text_edit(&doc, edit))
            .collect(),
    ))
}

pub fn on_type_formatting(
    snap: Snapshot,
    params: lsp::DocumentOnTypeFormattingParams,
) -> anyhow::Result<Option<Vec<lsp::TextEdit>>> {
    let (position, doc) = position(&snap, &params.text_document_position)?;
    let Some(typed) = params.ch.chars().next() else {
        return Ok(None);
    };
    let edits = snap
        .analysis
        .on_type_format(position, typed, &format_options(&params.options))?;
    Ok(Some(
        edits
            .iter()
            .map(|edit| proto::text_edit(&doc, edit))
            .collect(),
    ))
}

pub fn inlay_hint(
    snap: Snapshot,
    params: lsp::InlayHintParams,
) -> anyhow::Result<Option<Vec<lsp::InlayHint>>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let range = doc.text_range(params.range);
    let hints = snap.analysis.inlay_hints(file_id, Some(range))?;
    Ok(Some(
        hints
            .into_iter()
            .map(|hint| {
                let is_param = hint.kind == ide::InlayKind::Parameter;
                let is_chain = hint.kind == ide::InlayKind::Chain;
                lsp::InlayHint {
                    position: doc.position(hint.offset),
                    label: lsp::InlayHintLabel::String(hint.label),
                    kind: Some(if is_param {
                        lsp::InlayHintKind::PARAMETER
                    } else {
                        lsp::InlayHintKind::TYPE
                    }),
                    text_edits: None,
                    tooltip: None,
                    // 类型提示紧贴名字，方法链的类型和代码隔开，形参名提示后面留一个空格
                    padding_left: Some(is_chain),
                    padding_right: Some(is_param),
                    data: None,
                }
            })
            .collect(),
    ))
}

pub fn code_lens(
    snap: Snapshot,
    params: lsp::CodeLensParams,
) -> anyhow::Result<Option<Vec<lsp::CodeLens>>> {
    let file_id = snap.file_id(&params.text_document.uri)?;
    let doc = snap.doc(file_id)?;
    let lenses = snap.analysis.code_lenses(file_id)?;
    let config = &snap.config.user.code_lens;
    Ok(Some(
        lenses
            .into_iter()
            .filter(|lens| match lens.kind {
                ide::CodeLensKind::Implementations => config.implementations,
                ide::CodeLensKind::References => config.references,
            })
            .map(|lens| lsp::CodeLens {
                range: doc.range(lens.range),
                command: None,
                data: Some(serde_json::json!({
                    "uri": params.text_document.uri,
                    "implementations": lens.kind == ide::CodeLensKind::Implementations,
                })),
            })
            .collect(),
    ))
}

/// 算出 lens 的数量。标准 LSP 没有"在客户端展示一组位置"的命令，
/// 所以这里只给标题（命令名留空，客户端按纯文本显示）。
pub fn code_lens_resolve(snap: Snapshot, mut lens: lsp::CodeLens) -> anyhow::Result<lsp::CodeLens> {
    let data = lens.data.take().unwrap_or_default();
    let uri = data
        .get("uri")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<lsp::Url>().ok())
        .ok_or_else(|| {
            anyhow::anyhow!(base_db::trs!(
                "code lens 缺少 uri",
                "The code lens has no uri"
            ))
        })?;
    let implementations = data
        .get("implementations")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let file_id = snap.file_id(&uri)?;
    let doc = snap.doc(file_id)?;
    let position = ide::FilePosition {
        file_id,
        offset: doc.offset(lens.range.start),
    };
    let kind = if implementations {
        ide::CodeLensKind::Implementations
    } else {
        ide::CodeLensKind::References
    };
    let count = snap.analysis.code_lens_count(position, kind)?;
    let title = if implementations {
        base_db::tr!("{count} 个实现", "{count} implementation(s)")
    } else {
        base_db::tr!("{count} 处引用", "{count} reference(s)")
    };
    // 客户端有"显示一组位置"的命令时，点击就列出这些引用 / 实现
    let arguments = match snap.config.code_lens_command() {
        Some(_) if count > 0 => {
            let mut locations: Vec<lsp::Location> = Vec::new();
            if implementations {
                if let Some(found) = snap.analysis.goto_implementation(position)? {
                    for nav in &found.info {
                        if let Some(location) = proto::nav_location(&snap, nav)? {
                            locations.push(lsp::Location {
                                uri: location.url,
                                range: location.focus_range,
                            });
                        }
                    }
                }
            } else {
                for item in snap
                    .analysis
                    .find_all_refs(position, false)?
                    .unwrap_or_default()
                {
                    let Some(url) = snap.url(item.file_id) else {
                        continue;
                    };
                    let item_doc = snap.doc(item.file_id)?;
                    locations.push(lsp::Location {
                        uri: url,
                        range: item_doc.range(item.range),
                    });
                }
            }
            Some(vec![
                serde_json::to_value(&uri)?,
                serde_json::to_value(lens.range.start)?,
                serde_json::to_value(&locations)?,
            ])
        }
        _ => None,
    };
    lens.command = Some(lsp::Command {
        title,
        command: match &arguments {
            Some(_) => snap.config.code_lens_command().unwrap_or_default(),
            None => String::new(),
        },
        arguments,
    });
    Ok(lens)
}

/// 合法的 Java 标识符（新文件名、包名的每一段）。
fn is_java_identifier(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// 新建的空 `.java` 文件该有的包名：在某个源码根下时按目录算；否则照抄同目录下
/// 其它 `.java` 文件的 package 声明；都没有时为空（默认包）。
pub(crate) fn package_for_new_file(
    path: &std::path::Path,
    source_roots: &[std::path::PathBuf],
) -> Option<String> {
    let dir = path.parent()?;
    // 嵌套的源码根取最深的那个
    let root = source_roots
        .iter()
        .filter(|root| dir.starts_with(root))
        .max_by_key(|root| root.components().count());
    if let Some(root) = root {
        let relative = dir.strip_prefix(root).ok()?;
        let segments: Vec<&str> = relative
            .components()
            .map(|c| c.as_os_str().to_str())
            .collect::<Option<_>>()?;
        if !segments.iter().all(|s| is_java_identifier(s)) {
            return None;
        }
        return Some(segments.join("."));
    }
    let siblings = std::fs::read_dir(dir).ok()?;
    for entry in siblings.flatten() {
        let sibling = entry.path();
        if sibling == path || sibling.extension().is_none_or(|e| e != "java") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&sibling) else {
            continue;
        };
        let declared = text.lines().find_map(|line| {
            let rest = line.trim().strip_prefix("package")?;
            let name = rest.trim().strip_suffix(';')?.trim();
            name.split('.')
                .all(is_java_identifier)
                .then(|| name.to_owned())
        });
        if declared.is_some() {
            return declared;
        }
    }
    Some(String::new())
}

/// 新建的空 `.java` 文件的初始内容：package 声明加上和文件同名的类；
/// `package-info.java` 只有 package 声明。文件名不是合法的类名时为 `None`。
pub(crate) fn new_file_skeleton(path: &std::path::Path, package: &str) -> Option<String> {
    let name = path.file_stem()?.to_str()?;
    let package_line = (!package.is_empty()).then(|| format!("package {package};\n"));
    match name {
        "package-info" => package_line,
        "module-info" => None,
        _ if is_java_identifier(name) => {
            let mut text = package_line.map(|p| p + "\n").unwrap_or_default();
            text.push_str(&format!("public class {name} {{\n}}\n"));
            Some(text)
        }
        _ => None,
    }
}

/// 从"文件原来的位置和包名"反推源码根，再算出新位置对应的包名。
/// 原来的目录和包名对不上（文件本来就没放在包对应的目录里）时返回 `None`。
fn package_for_new_path(
    old_path: &std::path::Path,
    old_package: &str,
    new_path: &std::path::Path,
) -> Option<String> {
    let mut source_root = old_path.parent()?;
    for segment in old_package.split('.').filter(|s| !s.is_empty()).rev() {
        if source_root.file_name()?.to_str()? != segment {
            return None;
        }
        source_root = source_root.parent()?;
    }
    let relative = new_path.parent()?.strip_prefix(source_root).ok()?;
    let mut segments = Vec::new();
    for component in relative.components() {
        let segment = component.as_os_str().to_str()?;
        let valid = segment
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
            && segment.chars().all(|c| c.is_alphanumeric() || c == '_');
        if !valid {
            return None;
        }
        segments.push(segment);
    }
    Some(segments.join("."))
}

/// 编辑器里移动 / 重命名 `.java` 文件或目录之前：
/// 换了目录就更新 package 声明和各处的 import（同一批移动的文件一起处理）；
/// 只改了文件名就重命名同名的顶层类。
pub fn will_rename_files(
    snap: Snapshot,
    params: lsp::RenameFilesParams,
) -> anyhow::Result<Option<lsp::WorkspaceEdit>> {
    let mut moves: Vec<(FileId, String)> = Vec::new();
    let mut renames: Vec<(FileId, String, String)> = Vec::new();
    for rename in &params.files {
        let (Ok(old_url), Ok(new_url)) =
            (rename.old_uri.parse::<Url>(), rename.new_uri.parse::<Url>())
        else {
            continue;
        };
        let (Ok(old_path), Ok(new_path)) =
            (proto::url_to_path(&old_url), proto::url_to_path(&new_url))
        else {
            continue;
        };
        // 目录：其下的每个 `.java` 文件按相对路径换到新目录
        let files: Vec<(FileId, std::path::PathBuf, std::path::PathBuf)> =
            if old_path.extension().is_some_and(|e| e == "java") {
                if new_path.extension().is_none_or(|ext| ext != "java") {
                    continue;
                }
                match snap.file_id(&old_url) {
                    Ok(file_id) => vec![(file_id, old_path.clone(), new_path.clone())],
                    Err(_) => continue,
                }
            } else {
                let vfs = snap.vfs.read().unwrap_or_else(|e| e.into_inner());
                vfs.iter()
                    .filter(|(_, path)| {
                        path.extension().is_some_and(|e| e == "java") && path.starts_with(&old_path)
                    })
                    .filter_map(|(file_id, path)| {
                        let relative = path.strip_prefix(&old_path).ok()?;
                        Some((file_id, path.to_path_buf(), new_path.join(relative)))
                    })
                    .collect()
            };
        for (file_id, old_file, new_file) in files {
            if old_file.parent() != new_file.parent() {
                let old_package = snap.analysis.package_name(file_id)?.unwrap_or_default();
                if let Some(new_package) = package_for_new_path(&old_file, &old_package, &new_file)
                {
                    moves.push((file_id, new_package));
                }
            } else if let (Some(old_stem), Some(new_stem)) = (
                old_file.file_stem().and_then(|s| s.to_str()),
                new_file.file_stem().and_then(|s| s.to_str()),
            ) {
                renames.push((file_id, old_stem.to_owned(), new_stem.to_owned()));
            }
        }
    }

    let mut changes_list = Vec::new();
    if !moves.is_empty() {
        changes_list.push(snap.analysis.move_files(&moves)?);
    }
    // 同目录改名：文件名对应的顶层类跟着改
    for (file_id, old_stem, new_stem) in renames {
        let Some(offset) = snap.analysis.top_level_class_offset(file_id, &old_stem)? else {
            continue;
        };
        if let Ok(change) = snap
            .analysis
            .rename(FilePosition { file_id, offset }, &new_stem)?
        {
            changes_list.push(change);
        }
    }

    let mut changes: std::collections::HashMap<Url, Vec<lsp::TextEdit>> = Default::default();
    for change in changes_list {
        // 文件本身由客户端移动，这里只返回文本修改
        for (edited, edits) in &change.edits {
            let Some(url) = snap.url(*edited) else {
                continue;
            };
            let doc = snap.doc(*edited)?;
            changes
                .entry(url)
                .or_default()
                .extend(edits.iter().map(|e| proto::text_edit(&doc, e)));
        }
    }
    if changes.is_empty() {
        return Ok(None);
    }
    Ok(Some(lsp::WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }))
}

#[cfg(test)]
mod path_tests {
    use std::path::Path;

    use super::{new_file_skeleton, package_for_new_file, package_for_new_path};

    #[test]
    fn new_package_follows_directory() {
        let old = Path::new("/w/src/main/java/com/acme/util/Strings.java");
        let moved = Path::new("/w/src/main/java/com/acme/text/Strings.java");
        assert_eq!(
            package_for_new_path(old, "com.acme.util", moved).as_deref(),
            Some("com.acme.text")
        );

        let to_root = Path::new("/w/src/main/java/Strings.java");
        assert_eq!(
            package_for_new_path(old, "com.acme.util", to_root).as_deref(),
            Some("")
        );

        // 原来的目录和包名对不上：不猜
        assert_eq!(package_for_new_path(old, "org.other", moved), None);
        // 移到了源码根之外
        let outside = Path::new("/w/docs/Strings.java");
        assert_eq!(package_for_new_path(old, "com.acme.util", outside), None);
        // 目录名不是合法的包名
        let bad = Path::new("/w/src/main/java/com/acme/my-text/Strings.java");
        assert_eq!(package_for_new_path(old, "com.acme.util", bad), None);
    }

    #[test]
    fn new_file_gets_package_and_class() {
        let roots = vec![
            Path::new("/w/src/main/java").to_path_buf(),
            Path::new("/w/src/test/java").to_path_buf(),
        ];
        let path = Path::new("/w/src/main/java/com/acme/UserService.java");
        let package = package_for_new_file(path, &roots).unwrap();
        assert_eq!(package, "com.acme");
        assert_eq!(
            new_file_skeleton(path, &package).as_deref(),
            Some("package com.acme;\n\npublic class UserService {\n}\n")
        );
        let top = Path::new("/w/src/test/java/MainTest.java");
        let package = package_for_new_file(top, &roots).unwrap();
        assert_eq!(
            new_file_skeleton(top, &package).as_deref(),
            Some("public class MainTest {\n}\n")
        );
        let info = Path::new("/w/src/main/java/com/acme/package-info.java");
        assert_eq!(
            new_file_skeleton(info, "com.acme").as_deref(),
            Some("package com.acme;\n")
        );
        assert_eq!(
            new_file_skeleton(Path::new("/w/src/main/java/my-file.java"), ""),
            None
        );
        assert_eq!(
            new_file_skeleton(Path::new("/w/src/main/java/module-info.java"), ""),
            None
        );
        // 目录名不是合法的包名
        let bad = Path::new("/w/src/main/java/com/my-pkg/Foo.java");
        assert_eq!(package_for_new_file(bad, &roots), None);
    }
}
