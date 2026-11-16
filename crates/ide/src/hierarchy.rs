//! 调用层级与类型层级。

use base_db::{workspace, Db};
use hir::{
    class_data, direct_supers, source_class_index, BodyId, ClassId, Definition, Resolution,
    Semantics,
};
use ide_db::search::find_references;
use syntax::{SyntaxKind, TextRange};

use crate::{
    navigation::{definition_at, position_file, to_nav, NavigationTarget},
    DefKey, FilePosition,
};

/// 调用层级里的一条边。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallItem {
    /// 入向：调用者；出向：被调用者
    pub target: NavigationTarget,
    /// 调用点。入向时位于调用者的文件里，出向时位于被查询方法的文件里
    pub ranges: Vec<TextRange>,
}

pub(crate) fn prepare_call_hierarchy(
    db: &dyn Db,
    position: FilePosition,
) -> Option<Vec<NavigationTarget>> {
    let file = position_file(db, position)?;
    let (_, def) = definition_at(db, file, position.offset)?;
    match def {
        Definition::Method(_) => Some(to_nav(db, &def).into_iter().collect()),
        _ => None,
    }
}

fn group(db: &dyn Db, edges: Vec<(Definition, TextRange)>) -> Vec<CallItem> {
    let mut grouped: Vec<(Definition, Vec<TextRange>)> = Vec::new();
    for (def, range) in edges {
        match grouped.iter_mut().find(|(existing, _)| *existing == def) {
            Some((_, ranges)) => ranges.push(range),
            None => grouped.push((def, vec![range])),
        }
    }
    grouped
        .into_iter()
        .filter_map(|(def, mut ranges)| {
            ranges.sort_by_key(|r| r.start());
            Some(CallItem {
                target: to_nav(db, &def)?,
                ranges,
            })
        })
        .collect()
}

pub(crate) fn incoming_calls(db: &dyn Db, key: &DefKey) -> Vec<CallItem> {
    let Some(def) = ide_completion::resolve_definition(db, key) else {
        return Vec::new();
    };
    let sema = Semantics::new(db);
    let mut edges = Vec::new();
    for reference in find_references(db, &def) {
        let Some(file) = base_db::source_file(db, reference.file_id) else {
            continue;
        };
        let root = base_db::parse(db, file).syntax_node();
        if !root.text_range().contains_range(reference.range) {
            continue;
        }
        let node = match root.covering_element(reference.range) {
            syntax::NodeOrToken::Node(node) => node,
            syntax::NodeOrToken::Token(token) => match token.parent() {
                Some(node) => node,
                None => continue,
            },
        };
        // 调用者：包含调用点的方法；字段初始化式和初始化块里的调用记在类上
        let caller = node
            .ancestors()
            .filter(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL
                )
            })
            .find_map(|n| sema.method_of_decl(file, &n))
            .map(Definition::Method)
            .or_else(|| sema.enclosing_class(file, &node).map(Definition::Class));
        if let Some(caller) = caller {
            edges.push((caller, reference.range));
        }
    }
    group(db, edges)
}

pub(crate) fn outgoing_calls(db: &dyn Db, key: &DefKey) -> Vec<CallItem> {
    let Some(Definition::Method(method)) = ide_completion::resolve_definition(db, key) else {
        return Vec::new();
    };
    let result = hir::infer(db, BodyId::Method(method));
    let mut edges: Vec<(Definition, TextRange)> = result
        .resolutions()
        .filter_map(|(range, resolution)| match resolution {
            Resolution::Method(callee) => Some((Definition::Method(*callee), range)),
            _ => None,
        })
        .collect();
    edges.sort_by_key(|(_, range)| range.start());
    group(db, edges)
}

fn key_class(db: &dyn Db, key: &DefKey) -> Option<ClassId> {
    match ide_completion::resolve_definition(db, key)? {
        Definition::Class(class) => Some(class),
        _ => None,
    }
}

pub(crate) fn prepare_type_hierarchy(
    db: &dyn Db,
    position: FilePosition,
) -> Option<Vec<NavigationTarget>> {
    let file = position_file(db, position)?;
    let (_, def) = definition_at(db, file, position.offset)?;
    let class = match &def {
        Definition::Class(class) => *class,
        // 在成员上触发时取它所在的类
        Definition::Field(_) | Definition::Method(_) => def.container_class()?,
        _ => return None,
    };
    Some(to_nav(db, &Definition::Class(class)).into_iter().collect())
}

pub(crate) fn supertypes(db: &dyn Db, key: &DefKey) -> Vec<NavigationTarget> {
    let Some(class) = key_class(db, key) else {
        return Vec::new();
    };
    direct_supers(db, class)
        .iter()
        .filter_map(|sup| to_nav(db, &Definition::Class(*sup)))
        .collect()
}

pub(crate) fn subtypes(db: &dyn Db, key: &DefKey) -> Vec<NavigationTarget> {
    let Some(class) = key_class(db, key) else {
        return Vec::new();
    };
    let mut res = Vec::new();

    let index = source_class_index(db);
    for (binary_name, file, _) in index.iter_all() {
        let candidate = hir::class_in_file(db, file, binary_name);
        // 局部类和匿名类也列出（匿名类用 `new X() {…}` 当名字）
        if candidate != class && direct_supers(db, candidate).contains(&class) {
            res.extend(to_nav(db, &Definition::Class(candidate)));
        }
    }
    res.extend(crate::navigation::diamond_anonymous_navs(db, class, true));
    let class_path = workspace(db).class_path(db);
    for summary in class_path.direct_subtypes(&class.binary_name(db)) {
        if summary.is_anonymous_or_local() {
            continue;
        }
        let candidate = ClassId::from_binary_name(db, &summary.name);
        // 被源码遮蔽的同名类已经在上面处理过
        if class_data(db, candidate).is_some_and(|d| !d.is_source) {
            res.extend(to_nav(db, &Definition::Class(candidate)));
        }
    }
    res
}
