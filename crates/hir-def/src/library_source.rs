//! 依赖库 / JDK 里的类在源码包里的声明：形参名和文档注释。
//!
//! class 文件里没有文档，抽象方法和接口方法也没有形参名（形参名来自局部变量表，
//! 只有带方法体的方法才有）。有源码包（`src.zip`、sources jar）时从源码里补上。
//! 按最外层的类惰性解析，只在展示签名、文档时才用到，不影响类型检查。

use std::sync::Arc;

use base_db::{workspace, Db, Workspace};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use syntax::ast::{self, AstNode};

use crate::{class_data, ClassId, MethodData, MethodId, Name, TypeRef};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LibraryMethodSource {
    pub name: SmolStr,
    pub is_constructor: bool,
    /// 每个形参的（类型的简单名，名字）
    pub params: Vec<(String, SmolStr)>,
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryClassSource {
    pub doc: Option<String>,
    /// 字段和枚举常量的文档
    pub fields: FxHashMap<SmolStr, String>,
    pub methods: Vec<LibraryMethodSource>,
}

type SourceFile = FxHashMap<SmolStr, Arc<LibraryClassSource>>;

/// 源码里写的类型 → 用来和 class 文件里的形参类型比对的简单名：
/// 去掉包名、外层类名和类型实参，数组维数保留。
fn source_type_key(param: &ast::Param) -> String {
    let text = param
        .ty()
        .map(|ty| ty.syntax().text().to_string())
        .unwrap_or_default();
    let mut base = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 && !c.is_whitespace() => base.push(c),
            _ => {}
        }
    }
    let dims = base.matches("[]").count() + param.extra_dims() + usize::from(param.is_varargs());
    let name = base.replace("[]", "");
    let simple = name.rsplit('.').next().unwrap_or(&name);
    format!("{simple}{}", "[]".repeat(dims))
}

fn binary_type_key(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Primitive(p) => p.name().to_owned(),
        TypeRef::Class { name, .. } => name
            .rsplit(['/', '$'])
            .next()
            .unwrap_or(name.as_str())
            .to_owned(),
        TypeRef::TypeVar(name) => name.to_string(),
        TypeRef::Array(element) => format!("{}[]", binary_type_key(element)),
        _ => String::new(),
    }
}

fn doc_text(token: Option<syntax::SyntaxToken>) -> Option<String> {
    token.map(|t| t.text().to_owned())
}

fn collect_class(decl: &ast::TypeDecl, binary_name: &str, out: &mut SourceFile) {
    let mut class = LibraryClassSource {
        doc: doc_text(decl.doc_comment()),
        ..Default::default()
    };
    let params_of = |list: Option<ast::ParamList>| -> Vec<(String, SmolStr)> {
        list.map(|list| {
            list.params()
                .map(|p| {
                    let name = p.name().map(|n| SmolStr::new(n.text())).unwrap_or_default();
                    (source_type_key(&p), name)
                })
                .collect()
        })
        .unwrap_or_default()
    };
    if let Some(body) = decl.body() {
        for constant in body.enum_constants() {
            if let (Some(name), Some(doc)) = (
                constant.name(),
                doc_text(ast::doc_comment(constant.syntax())),
            ) {
                class.fields.insert(SmolStr::new(name.text()), doc);
            }
        }
        for node in body.syntax().children() {
            if let Some(method) = ast::MethodDecl::cast(node.clone()) {
                let Some(name) = method.name() else { continue };
                class.methods.push(LibraryMethodSource {
                    name: SmolStr::new(name.text()),
                    is_constructor: false,
                    params: params_of(method.param_list()),
                    doc: doc_text(ast::doc_comment(&node)),
                });
            } else if let Some(constructor) = ast::ConstructorDecl::cast(node.clone()) {
                let Some(name) = constructor.name() else {
                    continue;
                };
                class.methods.push(LibraryMethodSource {
                    name: SmolStr::new(name.text()),
                    is_constructor: true,
                    params: params_of(constructor.param_list()),
                    doc: doc_text(ast::doc_comment(&node)),
                });
            } else if let Some(field) = ast::FieldDecl::cast(node.clone()) {
                if let Some(doc) = doc_text(ast::doc_comment(&node)) {
                    for declarator in field.declarators() {
                        if let Some(name) = declarator.name() {
                            class.fields.insert(SmolStr::new(name.text()), doc.clone());
                        }
                    }
                }
            } else if let Some(nested) = ast::TypeDecl::cast(node) {
                if let Some(name) = nested.name() {
                    let nested_name = format!("{binary_name}${}", name.text());
                    collect_class(&nested, &nested_name, out);
                }
            }
        }
    }
    out.insert(SmolStr::new(binary_name), Arc::new(class));
}

fn outermost_name(binary_name: &str) -> &str {
    let simple_start = binary_name.rfind('/').map_or(0, |i| i + 1);
    match binary_name[simple_start..].find('$') {
        Some(idx) => &binary_name[..simple_start + idx],
        None => binary_name,
    }
}

/// 最外层类 `outermost` 所在的源文件里声明的全部类。没有源码包时为 `None`。
#[salsa::tracked]
fn library_source_file(db: &dyn Db, ws: Workspace, outermost: SmolStr) -> Option<Arc<SourceFile>> {
    let class_path = ws.class_path(db);
    let project = ws.project(db);
    let container = class_path.container_path(&outermost)?;
    let archive = match project.library_by_jar(container) {
        Some(library) => library.sources.clone()?,
        None => {
            let jdk = project.jdk.as_ref()?;
            if !container.starts_with(&jdk.home) {
                return None;
            }
            jdk.src_zip.clone()?
        }
    };
    let text = classfile::read_source(&archive, &format!("{outermost}.java"))?;
    let unit = ast::CompilationUnit::parse(&text).tree();
    let simple = outermost.rsplit('/').next().unwrap_or(&outermost);
    let mut classes = SourceFile::default();
    for decl in unit.type_decls() {
        let Some(name) = decl.name() else { continue };
        // 同一个文件里的其它顶层类和它同包
        let binary_name = if name.text() == simple {
            outermost.to_string()
        } else {
            match outermost.rsplit_once('/') {
                Some((package, _)) => format!("{package}/{}", name.text()),
                None => name.text().to_string(),
            }
        };
        collect_class(&decl, &binary_name, &mut classes);
    }
    Some(Arc::new(classes))
}

/// 依赖库里的类在源码包里的声明。源码里的类（或没有源码包的库）返回 `None`。
pub fn library_class_source(db: &dyn Db, class: ClassId) -> Option<Arc<LibraryClassSource>> {
    let data = class_data(db, class)?;
    if data.is_source {
        return None;
    }
    let outermost = SmolStr::new(outermost_name(&data.binary_name));
    library_source_file(db, workspace(db), outermost)?
        .get(&data.binary_name)
        .cloned()
}

fn find_method<'a>(
    source: &'a LibraryClassSource,
    method: &MethodData,
) -> Option<&'a LibraryMethodSource> {
    let candidates: Vec<&LibraryMethodSource> = source
        .methods
        .iter()
        .filter(|m| {
            m.is_constructor == method.is_constructor
                && (m.is_constructor || m.name == method.name.as_str())
                && m.params.len() == method.params.len()
        })
        .collect();
    if candidates.len() <= 1 {
        return candidates.first().copied();
    }
    // 同名同参数个数的重载：按形参类型的简单名区分
    let keys: Vec<String> = method
        .params
        .iter()
        .map(|p| binary_type_key(&p.ty))
        .collect();
    candidates
        .into_iter()
        .find(|m| m.params.iter().map(|(key, _)| key).eq(keys.iter()))
}

/// 依赖库方法在源码包里的声明。
pub fn library_method_source(db: &dyn Db, method: MethodId) -> Option<LibraryMethodSource> {
    let data = class_data(db, method.class)?;
    let method_data = data.methods.get(method.idx as usize)?;
    let source = library_class_source(db, method.class)?;
    find_method(&source, method_data).cloned()
}

/// 方法每个形参的名字：class 文件里有就用它的，没有的话从源码包里找；都没有时为 `None`。
pub fn method_param_names(db: &dyn Db, method: MethodId) -> Vec<Option<Name>> {
    let Some(data) = class_data(db, method.class) else {
        return Vec::new();
    };
    let Some(method_data) = data.methods.get(method.idx as usize) else {
        return Vec::new();
    };
    let names: Vec<Option<Name>> = method_data.params.iter().map(|p| p.name.clone()).collect();
    if data.is_source || names.iter().all(Option::is_some) {
        return names;
    }
    match library_method_source(db, method) {
        Some(source) => source
            .params
            .iter()
            .zip(names)
            .map(|((_, name), known)| known.or_else(|| (!name.is_empty()).then(|| Name::new(name))))
            .collect(),
        None => names,
    }
}
