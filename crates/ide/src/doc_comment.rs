//! 在声明上方输入 `/**` 后回车：生成 Javadoc 骨架（`@param`、`@return`、`@throws`）。
//!
//! 两种情况都处理：编辑器没有自动补上 `*/`（只有 `/**` 一行），或者已经补上了（`/**`、
//! 光标所在的空行、`*/` 三行，中间没有别的内容）。光标所在的行只补上 ` * `，其余的行插在
//! 下一行的开头，这样光标留在第一行文字的位置上。

use ide_db::TextEdit;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange, TextSize,
};

/// 声明需要的标签行（不含 ` * ` 前缀）。
fn tags_for(decl: &syntax::SyntaxNode) -> Vec<String> {
    let mut tags = Vec::new();
    let type_params = decl
        .children()
        .find_map(ast::TypeParamList::cast)
        .map(|l| {
            l.type_params()
                .filter_map(|p| p.name().map(|n| format!("@param <{}>", n.text())))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match decl.kind() {
        SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL => {
            if let Some(params) = decl.children().find_map(ast::ParamList::cast) {
                tags.extend(
                    params
                        .params()
                        .filter_map(|p| p.name().map(|n| format!("@param {}", n.text()))),
                );
            }
            tags.extend(type_params);
            if let Some(method) = ast::MethodDecl::cast(decl.clone()) {
                if method
                    .return_type()
                    .is_some_and(|t| t.syntax().text() != "void")
                {
                    tags.push("@return".to_owned());
                }
            }
            if let Some(throws) = decl.children().find_map(ast::ThrowsClause::cast) {
                tags.extend(
                    throws
                        .types()
                        .map(|t| format!("@throws {}", t.syntax().text())),
                );
            }
        }
        _ => {
            if let Some(decl) = ast::TypeDecl::cast(decl.clone()) {
                if let Some(components) = decl.record_components() {
                    tags.extend(
                        components
                            .params()
                            .filter_map(|p| p.name().map(|n| format!("@param {}", n.text()))),
                    );
                }
                tags.extend(type_params);
            }
        }
    }
    tags
}

pub(crate) fn javadoc_on_enter(text: &str, offset: TextSize) -> Option<Vec<TextEdit>> {
    let pos = usize::from(offset).min(text.len());
    let line_start = text[..pos].rfind('\n').map(|i| i + 1)?;
    let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    if !text[line_start..line_end].trim().is_empty() {
        return None;
    }
    // 上一行就是 `/**`
    let prev_start = text[..line_start - 1].rfind('\n').map_or(0, |i| i + 1);
    let prev = &text[prev_start..line_start - 1];
    if prev.trim() != "/**" {
        return None;
    }
    let indent: String = prev
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let next_start = (line_end + 1).min(text.len());
    let next_end = text[next_start..]
        .find('\n')
        .map_or(text.len(), |i| next_start + i);
    let closed = text[next_start..next_end].trim() == "*/";
    // 没有闭合时补上 `*/` 再解析，否则后面的代码都在注释里
    let (source, decl_search_from) = if closed {
        (text.to_owned(), next_end)
    } else {
        let mut fake = String::with_capacity(text.len() + 4);
        fake.push_str(&text[..pos]);
        fake.push_str(" */");
        fake.push_str(&text[pos..]);
        (fake, pos + 3)
    };
    let root = ast::CompilationUnit::parse(&source).syntax_node();
    let mut token = root
        .token_at_offset(TextSize::from(decl_search_from as u32))
        .right_biased()?;
    while token.kind().is_trivia() {
        token = token.next_token()?;
    }
    let decl = token.parent_ancestors().find(|n| {
        matches!(
            n.kind(),
            SyntaxKind::METHOD_DECL
                | SyntaxKind::CONSTRUCTOR_DECL
                | SyntaxKind::CLASS_DECL
                | SyntaxKind::INTERFACE_DECL
                | SyntaxKind::ENUM_DECL
                | SyntaxKind::RECORD_DECL
                | SyntaxKind::ANNOTATION_TYPE_DECL
                | SyntaxKind::FIELD_DECL
        )
    })?;
    // 声明得从这个 token 开始（注释紧挨在它前面）
    if decl.text_range().start() > token.text_range().start() {
        return None;
    }
    let tags = tags_for(&decl);
    let mut tail = String::new();
    if !tags.is_empty() {
        tail.push_str(&format!("{indent} *\n"));
        for tag in &tags {
            tail.push_str(&format!("{indent} * {tag}\n"));
        }
    }
    if !closed {
        tail.push_str(&format!("{indent} */\n"));
    }
    let current = TextEdit::replace(
        TextRange::new(
            TextSize::from(line_start as u32),
            TextSize::from(line_end as u32),
        ),
        format!("{indent} * "),
    );
    if tail.is_empty() {
        return Some(vec![current]);
    }
    if next_start >= text.len() || line_end >= text.len() {
        return None;
    }
    Some(vec![
        current,
        TextEdit::insert(TextSize::from(next_start as u32), tail),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, edits: Vec<TextEdit>) -> String {
        let mut out = text.to_owned();
        let mut edits = edits;
        edits.sort_by_key(|e| e.range.start());
        for edit in edits.into_iter().rev() {
            out.replace_range(
                usize::from(edit.range.start())..usize::from(edit.range.end()),
                &edit.new_text,
            );
        }
        out
    }

    fn check(before: &str, after: &str) {
        let offset = before.find('|').unwrap();
        let text = before.replace('|', "");
        let edits = javadoc_on_enter(&text, TextSize::from(offset as u32)).expect("no edits");
        assert_eq!(apply(&text, edits), after);
    }

    #[test]
    fn method_without_closing() {
        check(
            "class A {\n    /**\n    |\n    int sum(int a, int b) throws java.io.IOException {\n        return a + b;\n    }\n}\n",
            "class A {\n    /**\n     * \n     *\n     * @param a\n     * @param b\n     * @return\n     * @throws java.io.IOException\n     */\n    int sum(int a, int b) throws java.io.IOException {\n        return a + b;\n    }\n}\n",
        );
    }

    #[test]
    fn closed_by_editor() {
        check(
            "class A {\n    /**\n    |\n     */\n    <T> void put(T value) {}\n}\n",
            "class A {\n    /**\n     * \n     *\n     * @param value\n     * @param <T>\n     */\n    <T> void put(T value) {}\n}\n",
        );
    }

    #[test]
    fn class_and_record() {
        check(
            "/**\n|\nclass Box<T> {}\n",
            "/**\n * \n *\n * @param <T>\n */\nclass Box<T> {}\n",
        );
        check(
            "/**\n|\n*/\nrecord P(int x, int y) {}\n",
            "/**\n * \n *\n * @param x\n * @param y\n*/\nrecord P(int x, int y) {}\n",
        );
    }

    #[test]
    fn plain_field_and_non_doc() {
        check(
            "class A {\n    /**\n    |\n    int x;\n}\n",
            "class A {\n    /**\n     * \n     */\n    int x;\n}\n",
        );
        let text = "class A {\n    /*\n    \n    int x;\n}\n";
        assert!(javadoc_on_enter(text, TextSize::from(20)).is_none());
    }
}
