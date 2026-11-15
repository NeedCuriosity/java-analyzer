//! JDK 的发现与类来源的选择。

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use base_db::JdkInfo;
use classfile::{ClassContainer, CtSymContainer, JImageContainer, JarContainer};

/// 读 `$JDK/release` 里的 `JAVA_VERSION="21.0.2"`，得到特性版本号。
fn read_version(home: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(home.join("release")).ok()?;
    let line = text.lines().find(|l| l.starts_with("JAVA_VERSION="))?;
    let value = line
        .trim_start_matches("JAVA_VERSION=")
        .trim()
        .trim_matches('"');
    parse_feature_version(value)
}

/// `1.8.0_392` → 8，`17.0.9` → 17，`21` → 21。
pub fn parse_feature_version(version: &str) -> Option<u32> {
    let mut parts = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty());
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

fn inspect(home: &Path) -> Option<JdkInfo> {
    let has_runtime = home.join("lib/modules").is_file()
        || home.join("jre/lib/rt.jar").is_file()
        || home.join("lib/rt.jar").is_file();
    if !has_runtime {
        return None;
    }
    let version = read_version(home)?;
    let src_zip = [home.join("lib/src.zip"), home.join("src.zip")]
        .into_iter()
        .find(|p| p.is_file());
    Some(JdkInfo {
        home: home.to_path_buf(),
        version,
        src_zip,
    })
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// 按优先级查找 JDK：显式配置 → `JAVA_HOME` → 系统里装的最高版本。
pub fn discover_jdk(configured: Option<&Path>) -> Option<JdkInfo> {
    if let Some(home) = configured {
        match inspect(home) {
            Some(jdk) => return Some(jdk),
            None => tracing::warn!("配置的 JDK 路径无效: {}", home.display()),
        }
    }
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        if let Some(jdk) = inspect(Path::new(&home)) {
            return Some(jdk);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        if let Ok(output) = Command::new("/usr/libexec/java_home").output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if !path.is_empty() {
                    candidates.push(PathBuf::from(path));
                }
            }
        }
        for dir in subdirs(Path::new("/Library/Java/JavaVirtualMachines")) {
            candidates.push(dir.join("Contents/Home"));
        }
    }
    candidates.extend(subdirs(Path::new("/usr/lib/jvm")));
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".sdkman/candidates/java/current"));
        for dir in [
            ".sdkman/candidates/java",
            ".jdks",
            ".gradle/jdks",
            ".local/share/mise/installs/java",
            ".asdf/installs/java",
        ] {
            for sub in subdirs(&home.join(dir)) {
                // 有些管理器在 macOS 上多套一层 Contents/Home
                candidates.push(sub.join("Contents/Home"));
                candidates.push(sub);
            }
        }
    }

    candidates
        .iter()
        .filter_map(|home| inspect(home))
        .max_by_key(|jdk| jdk.version)
}

/// 为目标语言级别选择 JDK 类的来源。
///
/// - 目标级别低于 JDK 自身版本且 `ct.sym` 里有该版本：用 `ct.sym`（等价于 `javac --release N`）。
/// - 否则用运行时镜像 `lib/modules`；JDK 8 用 `rt.jar`。
pub fn jdk_containers(jdk: &JdkInfo, target_release: Option<u32>) -> Vec<Arc<dyn ClassContainer>> {
    let ct_sym = jdk.home.join("lib/ct.sym");
    if let Some(release) = target_release {
        if release < jdk.version
            && ct_sym.is_file()
            && CtSymContainer::supports_release(&ct_sym, release)
        {
            return vec![Arc::new(CtSymContainer::new(ct_sym, release))];
        }
    }

    let modules = jdk.home.join("lib/modules");
    if modules.is_file() {
        if let Some(image) = JImageContainer::open(modules) {
            return vec![Arc::new(image)];
        }
        // 镜像读不了（例如被 jlink 压缩过）：退而求其次，用 ct.sym 里最新的历史版本
        if ct_sym.is_file() {
            for release in (8..jdk.version).rev() {
                if CtSymContainer::supports_release(&ct_sym, release) {
                    tracing::warn!("无法读取 JDK 运行时镜像，改用 ct.sym 的 release {release}");
                    return vec![Arc::new(CtSymContainer::new(ct_sym, release))];
                }
            }
        }
    }

    for rt in [jdk.home.join("jre/lib/rt.jar"), jdk.home.join("lib/rt.jar")] {
        if rt.is_file() {
            return vec![Arc::new(JarContainer::new(rt))];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_versions() {
        assert_eq!(parse_feature_version("1.8.0_392"), Some(8));
        assert_eq!(parse_feature_version("17.0.9"), Some(17));
        assert_eq!(parse_feature_version("21"), Some(21));
        assert_eq!(parse_feature_version("25-ea"), Some(25));
        assert_eq!(parse_feature_version(""), None);
    }
}
