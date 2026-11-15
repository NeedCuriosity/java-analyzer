//! 内部表示 ↔ LSP 类型的转换。`ide` 层以下不出现任何 LSP 类型，转换都集中在这里。

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context;
use ide::{
    CompletionItemKind, DefKey, Diagnostic, FoldKind, HlMods, HlRange, HlTag, LineCol, LineIndex,
    NavFile, NavigationTarget, Severity, StructureNode, SymbolKind,
};
use lsp_types::{Position, Range, Url};
use syntax::{TextRange, TextSize};

use crate::{config::PositionEncoding, global_state::Snapshot};

pub fn url_to_path(url: &Url) -> anyhow::Result<PathBuf> {
    url.to_file_path()
        .map_err(|()| anyhow::anyhow!(base_db::tr!("不是文件 URI: {url}", "Not a file URI: {url}")))
}

pub fn path_to_url(path: &Path) -> anyhow::Result<Url> {
    Url::from_file_path(path).map_err(|()| {
        anyhow::anyhow!(base_db::tr!(
            "无法转换成 URI: {}",
            "Cannot convert to a URI: {}",
            path.display()
        ))
    })
}

/// 一份文本及其行索引：位置换算的上下文。
#[derive(Clone)]
pub struct Doc {
    pub text: Arc<str>,
    pub index: Arc<LineIndex>,
    pub encoding: PositionEncoding,
}

impl Doc {
    pub fn new(text: Arc<str>, encoding: PositionEncoding) -> Doc {
        let index = Arc::new(LineIndex::new(&text));
        Doc {
            text,
            index,
            encoding,
        }
    }

    /// LSP 位置 → 偏移。超出文件末尾的位置收敛到末尾（客户端的文本可能比我们新）。
    pub fn offset(&self, position: Position) -> TextSize {
        let found = match self.encoding {
            PositionEncoding::Utf8 => {
                self.index
                    .offset_utf8_clamped(&self.text, position.line, position.character)
            }
            PositionEncoding::Utf16 => {
                self.index
                    .offset_utf16(&self.text, position.line, position.character)
            }
        };
        found.unwrap_or_else(|| TextSize::of(&*self.text))
    }

    pub fn text_range(&self, range: Range) -> TextRange {
        let start = self.offset(range.start);
        let end = self.offset(range.end).max(start);
        TextRange::new(start, end)
    }

    pub fn position(&self, offset: TextSize) -> Position {
        let line_col = self.index.line_col(offset);
        let character = match self.encoding {
            PositionEncoding::Utf8 => line_col.col,
            PositionEncoding::Utf16 => self.index.utf16_col(&self.text, line_col),
        };
        Position {
            line: line_col.line,
            character,
        }
    }

    pub fn range(&self, range: TextRange) -> Range {
        Range {
            start: self.position(range.start()),
            end: self.position(range.end()),
        }
    }

    /// 一段文本在当前编码下的长度（语义高亮的 token 长度）。
    pub fn encoded_len(&self, range: TextRange) -> u32 {
        match self.encoding {
            PositionEncoding::Utf8 => u32::from(range.len()),
            PositionEncoding::Utf16 => self
                .text
                .get(usize::from(range.start())..usize::from(range.end()))
                .map_or(u32::from(range.len()), |s| s.encode_utf16().count() as u32),
        }
    }
}

// 保留给将来按 `LineCol` 直接换算的调用方
#[allow(dead_code)]
pub fn line_col(position: Position) -> LineCol {
    LineCol {
        line: position.line,
        col: position.character,
    }
}

pub fn text_edit(doc: &Doc, edit: &ide::TextEdit) -> lsp_types::TextEdit {
    lsp_types::TextEdit {
        range: doc.range(edit.range),
        new_text: edit.new_text.clone(),
    }
}

pub fn symbol_kind(kind: SymbolKind) -> lsp_types::SymbolKind {
    match kind {
        SymbolKind::Class | SymbolKind::Record => lsp_types::SymbolKind::CLASS,
        SymbolKind::Interface | SymbolKind::Annotation => lsp_types::SymbolKind::INTERFACE,
        SymbolKind::Enum => lsp_types::SymbolKind::ENUM,
        SymbolKind::Field => lsp_types::SymbolKind::FIELD,
        SymbolKind::EnumConstant => lsp_types::SymbolKind::ENUM_MEMBER,
        SymbolKind::Method => lsp_types::SymbolKind::METHOD,
        SymbolKind::Constructor => lsp_types::SymbolKind::CONSTRUCTOR,
        SymbolKind::Local | SymbolKind::Param => lsp_types::SymbolKind::VARIABLE,
        SymbolKind::TypeParam => lsp_types::SymbolKind::TYPE_PARAMETER,
        SymbolKind::Package => lsp_types::SymbolKind::PACKAGE,
    }
}

pub fn completion_item_kind(kind: CompletionItemKind) -> lsp_types::CompletionItemKind {
    use lsp_types::CompletionItemKind as K;
    match kind {
        CompletionItemKind::Keyword => K::KEYWORD,
        CompletionItemKind::Snippet => K::SNIPPET,
        CompletionItemKind::Symbol(symbol) => match symbol {
            SymbolKind::Class | SymbolKind::Record => K::CLASS,
            SymbolKind::Interface | SymbolKind::Annotation => K::INTERFACE,
            SymbolKind::Enum => K::ENUM,
            SymbolKind::Field => K::FIELD,
            SymbolKind::EnumConstant => K::ENUM_MEMBER,
            SymbolKind::Method => K::METHOD,
            SymbolKind::Constructor => K::CONSTRUCTOR,
            SymbolKind::Local | SymbolKind::Param => K::VARIABLE,
            SymbolKind::TypeParam => K::TYPE_PARAMETER,
            SymbolKind::Package => K::MODULE,
        },
    }
}

/// `DefKey` 放进 LSP 的 `data` 字段往返。
pub fn def_key_to_json(key: &DefKey) -> serde_json::Value {
    match key {
        DefKey::Class { binary_name } => serde_json::json!({ "k": "c", "c": binary_name.as_str() }),
        DefKey::Method { class, idx } => {
            serde_json::json!({ "k": "m", "c": class.as_str(), "i": idx })
        }
        DefKey::Field { class, idx } => {
            serde_json::json!({ "k": "f", "c": class.as_str(), "i": idx })
        }
    }
}

pub fn def_key_from_json(value: &serde_json::Value) -> Option<DefKey> {
    let class = syntax::SmolStr::new(value.get("c")?.as_str()?);
    let idx = || value.get("i").and_then(|i| i.as_u64()).map(|i| i as u32);
    match value.get("k")?.as_str()? {
        "c" => Some(DefKey::Class { binary_name: class }),
        "m" => Some(DefKey::Method { class, idx: idx()? }),
        "f" => Some(DefKey::Field { class, idx: idx()? }),
        _ => None,
    }
}

pub fn diagnostic(doc: &Doc, diag: &Diagnostic) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range: doc.range(diag.range),
        severity: Some(match diag.severity {
            Severity::Error => lsp_types::DiagnosticSeverity::ERROR,
            Severity::Warning => lsp_types::DiagnosticSeverity::WARNING,
            Severity::Hint => lsp_types::DiagnosticSeverity::HINT,
        }),
        code: Some(lsp_types::NumberOrString::String(
            diag.code.as_str().to_owned(),
        )),
        source: Some("java-analyzer".to_owned()),
        message: diag.message.clone(),
        tags: if diag.unused {
            Some(vec![lsp_types::DiagnosticTag::UNNECESSARY])
        } else if diag.code == ide::DiagnosticCode::DeprecatedUsage {
            Some(vec![lsp_types::DiagnosticTag::DEPRECATED])
        } else {
            None
        },
        ..Default::default()
    }
}

/// 跨文件的修改 → `WorkspaceEdit`。
///
/// 需要重命名文件且客户端支持资源操作时用 `documentChanges`（文本修改在前、改名在后，
/// 这样文本修改里的 URI 仍然指向旧文件名）；否则用最简单的 `changes`。
pub fn workspace_edit(
    snap: &Snapshot,
    change: &ide::SourceChange,
) -> anyhow::Result<lsp_types::WorkspaceEdit> {
    let mut per_file: Vec<(Url, Vec<lsp_types::TextEdit>)> = Vec::new();
    for (file_id, edits) in &change.edits {
        let Some(url) = snap.url(*file_id) else {
            continue;
        };
        let doc = snap.doc(*file_id)?;
        per_file.push((
            url,
            edits.iter().map(|edit| text_edit(&doc, edit)).collect(),
        ));
    }

    let renames: Vec<(Url, Url)> = change
        .file_renames
        .iter()
        .filter_map(|(file_id, new_name)| {
            let old = snap.url(*file_id)?;
            let old_path = url_to_path(&old).ok()?;
            let new = path_to_url(&old_path.with_file_name(new_name)).ok()?;
            Some((old, new))
        })
        .collect();

    let creates: Vec<(Url, String)> = change
        .file_creates
        .iter()
        .filter_map(|(sibling, name, text)| {
            let path = url_to_path(&snap.url(*sibling)?).ok()?;
            Some((path_to_url(&path.with_file_name(name)).ok()?, text.clone()))
        })
        .collect();
    let use_renames = !renames.is_empty() && snap.config.rename_file_ops;
    let use_creates = !creates.is_empty() && snap.config.create_file_ops;
    if !use_renames && !use_creates {
        return Ok(lsp_types::WorkspaceEdit {
            changes: Some(per_file.into_iter().collect()),
            document_changes: None,
            change_annotations: None,
        });
    }
    let mut operations: Vec<lsp_types::DocumentChangeOperation> = per_file
        .into_iter()
        .map(|(uri, edits)| {
            lsp_types::DocumentChangeOperation::Edit(lsp_types::TextDocumentEdit {
                text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                    uri,
                    version: None,
                },
                edits: edits.into_iter().map(lsp_types::OneOf::Left).collect(),
            })
        })
        .collect();
    for (uri, text) in creates.into_iter().filter(|_| use_creates) {
        operations.push(lsp_types::DocumentChangeOperation::Op(
            lsp_types::ResourceOp::Create(lsp_types::CreateFile {
                uri: uri.clone(),
                options: Some(lsp_types::CreateFileOptions {
                    overwrite: Some(false),
                    ignore_if_exists: Some(true),
                }),
                annotation_id: None,
            }),
        ));
        operations.push(lsp_types::DocumentChangeOperation::Edit(
            lsp_types::TextDocumentEdit {
                text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                    uri,
                    version: None,
                },
                edits: vec![lsp_types::OneOf::Left(lsp_types::TextEdit {
                    range: lsp_types::Range::default(),
                    new_text: text,
                })],
            },
        ));
    }
    operations.extend(
        renames
            .into_iter()
            .filter(|_| use_renames)
            .map(|(old_uri, new_uri)| {
                lsp_types::DocumentChangeOperation::Op(lsp_types::ResourceOp::Rename(
                    lsp_types::RenameFile {
                        old_uri,
                        new_uri,
                        options: None,
                        annotation_id: None,
                    },
                ))
            }),
    );
    Ok(lsp_types::WorkspaceEdit {
        changes: None,
        document_changes: Some(lsp_types::DocumentChanges::Operations(operations)),
        change_annotations: None,
    })
}

pub fn code_action_kind(kind: ide::AssistKind) -> lsp_types::CodeActionKind {
    use ide::AssistKind;
    match kind {
        AssistKind::QuickFix => lsp_types::CodeActionKind::QUICKFIX,
        AssistKind::Refactor => lsp_types::CodeActionKind::REFACTOR,
        AssistKind::RefactorExtract => lsp_types::CodeActionKind::REFACTOR_EXTRACT,
        AssistKind::RefactorInline => lsp_types::CodeActionKind::REFACTOR_INLINE,
        AssistKind::RefactorRewrite => lsp_types::CodeActionKind::REFACTOR_REWRITE,
        AssistKind::Generate => lsp_types::CodeActionKind::new("source.generate"),
        AssistKind::OrganizeImports => lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
    }
}

/// 跳转目标在磁盘上的位置。
pub struct NavLocation {
    pub url: Url,
    pub full_range: Range,
    pub focus_range: Range,
}

/// 解析跳转目标的位置。依赖库里的目标会在这里被物化成文件（可能涉及解压）。
pub fn nav_location(
    snap: &Snapshot,
    nav: &NavigationTarget,
) -> anyhow::Result<Option<NavLocation>> {
    match &nav.file {
        NavFile::Source(file_id) => {
            let Some(url) = snap.url(*file_id) else {
                return Ok(None);
            };
            let doc = snap.doc(*file_id)?;
            Ok(Some(NavLocation {
                url,
                full_range: doc.range(nav.full_range),
                focus_range: doc.range(nav.focus_range),
            }))
        }
        NavFile::Library { class, member } => {
            let Some(path) = snap.libraries.materialize(&snap.analysis, class)? else {
                return Ok(None);
            };
            let text = std::fs::read_to_string(&path)
                .with_context(|| base_db::tr!("读取 {}", "Reading {}", path.display()))?;
            let located = ide::locate_in_library_source(&text, class, member.as_ref());
            let doc = Doc::new(Arc::from(text), snap.config.encoding);
            let (full_range, focus_range) = match located {
                Some(source) => (doc.range(source.full_range), doc.range(source.focus_range)),
                None => (Range::default(), Range::default()),
            };
            Ok(Some(NavLocation {
                url: path_to_url(&path)?,
                full_range,
                focus_range,
            }))
        }
    }
}

/// 一批跳转目标 → 位置。依赖库目标最多物化 `library_budget` 个，其余丢弃。
pub fn nav_locations(
    snap: &Snapshot,
    navs: &[NavigationTarget],
    mut library_budget: usize,
) -> anyhow::Result<Vec<(usize, NavLocation)>> {
    let mut res = Vec::with_capacity(navs.len());
    for (idx, nav) in navs.iter().enumerate() {
        if matches!(nav.file, NavFile::Library { .. }) {
            if library_budget == 0 {
                continue;
            }
            library_budget -= 1;
        }
        if let Some(location) = nav_location(snap, nav)? {
            res.push((idx, location));
        }
    }
    Ok(res)
}

fn tags(deprecated: bool) -> Option<Vec<lsp_types::SymbolTag>> {
    deprecated.then(|| vec![lsp_types::SymbolTag::DEPRECATED])
}

pub fn call_hierarchy_item(
    nav: &NavigationTarget,
    location: NavLocation,
) -> lsp_types::CallHierarchyItem {
    lsp_types::CallHierarchyItem {
        name: nav.name.to_string(),
        kind: symbol_kind(nav.kind),
        tags: tags(nav.deprecated),
        detail: nav.container.clone(),
        uri: location.url,
        range: location.full_range,
        selection_range: location.focus_range,
        data: nav.key.as_ref().map(def_key_to_json),
    }
}

pub fn type_hierarchy_item(
    nav: &NavigationTarget,
    location: NavLocation,
) -> lsp_types::TypeHierarchyItem {
    lsp_types::TypeHierarchyItem {
        name: nav.name.to_string(),
        kind: symbol_kind(nav.kind),
        // lsp-types 0.95 里这个字段是单个标签，不是列表
        tags: nav.deprecated.then_some(lsp_types::SymbolTag::DEPRECATED),
        detail: nav.container.clone(),
        uri: location.url,
        range: location.full_range,
        selection_range: location.focus_range,
        data: nav.key.as_ref().map(def_key_to_json),
    }
}

#[allow(deprecated)] // `deprecated` 字段在协议里已被 `tags` 取代，但结构体仍要求填写
pub fn document_symbol(doc: &Doc, node: &StructureNode) -> lsp_types::DocumentSymbol {
    lsp_types::DocumentSymbol {
        name: node.name.clone(),
        detail: node.detail.clone(),
        kind: symbol_kind(node.kind),
        tags: tags(node.deprecated),
        deprecated: None,
        range: doc.range(node.full_range),
        selection_range: doc.range(node.focus_range),
        children: (!node.children.is_empty()).then(|| {
            node.children
                .iter()
                .map(|child| document_symbol(doc, child))
                .collect()
        }),
    }
}

#[allow(deprecated)]
pub fn flat_symbols(
    doc: &Doc,
    url: &Url,
    nodes: &[StructureNode],
    container: Option<&str>,
    out: &mut Vec<lsp_types::SymbolInformation>,
) {
    for node in nodes {
        out.push(lsp_types::SymbolInformation {
            name: node.name.clone(),
            kind: symbol_kind(node.kind),
            tags: tags(node.deprecated),
            deprecated: None,
            location: lsp_types::Location {
                uri: url.clone(),
                range: doc.range(node.focus_range),
            },
            container_name: container.map(str::to_owned),
        });
        flat_symbols(doc, url, &node.children, Some(&node.name), out);
    }
}

#[allow(deprecated)]
pub fn symbol_information(
    nav: &NavigationTarget,
    location: NavLocation,
) -> lsp_types::SymbolInformation {
    lsp_types::SymbolInformation {
        name: nav.name.to_string(),
        kind: symbol_kind(nav.kind),
        tags: tags(nav.deprecated),
        deprecated: None,
        location: lsp_types::Location {
            uri: location.url,
            range: location.focus_range,
        },
        container_name: nav.container.clone(),
    }
}

pub fn folding_range(doc: &Doc, fold: &ide::Fold) -> Option<lsp_types::FoldingRange> {
    let start = doc.position(fold.range.start());
    let end = doc.position(fold.range.end());
    let (kind, end_line) = match fold.kind {
        FoldKind::Comment => (lsp_types::FoldingRangeKind::Comment, end.line),
        FoldKind::Imports => (lsp_types::FoldingRangeKind::Imports, end.line),
        // 折叠后保留右括号所在的那一行
        FoldKind::Region => (
            lsp_types::FoldingRangeKind::Region,
            end.line.saturating_sub(1),
        ),
    };
    if end_line <= start.line {
        return None;
    }
    Some(lsp_types::FoldingRange {
        start_line: start.line,
        start_character: None,
        end_line,
        end_character: None,
        kind: Some(kind),
        collapsed_text: None,
    })
}

// ---------------------------------------------------------------------------
// 语义高亮
// ---------------------------------------------------------------------------

/// 图例里的 token 类型，下标即编码值。
pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "class",
    "interface",
    "enum",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "method",
    "keyword",
    "decorator",
];

/// 图例里的修饰符，下标即位号。顺序与 [`HlMods`] 的位一致。
pub const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "static",
    "readonly",
    "deprecated",
    "abstract",
    "defaultLibrary",
];

fn token_type_index(tag: HlTag) -> u32 {
    let name = match tag {
        HlTag::Keyword => "keyword",
        HlTag::Symbol(kind) => match kind {
            SymbolKind::Package => "namespace",
            SymbolKind::Class | SymbolKind::Record => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Annotation => "decorator",
            SymbolKind::Enum => "enum",
            SymbolKind::TypeParam => "typeParameter",
            SymbolKind::Param => "parameter",
            SymbolKind::Local => "variable",
            SymbolKind::Field => "property",
            SymbolKind::EnumConstant => "enumMember",
            SymbolKind::Method | SymbolKind::Constructor => "method",
        },
    };
    TOKEN_TYPES.iter().position(|t| *t == name).unwrap_or(0) as u32
}

fn token_modifier_bits(mods: HlMods) -> u32 {
    // `HlMods` 的位布局与 TOKEN_MODIFIERS 的顺序相同
    u32::from(mods.0)
}

pub fn semantic_tokens(doc: &Doc, mut ranges: Vec<HlRange>) -> Vec<lsp_types::SemanticToken> {
    ranges.sort_by_key(|hl| hl.range.start());
    let mut res = Vec::with_capacity(ranges.len());
    let mut prev = Position {
        line: 0,
        character: 0,
    };
    for hl in ranges {
        let start = doc.position(hl.range.start());
        let length = doc.encoded_len(hl.range);
        if length == 0 {
            continue;
        }
        let delta_line = start.line - prev.line;
        let delta_start = if delta_line == 0 {
            start.character - prev.character
        } else {
            start.character
        };
        res.push(lsp_types::SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: token_type_index(hl.tag),
            token_modifiers_bitset: token_modifier_bits(hl.mods),
        });
        prev = start;
    }
    res
}

/// 两次语义高亮结果之间的差异：去掉公共前后缀，中间一段整体替换。
/// 协议里的下标以整数为单位，每个 token 占 5 个整数。
pub fn semantic_tokens_edits(
    old: &[lsp_types::SemanticToken],
    new: &[lsp_types::SemanticToken],
) -> Vec<lsp_types::SemanticTokensEdit> {
    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let deleted = old.len() - prefix - suffix;
    let inserted = &new[prefix..new.len() - suffix];
    if deleted == 0 && inserted.is_empty() {
        return Vec::new();
    }
    vec![lsp_types::SemanticTokensEdit {
        start: (prefix * 5) as u32,
        delete_count: (deleted * 5) as u32,
        data: (!inserted.is_empty()).then(|| inserted.to_vec()),
    }]
}

/// 每个文件最近一次发给客户端的语义高亮结果，用于计算增量。
#[derive(Default)]
pub struct SemanticTokensCache {
    entries: std::sync::Mutex<rustc_hash::FxHashMap<Url, (String, Vec<lsp_types::SemanticToken>)>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl SemanticTokensCache {
    /// 记下新结果，返回它的 result id。
    pub fn store(&self, url: Url, data: Vec<lsp_types::SemanticToken>) -> String {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string();
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(url, (id.clone(), data));
        id
    }

    /// 取出 `result_id` 对应的结果；id 对不上（客户端基于更旧的版本）时返回 `None`。
    pub fn get(&self, url: &Url, result_id: &str) -> Option<Vec<lsp_types::SemanticToken>> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let (id, data) = entries.get(url)?;
        (id == result_id).then(|| data.clone())
    }

    pub fn remove(&self, url: &Url) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(url);
    }
}

/// 字节偏移 → UTF-16 偏移（签名帮助里形参在标签中的位置按 UTF-16 计）。
pub fn utf16_offset(text: &str, byte_offset: u32) -> u32 {
    text.get(..byte_offset as usize)
        .map_or(byte_offset, |s| s.encode_utf16().count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(delta_line: u32, delta_start: u32) -> lsp_types::SemanticToken {
        lsp_types::SemanticToken {
            delta_line,
            delta_start,
            length: 1,
            token_type: 0,
            token_modifiers_bitset: 0,
        }
    }

    #[test]
    fn semantic_token_edits_cover_only_the_changed_middle() {
        let old = vec![token(0, 0), token(1, 0), token(1, 4), token(2, 0)];
        // 中间一个 token 被换成两个
        let new = vec![
            token(0, 0),
            token(1, 0),
            token(0, 2),
            token(1, 2),
            token(2, 0),
        ];
        let edits = semantic_tokens_edits(&old, &new);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].start, 10);
        assert_eq!(edits[0].delete_count, 5);
        assert_eq!(edits[0].data.as_ref().map(Vec::len), Some(2));

        assert!(semantic_tokens_edits(&old, &old).is_empty());

        // 纯删除
        let edits = semantic_tokens_edits(&old, &old[..3]);
        assert_eq!((edits[0].start, edits[0].delete_count), (15, 5));
        assert!(edits[0].data.is_none());
    }

    #[test]
    fn positions_convert_in_both_encodings() {
        let text: Arc<str> = Arc::from("int a;\nString s = \"中文\"; x\n");
        let utf8 = Doc::new(text.clone(), PositionEncoding::Utf8);
        let utf16 = Doc::new(text.clone(), PositionEncoding::Utf16);
        let offset = TextSize::from(text.find(" x").unwrap() as u32 + 1);

        let p8 = utf8.position(offset);
        let p16 = utf16.position(offset);
        assert_eq!((p8.line, p16.line), (1, 1));
        // 两个汉字：UTF-8 各 3 字节，UTF-16 各 1 个单元
        assert_eq!(p8.character - p16.character, 4);
        assert_eq!(utf8.offset(p8), offset);
        assert_eq!(utf16.offset(p16), offset);

        // 超出文件末尾的位置收敛到末尾
        let far = Position {
            line: 99,
            character: 0,
        };
        assert_eq!(utf16.offset(far), TextSize::of(&*text));
    }

    #[test]
    fn def_key_round_trips_through_json() {
        let key = DefKey::Method {
            class: "a/B$C".into(),
            idx: 3,
        };
        assert_eq!(def_key_from_json(&def_key_to_json(&key)), Some(key));
        assert_eq!(
            def_key_from_json(&serde_json::json!({ "k": "x", "c": "a" })),
            None
        );
    }
}
