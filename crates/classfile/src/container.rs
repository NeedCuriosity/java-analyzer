//! class 文件的来源：jar、JDK 的 `ct.sym`、JDK 的 `lib/modules`（jimage）。

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

use rustc_hash::FxHashMap;

use crate::{jimage::JImage, reader::parse_module_info, ModuleDescriptor};

pub trait ClassContainer: Send + Sync {
    /// 用于日志和界面展示的名字。
    fn display_name(&self) -> String;
    /// 容器对应的磁盘文件，用于计算缓存键。
    fn path(&self) -> &Path;
    /// 影响内容的额外参数（例如 ct.sym 的目标版本），参与缓存键。
    fn variant(&self) -> String {
        String::new()
    }
    /// 所有类的二进制名（`java/util/Map$Entry`）。
    fn class_names(&self) -> Vec<String>;
    fn read_class(&self, binary_name: &str) -> Option<Vec<u8>>;
    /// 容器里的模块声明：JDK 镜像里的各个模块、模块化 jar 的 `module-info.class`，
    /// 普通 jar 则是一个导出全部包的自动模块。
    fn modules(&self) -> Vec<ModuleDescriptor> {
        Vec::new()
    }
    /// 读取容器里的一个非 class 资源（如 `META-INF/spring-configuration-metadata.json`）。
    /// 只有 jar 有这类资源。
    fn read_resource(&self, _entry: &str) -> Option<Vec<u8>> {
        None
    }
}

fn parse_module(bytes: &[u8]) -> Option<ModuleDescriptor> {
    parse_module_info(bytes)
        .map_err(|err| tracing::debug!("解析 module-info 失败: {err}"))
        .ok()
}

/// 普通 jar 作为自动模块时的名字：清单的 `Automatic-Module-Name`，否则按文件名推出
/// （去掉 `.jar` 和 `-版本号` 后缀，非字母数字的字符换成 `.`）。
pub(crate) fn automatic_module_name(file_name: &str, manifest: Option<&str>) -> Option<String> {
    if let Some(manifest) = manifest {
        for line in manifest.lines() {
            if let Some(value) = line.strip_prefix("Automatic-Module-Name:") {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
    }
    let stem = file_name.strip_suffix(".jar").unwrap_or(file_name);
    let bytes = stem.as_bytes();
    let version_start = (0..bytes.len().saturating_sub(1))
        .find(|&i| bytes[i] == b'-' && bytes[i + 1].is_ascii_digit())
        .unwrap_or(bytes.len());
    let mut name = String::new();
    for c in stem[..version_start].chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else if !name.ends_with('.') {
            name.push('.');
        }
    }
    let name = name.trim_matches('.').to_owned();
    (!name.is_empty()).then_some(name)
}

fn is_indexable_class(binary_name: &str) -> bool {
    !binary_name.ends_with("module-info") && !binary_name.ends_with("package-info")
}

// ---------------------------------------------------------------------------
// jar
// ---------------------------------------------------------------------------

pub struct JarContainer {
    path: PathBuf,
    archive: Mutex<Option<zip::ZipArchive<File>>>,
}

impl JarContainer {
    pub fn new(path: PathBuf) -> JarContainer {
        JarContainer {
            path,
            archive: Mutex::new(None),
        }
    }

    fn with_archive<T>(&self, f: impl FnOnce(&mut zip::ZipArchive<File>) -> T) -> Option<T> {
        let mut guard = self.archive.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            let file = File::open(&self.path).ok()?;
            match zip::ZipArchive::new(file) {
                Ok(archive) => *guard = Some(archive),
                Err(err) => {
                    tracing::warn!("无法打开 {}: {err}", self.path.display());
                    return None;
                }
            }
        }
        guard.as_mut().map(f)
    }
}

impl ClassContainer for JarContainer {
    fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn class_names(&self) -> Vec<String> {
        self.with_archive(|archive| {
            archive
                .file_names()
                // 多版本 jar 的 META-INF/versions/N/ 下是同名类的覆盖版本，这里只取基础版本
                .filter(|name| !name.starts_with("META-INF/"))
                .filter_map(|name| name.strip_suffix(".class"))
                .filter(|name| is_indexable_class(name))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
    }

    fn read_class(&self, binary_name: &str) -> Option<Vec<u8>> {
        self.read_entry(&format!("{binary_name}.class"))
    }

    fn read_resource(&self, entry: &str) -> Option<Vec<u8>> {
        self.read_entry(entry)
    }

    fn modules(&self) -> Vec<ModuleDescriptor> {
        if let Some(module) = self
            .read_entry("module-info.class")
            .and_then(|bytes| parse_module(&bytes))
        {
            return vec![module];
        }
        let manifest = self
            .read_entry("META-INF/MANIFEST.MF")
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        let Some(name) = automatic_module_name(&self.display_name(), manifest.as_deref()) else {
            return Vec::new();
        };
        let mut packages: Vec<String> = self
            .class_names()
            .iter()
            .map(|class| crate::package_of(class).to_owned())
            .filter(|package| !package.is_empty())
            .collect();
        packages.sort();
        packages.dedup();
        vec![ModuleDescriptor {
            name,
            exports: packages.into_iter().map(|p| (p, Vec::new())).collect(),
            automatic: true,
            ..ModuleDescriptor::default()
        }]
    }
}

impl JarContainer {
    fn read_entry(&self, entry: &str) -> Option<Vec<u8>> {
        self.with_archive(|archive| {
            let mut file = archive.by_name(entry).ok()?;
            let mut buf = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut buf).ok()?;
            Some(buf)
        })
        .flatten()
    }
}

// ---------------------------------------------------------------------------
// ct.sym
// ---------------------------------------------------------------------------

/// JDK 的 `lib/ct.sym`：一个 zip，按"发行版字符集合"分目录存放各历史版本的 API 签名（`.sig`，
/// 即不含方法体的 class 文件）。目录名里的每个字符代表一个版本：`8`、`9`、`A`(10)、`B`(11) …
///
/// 注意它只包含**早于**当前 JDK 的版本；当前版本自身的类在 `lib/modules` 里。
pub struct CtSymContainer {
    path: PathBuf,
    release: u32,
    archive: Mutex<Option<zip::ZipArchive<File>>>,
    /// 二进制名 → zip 条目名
    entries: Mutex<Option<FxHashMap<String, String>>>,
    /// 这个版本的各模块 `module-info.sig` 的条目名
    module_entries: Mutex<Vec<String>>,
}

impl CtSymContainer {
    pub fn new(path: PathBuf, release: u32) -> CtSymContainer {
        CtSymContainer {
            path,
            release,
            archive: Mutex::new(None),
            entries: Mutex::new(None),
            module_entries: Mutex::new(Vec::new()),
        }
    }

    fn release_char(release: u32) -> Option<char> {
        std::char::from_digit(release, 36).map(|c| c.to_ascii_uppercase())
    }

    /// 该 ct.sym 是否包含给定版本。
    pub fn supports_release(path: &Path, release: u32) -> bool {
        let container = CtSymContainer::new(path.to_path_buf(), release);
        container
            .with_entries(|entries| !entries.is_empty())
            .unwrap_or(false)
    }

    fn with_archive<T>(&self, f: impl FnOnce(&mut zip::ZipArchive<File>) -> T) -> Option<T> {
        let mut guard = self.archive.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            let file = File::open(&self.path).ok()?;
            *guard = Some(zip::ZipArchive::new(file).ok()?);
        }
        guard.as_mut().map(f)
    }

    fn with_entries<T>(&self, f: impl FnOnce(&FxHashMap<String, String>) -> T) -> Option<T> {
        let mut guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            let release_char = Self::release_char(self.release)?;
            let mut modules = Vec::new();
            let map = self.with_archive(|archive| {
                let mut map = FxHashMap::default();
                for name in archive.file_names() {
                    let Some(stem) = name.strip_suffix(".sig") else {
                        continue;
                    };
                    let Some((releases, rest)) = stem.split_once('/') else {
                        continue;
                    };
                    if !releases.contains(release_char) || releases.contains('-') {
                        continue;
                    }
                    // 较新的布局多一层模块目录（模块名总是带点，包目录不可能带点）
                    let binary_name = match rest.split_once('/') {
                        Some((first, tail)) if first.contains('.') => tail,
                        _ => rest,
                    };
                    if is_indexable_class(binary_name) {
                        map.insert(binary_name.to_owned(), name.to_owned());
                    } else if binary_name == "module-info" {
                        modules.push(name.to_owned());
                    }
                }
                map
            })?;
            *self
                .module_entries
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = modules;
            *guard = Some(map);
        }
        guard.as_ref().map(f)
    }
}

impl ClassContainer for CtSymContainer {
    fn display_name(&self) -> String {
        format!("JDK API (release {})", self.release)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn variant(&self) -> String {
        format!("release-{}", self.release)
    }

    fn class_names(&self) -> Vec<String> {
        self.with_entries(|entries| entries.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn modules(&self) -> Vec<ModuleDescriptor> {
        self.with_entries(|_| ());
        let entries = self
            .module_entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        entries
            .iter()
            .filter_map(|entry| self.read_zip_entry(entry))
            .filter_map(|bytes| parse_module(&bytes))
            .collect()
    }

    fn read_class(&self, binary_name: &str) -> Option<Vec<u8>> {
        let entry = self.with_entries(|entries| entries.get(binary_name).cloned())??;
        self.read_zip_entry(&entry)
    }
}

impl CtSymContainer {
    fn read_zip_entry(&self, entry: &str) -> Option<Vec<u8>> {
        self.with_archive(|archive| {
            let mut file = archive.by_name(entry).ok()?;
            let mut buf = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut buf).ok()?;
            Some(buf)
        })
        .flatten()
    }
}

// ---------------------------------------------------------------------------
// jimage
// ---------------------------------------------------------------------------

/// JDK 9+ 的运行时镜像 `lib/modules`。
pub struct JImageContainer {
    path: PathBuf,
    image: JImage,
}

impl JImageContainer {
    pub fn open(path: PathBuf) -> Option<JImageContainer> {
        let image = match JImage::open(&path) {
            Ok(image) => image,
            Err(err) => {
                tracing::warn!("无法读取 {}: {err}", path.display());
                return None;
            }
        };
        Some(JImageContainer { path, image })
    }
}

impl ClassContainer for JImageContainer {
    fn display_name(&self) -> String {
        "JDK runtime image".to_owned()
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn class_names(&self) -> Vec<String> {
        self.image
            .class_names()
            .filter(|name| is_indexable_class(name))
            .map(str::to_owned)
            .collect()
    }

    fn read_class(&self, binary_name: &str) -> Option<Vec<u8>> {
        self.image.read_class(binary_name)
    }

    fn modules(&self) -> Vec<ModuleDescriptor> {
        self.image
            .module_infos()
            .iter()
            .filter_map(|bytes| parse_module(bytes))
            .collect()
    }
}
