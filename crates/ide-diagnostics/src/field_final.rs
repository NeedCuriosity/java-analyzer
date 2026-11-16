//! 可以声明为 `final` 的私有字段：只在声明处、或者每个构造器 / 初始化块里恰好赋值一次。
//!
//! 判断是保守的：赋值必须是构造器（初始化块）体里的顶层语句 `x = ...` / `this.x = ...`，
//! 不能在分支、循环、`try`、lambda 或匿名类里；构造器里有 `return`、赋值之前读过这个字段、
//! 字段带注解（可能由框架注入）、类带会给字段赋值的 Lombok 注解（`@Setter` / `@Data` /
//! `@AllArgsConstructor` / `@Builder` / `@SuperBuilder`）、`volatile` / `transient`
//! 字段、一行声明多个变量，都不提示。

use hir::{class_data, BodyId, ClassKind, FieldId, Modifiers, Resolution};
use rustc_hash::FxHashMap;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange,
};

use crate::{view::View, Diagnostic, DiagnosticCode, Severity};

/// 对字段的一次写入。
struct Write {
    /// 写入所在的构造器或初始化块（是它体里的顶层语句 `x = ...` 时）
    owner: Option<SyntaxNode>,
    at: syntax::TextSize,
}

/// 赋值目标是不是对字段的"简单"写入（`x` 或 `this.x`），返回名字引用的范围。
fn target_name(target: &SyntaxNode) -> Option<(TextRange, bool)> {
    let mut target = target.clone();
    while target.kind() == SyntaxKind::PAREN_EXPR {
        target = target.children().find(|c| ast::Expr::can_cast(c.kind()))?;
    }
    match target.kind() {
        SyntaxKind::NAME_EXPR => {
            let name = target
                .children()
                .find(|c| c.kind() == SyntaxKind::NAME_REF)?;
            Some((name.text_range(), true))
        }
        SyntaxKind::FIELD_ACCESS => {
            let name = target
                .children()
                .filter(|c| c.kind() == SyntaxKind::NAME_REF)
                .last()?;
            let receiver = target.children().find(|c| ast::Expr::can_cast(c.kind()));
            // 只认不带限定的 `this.x`
            let simple =
                receiver.is_some_and(|r| r.kind() == SyntaxKind::THIS_EXPR && r.text() == "this");
            Some((name.text_range(), simple))
        }
        _ => None,
    }
}

/// `x = ...` 是不是构造器 / 初始化块体里的顶层语句；是的话返回那个构造器 / 初始化块。
fn top_level_owner(assign: &SyntaxNode) -> Option<SyntaxNode> {
    let stmt = assign
        .parent()
        .filter(|p| p.kind() == SyntaxKind::EXPR_STMT)?;
    let block = stmt.parent().filter(|p| p.kind() == SyntaxKind::BLOCK)?;
    let owner = block.parent()?;
    matches!(
        owner.kind(),
        SyntaxKind::CONSTRUCTOR_DECL | SyntaxKind::INITIALIZER_BLOCK
    )
    .then_some(owner)
}

fn is_static_initializer(node: &SyntaxNode) -> bool {
    // `static` 可能直接是子 token，也可能在修饰符列表里
    node.kind() == SyntaxKind::INITIALIZER_BLOCK
        && node
            .children_with_tokens()
            .take_while(|e| e.kind() != SyntaxKind::BLOCK)
            .flat_map(|e| match e {
                syntax::NodeOrToken::Node(n) => n
                    .descendants_with_tokens()
                    .filter_map(|d| d.into_token())
                    .collect::<Vec<_>>(),
                syntax::NodeOrToken::Token(t) => vec![t],
            })
            .any(|t| t.kind() == SyntaxKind::STATIC_KW)
}

/// 构造器以 `this(...)` 开头：字段由被委托的构造器赋值。
fn delegates(ctor: &SyntaxNode) -> bool {
    let Some(block) = ctor.children().find(|c| c.kind() == SyntaxKind::BLOCK) else {
        return false;
    };
    block
        .children()
        .find(|c| ast::Stmt::can_cast(c.kind()))
        .is_some_and(|first| first.text().to_string().trim_start().starts_with("this("))
}

fn has_lombok_setters(class: &SyntaxNode) -> bool {
    class
        .children()
        .find_map(ast::ModifierList::cast)
        .is_some_and(|m| {
            m.annotations().any(|a| {
                a.name().is_some_and(|n| {
                    let text = n.text();
                    let simple = text.rsplit('.').next().unwrap_or(&text).to_owned();
                    matches!(
                        simple.as_str(),
                        "Setter" | "Data" | "AllArgsConstructor" | "Builder" | "SuperBuilder"
                    )
                })
            })
        })
}

/// 要看整个文件（`view` 是整个文件）。
pub(crate) fn field_may_be_final(view: &View<'_>, out: &mut Vec<Diagnostic>) {
    let db = view.db;
    let root = &view.root;
    // 先找候选字段，没有就不用分析各个体
    let mut candidates: Vec<(FieldId, ast::VariableDeclarator, SyntaxNode)> = Vec::new();
    for field_decl in root.descendants().filter_map(ast::FieldDecl::cast) {
        let declarators: Vec<_> = field_decl.declarators().collect();
        let [declarator] = declarators.as_slice() else {
            continue;
        };
        if field_decl
            .modifier_list()
            .is_some_and(|m| m.annotations().next().is_some())
        {
            continue;
        }
        let Some(BodyId::Field(field)) = view.body_of(declarator.syntax()) else {
            continue;
        };
        let Some(data) = class_data(db, field.class) else {
            continue;
        };
        let Some(f) = data.fields.get(field.idx as usize) else {
            continue;
        };
        let flags = f.flags;
        if flags.visibility() != hir::Visibility::Private
            || flags.is_final()
            || flags.contains(Modifiers::VOLATILE)
            || flags.contains(Modifiers::TRANSIENT)
            || !matches!(data.kind, ClassKind::Class | ClassKind::Enum)
        {
            continue;
        }
        let Some(class_node) = field_decl
            .syntax()
            .parent()
            .filter(|p| p.kind() == SyntaxKind::CLASS_BODY)
            .and_then(|b| b.parent())
        else {
            continue;
        };
        if has_lombok_setters(&class_node) {
            continue;
        }
        candidates.push((field, declarator.clone(), class_node));
    }
    if candidates.is_empty() {
        return;
    }

    // 所有对候选字段的写入和读取
    let mut writes: FxHashMap<FieldId, Vec<Write>> = FxHashMap::default();
    // 读取的位置（用来判断构造器里是否在赋值之前读过）
    let mut reads: FxHashMap<FieldId, Vec<TextRange>> = FxHashMap::default();
    for body in &view.bodies {
        let node = &body.node;
        let result = body.result(view);
        let mut write_names: Vec<TextRange> = Vec::new();
        for n in node.descendants() {
            let (target, plain) = match n.kind() {
                SyntaxKind::ASSIGN_EXPR => {
                    let plain = ast::AssignExpr::cast(n.clone())
                        .and_then(|a| a.op_token())
                        .is_some_and(|t| t.kind() == SyntaxKind::EQ);
                    (n.children().find(|c| ast::Expr::can_cast(c.kind())), plain)
                }
                SyntaxKind::UNARY_EXPR | SyntaxKind::POSTFIX_EXPR => {
                    let is_increment =
                        n.children_with_tokens()
                            .filter_map(|e| e.into_token())
                            .any(|t| {
                                matches!(t.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS)
                            });
                    if !is_increment {
                        continue;
                    }
                    (n.children().find(|c| ast::Expr::can_cast(c.kind())), false)
                }
                _ => continue,
            };
            let Some((name, simple)) = target.as_ref().and_then(target_name) else {
                continue;
            };
            let Some(Resolution::Field(field)) = result.resolution(name) else {
                continue;
            };
            write_names.push(name);
            let owner = (plain && simple).then(|| top_level_owner(&n)).flatten();
            writes.entry(*field).or_default().push(Write {
                owner,
                at: n.text_range().start(),
            });
        }
        for (range, resolution) in result.resolutions() {
            if let Resolution::Field(field) = resolution {
                if !write_names.contains(&range) {
                    reads.entry(*field).or_default().push(range);
                }
            }
        }
    }

    for (field, declarator, class_node) in candidates {
        let Some(data) = class_data(db, field.class) else {
            continue;
        };
        let Some(f) = data.fields.get(field.idx as usize) else {
            continue;
        };
        let is_static = f.flags.is_static();
        let field_writes = writes.get(&field).map(Vec::as_slice).unwrap_or_default();
        let ok = if declarator.initializer().is_some() {
            field_writes.is_empty()
        } else {
            definitely_assigned_once(&class_node, is_static, field_writes, reads.get(&field))
        };
        if !ok {
            continue;
        }
        let Some(name) = declarator.name() else {
            continue;
        };
        out.push(Diagnostic {
            range: name.syntax().text_range(),
            message: base_db::tr!(
                "字段 `{}` 可以声明为 final",
                "Field `{}` may be final",
                f.name
            ),
            severity: Severity::Warning,
            code: DiagnosticCode::FieldMayBeFinal,
            unused: false,
        });
    }
}

/// 没有初值的字段：每条构造路径上恰好赋值一次。
fn definitely_assigned_once(
    class_node: &SyntaxNode,
    is_static: bool,
    writes: &[Write],
    reads: Option<&Vec<TextRange>>,
) -> bool {
    // 每次写入都得是某个构造器 / 初始化块的顶层语句，而且属于这个类
    let mut owners: Vec<SyntaxNode> = Vec::new();
    for write in writes {
        let Some(owner) = &write.owner else {
            return false;
        };
        let belongs = owner
            .parent()
            .filter(|p| p.kind() == SyntaxKind::CLASS_BODY)
            .and_then(|b| b.parent())
            .is_some_and(|c| &c == class_node);
        if !belongs {
            return false;
        }
        let static_owner = is_static_initializer(owner);
        if static_owner != is_static || (is_static && owner.kind() != SyntaxKind::INITIALIZER_BLOCK)
        {
            return false;
        }
        owners.push(owner.clone());
    }
    let count_in = |node: &SyntaxNode| owners.iter().filter(|o| *o == node).count();
    let members: Vec<SyntaxNode> = class_node
        .children()
        .find(|c| c.kind() == SyntaxKind::CLASS_BODY)
        .map(|b| b.children().collect())
        .unwrap_or_default();
    let initializers: Vec<&SyntaxNode> = members
        .iter()
        .filter(|m| {
            m.kind() == SyntaxKind::INITIALIZER_BLOCK && is_static_initializer(m) == is_static
        })
        .collect();
    let in_initializers: usize = initializers.iter().map(|i| count_in(i)).sum();
    if in_initializers > 1 {
        return false;
    }
    // 赋值之前读过、或者构造器里有 return：不提示
    let read_before = |owner: &SyntaxNode| {
        let Some(first_write) = writes
            .iter()
            .filter(|w| w.owner.as_ref() == Some(owner))
            .map(|w| w.at)
            .min()
        else {
            return false;
        };
        reads.is_some_and(|reads| {
            reads
                .iter()
                .any(|r| owner.text_range().contains_range(*r) && r.start() < first_write)
        })
    };
    if is_static {
        return in_initializers == 1 && !initializers.iter().any(|i| read_before(i));
    }
    let ctors: Vec<&SyntaxNode> = members
        .iter()
        .filter(|m| m.kind() == SyntaxKind::CONSTRUCTOR_DECL)
        .collect();
    if in_initializers == 1 {
        return ctors.iter().all(|c| count_in(c) == 0)
            && !initializers.iter().any(|i| read_before(i));
    }
    if ctors.is_empty() {
        return false;
    }
    ctors.iter().all(|ctor| {
        let has_return = ctor.descendants().any(|d| {
            d.kind() == SyntaxKind::RETURN_STMT
                && !d
                    .ancestors()
                    .take_while(|a| a != *ctor)
                    .any(|a| matches!(a.kind(), SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY))
        });
        if delegates(ctor) {
            count_in(ctor) == 0
        } else {
            count_in(ctor) == 1 && !has_return && !read_before(ctor)
        }
    })
}
