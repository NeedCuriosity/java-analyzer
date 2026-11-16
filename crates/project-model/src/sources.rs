//! 按需下载依赖库的 sources jar。
//!
//! 坐标从 jar 的路径推出来（Maven 本地仓库和 Gradle 缓存的目录结构都认），
//! 用 `mvn dependency:get` 下载到 Maven 本地仓库：这样沿用用户 `settings.xml` 里的镜像和认证。

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// 一个依赖库的 Maven 坐标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coordinates {
    pub group: String,
    pub artifact: String,
    pub version: String,
}

/// 从 jar 的路径推出坐标：
/// - Maven 本地仓库：`<repo>/com/acme/lib/1.0/lib-1.0.jar`；
/// - Gradle 缓存：`files-2.1/com.acme/lib/1.0/<hash>/lib-1.0.jar`。
///
/// 带 classifier 的 jar（`lib-1.0-tests.jar`）和对不上的路径返回 `None`。
pub fn coordinates_of(jar: &Path, maven_repo: &Path) -> Option<Coordinates> {
    let file = jar.file_name()?.to_str()?;
    let stem = file.strip_suffix(".jar")?;
    let components: Vec<&str> = jar
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    // Gradle：…/files-2.1/<group>/<name>/<version>/<hash>/<file>
    if let Some(pos) = components.iter().rposition(|c| *c == "files-2.1") {
        if let [group, artifact, version, _hash, _file] = &components[pos + 1..] {
            if stem == format!("{artifact}-{version}") {
                return Some(Coordinates {
                    group: group.to_string(),
                    artifact: artifact.to_string(),
                    version: version.to_string(),
                });
            }
        }
        return None;
    }
    // Maven：<repo>/<group path>/<artifact>/<version>/<artifact>-<version>.jar
    let relative = jar.strip_prefix(maven_repo).ok()?;
    let parts: Vec<&str> = relative
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    let [group @ .., artifact, version, _file] = parts.as_slice() else {
        return None;
    };
    if group.is_empty() || stem != format!("{artifact}-{version}") {
        return None;
    }
    Some(Coordinates {
        group: group.join("."),
        artifact: artifact.to_string(),
        version: version.to_string(),
    })
}

/// Maven 本地仓库里这个坐标的 sources jar 应在的位置。
pub fn sources_path(coords: &Coordinates, maven_repo: &Path) -> PathBuf {
    let mut path = maven_repo.to_path_buf();
    for segment in coords.group.split('.') {
        path.push(segment);
    }
    path.join(&coords.artifact)
        .join(&coords.version)
        .join(format!(
            "{}-{}-sources.jar",
            coords.artifact, coords.version
        ))
}

/// Maven 本地仓库的位置（`~/.m2/settings.xml` 里的 `localRepository`，默认 `~/.m2/repository`）。
pub fn maven_repository() -> PathBuf {
    crate::maven::local_repository()
}

/// 下载 `jar` 对应的 sources jar，返回它的路径。已经下载过（本地仓库里有）时直接返回；
/// 推不出坐标或者仓库里没有源码包时返回 `None`。
///
/// 先用 `mvn dependency:get`（工程里有 Maven Wrapper 时用它）；`mvn` 不可用时用 `curl`
/// 从 `settings.xml` 里配置的镜像（没有时是 Maven Central）直接下载。
/// `project_root` 用来找 Maven Wrapper 和 `.mvn/` 下的配置。会阻塞到下载结束。
pub fn download_sources(jar: &Path, project_root: &Path) -> Option<PathBuf> {
    let repo = maven_repository();
    let coords = coordinates_of(jar, &repo)?;
    let target = sources_path(&coords, &repo);
    if target.is_file() {
        return Some(target);
    }
    if !download_with_maven(&coords, project_root) {
        download_with_curl(&coords, &target);
    }
    target.is_file().then_some(target)
}

fn download_with_maven(coords: &Coordinates, project_root: &Path) -> bool {
    let wrapper = project_root
        .ancestors()
        .map(|d| d.join(if cfg!(windows) { "mvnw.cmd" } else { "mvnw" }))
        .find(|p| p.is_file());
    let program = wrapper.unwrap_or_else(|| PathBuf::from("mvn"));
    Command::new(program)
        .current_dir(project_root)
        .arg("-q")
        .arg("-B")
        .arg("dependency:get")
        .arg(format!(
            "-Dartifact={}:{}:{}:jar:sources",
            coords.group, coords.artifact, coords.version
        ))
        .arg("-Dtransitive=false")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

const MAVEN_CENTRAL: &str = "https://repo.maven.apache.org/maven2";

/// `settings.xml` 里镜像了 `central`（或 `*`）的仓库地址；没有时是 Maven Central。
fn remote_repository() -> String {
    let settings = dirs::home_dir()
        .map(|home| home.join(".m2/settings.xml"))
        .and_then(|path| std::fs::read_to_string(path).ok());
    settings
        .and_then(|text| mirror_url(&text))
        .unwrap_or_else(|| MAVEN_CENTRAL.to_owned())
}

fn mirror_url(settings: &str) -> Option<String> {
    let root = crate::xml::parse(settings)?;
    let mirrors = root.child("mirrors")?;
    let url = mirrors.children_named("mirror").find_map(|mirror| {
        let of = mirror.child_text("mirrorOf")?;
        let covers_central = of
            .split(',')
            .map(str::trim)
            .any(|m| m == "*" || m == "central")
            && !of.contains("!central");
        covers_central.then(|| {
            mirror
                .child_text("url")
                .map(|u| u.trim().trim_end_matches('/').to_owned())
        })?
    });
    url
}

fn download_with_curl(coords: &Coordinates, target: &Path) -> bool {
    let Some(dir) = target.parent() else {
        return false;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let url = format!(
        "{}/{}/{}/{}/{}-{}-sources.jar",
        remote_repository(),
        coords.group.replace('.', "/"),
        coords.artifact,
        coords.version,
        coords.artifact,
        coords.version
    );
    // 先写到临时文件，下载完整了再改名：半截的文件不会被当成源码包
    let partial = target.with_extension("jar.part");
    let ok = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "--max-time", "300", "-o"])
        .arg(&partial)
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if ok && std::fs::rename(&partial, target).is_ok() {
        return true;
    }
    let _ = std::fs::remove_file(&partial);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_from_maven_and_gradle_paths() {
        let repo = Path::new("/home/u/.m2/repository");
        let maven = Path::new("/home/u/.m2/repository/org/apache/commons/commons-lang3/3.14.0/commons-lang3-3.14.0.jar");
        let coords = coordinates_of(maven, repo).unwrap();
        assert_eq!(
            coords,
            Coordinates {
                group: "org.apache.commons".into(),
                artifact: "commons-lang3".into(),
                version: "3.14.0".into(),
            }
        );
        assert_eq!(
            sources_path(&coords, repo),
            Path::new("/home/u/.m2/repository/org/apache/commons/commons-lang3/3.14.0/commons-lang3-3.14.0-sources.jar")
        );

        let gradle = Path::new("/home/u/.gradle/caches/modules-2/files-2.1/com.google.guava/guava/33.0.0-jre/0123abcd/guava-33.0.0-jre.jar");
        assert_eq!(
            coordinates_of(gradle, repo),
            Some(Coordinates {
                group: "com.google.guava".into(),
                artifact: "guava".into(),
                version: "33.0.0-jre".into(),
            })
        );

        assert_eq!(
            mirror_url("<settings><mirrors><mirror><mirrorOf>central</mirrorOf><url>https://maven.example.com/repo/</url></mirror></mirrors></settings>").as_deref(),
            Some("https://maven.example.com/repo")
        );
        assert_eq!(
            mirror_url("<settings><mirrors><mirror><mirrorOf>internal</mirrorOf><url>https://x</url></mirror></mirrors></settings>"),
            None
        );

        // classifier、仓库外的 jar：推不出
        let tests = Path::new("/home/u/.m2/repository/com/acme/lib/1.0/lib-1.0-tests.jar");
        assert_eq!(coordinates_of(tests, repo), None);
        assert_eq!(coordinates_of(Path::new("/opt/lib/x-1.0.jar"), repo), None);
    }
}
