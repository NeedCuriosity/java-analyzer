//! 代码补全。
//!
//! 入口是 [`completions`]：先由 [`context::CompletionContext`] 判断光标所处的语法位置，
//! 再按位置调用对应的候选来源，最后按相关度排序并截断。

mod completions;
mod config_file;
mod context;
mod extras;
mod item;
mod smart;
mod spring;
mod templates;
#[cfg(test)]
mod tests;

use base_db::{Db, SourceFile};
use hir::{lookup_class, Definition, FieldId, MethodId};
use syntax::TextSize;

use crate::{
    context::{CompletionContext, ContextKind, TypeLocation},
    item::Completions,
};

pub use crate::item::{CompletionItem, CompletionItemKind, CompletionResolve};

#[derive(Debug, Clone)]
pub struct CompletionConfig {
    /// 客户端是否支持片段语法（`$0`、`${1:name}`）
    pub snippets: bool,
    /// 未导入的类型是否自动加 import（否则插入全限定名）
    pub auto_import: bool,
    /// 是否搜索未导入的类型
    pub unimported_types: bool,
    pub unimported_limit: usize,
    pub postfix: bool,
    /// 一次最多返回多少条
    pub limit: usize,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        CompletionConfig {
            snippets: true,
            auto_import: true,
            unimported_types: true,
            unimported_limit: 60,
            postfix: true,
            limit: 400,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionList {
    pub items: Vec<CompletionItem>,
    /// 结果被截断，或依赖于已输入的前缀：继续输入时客户端应当重新请求
    pub is_incomplete: bool,
}

/// Spring 配置文件（资源文件）里的补全。
pub fn config_file_completions(
    db: &dyn Db,
    resource: &base_db::ResourceFile,
    offset: TextSize,
    config: &CompletionConfig,
) -> CompletionList {
    config_file::complete(db, resource, offset, config)
}

pub fn completions(
    db: &dyn Db,
    file: SourceFile,
    offset: TextSize,
    config: &CompletionConfig,
) -> CompletionList {
    if let Some(list) = spring::complete(db, file, offset) {
        return list;
    }
    let Some(ctx) = CompletionContext::new(db, file, offset, config) else {
        return CompletionList::default();
    };
    let mut acc = Completions::default();
    // 这些位置的候选集合取决于前缀（包展开、未导入类型搜索）
    let mut prefix_dependent = false;

    match &ctx.kind {
        ContextKind::Dot {
            receiver,
            receiver_path,
            receiver_range,
            statement_level,
            method_ref,
        } => {
            let receiver_ty = completions::complete_dot(
                &mut acc,
                &ctx,
                receiver.as_ref(),
                receiver_path.as_deref(),
                *method_ref,
            );
            if !method_ref {
                extras::postfix_templates(
                    &mut acc,
                    &ctx,
                    *receiver_range,
                    receiver_ty.as_ref(),
                    *statement_level,
                );
            }
        }
        ContextKind::TypeMethodRef { ty } => {
            completions::complete_type_method_ref(&mut acc, &ctx, ty);
        }
        ContextKind::Type {
            qualifier,
            location,
        } => {
            if qualifier.is_empty() {
                prefix_dependent = true;
                if *location == TypeLocation::New {
                    smart::new_of_expected_type(&mut acc, &ctx);
                }
                completions::types_in_scope(&mut acc, &ctx, *location);
                extras::type_keywords(&mut acc, &ctx, *location);
                if *location == TypeLocation::Member {
                    extras::override_methods(&mut acc, &ctx);
                    extras::member_snippets(&mut acc, &ctx);
                    templates::member_templates(&mut acc, &ctx);
                }
            } else {
                completions::complete_qualified_type(&mut acc, &ctx, qualifier, *location);
            }
        }
        ContextKind::Import {
            qualifier,
            is_static,
        } => {
            completions::complete_import(&mut acc, &ctx, qualifier, *is_static);
            if qualifier.is_empty() && !is_static {
                acc.add_keyword(&ctx, "static");
            }
        }
        ContextKind::Package { qualifier } => {
            completions::complete_package(&mut acc, &ctx, qualifier);
        }
        ContextKind::Module { qualifier } => {
            completions::complete_module_name(&mut acc, &ctx, qualifier);
        }
        ContextKind::ModulePackage { qualifier } => {
            completions::complete_module_package(&mut acc, &ctx, qualifier);
        }
        ContextKind::Expr {
            statement_start,
            switch_scrutinee,
        } => {
            prefix_dependent = true;
            if switch_scrutinee.is_none() {
                smart::lambda_for_expected(&mut acc, &ctx);
                smart::constants_of_expected(&mut acc, &ctx);
            }
            completions::complete_expr(&mut acc, &ctx, switch_scrutinee.as_ref());
            extras::expr_keywords(&mut acc, &ctx, *statement_start);
            if *statement_start {
                extras::statement_snippets(&mut acc, &ctx);
                templates::statement_templates(&mut acc, &ctx);
            }
        }
        ContextKind::AnnotationArg { annotation } => {
            completions::complete_annotation_arg(&mut acc, &ctx, *annotation);
        }
        ContextKind::JavadocTag { inline } => extras::javadoc_tags(&mut acc, &ctx, *inline),
        ContextKind::JavadocParam { names } => extras::javadoc_params(&mut acc, &ctx, names),
    }

    let mut items = acc.items;
    apply_usage_frequency(&mut items, file.text(db), &ctx.prefix);
    items.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.label.len().cmp(&b.label.len()))
            .then_with(|| a.label.cmp(&b.label))
    });
    let truncated = items.len() > config.limit;
    items.truncate(config.limit);
    CompletionList {
        items,
        is_incomplete: truncated || prefix_dependent,
    }
}

/// 同一相关度档位内，当前文件里用得越多的名字越靠前。
/// 加成有上限且小于档位之间的差距，不会让它越过期望类型、作用域远近这些更强的信号。
fn apply_usage_frequency(items: &mut [CompletionItem], text: &str, prefix: &str) {
    const MAX_BONUS: i32 = 6;
    let mut counts: rustc_hash::FxHashMap<&str, i32> = rustc_hash::FxHashMap::default();
    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')) {
        if !word.is_empty() {
            *counts.entry(word).or_insert(0) += 1;
        }
    }
    // 正在输入的这个词本身不算一次使用
    if let Some(count) = counts.get_mut(prefix) {
        *count -= 1;
    }
    for item in items {
        if !matches!(item.kind, CompletionItemKind::Symbol(_)) {
            continue;
        }
        if let Some(count) = counts.get(item.lookup.as_str()) {
            item.score += (*count).clamp(0, MAX_BONUS);
        }
    }
}

/// 把延迟解析数据还原成定义（用于取文档和完整签名）。
pub fn resolve_definition(db: &dyn Db, resolve: &CompletionResolve) -> Option<Definition> {
    match resolve {
        CompletionResolve::Class { binary_name } => {
            lookup_class(db, binary_name).map(Definition::Class)
        }
        CompletionResolve::Method { class, idx } => {
            let class = lookup_class(db, class)?;
            Some(Definition::Method(MethodId { class, idx: *idx }))
        }
        CompletionResolve::Field { class, idx } => {
            let class = lookup_class(db, class)?;
            Some(Definition::Field(FieldId { class, idx: *idx }))
        }
        CompletionResolve::ConfigKey { .. } => None,
    }
}
