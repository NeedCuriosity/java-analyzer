//! 格式化：重排缩进、规整同一行里 token 之间的空格、压缩连续的空行、去掉行尾空白。
//!
//! 换行的位置保持原样（不会自动断行或合并行）。空格的规则见 [`spacing`]；缩进的规则：
//! - `{` 里的内容比它所属的声明 / 语句的起始行多缩进一级，`}` 与起始行对齐
//!   （形参表或条件折了行，`{` 落在续行上也一样）；
//! - `(` / `[` 里换行的内容、以及没写完的语句的续行：原来就比起始行缩进得多的，
//!   保持它相对起始行的偏移（与第一个实参对齐之类的写法不被打乱）；否则多缩进两级
//!   （Java 惯用的续行缩进）；
//! - 旧式 `case X:` 之后的语句比 `case` 多一级；
//! - 多行注释里以 `*` 开头的行与注释的第一行对齐；文本块的内容原样保留。

use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions {
    /// 一级缩进的文本（若干空格或一个制表符）
    pub indent_unit: String,
}

impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions {
            indent_unit: "    ".to_owned(),
        }
    }
}

/// 一行的缩进：若干级，外加续行对齐带来的零头（列数）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Indent {
    level: usize,
    extra: usize,
}

impl Indent {
    fn deeper(self, levels: usize) -> Indent {
        Indent {
            level: self.level + levels,
            extra: self.extra,
        }
    }

    fn render(self, unit: &str) -> String {
        let mut out = unit.repeat(self.level);
        out.extend(std::iter::repeat_n(' ', self.extra));
        out
    }
}

/// 一行重排后的缩进，以及它原来的缩进列数（原来的缩进里有制表符时为 `None`）。
#[derive(Debug, Clone, Copy, Default)]
struct LineInfo {
    indent: Indent,
    old_cols: Option<usize>,
}

/// 续行的缩进：原来就比 `anchor` 行缩进得多时保持这个偏移，否则多缩进两级。
fn continuation(anchor: LineInfo, old_cols: Option<usize>) -> Indent {
    match (anchor.old_cols, old_cols) {
        (Some(base), Some(cols)) if cols > base => Indent {
            level: anchor.indent.level,
            extra: anchor.indent.extra + (cols - base),
        },
        _ => anchor.indent.deeper(2),
    }
}

/// 一个尚未闭合的括号。
struct Open {
    kind: SyntaxKind,
    /// 括号所在的行
    line: LineInfo,
    /// `{`：`}` 应有的缩进，内容比它多一级。等于所属声明 / 语句起始行的缩进
    body: Indent,
    /// 当前是否处在旧式 `case X:` 之后（只对 `{` 有意义）
    in_case_body: bool,
    /// 括号打开时所在语句的起始行和括号深度，闭合后恢复
    statement: LineInfo,
    statement_depth: usize,
}

/// `{` 所属的声明 / 语句从哪里开始（跳过开头的注释）。裸的块、初始化块没有"所属"，返回 `None`。
fn owner_start(open: &SyntaxToken) -> Option<usize> {
    let body = open.parent()?;
    let owner = body.parent()?;
    let first = owner
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|t| t.kind() != SyntaxKind::WHITESPACE && !t.kind().is_comment())?;
    // 外层本身是一对花括号（块里的裸块、类体里的初始化块）：以 `{` 自己所在的行为准
    if first.kind() == SyntaxKind::L_CURLY {
        return None;
    }
    Some(usize::from(first.text_range().start()))
}

fn closing_of(kind: SyntaxKind) -> SyntaxKind {
    match kind {
        SyntaxKind::L_CURLY => SyntaxKind::R_CURLY,
        SyntaxKind::L_PAREN => SyntaxKind::R_PAREN,
        _ => SyntaxKind::R_BRACK,
    }
}

/// `token` 是否是一个独占一行的注解的结尾（`@Override` 之后换行不算续行）。
fn ends_annotation(token: &SyntaxToken) -> bool {
    token
        .parent_ancestors()
        .take_while(|n| n.text_range().end() == token.text_range().end())
        .any(|n| {
            n.kind() == SyntaxKind::ANNOTATION
                && n.parent()
                    .is_some_and(|p| p.kind() == SyntaxKind::MODIFIER_LIST)
        })
}

/// 一条语句 / 声明是否在 `prev` 处已经结束（下一行不是续行）。
fn terminates(prev: &SyntaxToken) -> bool {
    match prev.kind() {
        SyntaxKind::SEMICOLON | SyntaxKind::L_CURLY | SyntaxKind::R_CURLY => true,
        // 枚举常量、数组元素之间的逗号结束一项；`throws A,` / `implements A,` 后面是续行
        SyntaxKind::COMMA => !prev.parent().is_some_and(|p| {
            matches!(
                p.kind(),
                SyntaxKind::THROWS_CLAUSE
                    | SyntaxKind::IMPLEMENTS_CLAUSE
                    | SyntaxKind::EXTENDS_CLAUSE
                    | SyntaxKind::PERMITS_CLAUSE
            )
        }),
        // `case X:`、标签 `outer:`；三目运算符的 `:` 后面换行很少见，按续行处理
        SyntaxKind::COLON => prev.parent().is_some_and(|p| {
            matches!(
                p.kind(),
                SyntaxKind::SWITCH_LABEL | SyntaxKind::SWITCH_CASE | SyntaxKind::LABELED_STMT
            )
        }),
        _ => ends_annotation(prev),
    }
}

fn is_old_style_case(first: &SyntaxToken) -> Option<bool> {
    if !matches!(first.kind(), SyntaxKind::CASE_KW | SyntaxKind::DEFAULT_KW) {
        return None;
    }
    let case = first.parent_ancestors().find_map(ast::SwitchCase::cast)?;
    // 必须是这个 case 的开头，而不是 case 体里嵌套的别的东西
    if ast::support::first_token_in_subtree(case.syntax()).as_ref() != Some(first) {
        return None;
    }
    Some(!case.is_arrow())
}

pub(crate) fn format(
    root: &SyntaxNode,
    text: &str,
    range: Option<TextRange>,
    options: &FormatOptions,
) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    // 最近一个"不是续行"的行，以及当时的括号深度
    let mut statement = LineInfo::default();
    let mut statement_depth = 0usize;
    // 每个代码行重排后的缩进，按行首偏移升序
    let mut lines: Vec<(usize, LineInfo)> = Vec::new();
    let mut prev_significant: Option<SyntaxToken> = None;

    let mut tokens = root
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .peekable();

    let mut line_start = 0usize;
    let mut prev_blank = false;
    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        let content_end = line_start + content.len();
        let leading = content.len() - content.trim_start_matches([' ', '\t']).len();
        let first_offset = line_start + leading;
        let in_range = range.is_none_or(|r| {
            r.intersect(TextRange::new(
                (line_start as u32).into(),
                (line_end as u32).into(),
            ))
            .is_some()
        });

        // 跳过在这一行之前开始的 token，同时维护括号栈
        let mut inside_multiline: Option<SyntaxToken> = None;
        while let Some(token) = tokens.peek() {
            let start = usize::from(token.text_range().start());
            let end = usize::from(token.text_range().end());
            if start >= first_offset {
                break;
            }
            if end > first_offset {
                // 这一行的开头落在一个跨行的 token 里（块注释、文本块）
                inside_multiline = Some(token.clone());
                break;
            }
            tokens.next();
        }

        let is_blank = content.trim().is_empty();
        let mut new_indent: Option<String> = None;
        // 这一行按代码行处理时算出的缩进
        let mut current: Option<LineInfo> = None;
        let old_cols = text[line_start..first_offset]
            .bytes()
            .all(|b| b == b' ')
            .then_some(leading);

        if let Some(token) = &inside_multiline {
            if token.kind().is_comment() && content.trim_start().starts_with('*') {
                // 与注释第一行的 `/*` 对齐，再多一个空格
                let comment_start = usize::from(token.text_range().start());
                let comment_line = text[..comment_start].rfind('\n').map_or(0, |i| i + 1);
                let base: String = text[comment_line..comment_start].to_owned();
                if base.trim().is_empty() {
                    // 注释所在行如果在本次格式化里被重排过，以重排后的缩进为准
                    let level = comment_level(&edits, comment_line, &base);
                    new_indent = Some(format!("{level} "));
                }
            }
        } else if !is_blank {
            let first = tokens.peek().cloned();
            let indent = match &first {
                Some(first) => {
                    let depth = stack.len();
                    let closes_top = stack
                        .last()
                        .is_some_and(|top| first.kind() == closing_of(top.kind));
                    let old_case = is_old_style_case(first);
                    let base = match stack.last() {
                        None => Indent::default(),
                        Some(top) if closes_top => match top.kind {
                            SyntaxKind::L_CURLY => top.body,
                            _ => top.line.indent,
                        },
                        Some(top) => match top.kind {
                            SyntaxKind::L_CURLY => {
                                let extra = usize::from(top.in_case_body && old_case.is_none());
                                top.body.deeper(1 + extra)
                            }
                            _ => continuation(top.line, old_cols),
                        },
                    };
                    let in_brackets = stack
                        .last()
                        .is_some_and(|top| top.kind != SyntaxKind::L_CURLY);
                    let is_continuation = !first.kind().is_comment()
                        && !closes_top
                        && !in_brackets
                        && first.kind() != SyntaxKind::L_CURLY
                        && statement_depth == depth
                        && prev_significant
                            .as_ref()
                            .is_some_and(|prev| !terminates(prev));
                    let indent = if is_continuation {
                        continuation(statement, old_cols)
                    } else {
                        base
                    };
                    if !is_continuation {
                        statement = LineInfo { indent, old_cols };
                        statement_depth = depth;
                    }
                    if let (Some(old_case), Some(top)) = (old_case, stack.last_mut()) {
                        top.in_case_body = old_case;
                    }
                    indent
                }
                None => Indent::default(),
            };
            new_indent = Some(indent.render(&options.indent_unit));
            let info = LineInfo { indent, old_cols };
            lines.push((line_start, info));
            current = Some(info);
        }

        // 消费完全落在这一行上的 token，更新括号栈。延续到后面行的 token（块注释、文本块）
        // 留在原地，后面的行据此知道自己在它内部。
        if !is_blank {
            let line = current.unwrap_or(statement);
            while let Some(token) = tokens.peek() {
                if usize::from(token.text_range().start()) >= content_end
                    || usize::from(token.text_range().end()) > content_end
                {
                    break;
                }
                let token = tokens.next().expect("peek 过");
                match token.kind() {
                    SyntaxKind::L_CURLY | SyntaxKind::L_PAREN | SyntaxKind::L_BRACK => {
                        let body = if token.kind() == SyntaxKind::L_CURLY {
                            owner_start(&token)
                                .and_then(|start| {
                                    let at = lines.partition_point(|(s, _)| *s <= start);
                                    at.checked_sub(1).map(|i| lines[i].1.indent)
                                })
                                .unwrap_or(line.indent)
                        } else {
                            line.indent
                        };
                        stack.push(Open {
                            kind: token.kind(),
                            line,
                            body,
                            in_case_body: false,
                            statement,
                            statement_depth,
                        });
                    }
                    SyntaxKind::R_CURLY | SyntaxKind::R_PAREN | SyntaxKind::R_BRACK
                        // 只弹出配对的括号；残缺代码里多出来的右括号忽略
                        if stack
                            .last()
                            .is_some_and(|top| closing_of(top.kind) == token.kind())
                        => {
                            let closed = stack.pop().expect("刚检查过");
                            // 括号闭合后回到它所在语句的起始行
                            statement = closed.statement;
                            statement_depth = closed.statement_depth;
                        }
                    _ => {}
                }
                if !token.kind().is_trivia() {
                    prev_significant = Some(token);
                }
            }
        }

        // 连续的空行只留一行（跨行 token 内部的不动）
        let in_code = inside_multiline
            .as_ref()
            .is_none_or(|token| token.kind() == SyntaxKind::WHITESPACE);
        let extra_blank = is_blank && prev_blank && in_code;
        prev_blank = is_blank && in_code;
        if in_range && extra_blank && line_end > line_start {
            edits.push(TextEdit::replace(
                TextRange::new((line_start as u32).into(), (line_end as u32).into()),
                String::new(),
            ));
        } else if in_range {
            if is_blank {
                // 空行：去掉所有空白
                if !content.is_empty() && in_code {
                    edits.push(TextEdit::replace(
                        TextRange::new((line_start as u32).into(), (content_end as u32).into()),
                        String::new(),
                    ));
                }
            } else {
                if let Some(indent) = new_indent {
                    if text[line_start..first_offset] != indent {
                        edits.push(TextEdit::replace(
                            TextRange::new(
                                (line_start as u32).into(),
                                (first_offset as u32).into(),
                            ),
                            indent,
                        ));
                    }
                }
                // 行尾空白（跨行 token 内部的不动，例如文本块）
                let trimmed_end = line_start + content.trim_end_matches([' ', '\t']).len();
                let ends_inside_token = tokens.peek().is_some_and(|token| {
                    usize::from(token.text_range().start()) < trimmed_end
                        && usize::from(token.text_range().end()) > content_end
                        && !token.kind().is_comment()
                });
                if trimmed_end < content_end && !ends_inside_token {
                    edits.push(TextEdit::replace(
                        TextRange::new((trimmed_end as u32).into(), (content_end as u32).into()),
                        String::new(),
                    ));
                }
            }
        }
        line_start = line_end;
    }
    edits.extend(spacing_edits(root, text, range));
    edits.sort_by_key(|edit| (edit.range.start(), edit.range.end()));
    edits
}

/// 回车之后新的一行应有的缩进：在光标处放一个占位的语句，看格式化会把它缩进到哪里。
pub(crate) fn indent_for_new_line(
    text: &str,
    offset: TextSize,
    options: &FormatOptions,
) -> Option<TextEdit> {
    let pos = usize::from(offset).min(text.len());
    let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    let line = &text[line_start..line_end];
    // 只处理空行（刚回车出来的行）；有内容的行交给普通的格式化
    if !line.trim().is_empty() {
        return None;
    }
    const PLACEHOLDER: &str = "__ja_indent__;";
    let mut fake = String::with_capacity(text.len() + PLACEHOLDER.len());
    fake.push_str(&text[..line_start]);
    fake.push_str(PLACEHOLDER);
    fake.push_str(&text[line_end..]);
    let root = ast::CompilationUnit::parse(&fake).syntax_node();
    let fake_line = TextRange::at(
        (line_start as u32).into(),
        TextSize::from(PLACEHOLDER.len() as u32),
    );
    let start = TextSize::from(line_start as u32);
    let indent = format(&root, &fake, Some(fake_line), options)
        .into_iter()
        .find(|edit| edit.range.start() == start && edit.range.is_empty())
        .map(|edit| edit.new_text)
        .unwrap_or_default();
    (line != indent)
        .then(|| TextEdit::replace(TextRange::new(start, (line_end as u32).into()), indent))
}

// ---------------------------------------------------------------------------
// 空格
// ---------------------------------------------------------------------------

fn is_word(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::IDENT || kind.is_keyword() || kind.is_literal()
}

fn parent_kind(token: &SyntaxToken) -> Option<SyntaxKind> {
    token.parent().map(|p| p.kind())
}

/// 二元 / 赋值 / 三目运算符，以及 `->`、声明里的 `=`：两边各一个空格。
fn is_spaced_operator(token: &SyntaxToken) -> bool {
    use SyntaxKind::*;
    let kind = token.kind();
    if !kind.is_token() || is_word(kind) || kind.is_trivia() {
        return false;
    }
    match kind {
        ARROW | EQ => true,
        L_PAREN | R_PAREN | L_BRACK | R_BRACK | L_CURLY | R_CURLY | COMMA | SEMICOLON | DOT
        | AT | ELLIPSIS | COLON_COLON => false,
        COLON => matches!(
            parent_kind(token),
            Some(CONDITIONAL_EXPR | FOREACH_STMT | ASSERT_STMT)
        ),
        _ => matches!(
            parent_kind(token),
            Some(BINARY_EXPR | ASSIGN_EXPR | CONDITIONAL_EXPR)
        ),
    }
}

/// 同一行上相邻的两个 token 之间应不应该有空格。`None` 表示保持原样。
fn spacing(prev: &SyntaxToken, next: &SyntaxToken) -> Option<bool> {
    use SyntaxKind::*;
    let (p, n) = (prev.kind(), next.kind());
    let (pp, np) = (parent_kind(prev), parent_kind(next));

    // 残缺的代码不动
    if pp == Some(ERROR) || np == Some(ERROR) || p == ERROR_TOKEN || n == ERROR_TOKEN {
        return None;
    }
    // 泛型的尖括号
    let generic = |kind: Option<SyntaxKind>| matches!(kind, Some(TYPE_ARG_LIST | TYPE_PARAM_LIST));
    if n == LT && generic(np) {
        return Some(!(p == IDENT || p == DOT || p == COLON_COLON || p == GT));
    }
    if p == LT && generic(pp) {
        return Some(false);
    }
    if n == GT && generic(np) {
        return Some(false);
    }
    if p == GT && generic(pp) {
        // 显式类型实参 `this.<T>foo()`：方法名紧跟在后面
        let explicit_args = prev
            .parent()
            .and_then(|list| list.parent())
            .is_some_and(|owner| {
                matches!(
                    owner.kind(),
                    METHOD_CALL | METHOD_REF | EXPLICIT_CONSTRUCTOR_CALL
                )
            });
        if explicit_args {
            return Some(false);
        }
        return match n {
            IDENT | L_CURLY => Some(true),
            _ if n.is_keyword() => Some(true),
            L_PAREN | L_BRACK | GT | COMMA | R_PAREN | DOT | COLON_COLON | SEMICOLON | ELLIPSIS => {
                Some(false)
            }
            _ => None,
        };
    }

    match n {
        COMMA | R_PAREN | R_BRACK => return Some(false),
        // `for (;;)` 里连着的分号保持原样
        SEMICOLON => return (!matches!(p, SEMICOLON | L_PAREN)).then_some(false),
        DOT | COLON_COLON => return Some(false),
        ELLIPSIS => return Some(false),
        _ => {}
    }
    match p {
        L_PAREN | L_BRACK | DOT | COLON_COLON | AT => return Some(false),
        COMMA | ELLIPSIS => return Some(true),
        SEMICOLON => return Some(true),
        _ => {}
    }

    // 前缀 / 后缀运算符紧贴操作数
    if pp == Some(UNARY_EXPR) && !is_word(p) && p != R_PAREN {
        return Some(false);
    }
    if np == Some(POSTFIX_EXPR) && matches!(n, PLUS_PLUS | MINUS_MINUS) {
        return Some(false);
    }
    if is_spaced_operator(prev) || is_spaced_operator(next) {
        return Some(true);
    }

    match n {
        L_PAREN => {
            if matches!(np, Some(ARG_LIST | PARAM_LIST | ANNOTATION_ARG_LIST)) {
                // 记录的组件列表、方法的形参表、调用的实参表
                return Some(false);
            }
            if matches!(
                p,
                IF_KW
                    | FOR_KW
                    | WHILE_KW
                    | SWITCH_KW
                    | CATCH_KW
                    | SYNCHRONIZED_KW
                    | TRY_KW
                    | RETURN_KW
                    | THROW_KW
                    | YIELD_KW
                    | CASE_KW
                    | ASSERT_KW
                    | ELSE_KW
                    | DO_KW
            ) {
                return Some(true);
            }
            return if p == R_PAREN && pp == Some(CAST_EXPR) {
                Some(true)
            } else {
                None
            };
        }
        L_BRACK => {
            return matches!(
                np,
                Some(ARRAY_ACCESS | ARRAY_TYPE | DIM_EXPR | ARRAY_CREATION_EXPR)
            )
            .then_some(false);
        }
        L_CURLY => {
            // 数组初始化器和注解里的花括号保持原样
            return (!matches!(
                np,
                Some(ARRAY_INITIALIZER | ANNOTATION_ARG | ANNOTATION_ARG_LIST)
            ))
            .then_some(true);
        }
        COLON => {
            return matches!(np, Some(SWITCH_LABEL | SWITCH_CASE | LABELED_STMT)).then_some(false);
        }
        _ => {}
    }

    if p == R_CURLY {
        return matches!(n, ELSE_KW | CATCH_KW | FINALLY_KW | WHILE_KW).then_some(true);
    }
    if p == R_PAREN {
        // `(int) x`、`if (a) return;`、`foo() {`
        if pp == Some(CAST_EXPR) || is_word(n) {
            return Some(true);
        }
        return None;
    }
    if is_word(p) && (is_word(n) || n == AT) {
        return Some(true);
    }
    if p == R_BRACK && is_word(n) {
        return Some(true);
    }
    None
}

/// 规整同一行里 token 之间的空格。只动"两个 token 之间只有不含换行的空白"的地方。
fn spacing_edits(root: &SyntaxNode, text: &str, range: Option<TextRange>) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let mut prev: Option<SyntaxToken> = None;
    // prev 和当前 token 之间是否只有不含换行的空白
    let mut plain_gap = true;
    for token in root
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
    {
        let kind = token.kind();
        if kind == SyntaxKind::WHITESPACE {
            if token.text().contains('\n') {
                plain_gap = false;
            }
            continue;
        }
        if kind.is_comment() {
            // 注释前后的空格保持原样
            prev = None;
            continue;
        }
        if let (Some(before), true) = (&prev, plain_gap) {
            let gap = TextRange::new(before.text_range().end(), token.text_range().start());
            let in_range = range.is_none_or(|r| r.intersect(gap).is_some());
            if in_range {
                if let Some(space) = spacing(before, &token) {
                    let wanted = if space { " " } else { "" };
                    let current = &text[usize::from(gap.start())..usize::from(gap.end())];
                    if current != wanted {
                        edits.push(TextEdit::replace(gap, wanted.to_owned()));
                    }
                }
            }
        }
        prev = Some(token);
        plain_gap = true;
    }
    edits
}

/// 注释首行的缩进：如果它在本次格式化里已经被改写，取改写后的文本。
fn comment_level(edits: &[TextEdit], line_start: usize, original: &str) -> String {
    let start = TextSize::from(line_start as u32);
    edits
        .iter()
        .rev()
        .find(|edit| edit.range.start() == start)
        .map_or_else(|| original.to_owned(), |edit| edit.new_text.clone())
}

#[cfg(test)]
mod tests {
    use syntax::ast::CompilationUnit;

    use super::*;

    fn apply(text: &str, mut edits: Vec<TextEdit>) -> String {
        let mut out = text.to_owned();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start()));
        for edit in edits {
            out.replace_range(
                usize::from(edit.range.start())..usize::from(edit.range.end()),
                &edit.new_text,
            );
        }
        out
    }

    fn check(input: &str, expected: &str) {
        let root = CompilationUnit::parse(input).syntax_node();
        let edits = format(&root, input, None, &FormatOptions::default());
        assert_eq!(apply(input, edits), expected);
        // 已经格式化好的代码再格式化一遍不应有任何改动
        let root = CompilationUnit::parse(expected).syntax_node();
        let again = format(&root, expected, None, &FormatOptions::default());
        assert!(again.is_empty(), "不是幂等的: {again:?}");
    }

    #[test]
    fn reindents_blocks_and_strips_trailing_whitespace() {
        check(
            "class A {   \nint x;\n      void f() {\nif (x > 0) {\n            x++;\n}\n  \n}\n}\n",
            "class A {\n    int x;\n    void f() {\n        if (x > 0) {\n            x++;\n        }\n\n    }\n}\n",
        );
    }

    #[test]
    fn continuation_lines_and_wrapped_arguments() {
        check(
            "class A {\nvoid f() {\nString s = a\n+ b;\nfoo(1,\n2);\nlist.forEach(x -> {\nuse(x);\n});\n}\n}\n",
            "class A {\n    void f() {\n        String s = a\n                + b;\n        foo(1,\n                2);\n        list.forEach(x -> {\n            use(x);\n        });\n    }\n}\n",
        );
    }

    #[test]
    fn annotations_and_switch_cases() {
        check(
            "class A {\n@Override\npublic void f(int x) {\nswitch (x) {\ncase 1:\ng();\nbreak;\ncase 2 -> h();\ndefault:\nh();\n}\n}\n}\n",
            "class A {\n    @Override\n    public void f(int x) {\n        switch (x) {\n            case 1:\n                g();\n                break;\n            case 2 -> h();\n            default:\n                h();\n        }\n    }\n}\n",
        );
    }

    #[test]
    fn comments_and_text_blocks_are_preserved() {
        check(
            "class A {\n/**\n* Doc.\n*/\nString s = \"\"\"\n  keep\n    this\n  \"\"\";\n}\n",
            "class A {\n    /**\n     * Doc.\n     */\n    String s = \"\"\"\n  keep\n    this\n  \"\"\";\n}\n",
        );
    }

    #[test]
    fn range_formatting_touches_only_selected_lines() {
        let input = "class A {\nint x;\nint y;\n}\n";
        let root = CompilationUnit::parse(input).syntax_node();
        let second_line = TextRange::new(10.into(), 16.into());
        let edits = format(&root, input, Some(second_line), &FormatOptions::default());
        assert_eq!(apply(input, edits), "class A {\n    int x;\nint y;\n}\n");
    }

    #[test]
    fn normalizes_spaces_between_tokens() {
        check(
            "class A {\n    int f(int a,int b){\n        int x=a+b*2;\n        if(x>0&&a!=b){\n            x+=1;\n        }else{\n            x = -x;\n        }\n        for(int i=0;i<x;i++){\n            g( a , b );\n        }\n        String s=(String)null;\n        int[] arr=new int[ 3 ];\n        arr[ 0 ]=x++;\n        return a>b?a:b;\n    }\n}\n",
            "class A {\n    int f(int a, int b) {\n        int x = a + b * 2;\n        if (x > 0 && a != b) {\n            x += 1;\n        } else {\n            x = -x;\n        }\n        for (int i = 0; i < x; i++) {\n            g(a, b);\n        }\n        String s = (String) null;\n        int[] arr = new int[3];\n        arr[0] = x++;\n        return a > b ? a : b;\n    }\n}\n",
        );
    }

    #[test]
    fn spacing_leaves_generics_lambdas_and_annotations_readable() {
        let good = "import java.util.*;\n\n@SuppressWarnings(\"x\")\nclass A<T extends Comparable<T>> {\n    Map<String, List<T>> m = new HashMap<>();\n\n    <R> R f(List<? extends T> xs, String... rest) {\n        xs.forEach(x -> System.out.println(x));\n        Runnable r = () -> {};\n        Object o = (Runnable & java.io.Serializable) r;\n        int[] a = {1, 2};\n        outer:\n        for (;;) {\n            break outer;\n        }\n        switch (a.length) {\n            case 1 -> a[0]++;\n            default -> {}\n        }\n        return xs.isEmpty() ? null : this.<R>g(a[0] < 3, -1, !true);\n    }\n}\n";
        let root = CompilationUnit::parse(good).syntax_node();
        let edits = format(&root, good, None, &FormatOptions::default());
        assert_eq!(apply(good, edits), good);
    }

    #[test]
    fn collapses_repeated_blank_lines() {
        check(
            "class A {\n    int x;\n\n\n\n    int y;\n}\n",
            "class A {\n    int x;\n\n    int y;\n}\n",
        );
    }

    #[test]
    fn new_line_gets_the_indent_of_its_context() {
        let text = "class A {\n    void f() {\n        if (x) {\n\n        }\n        foo(a,\n\n    }\n}\n";
        let at = |needle: &str| TextSize::from((text.find(needle).unwrap() + needle.len()) as u32);
        let options = FormatOptions::default();
        let edit = indent_for_new_line(text, at("if (x) {\n"), &options).unwrap();
        assert_eq!(edit.new_text, "            ");
        let edit = indent_for_new_line(text, at("foo(a,\n"), &options).unwrap();
        assert_eq!(edit.new_text, "                ");
    }
}
