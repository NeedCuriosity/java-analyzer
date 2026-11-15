//! 从磁盘批量加载源码文件。

use std::path::{Path, PathBuf};

use rayon::prelude::*;

pub struct LoadedFile {
    pub path: PathBuf,
    pub text: String,
}

/// 读文件；非法 UTF-8 用替换字符兜底（旧工程里偶尔有 Latin-1 / GBK 源码）。
pub fn read_file_lossy(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    })
}

/// 递归收集目录下所有 `.java` 文件并并行读入。
/// 跳过隐藏目录；不跟随符号链接，避免环。
pub fn load_java_files(roots: &[PathBuf]) -> Vec<LoadedFile> {
    let mut paths = Vec::new();
    for root in roots {
        collect(root, &mut paths);
    }
    paths.sort();
    paths.dedup();
    paths
        .into_par_iter()
        .filter_map(|path| {
            let text = read_file_lossy(&path)?;
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
