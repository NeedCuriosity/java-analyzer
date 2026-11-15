//! Java 的 lexer 和容错 parser。
//!
//! 本 crate 不依赖任何语法树实现：输入是 token 序列，输出是一串建树指令（[`Step`]），
//! 由 `syntax` crate 负责把它们和空白/注释一起组装成无损语法树。

mod event;
mod grammar;
mod input;
mod lexer;
mod output;
mod parser;
mod syntax_kind;

pub use crate::{
    input::Input,
    lexer::{tokenize, LexError, Token},
    output::{Output, Step},
    syntax_kind::SyntaxKind,
};

/// 把整个文件解析为 `COMPILATION_UNIT`。
pub fn parse(input: &Input) -> Output {
    let mut p = parser::Parser::new(input);
    grammar::compilation_unit(&mut p);
    event::process(p.finish())
}

/// 把一段以 `{` 开头的文本解析为单个 `BLOCK`（增量重解析方法体时使用）。
/// 调用方负责确认花括号配对；如果输出没有消费完全部输入，说明这段文本不能独立成块。
pub fn parse_block(input: &Input) -> Output {
    let mut p = parser::Parser::new(input);
    grammar::block_fragment(&mut p);
    event::process(p.finish())
}
