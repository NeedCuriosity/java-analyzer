//! Code lens：类和方法声明上方的"N 处引用""N 个实现"。
//!
//! 列出 lens 只看语法（便宜）；数量在 resolve 阶段才算，客户端只会为可见的 lens 请求。

use base_db::{Db, SourceFile};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{navigation, references, FilePosition};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeLensKind {
    References,
    Implementations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CodeLens {
    /// 声明的名字
    pub range: TextRange,
    pub kind: CodeLensKind,
}

fn name_range(node: &SyntaxNode) -> Option<TextRange> {
    node.children()
        .find_map(ast::Name::cast)
        .map(|n| n.syntax().text_range())
}

pub(crate) fn code_lenses(db: &dyn Db, file: SourceFile) -> Vec<CodeLens> {
    let root = base_db::parse(db, file).syntax_node();
    let mut res = Vec::new();
    for node in root.descendants() {
        let kind = node.kind();
        let is_type = ast::TypeDecl::can_cast(kind);
        if !is_type && !matches!(kind, SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL) {
            continue;
        }
        // 局部类、匿名类里的声明不标注：太碎
        let in_body = node.ancestors().skip(1).any(|a| {
            matches!(
                a.kind(),
                SyntaxKind::BLOCK | SyntaxKind::NEW_EXPR | SyntaxKind::LAMBDA_EXPR
            )
        });
        if in_body {
            continue;
        }
        let Some(range) = name_range(&node) else {
            continue;
        };
        res.push(CodeLens {
            range,
            kind: CodeLensKind::References,
        });

        // 可以被继承 / 实现的声明再给一个"实现"
        let has = |token: SyntaxKind| {
            node.children()
                .find_map(ast::ModifierList::cast)
                .is_some_and(|mods| mods.has(token))
        };
        let overridable = match kind {
            SyntaxKind::INTERFACE_DECL => true,
            SyntaxKind::CLASS_DECL => has(SyntaxKind::ABSTRACT_KW),
            SyntaxKind::METHOD_DECL => {
                let in_interface = node
                    .ancestors()
                    .skip(1)
                    .find(|a| ast::TypeDecl::can_cast(a.kind()))
                    .is_some_and(|decl| decl.kind() == SyntaxKind::INTERFACE_DECL);
                let has_body = node.children().any(|c| c.kind() == SyntaxKind::BLOCK);
                (in_interface && !has_body && !has(SyntaxKind::STATIC_KW))
                    || has(SyntaxKind::ABSTRACT_KW)
            }
            _ => false,
        };
        if overridable {
            res.push(CodeLens {
                range,
                kind: CodeLensKind::Implementations,
            });
        }
    }
    res
}

/// lens 上显示的数量。`position` 是声明名字里的任意位置。
pub(crate) fn code_lens_count(db: &dyn Db, position: FilePosition, kind: CodeLensKind) -> usize {
    match kind {
        CodeLensKind::References => {
            references::find_all_refs(db, position, false).map_or(0, |refs| refs.len())
        }
        CodeLensKind::Implementations => {
            navigation::goto_implementation(db, position).map_or(0, |found| found.info.len())
        }
    }
}
