//! 签名修改的跟踪：记住每个方法 / 构造器在这一轮编辑之前的签名。
//!
//! 宿主在每次编辑应用到文本之前调用 [`SignatureTracker::record_edit`]。编辑第一次落在某个
//! 声明的签名头（返回类型 / 类型形参起，到形参列表的右括号止）上时，记下这个声明当时的签名头原文；
//! 之后的编辑只移动、伸缩记下的范围，不再改动原文。这样记下的总是"这一轮修改开始之前"的签名，
//! 中间存盘、格式化、改别处的代码都不影响它。
//!
//! 一个声明的记录在这些时候结束：按新签名更新了用法（宿主调用 [`SignatureTracker::accept`]），
//! 整段签名头被一次编辑整个盖掉（删除了方法、整体替换了文本），或者文件关闭。签名改回原样时
//! 记录还在，但新旧签名相同，不会提供更新用法的操作。

use std::sync::atomic::{AtomicU64, Ordering};

use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

/// 一个签名被改过的声明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedSignature {
    /// 全局唯一、递增的编号
    pub id: u64,
    /// 签名头在当前文本里的范围（随编辑移动）
    pub range: TextRange,
    /// 第一次被编辑之前的签名头原文
    pub old_header: String,
}

/// 一个文件里被改过签名的声明。
#[derive(Debug, Clone, Default)]
pub struct SignatureTracker {
    entries: Vec<TrackedSignature>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// 方法 / 构造器声明的签名头：从类型形参（或返回类型、名字）开始，到形参列表的右括号为止。
pub(crate) fn header_range(decl: &SyntaxNode) -> Option<TextRange> {
    let (start, params) = match decl.kind() {
        SyntaxKind::METHOD_DECL => {
            let method = ast::MethodDecl::cast(decl.clone())?;
            let start = method
                .type_param_list()
                .map(|t| t.syntax().text_range().start())
                .or_else(|| {
                    method
                        .return_type()
                        .map(|t| t.syntax().text_range().start())
                })
                .or_else(|| method.name().map(|n| n.syntax().text_range().start()))?;
            (start, method.param_list()?)
        }
        SyntaxKind::CONSTRUCTOR_DECL => {
            let ctor = ast::ConstructorDecl::cast(decl.clone())?;
            let start = ctor
                .type_param_list()
                .map(|t| t.syntax().text_range().start())
                .or_else(|| ctor.name().map(|n| n.syntax().text_range().start()))?;
            (start, ctor.param_list()?)
        }
        _ => return None,
    };
    let end = params.syntax().text_range().end();
    (start <= end).then(|| TextRange::new(start, end))
}

/// 签名头完整：形参列表有右括号，签名头里没有语法错误。写到一半的声明不记录。
fn header_is_complete(decl: &SyntaxNode, header: TextRange) -> bool {
    let closed = decl
        .children()
        .find(|n| n.kind() == SyntaxKind::PARAM_LIST)
        .and_then(|list| list.last_token())
        .is_some_and(|t| t.kind() == SyntaxKind::R_PAREN);
    closed
        && !decl
            .children()
            .filter(|n| header.contains_range(n.text_range()))
            .any(|n| {
                n.kind() == SyntaxKind::ERROR
                    || n.descendants().any(|d| d.kind() == SyntaxKind::ERROR)
            })
}

/// 一次编辑和一个范围的关系。
enum Relation {
    /// 编辑整个在范围之前：范围平移
    Before,
    /// 编辑整个在范围之后：范围不动
    After,
    /// 编辑盖住了整个范围还超出两侧：范围作废
    Covers,
    /// 编辑落在范围里或和它部分重叠：范围伸缩
    Touches,
}

fn relation(range: TextRange, edit: TextRange) -> Relation {
    if edit.end() < range.start() || (edit.end() == range.start() && !edit.is_empty()) {
        Relation::Before
    } else if edit.start() >= range.end() {
        Relation::After
    } else if edit.start() < range.start() && edit.end() > range.end() {
        Relation::Covers
    } else {
        Relation::Touches
    }
}

/// 把 `range` 映射到编辑之后的文本里；范围作废时为 `None`。
fn map_range(range: TextRange, edit: TextRange, insert_len: TextSize) -> Option<TextRange> {
    let delta = i64::from(u32::from(insert_len)) - i64::from(u32::from(edit.len()));
    let shift = |pos: TextSize| TextSize::from((i64::from(u32::from(pos)) + delta).max(0) as u32);
    let mapped = match relation(range, edit) {
        Relation::Before => TextRange::new(shift(range.start()), shift(range.end())),
        Relation::After => range,
        Relation::Covers => return None,
        Relation::Touches => {
            let start = range.start().min(edit.start());
            let end = shift(range.end().max(edit.end()));
            if end <= start {
                return None;
            }
            TextRange::new(start, end)
        }
    };
    (!mapped.is_empty()).then_some(mapped)
}

impl SignatureTracker {
    pub fn new() -> SignatureTracker {
        SignatureTracker::default()
    }

    pub fn entries(&self) -> &[TrackedSignature] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 下一条记录将得到的编号。宿主可以先记下它，之后用 [`SignatureTracker::forget_since`]
    /// 丢掉在这之后才出现的记录。
    pub fn next_id() -> u64 {
        NEXT_ID.load(Ordering::Relaxed)
    }

    /// 用法已经按新签名更新：结束这条记录，当前签名成为新的起点。
    pub fn accept(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    /// 丢掉编号不小于 `first_id` 的记录（应用重构本身改到的那些签名，例如覆写方法的形参列表）。
    pub fn forget_since(&mut self, first_id: u64) {
        self.entries.retain(|e| e.id < first_id);
    }

    /// 文本（语法树为 `root`）里的 `range` 即将被替换成长度为 `insert_len` 的新文本。
    pub fn record_edit(&mut self, root: &SyntaxNode, range: TextRange, insert_len: TextSize) {
        if root.text_range().contains_range(range) {
            self.capture(root, range);
        }
        self.entries = std::mem::take(&mut self.entries)
            .into_iter()
            .filter_map(|mut entry| {
                entry.range = map_range(entry.range, range, insert_len)?;
                Some(entry)
            })
            .collect();
    }

    /// 同 [`SignatureTracker::record_edit`]，自己解析 `text`。
    pub fn record_edit_in_text(&mut self, text: &str, range: TextRange, insert_len: TextSize) {
        let root = ast::CompilationUnit::parse(text).syntax_node();
        self.record_edit(&root, range, insert_len);
    }

    /// 从 `before` 改成 `after` 的一次编辑（取两者共同的前缀和后缀之间的部分）之后的记录。
    pub fn from_edit(before: &str, after: &str) -> SignatureTracker {
        let mut prefix = before
            .bytes()
            .zip(after.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let max_suffix = before.len().min(after.len()) - prefix;
        let mut suffix = before
            .bytes()
            .rev()
            .zip(after.bytes().rev())
            .take(max_suffix)
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(before.len() - suffix)
            || !after.is_char_boundary(after.len() - suffix)
        {
            suffix -= 1;
        }
        let range = TextRange::new(
            TextSize::from(prefix as u32),
            TextSize::from((before.len() - suffix) as u32),
        );
        let insert_len = TextSize::from((after.len() - suffix - prefix) as u32);
        let mut tracker = SignatureTracker::new();
        if before != after {
            tracker.record_edit_in_text(before, range, insert_len);
        }
        tracker
    }

    /// 编辑落在一个还没记录的声明的签名头上：记下它现在的签名头。
    fn capture(&mut self, root: &SyntaxNode, edit: TextRange) {
        let element = root.covering_element(edit);
        let start = match &element {
            syntax::NodeOrToken::Node(node) => Some(node.clone()),
            syntax::NodeOrToken::Token(token) => token.parent(),
        };
        let Some(decl) = start.into_iter().flat_map(|n| n.ancestors()).find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL
            )
        }) else {
            return;
        };
        let Some(header) = header_range(&decl) else {
            return;
        };
        if !matches!(relation(header, edit), Relation::Touches) {
            return;
        }
        let tracked = self
            .entries
            .iter()
            .any(|e| e.range.start() <= header.end() && header.start() <= e.range.end());
        if tracked || !header_is_complete(&decl, header) {
            return;
        }
        let text = root.text().to_string();
        let old_header = text[usize::from(header.start())..usize::from(header.end())].to_owned();
        self.entries.push(TrackedSignature {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            range: header,
            old_header,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(text: &str, needle: &str, replacement: &str) -> (TextRange, TextSize, String) {
        let start = text.find(needle).unwrap();
        let range = TextRange::new(
            TextSize::from(start as u32),
            TextSize::from((start + needle.len()) as u32),
        );
        let mut new_text = text.to_owned();
        new_text.replace_range(start..start + needle.len(), replacement);
        (range, TextSize::from(replacement.len() as u32), new_text)
    }

    #[test]
    fn records_the_signature_before_the_first_edit() {
        let text = "class A {\n    int f(int a) { return a; }\n    void g() {}\n}\n";
        let mut tracker = SignatureTracker::new();
        let (range, len, text2) = edit(text, "int a)", "int a, int b)");
        tracker.record_edit_in_text(text, range, len);
        assert_eq!(tracker.entries().len(), 1);
        assert_eq!(tracker.entries()[0].old_header, "int f(int a)");
        let tracked = tracker.entries()[0].range;
        assert_eq!(
            &text2[usize::from(tracked.start())..usize::from(tracked.end())],
            "int f(int a, int b)"
        );

        // 再改同一个签名：原文不变
        let (range, len, text3) = edit(&text2, "int b", "long b");
        tracker.record_edit_in_text(&text2, range, len);
        assert_eq!(tracker.entries().len(), 1);
        assert_eq!(tracker.entries()[0].old_header, "int f(int a)");

        // 改方法体、在前面加代码：范围跟着移动
        let (range, len, text4) = edit(&text3, "class A {\n", "class A {\n    int x;\n");
        tracker.record_edit_in_text(&text3, range, len);
        let tracked = tracker.entries()[0].range;
        assert_eq!(
            &text4[usize::from(tracked.start())..usize::from(tracked.end())],
            "int f(int a, long b)"
        );

        // 别的声明有了自己的记录
        let (range, len, _) = edit(&text4, "g()", "g(String s)");
        tracker.record_edit_in_text(&text4, range, len);
        assert_eq!(tracker.entries().len(), 2);
        assert_eq!(tracker.entries()[1].old_header, "void g()");

        let id = tracker.entries()[0].id;
        assert!(tracker.accept(id));
        assert_eq!(tracker.entries().len(), 1);
    }

    #[test]
    fn edits_outside_signatures_and_unfinished_declarations_are_ignored() {
        let text = "class A {\n    void f() { int x = 1; }\n}\n";
        let mut tracker = SignatureTracker::new();
        let (range, len, _) = edit(text, "int x = 1;", "int x = 2;");
        tracker.record_edit_in_text(text, range, len);
        assert!(tracker.is_empty());

        let broken = "class A {\n    void f(int { }\n}\n";
        let (range, len, _) = edit(broken, "int {", "int a) {");
        tracker.record_edit_in_text(broken, range, len);
        assert!(tracker.is_empty());
    }

    #[test]
    fn deleting_the_declaration_drops_the_record() {
        let text = "class A {\n    void f(int a) {}\n    void g() {}\n}\n";
        let mut tracker = SignatureTracker::from_edit(text, &text.replace("int a", "int b"));
        assert_eq!(tracker.entries().len(), 1);
        let text2 = text.replace("int a", "int b");
        let (range, len, _) = edit(&text2, "\n    void f(int b) {}", "");
        tracker.record_edit_in_text(&text2, range, len);
        assert!(tracker.is_empty());
    }

    #[test]
    fn from_edit_finds_the_changed_constructor() {
        let before = "class A {\n    A(int a) {}\n}\n";
        let after = "class A {\n    A(int a, String name) {}\n}\n";
        let tracker = SignatureTracker::from_edit(before, after);
        assert_eq!(tracker.entries().len(), 1);
        assert_eq!(tracker.entries()[0].old_header, "A(int a)");
    }
}
