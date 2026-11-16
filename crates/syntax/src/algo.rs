//! 语法树上的通用小算法。

use rowan::{TextRange, TextSize};

use crate::{AstNode, SyntaxKind, SyntaxNode, SyntaxToken};

/// 偏移处的 token；落在两个 token 之间时取 `priority` 较高的那个。
pub fn pick_token(
    syntax: &SyntaxNode,
    offset: TextSize,
    priority: impl Fn(SyntaxKind) -> u32,
) -> Option<SyntaxToken> {
    syntax
        .token_at_offset(offset)
        .max_by_key(|t| priority(t.kind()))
}

/// 偏移处最适合做"光标下符号"的 token：标识符优先，其次关键字和字面量，空白最低。
pub fn pick_ident_token(syntax: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    pick_token(syntax, offset, |kind| match kind {
        SyntaxKind::IDENT => 4,
        SyntaxKind::THIS_KW | SyntaxKind::SUPER_KW | SyntaxKind::NEW_KW => 3,
        k if k.is_literal() => 2,
        k if k.is_trivia() => 0,
        _ => 1,
    })
}

/// 从偏移处的 token 向上的所有祖先节点（由内到外）。
pub fn ancestors_at_offset(
    syntax: &SyntaxNode,
    offset: TextSize,
) -> impl Iterator<Item = SyntaxNode> {
    pick_ident_token(syntax, offset)
        .and_then(|t| t.parent())
        .into_iter()
        .flat_map(|n| n.ancestors())
}

/// 包含偏移处 token 的、最内层的 `N` 节点。
pub fn find_node_at_offset<N: AstNode>(syntax: &SyntaxNode, offset: TextSize) -> Option<N> {
    ancestors_at_offset(syntax, offset).find_map(N::cast)
}

/// 完全覆盖 `range` 的最内层 `N` 节点。
pub fn find_node_at_range<N: AstNode>(syntax: &SyntaxNode, range: TextRange) -> Option<N> {
    let element = syntax.covering_element(range);
    let node = match element {
        rowan::NodeOrToken::Node(node) => node,
        rowan::NodeOrToken::Token(token) => token.parent()?,
    };
    node.ancestors().find_map(N::cast)
}

/// 跳过空白和注释的下一个 token。
pub fn next_non_trivia_token(token: &SyntaxToken) -> Option<SyntaxToken> {
    let mut cur = token.next_token();
    while let Some(t) = cur {
        if !t.kind().is_trivia() {
            return Some(t);
        }
        cur = t.next_token();
    }
    None
}

/// 跳过空白和注释的上一个 token。
pub fn prev_non_trivia_token(token: &SyntaxToken) -> Option<SyntaxToken> {
    let mut cur = token.prev_token();
    while let Some(t) = cur {
        if !t.kind().is_trivia() {
            return Some(t);
        }
        cur = t.prev_token();
    }
    None
}

/// 偏移之前（含紧贴偏移）的最后一个非空白 token。
pub fn non_trivia_token_before(syntax: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    let token = match syntax.token_at_offset(offset) {
        rowan::TokenAtOffset::None => return None,
        rowan::TokenAtOffset::Single(t) => t,
        rowan::TokenAtOffset::Between(left, _) => left,
    };
    if token.kind().is_trivia() || token.text_range().start() >= offset {
        prev_non_trivia_token(&token)
    } else {
        Some(token)
    }
}

/// 节点去掉某些种类的直接子节点之后的绿树（例如去掉方法体、类体只留声明头）。
/// 结果和节点在文件里的位置无关，被去掉的部分里的编辑也不会改变它。
pub fn green_without_children(node: &SyntaxNode, kinds: &[SyntaxKind]) -> rowan::GreenNode {
    use rowan::{Language, NodeOrToken};
    let green = node.green();
    let children: Vec<_> = green
        .children()
        .filter_map(|child| match child {
            NodeOrToken::Node(n) => {
                let kind = crate::JavaLanguage::kind_from_raw(n.kind());
                (!kinds.contains(&kind)).then(|| NodeOrToken::Node(n.to_owned()))
            }
            NodeOrToken::Token(t) => Some(NodeOrToken::Token(t.to_owned())),
        })
        .collect();
    rowan::GreenNode::new(green.kind(), children)
}
