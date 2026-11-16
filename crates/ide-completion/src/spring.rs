//! Spring 配置项的补全：`@Value("${|}")`、`environment.getProperty("|")`。

use base_db::{Db, SourceFile};
use ide_db::{spring, spring_metadata, SymbolKind};
use syntax::{TextRange, TextSize};

use crate::{CompletionItem, CompletionItemKind, CompletionList};

pub(crate) fn complete(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<CompletionList> {
    let (placeholder, module) = spring::property_at(db, file, offset)?;
    let mut items: Vec<CompletionItem> = Vec::new();
    for property in spring::config_properties(db, module) {
        if let Some(existing) = items.iter_mut().find(|i| i.label == property.key) {
            // 多个配置文件（profile）里都有：说明里列出文件名
            if let Some(detail) = &mut existing.detail {
                if !detail.contains(property.file_name.as_str()) {
                    detail.push_str(&format!(", {}", property.file_name));
                }
            }
            continue;
        }
        items.push(CompletionItem {
            label: property.key.clone(),
            kind: CompletionItemKind::Symbol(SymbolKind::Field),
            label_detail: (!property.value.is_empty()).then(|| format!(" = {}", property.value)),
            detail: Some(property.file_name.to_string()),
            lookup: property.key.clone(),
            insert_text: property.key.clone(),
            is_snippet: false,
            source_range: TextRange::new(
                placeholder.key_range.start(),
                placeholder.key_range.end(),
            ),
            additional_edits: Vec::new(),
            deprecated: false,
            score: 0,
            resolve: None,
        });
    }
    items.sort_by(|a, b| a.label.cmp(&b.label));
    // 配置元数据里已知、配置文件里还没写的键（有默认值或由框架提供），排在写过的键后面
    let set = spring_metadata::MetadataSet::load(db);
    let written: rustc_hash::FxHashSet<String> =
        items.iter().map(|i| spring::canonical(&i.label)).collect();
    let mut known = Vec::new();
    for property in set.properties() {
        if written.contains(&spring::canonical(&property.name)) {
            continue;
        }
        known.push(CompletionItem {
            label: property.name.clone(),
            kind: CompletionItemKind::Symbol(SymbolKind::Field),
            label_detail: property.default_value.as_ref().map(|d| format!(" = {d}")),
            detail: property.ty.clone(),
            lookup: property.name.clone(),
            insert_text: property.name.clone(),
            is_snippet: false,
            source_range: placeholder.key_range,
            additional_edits: Vec::new(),
            deprecated: property.deprecation.is_some(),
            score: 0,
            resolve: Some(crate::CompletionResolve::ConfigKey {
                key: property.name.as_str().into(),
            }),
        });
    }
    known.sort_by(|a, b| a.label.cmp(&b.label));
    items.extend(known);
    Some(CompletionList {
        items,
        is_incomplete: false,
    })
}
