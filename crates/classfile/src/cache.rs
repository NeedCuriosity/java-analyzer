//! 容器索引的磁盘缓存。
//!
//! 缓存键 = 容器路径 + 文件大小 + 修改时间 + 变体 + 格式版本。任何一项变化都会得到不同的
//! 缓存文件名，旧文件自然失效。读缓存失败（损坏、版本不符）时静默回退到重新索引。
//! 目前是整体反序列化；以后可以改为 `rkyv::access` 零拷贝访问 mmap。

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    time::UNIX_EPOCH,
};

use rkyv::{rancor, Archive, Deserialize, Serialize};
use smol_str::SmolStr;

use crate::{index_container, ClassContainer, ClassSummary};

const FORMAT_VERSION: u32 = 1;

#[derive(Archive, Serialize, Deserialize)]
struct CachedSummary {
    name: String,
    access: u16,
    super_name: Option<String>,
    interfaces: Vec<String>,
}

#[derive(Archive, Serialize, Deserialize)]
struct CachedIndex {
    version: u32,
    classes: Vec<CachedSummary>,
}

pub(crate) fn load_or_index(container: &dyn ClassContainer) -> Vec<ClassSummary> {
    let cache_path = cache_path(container);
    if let Some(path) = &cache_path {
        if let Some(summaries) = read_cache(path) {
            return summaries;
        }
    }
    let start = std::time::Instant::now();
    let summaries = index_container(container);
    tracing::info!(
        "已索引 {}：{} 个类，耗时 {:?}",
        container.display_name(),
        summaries.len(),
        start.elapsed()
    );
    if let Some(path) = &cache_path {
        if let Err(err) = write_cache(path, &summaries) {
            tracing::debug!("写索引缓存 {} 失败: {err}", path.display());
        }
    }
    summaries
}

fn cache_path(container: &dyn ClassContainer) -> Option<PathBuf> {
    let path = container.path();
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;

    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    mtime.as_nanos().hash(&mut hasher);
    container.variant().hash(&mut hasher);
    FORMAT_VERSION.hash(&mut hasher);
    let key = hasher.finish();

    let dir = dirs::cache_dir()?.join("java-analyzer").join("index");
    Some(dir.join(format!("{key:016x}.idx")))
}

fn read_cache(path: &std::path::Path) -> Option<Vec<ClassSummary>> {
    let bytes = fs::read(path).ok()?;
    // rkyv 要求缓冲区按 16 字节对齐，`Vec<u8>` 不保证
    let mut aligned = rkyv::util::AlignedVec::<16>::with_capacity(bytes.len());
    aligned.extend_from_slice(&bytes);
    let index = rkyv::from_bytes::<CachedIndex, rancor::Error>(&aligned).ok()?;
    if index.version != FORMAT_VERSION {
        return None;
    }
    Some(
        index
            .classes
            .into_iter()
            .map(|c| ClassSummary {
                name: SmolStr::new(c.name),
                access: c.access,
                super_name: c.super_name.map(SmolStr::new),
                interfaces: c.interfaces.into_iter().map(SmolStr::new).collect(),
            })
            .collect(),
    )
}

fn write_cache(path: &std::path::Path, summaries: &[ClassSummary]) -> std::io::Result<()> {
    let index = CachedIndex {
        version: FORMAT_VERSION,
        classes: summaries
            .iter()
            .map(|s| CachedSummary {
                name: s.name.to_string(),
                access: s.access,
                super_name: s.super_name.as_ref().map(|n| n.to_string()),
                interfaces: s.interfaces.iter().map(|n| n.to_string()).collect(),
            })
            .collect(),
    };
    let bytes = rkyv::to_bytes::<rancor::Error>(&index)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // 先写临时文件再改名，避免并发启动的两个进程读到写了一半的缓存
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, path)
}
