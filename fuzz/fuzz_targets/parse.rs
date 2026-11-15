//! 任意输入都不能让 lexer / parser panic 或死循环，并且语法树必须无损。
//!
//! 运行：`cargo +nightly fuzz run parse`

#![no_main]

use libfuzzer_sys::fuzz_target;
use syntax::ast::CompilationUnit;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let parse = CompilationUnit::parse(text);
    let root = parse.syntax_node();
    assert_eq!(root.text().to_string(), text, "语法树文本必须等于原文");
    for err in parse.errors() {
        assert!(root.text_range().contains_range(err.range), "错误范围越界: {err:?}");
    }
});
