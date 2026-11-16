//! Spring Bean 注入。
//!
//! Bean 定义：工作区里带 `@Component` / `@Service` / `@Repository` / `@Controller` /
//! `@RestController` / `@Configuration` 等注解（或带着被这些注解标注过的自定义注解）的类，
//! `@FeignClient` / `@Mapper` 接口和 Spring Data 仓库接口，以及 `@Bean` 方法。
//!
//! 注入点：带 `@Autowired` / `@Inject` / `@Resource` 的字段，带 `@Autowired` / `@Inject` 的方法
//! （setter 等）的形参，Bean 类的构造器形参（唯一的构造器，或带 `@Autowired` 的那个；没有写
//! 构造器时 Lombok `@RequiredArgsConstructor` / `@AllArgsConstructor` 生成的构造器对应的字段），
//! 以及 `@Bean` 方法的形参。
//!
//! 先按类型找（泛型实参也比较），再按 `@Qualifier` / `@Named`、`@Resource(name = ...)`、
//! `@Primary` 和字段 / 形参名收窄；`List<Foo>`、`Set<Foo>`、`Map<String, Foo>`、`Foo[]` 收到全部
//! 候选。只看工作区里的定义：依赖库里自动配置的 Bean 看不到，所以这里不产生诊断。

use std::sync::Arc;

use base_db::{workspace, Db, FileId, FileKind, ModuleId, SourceFile, SourceRootKind};
use hir::{class_data, ClassId, Definition, Semantics, Ty};
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{
    endpoints::{annotations, attribute_strings},
    navigation::{to_nav, NavigationTarget},
};

/// 让类成为 Bean 的注解（简单名）。工作区里被它们标注的注解也算（见 [`stereotypes`]）。
const STEREOTYPES: &[&str] = &[
    "Component",
    "Service",
    "Repository",
    "Controller",
    "RestController",
    "Configuration",
    "ControllerAdvice",
    "RestControllerAdvice",
    "SpringBootApplication",
    "SpringBootConfiguration",
    "TestConfiguration",
    "Named",
];

/// `value` 就是 Bean 名的注解。
const NAMING: &[&str] = &[
    "Component",
    "Service",
    "Repository",
    "Controller",
    "RestController",
    "Configuration",
    "Named",
];

/// 让接口成为 Bean 的注解（实现由框架生成）。
const INTERFACE_BEANS: &[&str] = &[
    "FeignClient",
    "Mapper",
    "Repository",
    "RepositoryDefinition",
];

/// 字段上表示注入的注解。
const FIELD_INJECT: &[&str] = &["Autowired", "Inject", "Resource"];

/// 构造器 / 方法上表示注入的注解。
const METHOD_INJECT: &[&str] = &["Autowired", "Inject"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Bean {
    /// 类或 `@Bean` 方法
    def: Definition,
    file_id: FileId,
    /// 类名或方法名的范围
    focus_range: TextRange,
    /// Bean 名和别名（第一个是 Bean 名）
    names: Vec<SmolStr>,
    /// `@Qualifier` 的值
    qualifiers: Vec<SmolStr>,
    primary: bool,
    /// 类本身，或 `@Bean` 方法的返回类型
    ty: Ty,
    module: Option<ModuleId>,
    test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InjectionPoint {
    file_id: FileId,
    /// 字段名或形参名的范围
    name_range: TextRange,
    name: SmolStr,
    /// 声明的类型
    ty: Ty,
    /// `@Qualifier("x")` / `@Named("x")`
    qualifier: Option<SmolStr>,
    /// `@Resource`：先按名字找
    by_name: bool,
    /// `@Resource(name = "x")`
    resource_name: Option<SmolStr>,
    /// 注入点所在的类
    owner: ClassId,
    /// 构造器把注入的形参赋给的字段（`this.repo = repo`）：跳转和悬停时当作注入点，
    /// 查找 Bean 的引用时只列形参
    derived: bool,
    module: Option<ModuleId>,
    test: bool,
}

/// 文件所在的模块，以及是不是测试代码。
fn location(db: &dyn Db, file_id: FileId) -> (Option<ModuleId>, bool) {
    match workspace(db).files(db).get(file_id).map(|entry| entry.kind) {
        Some(FileKind::Workspace { module, root }) => (module, root == SourceRootKind::Test),
        _ => (None, false),
    }
}

fn has_annotation_text(text: &str, names: &[SmolStr]) -> bool {
    names.iter().any(|name| {
        text.match_indices('@')
            .any(|(i, _)| text[i + 1..].starts_with(name.as_str()))
    })
}

/// 类名对应的默认 Bean 名：首字母小写；前两个字母都是大写时不变（`URLParser`）。
fn decapitalize(name: &str) -> SmolStr {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(a), Some(b)) if a.is_uppercase() && b.is_uppercase() => SmolStr::new(name),
        (Some(a), _) => SmolStr::new(format!("{}{}", a.to_lowercase(), &name[a.len_utf8()..])),
        _ => SmolStr::new(name),
    }
}

fn has_modifier(node: &SyntaxNode, kind: SyntaxKind) -> bool {
    node.children()
        .find_map(ast::ModifierList::cast)
        .is_some_and(|m| m.has(kind))
}

fn first_string(annotation: &ast::Annotation, keys: &[&str]) -> Option<SmolStr> {
    attribute_strings(annotation, keys)
        .into_iter()
        .find(|s| !s.is_empty())
        .map(SmolStr::from)
}

fn find<'a>(list: &'a [(String, ast::Annotation)], names: &[&str]) -> Option<&'a ast::Annotation> {
    list.iter()
        .find(|(name, _)| names.contains(&name.as_str()))
        .map(|(_, a)| a)
}

/// 文件里的注解类型：名字和它身上注解的简单名。
#[salsa::tracked]
fn file_annotation_types(db: &dyn Db, file: SourceFile) -> Arc<Vec<(SmolStr, Vec<SmolStr>)>> {
    let text: &str = file.text(db);
    if !text.contains("@interface") {
        return Arc::new(Vec::new());
    }
    let root = base_db::parse(db, file).syntax_node();
    Arc::new(
        root.descendants()
            .filter(|n| n.kind() == SyntaxKind::ANNOTATION_TYPE_DECL)
            .filter_map(ast::TypeDecl::cast)
            .filter_map(|decl| {
                let names = annotations(decl.syntax())
                    .into_iter()
                    .map(|(name, _)| SmolStr::new(name))
                    .collect();
                Some((decl.name()?.text(), names))
            })
            .collect(),
    )
}

/// 让类成为 Bean 的注解：[`STEREOTYPES`]，加上工作区里（直接或间接）被它们标注的注解。
#[salsa::tracked]
fn stereotypes(db: &dyn Db) -> Arc<Vec<SmolStr>> {
    let mut res: Vec<SmolStr> = STEREOTYPES.iter().map(SmolStr::new).collect();
    let mut decls = Vec::new();
    for (_, file) in workspace(db).files(db).workspace_files() {
        db.unwind_if_revision_cancelled();
        decls.extend(file_annotation_types(db, file).iter().cloned());
    }
    loop {
        let before = res.len();
        for (name, marks) in &decls {
            if !res.contains(name) && marks.iter().any(|m| res.contains(m)) {
                res.push(name.clone());
            }
        }
        if res.len() == before {
            break;
        }
    }
    Arc::new(res)
}

fn is_stereotyped(list: &[(String, ast::Annotation)], stereotypes: &[SmolStr]) -> bool {
    list.iter()
        .any(|(name, _)| stereotypes.iter().any(|s| s == name))
}

/// 继承了 Spring Data 的 `Repository` 的接口。
fn is_spring_data(db: &dyn Db, class: ClassId) -> bool {
    hir::all_supers(db, class)
        .iter()
        .any(|sup| sup.binary_name(db) == "org/springframework/data/repository/Repository")
}

/// 类本身是不是 Bean（不含 `@Bean` 方法）。
fn class_bean(
    db: &dyn Db,
    decl: &ast::TypeDecl,
    class: ClassId,
    list: &[(String, ast::Annotation)],
    stereotypes: &[SmolStr],
) -> Option<(Vec<SmolStr>, Vec<SmolStr>, bool)> {
    let is_bean = match decl.syntax().kind() {
        SyntaxKind::CLASS_DECL | SyntaxKind::RECORD_DECL => {
            !has_modifier(decl.syntax(), SyntaxKind::ABSTRACT_KW)
                && is_stereotyped(list, stereotypes)
        }
        SyntaxKind::INTERFACE_DECL => {
            find(list, INTERFACE_BEANS).is_some() || is_spring_data(db, class)
        }
        _ => false,
    };
    if !is_bean {
        return None;
    }
    let explicit = list
        .iter()
        .filter(|(name, _)| NAMING.contains(&name.as_str()))
        .find_map(|(_, a)| first_string(a, &["value"]));
    let name = match explicit {
        Some(name) => name,
        None => decapitalize(&decl.name()?.text()),
    };
    let qualifiers = find(list, &["Qualifier"])
        .and_then(|a| first_string(a, &["value"]))
        .into_iter()
        .collect();
    Some((vec![name], qualifiers, find(list, &["Primary"]).is_some()))
}

/// 文件里的 Bean 定义（`module` / `test` 还没有填）。
#[salsa::tracked]
fn file_beans(db: &dyn Db, file: SourceFile) -> Arc<Vec<Bean>> {
    let text: &str = file.text(db);
    let stereotypes = stereotypes(db);
    let maybe = text.contains("@Bean")
        || text.contains("@FeignClient")
        || text.contains("@Mapper")
        || (text.contains("interface") && text.contains("Repository"))
        || has_annotation_text(text, &stereotypes);
    if !maybe {
        return Arc::new(Vec::new());
    }
    let file_id = file.file_id(db);
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let mut res = Vec::new();
    for decl in root.descendants().filter_map(ast::TypeDecl::cast) {
        let Some(class) = sema.class_of_decl(file, &decl) else {
            continue;
        };
        let list = annotations(decl.syntax());
        if let (Some((names, qualifiers, primary)), Some(name)) = (
            class_bean(db, &decl, class, &list, &stereotypes),
            decl.name(),
        ) {
            res.push(Bean {
                def: Definition::Class(class),
                file_id,
                focus_range: name.syntax().text_range(),
                names,
                qualifiers,
                primary,
                ty: Ty::class(class),
                module: None,
                test: false,
            });
        }
        let Some(body) = decl.body() else { continue };
        for method in body.syntax().children().filter_map(ast::MethodDecl::cast) {
            let method_annotations = annotations(method.syntax());
            let Some(bean) = find(&method_annotations, &["Bean"]) else {
                continue;
            };
            let (Some(name), Some(ret)) = (method.name(), method.return_type()) else {
                continue;
            };
            let ty = sema.resolve_type(file, &ret);
            if !matches!(ty, Ty::Class(..)) {
                continue;
            }
            let Some(method_id) = sema.method_of_decl(file, method.syntax()) else {
                continue;
            };
            let mut names: Vec<SmolStr> = attribute_strings(bean, &["value", "name"])
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(SmolStr::from)
                .collect();
            if names.is_empty() {
                names.push(name.text());
            }
            let qualifiers = find(&method_annotations, &["Qualifier"])
                .and_then(|a| first_string(a, &["value"]))
                .into_iter()
                .collect();
            res.push(Bean {
                def: Definition::Method(method_id),
                file_id,
                focus_range: name.syntax().text_range(),
                names,
                qualifiers,
                primary: find(&method_annotations, &["Primary"]).is_some(),
                ty,
                module: None,
                test: false,
            });
        }
    }
    Arc::new(res)
}

/// 工作区里的全部 Bean 定义。
#[salsa::tracked]
fn workspace_beans(db: &dyn Db) -> Arc<Vec<Bean>> {
    let files = workspace(db).files(db);
    let mut res = Vec::new();
    for (file_id, file) in files.workspace_files() {
        db.unwind_if_revision_cancelled();
        let found = file_beans(db, file);
        if found.is_empty() {
            continue;
        }
        let (module, test) = location(db, file_id);
        res.extend(found.iter().cloned().map(|mut bean| {
            bean.module = module;
            bean.test = test;
            bean
        }));
    }
    Arc::new(res)
}

/// 生成构造器的 Lombok 注解。
#[derive(Clone, Copy, PartialEq, Eq)]
enum LombokConstructor {
    /// `@RequiredArgsConstructor`（`@Data` 也有）：未初始化的 `final` 字段和 `@NonNull` 字段
    Required,
    /// `@AllArgsConstructor`：全部实例字段（已初始化的 `final` 字段除外）
    All,
}

struct PointCollector<'a> {
    db: &'a dyn Db,
    sema: Semantics<'a>,
    file: SourceFile,
    file_id: FileId,
    out: Vec<InjectionPoint>,
}

impl PointCollector<'_> {
    fn push(
        &mut self,
        owner: ClassId,
        name: &ast::Name,
        ty: &ast::Type,
        list: &[(String, ast::Annotation)],
        derived: bool,
    ) {
        let name_range = name.syntax().text_range();
        if self.out.iter().any(|p| p.name_range == name_range) {
            return;
        }
        let ty = self.sema.resolve_type(self.file, ty);
        if target_of(self.db, &ty).is_none() {
            return;
        }
        let resource = find(list, &["Resource"]);
        self.out.push(InjectionPoint {
            file_id: self.file_id,
            name_range,
            name: name.text(),
            ty,
            qualifier: find(list, &["Qualifier", "Named"])
                .and_then(|a| first_string(a, &["value"])),
            by_name: resource.is_some(),
            resource_name: resource.and_then(|a| first_string(a, &["name"])),
            owner,
            derived,
            module: None,
            test: false,
        });
    }

    fn params(&mut self, owner: ClassId, params: Option<ast::ParamList>) {
        for param in params.iter().flat_map(|l| l.params()) {
            let list = annotations(param.syntax());
            // `@Value("${...}")` 注入的是配置值
            if param.is_varargs() || find(&list, &["Value"]).is_some() {
                continue;
            }
            if let (Some(name), Some(ty)) = (param.name(), param.ty()) {
                self.push(owner, &name, &ty, &list, false);
            }
        }
    }

    /// 构造器里 `this.field = param` / `field = param` 赋值的字段：和形参收到同样的 Bean。
    fn assigned_fields(
        &mut self,
        owner: ClassId,
        body: &ast::ClassBody,
        ctor: &ast::ConstructorDecl,
    ) {
        let params: Vec<(SmolStr, Vec<(String, ast::Annotation)>)> = ctor
            .param_list()
            .iter()
            .flat_map(|l| l.params())
            .filter_map(|p| Some((p.name()?.text(), annotations(p.syntax()))))
            .collect();
        let Some(block) = ctor.body() else { return };
        let name_of = |expr: Option<ast::Expr>| -> Option<(SmolStr, bool)> {
            match expr? {
                ast::Expr::NameExpr(e) => Some((e.name_ref()?.text(), false)),
                ast::Expr::FieldAccess(e) => match e.receiver()? {
                    ast::Expr::ThisExpr(this) if this.qualifier().is_none() => {
                        Some((e.name_ref()?.text(), true))
                    }
                    _ => None,
                },
                _ => None,
            }
        };
        for assign in block
            .syntax()
            .descendants()
            .filter_map(ast::AssignExpr::cast)
        {
            if assign.op_token().is_none_or(|t| t.text() != "=") {
                continue;
            }
            let (Some((field, qualified)), Some((value, false))) =
                (name_of(assign.lhs()), name_of(assign.rhs()))
            else {
                continue;
            };
            // 没有 `this.` 时，和形参同名的名字指的是形参
            if !qualified && params.iter().any(|(name, _)| *name == field) {
                continue;
            }
            let Some((_, list)) = params.iter().find(|(name, _)| *name == value) else {
                continue;
            };
            if find(list, &["Value"]).is_some() {
                continue;
            }
            for decl in body.syntax().children().filter_map(ast::FieldDecl::cast) {
                if has_modifier(decl.syntax(), SyntaxKind::STATIC_KW) {
                    continue;
                }
                let Some(ty) = decl.ty() else { continue };
                for declarator in decl.declarators() {
                    if let Some(name) = declarator.name().filter(|n| n.text() == field) {
                        self.push(owner, &name, &ty, list, true);
                    }
                }
            }
        }
    }

    fn type_decl(&mut self, decl: &ast::TypeDecl, stereotypes: &[SmolStr]) {
        let Some(class) = self.sema.class_of_decl(self.file, decl) else {
            return;
        };
        let class_annotations = annotations(decl.syntax());
        let is_bean_class = matches!(
            decl.syntax().kind(),
            SyntaxKind::CLASS_DECL | SyntaxKind::RECORD_DECL
        ) && is_stereotyped(&class_annotations, stereotypes);
        let Some(body) = decl.body() else { return };
        let ctors: Vec<ast::ConstructorDecl> = body
            .syntax()
            .children()
            .filter_map(ast::ConstructorDecl::cast)
            .collect();
        let lombok = if !is_bean_class
            || !ctors.is_empty()
            || find(&class_annotations, &["NoArgsConstructor"]).is_some()
        {
            None
        } else if find(&class_annotations, &["AllArgsConstructor"]).is_some() {
            Some(LombokConstructor::All)
        } else if find(&class_annotations, &["RequiredArgsConstructor", "Data"]).is_some() {
            Some(LombokConstructor::Required)
        } else {
            None
        };
        for field in body.syntax().children().filter_map(ast::FieldDecl::cast) {
            if has_modifier(field.syntax(), SyntaxKind::STATIC_KW) {
                continue;
            }
            let list = annotations(field.syntax());
            let injected = find(&list, FIELD_INJECT).is_some();
            let is_final = has_modifier(field.syntax(), SyntaxKind::FINAL_KW);
            let non_null = find(&list, &["NonNull"]).is_some();
            let Some(ty) = field.ty() else { continue };
            for declarator in field.declarators() {
                let initialized = declarator.initializer().is_some();
                let generated = match lombok {
                    Some(LombokConstructor::All) => !(is_final && initialized),
                    Some(LombokConstructor::Required) => !initialized && (is_final || non_null),
                    None => false,
                };
                if !injected && !generated {
                    continue;
                }
                if let Some(name) = declarator.name() {
                    self.push(class, &name, &ty, &list, false);
                }
            }
        }
        if is_bean_class {
            if decl.syntax().kind() == SyntaxKind::RECORD_DECL && ctors.is_empty() {
                self.params(class, decl.record_components());
            }
            let marked: Vec<&ast::ConstructorDecl> = ctors
                .iter()
                .filter(|c| find(&annotations(c.syntax()), METHOD_INJECT).is_some())
                .collect();
            let chosen = if marked.is_empty() && ctors.len() == 1 {
                vec![&ctors[0]]
            } else {
                marked
            };
            for ctor in chosen {
                self.params(class, ctor.param_list());
                self.assigned_fields(class, &body, ctor);
            }
        }
        for method in body.syntax().children().filter_map(ast::MethodDecl::cast) {
            let list = annotations(method.syntax());
            if find(&list, METHOD_INJECT).is_some() || find(&list, &["Bean"]).is_some() {
                self.params(class, method.param_list());
            }
        }
    }
}

/// 文件里的注入点（`module` / `test` 还没有填）。
#[salsa::tracked]
fn file_injection_points(db: &dyn Db, file: SourceFile) -> Arc<Vec<InjectionPoint>> {
    let text: &str = file.text(db);
    let stereotypes = stereotypes(db);
    let maybe = ["@Autowired", "@Inject", "@Resource", "@Bean"]
        .iter()
        .any(|a| text.contains(a))
        || has_annotation_text(text, &stereotypes);
    if !maybe {
        return Arc::new(Vec::new());
    }
    let mut collector = PointCollector {
        db,
        sema: Semantics::new(db),
        file,
        file_id: file.file_id(db),
        out: Vec::new(),
    };
    let root = base_db::parse(db, file).syntax_node();
    for decl in root.descendants().filter_map(ast::TypeDecl::cast) {
        collector.type_decl(&decl, &stereotypes);
    }
    Arc::new(collector.out)
}

/// 工作区里的全部注入点。
#[salsa::tracked]
fn workspace_injection_points(db: &dyn Db) -> Arc<Vec<InjectionPoint>> {
    let files = workspace(db).files(db);
    let mut res = Vec::new();
    for (file_id, file) in files.workspace_files() {
        db.unwind_if_revision_cancelled();
        let found = file_injection_points(db, file);
        if found.is_empty() {
            continue;
        }
        let (module, test) = location(db, file_id);
        res.extend(found.iter().cloned().map(|mut point| {
            point.module = module;
            point.test = test;
            point
        }));
    }
    Arc::new(res)
}

/// 每个模块（按编号）能看到的模块：自己和（间接）依赖的模块。
#[salsa::tracked]
fn module_visibility(db: &dyn Db) -> Arc<Vec<Vec<ModuleId>>> {
    let project = workspace(db).project(db);
    let mut res = Vec::new();
    for idx in 0..project.modules.len() {
        let mut seen = vec![ModuleId(idx as u32)];
        let mut i = 0;
        while let Some(module) = seen.get(i).copied() {
            i += 1;
            let Some(data) = project.modules.get(module.0 as usize) else {
                continue;
            };
            for dependency in &data.dependencies {
                if let base_db::DependencyTarget::Module(dep)
                | base_db::DependencyTarget::ModuleTests(dep) = dependency.target
                {
                    if !seen.contains(&dep) {
                        seen.push(dep);
                    }
                }
            }
        }
        res.push(seen);
    }
    Arc::new(res)
}

fn visible(visibility: &[Vec<ModuleId>], point: &InjectionPoint, bean: &Bean) -> bool {
    if bean.test && !point.test {
        return false;
    }
    match (point.module, bean.module) {
        (Some(from), Some(to)) => visibility
            .get(from.0 as usize)
            .is_none_or(|seen| seen.contains(&to)),
        _ => true,
    }
}

/// 注入点要找的 Bean 类型，以及是不是收集全部候选（集合、数组、`Map<String, T>`）。
fn target_of(db: &dyn Db, ty: &Ty) -> Option<(Ty, bool)> {
    let (inner, multiple) = match ty {
        Ty::Array(element) => ((**element).clone(), true),
        Ty::Class(class, args) => match class.binary_name(db).as_str() {
            "java/util/List" | "java/util/Collection" | "java/util/Set" | "java/util/SortedSet" => {
                (args.first()?.clone(), true)
            }
            "java/util/Map" => {
                let [key, value] = &args[..] else {
                    return None;
                };
                if key.as_class()?.binary_name(db) != "java/lang/String" {
                    return None;
                }
                (value.clone(), true)
            }
            "java/util/Optional"
            | "org/springframework/beans/factory/ObjectProvider"
            | "org/springframework/beans/factory/ObjectFactory"
            | "javax/inject/Provider"
            | "jakarta/inject/Provider" => (args.first()?.clone(), false),
            _ => (ty.clone(), false),
        },
        _ => return None,
    };
    let inner = inner.upper_bound(db);
    let class = inner.as_class()?;
    let excluded = matches!(
        class.binary_name(db).as_str(),
        "java/lang/Object" | "java/lang/String"
    );
    (!excluded && !inner.contains_unknown()).then_some((inner, multiple))
}

/// 注入点收到的 Bean。
fn candidates<'a>(db: &dyn Db, point: &InjectionPoint, beans: &'a [Bean]) -> Vec<&'a Bean> {
    let Some((target, multiple)) = target_of(db, &point.ty) else {
        return Vec::new();
    };
    let visibility = module_visibility(db);
    let mut found: Vec<&Bean> = beans
        .iter()
        .filter(|bean| visible(&visibility, point, bean) && hir::is_subtype(db, &bean.ty, &target))
        .collect();
    if point.by_name {
        let name = point.resource_name.as_ref().unwrap_or(&point.name);
        let named: Vec<&Bean> = found
            .iter()
            .copied()
            .filter(|bean| bean.names.contains(name))
            .collect();
        if !named.is_empty() || point.resource_name.is_some() {
            return named;
        }
    }
    if let Some(qualifier) = &point.qualifier {
        found.retain(|bean| bean.qualifiers.contains(qualifier) || bean.names.contains(qualifier));
        return found;
    }
    if multiple || found.len() <= 1 {
        return found;
    }
    // 注入到同类型的 Bean 自己里（装饰器）：有别的候选时不算自己
    let own = Definition::Class(point.owner);
    if found.iter().any(|bean| bean.def != own) {
        found.retain(|bean| bean.def != own);
    }
    let primary: Vec<&Bean> = found.iter().copied().filter(|bean| bean.primary).collect();
    if primary.len() == 1 {
        return primary;
    }
    let named: Vec<&Bean> = found
        .iter()
        .copied()
        .filter(|bean| bean.names.contains(&point.name))
        .collect();
    if named.len() == 1 {
        return named;
    }
    found
}

/// 字段或形参的定义对应的注入点。
fn point_of(db: &dyn Db, def: &Definition) -> Option<InjectionPoint> {
    if !matches!(def, Definition::Field(_) | Definition::Local(_)) {
        return None;
    }
    let source = def.source(db)?;
    let mut point = file_injection_points(db, source.file)
        .iter()
        .find(|p| p.name_range == source.focus_range)
        .cloned()?;
    (point.module, point.test) = location(db, point.file_id);
    Some(point)
}

/// 类或方法的定义对应的 Bean。
fn bean_of(db: &dyn Db, def: &Definition) -> Option<Bean> {
    if !matches!(def, Definition::Class(_) | Definition::Method(_)) {
        return None;
    }
    let source = def.source(db)?;
    file_beans(db, source.file)
        .iter()
        .find(|b| b.focus_range == source.focus_range && b.def == *def)
        .cloned()
}

/// 注入点收到的 Bean 定义（跳转到实现时用）。不是注入点时返回 `None`。
pub(crate) fn injected_beans(db: &dyn Db, def: &Definition) -> Option<Vec<NavigationTarget>> {
    let point = point_of(db, def)?;
    let beans = workspace_beans(db);
    Some(
        candidates(db, &point, &beans)
            .into_iter()
            .filter_map(|bean| to_nav(db, &bean.def))
            .collect(),
    )
}

/// 收到这个 Bean（类或 `@Bean` 方法）的注入点：`(文件, 字段名或形参名的范围)`。
pub(crate) fn injection_points_of(db: &dyn Db, def: &Definition) -> Vec<(FileId, TextRange)> {
    let Some(bean) = bean_of(db, def) else {
        return Vec::new();
    };
    let beans = workspace_beans(db);
    workspace_injection_points(db)
        .iter()
        .filter(|point| {
            !point.derived
                && target_of(db, &point.ty)
                    .is_some_and(|(target, _)| hir::is_subtype(db, &bean.ty, &target))
                && candidates(db, point, &beans)
                    .iter()
                    .any(|candidate| candidate.def == bean.def)
        })
        .map(|point| (point.file_id, point.name_range))
        .collect()
}

/// `OwnerService`、`AppConfig.restTemplate()`
fn label(db: &dyn Db, bean: &Bean) -> String {
    let class_name = |class: ClassId| class_data(db, class).map(|d| d.name.to_string());
    match &bean.def {
        Definition::Method(method) => {
            let name = bean.def.name(db).unwrap_or_default();
            match class_name(method.class) {
                Some(class) => format!("{class}.{name}()"),
                None => format!("{name}()"),
            }
        }
        Definition::Class(class) => class_name(*class).unwrap_or_default(),
        _ => String::new(),
    }
}

/// 悬停时附加的信息：注入点列出收到的 Bean，Bean 定义显示 Bean 名。
pub(crate) fn hover_lines(db: &dyn Db, def: &Definition) -> Option<String> {
    if let Some(point) = point_of(db, def) {
        let beans = workspace_beans(db);
        let found = candidates(db, &point, &beans);
        if found.is_empty() {
            return None;
        }
        let mut lines = vec![base_db::tr!("注入的 Bean：", "Injected beans:")];
        for bean in found {
            lines.push(format!("`{}` → `{}`", bean.names[0], label(db, bean)));
        }
        return Some(lines.join("  \n"));
    }
    let bean = bean_of(db, def)?;
    let names = bean.names.join("`, `");
    Some(base_db::tr!(
        "Spring Bean `{names}`",
        "Spring bean `{names}`"
    ))
}

#[cfg(test)]
mod tests {
    use super::decapitalize;

    #[test]
    fn default_bean_names() {
        assert_eq!(decapitalize("OwnerService"), "ownerService");
        assert_eq!(decapitalize("URLParser"), "URLParser");
        assert_eq!(decapitalize("A"), "a");
        assert_eq!(decapitalize(""), "");
    }
}
