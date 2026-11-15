//! 读取源码包（JDK 的 `src.zip`、依赖的 sources jar）里的单个源文件。
//!
//! 压缩包的目录只读一次，之后按需解压单个文件；源码包在一次运行期间视为不变。

use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

struct Archive {
    zip: Mutex<zip::ZipArchive<File>>,
    /// `java/util/List.java` → 压缩包里的条目名。JDK 9+ 的 `src.zip` 多一层模块目录
    /// （`java.base/java/util/List.java`），去掉第一段之后的路径也登记进来。
    index: HashMap<String, String>,
}

fn open(path: &Path) -> Option<Archive> {
    let zip = zip::ZipArchive::new(File::open(path).ok()?).ok()?;
    let mut index = HashMap::new();
    for name in zip.file_names() {
        if !name.ends_with(".java") {
            continue;
        }
        if let Some((_, rest)) = name.split_once('/') {
            index
                .entry(rest.to_owned())
                .or_insert_with(|| name.to_owned());
        }
    }
    // 不带模块目录的布局优先级更高：整条路径直接命中
    for name in zip.file_names() {
        if name.ends_with(".java") {
            index.insert(name.to_owned(), name.to_owned());
        }
    }
    Some(Archive {
        zip: Mutex::new(zip),
        index,
    })
}

fn archive(path: &Path) -> Option<Arc<Archive>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<Archive>>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache
        .entry(path.to_path_buf())
        .or_insert_with(|| open(path).map(Arc::new))
        .clone()
}

/// 源码包 `archive_path` 里 `relative`（形如 `java/util/List.java`）的文本。
pub fn read_source(archive_path: &Path, relative: &str) -> Option<String> {
    let archive = archive(archive_path)?;
    let entry = archive.index.get(relative)?;
    let mut zip = archive.zip.lock().ok()?;
    let mut file = zip.by_name(entry).ok()?;
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}
