//! 测试用的内联 fixture。
//!
//! 一段 fixture 文本描述若干个源文件，文件之间用 `//- /路径` 分隔；没有分隔行时整段是一个文件
//! `/Main.java`。文本里至多出现一次 `$0`，表示光标位置（会从文本里去掉）。
//!
//! ```text
//! //- /a/Foo.java
//! package a;
//! public class Foo { public int x; }
//! //- /b/Bar.java
//! package b;
//! class Bar { void f(a.Foo foo) { foo.$0 } }
//! ```
//!
//! 每个 fixture 都自带 [`MINICORE`]：手写的 `java.lang` / `java.util` 最小源码桩，
//! 因此测试不依赖真实的 JDK。

mod minicore;

use std::sync::Arc;

use base_db::{
    salsa::Setter, workspace, FileEntry, FileId, FileKind, FileSet, ModuleData, ProjectGraph,
    SourceFile, SourceRootKind,
};
use ide_db::RootDatabase;
use syntax::TextSize;

pub use crate::minicore::MINICORE;

pub const CURSOR_MARKER: &str = "$0";

#[derive(Debug, Clone)]
pub struct FixtureFile {
    pub path: String,
    pub file_id: FileId,
    pub source: SourceFile,
}

pub struct Fixture {
    pub db: RootDatabase,
    /// fixture 里的文件（不含 minicore）
    pub files: Vec<FixtureFile>,
    /// `$0` 所在的文件和偏移
    pub cursor: Option<(FixtureFile, TextSize)>,
}

impl Fixture {
    /// 第一个文件（单文件 fixture 的那个文件）。
    pub fn file(&self) -> &FixtureFile {
        &self.files[0]
    }

    /// 光标位置；fixture 里没有 `$0` 时 panic。
    pub fn position(&self) -> (SourceFile, FileId, TextSize) {
        let (file, offset) = self.cursor.as_ref().expect("fixture 里没有 $0");
        (file.source, file.file_id, *offset)
    }

    pub fn file_by_path(&self, path: &str) -> &FixtureFile {
        self.files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("fixture 里没有文件 {path}"))
    }

    /// 把整个工程的语言级别设成 `level`：放一个只有语言级别的模块。fixture 的文件不属于任何
    /// 模块，按工程里最高的语言级别算。
    pub fn set_language_level(&mut self, level: u32) {
        let project = ProjectGraph {
            modules: vec![ModuleData {
                name: "fixture".to_owned(),
                root: std::path::PathBuf::from("/"),
                source_roots: Vec::new(),
                language_level: Some(level),
                test_language_level: None,
                api_release: None,
                preview: false,
                dependencies: Vec::new(),
                output_dirs: Vec::new(),
                encoding: None,
            }],
            ..ProjectGraph::default()
        };
        workspace(&self.db)
            .set_project(&mut self.db)
            .to(Arc::new(project));
    }
}

/// 带语言级别的 [`fixture`]。
pub fn fixture_with_level(text: &str, level: u32) -> Fixture {
    let mut fx = fixture(text);
    fx.set_language_level(level);
    fx
}

/// 把 fixture 文本拆成（路径，内容）。
pub fn split_fixture(text: &str) -> Vec<(String, String)> {
    let mut res: Vec<(String, String)> = Vec::new();
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.trim_start().strip_prefix("//- ") {
            res.push((rest.trim().to_owned(), String::new()));
        } else {
            match res.last_mut() {
                Some((_, content)) => content.push_str(line),
                // 第一个分隔行之前的空行不算内容
                None if line.trim().is_empty() => {}
                None => res.push(("/Main.java".to_owned(), line.to_owned())),
            }
        }
    }
    res
}

/// 建立一个装好 fixture 文件和 minicore 的数据库。
pub fn fixture(text: &str) -> Fixture {
    let mut db = RootDatabase::new();
    let mut set = FileSet::default();
    let mut files = Vec::new();
    let mut cursor = None;
    let mut next_id = 0u32;
    let kind = FileKind::Workspace {
        module: None,
        root: SourceRootKind::Main,
    };

    for (path, mut content) in split_fixture(text) {
        let offset = content.find(CURSOR_MARKER).map(|idx| {
            content.replace_range(idx..idx + CURSOR_MARKER.len(), "");
            TextSize::from(idx as u32)
        });
        let file_id = FileId(next_id);
        next_id += 1;
        let source = SourceFile::new(
            &db,
            file_id,
            Arc::from(content),
            base_db::file_stem(&path),
            base_db::file_dir(&path),
        );
        set.insert(file_id, FileEntry { source, kind });
        let file = FixtureFile {
            path,
            file_id,
            source,
        };
        if let Some(offset) = offset {
            assert!(cursor.is_none(), "fixture 里只能有一个 $0");
            cursor = Some((file.clone(), offset));
        }
        files.push(file);
    }
    for (path, content) in split_fixture(MINICORE) {
        let file_id = FileId(next_id);
        next_id += 1;
        let source = SourceFile::new(
            &db,
            file_id,
            Arc::from(content),
            base_db::file_stem(&path),
            base_db::file_dir(&path),
        );
        set.insert(file_id, FileEntry { source, kind });
    }

    let ws = workspace(&db);
    ws.set_files(&mut db).to(Arc::new(set));
    Fixture { db, files, cursor }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_files_and_extracts_cursor() {
        let fx = fixture("//- /a/A.java\npackage a;\nclass A {}\n//- /b/B.java\nclass B { $0 }\n");
        assert_eq!(fx.files.len(), 2);
        assert_eq!(fx.files[0].path, "/a/A.java");
        let (file, offset) = fx.cursor.as_ref().unwrap();
        assert_eq!(file.path, "/b/B.java");
        assert_eq!(
            &file.source.text(&fx.db)[..usize::from(*offset)],
            "class B { "
        );
        assert!(!file.source.text(&fx.db).contains("$0"));
    }

    #[test]
    fn language_level_applies_to_fixture_files() {
        let fx = fixture_with_level("class Main {}", 6);
        let project = workspace(&fx.db).project(&fx.db);
        assert_eq!(project.max_language_level(), Some(6));
    }

    #[test]
    fn single_file_without_header() {
        let fx = fixture("class Main {}");
        assert_eq!(fx.file().path, "/Main.java");
    }

    #[test]
    fn minicore_parses_without_errors() {
        for (path, content) in split_fixture(MINICORE) {
            let parse = syntax::ast::CompilationUnit::parse(&content);
            assert!(parse.errors().is_empty(), "{path}: {:?}", parse.errors());
        }
    }
}
