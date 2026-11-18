//! Maven 的 `settings.xml`：本地仓库位置和镜像。
//!
//! 和 Maven 一样有两级配置：用户级 `~/.m2/settings.xml` 优先，其次是安装目录下的
//! `conf/settings.xml`（`MAVEN_HOME` / `M2_HOME`，或者 `PATH` 上 `mvn` 所在的安装）。

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::xml;

/// 编辑器配置里指定的本地仓库（`maven.localRepository`），优先于一切自动发现。
static LOCAL_REPOSITORY_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// 设置（或清除）编辑器配置里指定的本地仓库位置。
pub fn set_local_repository_override(path: Option<PathBuf>) {
    if let Ok(mut slot) = LOCAL_REPOSITORY_OVERRIDE.write() {
        *slot = path;
    }
}

/// Maven 本地仓库的位置。依次看：编辑器配置、`MAVEN_OPTS` 里的 `-Dmaven.repo.local`、
/// 用户级 `settings.xml` 的 `localRepository`、安装目录 `conf/settings.xml` 的
/// `localRepository`，都没有时是 `~/.m2/repository`。
pub fn local_repository() -> PathBuf {
    if let Some(path) = LOCAL_REPOSITORY_OVERRIDE
        .read()
        .ok()
        .and_then(|slot| slot.clone())
    {
        return path;
    }
    let home = dirs::home_dir().unwrap_or_default();
    let install = maven_installation();
    let from_opts = std::env::var("MAVEN_OPTS")
        .ok()
        .and_then(|opts| repo_local_from_opts(&opts));
    from_opts
        .or_else(|| {
            settings_files(&home, install.as_deref())
                .iter()
                .find_map(|path| {
                    let text = std::fs::read_to_string(path).ok()?;
                    local_repository_in(&text)
                })
        })
        .map(|raw| PathBuf::from(interpolate(&raw, &home, install.as_deref())))
        .unwrap_or_else(|| home.join(".m2/repository"))
}

/// 所有存在的 `settings.xml`，用户级在前。
pub(crate) fn existing_settings_files() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    settings_files(&home, maven_installation().as_deref())
}

fn settings_files(home: &Path, install: Option<&Path>) -> Vec<PathBuf> {
    let mut files = vec![home.join(".m2/settings.xml")];
    if let Some(install) = install {
        files.push(install.join("conf/settings.xml"));
    }
    files.retain(|f| f.is_file());
    files
}

fn local_repository_in(settings: &str) -> Option<String> {
    let root = xml::parse(settings)?;
    let text = root.child_text("localRepository")?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// `MAVEN_OPTS` 里的 `-Dmaven.repo.local=...`。
fn repo_local_from_opts(opts: &str) -> Option<String> {
    opts.split_whitespace().find_map(|arg| {
        let value = arg.strip_prefix("-Dmaven.repo.local=")?;
        let value = value.trim_matches('"');
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// 展开 `${user.home}`、`${maven.home}` 和 `${env.NAME}`；不认识的占位符原样保留。
fn interpolate(raw: &str, home: &Path, install: Option<&Path>) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        let value = match name {
            "user.home" => Some(home.to_string_lossy().into_owned()),
            "maven.home" => install.map(|p| p.to_string_lossy().into_owned()),
            _ => name
                .strip_prefix("env.")
                .and_then(|var| std::env::var(var).ok()),
        };
        match value {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[start..start + 2 + end + 1]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Maven 的安装目录（下面有 `conf/settings.xml` 的那一层）。
fn maven_installation() -> Option<PathBuf> {
    for var in ["MAVEN_HOME", "M2_HOME"] {
        if let Some(dir) = std::env::var_os(var).map(PathBuf::from) {
            if dir.join("conf/settings.xml").is_file() {
                return Some(dir);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    let names: &[&str] = if cfg!(windows) {
        &["mvn.cmd", "mvn.bat", "mvn"]
    } else {
        &["mvn"]
    };
    std::env::split_paths(&path).find_map(|dir| {
        let exe = names.iter().map(|n| dir.join(n)).find(|p| p.is_file())?;
        installation_of(&exe)
    })
}

/// 由 `mvn` 可执行文件推出安装目录：跟随符号链接后取 `bin` 的上一级；
/// Homebrew 的 `bin/mvn` 是个包装脚本，真正的安装在旁边的 `libexec` 里。
fn installation_of(exe: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let base = real.parent()?.parent()?;
    [base.to_path_buf(), base.join("libexec")]
        .into_iter()
        .find(|dir| dir.join("conf/settings.xml").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_local_repository_element() {
        let settings = r#"<?xml version="1.0"?>
<settings xmlns="http://maven.apache.org/SETTINGS/1.0.0">
  <!-- <localRepository>/commented/out</localRepository> -->
  <localRepository>
    D:\repo\maven
  </localRepository>
</settings>"#;
        assert_eq!(
            local_repository_in(settings).as_deref(),
            Some(r"D:\repo\maven")
        );
        assert_eq!(local_repository_in("<settings></settings>"), None);
    }

    #[test]
    fn repo_local_from_maven_opts() {
        assert_eq!(
            repo_local_from_opts("-Xmx1g -Dmaven.repo.local=/data/m2 -B").as_deref(),
            Some("/data/m2")
        );
        assert_eq!(repo_local_from_opts("-Xmx1g"), None);
    }

    #[test]
    fn interpolates_known_placeholders() {
        let home = Path::new("/home/u");
        let install = Path::new("/opt/maven");
        assert_eq!(
            interpolate("${user.home}/.m2/custom", home, Some(install)),
            "/home/u/.m2/custom"
        );
        assert_eq!(
            interpolate("${maven.home}/repo", home, Some(install)),
            "/opt/maven/repo"
        );
        assert_eq!(interpolate("${unknown}/x", home, None), "${unknown}/x");
    }

    #[test]
    fn global_settings_are_read_after_user_settings() {
        let dir = std::env::temp_dir().join(format!("ja-maven-settings-{}", std::process::id()));
        let home = dir.join("home");
        let install = dir.join("maven");
        std::fs::create_dir_all(install.join("conf")).unwrap();
        std::fs::create_dir_all(install.join("bin")).unwrap();
        std::fs::write(install.join("bin/mvn"), "").unwrap();
        std::fs::write(
            install.join("conf/settings.xml"),
            "<settings><localRepository>/global/repo</localRepository></settings>",
        )
        .unwrap();

        // 只有全局配置
        let files = settings_files(&home, Some(&install));
        assert_eq!(files, vec![install.join("conf/settings.xml")]);
        assert_eq!(
            installation_of(&install.join("bin/mvn")),
            Some(std::fs::canonicalize(&install).unwrap())
        );

        // 用户配置优先
        std::fs::create_dir_all(home.join(".m2")).unwrap();
        std::fs::write(home.join(".m2/settings.xml"), "<settings></settings>").unwrap();
        let files = settings_files(&home, Some(&install));
        assert_eq!(files[0], home.join(".m2/settings.xml"));
        // 用户配置里没有 localRepository 时落到全局的
        let found = files
            .iter()
            .find_map(|f| local_repository_in(&std::fs::read_to_string(f).unwrap()));
        assert_eq!(found.as_deref(), Some("/global/repo"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
