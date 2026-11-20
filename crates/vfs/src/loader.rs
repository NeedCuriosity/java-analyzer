//! 从磁盘批量加载源码文件。

use std::path::{Path, PathBuf};

use encoding_rs::Encoding;
use rayon::prelude::*;

pub struct LoadedFile {
    pub path: PathBuf,
    pub text: String,
}

/// 各源码根的源文件编码（来自构建文件里的 `project.build.sourceEncoding` 等）。
/// 路径落在多个互相嵌套的根下时取最深的那个；没有声明编码的根按 UTF-8 读。
#[derive(Debug, Default, Clone)]
pub struct SourceEncodings {
    roots: Vec<(PathBuf, Option<&'static Encoding>)>,
}

impl SourceEncodings {
    /// `roots`：源码根和它所在模块声明的编码名（没有声明时为 `None`）。
    /// 认不出的编码名当作没有声明。
    pub fn new<'a>(roots: impl IntoIterator<Item = (PathBuf, Option<&'a str>)>) -> Self {
        let roots = roots
            .into_iter()
            .map(|(root, label)| {
                let encoding = label.and_then(|label| {
                    let encoding = encoding_for_label(label);
                    if encoding.is_none() {
                        tracing::warn!(
                            "不认识的源文件编码 {label}，{} 下按 UTF-8 读",
                            root.display()
                        );
                    }
                    encoding
                });
                (root, encoding)
            })
            .collect();
        SourceEncodings { roots }
    }

    /// 路径所在源码根声明的编码；不在任何源码根下或者没有声明时为 `None`。
    pub fn for_path(&self, path: &Path) -> Option<&'static Encoding> {
        self.roots
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .and_then(|(_, encoding)| *encoding)
    }
}

/// 构建文件里写的编码名对应的编码。除了 WHATWG 的名字（`GBK`、`UTF-8`、`ISO-8859-1` …），
/// 也认 Java 自己的别名（`Cp1252`、`MS936` …）。
pub fn encoding_for_label(label: &str) -> Option<&'static Encoding> {
    let label = label.trim();
    if let Some(encoding) = Encoding::for_label(label.as_bytes()) {
        return Some(encoding);
    }
    let alias = match label.to_ascii_lowercase().as_str() {
        "utf8" => "utf-8".to_owned(),
        "ms936" | "cp936" => "gbk".to_owned(),
        "ms932" | "cp932" | "windows-31j" => "shift_jis".to_owned(),
        "ms949" | "cp949" => "euc-kr".to_owned(),
        "ms950" | "cp950" => "big5".to_owned(),
        other => match other.strip_prefix("cp") {
            Some(page) if page.len() == 4 && page.starts_with("125") => format!("windows-{page}"),
            _ => return None,
        },
    };
    Encoding::for_label(alias.as_bytes())
}

/// 把文件内容解码成文本。合法的 UTF-8 总是按 UTF-8 读：不少工程声明了 GBK，文件却只有
/// ASCII 或者其实是按 UTF-8 存的。否则按 `encoding` 解码，没有给出时按 UTF-8；
/// 非法字节换成 U+FFFD。
pub fn decode(bytes: Vec<u8>, encoding: Option<&'static Encoding>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => match encoding {
            Some(encoding) if encoding != encoding_rs::UTF_8 => {
                encoding.decode(err.as_bytes()).0.into_owned()
            }
            _ => String::from_utf8_lossy(err.as_bytes()).into_owned(),
        },
    }
}

/// 读文件；非法 UTF-8 用替换字符兜底。不在源码根下的文件（配置文件等）用这个。
pub fn read_file_lossy(path: &Path) -> Option<String> {
    read_file(path, None)
}

/// 读文件并按 [`decode`] 的规则解码。
pub fn read_file(path: &Path, encoding: Option<&'static Encoding>) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(decode(bytes, encoding))
}

/// 递归收集目录下所有 `.java` 文件并并行读入，按各自源码根声明的编码解码。
/// 跳过隐藏目录；不跟随符号链接，避免环。
pub fn load_java_files(roots: &[PathBuf], encodings: &SourceEncodings) -> Vec<LoadedFile> {
    let mut paths = Vec::new();
    for root in roots {
        collect(root, &mut paths);
    }
    paths.sort();
    paths.dedup();
    paths
        .into_par_iter()
        .filter_map(|path| {
            let text = read_file(&path, encodings.for_path(&path))?;
            Some(LoadedFile { path, text })
        })
        .collect()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::debug!("无法读取目录 {}: {err}", dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if name.starts_with('.') {
                continue;
            }
            collect(&path, out);
        } else if file_type.is_file() && name.ends_with(".java") {
            out.push(path);
        }
    }
}

/// Spring Boot 的配置文件：`application*.properties` / `.yml` / `.yaml`（`bootstrap*` 同理）。
pub fn is_spring_config(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    (name.starts_with("application") || name.starts_with("bootstrap"))
        && [".properties", ".yml", ".yaml"]
            .iter()
            .any(|ext| name.ends_with(ext))
}

/// Spring Boot 的配置元数据：`META-INF/spring-configuration-metadata.json`
/// （构建时由注解处理器生成）和手写的 `META-INF/additional-spring-configuration-metadata.json`。
pub fn is_spring_metadata(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    matches!(
        name,
        "spring-configuration-metadata.json" | "additional-spring-configuration-metadata.json"
    ) && path
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|dir| dir == "META-INF")
}

/// 作为资源文件交给分析器的文件：Spring 配置文件和配置元数据。
pub fn is_spring_resource(path: &Path) -> bool {
    is_spring_config(path) || is_spring_metadata(path)
}

/// 构建输出里放配置元数据的目录（Maven、Gradle）。
const METADATA_OUTPUT_DIRS: &[&str] = &[
    "target/classes/META-INF",
    "build/classes/java/main/META-INF",
    "build/resources/main/META-INF",
    "build/tmp/kapt3/classes/main/META-INF",
];

/// 读入各模块资源目录（`src/main/resources`、`src/test/resources`，以及模块根下的 `config/`）
/// 里的 Spring 配置文件和配置元数据，以及构建输出里生成的配置元数据。
pub fn load_spring_configs(module_roots: &[PathBuf]) -> Vec<LoadedFile> {
    let mut paths = Vec::new();
    for root in module_roots {
        for dir in ["src/main/resources", "src/test/resources", "config"] {
            collect_configs(&root.join(dir), 0, &mut paths);
        }
        // 构建输出里只取元数据：复制过去的配置文件和源码里的是同一份
        for dir in METADATA_OUTPUT_DIRS {
            for name in [
                "spring-configuration-metadata.json",
                "additional-spring-configuration-metadata.json",
            ] {
                let path = root.join(dir).join(name);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let text = read_file_lossy(&path)?;
            Some(LoadedFile { path, text })
        })
        .collect()
}

fn collect_configs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    // 只看资源目录本身和 `config/` 这类浅层子目录
    if depth > 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if !entry.file_name().to_string_lossy().starts_with('.') {
                collect_configs(&path, depth + 1, out);
            }
        } else if file_type.is_file() && is_spring_resource(&path) {
            out.push(path);
        }
    }
}

/// `lombok.config`。
pub fn is_lombok_config(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "lombok.config")
}

/// `.editorconfig`（代码风格，例如 import 的分组顺序）。
pub fn is_editorconfig(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == ".editorconfig")
}

/// 作为资源文件交给分析器的非 Java 文件：Spring 配置、配置元数据、`lombok.config`
/// 和 `.editorconfig`。
pub fn is_resource_file(path: &Path) -> bool {
    is_spring_resource(path) || is_lombok_config(path) || is_editorconfig(path)
}

/// 按目录生效的配置文件：`lombok.config`（Lombok 从源文件所在目录逐级往上找）和
/// `.editorconfig`（代码风格）。读入各模块根以及它们往上的目录里的（遇到 `.git` 所在的目录
/// 为止），以及源码根下各级目录里的。按路径排序，外层目录的在前。
pub fn load_dir_configs(module_roots: &[PathBuf], source_roots: &[PathBuf]) -> Vec<LoadedFile> {
    const NAMES: [&str; 2] = ["lombok.config", ".editorconfig"];
    let mut paths: Vec<PathBuf> = NAMES
        .iter()
        .flat_map(|name| upward_paths(module_roots, name))
        .collect();
    for root in source_roots {
        collect_named(root, &NAMES, &mut paths);
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let text = read_file_lossy(&path)?;
            Some(LoadedFile { path, text })
        })
        .collect()
}

/// 目录下（递归，跳过隐藏目录）所有叫 `name` 的文件。
fn collect_named(dir: &Path, names: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let file_name = entry.file_name();
        if file_type.is_dir() {
            if !file_name.to_string_lossy().starts_with('.') {
                collect_named(&entry.path(), names, out);
            }
        } else if file_type.is_file() && names.iter().any(|name| file_name == *name) {
            out.push(entry.path());
        }
    }
}

fn upward_paths(module_roots: &[PathBuf], name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for root in module_roots {
        for dir in root.ancestors().take(8) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                paths.push(candidate);
            }
            if dir.join(".git").exists() {
                break;
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!("ja-vfs-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SOURCE: &str = "// 中文注释\nclass A { String s = \"你好\"; }\n";

    #[test]
    fn files_are_decoded_with_the_encoding_of_their_source_root() {
        let dir = TempDir::new("encoding");
        let gbk_root = dir.0.join("gbk/src/main/java");
        let plain_root = dir.0.join("plain/src/main/java");
        std::fs::create_dir_all(&gbk_root).unwrap();
        std::fs::create_dir_all(&plain_root).unwrap();
        let (gbk_bytes, _, _) = encoding_rs::GBK.encode(SOURCE);
        assert!(std::str::from_utf8(&gbk_bytes).is_err());
        std::fs::write(gbk_root.join("A.java"), &gbk_bytes).unwrap();
        // 声明了 GBK 的模块里按 UTF-8 存的文件
        std::fs::write(gbk_root.join("B.java"), SOURCE).unwrap();
        // 没有声明编码的模块里的 GBK 文件：按 UTF-8 读，非法字节变成替换字符
        std::fs::write(plain_root.join("C.java"), &gbk_bytes).unwrap();

        let encodings =
            SourceEncodings::new([(gbk_root.clone(), Some("GBK")), (plain_root.clone(), None)]);
        let files = load_java_files(&[gbk_root, plain_root], &encodings);
        let text = |name: &str| {
            files
                .iter()
                .find(|f| f.path.file_name().is_some_and(|n| n == name))
                .map(|f| f.text.as_str())
                .unwrap()
        };
        assert_eq!(text("A.java"), SOURCE);
        assert_eq!(text("B.java"), SOURCE);
        assert!(text("C.java").contains('\u{FFFD}'));
    }

    #[test]
    fn deepest_source_root_decides_the_encoding() {
        let encodings = SourceEncodings::new([
            (PathBuf::from("/p/src"), Some("GBK")),
            (PathBuf::from("/p/src/generated"), None),
            (PathBuf::from("/q/src"), Some("not-an-encoding")),
        ]);
        let gbk = Some(encoding_rs::GBK);
        assert_eq!(encodings.for_path(Path::new("/p/src/A.java")), gbk);
        assert_eq!(
            encodings.for_path(Path::new("/p/src/generated/A.java")),
            None
        );
        assert_eq!(encodings.for_path(Path::new("/q/src/A.java")), None);
        assert_eq!(encodings.for_path(Path::new("/r/A.java")), None);
    }

    #[test]
    fn java_encoding_names_are_recognized() {
        assert_eq!(encoding_for_label("UTF-8"), Some(encoding_rs::UTF_8));
        assert_eq!(encoding_for_label("utf8"), Some(encoding_rs::UTF_8));
        assert_eq!(encoding_for_label("gb2312"), Some(encoding_rs::GBK));
        assert_eq!(encoding_for_label("MS936"), Some(encoding_rs::GBK));
        assert_eq!(encoding_for_label("GB18030"), Some(encoding_rs::GB18030));
        assert_eq!(
            encoding_for_label("Cp1252"),
            Some(encoding_rs::WINDOWS_1252)
        );
        assert_eq!(
            encoding_for_label("ISO-8859-1"),
            Some(encoding_rs::WINDOWS_1252)
        );
        assert_eq!(encoding_for_label("nonsense"), None);
    }
}
