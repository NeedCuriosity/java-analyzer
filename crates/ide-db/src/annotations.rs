//! 注解写在什么位置：被注解的东西对应 `@Target` 里的哪些 `ElementType`（JLS 9.6.4.1、9.7.4）。

use hir::AnnotationTargets as T;
use syntax::{SyntaxKind, SyntaxNode};

/// 一个注解所在的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnotationSite {
    /// 适用位置（`@Target`）里有这些位之一的注解才能写在这里
    pub accepts: u16,
    /// 被注解的是什么已经确定。不确定的情形：类体里、语句开头的 `@`，后面那个声明可能
    /// 只是碰巧跟在后面（正在它前面写一个新声明）
    pub certain: bool,
}

impl AnnotationSite {
    fn new(accepts: u16, certain: bool) -> Option<AnnotationSite> {
        Some(AnnotationSite { accepts, certain })
    }
}

/// 声明处的注解若也能当类型注解，就也接受 `TYPE_USE`（JLS 9.7.4：声明有类型、或是类型声明 / 类型参数时）。
const DECL_TYPE_USE: u16 = T::TYPE_USE;

/// `ANNOTATION` 节点所在的位置。认不出来时（注解的参数里、残缺的代码）返回 `None`。
pub fn annotation_site(annotation: &SyntaxNode) -> Option<AnnotationSite> {
    debug_assert_eq!(annotation.kind(), SyntaxKind::ANNOTATION);
    let parent = annotation.parent()?;
    match parent.kind() {
        SyntaxKind::MODIFIER_LIST => {
            let owner = parent.parent()?;
            declaration_site(&owner)
        }
        SyntaxKind::ENUM_CONSTANT => AnnotationSite::new(T::FIELD | DECL_TYPE_USE, true),
        SyntaxKind::TYPE_PARAM => AnnotationSite::new(T::TYPE_PARAMETER | DECL_TYPE_USE, true),
        // 类型里面的注解：`List<@A String>`、`new @A Foo()`、`String @A []`
        SyntaxKind::CLASS_TYPE
        | SyntaxKind::PRIMITIVE_TYPE
        | SyntaxKind::ARRAY_TYPE
        | SyntaxKind::WILDCARD_TYPE => AnnotationSite::new(T::TYPE_USE, true),
        _ => None,
    }
}

fn declaration_site(owner: &SyntaxNode) -> Option<AnnotationSite> {
    let top_level = || {
        owner
            .parent()
            .is_some_and(|p| p.kind() == SyntaxKind::COMPILATION_UNIT)
    };
    match owner.kind() {
        SyntaxKind::CLASS_DECL
        | SyntaxKind::INTERFACE_DECL
        | SyntaxKind::ENUM_DECL
        | SyntaxKind::RECORD_DECL => AnnotationSite::new(T::TYPE | DECL_TYPE_USE, top_level()),
        SyntaxKind::ANNOTATION_TYPE_DECL => {
            AnnotationSite::new(T::TYPE | T::ANNOTATION_TYPE | DECL_TYPE_USE, top_level())
        }
        SyntaxKind::METHOD_DECL => {
            // 第一个类型子节点是返回类型；返回 void 的方法上不能写类型注解
            let returns_void = owner
                .children()
                .find(|c| {
                    matches!(
                        c.kind(),
                        SyntaxKind::PRIMITIVE_TYPE
                            | SyntaxKind::CLASS_TYPE
                            | SyntaxKind::ARRAY_TYPE
                    )
                })
                .is_some_and(|ty| ty.text().to_string().trim() == "void");
            let type_use = if returns_void { 0 } else { DECL_TYPE_USE };
            AnnotationSite::new(T::METHOD | type_use, false)
        }
        SyntaxKind::CONSTRUCTOR_DECL => AnnotationSite::new(T::CONSTRUCTOR | DECL_TYPE_USE, false),
        SyntaxKind::FIELD_DECL => AnnotationSite::new(T::FIELD | DECL_TYPE_USE, false),
        SyntaxKind::PARAM => param_site(owner),
        SyntaxKind::LOCAL_VAR_DECL => {
            // 语句开头的 `@`：也可能是在写一个局部类
            let for_init = owner
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::FOR_INIT);
            if for_init {
                AnnotationSite::new(T::LOCAL_VARIABLE | DECL_TYPE_USE, true)
            } else {
                AnnotationSite::new(T::LOCAL_VARIABLE | T::TYPE | DECL_TYPE_USE, false)
            }
        }
        SyntaxKind::RESOURCE | SyntaxKind::TYPE_PATTERN | SyntaxKind::RECORD_PATTERN => {
            AnnotationSite::new(T::LOCAL_VARIABLE | DECL_TYPE_USE, true)
        }
        SyntaxKind::PACKAGE_DECL => AnnotationSite::new(T::PACKAGE, true),
        SyntaxKind::MODULE_DECL => AnnotationSite::new(T::MODULE, true),
        // 只写了修饰符、后面还没有声明
        SyntaxKind::ERROR => match owner.parent()?.kind() {
            SyntaxKind::CLASS_BODY => AnnotationSite::new(
                T::TYPE
                    | T::ANNOTATION_TYPE
                    | T::FIELD
                    | T::METHOD
                    | T::CONSTRUCTOR
                    | DECL_TYPE_USE,
                false,
            ),
            SyntaxKind::COMPILATION_UNIT => AnnotationSite::new(
                T::TYPE
                    | T::ANNOTATION_TYPE
                    | T::PACKAGE
                    | T::MODULE
                    // 紧凑源文件里顶层可以写方法和字段
                    | T::FIELD
                    | T::METHOD
                    | DECL_TYPE_USE,
                false,
            ),
            SyntaxKind::PARAM_LIST => param_site(owner),
            _ => None,
        },
        _ => None,
    }
}

/// 形参（或记录组件，或 lambda / catch / 增强 for 的形参）。
fn param_site(param: &SyntaxNode) -> Option<AnnotationSite> {
    let list = param.parent()?;
    let record_component = list.kind() == SyntaxKind::PARAM_LIST
        && list
            .parent()
            .is_some_and(|p| p.kind() == SyntaxKind::RECORD_DECL);
    if record_component {
        // 记录组件上的注解会传到对应的字段、访问器和规范构造器的形参上
        return AnnotationSite::new(
            T::RECORD_COMPONENT | T::FIELD | T::METHOD | T::PARAMETER | DECL_TYPE_USE,
            true,
        );
    }
    // 增强 for 的变量是局部变量
    if list.kind() == SyntaxKind::FOREACH_STMT {
        return AnnotationSite::new(T::LOCAL_VARIABLE | DECL_TYPE_USE, true);
    }
    // catch 的形参：JLS 算形参，javac 当局部变量，两种都接受
    if list.kind() == SyntaxKind::CATCH_CLAUSE {
        return AnnotationSite::new(T::PARAMETER | T::LOCAL_VARIABLE | DECL_TYPE_USE, true);
    }
    AnnotationSite::new(T::PARAMETER | DECL_TYPE_USE, true)
}

#[cfg(test)]
mod tests {
    use syntax::ast::{self, AstNode};

    use super::*;

    fn site_of(text: &str, name: &str) -> Option<AnnotationSite> {
        let root = ast::CompilationUnit::parse(text).syntax_node();
        let annotation = root
            .descendants()
            .filter_map(ast::Annotation::cast)
            .find(|a| a.name().is_some_and(|n| n.text() == name))
            .expect("找不到注解");
        annotation_site(annotation.syntax())
    }

    #[test]
    fn sites_of_declarations() {
        let text = r#"
@Top class A<@Tp T> {
    @Field int x;
    @Ctor A() {}
    @Void void f(@Param String s) {
        @Local int y = 0;
        for (@Each String e : list) {}
        List<@Use String> l;
    }
    @Ret String g() { return null; }
    @Nested class B {}
    record R(@Comp int a) {}
    enum E { @Const ONE }
    @Dangling
}
"#;
        let site = |name| site_of(text, name).unwrap();
        assert_eq!(
            site("Top"),
            AnnotationSite {
                accepts: T::TYPE | T::TYPE_USE,
                certain: true
            }
        );
        assert_eq!(site("Tp").accepts, T::TYPE_PARAMETER | T::TYPE_USE);
        assert_eq!(site("Field").accepts, T::FIELD | T::TYPE_USE);
        assert_eq!(site("Ctor").accepts, T::CONSTRUCTOR | T::TYPE_USE);
        assert_eq!(site("Void").accepts, T::METHOD);
        assert_eq!(site("Ret").accepts, T::METHOD | T::TYPE_USE);
        assert_eq!(
            site("Param"),
            AnnotationSite {
                accepts: T::PARAMETER | T::TYPE_USE,
                certain: true
            }
        );
        assert_eq!(
            site("Local").accepts,
            T::LOCAL_VARIABLE | T::TYPE | T::TYPE_USE
        );
        assert_eq!(site("Each").accepts, T::LOCAL_VARIABLE | T::TYPE_USE);
        assert_eq!(
            site("Use"),
            AnnotationSite {
                accepts: T::TYPE_USE,
                certain: true
            }
        );
        assert!(!site("Nested").certain);
        assert!(site("Comp").accepts & T::RECORD_COMPONENT != 0);
        assert_eq!(site("Const").accepts, T::FIELD | T::TYPE_USE);
        assert!(!site("Dangling").certain);
    }
}
