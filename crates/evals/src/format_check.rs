//! 格式化器的语料检查：格式化不能改变代码的含义。
//!
//! 对目录下的每个 `.java` 文件：格式化后重新解析，非空白 token 的序列必须和原来逐个相同，
//! 不能多出语法错误；对结果再格式化一遍不能有任何改动（幂等）。
//!
//! 另外检查一条和具体风格无关的排版性质：独占行首的 `}` 要和它所属的声明 / 语句的
//! 起始行缩进相同。形参表、条件折行之后块的缩进算错，会在这里暴露出来。
//! 只有原文满足这条性质、格式化之后不满足了才算问题（"排坏了"），单独计数，
//! 可以用 `--max-misaligned N` 给一个上限。

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use syntax::{ast, SyntaxKind, SyntaxNode};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "java") {
            out.push(path);
        }
    }
}

pub(crate) fn apply(text: &str, mut edits: Vec<ide::TextEdit>) -> Option<String> {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start()));
    let mut out = text.to_owned();
    let mut limit = text.len();
    for edit in edits {
        let (start, end) = (
            usize::from(edit.range.start()),
            usize::from(edit.range.end()),
        );
        // 修改之间不能重叠
        if end > limit {
            return None;
        }
        out.replace_range(start..end, &edit.new_text);
        limit = start;
    }
    Some(out)
}

/// 非空白 token 的 (种类, 文本)。注释的缩进会被重排，只比较去掉每行首尾空白之后的内容。
fn significant(text: &str) -> (Vec<(SyntaxKind, String)>, usize) {
    let parse = ast::CompilationUnit::parse(text);
    let tokens = parse
        .syntax_node()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| token.kind() != SyntaxKind::WHITESPACE)
        .map(|token| {
            let text = if token.kind().is_comment() {
                token
                    .text()
                    .lines()
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                token.text().to_owned()
            };
            (token.kind(), text)
        })
        .collect();
    (tokens, parse.errors().len())
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// `offset` 所在行的缩进。
fn indent_at(text: &str, offset: usize) -> &str {
    let line = &text[line_start(text, offset)..];
    let len = line
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(line.len());
    &line[..len]
}

/// 独占行首的 `}` 和所属结构的起始行缩进不一致的地方，返回第一处的说明。
pub(crate) fn misaligned_brace(text: &str) -> Option<String> {
    let root: SyntaxNode = ast::CompilationUnit::parse(text).syntax_node();
    for token in root
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| token.kind() == SyntaxKind::R_CURLY)
    {
        let Some(body) = token.parent() else {
            continue;
        };
        // 语法错误恢复出来的节点里可能没有配对的 `{`
        let Some(open) = body
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .find(|t| t.kind() == SyntaxKind::L_CURLY)
        else {
            continue;
        };
        let close = usize::from(token.text_range().start());
        let open = usize::from(open.text_range().start());
        if line_start(text, open) == line_start(text, close) {
            continue;
        }
        // `}` 前面还有别的内容时不归这条性质管
        if !text[line_start(text, close)..close].trim().is_empty() {
            continue;
        }
        // 块所属的声明 / 语句从哪里开始：跳过开头的注释
        let owner = body.parent().unwrap_or(body);
        let Some(first) = owner
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .find(|t| t.kind() != SyntaxKind::WHITESPACE && !t.kind().is_comment())
        else {
            continue;
        };
        // 外层本身是一对花括号（块里的裸块、类体里的初始化块）：没有所属的语句
        if first.kind() == SyntaxKind::L_CURLY {
            continue;
        }
        let start = usize::from(first.text_range().start());
        let (expected, actual) = (indent_at(text, start), indent_at(text, close));
        if expected != actual {
            let line = |offset: usize| text[..offset].matches('\n').count() + 1;
            return Some(format!(
                "第 {} 行的 `}}` 缩进是 {} 列，它所属的结构从第 {} 行开始、缩进是 {} 列",
                line(close),
                actual.len(),
                line(start),
                expected.len()
            ));
        }
    }
    None
}

enum Problem {
    /// 含义被改变或不幂等
    Broken(String),
    /// 原文花括号是对齐的，格式化之后不对齐了
    Misaligned(String),
}

fn check_file(path: &Path) -> Result<bool, Problem> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    check_text(&text)
}

fn check_text(text: &str) -> Result<bool, Problem> {
    let text = text.to_owned();
    let options = ide::FormatOptions::default();
    let edits = ide::format_text(&text, &options);
    let changed = !edits.is_empty();
    let formatted =
        apply(&text, edits).ok_or_else(|| Problem::Broken("修改互相重叠".to_owned()))?;
    let (before, errors_before) = significant(&text);
    let (after, errors_after) = significant(&formatted);
    if errors_after > errors_before {
        return Err(Problem::Broken(format!(
            "格式化后多出语法错误（{errors_before} → {errors_after}）"
        )));
    }
    if before != after {
        let at = before
            .iter()
            .zip(after.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(before.len().min(after.len()));
        return Err(Problem::Broken(format!(
            "token 序列变了，第 {at} 个: {:?} → {:?}",
            before.get(at),
            after.get(at)
        )));
    }
    let again = ide::format_text(&formatted, &options);
    if let Some(edit) = again.first() {
        let start = usize::from(edit.range.start());
        let line = formatted[..start].matches('\n').count() + 1;
        return Err(Problem::Broken(format!(
            "不是幂等的：格式化后的第 {line} 行还会被改成 {:?}",
            edit.new_text
        )));
    }
    if errors_after == 0 && misaligned_brace(&text).is_none() {
        if let Some(problem) = misaligned_brace(&formatted) {
            return Err(Problem::Misaligned(problem));
        }
    }
    Ok(changed)
}

pub fn run(dir: &Path, max_misaligned: usize) -> anyhow::Result<bool> {
    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort();
    let started = std::time::Instant::now();
    let results: Vec<(PathBuf, Result<bool, Problem>)> = files
        .par_iter()
        .map(|path| (path.clone(), check_file(path)))
        .collect();
    let changed = results
        .iter()
        .filter(|(_, r)| matches!(r, Ok(true)))
        .count();
    let broken: Vec<_> = results
        .iter()
        .filter_map(|(path, r)| match r {
            Err(Problem::Broken(error)) => Some((path, error)),
            _ => None,
        })
        .collect();
    let misaligned: Vec<_> = results
        .iter()
        .filter_map(|(path, r)| match r {
            Err(Problem::Misaligned(error)) => Some((path, error)),
            _ => None,
        })
        .collect();
    println!(
        "== 格式化语料检查（{} 个文件，{:.2?}）==",
        files.len(),
        started.elapsed()
    );
    println!("  会被改动的文件: {changed}");
    println!("  含义被改变或不幂等的文件: {}", broken.len());
    for (path, error) in broken.iter().take(25) {
        println!("      {}: {error}", path.display());
    }
    println!(
        "  花括号被排歪的文件: {}（允许 {max_misaligned}）",
        misaligned.len()
    );
    for (path, error) in misaligned.iter().take(10) {
        println!("      {}: {error}", path.display());
    }
    Ok(broken.is_empty() && misaligned.len() <= max_misaligned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_braces_pass() {
        let text = "class A {\n    void f(int a,\n            int b) {\n        if (a > b\n                && b > 0) {\n            g(() -> {\n            });\n        } else {\n        }\n    }\n}\n";
        assert_eq!(misaligned_brace(text), None);
    }

    #[test]
    fn body_indented_after_wrapped_parameters_fails() {
        let text = "class A {\n    void f(int a,\n            int b) {\n                g();\n            }\n}\n";
        let problem = misaligned_brace(text).unwrap();
        assert!(problem.contains("第 5 行"), "{problem}");
    }
}
