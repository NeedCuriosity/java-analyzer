//! `.editorconfig` 里对 Java 源文件生效的设置（代码风格：缩进、空行、空格、import 布局等）。

use std::collections::BTreeMap;

use base_db::Db;

/// 合并后的设置：键统一成小写，值保持原文（去掉首尾空白）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditorConfig {
    properties: BTreeMap<String, String>,
}

impl EditorConfig {
    /// 按顺序叠加若干个 `.editorconfig` 文件的内容（外层目录的在前，后面的覆盖前面的）。
    /// 文件开头写了 `root = true` 的，丢掉它之前叠加的内容。
    pub fn from_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> EditorConfig {
        let mut config = EditorConfig::default();
        for text in texts {
            config.apply(text);
        }
        config
    }

    fn apply(&mut self, text: &str) {
        // 第一个节之前的是文件级的设置（只有 `root` 有意义）；`None` 表示还没进入任何节
        let mut applies: Option<bool> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                applies = Some(section_matches_java(header));
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match applies {
                None => {
                    if key == "root" && value.eq_ignore_ascii_case("true") {
                        self.properties.clear();
                    }
                }
                Some(false) => {}
                Some(true) => {
                    if value.eq_ignore_ascii_case("unset") {
                        self.properties.remove(&key);
                    } else {
                        self.properties.insert(key, value.to_owned());
                    }
                }
            }
        }
    }

    /// 一个设置的原文。
    pub fn get(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(String::as_str)
    }

    /// 解析成指定类型的设置（`true` / `false`、数字）；大小写不敏感，写错的值当作没设置。
    pub fn parse<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.get(key)?.to_ascii_lowercase().parse().ok()
    }

    pub fn is_empty(&self) -> bool {
        self.properties.is_empty()
    }
}

/// 工作区里对 Java 文件生效的 `.editorconfig` 设置：外层目录的先读，里层的覆盖。
#[salsa::tracked(returns(ref))]
pub fn java_editorconfig(db: &dyn Db) -> EditorConfig {
    let mut files: Vec<&base_db::ResourceFile> = base_db::workspace(db)
        .resources(db)
        .iter()
        .filter(|r| r.name == ".editorconfig")
        .collect();
    files.sort_by_key(|r| r.file_id);
    EditorConfig::from_texts(files.into_iter().map(|f| &*f.text))
}

/// 节的模式是否匹配 Java 源文件（只看文件名部分：`*`、`*.java`、`{*.java,*.kt}`、`**/*.java`）。
pub(crate) fn section_matches_java(pattern: &str) -> bool {
    let file_part = pattern.rsplit('/').next().unwrap_or(pattern);
    expand_braces(file_part)
        .iter()
        .any(|alt| glob_matches(alt, "Example.java"))
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let (Some(open), Some(close)) = (pattern.find('{'), pattern.find('}')) else {
        return vec![pattern.to_owned()];
    };
    if close < open {
        return vec![pattern.to_owned()];
    }
    let (head, rest) = pattern.split_at(open);
    let inner = &rest[1..close - open];
    let tail = &rest[close - open + 1..];
    inner
        .split(',')
        .flat_map(|alt| expand_braces(&format!("{head}{alt}{tail}")))
        .collect()
}

/// `*`（包括 `**`）匹配任意多个字符，`?` 匹配一个。
fn glob_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            while pi < p.len() && p[pi] == '*' {
                pi += 1;
            }
            star = Some(pi);
            mark = ni;
        } else if let Some(s) = star {
            pi = s;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_and_overrides() {
        assert!(section_matches_java("**/*.java"));
        assert!(section_matches_java("*"));
        assert!(section_matches_java("{*.java,*.kt}"));
        assert!(!section_matches_java("*.kt"));

        let outer = "root = true\n[*]\nindent_size = 2\nindent_style = space\n[*.java]\nij_java_keep_blank_lines_in_code = 1\n[*.md]\nindent_size = 8\n";
        let inner = "[*.java]\nindent_size = 4\nindent_style = unset\n";
        let config = EditorConfig::from_texts([outer, inner]);
        assert_eq!(config.get("indent_size"), Some("4"));
        assert_eq!(config.get("indent_style"), None);
        assert_eq!(
            config.parse::<usize>("ij_java_keep_blank_lines_in_code"),
            Some(1)
        );

        // 里层文件写了 `root = true`：外层的不再生效
        let config = EditorConfig::from_texts([outer, "root = TRUE\n[*]\ntab_width = 8\n"]);
        assert_eq!(config.get("indent_size"), None);
        assert_eq!(config.parse::<usize>("tab_width"), Some(8));
    }

    #[test]
    fn values_parse_case_insensitively() {
        let config =
            EditorConfig::from_texts(["[*]\nij_java_space_within_parentheses = True\nx = nope\n"]);
        assert_eq!(
            config.parse::<bool>("ij_java_space_within_parentheses"),
            Some(true)
        );
        assert_eq!(config.parse::<bool>("x"), None);
    }
}
