//! 可以声明为 `static` 的私有方法：方法体里没有用到 `this` / `super`、本类（或外部类）的
//! 实例字段和实例方法，也没有用到类的类型参数。私有方法不会被覆写，加上 `static` 不改变行为。
//!
//! 方法体里有匿名类 / 局部类时不报（它们可能隐式地捕获外部实例）。

use std::sync::Arc;

use base_db::{Db, SourceFile};
use hir::{class_data, BodyId, ClassKind, Resolution, Visibility};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

/// 文件里所有方法引用（`x::foo`）的方法名。每次编辑后重新计算，但结果通常不变：
/// 单独检查一个体时只依赖它，不依赖整个文件的语法树。
#[salsa::tracked]
fn method_ref_names(db: &dyn Db, file: SourceFile) -> Arc<Vec<String>> {
    let root = base_db::parse(db, file).syntax_node();
    let names = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::METHOD_REF)
        .filter_map(|n| {
            n.children_with_tokens()
                .filter_map(|e| match e {
                    syntax::NodeOrToken::Node(n) if n.kind() == SyntaxKind::NAME_REF => {
                        Some(n.text().to_string())
                    }
                    syntax::NodeOrToken::Token(t) if t.kind() == SyntaxKind::IDENT => {
                        Some(t.text().to_string())
                    }
                    _ => None,
                })
                .last()
        })
        .collect();
    Arc::new(names)
}

pub(crate) fn methods_may_be_static(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    let level = crate::language_level(db, view.file);
    for file_body in &view.bodies {
        let body = file_body.id;
        let BodyId::Method(method) = body else {
            continue;
        };
        let Some(data) = class_data(db, method.class) else {
            continue;
        };
        let Some(method_data) = data.methods.get(method.idx as usize) else {
            continue;
        };
        if method_data.is_constructor
            || method_data.flags.is_static()
            || method_data.flags.visibility() != Visibility::Private
            || method_data.flags.contains(hir::Modifiers::NATIVE)
            || method_data.flags.contains(hir::Modifiers::SYNCHRONIZED)
            || !matches!(
                data.kind,
                ClassKind::Class | ClassKind::Enum | ClassKind::Record | ClassKind::Interface
            )
        {
            continue;
        }
        // 内部类（非静态）里的静态方法要 Java 16+
        let inner =
            data.outer.is_some() && !data.flags.is_static() && data.kind == ClassKind::Class;
        if inner && level.is_some_and(|l| l < 16) {
            continue;
        }
        let Some(decl) = ast::MethodDecl::cast(file_body.node.clone()) else {
            continue;
        };
        let (Some(name), Some(block)) = (decl.name(), decl.body()) else {
            continue;
        };
        // `this::foo` 这样的绑定方法引用在方法变成 static 之后就编译不过了：同名的方法引用都跳过
        if method_ref_names(db, view.file)
            .iter()
            .any(|n| n == name.text().as_str())
        {
            continue;
        }
        // 序列化通过反射调用的私有实例方法
        if matches!(
            name.text().as_str(),
            "readObject" | "writeObject" | "readObjectNoData" | "readResolve" | "writeReplace"
        ) {
            continue;
        }
        // 方法上的注解（框架回调之类）不动
        if decl
            .modifier_list()
            .is_some_and(|m| m.annotations().next().is_some())
        {
            continue;
        }
        if block.syntax().descendants().any(|n| {
            matches!(
                n.kind(),
                SyntaxKind::THIS_EXPR | SyntaxKind::SUPER_EXPR | SyntaxKind::CLASS_BODY
            )
        }) {
            continue;
        }
        // 用到类的类型参数（签名或方法体里）
        let class_type_params: Vec<String> = decl
            .syntax()
            .ancestors()
            .filter_map(ast::TypeDecl::cast)
            .filter_map(|d| d.type_param_list())
            .flat_map(|l| {
                l.type_params()
                    .filter_map(|p| p.name().map(|n| n.text().to_string()))
                    .collect::<Vec<_>>()
            })
            .collect();
        if !class_type_params.is_empty() {
            let mentions = decl
                .syntax()
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .any(|t| {
                    t.kind() == SyntaxKind::IDENT && class_type_params.iter().any(|p| p == t.text())
                });
            if mentions {
                continue;
            }
        }
        let result = file_body.result(view);
        let uses_instance = block.syntax().descendants().any(|n| {
            let Some(name_ref) = ast::NameRef::cast(n) else {
                return false;
            };
            let Some(parent) = name_ref.syntax().parent() else {
                return false;
            };
            // 只看不带限定的名字：`x` / `foo()`；`obj.x` 不涉及 this
            let unqualified = match parent.kind() {
                SyntaxKind::NAME_EXPR => true,
                SyntaxKind::METHOD_CALL => {
                    ast::MethodCall::cast(parent.clone()).is_some_and(|c| c.receiver().is_none())
                }
                // `new Inner()`：非静态内部类需要外部实例
                SyntaxKind::CLASS_TYPE => {
                    return matches!(
                        result.resolution(name_ref.syntax().text_range()),
                        Some(Resolution::Method(_))
                    ) && parent
                        .parent()
                        .is_some_and(|p| p.kind() == SyntaxKind::NEW_EXPR)
                        && inner_class_ctor(db, &result, &name_ref);
                }
                _ => false,
            };
            if !unqualified {
                return false;
            }
            match result.resolution(name_ref.syntax().text_range()) {
                Some(Resolution::Field(field)) => class_data(db, field.class)
                    .and_then(|d| {
                        d.fields
                            .get(field.idx as usize)
                            .map(|f| !f.flags.is_static())
                    })
                    .unwrap_or(true),
                Some(Resolution::Method(m)) => class_data(db, m.class)
                    .and_then(|d| d.methods.get(m.idx as usize).map(|f| !f.flags.is_static()))
                    .unwrap_or(true),
                Some(_) => false,
                // 解析不了的名字：保守起见当作用到了实例
                None => parent.kind() == SyntaxKind::METHOD_CALL,
            }
        });
        if uses_instance {
            continue;
        }
        let method_name = name.text().to_string();
        out.push(Diagnostic {
            range: name.syntax().text_range(),
            message: base_db::tr!(
                "方法 `{method_name}` 可以声明为 `static`",
                "Method `{method_name}` can be `static`"
            ),
            severity: Severity::Hint,
            code: DiagnosticCode::MethodMayBeStatic,
            unused: false,
        });
    }
}

/// `new Inner()` 调用的构造器属于非静态的成员类。
fn inner_class_ctor(db: &dyn Db, result: &hir::InferenceResult, name_ref: &ast::NameRef) -> bool {
    let Some(Resolution::Method(ctor)) = result.resolution(name_ref.syntax().text_range()) else {
        return false;
    };
    class_data(db, ctor.class)
        .is_some_and(|d| d.outer.is_some() && !d.flags.is_static() && d.kind == ClassKind::Class)
}

/// 可以声明为 static 的私有方法（给快速修复用）。
pub fn methods_that_may_be_static(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    methods_may_be_static(&View::file(db, file), &mut out);
    out
}
