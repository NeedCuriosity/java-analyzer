//! 压制警告：`@SuppressWarnings("unused")` 标在类、方法、字段、局部变量、形参上，
//! 或者语句 / 成员前面一行写 `//noinspection unused`。
//!
//! 只压制警告和提示，编译错误压制不了。每种诊断接受几个名字：javac 的（`unused`、
//! `deprecation`）、常见检查工具的（`DataFlowIssue`、`NullAway` …），以及诊断代码本身；
//! `all` 压制所有警告。

use syntax::{ast, ast::AstNode, SyntaxKind, SyntaxNode, TextRange};

use crate::{Diagnostic, DiagnosticCode, Severity};

/// 能压制这种诊断的名字（不含 `all` 和诊断代码本身）。第一个是加压制时用的名字。
fn keys(code: DiagnosticCode) -> &'static [&'static str] {
    use DiagnosticCode::*;
    match code {
        UnusedLocal | UnusedPrivateMember | UnusedParameter => {
            &["unused", "UnusedDeclaration", "UNUSED_SYMBOL"]
        }
        UnusedImport => &["UnusedImport", "unused"],
        UnusedAssignment => &["UnusedAssignment", "unused", "UNUSED_VALUE"],
        DeprecatedUsage => &["deprecation", "removal"],
        NullDereference | NullableDereference | ConstantCondition | NullabilityViolation => &[
            "DataFlowIssue",
            "ConstantConditions",
            "ConstantValue",
            "NullableProblems",
            "NullAway",
            "nullness",
            "null",
        ],
        StringEquality => &["StringEquality"],
        SelfAssignment => &["SillyAssignment"],
        SelfComparison => &["EqualsWithItself"],
        PointlessBoolean => &["PointlessBooleanExpression"],
        UseIsEmpty => &["SizeReplaceableByIsEmpty"],
        RedundantIf => &["RedundantIfStatement", "SimplifiableIfStatement"],
        RedundantCast => &["RedundantCast", "cast"],
        FieldMayBeFinal => &["FieldMayBeFinal"],
        RedundantModifier => &["RedundantModifier"],
        RedundantLocal => &["UnnecessaryLocalVariable"],
        MalformedFormatString => &["MalformedFormatString"],
        SuspiciousCall => &[
            "SuspiciousMethodCalls",
            "EqualsBetweenInconvertibleTypes",
            "CollectionIncompatibleType",
            "EqualsIncompatibleType",
        ],
        IgnoredResult => &[
            "ResultOfMethodCallIgnored",
            "ReturnValueIgnored",
            "CheckReturnValue",
        ],
        ArrayEquals => &["ArrayEquals", "ArrayEquality"],
        ArrayToString => &["ImplicitArrayToString", "ArrayToString"],
        NumberEquality => &["NumberEquality"],
        LoopConditionNotUpdated => &["LoopConditionNotUpdatedInsideLoop", "InfiniteLoopStatement"],
        StringConcatInLoop => &["StringConcatenationInLoop", "StringConcatenationInLoops"],
        InfiniteRecursion => &["InfiniteRecursion"],
        RedundantJump => &["UnnecessaryReturnStatement", "UnnecessaryContinue"],
        MismatchedCollection => &["MismatchedQueryAndUpdateOfCollection"],
        IntegerDivision => &["IntegerDivisionInFloatingPointContext"],
        OctalLiteral => &["OctalInteger"],
        EqualsHashCode => &["EqualsAndHashcode", "EqualsHashCode"],
        ThreadRun => &["CallToThreadRun"],
        MethodMayBeStatic => &["MethodMayBeStatic"],
        LambdaToMethodRef => &["Convert2MethodRef"],
        OptionalGetWithoutCheck => &["OptionalGetWithoutIsPresent"],
        FieldCanBeLocal => &["FieldCanBeLocal"],
        RedundantThrows => &["RedundantThrows"],
        UnnecessaryBoxing => &["UnnecessaryBoxing", "UnnecessaryUnboxing"],
        RedundantTypeArguments => &["Convert2Diamond"],
        CStyleArray => &["CStyleArrayDeclaration"],
        RedundantToString => &["UnnecessaryToStringCall", "StringOperationCanBeSimplified"],
        RedundantStringConstructor => &["RedundantStringConstructorCall"],
        PatternVariable => &["PatternVariableCanBeUsed"],
        MapApi => &["Java8MapApi"],
        TryWithResources => &["TryFinallyCanBeTryWithResources"],
        EmptyBlock => &[
            "EmptyCatchBlock",
            "EmptyFinallyBlock",
            "CatchMayIgnoreException",
        ],
        EmptyStatementBody => &["EmptyStatementBody"],
        StreamApiChain => &["SimplifyStreamApiCallChains"],
        CollectionsSort => &["Java8ListSort"],
        IndexOfContains => &["IndexOfReplaceableByContains"],
        RedundantValueOf => &["UnnecessaryCallToStringValueOf"],
        RawType => &["rawtypes", "RawUseOfParameterizedType"],
        _ => &[],
    }
}

/// 给这种诊断加压制时用的名字；不能压制时为 `None`。
pub fn suppression_key(diagnostic: &Diagnostic) -> Option<&'static str> {
    if !suppressible(diagnostic) {
        return None;
    }
    Some(
        keys(diagnostic.code)
            .first()
            .copied()
            .unwrap_or(diagnostic.code.as_str()),
    )
}

fn suppressible(diagnostic: &Diagnostic) -> bool {
    diagnostic.severity != Severity::Error && diagnostic.code != DiagnosticCode::SyntaxError
}

fn accepts(code: DiagnosticCode, key: &str) -> bool {
    key.eq_ignore_ascii_case("all")
        || key == code.as_str()
        || keys(code).iter().any(|k| k.eq_ignore_ascii_case(key))
}

/// `@SuppressWarnings(...)` 里列出的名字。
fn annotation_keys(node: &SyntaxNode) -> Vec<String> {
    let Some(modifiers) = node.children().find_map(ast::ModifierList::cast) else {
        return Vec::new();
    };
    let mut res = Vec::new();
    for annotation in modifiers.annotations() {
        let is_suppress = annotation.name().is_some_and(|n| {
            let text = n.text();
            text == "SuppressWarnings" || text == "java.lang.SuppressWarnings"
        });
        let Some(args) = annotation.arg_list().filter(|_| is_suppress) else {
            continue;
        };
        for token in args
            .syntax()
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == SyntaxKind::STRING_LITERAL)
        {
            let text = token.text();
            res.push(text.trim_matches('"').to_owned());
        }
    }
    res
}

/// 节点前面紧挨着的 `//noinspection A,B` 注释里的名字。
fn noinspection_keys(node: &SyntaxNode) -> Vec<String> {
    let mut comments = Vec::new();
    // 前导注释可能在节点里面（开头的 trivia），也可能是前面的兄弟 token
    for element in node.children_with_tokens() {
        match element.into_token() {
            Some(token) if token.kind().is_trivia() => {
                if token.kind() == SyntaxKind::LINE_COMMENT {
                    comments.push(token.text().to_owned());
                }
            }
            _ => break,
        }
    }
    let mut prev = node.prev_sibling_or_token();
    while let Some(element) = prev {
        let Some(token) = element.as_token() else {
            break;
        };
        if !token.kind().is_trivia() {
            break;
        }
        if token.kind() == SyntaxKind::LINE_COMMENT {
            comments.push(token.text().to_owned());
        }
        prev = element.prev_sibling_or_token();
    }
    let mut res = Vec::new();
    for comment in comments {
        let Some(rest) = comment
            .strip_prefix("//")
            .map(str::trim_start)
            .and_then(|c| c.strip_prefix("noinspection"))
        else {
            continue;
        };
        res.extend(
            rest.split([',', ' ', '\t'])
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(str::to_owned),
        );
    }
    res
}

/// 可以在前面写 `//noinspection` 的节点：语句和成员声明。
fn takes_noinspection(node: &SyntaxNode) -> bool {
    ast::Stmt::can_cast(node.kind())
        || matches!(
            node.kind(),
            SyntaxKind::METHOD_DECL
                | SyntaxKind::CONSTRUCTOR_DECL
                | SyntaxKind::FIELD_DECL
                | SyntaxKind::CLASS_DECL
                | SyntaxKind::INTERFACE_DECL
                | SyntaxKind::ENUM_DECL
                | SyntaxKind::RECORD_DECL
                | SyntaxKind::IMPORT_DECL
        )
}

fn is_suppressed_at(root: &SyntaxNode, range: TextRange, code: DiagnosticCode) -> bool {
    let start = match root.covering_element(range) {
        syntax::NodeOrToken::Node(n) => n,
        syntax::NodeOrToken::Token(t) => match t.parent() {
            Some(p) => p,
            None => return false,
        },
    };
    for node in start.ancestors() {
        if annotation_keys(&node).iter().any(|k| accepts(code, k)) {
            return true;
        }
        if takes_noinspection(&node) && noinspection_keys(&node).iter().any(|k| accepts(code, k)) {
            return true;
        }
    }
    false
}

/// 去掉被压制的警告。
pub(crate) fn remove_suppressed(root: &SyntaxNode, diagnostics: &mut Vec<Diagnostic>) {
    // 文件里根本没写压制时不用逐条检查
    let text = root.text().to_string();
    if !text.contains("SuppressWarnings") && !text.contains("noinspection") {
        return;
    }
    diagnostics.retain(|d| !(suppressible(d) && is_suppressed_at(root, d.range, d.code)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_noinspection_comment() {
        let parse = ast::CompilationUnit::parse(
            "class A {\n    void f() {\n        //noinspection unused, DataFlowIssue\n        int x = 1;\n    }\n}\n",
        );
        let root = parse.syntax_node();
        let stmt = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::LOCAL_VAR_DECL)
            .unwrap();
        assert_eq!(noinspection_keys(&stmt), vec!["unused", "DataFlowIssue"]);
    }
}
