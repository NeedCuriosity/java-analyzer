//! 可以改成局部变量的私有字段：只在一个方法（或构造器）里用到，而且在那里的第一次使用是
//! 方法体顶层的一条赋值语句（`f = …;` / `this.f = …;`），之后才读它。每次调用都会先赋值，
//! 字段并不在调用之间保存状态。
//!
//! 字段带注解、有初值、是 `volatile` / `transient`、所在的类可序列化或带注解，或者在
//! lambda / 匿名类 / 局部类里用到、通过别的对象访问（`other.f`）时不报。

use base_db::{Db, SourceFile};
use hir::{body_syntax, class_data, infer, BodyId, FieldId, Modifiers, Resolution, Ty};
use syntax::{
    ast::{self, AstNode},
    SmolStr, SyntaxKind, SyntaxNode, TextRange,
};

use crate::{file_bodies, Diagnostic, DiagnosticCode, FileBody, Severity};

/// 一个可以改成局部变量的字段。
pub struct FieldToLocal {
    /// 字段声明（整条 `private int x;`）
    pub field_decl: TextRange,
    /// 字段名
    pub name_range: TextRange,
    /// 字段类型的原文
    pub ty: String,
    /// 第一次赋值的语句
    pub first_assignment: TextRange,
    /// 方法里所有的使用（名字或 `this.f` 整个表达式）
    pub uses: Vec<TextRange>,
}

struct Candidate {
    field: FieldId,
    decl: ast::FieldDecl,
    name: ast::Name,
    ty: String,
}

fn candidates(db: &dyn Db, file: SourceFile, root: &SyntaxNode) -> Vec<Candidate> {
    let sema = hir::Semantics::new(db);
    let serializable = hir::lookup_class(db, "java/io/Serializable");
    let mut res = Vec::new();
    for decl in root.descendants().filter_map(ast::FieldDecl::cast) {
        let declarators: Vec<_> = decl.declarators().collect();
        let [declarator] = declarators.as_slice() else {
            continue;
        };
        if declarator.initializer().is_some() {
            continue;
        }
        let Some(mods) = decl.modifier_list() else {
            continue;
        };
        if !mods.has(SyntaxKind::PRIVATE_KW) || mods.annotations().next().is_some() {
            continue;
        }
        let annotated_class = decl.syntax().ancestors().any(|a| {
            ast::TypeDecl::cast(a)
                .and_then(|d| d.modifier_list())
                .is_some_and(|m| m.annotations().next().is_some())
        });
        if annotated_class {
            continue;
        }
        let Some(BodyId::Field(field)) = sema.body_of(file, declarator.syntax()) else {
            continue;
        };
        let Some(flags) = class_data(db, field.class)
            .and_then(|d| d.fields.get(field.idx as usize).map(|f| f.flags))
        else {
            continue;
        };
        if flags.contains(Modifiers::VOLATILE) || flags.contains(Modifiers::TRANSIENT) {
            continue;
        }
        if serializable.is_some_and(|s| hir::find_super(db, &Ty::class(field.class), s).is_some()) {
            continue;
        }
        let (Some(name), Some(ty)) = (declarator.name(), decl.ty()) else {
            continue;
        };
        let ty = format!(
            "{}{}",
            ty.syntax().text(),
            "[]".repeat(declarator.extra_dims())
        );
        res.push(Candidate {
            field,
            decl,
            name,
            ty,
        });
    }
    res
}

pub fn fields_to_locals(db: &dyn Db, file: SourceFile) -> Vec<FieldToLocal> {
    fields_to_locals_in(db, file, &file_bodies(db, file))
}

fn fields_to_locals_in(db: &dyn Db, file: SourceFile, bodies: &[FileBody]) -> Vec<FieldToLocal> {
    // 候选字段都是 private 的
    if !file.text(db).contains("private") {
        return Vec::new();
    }
    let root = base_db::parse(db, file).syntax_node();
    let candidates = candidates(db, file, &root);
    if candidates.is_empty() {
        return Vec::new();
    }
    let names: Vec<SmolStr> = candidates.iter().map(|c| c.name.text()).collect();
    // 每个字段：用到它的体，以及在体里的使用（表达式节点）
    let mut uses: Vec<Vec<(BodyId, SyntaxNode)>> = candidates.iter().map(|_| Vec::new()).collect();
    let mut rejected: Vec<bool> = vec![false; candidates.len()];
    for body in bodies {
        // 源码里一个字段名都没出现的体不必逐个看名字引用
        let body_text = body.text(db);
        if !names.iter().any(|name| body_text.contains(name.as_str())) {
            continue;
        }
        let mut result = None;
        for name_ref in body.node.descendants().filter_map(ast::NameRef::cast) {
            let token = name_ref.ident_token();
            let text = token.as_ref().map_or("", |t| t.text());
            let Some(index) = names.iter().position(|name| name == text) else {
                continue;
            };
            let result = result.get_or_insert_with(|| infer(db, body.id));
            let Some(Resolution::Field(found)) = result.resolution(name_ref.syntax().text_range())
            else {
                continue;
            };
            if *found != candidates[index].field {
                continue;
            }
            let Some(parent) = name_ref.syntax().parent() else {
                continue;
            };
            let expr = match parent.kind() {
                SyntaxKind::NAME_EXPR => parent,
                SyntaxKind::FIELD_ACCESS => {
                    let this_receiver = ast::FieldAccess::cast(parent.clone())
                        .and_then(|a| a.receiver())
                        .is_some_and(
                            |r| matches!(r, ast::Expr::ThisExpr(t) if t.qualifier().is_none()),
                        );
                    if !this_receiver {
                        rejected[index] = true;
                        continue;
                    }
                    parent
                }
                _ => {
                    rejected[index] = true;
                    continue;
                }
            };
            uses[index].push((body.id, expr));
        }
    }
    let mut res = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if rejected[index] {
            continue;
        }
        let list = &uses[index];
        let Some((body, _)) = list.first() else {
            continue;
        };
        if list.iter().any(|(b, _)| b != body) || !matches!(body, BodyId::Method(_)) {
            continue;
        }
        let Some((_, method_node)) = body_syntax(db, *body) else {
            continue;
        };
        let Some(block) = method_node
            .children()
            .find(|c| c.kind() == SyntaxKind::BLOCK)
        else {
            continue;
        };
        // 在 lambda / 匿名类里用到时，局部变量可能不是事实上的 final
        if list.iter().any(|(_, e)| {
            e.ancestors()
                .take_while(|a| a != &block)
                .any(|a| matches!(a.kind(), SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY))
        }) {
            continue;
        }
        // 方法里已经有同名的局部变量 / 形参
        let name = candidate.name.text().to_string();
        let shadowed = method_node
            .descendants()
            .filter_map(ast::Name::cast)
            .any(|n| {
                n.text() == name.as_str()
                    && n.syntax().parent().is_some_and(|p| {
                        matches!(
                            p.kind(),
                            SyntaxKind::VARIABLE_DECLARATOR | SyntaxKind::PARAM
                        )
                    })
            });
        if shadowed {
            continue;
        }
        let mut sorted: Vec<&SyntaxNode> = list.iter().map(|(_, e)| e).collect();
        sorted.sort_by_key(|e| e.text_range().start());
        let first = sorted[0];
        // 第一次使用：方法体顶层的 `f = …;`，右边不能再用到 f
        let Some(assign) = first.parent().and_then(ast::AssignExpr::cast) else {
            continue;
        };
        let is_lhs = assign.lhs().is_some_and(|l| l.syntax() == first);
        let plain = assign
            .op_token()
            .is_some_and(|t| t.kind() == SyntaxKind::EQ);
        let Some(stmt) = assign
            .syntax()
            .parent()
            .filter(|p| p.kind() == SyntaxKind::EXPR_STMT)
        else {
            continue;
        };
        if !is_lhs || !plain || stmt.parent().as_ref() != Some(&block) {
            continue;
        }
        if assign.rhs().is_some_and(|r| {
            sorted
                .iter()
                .any(|u| r.syntax().text_range().contains_range(u.text_range()))
        }) {
            continue;
        }
        res.push(FieldToLocal {
            field_decl: candidate.decl.syntax().text_range(),
            name_range: candidate.name.syntax().text_range(),
            ty: candidate.ty.clone(),
            first_assignment: stmt.text_range(),
            uses: sorted.iter().map(|e| e.text_range()).collect(),
        });
    }
    res
}

pub(crate) fn field_can_be_local(
    db: &dyn Db,
    file: SourceFile,
    bodies: &[FileBody],
    out: &mut Vec<Diagnostic>,
) {
    let text: &str = file.text(db);
    for found in fields_to_locals_in(db, file, bodies) {
        let name = &text[found.name_range];
        out.push(Diagnostic {
            range: found.name_range,
            message: base_db::tr!(
                "字段 `{name}` 可以改成局部变量",
                "Field `{name}` can be converted to a local variable"
            ),
            severity: Severity::Warning,
            code: DiagnosticCode::FieldCanBeLocal,
            unused: false,
        });
    }
}
