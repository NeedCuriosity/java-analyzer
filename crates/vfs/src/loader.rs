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

/// 作为资源文件交给分析器的非 Java 文件：Spring 配置、配置元数据和 `lombok.config`。
pub fn is_resource_file(path: &Path) -> bool {
    is_spring_resource(path) || is_lombok_config(path)
}

/// 各模块根以及它们往上的目录里的 `lombok.config`（Lombok 从源文件所在目录逐级往上找；
/// 遇到 `.git` 所在的目录为止）。
pub fn load_lombok_configs(module_roots: &[PathBuf]) -> Vec<LoadedFile> {
    let mut paths = Vec::new();
    for root in module_roots {
        for dir in root.ancestors().take(8) {
            let candidate = dir.join("lombok.config");
            if candidate.is_file() {
                paths.push(candidate);
            }
            if dir.join(".git").exists() {
                break;
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
