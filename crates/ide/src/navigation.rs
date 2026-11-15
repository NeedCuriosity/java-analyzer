//! 跳转：定义、声明、类型定义、实现，以及工作区符号搜索。

use base_db::{workspace, Db, SourceFile};
use hir::{
    all_supers, class_data, field_ty, is_subclass_of, method_sig, source_class_index, ClassId,
    Definition, MethodId, Semantics, Ty,
};
use ide_db::{search::refers_to, symbol_index::search_symbols, SymbolKind};
use rustc_hash::FxHashSet;
use smol_str::SmolStr;
use syntax::{
    algo::pick_ident_token,
    ast::{self, AstNode},
    SyntaxKind, TextRange, TextSize,
};

use crate::{
    library::{LibraryMember, LibraryMemberKind},
    DefKey, FileId, FilePosition, RangeInfo,
};

/// 目标所在的文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavFile {
    /// 已在数据库里的源码文件
    Source(FileId),
    /// 依赖库 / JDK 里的类：宿主负责找到源码或生成桩代码，再用
    /// [`crate::locate_in_library_source`] 定位
    Library {
        class: SmolStr,
        member: Option<LibraryMember>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationTarget {
    pub file: NavFile,
    /// 整个声明（`NavFile::Library` 时为空范围）
    pub full_range: TextRange,
    /// 名字
    pub focus_range: TextRange,
    pub name: SmolStr,
    pub kind: SymbolKind,
    /// 所在的类或包（点分）
    pub container: Option<String>,
    /// 一行签名
    pub detail: Option<String>,
    pub deprecated: bool,
    /// 之后再找回这个定义用的稳定标识（层级视图的往返）
    pub key: Option<DefKey>,
}

pub(crate) fn def_key(db: &dyn Db, def: &Definition) -> Option<DefKey> {
    match def {
        Definition::Class(class) => Some(DefKey::Class {
            binary_name: class.binary_name(db),
        }),
        Definition::Method(method) => Some(DefKey::Method {
            class: method.class.binary_name(db),
            idx: method.idx,
        }),
        Definition::Field(field) => Some(DefKey::Field {
            class: field.class.binary_name(db),
            idx: field.idx,
        }),
        _ => None,
    }
}

fn container_name(db: &dyn Db, def: &Definition) -> Option<String> {
    match def {
        Definition::Class(class) => {
            let name = class.binary_name(db);
            let idx = name.rfind(['/', '$'])?;
            Some(name[..idx].replace(['/', '$'], "."))
        }
        _ => def.container_class().map(|class| class.qualified_name(db)),
    }
}

/// 把定义变成跳转目标。包没有位置，返回 `None`。
pub(crate) fn to_nav(db: &dyn Db, def: &Definition) -> Option<NavigationTarget> {
    let name = def.name(db)?;
    let kind = SymbolKind::of_definition(db, def);
    let container = container_name(db, def);
    let deprecated = ide_db::definition_flags(db, def).is_deprecated();
    let key = def_key(db, def);
    let detail = match def {
        Definition::Package(_) => None,
        _ => Some(ide_db::render::definition_signature(db, def)),
    };
    let target = |file, full_range, focus_range| NavigationTarget {
        file,
        full_range,
        focus_range,
        name: name.clone(),
        kind,
        container: container.clone(),
        detail: detail.clone(),
        deprecated,
        key: key.clone(),
    };

    if let Some(source) = def.source(db) {
        let file_id = source.file.file_id(db);
        return Some(target(
            NavFile::Source(file_id),
            source.full_range,
            source.focus_range,
        ));
    }

    let (class, member) = match def {
        Definition::Class(class) => (*class, None),
        Definition::Field(field) => {
            let member = LibraryMember {
                name: name.clone(),
                kind: LibraryMemberKind::Field,
                param_count: 0,
            };
            (field.class, Some(member))
        }
        Definition::Method(method) => {
            let data = class_data(db, method.class)?;
            let m = data.methods.get(method.idx as usize)?;
            let member = LibraryMember {
                name: name.clone(),
                kind: if m.is_constructor {
                    LibraryMemberKind::Constructor
                } else {
                    LibraryMemberKind::Method
                },
                param_count: m.params.len() as u32,
            };
            (method.class, Some(member))
        }
        _ => return None,
    };

    // 源码里的合成成员（隐式构造器、记录的访问器 …）没有自己的声明：落到所属的类上
    if let Some(class_source) = Definition::Class(class).source(db) {
        let file_id = class_source.file.file_id(db);
        return Some(target(
            NavFile::Source(file_id),
            class_source.full_range,
            class_source.focus_range,
        ));
    }

    let empty = TextRange::empty(TextSize::from(0));
    Some(target(
        NavFile::Library {
            class: class.binary_name(db),
            member,
        },
        empty,
        empty,
    ))
}

/// 光标下的名字指向的定义，以及这个名字的范围。
pub(crate) fn definition_at(
    db: &dyn Db,
    file: SourceFile,
    offset: TextSize,
) -> Option<(TextRange, Definition)> {
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let token = pick_ident_token(&root, offset)?;
    let range = token.text_range();
    let parent = token.parent()?;
    let def = match token.kind() {
        SyntaxKind::THIS_KW | SyntaxKind::SUPER_KW => {
            sema.explicit_constructor_target(file, &parent)?
        }
        _ => {
            if let Some(name_ref) = ast::NameRef::cast(parent.clone()) {
                sema.classify_name_ref(file, &name_ref)?
            } else {
                let name = ast::Name::cast(parent)?;
                sema.classify_name(file, &name)?
            }
        }
    };
    Some((range, def))
}

pub(crate) fn position_file(db: &dyn Db, position: FilePosition) -> Option<SourceFile> {
    base_db::source_file(db, position.file_id)
}

pub(crate) fn goto_definition(
    db: &dyn Db,
    position: FilePosition,
) -> Option<RangeInfo<Vec<NavigationTarget>>> {
    let file = position_file(db, position)?;
    let (range, def) = definition_at(db, file, position.offset)?;
    let targets: Vec<NavigationTarget> = to_nav(db, &def).into_iter().collect();
    Some(RangeInfo {
        range,
        info: targets,
    })
}

/// 方法 `method` 直接覆写 / 实现的父类型方法。
pub(crate) fn super_methods(db: &dyn Db, method: MethodId) -> Vec<MethodId> {
    let target = Definition::Method(method);
    let mut res = Vec::new();
    for sup in all_supers(db, method.class) {
        let Some(data) = class_data(db, sup) else {
            continue;
        };
        let Some(name) = target.name(db) else {
            continue;
        };
        for (idx, _) in data.methods_named(&name) {
            let candidate = MethodId { class: sup, idx };
            if refers_to(db, &Definition::Method(candidate), &target) {
                res.push(candidate);
            }
        }
    }
    res
}

/// 声明：对方法是它覆写的父类型方法（最近的一个），其余同定义。
pub(crate) fn goto_declaration(
    db: &dyn Db,
    position: FilePosition,
) -> Option<RangeInfo<Vec<NavigationTarget>>> {
    let file = position_file(db, position)?;
    let (range, def) = definition_at(db, file, position.offset)?;
    if let Definition::Method(method) = &def {
        if let Some(sup) = super_methods(db, *method).into_iter().next() {
            let targets: Vec<_> = to_nav(db, &Definition::Method(sup)).into_iter().collect();
            if !targets.is_empty() {
                return Some(RangeInfo {
                    range,
                    info: targets,
                });
            }
        }
    }
    Some(RangeInfo {
        range,
        info: to_nav(db, &def).into_iter().collect(),
    })
}

fn classes_of_ty(ty: &Ty, out: &mut Vec<ClassId>) {
    match ty {
        Ty::Class(class, _) => out.push(*class),
        Ty::Array(element) => classes_of_ty(element, out),
        Ty::Intersection(parts) => parts.iter().for_each(|part| classes_of_ty(part, out)),
        _ => {}
    }
}

pub(crate) fn goto_type_definition(
    db: &dyn Db,
    position: FilePosition,
) -> Option<RangeInfo<Vec<NavigationTarget>>> {
    let file = position_file(db, position)?;
    let (range, def) = definition_at(db, file, position.offset)?;
    let ty = match &def {
        Definition::Class(class) => Ty::class(*class),
        Definition::Field(field) => field_ty(db, *field),
        Definition::Method(method) => method_sig(db, *method).ret.clone(),
        Definition::Local(local) => hir::infer(db, local.body).local(local.id).ty.clone(),
        Definition::TypeParam(param) => hir::type_param_bounds(db, *param)
            .first()
            .cloned()
            .unwrap_or(Ty::Unknown),
        Definition::Package(_) => return None,
    };
    let mut classes = Vec::new();
    classes_of_ty(&ty.upper_bound(db), &mut classes);
    let targets: Vec<NavigationTarget> = classes
        .into_iter()
        .filter_map(|class| to_nav(db, &Definition::Class(class)))
        .collect();
    Some(RangeInfo {
        range,
        info: targets,
    })
}

/// `class` 的所有（直接和间接）子类型：工作区源码里的，加上依赖库里的。
pub(crate) fn all_subclasses(db: &dyn Db, class: ClassId) -> Vec<ClassId> {
    let mut res = Vec::new();
    let mut seen: FxHashSet<ClassId> = FxHashSet::default();
    seen.insert(class);

    let index = source_class_index(db);
    for (binary_name, _, _) in index.iter() {
        let candidate = ClassId::from_binary_name(db, binary_name);
        if candidate != class && is_subclass_of(db, candidate, class) && seen.insert(candidate) {
            res.push(candidate);
        }
    }

    // 依赖库：沿"直接子类型"表做广度优先遍历
    let class_path = workspace(db).class_path(db);
    let mut queue = vec![class.binary_name(db)];
    while let Some(name) = queue.pop() {
        for summary in class_path.direct_subtypes(&name) {
            if summary.is_anonymous_or_local() {
                continue;
            }
            let candidate = ClassId::from_binary_name(db, &summary.name);
            if seen.insert(candidate) {
                res.push(candidate);
                queue.push(summary.name.clone());
            }
        }
    }
    res
}

pub(crate) fn goto_implementation(
    db: &dyn Db,
    position: FilePosition,
) -> Option<RangeInfo<Vec<NavigationTarget>>> {
    let file = position_file(db, position)?;
    let (range, def) = definition_at(db, file, position.offset)?;
    let mut targets = Vec::new();
    match &def {
        Definition::Class(class) => {
            for sub in all_subclasses(db, *class) {
                targets.extend(to_nav(db, &Definition::Class(sub)));
            }
        }
        Definition::Method(method) => {
            let name = def.name(db)?;
            for sub in all_subclasses(db, method.class) {
                db.unwind_if_revision_cancelled();
                let Some(data) = class_data(db, sub) else {
                    continue;
                };
                for (idx, _) in data.methods_named(&name) {
                    let candidate = Definition::Method(MethodId { class: sub, idx });
                    if refers_to(db, &candidate, &def) {
                        targets.extend(to_nav(db, &candidate));
                    }
                }
            }
        }
        _ => targets.extend(to_nav(db, &def)),
    }
    Some(RangeInfo {
        range,
        info: targets,
    })
}

pub(crate) fn workspace_symbols(db: &dyn Db, query: &str, limit: usize) -> Vec<NavigationTarget> {
    // 带 `/` 的查询是在找 Web 端点（`/api/users`、`GET /orders`）
    if query.contains('/') {
        return crate::endpoints::search(db, query, limit);
    }
    // 只搜工作区源码：依赖库里的类没有现成的文件位置，逐个物化代价太高
    search_symbols(db, query, limit, false)
        .into_iter()
        .filter_map(|symbol| to_nav(db, &symbol.entry.def))
        .collect()
}
