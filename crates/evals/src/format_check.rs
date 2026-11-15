//! 格式化器的语料检查：格式化不能改变代码的含义。
//!
//! 对目录下的每个 `.java` 文件：格式化后重新解析，非空白 token 的序列必须和原来逐个相同，
//! 不能多出语法错误；对结果再格式化一遍不能有任何改动（幂等）。

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use syntax::{ast, SyntaxKind};

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

fn apply(text: &str, mut edits: Vec<ide::TextEdit>) -> Option<String> {
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

fn check_file(path: &Path) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    let options = ide::FormatOptions::default();
    let edits = ide::format_text(&text, &options);
    let changed = !edits.is_empty();
    let formatted = apply(&text, edits).ok_or("修改互相重叠")?;
    let (before, errors_before) = significant(&text);
    let (after, errors_after) = significant(&formatted);
    if errors_after > errors_before {
        return Err(format!(
            "格式化后多出语法错误（{errors_before} → {errors_after}）"
        ));
    }
    if before != after {
        let at = before
            .iter()
            .zip(after.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(before.len().min(after.len()));
        return Err(format!(
            "token 序列变了，第 {at} 个: {:?} → {:?}",
            before.get(at),
            after.get(at)
        ));
    }
    let again = ide::format_text(&formatted, &options);
    if let Some(edit) = again.first() {
        let start = usize::from(edit.range.start());
        let line = formatted[..start].matches('\n').count() + 1;
        return Err(format!(
            "不是幂等的：格式化后的第 {line} 行还会被改成 {:?}",
            edit.new_text
        ));
    }
    Ok(changed)
}

pub fn run(dir: &Path) -> anyhow::Result<bool> {
    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort();
    let started = std::time::Instant::now();
    let results: Vec<(PathBuf, Result<bool, String>)> = files
        .par_iter()
        .map(|path| (path.clone(), check_file(path)))
        .collect();
    let changed = results
        .iter()
        .filter(|(_, r)| matches!(r, Ok(true)))
        .count();
    let failures: Vec<_> = results
        .iter()
        .filter_map(|(path, r)| r.as_ref().err().map(|e| (path, e)))
        .collect();
    println!(
        "== 格式化语料检查（{} 个文件，{:.2?}）==",
        files.len(),
        started.elapsed()
    );
    println!("  会被改动的文件: {changed}");
    println!("  含义被改变或不幂等的文件: {}", failures.len());
    for (path, error) in failures.iter().take(25) {
        println!("      {}: {error}", path.display());
    }
    Ok(failures.is_empty())
}
