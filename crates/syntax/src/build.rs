//! 把 parser 的建树指令和 lexer 的原始 token（含空白、注释）组装成 rowan 语法树。

use parser::{Step, SyntaxKind, Token};
use rowan::{GreenNode, GreenNodeBuilder, TextRange, TextSize};

use crate::SyntaxError;

pub(crate) fn build_tree(text: &str) -> (GreenNode, Vec<SyntaxError>) {
    let (tokens, lex_errors) = parser::tokenize(text);
    let input = parser::Input::from_tokens(text, &tokens);
    let output = parser::parse(&input);
    assemble(text, &tokens, lex_errors, &output)
}

/// 把 `text` 当作一个独立的块来解析。只有在它确实是一个花括号配对、
/// 且被块语法完整消费的片段时才返回结果；否则调用方应退回整文件解析。
pub(crate) fn build_block(text: &str) -> Option<(GreenNode, Vec<SyntaxError>)> {
    let (tokens, lex_errors) = parser::tokenize(text);

    // 第一个 token 是 `{`，与它配对的 `}` 必须正好是最后一个 token
    let significant: Vec<SyntaxKind> = tokens
        .iter()
        .map(|t| t.kind)
        .filter(|k| !k.is_trivia())
        .collect();
    if significant.first() != Some(&SyntaxKind::L_CURLY)
        || tokens.last().map(|t| t.kind) != Some(SyntaxKind::R_CURLY)
    {
        return None;
    }
    let mut depth = 0i32;
    for (idx, kind) in significant.iter().enumerate() {
        match kind {
            SyntaxKind::L_CURLY => depth += 1,
            SyntaxKind::R_CURLY => {
                depth -= 1;
                if depth == 0 && idx + 1 != significant.len() {
                    return None;
                }
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }

    let input = parser::Input::from_tokens(text, &tokens);
    let output = parser::parse_block(&input);
    let (green, errors) = assemble(text, &tokens, lex_errors, &output);
    // 块语法提前收尾（错误恢复）时不会消费完全部输入
    let complete = green.kind() == rowan::SyntaxKind(SyntaxKind::BLOCK as u16)
        && usize::from(green.text_len()) == text.len();
    complete.then_some((green, errors))
}

fn assemble(
    text: &str,
    tokens: &[Token],
    lex_errors: Vec<parser::LexError>,
    output: &parser::Output,
) -> (GreenNode, Vec<SyntaxError>) {
    let mut builder = Builder {
        text,
        tokens,
        pos: 0,
        offset: 0,
        last_significant_end: 0,
        depth: 0,
        inner: GreenNodeBuilder::new(),
        errors: Vec::new(),
    };
    for step in output.steps() {
        match step {
            Step::Enter { kind } => builder.enter(*kind),
            Step::Token {
                kind,
                n_input_tokens,
            } => builder.token(*kind, *n_input_tokens),
            Step::Exit => builder.exit(),
            Step::Error { msg } => builder.error(msg.clone()),
        }
    }
    let Builder {
        inner, mut errors, ..
    } = builder;

    errors.extend(lex_errors.into_iter().map(|e| SyntaxError {
        message: e.message.to_string(),
        range: TextRange::at(TextSize::from(e.offset), TextSize::from(e.len)),
    }));
    errors.sort_by_key(|e| (e.range.start(), e.range.end()));
    (inner.finish(), errors)
}

struct Builder<'a> {
    text: &'a str,
    tokens: &'a [Token],
    /// 下一个尚未写入树的原始 token 下标
    pos: usize,
    /// `pos` 对应的字节偏移
    offset: usize,
    /// 上一个非空白 token 的结束偏移
    last_significant_end: usize,
    depth: usize,
    inner: GreenNodeBuilder<'static>,
    errors: Vec<SyntaxError>,
}

impl Builder<'_> {
    fn enter(&mut self, kind: SyntaxKind) {
        if self.depth > 0 {
            // 节点前面的空白和注释默认留在外面；声明节点会把紧贴着的注释（文档注释）收进来。
            let n_trivia = self.n_trivia();
            let n_attached = self.n_attached_trivia(kind, n_trivia);
            self.eat_n(n_trivia - n_attached);
        }
        self.inner.start_node(rowan::SyntaxKind(kind as u16));
        self.depth += 1;
    }

    fn exit(&mut self) {
        self.depth -= 1;
        if self.depth == 0 {
            // 根节点收走文件末尾的所有空白和注释
            let n = self.n_trivia();
            self.eat_n(n);
        }
        self.inner.finish_node();
    }

    fn token(&mut self, kind: SyntaxKind, n_input_tokens: u8) {
        let n = self.n_trivia();
        self.eat_n(n);
        let start = self.offset;
        let mut end = start;
        for token in self
            .tokens
            .iter()
            .skip(self.pos)
            .take(n_input_tokens as usize)
        {
            end += token.len as usize;
        }
        self.inner
            .token(rowan::SyntaxKind(kind as u16), &self.text[start..end]);
        self.pos += n_input_tokens as usize;
        self.offset = end;
        self.last_significant_end = end;
    }

    fn error(&mut self, message: String) {
        // 下一个有效 token 的范围
        let mut offset = self.offset;
        let mut next = None;
        for token in &self.tokens[self.pos..] {
            if !token.kind.is_trivia() {
                next = Some((offset, token.len as usize));
                break;
            }
            offset += token.len as usize;
        }
        let prev_end = self.last_significant_end;
        let range = match next {
            // 中间隔着换行（多半是"缺分号"之类）或已到文件末尾：标在上一个 token 的末尾
            Some((start, len)) if !self.text[prev_end..start].contains('\n') => {
                TextRange::at(TextSize::from(start as u32), TextSize::from(len as u32))
            }
            _ => TextRange::empty(TextSize::from(prev_end as u32)),
        };
        self.errors.push(SyntaxError { message, range });
    }

    fn n_trivia(&self) -> usize {
        self.tokens[self.pos..]
            .iter()
            .take_while(|t| t.kind.is_trivia())
            .count()
    }

    fn eat_n(&mut self, n: usize) {
        for _ in 0..n {
            let token = self.tokens[self.pos];
            let end = self.offset + token.len as usize;
            self.inner.token(
                rowan::SyntaxKind(token.kind as u16),
                &self.text[self.offset..end],
            );
            self.pos += 1;
            self.offset = end;
        }
    }

    /// 紧挨在声明前面的注释有多少个 trivia token 应当归入该声明节点。
    fn n_attached_trivia(&self, kind: SyntaxKind, n_trivia: usize) -> usize {
        use SyntaxKind::*;
        if !matches!(
            kind,
            CLASS_DECL
                | INTERFACE_DECL
                | ENUM_DECL
                | RECORD_DECL
                | ANNOTATION_TYPE_DECL
                | FIELD_DECL
                | METHOD_DECL
                | CONSTRUCTOR_DECL
                | ENUM_CONSTANT
                | MODULE_DECL
        ) {
            return 0;
        }
        // 每个 trivia 的结束偏移，倒着看
        let trivias = &self.tokens[self.pos..self.pos + n_trivia];
        let mut end = self.offset + trivias.iter().map(|t| t.len as usize).sum::<usize>();
        let mut attached = 0;
        for (i, token) in trivias.iter().rev().enumerate() {
            let start = end - token.len as usize;
            match token.kind {
                WHITESPACE => {
                    // 空行把注释和声明隔开了
                    if self.text[start..end].matches('\n').count() >= 2 {
                        break;
                    }
                }
                _ => attached = i + 1,
            }
            end = start;
        }
        attached
    }
}
