//! 把依赖库 / JDK 里的类"物化"成磁盘上可以打开的 `.java` 文件。
//!
//! 有源码包（sources jar、JDK 的 `src.zip`）时从里面解压对应的文件；
//! 没有时写一份按签名生成的桩代码。结果放在缓存目录下，按容器路径和修改时间分目录，
//! 因此 jar 更新后不会读到旧文件；桩代码还按生成器的版本分目录，生成规则变了之后会重新生成。

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
    /// (最外层类的二进制名, 是否反编译方法体) → 物化后的路径（`None` 表示确认无法物化）
    materialized: Mutex<FxHashMap<(String, bool), Option<PathBuf>>>,
    /// 等着下载源码包的 jar，以及请求过的全部 jar（每个 jar 只请求一次）
    wanted_sources: Mutex<(Vec<PathBuf>, rustc_hash::FxHashSet<PathBuf>)>,
}

/// 桩代码的格式版本：生成规则（`ide::class_stub`）有变化时加一，让旧的缓存文件失效。
const STUB_FORMAT: u32 = 3;

fn fingerprint(path: &Path) -> String {
    fingerprint_salted(path, "")
}

/// 桩代码的目录：除了容器本身，还取决于生成器的版本和是否反编译方法体。
fn stub_fingerprint(container: &Path, decompile: bool) -> String {
    let mode = if decompile {
        "decompiled"
    } else {
        "signatures"
    };
    fingerprint_salted(
        container,
        &format!("{}-{STUB_FORMAT}-{mode}", env!("CARGO_PKG_VERSION")),
    )
}

fn fingerprint_salted(path: &Path, salt: &str) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    salt.hash(&mut hasher);
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
            wanted_sources: Mutex::default(),
        }
    }

    /// 类所在的 jar 没有源码包时，记下来等后台下载。
    pub fn want_sources(&self, analysis: &Analysis, binary_name: &str) -> anyhow::Result<()> {
        let Some(info) = analysis.library_class_info(binary_name)? else {
            return Ok(());
        };
        let Some(jar) = info
            .container
            .filter(|c| info.sources.is_none() && c.is_file())
        else {
            return Ok(());
        };
        let mut wanted = self
            .wanted_sources
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if wanted.1.insert(jar.clone()) {
            wanted.0.push(jar);
        }
        Ok(())
    }

    /// 取出等着下载源码包的 jar。
    pub fn take_wanted_sources(&self) -> Vec<PathBuf> {
        std::mem::take(
            &mut self
                .wanted_sources
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .0,
        )
    }

    /// 源码包下载好之后，把这个 jar 已经生成的桩文件就地换成真正的源码
    /// （编辑器里开着的桩文件会随之刷新）。返回改写了的文件。
    pub fn replace_stubs(&self, jar: &Path, sources: &Path) -> Vec<PathBuf> {
        let mut replaced = Vec::new();
        // 两种桩（反编译的、只有签名的）都可能被打开过
        for decompile in [true, false] {
            let dir = self
                .cache_dir
                .join("stubs")
                .join(stub_fingerprint(jar, decompile));
            replace_stubs_in(&dir, sources, &mut replaced);
        }
        replaced
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

    /// 返回包含 `binary_name` 的源文件路径。没有源码时生成桩代码，`decompile` 决定方法体是否反编译。
    pub fn materialize(
        &self,
        analysis: &Analysis,
        binary_name: &str,
        decompile: bool,
    ) -> anyhow::Result<Option<PathBuf>> {
        let Some(info) = analysis.library_class_info(binary_name)? else {
            return Ok(None);
        };
        let key = (info.outermost.clone(), decompile);
        if let Some(cached) = self
            .materialized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
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
                .join(stub_fingerprint(container, decompile))
                .join(&entry);
            if target.is_file() {
                result = Some(target);
            } else if let Some(text) = analysis.class_stub(binary_name, decompile)? {
                if write_file(&target, &text) {
                    result = Some(target);
                }
            }
        }

        self.materialized
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, result.clone());
        Ok(result)
    }
}

/// 把 `dir` 下的桩文件换成源码包里的同名文件。
fn replace_stubs_in(dir: &Path, sources: &Path, replaced: &mut Vec<PathBuf>) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(relative) = path.strip_prefix(dir).ok().and_then(|r| r.to_str()) else {
                continue;
            };
            let entry_name = relative.replace(std::path::MAIN_SEPARATOR, "/");
            if let Some(text) = read_source_entry(sources, &entry_name) {
                if write_file(&path, &text) {
                    replaced.push(path);
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_directory_depends_on_generator_version() {
        let jar = Path::new("/nonexistent/lib.jar");
        assert_ne!(fingerprint(jar), stub_fingerprint(jar, true));
        assert_eq!(stub_fingerprint(jar, true), stub_fingerprint(jar, true));
        assert_ne!(stub_fingerprint(jar, true), stub_fingerprint(jar, false));
    }
}
