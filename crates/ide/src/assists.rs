//! 代码操作：快速修复、生成代码、小型重构。
//!
//! 每个操作是一个函数：看光标 / 选区处的语法和语义，适用时返回一个带完整修改的 [`Assist`]。
//! 操作都很便宜（只看当前文件），所以直接算出修改，不做延迟解析。

use base_db::{Db, SourceFile};
use hir::{
    all_methods, class_data, is_accessible, this_ty, ClassId, ClassKind, Definition, GenericOwner,
    Modifiers, Semantics, Ty, TypeParamId, Visibility,
};
use ide_db::{
    imports::insert_import,
    render,
    symbol_index::{binary_class_index, source_symbol_index},
    TextEdit,
};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};

use crate::{rename::SourceChange, FileId, FileRange};

mod fields;
mod fixes;
mod intentions;
mod introduce;
mod quick_fixes;
mod records;
mod refactor;
mod strings;
mod suppress;
mod surround;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssistKind {
    QuickFix,
    Refactor,
    RefactorExtract,
    RefactorInline,
    RefactorRewrite,
    Generate,
    OrganizeImports,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assist {
    /// 稳定的标识，例如 `add_import`
    pub id: &'static str,
    pub label: String,
    pub kind: AssistKind,
    pub change: SourceChange,
}

struct AssistCtx<'a> {
    db: &'a dyn Db,
    sema: Semantics<'a>,
    file: SourceFile,
    file_id: FileId,
    text: &'a str,
    root: SyntaxNode,
    range: TextRange,
}

impl AssistCtx<'_> {
    fn assist(
        &self,
        id: &'static str,
        label: impl Into<String>,
        kind: AssistKind,
        edits: Vec<TextEdit>,
    ) -> Assist {
        Assist {
            id,
            label: label.into(),
            kind,
            change: SourceChange::single_file(self.file_id, edits),
        }
    }

    /// 偏移所在行的缩进。
    fn indent_at(&self, offset: TextSize) -> String {
        let pos = usize::from(offset).min(self.text.len());
        let line_start = self.text[..pos].rfind('\n').map_or(0, |i| i + 1);
        self.text[line_start..]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    }

    /// 光标所在的最内层具名类及其声明节点。
    fn enclosing_class(&self) -> Option<(ClassId, ast::TypeDecl)> {
        let token = syntax::algo::pick_ident_token(&self.root, self.range.start())?;
        token
            .parent_ancestors()
            .filter_map(ast::TypeDecl::cast)
            .find_map(|decl| {
                let class = self.sema.class_of_decl(self.file, &decl)?;
                Some((class, decl))
            })
    }

    /// 往类体末尾（右花括号之前）插入成员。`members` 的每个元素是一个成员的各行（不含缩进）。
    fn insert_members(&self, decl: &ast::TypeDecl, members: &[Vec<String>]) -> Option<TextEdit> {
        let body = decl.body()?;
        let close = body
            .syntax()
            .last_token()
            .filter(|t| t.kind() == SyntaxKind::R_CURLY)?;
        let class_indent = self.indent_at(decl.name()?.syntax().text_range().start());
        let inner = format!("{class_indent}    ");

        let mut rendered = String::new();
        for member in members {
            rendered.push('\n');
            for line in member {
                if !line.is_empty() {
                    rendered.push_str(&inner);
                    rendered.push_str(line);
                }
                rendered.push('\n');
            }
        }

        let close_start = usize::from(close.text_range().start());
        let line_start = self.text[..close_start].rfind('\n').map_or(0, |i| i + 1);
        let only_whitespace_before = self.text[line_start..close_start].trim().is_empty();
        if only_whitespace_before && line_start > 0 {
            // 右花括号独占一行：插在这一行之前
            Some(TextEdit::insert(
                TextSize::from(line_start as u32),
                rendered,
            ))
        } else {
            // `class A {}` 这种写在一行里的类体
            Some(TextEdit::insert(
                close.text_range().start(),
                format!("{rendered}{class_indent}"),
            ))
        }
    }
}

/// 类体里声明的实例字段（语法层面的信息：类型原文、是否 final、是否有初值）。
struct FieldInfo {
    name: String,
    ty: String,
    is_static: bool,
    is_final: bool,
    has_initializer: bool,
}

fn fields_of(decl: &ast::TypeDecl) -> Vec<FieldInfo> {
    let Some(body) = decl.body() else {
        return Vec::new();
    };
    let mut res = Vec::new();
    for field in body.syntax().children().filter_map(ast::FieldDecl::cast) {
        let Some(ty) = field.ty() else { continue };
        let ty_text = ty.syntax().text().to_string();
        let mods = field.modifier_list();
        let is_static = mods.as_ref().is_some_and(|m| m.has(SyntaxKind::STATIC_KW));
        let is_final = mods.as_ref().is_some_and(|m| m.has(SyntaxKind::FINAL_KW));
        for declarator in field.declarators() {
            let Some(name) = declarator.name() else {
                continue;
            };
            res.push(FieldInfo {
                name: name.text().to_string(),
                ty: format!("{ty_text}{}", "[]".repeat(declarator.extra_dims())),
                is_static,
                is_final,
                has_initializer: declarator.initializer().is_some(),
            });
        }
    }
    res
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// 快速修复
// ---------------------------------------------------------------------------

/// 光标在一个无法解析的简单类型名上：为每个同名的类提供"导入"。
fn add_import(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    if token.kind() != SyntaxKind::IDENT {
        return;
    }
    let Some(name_ref) = token.parent().and_then(ast::NameRef::cast) else {
        return;
    };
    let Some(parent) = name_ref.syntax().parent() else {
        return;
    };
    let is_simple_name = match parent.kind() {
        SyntaxKind::CLASS_TYPE => {
            ast::ClassType::cast(parent.clone()).is_some_and(|t| t.qualifier().is_none())
        }
        SyntaxKind::NAME_EXPR => true,
        SyntaxKind::QUALIFIED_NAME => {
            ast::QualifiedName::cast(parent.clone()).is_some_and(|q| q.qualifier().is_none())
                && parent
                    .parent()
                    .is_some_and(|p| p.kind() == SyntaxKind::ANNOTATION)
        }
        _ => false,
    };
    if !is_simple_name || ctx.sema.classify_name_ref(ctx.file, &name_ref).is_some() {
        return;
    }
    let name = name_ref.text();
    // 类型名按惯例以大写字母开头；小写的多半是拼错的变量
    if !name.starts_with(|c: char| c.is_uppercase()) {
        return;
    }

    let source = source_symbol_index(ctx.db);
    let binary = binary_class_index(ctx.db);
    let mut candidates: Vec<String> = source
        .search(&name, 64)
        .into_iter()
        .chain(binary.search(&name, 64))
        .filter(|(_, found, _)| **found == name)
        .filter_map(|(_, _, entry)| match &entry.def {
            Definition::Class(class) => ide_db::imports::importable_name(ctx.db, *class),
            _ => None,
        })
        .collect();
    // 标准库排在前面，JDK 内部包（`com.sun.*`、`sun.*`、`jdk.*`）排在最后
    fn rank(qualified: &str) -> u8 {
        if qualified.starts_with("java.util.") || qualified.starts_with("java.lang.") {
            0
        } else if qualified.starts_with("java.") || qualified.starts_with("javax.") {
            1
        } else if ["com.sun.", "sun.", "jdk."]
            .iter()
            .any(|prefix| qualified.starts_with(prefix))
        {
            3
        } else {
            2
        }
    }
    candidates.sort_by(|a, b| (rank(a), a).cmp(&(rank(b), b)));
    candidates.dedup();
    for qualified in candidates.into_iter().take(8) {
        let edit = insert_import(ctx.db, ctx.file, &qualified);
        out.push(ctx.assist(
            "add_import",
            base_db::tr!("导入 {qualified}", "Import {qualified}"),
            AssistKind::QuickFix,
            vec![edit],
        ));
    }
}

/// 整理 import：去掉未使用和重复的，模块导入、普通 import 与静态 import 各自按字母序排列。
fn organize_imports(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let cu = ast::CompilationUnit::cast(ctx.root.clone());
    let Some(cu) = cu else { return };
    let imports: Vec<ast::ImportDecl> = cu.imports().collect();
    let (Some(first), Some(last)) = (imports.first(), imports.last()) else {
        return;
    };
    let used = ide_diagnostics::used_identifiers(&cu);

    let mut modules: Vec<String> = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    let mut statics: Vec<String> = Vec::new();
    for import in &imports {
        let Some(name) = import.name() else { continue };
        // 模块导入总是保留（看不出用没用到）
        if import.is_module() {
            modules.push(format!("import module {};", name.text()));
            continue;
        }
        let segments = name.segments();
        let Some(last_segment) = segments.last() else {
            continue;
        };
        let on_demand = import.is_on_demand();
        if !on_demand && !used.contains(last_segment.as_str()) {
            continue;
        }
        let path = if on_demand {
            format!("{}.*", name.text())
        } else {
            name.text()
        };
        if import.is_static() {
            statics.push(format!("import static {path};"));
        } else {
            plain.push(format!("import {path};"));
        }
    }
    modules.sort();
    modules.dedup();
    plain.sort();
    plain.dedup();
    statics.sort();
    statics.dedup();

    let groups: Vec<String> = [modules, plain, statics]
        .into_iter()
        .filter(|group| !group.is_empty())
        .map(|group| group.join("\n"))
        .collect();
    let new_text = groups.join("\n\n");

    let range = TextRange::new(
        first.syntax().text_range().start(),
        last.syntax().text_range().end(),
    );
    let old_text = &ctx.text[usize::from(range.start())..usize::from(range.end())];
    if old_text == new_text {
        return;
    }
    out.push(ctx.assist(
        "organize_imports",
        base_db::trs!("整理 import", "Organize imports"),
        AssistKind::OrganizeImports,
        vec![TextEdit::replace(range, new_text)],
    ));
}

// ---------------------------------------------------------------------------
// 生成
// ---------------------------------------------------------------------------

fn generate_constructor(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some((class, decl)) = ctx.enclosing_class() else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    if data.kind != ClassKind::Class {
        return;
    }
    let fields: Vec<FieldInfo> = fields_of(&decl)
        .into_iter()
        .filter(|f| !f.is_static && !(f.is_final && f.has_initializer))
        .collect();
    // 已经有参数个数相同的显式构造器就不再提供
    let exists = data
        .constructors()
        .any(|(_, ctor)| ctor.params.len() == fields.len() && ctor.decl != data.decl);
    if exists {
        return;
    }
    let params: Vec<String> = fields
        .iter()
        .map(|f| format!("{} {}", f.ty, f.name))
        .collect();
    let mut lines = vec![format!("public {}({}) {{", data.name, params.join(", "))];
    lines.extend(
        fields
            .iter()
            .map(|f| format!("    this.{0} = {0};", f.name)),
    );
    lines.push("}".to_owned());
    let Some(edit) = ctx.insert_members(&decl, &[lines]) else {
        return;
    };
    out.push(ctx.assist(
        "generate_constructor",
        base_db::trs!("生成构造器", "Generate constructor"),
        AssistKind::Generate,
        vec![edit],
    ));
}

fn generate_accessors(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some((class, decl)) = ctx.enclosing_class() else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    if !matches!(data.kind, ClassKind::Class | ClassKind::Enum) {
        return;
    }
    let has_method = |name: &str, arity: usize| {
        data.methods
            .iter()
            .any(|m| !m.is_constructor && m.name == name && m.params.len() == arity)
    };
    let mut getters: Vec<Vec<String>> = Vec::new();
    let mut setters: Vec<Vec<String>> = Vec::new();
    for field in fields_of(&decl).into_iter().filter(|f| !f.is_static) {
        let property = capitalize(&field.name);
        let getter = if field.ty == "boolean" {
            format!("is{property}")
        } else {
            format!("get{property}")
        };
        if !has_method(&getter, 0) {
            getters.push(vec![
                format!("public {} {getter}() {{", field.ty),
                format!("    return {};", field.name),
                "}".to_owned(),
            ]);
        }
        let setter = format!("set{property}");
        if !field.is_final && !has_method(&setter, 1) {
            setters.push(vec![
                format!("public void {setter}({} {}) {{", field.ty, field.name),
                format!("    this.{0} = {0};", field.name),
                "}".to_owned(),
            ]);
        }
    }
    if !getters.is_empty() {
        if let Some(edit) = ctx.insert_members(&decl, &getters) {
            out.push(ctx.assist(
                "generate_getters",
                base_db::trs!("生成 getter", "Generate getter"),
                AssistKind::Generate,
                vec![edit],
            ));
        }
    }
    if !getters.is_empty() || !setters.is_empty() {
        let mut both = getters;
        both.extend(setters);
        if let Some(edit) = ctx.insert_members(&decl, &both) {
            out.push(ctx.assist(
                "generate_getters_setters",
                base_db::trs!("生成 getter 和 setter", "Generate getter and setter"),
                AssistKind::Generate,
                vec![edit],
            ));
        }
    }
}

fn generate_object_methods(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some((class, decl)) = ctx.enclosing_class() else {
        return;
    };
    let Some(data) = class_data(ctx.db, class) else {
        return;
    };
    if data.kind != ClassKind::Class {
        return;
    }
    let declares = |name: &str, arity: usize| {
        data.methods
            .iter()
            .any(|m| m.decl != data.decl && m.name == name && m.params.len() == arity)
    };
    let fields: Vec<FieldInfo> = fields_of(&decl)
        .into_iter()
        .filter(|f| !f.is_static)
        .collect();
    let name = data.name.as_str();

    if !declares("toString", 0) {
        let mut lines = vec![
            "@Override".to_owned(),
            "public String toString() {".to_owned(),
        ];
        if fields.is_empty() {
            lines.push(format!("    return \"{name}{{}}\";"));
        } else {
            lines.push(format!("    return \"{name}{{\""));
            for (idx, field) in fields.iter().enumerate() {
                let sep = if idx == 0 { "" } else { ", " };
                lines.push(format!("            + \"{sep}{0}=\" + {0}", field.name));
            }
            lines.push("            + '}';".to_owned());
        }
        lines.push("}".to_owned());
        if let Some(edit) = ctx.insert_members(&decl, &[lines]) {
            out.push(ctx.assist(
                "generate_to_string",
                base_db::trs!("生成 toString()", "Generate toString()"),
                AssistKind::Generate,
                vec![edit],
            ));
        }
    }

    if !declares("equals", 1) && !declares("hashCode", 0) {
        let mut equals = vec![
            "@Override".to_owned(),
            "public boolean equals(Object o) {".to_owned(),
            "    if (this == o) return true;".to_owned(),
            "    if (o == null || getClass() != o.getClass()) return false;".to_owned(),
        ];
        if fields.is_empty() {
            equals.push("    return true;".to_owned());
        } else {
            equals.push(format!("    {name} that = ({name}) o;"));
            let comparisons: Vec<String> = fields
                .iter()
                .map(|f| {
                    let primitive = matches!(
                        f.ty.as_str(),
                        "boolean" | "byte" | "char" | "short" | "int" | "long"
                    );
                    if primitive {
                        format!("{0} == that.{0}", f.name)
                    } else if f.ty == "float" {
                        format!("Float.compare({0}, that.{0}) == 0", f.name)
                    } else if f.ty == "double" {
                        format!("Double.compare({0}, that.{0}) == 0", f.name)
                    } else if f.ty.ends_with("[]") {
                        format!("java.util.Arrays.equals({0}, that.{0})", f.name)
                    } else {
                        format!("java.util.Objects.equals({0}, that.{0})", f.name)
                    }
                })
                .collect();
            equals.push(format!(
                "    return {};",
                comparisons.join("\n            && ")
            ));
        }
        equals.push("}".to_owned());
        // 上面拼接的换行要拆回独立的行，才能得到正确的缩进
        let equals: Vec<String> = equals
            .iter()
            .flat_map(|l| l.split('\n'))
            .map(str::to_owned)
            .collect();

        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        let hash = vec![
            "@Override".to_owned(),
            "public int hashCode() {".to_owned(),
            format!("    return java.util.Objects.hash({});", names.join(", ")),
            "}".to_owned(),
        ];
        if let Some(edit) = ctx.insert_members(&decl, &[equals, hash]) {
            out.push(ctx.assist(
                "generate_equals_hash_code",
                base_db::trs!(
                    "生成 equals() 和 hashCode()",
                    "Generate equals() and hashCode()"
                ),
                AssistKind::Generate,
                vec![edit],
            ));
        }
    }
}

/// 非抽象类里还没有实现的抽象方法：生成桩。
fn implement_methods(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let db = ctx.db;
    let Some((class, decl)) = ctx.enclosing_class() else {
        return;
    };
    let Some(data) = class_data(db, class) else {
        return;
    };
    if data.is_interface() || data.flags.is_abstract() {
        return;
    }
    let mut members: Vec<Vec<String>> = Vec::new();
    for candidate in all_methods(db, &this_ty(db, class)) {
        if candidate.id.class == class {
            continue;
        }
        let Some(owner) = class_data(db, candidate.id.class) else {
            continue;
        };
        let Some(method) = owner.methods.get(candidate.id.idx as usize) else {
            continue;
        };
        // `all_methods` 已按覆写关系去重：留下来的抽象方法就是没被实现的
        if !method.flags.is_abstract()
            || method.flags.is_static()
            || !is_accessible(db, method.flags, candidate.id.class, Some(class))
        {
            continue;
        }
        let params = render::method_params(db, candidate.id, &candidate.subst);
        let ret = candidate.ret_ty(db);
        let visibility =
            if method.flags.visibility() == Visibility::Protected && !owner.is_interface() {
                "protected"
            } else {
                "public"
            };
        let type_params = if method.type_params.is_empty() {
            String::new()
        } else {
            let decls: Vec<String> = (0..method.type_params.len())
                .map(|idx| {
                    render::type_param_decl(
                        db,
                        TypeParamId {
                            owner: GenericOwner::Method(candidate.id),
                            idx: idx as u32,
                        },
                    )
                })
                .collect();
            format!("<{}> ", decls.join(", "))
        };
        let body = match &ret {
            Ty::Void => None,
            Ty::Primitive(hir::PrimitiveType::Boolean) => Some("return false;"),
            Ty::Primitive(_) => Some("return 0;"),
            _ => Some("return null;"),
        };
        let mut lines = vec![
            "@Override".to_owned(),
            format!(
                "{visibility} {type_params}{} {}({}) {{",
                ret.display(db),
                method.name,
                params.join(", ")
            ),
        ];
        if let Some(body) = body {
            lines.push(format!("    {body}"));
        }
        lines.push("}".to_owned());
        members.push(lines);
    }
    if members.is_empty() {
        return;
    }
    let count = members.len();
    let Some(edit) = ctx.insert_members(&decl, &members) else {
        return;
    };
    out.push(ctx.assist(
        "implement_methods",
        base_db::tr!(
            "实现 {count} 个抽象方法",
            "Implement {count} abstract method(s)"
        ),
        AssistKind::QuickFix,
        vec![edit],
    ));
}

// ---------------------------------------------------------------------------
// 重构
// ---------------------------------------------------------------------------

/// 选中的表达式提取成局部变量。
fn extract_variable(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    if ctx.range.is_empty() {
        return;
    }
    // 选区去掉两端空白后必须正好是一个表达式
    let selected = &ctx.text[usize::from(ctx.range.start())..usize::from(ctx.range.end())];
    let leading = selected.len() - selected.trim_start().len();
    let trailing = selected.len() - selected.trim_end().len();
    let range = TextRange::new(
        ctx.range.start() + TextSize::from(leading as u32),
        ctx.range.end() - TextSize::from(trailing as u32),
    );
    if range.is_empty() {
        return;
    }
    let Some(expr) = syntax::algo::find_node_at_range::<ast::Expr>(&ctx.root, range) else {
        return;
    };
    if expr.syntax().text_range() != range {
        return;
    }
    // 赋值的左侧、语句级的调用提取出来没有意义
    let Some(parent) = expr.syntax().parent() else {
        return;
    };
    if matches!(parent.kind(), SyntaxKind::EXPR_STMT) {
        return;
    }
    if parent.kind() == SyntaxKind::ASSIGN_EXPR
        && parent.first_child().as_ref() == Some(expr.syntax())
    {
        return;
    }
    // 所在的语句必须直接位于块里，才有地方放新的声明
    let Some(stmt) = expr.syntax().ancestors().find(|n| {
        n.parent().is_some_and(|p| p.kind() == SyntaxKind::BLOCK) && ast::Stmt::can_cast(n.kind())
    }) else {
        return;
    };
    // 提到语句之前必须仍然恰好求值一次：不能跨过条件求值、循环和 lambda
    if !hoistable(expr.syntax(), &stmt) {
        return;
    }
    // 用到的局部符号必须在语句之前就已声明（排除 `for` 的循环变量、同一条声明里靠前的变量、
    // 模式变量、lambda 形参）
    let stmt_range = stmt.text_range();
    let uses_inner_local = expr
        .syntax()
        .descendants()
        .filter_map(ast::NameRef::cast)
        .any(
            |name_ref| match ctx.sema.classify_name_ref(ctx.file, &name_ref) {
                Some(Definition::Local(local)) => {
                    let decl = hir::infer(ctx.db, local.body).local(local.id).decl_range;
                    stmt_range.contains_range(decl)
                }
                _ => false,
            },
        );
    if uses_inner_local {
        return;
    }

    let ty = ctx.sema.type_of_expr(ctx.file, &expr);
    let ty_text = match &ty {
        Some(Ty::Void) => return,
        Some(ty) if !ty.contains_unknown() && !matches!(ty, Ty::Null) => ty.display(ctx.db),
        _ => "var".to_owned(),
    };
    let name = fresh_local_name(&expr, &stmt);
    let stmt_start = stmt_range.start();
    let indent = ctx.indent_at(stmt_start);
    let expr_text = expr.syntax().text().to_string();
    let edits = vec![
        TextEdit::insert(
            stmt_start,
            format!("{ty_text} {name} = {expr_text};\n{indent}"),
        ),
        TextEdit::replace(range, name.as_str()),
    ];
    out.push(ctx.assist(
        "extract_variable",
        base_db::trs!("提取为局部变量", "Extract local variable"),
        AssistKind::RefactorExtract,
        edits,
    ));
}

/// 把 `expr` 提到 `stmt` 之前，它是否仍然恰好在原来的时机求值一次。
///
/// 沿着 `expr` 往上走到 `stmt`，途中只要经过下面的位置就不行：
/// `&&` / `||` 的右侧、`?:` 的两个分支、`if` 的分支语句、循环的条件 / 更新 / 循环体
/// （增强 `for` 的被迭代表达式只求值一次，可以）、lambda 体、`switch` 分支。
fn hoistable(expr: &SyntaxNode, stmt: &SyntaxNode) -> bool {
    let mut child = expr.clone();
    while child != *stmt {
        let Some(parent) = child.parent() else {
            return false;
        };
        let conditional = match parent.kind() {
            SyntaxKind::BINARY_EXPR => ast::BinaryExpr::cast(parent.clone()).is_some_and(|bin| {
                matches!(
                    bin.op_kind(),
                    Some(SyntaxKind::AMP_AMP | SyntaxKind::PIPE_PIPE)
                ) && bin.rhs().is_some_and(|rhs| *rhs.syntax() == child)
            }),
            SyntaxKind::CONDITIONAL_EXPR => ast::ConditionalExpr::cast(parent.clone())
                .and_then(|c| c.condition())
                .is_none_or(|cond| *cond.syntax() != child),
            SyntaxKind::IF_STMT => ast::IfStmt::cast(parent.clone())
                .and_then(|s| s.condition())
                .is_none_or(|cond| *cond.syntax() != child),
            SyntaxKind::FOREACH_STMT => ast::ForeachStmt::cast(parent.clone())
                .and_then(|s| s.iterable())
                .is_none_or(|iter| *iter.syntax() != child),
            SyntaxKind::WHILE_STMT
            | SyntaxKind::DO_WHILE_STMT
            | SyntaxKind::FOR_STMT
            | SyntaxKind::LAMBDA_EXPR
            | SyntaxKind::SWITCH_CASE
            | SyntaxKind::SWITCH_BLOCK => true,
            _ => false,
        };
        if conditional {
            return false;
        }
        child = parent;
    }
    true
}

/// 提取出来的局部变量的名字：按表达式猜一个（`s.trim()` → `trim`，`getName()` → `name`，
/// `new Foo()` → `foo`），猜不出来就用 `extracted`；再避开所在方法体里出现过的所有标识符，
/// 这样既不会重复声明，也不会遮蔽后面用到的字段。
fn fresh_local_name(expr: &ast::Expr, stmt: &SyntaxNode) -> String {
    fn decapitalize(name: &str) -> String {
        let mut chars = name.chars();
        match chars.next() {
            Some(first) => first.to_lowercase().chain(chars).collect(),
            None => String::new(),
        }
    }
    fn strip_accessor_prefix(name: &str) -> String {
        for prefix in ["get", "is"] {
            if let Some(rest) = name.strip_prefix(prefix) {
                if rest.starts_with(|c: char| c.is_uppercase()) {
                    return decapitalize(rest);
                }
            }
        }
        name.to_owned()
    }
    let guess = match expr {
        ast::Expr::MethodCall(call) => call.name_ref().map(|n| strip_accessor_prefix(&n.text())),
        ast::Expr::FieldAccess(access) => access.name_ref().map(|n| n.text().to_string()),
        ast::Expr::NewExpr(new) => match new.ty() {
            Some(ast::Type::ClassType(ty)) => ty.segments().last().map(|s| decapitalize(s)),
            _ => None,
        },
        _ => None,
    };
    let base = guess
        .filter(|name| crate::rename::is_valid_identifier(name))
        .unwrap_or_else(|| "extracted".to_owned());

    // 整个成员体（方法、构造器、初始化块）里出现过的标识符
    let body = stmt
        .ancestors()
        .filter(|n| n.kind() == SyntaxKind::BLOCK)
        .find(|n| {
            n.parent().is_some_and(|p| {
                matches!(
                    p.kind(),
                    SyntaxKind::METHOD_DECL
                        | SyntaxKind::CONSTRUCTOR_DECL
                        | SyntaxKind::INITIALIZER_BLOCK
                )
            })
        })
        .or_else(|| stmt.parent())
        .unwrap_or_else(|| stmt.clone());
    // 被调用的方法名不占用变量的名字空间，不算
    let is_called_method_name = |token: &syntax::SyntaxToken| {
        token
            .parent()
            .and_then(ast::NameRef::cast)
            .is_some_and(|name_ref| {
                name_ref
                    .syntax()
                    .parent()
                    .and_then(ast::MethodCall::cast)
                    .and_then(|call| call.name_ref())
                    .is_some_and(|n| n == name_ref)
            })
    };
    let taken: rustc_hash::FxHashSet<String> = body
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::IDENT && !is_called_method_name(t))
        .map(|t| t.text().to_owned())
        .collect();
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|name| !taken.contains(name))
        .expect("总能找到没用过的名字")
}

/// `var x = ...` → 写出显式类型。
fn replace_var_with_type(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(decl) = token.parent_ancestors().find_map(ast::LocalVarDecl::cast) else {
        return;
    };
    let Some(ast::Type::ClassType(class_ty)) = decl.ty() else {
        return;
    };
    if class_ty.segments().len() != 1 || class_ty.segments()[0] != "var" {
        return;
    }
    let mut declarators = decl.declarators();
    let (Some(declarator), None) = (declarators.next(), declarators.next()) else {
        return;
    };
    let Some(name) = declarator.name() else {
        return;
    };
    let Some(Definition::Local(local)) = ctx.sema.classify_name(ctx.file, &name) else {
        return;
    };
    let ty = hir::infer(ctx.db, local.body).local(local.id).ty.clone();
    if ty.contains_unknown() || matches!(ty, Ty::Null | Ty::Void | Ty::Intersection(_)) {
        return;
    }
    let edit = TextEdit::replace(class_ty.syntax().text_range(), ty.display(ctx.db));
    out.push(ctx.assist(
        "replace_var_with_type",
        base_db::trs!("把 var 替换为显式类型", "Replace var with explicit type"),
        AssistKind::RefactorRewrite,
        vec![edit],
    ));
}

/// 给覆写了父类型方法、却没有标注的方法加上 `@Override`。
fn add_override(ctx: &AssistCtx<'_>, out: &mut Vec<Assist>) {
    let Some(token) = syntax::algo::pick_ident_token(&ctx.root, ctx.range.start()) else {
        return;
    };
    let Some(method_decl) = token
        .parent()
        .and_then(|p| p.parent())
        .and_then(ast::MethodDecl::cast)
    else {
        return;
    };
    // 只在方法名上提供
    if method_decl.name().map(|n| n.syntax().text_range()) != Some(token.text_range()) {
        return;
    }
    let annotated = method_decl.modifier_list().is_some_and(|mods| {
        mods.annotations().any(|a| {
            a.name()
                .is_some_and(|n| n.segments().last().is_some_and(|s| s == "Override"))
        })
    });
    if annotated {
        return;
    }
    let Some(method) = ctx.sema.method_of_decl(ctx.file, method_decl.syntax()) else {
        return;
    };
    let is_static = class_data(ctx.db, method.class)
        .and_then(|d| {
            d.methods
                .get(method.idx as usize)
                .map(|m| m.flags.contains(Modifiers::STATIC))
        })
        .unwrap_or(true);
    if is_static || crate::navigation::super_methods(ctx.db, method).is_empty() {
        return;
    }
    // 插在声明的第一个有效 token 之前（文档注释之后）
    let Some(first) = ast::support::first_token_in_subtree(method_decl.syntax()) else {
        return;
    };
    let start = first.text_range().start();
    let indent = ctx.indent_at(start);
    let edit = TextEdit::insert(start, format!("@Override\n{indent}"));
    out.push(ctx.assist(
        "add_override",
        base_db::trs!("添加 @Override", "Add @Override"),
        AssistKind::QuickFix,
        vec![edit],
    ));
}

pub(crate) fn assists(db: &dyn Db, range: FileRange) -> Vec<Assist> {
    let Some(file) = base_db::source_file(db, range.file_id) else {
        return Vec::new();
    };
    let text: &str = file.text(db);
    let root = base_db::parse(db, file).syntax_node();
    if !root.text_range().contains_range(range.range) {
        return Vec::new();
    }
    let ctx = AssistCtx {
        db,
        sema: Semantics::new(db),
        file,
        file_id: range.file_id,
        text,
        root,
        range: range.range,
    };
    let mut out = Vec::new();
    add_import(&ctx, &mut out);
    fixes::create_method(&ctx, &mut out);
    fixes::create_variable(&ctx, &mut out);
    fixes::handle_exception(&ctx, &mut out);
    fixes::add_cast(&ctx, &mut out);
    fixes::add_missing_switch_cases(&ctx, &mut out);
    fixes::initialize_variable(&ctx, &mut out);
    fixes::replace_with_equals(&ctx, &mut out);
    quick_fixes::remove_unused_symbol(&ctx, &mut out);
    quick_fixes::create_class(&ctx, &mut out);
    quick_fixes::change_visibility(&ctx, &mut out);
    quick_fixes::remove_final(&ctx, &mut out);
    quick_fixes::change_type_to_match(&ctx, &mut out);
    quick_fixes::add_return(&ctx, &mut out);
    quick_fixes::make_class_abstract(&ctx, &mut out);
    quick_fixes::simplify(&ctx, &mut out);
    quick_fixes::fix_array_misuse(&ctx, &mut out);
    quick_fixes::fix_probable_bug(&ctx, &mut out);
    quick_fixes::make_method_static(&ctx, &mut out);
    quick_fixes::optional_get_to_or_else_throw(&ctx, &mut out);
    quick_fixes::convert_field_to_local(&ctx, &mut out);
    suppress::suppress_warning(&ctx, &mut out);
    surround::surround_with(&ctx, &mut out);
    surround::split_or_merge_if(&ctx, &mut out);
    surround::replace_if_with_switch(&ctx, &mut out);
    strings::concat_to_format(&ctx, &mut out);
    strings::concat_to_text_block(&ctx, &mut out);
    strings::concat_loop_to_builder(&ctx, &mut out);
    implement_methods(&ctx, &mut out);
    add_override(&ctx, &mut out);
    extract_variable(&ctx, &mut out);
    refactor::extract_method(&ctx, &mut out);
    refactor::extract_constant(&ctx, &mut out);
    introduce::introduce_parameter(&ctx, &mut out);
    fields::extract_field(&ctx, &mut out);
    fields::inline_constant(&ctx, &mut out);
    fields::generate_delegates(&ctx, &mut out);
    records::convert_to_record(&ctx, &mut out);
    refactor::inline_variable(&ctx, &mut out);
    refactor::inline_method(&ctx, &mut out);
    refactor::remove_unused_parameter(&ctx, &mut out);
    refactor::invert_if(&ctx, &mut out);
    refactor::move_parameter(&ctx, &mut out);
    refactor::pull_up_method(&ctx, &mut out);
    refactor::anonymous_to_lambda(&ctx, &mut out);
    replace_var_with_type(&ctx, &mut out);
    intentions::add_braces(&ctx, &mut out);
    intentions::split_declaration(&ctx, &mut out);
    intentions::join_declaration(&ctx, &mut out);
    intentions::flip_binary(&ctx, &mut out);
    intentions::ternary_to_if(&ctx, &mut out);
    intentions::if_to_ternary(&ctx, &mut out);
    intentions::lambda_to_method_ref(&ctx, &mut out);
    intentions::method_ref_to_lambda(&ctx, &mut out);
    intentions::convert_to_enhanced_for(&ctx, &mut out);
    intentions::switch_to_arrow(&ctx, &mut out);
    intentions::field_from_parameter(&ctx, &mut out);
    generate_constructor(&ctx, &mut out);
    generate_accessors(&ctx, &mut out);
    generate_object_methods(&ctx, &mut out);
    organize_imports(&ctx, &mut out);
    out
}
