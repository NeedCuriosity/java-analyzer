//! 增量重解析的结果必须与整文件解析完全一致。
//!
//! 输入的第一个字节决定切分点：前一半是旧文本，整段是把"后一半"插进旧文本某处得到的新文本。
//! 运行：`cargo +nightly fuzz run reparse`

#![no_main]

use libfuzzer_sys::fuzz_target;
use syntax::{ast::CompilationUnit, diff_range};

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else { return };
    let Ok(text) = std::str::from_utf8(rest) else { return };
    if text.is_empty() {
        return;
    }
    // 把 text 切成 old + inserted，再把 inserted 插到 old 的某个位置
    let mut split = text.len() * usize::from(selector) / 256;
    while !text.is_char_boundary(split) {
        split -= 1;
    }
    let (old, inserted) = text.split_at(split);
    let mut at = old.len() / 2;
    while !old.is_char_boundary(at) {
        at -= 1;
    }
    let mut new = String::with_capacity(text.len());
    new.push_str(&old[..at]);
    new.push_str(inserted);
    new.push_str(&old[at..]);

    let Some(edit) = diff_range(old, &new) else { return };
    let old_parse = CompilationUnit::parse(old);
    if let Some(incremental) = old_parse.reparse(edit, &new) {
        let full = CompilationUnit::parse(&new);
        assert_eq!(incremental.green(), full.green(), "增量与全量的语法树不一致");
        assert_eq!(incremental.errors(), full.errors(), "增量与全量的错误不一致");
    }
});
