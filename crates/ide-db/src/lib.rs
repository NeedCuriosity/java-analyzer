//! IDE 功能共用的设施：数据库实体、符号索引、引用搜索、签名渲染、Javadoc、import 编辑。

pub mod fuzzy;
pub mod imports;
pub mod javadoc;
pub mod render;
pub mod search;
pub mod spring;
pub mod symbol_index;

use std::sync::Arc;

use base_db::{Interner, ParseCache};
use hir::{class_data, ClassKind, Definition, LocalKind, Modifiers};
use syntax::TextRange;

pub use base_db::{self, Db, FileId, LineCol, LineIndex, SourceFile};

/// 整个分析层唯一的具体数据库。
///
/// `clone()` 得到的是共享同一份存储的句柄，用作只读快照；对原句柄的写入会让
/// 所有快照上正在进行的查询取消。
///
#[salsa::db]
#[derive(Clone, Default)]
pub struct RootDatabase {
    storage: salsa::Storage<Self>,
    interner: Arc<Interner>,
    parse_cache: Arc<ParseCache>,
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {
    fn interner(&self) -> &Interner {
        &self.interner
    }

    fn parse_cache(&self) -> &ParseCache {
        &self.parse_cache
    }
}

impl RootDatabase {
    pub fn new() -> RootDatabase {
        let db = RootDatabase::default();
        base_db::init_workspace(&db);
        db
    }
}

impl std::fmt::Debug for RootDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootDatabase").finish_non_exhaustive()
    }
}

/// 一处文本替换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: TextRange,
    pub new_text: String,
}

impl TextEdit {
    pub fn insert(offset: syntax::TextSize, text: impl Into<String>) -> TextEdit {
        TextEdit {
            range: TextRange::empty(offset),
            new_text: text.into(),
        }
    }

    pub fn replace(range: TextRange, text: impl Into<String>) -> TextEdit {
        TextEdit {
            range,
            new_text: text.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Class,
    Interface,
    Enum,
    Record,
    Annotation,
    Field,
    EnumConstant,
    Method,
    Constructor,
    Local,
    Param,
    TypeParam,
    Package,
}

impl SymbolKind {
    pub fn of_class(kind: ClassKind) -> SymbolKind {
        match kind {
            ClassKind::Class => SymbolKind::Class,
            ClassKind::Interface => SymbolKind::Interface,
            ClassKind::Enum => SymbolKind::Enum,
            ClassKind::Record => SymbolKind::Record,
            ClassKind::Annotation => SymbolKind::Annotation,
        }
    }

    pub fn of_definition(db: &dyn Db, def: &Definition) -> SymbolKind {
        match def {
            Definition::Class(class) => {
                class_data(db, *class).map_or(SymbolKind::Class, |d| SymbolKind::of_class(d.kind))
            }
            Definition::Field(field) => {
                let is_constant = class_data(db, field.class)
                    .and_then(|d| {
                        d.fields
                            .get(field.idx as usize)
                            .map(|f| f.is_enum_constant())
                    })
                    .unwrap_or(false);
                if is_constant {
                    SymbolKind::EnumConstant
                } else {
                    SymbolKind::Field
                }
            }
            Definition::Method(method) => {
                let is_constructor = class_data(db, method.class)
                    .and_then(|d| d.methods.get(method.idx as usize).map(|m| m.is_constructor))
                    .unwrap_or(false);
                if is_constructor {
                    SymbolKind::Constructor
                } else {
                    SymbolKind::Method
                }
            }
            Definition::Local(local) => match hir::infer(db, local.body).local(local.id).kind {
                LocalKind::Param | LocalKind::LambdaParam | LocalKind::CatchParam => {
                    SymbolKind::Param
                }
                LocalKind::LocalClassField => SymbolKind::Field,
                LocalKind::LocalClassMethod => SymbolKind::Method,
                _ => SymbolKind::Local,
            },
            Definition::TypeParam(_) => SymbolKind::TypeParam,
            Definition::Package(_) => SymbolKind::Package,
        }
    }
}

/// 定义的修饰符（局部符号、类型参数、包没有）。
pub fn definition_flags(db: &dyn Db, def: &Definition) -> Modifiers {
    let flags = match def {
        Definition::Class(class) => class_data(db, *class).map(|d| d.flags),
        Definition::Field(field) => class_data(db, field.class)
            .and_then(|d| d.fields.get(field.idx as usize).map(|f| f.flags)),
        Definition::Method(method) => class_data(db, method.class)
            .and_then(|d| d.methods.get(method.idx as usize).map(|m| m.flags)),
        _ => None,
    };
    flags.unwrap_or_default()
}
