//! 一次检查看到的代码（[`View`]）：整个文件、文件里各个体之外的部分，或者单独的一个体。
//!
//! 能按体划分的检查都写成在 `View` 上工作：同一份代码既可以对整个文件跑（快速修复等按文件
//! 取结果的接口），也可以对单独一个体在它的语法树（[`hir::BodyTree`]）上跑、按体缓存。
//! 检查只通过 `View` 拿语法树、文本、体、推断结果和 [`Semantics`]，不直接读文件。

use std::{borrow::Cow, cell::OnceCell, sync::Arc};

use base_db::{Db, SourceFile};
use hir::{BodyId, InferenceResult, Semantics};
use rustc_hash::FxHashSet;
use syntax::{SyntaxElement, SyntaxKind, SyntaxNode, TextRange, WalkEvent};

/// 视图里的一个体和它的语法节点（范围在视图的树里）。
pub(crate) struct FileBody {
    pub(crate) id: BodyId,
    pub(crate) node: SyntaxNode,
    /// 推断结果，用到时才取
    result: OnceCell<Arc<InferenceResult>>,
}

impl FileBody {
    fn new(id: BodyId, node: SyntaxNode) -> FileBody {
        FileBody {
            id,
            node,
            result: OnceCell::new(),
        }
    }

    /// 体的源码文本。
    pub(crate) fn text<'a>(&self, view: &'a View<'_>) -> &'a str {
        &view.text()[self.node.text_range()]
    }

    /// 体的推断结果。
    pub(crate) fn result(&self, view: &View<'_>) -> Arc<InferenceResult> {
        self.result.get_or_init(|| view.sema.infer(self.id)).clone()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    /// 整个文件
    File,
    /// 文件里单独检查、按体缓存的那些体之外的部分
    Rest,
    /// 单独的一个体
    Body,
}

pub(crate) struct View<'db> {
    pub(crate) db: &'db dyn Db,
    pub(crate) file: SourceFile,
    pub(crate) sema: Semantics<'db>,
    /// 语法树的根：文件的语法树，或者体的语法树
    pub(crate) root: SyntaxNode,
    text: Cow<'db, str>,
    /// 视图里的体
    pub(crate) bodies: Vec<FileBody>,
    part: Part,
    /// 只看体之外的部分时，要跳过的体的节点（种类，范围）
    skip: FxHashSet<(SyntaxKind, TextRange)>,
    /// 同上，体的节点的范围，按起点排序（体互不重叠）
    skip_ranges: Vec<TextRange>,
}

impl<'db> View<'db> {
    /// 整个文件。
    pub(crate) fn file(db: &'db dyn Db, file: SourceFile) -> View<'db> {
        let bodies = crate::file_bodies_syntax(db, file)
            .into_iter()
            .map(|(id, node)| FileBody::new(id, node))
            .collect();
        View {
            db,
            file,
            sema: Semantics::new(db),
            root: base_db::parse(db, file).syntax_node(),
            text: Cow::Borrowed(file.text(db)),
            bodies,
            part: Part::File,
            skip: FxHashSet::default(),
            skip_ranges: Vec::new(),
        }
    }

    /// 文件里 `skipped` 这些体之外的部分（类型声明的头部、字段的类型、import 等，以及
    /// `kept` 里的体）。
    pub(crate) fn rest(
        db: &'db dyn Db,
        file: SourceFile,
        kept: Vec<(BodyId, SyntaxNode)>,
        skipped: &[(BodyId, SyntaxNode)],
    ) -> View<'db> {
        let skip = skipped
            .iter()
            .map(|(_, node)| (node.kind(), node.text_range()))
            .collect();
        let mut skip_ranges: Vec<TextRange> =
            skipped.iter().map(|(_, node)| node.text_range()).collect();
        skip_ranges.sort_by_key(|r| r.start());
        View {
            db,
            file,
            sema: Semantics::new(db),
            root: base_db::parse(db, file).syntax_node(),
            text: Cow::Borrowed(file.text(db)),
            bodies: kept
                .into_iter()
                .map(|(id, node)| FileBody::new(id, node))
                .collect(),
            part: Part::Rest,
            skip,
            skip_ranges,
        }
    }

    /// 单独的一个体，在它自己的语法树上：结果只取决于体所在的成员和外层的声明头。
    pub(crate) fn body(db: &'db dyn Db, tree: Arc<hir::BodyTree>) -> Option<View<'db>> {
        let root = tree.root();
        let node = tree.body_node(&root)?;
        let file = tree.file;
        let body = tree.body;
        let sema = Semantics::for_body_tree(db, tree);
        Some(View {
            db,
            file,
            sema,
            text: Cow::Owned(root.text().to_string()),
            root,
            bodies: vec![FileBody::new(body, node)],
            part: Part::Body,
            skip: FxHashSet::default(),
            skip_ranges: Vec::new(),
        })
    }

    /// `root` 的文本，按树里的范围取。
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    fn start(&self) -> Option<SyntaxNode> {
        match self.part {
            Part::File | Part::Rest => Some(self.root.clone()),
            Part::Body => self.bodies.first().map(|b| b.node.clone()),
        }
    }

    fn skipped(&self, node: &SyntaxNode) -> bool {
        self.part == Part::Rest && self.skip.contains(&(node.kind(), node.text_range()))
    }

    /// 要检查的节点：整个文件、体之外的部分，或者体的子树。
    pub(crate) fn nodes(&self) -> impl Iterator<Item = SyntaxNode> + '_ {
        let mut preorder = self.start().map(|n| n.preorder());
        std::iter::from_fn(move || {
            let preorder = preorder.as_mut()?;
            loop {
                match preorder.next()? {
                    WalkEvent::Enter(node) => {
                        if self.skipped(&node) {
                            preorder.skip_subtree();
                            continue;
                        }
                        return Some(node);
                    }
                    WalkEvent::Leave(_) => continue,
                }
            }
        })
    }

    /// 要检查的节点和 token，范围同 [`Self::nodes`]。
    pub(crate) fn elements(&self) -> impl Iterator<Item = SyntaxElement> + '_ {
        let mut preorder = self.start().map(|n| n.preorder_with_tokens());
        std::iter::from_fn(move || {
            let preorder = preorder.as_mut()?;
            loop {
                match preorder.next()? {
                    WalkEvent::Enter(element) => {
                        if let syntax::NodeOrToken::Node(node) = &element {
                            if self.skipped(node) {
                                preorder.skip_subtree();
                                continue;
                            }
                        }
                        return Some(element);
                    }
                    WalkEvent::Leave(_) => continue,
                }
            }
        })
    }

    /// 要检查的那一段文本的范围：整个树，或者体的节点。
    pub(crate) fn region(&self) -> TextRange {
        match self.part {
            Part::File | Part::Rest => self.root.text_range(),
            Part::Body => self
                .bodies
                .first()
                .map_or_else(|| TextRange::empty(0.into()), |b| b.node.text_range()),
        }
    }

    /// 范围是否在要检查的部分里（只看体之外的部分时，不在任何体里）。
    pub(crate) fn covers(&self, range: TextRange) -> bool {
        match self.part {
            Part::File => true,
            Part::Body => self.region().contains_range(range),
            Part::Rest => {
                let idx = self
                    .skip_ranges
                    .partition_point(|r| r.start() <= range.start());
                idx == 0 || !self.skip_ranges[idx - 1].contains_range(range)
            }
        }
    }

    /// 是否只看单独的一个体。
    pub(crate) fn is_body(&self) -> bool {
        self.part == Part::Body
    }

    /// 体的推断结果（范围在视图的树里）。
    pub(crate) fn infer(&self, body: BodyId) -> Arc<InferenceResult> {
        match self.bodies.iter().find(|b| b.id == body) {
            Some(b) => b.result(self),
            None => self.sema.infer(body),
        }
    }

    /// 体的语法节点。
    pub(crate) fn body_node(&self, body: BodyId) -> Option<SyntaxNode> {
        match self.bodies.iter().find(|b| b.id == body) {
            Some(b) => Some(b.node.clone()),
            None if self.part == Part::Body => None,
            None => hir::body_syntax(self.db, body).map(|(_, node)| node),
        }
    }

    /// 包含节点的体。
    pub(crate) fn body_of(&self, node: &SyntaxNode) -> Option<BodyId> {
        self.sema.body_of(self.file, node)
    }

    /// 解析类型节点。
    pub(crate) fn resolve_type(&self, ty: &syntax::ast::Type) -> hir::Ty {
        self.sema.resolve_type(self.file, ty)
    }

    /// 整个文件的文本里有没有 `needle`（便宜的预检）。单独看一个体时只依赖这个结果，
    /// 不依赖文件的文本。
    pub(crate) fn file_contains(&self, needle: &str) -> bool {
        match self.part {
            Part::File | Part::Rest => self.file.text(self.db).contains(needle),
            Part::Body => file_contains(self.db, self.file, needle.to_owned()),
        }
    }
}

/// 文件的文本里有没有 `needle`。每次编辑后都重新计算，但结果通常不变，依赖它的查询
/// 也就不会重新执行。
#[salsa::tracked]
fn file_contains(db: &dyn Db, file: SourceFile, needle: String) -> bool {
    file.text(db).contains(needle.as_str())
}
