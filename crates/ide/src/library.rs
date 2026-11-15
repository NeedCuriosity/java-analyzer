//! 依赖库 / JDK 里的类：找源码包、生成签名桩代码、在源码文本里定位声明。
//!
//! 这里不做任何文件读写；解压源码、写缓存文件由宿主完成。

use base_db::{workspace, Db};
use hir::{
    class_data, class_supertypes, field_ty, lookup_class, member_type, method_sig,
    type_param_bounds, ClassId, ClassKind, FieldId, GenericOwner, MethodId, Subst, Ty, TypeParamId,
    Wildcard,
};
use ide_db::render::{class_header, field_signature, method_signature};
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    TextRange,
};

use crate::LibraryClassInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibraryMemberKind {
    Field,
    Method,
    Constructor,
}

/// 依赖库类里的一个成员，用名字和形参个数标识（源码与 class 文件之间没有更可靠的对应）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LibraryMember {
    pub name: SmolStr,
    pub kind: LibraryMemberKind,
    pub param_count: u32,
}

/// 物化后的源码位置描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySource {
    pub full_range: TextRange,
    pub focus_range: TextRange,
}

fn outermost_name(binary_name: &str) -> &str {
    let simple_start = binary_name.rfind('/').map_or(0, |i| i + 1);
    match binary_name[simple_start..].find('$') {
        Some(idx) => &binary_name[..simple_start + idx],
        None => binary_name,
    }
}

pub(crate) fn library_class_info(db: &dyn Db, binary_name: &str) -> Option<LibraryClassInfo> {
    let ws = workspace(db);
    let class_path = ws.class_path(db);
    let project = ws.project(db);
    let outermost = outermost_name(binary_name).to_owned();
    if !class_path.contains(&outermost) {
        return None;
    }
    let container = class_path
        .container_path(&outermost)
        .map(|p| p.to_path_buf());
    let sources = container.as_deref().and_then(|container| {
        if let Some(library) = project.library_by_jar(container) {
            return library.sources.clone();
        }
        let jdk = project.jdk.as_ref()?;
        container
            .starts_with(&jdk.home)
            .then(|| jdk.src_zip.clone())
            .flatten()
    });
    Some(LibraryClassInfo {
        outermost,
        container,
        sources,
    })
}

/// 类型里出现的类：`java.lang` 和本包之外的记下它最外层类的全限定名。
fn imports_of_ty(
    db: &dyn Db,
    ty: &Ty,
    package: &str,
    imports: &mut std::collections::BTreeSet<String>,
) {
    match ty {
        Ty::Class(class, args) => {
            let binary_name = class.binary_name(db);
            let outermost = outermost_name(&binary_name);
            let class_package = outermost.rsplit_once('/').map_or("", |(p, _)| p);
            if class_package != package && class_package != "java/lang" {
                imports.insert(outermost.replace('/', "."));
            }
            for arg in args.iter() {
                imports_of_ty(db, arg, package, imports);
            }
        }
        Ty::Array(element) => imports_of_ty(db, element, package, imports),
        Ty::Wildcard(Wildcard::Extends(bound) | Wildcard::Super(bound)) => {
            imports_of_ty(db, bound, package, imports)
        }
        Ty::Intersection(parts) => {
            for part in parts.iter() {
                imports_of_ty(db, part, package, imports);
            }
        }
        _ => {}
    }
}

/// 一个类（连同嵌套类）的签名里用到的全部类型。
fn collect_imports(
    db: &dyn Db,
    class: ClassId,
    package: &str,
    imports: &mut std::collections::BTreeSet<String>,
) {
    let Some(data) = class_data(db, class) else {
        return;
    };
    let mut bounds = |owner: GenericOwner, count: usize| {
        for idx in 0..count {
            let param = TypeParamId {
                owner,
                idx: idx as u32,
            };
            for bound in type_param_bounds(db, param).iter() {
                imports_of_ty(db, bound, package, imports);
            }
        }
    };
    bounds(GenericOwner::Class(class), data.type_params.len());
    for (idx, method) in data.methods.iter().enumerate() {
        let id = MethodId {
            class,
            idx: idx as u32,
        };
        bounds(GenericOwner::Method(id), method.type_params.len());
    }
    for sup in class_supertypes(db, class).iter() {
        imports_of_ty(db, sup, package, imports);
    }
    for idx in 0..data.fields.len() {
        let id = FieldId {
            class,
            idx: idx as u32,
        };
        imports_of_ty(db, &field_ty(db, id), package, imports);
    }
    for idx in 0..data.methods.len() {
        let id = MethodId {
            class,
            idx: idx as u32,
        };
        let sig = method_sig(db, id);
        for ty in sig.params.iter().chain(&sig.throws).chain([&sig.ret]) {
            imports_of_ty(db, ty, package, imports);
        }
    }
    for name in &data.nested {
        if let Some(nested) = member_type(db, class, name) {
            if class_data(db, nested)
                .is_some_and(|d| d.outer.as_deref() == Some(&*data.binary_name))
            {
                collect_imports(db, nested, package, imports);
            }
        }
    }
}

fn render_class(db: &dyn Db, class: ClassId, indent: usize, out: &mut String) {
    let Some(data) = class_data(db, class) else {
        return;
    };
    let pad = "    ".repeat(indent);
    let inner = "    ".repeat(indent + 1);
    out.push_str(&pad);
    out.push_str(&class_header(db, class));
    out.push_str(" {\n");

    let subst = Subst::empty();
    // 枚举常量必须排在类体最前面
    if data.kind == ClassKind::Enum {
        let constants: Vec<&str> = data
            .fields
            .iter()
            .filter(|f| f.is_enum_constant())
            .map(|f| f.name.as_str())
            .collect();
        if !constants.is_empty() {
            out.push_str(&inner);
            out.push_str(&constants.join(", "));
            out.push_str(";\n\n");
        }
    }
    for (idx, field) in data.fields.iter().enumerate() {
        if field.is_enum_constant() || field.flags.contains(hir::Modifiers::SYNTHETIC) {
            continue;
        }
        out.push_str(&inner);
        out.push_str(&field_signature(
            db,
            FieldId {
                class,
                idx: idx as u32,
            },
            &subst,
        ));
        out.push_str(";\n");
    }
    if !data.fields.is_empty() && !data.methods.is_empty() {
        out.push('\n');
    }
    for (idx, method) in data.methods.iter().enumerate() {
        if method.flags.contains(hir::Modifiers::SYNTHETIC) {
            continue;
        }
        out.push_str(&inner);
        out.push_str(&method_signature(
            db,
            MethodId {
                class,
                idx: idx as u32,
            },
            &subst,
        ));
        let has_body = !method.flags.is_abstract()
            && !method.flags.contains(hir::Modifiers::NATIVE)
            && data.kind != ClassKind::Annotation;
        out.push_str(if has_body {
            " { /* compiled code */ }\n"
        } else {
            ";\n"
        });
    }
    for name in &data.nested {
        if let Some(nested) = member_type(db, class, name) {
            // 继承来的成员类型不属于这个类体
            if class_data(db, nested)
                .is_some_and(|d| d.outer.as_deref() == Some(&*data.binary_name))
            {
                out.push('\n');
                render_class(db, nested, indent + 1, out);
            }
        }
    }
    out.push_str(&pad);
    out.push_str("}\n");
}

/// 按签名生成一份桩代码（顶层类及其全部嵌套类）。
pub(crate) fn class_stub(db: &dyn Db, binary_name: &str) -> Option<String> {
    let outermost = outermost_name(binary_name);
    let class = lookup_class(db, outermost)?;
    let data = class_data(db, class)?;
    let mut out = String::new();
    out.push_str(base_db::trs!("// 由 java-analyzer 根据 class 文件的签名生成：没有找到源码，方法体不可用。\n\n", "// Generated by java-analyzer from class file signatures: no source was found, method bodies are unavailable.\n\n"));
    let package = data.package();
    if !package.is_empty() {
        out.push_str(&format!("package {};\n\n", package.replace('/', ".")));
    }
    // 签名里的类型按简单名渲染，所以要把别的包里的类 import 进来
    let mut imports = std::collections::BTreeSet::new();
    collect_imports(db, class, package, &mut imports);
    for import in &imports {
        out.push_str(&format!("import {import};\n"));
    }
    if !imports.is_empty() {
        out.push('\n');
    }
    render_class(db, class, 0, &mut out);
    Some(out)
}

/// 在一份源码文本里找到 `binary_name` 这个类（及其成员）的声明位置。
/// 纯语法操作，不依赖数据库：宿主拿到源码文本后直接调用。
pub fn locate_in_library_source(
    text: &str,
    binary_name: &str,
    member: Option<&LibraryMember>,
) -> Option<LibrarySource> {
    let cu = ast::CompilationUnit::parse(text).tree();
    let simple_start = binary_name.rfind('/').map_or(0, |i| i + 1);
    let mut names = binary_name[simple_start..].split('$');

    let first = names.next()?;
    let mut decl = cu
        .type_decls()
        .find(|d| d.name().is_some_and(|n| n.text() == first))?;
    for name in names {
        decl = decl
            .body()?
            .syntax()
            .children()
            .filter_map(ast::TypeDecl::cast)
            .find(|d| d.name().is_some_and(|n| n.text() == name))?;
    }

    let class_source = || {
        let full_range = decl.syntax().text_range();
        let focus_range = decl.name().map_or(full_range, |n| n.syntax().text_range());
        LibrarySource {
            full_range,
            focus_range,
        }
    };
    let Some(member) = member else {
        return Some(class_source());
    };
    let Some(body) = decl.body() else {
        return Some(class_source());
    };

    let found = match member.kind {
        LibraryMemberKind::Field => body
            .enum_constants()
            .filter_map(|c| Some((c.syntax().clone(), c.name()?)))
            .chain(
                body.syntax()
                    .children()
                    .filter_map(ast::FieldDecl::cast)
                    .flat_map(|field| {
                        let node = field.syntax().clone();
                        field
                            .declarators()
                            .filter_map(move |d| Some((node.clone(), d.name()?)))
                            .collect::<Vec<_>>()
                    }),
            )
            .find(|(_, name)| name.text() == member.name),
        LibraryMemberKind::Method | LibraryMemberKind::Constructor => {
            let callables: Vec<(syntax::SyntaxNode, ast::Name, usize)> = body
                .syntax()
                .children()
                .filter_map(|node| {
                    let (name, params) = if let Some(m) = ast::MethodDecl::cast(node.clone()) {
                        if member.kind != LibraryMemberKind::Method {
                            return None;
                        }
                        (m.name()?, m.param_list())
                    } else {
                        let c = ast::ConstructorDecl::cast(node.clone())?;
                        if member.kind != LibraryMemberKind::Constructor {
                            return None;
                        }
                        (c.name()?, c.param_list())
                    };
                    let count = params.map_or(0, |p| p.params().count());
                    Some((node, name, count))
                })
                .filter(|(_, name, _)| {
                    member.kind == LibraryMemberKind::Constructor || name.text() == member.name
                })
                .collect();
            // 形参个数相同的优先；否则退回同名的第一个
            callables
                .iter()
                .find(|(_, _, count)| *count == member.param_count as usize)
                .or_else(|| callables.first())
                .map(|(node, name, _)| (node.clone(), name.clone()))
        }
    };
    match found {
        Some((node, name)) => Some(LibrarySource {
            full_range: node.text_range(),
            focus_range: name.syntax().text_range(),
        }),
        // 找不到成员（隐式构造器、记录的访问器 …）时落在类上
        None => Some(class_source()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outermost_strips_nested_names() {
        assert_eq!(outermost_name("java/util/Map$Entry"), "java/util/Map");
        assert_eq!(outermost_name("java/util/Map"), "java/util/Map");
        assert_eq!(outermost_name("Top$A$B"), "Top");
    }

    #[test]
    fn locates_members_by_name_and_arity() {
        let text = "package a;\nclass Outer {\n  int x;\n  void m() {}\n  void m(int a) {}\n  static class Inner { Inner(String s) {} }\n}\n";
        let slice = |range: TextRange| &text[usize::from(range.start())..usize::from(range.end())];

        let class = locate_in_library_source(text, "a/Outer$Inner", None).unwrap();
        assert_eq!(slice(class.focus_range), "Inner");

        let method = LibraryMember {
            name: SmolStr::new("m"),
            kind: LibraryMemberKind::Method,
            param_count: 1,
        };
        let found = locate_in_library_source(text, "a/Outer", Some(&method)).unwrap();
        assert_eq!(slice(found.full_range), "void m(int a) {}");

        let field = LibraryMember {
            name: SmolStr::new("x"),
            kind: LibraryMemberKind::Field,
            param_count: 0,
        };
        let found = locate_in_library_source(text, "a/Outer", Some(&field)).unwrap();
        assert_eq!(slice(found.focus_range), "x");

        let ctor = LibraryMember {
            name: SmolStr::new("Inner"),
            kind: LibraryMemberKind::Constructor,
            param_count: 1,
        };
        let found = locate_in_library_source(text, "a/Outer$Inner", Some(&ctor)).unwrap();
        assert_eq!(slice(found.focus_range), "Inner");
    }
}
