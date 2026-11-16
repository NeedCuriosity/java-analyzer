//! `evaluate` 和条件断点用的小表达式语言：
//! - 变量名、`this`、字段访问链 `a.b.c`、数组下标 `a[0]`、数组的 `length`；
//! - 字面量：整数、浮点数、`true` / `false`、`null`、字符串、字符；
//! - 比较 `== != < <= > >=`、逻辑 `&& || !`、括号。
//!
//! 字符串之间的 `==` 比较内容（相当于 `equals`），对象之间比较引用。
//! 方法调用、算术等完整的 Java 表达式不支持。

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Field(String),
    Index(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    Str(String),
    Char(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// 第一个名字和后面的访问
    Path(String, Vec<Segment>),
    Literal(Literal),
    Not(Box<Expr>),
    Binary(Box<Expr>, BinOp, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Char(u16),
    Dot,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Not,
    Op(BinOp),
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            tokens.push(Token::Ident(chars[start..i].iter().collect()));
            continue;
        }
        let negative_number = c == '-'
            && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())
            && !matches!(
                tokens.last(),
                Some(
                    Token::Ident(_)
                        | Token::Int(_)
                        | Token::Float(_)
                        | Token::RParen
                        | Token::RBracket
                )
            );
        if c.is_ascii_digit() || negative_number {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            let raw: String = chars[start..i].iter().filter(|&&c| c != '_').collect();
            let trimmed = raw.trim_end_matches(['L', 'l']);
            if let Ok(n) = trimmed.parse::<i64>() {
                tokens.push(Token::Int(n));
            } else if let Ok(f) = raw.trim_end_matches(['f', 'F', 'd', 'D']).parse::<f64>() {
                tokens.push(Token::Float(f));
            } else {
                return Err(format!("无法识别的数字 `{raw}`"));
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                let quote = c;
                let mut value = String::new();
                i += 1;
                loop {
                    let Some(&ch) = chars.get(i) else {
                        return Err("字符串没有结束".to_owned());
                    };
                    i += 1;
                    if ch == quote {
                        break;
                    }
                    if ch == '\\' {
                        let Some(&esc) = chars.get(i) else {
                            return Err("字符串没有结束".to_owned());
                        };
                        i += 1;
                        value.push(match esc {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '0' => '\0',
                            other => other,
                        });
                    } else {
                        value.push(ch);
                    }
                }
                if quote == '\'' {
                    let units: Vec<u16> = value.encode_utf16().collect();
                    match units.as_slice() {
                        [unit] => tokens.push(Token::Char(*unit)),
                        _ => return Err(format!("字符字面量 '{value}' 不是一个字符")),
                    }
                } else {
                    tokens.push(Token::Str(value));
                }
                continue;
            }
            '.' => tokens.push(Token::Dot),
            '[' => tokens.push(Token::LBracket),
            ']' => tokens.push(Token::RBracket),
            '(' => tokens.push(Token::LParen),
            ')' => tokens.push(Token::RParen),
            _ => {
                let next = chars.get(i + 1).copied();
                let (token, len) = match (c, next) {
                    ('=', Some('=')) => (Token::Op(BinOp::Eq), 2),
                    ('!', Some('=')) => (Token::Op(BinOp::Ne), 2),
                    ('<', Some('=')) => (Token::Op(BinOp::Le), 2),
                    ('>', Some('=')) => (Token::Op(BinOp::Ge), 2),
                    ('&', Some('&')) => (Token::Op(BinOp::And), 2),
                    ('|', Some('|')) => (Token::Op(BinOp::Or), 2),
                    ('<', _) => (Token::Op(BinOp::Lt), 1),
                    ('>', _) => (Token::Op(BinOp::Gt), 1),
                    ('!', _) => (Token::Not, 1),
                    _ => return Err(format!("不支持的字符 `{c}`")),
                };
                tokens.push(token);
                i += len;
                continue;
            }
        }
        i += 1;
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        token
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut lhs = self.and()?;
        while self.peek() == Some(&Token::Op(BinOp::Or)) {
            self.pos += 1;
            lhs = Expr::Binary(Box::new(lhs), BinOp::Or, Box::new(self.and()?));
        }
        Ok(lhs)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut lhs = self.comparison()?;
        while self.peek() == Some(&Token::Op(BinOp::And)) {
            self.pos += 1;
            lhs = Expr::Binary(Box::new(lhs), BinOp::And, Box::new(self.comparison()?));
        }
        Ok(lhs)
    }

    fn comparison(&mut self) -> Result<Expr, String> {
        let lhs = self.unary()?;
        if let Some(Token::Op(op)) = self.peek() {
            let op = *op;
            if !matches!(op, BinOp::And | BinOp::Or) {
                self.pos += 1;
                let rhs = self.unary()?;
                return Ok(Expr::Binary(Box::new(lhs), op, Box::new(rhs)));
            }
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Token::Not) {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Token::LParen) => {
                let expr = self.or()?;
                match self.next() {
                    Some(Token::RParen) => Ok(expr),
                    _ => Err("缺少 `)`".to_owned()),
                }
            }
            Some(Token::Int(n)) => Ok(Expr::Literal(Literal::Int(n))),
            Some(Token::Float(f)) => Ok(Expr::Literal(Literal::Float(f))),
            Some(Token::Str(s)) => Ok(Expr::Literal(Literal::Str(s))),
            Some(Token::Char(c)) => Ok(Expr::Literal(Literal::Char(c))),
            Some(Token::Ident(name)) => match name.as_str() {
                "true" => Ok(Expr::Literal(Literal::Bool(true))),
                "false" => Ok(Expr::Literal(Literal::Bool(false))),
                "null" => Ok(Expr::Literal(Literal::Null)),
                _ => {
                    let mut segments = Vec::new();
                    loop {
                        match self.peek() {
                            Some(Token::Dot) => {
                                self.pos += 1;
                                match self.next() {
                                    Some(Token::Ident(field)) => {
                                        segments.push(Segment::Field(field))
                                    }
                                    _ => return Err("`.` 后面应该是字段名".to_owned()),
                                }
                            }
                            Some(Token::LBracket) => {
                                self.pos += 1;
                                let index = match self.next() {
                                    Some(Token::Int(n)) => n,
                                    _ => return Err("数组下标只支持整数字面量".to_owned()),
                                };
                                if self.next() != Some(Token::RBracket) {
                                    return Err("缺少 `]`".to_owned());
                                }
                                segments.push(Segment::Index(index));
                            }
                            Some(Token::LParen) => {
                                return Err("不支持方法调用：只能求值变量和字段访问".to_owned())
                            }
                            _ => break,
                        }
                    }
                    Ok(Expr::Path(name, segments))
                }
            },
            Some(token) => Err(format!("意外的 {token:?}")),
            None => Err("表达式不完整".to_owned()),
        }
    }
}

pub fn parse(text: &str) -> Result<Expr, String> {
    let tokens = tokenize(text)?;
    if tokens.is_empty() {
        return Err("表达式为空".to_owned());
    }
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.or()?;
    if parser.pos < parser.tokens.len() {
        return Err(format!(
            "不支持的表达式：只能求值变量、字段访问链和简单比较（在 {:?} 处停下）",
            parser.tokens[parser.pos]
        ));
    }
    Ok(expr)
}

/// 断点命中次数条件：`5`（第 5 次）、`>= 5`、`> 5`、`< 5`、`<= 5`、`== 5`、`% 5`（每 5 次）。
pub fn hit_condition_met(condition: &str, hits: u64) -> Result<bool, String> {
    let condition = condition.trim();
    let (op, number) = ["%", ">=", "<=", "==", ">", "<", "="]
        .iter()
        .find_map(|op| condition.strip_prefix(op).map(|rest| (*op, rest)))
        .unwrap_or(("==", condition));
    let n: u64 = number
        .trim()
        .parse()
        .map_err(|_| format!("无法识别的命中次数条件 `{condition}`"))?;
    Ok(match op {
        "%" => n != 0 && hits.is_multiple_of(n),
        ">=" => hits >= n,
        "<=" => hits <= n,
        ">" => hits > n,
        "<" => hits < n,
        _ => hits == n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str, segments: Vec<Segment>) -> Expr {
        Expr::Path(name.to_owned(), segments)
    }

    #[test]
    fn parses_paths() {
        assert_eq!(parse("x").unwrap(), path("x", vec![]));
        assert_eq!(
            parse("this.a.b").unwrap(),
            path(
                "this",
                vec![Segment::Field("a".into()), Segment::Field("b".into())]
            )
        );
        assert_eq!(
            parse("arr[2].length").unwrap(),
            path(
                "arr",
                vec![Segment::Index(2), Segment::Field("length".into())]
            )
        );
        assert!(parse("foo()").is_err());
        assert!(parse("a +").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn parses_conditions() {
        let expr = parse("i == 2 && !done || name != \"x\"").unwrap();
        let Expr::Binary(lhs, BinOp::Or, rhs) = expr else {
            panic!("{expr:?}")
        };
        assert!(matches!(*lhs, Expr::Binary(_, BinOp::And, _)));
        assert_eq!(
            *rhs,
            Expr::Binary(
                Box::new(path("name", vec![])),
                BinOp::Ne,
                Box::new(Expr::Literal(Literal::Str("x".into())))
            )
        );
        assert_eq!(
            parse("x > -3").unwrap(),
            Expr::Binary(
                Box::new(path("x", vec![])),
                BinOp::Gt,
                Box::new(Expr::Literal(Literal::Int(-3)))
            )
        );
        assert_eq!(
            parse("(c == 'a')").unwrap(),
            Expr::Binary(
                Box::new(path("c", vec![])),
                BinOp::Eq,
                Box::new(Expr::Literal(Literal::Char('a' as u16)))
            )
        );
        assert_eq!(parse("1.5").unwrap(), Expr::Literal(Literal::Float(1.5)));
        assert_eq!(parse("10L").unwrap(), Expr::Literal(Literal::Int(10)));
    }

    #[test]
    fn hit_conditions() {
        assert!(hit_condition_met("3", 3).unwrap());
        assert!(!hit_condition_met("3", 4).unwrap());
        assert!(hit_condition_met(">= 2", 5).unwrap());
        assert!(hit_condition_met("% 2", 4).unwrap());
        assert!(!hit_condition_met("%2", 3).unwrap());
        assert!(hit_condition_met("<3", 2).unwrap());
        assert!(hit_condition_met("x", 1).is_err());
    }
}
