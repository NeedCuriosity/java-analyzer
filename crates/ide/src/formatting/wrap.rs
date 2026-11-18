//! 按右边界折行：超过 [`FormatOptions::max_line_length`] 的代码行在合适的位置断开。
//!
//! 只插入换行，不合并已有的行，也不改变 token 序列。可以断开的位置：
//! - 实参表 / 形参表 / 注解参数表的 `(` 之后和 `,` 之后，数组初始化器的 `,` 之后；
//! - 链式调用里 `)` 之后的 `.` 之前；
//! - 类型声明的 `extends` / `implements` / `permits`、方法的 `throws` 之前，以及这些列表的 `,` 之后；
//! - 三目运算符的 `?` / `:` 之前；二元运算符之后（或之前，见
//!   [`FormatOptions::binary_operation_sign_on_next_line`]）；
//! - 实在没有别的位置时，赋值的 `=` 之后。
//!
//! 一行里先在括号嵌套最浅的位置断开（同一层里逗号、链式调用优先于运算符），在这一组位置里
//! 尽量往后断（每行尽量填满）；断开后的行如果还超长，下一轮再往里一层断。断开之后的缩进交给
//! 普通的格式化（续行缩进），所以每一轮都重新解析、重排一次。

use ide_db::TextEdit;
use syntax::{ast, SyntaxKind, SyntaxToken, TextRange};

use super::{format_impl, FormatOptions};

/// 最多折几轮（每轮往里一层）；正常代码远用不到。
const MAX_ROUNDS: usize = 16;

/// 赋值 `=` 之后：只有一行里没有任何别的可断位置时才用。
const ASSIGNMENT: u8 = u8::MAX;

/// 格式化器动的空白字符；其余字符（包括注释、字面量里的）在格式化前后逐个对应。
fn is_layout_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// 第 `k` 个（从 0 起）非空白字符的 `(起点, 终点)`。
fn nth_significant(text: &str, k: usize) -> Option<(usize, usize)> {
    text.char_indices()
        .filter(|(_, c)| !is_layout_space(*c))
        .nth(k)
        .map(|(i, c)| (i, i + c.len_utf8()))
}

fn significant_before(text: &str, offset: usize) -> usize {
    text[..offset]
        .chars()
        .filter(|c| !is_layout_space(*c))
        .count()
}

/// 用非空白字符的序号表示的范围：格式化只改空白，序号在各轮之间不变。范围扩展到整行，
/// 这样被折开的行断出来的新行仍在范围里。
#[derive(Clone, Copy)]
pub(super) struct StableRange {
    start: usize,
    end: usize,
}

impl StableRange {
    pub(super) fn new(text: &str, range: TextRange) -> StableRange {
        let start = usize::from(range.start()).min(text.len());
        let end = usize::from(range.end()).min(text.len());
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
        StableRange {
            start: significant_before(text, line_start),
            end: significant_before(text, line_end),
        }
    }

    /// 在 `text` 里的范围：从第一个字符到最后一个字符的末尾。
    fn resolve(self, text: &str) -> TextRange {
        let start = nth_significant(text, self.start).map_or(text.len(), |(s, _)| s);
        let end = match self.end.checked_sub(1) {
            Some(last) => nth_significant(text, last).map_or(text.len(), |(_, e)| e),
            None => start,
        };
        TextRange::new((start as u32).into(), (end.max(start) as u32).into())
    }
}

fn apply(text: &str, mut edits: Vec<TextEdit>) -> String {
    edits.sort_by_key(|edit| std::cmp::Reverse((edit.range.start(), edit.range.end())));
    let mut out = text.to_owned();
    for edit in edits {
        out.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.new_text,
        );
    }
    out
}

/// 对已经格式化过的 `text` 折行；没有要折的行时返回 `None`。
pub(super) fn wrap_long_lines(
    text: &str,
    range: Option<StableRange>,
    options: &FormatOptions,
    max: usize,
) -> Option<String> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut current = text.to_owned();
    let mut changed = false;
    for _ in 0..MAX_ROUNDS {
        let root = ast::CompilationUnit::parse(&current).syntax_node();
        let breaks = line_breaks(
            &current,
            &root,
            range.map(|r| r.resolve(&current)),
            options,
            max,
            newline,
        );
        if breaks.is_empty() {
            break;
        }
        changed = true;
        let broken = apply(&current, breaks);
        let root = ast::CompilationUnit::parse(&broken).syntax_node();
        let edits = format_impl(
            &root,
            &broken,
            range.map(|r| r.resolve(&broken)),
            options,
            true,
        );
        current = apply(&broken, edits);
    }
    changed.then_some(current)
}

/// 把 `original` 改成 `formatted` 的修改：两者只有空白不同，逐段比较非空白字符之间的空白。
pub(super) fn whitespace_diff(original: &str, formatted: &str) -> Option<Vec<TextEdit>> {
    let mut edits = Vec::new();
    let mut a = original
        .char_indices()
        .filter(|(_, c)| !is_layout_space(*c));
    let mut b = formatted
        .char_indices()
        .filter(|(_, c)| !is_layout_space(*c));
    let (mut prev_a, mut prev_b) = (0usize, 0usize);
    loop {
        let (next_a, next_b) = (a.next(), b.next());
        let (gap_a, gap_b) = match (next_a, next_b) {
            (Some((i, ca)), Some((j, cb))) => {
                if ca != cb {
                    return None;
                }
                (prev_a..i, prev_b..j)
            }
            (None, None) => (prev_a..original.len(), prev_b..formatted.len()),
            _ => return None,
        };
        if original[gap_a.clone()] != formatted[gap_b.clone()] {
            edits.push(TextEdit::replace(
                TextRange::new((gap_a.start as u32).into(), (gap_a.end as u32).into()),
                formatted[gap_b].to_owned(),
            ));
        }
        match (next_a, next_b) {
            (Some((i, ca)), Some((j, _))) => {
                prev_a = i + ca.len_utf8();
                prev_b = j + ca.len_utf8();
            }
            _ => break,
        }
    }
    Some(edits)
}

/// 一个可以断开的位置：把 `gap`（两个 token 之间的空白）换成换行。
struct Break {
    gap: TextRange,
    /// 括号嵌套深度（相对行首，可以是负数）
    depth: i32,
    /// 同一深度里的优先级，越小越先断
    rank: u8,
    /// 断开处之前的文本结束在第几列
    col_before: usize,
    /// 断开处之后的 token 从第几列开始
    col_after: usize,
}

/// 这一轮要插入的换行。
fn line_breaks(
    text: &str,
    root: &syntax::SyntaxNode,
    range: Option<TextRange>,
    options: &FormatOptions,
    max: usize,
    newline: &str,
) -> Vec<TextEdit> {
    let tokens: Vec<SyntaxToken> = root
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| token.kind() != SyntaxKind::WHITESPACE)
        .collect();
    let mut edits = Vec::new();
    let mut next = 0usize;
    let mut line_start = 0usize;
    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let content_end = line_start + line.trim_end_matches(['\n', '\r']).len();
        while next < tokens.len() && usize::from(tokens[next].text_range().start()) < line_start {
            next += 1;
        }
        let first = next;
        let mut last = next;
        while last < tokens.len() && usize::from(tokens[last].text_range().start()) < content_end {
            last += 1;
        }
        let on_line = &tokens[first..last];
        // 行首在跨行的 token（块注释、文本块）里，或者行里有延续到下一行的 token：不动
        let starts_inside = first
            .checked_sub(1)
            .is_some_and(|prev| usize::from(tokens[prev].text_range().end()) > line_start);
        let ends_inside = on_line
            .last()
            .is_some_and(|token| usize::from(token.text_range().end()) > content_end);
        let in_range = range.is_none_or(|r| {
            r.intersect(TextRange::new(
                (line_start as u32).into(),
                (line_end as u32).into(),
            ))
            .is_some()
        });
        if !starts_inside && !ends_inside && in_range {
            for gap in breaks_in_line(text, line_start, on_line, options, max) {
                edits.push(TextEdit::replace(gap, newline.to_owned()));
            }
        }
        line_start = line_end;
    }
    edits
}

/// 一行里要断开的位置（空的表示不用断或断不了）。
fn breaks_in_line(
    text: &str,
    line_start: usize,
    tokens: &[SyntaxToken],
    options: &FormatOptions,
    max: usize,
) -> Vec<TextRange> {
    let Some(last_code) = tokens.iter().rev().find(|t| !t.kind().is_comment()) else {
        return Vec::new();
    };
    if matches!(
        tokens[0].kind(),
        SyntaxKind::IMPORT_KW | SyntaxKind::PACKAGE_KW
    ) {
        return Vec::new();
    }
    let col = |offset: usize| options.columns(&text[line_start..offset]);
    // 只看代码：行尾注释超出右边界不折
    let width = col(usize::from(last_code.text_range().end()));
    if width <= max {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let mut depth = 0i32;
    for pair in tokens.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        match a.kind() {
            SyntaxKind::L_PAREN | SyntaxKind::L_BRACK | SyntaxKind::L_CURLY => depth += 1,
            SyntaxKind::R_PAREN | SyntaxKind::R_BRACK | SyntaxKind::R_CURLY => depth -= 1,
            _ => {}
        }
        if a.kind().is_comment() || b.kind().is_comment() {
            continue;
        }
        if usize::from(b.text_range().start()) > usize::from(last_code.text_range().end()) {
            break;
        }
        let Some(rank) = break_rank(a, b, options) else {
            continue;
        };
        let gap = TextRange::new(a.text_range().end(), b.text_range().start());
        candidates.push(Break {
            gap,
            depth,
            rank,
            col_before: col(usize::from(gap.start())),
            col_after: col(usize::from(gap.end())),
        });
    }
    // 赋值只在没有别的位置时考虑
    if candidates.iter().any(|c| c.rank != ASSIGNMENT) {
        candidates.retain(|c| c.rank != ASSIGNMENT);
    }
    let Some(key) = candidates.iter().map(|c| (c.depth, c.rank)).min() else {
        return Vec::new();
    };
    let group: Vec<&Break> = candidates
        .iter()
        .filter(|c| (c.depth, c.rank) == key)
        .collect();

    let indent = col(usize::from(tokens[0].text_range().start()));
    // 断开后的行大约从这一列开始（续行缩进）；估计不准时下一轮会再折
    let continuation = indent + options.continuation();
    // 赋值右边整个挪到下一行也放不下时，断开没有用
    if key.1 == ASSIGNMENT
        && group
            .iter()
            .all(|c| continuation + width.saturating_sub(c.col_after) > max)
    {
        return Vec::new();
    }
    let mut chosen = Vec::new();
    // 当前输出行的内容在原行里从第几列开始，以及它在输出里从第几列开始
    let mut origin = indent;
    let mut base = indent;
    let mut last_fit: Option<usize> = None;
    let mut i = 0;
    while i < group.len() {
        let candidate = group[i];
        let end = base + candidate.col_before.saturating_sub(origin);
        if end <= max {
            last_fit = Some(i);
            i += 1;
            continue;
        }
        let at = last_fit.take().unwrap_or(i);
        chosen.push(group[at].gap);
        origin = group[at].col_after;
        base = continuation;
        if at == i {
            i += 1;
        }
    }
    if let Some(at) = last_fit {
        if base + width.saturating_sub(origin) > max {
            chosen.push(group[at].gap);
        }
    }
    chosen
}

fn parent_kind(token: &SyntaxToken) -> Option<SyntaxKind> {
    token.parent().map(|p| p.kind())
}

fn is_binary_operator(kind: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        kind,
        PIPE_PIPE
            | AMP_AMP
            | PIPE
            | CARET
            | AMP
            | EQ_EQ
            | NEQ
            | LT
            | GT
            | LT_EQ
            | GT_EQ
            | SHL
            | SHR
            | USHR
            | PLUS
            | MINUS
            | STAR
            | SLASH
            | PERCENT
    )
}

fn operator_rank(kind: SyntaxKind) -> u8 {
    match kind {
        SyntaxKind::PIPE_PIPE => 2,
        SyntaxKind::AMP_AMP => 3,
        _ => 4,
    }
}

/// 在 `a` 和 `b` 之间能不能断开，能的话同一深度里的优先级。
fn break_rank(a: &SyntaxToken, b: &SyntaxToken, options: &FormatOptions) -> Option<u8> {
    use SyntaxKind::*;
    let pa = parent_kind(a);
    let pb = parent_kind(b);
    let lists = |kind: Option<SyntaxKind>| {
        matches!(
            kind,
            Some(
                ARG_LIST
                    | PARAM_LIST
                    | ANNOTATION_ARG_LIST
                    | ARRAY_INITIALIZER
                    | THROWS_CLAUSE
                    | IMPLEMENTS_CLAUSE
                    | EXTENDS_CLAUSE
                    | PERMITS_CLAUSE
            )
        )
    };
    if a.kind() == COMMA && lists(pa) {
        return Some(0);
    }
    if a.kind() == L_PAREN
        && b.kind() != R_PAREN
        && matches!(pa, Some(ARG_LIST | PARAM_LIST | ANNOTATION_ARG_LIST))
    {
        return Some(0);
    }
    if b.kind() == DOT && a.kind() == R_PAREN && matches!(pb, Some(METHOD_CALL | FIELD_ACCESS)) {
        return Some(0);
    }
    if matches!(
        b.kind(),
        EXTENDS_KW | IMPLEMENTS_KW | THROWS_KW | PERMITS_KW
    ) && matches!(
        pb,
        Some(EXTENDS_CLAUSE | IMPLEMENTS_CLAUSE | THROWS_CLAUSE | PERMITS_CLAUSE)
    ) {
        return Some(0);
    }
    if matches!(b.kind(), QUESTION | COLON) && pb == Some(CONDITIONAL_EXPR) {
        return Some(1);
    }
    if options.binary_operation_sign_on_next_line {
        if pb == Some(BINARY_EXPR) && is_binary_operator(b.kind()) {
            return Some(operator_rank(b.kind()));
        }
    } else if pa == Some(BINARY_EXPR) && is_binary_operator(a.kind()) {
        return Some(operator_rank(a.kind()));
    }
    let assignment = match pa {
        Some(VARIABLE_DECLARATOR) => a.kind() == EQ,
        Some(ASSIGN_EXPR) => matches!(
            a.kind(),
            EQ | PLUS_EQ
                | MINUS_EQ
                | STAR_EQ
                | SLASH_EQ
                | AMP_EQ
                | PIPE_EQ
                | CARET_EQ
                | PERCENT_EQ
                | SHL_EQ
                | SHR_EQ
                | USHR_EQ
        ),
        _ => false,
    };
    // `int[] a = {...}` 的数组初始化器有自己的折法
    if assignment && b.kind() != L_CURLY {
        return Some(ASSIGNMENT);
    }
    None
}
