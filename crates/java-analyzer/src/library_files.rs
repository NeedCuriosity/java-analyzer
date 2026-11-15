//! 把依赖库 / JDK 里的类"物化"成磁盘上可以打开的 `.java` 文件。
//!
//! 有源码包（sources jar、JDK 的 `src.zip`）时从里面解压对应的文件；
//! 没有时写一份按签名生成的桩代码。结果放在缓存目录下，按容器路径和修改时间分目录，
//! 因此 jar 更新后不会读到旧文件。

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

use ide::Analysis;
use rustc_hash::FxHashMap;

pub struct LibraryFiles {
    cache_dir: PathBuf,
    /// 最外层类的二进制名 → 物化后的路径（`None` 表示确认无法物化）
    materialized: Mutex<FxHashMap<String, Option<PathBuf>>>,
}

fn fingerprint(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    if let Ok(meta) = fs::metadata(path) {
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            modified.hash(&mut hasher);
        }
    }
    format!("{:016x}", hasher.finish())
}

/// 从源码包里读出 `entry`（例如 `java/util/List.java`）。
/// JDK 9+ 的 `src.zip` 多一层模块目录（`java.base/java/util/List.java`），按后缀匹配。
fn read_source_entry(archive_path: &Path, entry: &str) -> Option<String> {
    let file = fs::File::open(archive_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let suffix = format!("/{entry}");
    let name = archive
        .file_names()
        .find(|name| *name == entry || name.ends_with(&suffix))
        .map(str::to_owned)?;
    let mut text = String::new();
    let mut zip_file = archive.by_name(&name).ok()?;
    let mut bytes = Vec::new();
    zip_file.read_to_end(&mut bytes).ok()?;
    text.push_str(&String::from_utf8_lossy(&bytes));
    Some(text)
}

impl LibraryFiles {
    pub fn new() -> LibraryFiles {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("java-analyzer")
            .join("sources");
        LibraryFiles {
            cache_dir,
            materialized: Mutex::new(FxHashMap::default()),
        }
    }

    /// 路径是否是物化出来的库源码文件。
    pub fn is_library_path(&self, path: &Path) -> bool {
        path.starts_with(&self.cache_dir)
    }

    /// 工程重新导入后类路径可能变了，之前的结论作废。
    pub fn clear(&self) {
        self.materialized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// 返回包含 `binary_name` 的源文件路径。
    pub fn materialize(
        &self,
        analysis: &Analysis,
        binary_name: &str,
    ) -> anyhow::Result<Option<PathBuf>> {
        let Some(info) = analysis.library_class_info(binary_name)? else {
            return Ok(None);
        };
        if let Some(cached) = self
            .materialized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&info.outermost)
        {
            return Ok(cached.clone());
        }

        let entry = format!("{}.java", info.outermost);
        let mut result = None;

        if let Some(sources) = &info.sources {
            let target = self
                .cache_dir
                .join("src")
                .join(fingerprint(sources))
                .join(&entry);
            if target.is_file() {
                result = Some(target);
            } else if let Some(text) = read_source_entry(sources, &entry) {
                if write_file(&target, &text) {
                    result = Some(target);
                }
            }
        }
        if result.is_none() {
            let container = info.container.as_deref().unwrap_or(Path::new("unknown"));
            let target = self
                .cache_dir
                .join("stubs")
                .join(fingerprint(container))
                .join(&entry);
            if target.is_file() {
                result = Some(target);
            } else if let Some(text) = analysis.class_stub(binary_name)? {
                if write_file(&target, &text) {
                    result = Some(target);
                }
            }
        }

        self.materialized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(info.outermost, result.clone());
        Ok(result)
    }
}

fn write_file(path: &Path, text: &str) -> bool {
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    match fs::write(path, text) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!("无法写入 {}: {err}", path.display());
            false
        }
    }
}
