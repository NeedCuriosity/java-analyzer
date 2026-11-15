//! 语法节点的"指针"：只记种类和范围，不持有树。
//!
//! `SyntaxNode` 不是 `Send`，也不能跨重新解析存活；需要长期保存或跨线程传递的地方
//! 一律保存指针，用的时候再对着当前的树解引用。

use std::{
    hash::{Hash, Hasher},
    iter::successors,
    marker::PhantomData,
};

use rowan::TextRange;

use crate::{AstNode, SyntaxKind, SyntaxNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyntaxNodePtr {
    kind: SyntaxKind,
    range: TextRange,
}

impl SyntaxNodePtr {
    pub fn new(node: &SyntaxNode) -> SyntaxNodePtr {
        SyntaxNodePtr {
            kind: node.kind(),
            range: node.text_range(),
        }
    }

    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    pub fn text_range(&self) -> TextRange {
        self.range
    }

    /// 在 `root` 所在的树里找回节点。`root` 必须是创建指针时那棵树（或文本相同的树）的根。
    pub fn try_to_node(&self, root: &SyntaxNode) -> Option<SyntaxNode> {
        if !root.text_range().contains_range(self.range) {
            return None;
        }
        successors(Some(root.clone()), |node| {
            node.child_or_token_at_range(self.range)
                .and_then(|it| it.into_node())
        })
        .find(|it| it.text_range() == self.range && it.kind() == self.kind)
    }

    pub fn to_node(&self, root: &SyntaxNode) -> SyntaxNode {
        self.try_to_node(root)
            .unwrap_or_else(|| panic!("无法解引用 {self:?}"))
    }
}

/// 带类型的 [`SyntaxNodePtr`]。
pub struct AstPtr<N> {
    raw: SyntaxNodePtr,
    _ty: PhantomData<fn() -> N>,
}

impl<N> Clone for AstPtr<N> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<N> Copy for AstPtr<N> {}

impl<N> PartialEq for AstPtr<N> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<N> Eq for AstPtr<N> {}

impl<N> Hash for AstPtr<N> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

impl<N> std::fmt::Debug for AstPtr<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AstPtr").field(&self.raw).finish()
    }
}

impl<N: AstNode> AstPtr<N> {
    pub fn new(node: &N) -> AstPtr<N> {
        AstPtr {
            raw: SyntaxNodePtr::new(node.syntax()),
            _ty: PhantomData,
        }
    }

    pub fn try_to_node(&self, root: &SyntaxNode) -> Option<N> {
        self.raw.try_to_node(root).and_then(N::cast)
    }

    pub fn to_node(&self, root: &SyntaxNode) -> N {
        N::cast(self.raw.to_node(root)).expect("指针的类型与节点种类不符")
    }

    pub fn syntax_node_ptr(&self) -> SyntaxNodePtr {
        self.raw
    }

    pub fn text_range(&self) -> TextRange {
        self.raw.text_range()
    }
}
