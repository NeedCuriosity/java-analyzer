//! 增量计算的地基：salsa 输入、数据库 trait、解析查询。
//!
//! 整个分析层只有一个数据库 trait [`Db`]，各层的查询都是以 `&dyn Db` 为参数的 tracked 函数。
//! 输入只有三类：
//! - [`SourceFile`]：每个源码文件一份，字段是文本；
//! - [`Workspace`]（单例）：文件集合、工程图、类路径索引；
//! - 以及不走 salsa 的 [`Interner`]（纯函数式的字符串驻留，不影响增量正确性）。

mod interner;
#[path = "line_index.rs"]
mod line_idx;
mod parse_cache;
mod project;

use std::sync::Arc;

use classfile::ClassPath;
use rustc_hash::FxHashMap;
use syntax::{ast, Parse};

pub use salsa;
pub use vfs::FileId;

pub use crate::{
    interner::Interner,
    line_idx::{LineCol, LineIndex},
    parse_cache::ParseCache,
    project::{
        Dependency, DependencyTarget, JdkInfo, LibraryData, LibraryId, ModuleData, ModuleId,
        ProjectGraph, Scope, SourceRoot, SourceRootKind,
    },
};

#[salsa::db]
pub trait Db: salsa::Database {
    /// 进程级的字符串驻留器。同一个字符串永远得到同一个 id，因此在查询里使用是安全的。
    fn interner(&self) -> &Interner;

    /// 每个文件上一次的解析结果，供块级增量重解析使用。
    /// 增量结果与整文件解析等价，因此不影响查询的确定性。
    fn parse_cache(&self) -> &ParseCache;
}

/// 一个源码文件。
#[salsa::input(debug)]
pub struct SourceFile {
    pub file_id: FileId,
    #[returns(ref)]
    pub text: Arc<str>,
    /// 去掉 `.java` 的文件名。紧凑源文件的隐式类以它命名
    #[returns(ref)]
    pub name: smol_str::SmolStr,
}

/// 路径的文件名去掉 `.java` 后缀：`/a/b/Main.java` → `Main`。
pub fn file_stem(path: &str) -> smol_str::SmolStr {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    smol_str::SmolStr::new(name.strip_suffix(".java").unwrap_or(name))
}

/// 文件在工程里的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// 工作区源码。`module` 为 `None` 表示它不在任何已知的源码根下（散落的单文件）。
    Workspace {
        module: Option<ModuleId>,
        root: SourceRootKind,
    },
    /// 依赖库附带的源码（跳转到定义时打开），不参与工作区的类索引。
    Library,
}

#[derive(Clone, Copy)]
pub struct FileEntry {
    pub source: SourceFile,
    pub kind: FileKind,
}

impl std::fmt::Debug for FileEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileEntry")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// 当前已知的全部源码文件。
#[derive(Debug, Default, Clone)]
pub struct FileSet {
    files: FxHashMap<FileId, FileEntry>,
}

impl FileSet {
    pub fn insert(&mut self, file_id: FileId, entry: FileEntry) {
        self.files.insert(file_id, entry);
    }

    pub fn remove(&mut self, file_id: FileId) -> Option<FileEntry> {
        self.files.remove(&file_id)
    }

    pub fn get(&self, file_id: FileId) -> Option<FileEntry> {
        self.files.get(&file_id).copied()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (FileId, FileEntry)> + '_ {
        self.files.iter().map(|(id, entry)| (*id, *entry))
    }

    /// 工作区源码文件，按 `FileId` 排序（保证依赖它的查询结果是确定的）。
    pub fn workspace_files(&self) -> Vec<(FileId, SourceFile)> {
        let mut res: Vec<_> = self
            .files
            .iter()
            .filter(|(_, entry)| matches!(entry.kind, FileKind::Workspace { .. }))
            .map(|(id, entry)| (*id, entry.source))
            .collect();
        res.sort_by_key(|(id, _)| *id);
        res
    }
}

/// 全局单例输入：文件集合、工程结构、二进制类路径。
#[salsa::input(singleton)]
pub struct Workspace {
    #[returns(ref)]
    pub files: Arc<FileSet>,
    #[returns(ref)]
    pub project: Arc<ProjectGraph>,
    #[returns(ref)]
    pub class_path: Arc<ClassPath>,
}

/// 取得工作区单例。宿主必须在启动时先调用一次 [`init_workspace`]。
pub fn workspace(db: &dyn Db) -> Workspace {
    Workspace::get(db)
}

/// 创建空的工作区单例（已存在则直接返回）。
pub fn init_workspace(db: &dyn Db) -> Workspace {
    match Workspace::try_get(db) {
        Some(ws) => ws,
        None => Workspace::new(
            db,
            Arc::new(FileSet::default()),
            Arc::new(ProjectGraph::default()),
            Arc::new(ClassPath::default()),
        ),
    }
}

pub fn file_entry(db: &dyn Db, file_id: FileId) -> Option<FileEntry> {
    workspace(db).files(db).get(file_id)
}

pub fn source_file(db: &dyn Db, file_id: FileId) -> Option<SourceFile> {
    file_entry(db, file_id).map(|entry| entry.source)
}

/// 解析一个文件。语法树占内存较多，只保留最近使用的一批，其余按需重算。
#[salsa::tracked(lru = 256)]
pub fn parse(db: &dyn Db, file: SourceFile) -> Parse<ast::CompilationUnit> {
    let text = file.text(db);
    let file_id = file.file_id(db);
    let cache = db.parse_cache();

    // 在方法体里敲字是最常见的编辑：只重新解析被改动的那个块
    let incremental =
        cache.get(file_id).and_then(|(old_text, old_parse)| {
            match syntax::diff_range(&old_text, text) {
                None => Some(old_parse),
                Some(edit) => old_parse.reparse(edit, text),
            }
        });
    let parse = incremental.unwrap_or_else(|| ast::CompilationUnit::parse(text));
    cache.put(file_id, text.clone(), parse.clone());
    parse
}

#[salsa::tracked]
pub fn line_index(db: &dyn Db, file: SourceFile) -> Arc<LineIndex> {
    Arc::new(LineIndex::new(file.text(db)))
}

// ---------------------------------------------------------------------------
// 界面语言
// ---------------------------------------------------------------------------

/// 面向用户的文案（诊断、代码操作的名字、提示）用哪种语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

static LANG: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// 当前的界面语言。进程级的设置：语言服务器在 `initialize` 时按客户端的 `locale` 定下来。
pub fn lang() -> Lang {
    match LANG.load(std::sync::atomic::Ordering::Relaxed) {
        0 => Lang::Zh,
        _ => Lang::En,
    }
}

pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, std::sync::atomic::Ordering::Relaxed);
}

/// 按界面语言格式化一条文案：`tr!("无法解析符号 `{}`", "Cannot resolve symbol `{}`", name)`。
/// 两种语言的占位符必须一一对应。
#[macro_export]
macro_rules! tr {
    ($zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        match $crate::lang() {
            $crate::Lang::Zh => format!($zh $(, $arg)*),
            $crate::Lang::En => format!($en $(, $arg)*),
        }
    };
}

/// 不带参数的文案，结果是 `&'static str`。
#[macro_export]
macro_rules! trs {
    ($zh:literal, $en:literal) => {
        match $crate::lang() {
            $crate::Lang::Zh => $zh,
            $crate::Lang::En => $en,
        }
    };
}
