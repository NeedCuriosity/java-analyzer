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
        Definition::Field(_) | Definition::Method(_) => def.container_class().map(|class| {
            // 匿名类没有名字，显示成 `<anonymous Runnable>`
            let anonymous =
                class_data(db, class).is_some_and(|d| d.flags.contains(hir::Modifiers::ANONYMOUS));
            if anonymous {
                hir::Ty::class(class).display(db)
            } else {
                class.qualified_name(db)
            }
        }),
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

/// 光标在方法调用的名字上时，代入接收者的类型实参（`Map<String, Integer>` 上的 `get` 返回
/// `Integer`）和从调用结果反推出来的方法类型参数之后的签名。
fn instantiated_signature(
    db: &dyn Db,
    file: base_db::SourceFile,
    offset: syntax::TextSize,
    method: MethodId,
) -> Option<String> {
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let token = pick_ident_token(&root, offset)?;
    let name_ref = ast::NameRef::cast(token.parent()?)?;
    let call = ast::MethodCall::cast(name_ref.syntax().parent()?)?;
    if call.name_ref().as_ref() != Some(&name_ref) {
        return None;
    }
    let arg_list = call.arg_list()?;
    let body = sema.body_of(file, call.syntax())?;
    let result = hir::infer(db, body);
    let info = result.call_info(arg_list.syntax().text_range())?;
    let candidate = info
        .candidates
        .iter()
        .find(|c| info.chosen == Some(c.id) && c.id == method)?;
    let mut subst = candidate.subst.clone();
    let sig = hir::method_sig(db, method);
    let mut all_bound = sig.n_type_params == 0;
    if sig.n_type_params > 0 {
        if let Some(actual) = result.type_of(call.syntax().text_range()) {
            let declared = sig.ret.subst(&subst);
            bind_method_params(&declared, actual, method, &mut subst);
            all_bound = (0..sig.n_type_params).all(|idx| {
                subst.contains(hir::TypeParamId {
                    owner: hir::GenericOwner::Method(method),
                    idx,
                })
            });
        }
    }
    let rendered = ide_db::render::method_signature(db, method, &subst);
    if subst.is_empty() {
        return None;
    }
    // 方法的类型参数都定下来了：不再显示 `<R>` 这样的声明
    if all_bound && sig.n_type_params > 0 {
        let decl = ide_db::render::type_params_decl(
            db,
            hir::GenericOwner::Method(method),
            sig.n_type_params as usize,
        );
        return Some(rendered.replacen(&format!("{decl} "), "", 1));
    }
    Some(rendered)
}

/// 把声明的返回类型和调用结果的类型对齐，记下方法自己的类型参数对应的实际类型。
fn bind_method_params(
    declared: &hir::Ty,
    actual: &hir::Ty,
    method: MethodId,
    out: &mut hir::Subst,
) {
    use hir::Ty;
    match (declared, actual) {
        (Ty::TypeParam(id), actual)
            if id.owner == hir::GenericOwner::Method(method)
                && !actual.contains_unknown()
                && !out.contains(*id) =>
        {
            out.insert(*id, actual.clone());
        }
        (Ty::Class(a, a_args), Ty::Class(b, b_args)) if a == b && a_args.len() == b_args.len() => {
            for (d, x) in a_args.iter().zip(b_args.iter()) {
                bind_method_params(d, x, method, out);
            }
        }
        (Ty::Array(d), Ty::Array(x)) => bind_method_params(d, x, method, out),
        _ => {}
    }
}

pub(crate) fn hover(db: &dyn Db, position: FilePosition) -> Option<RangeInfo<HoverResult>> {
    let file = position_file(db, position)?;
    if let Some((placeholder, module)) = ide_db::spring::property_at(db, file, position.offset) {
        let found = ide_db::spring::matching_properties(db, module, &placeholder.key);
        if found.is_empty() {
            return None;
        }
        let signature = found
            .iter()
            .map(|p| format!("{} = {}", p.key, p.value))
            .collect::<Vec<_>>()
            .join("\n");
        let mut files: Vec<String> = found.iter().map(|p| p.file_name.to_string()).collect();
        files.dedup();
        return Some(RangeInfo {
            range: placeholder.key_range,
            info: HoverResult {
                signature,
                container: Some(files.join(", ")),
                docs: None,
            },
        });
    }
    if let Some((range, def)) = definition_at(db, file, position.offset) {
        let mut info = hover_for_definition(db, &def);
        // 调用处：签名换成代入了接收者和推断出的类型实参之后的样子，声明的签名放在下面
        if let Definition::Method(method) = def {
            if let Some(instantiated) = instantiated_signature(db, file, position.offset, method)
                .filter(|s| *s != info.signature)
            {
                let declared = std::mem::replace(&mut info.signature, instantiated);
                let line = base_db::tr!("声明：`{declared}`", "Declared as `{declared}`");
                info.docs = Some(match info.docs.take() {
                    Some(docs) => format!("{line}\n\n{docs}"),
                    None => line,
                });
            }
        }
        // Web 端点：服务端方法的路由，客户端方法匹配的服务端方法
        if let Some(lines) = crate::endpoints::hover_lines(db, &def) {
            info.docs = Some(match info.docs.take() {
                Some(docs) => format!("{lines}\n\n---\n\n{docs}"),
                None => lines,
            });
        }
        return Some(RangeInfo { range, info });
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
