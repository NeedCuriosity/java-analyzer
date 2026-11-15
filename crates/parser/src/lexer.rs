//! 手写 lexer：把源码切成带长度的 token 序列（包含空白和注释）。
//!
//! 约定：
//! - `>` 永远单独成 token（`>=` 除外），`>>`、`>>>`、`>>=`、`>>>=` 由 parser 根据相邻关系拼接，
//!   这样泛型的闭合尖括号不需要特殊处理。
//! - 上下文关键字（`var`、`record`、`sealed` …）一律产出 `IDENT`。

use crate::SyntaxKind::{self, *};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub len: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    /// 出错 token 的起始字节偏移
    pub offset: u32,
    pub len: u32,
    pub message: &'static str,
}

pub fn tokenize(text: &str) -> (Vec<Token>, Vec<LexError>) {
    let mut lexer = Lexer {
        text,
        pos: 0,
        errors: Vec::new(),
    };
    let mut tokens = Vec::with_capacity(text.len() / 4);
    while lexer.pos < text.len() {
        let start = lexer.pos;
        let kind = lexer.next_kind();
        debug_assert!(lexer.pos > start);
        tokens.push(Token {
            kind,
            len: (lexer.pos - start) as u32,
        });
    }
    (tokens, lexer.errors)
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
    errors: Vec<LexError>,
}

fn is_java_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C')
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.text[self.pos..].chars().nth(n)
    }

    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn eat_while(&mut self, pred: impl Fn(char) -> bool) {
        while let Some(c) = self.peek() {
            if !pred(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }

    fn error(&mut self, start: usize, message: &'static str) {
        self.errors.push(LexError {
            offset: start as u32,
            len: (self.pos - start) as u32,
            message,
        });
    }

    fn next_kind(&mut self) -> SyntaxKind {
        let start = self.pos;
        let c = match self.bump() {
            Some(c) => c,
            None => return EOF,
        };
        match c {
            c if is_java_whitespace(c) => {
                self.eat_while(is_java_whitespace);
                WHITESPACE
            }
            '/' => match self.peek() {
                Some('/') => {
                    self.eat_while(|c| c != '\n' && c != '\r');
                    LINE_COMMENT
                }
                Some('*') => {
                    self.bump();
                    let is_doc = self.peek() == Some('*') && self.peek_nth(1) != Some('/');
                    match self.rest().find("*/") {
                        Some(idx) => self.pos += idx + 2,
                        None => {
                            self.pos = self.text.len();
                            self.error(start, "注释未闭合");
                        }
                    }
                    if is_doc {
                        DOC_COMMENT
                    } else {
                        BLOCK_COMMENT
                    }
                }
                Some('=') => {
                    self.bump();
                    SLASH_EQ
                }
                _ => SLASH,
            },
            '"' => {
                if self.rest().starts_with("\"\"") {
                    self.pos += 2;
                    self.text_block(start)
                } else {
                    self.quoted(start, '"', "字符串字面量未闭合");
                    STRING_LITERAL
                }
            }
            '\'' => {
                self.quoted(start, '\'', "字符字面量未闭合");
                CHAR_LITERAL
            }
            '0'..='9' => self.number(c),
            '.' => {
                if matches!(self.peek(), Some('0'..='9')) {
                    self.decimal_fraction_and_suffix()
                } else if self.rest().starts_with("..") {
                    self.pos += 2;
                    ELLIPSIS
                } else {
                    DOT
                }
            }
            c if is_ident_start(c) => {
                self.eat_while(is_ident_continue);
                let text = &self.text[start..self.pos];
                SyntaxKind::from_keyword(text).unwrap_or(IDENT)
            }
            '(' => L_PAREN,
            ')' => R_PAREN,
            '{' => L_CURLY,
            '}' => R_CURLY,
            '[' => L_BRACK,
            ']' => R_BRACK,
            ';' => SEMICOLON,
            ',' => COMMA,
            '@' => AT,
            '?' => QUESTION,
            '~' => TILDE,
            ':' => {
                if self.eat(':') {
                    COLON_COLON
                } else {
                    COLON
                }
            }
            '=' => {
                if self.eat('=') {
                    EQ_EQ
                } else {
                    EQ
                }
            }
            '!' => {
                if self.eat('=') {
                    NEQ
                } else {
                    BANG
                }
            }
            '<' => {
                if self.eat('=') {
                    LT_EQ
                } else if self.eat('<') {
                    if self.eat('=') {
                        SHL_EQ
                    } else {
                        SHL
                    }
                } else {
                    LT
                }
            }
            '>' => {
                if self.eat('=') {
                    GT_EQ
                } else {
                    GT
                }
            }
            '+' => {
                if self.eat('+') {
                    PLUS_PLUS
                } else if self.eat('=') {
                    PLUS_EQ
                } else {
                    PLUS
                }
            }
            '-' => {
                if self.eat('-') {
                    MINUS_MINUS
                } else if self.eat('=') {
                    MINUS_EQ
                } else if self.eat('>') {
                    ARROW
                } else {
                    MINUS
                }
            }
            '*' => {
                if self.eat('=') {
                    STAR_EQ
                } else {
                    STAR
                }
            }
            '%' => {
                if self.eat('=') {
                    PERCENT_EQ
                } else {
                    PERCENT
                }
            }
            '^' => {
                if self.eat('=') {
                    CARET_EQ
                } else {
                    CARET
                }
            }
            '&' => {
                if self.eat('&') {
                    AMP_AMP
                } else if self.eat('=') {
                    AMP_EQ
                } else {
                    AMP
                }
            }
            '|' => {
                if self.eat('|') {
                    PIPE_PIPE
                } else if self.eat('=') {
                    PIPE_EQ
                } else {
                    PIPE
                }
            }
            _ => {
                self.error(start, "无法识别的字符");
                ERROR_TOKEN
            }
        }
    }

    /// 普通字符串 / 字符字面量：遇到行尾仍未闭合则报错并在行尾截断。
    fn quoted(&mut self, start: usize, quote: char, message: &'static str) {
        loop {
            match self.peek() {
                None | Some('\n') | Some('\r') => {
                    self.error(start, message);
                    return;
                }
                Some('\\') => {
                    self.bump();
                    if !matches!(self.peek(), None | Some('\n') | Some('\r')) {
                        self.bump();
                    }
                }
                Some(c) => {
                    self.bump();
                    if c == quote {
                        return;
                    }
                }
            }
        }
    }

    fn text_block(&mut self, start: usize) -> SyntaxKind {
        loop {
            match self.peek() {
                None => {
                    self.error(start, "文本块未闭合");
                    break;
                }
                Some('\\') => {
                    self.bump();
                    self.bump();
                }
                Some('"') if self.rest().starts_with("\"\"\"") => {
                    self.pos += 3;
                    break;
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
        TEXT_BLOCK
    }

    fn number(&mut self, first: char) -> SyntaxKind {
        if first == '0' {
            match self.peek() {
                Some('x') | Some('X') => {
                    self.bump();
                    self.eat_while(|c| c.is_ascii_hexdigit() || c == '_');
                    let mut is_float = false;
                    if self.peek() == Some('.')
                        && matches!(self.peek_nth(1), Some(c) if c.is_ascii_hexdigit() || c == 'p' || c == 'P')
                    {
                        self.bump();
                        self.eat_while(|c| c.is_ascii_hexdigit() || c == '_');
                        is_float = true;
                    }
                    if matches!(self.peek(), Some('p') | Some('P')) {
                        self.bump();
                        if matches!(self.peek(), Some('+') | Some('-')) {
                            self.bump();
                        }
                        self.eat_while(|c| c.is_ascii_digit() || c == '_');
                        is_float = true;
                    }
                    return if is_float {
                        self.float_suffix()
                    } else {
                        self.int_suffix()
                    };
                }
                Some('b') | Some('B') => {
                    self.bump();
                    self.eat_while(|c| c == '0' || c == '1' || c == '_');
                    return self.int_suffix();
                }
                _ => {}
            }
        }
        self.eat_while(|c| c.is_ascii_digit() || c == '_');
        // `1.5`、`1.`、`1.e3` 都是浮点数；`1.foo` / `1..` 不把点吃进来
        if self.peek() == Some('.') {
            let after = self.peek_nth(1);
            let is_fraction = match after {
                Some(c) if c.is_ascii_digit() => true,
                Some('e') | Some('E') | Some('f') | Some('F') | Some('d') | Some('D') => true,
                Some('.') => false,
                Some(c) if is_ident_start(c) => false,
                _ => true,
            };
            if is_fraction {
                self.bump();
                return self.decimal_fraction_and_suffix();
            }
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.exponent();
            return self.float_suffix();
        }
        match self.peek() {
            Some('f') | Some('F') => {
                self.bump();
                FLOAT_LITERAL
            }
            Some('d') | Some('D') => {
                self.bump();
                DOUBLE_LITERAL
            }
            _ => self.int_suffix(),
        }
    }

    /// 已经吃掉小数点，继续吃小数部分、指数和后缀。
    fn decimal_fraction_and_suffix(&mut self) -> SyntaxKind {
        self.eat_while(|c| c.is_ascii_digit() || c == '_');
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.exponent();
        }
        self.float_suffix()
    }

    fn exponent(&mut self) {
        self.bump();
        if matches!(self.peek(), Some('+') | Some('-')) {
            self.bump();
        }
        self.eat_while(|c| c.is_ascii_digit() || c == '_');
    }

    fn float_suffix(&mut self) -> SyntaxKind {
        match self.peek() {
            Some('f') | Some('F') => {
                self.bump();
                FLOAT_LITERAL
            }
            Some('d') | Some('D') => {
                self.bump();
                DOUBLE_LITERAL
            }
            _ => DOUBLE_LITERAL,
        }
    }

    fn int_suffix(&mut self) -> SyntaxKind {
        match self.peek() {
            Some('l') | Some('L') => {
                self.bump();
                LONG_LITERAL
            }
            _ => INT_LITERAL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<SyntaxKind> {
        let (tokens, _) = tokenize(text);
        tokens
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| !k.is_trivia())
            .collect()
    }

    #[test]
    fn lengths_cover_whole_input() {
        let text = "class A { int x = 0x1F; String s = \"a\\\"b\"; /** doc */ // c\n }";
        let (tokens, errors) = tokenize(text);
        assert!(errors.is_empty());
        let total: u32 = tokens.iter().map(|t| t.len).sum();
        assert_eq!(total as usize, text.len());
    }

    #[test]
    fn numbers() {
        assert_eq!(
            kinds("1 1L 1.0 1f 1e3 0x1p3 0b101 1_000 .5 1.d"),
            vec![
                INT_LITERAL,
                LONG_LITERAL,
                DOUBLE_LITERAL,
                FLOAT_LITERAL,
                DOUBLE_LITERAL,
                DOUBLE_LITERAL,
                INT_LITERAL,
                INT_LITERAL,
                DOUBLE_LITERAL,
                DOUBLE_LITERAL,
            ]
        );
    }

    #[test]
    fn generics_close_as_single_gt() {
        assert_eq!(
            kinds("Map<A, List<B>> x"),
            vec![IDENT, LT, IDENT, COMMA, IDENT, LT, IDENT, GT, GT, IDENT]
        );
    }

    #[test]
    fn comments_and_text_block() {
        let (tokens, errors) = tokenize("/**/ /** d */ \"\"\"\n a \"x\" \"\"\"");
        assert!(errors.is_empty());
        let kinds: Vec<_> = tokens
            .iter()
            .map(|t| t.kind)
            .filter(|k| *k != WHITESPACE)
            .collect();
        assert_eq!(kinds, vec![BLOCK_COMMENT, DOC_COMMENT, TEXT_BLOCK]);
    }

    #[test]
    fn unterminated_string_reports_error() {
        let (_, errors) = tokenize("\"abc\nint");
        assert_eq!(errors.len(), 1);
    }
}
