//! IDE 功能的统一入口。
//!
//! - [`AnalysisHost`] 持有可变的数据库，只在主线程上使用：应用变更。
//! - [`Analysis`] 是不可变快照，可以发到任意线程上做查询。宿主一旦写入，
//!   所有快照上正在进行的查询都会被取消，对应的方法返回 `Err(Cancelled)`。
//!
//! 这一层不出现任何 LSP 类型：位置用 `TextSize` / `TextRange`，文件用 `FileId`。

mod assists;
mod code_lens;
mod config_file;
mod doc_comment;
mod endpoints;
mod formatting;
mod hierarchy;
mod highlight;
mod hover;
#[cfg(test)]
mod incremental_tests;
mod inlay_hints;
mod library;
mod move_class;
mod navigation;
mod references;
mod rename;
mod signature_help;
mod structure;
#[cfg(test)]
mod tests;

use std::{panic::AssertUnwindSafe, path::PathBuf, sync::Arc};

use base_db::{
    salsa::{self, Setter},
    workspace, FileEntry, FileSet, ProjectGraph, SourceFile,
};
use classfile::ClassPath;
use ide_db::RootDatabase;
use syntax::{TextRange, TextSize};

pub use base_db::{FileId, FileKind, LineCol, LineIndex};
pub use ide_completion::{
    CompletionConfig, CompletionItem, CompletionItemKind, CompletionList,
    CompletionResolve as DefKey,
};
pub use ide_db::{QueryExecutions, SymbolKind, TextEdit};

/// 不依赖工程的纯文本格式化：解析 `text` 并返回格式化的修改。
pub fn format_text(text: &str, options: &FormatOptions) -> Vec<TextEdit> {
    let root = syntax::ast::CompilationUnit::parse(text).syntax_node();
    formatting::format(&root, text, None, options)
}
pub use ide_diagnostics::{
    timing as diagnostics_timing, Diagnostic, DiagnosticCode, DiagnosticsConfig, Severity,
};
pub use salsa::Cancelled;

pub use crate::{
    assists::{Assist, AssistKind},
    code_lens::{CodeLens, CodeLensKind},
    formatting::{BlankLineOptions, BraceStyle, FormatOptions, SpacingOptions},
    hierarchy::CallItem,
    highlight::{HlMods, HlRange, HlTag},
    hover::HoverResult,
    inlay_hints::{InlayHint, InlayKind},
    library::{locate_in_library_source, LibraryMember, LibraryMemberKind, LibrarySource},
    navigation::{NavFile, NavigationTarget},
    references::{HighlightedRange, ReferenceItem},
    rename::SourceChange,
    signature_help::{SignatureHelp, SignatureInfo},
    structure::{Fold, FoldKind, StructureNode},
};

pub type Cancellable<T> = Result<T, Cancelled>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilePosition {
    pub file_id: FileId,
    pub offset: TextSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileRange {
    pub file_id: FileId,
    pub range: TextRange,
}

/// 附带"触发它的源范围"的结果（悬停时高亮的词、跳转时的来源范围）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeInfo<T> {
    pub range: TextRange,
    pub info: T,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub text: Arc<str>,
    pub kind: FileKind,
    /// 去掉 `.java` 的文件名（见 [`base_db::file_stem`]）
    pub name: smol_str::SmolStr,
    /// 所在的目录（见 [`base_db::file_dir`]），决定哪些 `lombok.config` 对它生效
    pub dir: smol_str::SmolStr,
}

impl FileChange {
    /// 路径为 `path` 的文件：文件名和目录都从路径里取。
    pub fn at_path(path: &str, text: Arc<str>, kind: FileKind) -> FileChange {
        FileChange {
            text,
            kind,
            name: base_db::file_stem(path),
            dir: base_db::file_dir(path),
        }
    }
}

/// 一批要原子地应用的变更。
#[derive(Debug, Default)]
pub struct Change {
    /// 新增 / 修改（`Some`）或删除（`None`）的文件
    pub files: Vec<(FileId, Option<FileChange>)>,
    /// 只改变文件的角色（工程模型重新导入之后）
    pub kinds: Vec<(FileId, FileKind)>,
    pub project: Option<Arc<ProjectGraph>>,
    pub class_path: Option<Arc<ClassPath>>,
    /// 资源文件的全集（`None` 表示不变）
    pub resources: Option<Arc<Vec<base_db::ResourceFile>>>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
            && self.kinds.is_empty()
            && self.project.is_none()
            && self.class_path.is_none()
            && self.resources.is_none()
    }
}

#[derive(Debug)]
pub struct AnalysisHost {
    db: RootDatabase,
}

impl Default for AnalysisHost {
    fn default() -> Self {
        AnalysisHost::new()
    }
}

impl AnalysisHost {
    pub fn new() -> AnalysisHost {
        AnalysisHost {
            db: RootDatabase::new(),
        }
    }

    /// 取一个快照。代价很小（共享存储）。
    pub fn analysis(&self) -> Analysis {
        Analysis {
            db: self.db.clone(),
        }
    }

    pub fn raw_database(&self) -> &RootDatabase {
        &self.db
    }

    /// 应用变更。会先取消所有快照上正在进行的查询，并等它们退出。
    ///
    /// 输入的耐久度（durability）：依赖库和工程结构几乎不变（高）；导入时读进来的源码是中等，
    /// 之后被编辑过的文件降为低。编辑一个文件只改动低耐久度的输入，只依赖中高耐久度输入的
    /// 缓存（其它文件的签名、类路径上的类）不必逐个往下核对就能复用：大工程里每次按键后
    /// 第一次查询不再要遍历全部文件的依赖。
    pub fn apply_change(&mut self, change: Change) {
        let ws = workspace(&self.db);
        let mut files: FileSet = (**ws.files(&self.db)).clone();
        let mut files_changed = false;

        for (file_id, file_change) in change.files {
            match file_change {
                None => files_changed |= files.remove(file_id).is_some(),
                Some(FileChange {
                    text,
                    kind,
                    name,
                    dir,
                }) => match files.get(file_id) {
                    Some(entry) => {
                        let same_text = **entry.source.text(&self.db) == *text;
                        if !same_text {
                            entry
                                .source
                                .set_text(&mut self.db)
                                .with_durability(salsa::Durability::LOW)
                                .to(text);
                        }
                        if *entry.source.name(&self.db) != name {
                            entry
                                .source
                                .set_name(&mut self.db)
                                .with_durability(salsa::Durability::LOW)
                                .to(name);
                        }
                        if *entry.source.dir(&self.db) != dir {
                            entry
                                .source
                                .set_dir(&mut self.db)
                                .with_durability(salsa::Durability::LOW)
                                .to(dir);
                        }
                        if entry.kind != kind {
                            files.insert(
                                file_id,
                                FileEntry {
                                    source: entry.source,
                                    kind,
                                },
                            );
                            files_changed = true;
                        }
                    }
                    None => {
                        let durability = match kind {
                            FileKind::Library => salsa::Durability::HIGH,
                            FileKind::Workspace { .. } => salsa::Durability::MEDIUM,
                        };
                        let source = SourceFile::builder(file_id, text, name, dir)
                            .durability(durability)
                            .new(&self.db);
                        files.insert(file_id, FileEntry { source, kind });
                        files_changed = true;
                    }
                },
            }
        }
        for (file_id, kind) in change.kinds {
            if let Some(entry) = files.get(file_id) {
                if entry.kind != kind {
                    files.insert(
                        file_id,
                        FileEntry {
                            source: entry.source,
                            kind,
                        },
                    );
                    files_changed = true;
                }
            }
        }

        if files_changed {
            ws.set_files(&mut self.db)
                .with_durability(salsa::Durability::MEDIUM)
                .to(Arc::new(files));
        }
        if let Some(project) = change.project {
            ws.set_project(&mut self.db)
                .with_durability(salsa::Durability::HIGH)
                .to(project);
        }
        if let Some(class_path) = change.class_path {
            ws.set_class_path(&mut self.db)
                .with_durability(salsa::Durability::HIGH)
                .to(class_path);
        }
        if let Some(resources) = change.resources {
            if **ws.resources(&self.db) != *resources {
                ws.set_resources(&mut self.db)
                    .with_durability(salsa::Durability::MEDIUM)
                    .to(resources);
            }
        }
    }
}

/// 数据库的只读快照。所有方法都可能因为宿主写入而被取消。
#[derive(Debug, Clone)]
pub struct Analysis {
    db: RootDatabase,
}

/// 依赖库里的一个类的来源信息，供宿主把它"物化"成可以打开的文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryClassInfo {
    /// 最外层类的二进制名（一个源文件对应一个最外层类）
    pub outermost: String,
    /// 包含它的 jar / JDK 镜像
    pub container: Option<PathBuf>,
    /// 附带的源码包（sources jar 或 JDK 的 `src.zip`）
    pub sources: Option<PathBuf>,
}

impl Analysis {
    fn with_db<T>(&self, f: impl FnOnce(&RootDatabase) -> T) -> Cancellable<T> {
        // salsa 的取消以 panic 展开的方式传播，这里把它接住
        Cancelled::catch(AssertUnwindSafe(|| f(&self.db)))
    }

    /// 统计 `f` 里（当前线程上）真正执行了的查询，用来检查编辑之后的失效范围。
    pub fn record_executions<T>(&self, f: impl FnOnce(&Analysis) -> T) -> (T, QueryExecutions) {
        ide_db::record_executions(&self.db, || f(self))
    }

    /// 和 [`Analysis::record_executions`] 一样，另外返回核对依赖后复用的缓存个数。
    pub fn record_query_events<T>(
        &self,
        f: impl FnOnce(&Analysis) -> T,
    ) -> (T, QueryExecutions, QueryExecutions) {
        ide_db::record_query_events(&self.db, || f(self))
    }

    /// Spring 的配置文件（`application.yml` 等）；`lombok.config`、元数据 JSON 等别的资源文件不算。
    fn config_file(db: &RootDatabase, file_id: FileId) -> Option<base_db::ResourceFile> {
        base_db::resource_file(db, file_id).filter(|r| ide_db::spring::is_config_name(&r.name))
    }

    fn file(db: &RootDatabase, file_id: FileId) -> Option<SourceFile> {
        base_db::source_file(db, file_id)
    }

    /// 文件的文本。资源文件（Spring 配置）也能取到。
    pub fn file_text(&self, file_id: FileId) -> Cancellable<Option<Arc<str>>> {
        self.with_db(|db| match Self::file(db, file_id) {
            Some(file) => Some(file.text(db).clone()),
            None => base_db::resource_file(db, file_id).map(|r| r.text),
        })
    }

    pub fn line_index(&self, file_id: FileId) -> Cancellable<Option<Arc<LineIndex>>> {
        self.with_db(|db| match Self::file(db, file_id) {
            Some(file) => Some(base_db::line_index(db, file)),
            None => base_db::resource_file(db, file_id).map(|r| Arc::new(LineIndex::new(&r.text))),
        })
    }

    /// 调试用：语法树的文本形式。
    pub fn syntax_tree(&self, file_id: FileId) -> Cancellable<Option<String>> {
        self.with_db(|db| {
            Self::file(db, file_id)
                .map(|file| format!("{:#?}", base_db::parse(db, file).syntax_node()))
        })
    }

    /// 全部工作区源文件（不含依赖库的源码）。
    pub fn workspace_files(&self) -> Cancellable<Vec<FileId>> {
        self.with_db(|db| {
            workspace(db)
                .files(db)
                .workspace_files()
                .into_iter()
                .map(|(file_id, _)| file_id)
                .collect()
        })
    }

    /// 预热：解析所有工作区文件并建立符号索引。返回（文件数，符号数）。
    pub fn prime_caches(&self) -> Cancellable<(usize, usize)> {
        self.with_db(|db| {
            let files = workspace(db).files(db).workspace_files();
            for (_, file) in &files {
                salsa::Database::unwind_if_revision_cancelled(db);
                hir::item_tree(db, *file);
            }
            let symbols = ide_db::symbol_index::source_symbol_index(db).len();
            ide_db::symbol_index::source_type_index(db);
            ide_db::symbol_index::binary_class_index(db);
            (files.len(), symbols)
        })
    }

    /// 统计用：文件里（已知类型的表达式数，表达式总数）。
    pub fn expr_type_stats(&self, file_id: FileId) -> Cancellable<(usize, usize)> {
        self.with_db(|db| {
            let Some(file) = Self::file(db, file_id) else {
                return (0, 0);
            };
            let mut known = 0;
            let mut total = 0;
            for body in ide_diagnostics::bodies_in_file(db, file) {
                let result = hir::infer(db, body);
                for (_, ty) in result.expr_types() {
                    total += 1;
                    if !ty.contains_unknown() {
                        known += 1;
                    }
                }
            }
            (known, total)
        })
    }

    /// 统计用：文件里可推断的体（方法、构造器、字段初始化式、初始化块）的个数。
    pub fn body_count(&self, file_id: FileId) -> Cancellable<usize> {
        self.with_db(|db| {
            Self::file(db, file_id)
                .map(|file| ide_diagnostics::bodies_in_file(db, file).len())
                .unwrap_or(0)
        })
    }

    /// 排查用：文件里类型（或类型的一部分）未知的表达式的范围。
    pub fn unknown_expr_ranges(&self, file_id: FileId) -> Cancellable<Vec<TextRange>> {
        self.with_db(|db| {
            let Some(file) = Self::file(db, file_id) else {
                return Vec::new();
            };
            let mut res = Vec::new();
            for body in ide_diagnostics::bodies_in_file(db, file) {
                let result = hir::infer(db, body);
                res.extend(
                    result
                        .expr_types()
                        .filter(|(_, ty)| ty.contains_unknown())
                        .map(|(range, _)| range),
                );
            }
            res.sort_by_key(|r| (r.start(), r.end()));
            res
        })
    }

    /// 核对用：整个文件一起检查得到的诊断（不按体缓存）。应当和 [`Self::diagnostics`] 完全相同。
    pub fn whole_file_diagnostics(
        &self,
        file_id: FileId,
        config: &DiagnosticsConfig,
    ) -> Cancellable<Vec<Diagnostic>> {
        self.with_db(|db| match Self::file(db, file_id) {
            Some(file) => ide_diagnostics::whole_file_diagnostics(db, file, config),
            None => Vec::new(),
        })
    }

    pub fn diagnostics(
        &self,
        file_id: FileId,
        config: &DiagnosticsConfig,
    ) -> Cancellable<Vec<Diagnostic>> {
        self.with_db(|db| match Self::file(db, file_id) {
            Some(file) => ide_diagnostics::diagnostics(db, file, config),
            None => Self::config_file(db, file_id)
                .filter(|_| config.semantic)
                .map(|r| ide_diagnostics::config_file_diagnostics(db, &r))
                .unwrap_or_default(),
        })
    }

    pub fn completions(
        &self,
        position: FilePosition,
        config: &CompletionConfig,
    ) -> Cancellable<CompletionList> {
        self.with_db(|db| match Self::file(db, position.file_id) {
            Some(file) => ide_completion::completions(db, file, position.offset, config),
            None => Self::config_file(db, position.file_id)
                .map(|r| ide_completion::config_file_completions(db, &r, position.offset, config))
                .unwrap_or_default(),
        })
    }

    /// 补全条目的延迟信息：完整签名和文档（Markdown）。
    pub fn resolve_completion(&self, key: &DefKey) -> Cancellable<Option<HoverResult>> {
        self.with_db(|db| {
            if let DefKey::ConfigKey { key } = key {
                return config_file::resolve_key(db, key);
            }
            let def = ide_completion::resolve_definition(db, key)?;
            Some(hover::hover_for_definition(db, &def))
        })
    }

    pub fn hover(&self, position: FilePosition) -> Cancellable<Option<RangeInfo<HoverResult>>> {
        self.with_db(|db| match Self::config_file(db, position.file_id) {
            Some(r) => config_file::hover(db, &r, position.offset),
            None => hover::hover(db, position),
        })
    }

    pub fn goto_definition(
        &self,
        position: FilePosition,
    ) -> Cancellable<Option<RangeInfo<Vec<NavigationTarget>>>> {
        self.with_db(|db| match Self::config_file(db, position.file_id) {
            Some(r) => config_file::goto_definition(db, &r, position.offset),
            None => navigation::goto_definition(db, position),
        })
    }

    pub fn goto_declaration(
        &self,
        position: FilePosition,
    ) -> Cancellable<Option<RangeInfo<Vec<NavigationTarget>>>> {
        self.with_db(|db| navigation::goto_declaration(db, position))
    }

    pub fn goto_type_definition(
        &self,
        position: FilePosition,
    ) -> Cancellable<Option<RangeInfo<Vec<NavigationTarget>>>> {
        self.with_db(|db| navigation::goto_type_definition(db, position))
    }

    pub fn goto_implementation(
        &self,
        position: FilePosition,
    ) -> Cancellable<Option<RangeInfo<Vec<NavigationTarget>>>> {
        self.with_db(|db| navigation::goto_implementation(db, position))
    }

    pub fn find_all_refs(
        &self,
        position: FilePosition,
        include_declaration: bool,
    ) -> Cancellable<Option<Vec<ReferenceItem>>> {
        self.with_db(|db| references::find_all_refs(db, position, include_declaration))
    }

    pub fn highlight_related(
        &self,
        position: FilePosition,
    ) -> Cancellable<Option<Vec<HighlightedRange>>> {
        self.with_db(|db| references::highlight_related(db, position))
    }

    pub fn workspace_symbols(
        &self,
        query: &str,
        limit: usize,
    ) -> Cancellable<Vec<NavigationTarget>> {
        self.with_db(|db| navigation::workspace_symbols(db, query, limit))
    }

    pub fn file_structure(&self, file_id: FileId) -> Cancellable<Vec<StructureNode>> {
        self.with_db(|db| {
            Self::file(db, file_id)
                .map(|file| structure::file_structure(&base_db::parse(db, file).tree()))
                .unwrap_or_default()
        })
    }

    pub fn folding_ranges(&self, file_id: FileId) -> Cancellable<Vec<Fold>> {
        self.with_db(|db| {
            Self::file(db, file_id)
                .map(|file| structure::folding_ranges(&base_db::parse(db, file).syntax_node()))
                .unwrap_or_default()
        })
    }

    /// 由内到外逐级扩大的选区。
    pub fn selection_ranges(&self, position: FilePosition) -> Cancellable<Vec<TextRange>> {
        self.with_db(|db| {
            Self::file(db, position.file_id)
                .map(|file| {
                    structure::selection_ranges(
                        &base_db::parse(db, file).syntax_node(),
                        position.offset,
                    )
                })
                .unwrap_or_default()
        })
    }

    pub fn matching_brace(&self, position: FilePosition) -> Cancellable<Option<TextSize>> {
        self.with_db(|db| {
            let file = Self::file(db, position.file_id)?;
            structure::matching_brace(&base_db::parse(db, file).syntax_node(), position.offset)
        })
    }

    /// 语义高亮。`range` 为 `None` 时处理整个文件。
    pub fn highlight(
        &self,
        file_id: FileId,
        range: Option<TextRange>,
    ) -> Cancellable<Vec<HlRange>> {
        self.with_db(|db| {
            Self::file(db, file_id)
                .map(|file| highlight::highlight(db, file, range))
                .unwrap_or_default()
        })
    }

    pub fn signature_help(&self, position: FilePosition) -> Cancellable<Option<SignatureHelp>> {
        self.with_db(|db| signature_help::signature_help(db, position))
    }

    pub fn prepare_call_hierarchy(
        &self,
        position: FilePosition,
    ) -> Cancellable<Option<Vec<NavigationTarget>>> {
        self.with_db(|db| hierarchy::prepare_call_hierarchy(db, position))
    }

    pub fn incoming_calls(&self, key: &DefKey) -> Cancellable<Vec<CallItem>> {
        self.with_db(|db| hierarchy::incoming_calls(db, key))
    }

    pub fn outgoing_calls(&self, key: &DefKey) -> Cancellable<Vec<CallItem>> {
        self.with_db(|db| hierarchy::outgoing_calls(db, key))
    }

    pub fn prepare_type_hierarchy(
        &self,
        position: FilePosition,
    ) -> Cancellable<Option<Vec<NavigationTarget>>> {
        self.with_db(|db| hierarchy::prepare_type_hierarchy(db, position))
    }

    pub fn supertypes(&self, key: &DefKey) -> Cancellable<Vec<NavigationTarget>> {
        self.with_db(|db| hierarchy::supertypes(db, key))
    }

    pub fn subtypes(&self, key: &DefKey) -> Cancellable<Vec<NavigationTarget>> {
        self.with_db(|db| hierarchy::subtypes(db, key))
    }

    /// 重命名前的检查：返回可以重命名的范围和当前名字，或者不能重命名的原因。
    pub fn prepare_rename(
        &self,
        position: FilePosition,
    ) -> Cancellable<Result<RangeInfo<String>, String>> {
        self.with_db(|db| rename::prepare_rename(db, position))
    }

    pub fn rename(
        &self,
        position: FilePosition,
        new_name: &str,
    ) -> Cancellable<Result<SourceChange, String>> {
        self.with_db(|db| rename::rename(db, position, new_name))
    }

    /// 光标 / 选区处可用的代码操作。
    pub fn assists(&self, range: FileRange) -> Cancellable<Vec<Assist>> {
        self.with_db(|db| assists::assists(db, range))
    }

    /// 整个文件里不改变行为的修复，合成一个操作（`source.fixAll`）。
    pub fn fix_all(&self, file_id: FileId) -> Cancellable<Option<Assist>> {
        self.with_db(|db| assists::fix_all(db, file_id))
    }

    /// 格式化。`range` 为 `None` 时处理整个文件。`options` 通常来自编辑器，
    /// `.editorconfig` 里设置了的项覆盖它。
    pub fn format(
        &self,
        file_id: FileId,
        range: Option<TextRange>,
        options: &FormatOptions,
    ) -> Cancellable<Vec<TextEdit>> {
        self.with_db(|db| {
            let Some(file) = Self::file(db, file_id) else {
                return Vec::new();
            };
            let options =
                options.with_editorconfig(ide_db::editorconfig::java_editorconfig(db, file));
            let root = base_db::parse(db, file).syntax_node();
            formatting::format(&root, file.text(db), range, &options)
        })
    }

    /// 输入一个字符之后的自动格式化：回车后给新行补缩进（`/**` 之后生成 Javadoc 骨架），
    /// `}` / `;` 之后重排所在的行。
    pub fn on_type_format(
        &self,
        position: FilePosition,
        typed: char,
        options: &FormatOptions,
    ) -> Cancellable<Vec<TextEdit>> {
        self.with_db(|db| {
            let Some(file) = Self::file(db, position.file_id) else {
                return Vec::new();
            };
            let text: &str = file.text(db);
            let options =
                &options.with_editorconfig(ide_db::editorconfig::java_editorconfig(db, file));
            if typed == '\n' {
                // `/**` 之后回车：生成 Javadoc 骨架
                if let Some(edits) = doc_comment::javadoc_on_enter(text, position.offset) {
                    return edits;
                }
                return formatting::indent_for_new_line(text, position.offset, options)
                    .into_iter()
                    .collect();
            }
            // 只重排光标所在的那一行
            let pos = usize::from(position.offset).min(text.len());
            let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
            let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
            if line_start == line_end {
                return Vec::new();
            }
            let line = TextRange::new((line_start as u32).into(), (line_end as u32).into());
            let root = base_db::parse(db, file).syntax_node();
            formatting::format_while_typing(&root, text, Some(line), options)
                .into_iter()
                .filter(|edit| line.contains_range(edit.range))
                .collect()
        })
    }

    pub fn inlay_hints(
        &self,
        file_id: FileId,
        range: Option<TextRange>,
    ) -> Cancellable<Vec<InlayHint>> {
        self.with_db(|db| {
            Self::file(db, file_id)
                .map(|file| inlay_hints::inlay_hints(db, file, range))
                .unwrap_or_default()
        })
    }

    /// 把一个源文件移到另一个包（点分形式）时需要同步的修改：package 声明和各处的 import。
    pub fn move_to_package(&self, file_id: FileId, new_package: &str) -> Cancellable<SourceChange> {
        self.with_db(|db| move_class::move_to_package(db, file_id, new_package))
    }

    /// 一起移动一批文件：`(文件, 新包)`。
    pub fn move_files(&self, moves: &[(FileId, String)]) -> Cancellable<SourceChange> {
        self.with_db(|db| move_class::move_files(db, moves))
    }

    /// 文件的包名（点分形式）；默认包为空串。
    pub fn package_name(&self, file_id: FileId) -> Cancellable<Option<String>> {
        self.with_db(|db| {
            let file = Self::file(db, file_id)?;
            let cu = hir::Semantics::new(db).parse(file);
            Some(
                cu.package()
                    .and_then(|p| p.name())
                    .map(|n| n.text().to_string())
                    .unwrap_or_default(),
            )
        })
    }

    /// 名为 `name` 的顶层类的名字在文件里的位置。
    pub fn top_level_class_offset(
        &self,
        file_id: FileId,
        name: &str,
    ) -> Cancellable<Option<TextSize>> {
        use syntax::ast::AstNode as _;
        self.with_db(|db| {
            let file = Self::file(db, file_id)?;
            let cu = hir::Semantics::new(db).parse(file);
            cu.type_decls()
                .filter_map(|decl| decl.name())
                .find(|n| n.text().as_str() == name)
                .map(|n| n.syntax().text_range().start())
        })
    }

    /// 文件里所有可以挂 code lens 的声明（不含数量）。
    pub fn code_lenses(&self, file_id: FileId) -> Cancellable<Vec<CodeLens>> {
        self.with_db(|db| {
            Self::file(db, file_id)
                .map(|file| code_lens::code_lenses(db, file))
                .unwrap_or_default()
        })
    }

    /// 一个 code lens 的数量。
    pub fn code_lens_count(
        &self,
        position: FilePosition,
        kind: CodeLensKind,
    ) -> Cancellable<usize> {
        self.with_db(|db| code_lens::code_lens_count(db, position, kind))
    }

    /// 依赖库里的类在哪里能找到源码。
    pub fn library_class_info(&self, binary_name: &str) -> Cancellable<Option<LibraryClassInfo>> {
        self.with_db(|db| library::library_class_info(db, binary_name))
    }

    /// 没有源码时，按 class 文件的签名生成一份只读的 Java 桩代码。
    /// `decompile` 为真时方法体从字节码反编译，否则只有签名。
    pub fn class_stub(&self, binary_name: &str, decompile: bool) -> Cancellable<Option<String>> {
        self.with_db(|db| library::class_stub(db, binary_name, decompile))
    }
}
