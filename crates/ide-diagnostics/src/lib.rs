//! 诊断：语法错误 + 保守的语义诊断。
//!
//! 语义诊断只报"确定是错的"：推断拿不准的地方一律是未知类型，不产生诊断。
//! 没有 JDK（类路径为空）时语义诊断整体关闭，否则满屏都是 `String` 无法解析。

mod exceptions;
pub use exceptions::{unhandled_exceptions, UnhandledException};
mod flow;
mod nullness;
mod typeck;

use base_db::{workspace, Db, FileKind, SourceFile};
use hir::{infer, package_exists, resolve_qualified_class, BodyId, InferDiagnosticKind, Semantics};
use rustc_hash::{FxHashMap, FxHashSet};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    SyntaxError,
    UnresolvedName,
    UnresolvedType,
    UnresolvedField,
    UnresolvedMethod,
    TypeMismatch,
    ArgumentMismatch,
    BadOperand,
    DuplicateVariable,
    AssignToFinal,
    InaccessibleMember,
    StaticContext,
    MissingImplementation,
    InvalidOverride,
    NonExhaustiveSwitch,
    UnresolvedImport,
    UnusedImport,
    UnusedLocal,
    UnusedPrivateMember,
    UnreachableCode,
    MissingReturn,
    StringEquality,
    SelfAssignment,
    SelfComparison,
    UninitializedVariable,
    UnhandledException,
    NullDereference,
    ConstantCondition,
}

impl DiagnosticCode {
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticCode::SyntaxError => "syntax-error",
            DiagnosticCode::UnresolvedName => "unresolved-name",
            DiagnosticCode::UnresolvedType => "unresolved-type",
            DiagnosticCode::UnresolvedField => "unresolved-field",
            DiagnosticCode::UnresolvedMethod => "unresolved-method",
            DiagnosticCode::TypeMismatch => "type-mismatch",
            DiagnosticCode::ArgumentMismatch => "argument-mismatch",
            DiagnosticCode::BadOperand => "bad-operand",
            DiagnosticCode::DuplicateVariable => "duplicate-variable",
            DiagnosticCode::AssignToFinal => "assign-to-final",
            DiagnosticCode::InaccessibleMember => "inaccessible-member",
            DiagnosticCode::StaticContext => "static-context",
            DiagnosticCode::MissingImplementation => "missing-implementation",
            DiagnosticCode::InvalidOverride => "invalid-override",
            DiagnosticCode::NonExhaustiveSwitch => "non-exhaustive-switch",
            DiagnosticCode::UnresolvedImport => "unresolved-import",
            DiagnosticCode::UnusedImport => "unused-import",
            DiagnosticCode::UnusedLocal => "unused-local",
            DiagnosticCode::UnusedPrivateMember => "unused-private-member",
            DiagnosticCode::UnreachableCode => "unreachable-code",
            DiagnosticCode::MissingReturn => "missing-return",
            DiagnosticCode::StringEquality => "string-equality",
            DiagnosticCode::SelfAssignment => "self-assignment",
            DiagnosticCode::SelfComparison => "self-comparison",
            DiagnosticCode::UninitializedVariable => "uninitialized-variable",
            DiagnosticCode::UnhandledException => "unhandled-exception",
            DiagnosticCode::NullDereference => "null-dereference",
            DiagnosticCode::ConstantCondition => "constant-condition",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub message: String,
    pub severity: Severity,
    pub code: DiagnosticCode,
    /// 编辑器会把这段代码显示成灰色（未使用）
    pub unused: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct DiagnosticsConfig {
    pub semantic: bool,
    pub unused_imports: bool,
    /// 未使用的局部变量和私有成员
    pub unused_symbols: bool,
    /// 流分析（不可达语句、缺少 return）和表达式检查项
    pub inspections: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        DiagnosticsConfig {
            semantic: true,
            unused_imports: true,
            unused_symbols: true,
            inspections: true,
        }
    }
}

pub fn syntax_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    base_db::parse(db, file)
        .errors()
        .iter()
        .map(|err| Diagnostic {
            range: err.range,
            message: err.message.clone(),
            severity: Severity::Error,
            code: DiagnosticCode::SyntaxError,
            unused: false,
        })
        .collect()
}

/// 文件里所有可推断的体。
pub fn bodies_in_file(db: &dyn Db, file: SourceFile) -> Vec<BodyId> {
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let mut seen = FxHashSet::default();
    let mut res = Vec::new();
    for node in root.descendants() {
        let is_body_root = match node.kind() {
            SyntaxKind::METHOD_DECL
            | SyntaxKind::CONSTRUCTOR_DECL
            | SyntaxKind::ENUM_CONSTANT
            | SyntaxKind::INITIALIZER_BLOCK => true,
            SyntaxKind::VARIABLE_DECLARATOR => node
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::FIELD_DECL),
            _ => false,
        };
        if !is_body_root {
            continue;
        }
        if let Some(body) = sema.body_of(file, &node) {
            if seen.insert(body) {
                res.push(body);
            }
        }
    }
    res
}

fn infer_diagnostics(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    for body in bodies_in_file(db, file) {
        db.unwind_if_revision_cancelled();
        let result = infer(db, body);
        for diag in result.diagnostics() {
            let (message, code) = match &diag.kind {
                InferDiagnosticKind::UnresolvedName { name } => (
                    format!("无法解析符号 `{name}`"),
                    DiagnosticCode::UnresolvedName,
                ),
                InferDiagnosticKind::UnresolvedField { name, receiver } => (
                    format!("类型 `{receiver}` 上没有字段 `{name}`"),
                    DiagnosticCode::UnresolvedField,
                ),
                InferDiagnosticKind::UnresolvedMethod { name, receiver } => (
                    match receiver {
                        Some(receiver) => format!("类型 `{receiver}` 上没有方法 `{name}`"),
                        None => format!("无法解析方法 `{name}`"),
                    },
                    DiagnosticCode::UnresolvedMethod,
                ),
                InferDiagnosticKind::TypeMismatch { expected, actual } => (
                    format!("类型不匹配：需要 `{expected}`，实际是 `{actual}`"),
                    DiagnosticCode::TypeMismatch,
                ),
                InferDiagnosticKind::ArgumentMismatch { name, args } => (
                    format!("`{name}` 没有可以接受实参 ({args}) 的重载"),
                    DiagnosticCode::ArgumentMismatch,
                ),
                InferDiagnosticKind::BadOperand { op, operands } => (
                    format!("运算符 `{op}` 不能用于 {operands}"),
                    DiagnosticCode::BadOperand,
                ),
                InferDiagnosticKind::DuplicateLocal { name } => (
                    format!("变量 `{name}` 已经在这个方法里定义过"),
                    DiagnosticCode::DuplicateVariable,
                ),
            };
            out.push(Diagnostic {
                range: diag.range,
                message,
                severity: Severity::Error,
                code,
                unused: false,
            });
        }
    }
}

/// 类及其所有祖先是否都解析成功：只有这样，"继承来的成员类型里也没有"才是确定的。
pub(crate) fn hierarchy_is_resolved(db: &dyn Db, class: hir::ClassId) -> bool {
    std::iter::once(class)
        .chain(hir::all_supers(db, class))
        .all(|c| {
            hir::class_data(db, c).is_some_and(|data| {
                let declared = data.superclass.iter().count() + data.interfaces.len();
                hir::class_supertypes(db, c).len() == declared
            })
        })
}

/// 类型位置上解析不了的简单名：声明的类型、类型实参、`new`、强制转换、`throws`、`catch` 等。
///
/// 只报确定的：带限定符的名字（第一段可能是包）、匿名类和局部类里的名字（可能来自
/// 推断时才知道的父类型）、有按需 import 解析不了的文件，一律不报。
fn unresolved_types(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let tree = hir::item_tree(db, file);

    let on_demand_ok = tree.imports.iter().filter(|i| i.is_on_demand).all(|i| {
        package_exists(db, &i.segments.join("/"))
            || resolve_qualified_class(db, &i.segments).is_some()
    });
    if !on_demand_ok {
        return;
    }

    // 文件里声明的类型名、类型参数名，以及单类型 import 带进来的名字（解析不了的 import 已经单独报过）
    let mut known: FxHashSet<String> = tree
        .imports
        .iter()
        .filter(|i| !i.is_on_demand)
        .filter_map(|i| i.segments.last().map(|s| s.to_string()))
        .collect();
    for node in root.descendants() {
        if matches!(
            node.kind(),
            SyntaxKind::TYPE_PARAM
                | SyntaxKind::CLASS_DECL
                | SyntaxKind::INTERFACE_DECL
                | SyntaxKind::ENUM_DECL
                | SyntaxKind::RECORD_DECL
                | SyntaxKind::ANNOTATION_TYPE_DECL
        ) {
            if let Some(name) = node.children().find_map(ast::Name::cast) {
                known.insert(name.text().to_string());
            }
        }
    }

    let mut resolved_hierarchy: FxHashMap<hir::ClassId, bool> = FxHashMap::default();
    for node in root.descendants() {
        let Some(class_ty) = ast::ClassType::cast(node.clone()) else {
            continue;
        };
        if class_ty.qualifier().is_some()
            || node
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::CLASS_TYPE)
        {
            continue;
        }
        let Some(name_ref) = class_ty.name_ref() else {
            continue;
        };
        let name = name_ref.text();
        if name == "var" || known.contains(name.as_str()) {
            continue;
        }
        if in_local_or_anonymous_class(&node) {
            continue;
        }
        // `outer.new Inner()`：`Inner` 是 `outer` 的类型的成员，不按简单名解析
        let qualified_new = node.parent().is_some_and(|p| {
            p.kind() == SyntaxKind::NEW_EXPR && p.children().any(|c| ast::Expr::can_cast(c.kind()))
        });
        if qualified_new {
            continue;
        }
        let scope = sema.type_scope(file, &node);
        if hir::resolve_simple_type(db, &scope, &name).is_some() {
            continue;
        }
        // 记录头部的组件类型可以用记录自己继承来的成员类型
        let inherited_by_decl = node
            .ancestors()
            .find_map(ast::TypeDecl::cast)
            .and_then(|decl| sema.class_of_decl(file, &decl))
            .is_some_and(|class| hir::member_type(db, class, &name).is_some());
        if inherited_by_decl {
            continue;
        }
        // 外围各层类的继承链必须完整，否则名字可能是没解析出来的父类型的成员类型
        let mut enclosing = scope.class;
        let mut reliable = true;
        while let Some(class) = enclosing {
            let ok = *resolved_hierarchy
                .entry(class)
                .or_insert_with(|| hierarchy_is_resolved(db, class));
            if !ok {
                reliable = false;
                break;
            }
            enclosing = hir::class_data(db, class).and_then(|data| {
                data.outer
                    .as_deref()
                    .and_then(|outer| hir::lookup_class(db, outer))
            });
        }
        if !reliable {
            continue;
        }
        out.push(Diagnostic {
            range: name_ref.syntax().text_range(),
            message: format!("无法解析类型 `{name}`"),
            severity: Severity::Error,
            code: DiagnosticCode::UnresolvedType,
            unused: false,
        });
    }
}

/// 节点是否在匿名类、枚举常量的类体或者局部类里面。
fn in_local_or_anonymous_class(node: &syntax::SyntaxNode) -> bool {
    let mut inside_type_decl = false;
    for ancestor in node.ancestors() {
        match ancestor.kind() {
            SyntaxKind::CLASS_BODY
                if ancestor.parent().is_some_and(|p| {
                    matches!(p.kind(), SyntaxKind::NEW_EXPR | SyntaxKind::ENUM_CONSTANT)
                }) =>
            {
                return true;
            }
            SyntaxKind::CLASS_DECL
            | SyntaxKind::INTERFACE_DECL
            | SyntaxKind::ENUM_DECL
            | SyntaxKind::RECORD_DECL
            | SyntaxKind::ANNOTATION_TYPE_DECL => inside_type_decl = true,
            SyntaxKind::BLOCK if inside_type_decl => return true,
            _ => {}
        }
    }
    false
}

/// 除 import 以外，文件里出现过的全部标识符（含注释里的词，Javadoc 的 `{@link Foo}` 也算使用）。
pub fn used_identifiers(cu: &ast::CompilationUnit) -> FxHashSet<String> {
    let mut used = FxHashSet::default();
    let tokens = cu
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token());
    for token in tokens {
        match token.kind() {
            SyntaxKind::IDENT => {
                let in_import = token
                    .parent_ancestors()
                    .any(|n| n.kind() == SyntaxKind::IMPORT_DECL);
                if !in_import {
                    used.insert(token.text().to_owned());
                }
            }
            kind if kind.is_trivia() && token.text().starts_with('/') => {
                for word in token
                    .text()
                    .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                    .filter(|w| !w.is_empty())
                {
                    used.insert(word.to_owned());
                }
            }
            _ => {}
        }
    }
    used
}

fn import_diagnostics(
    db: &dyn Db,
    file: SourceFile,
    config: &DiagnosticsConfig,
    out: &mut Vec<Diagnostic>,
) {
    let cu = base_db::parse(db, file).tree();
    let used = used_identifiers(&cu);
    let mut seen: FxHashSet<(bool, bool, String)> = FxHashSet::default();

    for import in cu.imports() {
        let Some(name) = import.name() else { continue };
        let segments = name.segments();
        let Some(last) = segments.last() else {
            continue;
        };
        let range = import.syntax().text_range();
        let is_static = import.is_static();
        let on_demand = import.is_on_demand();

        // 能否解析
        let resolved = if on_demand {
            resolve_qualified_class(db, &segments).is_some()
                || (!is_static && package_exists(db, &segments.join("/")))
        } else if is_static {
            // 只确认类存在；成员可能是继承来的或由注解处理器生成
            segments.len() >= 2
                && resolve_qualified_class(db, &segments[..segments.len() - 1]).is_some()
        } else {
            resolve_qualified_class(db, &segments).is_some()
        };
        if !resolved {
            out.push(Diagnostic {
                range: name.syntax().text_range(),
                message: format!("无法解析 import `{}`", name.text()),
                severity: Severity::Error,
                code: DiagnosticCode::UnresolvedImport,
                unused: false,
            });
            continue;
        }

        if !config.unused_imports {
            continue;
        }
        let duplicate = !seen.insert((is_static, on_demand, name.text()));
        let unused = duplicate || (!on_demand && !used.contains(last.as_str()));
        if unused {
            out.push(Diagnostic {
                range,
                message: if duplicate {
                    format!("重复的 import `{}`", name.text())
                } else {
                    format!("未使用的 import `{}`", name.text())
                },
                severity: Severity::Hint,
                code: DiagnosticCode::UnusedImport,
                unused: true,
            });
        }
    }
}

/// 声明了但从未读写的局部变量。
fn unused_locals(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    for body in bodies_in_file(db, file) {
        let result = infer(db, body);
        let mut used = vec![false; result.locals().len()];
        for (_, resolution) in result.resolutions() {
            if let hir::Resolution::Local(id) = resolution {
                if let Some(slot) = used.get_mut(*id as usize) {
                    *slot = true;
                }
            }
        }
        for (local, used) in result.locals().iter().zip(used) {
            // 形参、catch 参数、lambda 参数、模式变量没用到很常见，不提示；
            // `for (E e : xs) n++;` 这种只为计数的遍历也一样
            let reportable = local.kind == hir::LocalKind::Local;
            if used || !reportable || local.name.starts_with('_') || local.name == "ignored" {
                continue;
            }
            out.push(Diagnostic {
                range: local.decl_range,
                message: format!("局部变量 `{}` 从未被使用", local.name),
                severity: Severity::Hint,
                code: DiagnosticCode::UnusedLocal,
                unused: true,
            });
        }
    }
}

/// 没有任何引用的私有字段和私有方法。带注解的成员可能被框架反射使用，不提示。
fn unused_private_members(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    use hir::{Definition, FieldId, MethodId, Visibility};

    let tree = hir::item_tree(db, file);
    let root = base_db::parse(db, file).syntax_node();
    let has_annotation = |range: TextRange| {
        let node = match root.covering_element(range) {
            syntax::NodeOrToken::Node(node) => node,
            syntax::NodeOrToken::Token(token) => match token.parent() {
                Some(node) => node,
                None => return true,
            },
        };
        // 字段的声明节点是 VARIABLE_DECLARATOR，注解挂在外层的 FIELD_DECL 上
        // 范围可能正好落在名字 token 上：NAME → VARIABLE_DECLARATOR → FIELD_DECL
        node.ancestors()
            .take(3)
            .filter_map(|n| n.children().find_map(ast::ModifierList::cast))
            .any(|mods| mods.annotations().next().is_some())
    };

    for class_data in tree.classes.iter() {
        // 类上有注解（Lombok、JPA、序列化框架 …）时成员可能被间接使用
        if !class_data.annotations.is_empty() {
            continue;
        }
        let class = hir::ClassId::from_binary_name(db, &class_data.binary_name);
        let mut candidates: Vec<(Definition, &str, &str)> = Vec::new();
        for (idx, field) in class_data.fields.iter().enumerate() {
            // 记录的组件通过同名的访问器使用，字段本身看不到引用
            let record_component =
                class_data.kind == hir::ClassKind::Record && !field.flags.is_static();
            if field.flags.visibility() == Visibility::Private
                && !field.flags.contains(hir::Modifiers::SYNTHETIC)
                && field.name != "serialVersionUID"
                && !record_component
            {
                let def = Definition::Field(FieldId {
                    class,
                    idx: idx as u32,
                });
                candidates.push((def, field.name.as_str(), "字段"));
            }
        }
        for (idx, method) in class_data.methods.iter().enumerate() {
            let serialization_hook = matches!(
                method.name.as_str(),
                "readObject" | "writeObject" | "readResolve" | "writeReplace" | "readObjectNoData"
            );
            if method.flags.visibility() == Visibility::Private
                && !method.is_constructor
                && !method.flags.contains(hir::Modifiers::SYNTHETIC)
                && !serialization_hook
            {
                let def = Definition::Method(MethodId {
                    class,
                    idx: idx as u32,
                });
                candidates.push((def, method.name.as_str(), "方法"));
            }
        }
        for (def, name, what) in candidates {
            let Some(source) = def.source(db) else {
                continue;
            };
            if has_annotation(source.full_range) {
                continue;
            }
            // 私有成员只可能在本文件里被引用
            if ide_db::search::references_in_file(db, file, &def, name).is_empty() {
                out.push(Diagnostic {
                    range: source.focus_range,
                    message: format!("私有{what} `{name}` 从未被使用"),
                    severity: Severity::Hint,
                    code: DiagnosticCode::UnusedPrivateMember,
                    unused: true,
                });
            }
        }
    }
}

/// 一个文件的全部诊断，按位置排序。
pub fn diagnostics(db: &dyn Db, file: SourceFile, config: &DiagnosticsConfig) -> Vec<Diagnostic> {
    let mut res = syntax_diagnostics(db, file);
    let syntax_ok = res.is_empty();

    let ws = workspace(db);
    let is_workspace_file = ws
        .files(db)
        .get(file.file_id(db))
        .is_some_and(|entry| matches!(entry.kind, FileKind::Workspace { .. }));
    // 没有 JDK 时连 `String` 都解析不了，语义诊断没有意义
    let has_jdk = hir::lookup_class(db, "java/lang/Object").is_some();

    if config.semantic && is_workspace_file && has_jdk {
        infer_diagnostics(db, file, &mut res);
        unresolved_types(db, file, &mut res);
        if syntax_ok {
            typeck::typeck_diagnostics(db, file, &mut res);
        }
        import_diagnostics(db, file, config, &mut res);
        // 残缺的语法树上做流分析只会得到误报
        if config.inspections && syntax_ok {
            flow::flow_diagnostics(&base_db::parse(db, file).syntax_node(), &mut res);
            flow::expression_inspections(db, file, &mut res);
            exceptions::exception_diagnostics(db, file, &mut res);
            nullness::nullness_diagnostics(db, file, &mut res);
        }
        if config.unused_symbols {
            unused_locals(db, file, &mut res);
            unused_private_members(db, file, &mut res);
        }
    }
    res.sort_by_key(|d| (d.range.start(), d.range.end()));
    res.dedup();
    res
}

#[cfg(test)]
mod tests {
    use test_fixture::fixture;

    use super::*;

    fn check(text: &str) -> Vec<(DiagnosticCode, String)> {
        let fx = fixture(text);
        let file = fx.file().source;
        let source: &str = file.text(&fx.db);
        diagnostics(&fx.db, file, &DiagnosticsConfig::default())
            .into_iter()
            .map(|d| {
                let range = usize::from(d.range.start())..usize::from(d.range.end());
                (d.code, source[range].to_owned())
            })
            .collect()
    }

    #[test]
    fn clean_file_has_no_diagnostics() {
        let diags = check(
            r#"
import java.util.List;
import java.util.ArrayList;

class Main {
    int total(List<Integer> xs) {
        int sum = 0;
        for (Integer x : xs) {
            sum += x;
        }
        List<String> names = new ArrayList<>();
        names.add("a");
        return sum + names.size();
    }
}
"#,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn reports_syntax_errors() {
        let diags = check("class Main { void f() { int x = ; } }");
        assert!(
            diags
                .iter()
                .any(|(code, _)| *code == DiagnosticCode::SyntaxError),
            "{diags:?}"
        );
    }

    #[test]
    fn reports_unresolved_and_unused_imports() {
        let diags = check(
            r#"
import java.util.List;
import java.util.Missing;
import java.util.Map;

class Main { Map<String, String> m; }
"#,
        );
        assert!(diags.contains(&(
            DiagnosticCode::UnresolvedImport,
            "java.util.Missing".to_owned()
        )));
        assert!(diags.contains(&(
            DiagnosticCode::UnusedImport,
            "import java.util.List;".to_owned()
        )));
        assert!(
            !diags.iter().any(|(_, text)| text.contains("Map")),
            "{diags:?}"
        );
    }

    #[test]
    fn reports_unused_locals_and_private_members() {
        let diags = check(
            r#"
class Main {
    private int used;
    private int unused;
    private void helper() {}
    private void dead() {}
    @Deprecated private int annotated;

    int f(int param) {
        int a = 1;
        int b = 2;
        helper();
        return used + a;
    }
}
"#,
        );
        let unused: Vec<&str> = diags
            .iter()
            .filter(|(code, _)| {
                matches!(
                    code,
                    DiagnosticCode::UnusedLocal | DiagnosticCode::UnusedPrivateMember
                )
            })
            .map(|(_, text)| text.as_str())
            .collect();
        assert_eq!(unused, ["unused", "dead", "b"]);
    }

    #[test]
    fn reports_unresolved_name() {
        let diags = check("class Main { int f() { return missing; } }");
        assert!(
            diags.contains(&(DiagnosticCode::UnresolvedName, "missing".to_owned())),
            "{diags:?}"
        );
    }

    #[test]
    fn inspections_report_suspicious_expressions() {
        let diags = check(
            r#"
class Main {
    private int count;
    boolean f(String a, String b, int n) {
        this.count = this.count;
        n = n;
        if (n == n) { return false; }
        return a == b;
    }
    boolean g(String a) { return a == null; }
}
"#,
        );
        let codes: Vec<DiagnosticCode> = diags.iter().map(|(code, _)| *code).collect();
        assert_eq!(
            codes,
            [
                DiagnosticCode::SelfAssignment,
                DiagnosticCode::SelfAssignment,
                DiagnosticCode::SelfComparison,
                DiagnosticCode::StringEquality,
            ],
            "{diags:?}"
        );
    }

    #[test]
    fn flow_checks_are_skipped_on_broken_code() {
        let diags = check("class Main { int f() { return 1; int x = ; } }");
        let flow_codes = [
            DiagnosticCode::UnreachableCode,
            DiagnosticCode::MissingReturn,
        ];
        assert!(
            diags.iter().all(|(code, _)| !flow_codes.contains(code)),
            "{diags:?}"
        );
    }

    #[test]
    fn reports_reads_of_unassigned_locals() {
        let diags = check(
            r#"
class Main {
    int f(boolean b, int n) {
        int never;
        int later;
        int ok;
        int both;
        int oneBranch;
        int inLoop;
        int orReturn;
        int inTry;
        ok = 1;
        if (b) { both = 1; } else { both = 2; }
        if (b) { oneBranch = 1; }
        while (n > 0) { inLoop = n; n--; }
        if (b) { orReturn = 1; } else { return 0; }
        try { inTry = Integer.parseInt("1"); } catch (RuntimeException e) { e.toString(); }
        int sum = never + later + ok + both + oneBranch + inLoop + orReturn + inTry;
        later = later + 1;
        return sum + later;
    }
}
"#,
        );
        let uninit: Vec<&str> = diags
            .iter()
            .filter(|(code, _)| *code == DiagnosticCode::UninitializedVariable)
            .map(|(_, text)| text.as_str())
            .collect();
        assert_eq!(
            uninit,
            ["never", "later", "oneBranch", "inLoop", "inTry"],
            "{diags:?}"
        );
    }

    #[test]
    fn definite_assignment_has_no_false_positives() {
        let diags = check(
            r#"
class Main {
    int f(boolean b, int n) {
        int viaSwitch;
        switch (n) {
            case 1: viaSwitch = 1; break;
            default: viaSwitch = 2;
        }
        int viaFinally;
        try { n++; } finally { viaFinally = 3; }
        int viaLoop;
        while (true) { viaLoop = 4; break; }
        int viaDo;
        do { viaDo = 5; } while (b);
        int viaThrow;
        if (b) { viaThrow = 6; } else { throw new RuntimeException(); }
        int inLambda;
        inLambda = 7;
        Runnable r = () -> { int copy = inLambda; };
        r.run();
        return viaSwitch + viaFinally + viaLoop + viaDo + viaThrow;
    }
}
"#,
        );
        assert!(
            diags
                .iter()
                .all(|(code, _)| *code != DiagnosticCode::UninitializedVariable),
            "{diags:?}"
        );
    }

    #[test]
    fn reports_unhandled_checked_exceptions() {
        let diags = check(
            r#"
import java.io.IOException;

class Main {
    void read() throws IOException { throw new IOException(); }

    void declared() throws IOException { read(); }
    void wider() throws Exception { read(); }
    void caught() {
        try { read(); } catch (IOException e) { e.toString(); }
    }
    void unchecked() { throw new RuntimeException(); }
    void inLambda() { Runnable r = () -> { read(); }; r.run(); }

    void missing() { read(); }
    void wrongCatch() {
        try { read(); } catch (RuntimeException e) { e.toString(); }
    }
    void thrown() { throw new IOException("x"); }
    void inCatch() {
        try { unchecked(); } catch (RuntimeException e) { read(); }
    }
}
"#,
        );
        let unhandled: Vec<&str> = diags
            .iter()
            .filter(|(code, _)| *code == DiagnosticCode::UnhandledException)
            .map(|(_, text)| text.as_str())
            .collect();
        assert_eq!(
            unhandled,
            // 第一条在 lambda 里：`Runnable.run()` 不声明任何受检异常
            ["read", "read", "read", "IOException", "read"],
            "{diags:?}"
        );
    }

    /// 每个文件里错误级的诊断（提示和警告不算）。
    fn errors_in_all_files(text: &str) -> Vec<(String, DiagnosticCode, String)> {
        let fx = fixture(text);
        let mut res = Vec::new();
        for file in &fx.files {
            let source: &str = file.source.text(&fx.db);
            for d in diagnostics(&fx.db, file.source, &DiagnosticsConfig::default()) {
                if d.severity == Severity::Error {
                    let range = usize::from(d.range.start())..usize::from(d.range.end());
                    res.push((file.path.clone(), d.code, source[range].to_owned()));
                }
            }
        }
        res
    }

    // 下面几组都是在 JDK 源码上跑全量分析时遇到过的误报，写法取自真实代码。

    #[test]
    fn no_false_errors_in_anonymous_and_local_classes() {
        let errors = errors_in_all_files(
            r#"
//- /app/Parser.java
package app;

abstract class Parser {
    enum Kind { INLINE, BLOCK }
    abstract int parse(int pos, Kind kind);
    int base() { return 1; }
}
//- /app/Main.java
package app;

class Main {
    int anonymous(String name) {
        Parser p = new Parser() {
            boolean done = true;
            int count;

            boolean done() { return done; }
            String name() { return name.trim(); }

            @Override
            int parse(int pos, Kind kind) {
                this.count = pos;
                int extra = switch (kind) {
                    case INLINE -> 1;
                    case BLOCK -> Kind.BLOCK == kind ? 2 : 3;
                };
                return super.base() + this.count + extra + (done() ? 1 : 0);
            }
        };
        return p.parse(0, Parser.Kind.INLINE);
    }

    int varOfAnonymousType() {
        var counter = new Object() {
            int hits = 3;
        };
        return counter.hits;
    }

    Object localClass(boolean b) {
        class Dummy {
            static final int LIMIT = 4;
        }
        if (b) {
            return Dummy.class;
        }
        return Dummy.LIMIT;
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:#?}");
    }

    #[test]
    fn no_false_errors_from_type_name_lookup() {
        let errors = errors_in_all_files(
            r#"
//- /lib/Table.java
package lib;

public class Table {
    // 私有的成员类型不被继承，也挡住了更上层的同名类型
    private static class Entry { }
    Object first() { return new Entry(); }
}
//- /lib/Hidden.java
package lib;

// 包私有：`import lib.*` 带不进它
class Shape { }
//- /lib/Tree.java
package lib;

public interface Tree {
    enum Kind { LEAF }
}
//- /lib/Node.java
package lib;

public class Node implements Tree {
    public static class Leaf extends Node { }
}
//- /app/Kinds.java
package app;

public class Kinds {
    public enum Kind { VAL, ERR }
}
//- /app/Shape.java
package app;

public class Shape {
    public int sides() { return 3; }
}
//- /app/Main.java
package app;

import lib.*;
import lib.Node.*;
import static app.Kinds.*;

class Main {
    static final class Entry {
        int version() { return 1; }
    }

    static final class Cache extends Table {
        int read(Object item) {
            var entry = (Entry) item;
            return entry.version();
        }
    }

    class Inner {
        int size() { return 2; }
    }

    int imports(Shape shape, Leaf leaf) {
        // `Kind` 来自静态 import：`lib.Node.*` 不带进 Node 继承来的 `Tree.Kind`
        Kind kind = Kind.ERR;
        return shape.sides() + (kind == Kind.VAL ? 1 : 0) + leaf.hashCode();
    }

    int qualifiedNew(Main outer) {
        Main.Inner inner = outer.new Inner();
        return inner.size();
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:#?}");
    }

    #[test]
    fn no_false_errors_from_flow_analysis() {
        let errors = errors_in_all_files(
            r#"
import java.io.IOException;

class Main {
    static final boolean DEBUG = false;

    int labeled(int[] xs) {
        outer:
        for (;;) {
            for (int x : xs) {
                if (x > 0) {
                    break outer;
                }
                if (x < 0) {
                    return x;
                }
            }
        }
        return 0;
    }

    int endless(int[] xs) {
        scan:
        for (;;) {
            inner:
            for (int x : xs) {
                if (x == 0) continue scan;
                if (x == 1) break inner;
                return x;
            }
        }
    }

    int sharedSwitchScope(int tag) {
        switch (tag) {
            case 1:
                int width = 2;
                return width;
            case 2:
                width = 3;
                return width;
            default:
                return 0;
        }
    }

    int constantCondition(int n) {
        int pos;
        if (DEBUG) {
            pos = n;
        }
        try {
            n++;
        } finally {
            if (DEBUG) {
                n += pos;
            }
        }
        return n;
    }

    int assignedOrThrown(int code) throws IOException {
        int[] values;
        try {
            values = load(code);
        } catch (IOException e) {
            switch (code) {
                case 1:
                case 2:
                    throw new IllegalStateException();
                default:
                    throw e;
            }
        }
        return values.length;
    }

    int[] load(int code) throws IOException {
        if (code < 0) throw new IOException();
        return new int[code];
    }

    int arrays(byte[] key) {
        byte[] copy = key.clone();
        Object[] boxes = new Object[] { copy }.clone();
        return copy.length + boxes.length;
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:#?}");
    }

    #[test]
    fn no_false_errors_from_generic_inference() {
        let errors = errors_in_all_files(
            r#"
import java.util.List;
import java.util.Map;
import java.util.Optional;

class Box<T> {
    T value;
}

class Main {
    static <V> V pick(Box<?> hint, Box<? extends V> typed, V fallback) { return fallback; }

    int size() { return 1; }

    // 第一个实参是 `Box<?>`，不提供信息；V 要由后面的实参定出来
    int wildcardFirst(Box<?> any, Box<Main> typed) {
        return pick(any, typed, this).size();
    }

    // 接收者是不带实参的泛型类：返回类型靠目标类型推断，这里不能当成 Object
    String rawReceiverMethodRef(List<Map.Entry<String, Integer>> entries) {
        return entries.stream().map(Map.Entry::getKey).findFirst().orElse("none");
    }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:#?}");
    }
}
