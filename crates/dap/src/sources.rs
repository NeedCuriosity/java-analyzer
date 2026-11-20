//! 源文件和类之间的对应：
//! - 设置断点时，从源文件读出包名和顶层类型名，得到要匹配的类名（`pkg.Outer`、`pkg.Outer$*`）；
//! - 显示调用栈时，从类名和 `SourceFile` 属性得到 `pkg/Outer.java`，在源码根和源码包里找。

use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use base_db::tr;
use syntax::ast;

/// 读源文件；非 UTF-8 的字节（GBK 等编码的注释、字符串）换成 U+FFFD。这里只用到包名、
/// 类名和 import，不必按工程的编码解码。
pub(crate) fn read_lossy(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    })
}

/// 一个源文件里声明的类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceClasses {
    /// 包名，默认包为空
    pub package: String,
    /// 顶层类型的简单名字
    pub types: Vec<String>,
    /// 文件名，例如 `Main.java`
    pub file_name: String,
}

impl SourceClasses {
    /// 读取源文件。读不到或者解析不出类型时，按文件名猜一个类名。
    pub fn read(path: &Path) -> SourceClasses {
        let text = read_lossy(path).unwrap_or_default();
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut classes = SourceClasses::parse(&text, &file_name);
        if classes.types.is_empty() {
            if let Some(stem) = path.file_stem() {
                classes.types.push(stem.to_string_lossy().into_owned());
            }
        }
        classes
    }

    pub fn parse(text: &str, file_name: &str) -> SourceClasses {
        let unit = ast::CompilationUnit::parse(text).tree();
        let package = unit
            .package()
            .and_then(|p| p.name())
            .map(|n| n.text())
            .unwrap_or_default();
        let types = unit
            .type_decls()
            .filter_map(|decl| decl.name())
            .map(|name| name.text().to_string())
            .filter(|name| !name.is_empty())
            .collect();
        SourceClasses {
            package,
            types,
            file_name: file_name.to_owned(),
        }
    }

    fn qualified(&self, simple: &str) -> String {
        if self.package.is_empty() {
            simple.to_owned()
        } else {
            format!("{}.{simple}", self.package)
        }
    }

    /// `ClassPrepare` 事件用的类名模式（只支持开头或结尾的 `*`）。
    pub fn class_patterns(&self) -> Vec<String> {
        self.types
            .iter()
            .flat_map(|t| {
                let name = self.qualified(t);
                [format!("{name}$*"), name]
            })
            .collect()
    }

    /// 类（Java 名字，例如 `demo.Main$1`）是不是这个文件里声明的。
    pub fn contains_class(&self, class_name: &str) -> bool {
        self.types.iter().any(|t| {
            let name = self.qualified(t);
            class_name == name
                || class_name
                    .strip_prefix(name.as_str())
                    .is_some_and(|rest| rest.starts_with('$'))
        })
    }
}

/// 类所在源文件相对源码根的路径：`demo.Outer$Inner` + `Outer.java` → `demo/Outer.java`。
pub fn relative_source_path(class_name: &str, source_file: Option<&str>) -> String {
    let (package, simple) = match class_name.rsplit_once('.') {
        Some((package, simple)) => (package, simple),
        None => ("", class_name),
    };
    let file = match source_file {
        Some(file) => file.to_owned(),
        None => format!("{}.java", simple.split('$').next().unwrap_or(simple)),
    };
    if package.is_empty() {
        file
    } else {
        format!("{}/{file}", package.replace('.', "/"))
    }
}

/// 找到的源码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoundSource {
    File(PathBuf),
    /// 源码包里的条目，按 `sourceReference` 取内容
    Archive {
        reference: i64,
        name: String,
    },
}

/// 在源码根目录和源码包（`-sources.jar`、JDK 的 `src.zip`）里找源文件。
#[derive(Default)]
pub struct SourceLocator {
    roots: Vec<PathBuf>,
    archives: Vec<PathBuf>,
    /// 源码包的索引：相对路径 → (源码包下标, 条目名)，第一次用到时才建
    index: Option<HashMap<String, (usize, String)>>,
    /// 已分配的 `sourceReference`：下标 + 1 就是引用号
    references: Vec<(usize, String)>,
    cache: HashMap<String, Option<FoundSource>>,
}

impl SourceLocator {
    pub fn new(roots: Vec<PathBuf>, archives: Vec<PathBuf>) -> SourceLocator {
        let mut unique_roots: Vec<PathBuf> = Vec::new();
        for root in roots {
            if !unique_roots.contains(&root) {
                unique_roots.push(root);
            }
        }
        let mut unique_archives: Vec<PathBuf> = Vec::new();
        for archive in archives {
            if archive.is_file() && !unique_archives.contains(&archive) {
                unique_archives.push(archive);
            }
        }
        SourceLocator {
            roots: unique_roots,
            archives: unique_archives,
            ..SourceLocator::default()
        }
    }

    pub fn find(&mut self, relative: &str) -> Option<FoundSource> {
        if let Some(found) = self.cache.get(relative) {
            return found.clone();
        }
        let found = self
            .roots
            .iter()
            .map(|root| root.join(relative))
            .find(|path| path.is_file())
            .map(FoundSource::File)
            .or_else(|| self.find_in_archives(relative));
        self.cache.insert(relative.to_owned(), found.clone());
        found
    }

    fn find_in_archives(&mut self, relative: &str) -> Option<FoundSource> {
        if self.archives.is_empty() {
            return None;
        }
        let archives = &self.archives;
        let index = self.index.get_or_insert_with(|| build_index(archives));
        let (archive, entry) = index.get(relative)?.clone();
        let reference = match self
            .references
            .iter()
            .position(|(a, e)| *a == archive && *e == entry)
        {
            Some(i) => i as i64 + 1,
            None => {
                self.references.push((archive, entry));
                self.references.len() as i64
            }
        };
        let name = relative.rsplit('/').next().unwrap_or(relative).to_owned();
        Some(FoundSource::Archive { reference, name })
    }

    /// 源码包条目的内容。
    pub fn read_reference(&self, reference: i64) -> anyhow::Result<String> {
        let (archive, entry) = usize::try_from(reference - 1)
            .ok()
            .and_then(|i| self.references.get(i))
            .ok_or_else(|| {
                anyhow::anyhow!(tr!(
                    "未知的 sourceReference {reference}",
                    "Unknown sourceReference {reference}"
                ))
            })?;
        let file = File::open(&self.archives[*archive])?;
        let mut zip = zip::ZipArchive::new(file)?;
        let mut content = String::new();
        zip.by_name(entry)?.read_to_string(&mut content)?;
        Ok(content)
    }

    /// 源码包所在路径（显示给用户看来源）。
    pub fn reference_origin(&self, reference: i64) -> Option<&Path> {
        let (archive, _) = self.references.get(usize::try_from(reference - 1).ok()?)?;
        Some(&self.archives[*archive])
    }
}

fn build_index(archives: &[PathBuf]) -> HashMap<String, (usize, String)> {
    let mut index: HashMap<String, (usize, String)> = HashMap::new();
    for (i, path) in archives.iter().enumerate() {
        let Ok(file) = File::open(path) else { continue };
        let Ok(zip) = zip::ZipArchive::new(file) else {
            continue;
        };
        for name in zip.file_names() {
            if !name.ends_with(".java") {
                continue;
            }
            index
                .entry(name.to_owned())
                .or_insert_with(|| (i, name.to_owned()));
            // JDK 的 src.zip 按模块分目录：`java.base/java/lang/String.java`
            if let Some((first, rest)) = name.split_once('/') {
                if first.contains('.') {
                    index
                        .entry(rest.to_owned())
                        .or_insert_with(|| (i, name.to_owned()));
                }
            }
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_of_a_source_file() {
        let text = "package a.b;\n\nimport java.util.*;\n\npublic class Main {\n  class Inner {}\n}\nclass Helper {}\n";
        let classes = SourceClasses::parse(text, "Main.java");
        assert_eq!(classes.package, "a.b");
        assert_eq!(classes.types, ["Main", "Helper"]);
        assert_eq!(
            classes.class_patterns(),
            ["a.b.Main$*", "a.b.Main", "a.b.Helper$*", "a.b.Helper"]
        );
        assert!(classes.contains_class("a.b.Main"));
        assert!(classes.contains_class("a.b.Main$Inner"));
        assert!(classes.contains_class("a.b.Main$1"));
        assert!(classes.contains_class("a.b.Helper"));
        assert!(!classes.contains_class("a.b.MainTest"));
        assert!(!classes.contains_class("a.Main"));

        let default_package = SourceClasses::parse("record Point(int x) {}", "Point.java");
        assert_eq!(default_package.package, "");
        assert_eq!(default_package.class_patterns(), ["Point$*", "Point"]);
    }

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative_source_path("demo.Outer$Inner", Some("Outer.java")),
            "demo/Outer.java"
        );
        assert_eq!(relative_source_path("a.b.C$1", None), "a/b/C.java");
        assert_eq!(relative_source_path("Main", Some("Main.java")), "Main.java");
    }

    #[test]
    fn finds_files_under_roots() {
        let dir = std::env::temp_dir().join(format!("dap-sources-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("demo")).unwrap();
        std::fs::write(dir.join("demo/Main.java"), "package demo;").unwrap();
        let mut locator =
            SourceLocator::new(vec![PathBuf::from("/nonexistent"), dir.clone()], Vec::new());
        assert_eq!(
            locator.find("demo/Main.java"),
            Some(FoundSource::File(dir.join("demo/Main.java")))
        );
        assert_eq!(locator.find("demo/Missing.java"), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
