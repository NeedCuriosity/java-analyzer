//! 补全上下文：光标在什么语法位置、周围有哪些语义信息。
//!
//! 光标处的代码通常是残缺的。做法是在光标处插入一个占位标识符后重新解析（"假树"），
//! 在假树上判断语法位置；语义信息（类型、作用域）仍然从数据库里的原始文件上取，
//! 通过文本范围把假树里光标之前的节点对应回原树。

use base_db::{Db, SourceFile};
use hir::{class_data, method_sig, BodyId, ClassId, Semantics, Ty, TypeNs, TypeScope};
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize,
};

use crate::CompletionConfig;

const MARKER: &str = "__ja_completion_marker__";

/// 类型名出现的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeLocation {
    /// `new Foo|`
    New,
    /// `@Foo|`
    Annotation,
    /// 类体里成员声明的开头
    Member,
    /// 编译单元顶层
    TopLevel,
    Other,
}

#[derive(Debug, Clone)]
pub(crate) enum ContextKind {
    /// `receiver.|` 或 `receiver::|`
    Dot {
        /// 原树里的接收者表达式（找不到对应节点时为 `None`）
        receiver: Option<ast::Expr>,
        /// 接收者是一串点分名字时的各段（`java.util.Li|` 的 `["java", "util"]`）
        receiver_path: Option<Vec<SmolStr>>,
        receiver_range: TextRange,
        /// 整个 `receiver.xxx` 独占一条语句（可以用语句级的 postfix 模板）
        statement_level: bool,
        method_ref: bool,
    },
    /// `Type::|`
    TypeMethodRef {
        ty: ast::Type,
    },
    Type {
        qualifier: Vec<SmolStr>,
        location: TypeLocation,
    },
    Import {
        qualifier: Vec<SmolStr>,
        is_static: bool,
    },
    Package {
        qualifier: Vec<SmolStr>,
    },
    /// 模块名：`import module java.|`、`requires java.|`、`exports p to m.|`
    Module {
        qualifier: Vec<SmolStr>,
    },
    /// `module-info.java` 里 `exports` / `opens` 的包名
    ModulePackage {
        qualifier: Vec<SmolStr>,
    },
    Expr {
        statement_start: bool,
        /// 位于 `case` 标签里时，被 switch 的表达式的类型
        switch_scrutinee: Option<Ty>,
    },
    AnnotationArg {
        annotation: ClassId,
    },
    /// 文档注释里的 `@tag`；`inline` 表示 `{@tag}` 形式
    JavadocTag {
        inline: bool,
    },
    /// `@param ` 之后：被注释的声明的形参名和类型参数
    JavadocParam {
        names: Vec<SmolStr>,
    },
}

pub(crate) struct CompletionContext<'a> {
    pub(crate) db: &'a dyn Db,
    pub(crate) sema: Semantics<'a>,
    pub(crate) config: &'a CompletionConfig,
    pub(crate) file: SourceFile,
    pub(crate) offset: TextSize,
    pub(crate) original_root: SyntaxNode,
    /// 光标前已经输入的标识符部分
    pub(crate) prefix: String,
    /// `prefix` 在原文里的范围
    pub(crate) source_range: TextRange,
    pub(crate) kind: ContextKind,
    pub(crate) scope: TypeScope,
    pub(crate) enclosing_class: Option<ClassId>,
    /// 光标所在的局部类 / 匿名类（在它们的类体里），由内到外
    pub(crate) local_classes: Vec<ClassId>,
    pub(crate) body: Option<BodyId>,
    pub(crate) expected_ty: Option<Ty>,
    pub(crate) in_static_context: bool,
    /// 光标（及正在输入的标识符）后面紧跟着 `(`
    pub(crate) followed_by_paren: bool,
    /// 光标所在行的缩进
    pub(crate) indent: String,
    /// `prefix` 之前的那个非空白 token
    pub(crate) prev_token: Option<SyntaxKind>,
    /// 假树里占位符后面是否补了一个分号
    pub(crate) fake_terminator: bool,
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

impl<'a> CompletionContext<'a> {
    pub(crate) fn new(
        db: &'a dyn Db,
        file: SourceFile,
        offset: TextSize,
        config: &'a CompletionConfig,
    ) -> Option<CompletionContext<'a>> {
        let text: &str = file.text(db);
        let pos = usize::from(offset);
        if pos > text.len() || !text.is_char_boundary(pos) {
            return None;
        }
        let sema = Semantics::new(db);
        let original_root = base_db::parse(db, file).syntax_node();

        // 注释和字符串里不补全；文档注释里只补全 Javadoc 标签
        let mut doc_comment = None;
        if let Some(token) = original_root.token_at_offset(offset).left_biased() {
            let range = token.text_range();
            let kind = token.kind();
            let inside = range.start() < offset && offset < range.end();
            if kind.is_trivia() && token.text().starts_with('/') {
                let line_comment = token.text().starts_with("//");
                if inside || (line_comment && offset == range.end()) {
                    if kind != SyntaxKind::DOC_COMMENT {
                        return None;
                    }
                    doc_comment = Some(token);
                }
            } else if kind.is_literal() {
                let is_number = matches!(
                    kind,
                    SyntaxKind::INT_LITERAL
                        | SyntaxKind::LONG_LITERAL
                        | SyntaxKind::FLOAT_LITERAL
                        | SyntaxKind::DOUBLE_LITERAL
                );
                if inside || (is_number && offset == range.end()) {
                    return None;
                }
            }
        }

        let prefix_len: usize = text[..pos]
            .chars()
            .rev()
            .take_while(|c| is_ident_char(*c))
            .map(char::len_utf8)
            .sum();
        let prefix = text[pos - prefix_len..pos].to_owned();
        // 数字后面不是标识符
        if prefix.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let source_range = TextRange::new(TextSize::from((pos - prefix_len) as u32), offset);

        let suffix_len: usize = text[pos..]
            .chars()
            .take_while(|c| is_ident_char(*c))
            .map(char::len_utf8)
            .sum();
        // 只看同一行：下一行开头的 `(` 属于别的语句
        let followed_by_paren = text[pos + suffix_len..]
            .trim_start_matches([' ', '\t'])
            .starts_with('(');

        let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
        let indent: String = text[line_start..pos]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();

        let prev_token =
            syntax::algo::non_trivia_token_before(&original_root, source_range.start())
                .map(|t| t.kind());

        // 原树里光标处的锚点节点，用来取作用域和所在的体
        let anchor = original_root
            .token_at_offset(offset)
            .left_biased()
            .and_then(|t| t.parent())
            .unwrap_or_else(|| original_root.clone());
        let scope = sema.type_scope(file, &anchor);
        let enclosing_class = sema.enclosing_class(file, &anchor);
        let local_classes = sema.local_classes_at(file, &anchor);
        let body = sema.body_of(file, &anchor);
        let in_static_context = match body {
            Some(BodyId::Method(m)) => class_data(db, m.class)
                .and_then(|d| d.methods.get(m.idx as usize).map(|m| m.flags.is_static()))
                .unwrap_or(false),
            Some(BodyId::Field(f)) => class_data(db, f.class)
                .and_then(|d| d.fields.get(f.idx as usize).map(|f| f.flags.is_static()))
                .unwrap_or(false),
            _ => false,
        };

        let mut ctx = CompletionContext {
            db,
            sema,
            config,
            file,
            offset,
            original_root,
            prefix,
            source_range,
            kind: ContextKind::Expr {
                statement_start: false,
                switch_scrutinee: None,
            },
            scope,
            enclosing_class,
            local_classes,
            body,
            expected_ty: None,
            in_static_context,
            followed_by_paren,
            indent,
            prev_token,
            fake_terminator: false,
        };
        if let Some(token) = doc_comment {
            ctx.kind = javadoc_context(&token, text, pos - prefix_len)?;
            return Some(ctx);
        }

        let mut fake_text = String::with_capacity(text.len() + MARKER.len());
        fake_text.push_str(&text[..pos]);
        fake_text.push_str(MARKER);
        // `foo.` 写到一半、下一行是另一条以标识符开头的语句：不补一个分号的话，
        // `foo.<占位符>` 会和下一行连成 `类型 变量名` 的声明
        let rest_of_line = text[pos..].split('\n').next().unwrap_or("");
        let next_starts_name = text[pos..].trim_start().starts_with(is_ident_char);
        if ctx.prev_token == Some(SyntaxKind::DOT)
            && rest_of_line.trim().is_empty()
            && next_starts_name
        {
            fake_text.push(';');
            ctx.fake_terminator = true;
        }
        fake_text.push_str(&text[pos..]);
        let fake_root = ast::CompilationUnit::parse(&fake_text).syntax_node();
        let fake_token = fake_root
            .token_at_offset(offset)
            .find(|t| t.kind() == SyntaxKind::IDENT && t.text().contains(MARKER))?;

        ctx.kind = ctx.classify(&fake_token)?;
        Some(ctx)
    }

    /// 假树里的节点对应的原树节点：范围相同（含光标的节点去掉占位符长度）、种类相同。
    pub(crate) fn original_node(&self, fake: &SyntaxNode) -> Option<SyntaxNode> {
        let range = fake.text_range();
        let range = if range.end() <= self.offset {
            range
        } else if range.start() <= self.offset {
            let mut end = range.end().checked_sub(TextSize::of(MARKER))?;
            // 假树里补的分号在占位符后面：跨过它的节点再减去它的长度
            if self.fake_terminator && range.end() > self.offset + TextSize::of(MARKER) {
                end = end.checked_sub(TextSize::of(';'))?;
            }
            if end < range.start() {
                return None;
            }
            TextRange::new(range.start(), end)
        } else {
            return None;
        };
        if range.is_empty() || !self.original_root.text_range().contains_range(range) {
            return None;
        }
        let covering = match self.original_root.covering_element(range) {
            syntax::NodeOrToken::Node(node) => node,
            syntax::NodeOrToken::Token(token) => token.parent()?,
        };
        covering
            .ancestors()
            .find(|n| n.text_range() == range && n.kind() == fake.kind())
    }

    pub(crate) fn original_expr(&self, fake: &ast::Expr) -> Option<ast::Expr> {
        self.original_node(fake.syntax()).and_then(ast::Expr::cast)
    }

    fn resolve_fake_type(&self, ty: &ast::Type) -> Ty {
        let node = self.original_node(ty.syntax()).and_then(ast::Type::cast);
        self.sema
            .resolve_type(self.file, node.as_ref().unwrap_or(ty))
    }

    fn classify(&mut self, fake_token: &SyntaxToken) -> Option<ContextKind> {
        let name_ref = fake_token.parent()?;
        if name_ref.kind() != SyntaxKind::NAME_REF {
            // 声明处的名字（`int fo|`）等位置不补全
            return None;
        }
        let parent = name_ref.parent()?;
        match parent.kind() {
            SyntaxKind::FIELD_ACCESS => {
                let access = ast::FieldAccess::cast(parent.clone())?;
                let receiver = access.receiver()?;
                self.expected_ty = self.expected_type_of(&parent);
                Some(self.dot_context(&parent, receiver, false))
            }
            SyntaxKind::METHOD_CALL => {
                let call = ast::MethodCall::cast(parent.clone())?;
                match call.receiver() {
                    Some(receiver) => {
                        self.expected_ty = self.expected_type_of(&parent);
                        Some(self.dot_context(&parent, receiver, false))
                    }
                    // 不带接收者的调用 `foo|(...)`
                    None => {
                        self.expected_ty = self.expected_type_of(&parent);
                        Some(self.expr_context(&parent))
                    }
                }
            }
            SyntaxKind::METHOD_REF => {
                let method_ref = ast::MethodRef::cast(parent.clone())?;
                if let Some(receiver) = method_ref.receiver_expr() {
                    Some(self.dot_context(&parent, receiver, true))
                } else {
                    let ty = method_ref.receiver_type()?;
                    let ty = self
                        .original_node(ty.syntax())
                        .and_then(ast::Type::cast)
                        .unwrap_or(ty);
                    Some(ContextKind::TypeMethodRef { ty })
                }
            }
            SyntaxKind::NAME_EXPR => {
                self.expected_ty = self.expected_type_of(&parent);
                Some(self.expr_context(&parent))
            }
            SyntaxKind::CLASS_TYPE => {
                let class_ty = ast::ClassType::cast(parent.clone())?;
                let qualifier = class_ty
                    .qualifier()
                    .map(|q| q.segments())
                    .unwrap_or_default();
                // 找到最外层的 CLASS_TYPE（`a.b.C|` 的限定符是嵌套在里面的）
                let outer = parent.clone();
                let owner = outer.parent()?;
                let location = match owner.kind() {
                    SyntaxKind::NEW_EXPR => TypeLocation::New,
                    SyntaxKind::COMPILATION_UNIT => TypeLocation::TopLevel,
                    SyntaxKind::CLASS_BODY => TypeLocation::Member,
                    SyntaxKind::FIELD_DECL | SyntaxKind::METHOD_DECL | SyntaxKind::ERROR => {
                        match owner.parent().map(|p| p.kind()) {
                            Some(SyntaxKind::CLASS_BODY) => TypeLocation::Member,
                            Some(SyntaxKind::COMPILATION_UNIT) => TypeLocation::TopLevel,
                            _ if owner.kind() == SyntaxKind::ERROR => TypeLocation::Other,
                            _ => TypeLocation::Other,
                        }
                    }
                    // 语句开头的 `Foo|` 会被解析成局部变量声明的类型：按表达式位置处理更有用
                    SyntaxKind::LOCAL_VAR_DECL if qualifier.is_empty() => {
                        let has_declarator = owner
                            .children()
                            .any(|n| n.kind() == SyntaxKind::VARIABLE_DECLARATOR);
                        // `ret|` 后面换行接着另一条语句（`ret|\n names.clear();`）时，下一行的
                        // 第一个名字会被当成变量名：声明的类型和名字不在同一行就不当它是声明
                        let split_across_lines = owner
                            .children()
                            .find(|n| n.kind() == SyntaxKind::VARIABLE_DECLARATOR)
                            .and_then(|d| {
                                let between = TextRange::new(
                                    parent.text_range().end(),
                                    d.text_range().start(),
                                );
                                let text = owner.text().to_string();
                                let start =
                                    usize::from(between.start() - owner.text_range().start());
                                let end = usize::from(between.end() - owner.text_range().start());
                                text.get(start..end).map(|gap| gap.contains('\n'))
                            })
                            .unwrap_or(false);
                        if !has_declarator || split_across_lines {
                            return Some(ContextKind::Expr {
                                statement_start: true,
                                switch_scrutinee: None,
                            });
                        }
                        TypeLocation::Other
                    }
                    _ => TypeLocation::Other,
                };
                // `new` 之后按期望类型推荐实现类
                if location == TypeLocation::New {
                    self.expected_ty = self.expected_type_of(&owner);
                }
                Some(ContextKind::Type {
                    qualifier,
                    location,
                })
            }
            SyntaxKind::QUALIFIED_NAME => {
                let qualified = ast::QualifiedName::cast(parent.clone())?;
                let qualifier = qualified
                    .qualifier()
                    .map(|q| q.segments())
                    .unwrap_or_default();
                let owner = parent
                    .ancestors()
                    .find(|n| n.kind() != SyntaxKind::QUALIFIED_NAME)?;
                match owner.kind() {
                    SyntaxKind::IMPORT_DECL => {
                        let import = ast::ImportDecl::cast(owner)?;
                        if import.is_module() {
                            return Some(ContextKind::Module { qualifier });
                        }
                        Some(ContextKind::Import {
                            qualifier,
                            is_static: import.is_static(),
                        })
                    }
                    SyntaxKind::REQUIRES_DIRECTIVE => Some(ContextKind::Module { qualifier }),
                    SyntaxKind::EXPORTS_DIRECTIVE | SyntaxKind::OPENS_DIRECTIVE => {
                        // 第一个名字是包，`to` 后面的是模块
                        let outermost = parent
                            .ancestors()
                            .take_while(|n| n.kind() == SyntaxKind::QUALIFIED_NAME)
                            .last()?;
                        let is_package = owner
                            .children()
                            .find(|c| c.kind() == SyntaxKind::QUALIFIED_NAME)
                            .is_some_and(|first| first == outermost);
                        Some(if is_package {
                            ContextKind::ModulePackage { qualifier }
                        } else {
                            ContextKind::Module { qualifier }
                        })
                    }
                    SyntaxKind::PACKAGE_DECL => Some(ContextKind::Package { qualifier }),
                    SyntaxKind::ANNOTATION => Some(ContextKind::Type {
                        qualifier,
                        location: TypeLocation::Annotation,
                    }),
                    _ => None,
                }
            }
            SyntaxKind::ANNOTATION_ARG => {
                let annotation = parent.ancestors().find_map(ast::Annotation::cast)?;
                let segments = annotation.name()?.segments();
                match hir::resolve_type_path(self.db, &self.scope, &segments)? {
                    TypeNs::Class(class) => Some(ContextKind::AnnotationArg { annotation: class }),
                    TypeNs::TypeParam(_) => None,
                }
            }
            _ => None,
        }
    }

    fn dot_context(
        &self,
        access: &SyntaxNode,
        fake_receiver: ast::Expr,
        method_ref: bool,
    ) -> ContextKind {
        let receiver_range = fake_receiver.syntax().text_range();
        let statement_level = access
            .parent()
            .is_some_and(|p| p.kind() == SyntaxKind::EXPR_STMT);
        ContextKind::Dot {
            receiver: self.original_expr(&fake_receiver),
            receiver_path: expr_path(&fake_receiver),
            receiver_range,
            statement_level,
            method_ref,
        }
    }

    fn expr_context(&self, fake_expr: &SyntaxNode) -> ContextKind {
        let statement_start = fake_expr
            .parent()
            .is_some_and(|p| p.kind() == SyntaxKind::EXPR_STMT);
        let switch_scrutinee = fake_expr
            .ancestors()
            .take_while(|n| !matches!(n.kind(), SyntaxKind::BLOCK | SyntaxKind::SWITCH_CASE))
            .find(|n| n.kind() == SyntaxKind::SWITCH_LABEL)
            .and_then(|label| {
                let switch = label.ancestors().find(|n| {
                    matches!(n.kind(), SyntaxKind::SWITCH_STMT | SyntaxKind::SWITCH_EXPR)
                })?;
                let scrutinee = switch.children().find_map(ast::Expr::cast)?;
                let original = self.original_expr(&scrutinee)?;
                self.sema.type_of_expr(self.file, &original)
            });
        ContextKind::Expr {
            statement_start,
            switch_scrutinee,
        }
    }

    /// 假树里的表达式节点所处位置期望的类型（只覆盖最常见的几种位置）。
    fn expected_type_of(&self, fake_expr: &SyntaxNode) -> Option<Ty> {
        let parent = fake_expr.parent()?;
        let ty = match parent.kind() {
            SyntaxKind::VARIABLE_DECLARATOR => {
                let decl = parent.parent()?;
                let ty = decl.children().find_map(ast::Type::cast)?;
                self.resolve_fake_type(&ty)
            }
            SyntaxKind::RETURN_STMT => match self.body? {
                BodyId::Method(method) => method_sig(self.db, method).ret.clone(),
                _ => return None,
            },
            SyntaxKind::ASSIGN_EXPR => {
                let assign = ast::AssignExpr::cast(parent)?;
                let lhs = assign.lhs()?;
                if lhs.syntax() == fake_expr {
                    return None;
                }
                let lhs = self.original_expr(&lhs)?;
                self.sema.type_of_expr(self.file, &lhs)?
            }
            SyntaxKind::IF_STMT | SyntaxKind::WHILE_STMT | SyntaxKind::DO_WHILE_STMT => Ty::BOOLEAN,
            // 实参位置：被调用方法对应形参的类型
            SyntaxKind::ARG_LIST => {
                let idx = parent
                    .children()
                    .filter(|n| ast::Expr::can_cast(n.kind()))
                    .position(|n| &n == fake_expr)?;
                let original = self.original_node(&parent)?;
                let result = hir::infer(self.db, self.body?);
                let info = result.call_info(original.text_range())?;
                // 形参类型按接收者代换（`Stream<String>.map` 的形参是 `Function<? super String, …>`）
                let candidate = info
                    .candidates
                    .iter()
                    .find(|c| Some(c.id) == info.chosen)
                    .or_else(|| info.candidates.first());
                let method = info.chosen.or_else(|| candidate.map(|c| c.id))?;
                let sig = method_sig(self.db, method);
                let params = match candidate.filter(|c| c.id == method) {
                    Some(c) => c.param_tys(self.db),
                    None => sig.params.clone(),
                };
                let last = params.len().checked_sub(1)?;
                match params.get(idx.min(last))? {
                    Ty::Array(element) if sig.varargs && idx >= last => (**element).clone(),
                    other => other.clone(),
                }
            }
            _ => return None,
        };
        (!ty.is_unknown() && ty != Ty::Void).then_some(ty)
    }

    /// 由内到外的各层类（光标在局部类 / 匿名类里时从它们开始）。
    pub(crate) fn enclosing_classes(&self) -> Vec<ClassId> {
        let mut res = Vec::new();
        // 局部类 / 匿名类的外层链最终也到具名类
        let mut current = self.local_classes.first().copied().or(self.enclosing_class);
        while let Some(class) = current {
            res.push(class);
            current = class_data(self.db, class)
                .and_then(|d| d.outer.clone())
                .and_then(|outer| hir::lookup_class(self.db, &outer));
        }
        res
    }
}

/// 表达式是一串点分名字（`a`、`a.b.c`）时返回各段。
fn expr_path(expr: &ast::Expr) -> Option<Vec<SmolStr>> {
    match expr {
        ast::Expr::NameExpr(name) => Some(vec![name.name_ref()?.text()]),
        ast::Expr::FieldAccess(access) => {
            let mut path = expr_path(&access.receiver()?)?;
            path.push(access.name_ref()?.text());
            Some(path)
        }
        _ => None,
    }
}

/// 文档注释里光标前的内容决定补全什么。`word_start` 是正在输入的词的起始字节偏移。
fn javadoc_context(token: &SyntaxToken, text: &str, word_start: usize) -> Option<ContextKind> {
    let line_start = text[..word_start].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..word_start];

    if let Some(head) = before.strip_suffix('@') {
        if head.ends_with('{') {
            return Some(ContextKind::JavadocTag { inline: true });
        }
        // 块标签只能出现在行首（前面只有空白和 `*`，或注释的开头 `/**`）
        let is_line_start = head
            .trim()
            .trim_start_matches('/')
            .chars()
            .all(|c| c == '*');
        return is_line_start.then_some(ContextKind::JavadocTag { inline: false });
    }

    let content = before
        .trim()
        .trim_start_matches('/')
        .trim_start_matches('*')
        .trim();
    if content == "@param" && before.ends_with(char::is_whitespace) {
        let owner = token.parent()?;
        let mut names: Vec<SmolStr> = Vec::new();
        if let Some(params) = owner.children().find_map(ast::ParamList::cast) {
            names.extend(params.params().filter_map(|p| p.name()).map(|n| n.text()));
        }
        if let Some(type_params) = owner.children().find_map(ast::TypeParamList::cast) {
            names.extend(
                type_params
                    .type_params()
                    .filter_map(|p| p.name())
                    .map(|n| SmolStr::new(format!("<{}>", n.text()))),
            );
        }
        return Some(ContextKind::JavadocParam { names });
    }
    None
}
