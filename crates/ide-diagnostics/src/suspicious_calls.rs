//! 结果注定不对的调用：
//!
//! - `a.equals(b)` / `Objects.equals(a, b)` 的两个对象类型不可转换（`Integer` 和 `Long`），
//!   结果总是 false；
//! - 集合的 `contains` / `remove` / `indexOf` / `lastIndexOf`、`Map` 的 `get` / `containsKey` /
//!   `remove` / `getOrDefault` / `containsValue` 传入的对象不可能是元素 / 键 / 值。
//!
//! "不可转换"按强制转换的规则保守判断：两个互不继承的类；或者一个接口和一个没实现它的 final
//! 类。只看擦除后的类，类型变量、数组和未知类型都不判断。

use base_db::{Db, SourceFile};
use hir::{
    body_syntax, class_data, infer, is_subclass_of, ClassId, ClassKind, InferenceResult,
    Resolution, Ty,
};
use syntax::ast::{self, AstNode};

use crate::{bodies_in_file, Diagnostic, DiagnosticCode, Severity};

fn is_final(db: &dyn Db, class: ClassId) -> bool {
    class_data(db, class).is_some_and(|d| {
        d.flags.is_final() || matches!(d.kind, ClassKind::Enum | ClassKind::Record)
    })
}

fn is_interface(db: &dyn Db, class: ClassId) -> bool {
    class_data(db, class)
        .is_some_and(|d| matches!(d.kind, ClassKind::Interface | ClassKind::Annotation))
}

/// 两个类型的对象不可能相同（强制转换会是编译错误）。
fn inconvertible(db: &dyn Db, a: &Ty, b: &Ty) -> bool {
    let (Some(a), Some(b)) = (a.boxed(db).as_class(), b.boxed(db).as_class()) else {
        return false;
    };
    if is_subclass_of(db, a, b) || is_subclass_of(db, b, a) {
        return false;
    }
    match (is_interface(db, a), is_interface(db, b)) {
        (false, false) => true,
        (true, false) => is_final(db, b),
        (false, true) => is_final(db, a),
        (true, true) => false,
    }
}

fn class_named(db: &dyn Db, class: ClassId) -> String {
    class_data(db, class)
        .map(|d| d.binary_name.to_string())
        .unwrap_or_default()
}

/// 接收者类型在 `target` 上的第 `index` 个类型实参（通配符取上界）。
fn type_arg(db: &dyn Db, ty: &Ty, target: &str, index: usize) -> Option<Ty> {
    let target = hir::lookup_class(db, target)?;
    match hir::find_super(db, ty, target)? {
        Ty::Class(_, args) => args.get(index).map(|a| a.wildcard_bound(db)),
        _ => None,
    }
}

fn check_call(
    db: &dyn Db,
    result: &InferenceResult,
    call: &ast::MethodCall,
    out: &mut Vec<Diagnostic>,
) -> Option<()> {
    let name = call.name_ref()?;
    let Resolution::Method(method) = result.resolution(name.syntax().text_range())? else {
        return None;
    };
    let method_name = name.text();
    let args: Vec<ast::Expr> = call.arg_list()?.args().collect();
    let type_of = |e: &ast::Expr| result.type_of(e.syntax().text_range()).cloned();
    let owner = class_named(db, method.class);
    let sig = hir::method_sig(db, *method);
    let object_param = |i: usize| {
        sig.params
            .get(i)
            .and_then(Ty::as_class)
            .is_some_and(|c| class_named(db, c) == "java/lang/Object")
    };

    // equals
    let equals_pair = match (method_name.as_str(), owner.as_str(), args.as_slice()) {
        ("equals", "java/util/Objects", [a, b]) => Some((type_of(a)?, type_of(b)?, b.clone())),
        ("equals", _, [arg]) if object_param(0) && !sig.params.is_empty() => {
            let receiver = call.receiver()?;
            Some((type_of(&receiver)?, type_of(arg)?, arg.clone()))
        }
        _ => None,
    };
    if let Some((a, b, at)) = equals_pair {
        if inconvertible(db, &a, &b) {
            let (a, b) = (a.boxed(db).display(db), b.boxed(db).display(db));
            out.push(Diagnostic {
                range: at.syntax().text_range(),
                message: base_db::tr!(
                    "`equals()` 比较的两个对象类型不可转换（`{a}` 和 `{b}`），结果总是 false",
                    "`equals()` between objects of inconvertible types `{a}` and `{b}` is always false"
                ),
                severity: Severity::Warning,
                code: DiagnosticCode::SuspiciousCall,
                unused: false,
            });
        }
        return Some(());
    }

    // 集合 / Map
    let [first, ..] = args.as_slice() else {
        return None;
    };
    if !object_param(0) {
        return None;
    }
    let receiver_ty = type_of(&call.receiver()?)?;
    let expected = match method_name.as_str() {
        "contains" | "remove" | "indexOf" | "lastIndexOf" if args.len() == 1 => {
            type_arg(db, &receiver_ty, "java/util/Collection", 0)
        }
        "get" | "containsKey" if args.len() == 1 => type_arg(db, &receiver_ty, "java/util/Map", 0),
        "remove" | "getOrDefault" if args.len() == 2 => {
            type_arg(db, &receiver_ty, "java/util/Map", 0)
        }
        "containsValue" if args.len() == 1 => type_arg(db, &receiver_ty, "java/util/Map", 1),
        _ => None,
    }?;
    let actual = type_of(first)?;
    if inconvertible(db, &expected, &actual) {
        let (expected, actual) = (expected.display(db), actual.boxed(db).display(db));
        out.push(Diagnostic {
            range: first.syntax().text_range(),
            message: base_db::tr!(
                "可疑的 `{method_name}()` 调用：`{actual}` 类型的对象不可能是 `{expected}`",
                "Suspicious call to `{method_name}()`: an object of type `{actual}` can never be a `{expected}`"
            ),
            severity: Severity::Warning,
            code: DiagnosticCode::SuspiciousCall,
            unused: false,
        });
    }
    Some(())
}

pub(crate) fn suspicious_calls(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    for body in bodies_in_file(db, file) {
        let Some((_, node)) = body_syntax(db, body) else {
            continue;
        };
        let result = infer(db, body);
        for call in node.descendants().filter_map(ast::MethodCall::cast) {
            check_call(db, &result, &call, out);
        }
    }
}
