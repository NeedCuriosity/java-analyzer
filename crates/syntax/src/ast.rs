//! 类型化 AST：对 `SyntaxNode` 的零成本包装。
//!
//! 所有访问器都返回 `Option` / 迭代器，因为语法树可能来自残缺的代码。

mod nodes;

use std::marker::PhantomData;

pub use self::nodes::*;
use crate::{SyntaxKind, SyntaxNode, SyntaxNodeChildren, SyntaxToken};

pub trait AstNode: Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(syntax: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
}

/// 某种类型的直接子节点的迭代器。
#[derive(Debug, Clone)]
pub struct AstChildren<N> {
    inner: SyntaxNodeChildren,
    _ty: PhantomData<N>,
}

impl<N> AstChildren<N> {
    fn new(parent: &SyntaxNode) -> Self {
        AstChildren {
            inner: parent.children(),
            _ty: PhantomData,
        }
    }
}

impl<N: AstNode> Iterator for AstChildren<N> {
    type Item = N;

    fn next(&mut self) -> Option<N> {
        self.inner.find_map(N::cast)
    }
}

pub mod support {
    use super::{AstChildren, AstNode};
    use crate::{SyntaxKind, SyntaxNode, SyntaxToken};

    pub fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
        parent.children().find_map(N::cast)
    }

    pub fn nth_child<N: AstNode>(parent: &SyntaxNode, n: usize) -> Option<N> {
        parent.children().filter_map(N::cast).nth(n)
    }

    pub fn children<N: AstNode>(parent: &SyntaxNode) -> AstChildren<N> {
        AstChildren::new(parent)
    }

    pub fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
        parent
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|it| it.kind() == kind)
    }

    pub fn has_token(parent: &SyntaxNode, kind: SyntaxKind) -> bool {
        token(parent, kind).is_some()
    }

    pub fn count_tokens(parent: &SyntaxNode, kind: SyntaxKind) -> usize {
        parent
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .filter(|it| it.kind() == kind)
            .count()
    }

    /// 第一个非空白、非注释的直接子 token。
    pub fn first_significant_token(parent: &SyntaxNode) -> Option<SyntaxToken> {
        parent
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|it| !it.kind().is_trivia())
    }

    /// 子树里的第一个有效 token（会进入子节点，例如声明开头的修饰符列表）。
    pub fn first_token_in_subtree(parent: &SyntaxNode) -> Option<SyntaxToken> {
        parent
            .descendants_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|it| !it.kind().is_trivia())
    }
}

/// 声明节点开头附着的文档注释（`/** ... */`）。
pub fn doc_comment(node: &SyntaxNode) -> Option<SyntaxToken> {
    let mut res = None;
    for element in node.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => {
                if token.kind() == SyntaxKind::DOC_COMMENT {
                    res = Some(token);
                } else if !token.kind().is_trivia() {
                    break;
                }
            }
            rowan::NodeOrToken::Node(_) => break,
        }
    }
    res
}
