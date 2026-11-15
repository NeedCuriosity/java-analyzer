//! 源码 → [`ItemTree`]。
//!
//! 同一趟遍历同时产出 [`SourceMap`]（声明下标 → 语法节点指针）。两者分开存放：
//! `ItemTree` 不含任何位置信息，方法体内的编辑不会改变它；`SourceMap` 每次编辑都会变，
//! 但只有真正需要语法节点的功能才依赖它。

use std::sync::Arc;

use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, SyntaxNodePtr,
};

use crate::data::{
    ClassData, ClassKind, DeclIdx, FieldData, Import, ItemTree, MethodData, Modifiers, Name,
    ParamData, PathSegment, PrimitiveType, TypeParamData, TypeRef, WildcardBound,
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
    let package: SmolStr = cu
        .package()
        .and_then(|p| p.name())
        .map(|n| SmolStr::new(n.segments().join("/")))
        .unwrap_or_default();

    let imports = cu
        .imports()
        .filter_map(|import| {
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

    let mut ctx = Ctx {
        package: package.clone(),
        classes: Vec::new(),
        decls: Vec::new(),
    };
    for decl in cu.type_decls() {
        ctx.lower_type_decl(&decl, None);
    }

    let classes = ctx.classes.into_iter().flatten().map(Arc::new).collect();
    (
        ItemTree {
            package,
            imports,
            classes,
        },
        SourceMap { decls: ctx.decls },
    )
}

struct Outer<'a> {
    binary_name: &'a str,
    is_interface: bool,
}

struct Ctx {
    package: SmolStr,
    /// 先占位再回填，保证先序下标
    classes: Vec<Option<ClassData>>,
    decls: Vec<SyntaxNodePtr>,
}

impl Ctx {
    fn alloc_decl(&mut self, node: &SyntaxNode) -> DeclIdx {
        self.decls.push(SyntaxNodePtr::new(node));
        (self.decls.len() - 1) as DeclIdx
    }

    fn lower_type_decl(&mut self, decl: &ast::TypeDecl, outer: Option<&Outer<'_>>) -> Option<Name> {
        let name = decl.name()?.text();
        let binary_name: SmolStr = match outer {
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
        let is_interface = matches!(kind, ClassKind::Interface | ClassKind::Annotation);

        let slot = self.classes.len();
        self.classes.push(None);
        let decl_idx = self.alloc_decl(decl.syntax());

        let (mut flags, annotations) = lower_modifiers(decl.modifier_list());
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
            });
        }

        // 每个字段上的注解（Lombok 的字段级注解需要）
        let mut field_annotations: Vec<(usize, Vec<SmolStr>)> = Vec::new();
        let this_outer = Outer {
            binary_name: &binary_name,
            is_interface,
        };
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
                data.fields.push(FieldData {
                    name: cname,
                    ty: self_ty.clone(),
                    flags,
                    decl: Some(idx),
                });
            }
            for member in body.members() {
                match member {
                    ast::Member::FieldDecl(field) => {
                        self.lower_field(&field, is_interface, &mut data, &mut field_annotations);
                    }
                    ast::Member::MethodDecl(method) => {
                        let idx = self.alloc_decl(method.syntax());
                        data.methods.push(lower_method(&method, kind, idx));
                    }
                    ast::Member::ConstructorDecl(ctor) => {
                        let idx = self.alloc_decl(ctor.syntax());
                        data.methods
                            .push(lower_constructor(&ctor, &name, &record_components, idx));
                    }
                    ast::Member::InitializerBlock(_) => {}
                    ast::Member::ClassDecl(it) => {
                        self.lower_nested(it.into(), &this_outer, &mut data)
                    }
                    ast::Member::InterfaceDecl(it) => {
                        self.lower_nested(it.into(), &this_outer, &mut data)
                    }
                    ast::Member::EnumDecl(it) => {
                        self.lower_nested(it.into(), &this_outer, &mut data)
                    }
                    ast::Member::RecordDecl(it) => {
                        self.lower_nested(it.into(), &this_outer, &mut data)
                    }
                    ast::Member::AnnotationTypeDecl(it) => {
                        self.lower_nested(it.into(), &this_outer, &mut data)
                    }
                }
            }
        }

        // Lombok 生成的成员要先于"隐式无参构造器"的判断
        if let Some(builder) = crate::lombok::apply(&mut data, &field_annotations, &self_ty) {
            data.nested.push(builder.name.clone());
            self.classes.push(Some(builder));
        }
        add_synthetic_members(&mut data, &record_components, &self_ty);
        self.classes[slot] = Some(data);
        Some(name)
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
        annotations_out: &mut Vec<(usize, Vec<SmolStr>)>,
    ) {
        let (mut flags, annotations) = lower_modifiers(field.modifier_list());
        if in_interface {
            flags.insert(Modifiers::PUBLIC | Modifiers::STATIC | Modifiers::FINAL);
        }
        let base = field.ty().map(|t| lower_type(&t)).unwrap_or(TypeRef::Error);
        for declarator in field.declarators() {
            let Some(name) = declarator.name().map(|n| n.text()) else {
                continue;
            };
            let idx = self.alloc_decl(declarator.syntax());
            if !annotations.is_empty() {
                annotations_out.push((data.fields.len(), annotations.clone()));
            }
            data.fields.push(FieldData {
                name,
                ty: base.clone().array_of(declarator.extra_dims()),
                flags,
                decl: Some(idx),
            });
        }
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
            annotations.push(SmolStr::new(name.text()));
        }
    }
    (flags, annotations)
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
    // 返回类型是第一个直接的 Type 子节点；`int f()[]` 这种旧写法的维度要补上
    let ret = method
        .return_type()
        .map(|t| lower_type(&t))
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
