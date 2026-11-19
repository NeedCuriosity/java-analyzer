//! JDK 的发现与类来源的选择。

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use base_db::JdkInfo;
use classfile::{ClassContainer, CtSymContainer, JImageContainer, JarContainer};

/// JDK 的特性版本号：读 `$JDK/release` 里的 `JAVA_VERSION="21.0.2"`；JDK 6 还没有这个文件，
/// 改读 `rt.jar` 清单里的 `Specification-Version: 1.6`。
fn read_version(home: &Path) -> Option<u32> {
    let Ok(text) = std::fs::read_to_string(home.join("release")) else {
        return rt_jar_version(home);
    };
    let line = text.lines().find(|l| l.starts_with("JAVA_VERSION="))?;
    let value = line
        .trim_start_matches("JAVA_VERSION=")
        .trim()
        .trim_matches('"');
    parse_feature_version(value)
}

fn rt_jar_version(home: &Path) -> Option<u32> {
    let rt = [home.join("jre/lib/rt.jar"), home.join("lib/rt.jar")]
        .into_iter()
        .find(|p| p.is_file())?;
    let mut archive = zip::ZipArchive::new(File::open(rt).ok()?).ok()?;
    let mut manifest = String::new();
    archive
        .by_name("META-INF/MANIFEST.MF")
        .ok()?
        .read_to_string(&mut manifest)
        .ok()?;
    manifest_spec_version(&manifest)
}

fn manifest_spec_version(manifest: &str) -> Option<u32> {
    let value = manifest
        .lines()
        .find_map(|l| l.strip_prefix("Specification-Version:"))?;
    parse_feature_version(value.trim())
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
/// - 否则用运行时镜像 `lib/modules`；JDK 8 用 `rt.jar` 及同目录、`ext/` 下的 jar。
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

    for lib in [jdk.home.join("jre/lib"), jdk.home.join("lib")] {
        if lib.join("rt.jar").is_file() {
            return jdk8_jars(&lib)
                .into_iter()
                .map(|jar| Arc::new(JarContainer::jdk(jar)) as Arc<dyn ClassContainer>)
                .collect();
        }
    }
    Vec::new()
}

/// JDK 8 的启动类路径和扩展目录：`rt.jar` 在前，然后是 `lib/` 下其它 jar（`jce.jar`、
/// `jsse.jar` 等），最后是 `lib/ext/` 下的 jar，和 javac 默认的查找范围一致。
fn jdk8_jars(lib: &Path) -> Vec<PathBuf> {
    let jars_in = |dir: &Path| {
        let mut jars: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "jar") && p.is_file())
                    .collect()
            })
            .unwrap_or_default();
        jars.sort();
        jars
    };
    let rt = lib.join("rt.jar");
    let mut jars = vec![rt.clone()];
    jars.extend(jars_in(lib).into_iter().filter(|jar| *jar != rt));
    jars.extend(jars_in(&lib.join("ext")));
    jars
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

    #[test]
    fn version_from_rt_jar_manifest() {
        let manifest = "Manifest-Version: 1.0\r\nImplementation-Version: 1.6.0-119\r\n\
                        Specification-Version: 1.6\r\n\r\nName: javax/swing/JList.class\r\n";
        assert_eq!(manifest_spec_version(manifest), Some(6));
        assert_eq!(manifest_spec_version("Manifest-Version: 1.0\n"), None);
    }

    #[test]
    fn jdk8_boot_and_extension_jars() {
        let lib = std::env::temp_dir().join(format!("ja-jdk8-{}/jre/lib", std::process::id()));
        std::fs::create_dir_all(lib.join("ext")).unwrap();
        for jar in [
            "rt.jar",
            "charsets.jar",
            "jce.jar",
            "jsse.jar",
            "ext/sunjce_provider.jar",
        ] {
            std::fs::write(lib.join(jar), b"").unwrap();
        }
        std::fs::write(lib.join("blacklisted.certs"), b"").unwrap();
        let names: Vec<String> = jdk8_jars(&lib)
            .iter()
            .map(|p| p.strip_prefix(&lib).unwrap().display().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "rt.jar",
                "charsets.jar",
                "jce.jar",
                "jsse.jar",
                "ext/sunjce_provider.jar"
            ]
        );
        std::fs::remove_dir_all(lib.parent().unwrap().parent().unwrap()).unwrap();
    }
}
