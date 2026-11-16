//! 较大的重构：内联局部变量、提取方法、提取常量、删除未使用的形参、反转 if。
//!
//! 和其它代码操作一样，只在能确定不改变语义时才提供；拿不准就不出现在列表里。

use hir::{class_data, ClassId, Definition, LocalDef, LocalKind, Ty};
use ide_db::{
    search::{find_references, references_in_file, Reference, ReferenceKind},
    TextEdit,
};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use super::{Assist, AssistCtx, AssistKind};
use crate::{rename::SourceChange, FileId};

/// 去掉选区两端的空白。
fn trimmed_selection(ctx: &AssistCtx<'_>) -> Option<TextRange> {
    if ctx.range.is_empty() {
        return None;
    }
    let selected = &ctx.text[usize::from(ctx.range.start())..usize::from(ctx.range.end())];
    let leading = selected.len() - selected.trim_start().len();
    let trailing = selected.len() - selected.trim_end().len();
    if leading + trailing >= selected.len() {
        return None;
    }
    Some(TextRange::new(
        ctx.range.start() + TextSize::from(leading as u32),
        ctx.range.end() - TextSize::from(trailing as u32),
    ))
}

/// 表达式原样放进别的表达式里不需要加括号。
pub(super) fn is_atomic(expr: &ast::Expr) -> bool {
    matches!(
        expr,
        ast::Expr::Literal(_)
            | ast::Expr::NameExpr(_)
            | ast::Expr::ThisExpr(_)
            | ast::Expr::ParenExpr(_)
            | ast::Expr::FieldAccess(_)
            | ast::Expr::MethodCall(_)
            | ast::Expr::NewExpr(_)
            | ast::Expr::ArrayAccess(_)
            | ast::Expr::ClassLiteral(_)
    )
}

/// 删除一条独占若干行的语句：连同它前面的缩进和后面的换行。
pub(super) fn statement_removal_range(text: &str, stmt: TextRange) -> TextRange {
    let start = usize::from(stmt.start());
    let end = usize::from(stmt.end());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let alone_before = text[line_start..start].trim().is_empty();
    let rest_len = text[end..].find('\n').map_or(text.len() - end, |i| i + 1);
    let alone_after = text[end..end + rest_len].trim().is_empty();
    if alone_before && alone_after {
        TextRange::new(
            TextSize::from(line_start as u32),
            TextSize::from((end + rest_len) as u32),
        )
    } else {
        stmt
    }
}

/// 删除逗号分隔列表里的第 `idx` 个元素（连同多出来的逗号）。
pub(super) fn list_element_removal(ranges: &[TextRange], idx: usize) -> Option<TextRange> {
    let current = *ranges.get(idx)?;
    Some(if idx + 1 < ranges.len() {
        TextRange::new(current.start(), ranges[idx + 1].start())
    } else if idx > 0 {
        TextRange::new(ranges[idx - 1].end(), current.end())
    } else {
        current
    })
}

// ---------------------------------------------------------------------------
// 内联局部变量
// ---------------------------------------------------------------------------

pub(super) fn inline_variable(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(parent) = token.parent() else { return };
    let def = if let Some(name) = ast::Name::cast(parent.clone()) {
        ctx.sema.classify_name(ctx.file, &name)
    } else if let Some(name_ref) = ast::NameRef::cast(parent) {
        ctx.sema.classify_name_ref(ctx.file, &name_ref)
    } else {
        None
    };
    let Some(Definition::Local(local)) = def else {
        return;
    };
    let infer = hir::infer(ctx.db, local.body);
    let data = infer.local(local.id);
    if data.kind != LocalKind::Local {
        return;
    }

    // 声明必须是 `T x = init;`：单个声明符、有初值、直接位于块里
    let Some(declarator) = ctx
        .root
        .covering_element(data.decl_range)
        .ancestors()
        .find_map(ast::VariableDeclarator::cast)
    else {
        return;
    };
    let Some(init) = declarator.initializer() else {
        return;
    };
    if matches!(
        init,
        ast::Expr::ArrayInitializer(_) | ast::Expr::LambdaExpr(_) | ast::Expr::MethodRef(_)
    ) {
        // 这些形式离开声明的目标类型就不成立了
        return;
    }
    let Some(decl) = declarator
        .syntax()
        .parent()
        .and_then(ast::LocalVarDecl::cast)
    else {
        return;
    };
    if decl.declarators().count() != 1
        || decl
            .syntax()
            .parent()
            .is_none_or(|p| p.kind() != SyntaxKind::BLOCK)
    {
        return;
    }

    let refs = references_in_file(
        ctx.db,
        ctx.file,
        &Definition::Local(local),
        data.name.as_str(),
    );
    if refs.is_empty() || refs.iter().any(|r| r.kind == ReferenceKind::Write) {
        return;
    }

    // 初值可能有副作用（方法调用、`new`、赋值……）时，内联后它必须仍然恰好求值一次：
    // 只有一处引用，而且这处引用不在条件分支、循环、lambda 里
    if !is_pure_argument(&init) {
        let Some(block) = decl.syntax().parent() else {
            return;
        };
        let [reference] = refs.as_slice() else {
            return;
        };
        let usage = ctx.root.covering_element(reference.range);
        let Some(usage) = usage
            .ancestors()
            .find(|n| n.kind() == SyntaxKind::NAME_EXPR)
        else {
            return;
        };
        if !usage.ancestors().any(|n| n == block) || !super::hoistable(&usage, &block) {
            return;
        }
    }

    // 初值里用到的局部变量在声明之后不能再被改写，否则内联会读到新值
    let after_decl = decl.syntax().text_range().end();
    for name_ref in init.syntax().descendants().filter_map(ast::NameRef::cast) {
        let Some(Definition::Local(used)) = ctx.sema.classify_name_ref(ctx.file, &name_ref) else {
            continue;
        };
        let used_name = infer.local(used.id).name.clone();
        let rewritten = references_in_file(
            ctx.db,
            ctx.file,
            &Definition::Local(used),
            used_name.as_str(),
        )
        .iter()
        .any(|r| r.kind == ReferenceKind::Write && r.range.start() >= after_decl);
        if rewritten {
            return;
        }
    }

    let init_text = init.syntax().text().to_string();
    let atomic = is_atomic(&init);
    let mut edits = vec![TextEdit::replace(
        statement_removal_range(ctx.text, decl.syntax().text_range()),
        "",
    )];
    for reference in &refs {
        let usage = ctx.root.covering_element(reference.range);
        // 引用是 NAME_REF ⊂ NAME_EXPR；看 NAME_EXPR 的父节点决定是否加括号
        let context = usage
            .ancestors()
            .find(|n| n.kind() == SyntaxKind::NAME_EXPR)
            .and_then(|n| n.parent())
            .map(|n| n.kind());
        let safe_context = matches!(
            context,
            Some(
                SyntaxKind::ARG_LIST
                    | SyntaxKind::VARIABLE_DECLARATOR
                    | SyntaxKind::RETURN_STMT
                    | SyntaxKind::EXPR_STMT
                    | SyntaxKind::PAREN_EXPR
            )
        );
        let replacement = if atomic || safe_context {
            init_text.clone()
        } else {
            format!("({init_text})")
        };
        edits.push(TextEdit::replace(reference.range, replacement));
    }
    edits.sort_by_key(|e| e.range.start());
    out.push(ctx.assist(
        "inline_variable",
        base_db::tr!("内联变量 {}", "Inline variable {}", data.name),
        AssistKind::RefactorInline,
        edits,
    ));
}

// ---------------------------------------------------------------------------
// 提取方法
// ---------------------------------------------------------------------------

/// 被提取的代码与外界交换的局部变量。
struct Flow {
    /// 在选区之前声明、选区里用到的：变成形参 `(类型, 名字)`
    params: Vec<(String, String)>,
    /// 选区里产生、选区之后还要用的值：`(类型, 名字, 是否在选区里声明)`
    output: Option<(String, String, bool)>,
}

fn analyze_flow(ctx: &AssistCtx<'_>, selection: TextRange, anchor: &SyntaxNode) -> Option<Flow> {
    let body = ctx.sema.body_of(ctx.file, anchor)?;
    let infer = hir::infer(ctx.db, body);
    let mut params = Vec::new();
    let mut outputs = Vec::new();

    for data in infer.locals() {
        if matches!(
            data.kind,
            LocalKind::LocalClassField | LocalKind::LocalClassMethod
        ) {
            continue;
        }
        let Some(id) = infer.local_at_decl(data.decl_range) else {
            continue;
        };
        let def = Definition::Local(LocalDef { body, id });
        let refs: Vec<Reference> = references_in_file(ctx.db, ctx.file, &def, data.name.as_str());
        let inside = |r: &&Reference| selection.contains_range(r.range);
        let used_after = refs.iter().any(|r| r.range.start() >= selection.end());
        let declared_inside = selection.contains_range(data.decl_range);

        if declared_inside {
            if used_after {
                outputs.push((data.ty.clone(), data.name.to_string(), true));
            }
            continue;
        }
        if data.decl_range.start() > selection.end() || !refs.iter().any(|r| inside(&r)) {
            continue;
        }
        if data.ty.contains_unknown() || matches!(data.ty, Ty::Null | Ty::Intersection(_)) {
            return None;
        }
        params.push((
            data.decl_range.start(),
            data.ty.display(ctx.db),
            data.name.to_string(),
        ));
        let written_inside = refs
            .iter()
            .filter(inside)
            .any(|r| r.kind == ReferenceKind::Write);
        if written_inside && used_after {
            outputs.push((data.ty.clone(), data.name.to_string(), false));
        }
    }
    if outputs.len() > 1 {
        return None;
    }
    let output = match outputs.pop() {
        Some((ty, _, _))
            if ty.contains_unknown() || matches!(ty, Ty::Null | Ty::Intersection(_)) =>
        {
            return None;
        }
        Some((ty, name, declared)) => Some((ty.display(ctx.db), name, declared)),
        None => None,
    };
    params.sort();
    Some(Flow {
        params: params.into_iter().map(|(_, ty, name)| (ty, name)).collect(),
        output,
    })
}

/// 选区里有没有会跳到选区之外的控制流。
fn escapes_selection(nodes: &[SyntaxNode], selection: TextRange) -> bool {
    let within = |node: &SyntaxNode, kinds: &[SyntaxKind]| {
        node.ancestors()
            .skip(1)
            .take_while(|a| selection.contains_range(a.text_range()))
            .any(|a| kinds.contains(&a.kind()))
    };
    const FUNCTIONS: &[SyntaxKind] = &[
        SyntaxKind::LAMBDA_EXPR,
        SyntaxKind::CLASS_BODY,
        SyntaxKind::METHOD_DECL,
    ];
    const LOOPS: &[SyntaxKind] = &[
        SyntaxKind::WHILE_STMT,
        SyntaxKind::DO_WHILE_STMT,
        SyntaxKind::FOR_STMT,
        SyntaxKind::FOREACH_STMT,
    ];
    const SWITCHES: &[SyntaxKind] = &[SyntaxKind::SWITCH_STMT, SyntaxKind::SWITCH_EXPR];

    nodes
        .iter()
        .flat_map(|n| n.descendants())
        .any(|node| match node.kind() {
            SyntaxKind::RETURN_STMT => !within(&node, FUNCTIONS),
            SyntaxKind::YIELD_STMT => !within(&node, &[SyntaxKind::SWITCH_EXPR]),
            SyntaxKind::CONTINUE_STMT | SyntaxKind::BREAK_STMT => {
                // 带标签的跳转不分析，直接放弃
                let labeled = node
                    .children_with_tokens()
                    .any(|e| e.kind() == SyntaxKind::IDENT)
                    || node
                        .children()
                        .any(|c| matches!(c.kind(), SyntaxKind::NAME_REF | SyntaxKind::NAME));
                let targets_inside = within(&node, LOOPS)
                    || (node.kind() == SyntaxKind::BREAK_STMT && within(&node, SWITCHES));
                labeled || (!targets_inside && !within(&node, FUNCTIONS))
            }
            _ => false,
        })
}

/// 提取出来的方法的 `throws` 子句和它需要的 import。
///
/// 能算出选中代码实际抛出、又没有在其中捕获的受检异常时用它们；有拿不准的抛出点时沿用所在
/// 方法的 `throws`。选中的代码重抛了选区之外的 catch 形参时返回 `None`：精确重抛的类型到了
/// 新方法里会变成形参声明的类型，调用处不一定处理得了。
fn extracted_throws(
    ctx: &AssistCtx<'_>,
    nodes: &[SyntaxNode],
    selection: TextRange,
    enclosing: Option<String>,
) -> Option<(Option<String>, Vec<TextEdit>)> {
    for throw in nodes
        .iter()
        .flat_map(|n| n.descendants())
        .filter_map(ast::ThrowStmt::cast)
    {
        let Some(ast::Expr::NameExpr(name)) = throw.expr() else {
            continue;
        };
        let Some(Definition::Local(local)) = name
            .name_ref()
            .and_then(|r| ctx.sema.classify_name_ref(ctx.file, &r))
        else {
            continue;
        };
        let infer = hir::infer(ctx.db, local.body);
        let data = infer.local(local.id);
        if data.kind == LocalKind::CatchParam && !selection.contains_range(data.decl_range) {
            return None;
        }
    }
    let Some(thrown) = ide_diagnostics::thrown_exceptions(ctx.db, ctx.file, nodes) else {
        return Some((enclosing, Vec::new()));
    };
    // 被别的类型涵盖的子类不用再写
    let thrown: Vec<&Ty> = thrown
        .iter()
        .filter(|ty| {
            !thrown
                .iter()
                .any(|other| other != *ty && hir::is_subtype(ctx.db, ty, other))
        })
        .collect();
    if thrown.is_empty() {
        return Some((None, Vec::new()));
    }
    let mut imports: Vec<TextEdit> = Vec::new();
    let mut names = Vec::new();
    for ty in thrown {
        if let Some(import) = super::fixes::import_for(ctx, ty, &nodes[0]) {
            if !imports.contains(&import) {
                imports.push(import);
            }
        }
        names.push(ty.display(ctx.db));
    }
    Some((Some(format!("throws {}", names.join(", "))), imports))
}

/// 新方法用到的、所在方法声明的类型参数（连同它们的上界里引用到的），按原来的写法和顺序。
fn used_type_params(
    callable: &SyntaxNode,
    nodes: &[SyntaxNode],
    signature_types: &[&str],
) -> Option<String> {
    let list = ast::MethodDecl::cast(callable.clone())
        .and_then(|m| m.type_param_list())
        .or_else(|| {
            ast::ConstructorDecl::cast(callable.clone()).and_then(|c| c.type_param_list())
        })?;
    let params: Vec<ast::TypeParam> = list.type_params().collect();
    let names: Vec<String> = params
        .iter()
        .map(|p| p.name().map(|n| n.text().to_string()).unwrap_or_default())
        .collect();
    let identifiers = |text: &str| -> Vec<String> {
        text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let mut used = vec![false; params.len()];
    let mark = |ident: &str, used: &mut [bool]| {
        if let Some(idx) = names.iter().position(|n| n == ident) {
            used[idx] = true;
        }
    };
    for text in signature_types {
        for ident in identifiers(text) {
            mark(&ident, &mut used);
        }
    }
    for ty in nodes
        .iter()
        .flat_map(|n| n.descendants())
        .filter_map(ast::ClassType::cast)
    {
        if ty.qualifier().is_none() {
            if let Some(name) = ty.name_ref() {
                mark(&name.text(), &mut used);
            }
        }
    }
    // 上界里引用的类型参数也要一起声明
    loop {
        let mut changed = false;
        for (idx, param) in params.iter().enumerate() {
            if !used[idx] {
                continue;
            }
            for bound in param.bounds() {
                for ident in identifiers(&bound.syntax().text().to_string()) {
                    if let Some(other) = names.iter().position(|n| *n == ident) {
                        if !used[other] {
                            used[other] = true;
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let declared: Vec<String> = params
        .iter()
        .zip(&used)
        .filter(|(_, used)| **used)
        .map(|(param, _)| param.syntax().text().to_string())
        .collect();
    (!declared.is_empty()).then(|| format!("<{}>", declared.join(", ")))
}

/// 包含 `node` 的方法 / 构造器：`(声明节点, 是否静态, throws 子句原文)`。
fn enclosing_callable(node: &SyntaxNode) -> Option<(SyntaxNode, bool, Option<String>)> {
    for ancestor in node.ancestors() {
        if let Some(method) = ast::MethodDecl::cast(ancestor.clone()) {
            let is_static = method
                .modifier_list()
                .is_some_and(|m| m.has(SyntaxKind::STATIC_KW));
            let throws = method
                .throws_clause()
                .map(|t| t.syntax().text().to_string());
            return Some((ancestor, is_static, throws));
        }
        if let Some(ctor) = ast::ConstructorDecl::cast(ancestor.clone()) {
            let throws = ctor.throws_clause().map(|t| t.syntax().text().to_string());
            // 构造器里提取出来的代码可能在 `this(...)` 之前运行，保守地不支持显式构造器调用
            return Some((ancestor, false, throws));
        }
        if matches!(
            ancestor.kind(),
            SyntaxKind::CLASS_BODY | SyntaxKind::LAMBDA_EXPR
        ) {
            return None;
        }
    }
    None
}

/// 把一段代码从 `old_indent` 改排到 `new_indent`。
fn reindent(text: &str, old_indent: &str, new_indent: &str) -> String {
    let mut res = String::new();
    for (idx, line) in text.lines().enumerate() {
        if idx > 0 {
            res.push('\n');
        }
        if line.trim().is_empty() {
            continue;
        }
        res.push_str(new_indent);
        res.push_str(if idx == 0 {
            line
        } else {
            line.strip_prefix(old_indent).unwrap_or(line.trim_start())
        });
    }
    res
}

pub(super) fn extract_method(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(selection) = trimmed_selection(ctx) else {
        return;
    };
    let name = "extracted";

    // 两种形态：选中若干条完整的语句，或选中一个表达式
    let covering = match ctx.root.covering_element(selection) {
        syntax::SyntaxElement::Node(node) => node,
        syntax::SyntaxElement::Token(token) => match token.parent() {
            Some(node) => node,
            None => return,
        },
    };
    let statements: Vec<SyntaxNode> = if covering.kind() == SyntaxKind::BLOCK
        && covering.text_range() != selection
    {
        covering
            .children()
            .filter(|n| ast::Stmt::can_cast(n.kind()) && selection.contains_range(n.text_range()))
            .collect()
    } else {
        covering
            .ancestors()
            .take_while(|n| n.text_range() == selection)
            .filter(|n| {
                ast::Stmt::can_cast(n.kind())
                    && n.parent().is_some_and(|p| p.kind() == SyntaxKind::BLOCK)
            })
            .take(1)
            .collect()
    };

    let exact_statements = match (statements.first(), statements.last()) {
        (Some(first), Some(last)) => {
            first.text_range().start() == selection.start()
                && last.text_range().end() == selection.end()
        }
        _ => false,
    };

    if exact_statements {
        let first = &statements[0];
        let Some((callable, is_static, throws)) = enclosing_callable(first) else {
            return;
        };
        if escapes_selection(&statements, selection) {
            return;
        }
        if statements
            .iter()
            .flat_map(|n| n.descendants())
            .any(|n| n.kind() == SyntaxKind::EXPLICIT_CONSTRUCTOR_CALL)
        {
            return;
        }
        let Some(flow) = analyze_flow(ctx, selection, first) else {
            return;
        };

        let args: Vec<&str> = flow.params.iter().map(|(_, name)| name.as_str()).collect();
        let call = format!("{name}({})", args.join(", "));
        let (ret_ty, call_stmt, tail) = match &flow.output {
            None => ("void".to_owned(), format!("{call};"), None),
            Some((ty, var, true)) => (ty.clone(), format!("{ty} {var} = {call};"), Some(var)),
            Some((ty, var, false)) => (ty.clone(), format!("{var} = {call};"), Some(var)),
        };

        let method_indent = ctx.indent_at(callable.text_range().start());
        let body_indent = format!("{method_indent}    ");
        let stmt_indent = ctx.indent_at(selection.start());
        let selected = &ctx.text[usize::from(selection.start())..usize::from(selection.end())];
        let mut body = reindent(selected, &stmt_indent, &body_indent);
        if let Some(var) = tail {
            body.push_str(&format!("\n{body_indent}return {var};"));
        }
        let Some((throws, imports)) = extracted_throws(ctx, &statements, selection, throws) else {
            return;
        };
        let signature_types: Vec<&str> = flow
            .params
            .iter()
            .map(|(ty, _)| ty.as_str())
            .chain([ret_ty.as_str()])
            .collect();
        let type_params = used_type_params(&callable, &statements, &signature_types);
        let method = render_method(
            &method_indent,
            is_static,
            type_params,
            &ret_ty,
            name,
            &flow.params,
            throws,
            &body,
        );
        let mut edits = vec![
            TextEdit::replace(selection, call_stmt),
            TextEdit::insert(callable.text_range().end(), method),
        ];
        edits.extend(imports);
        edits.sort_by_key(|e| e.range.start());
        out.push(ctx.assist(
            "extract_method",
            base_db::trs!("提取为方法", "Extract method"),
            AssistKind::RefactorExtract,
            edits,
        ));
        return;
    }

    // 表达式
    let Some(expr) = syntax::algo::find_node_at_range::<ast::Expr>(&ctx.root, selection) else {
        return;
    };
    if expr.syntax().text_range() != selection {
        return;
    }
    let Some(parent) = expr.syntax().parent() else {
        return;
    };
    if parent.kind() == SyntaxKind::ASSIGN_EXPR
        && parent.first_child().as_ref() == Some(expr.syntax())
    {
        return;
    }
    let Some((callable, is_static, throws)) = enclosing_callable(expr.syntax()) else {
        return;
    };
    let ret_ty = match ctx.sema.type_of_expr(ctx.file, &expr) {
        Some(ty)
            if !ty.contains_unknown()
                && !matches!(ty, Ty::Null | Ty::Void | Ty::Intersection(_)) =>
        {
            ty.display(ctx.db)
        }
        _ => return,
    };
    let Some(flow) = analyze_flow(ctx, selection, expr.syntax()) else {
        return;
    };
    // 表达式里声明的模式变量 / 被赋值的外部变量无法带出来
    if flow.output.is_some() {
        return;
    }
    let args: Vec<&str> = flow.params.iter().map(|(_, name)| name.as_str()).collect();
    let method_indent = ctx.indent_at(callable.text_range().start());
    let body = format!("{method_indent}    return {};", expr.syntax().text());
    let nodes = [expr.syntax().clone()];
    let Some((throws, imports)) = extracted_throws(ctx, &nodes, selection, throws) else {
        return;
    };
    let signature_types: Vec<&str> = flow
        .params
        .iter()
        .map(|(ty, _)| ty.as_str())
        .chain([ret_ty.as_str()])
        .collect();
    let type_params = used_type_params(&callable, &nodes, &signature_types);
    let method = render_method(
        &method_indent,
        is_static,
        type_params,
        &ret_ty,
        name,
        &flow.params,
        throws,
        &body,
    );
    let mut edits = vec![
        TextEdit::replace(selection, format!("{name}({})", args.join(", "))),
        TextEdit::insert(callable.text_range().end(), method),
    ];
    edits.extend(imports);
    edits.sort_by_key(|e| e.range.start());
    out.push(ctx.assist(
        "extract_method",
        base_db::trs!("提取为方法", "Extract method"),
        AssistKind::RefactorExtract,
        edits,
    ));
}

#[allow(clippy::too_many_arguments)]
fn render_method(
    indent: &str,
    is_static: bool,
    type_params: Option<String>,
    ret_ty: &str,
    name: &str,
    params: &[(String, String)],
    throws: Option<String>,
    body: &str,
) -> String {
    let params: Vec<String> = params
        .iter()
        .map(|(ty, name)| format!("{ty} {name}"))
        .collect();
    let modifiers = if is_static {
        "private static"
    } else {
        "private"
    };
    let type_params = type_params.map(|t| format!("{t} ")).unwrap_or_default();
    let throws = throws.map(|t| format!(" {t}")).unwrap_or_default();
    format!(
        "\n\n{indent}{modifiers} {type_params}{ret_ty} {name}({}){throws} {{\n{body}\n{indent}}}",
        params.join(", ")
    )
}

// ---------------------------------------------------------------------------
// 提取常量
// ---------------------------------------------------------------------------

/// 选中的、不依赖任何变量的表达式提取成 `private static final` 常量。
pub(super) fn extract_constant(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(selection) = trimmed_selection(ctx) else {
        return;
    };
    let Some(expr) = syntax::algo::find_node_at_range::<ast::Expr>(&ctx.root, selection) else {
        return;
    };
    if expr.syntax().text_range() != selection {
        return;
    }
    // 只接受由字面量和运算符组成的表达式
    let constant = expr.syntax().descendants().all(|n| {
        matches!(
            n.kind(),
            SyntaxKind::LITERAL
                | SyntaxKind::BINARY_EXPR
                | SyntaxKind::UNARY_EXPR
                | SyntaxKind::PAREN_EXPR
                | SyntaxKind::CONDITIONAL_EXPR
        )
    });
    if !constant {
        return;
    }
    let ty = match ctx.sema.type_of_expr(ctx.file, &expr) {
        Some(ty) if !ty.contains_unknown() && !matches!(ty, Ty::Null | Ty::Void) => {
            ty.display(ctx.db)
        }
        _ => return,
    };
    let Some((class, decl)) = ctx.enclosing_class() else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    // 接口里的字段写法不同；内部类（非静态）在旧语言级别下不能有静态成员
    if data.is_interface() || (data.outer.is_some() && !data.flags.is_static()) {
        return;
    }
    let name = "EXTRACTED";
    if data.fields.iter().any(|f| f.name == name) {
        return;
    }
    let line = format!(
        "private static final {ty} {name} = {};",
        expr.syntax().text()
    );
    let Some(insert) = ctx.insert_members(&decl, &[vec![line]]) else {
        return;
    };
    let mut edits = vec![TextEdit::replace(selection, name), insert];
    edits.sort_by_key(|e| e.range.start());
    out.push(ctx.assist(
        "extract_constant",
        base_db::trs!("提取为常量", "Extract constant"),
        AssistKind::RefactorExtract,
        edits,
    ));
}

// ---------------------------------------------------------------------------
// 删除未使用的形参
// ---------------------------------------------------------------------------

/// 光标在一个没被用到的形参上：从声明和所有调用处删掉它。
/// 只处理不参与覆写的方法（`private` 或 `static`），否则要改整条覆写链。
pub(super) fn remove_unused_parameter(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(name) = token.parent().and_then(ast::Name::cast) else {
        return;
    };
    let Some(param) = name.syntax().parent().and_then(ast::Param::cast) else {
        return;
    };
    if param.is_varargs() {
        return;
    }
    let Some(param_list) = param.syntax().parent().and_then(ast::ParamList::cast) else {
        return;
    };
    let Some(method_decl) = param_list.syntax().parent().and_then(ast::MethodDecl::cast) else {
        return;
    };
    if method_decl.body().is_none() {
        return;
    }
    let Some(method) = ctx.sema.method_of_decl(ctx.file, method_decl.syntax()) else {
        return;
    };
    let Some(data) = class_data(db, method.class) else {
        return;
    };
    let Some(method_data) = data.methods.get(method.idx as usize) else {
        return;
    };
    let standalone =
        method_data.flags.is_static() || method_data.flags.visibility() == hir::Visibility::Private;
    if !standalone {
        return;
    }

    let Some(Definition::Local(local)) = ctx.sema.classify_name(ctx.file, &name) else {
        return;
    };
    let param_name = name.text().to_string();
    if !references_in_file(db, ctx.file, &Definition::Local(local), &param_name).is_empty() {
        return;
    }

    let param_ranges: Vec<TextRange> = param_list
        .params()
        .map(|p| p.syntax().text_range())
        .collect();
    let Some(idx) = param_ranges
        .iter()
        .position(|r| *r == param.syntax().text_range())
    else {
        return;
    };
    let Some(decl_removal) = list_element_removal(&param_ranges, idx) else {
        return;
    };

    let mut edits: Vec<(FileId, TextEdit)> =
        vec![(ctx.file_id, TextEdit::replace(decl_removal, ""))];
    for reference in find_references(db, &Definition::Method(method)) {
        let Some(file) = base_db::source_file(db, reference.file_id) else {
            return;
        };
        let root = base_db::parse(db, file).syntax_node();
        let Some(call) = root
            .covering_element(reference.range)
            .ancestors()
            .take(3)
            .find_map(ast::MethodCall::cast)
        else {
            // 方法引用之类的用法：形参个数是隐含的，不能安全地改
            return;
        };
        let Some(arg_list) = call.arg_list() else {
            return;
        };
        let args: Vec<ast::Expr> = arg_list.args().collect();
        if args.len() != param_ranges.len() {
            return;
        }
        // 被删掉的实参不能有副作用
        let pure = args[idx].syntax().descendants().all(|n| {
            !matches!(
                n.kind(),
                SyntaxKind::METHOD_CALL
                    | SyntaxKind::NEW_EXPR
                    | SyntaxKind::ASSIGN_EXPR
                    | SyntaxKind::POSTFIX_EXPR
                    | SyntaxKind::UNARY_EXPR
            )
        });
        if !pure {
            return;
        }
        let arg_ranges: Vec<TextRange> = args.iter().map(|a| a.syntax().text_range()).collect();
        let Some(removal) = list_element_removal(&arg_ranges, idx) else {
            return;
        };
        edits.push((reference.file_id, TextEdit::replace(removal, "")));
    }

    let mut change = SourceChange::default();
    edits.sort_by_key(|(file_id, edit)| (*file_id, edit.range.start()));
    for (file_id, edit) in edits {
        match change.edits.last_mut() {
            Some((last, list)) if *last == file_id => list.push(edit),
            _ => change.edits.push((file_id, vec![edit])),
        }
    }
    out.push(Assist {
        id: "remove_unused_parameter",
        label: base_db::tr!(
            "删除未使用的形参 {param_name}",
            "Remove unused parameter {param_name}"
        ),
        kind: AssistKind::Refactor,
        change,
    });
}

// ---------------------------------------------------------------------------
// 反转 if
// ---------------------------------------------------------------------------

pub(super) fn negate(expr: &ast::Expr) -> String {
    match expr {
        ast::Expr::UnaryExpr(unary)
            if unary
                .op_token()
                .is_some_and(|t| t.kind() == SyntaxKind::BANG) =>
        {
            if let Some(operand) = unary.operand() {
                return match operand {
                    ast::Expr::ParenExpr(paren) => paren
                        .expr()
                        .map(|e| e.syntax().text().to_string())
                        .unwrap_or_default(),
                    other => other.syntax().text().to_string(),
                };
            }
        }
        ast::Expr::BinaryExpr(binary) => {
            let flipped = match binary.op_kind() {
                Some(SyntaxKind::EQ_EQ) => Some("!="),
                Some(SyntaxKind::NEQ) => Some("=="),
                Some(SyntaxKind::LT) => Some(">="),
                Some(SyntaxKind::GT) => Some("<="),
                Some(SyntaxKind::LT_EQ) => Some(">"),
                Some(SyntaxKind::GT_EQ) => Some("<"),
                _ => None,
            };
            if let (Some(op), Some(lhs), Some(rhs)) = (flipped, binary.lhs(), binary.rhs()) {
                return format!("{} {op} {}", lhs.syntax().text(), rhs.syntax().text());
            }
        }
        ast::Expr::Literal(literal) => match literal.kind() {
            Some(SyntaxKind::TRUE_KW) => return "false".to_owned(),
            Some(SyntaxKind::FALSE_KW) => return "true".to_owned(),
            _ => {}
        },
        _ => {}
    }
    if is_atomic(expr) {
        format!("!{}", expr.syntax().text())
    } else {
        format!("!({})", expr.syntax().text())
    }
}

/// `if (c) A else B` → `if (!c) B else A`。光标要在 `if` 关键字上。
pub(super) fn invert_if(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_token(&ctx.root, ctx.range.start(), |kind| {
        if kind == SyntaxKind::IF_KW {
            1
        } else {
            0
        }
    }) else {
        return;
    };
    if token.kind() != SyntaxKind::IF_KW {
        return;
    }
    let Some(if_stmt) = token.parent().and_then(ast::IfStmt::cast) else {
        return;
    };
    let (Some(condition), Some(then_branch), Some(else_branch)) = (
        if_stmt.condition(),
        if_stmt.then_branch(),
        if_stmt.else_branch(),
    ) else {
        return;
    };
    // `else if` 链反转后可读性更差，不提供
    if matches!(else_branch, ast::Stmt::IfStmt(_)) {
        return;
    }
    // `if (c)` 的括号可能被解析成括号表达式：取反里面的条件，括号留在原处
    let condition = match condition {
        ast::Expr::ParenExpr(paren) => match paren.expr() {
            Some(inner) => inner,
            None => return,
        },
        other => other,
    };
    let edits = vec![
        TextEdit::replace(condition.syntax().text_range(), negate(&condition)),
        TextEdit::replace(
            then_branch.syntax().text_range(),
            else_branch.syntax().text().to_string(),
        ),
        TextEdit::replace(
            else_branch.syntax().text_range(),
            then_branch.syntax().text().to_string(),
        ),
    ];
    out.push(ctx.assist(
        "invert_if",
        base_db::trs!("反转 if 条件", "Invert if condition"),
        AssistKind::RefactorRewrite,
        edits,
    ));
}

// ---------------------------------------------------------------------------
// 内联方法
// ---------------------------------------------------------------------------

/// 实参原样重复出现也不改变语义。
pub(super) fn is_pure_argument(expr: &ast::Expr) -> bool {
    expr.syntax().descendants().all(|n| {
        !matches!(
            n.kind(),
            SyntaxKind::METHOD_CALL
                | SyntaxKind::NEW_EXPR
                | SyntaxKind::ASSIGN_EXPR
                | SyntaxKind::POSTFIX_EXPR
                | SyntaxKind::UNARY_EXPR
                | SyntaxKind::LAMBDA_EXPR
                | SyntaxKind::ARRAY_INITIALIZER
        )
    })
}

/// 把只有一条语句的方法内联到所有调用处，并删除方法。
///
/// 只处理能确定安全的情形：方法不可被覆写（`private` / `static` / `final`）、不递归，
/// 方法体是单条 `return 表达式;` 或单条表达式语句；所有调用都在同一个类里、
/// 不带限定（或以 `this` 限定），实参没有副作用。
pub(super) fn inline_method(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(parent) = token.parent() else { return };
    let def = if let Some(name) = ast::Name::cast(parent.clone()) {
        ctx.sema.classify_name(ctx.file, &name)
    } else if let Some(name_ref) = ast::NameRef::cast(parent) {
        ctx.sema.classify_name_ref(ctx.file, &name_ref)
    } else {
        None
    };
    let Some(Definition::Method(method)) = def else {
        return;
    };
    let Some(data) = class_data(db, method.class) else {
        return;
    };
    let Some(method_data) = data.methods.get(method.idx as usize) else {
        return;
    };
    let fixed = method_data.flags.is_static()
        || method_data.flags.visibility() == hir::Visibility::Private
        || method_data.flags.contains(hir::Modifiers::FINAL)
        || data.flags.contains(hir::Modifiers::FINAL);
    if method_data.is_constructor || !fixed {
        return;
    }

    // 声明必须在当前文件里
    let Some(source) = Definition::Method(method).source(db) else {
        return;
    };
    if source.file != ctx.file {
        return;
    }
    let Some(decl) = ctx
        .root
        .covering_element(source.focus_range)
        .ancestors()
        .find_map(ast::MethodDecl::cast)
    else {
        return;
    };
    let Some(body) = decl.body() else { return };
    let mut statements = body.statements();
    let (Some(only), None) = (statements.next(), statements.next()) else {
        return;
    };
    let (result, is_void) = match &only {
        ast::Stmt::ReturnStmt(ret) => match ret.expr() {
            Some(expr) => (expr, false),
            None => return,
        },
        ast::Stmt::ExprStmt(stmt) => match stmt.syntax().children().find_map(ast::Expr::cast) {
            Some(expr) => (expr, true),
            None => return,
        },
        _ => return,
    };
    let body_range = result.syntax().text_range();
    let body_text = result.syntax().text().to_string();

    // 形参在方法体里出现的位置（相对于表达式起点）
    let params: Vec<ast::Param> = decl
        .param_list()
        .map(|l| l.params().collect())
        .unwrap_or_default();
    if params.iter().any(|p| p.is_varargs()) {
        return;
    }
    let mut param_uses: Vec<Vec<TextRange>> = Vec::new();
    for param in &params {
        let Some(name) = param.name() else { return };
        let Some(Definition::Local(local)) = ctx.sema.classify_name(ctx.file, &name) else {
            return;
        };
        let refs = references_in_file(db, ctx.file, &Definition::Local(local), &name.text());
        if refs.iter().any(|r| r.kind == ReferenceKind::Write) {
            return;
        }
        param_uses.push(
            refs.into_iter()
                .map(|r| r.range - body_range.start())
                .collect(),
        );
    }

    let references = find_references(db, &Definition::Method(method));
    if references.is_empty() {
        return;
    }
    let mut edits = vec![TextEdit::replace(
        statement_removal_range(ctx.text, decl.syntax().text_range()),
        "",
    )];
    for reference in &references {
        // 递归调用、其它文件里的调用都放弃
        if reference.file_id != ctx.file_id
            || decl.syntax().text_range().contains_range(reference.range)
        {
            return;
        }
        let Some(call) = ctx
            .root
            .covering_element(reference.range)
            .ancestors()
            .take(3)
            .find_map(ast::MethodCall::cast)
        else {
            return;
        };
        if !matches!(call.receiver(), None | Some(ast::Expr::ThisExpr(_))) {
            return;
        }
        // 调用必须位于声明方法的那个类里，方法体里的名字才指向同样的东西
        if ctx.sema.enclosing_class(ctx.file, call.syntax()) != Some(method.class) {
            return;
        }
        let args: Vec<ast::Expr> = call
            .arg_list()
            .map(|l| l.args().collect())
            .unwrap_or_default();
        if args.len() != params.len() || !args.iter().all(is_pure_argument) {
            return;
        }
        // 实参里的名字不能被方法体里的 lambda 形参之类的声明捕获：
        // 方法体只有一条表达式，出现 lambda 时保守放弃带名字的实参
        let body_declares_names = result.syntax().descendants().any(|n| {
            matches!(
                n.kind(),
                SyntaxKind::LAMBDA_EXPR | SyntaxKind::TYPE_PATTERN | SyntaxKind::SWITCH_EXPR
            )
        });
        if body_declares_names && args.iter().any(|a| !matches!(a, ast::Expr::Literal(_))) {
            return;
        }

        // 把形参替换成实参：从后往前改，偏移才不会错位
        let mut substitutions: Vec<(TextRange, String)> = Vec::new();
        for (uses, arg) in param_uses.iter().zip(&args) {
            let text = arg.syntax().text().to_string();
            let text = if is_atomic(arg) {
                text
            } else {
                format!("({text})")
            };
            substitutions.extend(uses.iter().map(|range| (*range, text.clone())));
        }
        substitutions.sort_by_key(|(range, _)| range.start());
        let mut inlined = body_text.clone();
        for (range, text) in substitutions.iter().rev() {
            inlined.replace_range(usize::from(range.start())..usize::from(range.end()), text);
        }

        let call_parent = call.syntax().parent().map(|p| p.kind());
        if is_void && call_parent != Some(SyntaxKind::EXPR_STMT) {
            return;
        }
        let safe_context = matches!(
            call_parent,
            Some(
                SyntaxKind::ARG_LIST
                    | SyntaxKind::VARIABLE_DECLARATOR
                    | SyntaxKind::RETURN_STMT
                    | SyntaxKind::EXPR_STMT
                    | SyntaxKind::PAREN_EXPR
            )
        );
        // 语句位置上只能放"语句表达式"：`a + b;` 不合法
        if call_parent == Some(SyntaxKind::EXPR_STMT)
            && !matches!(
                result,
                ast::Expr::MethodCall(_)
                    | ast::Expr::AssignExpr(_)
                    | ast::Expr::NewExpr(_)
                    | ast::Expr::PostfixExpr(_)
            )
        {
            return;
        }
        let replacement = if is_atomic(&result) || safe_context {
            inlined
        } else {
            format!("({inlined})")
        };
        edits.push(TextEdit::replace(call.syntax().text_range(), replacement));
    }
    edits.sort_by_key(|e| e.range.start());
    out.push(ctx.assist(
        "inline_method",
        base_db::tr!("内联方法 {}", "Inline method {}", method_data.name),
        AssistKind::RefactorInline,
        edits,
    ));
}

// ---------------------------------------------------------------------------
// 修改签名：移动形参
// ---------------------------------------------------------------------------

pub(super) fn group_edits(mut edits: Vec<(FileId, TextEdit)>) -> SourceChange {
    let mut change = SourceChange::default();
    edits.sort_by_key(|(file_id, edit)| (*file_id, edit.range.start()));
    for (file_id, edit) in edits {
        match change.edits.last_mut() {
            Some((last, list)) if *last == file_id => list.push(edit),
            _ => change.edits.push((file_id, vec![edit])),
        }
    }
    change
}

/// 方法不参与覆写（静态、私有、final，或所在的类是 final）：改它的签名只需要改调用点。
fn is_standalone_method(db: &dyn base_db::Db, method: hir::MethodId) -> bool {
    let Some(data) = class_data(db, method.class) else {
        return false;
    };
    let Some(method_data) = data.methods.get(method.idx as usize) else {
        return false;
    };
    !method_data.is_constructor
        && (method_data.flags.is_static()
            || method_data.flags.visibility() == hir::Visibility::Private
            || method_data.flags.contains(hir::Modifiers::FINAL)
            || data.flags.contains(hir::Modifiers::FINAL))
}

/// 光标在形参上：把它和左边 / 右边的形参交换位置，同时调整所有调用点的实参。
pub(super) fn move_parameter(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(param) = token.parent_ancestors().find_map(ast::Param::cast) else {
        return;
    };
    let Some(param_list) = param.syntax().parent().and_then(ast::ParamList::cast) else {
        return;
    };
    let Some(method_decl) = param_list.syntax().parent().and_then(ast::MethodDecl::cast) else {
        return;
    };
    let Some(method) = ctx.sema.method_of_decl(ctx.file, method_decl.syntax()) else {
        return;
    };
    if !is_standalone_method(db, method) {
        return;
    }
    let params: Vec<ast::Param> = param_list.params().collect();
    let Some(idx) = params.iter().position(|p| p.syntax() == param.syntax()) else {
        return;
    };
    // 交换之后不能和同名、参数个数相同的重载混淆
    let overloaded = class_data(db, method.class).is_some_and(|data| {
        let this = &data.methods[method.idx as usize];
        data.methods
            .iter()
            .filter(|m| m.name == this.name && m.params.len() == this.params.len())
            .count()
            > 1
    });
    if overloaded {
        return;
    }
    let references = find_references(db, &Definition::Method(method));

    for (other, id, label) in [
        (
            idx.checked_sub(1),
            "move_parameter_left",
            base_db::trs!("向左移动形参", "Move parameter left"),
        ),
        (
            (idx + 1 < params.len()).then_some(idx + 1),
            "move_parameter_right",
            base_db::trs!("向右移动形参", "Move parameter right"),
        ),
    ] {
        let Some(other) = other else { continue };
        if params[idx].is_varargs() || params[other].is_varargs() {
            continue;
        }
        let (first, second) = (idx.min(other), idx.max(other));
        let swap = |ranges: &[TextRange], text: &str, file_id: FileId| {
            let a = ranges[first];
            let b = ranges[second];
            let text_of =
                |r: TextRange| text[usize::from(r.start())..usize::from(r.end())].to_owned();
            vec![
                (file_id, TextEdit::replace(a, text_of(b))),
                (file_id, TextEdit::replace(b, text_of(a))),
            ]
        };
        let param_ranges: Vec<TextRange> = params.iter().map(|p| p.syntax().text_range()).collect();
        let mut edits = swap(&param_ranges, ctx.text, ctx.file_id);
        let mut ok = true;
        for reference in &references {
            let Some(file) = base_db::source_file(db, reference.file_id) else {
                ok = false;
                break;
            };
            let root = base_db::parse(db, file).syntax_node();
            let call = root
                .covering_element(reference.range)
                .ancestors()
                .take(3)
                .find_map(ast::MethodCall::cast);
            let args: Vec<ast::Expr> = call
                .and_then(|c| c.arg_list())
                .map(|l| l.args().collect())
                .unwrap_or_default();
            // 方法引用、实参个数对不上的调用：不能安全地改。交换会改变求值顺序，实参必须没有副作用
            if args.len() != params.len()
                || !is_pure_argument(&args[first])
                || !is_pure_argument(&args[second])
            {
                ok = false;
                break;
            }
            let arg_ranges: Vec<TextRange> = args.iter().map(|a| a.syntax().text_range()).collect();
            let text: &str = file.text(db);
            edits.extend(swap(&arg_ranges, text, reference.file_id));
        }
        if !ok {
            continue;
        }
        out.push(Assist {
            id,
            label: label.to_owned(),
            kind: AssistKind::Refactor,
            change: group_edits(edits),
        });
    }
}

// ---------------------------------------------------------------------------
// 上移方法
// ---------------------------------------------------------------------------

/// 光标在方法名上：把方法移到父类链上的某个类里（必须是工作区里的源码）。
pub(super) fn pull_up_method(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(name) = token.parent().and_then(ast::Name::cast) else {
        return;
    };
    let Some(method_decl) = name.syntax().parent().and_then(ast::MethodDecl::cast) else {
        return;
    };
    let Some(body) = method_decl.body() else {
        return;
    };
    let Some(method) = ctx.sema.method_of_decl(ctx.file, method_decl.syntax()) else {
        return;
    };
    let class = method.class;
    let Some(data) = class_data(db, class) else {
        return;
    };
    let method_data = &data.methods[method.idx as usize];
    if method_data.is_constructor || !data.type_params.is_empty() {
        return;
    }
    // 父类链上的源码普通类（遇到泛型类为止）：可以上移到其中任何一个
    let mut chain: Vec<ClassId> = Vec::new();
    let mut current = class;
    while let Some(parent) = hir::direct_supers(db, current).iter().copied().find(|sup| {
        class_data(db, *sup).is_some_and(|d| !d.is_interface())
            && hir::class_source_file(db, *sup).is_some()
    }) {
        if chain.contains(&parent)
            || class_data(db, parent).is_none_or(|d| !d.type_params.is_empty())
        {
            break;
        }
        chain.push(parent);
        current = parent;
    }
    for (i, &parent) in chain.iter().enumerate() {
        // 方法不能用到被跳过的中间类的成员；中间类里也不能有同名方法（否则会继承到它的版本）
        let mut own = vec![class];
        own.extend_from_slice(&chain[..i]);
        if let Some(assist) = pull_up_to(ctx, &method_decl, &body, method, &own, parent) {
            out.push(assist);
        }
    }
}

/// 把方法上移到 `parent`。`own` 是方法体不能依赖的类：方法所在的类，以及它和 `parent` 之间的类。
fn pull_up_to(
    ctx: &AssistCtx<'_>,
    method_decl: &ast::MethodDecl,
    body: &ast::Block,
    method: hir::MethodId,
    own: &[ClassId],
    parent: ClassId,
) -> Option<Assist> {
    let db = ctx.db;
    let class = method.class;
    let data = class_data(db, class)?;
    let method_data = &data.methods[method.idx as usize];
    let parent_data = class_data(db, parent)?;
    let same_method = |m: &hir::MethodData| {
        m.name == method_data.name && m.params.len() == method_data.params.len()
    };
    if parent_data.methods.iter().any(same_method)
        || own[1..]
            .iter()
            .any(|c| class_data(db, *c).is_some_and(|d| d.methods.iter().any(same_method)))
    {
        return None;
    }
    let parent_source = Definition::Class(parent).source(db)?;
    let parent_file = parent_source.file;
    let parent_file_id = parent_file.file_id(db);
    let parent_root = base_db::parse(db, parent_file).syntax_node();
    let parent_decl = parent_root
        .covering_element(parent_source.focus_range)
        .ancestors()
        .find_map(ast::TypeDecl::cast)?;

    // 方法体不能依赖子类自己的成员
    let body_id = ctx.sema.body_of(ctx.file, body.syntax())?;
    let result = hir::infer(db, body_id);
    let uses_own_members = result.resolutions().any(|(range, resolution)| {
        method_decl.syntax().text_range().contains_range(range)
            && match resolution {
                hir::Resolution::Field(field) => own.contains(&field.class),
                hir::Resolution::Method(m) => own.contains(&m.class) && *m != method,
                hir::Resolution::Class(c) => {
                    own.contains(c)
                        || class_data(db, *c).is_some_and(|d| {
                            own.iter().any(|o| {
                                class_data(db, *o).is_some_and(|od| {
                                    d.outer.as_deref() == Some(od.binary_name.as_str())
                                })
                            })
                        })
                }
                _ => false,
            }
    });
    // `this` 只允许作为接收者出现（它的类型会从子类变成父类）
    let bare_this = method_decl.syntax().descendants().any(|n| {
        n.kind() == SyntaxKind::THIS_EXPR
            && n.parent().is_none_or(|p| {
                !matches!(p.kind(), SyntaxKind::FIELD_ACCESS | SyntaxKind::METHOD_CALL)
            })
    });
    let uses_super = method_decl
        .syntax()
        .descendants()
        .any(|n| n.kind() == SyntaxKind::SUPER_EXPR);
    if uses_own_members || bare_this || uses_super {
        return None;
    }

    // 方法里用到的类型在父类的文件里也要能用简单名写出来，否则补 import
    let mut edits: Vec<(FileId, TextEdit)> = Vec::new();
    if parent_file != ctx.file {
        let parent_scope = hir::TypeScope::class(db, parent);
        let mut imported: Vec<String> = Vec::new();
        for name_ref in method_decl
            .syntax()
            .descendants()
            .filter_map(ast::NameRef::cast)
        {
            // `new Foo()` 里的类名解析到的是构造器
            let used = match ctx.sema.classify_name_ref(ctx.file, &name_ref) {
                Some(Definition::Class(used)) => used,
                Some(Definition::Method(ctor))
                    if class_data(db, ctor.class).is_some_and(|d| {
                        d.methods
                            .get(ctor.idx as usize)
                            .is_some_and(|m| m.is_constructor)
                    }) =>
                {
                    ctor.class
                }
                _ => continue,
            };
            // 只看简单名（限定名的第一段）
            let simple = name_ref.syntax().parent().is_some_and(|p| match p.kind() {
                SyntaxKind::CLASS_TYPE => {
                    ast::ClassType::cast(p).is_some_and(|t| t.qualifier().is_none())
                }
                SyntaxKind::NAME_EXPR => true,
                _ => false,
            });
            if !simple {
                continue;
            }
            let visible = matches!(
                hir::resolve_simple_type(db, &parent_scope, &name_ref.text()),
                Some(hir::TypeNs::Class(found)) if found == used
            );
            if visible {
                continue;
            }
            let qualified = ide_db::imports::importable_name(db, used)?;
            if !imported.contains(&qualified) {
                imported.push(qualified);
            }
        }
        // 多条 import 插在同一个位置，合并成一处修改
        imported.sort();
        if let Some(first) = imported.first() {
            let mut edit = ide_db::imports::insert_import(db, parent_file, first);
            let all: Vec<String> = imported.iter().map(|q| format!("import {q};")).collect();
            edit.new_text = edit
                .new_text
                .replace(&format!("import {first};"), &all.join("\n"));
            edits.push((parent_file_id, edit));
        }
    }

    // 文本：去掉原来的缩进，按父类的缩进重排；private 改成 protected，子类才能继续调用
    let own_indent = ctx.indent_at(method_decl.syntax().text_range().start());
    let mut text = method_decl.syntax().text().to_string();
    if let Some(private) = method_decl
        .modifier_list()
        .and_then(|m| m.keywords().find(|k| k.kind() == SyntaxKind::PRIVATE_KW))
    {
        let start =
            usize::from(private.text_range().start() - method_decl.syntax().text_range().start());
        text.replace_range(start..start + "private".len(), "protected");
    }
    let parent_text: &str = parent_file.text(db);
    let parent_name = parent_decl.name()?;
    let name_start = usize::from(parent_name.syntax().text_range().start());
    let line_start = parent_text[..name_start].rfind('\n').map_or(0, |i| i + 1);
    let parent_indent: String = parent_text[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let inner = format!("{parent_indent}    ");
    let mut rendered = String::from("\n");
    for (i, line) in text.lines().enumerate() {
        let line = if i == 0 {
            line
        } else {
            line.strip_prefix(own_indent.as_str())
                .unwrap_or(line.trim_start())
        };
        if !line.trim().is_empty() {
            rendered.push_str(&inner);
            rendered.push_str(line);
        }
        rendered.push('\n');
    }
    let close = parent_decl
        .body()
        .and_then(|b| b.syntax().last_token())
        .filter(|t| t.kind() == SyntaxKind::R_CURLY)?;
    let close_start = usize::from(close.text_range().start());
    let close_line = parent_text[..close_start].rfind('\n').map_or(0, |i| i + 1);
    let insert = if parent_text[close_line..close_start].trim().is_empty() && close_line > 0 {
        TextEdit::insert(TextSize::from(close_line as u32), rendered)
    } else {
        TextEdit::insert(
            close.text_range().start(),
            format!("{rendered}{parent_indent}"),
        )
    };
    edits.push((parent_file_id, insert));
    // 删掉方法，连同它前面的一个空行（否则会留下两个连着的空行）
    let mut removal = statement_removal_range(ctx.text, method_decl.syntax().text_range());
    let before = &ctx.text[..usize::from(removal.start())];
    let after = &ctx.text[usize::from(removal.end())..];
    let blank_after = after.lines().next().is_some_and(|l| l.trim().is_empty());
    if blank_after && before.ends_with("\n\n") {
        removal = TextRange::new(removal.start() - TextSize::from(1), removal.end());
    }
    edits.push((ctx.file_id, TextEdit::replace(removal, "")));
    Some(Assist {
        id: "pull_up_method",
        label: base_db::tr!(
            "把 {} 上移到 {}",
            "Pull {} up to {}",
            method_data.name,
            parent_data.name
        ),
        kind: AssistKind::Refactor,
        change: group_edits(edits),
    })
}

// ---------------------------------------------------------------------------
// 匿名类 → lambda
// ---------------------------------------------------------------------------

/// 光标在只实现了一个方法的匿名类上，且它的类型是函数式接口：换成 lambda。
pub(super) fn anonymous_to_lambda(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    // 只在 `new 类型(...)` 这一段上提供，类体里面不提供
    let Some(new_expr) = token.parent_ancestors().find_map(ast::NewExpr::cast) else {
        return;
    };
    let Some(class_body) = new_expr.body() else {
        return;
    };
    if class_body.syntax().text_range().contains_range(ctx.range)
        && ctx.range.start() > class_body.syntax().text_range().start()
    {
        return;
    }
    if new_expr
        .arg_list()
        .is_some_and(|l| l.args().next().is_some())
    {
        return;
    }
    let members: Vec<SyntaxNode> = class_body
        .syntax()
        .children()
        .filter(|c| !c.kind().is_trivia())
        .collect();
    let [only] = members.as_slice() else { return };
    let Some(method) = ast::MethodDecl::cast(only.clone()) else {
        return;
    };
    let (Some(method_name), Some(body)) = (method.name(), method.body()) else {
        return;
    };
    let Some(ty) = ctx
        .sema
        .type_of_expr(ctx.file, &ast::Expr::NewExpr(new_expr.clone()))
    else {
        return;
    };
    let Some(functional) = hir::functional_method(db, &ty) else {
        return;
    };
    let matches_interface = class_data(db, functional.id.class)
        .and_then(|d| {
            d.methods
                .get(functional.id.idx as usize)
                .map(|m| m.name.clone())
        })
        .is_some_and(|name| name == method_name.text());
    if !matches_interface {
        return;
    }
    // lambda 里没有自己的 this，也不能递归调用自己
    let name_text = method_name.text();
    let unsupported = body.syntax().descendants().any(|n| match n.kind() {
        SyntaxKind::THIS_EXPR | SyntaxKind::SUPER_EXPR => true,
        SyntaxKind::METHOD_CALL => ast::MethodCall::cast(n).is_some_and(|call| {
            call.receiver().is_none() && call.name_ref().is_some_and(|r| r.text() == name_text)
        }),
        _ => false,
    });
    if unsupported {
        return;
    }
    let params: Vec<String> = method
        .param_list()
        .map(|l| {
            l.params()
                .filter_map(|p| p.name())
                .map(|n| n.text().to_string())
                .collect()
        })
        .unwrap_or_default();
    // lambda 的形参不能和外面的局部变量重名
    let enclosing = new_expr.syntax().ancestors().find(|a| {
        matches!(
            a.kind(),
            SyntaxKind::METHOD_DECL | SyntaxKind::CONSTRUCTOR_DECL | SyntaxKind::INITIALIZER_BLOCK
        )
    });
    let clashes = enclosing.is_some_and(|callable| {
        callable
            .descendants()
            .filter(|n| {
                !class_body
                    .syntax()
                    .text_range()
                    .contains_range(n.text_range())
            })
            .filter(|n| {
                matches!(
                    n.kind(),
                    SyntaxKind::VARIABLE_DECLARATOR | SyntaxKind::PARAM
                )
            })
            .filter_map(|n| n.children().find_map(ast::Name::cast))
            .any(|n| params.iter().any(|p| *p == n.text()))
    });
    if clashes {
        return;
    }

    let head = match params.as_slice() {
        [single] => single.clone(),
        many => format!("({})", many.join(", ")),
    };
    let statements: Vec<ast::Stmt> = body.statements().collect();
    let lambda_body = match statements.as_slice() {
        [ast::Stmt::ReturnStmt(ret)] if ret.expr().is_some() => ret
            .expr()
            .map(|e| e.syntax().text().to_string())
            .unwrap_or_default(),
        [ast::Stmt::ExprStmt(stmt)] => stmt
            .syntax()
            .children()
            .find_map(ast::Expr::cast)
            .map(|e| e.syntax().text().to_string())
            .unwrap_or_default(),
        _ => {
            // 块：方法体原来比 `new` 所在的行深两级，去掉多出来的一级缩进
            let text = body.syntax().text().to_string();
            text.lines()
                .enumerate()
                .map(|(i, line)| {
                    if i == 0 {
                        line.to_owned()
                    } else {
                        line.strip_prefix("    ").unwrap_or(line).to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    out.push(ctx.assist(
        "anonymous_to_lambda",
        base_db::trs!("换成 lambda 表达式", "Convert to lambda"),
        AssistKind::RefactorRewrite,
        vec![TextEdit::replace(
            new_expr.syntax().text_range(),
            format!("{head} -> {lambda_body}"),
        )],
    ));
}
