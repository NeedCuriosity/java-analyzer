//! 查找引用、文档内高亮。

use base_db::Db;
use hir::Definition;
use ide_db::search::{find_references, references_in_file, ReferenceKind};
use syntax::TextRange;

use crate::{
    navigation::{definition_at, position_file},
    FileId, FilePosition,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReferenceItem {
    pub file_id: FileId,
    pub range: TextRange,
    pub is_write: bool,
    pub is_declaration: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HighlightedRange {
    pub range: TextRange,
    pub is_write: bool,
}

fn declaration_item(db: &dyn Db, def: &Definition) -> Option<ReferenceItem> {
    let source = def.source(db)?;
    Some(ReferenceItem {
        file_id: source.file.file_id(db),
        range: source.focus_range,
        is_write: false,
        is_declaration: true,
    })
}

pub(crate) fn find_all_refs(
    db: &dyn Db,
    position: FilePosition,
    include_declaration: bool,
) -> Option<Vec<ReferenceItem>> {
    let file = position_file(db, position)?;
    let (_, def) = definition_at(db, file, position.offset).or_else(|| {
        // 端点方法映射注解里的路径：按方法本身查
        let name = crate::endpoints::method_name_at_literal(db, file, position.offset)?;
        definition_at(db, file, name)
    })?;

    let mut res: Vec<ReferenceItem> = find_references(db, &def)
        .into_iter()
        .map(|reference| ReferenceItem {
            file_id: reference.file_id,
            range: reference.range,
            is_write: reference.kind == ReferenceKind::Write,
            is_declaration: false,
        })
        .collect();
    // 服务端控制器方法：调用它的 HTTP 客户端方法（Feign / HTTP 接口）也算引用
    for (file_id, range) in crate::endpoints::clients_of_method(db, &def) {
        res.push(ReferenceItem {
            file_id,
            range,
            is_write: false,
            is_declaration: false,
        });
    }
    if include_declaration {
        res.extend(declaration_item(db, &def));
    }
    res.sort_by_key(|item| (item.file_id, item.range.start()));
    res.dedup_by_key(|item| (item.file_id, item.range));
    Some(res)
}

/// 当前文件里与光标下符号相同的所有出现位置。
pub(crate) fn highlight_related(
    db: &dyn Db,
    position: FilePosition,
) -> Option<Vec<HighlightedRange>> {
    let file = position_file(db, position)?;
    let (_, def) = definition_at(db, file, position.offset)?;
    let name = def.name(db)?;

    let mut res: Vec<HighlightedRange> = references_in_file(db, file, &def, &name)
        .into_iter()
        .chain(ide_db::doc_links::doc_references_in_file(
            db, file, &def, &name,
        ))
        .map(|reference| HighlightedRange {
            range: reference.range,
            is_write: reference.kind == ReferenceKind::Write,
        })
        .collect();
    if let Some(decl) = declaration_item(db, &def).filter(|d| d.file_id == position.file_id) {
        // 声明处算作写入（变量在这里被赋初值）
        let is_write = matches!(def, Definition::Local(_) | Definition::Field(_));
        res.push(HighlightedRange {
            range: decl.range,
            is_write,
        });
    }
    res.sort_by_key(|item| item.range.start());
    res.dedup_by_key(|item| item.range);
    Some(res)
}
