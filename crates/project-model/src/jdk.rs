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
    select_jdk(configured, None)
}

/// 按优先级查找 JDK，`wanted` 是工程要求的版本（语言级别或 Maven toolchain 里写的版本）：
///
/// 1. 显式配置的路径，有效就用它，不管版本；
/// 2. 有 `wanted` 时，在 `JAVA_HOME` 和系统里装的 JDK 里取版本恰好等于它的，没有就取
///    不低于它的最低版本；
/// 3. 都不满足（或者没有 `wanted`）时：`JAVA_HOME` → 系统里装的最高版本。
pub fn select_jdk(configured: Option<&Path>, wanted: Option<u32>) -> Option<JdkInfo> {
    if let Some(home) = configured {
        match inspect(home) {
            Some(jdk) => return Some(jdk),
            None => tracing::warn!("配置的 JDK 路径无效: {}", home.display()),
        }
    }
    let java_home = std::env::var_os("JAVA_HOME").and_then(|home| inspect(Path::new(&home)));
    // `JAVA_HOME` 就合适时不用再扫描系统里装的 JDK
    if let Some(jdk) = &java_home {
        if wanted.is_none() || wanted == Some(jdk.version) {
            return java_home;
        }
    }
    let installed: Vec<JdkInfo> = candidate_homes()
        .iter()
        .filter_map(|home| inspect(home))
        .collect();
    let chosen = pick_jdk(java_home, installed, wanted);
    if let (Some(jdk), Some(wanted)) = (&chosen, wanted) {
        tracing::info!(
            "工程要求 Java {wanted}，选用 JDK {}: {}",
            jdk.version,
            jdk.home.display()
        );
    }
    chosen
}

/// 在找到的 JDK 里挑一个：有 `wanted` 时取不低于它的最低版本（恰好相等的自然排在最前），
/// 版本相同时 `JAVA_HOME` 优先、完整的 JDK 优先于 JRE；没有合适的就回到 `JAVA_HOME` →
/// 最高版本。
fn pick_jdk(
    java_home: Option<JdkInfo>,
    installed: Vec<JdkInfo>,
    wanted: Option<u32>,
) -> Option<JdkInfo> {
    if let Some(wanted) = wanted {
        let fitting = java_home
            .iter()
            .chain(&installed)
            .filter(|jdk| jdk.version >= wanted)
            .min_by_key(|jdk| (jdk.version, !has_javac(&jdk.home)));
        if let Some(jdk) = fitting {
            return Some(jdk.clone());
        }
    }
    java_home.or_else(|| {
        installed
            .into_iter()
            .max_by_key(|jdk| (jdk.version, has_javac(&jdk.home)))
    })
}

fn has_javac(home: &Path) -> bool {
    home.join("bin/javac").is_file() || home.join("bin/javac.exe").is_file()
}

/// 常见的 JDK 安装位置（不含 `JAVA_HOME`）。
fn candidate_homes() -> Vec<PathBuf> {
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
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            let Some(program_files) = std::env::var_os(var) else {
                continue;
            };
            for vendor in [
                "Java",
                "Eclipse Adoptium",
                "Eclipse Foundation",
                "AdoptOpenJDK",
                "Microsoft",
                "Zulu",
                "Amazon Corretto",
                "BellSoft",
                "Semeru",
            ] {
                candidates.extend(subdirs(&Path::new(&program_files).join(vendor)));
            }
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

    /// 假的 JDK 目录：`release` 文件加一个空的 `lib/modules`（JDK 8 的写法放 `jre/lib/rt.jar`）。
    fn fake_jdk(root: &Path, name: &str, java_version: &str, javac: bool) -> JdkInfo {
        let home = root.join(name);
        std::fs::create_dir_all(home.join("lib")).unwrap();
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(
            home.join("release"),
            format!("IMPLEMENTOR=\"Test\"\nJAVA_VERSION=\"{java_version}\"\n"),
        )
        .unwrap();
        if java_version.starts_with("1.") {
            std::fs::create_dir_all(home.join("jre/lib")).unwrap();
            std::fs::write(home.join("jre/lib/rt.jar"), b"").unwrap();
        } else {
            std::fs::write(home.join("lib/modules"), b"").unwrap();
        }
        if javac {
            std::fs::write(home.join("bin/javac"), b"").unwrap();
        }
        inspect(&home).unwrap()
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ja-jdks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn picks_jdk_matching_the_requested_version() {
        let root = temp_root("pick");
        let jdk8 = fake_jdk(&root, "jdk8", "1.8.0_151", true);
        let jdk17 = fake_jdk(&root, "jdk17", "17.0.20", true);
        let jdk21 = fake_jdk(&root, "jdk21", "21.0.11", true);
        let jdk25 = fake_jdk(&root, "jdk25", "25", true);
        assert_eq!((jdk8.version, jdk17.version, jdk25.version), (8, 17, 25));
        let installed = vec![jdk25.clone(), jdk21.clone(), jdk8.clone(), jdk17.clone()];
        let pick = |java_home: Option<&JdkInfo>, wanted| {
            pick_jdk(java_home.cloned(), installed.clone(), wanted).map(|jdk| jdk.home)
        };

        // 恰好相等的优先，即使 JAVA_HOME 是别的版本
        assert_eq!(pick(Some(&jdk25), Some(8)), Some(jdk8.home.clone()));
        assert_eq!(pick(Some(&jdk25), Some(17)), Some(jdk17.home.clone()));
        // 没有恰好相等的：不低于它的最低版本
        assert_eq!(pick(Some(&jdk25), Some(11)), Some(jdk17.home.clone()));
        assert_eq!(pick(Some(&jdk25), Some(6)), Some(jdk8.home.clone()));
        // 都太旧：照旧，JAVA_HOME → 最高版本
        assert_eq!(pick(Some(&jdk17), Some(26)), Some(jdk17.home.clone()));
        assert_eq!(pick(None, Some(26)), Some(jdk25.home.clone()));
        // 没有要求的版本：照旧
        assert_eq!(pick(Some(&jdk17), None), Some(jdk17.home.clone()));
        assert_eq!(pick(None, None), Some(jdk25.home.clone()));
        assert_eq!(pick_jdk(None, Vec::new(), Some(8)), None);

        // 版本相同：JAVA_HOME 优先，完整的 JDK 优先于没有 javac 的 JRE
        let other17 = fake_jdk(&root, "other17", "17.0.2", true);
        assert_eq!(pick(Some(&other17), Some(17)), Some(other17.home.clone()));
        let jre8 = fake_jdk(&root, "jre8", "1.8.0_151", false);
        let with_jre = vec![jre8.clone(), jdk8.clone(), jdk17.clone()];
        assert_eq!(
            pick_jdk(None, with_jre.clone(), Some(8)).map(|jdk| jdk.home),
            Some(jdk8.home.clone())
        );
        assert_eq!(
            pick_jdk(None, with_jre, Some(7)).map(|jdk| jdk.home),
            Some(jdk8.home.clone())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn configured_jdk_wins_over_the_requested_version() {
        let root = temp_root("configured");
        let jdk25 = fake_jdk(&root, "jdk25", "25.0.1", true);
        let chosen = select_jdk(Some(&jdk25.home), Some(8)).unwrap();
        assert_eq!(chosen.home, jdk25.home);
        assert_eq!(chosen.version, 25);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn jdk6_version_from_rt_jar() {
        use std::io::Write;
        let root = temp_root("jdk6");
        let lib = root.join("jdk6/jre/lib");
        std::fs::create_dir_all(&lib).unwrap();
        let mut jar = zip::ZipWriter::new(File::create(lib.join("rt.jar")).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        jar.start_file("META-INF/MANIFEST.MF", options).unwrap();
        jar.write_all(b"Manifest-Version: 1.0\r\nSpecification-Version: 1.6\r\n")
            .unwrap();
        jar.finish().unwrap();
        let jdk6 = inspect(&root.join("jdk6")).unwrap();
        assert_eq!(jdk6.version, 6);
        let jdk8 = fake_jdk(&root, "jdk8", "1.8.0_392", true);
        assert_eq!(
            pick_jdk(None, vec![jdk8, jdk6.clone()], Some(6)).map(|jdk| jdk.home),
            Some(jdk6.home)
        );
        let _ = std::fs::remove_dir_all(&root);
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
            .map(|p| {
                let relative = p.strip_prefix(&lib).unwrap();
                relative.to_string_lossy().replace('\\', "/")
            })
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
