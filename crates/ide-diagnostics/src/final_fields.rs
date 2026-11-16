//! `final` 字段的初始化（JLS §8.3.1.2、§16.8、§16.9）：
//!
//! - 实例 blank final 字段：没有初值、实例初始化块之后也没有一定赋值时，每个不以 `this(...)`
//!   开头的构造器正常结束时都必须一定已经给它赋值；没有写构造器时报默认构造器没有初始化它；
//! - 静态 blank final 字段：静态初始化块全部执行完之后必须一定已经赋值；
//! - 一定赋值之前的读取：初始化块、字段初值（按书写顺序）和构造器里用简单名（实例字段
//!   还有 `this.x`）读取还没有一定赋值的 blank final 字段；
//! - 给 `final` 字段赋值的位置是否合法（供"给 final 赋值"的检查使用）。
//!
//! 只报确定的：
//! - 字段在初值、初始化块、构造器之外（方法、lambda、匿名类 / 局部类、别的类）被赋值过，
//!   或者用 `this.x` 之外的限定形式赋值的，不判断；
//! - 确定赋值沿用局部变量的保守分析（拿不准就算已赋值），lambda、匿名类 / 局部类里的读取、
//!   `return` 前是否已赋值不检查；构造器有序言（`super(...)` 之前的语句）的类不检查读取；
//! - 用了 Lombok 的类、记录类、接口、匿名类和局部类不检查。

use base_db::{Db, SourceFile};
use hir::{body_syntax, class_data, BodyId, ClassId, ClassKind, FieldId, Resolution, Semantics};
use rustc_hash::{FxHashMap, FxHashSet};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{flow::DefiniteAssignment, view::View, Diagnostic, DiagnosticCode, Severity};

fn has_modifier(node: &SyntaxNode, kind: SyntaxKind) -> bool {
    node.children()
        .find_map(ast::ModifierList::cast)
        .is_some_and(|list| list.has(kind))
}

fn strip_parens(mut node: SyntaxNode) -> SyntaxNode {
    while node.kind() == SyntaxKind::PAREN_EXPR {
        match node.children().find(|c| ast::Expr::can_cast(c.kind())) {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

/// 被赋值的表达式（`x = ..`、`x += ..`、`x++`、`--x` 的 `x`），去掉了括号。
fn assignment_target(node: &SyntaxNode) -> Option<SyntaxNode> {
    let target = match node.kind() {
        SyntaxKind::ASSIGN_EXPR => node.children().find(|c| ast::Expr::can_cast(c.kind())),
        SyntaxKind::UNARY_EXPR | SyntaxKind::POSTFIX_EXPR => {
            let mutates = node
                .children_with_tokens()
                .any(|e| matches!(e.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS));
            if mutates {
                node.children().find(|c| ast::Expr::can_cast(c.kind()))
            } else {
                None
            }
        }
        _ => None,
    }?;
    Some(strip_parens(target))
}

/// 被赋值的表达式里引用的名字，以及它是简单名或 `this.x`。
fn target_name(target: &SyntaxNode) -> Option<(ast::NameRef, bool)> {
    match ast::Expr::cast(target.clone())? {
        ast::Expr::NameExpr(name) => Some((name.name_ref()?, true)),
        ast::Expr::FieldAccess(access) => {
            let this = matches!(
                access.receiver(),
                Some(ast::Expr::ThisExpr(this)) if this.qualifier().is_none()
            );
            Some((access.name_ref()?, this))
        }
        _ => None,
    }
}

/// 字段在源码里写了初值。class 文件里的字段为 `None`。
///
/// 看字段自己的语法树（[`hir::body_tree`]）而不是文件的：单独检查一个体时，结果不该因为
/// 文件别处的编辑而失效。
fn has_initializer(db: &dyn Db, field: FieldId) -> Option<bool> {
    let tree = hir::body_tree(db, BodyId::Field(field))?;
    let node = tree.body_node(&tree.root())?;
    Some(ast::VariableDeclarator::cast(node)?.initializer().is_some())
}

/// 类体所属的具名类；匿名类、枚举常量的类体和局部类为 `None`。
fn class_of_body(sema: &Semantics<'_>, file: SourceFile, body: &SyntaxNode) -> Option<ClassId> {
    let decl = ast::TypeDecl::cast(body.parent()?)?;
    sema.class_of_decl(file, &decl)
}

/// 赋值所在的初始化上下文：`(所属的具名类, 是否静态)`。
enum Context {
    /// 构造器、初始化块或字段初值
    Initializer(Option<ClassId>, bool),
    /// 方法（包括其中的 lambda）
    Method,
    /// 构造器 / 初始化块里的 lambda，或者认不出来的位置
    Unknown,
}

fn context_of(sema: &Semantics<'_>, file: SourceFile, target: &SyntaxNode) -> Context {
    let mut in_lambda = false;
    let mut inner = target.clone();
    for ancestor in target.ancestors().skip(1) {
        match ancestor.kind() {
            SyntaxKind::LAMBDA_EXPR => in_lambda = true,
            SyntaxKind::CLASS_BODY | SyntaxKind::COMPILATION_UNIT => {
                let is_static = match inner.kind() {
                    SyntaxKind::METHOD_DECL => return Context::Method,
                    SyntaxKind::CONSTRUCTOR_DECL => false,
                    SyntaxKind::INITIALIZER_BLOCK => {
                        ast::InitializerBlock::cast(inner.clone()).is_some_and(|b| b.is_static())
                    }
                    SyntaxKind::FIELD_DECL => has_modifier(&inner, SyntaxKind::STATIC_KW),
                    _ => return Context::Unknown,
                };
                if in_lambda {
                    return Context::Unknown;
                }
                let class = (ancestor.kind() == SyntaxKind::CLASS_BODY)
                    .then(|| class_of_body(sema, file, &ancestor))
                    .flatten();
                return Context::Initializer(class, is_static);
            }
            _ => {}
        }
        inner = ancestor;
    }
    Context::Unknown
}

/// 在 `target` 处给 `final` 字段 `field` 赋值是否合法。拿不准时为 `None`。
///
/// 合法的只有：字段没有初值，赋值写在字段所在类自己的构造器、初始化块或字段初值里
/// （静态字段在静态初始化块 / 静态字段初值里），用简单名或 `this.x`。
pub(crate) fn may_assign_final_field(
    db: &dyn Db,
    sema: &Semantics<'_>,
    file: SourceFile,
    target: &SyntaxNode,
    field: FieldId,
) -> Option<bool> {
    let field_static = hir::field_flags(db, field)?.is_static();
    match context_of(sema, file, target) {
        Context::Method => Some(false),
        Context::Unknown => None,
        Context::Initializer(class, is_static) => {
            // 匿名类 / 局部类的构造器和初始化块：字段可能就是它自己的
            let class = class?;
            if class != field.class {
                return Some(false);
            }
            if is_static != field_static {
                // 静态字段不能在实例上下文里初始化；反过来是"静态上下文"错误，不在这里报
                return field_static.then_some(false);
            }
            if has_initializer(db, field)? {
                return Some(false);
            }
            // `Foo.X = ..`、`other.x = ..` 这些限定形式不判断
            target_name(target)?.1.then_some(true)
        }
    }
}

/// 一个 blank final 字段的赋值情况。
#[derive(Default)]
struct FieldUses {
    /// 解析到这个字段的名字引用
    refs: FxHashSet<TextRange>,
    /// 赋值都写在本类的初始化上下文里、用简单名或 `this.x`
    simple: bool,
    /// 在字段初值里赋值过
    in_field_initializer: bool,
    /// 解析到这个字段、按确定赋值规则算作读取它的名字引用：简单名，实例字段还有 `this.x`
    reads: FxHashSet<TextRange>,
}

/// 要看整个文件（`view` 是整个文件）。
pub(crate) fn uninitialized_final_fields(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    let file = view.file;
    let sema = &view.sema;
    let root = &view.root;

    // 先找出要检查的字段：具名类里没有初值的 final 字段
    let mut fields: FxHashMap<FieldId, FieldUses> = FxHashMap::default();
    let mut classes = Vec::new();
    for decl in root.descendants().filter_map(ast::TypeDecl::cast) {
        let Some(class) = sema.class_of_decl(file, &decl) else {
            continue;
        };
        let Some(data) = class_data(db, class) else {
            continue;
        };
        if !matches!(data.kind, ClassKind::Class | ClassKind::Enum)
            || crate::typeck::lombok_involved(db, class)
        {
            continue;
        }
        let Some(body) = decl.body() else { continue };
        let mut any = false;
        for member in body.members() {
            let ast::Member::FieldDecl(field) = member else {
                continue;
            };
            if !has_modifier(field.syntax(), SyntaxKind::FINAL_KW) {
                continue;
            }
            for declarator in field.declarators() {
                if declarator.initializer().is_some() {
                    continue;
                }
                if let Some(id) = sema.field_of_decl(file, declarator.syntax()) {
                    fields.insert(
                        id,
                        FieldUses {
                            simple: true,
                            ..FieldUses::default()
                        },
                    );
                    any = true;
                }
            }
        }
        if any {
            classes.push((class, body));
        }
    }
    if fields.is_empty() {
        return;
    }

    let mut by_name: FxHashMap<String, Vec<FieldId>> = FxHashMap::default();
    for id in fields.keys() {
        let name = hir::field_data(db, *id).map(|f| f.name.to_string());
        if let Some(name) = name {
            by_name.entry(name).or_default().push(*id);
        }
    }

    // 引用和赋值
    let mut static_finals: FxHashSet<TextRange> = FxHashSet::default();
    for body in &view.bodies {
        let result = body.result(view);
        // 没有解析结果的同名引用（`x` 或 `this.x`）也当作这个字段，宁可算作已赋值
        for name_ref in body.node.descendants().filter_map(ast::NameRef::cast) {
            let range = name_ref.syntax().text_range();
            if result.resolution(range).is_some() {
                continue;
            }
            let Some(ids) = by_name.get(name_ref.text().as_str()) else {
                continue;
            };
            let candidate = name_ref.syntax().parent().is_some_and(|parent| {
                parent.kind() == SyntaxKind::NAME_EXPR
                    || target_name(&parent).is_some_and(|(_, simple)| simple)
            });
            if candidate {
                for id in ids {
                    if let Some(uses) = fields.get_mut(id) {
                        uses.refs.insert(range);
                    }
                }
            }
        }
        for (range, resolution) in result.resolutions() {
            let Resolution::Field(field) = resolution else {
                continue;
            };
            if let Some(uses) = fields.get_mut(field) {
                uses.refs.insert(range);
                if counts_as_read(root, range, field_is_static(db, *field)) {
                    uses.reads.insert(range);
                }
            }
            let is_static_final = hir::field_flags(db, *field)
                .is_some_and(|flags| flags.is_static() && flags.contains(hir::Modifiers::FINAL));
            if is_static_final {
                static_finals.insert(range);
            }
        }
        for node in body.node.descendants() {
            let Some(target) = assignment_target(&node) else {
                continue;
            };
            let Some((name, simple)) = target_name(&target) else {
                continue;
            };
            let Some(Resolution::Field(field)) = result.resolution(name.syntax().text_range())
            else {
                continue;
            };
            let Some(uses) = fields.get_mut(field) else {
                continue;
            };
            let field_static = field_is_static(db, *field);
            match context_of(sema, file, &target) {
                Context::Initializer(Some(class), is_static)
                    if class == field.class && is_static == field_static && simple =>
                {
                    if target
                        .ancestors()
                        .any(|a| a.kind() == SyntaxKind::FIELD_DECL)
                    {
                        uses.in_field_initializer = true;
                    }
                }
                _ => uses.simple = false,
            }
        }
    }

    for (class, body) in classes {
        // 初始化块和字段初值按书写顺序执行：(是否静态, 要走的语法节点)
        let initializers: Vec<(bool, SyntaxNode)> = body
            .members()
            .flat_map(|m| match m {
                ast::Member::InitializerBlock(block) => block
                    .body()
                    .map(|body| (block.is_static(), body.syntax().clone()))
                    .into_iter()
                    .collect::<Vec<_>>(),
                ast::Member::FieldDecl(field) => {
                    let is_static = has_modifier(field.syntax(), SyntaxKind::STATIC_KW);
                    field
                        .declarators()
                        .filter_map(|d| d.initializer())
                        .map(|init| (is_static, init.syntax().clone()))
                        .collect()
                }
                _ => Vec::new(),
            })
            .collect();
        let ctors: Vec<ast::ConstructorDecl> = body
            .members()
            .filter_map(|m| match m {
                ast::Member::ConstructorDecl(ctor) => Some(ctor),
                _ => None,
            })
            .collect();
        let Some(data) = class_data(db, class) else {
            continue;
        };
        // 构造器在 `super(...)` / `this(...)` 之前有语句（JDK 22 起的构造器序言）时，序言里可以
        // 给字段赋值，赋值发生在初始化块之前：这种类不检查读取
        let has_prologue = ctors.iter().any(|ctor| {
            ctor.body().is_some_and(|block| {
                block.statements().skip(1).any(|stmt| {
                    matches!(&stmt, ast::Stmt::ExprStmt(it)
                        if matches!(it.expr(), Some(ast::Expr::ExplicitConstructorCall(_))))
                })
            })
        });
        // 只在普通类里检查静态字段的读取（枚举常量的实参、类体先于静态初始化执行）
        let check_static_reads = data.kind == ClassKind::Class;
        for (id, uses) in &fields {
            if id.class != class || !uses.simple || uses.in_field_initializer {
                continue;
            }
            let Some(field) = data.fields.get(id.idx as usize) else {
                continue;
            };
            let is_static = field.flags.is_static();
            let Some((_, declarator)) = body_syntax(db, BodyId::Field(*id)) else {
                continue;
            };
            let Some(name_range) = ast::VariableDeclarator::cast(declarator)
                .and_then(|d| d.name())
                .map(|n| n.syntax().text_range())
            else {
                continue;
            };
            let field_name = &field.name;
            let check_reads = !has_prologue && (!is_static || check_static_reads);
            let mut analysis = DefiniteAssignment {
                refs: &uses.refs,
                field_refs: &static_finals,
                assigned: false,
                reportable: Some(&uses.reads),
                bad_reads: Vec::new(),
                labeled_breaks: Vec::new(),
            };
            // 初始化块和字段初值按顺序执行
            for (_, node) in initializers.iter().filter(|(s, _)| *s == is_static) {
                analysis.expr(node);
            }
            if analysis.assigned {
                if check_reads {
                    report_reads(&analysis.bad_reads, field_name, out);
                }
                continue;
            }
            if is_static {
                if check_reads {
                    report_reads(&analysis.bad_reads, field_name, out);
                }
                out.push(Diagnostic {
                    range: name_range,
                    message: base_db::tr!(
                        "静态 final 字段 `{field_name}` 可能尚未初始化：静态初始化块里没有给它赋值",
                        "Variable `{field_name}` might not have been initialized: no static initializer assigns it"
                    ),
                    severity: Severity::Error,
                    code: DiagnosticCode::UninitializedFinalField,
                    unused: false,
                });
                continue;
            }
            if ctors.is_empty() {
                if check_reads {
                    report_reads(&analysis.bad_reads, field_name, out);
                }
                out.push(Diagnostic {
                    range: name_range,
                    message: base_db::tr!(
                        "final 字段 `{field_name}` 没有在默认构造器里初始化",
                        "Variable `{field_name}` not initialized in the default constructor"
                    ),
                    severity: Severity::Error,
                    code: DiagnosticCode::UninitializedFinalField,
                    unused: false,
                });
                continue;
            }
            let after_initializers = analysis.assigned;
            for ctor in &ctors {
                let Some(block) = ctor.body() else {
                    continue;
                };
                // `this(...)` 委托给别的构造器，由那个构造器负责
                let delegates = block.statements().any(|stmt| {
                    matches!(&stmt, ast::Stmt::ExprStmt(it)
                        if matches!(it.expr(), Some(ast::Expr::ExplicitConstructorCall(call))
                            if call.syntax().first_child().is_some_and(|c| c.kind() == SyntaxKind::THIS_EXPR)))
                });
                if delegates {
                    continue;
                }
                analysis.assigned = after_initializers;
                analysis.labeled_breaks.clear();
                analysis.expr(block.syntax());
                if analysis.assigned {
                    continue;
                }
                let range = block
                    .r_curly_token()
                    .map(|t| t.text_range())
                    .unwrap_or_else(|| block.syntax().text_range());
                out.push(Diagnostic {
                    range,
                    message: base_db::tr!(
                        "final 字段 `{field_name}` 可能尚未初始化：构造器没有在所有路径上给它赋值",
                        "Variable `{field_name}` might not have been initialized: the constructor does not assign it on every path"
                    ),
                    severity: Severity::Error,
                    code: DiagnosticCode::UninitializedFinalField,
                    unused: false,
                });
            }
            if check_reads {
                report_reads(&analysis.bad_reads, field_name, out);
            }
        }
    }
}

fn field_is_static(db: &dyn Db, field: FieldId) -> bool {
    hir::field_flags(db, field).is_some_and(|f| f.is_static())
}

/// 名字引用 `range` 处是不是确定赋值规则（JLS §16）里对字段的读取：简单名 `x`，实例字段
/// 还有 `this.x`。`other.x`、`Outer.this.x`、`Foo.X` 不算。
fn counts_as_read(root: &SyntaxNode, range: TextRange, is_static: bool) -> bool {
    let start = match root.covering_element(range) {
        syntax::NodeOrToken::Node(node) => Some(node),
        syntax::NodeOrToken::Token(token) => token.parent(),
    };
    let Some(name) = start.and_then(|start| {
        start
            .ancestors()
            .find(|n| n.kind() == SyntaxKind::NAME_REF && n.text_range() == range)
    }) else {
        return false;
    };
    let Some(parent) = name.parent() else {
        return false;
    };
    match parent.kind() {
        SyntaxKind::NAME_EXPR => true,
        SyntaxKind::FIELD_ACCESS if !is_static => ast::FieldAccess::cast(parent).is_some_and(|access| {
            access.name_ref().is_some_and(|n| n.syntax() == &name)
                && matches!(access.receiver(), Some(ast::Expr::ThisExpr(this)) if this.qualifier().is_none())
        }),
        _ => false,
    }
}

/// blank final 字段在一定赋值之前被读取。
fn report_reads(reads: &[TextRange], field_name: &str, out: &mut Vec<Diagnostic>) {
    for &range in reads {
        out.push(Diagnostic {
            range,
            message: base_db::tr!(
                "变量 `{field_name}` 可能尚未初始化",
                "Variable `{field_name}` might not have been initialized"
            ),
            severity: Severity::Error,
            code: DiagnosticCode::UninitializedVariable,
            unused: false,
        });
    }
}
