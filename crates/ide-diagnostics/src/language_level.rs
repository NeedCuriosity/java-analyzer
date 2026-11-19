//! 按模块的语言级别和预览设置检查语法特性：语言级别 6 的模块里不能写 lambda、try-with-resources，
//! 语言级别 8 的模块里不能写 record、`var`、文本块……
//!
//! 只看语法，在出现该特性的第一个 token 上报 `feature-not-available`。语言级别未知时按 JDK 的版本算。
//! 预览特性要求模块开启了预览，并且语言级别等于 JDK 版本（javac 的要求）。

use base_db::{workspace, Db, FileKind, SourceFile, SourceRootKind};
use syntax::{
    ast::{self, AstNode},
    NodeOrToken, SyntaxKind, SyntaxNode, SyntaxToken, TextRange,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

/// 文件所在模块的语言设置。
struct Level {
    level: u32,
    preview: bool,
    jdk: Option<u32>,
    module: Option<String>,
}

/// 文件所在模块的语言级别（没配置时用 JDK 的版本）。
pub fn language_level(db: &dyn Db, file: SourceFile) -> Option<u32> {
    level_of(db, file).map(|l| l.level)
}

fn level_of(db: &dyn Db, file: SourceFile) -> Option<Level> {
    let ws = workspace(db);
    let project = ws.project(db);
    let jdk = project.jdk.as_ref().map(|jdk| jdk.version);
    match ws.files(db).get(file.file_id(db))?.kind {
        FileKind::Workspace {
            module: Some(module),
            root,
        } => {
            let data = project.modules.get(module.0 as usize)?;
            let level = match root {
                SourceRootKind::Test => data.test_language_level.or(data.language_level),
                SourceRootKind::Main | SourceRootKind::Generated => data.language_level,
            };
            Some(Level {
                level: level.or(jdk)?,
                preview: data.preview,
                jdk,
                module: Some(data.name.clone()),
            })
        }
        FileKind::Workspace { module: None, .. } => Some(Level {
            level: project.max_language_level().or(jdk)?,
            preview: false,
            jdk,
            module: None,
        }),
        FileKind::Library => None,
    }
}

/// 一个语法特性：名字（复数形式的说法）和最低语言级别；`None` 表示还是预览特性。
#[derive(Clone, Copy)]
struct Feature {
    zh: &'static str,
    en: &'static str,
    since: Option<u32>,
}

const DIAMOND: Feature = Feature {
    zh: "菱形语法 `<>`",
    en: "Diamond type arguments `<>`",
    since: Some(7),
};
const TRY_WITH_RESOURCES: Feature = Feature {
    zh: "try-with-resources",
    en: "try-with-resources statements",
    since: Some(7),
};
const MULTI_CATCH: Feature = Feature {
    zh: "一个 catch 捕获多个异常",
    en: "Multi-catch clauses",
    since: Some(7),
};
const STRING_SWITCH: Feature = Feature {
    zh: "对字符串的 switch",
    en: "Strings in switch",
    since: Some(7),
};
const BINARY_LITERALS: Feature = Feature {
    zh: "二进制字面量",
    en: "Binary literals",
    since: Some(7),
};
const LITERAL_UNDERSCORES: Feature = Feature {
    zh: "数字字面量里的下划线",
    en: "Underscores in numeric literals",
    since: Some(7),
};
const LAMBDAS: Feature = Feature {
    zh: "lambda 表达式",
    en: "Lambda expressions",
    since: Some(8),
};
const METHOD_REFS: Feature = Feature {
    zh: "方法引用",
    en: "Method references",
    since: Some(8),
};
const DEFAULT_METHODS: Feature = Feature {
    zh: "接口的默认方法",
    en: "Default interface methods",
    since: Some(8),
};
const STATIC_INTERFACE_METHODS: Feature = Feature {
    zh: "接口的静态方法",
    en: "Static interface methods",
    since: Some(8),
};
const PRIVATE_INTERFACE_METHODS: Feature = Feature {
    zh: "接口的私有方法",
    en: "Private interface methods",
    since: Some(9),
};
const VAR: Feature = Feature {
    zh: "局部变量的 `var`",
    en: "`var` for local variables",
    since: Some(10),
};
const VAR_LAMBDA: Feature = Feature {
    zh: "lambda 形参的 `var`",
    en: "`var` in lambda parameters",
    since: Some(11),
};
const SWITCH_EXPRESSIONS: Feature = Feature {
    zh: "switch 表达式",
    en: "Switch expressions",
    since: Some(14),
};
const ARROW_CASES: Feature = Feature {
    zh: "`->` 形式的 case",
    en: "`->` case labels",
    since: Some(14),
};
const YIELD: Feature = Feature {
    zh: "`yield` 语句",
    en: "`yield` statements",
    since: Some(14),
};
const TEXT_BLOCKS: Feature = Feature {
    zh: "文本块",
    en: "Text blocks",
    since: Some(15),
};
const RECORDS: Feature = Feature {
    zh: "record",
    en: "Records",
    since: Some(16),
};
const INSTANCEOF_PATTERNS: Feature = Feature {
    zh: "`instanceof` 的类型模式",
    en: "`instanceof` type patterns",
    since: Some(16),
};
const SEALED: Feature = Feature {
    zh: "sealed 类型",
    en: "Sealed types",
    since: Some(17),
};
const SWITCH_PATTERNS: Feature = Feature {
    zh: "switch 里的模式",
    en: "Patterns in switch",
    since: Some(21),
};
const RECORD_PATTERNS: Feature = Feature {
    zh: "record 模式",
    en: "Record patterns",
    since: Some(21),
};
const CASE_NULL: Feature = Feature {
    zh: "`case null`",
    en: "`case null`",
    since: Some(21),
};
const GUARDS: Feature = Feature {
    zh: "`when` 守卫",
    en: "`when` guards",
    since: Some(21),
};
const UNNAMED: Feature = Feature {
    zh: "无名变量和模式 `_`",
    en: "Unnamed variables and patterns `_`",
    since: Some(22),
};
const MODULE_IMPORTS: Feature = Feature {
    zh: "模块导入",
    en: "Module imports",
    since: Some(25),
};
const COMPACT_SOURCE_FILES: Feature = Feature {
    zh: "紧凑源文件",
    en: "Compact source files",
    since: Some(25),
};
const STATEMENTS_BEFORE_SUPER: Feature = Feature {
    zh: "`super(...)` / `this(...)` 之前的语句",
    en: "Statements before `super(...)` / `this(...)`",
    since: Some(25),
};
const PRIMITIVE_PATTERNS: Feature = Feature {
    zh: "模式、`instanceof` 和 switch 里的基本类型",
    en: "Primitive types in patterns, `instanceof` and switch",
    since: None,
};

struct Checker<'a> {
    level: &'a Level,
    out: &'a mut Vec<Diagnostic>,
}

impl Checker<'_> {
    fn check(&mut self, feature: Feature, range: TextRange) {
        let current = self.level;
        let name = match base_db::lang() {
            base_db::Lang::Zh => feature.zh,
            base_db::Lang::En => feature.en,
        };
        let level = current.level;
        let message = match feature.since {
            Some(since) if level >= since => return,
            Some(since) => match &current.module {
                Some(module) => base_db::tr!(
                    "{name}需要 Java {since}（模块 `{module}` 的语言级别是 {level}）",
                    "{name} require Java {since} (module `{module}` is at {level})",
                ),
                None => base_db::tr!(
                    "{name}需要 Java {since}（当前的语言级别是 {level}）",
                    "{name} require Java {since} (the language level is {level})",
                ),
            },
            None => {
                if current.preview && Some(level) == current.jdk {
                    return;
                }
                base_db::tr!(
                    "{name}是预览特性：需要开启预览（--enable-preview），并且语言级别等于 JDK 版本",
                    "{name} are a preview feature: enable preview (--enable-preview) and use the JDK's language level",
                )
            }
        };
        self.out.push(Diagnostic {
            range,
            message,
            severity: Severity::Error,
            code: DiagnosticCode::FeatureNotAvailable,
            unused: false,
        });
    }
}

fn first_token(node: &SyntaxNode) -> TextRange {
    node.descendants_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| !t.kind().is_trivia())
        .map_or_else(|| node.text_range(), |t| t.text_range())
}

fn token_of(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == kind)
}

/// `var` 写在哪里：局部变量 / 资源 / 增强 for 的变量，还是 lambda 形参。
fn var_feature(class_ty: &SyntaxNode) -> Option<Feature> {
    if class_ty.text() != "var" {
        return None;
    }
    let parent = class_ty.parent()?;
    match parent.kind() {
        SyntaxKind::LOCAL_VAR_DECL | SyntaxKind::RESOURCE => Some(VAR),
        SyntaxKind::PARAM => {
            let owner = parent.parent()?;
            if owner.kind() == SyntaxKind::FOREACH_STMT {
                Some(VAR)
            } else if owner.parent()?.kind() == SyntaxKind::LAMBDA_EXPR {
                Some(VAR_LAMBDA)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 名字 `_` 的声明（变量、形参、模式变量）。
fn is_underscore_decl(name: &SyntaxNode) -> bool {
    name.text() == "_"
        && name.parent().is_some_and(|p| {
            matches!(
                p.kind(),
                SyntaxKind::VARIABLE_DECLARATOR
                    | SyntaxKind::PARAM
                    | SyntaxKind::TYPE_PATTERN
                    | SyntaxKind::RESOURCE
            )
        })
}

/// `case "a":` 这样的字符串标签；一个 switch 只在第一个上报。
fn string_label(label: &SyntaxNode) -> Option<SyntaxNode> {
    label.children().find(|c| {
        c.kind() == SyntaxKind::LITERAL && token_of(c, SyntaxKind::STRING_LITERAL).is_some()
    })
}

fn first_string_label(label: &SyntaxNode) -> Option<TextRange> {
    let literal = string_label(label)?;
    let case = label.parent()?;
    let earlier = case
        .siblings(syntax::Direction::Prev)
        .skip(1)
        .filter_map(|c| c.children().find(|l| l.kind() == SyntaxKind::SWITCH_LABEL))
        .any(|l| string_label(&l).is_some());
    (!earlier).then(|| literal.text_range())
}

/// 构造器体里显式的 `super(...)` / `this(...)` 前面还有别的语句。
fn statement_before_constructor_call(block: &SyntaxNode) -> Option<TextRange> {
    let ctor = block.parent()?;
    if ctor.kind() != SyntaxKind::CONSTRUCTOR_DECL {
        return None;
    }
    let statements: Vec<SyntaxNode> = block
        .children()
        .filter(|n| ast::Stmt::can_cast(n.kind()))
        .collect();
    let call = statements.iter().position(|stmt| {
        stmt.children()
            .any(|c| c.kind() == SyntaxKind::EXPLICIT_CONSTRUCTOR_CALL)
    })?;
    (call > 0).then(|| first_token(&statements[0]))
}

pub(crate) fn feature_diagnostics(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let Some(level) = level_of(view.db, view.file) else {
        return;
    };
    let root = &view.root;
    let mut checker = Checker { level: &level, out };

    // 文件级的写法（模块导入、紧凑源文件）不在任何体里
    let cu = (!view.is_body())
        .then(|| ast::CompilationUnit::cast(root.clone()))
        .flatten();
    if let Some(cu) = cu {
        for import in cu.imports().filter(|i| i.is_module()) {
            checker.check(MODULE_IMPORTS, import.syntax().text_range());
        }
        if hir::is_compact_source(&cu) {
            if let Some(member) = root
                .children()
                .find(|n| matches!(n.kind(), SyntaxKind::METHOD_DECL | SyntaxKind::FIELD_DECL))
            {
                checker.check(COMPACT_SOURCE_FILES, first_token(&member));
            }
        }
    }

    for element in view.elements() {
        let node = match element {
            NodeOrToken::Token(token) => {
                match token.kind() {
                    SyntaxKind::INT_LITERAL
                    | SyntaxKind::LONG_LITERAL
                    | SyntaxKind::FLOAT_LITERAL
                    | SyntaxKind::DOUBLE_LITERAL => {
                        let text = token.text();
                        if text.starts_with("0b") || text.starts_with("0B") {
                            checker.check(BINARY_LITERALS, token.text_range());
                        }
                        if text.contains('_') {
                            checker.check(LITERAL_UNDERSCORES, token.text_range());
                        }
                    }
                    SyntaxKind::TEXT_BLOCK => checker.check(TEXT_BLOCKS, token.text_range()),
                    SyntaxKind::SEALED_KW | SyntaxKind::NON_SEALED_KW => {
                        checker.check(SEALED, token.text_range())
                    }
                    _ => {}
                }
                continue;
            }
            NodeOrToken::Node(node) => node,
        };
        match node.kind() {
            SyntaxKind::METHOD_DECL => {
                let in_interface = node
                    .parent()
                    .and_then(|body| body.parent())
                    .is_some_and(|decl| decl.kind() == SyntaxKind::INTERFACE_DECL);
                let modifier = |kind| {
                    node.children()
                        .find(|c| c.kind() == SyntaxKind::MODIFIER_LIST)
                        .and_then(|mods| token_of(&mods, kind))
                };
                if in_interface {
                    if let Some(private) = modifier(SyntaxKind::PRIVATE_KW) {
                        checker.check(PRIVATE_INTERFACE_METHODS, private.text_range());
                    }
                    if let Some(default) = modifier(SyntaxKind::DEFAULT_KW) {
                        checker.check(DEFAULT_METHODS, default.text_range());
                    }
                    if let Some(stat) = modifier(SyntaxKind::STATIC_KW) {
                        checker.check(STATIC_INTERFACE_METHODS, stat.text_range());
                    }
                }
            }
            SyntaxKind::TYPE_ARG_LIST if node.children().next().is_none() => {
                checker.check(DIAMOND, node.text_range())
            }
            SyntaxKind::RESOURCE_LIST => checker.check(TRY_WITH_RESOURCES, first_token(&node)),
            SyntaxKind::UNION_TYPE => {
                if let Some(pipe) = token_of(&node, SyntaxKind::PIPE) {
                    checker.check(MULTI_CATCH, pipe.text_range());
                }
            }
            SyntaxKind::LAMBDA_EXPR => {
                let arrow = token_of(&node, SyntaxKind::ARROW);
                checker.check(
                    LAMBDAS,
                    arrow.map_or_else(|| first_token(&node), |t| t.text_range()),
                );
            }
            SyntaxKind::METHOD_REF => {
                let colons = token_of(&node, SyntaxKind::COLON_COLON);
                checker.check(
                    METHOD_REFS,
                    colons.map_or_else(|| node.text_range(), |t| t.text_range()),
                );
            }
            SyntaxKind::CLASS_TYPE => {
                if let Some(feature) = var_feature(&node) {
                    checker.check(feature, node.text_range());
                }
            }
            SyntaxKind::SWITCH_EXPR => checker.check(SWITCH_EXPRESSIONS, first_token(&node)),
            SyntaxKind::SWITCH_CASE => {
                if let Some(arrow) = token_of(&node, SyntaxKind::ARROW) {
                    checker.check(ARROW_CASES, arrow.text_range());
                }
            }
            SyntaxKind::SWITCH_LABEL => {
                if let Some(range) = first_string_label(&node) {
                    checker.check(STRING_SWITCH, range);
                }
                let label = node.clone();
                for child in label.children() {
                    match child.kind() {
                        SyntaxKind::TYPE_PATTERN | SyntaxKind::RECORD_PATTERN => {
                            checker.check(SWITCH_PATTERNS, child.text_range());
                            if child.kind() == SyntaxKind::TYPE_PATTERN
                                && child
                                    .children()
                                    .any(|c| c.kind() == SyntaxKind::PRIMITIVE_TYPE)
                            {
                                checker.check(PRIMITIVE_PATTERNS, child.text_range());
                            }
                        }
                        SyntaxKind::LITERAL if token_of(&child, SyntaxKind::NULL_KW).is_some() => {
                            checker.check(CASE_NULL, child.text_range());
                        }
                        _ => {}
                    }
                }
            }
            SyntaxKind::GUARD => checker.check(GUARDS, first_token(&node)),
            SyntaxKind::YIELD_STMT => checker.check(YIELD, first_token(&node)),
            SyntaxKind::RECORD_DECL => {
                let keyword = node
                    .children_with_tokens()
                    .filter_map(NodeOrToken::into_token)
                    .find(|t| t.text() == "record")
                    .map_or_else(|| first_token(&node), |t| t.text_range());
                checker.check(RECORDS, keyword);
            }
            SyntaxKind::INSTANCEOF_EXPR => {
                for child in node.children() {
                    match child.kind() {
                        SyntaxKind::TYPE_PATTERN => {
                            checker.check(INSTANCEOF_PATTERNS, child.text_range());
                            if child
                                .children()
                                .any(|c| c.kind() == SyntaxKind::PRIMITIVE_TYPE)
                            {
                                checker.check(PRIMITIVE_PATTERNS, child.text_range());
                            }
                        }
                        SyntaxKind::PRIMITIVE_TYPE => {
                            checker.check(PRIMITIVE_PATTERNS, child.text_range())
                        }
                        _ => {}
                    }
                }
            }
            SyntaxKind::RECORD_PATTERN => checker.check(RECORD_PATTERNS, node.text_range()),
            SyntaxKind::PERMITS_CLAUSE => checker.check(SEALED, first_token(&node)),
            SyntaxKind::UNNAMED_PATTERN => checker.check(UNNAMED, node.text_range()),
            // `_` 作为名字：8 及以前是合法的标识符（javac 只警告），9 起是关键字，22 起是无名变量
            SyntaxKind::NAME if level.level >= 9 && is_underscore_decl(&node) => {
                checker.check(UNNAMED, node.text_range())
            }
            SyntaxKind::BLOCK => {
                if let Some(range) = statement_before_constructor_call(&node) {
                    checker.check(STATEMENTS_BEFORE_SUPER, range);
                }
            }
            _ => {}
        }
    }
}
