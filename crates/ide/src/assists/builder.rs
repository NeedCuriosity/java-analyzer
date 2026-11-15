//! 生成 Builder：光标在构造器名（或 record 名）上时，生成静态嵌套类 `Builder`（每个形参一个
//! 链式设置方法和 `build()`）以及静态工厂方法 `builder()`。

use hir::{class_data, ClassKind, Modifiers};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind,
};

use super::{Assist, AssistCtx, AssistKind};

struct Param {
    name: String,
    /// 字段类型（变长参数换成数组）
    ty: String,
    /// 设置方法的形参类型（保留 `...`）
    param_ty: String,
}

fn params_of(list: &ast::ParamList) -> Option<Vec<Param>> {
    list.params()
        .map(|p| {
            let name = p.name()?.text().to_string();
            let base = format!("{}{}", p.ty()?.syntax().text(), "[]".repeat(p.extra_dims()));
            Some(if p.is_varargs() {
                Param {
                    name,
                    ty: format!("{base}[]"),
                    param_ty: format!("{base}..."),
                }
            } else {
                Param {
                    name,
                    ty: base.clone(),
                    param_ty: base,
                }
            })
        })
        .collect()
}

pub(super) fn generate_builder(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(name_node) = token.parent().filter(|p| p.kind() == SyntaxKind::NAME) else {
        return;
    };
    let Some(owner) = name_node.parent() else {
        return;
    };
    // 构造器：形参来自构造器；record：来自记录组件
    let (decl, list) = match owner.kind() {
        SyntaxKind::CONSTRUCTOR_DECL => {
            let Some(ctor) = ast::ConstructorDecl::cast(owner.clone()) else {
                return;
            };
            let Some(decl) = owner
                .parent()
                .and_then(|b| b.parent())
                .and_then(ast::TypeDecl::cast)
            else {
                return;
            };
            (decl, ctor.param_list())
        }
        _ => {
            let Some(decl) = ast::TypeDecl::cast(owner.clone()) else {
                return;
            };
            let list = decl.record_components();
            if list.is_none() {
                return;
            }
            (decl, list)
        }
    };
    let Some(list) = list else {
        return;
    };
    let Some(params) = params_of(&list) else {
        return;
    };
    if params.is_empty() {
        return;
    }
    let Some(class) = ctx.sema.class_of_decl(ctx.file, &decl) else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    // 非静态内部类需要外部实例，抽象类不能实例化
    let inner = data.outer.is_some() && !data.flags.is_static() && data.kind == ClassKind::Class;
    if inner
        || data.flags.contains(Modifiers::ABSTRACT)
        || !matches!(data.kind, ClassKind::Class | ClassKind::Record)
    {
        return;
    }
    let has_builder = decl.body().is_some_and(|body| {
        body.syntax()
            .children()
            .filter_map(ast::TypeDecl::cast)
            .any(|t| t.name().is_some_and(|n| n.text() == "Builder"))
    });
    if has_builder {
        return;
    }
    let class_name = data.name.to_string();
    let type_params = decl
        .type_param_list()
        .map(|l| l.syntax().text().to_string())
        .unwrap_or_default();
    let type_args = decl
        .type_param_list()
        .map(|l| {
            let names: Vec<String> = l
                .type_params()
                .filter_map(|p| p.name().map(|n| n.text().to_string()))
                .collect();
            format!("<{}>", names.join(", "))
        })
        .unwrap_or_default();
    let factory_params = if type_params.is_empty() {
        String::new()
    } else {
        format!("{type_params} ")
    };
    let factory = vec![
        format!("public static {factory_params}Builder{type_args} builder() {{"),
        format!("    return new Builder{type_args}();"),
        "}".to_owned(),
    ];
    let mut builder = vec![format!("public static final class Builder{type_params} {{")];
    for p in &params {
        builder.push(format!("    private {} {};", p.ty, p.name));
    }
    builder.push(String::new());
    builder.push("    private Builder() {".to_owned());
    builder.push("    }".to_owned());
    for p in &params {
        builder.push(String::new());
        builder.push(format!(
            "    public Builder{type_args} {0}({1} {0}) {{",
            p.name, p.param_ty
        ));
        builder.push(format!("        this.{0} = {0};", p.name));
        builder.push("        return this;".to_owned());
        builder.push("    }".to_owned());
    }
    builder.push(String::new());
    let args: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    builder.push(format!("    public {class_name}{type_args} build() {{"));
    builder.push(format!(
        "        return new {class_name}{type_args}({});",
        args.join(", ")
    ));
    builder.push("    }".to_owned());
    builder.push("}".to_owned());
    let Some(edit) = ctx.insert_members(&decl, &[factory, builder]) else {
        return;
    };
    out.push(ctx.assist(
        "generate_builder",
        base_db::trs!("生成 Builder", "Generate builder"),
        AssistKind::Generate,
        vec![edit],
    ));
}
