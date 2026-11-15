//! Eclipse PDE / Tycho 工程（`packaging` 为 `eclipse-plugin` 等）的源码目录和插件依赖。
//!
//! - 源码目录取自 `.classpath` 里的 `<classpathentry kind="src">`（带 `test` 属性的是测试源码），
//!   以及 `build.properties` 的 `source.<jar> = 目录,...`；
//! - 依赖取自 `META-INF/MANIFEST.MF` 的 `Require-Bundle` 和 `Fragment-Host`：reactor 里有同名模块
//!   的连到模块上，否则在本地仓库里 Tycho 缓存的 p2 构件（`p2/osgi/bundle/<名字>/<版本>/`）中找。

use std::path::{Path, PathBuf};

use base_db::{SourceRoot, SourceRootKind};

use crate::xml;

/// `.classpath` 和 `build.properties` 里声明的源码目录（只返回存在的）。
pub(crate) fn source_roots(dir: &Path) -> Vec<SourceRoot> {
    let mut roots: Vec<SourceRoot> = Vec::new();
    let mut push = |path: PathBuf, kind: SourceRootKind| {
        if path.is_dir() && !roots.iter().any(|r| r.path == path) {
            roots.push(SourceRoot { path, kind });
        }
    };
    if let Some(root) = std::fs::read_to_string(dir.join(".classpath"))
        .ok()
        .and_then(|text| xml::parse(&text))
    {
        for entry in root.children_named("classpathentry") {
            let Some(path) = entry.attr("path") else {
                continue;
            };
            // 以 `/` 开头的是对别的工程的引用
            if entry.attr("kind") != Some("src") || path.starts_with('/') {
                continue;
            }
            let is_test = entry
                .child("attributes")
                .into_iter()
                .flat_map(|attrs| attrs.children_named("attribute"))
                .any(|a| a.attr("name") == Some("test") && a.attr("value") == Some("true"));
            let kind = if is_test {
                SourceRootKind::Test
            } else {
                SourceRootKind::Main
            };
            push(dir.join(path), kind);
        }
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("build.properties")) {
        for (key, value) in properties(&text) {
            if key.starts_with("source.") {
                for path in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    push(dir.join(path), SourceRootKind::Main);
                }
            }
        }
    }
    roots
}

/// Java properties 文件：`key = value` / `key: value`，行尾 `\` 续行，`#` / `!` 注释。
fn properties(text: &str) -> Vec<(String, String)> {
    let mut res = Vec::new();
    let mut logical = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if logical.is_empty() && (trimmed.starts_with('#') || trimmed.starts_with('!')) {
            continue;
        }
        match trimmed.strip_suffix('\\') {
            Some(continued) => {
                logical.push_str(continued);
                continue;
            }
            None => logical.push_str(trimmed),
        }
        let line = std::mem::take(&mut logical);
        if let Some(split) = line.find(['=', ':']) {
            res.push((
                line[..split].trim().to_owned(),
                line[split + 1..].trim().to_owned(),
            ));
        }
    }
    res
}

/// `MANIFEST.MF` 的主段：续行（以一个空格开头）拼回去。
fn manifest_headers(text: &str) -> Vec<(String, String)> {
    let mut res: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        if let Some(continued) = line.strip_prefix(' ') {
            if let Some((_, value)) = res.last_mut() {
                value.push_str(continued);
            }
        } else if let Some((name, value)) = line.split_once(':') {
            res.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    res
}

/// 按逗号拆开清单头的各项（引号里的逗号不算），每项只取 `;` 之前的名字。
fn clause_names(value: &str) -> Vec<String> {
    let mut res = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => res.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    res.push(current);
    res.into_iter()
        .filter_map(|clause| {
            let name = clause.split(';').next()?.trim().to_owned();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// 插件依赖的其它插件（`Require-Bundle` 和片段的 `Fragment-Host`）的符号名。
pub(crate) fn required_bundles(dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join("META-INF/MANIFEST.MF")) else {
        return Vec::new();
    };
    let mut res = Vec::new();
    for (name, value) in manifest_headers(&text) {
        if name == "Require-Bundle" || name == "Fragment-Host" {
            for bundle in clause_names(&value) {
                if !res.contains(&bundle) {
                    res.push(bundle);
                }
            }
        }
    }
    res
}

/// 插件自己的符号名（`Bundle-SymbolicName`）。
pub(crate) fn symbolic_name(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("META-INF/MANIFEST.MF")).ok()?;
    manifest_headers(&text)
        .into_iter()
        .find(|(name, _)| name == "Bundle-SymbolicName")
        .and_then(|(_, value)| clause_names(&value).into_iter().next())
}

/// 版本号按数字段比较（`3.10.0` > `3.9.1`），非数字段按字符串。
fn version_key(version: &str) -> Vec<(u64, String)> {
    version
        .split(['.', '-', '_'])
        .map(|part| match part.parse::<u64>() {
            Ok(n) => (n, String::new()),
            Err(_) => (0, part.to_owned()),
        })
        .collect()
}

/// 本地仓库里 Tycho 缓存的插件 jar：取最高的版本。
pub(crate) fn p2_bundle_jar(repo: &Path, bundle: &str) -> Option<PathBuf> {
    let dir = repo.join("p2/osgi/bundle").join(bundle);
    let mut versions: Vec<(Vec<(u64, String)>, PathBuf)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let version = entry.file_name().to_string_lossy().into_owned();
            let jar = entry.path().join(format!("{bundle}-{version}.jar"));
            jar.is_file().then(|| (version_key(&version), jar))
        })
        .collect();
    versions.sort();
    versions.pop().map(|(_, jar)| jar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_clauses() {
        let manifest = "Manifest-Version: 1.0\nBundle-SymbolicName: org.example.core;singleton:=true\nRequire-Bundle: org.eclipse.core.runtime;bundle-version=\"[3.0.0,4\n .0.0)\",\n org.eclipse.jdt.core;resolution:=optional\n\nName: other\nRequire-Bundle: ignored\n";
        let headers = manifest_headers(manifest);
        let require = headers
            .iter()
            .find(|(n, _)| n == "Require-Bundle")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(
            clause_names(require),
            vec!["org.eclipse.core.runtime", "org.eclipse.jdt.core"]
        );
        assert_eq!(
            headers
                .iter()
                .filter(|(n, _)| n == "Require-Bundle")
                .count(),
            1
        );
    }

    #[test]
    fn properties_with_continuations() {
        let props =
            properties("# comment\nsource.. = src/,\\\n  src-gen/\nbin.includes = META-INF/\n");
        assert_eq!(
            props,
            vec![
                ("source..".to_owned(), "src/,src-gen/".to_owned()),
                ("bin.includes".to_owned(), "META-INF/".to_owned()),
            ]
        );
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_key("3.10.0.v2024") > version_key("3.9.1.v2023"));
    }

    #[test]
    fn plugin_layout() {
        let dir = std::env::temp_dir().join(format!("ja-eclipse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::create_dir_all(dir.join("gen")).unwrap();
        std::fs::write(
            dir.join(".classpath"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<classpath>
  <classpathentry kind="src" path="src"/>
  <classpathentry kind="src" path="tests"><attributes><attribute name="test" value="true"/></attributes></classpathentry>
  <classpathentry kind="src" path="/other.project"/>
  <classpathentry kind="con" path="org.eclipse.pde.core.requiredPlugins"/>
  <classpathentry kind="output" path="bin"/>
</classpath>"#,
        )
        .unwrap();
        std::fs::write(dir.join("build.properties"), "source.. = src/,gen/\n").unwrap();
        let roots = source_roots(&dir);
        let shown: Vec<(PathBuf, SourceRootKind)> =
            roots.into_iter().map(|r| (r.path, r.kind)).collect();
        assert_eq!(
            shown,
            vec![
                (dir.join("src"), SourceRootKind::Main),
                (dir.join("tests"), SourceRootKind::Test),
                (dir.join("gen/"), SourceRootKind::Main),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
