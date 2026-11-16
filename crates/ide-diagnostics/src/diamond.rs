//! 可以换成 `<>` 的显式类型实参：`List<String> names = new ArrayList<String>();`。
//!
//! 只看初值、赋值右边和返回值（目标类型确定），而且要求：目标类型就是 `new` 的类型在那个父类型上的
//! 样子（没有通配符之类的差别），`new` 的类的每个类型参数都直接出现在目标类型的实参里（菱形推断
//! 只靠目标类型就能得到同样的实参）。匿名类不报。

use base_db::{Db, SourceFile};
use hir::{InferenceResult, Ty};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange,
};

use crate::{boxing::target_type, view::View, Diagnostic, DiagnosticCode, Severity};

fn redundant_type_args(
    view: &View<'_>,
    result: &InferenceResult,
    new: &ast::NewExpr,
) -> Option<TextRange> {
    let db = view.db;
    if new.body().is_some() || new.qualifier().is_some() {
        return None;
    }
    let ast::Type::ClassType(class_ty) = new.ty()? else {
        return None;
    };
    let args = class_ty.type_arg_list()?;
    if args
        .syntax()
        .text()
        .to_string()
        .replace(char::is_whitespace, "")
        == "<>"
    {
        return None;
    }
    // `Outer<A>.Inner<B>` 不处理
    let mut qualifier = class_ty.qualifier();
    while let Some(q) = qualifier {
        if q.type_arg_list().is_some() {
            return None;
        }
        qualifier = q.qualifier();
    }
    let actual = result.type_of(new.syntax().text_range())?;
    let Ty::Class(class, actual_args) = actual else {
        return None;
    };
    if actual_args.is_empty() || actual.contains_unknown() {
        return None;
    }
    let wanted = target_type(view, result, new.syntax(), false)?;
    let Ty::Class(target_class, target_args) = &wanted else {
        return None;
    };
    if target_args.is_empty()
        || hir::find_super(db, actual, *target_class).as_ref() != Some(&wanted)
    {
        return None;
    }
    // 每个类型参数都能从目标类型推出来
    let generic = hir::this_ty(db, *class);
    let generic_super = hir::find_super(db, &generic, *target_class)?;
    let determined = generic
        .type_args()
        .iter()
        .all(|param| generic_super.type_args().contains(param));
    determined.then(|| args.syntax().text_range())
}

pub(crate) fn redundant_diamond_args(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    if crate::language_level(view.db, view.file).is_some_and(|l| l < 7) {
        return;
    }
    if !view.file_contains("new ") {
        return;
    }
    for body in &view.bodies {
        let mut result: Option<std::sync::Arc<InferenceResult>> = None;
        for new in body.node.descendants().filter_map(ast::NewExpr::cast) {
            let has_args = new.ty().is_some_and(|t| {
                t.syntax()
                    .children()
                    .any(|c| c.kind() == SyntaxKind::TYPE_ARG_LIST)
            });
            if !has_args {
                continue;
            }
            let result = result.get_or_insert_with(|| body.result(view));
            let Some(range) = redundant_type_args(view, result, &new) else {
                continue;
            };
            out.push(Diagnostic {
                range,
                message: base_db::trs!(
                    "显式类型实参可以换成 `<>`",
                    "Explicit type arguments can be replaced with `<>`"
                )
                .to_owned(),
                severity: Severity::Hint,
                code: DiagnosticCode::RedundantTypeArguments,
                unused: true,
            });
        }
    }
}

/// 给快速修复用。
pub fn redundant_diamond_ranges(db: &dyn Db, file: SourceFile) -> Vec<TextRange> {
    let mut out = Vec::new();
    redundant_diamond_args(&View::file(db, file), &mut out);
    out.into_iter().map(|d| d.range).collect()
}
