//! 语义高亮：给每个标识符标出它是什么（类、字段、局部变量 …）以及修饰信息。
//!
//! 普通关键字、字面量、注释由编辑器自带的语法高亮负责；这里只补充需要语义才能区分的部分，
//! 以及词法上是标识符的上下文关键字（`var`、`record`、`sealed` …）。

use base_db::{Db, SourceFile};
use hir::{Definition, Semantics};
use ide_db::{definition_flags, SymbolKind};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HlTag {
    Symbol(SymbolKind),
    Keyword,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct HlMods(pub u8);

impl HlMods {
    pub const DECLARATION: u8 = 1 << 0;
    pub const STATIC: u8 = 1 << 1;
    pub const READONLY: u8 = 1 << 2;
    pub const DEPRECATED: u8 = 1 << 3;
    pub const ABSTRACT: u8 = 1 << 4;
    /// JDK 的类和成员（LSP 的 `defaultLibrary`：标准库）
    pub const LIBRARY: u8 = 1 << 5;

    pub fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    fn set(&mut self, flag: u8, on: bool) {
        if on {
            self.0 |= flag;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HlRange {
    pub range: TextRange,
    pub tag: HlTag,
    pub mods: HlMods,
}

fn mods_of(db: &dyn Db, def: &Definition, is_declaration: bool) -> HlMods {
    let flags = definition_flags(db, def);
    let mut mods = HlMods::default();
    mods.set(HlMods::DECLARATION, is_declaration);
    mods.set(HlMods::STATIC, flags.is_static());
    mods.set(HlMods::DEPRECATED, flags.is_deprecated());
    match def {
        Definition::Field(_) => mods.set(HlMods::READONLY, flags.is_final()),
        Definition::Class(_) | Definition::Method(_) => {
            mods.set(HlMods::ABSTRACT, flags.is_abstract())
        }
        _ => {}
    }
    // 只标引用：局部变量、参数、类型参数不算；声明处（包括打开库源码时文件自己声明的成员）不算
    let class = match def {
        _ if is_declaration => None,
        Definition::Class(class) => Some(*class),
        Definition::Field(_) | Definition::Method(_) => def.container_class(),
        _ => None,
    };
    if let Some(class) = class {
        mods.set(HlMods::LIBRARY, is_jdk_class(db, class));
    }
    mods
}

/// 类来自 JDK（而不是工作区或第三方依赖库）。
fn is_jdk_class(db: &dyn Db, class: hir::ClassId) -> bool {
    match hir::class_origin(db, class) {
        Some(hir::ClassOrigin::Binary { container }) => base_db::workspace(db)
            .class_path(db)
            .is_jdk_container(container),
        _ => false,
    }
}

pub(crate) fn highlight(db: &dyn Db, file: SourceFile, range: Option<TextRange>) -> Vec<HlRange> {
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let mut res = Vec::new();

    let tokens = root
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| range.is_none_or(|r| r.intersect(token.text_range()).is_some()));

    for (count, token) in tokens.enumerate() {
        if count % 512 == 0 {
            db.unwind_if_revision_cancelled();
        }
        let kind = token.kind();
        if kind.is_contextual_keyword() {
            res.push(HlRange {
                range: token.text_range(),
                tag: HlTag::Keyword,
                mods: HlMods::default(),
            });
            continue;
        }
        if kind != SyntaxKind::IDENT {
            continue;
        }
        let Some(parent) = token.parent() else {
            continue;
        };
        let (def, is_declaration) = if let Some(name) = ast::Name::cast(parent.clone()) {
            (sema.classify_name(file, &name), true)
        } else if let Some(name_ref) = ast::NameRef::cast(parent.clone()) {
            // `var` 在词法上是普通标识符
            let is_var = token.text() == "var"
                && parent
                    .parent()
                    .is_some_and(|p| p.kind() == SyntaxKind::CLASS_TYPE);
            if is_var {
                res.push(HlRange {
                    range: token.text_range(),
                    tag: HlTag::Keyword,
                    mods: HlMods::default(),
                });
                continue;
            }
            (sema.classify_name_ref(file, &name_ref), false)
        } else {
            continue;
        };
        let Some(def) = def else { continue };
        res.push(HlRange {
            range: token.text_range(),
            tag: HlTag::Symbol(SymbolKind::of_definition(db, &def)),
            mods: mods_of(db, &def, is_declaration),
        });
    }
    res
}
