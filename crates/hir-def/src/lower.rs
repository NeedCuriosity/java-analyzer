//! 源码 → [`ItemTree`]。
//!
//! 同一趟遍历同时产出 [`SourceMap`]（声明下标 → 语法节点指针）。两者分开存放：
//! `ItemTree` 不含任何位置信息，方法体内的编辑不会改变它；`SourceMap` 每次编辑都会变，
//! 但只有真正需要语法节点的功能才依赖它。

use std::sync::Arc;

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, SyntaxNodePtr,
};

use crate::data::{
    ClassData, ClassKind, DeclIdx, FieldData, Import, ItemTree, LocalClass, MethodData, Modifiers,
    ModuleDeclData, Name, NullAnnotation, ParamData, PathSegment, PrimitiveType, TypeParamData,
    TypeRef, WildcardBound,
};

/// 声明下标 → 语法节点。
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SourceMap {
    decls: Vec<SyntaxNodePtr>,
}

impl SourceMap {
    pub fn get(&self, idx: DeclIdx) -> Option<SyntaxNodePtr> {
        self.decls.get(idx as usize).copied()
    }

    pub fn to_node(&self, idx: DeclIdx, root: &SyntaxNode) -> Option<SyntaxNode> {
        self.get(idx)?.try_to_node(root)
    }

    /// 反查：某个语法节点对应的声明下标。
    pub fn find(&self, node: &SyntaxNode) -> Option<DeclIdx> {
        let ptr = SyntaxNodePtr::new(node);
        self.decls
            .iter()
            .position(|p| *p == ptr)
            .map(|i| i as DeclIdx)
    }
}

pub fn lower_file(cu: &ast::CompilationUnit) -> (ItemTree, SourceMap) {
    lower_file_with(
        cu,
        || SmolStr::new("Main"),
        &crate::lombok::LombokConfig::default(),
    )
}

/// 紧凑源文件：顶层直接写了方法或字段。
pub fn is_compact_source(cu: &ast::CompilationUnit) -> bool {
    cu.syntax()
        .children()
        .any(|n| matches!(n.kind(), SyntaxKind::METHOD_DECL | SyntaxKind::FIELD_DECL))
}

/// `implicit_name` 是紧凑源文件的隐式类的名字（文件名），只在需要时才取。
/// `lombok` 是工作区的 `lombok.config` 配置。
pub fn lower_file_with(
    cu: &ast::CompilationUnit,
    implicit_name: impl FnOnce() -> SmolStr,
    lombok: &crate::lombok::LombokConfig,
) -> (ItemTree, SourceMap) {
    let compact = is_compact_source(cu);
    // 紧凑源文件的隐式类总在无名包里（写了 `package` 是错误，另行报告）
    let package: SmolStr = if compact {
        SmolStr::default()
    } else {
        cu.package()
            .and_then(|p| p.name())
            .map(|n| SmolStr::new(n.segments().join("/")))
            .unwrap_or_default()
    };

    let mut module_imports = Vec::new();
    let imports = cu
        .imports()
        .filter_map(|import| {
            if import.is_module() {
                if let Some(name) = import.name() {
                    module_imports.push(SmolStr::new(name.text()));
                }
                return None;
            }
            let segments = import.name()?.segments();
            if segments.is_empty() {
                return None;
            }
            Some(Import {
                segments,
                is_static: import.is_static(),
                is_on_demand: import.is_on_demand(),
            })
        })
        .collect();
    let module = cu.module().and_then(|decl| lower_module_decl(&decl));

    let mut ctx = Ctx {
        lombok_config: lombok,
        package: package.clone(),
        classes: Vec::new(),
        decls: Vec::new(),
        local_classes: Vec::new(),
        local_counts: FxHashMap::default(),
        anonymous_counts: FxHashMap::default(),
    };
    if compact {
        // 隐式类隐式导入 `java.base` 模块
        ctx.lower_implicit_class(cu, implicit_name());
        if !module_imports.iter().any(|m| m == "java.base") {
            module_imports.push(SmolStr::new("java.base"));
        }
    } else {
        for decl in cu.type_decls() {
            ctx.lower_type_decl(&decl, None);
        }
    }

    let classes = ctx.classes.into_iter().flatten().map(Arc::new).collect();
    (
        ItemTree {
            package,
            imports,
            classes,
            local_classes: ctx.local_classes,
            module_imports,
            module,
        },
        SourceMap { decls: ctx.decls },
    )
}

struct Outer<'a> {
    binary_name: &'a str,
    is_interface: bool,
    /// 局部类：`(外层类里同名局部类的序号, 所在的方法)`，二进制名是 `Outer$1Name`
    local: Option<(u32, Option<u32>)>,
}

struct Ctx<'c> {
    lombok_config: &'c crate::lombok::LombokConfig,
    package: SmolStr,
    /// 先占位再回填，保证先序下标
    classes: Vec<Option<ClassData>>,
    decls: Vec<SyntaxNodePtr>,
    local_classes: Vec<LocalClass>,
    /// (外层类, 名字) → 已经有几个同名局部类
    local_counts: FxHashMap<(SmolStr, Name), u32>,
    /// 外层类 → 已经有几个匿名类
    anonymous_counts: FxHashMap<SmolStr, u32>,
}

impl Ctx<'_> {
    fn alloc_decl(&mut self, node: &SyntaxNode) -> DeclIdx {
        self.decls.push(SyntaxNodePtr::new(node));
        (self.decls.len() - 1) as DeclIdx
    }

    fn lower_type_decl(&mut self, decl: &ast::TypeDecl, outer: Option<&Outer<'_>>) -> Option<Name> {
        let name = decl.name()?.text();
        let binary_name: SmolStr = match outer {
            Some(Outer {
                binary_name,
                local: Some((n, _)),
                ..
            }) => format!("{binary_name}${n}{name}").into(),
            Some(outer) => format!("{}${}", outer.binary_name, name).into(),
            None if self.package.is_empty() => name.clone(),
            None => format!("{}/{}", self.package, name).into(),
        };
        let kind = match decl {
            ast::TypeDecl::ClassDecl(_) => ClassKind::Class,
            ast::TypeDecl::InterfaceDecl(_) => ClassKind::Interface,
            ast::TypeDecl::EnumDecl(_) => ClassKind::Enum,
            ast::TypeDecl::RecordDecl(_) => ClassKind::Record,
            ast::TypeDecl::AnnotationTypeDecl(_) => ClassKind::Annotation,
        };
        let slot = self.classes.len();
        self.classes.push(None);
        let decl_idx = self.alloc_decl(decl.syntax());

        let (mut flags, annotations) = lower_modifiers(decl.modifier_list());
        if let Some(Outer {
            local: Some((_, owner)),
            binary_name: outer_binary,
            ..
        }) = outer
        {
            flags.insert(Modifiers::LOCAL_CLASS);
            self.local_classes.push(LocalClass {
                name: name.clone(),
                binary_name: binary_name.clone(),
                outer: SmolStr::new(*outer_binary),
                owner: *owner,
                decl: decl_idx,
            });
        }
        if let Some(outer) = outer {
            // 接口的成员类型隐式 public static；枚举、记录、接口作为成员时隐式 static
            if outer.is_interface {
                flags.insert(Modifiers::PUBLIC | Modifiers::STATIC);
            }
            if kind != ClassKind::Class {
                flags.insert(Modifiers::STATIC);
            }
        }
        match kind {
            ClassKind::Interface | ClassKind::Annotation => flags.insert(Modifiers::ABSTRACT),
            ClassKind::Record => flags.insert(Modifiers::FINAL),
            _ => {}
        }

        let self_ty = TypeRef::class(&binary_name);
        let mut data = ClassData {
            name: name.clone(),
            binary_name: binary_name.clone(),
            kind,
            flags,
            type_params: lower_type_params(decl.type_param_list()),
            superclass: None,
            interfaces: Vec::new(),
            permits: decl
                .permits_clause()
                .map(|c| c.types().map(|t| lower_type(&t)).collect())
                .unwrap_or_default(),
            fields: Vec::new(),
            methods: Vec::new(),
            nested: Vec::new(),
            outer: outer.map(|o| SmolStr::new(o.binary_name)),
            annotations,
            decl: Some(decl_idx),
            is_source: true,
        };

        let extends: Vec<TypeRef> = decl
            .extends_clause()
            .map(|c| c.types().map(|t| lower_type(&t)).collect())
            .unwrap_or_default();
        let implements: Vec<TypeRef> = decl
            .implements_clause()
            .map(|c| c.types().map(|t| lower_type(&t)).collect())
            .unwrap_or_default();
        match kind {
            ClassKind::Class => {
                data.superclass = Some(extends.into_iter().next().unwrap_or_else(|| {
                    if binary_name == "java/lang/Object" {
                        TypeRef::Error
                    } else {
                        TypeRef::class("java/lang/Object")
                    }
                }));
                if matches!(data.superclass, Some(TypeRef::Error)) {
                    data.superclass = None;
                }
                data.interfaces = implements;
            }
            ClassKind::Interface => data.interfaces = extends,
            ClassKind::Annotation => {
                data.interfaces = vec![TypeRef::class("java/lang/annotation/Annotation")];
            }
            ClassKind::Enum => {
                data.superclass = Some(TypeRef::Class {
                    name: SmolStr::new("java/lang/Enum"),
                    args: vec![self_ty.clone()],
                });
                data.interfaces = implements;
            }
            ClassKind::Record => {
                data.superclass = Some(TypeRef::class("java/lang/Record"));
                data.interfaces = implements;
            }
        }

        // 记录组件：私有 final 字段 + 同名访问器 + 规范构造器
        let mut record_components: Vec<(Name, TypeRef, DeclIdx, bool)> = Vec::new();
        if let Some(components) = decl.record_components() {
            for param in components.params() {
                let Some(pname) = param.name().map(|n| n.text()) else {
                    continue;
                };
                let ty = lower_param_type(&param);
                let idx = self.alloc_decl(param.syntax());
                record_components.push((pname, ty, idx, param.is_varargs()));
            }
        }
        for (pname, ty, idx, _) in &record_components {
            data.fields.push(FieldData {
                name: pname.clone(),
                ty: ty.clone(),
                flags: Modifiers(Modifiers::PRIVATE | Modifiers::FINAL),
                decl: Some(*idx),
                constant: None,
            });
        }

        // 字段上的注解、带 `@Builder` 的方法（Lombok 需要）
        let mut lombok = crate::lombok::LombokInput::default();
        if let Some(body) = decl.body() {
            for constant in body.enum_constants() {
                let Some(cname) = constant.name().map(|n| n.text()) else {
                    continue;
                };
                let idx = self.alloc_decl(constant.syntax());
                let mut flags = Modifiers(
                    Modifiers::PUBLIC
                        | Modifiers::STATIC
                        | Modifiers::FINAL
                        | Modifiers::ENUM_CONSTANT,
                );
                if constant.annotations().any(|a| is_deprecated_annotation(&a)) {
                    flags.insert(Modifiers::DEPRECATED);
                }
                self.lower_local_types(constant.syntax(), &binary_name, None);
                data.fields.push(FieldData {
                    name: cname,
                    ty: self_ty.clone(),
                    flags,
                    decl: Some(idx),
                    constant: None,
                });
            }
            self.lower_members(
                body.members(),
                kind,
                &binary_name,
                &record_components,
                &mut data,
                &mut lombok,
            );
        }

        // Lombok 生成的成员要先于"隐式无参构造器"的判断
        for generated in crate::lombok::apply(&mut data, &lombok, &self_ty, self.lombok_config) {
            // 用户自己写了一部分的同名成员类型（定制的构建器）：只补缺的成员
            let existing = self.classes[slot + 1..]
                .iter_mut()
                .flatten()
                .find(|c| c.binary_name == generated.binary_name);
            match existing {
                Some(existing) => crate::lombok::merge_into(existing, generated),
                None => {
                    data.nested.push(generated.name.clone());
                    self.classes.push(Some(generated));
                }
            }
        }
        add_synthetic_members(&mut data, &record_components, &self_ty);
        self.classes[slot] = Some(data);
        Some(name)
    }

    /// 类体（或紧凑源文件顶层）的成员。
    fn lower_members(
        &mut self,
        members: impl Iterator<Item = ast::Member>,
        kind: ClassKind,
        binary_name: &SmolStr,
        record_components: &[(Name, TypeRef, DeclIdx, bool)],
        data: &mut ClassData,
        lombok: &mut crate::lombok::LombokInput,
    ) {
        let is_interface = matches!(kind, ClassKind::Interface | ClassKind::Annotation);
        let name = data.name.clone();
        let this_outer = Outer {
            binary_name,
            is_interface,
            local: None,
        };
        for member in members {
            match member {
                ast::Member::FieldDecl(field) => {
                    self.lower_field(&field, is_interface, data, lombok);
                    self.lower_local_types(field.syntax(), binary_name, None);
                }
                ast::Member::MethodDecl(method) => {
                    let idx = self.alloc_decl(method.syntax());
                    data.methods.push(lower_method(&method, kind, idx));
                    lombok.builder_methods.extend(builder_method(
                        data.methods.len() - 1,
                        method.modifier_list(),
                        method.param_list(),
                    ));
                    let owner = Some(data.methods.len() as u32 - 1);
                    self.lower_local_types(method.syntax(), binary_name, owner);
                }
                ast::Member::ConstructorDecl(ctor) => {
                    let idx = self.alloc_decl(ctor.syntax());
                    data.methods
                        .push(lower_constructor(&ctor, &name, record_components, idx));
                    lombok.builder_methods.extend(builder_method(
                        data.methods.len() - 1,
                        ctor.modifier_list(),
                        ctor.param_list(),
                    ));
                    let owner = Some(data.methods.len() as u32 - 1);
                    self.lower_local_types(ctor.syntax(), binary_name, owner);
                }
                ast::Member::InitializerBlock(block) => {
                    self.lower_local_types(block.syntax(), binary_name, None);
                }
                ast::Member::ClassDecl(it) => self.lower_nested(it.into(), &this_outer, data),
                ast::Member::InterfaceDecl(it) => self.lower_nested(it.into(), &this_outer, data),
                ast::Member::EnumDecl(it) => self.lower_nested(it.into(), &this_outer, data),
                ast::Member::RecordDecl(it) => self.lower_nested(it.into(), &this_outer, data),
                ast::Member::AnnotationTypeDecl(it) => {
                    self.lower_nested(it.into(), &this_outer, data)
                }
            }
        }
    }

    /// 紧凑源文件（顶层有方法或字段）的隐式类：`final`、在无名包里、以文件名命名，
    /// 顶层的字段、方法、类型声明都是它的成员。
    fn lower_implicit_class(&mut self, cu: &ast::CompilationUnit, name: SmolStr) {
        let binary_name = name.clone();
        let slot = self.classes.len();
        self.classes.push(None);
        let decl_idx = self.alloc_decl(cu.syntax());
        let self_ty = TypeRef::class(&binary_name);
        let mut data = ClassData {
            name: name.clone(),
            binary_name: binary_name.clone(),
            kind: ClassKind::Class,
            flags: Modifiers(Modifiers::FINAL | Modifiers::IMPLICIT_CLASS),
            type_params: Vec::new(),
            superclass: Some(TypeRef::class("java/lang/Object")),
            interfaces: Vec::new(),
            permits: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            nested: Vec::new(),
            outer: None,
            annotations: Vec::new(),
            decl: Some(decl_idx),
            is_source: true,
        };
        let mut lombok = crate::lombok::LombokInput::default();
        let members = cu.syntax().children().filter_map(ast::Member::cast);
        self.lower_members(
            members,
            ClassKind::Class,
            &binary_name,
            &[],
            &mut data,
            &mut lombok,
        );
        add_synthetic_members(&mut data, &[], &self_ty);
        self.classes[slot] = Some(data);
    }

    /// 成员的体里声明的局部类型和匿名类（不进入嵌套的类型声明，它们自己的体由它们自己处理）。
    /// 局部类不登记为外层类的成员类型。
    fn lower_local_types(&mut self, member: &SyntaxNode, outer: &str, owner: Option<u32>) {
        let mut walk = member.preorder();
        while let Some(event) = walk.next() {
            let syntax::WalkEvent::Enter(node) = event else {
                continue;
            };
            if node == *member {
                continue;
            }
            if node.kind() == SyntaxKind::CLASS_BODY {
                if let Some(superclass) = anonymous_superclass(&node, outer) {
                    walk.skip_subtree();
                    self.lower_anonymous(&node, superclass, outer, owner);
                }
                continue;
            }
            let Some(decl) = ast::TypeDecl::cast(node.clone()) else {
                continue;
            };
            walk.skip_subtree();
            // 没有登记成类的匿名类（菱形等）的成员类型不是局部类，这里不管
            if node
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::CLASS_BODY)
            {
                continue;
            }
            let Some(name) = decl.name().map(|n| n.text()) else {
                continue;
            };
            let count = self
                .local_counts
                .entry((SmolStr::new(outer), name))
                .or_insert(0);
            *count += 1;
            let n = *count;
            let local_outer = Outer {
                binary_name: outer,
                is_interface: false,
                local: Some((n, owner)),
            };
            self.lower_type_decl(&decl, Some(&local_outer));
        }
    }

    /// 匿名类：二进制名是 `Outer$1`，父类型是 `new` 后面的类型（是接口时也放在 `superclass` 里，
    /// 签名层不知道它是不是接口）。没有构造器，也没有合成成员。
    fn lower_anonymous(
        &mut self,
        body: &SyntaxNode,
        superclass: TypeRef,
        outer: &str,
        owner: Option<u32>,
    ) {
        let Some(class_body) = ast::ClassBody::cast(body.clone()) else {
            return;
        };
        let count = self
            .anonymous_counts
            .entry(SmolStr::new(outer))
            .or_insert(0);
        *count += 1;
        let binary_name: SmolStr = format!("{outer}${count}").into();
        let slot = self.classes.len();
        self.classes.push(None);
        let decl_idx = self.alloc_decl(body);
        self.local_classes.push(LocalClass {
            name: Name::default(),
            binary_name: binary_name.clone(),
            outer: SmolStr::new(outer),
            owner,
            decl: decl_idx,
        });
        let mut data = ClassData {
            name: Name::default(),
            binary_name: binary_name.clone(),
            kind: ClassKind::Class,
            flags: Modifiers(Modifiers::LOCAL_CLASS | Modifiers::ANONYMOUS),
            type_params: Vec::new(),
            superclass: Some(superclass),
            interfaces: Vec::new(),
            permits: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            nested: Vec::new(),
            outer: Some(SmolStr::new(outer)),
            annotations: Vec::new(),
            decl: Some(decl_idx),
            is_source: true,
        };
        let mut lombok = crate::lombok::LombokInput::default();
        self.lower_members(
            class_body.members(),
            ClassKind::Class,
            &binary_name,
            &[],
            &mut data,
            &mut lombok,
        );
        self.classes[slot] = Some(data);
    }

    fn lower_nested(&mut self, decl: ast::TypeDecl, outer: &Outer<'_>, data: &mut ClassData) {
        if let Some(name) = self.lower_type_decl(&decl, Some(outer)) {
            data.nested.push(name);
        }
    }

    fn lower_field(
        &mut self,
        field: &ast::FieldDecl,
        in_interface: bool,
        data: &mut ClassData,
        lombok: &mut crate::lombok::LombokInput,
    ) {
        let (mut flags, annotations) = lower_modifiers(field.modifier_list());
        if in_interface {
            flags.insert(Modifiers::PUBLIC | Modifiers::STATIC | Modifiers::FINAL);
        }
        let field_ty = field.ty();
        let base = field_ty.as_ref().map(lower_type).unwrap_or(TypeRef::Error);
        let modifier_flags = flags;
        for declarator in field.declarators() {
            let Some(name) = declarator.name().map(|n| n.text()) else {
                continue;
            };
            let mut flags = modifier_flags;
            set_nullness(
                &mut flags,
                declared_nullness(modifier_flags, field_ty.as_ref(), declarator.extra_dims()),
            );
            let idx = self.alloc_decl(declarator.syntax());
            let initialized = declarator.initializer().is_some();
            if !annotations.is_empty() || initialized {
                lombok.fields.push(crate::lombok::FieldExtra {
                    idx: data.fields.len(),
                    annotations: annotations.clone(),
                    initialized,
                });
            }
            data.fields.push(FieldData {
                name,
                ty: base.clone().array_of(declarator.extra_dims()),
                flags,
                decl: Some(idx),
                constant: None,
            });
        }
    }
}

/// 匿名类的类体（`new T(..) { ... }`、枚举常量的类体）的父类型。枚举常量的类体在
/// `enum_binary_name` 的类里。限定的 `outer.new Inner() { ... }` 和菱形 `new T<>() { ... }`
/// 的父类型要靠推断才知道，不登记成类（它们的成员在体里按局部符号处理）。
fn anonymous_superclass(body: &SyntaxNode, enum_binary_name: &str) -> Option<TypeRef> {
    let parent = body.parent()?;
    match parent.kind() {
        SyntaxKind::ENUM_CONSTANT => Some(TypeRef::class(enum_binary_name)),
        SyntaxKind::NEW_EXPR => {
            let new = ast::NewExpr::cast(parent)?;
            if new.qualifier().is_some() {
                return None;
            }
            let ast::Type::ClassType(ty) = new.ty()? else {
                return None;
            };
            if ty
                .syntax()
                .descendants()
                .filter_map(ast::TypeArgList::cast)
                .any(|list| list.args().next().is_none())
            {
                return None;
            }
            Some(lower_type(&ast::Type::ClassType(ty)))
        }
        _ => None,
    }
}

fn add_synthetic_members(
    data: &mut ClassData,
    record_components: &[(Name, TypeRef, DeclIdx, bool)],
    self_ty: &TypeRef,
) {
    let synthetic_public = Modifiers(Modifiers::PUBLIC | Modifiers::SYNTHETIC);
    match data.kind {
        ClassKind::Record => {
            for (name, ty, idx, _) in record_components {
                let declared = data
                    .methods
                    .iter()
                    .any(|m| !m.is_constructor && m.name == *name && m.params.is_empty());
                if !declared {
                    data.methods.push(MethodData {
                        name: name.clone(),
                        is_constructor: false,
                        type_params: Vec::new(),
                        params: Vec::new(),
                        ret: ty.clone(),
                        throws: Vec::new(),
                        flags: synthetic_public,
                        has_body: true,
                        decl: Some(*idx),
                    });
                }
            }
            let has_canonical = data.methods.iter().any(|m| {
                m.is_constructor
                    && m.params.len() == record_components.len()
                    && m.params
                        .iter()
                        .zip(record_components)
                        .all(|(p, (_, ty, _, _))| p.ty == *ty)
            });
            if !has_canonical {
                let mut flags = synthetic_public;
                if record_components.last().is_some_and(|c| c.3) {
                    flags.insert(Modifiers::VARARGS);
                }
                data.methods.push(MethodData {
                    name: data.name.clone(),
                    is_constructor: true,
                    type_params: Vec::new(),
                    params: record_components
                        .iter()
                        .map(|(name, ty, _, _)| ParamData {
                            name: Some(name.clone()),
                            ty: ty.clone(),
                            nullness: Default::default(),
                        })
                        .collect(),
                    ret: TypeRef::Void,
                    throws: Vec::new(),
                    flags,
                    has_body: true,
                    decl: data.decl,
                });
            }
        }
        ClassKind::Enum => {
            let static_public =
                Modifiers(Modifiers::PUBLIC | Modifiers::STATIC | Modifiers::SYNTHETIC);
            data.methods.push(MethodData {
                name: Name::new("values"),
                is_constructor: false,
                type_params: Vec::new(),
                params: Vec::new(),
                ret: TypeRef::Array(Box::new(self_ty.clone())),
                throws: Vec::new(),
                flags: static_public,
                has_body: true,
                decl: None,
            });
            data.methods.push(MethodData {
                name: Name::new("valueOf"),
                is_constructor: false,
                type_params: Vec::new(),
                params: vec![ParamData {
                    name: Some(Name::new("name")),
                    ty: TypeRef::class("java/lang/String"),
                    nullness: Default::default(),
                }],
                ret: self_ty.clone(),
                throws: Vec::new(),
                flags: static_public,
                has_body: true,
                decl: None,
            });
        }
        _ => {}
    }

    // 没有声明任何构造器的类有一个隐式的无参构造器
    if matches!(data.kind, ClassKind::Class | ClassKind::Enum)
        && !data.methods.iter().any(|m| m.is_constructor)
    {
        let visibility = if data.kind == ClassKind::Enum {
            Modifiers::PRIVATE
        } else {
            data.flags.0 & (Modifiers::PUBLIC | Modifiers::PROTECTED | Modifiers::PRIVATE)
        };
        data.methods.push(MethodData {
            name: data.name.clone(),
            is_constructor: true,
            type_params: Vec::new(),
            params: Vec::new(),
            ret: TypeRef::Void,
            throws: Vec::new(),
            flags: Modifiers(visibility | Modifiers::SYNTHETIC),
            has_body: true,
            decl: data.decl,
        });
    }
}

fn lower_module_decl(decl: &ast::ModuleDecl) -> Option<ModuleDeclData> {
    let mut data = ModuleDeclData {
        name: SmolStr::new(decl.name()?.text()),
        ..ModuleDeclData::default()
    };
    let Some(body) = decl.body() else {
        return Some(data);
    };
    for requires in body.requires() {
        if let Some(name) = requires.name() {
            data.requires
                .push((SmolStr::new(name.text()), requires.is_transitive()));
        }
    }
    let package = |name: ast::QualifiedName| SmolStr::new(name.segments().join("/"));
    let module = |name: ast::QualifiedName| SmolStr::new(name.text());
    for exports in body.exports() {
        if let Some(name) = exports.name() {
            data.exports
                .push((package(name), exports.targets().map(module).collect()));
        }
    }
    for opens in body.opens() {
        if let Some(name) = opens.name() {
            data.opens
                .push((package(name), opens.targets().map(module).collect()));
        }
    }
    Some(data)
}

fn is_deprecated_annotation(annotation: &ast::Annotation) -> bool {
    annotation
        .name()
        .and_then(|n| n.name_ref())
        .is_some_and(|n| n.text() == "Deprecated")
}

fn lower_modifiers(list: Option<ast::ModifierList>) -> (Modifiers, Vec<SmolStr>) {
    let mut flags = Modifiers::default();
    let mut annotations = Vec::new();
    let Some(list) = list else {
        return (flags, annotations);
    };
    for token in list.keywords() {
        let flag = match token.kind() {
            SyntaxKind::PUBLIC_KW => Modifiers::PUBLIC,
            SyntaxKind::PROTECTED_KW => Modifiers::PROTECTED,
            SyntaxKind::PRIVATE_KW => Modifiers::PRIVATE,
            SyntaxKind::STATIC_KW => Modifiers::STATIC,
            SyntaxKind::FINAL_KW => Modifiers::FINAL,
            SyntaxKind::ABSTRACT_KW => Modifiers::ABSTRACT,
            SyntaxKind::NATIVE_KW => Modifiers::NATIVE,
            SyntaxKind::SYNCHRONIZED_KW => Modifiers::SYNCHRONIZED,
            SyntaxKind::TRANSIENT_KW => Modifiers::TRANSIENT,
            SyntaxKind::VOLATILE_KW => Modifiers::VOLATILE,
            SyntaxKind::STRICTFP_KW => Modifiers::STRICTFP,
            SyntaxKind::DEFAULT_KW => Modifiers::DEFAULT,
            SyntaxKind::SEALED_KW => Modifiers::SEALED,
            SyntaxKind::NON_SEALED_KW => Modifiers::NON_SEALED,
            _ => continue,
        };
        flags.insert(flag);
    }
    for annotation in list.annotations() {
        if is_deprecated_annotation(&annotation) {
            flags.insert(Modifiers::DEPRECATED);
        }
        if let Some(name) = annotation.name() {
            let name = name.text();
            flags.insert_null_annotation(NullAnnotation::from_name(&name));
            let options = crate::lombok::annotation_options(&annotation, &name);
            annotations.push(SmolStr::new(name));
            annotations.extend(options);
        }
    }
    (flags, annotations)
}

/// 带 `@Builder` 的构造器 / 方法：记下方法和各形参上的注解，交给 Lombok 生成构建器。
fn builder_method(
    idx: usize,
    modifiers: Option<ast::ModifierList>,
    params: Option<ast::ParamList>,
) -> Option<crate::lombok::BuilderMethod> {
    let modifiers = modifiers?;
    let is_builder = modifiers.annotations().any(|a| {
        a.name()
            .is_some_and(|n| crate::lombok::is_builder_annotation(&n.text()))
    });
    if !is_builder {
        return None;
    }
    let params = params
        .map(|list| {
            list.params()
                .map(|p| lower_modifiers(p.modifier_list()).1)
                .collect()
        })
        .unwrap_or_default();
    Some(crate::lombok::BuilderMethod {
        idx,
        annotations: lower_modifiers(Some(modifiers)).1,
        params,
    })
}

/// 节点上直接写的注解里的空值注解。
fn direct_null_annotation(node: &syntax::SyntaxNode) -> NullAnnotation {
    let names: Vec<String> = node
        .children()
        .filter_map(ast::Annotation::cast)
        .filter_map(|a| a.name())
        .map(|n| n.text().to_string())
        .collect();
    NullAnnotation::of(names.iter().map(|s| s.as_str()))
}

/// 声明（字段、方法返回值、形参）的空值注解。修饰符位置上的注解对数组类型修饰的是元素
/// （类型注解的语义），数组本身的注解写在第一个 `[]` 前面：`String @Nullable []`。
fn declared_nullness(
    flags: Modifiers,
    ty: Option<&ast::Type>,
    extra_dims: usize,
) -> NullAnnotation {
    match ty {
        Some(ast::Type::ArrayType(array)) => {
            // 最外层数组的注解在最里面那个 ARRAY_TYPE 节点上
            let mut inner = array.clone();
            while let Some(ast::Type::ArrayType(element)) = inner.element() {
                inner = element;
            }
            direct_null_annotation(inner.syntax())
        }
        _ if extra_dims > 0 => NullAnnotation::Unknown,
        _ => flags.null_annotation(),
    }
}

fn set_nullness(flags: &mut Modifiers, nullness: NullAnnotation) {
    flags.remove(Modifiers::NULLABLE | Modifiers::NOT_NULL);
    flags.insert_null_annotation(nullness);
}

/// 形参的空值注解；可变参数 `T @Nullable ... xs` 的注解写在 `...` 前面。
pub fn param_null_annotation(param: &ast::Param) -> NullAnnotation {
    if param.is_varargs() {
        return direct_null_annotation(param.syntax());
    }
    let (flags, _) = lower_modifiers(param.modifier_list());
    declared_nullness(flags, param.ty().as_ref(), param.extra_dims())
}

fn lower_type_params(list: Option<ast::TypeParamList>) -> Vec<TypeParamData> {
    let Some(list) = list else { return Vec::new() };
    list.type_params()
        .filter_map(|p| {
            Some(TypeParamData {
                name: p.name()?.text(),
                bounds: p.bounds().map(|t| lower_type(&t)).collect(),
            })
        })
        .collect()
}

/// 参数的类型：算上可变参数和写在名字后面的数组维度。
pub fn lower_param_type(param: &ast::Param) -> TypeRef {
    let base = param.ty().map(|t| lower_type(&t)).unwrap_or(TypeRef::Error);
    let dims = param.extra_dims() + usize::from(param.is_varargs());
    base.array_of(dims)
}

fn lower_params(list: Option<ast::ParamList>) -> (Vec<ParamData>, bool) {
    let Some(list) = list else {
        return (Vec::new(), false);
    };
    let mut varargs = false;
    let params = list
        .params()
        // 接收者参数 `Foo this` 没有名字节点，不算形参
        .filter(|p| p.name().is_some())
        .map(|p| {
            varargs = p.is_varargs();
            ParamData {
                name: p.name().map(|n| n.text()),
                ty: lower_param_type(&p),
                nullness: param_null_annotation(&p),
            }
        })
        .collect();
    (params, varargs)
}

fn lower_method(method: &ast::MethodDecl, class_kind: ClassKind, decl: DeclIdx) -> MethodData {
    let (mut flags, _) = lower_modifiers(method.modifier_list());
    let has_body = method.body().is_some();
    if matches!(class_kind, ClassKind::Interface | ClassKind::Annotation) {
        if !flags.contains(Modifiers::PRIVATE) {
            flags.insert(Modifiers::PUBLIC);
        }
        if !has_body && !flags.contains(Modifiers::STATIC) {
            flags.insert(Modifiers::ABSTRACT);
        }
    }
    let (params, varargs) = lower_params(method.param_list());
    if varargs {
        flags.insert(Modifiers::VARARGS);
    }
    let return_type = method.return_type();
    let nullness = declared_nullness(flags, return_type.as_ref(), method.extra_dims());
    set_nullness(&mut flags, nullness);
    // 返回类型是第一个直接的 Type 子节点；`int f()[]` 这种旧写法的维度要补上
    let ret = return_type
        .as_ref()
        .map(lower_type)
        .unwrap_or(TypeRef::Error)
        .array_of(method.extra_dims());
    MethodData {
        name: method.name().map(|n| n.text()).unwrap_or_default(),
        is_constructor: false,
        type_params: lower_type_params(method.type_param_list()),
        params,
        ret,
        throws: method
            .throws_clause()
            .map(|c| c.types().map(|t| lower_type(&t)).collect())
            .unwrap_or_default(),
        flags,
        has_body,
        decl: Some(decl),
    }
}

fn lower_constructor(
    ctor: &ast::ConstructorDecl,
    class_name: &Name,
    record_components: &[(Name, TypeRef, DeclIdx, bool)],
    decl: DeclIdx,
) -> MethodData {
    let (mut flags, _) = lower_modifiers(ctor.modifier_list());
    let params = match ctor.param_list() {
        Some(list) => {
            let (params, varargs) = lower_params(Some(list));
            if varargs {
                flags.insert(Modifiers::VARARGS);
            }
            params
        }
        // 记录的紧凑构造器：形参就是全部组件
        None => record_components
            .iter()
            .map(|(name, ty, _, _)| ParamData {
                name: Some(name.clone()),
                ty: ty.clone(),
                nullness: Default::default(),
            })
            .collect(),
    };
    MethodData {
        name: class_name.clone(),
        is_constructor: true,
        type_params: lower_type_params(ctor.type_param_list()),
        params,
        ret: TypeRef::Void,
        throws: ctor
            .throws_clause()
            .map(|c| c.types().map(|t| lower_type(&t)).collect())
            .unwrap_or_default(),
        flags,
        has_body: true,
        decl: Some(decl),
    }
}

pub fn lower_type(ty: &ast::Type) -> TypeRef {
    match ty {
        ast::Type::PrimitiveType(it) => match it.keyword().map(|t| t.kind()) {
            Some(SyntaxKind::BOOLEAN_KW) => TypeRef::Primitive(PrimitiveType::Boolean),
            Some(SyntaxKind::BYTE_KW) => TypeRef::Primitive(PrimitiveType::Byte),
            Some(SyntaxKind::SHORT_KW) => TypeRef::Primitive(PrimitiveType::Short),
            Some(SyntaxKind::CHAR_KW) => TypeRef::Primitive(PrimitiveType::Char),
            Some(SyntaxKind::INT_KW) => TypeRef::Primitive(PrimitiveType::Int),
            Some(SyntaxKind::LONG_KW) => TypeRef::Primitive(PrimitiveType::Long),
            Some(SyntaxKind::FLOAT_KW) => TypeRef::Primitive(PrimitiveType::Float),
            Some(SyntaxKind::DOUBLE_KW) => TypeRef::Primitive(PrimitiveType::Double),
            Some(SyntaxKind::VOID_KW) => TypeRef::Void,
            _ => TypeRef::Error,
        },
        ast::Type::ClassType(it) => {
            let mut segments = Vec::new();
            collect_segments(it, &mut segments);
            if segments.is_empty() {
                TypeRef::Error
            } else {
                TypeRef::Path(segments)
            }
        }
        ast::Type::ArrayType(it) => {
            let element = it
                .element()
                .map(|t| lower_type(&t))
                .unwrap_or(TypeRef::Error);
            TypeRef::Array(Box::new(element))
        }
        ast::Type::WildcardType(it) => {
            let bound = it.bound().map(|t| Box::new(lower_type(&t)));
            TypeRef::Wildcard(match bound {
                Some(bound) if it.is_super() => WildcardBound::Super(bound),
                Some(bound) => WildcardBound::Extends(bound),
                None => WildcardBound::Unbounded,
            })
        }
        ast::Type::IntersectionType(it) => {
            TypeRef::Intersection(it.types().map(|t| lower_type(&t)).collect())
        }
        // 多重 catch 的联合类型：取第一个备选作为近似
        ast::Type::UnionType(it) => it
            .types()
            .next()
            .map(|t| lower_type(&t))
            .unwrap_or(TypeRef::Error),
    }
}

fn collect_segments(ty: &ast::ClassType, out: &mut Vec<PathSegment>) {
    if let Some(qualifier) = ty.qualifier() {
        collect_segments(&qualifier, out);
    }
    if let Some(name) = ty.name_ref() {
        let args = ty
            .type_arg_list()
            .map(|list| list.args().map(|t| lower_type(&t)).collect())
            .unwrap_or_default();
        out.push(PathSegment {
            name: name.text(),
            args,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower(text: &str) -> ItemTree {
        let parse = ast::CompilationUnit::parse(text);
        lower_file(&parse.tree()).0
    }

    #[test]
    fn binary_names_and_nesting() {
        let tree =
            lower("package a.b; class Outer { interface In { class Deep {} } enum E { X } }");
        let names: Vec<_> = tree
            .classes
            .iter()
            .map(|c| c.binary_name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "a/b/Outer",
                "a/b/Outer$In",
                "a/b/Outer$In$Deep",
                "a/b/Outer$E"
            ]
        );
        assert_eq!(tree.classes[0].nested, ["In", "E"]);
        // 接口里的成员类隐式 public static
        assert!(tree.classes[2]
            .flags
            .contains(Modifiers::PUBLIC | Modifiers::STATIC));
        assert_eq!(tree.package, "a/b");
    }

    #[test]
    fn anonymous_and_local_classes() {
        let tree = lower(
            "class A {
                Object f = new Object() { int n; };
                void m() {
                    Runnable r = new Runnable() {
                        public void run() { class L {} new Object() {}; }
                    };
                    java.util.Comparator<String> c = new java.util.Comparator<>() {};
                }
                enum E { X { void g() {} }, Y }
            }",
        );
        let names: Vec<_> = tree
            .classes
            .iter()
            .map(|c| c.binary_name.as_str())
            .collect();
        // 菱形的匿名类不登记
        assert_eq!(
            names,
            ["A", "A$1", "A$2", "A$2$1L", "A$2$1", "A$E", "A$E$1"]
        );
        let anon = &tree.classes[1];
        assert!(anon
            .flags
            .contains(Modifiers::ANONYMOUS | Modifiers::LOCAL_CLASS));
        assert_eq!(anon.fields[0].name, "n");
        assert_eq!(anon.outer.as_deref(), Some("A"));
        // 匿名类没有构造器
        assert_eq!(tree.classes[2].constructors().count(), 0);
        assert!(
            matches!(&tree.classes[2].superclass, Some(TypeRef::Path(p)) if p[0].name == "Runnable")
        );
        // 枚举常量的类体继承枚举本身
        assert_eq!(tree.classes[6].superclass, Some(TypeRef::class("A$E")));
        // 匿名类里方法体中的局部类属于匿名类的那个方法
        let local = tree
            .local_classes
            .iter()
            .find(|l| l.binary_name == "A$2$1L")
            .unwrap();
        assert_eq!((local.outer.as_str(), local.owner), ("A$2", Some(0)));
        let field_anon = tree
            .local_classes
            .iter()
            .find(|l| l.binary_name == "A$1")
            .unwrap();
        assert_eq!((field_anon.name.as_str(), field_anon.owner), ("", None));
    }

    #[test]
    fn body_edits_do_not_change_item_tree() {
        let a = lower("class A { int f() { return 1; } }");
        let b = lower("class A { int f() {\n\n  int x = 2; return x + 1; } }");
        assert_eq!(a, b);
    }

    #[test]
    fn record_and_enum_synthetics() {
        let tree = lower("record P(int x, String... rest) { P { } int x() { return x; } }");
        let p = &tree.classes[0];
        assert_eq!(p.fields.len(), 2);
        // 显式声明了 x()，只为 rest 合成访问器
        assert_eq!(p.methods.iter().filter(|m| m.name == "x").count(), 1);
        assert!(p
            .methods
            .iter()
            .any(|m| m.name == "rest" && m.flags.contains(Modifiers::SYNTHETIC)));
        // 紧凑构造器就是规范构造器，不再合成
        assert_eq!(p.constructors().count(), 1);

        let tree = lower("enum E { A, B; }");
        let e = &tree.classes[0];
        assert!(e.fields.iter().all(|f| f.is_enum_constant()));
        assert!(e.methods.iter().any(|m| m.name == "values"));
        assert!(e.constructors().count() == 1);
    }

    #[test]
    fn interface_members_get_implicit_modifiers() {
        let tree = lower("interface I { int X = 1; void f(); default void g() {} }");
        let i = &tree.classes[0];
        assert!(i.fields[0]
            .flags
            .contains(Modifiers::PUBLIC | Modifiers::STATIC | Modifiers::FINAL));
        assert!(i.methods[0].flags.is_abstract());
        assert!(!i.methods[1].flags.is_abstract());
    }
}
