//! 补全条目及其构造。

use hir::{
    class_data, is_assignable, resolve_simple_type, ClassId, ClassKind, FieldCandidate, LocalData,
    LocalKind, MethodCandidate, Ty, TypeNs,
};
use ide_db::{
    imports::{importable_name, insert_import},
    render, SymbolKind, TextEdit,
};
use rustc_hash::FxHashSet;
use smol_str::SmolStr;
use syntax::TextRange;

use crate::context::{CompletionContext, TypeLocation};

/// 补全位置所在文件的包名（斜杠形式，默认包为空串）。
fn file_package(ctx: &CompletionContext<'_>) -> String {
    <syntax::ast::CompilationUnit as syntax::ast::AstNode>::cast(ctx.original_root.clone())
        .and_then(|cu| cu.package())
        .and_then(|p| p.name())
        .map(|n| n.text().replace('.', "/"))
        .unwrap_or_default()
}

fn outermost(db: &dyn hir::Db, class: ClassId) -> SmolStr {
    let name = class.binary_name(db);
    match name.split_once('$') {
        Some((outer, _)) => SmolStr::new(outer),
        None => name,
    }
}

/// 类在补全位置能不能访问（JLS 6.6.1）：它自己和它的每一层外层类都要能访问。
/// 包私有的类只在同一个包里能用，私有的嵌套类只在同一个顶层类里能用，
/// 受保护的嵌套类还可以在外层类的子类里用。
fn class_accessible(ctx: &CompletionContext<'_>, class: ClassId) -> bool {
    let db = ctx.db;
    let package = file_package(ctx);
    let mut current = Some(class);
    for _ in 0..32 {
        let Some(c) = current else {
            break;
        };
        let Some(data) = class_data(db, c) else {
            return true;
        };
        let outer = data.outer.as_deref().and_then(|o| hir::lookup_class(db, o));
        let same_package = data.package() == package;
        let accessible = match data.flags.visibility() {
            hir::Visibility::Public => true,
            hir::Visibility::Package => same_package,
            hir::Visibility::Protected => {
                same_package
                    || match (ctx.enclosing_class, outer) {
                        (Some(from), Some(outer)) => {
                            hir::is_subclass_of(db, from, outer)
                                || hir::lookup_class(db, &outermost(db, from))
                                    .is_some_and(|top| hir::is_subclass_of(db, top, outer))
                        }
                        _ => false,
                    }
            }
            hir::Visibility::Private => ctx
                .enclosing_class
                .is_some_and(|from| outermost(db, from) == outermost(db, c)),
        };
        if !accessible {
            return false;
        }
        current = outer;
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionItemKind {
    Symbol(SymbolKind),
    Keyword,
    Snippet,
}

/// 延迟解析（`completionItem/resolve`）时用来找回定义的数据。只含可序列化的稳定标识。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionResolve {
    Class {
        binary_name: SmolStr,
    },
    Method {
        class: SmolStr,
        idx: u32,
    },
    Field {
        class: SmolStr,
        idx: u32,
    },
    /// Spring 配置文件里的一个键（文档来自配置元数据）
    ConfigKey {
        key: SmolStr,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionItemKind,
    /// 紧跟在标签后面的灰色文字（方法的形参表、类所在的包）
    pub label_detail: Option<String>,
    /// 右侧的说明（类型）
    pub detail: Option<String>,
    /// 用于过滤的文本
    pub lookup: String,
    pub insert_text: String,
    /// `insert_text` 是否是带 `$0` / `${1:x}` 占位的片段
    pub is_snippet: bool,
    /// 被 `insert_text` 替换的范围
    pub source_range: TextRange,
    pub additional_edits: Vec<TextEdit>,
    pub deprecated: bool,
    /// 相关度，越大越靠前
    pub score: i32,
    pub resolve: Option<CompletionResolve>,
}

/// 各类候选的基础分。
pub(crate) mod score {
    pub(crate) const ENUM_CASE: i32 = 120;
    pub(crate) const LOCAL: i32 = 100;
    pub(crate) const MEMBER: i32 = 85;
    pub(crate) const STATIC_IMPORT: i32 = 70;
    pub(crate) const TYPE_IN_SCOPE: i32 = 60;
    pub(crate) const OVERRIDE: i32 = 58;
    pub(crate) const KEYWORD: i32 = 50;
    pub(crate) const POSTFIX: i32 = 45;
    pub(crate) const SNIPPET: i32 = 40;
    pub(crate) const PACKAGE: i32 = 35;
    pub(crate) const TYPE_UNIMPORTED: i32 = 20;

    pub(crate) const EXPECTED_TYPE: i32 = 30;
    /// 以已输入的前缀开头的候选要排在只是模糊匹配上的候选前面（`ret` → `return` 先于 `Deprecated`）
    pub(crate) const EXACT_PREFIX: i32 = 25;
    pub(crate) const DEPRECATED: i32 = -25;
    pub(crate) const OBJECT_MEMBER: i32 = -20;
    pub(crate) const STATIC_VIA_INSTANCE: i32 = -30;
}

#[derive(Default)]
pub(crate) struct Completions {
    pub(crate) items: Vec<CompletionItem>,
    seen_classes: FxHashSet<ClassId>,
}

impl Completions {
    fn base(
        ctx: &CompletionContext<'_>,
        label: impl Into<String>,
        kind: CompletionItemKind,
        score: i32,
    ) -> CompletionItem {
        let label = label.into();
        let exact = !ctx.prefix.is_empty() && label.starts_with(&ctx.prefix);
        CompletionItem {
            lookup: label.clone(),
            insert_text: label.clone(),
            label,
            kind,
            label_detail: None,
            detail: None,
            is_snippet: false,
            source_range: ctx.source_range,
            additional_edits: Vec::new(),
            deprecated: false,
            score: score + if exact { score::EXACT_PREFIX } else { 0 },
            resolve: None,
        }
    }

    fn type_bonus(ctx: &CompletionContext<'_>, ty: &Ty) -> i32 {
        let Some(expected) = &ctx.expected_ty else {
            return 0;
        };
        if ty.is_unknown() || *ty == Ty::Void || *expected == Ty::object(ctx.db) {
            return 0;
        }
        if is_assignable(ctx.db, ty, expected) {
            score::EXPECTED_TYPE
        } else {
            0
        }
    }

    pub(crate) fn matches(ctx: &CompletionContext<'_>, name: &str) -> bool {
        ide_db::fuzzy::matches_prefix(&ctx.prefix, name)
    }

    pub(crate) fn add(&mut self, item: CompletionItem) {
        self.items.push(item);
    }

    pub(crate) fn add_local(&mut self, ctx: &CompletionContext<'_>, local: &LocalData) {
        if !Self::matches(ctx, &local.name) {
            return;
        }
        let kind = match local.kind {
            LocalKind::Param | LocalKind::LambdaParam | LocalKind::CatchParam => SymbolKind::Param,
            LocalKind::LocalClassField => SymbolKind::Field,
            LocalKind::LocalClassMethod => SymbolKind::Method,
            _ => SymbolKind::Local,
        };
        let mut item = Self::base(
            ctx,
            local.name.as_str(),
            CompletionItemKind::Symbol(kind),
            score::LOCAL + Self::type_bonus(ctx, &local.ty),
        );
        item.detail = Some(local.ty.display(ctx.db));
        if local.kind == LocalKind::LocalClassMethod && !ctx.followed_by_paren {
            item.insert_text = format!("{}()", local.name);
        }
        self.add(item);
    }

    pub(crate) fn add_field(
        &mut self,
        ctx: &CompletionContext<'_>,
        field: &FieldCandidate,
        base_score: i32,
    ) {
        let db = ctx.db;
        let Some(data) = class_data(db, field.id.class) else {
            return;
        };
        let Some(f) = data.fields.get(field.id.idx as usize) else {
            return;
        };
        if !Self::matches(ctx, &f.name) {
            return;
        }
        let kind = if f.is_enum_constant() {
            SymbolKind::EnumConstant
        } else {
            SymbolKind::Field
        };
        let mut item = Self::base(
            ctx,
            f.name.as_str(),
            CompletionItemKind::Symbol(kind),
            base_score + Self::type_bonus(ctx, &field.ty),
        );
        item.detail = Some(field.ty.display(db));
        item.deprecated = f.flags.is_deprecated();
        if item.deprecated {
            item.score += score::DEPRECATED;
        }
        item.resolve = Some(CompletionResolve::Field {
            class: field.id.class.binary_name(db),
            idx: field.id.idx,
        });
        self.add(item);
    }

    /// `name_only`：只插入方法名（方法引用、静态 import）。
    pub(crate) fn add_method(
        &mut self,
        ctx: &CompletionContext<'_>,
        method: &MethodCandidate,
        base_score: i32,
        name_only: bool,
    ) {
        let db = ctx.db;
        let Some(data) = class_data(db, method.id.class) else {
            return;
        };
        let Some(m) = data.methods.get(method.id.idx as usize) else {
            return;
        };
        if m.is_constructor || !Self::matches(ctx, &m.name) {
            return;
        }
        let ret = method.ret_ty(db);
        // `Object` 的方法不因为返回类型合适而提前：它们总是排在接收者自己的成员后面
        let from_object = &*method.id.class.binary_name(db) == "java/lang/Object";
        let bonus = if from_object {
            0
        } else {
            Self::type_bonus(ctx, &ret)
        };
        let mut item = Self::base(
            ctx,
            m.name.as_str(),
            CompletionItemKind::Symbol(SymbolKind::Method),
            base_score + bonus,
        );
        let params = render::method_params(db, method.id, &method.subst);
        item.label_detail = Some(format!("({})", params.join(", ")));
        item.detail = Some(ret.display(db));
        item.deprecated = m.flags.is_deprecated();
        if item.deprecated {
            item.score += score::DEPRECATED;
        }
        if from_object {
            item.score += score::OBJECT_MEMBER;
        }
        if !name_only && !ctx.followed_by_paren {
            if params.is_empty() || !ctx.config.snippets {
                item.insert_text = format!("{}()", m.name);
            } else {
                item.insert_text = format!("{}($0)", m.name);
                item.is_snippet = true;
            }
        }
        item.resolve = Some(CompletionResolve::Method {
            class: method.id.class.binary_name(db),
            idx: method.id.idx,
        });
        self.add(item);
    }

    /// 加入一个类。`import` 为真时按需附带 import 编辑（或改用全限定名）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_class(
        &mut self,
        ctx: &CompletionContext<'_>,
        class: ClassId,
        name: &str,
        kind: SymbolKind,
        deprecated: bool,
        base_score: i32,
        location: TypeLocation,
        import: bool,
    ) {
        if !hir::is_visible_from(ctx.db, Some(ctx.file), class) || !class_accessible(ctx, class) {
            return;
        }
        let accepted = match location {
            TypeLocation::Annotation => kind == SymbolKind::Annotation,
            TypeLocation::New => !matches!(kind, SymbolKind::Annotation | SymbolKind::Enum),
            _ => true,
        };
        if !accepted || !Self::matches(ctx, name) || !self.seen_classes.insert(class) {
            return;
        }
        let db = ctx.db;
        let mut item = Self::base(ctx, name, CompletionItemKind::Symbol(kind), base_score);
        let binary_name = class.binary_name(db);
        if let Some(idx) = binary_name.rfind(['/', '$']) {
            item.label_detail = Some(format!(
                " ({})",
                binary_name[..idx].replace(['/', '$'], ".")
            ));
        }
        item.deprecated = deprecated;
        if deprecated {
            item.score += score::DEPRECATED;
        }

        let mut insert = name.to_owned();
        if import {
            match resolve_simple_type(db, &ctx.scope, name) {
                Some(TypeNs::Class(found)) if found == class => {}
                Some(_) => {
                    if let Some(qualified) = importable_name(db, class) {
                        insert = qualified;
                    }
                }
                None => {
                    if let Some(qualified) = importable_name(db, class) {
                        if ctx.config.auto_import {
                            item.additional_edits
                                .push(insert_import(db, ctx.file, &qualified));
                        } else {
                            insert = qualified;
                        }
                    }
                }
            }
        }

        if location == TypeLocation::New && !ctx.followed_by_paren {
            if let Some(data) = class_data(db, class) {
                let diamond = if data.type_params.is_empty() {
                    ""
                } else {
                    "<>"
                };
                let needs_body = data.kind == ClassKind::Interface || data.flags.is_abstract();
                let has_params = data.constructors().any(|(_, ctor)| !ctor.params.is_empty());
                if !ctx.config.snippets {
                    insert = format!("{insert}{diamond}()");
                } else if needs_body {
                    let indent = &ctx.indent;
                    insert = format!("{insert}{diamond}() {{\n{indent}    $0\n{indent}}}");
                    item.is_snippet = true;
                } else if has_params {
                    insert = format!("{insert}{diamond}($0)");
                    item.is_snippet = true;
                } else {
                    insert = format!("{insert}{diamond}()");
                }
            }
        }
        item.insert_text = insert;
        item.resolve = Some(CompletionResolve::Class { binary_name });
        self.add(item);
    }

    /// 已经加载过签名的类（作用域内的类）。
    pub(crate) fn add_known_class(
        &mut self,
        ctx: &CompletionContext<'_>,
        class: ClassId,
        base_score: i32,
        location: TypeLocation,
        import: bool,
    ) {
        let Some(data) = class_data(ctx.db, class) else {
            return;
        };
        self.add_class(
            ctx,
            class,
            &data.name,
            SymbolKind::of_class(data.kind),
            data.flags.is_deprecated(),
            base_score,
            location,
            import,
        );
    }

    pub(crate) fn add_package(&mut self, ctx: &CompletionContext<'_>, name: &str) {
        if !Self::matches(ctx, name) {
            return;
        }
        let item = Self::base(
            ctx,
            name,
            CompletionItemKind::Symbol(SymbolKind::Package),
            score::PACKAGE,
        );
        self.add(item);
    }

    pub(crate) fn add_keyword(&mut self, ctx: &CompletionContext<'_>, keyword: &str) {
        if !Self::matches(ctx, keyword) {
            return;
        }
        let item = Self::base(ctx, keyword, CompletionItemKind::Keyword, score::KEYWORD);
        self.add(item);
    }

    /// 片段。`body` 里的换行会自动接上当前行的缩进。
    pub(crate) fn add_snippet(
        &mut self,
        ctx: &CompletionContext<'_>,
        label: &str,
        detail: &str,
        body: &str,
        base_score: i32,
    ) -> Option<&mut CompletionItem> {
        if !ctx.config.snippets || !Self::matches(ctx, label) {
            return None;
        }
        let mut item = Self::base(ctx, label, CompletionItemKind::Snippet, base_score);
        item.detail = Some(detail.to_owned());
        item.insert_text = body.replace('\n', &format!("\n{}", ctx.indent));
        item.is_snippet = true;
        self.items.push(item);
        self.items.last_mut()
    }
}
