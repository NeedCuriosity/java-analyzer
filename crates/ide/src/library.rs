//! 依赖库 / JDK 里的类：找源码包、生成签名桩代码、在源码文本里定位声明。
//!
//! 这里不做任何文件读写；解压源码、写缓存文件由宿主完成。

use base_db::{workspace, Db};
use hir::{
    class_data, class_supertypes, field_ty, lookup_class, member_type, method_sig,
    source_class_index, type_param_bounds, ClassId, ClassKind, FieldId, GenericOwner, MethodId,
    Subst, Ty, TypeParamId, Wildcard,
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

/// 依赖库类里的一个成员，用名字和形参类型标识（源码与 class 文件之间没有更可靠的对应）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LibraryMember {
    pub name: SmolStr,
    pub kind: LibraryMemberKind,
    /// 方法、构造器各形参擦除后的类型的简单名（`int`、`String`、`Object[]`）；
    /// 拿不准的类型为 `None`，和任何类型都对得上。字段没有形参。
    pub params: Vec<Option<SmolStr>>,
}

/// 类型擦除之后的简单名：`java/util/Map$Entry` → `Entry`，数组加 `[]`。
/// 源码文本里只能按写出来的名字比较，所以 class 文件这一侧也只取简单名。
pub(crate) fn erased_type_key(db: &dyn Db, ty: &Ty) -> Option<SmolStr> {
    fn key(db: &dyn Db, ty: &Ty) -> Option<String> {
        Some(match ty {
            Ty::Primitive(p) => p.name().to_owned(),
            Ty::Class(id, _) => simple_binary_name(&id.binary_name(db)).to_owned(),
            Ty::Array(element) => format!("{}[]", key(db, element)?),
            _ => return None,
        })
    }
    key(db, &ty.erasure(db)).map(SmolStr::from)
}

/// 二进制名的最后一段：去掉包名、外层类，以及编译器加在局部类名前面的序号。
fn simple_binary_name(binary_name: &str) -> &str {
    let last = binary_name.rsplit(['/', '$']).next().unwrap_or(binary_name);
    match last.trim_start_matches(|c: char| c.is_ascii_digit()) {
        "" => last,
        trimmed => trimmed,
    }
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

pub(crate) fn source_class_file(
    db: &dyn Db,
    binary_name: &str,
    output_dir: Option<&std::path::Path>,
) -> Option<base_db::FileId> {
    let index = source_class_index(db);
    // 匿名类、局部类在源码索引里没有名字：逐级去掉最后一段 `$...`，直到找到具名的外层类
    let mut name = binary_name;
    let declarations: Vec<base_db::SourceFile> = loop {
        let found: Vec<_> = index.declarations(name).map(|(file, _)| file).collect();
        if !found.is_empty() {
            break found;
        }
        let simple_start = name.rfind('/').map_or(0, |i| i + 1);
        let dollar = name[simple_start..].rfind('$')?;
        name = &name[..simple_start + dollar];
    };
    let preferred = output_dir.and_then(|output_dir| {
        let project = workspace(db).project(db);
        let module = project
            .modules
            .iter()
            .find(|m| m.output_dirs.iter().any(|o| o.path == output_dir))?;
        declarations.iter().copied().find(|file| {
            let dir = std::path::Path::new(file.dir(db).as_str());
            module
                .source_roots
                .iter()
                .any(|root| dir.starts_with(&root.path))
        })
    });
    Some(preferred.unwrap_or(declarations[0]).file_id(db))
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

/// 反编译方法体时从类路径读 class 文件。
struct StubEnv<'a> {
    db: &'a dyn Db,
    class_path: &'a classfile::ClassPath,
}

impl decompiler::ClassEnv for StubEnv<'_> {
    fn class_bytes(&self, binary_name: &str) -> Option<Vec<u8>> {
        self.class_path.read_class_bytes(binary_name)
    }

    fn class_exists(&self, binary_name: &str) -> bool {
        self.class_path.contains(binary_name)
    }

    fn is_generic(&self, binary_name: &str) -> bool {
        lookup_class(self.db, binary_name)
            .and_then(|c| class_data(self.db, c))
            .is_some_and(|d| !d.type_params.is_empty())
    }
}

/// 生成桩代码时反编译方法体的状态：方法体里的类名共用一张名字表，最后统一补 import。
struct BodyDecompiler<'a> {
    env: StubEnv<'a>,
    names: decompiler::NameTable,
}

/// 一个类的反编译器，以及签名模型里第 i 个方法对应的 class 文件方法下标。
struct ClassBodies {
    decompiler: decompiler::ClassDecompiler,
    methods: Vec<usize>,
}

impl BodyDecompiler<'_> {
    fn class(&self, data: &hir::ClassData) -> Option<ClassBodies> {
        if data.is_source {
            return None;
        }
        let bytes = self.env.class_path.read_class_bytes(&data.binary_name)?;
        let decompiler = decompiler::ClassDecompiler::new(&bytes).ok()?;
        let methods = decompiler.source_methods();
        // 两边的方法列表按同样的规则过滤，逐个核对名字，对不上就整个类不反编译
        let matches = methods.len() == data.methods.len()
            && methods.iter().zip(&data.methods).all(|(&i, m)| {
                let name = &decompiler.method(i).name;
                if m.is_constructor {
                    name == "<init>"
                } else {
                    name.as_str() == m.name.as_str()
                }
            });
        matches.then_some(ClassBodies {
            decompiler,
            methods,
        })
    }
}

fn render_class(
    db: &dyn Db,
    class: ClassId,
    indent: usize,
    out: &mut String,
    mut bodies: Option<&mut BodyDecompiler<'_>>,
) {
    let Some(data) = class_data(db, class) else {
        return;
    };
    let pad = "    ".repeat(indent);
    let inner = "    ".repeat(indent + 1);
    // 注解类型的适用位置（写全限定名，不用为它加 import）
    if let Some(targets) = data.targets.filter(|_| data.kind == ClassKind::Annotation) {
        let names: Vec<String> = targets
            .names()
            .map(|name| format!("java.lang.annotation.ElementType.{name}"))
            .collect();
        out.push_str(&pad);
        out.push_str(&format!(
            "@java.lang.annotation.Target({{{}}})\n",
            names.join(", ")
        ));
    }
    out.push_str(&pad);
    out.push_str(&class_header(db, class));
    out.push_str(" {\n");
    let class_bodies = bodies.as_deref().and_then(|b| b.class(&data));

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
    // 静态初始化块（枚举的初始化块只是在创建常量，接口不能有）
    if let (Some(b), Some(cb)) = (bodies.as_deref_mut(), &class_bodies) {
        if !matches!(
            data.kind,
            ClassKind::Enum | ClassKind::Interface | ClassKind::Annotation
        ) {
            if let Some(Ok(text)) = cb.decompiler.static_init(&mut b.names, &b.env, indent + 1) {
                out.push('\n');
                out.push_str(&inner);
                out.push_str("static ");
                out.push_str(&text);
                out.push('\n');
            }
        }
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
        if !has_body {
            out.push_str(";\n");
            continue;
        }
        // 枚举的 values / valueOf 是编译器生成的
        let enum_helper = data.kind == ClassKind::Enum
            && ((method.name == "values" && method.params.is_empty())
                || (method.name == "valueOf" && method.params.len() == 1));
        let body = match (bodies.as_deref_mut(), &class_bodies) {
            (Some(b), Some(cb)) if !enum_helper => {
                let id = MethodId {
                    class,
                    idx: idx as u32,
                };
                // 和声明头（`method_signature`）用一样的名字
                let names = hir::method_param_names(db, id);
                let declared: Vec<String> = (0..method_sig(db, id).params.len())
                    .map(|i| match names.get(i) {
                        Some(Some(name)) => name.to_string(),
                        _ => format!("arg{i}"),
                    })
                    .collect();
                Some(cb.decompiler.method_body(
                    cb.methods[idx],
                    Some(&declared),
                    &mut b.names,
                    &b.env,
                    indent + 1,
                ))
            }
            _ => None,
        };
        match body {
            Some(Ok(text)) => {
                out.push(' ');
                out.push_str(&text);
                out.push('\n');
            }
            Some(Err(fail)) => {
                out.push_str(&base_db::tr!(
                    " {{ /* 无法反编译：{} */ }}\n",
                    " {{ /* could not decompile: {} */ }}\n",
                    fail.0
                ));
            }
            None => out.push_str(" { /* compiled code */ }\n"),
        }
    }
    for name in &data.nested {
        if let Some(nested) = member_type(db, class, name) {
            // 继承来的成员类型不属于这个类体
            if class_data(db, nested)
                .is_some_and(|d| d.outer.as_deref() == Some(&*data.binary_name))
            {
                out.push('\n');
                render_class(db, nested, indent + 1, out, bodies.as_deref_mut());
            }
        }
    }
    out.push_str(&pad);
    out.push_str("}\n");
}

/// 当前类及其嵌套类（递归）的简单名：方法体里同名的别处的类不能用简单名。
fn nested_names(db: &dyn Db, class: ClassId, out: &mut Vec<String>) {
    let Some(data) = class_data(db, class) else {
        return;
    };
    for name in &data.nested {
        out.push(name.to_string());
        if let Some(nested) = member_type(db, class, name) {
            if class_data(db, nested)
                .is_some_and(|d| d.outer.as_deref() == Some(&*data.binary_name))
            {
                nested_names(db, nested, out);
            }
        }
    }
}

/// 生成一份桩代码（顶层类及其全部嵌套类）：签名来自签名模型；`decompile` 为真时
/// 方法体从字节码反编译，反编译不了的方法退回空方法体。
pub(crate) fn class_stub(db: &dyn Db, binary_name: &str, decompile: bool) -> Option<String> {
    let outermost = outermost_name(binary_name);
    let class = hir::binary_class(db, outermost).or_else(|| lookup_class(db, outermost))?;
    let data = class_data(db, class)?;
    let package = data.package();
    // 签名里的类型按简单名渲染，所以要把别的包里的类 import 进来
    let mut imports = std::collections::BTreeSet::new();
    collect_imports(db, class, package, &mut imports);

    let mut body = String::new();
    let mut bodies = decompile.then(|| {
        let mut names = decompiler::NameTable::new(package);
        names.set_top(outermost);
        for import in &imports {
            names.bind(&import.replace('.', "/"));
        }
        let mut nested = Vec::new();
        nested_names(db, class, &mut nested);
        for name in &nested {
            names.reserve(name);
        }
        BodyDecompiler {
            env: StubEnv {
                db,
                class_path: workspace(db).class_path(db),
            },
            names,
        }
    });
    render_class(db, class, 0, &mut body, bodies.as_mut());
    // 方法体里用到的类
    if let Some(b) = &bodies {
        imports.extend(b.names.imports().cloned());
    }

    let mut out = String::new();
    out.push_str(if decompile {
        base_db::trs!(
            "// 由 java-analyzer 根据 class 文件生成：没有找到源码，方法体是从字节码反编译的，和原始源码可能不同。\n\n",
            "// Generated by java-analyzer from the class file: no source was found, method bodies are decompiled from bytecode and may differ from the original source.\n\n"
        )
    } else {
        base_db::trs!(
            "// 由 java-analyzer 根据 class 文件的签名生成：没有找到源码，方法体不可用。\n\n",
            "// Generated by java-analyzer from class file signatures: no source was found, method bodies are unavailable.\n\n"
        )
    });
    if !package.is_empty() {
        out.push_str(&format!("package {};\n\n", package.replace('/', ".")));
    }
    for import in &imports {
        out.push_str(&format!("import {import};\n"));
    }
    if !imports.is_empty() {
        out.push('\n');
    }
    out.push_str(&body);
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
            // 这个类和各层外层类的类型参数（内层的在前，遮蔽外层的）
            let class_type_params: Vec<ast::TypeParam> = decl
                .syntax()
                .ancestors()
                .filter_map(ast::TypeDecl::cast)
                .filter_map(|d| d.type_param_list())
                .flat_map(|list| list.type_params())
                .collect();
            let callables: Vec<Callable> = body
                .syntax()
                .children()
                .filter_map(|node| {
                    let (name, type_params, params) =
                        if let Some(m) = ast::MethodDecl::cast(node.clone()) {
                            if member.kind != LibraryMemberKind::Method {
                                return None;
                            }
                            (m.name()?, m.type_param_list(), m.param_list())
                        } else {
                            let c = ast::ConstructorDecl::cast(node.clone())?;
                            if member.kind != LibraryMemberKind::Constructor {
                                return None;
                            }
                            (c.name()?, c.type_param_list(), c.param_list())
                        };
                    // 方法自己的类型参数遮蔽类的
                    let scope: Vec<ast::TypeParam> = type_params
                        .into_iter()
                        .flat_map(|list| list.type_params())
                        .chain(class_type_params.iter().cloned())
                        .collect();
                    let params = params.map_or_else(Vec::new, |list| {
                        list.params()
                            .map(|p| source_param_key(&p, &scope))
                            .collect()
                    });
                    Some(Callable { node, name, params })
                })
                .filter(|c| {
                    member.kind == LibraryMemberKind::Constructor || c.name.text() == member.name
                })
                .collect();
            pick_callable(
                &callables,
                &member.params,
                member.kind == LibraryMemberKind::Constructor,
            )
            .map(|c| (c.node.clone(), c.name.clone()))
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

/// 源码里的一个方法或构造器，以及它各形参擦除后的类型的简单名。
struct Callable {
    node: syntax::SyntaxNode,
    name: ast::Name,
    params: Vec<Option<String>>,
}

/// 源码里一个形参擦除后的类型的简单名，与 [`erased_type_key`] 的结果对应。
fn source_param_key(param: &ast::Param, scope: &[ast::TypeParam]) -> Option<String> {
    let mut key = source_type_key(&param.ty()?, scope, 0)?;
    // `String... args`、`String args[]`
    for _ in 0..param.extra_dims() + usize::from(param.is_varargs()) {
        key.push_str("[]");
    }
    Some(key)
}

fn source_type_key(ty: &ast::Type, scope: &[ast::TypeParam], depth: u32) -> Option<String> {
    // `<T extends U, U extends T>` 这类非法的循环上界
    if depth > 16 {
        return None;
    }
    match ty {
        ast::Type::PrimitiveType(p) => Some(p.keyword()?.text().to_owned()),
        ast::Type::ArrayType(array) => Some(format!(
            "{}[]",
            source_type_key(&array.element()?, scope, depth + 1)?
        )),
        ast::Type::ClassType(class) => {
            let name = class.name_ref()?.text();
            // 类型变量擦除成它的第一个上界，没有上界时是 `Object`
            let type_param = class
                .qualifier()
                .is_none()
                .then(|| {
                    scope
                        .iter()
                        .find(|p| p.name().is_some_and(|n| n.text() == name))
                })
                .flatten();
            match type_param {
                Some(param) => match param.bounds().next() {
                    Some(bound) => source_type_key(&bound, scope, depth + 1),
                    None => Some("Object".to_owned()),
                },
                None => Some(name.to_string()),
            }
        }
        _ => None,
    }
}

/// 在同名的方法（或构造器）里挑出 class 文件里的那一个：形参类型都对得上的优先；
/// 构造器还要考虑 class 文件里多出的前置合成形参（外层实例、枚举常量名和序号）；
/// 都对不上时（例如源码包和 jar 的版本不一致）退到形参个数相同的，最后是第一个。
fn pick_callable<'a>(
    callables: &'a [Callable],
    binary: &[Option<SmolStr>],
    is_constructor: bool,
) -> Option<&'a Callable> {
    let types_match = |binary: &[Option<SmolStr>], source: &[Option<String>]| {
        binary.len() == source.len()
            && binary.iter().zip(source).all(|(b, s)| match (b, s) {
                (Some(b), Some(s)) => b.as_str() == s.as_str(),
                _ => true,
            })
    };
    callables
        .iter()
        .find(|c| types_match(binary, &c.params))
        .or_else(|| {
            callables.iter().filter(|_| is_constructor).find(|c| {
                c.params.len() < binary.len()
                    && types_match(&binary[binary.len() - c.params.len()..], &c.params)
            })
        })
        .or_else(|| callables.iter().find(|c| c.params.len() == binary.len()))
        .or_else(|| callables.first())
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

        let method = member("m", LibraryMemberKind::Method, &[Some("int")]);
        let found = locate_in_library_source(text, "a/Outer", Some(&method)).unwrap();
        assert_eq!(slice(found.full_range), "void m(int a) {}");

        let field = member("x", LibraryMemberKind::Field, &[]);
        let found = locate_in_library_source(text, "a/Outer", Some(&field)).unwrap();
        assert_eq!(slice(found.focus_range), "x");

        let ctor = member("Inner", LibraryMemberKind::Constructor, &[Some("String")]);
        let found = locate_in_library_source(text, "a/Outer$Inner", Some(&ctor)).unwrap();
        assert_eq!(slice(found.focus_range), "Inner");
    }

    fn member(name: &str, kind: LibraryMemberKind, params: &[Option<&str>]) -> LibraryMember {
        LibraryMember {
            name: SmolStr::new(name),
            kind,
            params: params.iter().map(|p| p.map(SmolStr::new)).collect(),
        }
    }

    #[test]
    fn locates_overloads_by_erased_parameter_types() {
        let text = r#"package a;
import java.io.*;
import java.util.*;
public class Parser<E extends CharSequence> {
  public Parser(File file) {}
  public Parser(java.io.Reader reader) {}
  public Object parse(File inputFile) { return null; }
  public Object parse(Reader reader) { return null; }
  public Object parse(final java.io.InputStream inputStream) { return null; }
  private Object parse(org.xml.sax.InputSource inputSource) { return null; }
  public void add(E element) {}
  public void add(List<? extends E> elements) {}
  public <T extends Comparable<? super T>> void sort(T[] values) {}
  public void sort(Object[] values) {}
  public <T> void sort(List<T> values) {}
  public void join(String... parts) {}
  public void join(int @Annotated [] parts[]) {}
  public void join(Map.Entry<String, E> entry) {}
  public void join(char c) {}
  enum Kind { A; Kind(int code) {} Kind(String code) {} }
}
"#;
        let slice = |range: TextRange| &text[usize::from(range.start())..usize::from(range.end())];
        let locate = |class: &str, member: &LibraryMember| {
            let found = locate_in_library_source(text, class, Some(member)).unwrap();
            let full = slice(found.full_range);
            full.split('{').next().unwrap().trim().to_owned()
        };
        let method = |name: &str, params: &[Option<&str>]| {
            locate("a/Parser", &member(name, LibraryMemberKind::Method, params))
        };

        assert_eq!(
            method("parse", &[Some("InputStream")]),
            "public Object parse(final java.io.InputStream inputStream)"
        );
        assert_eq!(
            method("parse", &[Some("Reader")]),
            "public Object parse(Reader reader)"
        );
        assert_eq!(
            method("parse", &[Some("File")]),
            "public Object parse(File inputFile)"
        );
        assert_eq!(
            method("parse", &[Some("InputSource")]),
            "private Object parse(org.xml.sax.InputSource inputSource)"
        );
        // 类型变量擦除成上界
        assert_eq!(
            method("add", &[Some("CharSequence")]),
            "public void add(E element)"
        );
        assert_eq!(
            method("add", &[Some("List")]),
            "public void add(List<? extends E> elements)"
        );
        assert_eq!(
            method("sort", &[Some("Comparable[]")]),
            "public <T extends Comparable<? super T>> void sort(T[] values)"
        );
        assert_eq!(
            method("sort", &[Some("Object[]")]),
            "public void sort(Object[] values)"
        );
        assert_eq!(
            method("sort", &[Some("List")]),
            "public <T> void sort(List<T> values)"
        );
        // 可变参数、写在名字后面的维度、嵌套类型
        assert_eq!(
            method("join", &[Some("String[]")]),
            "public void join(String... parts)"
        );
        assert_eq!(
            method("join", &[Some("int[][]")]),
            "public void join(int @Annotated [] parts[])"
        );
        assert_eq!(
            method("join", &[Some("Entry")]),
            "public void join(Map.Entry<String, E> entry)"
        );
        // 拿不准的类型和任何类型都对得上
        assert_eq!(method("join", &[None]), "public void join(String... parts)");
        // 对不上时退回同名的第一个
        assert_eq!(
            method("parse", &[Some("Path")]),
            "public Object parse(File inputFile)"
        );

        let ctor = |class: &str, params: &[Option<&str>]| {
            locate(class, &member("", LibraryMemberKind::Constructor, params))
        };
        assert_eq!(
            ctor("a/Parser", &[Some("Reader")]),
            "public Parser(java.io.Reader reader)"
        );
        // 枚举构造器在 class 文件里多出常量名和序号
        assert_eq!(
            ctor(
                "a/Parser$Kind",
                &[Some("String"), Some("int"), Some("String")]
            ),
            "Kind(String code)"
        );
    }
}
