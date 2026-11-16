//! 以 javac 为标准答案的差分评测。
//!
//! 语料是 `evals/javac/` 下一批自包含、能通过编译的 Java 文件。对每个文件：
//!
//! 1. 原样交给 javac 和分析器：javac 接受的代码，分析器不能报任何错误级诊断（误报）。
//! 2. 用一组变异算子在语法树上制造单点错误（把名字改成未定义的、删掉 `return`、
//!    删掉初值、删掉 `throws` …），每个变体再交给两边。javac 的输出就是标准答案：
//!    - javac 报错的那一行，分析器有没有报（检出率）、类别对不对；
//!    - 分析器报了而 javac 在那一行没报的，记为"多报"。
//!      javac 在类型检查出错后不做流分析，所以这种变体里分析器的流分析诊断不计入。
//!
//! 变异不需要人工标注预期——变体到底错没错、错在哪，全由 javac 决定。
//! 分析器的诊断策略是保守的，所以检出率不会是 100%；这里要守住的是"不误报"，
//! 检出率用基线防止退步。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use anyhow::Context;
use ide::{DiagnosticsConfig, FileId, Severity};
use rayon::prelude::*;
use serde::Serialize;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::run::Env;

/// 两边诊断的共同分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Unresolved,
    TypeMismatch,
    ArgumentMismatch,
    Ambiguous,
    BadOperand,
    Duplicate,
    AssignToFinal,
    Access,
    StaticContext,
    MissingImplementation,
    InvalidOverride,
    NonExhaustive,
    Dominated,
    UnhandledException,
    NeverThrown,
    MissingReturn,
    Unreachable,
    Uninitialized,
    Syntax,
    /// javac 有、分析器不做的检查
    Other,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::Unresolved => "无法解析的符号",
            Category::TypeMismatch => "类型不匹配",
            Category::ArgumentMismatch => "实参不匹配",
            Category::Ambiguous => "调用不明确",
            Category::BadOperand => "运算符操作数",
            Category::Duplicate => "重复定义",
            Category::AssignToFinal => "给 final 赋值",
            Category::Access => "访问权限",
            Category::StaticContext => "静态上下文",
            Category::MissingImplementation => "未实现抽象方法",
            Category::InvalidOverride => "非法覆写",
            Category::NonExhaustive => "switch 未穷尽",
            Category::Dominated => "被支配的 case",
            Category::UnhandledException => "未处理的异常",
            Category::NeverThrown => "catch 了不会抛出的异常",
            Category::MissingReturn => "缺少 return",
            Category::Unreachable => "不可达语句",
            Category::Uninitialized => "变量未初始化",
            Category::Syntax => "语法错误",
            Category::Other => "其它（不支持）",
        }
    }

    /// 由 javac 的流分析阶段（`Flow`）报告的类别。
    fn is_flow(self) -> bool {
        matches!(
            self,
            Category::UnhandledException
                | Category::NeverThrown
                | Category::MissingReturn
                | Category::Unreachable
                | Category::Uninitialized
                | Category::NonExhaustive
        )
    }

    fn of_javac_key(key: &str) -> Category {
        if key.starts_with("cant.resolve")
            || key == "doesnt.exist"
            || key == "module.not.found"
            || key == "package.empty.or.not.found"
        {
            Category::Unresolved
        } else if key == "prob.found.req"
            || key == "incomparable.types"
            || key == "not.within.bounds"
        {
            Category::TypeMismatch
        } else if key.starts_with("cant.apply") {
            Category::ArgumentMismatch
        } else if key == "ref.ambiguous" {
            Category::Ambiguous
        } else if key.starts_with("operator.cant.be.applied") {
            Category::BadOperand
        } else if key == "already.defined" {
            Category::Duplicate
        } else if key == "cant.assign.val.to.var" {
            Category::AssignToFinal
        } else if key == "report.access" {
            Category::Access
        } else if key.starts_with("non-static.cant.be.ref") {
            Category::StaticContext
        } else if key == "does.not.override.abstract" {
            Category::MissingImplementation
        } else if key.starts_with("override.") {
            Category::InvalidOverride
        } else if key == "pattern.dominated"
            || key == "unconditional.pattern.and.default"
            || key == "duplicate.unconditional.pattern"
        {
            Category::Dominated
        } else if key.starts_with("not.exhaustive") {
            Category::NonExhaustive
        } else if key.starts_with("unreported.exception") {
            Category::UnhandledException
        } else if key == "missing.ret.stmt" {
            Category::MissingReturn
        } else if key == "unreachable.stmt" {
            Category::Unreachable
        } else if key == "except.never.thrown.in.try" {
            Category::NeverThrown
        } else if key == "var.might.not.have.been.initialized"
            || key == "var.not.initialized.in.default.constructor"
        {
            Category::Uninitialized
        } else if key.starts_with("expected")
            || key.starts_with("illegal.start")
            || key == "premature.eof"
        {
            Category::Syntax
        } else {
            Category::Other
        }
    }

    fn of_our_code(code: &str) -> Category {
        match code {
            "unresolved-name" | "unresolved-type" | "unresolved-field" | "unresolved-method"
            | "unresolved-import" | "unresolved-module" => Category::Unresolved,
            "type-mismatch" | "type-arg-out-of-bounds" => Category::TypeMismatch,
            "argument-mismatch" => Category::ArgumentMismatch,
            "ambiguous-call" => Category::Ambiguous,
            "bad-operand" => Category::BadOperand,
            "duplicate-variable" => Category::Duplicate,
            "assign-to-final" => Category::AssignToFinal,
            "inaccessible-member" => Category::Access,
            "static-context" => Category::StaticContext,
            "missing-implementation" => Category::MissingImplementation,
            "invalid-override" => Category::InvalidOverride,
            "non-exhaustive-switch" => Category::NonExhaustive,
            "dominated-case" => Category::Dominated,
            "unhandled-exception" => Category::UnhandledException,
            "exception-never-thrown" => Category::NeverThrown,
            "missing-return" => Category::MissingReturn,
            "unreachable-code" => Category::Unreachable,
            "uninitialized-variable" | "uninitialized-final-field" => Category::Uninitialized,
            "syntax-error" => Category::Syntax,
            _ => Category::Other,
        }
    }
}

/// 一条诊断：行号（从 1 开始）和分类。
type Finding = (usize, Category);

// ---------------------------------------------------------------------------
// 变异算子
// ---------------------------------------------------------------------------

struct Mutator {
    name: &'static str,
    /// 这个算子想制造的错误，仅用于报告里的说明
    intent: &'static str,
    sites: fn(&SyntaxNode, &str) -> Vec<(TextRange, String)>,
}

fn name_refs_where(root: &SyntaxNode, parent: SyntaxKind) -> impl Iterator<Item = SyntaxNode> + '_ {
    root.descendants()
        .filter(move |n| n.kind() == SyntaxKind::NAME_REF)
        .filter(move |n| n.parent().is_some_and(|p| p.kind() == parent))
}

fn undefined_name(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    name_refs_where(root, SyntaxKind::NAME_EXPR)
        .map(|n| (n.text_range(), "undefinedName".to_owned()))
        .collect()
}

fn undefined_method(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    name_refs_where(root, SyntaxKind::METHOD_CALL)
        .map(|n| (n.text_range(), "undefinedCall".to_owned()))
        .collect()
}

fn undefined_type(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::ClassType::cast)
        .filter(|ty| ty.qualifier().is_none())
        .filter(|ty| {
            ty.syntax().parent().is_some_and(|p| {
                matches!(
                    p.kind(),
                    SyntaxKind::LOCAL_VAR_DECL | SyntaxKind::FIELD_DECL | SyntaxKind::PARAM
                )
            })
        })
        .filter_map(|ty| ty.name_ref())
        .filter(|name| name.text() != "var")
        .map(|name| (name.syntax().text_range(), "UndefinedType".to_owned()))
        .collect()
}

fn last_statement_of(block: &SyntaxNode) -> Option<SyntaxNode> {
    block
        .children()
        .filter(|c| ast::Stmt::can_cast(c.kind()))
        .last()
}

fn drop_return(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::MethodDecl::cast)
        .filter_map(|method| method.body())
        .filter_map(|body| last_statement_of(body.syntax()))
        .filter(|stmt| stmt.kind() == SyntaxKind::RETURN_STMT)
        .filter(|stmt| stmt.children().any(|c| ast::Expr::can_cast(c.kind())))
        .map(|stmt| (stmt.text_range(), String::new()))
        .collect()
}

fn dead_code(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter(|n| n.kind() == SyntaxKind::BLOCK)
        .filter_map(|block| last_statement_of(&block))
        .filter(|stmt| {
            matches!(
                stmt.kind(),
                SyntaxKind::RETURN_STMT | SyntaxKind::THROW_STMT
            )
        })
        .map(|stmt| {
            (
                TextRange::empty(stmt.text_range().end()),
                " int deadCode = 0;".to_owned(),
            )
        })
        .collect()
}

fn drop_initializer(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::LocalVarDecl::cast)
        .filter(|decl| {
            decl.syntax()
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::BLOCK)
        })
        .filter(|decl| decl.declarators().count() == 1)
        .filter(|decl| decl.ty().is_some_and(|ty| ty.syntax().text() != "var"))
        .filter_map(|decl| decl.declarators().next())
        .filter_map(|declarator| {
            let name = declarator.name()?;
            let init = declarator.initializer()?;
            // 数组初始化器 `{1, 2}` 删掉后剩下的声明本身合法，保留这种情况
            Some((
                TextRange::new(
                    name.syntax().text_range().end(),
                    init.syntax().text_range().end(),
                ),
                String::new(),
            ))
        })
        .collect()
}

fn drop_throws(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter(|n| n.kind() == SyntaxKind::THROWS_CLAUSE)
        .map(|n| (n.text_range(), String::new()))
        .collect()
}

/// 类和枚举里 `final` 字段的初值（接口和注解里的字段必须有初值，记录类只能有静态字段）。
fn drop_field_initializer(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::FieldDecl::cast)
        .filter(|field| {
            field
                .modifier_list()
                .is_some_and(|m| m.has(SyntaxKind::FINAL_KW))
        })
        .filter(|field| {
            field
                .syntax()
                .parent()
                .and_then(|body| body.parent())
                .is_some_and(|owner| {
                    matches!(owner.kind(), SyntaxKind::CLASS_DECL | SyntaxKind::ENUM_DECL)
                })
        })
        .flat_map(|field| field.declarators())
        .filter_map(|declarator| {
            let name = declarator.name()?;
            let init = declarator.initializer()?;
            Some((
                TextRange::new(
                    name.syntax().text_range().end(),
                    init.syntax().text_range().end(),
                ),
                String::new(),
            ))
        })
        .collect()
}

/// 把块里的一条表达式语句包进捕获 `IOException` 的 try：语句不会抛出它时 javac 报错。
fn needless_catch(root: &SyntaxNode, text: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::ExprStmt::cast)
        .filter(|stmt| {
            stmt.syntax()
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::BLOCK)
                && !matches!(stmt.expr(), Some(ast::Expr::ExplicitConstructorCall(_)))
        })
        .map(|stmt| {
            let range = stmt.syntax().text_range();
            let original = &text[usize::from(range.start())..usize::from(range.end())];
            (
                range,
                format!(
                    "try {{ {original} }} catch (java.io.IOException mutantEx) {{ throw new RuntimeException(mutantEx); }}"
                ),
            )
        })
        .collect()
}

fn wrong_type(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::VariableDeclarator::cast)
        .filter_map(|declarator| declarator.initializer())
        .filter_map(|init| match init {
            ast::Expr::Literal(literal) => Some(literal),
            _ => None,
        })
        .filter_map(|literal| {
            let replacement = match literal.kind()? {
                SyntaxKind::INT_LITERAL => "\"text\"",
                SyntaxKind::STRING_LITERAL => "42",
                SyntaxKind::TRUE_KW | SyntaxKind::FALSE_KW => "\"flag\"",
                _ => return None,
            };
            Some((literal.syntax().text_range(), replacement.to_owned()))
        })
        .collect()
}

fn extra_arg(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::MethodCall::cast)
        .filter_map(|call| call.arg_list())
        .filter_map(|list| {
            let close = list.syntax().last_token()?;
            let text = if list.args().next().is_some() {
                ", 0"
            } else {
                "0"
            };
            Some((
                TextRange::empty(close.text_range().start()),
                text.to_owned(),
            ))
        })
        .collect()
}

fn nonbool_condition(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::IfStmt::cast)
        .filter_map(|stmt| stmt.condition())
        .map(|cond| (cond.syntax().text_range(), "1".to_owned()))
        .collect()
}

fn bad_operand(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::BinaryExpr::cast)
        .filter(|expr| {
            matches!(
                expr.op_kind(),
                Some(
                    SyntaxKind::MINUS
                        | SyntaxKind::STAR
                        | SyntaxKind::SLASH
                        | SyntaxKind::LT
                        | SyntaxKind::GT
                )
            )
        })
        .filter_map(|expr| expr.rhs())
        .map(|rhs| (rhs.syntax().text_range(), "true".to_owned()))
        .collect()
}

fn duplicate_local(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::LocalVarDecl::cast)
        .filter(|decl| {
            decl.syntax()
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::BLOCK)
        })
        .filter_map(|decl| {
            let name = decl.declarators().next()?.name()?;
            Some((
                TextRange::empty(decl.syntax().text_range().end()),
                format!(" int {} = 0;", name.text()),
            ))
        })
        .collect()
}

fn make_final(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::LocalVarDecl::cast)
        .filter(|decl| {
            decl.syntax()
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::BLOCK)
        })
        .filter(|decl| decl.declarators().all(|d| d.initializer().is_some()))
        .filter(|decl| !decl.syntax().text().to_string().starts_with("final"))
        .map(|decl| {
            (
                TextRange::empty(decl.syntax().text_range().start()),
                "final ".to_owned(),
            )
        })
        .collect()
}

fn make_static(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::MethodDecl::cast)
        .filter(|method| method.body().is_some())
        .filter(|method| {
            !method
                .modifier_list()
                .is_some_and(|m| m.has(SyntaxKind::STATIC_KW) || m.has(SyntaxKind::ABSTRACT_KW))
        })
        .filter_map(|method| method.return_type())
        .map(|ty| {
            (
                TextRange::empty(ty.syntax().text_range().start()),
                "static ".to_owned(),
            )
        })
        .collect()
}

fn drop_override(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::MethodDecl::cast)
        .filter(|method| {
            method.modifier_list().is_some_and(|m| {
                m.annotations()
                    .any(|a| a.syntax().text().to_string().contains("Override"))
            })
        })
        .map(|method| (method.syntax().text_range(), String::new()))
        .collect()
}

fn narrow_access(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::MethodDecl::cast)
        .filter_map(|method| method.modifier_list())
        .filter_map(|list| list.keywords().find(|k| k.kind() == SyntaxKind::PUBLIC_KW))
        .map(|keyword| (keyword.text_range(), "private".to_owned()))
        .collect()
}

fn drop_case(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::SwitchExpr::cast)
        .filter_map(|switch| switch.block())
        .filter_map(|block| block.cases().last())
        .map(|case| (case.syntax().text_range(), String::new()))
        .collect()
}

fn to_void(root: &SyntaxNode, _: &str) -> Vec<(TextRange, String)> {
    root.descendants()
        .filter_map(ast::MethodDecl::cast)
        .filter(|method| method.body().is_some())
        .filter_map(|method| method.return_type())
        .filter(|ty| ty.syntax().text() != "void")
        .map(|ty| (ty.syntax().text_range(), "void".to_owned()))
        .collect()
}

const MUTATORS: &[Mutator] = &[
    Mutator {
        name: "undefined-name",
        intent: "把变量名改成未定义的名字",
        sites: undefined_name,
    },
    Mutator {
        name: "undefined-method",
        intent: "把方法名改成未定义的名字",
        sites: undefined_method,
    },
    Mutator {
        name: "undefined-type",
        intent: "把类型名改成未定义的名字",
        sites: undefined_type,
    },
    Mutator {
        name: "wrong-type",
        intent: "把初值换成类型不符的字面量",
        sites: wrong_type,
    },
    Mutator {
        name: "drop-return",
        intent: "删掉方法末尾的 return",
        sites: drop_return,
    },
    Mutator {
        name: "dead-code",
        intent: "在 return / throw 后面加一条语句",
        sites: dead_code,
    },
    Mutator {
        name: "drop-initializer",
        intent: "删掉局部变量的初值",
        sites: drop_initializer,
    },
    Mutator {
        name: "drop-throws",
        intent: "删掉方法的 throws 子句",
        sites: drop_throws,
    },
    Mutator {
        name: "drop-field-initializer",
        intent: "删掉 final 字段的初值",
        sites: drop_field_initializer,
    },
    Mutator {
        name: "needless-catch",
        intent: "把一条语句包进捕获 IOException 的 try",
        sites: needless_catch,
    },
    Mutator {
        name: "extra-arg",
        intent: "给方法调用多加一个实参",
        sites: extra_arg,
    },
    Mutator {
        name: "nonbool-condition",
        intent: "把 if 的条件换成整数",
        sites: nonbool_condition,
    },
    Mutator {
        name: "bad-operand",
        intent: "把算术 / 比较运算的右操作数换成 true",
        sites: bad_operand,
    },
    Mutator {
        name: "duplicate-local",
        intent: "在局部变量声明后面再声明一个同名变量",
        sites: duplicate_local,
    },
    Mutator {
        name: "make-final",
        intent: "给局部变量加上 final",
        sites: make_final,
    },
    Mutator {
        name: "make-static",
        intent: "给实例方法加上 static",
        sites: make_static,
    },
    Mutator {
        name: "drop-override",
        intent: "删掉带 @Override 的方法",
        sites: drop_override,
    },
    Mutator {
        name: "narrow-access",
        intent: "把方法的 public 改成 private",
        sites: narrow_access,
    },
    Mutator {
        name: "drop-case",
        intent: "删掉 switch 表达式的最后一个分支",
        sites: drop_case,
    },
    Mutator {
        name: "to-void",
        intent: "把方法的返回类型改成 void",
        sites: to_void,
    },
];

/// 从候选位置里均匀取至多 `limit` 个（确定性的，不依赖随机数）。
fn pick<T: Clone>(sites: &[T], limit: usize) -> Vec<T> {
    if sites.len() <= limit {
        return sites.to_vec();
    }
    (0..limit)
        .map(|i| sites[i * sites.len() / limit].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// 运行两边
// ---------------------------------------------------------------------------

struct Javac {
    binary: PathBuf,
    scratch: PathBuf,
    counter: AtomicUsize,
}

impl Javac {
    /// 编译一个文件，返回错误（行号、分类、javac 的诊断键）。
    fn compile(
        &self,
        file_name: &str,
        text: &str,
    ) -> anyhow::Result<Vec<(usize, Category, String)>> {
        let dir = self
            .scratch
            .join(self.counter.fetch_add(1, Ordering::Relaxed).to_string());
        std::fs::create_dir_all(dir.join("out"))?;
        std::fs::write(dir.join(file_name), text)?;
        let output = Command::new(&self.binary)
            .current_dir(&dir)
            .args([
                "-proc:none",
                "-XDrawDiagnostics",
                "-Xlint:none",
                "-nowarn",
                "-Xmaxerrs",
                "1000",
                "-d",
                "out",
            ])
            .arg(file_name)
            .output()
            .with_context(|| format!("无法运行 {}", self.binary.display()))?;
        let _ = std::fs::remove_dir_all(&dir);

        let mut errors = Vec::new();
        let prefix = format!("{file_name}:");
        for line in String::from_utf8_lossy(&output.stderr).lines() {
            // `Foo.java:12:9: compiler.err.cant.resolve.location: ...`
            let Some(rest) = line.strip_prefix(&prefix) else {
                continue;
            };
            let mut parts = rest.splitn(3, ':');
            let (Some(line_no), Some(_column), Some(message)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(line_no) = line_no.parse::<usize>() else {
                continue;
            };
            let Some(key) = message.trim().strip_prefix("compiler.err.") else {
                continue;
            };
            let key = key.split(':').next().unwrap_or(key).trim().to_owned();
            errors.push((line_no, Category::of_javac_key(&key), key));
        }
        if !output.status.success() && errors.is_empty() {
            anyhow::bail!(
                "javac 失败但没有可识别的错误输出:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(errors)
    }
}

fn our_errors(env: &Env, file_name: &str, text: &str) -> Vec<(usize, Category, String)> {
    let host = env.load(&[(format!("/{file_name}"), text.to_owned())]);
    let analysis = host.analysis();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        analysis
            .diagnostics(FileId(0), &DiagnosticsConfig::default())
            .unwrap_or_default()
    }));
    let Ok(diagnostics) = outcome else {
        return vec![(0, Category::Other, "分析器 panic".to_owned())];
    };
    diagnostics
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| {
            let offset = usize::from(d.range.start()).min(text.len());
            let line = text[..offset].matches('\n').count() + 1;
            (
                line,
                Category::of_our_code(d.code.as_str()),
                format!("[{}] {}", d.code.as_str(), d.message),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 汇总
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize)]
pub struct Tally {
    /// javac 报出的错误数（按行和分类去重）
    pub javac_errors: usize,
    /// 分析器在同一行报了错
    pub detected: usize,
    /// 并且分类一致
    pub exact: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct JavacReport {
    pub corpus_files: usize,
    /// javac 不接受的语料文件（语料本身有问题）
    pub broken_corpus: Vec<String>,
    /// javac 接受、分析器却报了错的地方
    pub false_positives: Vec<String>,
    pub mutants: usize,
    /// javac 也接受的变体（变异没有造成错误）
    pub benign_mutants: usize,
    /// 分析器在 javac 没报错的行上报了错（变体本身是错的，但位置或内容对不上）
    pub extra_reports: Vec<String>,
    /// 按 javac 的错误分类
    pub by_category: BTreeMap<String, Tally>,
    /// 按变异算子
    pub by_mutator: BTreeMap<String, Tally>,
    /// 没检出的例子（每类留几条，方便看差距在哪）
    pub missed_examples: Vec<String>,
}

impl JavacReport {
    pub fn supported_totals(&self) -> Tally {
        let mut total = Tally::default();
        for (name, tally) in &self.by_category {
            if name != Category::Other.name() {
                total.javac_errors += tally.javac_errors;
                total.detected += tally.detected;
                total.exact += tally.exact;
            }
        }
        total
    }
}

struct CaseOutcome {
    label: String,
    mutator: Option<&'static str>,
    javac: Vec<(usize, Category, String)>,
    ours: Vec<(usize, Category, String)>,
}

pub fn run(
    env: &Env,
    corpus_dir: &Path,
    filter: Option<&str>,
    sites_per_mutator: usize,
) -> anyhow::Result<JavacReport> {
    let jdk_home = project_model::discover_jdk(None)
        .context("没有找到 JDK")?
        .home;
    let binary = jdk_home.join("bin/javac");
    anyhow::ensure!(
        binary.is_file(),
        "{} 不存在：差分评测需要完整的 JDK",
        binary.display()
    );
    let scratch = std::env::temp_dir().join(format!("ja-eval-{}", std::process::id()));
    let javac = Javac {
        binary,
        scratch: scratch.clone(),
        counter: AtomicUsize::new(0),
    };

    let mut corpus: Vec<PathBuf> = std::fs::read_dir(corpus_dir)
        .with_context(|| format!("无法读取 {}", corpus_dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "java"))
        .filter(|p| filter.is_none_or(|f| p.to_string_lossy().contains(f)))
        .collect();
    corpus.sort();

    // 展开成"原文 + 变体"的任务列表
    let mut cases: Vec<(String, Option<&'static str>, String, String)> = Vec::new();
    for path in &corpus {
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path)?;
        cases.push((file_name.clone(), None, file_name.clone(), text.clone()));
        let root = ast::CompilationUnit::parse(&text).syntax_node();
        for mutator in MUTATORS {
            let sites = (mutator.sites)(&root, &text);
            for (range, replacement) in pick(&sites, sites_per_mutator) {
                let mut mutated = text.clone();
                mutated.replace_range(
                    usize::from(range.start())..usize::from(range.end()),
                    &replacement,
                );
                let line = text[..usize::from(range.start())].matches('\n').count() + 1;
                cases.push((
                    format!("{file_name}:{line} {}", mutator.name),
                    Some(mutator.name),
                    file_name.clone(),
                    mutated,
                ));
            }
        }
    }

    let outcomes: Vec<anyhow::Result<CaseOutcome>> = cases
        .par_iter()
        .map(|(label, mutator, file_name, text)| {
            Ok(CaseOutcome {
                label: label.clone(),
                mutator: *mutator,
                javac: javac.compile(file_name, text)?,
                ours: our_errors(env, file_name, text),
            })
        })
        .collect();
    let _ = std::fs::remove_dir_all(&scratch);

    let mut report = JavacReport {
        corpus_files: corpus.len(),
        ..JavacReport::default()
    };
    let mut missed_per_category: BTreeMap<Category, usize> = BTreeMap::new();
    for outcome in outcomes {
        let case = outcome?;
        match case.mutator {
            None => {
                if !case.javac.is_empty() {
                    report
                        .broken_corpus
                        .push(format!("{}: javac 报 {:?}", case.label, case.javac));
                    continue;
                }
                for (line, _, message) in &case.ours {
                    report
                        .false_positives
                        .push(format!("{}:{line} {message}", case.label));
                }
            }
            Some(mutator) => {
                report.mutants += 1;
                if case.javac.is_empty() {
                    report.benign_mutants += 1;
                    for (line, _, message) in &case.ours {
                        report.false_positives.push(format!(
                            "{}（javac 接受这个变体）第 {line} 行 {message}",
                            case.label
                        ));
                    }
                    continue;
                }
                let mut javac_findings: Vec<Finding> = case
                    .javac
                    .iter()
                    .map(|(line, category, _)| (*line, *category))
                    .collect();
                javac_findings.sort();
                javac_findings.dedup();
                for (line, category) in &javac_findings {
                    let detected = case.ours.iter().any(|(l, _, _)| l == line);
                    let exact = case.ours.iter().any(|(l, c, _)| l == line && c == category);
                    for tally in [
                        report
                            .by_category
                            .entry(category.name().to_owned())
                            .or_default(),
                        report.by_mutator.entry(mutator.to_owned()).or_default(),
                    ] {
                        tally.javac_errors += 1;
                        tally.detected += usize::from(detected);
                        tally.exact += usize::from(exact);
                    }
                    if !detected && *category != Category::Other {
                        let seen = missed_per_category.entry(*category).or_default();
                        if *seen < 4 {
                            *seen += 1;
                            let key = case
                                .javac
                                .iter()
                                .find(|(l, c, _)| l == line && c == category)
                                .map_or("", |(_, _, key)| key.as_str());
                            report.missed_examples.push(format!(
                                "{}: javac 在第 {line} 行报 {key}（{}），分析器没报",
                                case.label,
                                category.name()
                            ));
                        }
                    }
                }
                // javac 在类型检查阶段出错之后不再做流分析（可达性、确定赋值、异常），
                // 这时分析器多出来的流分析诊断没有可比的标准答案，不算多报
                let javac_skipped_flow = case.javac.iter().any(|(_, c, _)| !c.is_flow());
                for (line, category, message) in &case.ours {
                    if javac_skipped_flow && category.is_flow() {
                        continue;
                    }
                    if !case.javac.iter().any(|(l, _, _)| l == line) {
                        report.extra_reports.push(format!(
                            "{} 第 {line} 行 {message}（javac 的错误在第 {:?} 行）",
                            case.label,
                            case.javac.iter().map(|(l, _, _)| *l).collect::<Vec<_>>()
                        ));
                    }
                }
            }
        }
    }
    Ok(report)
}

pub fn mutator_descriptions() -> Vec<(&'static str, &'static str)> {
    MUTATORS.iter().map(|m| (m.name, m.intent)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "class A {\n    int f(int a) throws Exception {\n        int x = 1;\n        String s = \"t\";\n        g(a);\n        return x + s.length();\n    }\n    void g(int v) {}\n}\n";

    fn mutants(name: &str) -> Vec<String> {
        let root = ast::CompilationUnit::parse(SAMPLE).syntax_node();
        let mutator = MUTATORS.iter().find(|m| m.name == name).unwrap();
        (mutator.sites)(&root, SAMPLE)
            .into_iter()
            .map(|(range, replacement)| {
                let mut text = SAMPLE.to_owned();
                text.replace_range(
                    usize::from(range.start())..usize::from(range.end()),
                    &replacement,
                );
                text
            })
            .collect()
    }

    #[test]
    fn mutators_produce_the_intended_edits() {
        assert!(mutants("undefined-name")
            .iter()
            .any(|m| m.contains("g(undefinedName);")));
        assert!(mutants("undefined-method")
            .iter()
            .any(|m| m.contains("undefinedCall(a);")));
        assert!(mutants("undefined-type")
            .iter()
            .any(|m| m.contains("UndefinedType s = ")));
        assert!(mutants("wrong-type")
            .iter()
            .any(|m| m.contains("int x = \"text\";")));
        assert!(mutants("wrong-type")
            .iter()
            .any(|m| m.contains("String s = 42;")));
        assert_eq!(mutants("drop-return").len(), 1);
        assert!(!mutants("drop-return")[0].contains("return"));
        assert!(mutants("dead-code")[0].contains("return x + s.length(); int deadCode = 0;"));
        assert!(mutants("drop-initializer")
            .iter()
            .any(|m| m.contains("int x;")));
        assert!(mutants("drop-throws")[0].contains("int f(int a)  {"));
        assert!(mutants("needless-catch")
            .iter()
            .any(|m| m.contains("try { g(a); } catch (java.io.IOException mutantEx)")));
        assert!(mutants("drop-field-initializer").is_empty());

        let text = "class B {\n    private final int n = 1;\n    static final String S = \"s\";\n}\ninterface I {\n    int K = 1;\n}\n";
        let root = ast::CompilationUnit::parse(text).syntax_node();
        let sites = drop_field_initializer(&root, text);
        assert_eq!(sites.len(), 2);
        let mut mutated = text.to_owned();
        mutated.replace_range(
            usize::from(sites[0].0.start())..usize::from(sites[0].0.end()),
            "",
        );
        assert!(mutated.contains("private final int n;"));
    }

    #[test]
    fn pick_is_deterministic_and_bounded() {
        let sites: Vec<u32> = (0..10).collect();
        assert_eq!(pick(&sites, 3), [0, 3, 6]);
        assert_eq!(pick(&sites[..2], 3), [0, 1]);
    }

    #[test]
    fn javac_keys_map_to_categories() {
        assert_eq!(
            Category::of_javac_key("cant.resolve.location"),
            Category::Unresolved
        );
        assert_eq!(
            Category::of_javac_key("prob.found.req"),
            Category::TypeMismatch
        );
        assert_eq!(
            Category::of_javac_key("unreported.exception.need.to.catch.or.throw"),
            Category::UnhandledException
        );
        assert_eq!(
            Category::of_javac_key("missing.ret.stmt"),
            Category::MissingReturn
        );
        assert_eq!(
            Category::of_javac_key("already.defined"),
            Category::Duplicate
        );
        assert_eq!(
            Category::of_javac_key("cant.apply.symbols"),
            Category::ArgumentMismatch
        );
        assert_eq!(
            Category::of_javac_key("except.never.thrown.in.try"),
            Category::NeverThrown
        );
        assert_eq!(
            Category::of_javac_key("var.not.initialized.in.default.constructor"),
            Category::Uninitialized
        );
        assert_eq!(Category::of_javac_key("illegal.char"), Category::Other);
    }
}
