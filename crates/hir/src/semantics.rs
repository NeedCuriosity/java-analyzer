//! 语法 → 语义的桥梁。

use base_db::{Db, SourceFile};
use hir_def::{
    class_data, class_decl_file, item_tree, lookup_class, lower_type, member_type, package_exists,
    resolve_qualified_class, resolve_type_path, source_map, ClassId, FieldId, GenericOwner,
    MethodId, Name, TypeNs, TypeParamId, TypeScope,
};
use hir_ty::{infer, lookup_field, lookup_methods, lower_type_ref, BodyId, Resolution, Ty};
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode,
};

use crate::definition::{Definition, LocalDef};

pub struct Semantics<'db> {
    pub db: &'db dyn Db,
}

impl<'db> Semantics<'db> {
    pub fn new(db: &'db dyn Db) -> Semantics<'db> {
        Semantics { db }
    }

    pub fn parse(&self, file: SourceFile) -> ast::CompilationUnit {
        base_db::parse(self.db, file).tree()
    }

    /// 紧凑源文件的隐式类。
    pub fn implicit_class(&self, file: SourceFile) -> Option<ClassId> {
        let tree = item_tree(self.db, file);
        let class = tree
            .classes
            .iter()
            .find(|c| c.flags.contains(hir_def::Modifiers::IMPLICIT_CLASS))?;
        Some(ClassId::from_binary_name(self.db, &class.binary_name))
    }

    /// 类型声明节点对应的类。局部类和匿名类没有 `ClassId`，返回 `None`。
    pub fn class_of_decl(&self, file: SourceFile, decl: &ast::TypeDecl) -> Option<ClassId> {
        // 自内向外收集名字；中途遇到不是"类体里的成员类"的情况说明是局部类
        let mut names: Vec<Name> = vec![decl.name()?.text()];
        let mut node = decl.syntax().clone();
        loop {
            let parent = node.parent()?;
            match parent.kind() {
                SyntaxKind::COMPILATION_UNIT => break,
                SyntaxKind::CLASS_BODY => {
                    let outer = ast::TypeDecl::cast(parent.parent()?)?;
                    names.push(outer.name()?.text());
                    node = outer.syntax().clone();
                }
                _ => return None,
            }
        }
        // 紧凑源文件里的顶层类型是隐式类的成员
        if let Some(implicit) = self.implicit_class(file) {
            names.push(Name::new(implicit.binary_name(self.db)));
        }
        names.reverse();
        let tree = item_tree(self.db, file);
        let mut binary_name = String::new();
        if !tree.package.is_empty() {
            binary_name.push_str(&tree.package);
            binary_name.push('/');
        }
        binary_name.push_str(&names.join("$"));
        tree.class_idx(&binary_name)?;
        Some(ClassId::from_binary_name(self.db, &binary_name))
    }

    /// 方法体里声明的类型（局部类，以及局部类的成员类）对应的类。
    ///
    /// 和 [`Self::class_of_decl`] 分开：局部类的成员仍然属于外层的体（在那里就地推断），
    /// 找"体"和作用域时不能把它们当作独立的类。
    pub fn local_class_of_decl(&self, file: SourceFile, decl: &ast::TypeDecl) -> Option<ClassId> {
        let idx = source_map(self.db, file).find(decl.syntax())?;
        let tree = item_tree(self.db, file);
        let class = tree.classes.iter().find(|c| c.decl == Some(idx))?;
        Some(ClassId::from_binary_name(self.db, &class.binary_name))
    }

    /// 局部类里声明的方法。
    fn local_class_method(&self, file: SourceFile, node: &SyntaxNode) -> Option<MethodId> {
        let body = node
            .parent()
            .filter(|p| p.kind() == SyntaxKind::CLASS_BODY)?;
        let class = self.local_class_of_decl(file, &ast::TypeDecl::cast(body.parent()?)?)?;
        let decl = source_map(self.db, file).find(node)?;
        let idx = class_data(self.db, class)?
            .methods
            .iter()
            .position(|m| m.decl == Some(decl))?;
        Some(MethodId {
            class,
            idx: idx as u32,
        })
    }

    /// 包含 `node` 的最内层具名类（紧凑源文件的顶层成员属于隐式类）。
    pub fn enclosing_class(&self, file: SourceFile, node: &SyntaxNode) -> Option<ClassId> {
        node.ancestors()
            .filter_map(ast::TypeDecl::cast)
            .find_map(|decl| self.class_of_decl(file, &decl))
            .or_else(|| self.implicit_class(file))
    }

    /// 类体的直接成员节点所属的具名类。
    fn member_owner(&self, file: SourceFile, member: &SyntaxNode) -> Option<ClassId> {
        let parent = member.parent()?;
        if parent.kind() == SyntaxKind::COMPILATION_UNIT {
            return self.implicit_class(file);
        }
        let body = Some(parent).filter(|p| p.kind() == SyntaxKind::CLASS_BODY)?;
        let decl = ast::TypeDecl::cast(body.parent()?)?;
        self.class_of_decl(file, &decl)
    }

    pub fn method_of_decl(&self, file: SourceFile, node: &SyntaxNode) -> Option<MethodId> {
        let class = self.member_owner(file, node)?;
        if class_decl_file(self.db, class) != Some(file) {
            return None;
        }
        let decl = source_map(self.db, file).find(node)?;
        let data = class_data(self.db, class)?;
        let idx = data.methods.iter().position(|m| m.decl == Some(decl))?;
        Some(MethodId {
            class,
            idx: idx as u32,
        })
    }

    /// 字段的声明节点：`VARIABLE_DECLARATOR`、`ENUM_CONSTANT` 或记录组件的 `PARAM`。
    pub fn field_of_decl(&self, file: SourceFile, node: &SyntaxNode) -> Option<FieldId> {
        let class = match node.kind() {
            SyntaxKind::VARIABLE_DECLARATOR => {
                let field_decl = node
                    .parent()
                    .filter(|p| p.kind() == SyntaxKind::FIELD_DECL)?;
                self.member_owner(file, &field_decl)?
            }
            SyntaxKind::ENUM_CONSTANT => self.member_owner(file, node)?,
            SyntaxKind::PARAM => {
                let record = node.parent()?.parent().and_then(ast::TypeDecl::cast)?;
                self.class_of_decl(file, &record)?
            }
            _ => return None,
        };
        if class_decl_file(self.db, class) != Some(file) {
            return None;
        }
        let decl = source_map(self.db, file).find(node)?;
        let data = class_data(self.db, class)?;
        let idx = data.fields.iter().position(|f| f.decl == Some(decl))?;
        Some(FieldId {
            class,
            idx: idx as u32,
        })
    }

    /// 包含 `node` 的可推断的"体"。
    pub fn body_of(&self, file: SourceFile, node: &SyntaxNode) -> Option<BodyId> {
        for ancestor in node.ancestors() {
            let body = match ancestor.kind() {
                SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL => {
                    self.method_of_decl(file, &ancestor).map(BodyId::Method)
                }
                SyntaxKind::VARIABLE_DECLARATOR | SyntaxKind::ENUM_CONSTANT => {
                    self.field_of_decl(file, &ancestor).map(BodyId::Field)
                }
                SyntaxKind::INITIALIZER_BLOCK => {
                    self.member_owner(file, &ancestor).and_then(|class| {
                        let idx = ancestor
                            .parent()?
                            .children()
                            .filter(|n| n.kind() == SyntaxKind::INITIALIZER_BLOCK)
                            .position(|n| n == ancestor)?;
                        Some(BodyId::Initializer {
                            class,
                            idx: idx as u32,
                        })
                    })
                }
                _ => None,
            };
            if body.is_some() {
                return body;
            }
        }
        None
    }

    /// 在 `node` 处解析类型名时使用的作用域。
    pub fn type_scope(&self, file: SourceFile, node: &SyntaxNode) -> TypeScope {
        let mut scope = TypeScope::file(file);
        // 最内层的具名类，以及 node 是否在它的类体之内
        let mut inside_body = false;
        for ancestor in node.ancestors() {
            if ancestor.kind() == SyntaxKind::CLASS_BODY {
                inside_body = true;
                continue;
            }
            if let Some(decl) = ast::TypeDecl::cast(ancestor.clone()) {
                if let Some(class) = self.class_of_decl(file, &decl) {
                    scope.class = Some(class);
                    scope.in_class_header = !inside_body;
                    break;
                }
                // 局部类：继续向外找具名类，此时一定已经在外层类体之内
                inside_body = true;
            }
        }
        if scope.class.is_none() {
            scope.class = self.implicit_class(file);
        }
        scope.method = node
            .ancestors()
            .filter(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL
                )
            })
            .find_map(|n| self.method_of_decl(file, &n));
        scope
    }

    pub fn resolve_type(&self, file: SourceFile, ty: &ast::Type) -> Ty {
        let scope = self.type_scope(file, ty.syntax());
        lower_type_ref(self.db, &scope, &lower_type(ty))
    }

    pub fn type_of_expr(&self, file: SourceFile, expr: &ast::Expr) -> Option<Ty> {
        let body = self.body_of(file, expr.syntax())?;
        infer(self.db, body)
            .type_of(expr.syntax().text_range())
            .cloned()
    }

    fn resolution_to_def(&self, body: BodyId, resolution: &Resolution) -> Option<Definition> {
        Some(match resolution {
            Resolution::Local(id) => Definition::Local(LocalDef { body, id: *id }),
            Resolution::Field(field) => Definition::Field(*field),
            Resolution::Method(method) => Definition::Method(*method),
            Resolution::Class(class) => Definition::Class(*class),
            Resolution::TypeParam(param) => Definition::TypeParam(*param),
            Resolution::Package(package) => Definition::Package(package.clone()),
            Resolution::ArrayLength => return None,
        })
    }

    /// 方法体推断里对某个范围记录的解析结果。
    fn body_resolution(&self, file: SourceFile, node: &SyntaxNode) -> Option<Definition> {
        let body = self.body_of(file, node)?;
        let result = infer(self.db, body);
        let resolution = result.resolution(node.text_range())?;
        self.resolution_to_def(body, resolution)
    }

    /// 一个名字引用指向什么。
    pub fn classify_name_ref(
        &self,
        file: SourceFile,
        name_ref: &ast::NameRef,
    ) -> Option<Definition> {
        let db = self.db;
        let node = name_ref.syntax();
        let parent = node.parent()?;
        match parent.kind() {
            SyntaxKind::CLASS_TYPE => {
                // `new Foo(...)` 里的类名优先指向被选中的构造器
                if let Some(def) = self.body_resolution(file, node) {
                    return Some(def);
                }
                let class_ty = ast::ClassType::cast(parent)?;
                let segments = class_ty.segments();
                let scope = self.type_scope(file, node);
                match resolve_type_path(db, &scope, &segments) {
                    Some(TypeNs::Class(class)) => Some(Definition::Class(class)),
                    Some(TypeNs::TypeParam(param)) => Some(Definition::TypeParam(param)),
                    None => {
                        let package = segments.join("/");
                        package_exists(db, &package)
                            .then(|| Definition::Package(SmolStr::new(package)))
                    }
                }
            }
            SyntaxKind::QUALIFIED_NAME => {
                let qualified = ast::QualifiedName::cast(parent)?;
                let segments = qualified.segments();
                let owner = qualified.syntax().ancestors().find(|n| {
                    matches!(
                        n.kind(),
                        SyntaxKind::IMPORT_DECL | SyntaxKind::PACKAGE_DECL | SyntaxKind::ANNOTATION
                    )
                })?;
                match owner.kind() {
                    SyntaxKind::PACKAGE_DECL => {
                        Some(Definition::Package(SmolStr::new(segments.join("/"))))
                    }
                    SyntaxKind::ANNOTATION => {
                        let scope = self.type_scope(file, node);
                        match resolve_type_path(db, &scope, &segments)? {
                            TypeNs::Class(class) => Some(Definition::Class(class)),
                            TypeNs::TypeParam(_) => None,
                        }
                    }
                    _ => {
                        let import = ast::ImportDecl::cast(owner)?;
                        self.classify_import_segment(&import, &segments)
                    }
                }
            }
            SyntaxKind::ANNOTATION_ARG => {
                // `@Foo(name = ...)`：注解类型的同名元素方法
                let annotation = parent.ancestors().find_map(ast::Annotation::cast)?;
                let segments = annotation.name()?.segments();
                let scope = self.type_scope(file, node);
                let TypeNs::Class(class) = resolve_type_path(db, &scope, &segments)? else {
                    return None;
                };
                let name = name_ref.text();
                let data = class_data(db, class)?;
                let (idx, _) = data.methods_named(&name).next()?;
                Some(Definition::Method(MethodId { class, idx }))
            }
            _ => self.body_resolution(file, node),
        }
    }

    /// import 里光标所在的那一段（`segments` 是到这一段为止的前缀）。
    fn classify_import_segment(
        &self,
        import: &ast::ImportDecl,
        segments: &[Name],
    ) -> Option<Definition> {
        let db = self.db;
        // 模块名不是包或类
        if import.is_module() {
            return None;
        }
        if let Some(class) = resolve_qualified_class(db, segments) {
            return Some(Definition::Class(class));
        }
        let package = segments.join("/");
        if package_exists(db, &package) {
            return Some(Definition::Package(SmolStr::new(package)));
        }
        // `import static a.B.member;` 的最后一段是静态成员
        let full = import.name()?.segments();
        if import.is_static() && !import.is_on_demand() && full.len() == segments.len() {
            let (member, prefix) = segments.split_last()?;
            let class = resolve_qualified_class(db, prefix)?;
            let ty = Ty::class(class);
            if let Some(field) = lookup_field(db, &ty, member) {
                return Some(Definition::Field(field.id));
            }
            if let Some(method) = lookup_methods(db, &ty, member).into_iter().next() {
                return Some(Definition::Method(method.id));
            }
            if let Some(nested) = member_type(db, class, member) {
                return Some(Definition::Class(nested));
            }
        }
        None
    }

    /// 一个声明处的名字定义了什么。
    pub fn classify_name(&self, file: SourceFile, name: &ast::Name) -> Option<Definition> {
        let node = name.syntax();
        let parent = node.parent()?;
        let local = || -> Option<Definition> {
            let body = self.body_of(file, node)?;
            let id = infer(self.db, body).local_at_decl(node.text_range())?;
            Some(Definition::Local(LocalDef { body, id }))
        };
        match parent.kind() {
            SyntaxKind::CLASS_DECL
            | SyntaxKind::INTERFACE_DECL
            | SyntaxKind::ENUM_DECL
            | SyntaxKind::RECORD_DECL
            | SyntaxKind::ANNOTATION_TYPE_DECL => {
                let decl = ast::TypeDecl::cast(parent)?;
                self.class_of_decl(file, &decl)
                    .or_else(|| self.local_class_of_decl(file, &decl))
                    .map(Definition::Class)
            }
            SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL => self
                .method_of_decl(file, &parent)
                .or_else(|| self.local_class_method(file, &parent))
                .map(Definition::Method)
                .or_else(local),
            SyntaxKind::VARIABLE_DECLARATOR | SyntaxKind::ENUM_CONSTANT | SyntaxKind::PARAM => self
                .field_of_decl(file, &parent)
                .map(Definition::Field)
                .or_else(local),
            SyntaxKind::TYPE_PARAM => {
                let list = parent.parent()?;
                let idx = list
                    .children()
                    .filter(|n| n.kind() == SyntaxKind::TYPE_PARAM)
                    .position(|n| n == parent)? as u32;
                let owner_node = list.parent()?;
                let owner = if let Some(decl) = ast::TypeDecl::cast(owner_node.clone()) {
                    GenericOwner::Class(self.class_of_decl(file, &decl)?)
                } else {
                    GenericOwner::Method(self.method_of_decl(file, &owner_node)?)
                };
                Some(Definition::TypeParam(TypeParamId { owner, idx }))
            }
            _ => local(),
        }
    }

    /// `this(...)` / `super(...)` 调用的构造器。`node` 是 `THIS_EXPR` 或 `SUPER_EXPR`。
    pub fn explicit_constructor_target(
        &self,
        file: SourceFile,
        node: &SyntaxNode,
    ) -> Option<Definition> {
        if node.parent()?.kind() != SyntaxKind::EXPLICIT_CONSTRUCTOR_CALL {
            return None;
        }
        self.body_resolution(file, node)
    }

    /// 给定二进制名（可能来自外部输入）查类。
    pub fn class_by_binary_name(&self, binary_name: &str) -> Option<ClassId> {
        lookup_class(self.db, binary_name)
    }
}
