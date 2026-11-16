//! 单独检查一个体时用的语法树（[`BodyTree`]）。
//!
//! 诊断里的很多检查要看体所在的上下文：外层类型声明的种类、修饰符、类型参数、注解等。
//! 从文件里摘出体所在的成员，外层各级类型声明只保留头部和通往这个成员的路径（别的成员、
//! import、注释都去掉），拼成一棵位置从 0 开始的树：检查照常在树上往上找，结果却只取决于
//! 这个成员自己和外层的声明头。别的成员里的编辑、让体挪动位置的编辑都不会改变它。

use std::sync::Arc;

use base_db::{Db, SourceFile};
use hir_def::{source_map, DeclIdx};
use syntax::{GreenNode, GreenToken, NodeOrToken, SyntaxKind, SyntaxNode, TextRange, TextSize};

use crate::infer::{body_anchor, body_syntax, infer_tracked, BodyId, InferenceResult};

/// 一个体的语法树：体所在的成员原样保留，外层只有类型声明的头部。位置从 0 开始。
#[derive(Eq, Debug)]
pub struct BodyTree {
    pub body: BodyId,
    pub file: SourceFile,
    green: GreenNode,
    /// 原样保留的成员（方法、构造器、初始化块、枚举常量，字段的话是整个字段声明；
    /// 记录的紧凑构造器是整个记录声明）在树里的范围
    member: TextRange,
    /// 体的节点在树里的种类和范围
    body_node: (SyntaxKind, TextRange),
    /// 推断用的子树在树里的起点
    infer_offset: TextSize,
    /// 成员里的声明（局部类、匿名类体和它们的成员，以及成员自己）：（种类，范围，声明下标），
    /// 按起点排序
    decls: Vec<(SyntaxKind, TextRange, DeclIdx)>,
}

impl PartialEq for BodyTree {
    fn eq(&self, other: &Self) -> bool {
        self.body == other.body
            && self.file == other.file
            && self.member == other.member
            && self.body_node == other.body_node
            && self.infer_offset == other.infer_offset
            && self.decls == other.decls
            && syntax::green_eq(&self.green, &other.green)
    }
}

impl BodyTree {
    /// 树的根（`COMPILATION_UNIT`）。
    pub fn root(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// 原样保留的成员在树里的范围。体的检查结果都落在这里面。
    pub fn member_range(&self) -> TextRange {
        self.member
    }

    /// 体的节点（方法 / 构造器声明、变量声明符、枚举常量或初始化块）。
    pub fn body_node(&self, root: &SyntaxNode) -> Option<SyntaxNode> {
        let (kind, range) = self.body_node;
        let node = match root.covering_element(range) {
            NodeOrToken::Node(node) => node,
            NodeOrToken::Token(token) => token.parent()?,
        };
        node.ancestors()
            .take_while(|n| n.text_range() == range)
            .find(|n| n.kind() == kind)
    }

    /// `node`（树里的节点）是不是体的节点。
    pub fn is_body_node(&self, node: &SyntaxNode) -> bool {
        (node.kind(), node.text_range()) == self.body_node
    }

    /// 树里某个声明节点的声明下标（只有成员里的声明）。
    pub fn decl_idx(&self, node: &SyntaxNode) -> Option<DeclIdx> {
        let (kind, range) = (node.kind(), node.text_range());
        let first = self
            .decls
            .partition_point(|(_, r, _)| r.start() < range.start());
        self.decls[first..]
            .iter()
            .take_while(|(_, r, _)| r.start() == range.start())
            .find(|(k, r, _)| *k == kind && *r == range)
            .map(|(_, _, idx)| *idx)
    }

    /// 体的推断结果，范围换算到这棵树里。
    pub fn infer(&self, db: &dyn Db) -> InferenceResult {
        let data = infer_tracked(db, base_db::workspace(db), self.body);
        InferenceResult::new(data, self.infer_offset)
    }
}

/// 体所在、单独检查时原样保留的成员节点（见 [`BodyTree`]）：一般就是体推断用的子树，
/// 字段的话是整个字段声明。`node` 是体的节点。
pub fn body_member(node: &SyntaxNode) -> SyntaxNode {
    match node.parent() {
        Some(parent)
            if node.kind() == SyntaxKind::VARIABLE_DECLARATOR
                && parent.kind() == SyntaxKind::FIELD_DECL =>
        {
            parent
        }
        _ => body_anchor(node),
    }
}

/// 体的语法树（见 [`BodyTree`]）。拿不到体的语法时为 `None`。
pub fn body_tree(db: &dyn Db, body: BodyId) -> Option<Arc<BodyTree>> {
    body_tree_tracked(db, base_db::workspace(db), body)
}

// 每次编辑所在文件后都会重新计算（很便宜），只要成员和外层声明头没变，结果就和上次相等，
// 依赖它的查询不会重新执行。
#[salsa::tracked]
fn body_tree_tracked(db: &dyn Db, _ws: base_db::Workspace, body: BodyId) -> Option<Arc<BodyTree>> {
    let (file, node) = body_syntax(db, body)?;
    let anchor = body_anchor(&node);
    // 字段的初值以外，检查还要看字段声明的类型和修饰符：保留整个字段声明
    let member = body_member(&node);
    let member_range = member.text_range();
    // 自内向外重建通往成员的各级节点，记下成员在新树里的起点
    let mut green = member.green().into_owned();
    let mut offset = TextSize::from(0);
    let mut child = member.clone();
    while let Some(parent) = child.parent() {
        let mut children = Vec::new();
        let mut before = TextSize::from(0);
        match parent.kind() {
            // 类体和文件只留下通往成员的那一个子节点（类体另外留下花括号）。成员很多时
            // 不逐个看别的成员：花括号只在两头找
            SyntaxKind::CLASS_BODY | SyntaxKind::COMPILATION_UNIT => {
                let brace = |element: NodeOrToken<SyntaxNode, syntax::SyntaxToken>, kind| {
                    element.into_token().filter(|t| t.kind() == kind)
                };
                let is_body = parent.kind() == SyntaxKind::CLASS_BODY;
                let l_curly = is_body
                    .then(|| {
                        parent
                            .children_with_tokens()
                            .take_while(|e| e.as_node() != Some(&child))
                            .find_map(|e| brace(e, SyntaxKind::L_CURLY))
                    })
                    .flatten();
                let r_curly = is_body
                    .then(|| {
                        std::iter::successors(parent.last_child_or_token(), |e| {
                            e.prev_sibling_or_token()
                        })
                        .take_while(|e| e.as_token().is_some())
                        .find_map(|e| brace(e, SyntaxKind::R_CURLY))
                    })
                    .flatten();
                if let Some(l_curly) = l_curly {
                    before += l_curly.text_range().len();
                    children.push(NodeOrToken::Token(l_curly.green().to_owned()));
                }
                children.push(NodeOrToken::Node(green.clone()));
                if let Some(r_curly) = r_curly {
                    children.push(NodeOrToken::Token(r_curly.green().to_owned()));
                }
            }
            _ => {
                let mut seen = false;
                for element in parent.children_with_tokens() {
                    let kept = match &element {
                        NodeOrToken::Node(n) if *n == child => {
                            seen = true;
                            NodeOrToken::Node(green.clone())
                        }
                        NodeOrToken::Node(n) => NodeOrToken::Node(n.green().into_owned()),
                        NodeOrToken::Token(t) => NodeOrToken::Token(t.green().to_owned()),
                    };
                    if !seen {
                        before += kept_len(&kept);
                    }
                    children.push(kept);
                }
            }
        }
        offset += before;
        green = syntax::algo::new_green_node(parent.kind(), children);
        child = parent;
    }
    let shift = |range: TextRange| range - member_range.start() + offset;
    let decls = source_map(db, file)
        .decls_in(member_range)
        .map(|(idx, ptr)| (ptr.kind(), shift(ptr.text_range()), idx))
        .collect();
    Some(Arc::new(BodyTree {
        body,
        file,
        green,
        member: shift(member_range),
        body_node: (node.kind(), shift(node.text_range())),
        infer_offset: anchor.text_range().start() - member_range.start() + offset,
        decls,
    }))
}

fn kept_len(element: &NodeOrToken<GreenNode, GreenToken>) -> TextSize {
    match element {
        NodeOrToken::Node(n) => n.text_len(),
        NodeOrToken::Token(t) => t.text_len(),
    }
}
