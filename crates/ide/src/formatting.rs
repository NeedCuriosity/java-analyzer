//! 格式化：重排缩进、规整同一行里 token 之间的空格、按设置保留 / 补足空行、去掉行尾空白；
//! 设置了花括号风格时，把 `{` 挪到上一行末尾或单独放一行。
//!
//! 超过右边界（[`FormatOptions::max_line_length`]）的代码行会被断开，见 [`wrap`]；已有的换行
//! 不会被合并。空格的规则见 [`spacing`]；缩进的规则：
//! - `{` 里的内容比它所属的声明 / 语句的起始行多缩进一级，`}` 与起始行对齐
//!   （形参表或条件折了行，`{` 落在续行上也一样）；
//! - `(` / `[` 里换行的内容、以及没写完的语句的续行：原来就比起始行缩进得多的，
//!   保持它相对起始行的偏移（与第一个实参对齐之类的写法不被打乱）；否则多一个续行缩进
//!   （默认两级）；
//! - 旧式 `case X:` 之后的语句比 `case` 多一级；
//! - 多行注释里以 `*` 开头的行与注释的第一行对齐；文本块的内容原样保留。
//!
//! 可调的部分见 [`FormatOptions`]，可以由 `.editorconfig` 设置（[`FormatOptions::with_editorconfig`]）。

mod wrap;

use ide_db::{editorconfig::EditorConfig, TextEdit};
use rustc_hash::FxHashMap;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize,
};

/// 一组从 `.editorconfig` 读的设置：字段名就是键名去掉 `ij_java_` 前缀。
macro_rules! editorconfig_options {
    ($(#[$meta:meta])* $name:ident: $ty:ty { $($field:ident = $default:expr,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name {
            $(pub $field: $ty,)*
        }

        impl Default for $name {
            fn default() -> Self {
                $name { $($field: $default,)* }
            }
        }

        impl $name {
            fn apply(&mut self, config: &EditorConfig) {
                $(
                    if let Some(value) =
                        config.parse::<$ty>(concat!("ij_java_", stringify!($field)))
                    {
                        self.$field = value;
                    }
                )*
            }
        }
    };
}

editorconfig_options! {
    /// 空行。`keep_blank_lines_*`：最多保留几个连续的空行（代码里、声明之间、`}` 之前）；
    /// `blank_lines_*`：类的成员之间（和类头之后）至少有几个空行，不够时补上，不会删。
    BlankLineOptions: usize {
        keep_blank_lines_in_code = 2,
        keep_blank_lines_in_declarations = 2,
        keep_blank_lines_before_right_brace = 2,
        blank_lines_after_class_header = 0,
        blank_lines_around_class = 1,
        blank_lines_around_field = 0,
        blank_lines_around_field_in_interface = 0,
        blank_lines_around_method = 1,
        blank_lines_around_method_in_interface = 1,
        blank_lines_around_initializer = 1,
    }
}

editorconfig_options! {
    /// 同一行里 token 之间要不要空格。
    SpacingOptions: bool {
        space_before_method_call_parentheses = false,
        space_before_method_parentheses = false,
        space_before_annotation_parameter_list = false,
        space_before_if_parentheses = true,
        space_before_for_parentheses = true,
        space_before_while_parentheses = true,
        space_before_switch_parentheses = true,
        space_before_try_parentheses = true,
        space_before_catch_parentheses = true,
        space_before_synchronized_parentheses = true,
        space_within_parentheses = false,
        space_within_method_call_parentheses = false,
        space_within_empty_method_call_parentheses = false,
        space_within_method_parentheses = false,
        space_within_empty_method_parentheses = false,
        space_within_if_parentheses = false,
        space_within_for_parentheses = false,
        space_within_while_parentheses = false,
        space_within_switch_parentheses = false,
        space_within_try_parentheses = false,
        space_within_catch_parentheses = false,
        space_within_synchronized_parentheses = false,
        space_within_cast_parentheses = false,
        space_within_annotation_parentheses = false,
        space_within_brackets = false,
        space_around_assignment_operators = true,
        space_around_logical_operators = true,
        space_around_equality_operators = true,
        space_around_relational_operators = true,
        space_around_bitwise_operators = true,
        space_around_additive_operators = true,
        space_around_multiplicative_operators = true,
        space_around_shift_operators = true,
        space_around_unary_operator = false,
        space_around_lambda_arrow = true,
        space_around_method_ref_dbl_colon = false,
        space_around_annotation_eq = true,
        space_before_quest = true,
        space_after_quest = true,
        space_before_colon = true,
        space_after_colon = true,
        space_before_colon_in_foreach = true,
        space_before_comma = false,
        space_after_comma = true,
        space_before_semicolon = false,
        space_after_semicolon = true,
        space_after_type_cast = true,
        space_before_class_left_brace = true,
        space_before_method_left_brace = true,
        space_before_if_left_brace = true,
        space_before_else_left_brace = true,
        space_before_for_left_brace = true,
        space_before_while_left_brace = true,
        space_before_do_left_brace = true,
        space_before_switch_left_brace = true,
        space_before_try_left_brace = true,
        space_before_catch_left_brace = true,
        space_before_finally_left_brace = true,
        space_before_synchronized_left_brace = true,
        space_before_else_keyword = true,
        space_before_catch_keyword = true,
        space_before_finally_keyword = true,
        space_before_while_keyword = true,
    }
}

/// `{` 放在哪里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BraceStyle {
    /// 声明 / 语句头的行尾
    EndOfLine,
    /// 单独一行，与声明 / 语句的起始行对齐
    NextLine,
    /// 声明 / 语句头折了行时单独一行，否则在行尾
    NextLineIfWrapped,
}

impl BraceStyle {
    fn parse(value: &str) -> Option<BraceStyle> {
        match value.to_ascii_lowercase().as_str() {
            "end_of_line" => Some(BraceStyle::EndOfLine),
            "next_line" => Some(BraceStyle::NextLine),
            "next_line_if_wrapped" => Some(BraceStyle::NextLineIfWrapped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions {
    /// 一级缩进的列数
    pub indent_size: usize,
    /// 一个制表符占的列数
    pub tab_width: usize,
    /// 缩进用制表符（凑不满一个制表符的零头用空格）
    pub use_tabs: bool,
    /// 续行缩进的列数；`None` 时为两级缩进
    pub continuation_indent_size: Option<usize>,
    /// `switch` 里的 `case` 比 `switch` 多缩进一级
    pub indent_case_from_switch: bool,
    /// 顶层类的成员不缩进
    pub do_not_indent_top_level_class_members: bool,
    pub blank_lines: BlankLineOptions,
    pub spacing: SpacingOptions,
    /// 类、接口、枚举、记录的 `{`；`None` 时保持原样
    pub class_brace_style: Option<BraceStyle>,
    /// 方法、构造器的 `{`；`None` 时保持原样
    pub method_brace_style: Option<BraceStyle>,
    /// 其余语句（`if`、`for`、`try`、`switch`……）的 `{`；`None` 时保持原样
    pub brace_style: Option<BraceStyle>,
    /// 去掉代码行末尾的空白
    pub trim_trailing_whitespace: bool,
    /// 文件末尾没有换行时补一个
    pub insert_final_newline: bool,
    /// 右边界（列数）：更长的代码行会被断开；`None` 时不折行
    pub max_line_length: Option<usize>,
    /// 二元运算符处断行时，运算符放在下一行的开头（否则留在上一行末尾）
    pub binary_operation_sign_on_next_line: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions {
            indent_size: 4,
            tab_width: 4,
            use_tabs: false,
            continuation_indent_size: None,
            indent_case_from_switch: true,
            do_not_indent_top_level_class_members: false,
            blank_lines: BlankLineOptions::default(),
            spacing: SpacingOptions::default(),
            class_brace_style: None,
            method_brace_style: None,
            brace_style: None,
            trim_trailing_whitespace: true,
            insert_final_newline: false,
            max_line_length: Some(120),
            binary_operation_sign_on_next_line: false,
        }
    }
}

impl FormatOptions {
    /// 叠加 `.editorconfig` 里的设置：设置了的键覆盖 `self`（通常来自编辑器）里的值。
    pub fn with_editorconfig(&self, config: &EditorConfig) -> FormatOptions {
        let mut options = self.clone();
        if config.is_empty() {
            return options;
        }
        match config
            .get("indent_style")
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("tab") => options.use_tabs = true,
            Some("space") => options.use_tabs = false,
            _ => {}
        }
        let positive = |n: &usize| *n > 0;
        let tab_width = config.parse::<usize>("tab_width").filter(positive);
        let indent_size = match config.get("indent_size") {
            Some(value) if value.eq_ignore_ascii_case("tab") => {
                Some(tab_width.unwrap_or(options.tab_width))
            }
            Some(_) => config.parse::<usize>("indent_size").filter(positive),
            None => None,
        };
        match (indent_size, tab_width) {
            (Some(indent), Some(tab)) => {
                options.indent_size = indent;
                options.tab_width = tab;
            }
            (Some(indent), None) => {
                options.indent_size = indent;
                options.tab_width = indent;
            }
            (None, Some(tab)) => {
                options.tab_width = tab;
                if options.use_tabs {
                    options.indent_size = tab;
                }
            }
            (None, None) => {}
        }
        if let Some(size) = config
            .parse::<usize>("ij_java_continuation_indent_size")
            .or_else(|| config.parse::<usize>("ij_continuation_indent_size"))
        {
            options.continuation_indent_size = Some(size);
        }
        if let Some(value) = config.parse::<bool>("ij_java_indent_case_from_switch") {
            options.indent_case_from_switch = value;
        }
        if let Some(value) = config.parse::<bool>("ij_java_do_not_indent_top_level_class_members") {
            options.do_not_indent_top_level_class_members = value;
        }
        options.blank_lines.apply(config);
        options.spacing.apply(config);
        let brace = |key: &str| config.get(key).and_then(BraceStyle::parse);
        if let Some(style) = brace("ij_java_brace_style") {
            options.brace_style = Some(style);
        }
        if let Some(style) = brace("ij_java_class_brace_style") {
            options.class_brace_style = Some(style);
        }
        if let Some(style) = brace("ij_java_method_brace_style") {
            options.method_brace_style = Some(style);
        }
        if let Some(value) = config.parse::<bool>("trim_trailing_whitespace") {
            options.trim_trailing_whitespace = value;
        }
        if let Some(value) = config.parse::<bool>("insert_final_newline") {
            options.insert_final_newline = value;
        }
        match config.get("max_line_length") {
            Some(value) if value.eq_ignore_ascii_case("off") => options.max_line_length = None,
            Some(_) => {
                if let Some(max) = config.parse::<usize>("max_line_length").filter(positive) {
                    options.max_line_length = Some(max);
                }
            }
            None => {}
        }
        if let Some(value) = config.parse::<bool>("ij_java_binary_operation_sign_on_next_line") {
            options.binary_operation_sign_on_next_line = value;
        }
        options
    }

    fn continuation(&self) -> usize {
        self.continuation_indent_size
            .unwrap_or(2 * self.indent_size)
    }

    fn tab_width(&self) -> usize {
        self.tab_width.max(1)
    }

    /// `cols` 列的缩进文本。
    fn render(&self, cols: usize) -> String {
        if self.use_tabs {
            let tab = self.tab_width();
            let mut out = "\t".repeat(cols / tab);
            out.extend(std::iter::repeat_n(' ', cols % tab));
            out
        } else {
            " ".repeat(cols)
        }
    }

    /// 缩进文本占的列数（制表符按 `tab_width` 对齐）。
    fn columns(&self, indent: &str) -> usize {
        let tab = self.tab_width();
        indent.chars().fold(0, |cols, c| {
            if c == '\t' {
                (cols / tab + 1) * tab
            } else {
                cols + 1
            }
        })
    }
}

/// 一行重排后的缩进（列数），以及它原来的缩进列数（还没有遇到任何代码行时为 `None`）。
#[derive(Debug, Clone, Copy, Default)]
struct LineInfo {
    indent: usize,
    old_cols: Option<usize>,
}

/// 续行的缩进：原来就比 `anchor` 行缩进得多时保持这个偏移，否则多一个续行缩进。
fn continuation(anchor: LineInfo, old_cols: usize, options: &FormatOptions) -> usize {
    match anchor.old_cols {
        Some(base) if old_cols > base => anchor.indent + (old_cols - base),
        _ => anchor.indent + options.continuation(),
    }
}

/// 一个尚未闭合的括号。
struct Open {
    kind: SyntaxKind,
    /// 括号所在的行
    line: LineInfo,
    /// `{`：`}` 应有的缩进，内容比它多 `inner` 级。等于所属声明 / 语句起始行的缩进
    body: usize,
    /// `{` 里的内容比 `body` 多缩进几级（通常是一级）
    inner: usize,
    /// `{` 是类体（其中的空行按"声明之间"算）
    declarations: bool,
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

fn is_type_decl(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::CLASS_DECL
            | SyntaxKind::INTERFACE_DECL
            | SyntaxKind::ENUM_DECL
            | SyntaxKind::RECORD_DECL
            | SyntaxKind::ANNOTATION_TYPE_DECL
    )
}

/// `{` 里的内容比 `{` 所属的行多缩进几级。
fn inner_levels(open: &SyntaxToken, options: &FormatOptions) -> usize {
    let Some(parent) = open.parent() else {
        return 1;
    };
    match parent.kind() {
        SyntaxKind::SWITCH_BLOCK if !options.indent_case_from_switch => 0,
        SyntaxKind::CLASS_BODY if options.do_not_indent_top_level_class_members => {
            let top_level = parent.parent().is_some_and(|owner| {
                is_type_decl(owner.kind())
                    && owner
                        .parent()
                        .is_some_and(|p| p.kind() == SyntaxKind::COMPILATION_UNIT)
            });
            usize::from(!top_level)
        }
        _ => 1,
    }
}

/// 一串连续的空行里的一行。
struct BlankLine {
    start: usize,
    end: usize,
    content_end: usize,
    in_range: bool,
}

/// 处理一串连续的空行：保留前 `target` 行（去掉其中的空白），删掉其余的。
fn flush_blank_run(run: &mut Vec<BlankLine>, target: usize, edits: &mut Vec<TextEdit>) {
    for (i, line) in run.drain(..).enumerate() {
        if !line.in_range {
            continue;
        }
        let range = if i < target {
            (line.content_end > line.start).then_some((line.start, line.content_end))
        } else {
            (line.end > line.start).then_some((line.start, line.end))
        };
        if let Some((start, end)) = range {
            edits.push(TextEdit::replace(
                TextRange::new((start as u32).into(), (end as u32).into()),
                String::new(),
            ));
        }
    }
}

pub(crate) fn format(
    root: &SyntaxNode,
    text: &str,
    range: Option<TextRange>,
    options: &FormatOptions,
) -> Vec<TextEdit> {
    let edits = format_impl(root, text, range, options, true);
    let Some(max) = options.max_line_length else {
        return edits;
    };
    let mut formatted = text.to_owned();
    let mut sorted = edits.clone();
    sorted.sort_by_key(|edit| std::cmp::Reverse((edit.range.start(), edit.range.end())));
    for edit in sorted {
        formatted.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.new_text,
        );
    }
    let stable = range.map(|r| wrap::StableRange::new(text, r));
    match wrap::wrap_long_lines(&formatted, stable, options, max) {
        Some(wrapped) => wrap::whitespace_diff(text, &wrapped).unwrap_or(edits),
        None => edits,
    }
}

/// 输入时的格式化：只重排缩进和空格，不补空行、不挪花括号。
pub(crate) fn format_while_typing(
    root: &SyntaxNode,
    text: &str,
    range: Option<TextRange>,
    options: &FormatOptions,
) -> Vec<TextEdit> {
    format_impl(root, text, range, options, false)
}

fn format_impl(
    root: &SyntaxNode,
    text: &str,
    range: Option<TextRange>,
    options: &FormatOptions,
    layout: bool,
) -> Vec<TextEdit> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let blank = &options.blank_lines;
    let required_blank = if layout {
        required_blank_lines(root, text, blank)
    } else {
        FxHashMap::default()
    };
    let mut edits = Vec::new();
    let mut brace_edits = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    // 最近一个"不是续行"的行，以及当时的括号深度
    let mut statement = LineInfo::default();
    let mut statement_depth = 0usize;
    // 每个代码行重排后的缩进，按行首偏移升序
    let mut lines: Vec<(usize, LineInfo)> = Vec::new();
    let mut prev_significant: Option<SyntaxToken> = None;
    let mut blank_run: Vec<BlankLine> = Vec::new();
    // 空行所在的上下文最多保留几个空行
    let keep_limit = |stack: &[Open]| match stack.last() {
        None => blank.keep_blank_lines_in_declarations,
        Some(top) if top.declarations => blank.keep_blank_lines_in_declarations,
        Some(_) => blank.keep_blank_lines_in_code,
    };

    let mut tokens = root
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .peekable();

    let mut line_start = 0usize;
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
        // 跨行 token（块注释、文本块）内部的行不算代码行
        let in_code = inside_multiline
            .as_ref()
            .is_none_or(|token| token.kind() == SyntaxKind::WHITESPACE);
        if is_blank {
            if in_code {
                blank_run.push(BlankLine {
                    start: line_start,
                    end: line_end,
                    content_end,
                    in_range,
                });
            }
            line_start = line_end;
            continue;
        }

        // 前面的一串空行：按上下文限制行数，这一行是类的成员时补足要求的空行数
        let first = if inside_multiline.is_none() {
            tokens.peek().cloned()
        } else {
            None
        };
        let limit = match &first {
            Some(first) if first.kind() == SyntaxKind::R_CURLY => {
                blank.keep_blank_lines_before_right_brace
            }
            _ => keep_limit(&stack),
        };
        let required = if first.is_some() {
            required_blank.get(&line_start).copied().unwrap_or(0)
        } else {
            0
        };
        let count = blank_run.len();
        let target = count.min(limit).max(required);
        flush_blank_run(&mut blank_run, target, &mut edits);
        let missing_blank = target.saturating_sub(count);

        let mut new_indent: Option<String> = None;
        // 这一行按代码行处理时算出的缩进
        let mut current: Option<LineInfo> = None;
        let old_cols = options.columns(&text[line_start..first_offset]);

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
        } else {
            let indent = match &first {
                Some(first) => {
                    let depth = stack.len();
                    let closes_top = stack
                        .last()
                        .is_some_and(|top| first.kind() == closing_of(top.kind));
                    let old_case = is_old_style_case(first);
                    let base = match stack.last() {
                        None => 0,
                        Some(top) if closes_top => match top.kind {
                            SyntaxKind::L_CURLY => top.body,
                            _ => top.line.indent,
                        },
                        Some(top) => match top.kind {
                            SyntaxKind::L_CURLY => {
                                let extra = usize::from(top.in_case_body && old_case.is_none());
                                top.body + (top.inner + extra) * options.indent_size
                            }
                            _ => continuation(top.line, old_cols, options),
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
                        continuation(statement, old_cols, options)
                    } else {
                        base
                    };
                    let info = LineInfo {
                        indent,
                        old_cols: Some(old_cols),
                    };
                    if !is_continuation {
                        statement = info;
                        statement_depth = depth;
                    }
                    if let (Some(old_case), Some(top)) = (old_case, stack.last_mut()) {
                        top.in_case_body = old_case;
                    }
                    indent
                }
                None => 0,
            };
            new_indent = Some(options.render(indent));
            let info = LineInfo {
                indent,
                old_cols: Some(old_cols),
            };
            lines.push((line_start, info));
            current = Some(info);
        }

        // 消费完全落在这一行上的 token，更新括号栈。延续到后面行的 token（块注释、文本块）
        // 留在原地，后面的行据此知道自己在它内部。
        let line_info = current.unwrap_or(statement);
        while let Some(token) = tokens.peek() {
            if usize::from(token.text_range().start()) >= content_end
                || usize::from(token.text_range().end()) > content_end
            {
                break;
            }
            let token = tokens.next().expect("peek 过");
            match token.kind() {
                SyntaxKind::L_CURLY | SyntaxKind::L_PAREN | SyntaxKind::L_BRACK => {
                    let is_curly = token.kind() == SyntaxKind::L_CURLY;
                    let body = if is_curly {
                        owner_start(&token)
                            .and_then(|start| {
                                let at = lines.partition_point(|(s, _)| *s <= start);
                                at.checked_sub(1).map(|i| lines[i].1.indent)
                            })
                            .unwrap_or(line_info.indent)
                    } else {
                        line_info.indent
                    };
                    if is_curly && layout {
                        brace_edits.extend(brace_edit(&token, text, range, options, body, newline));
                    }
                    stack.push(Open {
                        kind: token.kind(),
                        line: line_info,
                        body,
                        inner: if is_curly {
                            inner_levels(&token, options)
                        } else {
                            1
                        },
                        declarations: is_curly
                            && token
                                .parent()
                                .is_some_and(|p| p.kind() == SyntaxKind::CLASS_BODY),
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

        if in_range {
            if let Some(indent) = new_indent {
                if missing_blank > 0 || text[line_start..first_offset] != indent {
                    edits.push(TextEdit::replace(
                        TextRange::new((line_start as u32).into(), (first_offset as u32).into()),
                        format!("{}{indent}", newline.repeat(missing_blank)),
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
            if options.trim_trailing_whitespace && trimmed_end < content_end && !ends_inside_token {
                edits.push(TextEdit::replace(
                    TextRange::new((trimmed_end as u32).into(), (content_end as u32).into()),
                    String::new(),
                ));
            }
        }
        line_start = line_end;
    }
    // 文件末尾的空行
    let limit = keep_limit(&stack);
    let count = blank_run.len();
    flush_blank_run(&mut blank_run, count.min(limit), &mut edits);
    // 文件末尾补换行（最后一行是空白的，去掉空白之后已经以换行结尾）
    let last_line = text.rsplit('\n').next().unwrap_or_default();
    let end = TextSize::from(text.len() as u32);
    if layout
        && options.insert_final_newline
        && !last_line.trim().is_empty()
        && range.is_none_or(|r| r.contains_inclusive(end))
    {
        edits.push(TextEdit::insert(end, newline.to_owned()));
    }

    edits.extend(spacing_edits(root, text, range, &options.spacing));
    // 挪动花括号的修改覆盖了它前面的空白：这段空白里别的修改都作废
    if !brace_edits.is_empty() {
        edits.retain(|edit| {
            !brace_edits.iter().any(|brace: &TextEdit| {
                let gap = brace.range;
                (edit.range.start() >= gap.start() && edit.range.end() <= gap.end())
                    || (edit.range.start() < gap.end() && gap.start() < edit.range.end())
            })
        });
        edits.extend(brace_edits);
    }
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
    let indent = format_while_typing(&root, &fake, Some(fake_line), options)
        .into_iter()
        .find(|edit| edit.range.start() == start && edit.range.is_empty())
        .map(|edit| edit.new_text)
        .unwrap_or_default();
    (line != indent)
        .then(|| TextEdit::replace(TextRange::new(start, (line_end as u32).into()), indent))
}

// ---------------------------------------------------------------------------
// 空行
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Member {
    Field,
    Method,
    Class,
    Initializer,
}

fn member_kind(kind: SyntaxKind) -> Option<Member> {
    match kind {
        SyntaxKind::FIELD_DECL => Some(Member::Field),
        SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL => Some(Member::Method),
        SyntaxKind::INITIALIZER_BLOCK => Some(Member::Initializer),
        kind if is_type_decl(kind) => Some(Member::Class),
        _ => None,
    }
}

/// `offset` 所在行的行首。
fn line_start_of(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// `offset` 之前、同一行上只有空白。
fn starts_line(text: &str, offset: usize) -> bool {
    text[line_start_of(text, offset)..offset].trim().is_empty()
}

/// 成员从哪一行开始：紧挨在它前面、独占行首的注释（Javadoc 等）算成员的一部分。
/// 成员前面同一行上还有别的代码时返回 `None`。
fn member_start_line(member: &SyntaxNode, text: &str) -> Option<usize> {
    let first = member
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|t| !t.kind().is_trivia())?;
    let mut start = usize::from(first.text_range().start());
    let mut cur = first.prev_token();
    while let Some(token) = cur {
        match token.kind() {
            SyntaxKind::WHITESPACE => {
                // 隔着空行的注释不算这个成员的
                if token.text().matches('\n').count() >= 2 {
                    break;
                }
            }
            kind if kind.is_comment() => {
                let at = usize::from(token.text_range().start());
                if !starts_line(text, at) {
                    break;
                }
                start = at;
            }
            _ => break,
        }
        cur = token.prev_token();
    }
    starts_line(text, start).then(|| line_start_of(text, start))
}

/// 类的成员要求前面至少有几个空行：成员起始行的行首 → 空行数。
fn required_blank_lines(
    root: &SyntaxNode,
    text: &str,
    blank: &BlankLineOptions,
) -> FxHashMap<usize, usize> {
    let mut out = FxHashMap::default();
    for body in root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::CLASS_BODY)
    {
        let owner = body.parent().map(|o| o.kind());
        let interface = matches!(
            owner,
            Some(SyntaxKind::INTERFACE_DECL | SyntaxKind::ANNOTATION_TYPE_DECL)
        );
        let named = owner.is_some_and(is_type_decl);
        let around = |member: Member| match member {
            Member::Field if interface => blank.blank_lines_around_field_in_interface,
            Member::Field => blank.blank_lines_around_field,
            Member::Method if interface => blank.blank_lines_around_method_in_interface,
            Member::Method => blank.blank_lines_around_method,
            Member::Class => blank.blank_lines_around_class,
            Member::Initializer => blank.blank_lines_around_initializer,
        };
        let mut prev: Option<Member> = None;
        let mut first = true;
        for child in body.children() {
            let Some(member) = member_kind(child.kind()) else {
                // 枚举常量、残缺的代码：下一个成员前面不做要求
                prev = None;
                first = false;
                continue;
            };
            let required = match prev {
                Some(prev) => around(prev).max(around(member)),
                None if first && named => blank.blank_lines_after_class_header,
                None => 0,
            };
            prev = Some(member);
            first = false;
            if required > 0 {
                if let Some(line) = member_start_line(&child, text) {
                    out.insert(line, required);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 花括号的位置
// ---------------------------------------------------------------------------

/// 按花括号风格挪动 `{`：替换它和前一个 token 之间的空白。`body` 是 `{` 单独成行时的缩进。
fn brace_edit(
    open: &SyntaxToken,
    text: &str,
    range: Option<TextRange>,
    options: &FormatOptions,
    body: usize,
    newline: &str,
) -> Option<TextEdit> {
    use SyntaxKind::*;
    let parent = open.parent()?;
    let owner = parent.parent()?;
    let style = match (parent.kind(), owner.kind()) {
        (CLASS_BODY, kind) if is_type_decl(kind) => options.class_brace_style,
        (BLOCK, METHOD_DECL | CONSTRUCTOR_DECL) => options.method_brace_style,
        (
            BLOCK,
            IF_STMT | WHILE_STMT | DO_WHILE_STMT | FOR_STMT | FOREACH_STMT | TRY_STMT
            | CATCH_CLAUSE | FINALLY_CLAUSE | SYNCHRONIZED_STMT | INITIALIZER_BLOCK,
        )
        | (SWITCH_BLOCK, _)
        | (MODULE_BODY, _) => options.brace_style,
        _ => None,
    }?;
    // 裸块、实例初始化块没有可以跟在后面的头
    let header = owner_start(open)?;
    let open_at = open.text_range().start();
    if range.is_some_and(|r| !r.contains_inclusive(open_at)) {
        return None;
    }
    let mut prev = open.prev_token()?;
    while prev.kind() == WHITESPACE {
        prev = prev.prev_token()?;
    }
    // 中间隔着注释的不动
    if prev.kind().is_comment() || matches!(prev.kind(), SEMICOLON | L_CURLY | R_CURLY) {
        return None;
    }
    let gap = TextRange::new(prev.text_range().end(), open_at);
    let on_own_line = text[gap].contains('\n');
    let next_line = match style {
        BraceStyle::EndOfLine => false,
        BraceStyle::NextLine => true,
        BraceStyle::NextLineIfWrapped => {
            // 头从注解之后开始算：独占一行的注解不算折行
            let header = owner
                .descendants_with_tokens()
                .filter_map(|element| element.into_token())
                .find(|t| {
                    !t.kind().is_trivia()
                        && !t
                            .parent_ancestors()
                            .take_while(|n| n != &owner)
                            .any(|n| n.kind() == ANNOTATION)
                })
                .map_or(header, |t| usize::from(t.text_range().start()));
            text[header..usize::from(prev.text_range().end())].contains('\n')
        }
    };
    match (next_line, on_own_line) {
        (true, false) => Some(TextEdit::replace(
            gap,
            format!("{newline}{}", options.render(body)),
        )),
        (false, true) => {
            let space = spacing(&prev, open, &options.spacing).unwrap_or(true);
            Some(TextEdit::replace(
                gap,
                if space { " " } else { "" }.to_owned(),
            ))
        }
        _ => None,
    }
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

/// 运算符 `token` 前面（`before`）/ 后面要不要空格；不是运算符时为 `None`。
fn operator_space(token: &SyntaxToken, s: &SpacingOptions, before: bool) -> Option<bool> {
    use SyntaxKind::*;
    if !is_spaced_operator(token) {
        return None;
    }
    let kind = token.kind();
    let parent = parent_kind(token);
    Some(match kind {
        ARROW => parent != Some(LAMBDA_EXPR) || s.space_around_lambda_arrow,
        EQ if parent == Some(ANNOTATION_ARG) => s.space_around_annotation_eq,
        QUESTION if before => s.space_before_quest,
        QUESTION => s.space_after_quest,
        COLON => match parent {
            Some(CONDITIONAL_EXPR) if before => s.space_before_colon,
            Some(CONDITIONAL_EXPR) => s.space_after_colon,
            Some(FOREACH_STMT) if before => s.space_before_colon_in_foreach,
            _ => true,
        },
        _ if kind == EQ || parent == Some(ASSIGN_EXPR) => s.space_around_assignment_operators,
        AMP_AMP | PIPE_PIPE => s.space_around_logical_operators,
        EQ_EQ | NEQ => s.space_around_equality_operators,
        LT | GT | LT_EQ | GT_EQ => s.space_around_relational_operators,
        AMP | PIPE | CARET => s.space_around_bitwise_operators,
        PLUS | MINUS => s.space_around_additive_operators,
        STAR | SLASH | PERCENT => s.space_around_multiplicative_operators,
        SHL | SHR | USHR => s.space_around_shift_operators,
        _ => true,
    })
}

/// 一对圆括号里面靠着括号的地方要不要空格（`paren` 是 `(` 或 `)`；`empty`：括号里什么都没有）。
fn within_parens(paren: &SyntaxToken, s: &SpacingOptions, empty: bool) -> bool {
    use SyntaxKind::*;
    let Some(parent) = paren.parent() else {
        return false;
    };
    let declares = || {
        parent
            .parent()
            .is_some_and(|o| matches!(o.kind(), METHOD_DECL | CONSTRUCTOR_DECL | RECORD_DECL))
    };
    match parent.kind() {
        ARG_LIST if empty => s.space_within_empty_method_call_parentheses,
        ARG_LIST => s.space_within_method_call_parentheses,
        PARAM_LIST if declares() && empty => s.space_within_empty_method_parentheses,
        PARAM_LIST if declares() => s.space_within_method_parentheses,
        _ if empty => false,
        ANNOTATION_ARG_LIST => s.space_within_annotation_parentheses,
        CAST_EXPR => s.space_within_cast_parentheses,
        PAREN_EXPR => s.space_within_parentheses,
        IF_STMT => s.space_within_if_parentheses,
        WHILE_STMT | DO_WHILE_STMT => s.space_within_while_parentheses,
        FOR_STMT | FOREACH_STMT => s.space_within_for_parentheses,
        SWITCH_STMT | SWITCH_EXPR => s.space_within_switch_parentheses,
        SYNCHRONIZED_STMT => s.space_within_synchronized_parentheses,
        CATCH_CLAUSE => s.space_within_catch_parentheses,
        RESOURCE_LIST => s.space_within_try_parentheses,
        _ => false,
    }
}

/// 下标 / 数组维度的方括号里面要不要空格。
fn within_brackets(bracket: &SyntaxToken, s: &SpacingOptions) -> bool {
    matches!(
        parent_kind(bracket),
        Some(SyntaxKind::ARRAY_ACCESS | SyntaxKind::DIM_EXPR)
    ) && s.space_within_brackets
}

/// `{` 前面要不要空格（前一个 token 在同一行上）。
fn before_left_brace(prev: &SyntaxToken, open: &SyntaxToken, s: &SpacingOptions) -> bool {
    use SyntaxKind::*;
    let Some(parent) = open.parent() else {
        return true;
    };
    match parent.kind() {
        CLASS_BODY => s.space_before_class_left_brace,
        SWITCH_BLOCK => s.space_before_switch_left_brace,
        BLOCK => match parent.parent().map(|o| o.kind()) {
            Some(METHOD_DECL | CONSTRUCTOR_DECL) => s.space_before_method_left_brace,
            Some(IF_STMT) if prev.kind() == ELSE_KW => s.space_before_else_left_brace,
            Some(IF_STMT) => s.space_before_if_left_brace,
            Some(FOR_STMT | FOREACH_STMT) => s.space_before_for_left_brace,
            Some(WHILE_STMT) => s.space_before_while_left_brace,
            Some(DO_WHILE_STMT) => s.space_before_do_left_brace,
            Some(TRY_STMT) => s.space_before_try_left_brace,
            Some(CATCH_CLAUSE) => s.space_before_catch_left_brace,
            Some(FINALLY_CLAUSE) => s.space_before_finally_left_brace,
            Some(SYNCHRONIZED_STMT) => s.space_before_synchronized_left_brace,
            _ => true,
        },
        _ => true,
    }
}

/// 同一行上相邻的两个 token 之间应不应该有空格。`None` 表示保持原样。
fn spacing(prev: &SyntaxToken, next: &SyntaxToken, s: &SpacingOptions) -> Option<bool> {
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
        COMMA => return Some(s.space_before_comma),
        R_PAREN => return Some(within_parens(next, s, p == L_PAREN)),
        R_BRACK => return Some(p != L_BRACK && within_brackets(next, s)),
        // `for (;;)` 里连着的分号保持原样
        SEMICOLON => {
            return (!matches!(p, SEMICOLON | L_PAREN))
                .then_some(np == Some(FOR_STMT) && s.space_before_semicolon)
        }
        DOT | ELLIPSIS => return Some(false),
        COLON_COLON => return Some(s.space_around_method_ref_dbl_colon),
        _ => {}
    }
    match p {
        L_PAREN => return Some(within_parens(prev, s, false)),
        L_BRACK => return Some(within_brackets(prev, s)),
        DOT | AT => return Some(false),
        COLON_COLON => return Some(s.space_around_method_ref_dbl_colon),
        COMMA => return Some(s.space_after_comma),
        ELLIPSIS => return Some(true),
        SEMICOLON => return Some(pp != Some(FOR_STMT) || s.space_after_semicolon),
        _ => {}
    }

    // 连着的正负号（`- -x`）贴在一起会变成 `--`
    if (matches!(p, PLUS | MINUS) && matches!(n, PLUS | MINUS | PLUS_PLUS | MINUS_MINUS))
        || (matches!(p, PLUS_PLUS | MINUS_MINUS) && matches!(n, PLUS | MINUS))
    {
        return None;
    }
    // 前缀 / 后缀运算符紧贴操作数
    if pp == Some(UNARY_EXPR) && !is_word(p) && p != R_PAREN {
        return Some(s.space_around_unary_operator);
    }
    if np == Some(POSTFIX_EXPR) && matches!(n, PLUS_PLUS | MINUS_MINUS) {
        return Some(s.space_around_unary_operator);
    }
    let after = operator_space(prev, s, false);
    let before = operator_space(next, s, true);
    if after.is_some() || before.is_some() {
        return Some(after == Some(true) || before == Some(true));
    }

    match n {
        L_PAREN => {
            match np {
                // 调用的实参表（包括 `new`、枚举常量）
                Some(ARG_LIST) => return Some(s.space_before_method_call_parentheses),
                Some(ANNOTATION_ARG_LIST) => return Some(s.space_before_annotation_parameter_list),
                // 方法的形参表、记录的组件列表；lambda 的形参表保持原样
                Some(PARAM_LIST) => {
                    let lambda = next
                        .parent()
                        .and_then(|list| list.parent())
                        .is_some_and(|owner| owner.kind() == LAMBDA_EXPR);
                    return (!lambda).then_some(s.space_before_method_parentheses);
                }
                _ => {}
            }
            let keyword = match p {
                IF_KW => Some(s.space_before_if_parentheses),
                FOR_KW => Some(s.space_before_for_parentheses),
                WHILE_KW => Some(s.space_before_while_parentheses),
                SWITCH_KW => Some(s.space_before_switch_parentheses),
                CATCH_KW => Some(s.space_before_catch_parentheses),
                SYNCHRONIZED_KW => Some(s.space_before_synchronized_parentheses),
                TRY_KW => Some(s.space_before_try_parentheses),
                RETURN_KW | THROW_KW | YIELD_KW | CASE_KW | ASSERT_KW | ELSE_KW | DO_KW => {
                    Some(true)
                }
                _ => None,
            };
            if keyword.is_some() {
                return keyword;
            }
            return (p == R_PAREN && pp == Some(CAST_EXPR)).then_some(s.space_after_type_cast);
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
            if matches!(
                np,
                Some(ARRAY_INITIALIZER | ANNOTATION_ARG | ANNOTATION_ARG_LIST)
            ) {
                return None;
            }
            return Some(before_left_brace(prev, next, s));
        }
        COLON => {
            return matches!(np, Some(SWITCH_LABEL | SWITCH_CASE | LABELED_STMT)).then_some(false);
        }
        _ => {}
    }

    if p == R_CURLY {
        return match n {
            ELSE_KW => Some(s.space_before_else_keyword),
            CATCH_KW => Some(s.space_before_catch_keyword),
            FINALLY_KW => Some(s.space_before_finally_keyword),
            WHILE_KW => Some(s.space_before_while_keyword),
            _ => None,
        };
    }
    if p == R_PAREN {
        // `(int) x`、`if (a) return;`、`foo() {`
        if pp == Some(CAST_EXPR) {
            return Some(s.space_after_type_cast);
        }
        if is_word(n) {
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
fn spacing_edits(
    root: &SyntaxNode,
    text: &str,
    range: Option<TextRange>,
    options: &SpacingOptions,
) -> Vec<TextEdit> {
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
                if let Some(space) = spacing(before, &token, options) {
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
        .map_or_else(
            || original.to_owned(),
            // 改写时可能在前面补了空行
            |edit| {
                edit.new_text
                    .rsplit('\n')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            },
        )
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

    fn check_with(options: &FormatOptions, input: &str, expected: &str) {
        let root = CompilationUnit::parse(input).syntax_node();
        let edits = format(&root, input, None, options);
        assert_eq!(apply(input, edits), expected);
        // 已经格式化好的代码再格式化一遍不应有任何改动
        let root = CompilationUnit::parse(expected).syntax_node();
        let again = format(&root, expected, None, options);
        assert!(again.is_empty(), "不是幂等的: {again:?}");
    }

    fn check(input: &str, expected: &str) {
        check_with(&FormatOptions::default(), input, expected);
    }

    fn editorconfig(text: &str) -> FormatOptions {
        FormatOptions::default().with_editorconfig(&EditorConfig::from_texts([text]))
    }

    #[test]
    fn reindents_blocks_and_strips_trailing_whitespace() {
        check(
            "class A {   \nint x;\n      void f() {\nif (x > 0) {\n            x++;\n}\n  \n}\n}\n",
            "class A {\n    int x;\n\n    void f() {\n        if (x > 0) {\n            x++;\n        }\n\n    }\n}\n",
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
        let good = "import java.util.*;\n\n@SuppressWarnings(\"x\")\nclass A<T extends Comparable<T>> {\n    Map<String, List<T>> m = new HashMap<>();\n\n    <R> R f(List<? extends T> xs, String... rest) {\n        xs.forEach(x -> System.out.println(x));\n        Runnable r = () -> {};\n        Object o = (Runnable & java.io.Serializable) r;\n        int[] a = {1, 2};\n        int b = - -a[0];\n        outer:\n        for (;;) {\n            break outer;\n        }\n        switch (a.length) {\n            case 1 -> a[0]++;\n            default -> {}\n        }\n        return xs.isEmpty() ? null : this.<R>g(a[0] < 3, -1, !true);\n    }\n}\n";
        let root = CompilationUnit::parse(good).syntax_node();
        let edits = format(&root, good, None, &FormatOptions::default());
        assert_eq!(apply(good, edits), good);
    }

    #[test]
    fn keeps_at_most_two_blank_lines_by_default() {
        check(
            "class A {\n    int x;\n\n\n\n    int y;\n    void f() {\n        x++;\n\n\n\n        y++;\n\n\n\n    }\n}\n",
            "class A {\n    int x;\n\n\n    int y;\n\n    void f() {\n        x++;\n\n\n        y++;\n\n\n    }\n}\n",
        );
    }

    #[test]
    fn blank_line_limits_from_editorconfig() {
        let options = editorconfig(
            "[*.java]\nij_java_keep_blank_lines_in_code = 0\nij_java_keep_blank_lines_in_declarations = 1\nij_java_keep_blank_lines_before_right_brace = 0\n",
        );
        check_with(
            &options,
            "class A {\n    int x;\n\n\n    int y;\n\n    void f() {\n        x++;\n\n        y++;\n\n    }\n\n}\n",
            "class A {\n    int x;\n\n    int y;\n\n    void f() {\n        x++;\n        y++;\n    }\n}\n",
        );
    }

    #[test]
    fn adds_blank_lines_between_members() {
        // Javadoc 和注解跟着成员走；同一行上的成员不拆开
        check(
            "class A {\n    int x;\n    int y; int z;\n    /** Doc. */\n    @Deprecated\n    void f() {}\n    void g() {}\n    // h\n    void h() {}\n    class B {}\n}\ninterface I {\n    void a();\n    int B = 1;\n}\n",
            "class A {\n    int x;\n    int y; int z;\n\n    /** Doc. */\n    @Deprecated\n    void f() {}\n\n    void g() {}\n\n    // h\n    void h() {}\n\n    class B {}\n}\ninterface I {\n    void a();\n\n    int B = 1;\n}\n",
        );
        let options = editorconfig(
            "[*]\nij_java_blank_lines_after_class_header = 1\nij_java_blank_lines_around_field = 1\nij_java_blank_lines_around_method = 0\n",
        );
        check_with(
            &options,
            "class A {\n    int x;\n    int y;\n    void f() {}\n    void g() {}\n}\n",
            "class A {\n\n    int x;\n\n    int y;\n\n    void f() {}\n    void g() {}\n}\n",
        );
    }

    #[test]
    fn indentation_from_editorconfig() {
        let input = "class A {\nvoid f(int x) {\nswitch (x) {\ncase 1:\ng(1,\n2);\n}\n}\n}\n";
        let options = editorconfig(
            "[*.java]\nindent_style = tab\ntab_width = 4\nij_continuation_indent_size = 4\nij_java_indent_case_from_switch = false\n",
        );
        check_with(
            &options,
            input,
            "class A {\n\tvoid f(int x) {\n\t\tswitch (x) {\n\t\tcase 1:\n\t\t\tg(1,\n\t\t\t\t2);\n\t\t}\n\t}\n}\n",
        );
        let options = editorconfig(
            "[*]\nindent_size = 2\n[*.java]\nij_java_do_not_indent_top_level_class_members = true\n",
        );
        check_with(
            &options,
            "class A {\nvoid f() {\ng(1,\n2);\n}\nclass B {\nint x;\n}\n}\n",
            "class A {\nvoid f() {\n  g(1,\n      2);\n}\n\nclass B {\n  int x;\n}\n}\n",
        );
    }

    #[test]
    fn spacing_from_editorconfig() {
        let options = editorconfig(
            "[*.java]\nij_java_space_before_if_parentheses = false\nij_java_space_before_method_call_parentheses = true\nij_java_space_within_parentheses = true\nij_java_space_within_method_call_parentheses = true\nij_java_space_around_assignment_operators = false\nij_java_space_around_additive_operators = false\nij_java_space_after_comma = false\nij_java_space_before_method_left_brace = false\n",
        );
        check_with(
            &options,
            "class A {\n    void f(int a, int b) {\n        if (a > b) {\n            a = (a + b) * 2;\n            g(a, b);\n            g();\n        }\n    }\n}\n",
            "class A {\n    void f(int a,int b){\n        if(a > b) {\n            a=( a+b ) * 2;\n            g ( a,b );\n            g ();\n        }\n    }\n}\n",
        );
    }

    #[test]
    fn brace_styles_from_editorconfig() {
        let options = editorconfig(
            "[*.java]\nij_java_class_brace_style = next_line\nij_java_method_brace_style = next_line_if_wrapped\nij_java_brace_style = end_of_line\n",
        );
        check_with(
            &options,
            "class A {\n    @Override\n    void f() {\n        if (x)\n        {\n            y();\n        }\n        else\n        {\n        }\n    }\n\n    void g(int a,\n            int b) {\n    }\n}\n",
            "class A\n{\n    @Override\n    void f() {\n        if (x) {\n            y();\n        }\n        else {\n        }\n    }\n\n    void g(int a,\n            int b)\n    {\n    }\n}\n",
        );
    }

    #[test]
    fn final_newline_and_trailing_whitespace_from_editorconfig() {
        let options =
            editorconfig("[*]\ninsert_final_newline = true\ntrim_trailing_whitespace = false\n");
        check_with(
            &options,
            "class A {   \n    int x;\n}",
            "class A {   \n    int x;\n}\n",
        );
    }

    #[test]
    fn editorconfig_overrides_client_indent() {
        let client = FormatOptions {
            indent_size: 2,
            tab_width: 2,
            use_tabs: true,
            ..FormatOptions::default()
        };
        let options = client.with_editorconfig(&EditorConfig::from_texts([
            "[*.java]\nindent_style = space\nindent_size = 3\n",
        ]));
        assert_eq!(
            (options.indent_size, options.tab_width, options.use_tabs),
            (3, 3, false)
        );
        assert_eq!(options.continuation(), 6);
        // 没有设置缩进的 `.editorconfig` 不影响编辑器给的值
        let options = client.with_editorconfig(&EditorConfig::from_texts([
            "[*.java]\nij_java_keep_blank_lines_in_code = 1\n",
        ]));
        assert_eq!((options.indent_size, options.use_tabs), (2, true));
        assert_eq!(options.blank_lines.keep_blank_lines_in_code, 1);
    }

    fn margin(max: usize) -> FormatOptions {
        FormatOptions {
            max_line_length: Some(max),
            ..FormatOptions::default()
        }
    }

    fn method(body: &str) -> String {
        format!("class A {{\n    void f() {{\n{body}    }}\n}}\n")
    }

    #[test]
    fn wraps_long_argument_lists_filling_each_line() {
        check_with(
            &margin(50),
            &method("        foo(alpha, beta, gamma, delta, epsilon, zeta);\n"),
            &method("        foo(alpha, beta, gamma, delta, epsilon,\n                zeta);\n"),
        );
        // 第一个实参就放不下时在 `(` 之后断开
        check_with(
            &margin(40),
            &method("        someObject.doSomethingVeryLong(argumentOne, argumentTwo);\n"),
            &method("        someObject.doSomethingVeryLong(\n                argumentOne,\n                argumentTwo);\n"),
        );
    }

    #[test]
    fn wraps_method_chains_before_the_dot() {
        check_with(
            &margin(60),
            &method("        String s = list.stream().filter(x -> x.isEmpty()).map(x -> x.trim()).collect(toList());\n"),
            &method("        String s = list.stream().filter(x -> x.isEmpty())\n                .map(x -> x.trim()).collect(toList());\n"),
        );
    }

    #[test]
    fn wraps_binary_expressions_at_the_loosest_operator() {
        let input = method(
            "        boolean ok = first != null && second != null && third != null && fourth;\n",
        );
        check_with(
            &margin(50),
            &input,
            &method("        boolean ok = first != null &&\n                second != null && third != null &&\n                fourth;\n"),
        );
        let options = FormatOptions {
            binary_operation_sign_on_next_line: true,
            ..margin(50)
        };
        check_with(
            &options,
            &input,
            &method("        boolean ok = first != null\n                && second != null && third != null\n                && fourth;\n"),
        );
    }

    #[test]
    fn wraps_throws_and_implements_before_parameters() {
        check_with(
            &margin(60),
            "class A {\n    public void handle(String request, int code) throws IOException, InterruptedException {\n        body();\n    }\n}\n",
            "class A {\n    public void handle(String request, int code)\n            throws IOException, InterruptedException {\n        body();\n    }\n}\n",
        );
        check_with(
            &margin(50),
            "public class LongName extends BaseClass implements First, Second {\n}\n",
            "public class LongName extends BaseClass\n        implements First, Second {\n}\n",
        );
    }

    #[test]
    fn leaves_short_lines_unbreakable_lines_and_manual_wraps_alone() {
        let text = method(
            "        foo(a,\n                b);\n        String s = \"a string literal that is far too long to fit\";\n        int x = 1; // a trailing comment that goes past the right margin\n",
        );
        check_with(&margin(40), &text, &text);
        // 关掉折行
        let options = editorconfig("[*.java]\nmax_line_length = off\n");
        assert_eq!(options.max_line_length, None);
        let long = method("        foo(alpha, beta, gamma, delta, epsilon, zeta, eta, theta, iota, kappa, lambda, mu, nu, xi);\n");
        check_with(&options, &long, &long);
        assert_eq!(
            editorconfig("[*.java]\nmax_line_length = 100\n").max_line_length,
            Some(100)
        );
    }

    #[test]
    fn range_formatting_wraps_only_selected_lines() {
        let line = "        foo(alpha, beta, gamma, delta, epsilon, zeta);\n";
        let text = method(&format!("{line}{line}"));
        let second = text.rfind("foo").unwrap();
        let range = TextRange::new(
            TextSize::from(second as u32),
            TextSize::from((second + 10) as u32),
        );
        let root = CompilationUnit::parse(&text).syntax_node();
        let edits = format(&root, &text, Some(range), &margin(50));
        assert_eq!(
            apply(&text, edits),
            method(&format!(
                "{line}        foo(alpha, beta, gamma, delta, epsilon,\n                zeta);\n"
            ))
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
