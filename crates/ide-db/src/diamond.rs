//! 菱形匿名类（`new Comparator<>() {…}`）：父类型要推断才知道，没有登记成类。
//! 按文件找出它们的父类型和声明的方法，供"跳转到实现"、类型层级和重命名使用。

use std::sync::Arc;

use base_db::{Db, SourceFile};
use hir::{ClassId, Semantics};
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    TextRange,
};

/// 菱形匿名类里声明的一个方法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiamondMethod {
    pub name: SmolStr,
    pub arity: usize,
    pub name_range: TextRange,
    pub full_range: TextRange,
}

/// 一个菱形匿名类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiamondAnonymous {
    pub super_class: ClassId,
    /// `new` 表达式里的类型（`Comparator<>`）
    pub ty_range: TextRange,
    /// 整个 `new` 表达式
    pub full_range: TextRange,
    /// `new Comparator<>() {…}`
    pub label: SmolStr,
    pub methods: Vec<DiamondMethod>,
}

/// 文件里的菱形匿名类。
#[salsa::tracked]
pub fn diamond_anonymous_classes(db: &dyn Db, file: SourceFile) -> Arc<Vec<DiamondAnonymous>> {
    let text = file.text(db);
    if !text.contains("<>") {
        return Arc::default();
    }
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let mut res = Vec::new();
    for new in root.descendants().filter_map(ast::NewExpr::cast) {
        let (Some(ty), Some(body)) = (new.ty(), new.body()) else {
            continue;
        };
        let ty_text = ty.syntax().text().to_string();
        if !ty_text.ends_with("<>") || sema.anonymous_class_of_body(file, body.syntax()).is_some() {
            continue;
        }
        let hir::Ty::Class(super_class, _) = sema.resolve_type(file, &ty) else {
            continue;
        };
        let methods = body
            .syntax()
            .children()
            .filter_map(ast::MethodDecl::cast)
            .filter_map(|m| {
                let name = m.name()?;
                Some(DiamondMethod {
                    name: name.text(),
                    arity: m.param_list().map_or(0, |p| p.params().count()),
                    name_range: name.syntax().text_range(),
                    full_range: m.syntax().text_range(),
                })
            })
            .collect();
        let label: String = ty_text.split_whitespace().collect::<Vec<_>>().join(" ");
        res.push(DiamondAnonymous {
            super_class,
            ty_range: ty.syntax().text_range(),
            full_range: new.syntax().text_range(),
            label: SmolStr::new(format!("new {label}() {{…}}")),
            methods,
        });
    }
    Arc::new(res)
}

/// 工作区里父类型满足 `accept` 的菱形匿名类。`words` 非空时只看文本里出现了其中某个词的文件
/// （找覆写时传方法名，先按文本筛掉绝大多数文件）。
pub fn find_diamond_anonymous(
    db: &dyn Db,
    words: &[&str],
    mut accept: impl FnMut(ClassId) -> bool,
) -> Vec<(SourceFile, DiamondAnonymous)> {
    let mut res = Vec::new();
    for (_, file) in base_db::workspace(db).files(db).workspace_files() {
        db.unwind_if_revision_cancelled();
        let text = file.text(db);
        if !text.contains("<>") || !(words.is_empty() || words.iter().any(|w| text.contains(w))) {
            continue;
        }
        for anon in diamond_anonymous_classes(db, file).iter() {
            if accept(anon.super_class) {
                res.push((file, anon.clone()));
            }
        }
    }
    res
}
