//! 悬停：签名 + 所在位置 + Javadoc。

use base_db::Db;
use hir::{class_data, Definition, MethodId, Semantics};
use ide_db::{javadoc::javadoc_to_markdown, render::definition_signature};
use syntax::{
    algo::pick_ident_token,
    ast::{self, AstNode},
};

use crate::{
    navigation::{definition_at, position_file, super_methods},
    FilePosition, RangeInfo,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HoverResult {
    /// Java 签名（不带 Markdown 围栏）
    pub signature: String,
    /// 所在的类 / 包（点分）
    pub container: Option<String>,
    /// 已转成 Markdown 的文档
    pub docs: Option<String>,
}

impl HoverResult {
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        if let Some(container) = &self.container {
            out.push_str(&format!("*{container}*\n\n"));
        }
        if !self.signature.is_empty() {
            out.push_str("```java\n");
            out.push_str(&self.signature);
            out.push_str("\n```");
        }
        if let Some(docs) = &self.docs {
            out.push_str("\n\n---\n\n");
            out.push_str(docs);
        }
        out
    }
}

/// 定义自己的文档；方法没有文档时沿覆写链向上找。
fn docs_of(db: &dyn Db, def: &Definition) -> Option<String> {
    if let Some(raw) = def.doc_comment(db) {
        let markdown = javadoc_to_markdown(&raw);
        if !markdown.is_empty() {
            return Some(markdown);
        }
    }
    if let Definition::Method(method) = def {
        return inherited_docs(db, *method);
    }
    None
}

fn inherited_docs(db: &dyn Db, method: MethodId) -> Option<String> {
    super_methods(db, method).into_iter().find_map(|sup| {
        let raw = Definition::Method(sup).doc_comment(db)?;
        let markdown = javadoc_to_markdown(&raw);
        (!markdown.is_empty()).then_some(markdown)
    })
}

pub(crate) fn hover_for_definition(db: &dyn Db, def: &Definition) -> HoverResult {
    let container = match def {
        Definition::Class(class) => {
            let name = class.binary_name(db);
            name.rfind(['/', '$'])
                .map(|idx| name[..idx].replace(['/', '$'], "."))
        }
        Definition::Field(_) | Definition::Method(_) => {
            def.container_class().map(|class| class.qualified_name(db))
        }
        _ => None,
    };
    let mut signature = definition_signature(db, def);
    // 常量变量带上值：`static final int MAX = 100`
    let constant = match def {
        Definition::Field(field) => hir::field_constant(db, *field),
        Definition::Local(local) => hir::local_constant(db, local),
        _ => None,
    };
    if let Some(constant) = constant.filter(|_| !signature.is_empty()) {
        signature.push_str(" = ");
        signature.push_str(&constant.render());
    }
    if let Definition::Method(method) = def {
        // 注解元素等没有类数据时签名为空，退回名字
        if signature.is_empty() {
            signature = class_data(db, method.class)
                .and_then(|d| {
                    d.methods
                        .get(method.idx as usize)
                        .map(|m| m.name.to_string())
                })
                .unwrap_or_default();
        }
    }
    HoverResult {
        signature,
        container,
        docs: docs_of(db, def),
    }
}

pub(crate) fn hover(db: &dyn Db, position: FilePosition) -> Option<RangeInfo<HoverResult>> {
    let file = position_file(db, position)?;
    if let Some((range, def)) = definition_at(db, file, position.offset) {
        return Some(RangeInfo {
            range,
            info: hover_for_definition(db, &def),
        });
    }

    // 不在名字上：显示光标下最内层表达式的类型（字面量、`this`、括号表达式 …）
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let token = pick_ident_token(&root, position.offset)?;
    if token.kind().is_trivia() {
        return None;
    }
    let expr = token.parent_ancestors().find_map(ast::Expr::cast)?;
    // 只对 token 直接所属的表达式给出类型，避免在运算符上显示整个大表达式的类型
    if token.parent().as_ref() != Some(expr.syntax()) {
        return None;
    }
    let ty = sema
        .type_of_expr(file, &expr)
        .filter(|ty| !ty.is_unknown())?;
    Some(RangeInfo {
        range: expr.syntax().text_range(),
        info: HoverResult {
            signature: ty.display(db),
            container: None,
            docs: None,
        },
    })
}
