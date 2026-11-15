//! 诊断：语法错误 + 保守的语义诊断。
//!
//! 语义诊断只报"确定是错的"：推断拿不准的地方一律是未知类型，不产生诊断。
//! 没有 JDK（类路径为空）时语义诊断整体关闭，否则满屏都是 `String` 无法解析。

mod exceptions;
mod field_final;
mod field_local;
mod format_strings;
pub use arrays::array_misuse;
pub use boxing::unnecessary_boxing_fixes;
pub use diamond::redundant_diamond_ranges;
pub use exceptions::{redundant_throws, unhandled_exceptions, RedundantThrows, UnhandledException};
pub use field_local::{fields_to_locals, FieldToLocal};
pub use language_level::language_level;
pub use loops::string_concat_in_loops;
pub use may_be_static::methods_that_may_be_static;
pub use optionals::unchecked_optional_get_diagnostics;
pub use probable_bugs::probable_bug_diagnostics;
pub use redundancy::{redundancy_fixes, RedundancyFix};
pub use suppress::suppression_key;
mod arrays;
mod boxing;
mod collections;
mod dead_stores;
mod diamond;
mod flow;
mod ignored_results;
mod inspections;
mod jumps;
mod lambdas;
mod language_level;
mod loops;
mod may_be_static;
mod nullness;
mod optionals;
mod pattern_vars;
mod probable_bugs;
mod recursion;
mod redundancy;
mod redundant_modifiers;
mod suppress;
mod suspicious_calls;
pub mod timing;
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
    AmbiguousCall,
    TypeArgOutOfBounds,
    BadOperand,
    DuplicateVariable,
    AssignToFinal,
    InaccessibleMember,
    StaticContext,
    MissingImplementation,
    InvalidOverride,
    SealedSubtype,
    NonExhaustiveSwitch,
    DominatedCase,
    UnresolvedImport,
    UnresolvedModule,
    CompactSourceFile,
    FeatureNotAvailable,
    UnusedImport,
    UnusedLocal,
    UnusedParameter,
    UnusedPrivateMember,
    UnreachableCode,
    MissingReturn,
    StringEquality,
    SelfAssignment,
    SelfComparison,
    UninitializedVariable,
    UnhandledException,
    NullDereference,
    /// 解引用可能为 null 的值（来自 `@Nullable`）
    NullableDereference,
    /// 把 null 传给 `@NotNull` 形参、从 `@NotNull` 方法返回 null
    NullabilityViolation,
    ConstantCondition,
    DeprecatedUsage,
    PointlessBoolean,
    UseIsEmpty,
    RedundantIf,
    RedundantCast,
    UnusedAssignment,
    FieldMayBeFinal,
    RedundantModifier,
    RedundantLocal,
    MalformedFormatString,
    SuspiciousCall,
    IgnoredResult,
    ArrayEquals,
    ArrayToString,
    NumberEquality,
    LoopConditionNotUpdated,
    StringConcatInLoop,
    InfiniteRecursion,
    RedundantJump,
    MismatchedCollection,
    IntegerDivision,
    OctalLiteral,
    EqualsHashCode,
    ThreadRun,
    MethodMayBeStatic,
    LambdaToMethodRef,
    OptionalGetWithoutCheck,
    FieldCanBeLocal,
    RedundantThrows,
    UnnecessaryBoxing,
    RedundantTypeArguments,
    CStyleArray,
    RedundantToString,
    RedundantStringConstructor,
    PatternVariable,
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
            DiagnosticCode::AmbiguousCall => "ambiguous-call",
            DiagnosticCode::TypeArgOutOfBounds => "type-arg-out-of-bounds",
            DiagnosticCode::BadOperand => "bad-operand",
            DiagnosticCode::DuplicateVariable => "duplicate-variable",
            DiagnosticCode::AssignToFinal => "assign-to-final",
            DiagnosticCode::InaccessibleMember => "inaccessible-member",
            DiagnosticCode::StaticContext => "static-context",
            DiagnosticCode::MissingImplementation => "missing-implementation",
            DiagnosticCode::InvalidOverride => "invalid-override",
            DiagnosticCode::SealedSubtype => "sealed-subtype",
            DiagnosticCode::NonExhaustiveSwitch => "non-exhaustive-switch",
            DiagnosticCode::DominatedCase => "dominated-case",
            DiagnosticCode::UnresolvedImport => "unresolved-import",
            DiagnosticCode::UnresolvedModule => "unresolved-module",
            DiagnosticCode::CompactSourceFile => "compact-source-file",
            DiagnosticCode::FeatureNotAvailable => "feature-not-available",
            DiagnosticCode::UnusedImport => "unused-import",
            DiagnosticCode::UnusedLocal => "unused-local",
            DiagnosticCode::UnusedParameter => "unused-parameter",
            DiagnosticCode::UnusedPrivateMember => "unused-private-member",
            DiagnosticCode::UnreachableCode => "unreachable-code",
            DiagnosticCode::MissingReturn => "missing-return",
            DiagnosticCode::StringEquality => "string-equality",
            DiagnosticCode::SelfAssignment => "self-assignment",
            DiagnosticCode::SelfComparison => "self-comparison",
            DiagnosticCode::UninitializedVariable => "uninitialized-variable",
            DiagnosticCode::UnhandledException => "unhandled-exception",
            DiagnosticCode::NullDereference => "null-dereference",
            DiagnosticCode::NullableDereference => "nullable-dereference",
            DiagnosticCode::NullabilityViolation => "nullability-violation",
            DiagnosticCode::ConstantCondition => "constant-condition",
            DiagnosticCode::DeprecatedUsage => "deprecated",
            DiagnosticCode::PointlessBoolean => "pointless-boolean",
            DiagnosticCode::UseIsEmpty => "use-is-empty",
            DiagnosticCode::RedundantIf => "redundant-if",
            DiagnosticCode::RedundantCast => "redundant-cast",
            DiagnosticCode::UnusedAssignment => "unused-assignment",
            DiagnosticCode::FieldMayBeFinal => "field-may-be-final",
            DiagnosticCode::RedundantModifier => "redundant-modifier",
            DiagnosticCode::RedundantLocal => "redundant-local",
            DiagnosticCode::MalformedFormatString => "malformed-format-string",
            DiagnosticCode::SuspiciousCall => "suspicious-call",
            DiagnosticCode::IgnoredResult => "ignored-result",
            DiagnosticCode::ArrayEquals => "array-equals",
            DiagnosticCode::ArrayToString => "array-to-string",
            DiagnosticCode::NumberEquality => "number-equality",
            DiagnosticCode::LoopConditionNotUpdated => "loop-condition-not-updated",
            DiagnosticCode::StringConcatInLoop => "string-concat-in-loop",
            DiagnosticCode::InfiniteRecursion => "infinite-recursion",
            DiagnosticCode::RedundantJump => "redundant-jump",
            DiagnosticCode::MismatchedCollection => "mismatched-collection",
            DiagnosticCode::IntegerDivision => "integer-division",
            DiagnosticCode::OctalLiteral => "octal-literal",
            DiagnosticCode::EqualsHashCode => "equals-hash-code",
            DiagnosticCode::ThreadRun => "thread-run",
            DiagnosticCode::MethodMayBeStatic => "method-may-be-static",
            DiagnosticCode::LambdaToMethodRef => "lambda-to-method-ref",
            DiagnosticCode::OptionalGetWithoutCheck => "optional-get-without-check",
            DiagnosticCode::FieldCanBeLocal => "field-can-be-local",
            DiagnosticCode::RedundantThrows => "redundant-throws",
            DiagnosticCode::UnnecessaryBoxing => "unnecessary-boxing",
            DiagnosticCode::RedundantTypeArguments => "redundant-type-arguments",
            DiagnosticCode::CStyleArray => "c-style-array",
            DiagnosticCode::RedundantToString => "redundant-to-string",
            DiagnosticCode::RedundantStringConstructor => "redundant-string-constructor",
            DiagnosticCode::PatternVariable => "pattern-variable",
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

/// 语法错误的文案。解析器（不依赖界面语言）产出中文，界面语言是英文时在这里翻译。
fn syntax_error_message(message: &str, lang: base_db::Lang) -> String {
    if lang == base_db::Lang::Zh {
        return message.to_owned();
    }
    let fixed = match message {
        "注释未闭合" => Some("Unterminated comment"),
        "字符串字面量未闭合" => Some("Unterminated string literal"),
        "字符字面量未闭合" => Some("Unterminated character literal"),
        "文本块未闭合" => Some("Unterminated text block"),
        "无法识别的字符" => Some("Unrecognized character"),
        _ => None,
    };
    if let Some(fixed) = fixed {
        return fixed.to_owned();
    }
    let Some(expected) = message.strip_prefix("此处应为") else {
        return message.to_owned();
    };
    let expected = expected.trim();
    let what = match expected {
        "标识符" => "identifier".to_owned(),
        "文件结尾" => "end of file".to_owned(),
        "声明" => "a declaration".to_owned(),
        "模块指令" => "a module directive".to_owned(),
        "名字" => "a name".to_owned(),
        "类型" => "a type".to_owned(),
        "类型名" => "a type name".to_owned(),
        "方法名" => "a method name".to_owned(),
        "实参列表" => "an argument list".to_owned(),
        "成员名" => "a member name".to_owned(),
        "表达式" => "an expression".to_owned(),
        "语句" => "a statement".to_owned(),
        "模式" => "a pattern".to_owned(),
        "绑定变量名" => "a binding variable name".to_owned(),
        "记录的组件列表" => "a record component list".to_owned(),
        "成员声明" => "a member declaration".to_owned(),
        "参数" => "a parameter".to_owned(),
        other => other
            .replace("标识符", "identifier")
            .replace("方法名", "a method name")
            .replace(" 或 ", " or ")
            .replace("或 ", " or "),
    };
    format!("Expected {what}")
}

pub fn syntax_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    base_db::parse(db, file)
        .errors()
        .iter()
        .map(|err| Diagnostic {
            range: err.range,
            message: syntax_error_message(&err.message, base_db::lang()),
            severity: Severity::Error,
            code: DiagnosticCode::SyntaxError,
            unused: false,
        })
        .collect()
}

/// 文件里所有可推断的体。
pub fn bodies_in_file(db: &dyn Db, file: SourceFile) -> Vec<BodyId> {
    // 同一个文件的诊断要调用很多次，结果缓存在 salsa 里
    bodies_tracked(db, file).as_ref().clone()
}

#[salsa::tracked]
fn bodies_tracked(db: &dyn Db, file: SourceFile) -> std::sync::Arc<Vec<BodyId>> {
    std::sync::Arc::new(compute_bodies_in_file(db, file))
}

fn compute_bodies_in_file(db: &dyn Db, file: SourceFile) -> Vec<BodyId> {
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
                    base_db::tr!("无法解析符号 `{name}`", "Cannot resolve symbol `{name}`"),
                    DiagnosticCode::UnresolvedName,
                ),
                InferDiagnosticKind::UnresolvedField { name, receiver } => (
                    base_db::tr!(
                        "类型 `{receiver}` 上没有字段 `{name}`",
                        "No field `{name}` on type `{receiver}`"
                    ),
                    DiagnosticCode::UnresolvedField,
                ),
                InferDiagnosticKind::UnresolvedMethod { name, receiver } => (
                    match receiver {
                        Some(receiver) => base_db::tr!(
                            "类型 `{receiver}` 上没有方法 `{name}`",
                            "No method `{name}` on type `{receiver}`"
                        ),
                        None => {
                            base_db::tr!("无法解析方法 `{name}`", "Cannot resolve method `{name}`")
                        }
                    },
                    DiagnosticCode::UnresolvedMethod,
                ),
                InferDiagnosticKind::TypeMismatch { expected, actual } => (
                    base_db::tr!(
                        "类型不匹配：需要 `{expected}`，实际是 `{actual}`",
                        "Type mismatch: expected `{expected}`, found `{actual}`"
                    ),
                    DiagnosticCode::TypeMismatch,
                ),
                InferDiagnosticKind::ArgumentMismatch { name, args } => (
                    base_db::tr!(
                        "`{name}` 没有可以接受实参 ({args}) 的重载",
                        "No overload of `{name}` accepts the arguments ({args})"
                    ),
                    DiagnosticCode::ArgumentMismatch,
                ),
                InferDiagnosticKind::BadOperand { op, operands } => (
                    base_db::tr!(
                        "运算符 `{op}` 不能用于 {operands}",
                        "Operator `{op}` cannot be applied to {operands}"
                    ),
                    DiagnosticCode::BadOperand,
                ),
                InferDiagnosticKind::DuplicateLocal { name } => (
                    base_db::tr!(
                        "变量 `{name}` 已经在这个方法里定义过",
                        "Variable `{name}` is already defined in this method"
                    ),
                    DiagnosticCode::DuplicateVariable,
                ),
                InferDiagnosticKind::AmbiguousCall {
                    name,
                    first,
                    second,
                } => (
                    base_db::tr!(
                        "对 `{name}` 的调用不明确：`{first}` 和 `{second}` 都匹配",
                        "Reference to `{name}` is ambiguous: both `{first}` and `{second}` match"
                    ),
                    DiagnosticCode::AmbiguousCall,
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
    let graph = hir::module_graph(db);
    let module_imports_ok = tree
        .module_imports
        .iter()
        .all(|module| graph.get(module).is_some());
    if !on_demand_ok || !module_imports_ok {
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
        // 几个模块导入带进了同名的类
        let ambiguous = hir::module_import_candidates(db, file, &name);
        let message = if ambiguous.len() > 1 {
            let names: Vec<String> = ambiguous
                .iter()
                .map(|class| class.binary_name(db).replace(['/', '$'], "."))
                .collect();
            base_db::tr!(
                "类型 `{name}` 不明确：模块导入同时带进了 {}",
                "Type `{name}` is ambiguous: module imports bring in {}",
                names.join(", ")
            )
        } else {
            base_db::tr!("无法解析类型 `{name}`", "Cannot resolve type `{name}`")
        };
        out.push(Diagnostic {
            range: name_ref.syntax().text_range(),
            message,
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

    let graph = hir::module_graph(db);
    for import in cu.imports() {
        let Some(name) = import.name() else { continue };
        // 模块导入：模块不存在时报错（类路径上一个模块也没有时说明没找到 JDK，不报）；不提示"未使用"
        if import.is_module() {
            if graph.get(&name.text()).is_none() && !graph.is_empty() {
                out.push(Diagnostic {
                    range: name.syntax().text_range(),
                    message: base_db::tr!("找不到模块 `{}`", "Module `{}` not found", name.text()),
                    severity: Severity::Error,
                    code: DiagnosticCode::UnresolvedImport,
                    unused: false,
                });
            }
            continue;
        }
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
            resolve_qualified_class(db, &segments)
                .is_some_and(|class| hir::is_visible_from(db, Some(file), class))
        };
        if !resolved {
            out.push(Diagnostic {
                range: name.syntax().text_range(),
                message: base_db::tr!(
                    "无法解析 import `{}`",
                    "Cannot resolve import `{}`",
                    name.text()
                ),
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
                    base_db::tr!("重复的 import `{}`", "Duplicate import `{}`", name.text())
                } else {
                    base_db::tr!("未使用的 import `{}`", "Unused import `{}`", name.text())
                },
                severity: Severity::Hint,
                code: DiagnosticCode::UnusedImport,
                unused: true,
            });
        }
    }
}

/// 可以简化的写法（快速修复用）。
pub fn simplification_diagnostics(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut res = Vec::new();
    inspections::simplifications(db, file, &mut res);
    field_final::field_may_be_final(db, file, &mut res);
    redundant_modifiers::redundant_modifiers(&base_db::parse(db, file).syntax_node(), &mut res);
    jumps::redundant_jumps(&base_db::parse(db, file).syntax_node(), &mut res);
    res
}

/// 未使用的局部变量和私有成员，以及从未被读到的赋值（快速修复用）。
pub fn unused_symbols(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    let mut res = Vec::new();
    dead_stores::dead_stores(db, file, &mut res);
    unused_locals(db, file, &mut res);
    unused_private_members(db, file, &mut res);
    res
}

/// 缺少 `return` 的方法（诊断的范围）。语法有错时不判断。
pub fn missing_returns(db: &dyn Db, file: SourceFile) -> Vec<TextRange> {
    if !syntax_diagnostics(db, file).is_empty() {
        return Vec::new();
    }
    let mut res = Vec::new();
    flow::flow_diagnostics(&base_db::parse(db, file).syntax_node(), &mut res);
    res.into_iter()
        .filter(|d| d.code == DiagnosticCode::MissingReturn)
        .map(|d| d.range)
        .collect()
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
            // `ignored` / `unused` 是"有意不用"的约定名字
            let intentionally_unused = local.name.starts_with('_')
                || matches!(&*local.name, "ignored" | "ignore" | "unused");
            if used || !reportable || intentionally_unused {
                continue;
            }
            out.push(Diagnostic {
                range: local.decl_range,
                message: base_db::tr!(
                    "局部变量 `{}` 从未被使用",
                    "Local variable `{}` is never used",
                    local.name
                ),
                severity: Severity::Hint,
                code: DiagnosticCode::UnusedLocal,
                unused: true,
            });
        }
    }
}

/// 私有方法里从没用到的形参。私有方法不参与覆写，形参可以放心删掉；带注解的方法可能是
/// 框架的入口、被方法引用的方法签名受函数式接口约束、序列化方法的签名是固定的，都不提示。
fn unused_parameters(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    const SERIALIZATION: &[&str] = &[
        "readObject",
        "writeObject",
        "readObjectNoData",
        "readResolve",
        "writeReplace",
    ];
    let text: &str = file.text(db);
    for body in bodies_in_file(db, file) {
        let hir::BodyId::Method(method) = body else {
            continue;
        };
        let Some(data) = hir::class_data(db, method.class) else {
            continue;
        };
        let Some(method_data) = data.methods.get(method.idx as usize) else {
            continue;
        };
        if method_data.is_constructor
            || method_data.flags.visibility() != hir::Visibility::Private
            || !method_data.has_body
            || method_data.params.is_empty()
            || SERIALIZATION.contains(&&*method_data.name)
            || text.contains(&format!("::{}", method_data.name))
        {
            continue;
        }
        let Some((_, node)) = hir::body_syntax(db, body) else {
            continue;
        };
        let Some(decl) = ast::MethodDecl::cast(node) else {
            continue;
        };
        if decl
            .modifier_list()
            .is_some_and(|m| m.annotations().next().is_some())
        {
            continue;
        }
        let Some(params) = decl.param_list() else {
            continue;
        };
        let params_range = params.syntax().text_range();
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
            let intentionally_unused = local.name.starts_with('_')
                || matches!(&*local.name, "ignored" | "ignore" | "unused");
            if used
                || local.kind != hir::LocalKind::Param
                || !params_range.contains_range(local.decl_range)
                || intentionally_unused
            {
                continue;
            }
            out.push(Diagnostic {
                range: local.decl_range,
                message: base_db::tr!(
                    "形参 `{}` 从未被使用",
                    "Parameter `{}` is never used",
                    local.name
                ),
                severity: Severity::Hint,
                code: DiagnosticCode::UnusedParameter,
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
        // 局部类（及其成员类）的成员在外层的体里就地推断，类体里的引用解析成局部符号，
        // 按成员查引用会漏掉
        let in_local_class = class_data.flags.contains(hir::Modifiers::LOCAL_CLASS)
            || class_data
                .binary_name
                .split('$')
                .skip(1)
                .any(|part| part.starts_with(|c: char| c.is_ascii_digit()));
        if in_local_class {
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
                candidates.push((def, field.name.as_str(), base_db::trs!("字段", "field")));
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
                candidates.push((def, method.name.as_str(), base_db::trs!("方法", "method")));
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
                    message: base_db::tr!(
                        "私有{what} `{name}` 从未被使用",
                        "Private {what} `{name}` is never used"
                    ),
                    severity: Severity::Hint,
                    code: DiagnosticCode::UnusedPrivateMember,
                    unused: true,
                });
            }
        }
    }
}

/// `module-info.java`：`requires` 的模块必须存在，`exports` / `opens` 的包必须在工作区里有类。
fn module_info_diagnostics(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    if hir::item_tree(db, file).module.is_none() {
        return;
    }
    let cu = base_db::parse(db, file).tree();
    let Some(body) = cu.module().and_then(|m| m.body()) else {
        return;
    };
    let graph = hir::module_graph(db);
    if !graph.is_empty() {
        for requires in body.requires() {
            let Some(name) = requires.name() else {
                continue;
            };
            if graph.get(&name.text()).is_none() {
                out.push(Diagnostic {
                    range: name.syntax().text_range(),
                    message: base_db::tr!("找不到模块 `{}`", "Module `{}` not found", name.text()),
                    severity: Severity::Error,
                    code: DiagnosticCode::UnresolvedModule,
                    unused: false,
                });
            }
        }
    }
    let index = hir::source_class_index(db);
    let packages = body
        .exports()
        .filter_map(|e| e.name())
        .chain(body.opens().filter_map(|o| o.name()));
    for name in packages {
        let package = name.segments().join("/");
        if hir::package_classes(db, &package)
            .iter()
            .any(|class| index.get(&format!("{package}/{class}")).is_some())
        {
            continue;
        }
        out.push(Diagnostic {
            range: name.syntax().text_range(),
            message: base_db::tr!(
                "包 `{}` 是空的或者不存在",
                "Package `{}` is empty or does not exist",
                name.text()
            ),
            severity: Severity::Error,
            code: DiagnosticCode::UnresolvedModule,
            unused: false,
        });
    }
}

/// 紧凑源文件：不能声明包，必须有可以启动的 `main`（`void main()` 或 `void main(String[])`）。
fn compact_source_diagnostics(db: &dyn Db, file: SourceFile, out: &mut Vec<Diagnostic>) {
    let sema = Semantics::new(db);
    let Some(class) = sema.implicit_class(file) else {
        return;
    };
    let cu = base_db::parse(db, file).tree();
    if let Some(package) = cu.package() {
        out.push(Diagnostic {
            range: package.syntax().text_range(),
            message: base_db::trs!(
                "紧凑源文件不能声明包",
                "A compact source file cannot have a package declaration"
            )
            .to_owned(),
            severity: Severity::Error,
            code: DiagnosticCode::CompactSourceFile,
            unused: false,
        });
    }
    let Some(data) = hir::class_data(db, class) else {
        return;
    };
    let launchable = data.methods.iter().enumerate().any(|(idx, method)| {
        if method.is_constructor
            || method.name != "main"
            || method.flags.visibility() == hir::Visibility::Private
        {
            return false;
        }
        let sig = hir::method_sig(
            db,
            hir::MethodId {
                class,
                idx: idx as u32,
            },
        );
        sig.ret == hir::Ty::Void
            && match sig.params.as_slice() {
                [] => true,
                [hir::Ty::Array(element)] => element
                    .as_class()
                    .is_some_and(|c| c.binary_name(db) == "java/lang/String"),
                _ => false,
            }
    });
    if launchable {
        return;
    }
    // 报在第一个顶层方法 / 字段的名字上
    let anchor = cu
        .syntax()
        .children()
        .filter(|n| matches!(n.kind(), SyntaxKind::METHOD_DECL | SyntaxKind::FIELD_DECL))
        .find_map(|n| {
            n.descendants()
                .find(|d| d.kind() == SyntaxKind::NAME)
                .map(|name| name.text_range())
        });
    if let Some(range) = anchor {
        out.push(Diagnostic {
            range,
            message: base_db::trs!(
                "紧凑源文件没有可以启动的 `main` 方法（`void main()` 或 `void main(String[] args)`）",
                "The compact source file has no launchable `main` method (`void main()` or `void main(String[] args)`)"
            )
            .to_owned(),
            severity: Severity::Error,
            code: DiagnosticCode::CompactSourceFile,
            unused: false,
        });
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

    if config.semantic && is_workspace_file {
        language_level::feature_diagnostics(db, file, &mut res);
    }
    if config.semantic && is_workspace_file && has_jdk {
        infer_diagnostics(db, file, &mut res);
        unresolved_types(db, file, &mut res);
        if syntax_ok {
            typeck::typeck_diagnostics(db, file, &mut res);
        }
        import_diagnostics(db, file, config, &mut res);
        module_info_diagnostics(db, file, &mut res);
        compact_source_diagnostics(db, file, &mut res);
        // 残缺的语法树上做流分析只会得到误报
        if config.inspections && syntax_ok {
            timing::timed("flow::flow_diagnostics", || {
                flow::flow_diagnostics(&base_db::parse(db, file).syntax_node(), &mut res)
            });
            timing::timed("flow::expression_inspections", || {
                flow::expression_inspections(db, file, &mut res)
            });
            timing::timed("exceptions::exception_diagnostics", || {
                exceptions::exception_diagnostics(db, file, &mut res)
            });
            timing::timed("nullness::nullness_diagnostics", || {
                nullness::nullness_diagnostics(db, file, &mut res)
            });
            timing::timed("inspections::deprecated_usages", || {
                inspections::deprecated_usages(db, file, &mut res)
            });
            timing::timed("inspections::simplifications", || {
                inspections::simplifications(db, file, &mut res)
            });
            timing::timed("dead_stores::dead_stores", || {
                dead_stores::dead_stores(db, file, &mut res)
            });
            timing::timed("field_final::field_may_be_final", || {
                field_final::field_may_be_final(db, file, &mut res)
            });
            timing::timed("format_strings::format_string_diagnostics", || {
                format_strings::format_string_diagnostics(db, file, &mut res)
            });
            timing::timed("suspicious_calls::suspicious_calls", || {
                suspicious_calls::suspicious_calls(db, file, &mut res)
            });
            timing::timed("ignored_results::ignored_results", || {
                ignored_results::ignored_results(db, file, &mut res)
            });
            timing::timed("arrays::array_diagnostics", || {
                arrays::array_diagnostics(db, file, &mut res)
            });
            timing::timed("loops::loop_diagnostics", || {
                loops::loop_diagnostics(db, file, &mut res)
            });
            timing::timed("recursion::infinite_recursion", || {
                recursion::infinite_recursion(db, file, &mut res)
            });
            timing::timed("collections::mismatched_collections", || {
                collections::mismatched_collections(db, file, &mut res)
            });
            timing::timed("collections::mismatched_collection_fields", || {
                collections::mismatched_collection_fields(db, file, &mut res)
            });
            timing::timed("probable_bugs::probable_bugs", || {
                probable_bugs::probable_bugs(db, file, &mut res)
            });
            timing::timed("may_be_static::methods_may_be_static", || {
                may_be_static::methods_may_be_static(db, file, &mut res)
            });
            timing::timed("lambdas::lambdas_to_method_refs", || {
                lambdas::lambdas_to_method_refs(db, file, &mut res)
            });
            timing::timed("optionals::unchecked_optional_gets", || {
                optionals::unchecked_optional_gets(db, file, &mut res)
            });
            timing::timed("field_local::field_can_be_local", || {
                field_local::field_can_be_local(db, file, &mut res)
            });
            timing::timed("exceptions::redundant_throws_diagnostics", || {
                exceptions::redundant_throws_diagnostics(db, file, &mut res)
            });
            timing::timed("boxing::unnecessary_boxing", || {
                boxing::unnecessary_boxing(db, file, &mut res)
            });
            timing::timed("diamond::redundant_diamond_args", || {
                diamond::redundant_diamond_args(db, file, &mut res)
            });
            timing::timed("redundancy::redundancies", || {
                redundancy::redundancies(db, file, &mut res)
            });
            timing::timed("redundant_modifiers::redundant_modifiers", || {
                redundant_modifiers::redundant_modifiers(
                    &base_db::parse(db, file).syntax_node(),
                    &mut res,
                )
            });
            timing::timed("jumps::redundant_jumps", || {
                jumps::redundant_jumps(&base_db::parse(db, file).syntax_node(), &mut res)
            });
        }
        if config.unused_symbols {
            unused_locals(db, file, &mut res);
            unused_parameters(db, file, &mut res);
            unused_private_members(db, file, &mut res);
        }
    }
    suppress::remove_suppressed(&base_db::parse(db, file).syntax_node(), &mut res);
    res.sort_by_key(|d| (d.range.start(), d.range.end()));
    res.dedup();
    res
}

#[cfg(test)]
mod tests {
    #[test]
    fn syntax_errors_follow_language() {
        use base_db::Lang::{En, Zh};
        let msg = super::syntax_error_message;
        assert_eq!(msg("此处应为成员名", En), "Expected a member name");
        assert_eq!(msg("此处应为 `;` 或 `}`", En), "Expected `;` or `}`");
        assert_eq!(msg("此处应为 标识符", En), "Expected identifier");
        assert_eq!(
            msg("此处应为标识符或 `*`", En),
            "Expected identifier or `*`"
        );
        assert_eq!(msg("文本块未闭合", En), "Unterminated text block");
        assert_eq!(msg("此处应为成员名", Zh), "此处应为成员名");
    }

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
