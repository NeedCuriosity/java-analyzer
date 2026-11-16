//! 注解用在了 `@Target` 不允许的位置（javac：annotation interface not applicable to this kind of declaration）。
//!
//! 只看声明上的注解（修饰符里的、枚举常量和类型参数上的），类型里面的注解不报：
//! `<T> @A T f()` 这种写法里类型位置上的注解其实也是方法的声明注解。
//! 注解类型必须写明了 `@Target`，并且每个取值都认得出来。

use base_db::{Db, SourceFile};
use hir::{class_data, ClassKind, Semantics, TypeNs, TypeScope};
use ide_db::annotations::annotation_site;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind,
};

use crate::{Diagnostic, DiagnosticCode, Severity};

pub(crate) fn annotation_target_diagnostics(
    db: &dyn Db,
    file: SourceFile,
    out: &mut Vec<Diagnostic>,
) {
    let text = file.text(db);
    if !text.contains('@') {
        return;
    }
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    for annotation in root.descendants().filter_map(ast::Annotation::cast) {
        let declaration = annotation.syntax().parent().is_some_and(|p| {
            matches!(
                p.kind(),
                SyntaxKind::MODIFIER_LIST | SyntaxKind::ENUM_CONSTANT | SyntaxKind::TYPE_PARAM
            )
        });
        if !declaration {
            continue;
        }
        let Some(site) = annotation_site(annotation.syntax()) else {
            continue;
        };
        let Some(name) = annotation.name() else {
            continue;
        };
        let scope = sema.type_scope(file, annotation.syntax());
        let Some(TypeNs::Class(class)) = hir::resolve_type_path(db, &scope, &name.segments())
        else {
            continue;
        };
        let Some(data) = class_data(db, class) else {
            continue;
        };
        if data.kind != ClassKind::Annotation {
            continue;
        }
        let Some(targets) = data.targets else {
            continue;
        };
        if targets.intersects(site.accepts) || !target_is_reliable(db, class, &data) {
            continue;
        }
        out.push(Diagnostic {
            range: name.syntax().text_range(),
            message: base_db::tr!(
                "注解 `@{}` 不能用在这里（它的 @Target 是 {}）",
                "Annotation `@{}` is not applicable here (its @Target is {})",
                data.name,
                targets.names().collect::<Vec<_>>().join(", ")
            ),
            severity: Severity::Error,
            code: DiagnosticCode::AnnotationNotApplicable,
            unused: false,
        });
    }
}

/// 源码里声明的注解类型：记下的 `@Target` 确实是 `java.lang.annotation.Target`
/// （而不是碰巧同名的别的注解）。class 文件里的已经按全限定名认过了。
fn target_is_reliable(db: &dyn Db, class: hir::ClassId, data: &hir::ClassData) -> bool {
    if !data.is_source {
        return true;
    }
    let Some(file) = hir::class_decl_file(db, class) else {
        return false;
    };
    let scope = TypeScope::file(file);
    data.annotations
        .iter()
        .filter(|a| a.as_str() == "Target" || a.as_str() == "java.lang.annotation.Target")
        .any(|a| {
            let segments: Vec<hir::Name> = a.split('.').map(hir::Name::new).collect();
            matches!(
                hir::resolve_type_path(db, &scope, &segments),
                Some(TypeNs::Class(target)) if &*target.binary_name(db) == "java/lang/annotation/Target"
            )
        })
}
