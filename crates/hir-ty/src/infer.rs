//! 方法体级别的推断：名字解析 + 表达式类型。
//!
//! 推断直接在语法树上进行，结果按节点的文本范围索引。一个"体"（[`BodyId`]）是一个方法、
//! 构造器、字段初始化式或初始化块；匿名类和局部类的成员在所属的体里就地推断。
//!
//! 每个体是一个独立的惰性查询：只有被请求到的体才会计算；别的文件的体只依赖签名级查询，
//! 所以在某个方法体里编辑不会让其它文件的推断结果失效。

mod expr;

use std::sync::Arc;

use base_db::{Db, SourceFile};
use hir_def::{
    class_data, class_origin, item_tree, lower_type, source_map, ClassId, ClassOrigin, FieldId,
    ItemTree, MethodId, Name, TypeParamId, TypeScope,
};
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;
use syntax::{
    ast::{self, AstNode},
    SyntaxNode, TextRange,
};

use crate::{
    lower::{class_supertypes, field_ty, lower_type_ref, method_sig, this_ty},
    members::MethodCandidate,
    subtype::{find_super, is_assignable},
    ty::Ty,
};

/// 一段可以独立推断的代码。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BodyId {
    Method(MethodId),
    /// 字段初始化式，或枚举常量的实参与类体
    Field(FieldId),
    /// 类里第 `idx` 个初始化块
    Initializer {
        class: ClassId,
        idx: u32,
    },
}

impl BodyId {
    pub fn class(self) -> ClassId {
        match self {
            BodyId::Method(m) => m.class,
            BodyId::Field(f) => f.class,
            BodyId::Initializer { class, .. } => class,
        }
    }
}

pub type LocalId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LocalKind {
    Param,
    Local,
    LambdaParam,
    CatchParam,
    Pattern,
    Resource,
    ForeachVar,
    /// 匿名类 / 局部类里声明的字段
    LocalClassField,
    /// 匿名类 / 局部类里声明的方法；`ty` 是它的返回类型
    LocalClassMethod,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct LocalData {
    pub name: Name,
    pub ty: Ty,
    /// 声明处名字节点的范围
    pub decl_range: TextRange,
    pub kind: LocalKind,
}

/// 一个名字引用解析到了什么。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Resolution {
    Local(LocalId),
    Field(FieldId),
    Method(MethodId),
    Class(ClassId),
    TypeParam(TypeParamId),
    Package(SmolStr),
    /// 数组的 `length`
    ArrayLength,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum InferDiagnosticKind {
    UnresolvedName {
        name: Name,
    },
    UnresolvedField {
        name: Name,
        receiver: String,
    },
    UnresolvedMethod {
        name: Name,
        receiver: Option<String>,
    },
    TypeMismatch {
        expected: String,
        actual: String,
    },
    /// 没有一个重载能接受这组实参
    ArgumentMismatch {
        name: Name,
        args: String,
    },
    /// 运算符不能用在这样的操作数上
    BadOperand {
        op: String,
        operands: String,
    },
    /// 同一个方法体里已经有同名的局部变量 / 形参
    DuplicateLocal {
        name: Name,
    },
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct InferDiagnostic {
    pub range: TextRange,
    pub kind: InferDiagnosticKind,
}

/// 一次调用的重载信息，供签名帮助使用。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CallInfo {
    pub candidates: Vec<MethodCandidate>,
    pub chosen: Option<MethodId>,
}

#[derive(Default, PartialEq, Eq, Debug)]
pub struct InferenceResult {
    /// 表达式节点范围 → 类型
    expr_types: FxHashMap<TextRange, Ty>,
    /// 名字引用（`NAME_REF`）范围 → 解析结果。
    /// 显式构造器调用 `this(...)` / `super(...)` 以 `this` / `super` 表达式的范围为键。
    resolutions: FxHashMap<TextRange, Resolution>,
    locals: Vec<LocalData>,
    /// 声明处名字节点范围 → 局部变量
    local_decls: FxHashMap<TextRange, LocalId>,
    /// 实参列表（`ARG_LIST`）范围 → 调用信息
    calls: FxHashMap<TextRange, CallInfo>,
    diagnostics: Vec<InferDiagnostic>,
}

impl InferenceResult {
    pub fn type_of(&self, expr_range: TextRange) -> Option<&Ty> {
        self.expr_types.get(&expr_range)
    }

    pub fn resolution(&self, name_range: TextRange) -> Option<&Resolution> {
        self.resolutions.get(&name_range)
    }

    pub fn local(&self, id: LocalId) -> &LocalData {
        &self.locals[id as usize]
    }

    pub fn locals(&self) -> &[LocalData] {
        &self.locals
    }

    pub fn local_at_decl(&self, name_range: TextRange) -> Option<LocalId> {
        self.local_decls.get(&name_range).copied()
    }

    pub fn call_info(&self, arg_list_range: TextRange) -> Option<&CallInfo> {
        self.calls.get(&arg_list_range)
    }

    pub fn diagnostics(&self) -> &[InferDiagnostic] {
        &self.diagnostics
    }

    pub fn expr_types(&self) -> impl Iterator<Item = (TextRange, &Ty)> {
        self.expr_types.iter().map(|(range, ty)| (*range, ty))
    }

    pub fn resolutions(&self) -> impl Iterator<Item = (TextRange, &Resolution)> {
        self.resolutions.iter().map(|(range, res)| (*range, res))
    }
}

/// 体对应的语法节点（方法 / 构造器声明、变量声明符、枚举常量或初始化块）。
pub fn body_syntax(db: &dyn Db, body: BodyId) -> Option<(SourceFile, SyntaxNode)> {
    let class = body.class();
    let ClassOrigin::Source { file, .. } = class_origin(db, class)? else {
        return None;
    };
    let data = class_data(db, class)?;
    let root = base_db::parse(db, file).syntax_node();
    let map = source_map(db, file);
    let node = match body {
        BodyId::Method(m) => map.to_node(data.methods.get(m.idx as usize)?.decl?, &root)?,
        BodyId::Field(f) => map.to_node(data.fields.get(f.idx as usize)?.decl?, &root)?,
        BodyId::Initializer { idx, .. } => {
            let class_node = map.to_node(data.decl?, &root)?;
            let decl = ast::TypeDecl::cast(class_node)?;
            decl.body()?
                .members()
                .filter_map(|m| match m {
                    ast::Member::InitializerBlock(it) => Some(it),
                    _ => None,
                })
                .nth(idx as usize)?
                .syntax()
                .clone()
        }
    };
    Some((file, node))
}

pub fn infer(db: &dyn Db, body: BodyId) -> Arc<InferenceResult> {
    infer_tracked(db, base_db::workspace(db), body)
}

// salsa 的追踪函数要求第一个参数是 salsa 结构体；键是普通值时借工作区单例占位，
// 多参数的追踪函数会自动把参数驻留。
#[salsa::tracked]
fn infer_tracked(db: &dyn Db, _ws: base_db::Workspace, body: BodyId) -> Arc<InferenceResult> {
    let Some((file, node)) = body_syntax(db, body) else {
        return Arc::default();
    };
    let mut cx = InferCtx::new(db, file, body);
    cx.infer_body(body, &node);
    Arc::new(cx.result)
}

/// 一层"当前类"。最外层是体所属的具名类，匿名类 / 局部类的类体各压一层。
struct Frame {
    this_ty: Ty,
}

pub(crate) struct InferCtx<'a> {
    db: &'a dyn Db,
    tree: Arc<ItemTree>,
    scope: TypeScope,
    class: ClassId,
    frames: Vec<Frame>,
    scopes: Vec<FxHashMap<Name, LocalId>>,
    /// 每一层帧开始时 `scopes` 的深度：匿名类 / 局部类的类体可以重新声明外层的局部变量
    frame_scope_base: Vec<usize>,
    /// 下一次重载决议失败时可以报"实参不匹配"的方法名（由调用方在确认候选完整之后设置）
    pub(crate) report_inapplicable: Option<Name>,
    /// 当前函数体（方法或 lambda）里各 `return` 的类型
    return_tys: Vec<Vec<Ty>>,
    /// 当前函数体期望的返回类型
    ret_expected: Vec<Option<Ty>>,
    /// 当前 switch 表达式各分支的结果类型
    yield_tys: Vec<Vec<Ty>>,
    /// 是否抑制"无法解析"类诊断（没有 JDK、文件用了 Lombok 等会凭空生成成员的工具）
    suppress_unresolved: bool,
    /// 体里声明的局部类 / 记录 / 接口 / 枚举的名字
    local_type_names: FxHashSet<Name>,
    result: InferenceResult,
}

impl<'a> InferCtx<'a> {
    fn new(db: &'a dyn Db, file: SourceFile, body: BodyId) -> InferCtx<'a> {
        let class = body.class();
        let tree = item_tree(db, file);
        let scope = match body {
            BodyId::Method(m) => TypeScope::method(db, m),
            _ => TypeScope::class(db, class),
        };
        let no_jdk = Ty::object(db).is_unknown();
        let generated_members = tree
            .imports
            .iter()
            .any(|i| i.segments.first().is_some_and(|s| s == "lombok"));
        InferCtx {
            db,
            tree,
            scope,
            class,
            frames: vec![Frame {
                this_ty: this_ty(db, class),
            }],
            scopes: vec![FxHashMap::default()],
            frame_scope_base: vec![0],
            report_inapplicable: None,
            return_tys: Vec::new(),
            ret_expected: Vec::new(),
            yield_tys: Vec::new(),
            suppress_unresolved: no_jdk || generated_members,
            local_type_names: FxHashSet::default(),
            result: InferenceResult::default(),
        }
    }

    fn infer_body(&mut self, body: BodyId, node: &SyntaxNode) {
        self.local_type_names = node
            .descendants()
            .filter(|n| *n != *node)
            .filter_map(ast::TypeDecl::cast)
            .filter_map(|decl| decl.name())
            .map(|name| name.text())
            .collect();
        match body {
            BodyId::Method(method) => {
                let sig = method_sig(self.db, method);
                if let Some(decl) = ast::MethodDecl::cast(node.clone()) {
                    self.declare_params(decl.param_list(), &sig.params);
                    self.ret_expected.push(Some(sig.ret.clone()));
                    if let Some(block) = decl.body() {
                        self.infer_block(&block);
                    }
                    if let Some(default) = decl.default_value().and_then(|d| d.expr()) {
                        self.infer_expr(&default, Some(&sig.ret));
                    }
                    self.ret_expected.pop();
                } else if let Some(decl) = ast::ConstructorDecl::cast(node.clone()) {
                    match decl.param_list() {
                        Some(list) => self.declare_params(Some(list), &sig.params),
                        // 记录的紧凑构造器：形参是记录的全部组件
                        None => {
                            let components = decl
                                .syntax()
                                .ancestors()
                                .find_map(ast::TypeDecl::cast)
                                .and_then(|class| class.record_components());
                            self.declare_params(components, &sig.params);
                        }
                    }
                    self.ret_expected.push(Some(Ty::Void));
                    if let Some(block) = decl.body() {
                        self.infer_block(&block);
                    }
                    self.ret_expected.pop();
                }
            }
            BodyId::Field(field) => {
                let ty = field_ty(self.db, field);
                if let Some(declarator) = ast::VariableDeclarator::cast(node.clone()) {
                    if let Some(init) = declarator.initializer() {
                        let actual = self.infer_expr(&init, Some(&ty));
                        self.check_assignable(init.syntax().text_range(), &actual, &ty, &init);
                    }
                } else if let Some(constant) = ast::EnumConstant::cast(node.clone()) {
                    let class_ty = Ty::class(field.class);
                    if let Some(args) = constant.arg_list() {
                        let key = constant.name().map(|n| n.syntax().text_range());
                        self.infer_constructor_call(
                            field.class,
                            &class_ty,
                            false,
                            &args,
                            None,
                            key,
                        );
                    }
                    if let Some(body) = constant.body() {
                        self.infer_class_body(&body, class_ty);
                    }
                }
            }
            BodyId::Initializer { .. } => {
                if let Some(block) =
                    ast::InitializerBlock::cast(node.clone()).and_then(|b| b.body())
                {
                    self.ret_expected.push(None);
                    self.infer_block(&block);
                    self.ret_expected.pop();
                }
            }
        }
    }

    // ---- 作用域与局部变量 ----

    fn push_scope(&mut self) {
        self.scopes.push(FxHashMap::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &ast::Name, ty: Ty, kind: LocalKind) -> LocalId {
        let id = self.result.locals.len() as LocalId;
        let text = name.text();
        let decl_range = name.syntax().text_range();
        // 局部变量不能遮蔽同一个方法体里的另一个局部变量或形参（JLS §6.4）。
        // 模式变量的作用域取决于控制流，这里不建模，不参与判断。
        let declares_variable = matches!(
            kind,
            LocalKind::Local
                | LocalKind::LambdaParam
                | LocalKind::CatchParam
                | LocalKind::ForeachVar
                | LocalKind::Resource
        );
        if declares_variable && text != "_" {
            let base = self.frame_scope_base.last().copied().unwrap_or(0);
            let clash = self.scopes[base.min(self.scopes.len())..]
                .iter()
                .rev()
                .find_map(|scope| scope.get(text.as_str()).copied())
                .is_some_and(|existing| {
                    matches!(
                        self.result.locals[existing as usize].kind,
                        LocalKind::Param
                            | LocalKind::Local
                            | LocalKind::LambdaParam
                            | LocalKind::CatchParam
                            | LocalKind::ForeachVar
                            | LocalKind::Resource
                    )
                });
            if clash {
                self.diagnostic(
                    decl_range,
                    InferDiagnosticKind::DuplicateLocal { name: text.clone() },
                );
            }
        }
        self.result.locals.push(LocalData {
            name: text.clone(),
            ty,
            decl_range,
            kind,
        });
        self.result.local_decls.insert(decl_range, id);
        // 方法和变量是两个名字空间：方法以 `名字()` 为键，
        // 这样同名的字段和方法（`boolean hasNext; boolean hasNext()`）互不覆盖
        let key = if kind == LocalKind::LocalClassMethod {
            Name::from(format!("{text}()"))
        } else {
            text
        };
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(key, id);
        }
        id
    }

    /// 匿名类 / 局部类自己声明的方法。
    fn lookup_local_method(&self, name: &str) -> Option<LocalId> {
        self.lookup_local(&format!("{name}()"))
    }

    fn lookup_local(&self, name: &str) -> Option<LocalId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn declare_params(&mut self, list: Option<ast::ParamList>, tys: &[Ty]) {
        let Some(list) = list else { return };
        let named = list.params().filter_map(|p| p.name());
        for (i, name) in named.enumerate() {
            let ty = tys.get(i).cloned().unwrap_or(Ty::Unknown);
            self.declare(&name, ty, LocalKind::Param);
        }
    }

    // ---- 类型 ----

    fn lower_ty(&self, ty: &ast::Type) -> Ty {
        self.lower_ref(&lower_type(ty))
    }

    fn lower_ref(&self, type_ref: &hir_def::TypeRef) -> Ty {
        // 匿名类 / 局部类的类体里，简单名可以指向从父类型继承来的成员类型
        if self.frames.len() > 1 {
            if let hir_def::TypeRef::Path(segments) = type_ref {
                if let [segment] = segments.as_slice() {
                    let inherited = self
                        .frames
                        .iter()
                        .skip(1)
                        .rev()
                        .find_map(|frame| match &frame.this_ty {
                            Ty::Class(class, _) => {
                                hir_def::member_type(self.db, *class, &segment.name)
                            }
                            _ => None,
                        });
                    if let (Some(class), true) = (inherited, segment.args.is_empty()) {
                        return Ty::class(class);
                    }
                }
            }
        }
        lower_type_ref(self.db, &self.scope, type_ref)
    }

    fn this_ty(&self) -> Ty {
        self.frames
            .last()
            .map_or(Ty::Unknown, |f| f.this_ty.clone())
    }

    fn super_ty(&self) -> Ty {
        // 匿名类 / 局部类 / 枚举常量的类体：这一层记录的 `this` 类型就是它的父类型
        if self.frames.len() > 1 {
            return self.this_ty();
        }
        match self.this_ty() {
            Ty::Class(class, args) => {
                let subst = crate::ty::Subst::for_class(self.db, class, &args);
                class_supertypes(self.db, class)
                    .first()
                    .map(|sup| sup.subst(&subst))
                    .unwrap_or_else(|| Ty::object(self.db))
            }
            _ => Ty::Unknown,
        }
    }

    /// `for (T x : iterable)` 里元素的类型。
    fn iterable_element(&self, iterable: &Ty) -> Ty {
        match iterable {
            Ty::Array(element) => (**element).clone(),
            Ty::Unknown => Ty::Unknown,
            _ => {
                let Some(iterable_class) = hir_def::lookup_class(self.db, "java/lang/Iterable")
                else {
                    return Ty::Unknown;
                };
                match find_super(self.db, iterable, iterable_class) {
                    Some(found) => match found.type_args().first() {
                        Some(arg) => arg.upper_bound(self.db),
                        // 原始类型的 Iterable：元素是 Object
                        None => Ty::object(self.db),
                    },
                    None => Ty::Unknown,
                }
            }
        }
    }

    // ---- 诊断 ----

    fn diagnostic(&mut self, range: TextRange, kind: InferDiagnosticKind) {
        self.result
            .diagnostics
            .push(InferDiagnostic { range, kind });
    }

    /// "简单"类型：基本类型或非泛型类。类型检查只对两边都简单的情形下结论。
    pub(crate) fn is_simple_ty(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Primitive(_) => true,
            Ty::Class(class, args) => {
                args.is_empty()
                    && class_data(self.db, *class).is_some_and(|d| d.type_params.is_empty())
            }
            _ => false,
        }
    }

    /// 推断条件表达式，类型确定不是 boolean 时报错。
    pub(crate) fn infer_condition(&mut self, cond: &ast::Expr) {
        let ty = self.infer_expr(cond, Some(&Ty::BOOLEAN));
        let definite = matches!(
            cond,
            ast::Expr::Literal(_)
                | ast::Expr::NameExpr(_)
                | ast::Expr::FieldAccess(_)
                | ast::Expr::MethodCall(_)
                | ast::Expr::NewExpr(_)
                | ast::Expr::BinaryExpr(_)
        );
        if definite
            && self.is_simple_ty(&ty)
            && ty.unboxed(self.db) != Some(hir_def::PrimitiveType::Boolean)
        {
            self.diagnostic(
                cond.syntax().text_range(),
                InferDiagnosticKind::TypeMismatch {
                    expected: Ty::BOOLEAN.display(self.db),
                    actual: ty.display(self.db),
                },
            );
        }
    }

    /// 只在两边类型都完全确定、且都是"简单"类型（基本类型或非泛型类）时才报类型不匹配。
    fn check_assignable(&mut self, range: TextRange, from: &Ty, to: &Ty, expr: &ast::Expr) {
        // `int x = voidMethod();`
        if matches!(from, Ty::Void)
            && matches!(expr, ast::Expr::MethodCall(_))
            && !to.contains_unknown()
            && !matches!(to, Ty::Void)
        {
            self.diagnostic(
                range,
                InferDiagnosticKind::TypeMismatch {
                    expected: to.display(self.db),
                    actual: from.display(self.db),
                },
            );
            return;
        }
        if !self.is_simple_ty(from) || !self.is_simple_ty(to) {
            return;
        }
        if is_assignable(self.db, from, to) {
            return;
        }
        // 整型常量表达式可以隐式收窄到 byte / short / char（及其包装类），这里不做常量求值，直接放过
        let from_small_int = matches!(
            from.as_primitive(),
            Some(
                hir_def::PrimitiveType::Int
                    | hir_def::PrimitiveType::Short
                    | hir_def::PrimitiveType::Char
                    | hir_def::PrimitiveType::Byte
            )
        );
        let to_small_int = matches!(
            to.unboxed(self.db),
            Some(
                hir_def::PrimitiveType::Byte
                    | hir_def::PrimitiveType::Short
                    | hir_def::PrimitiveType::Char
            )
        );
        if from_small_int && to_small_int {
            return;
        }
        // lambda、方法引用、switch、条件表达式等的类型取决于上下文，不在这里判断
        if !matches!(
            expr,
            ast::Expr::Literal(_)
                | ast::Expr::NameExpr(_)
                | ast::Expr::FieldAccess(_)
                | ast::Expr::MethodCall(_)
                | ast::Expr::NewExpr(_)
                | ast::Expr::BinaryExpr(_)
                | ast::Expr::CastExpr(_)
        ) {
            return;
        }
        self.diagnostic(
            range,
            InferDiagnosticKind::TypeMismatch {
                expected: to.display(self.db),
                actual: from.display(self.db),
            },
        );
    }

    // ---- 语句 ----

    fn infer_block(&mut self, block: &ast::Block) {
        self.push_scope();
        for stmt in block.statements() {
            self.infer_stmt(&stmt);
        }
        self.pop_scope();
    }

    fn infer_local_var_decl(&mut self, decl: &ast::LocalVarDecl, kind: LocalKind) {
        let ty_node = decl.ty();
        let is_var = ty_node.as_ref().is_some_and(|t| lower_type(t).is_var());
        let base = match &ty_node {
            Some(t) if !is_var => self.lower_ty(t),
            _ => Ty::Unknown,
        };
        for declarator in decl.declarators() {
            let mut declared = base.clone();
            for _ in 0..declarator.extra_dims() {
                declared = Ty::array(declared);
            }
            let mut ty = declared.clone();
            if let Some(init) = declarator.initializer() {
                let expected = if is_var { None } else { Some(&declared) };
                let actual = self.infer_expr(&init, expected);
                if is_var {
                    // `var x = new T() { ... }`：变量的类型是匿名类本身，它自己声明的成员
                    // 这里表示不出来，按未知处理，避免把 `x.ownMember` 报成找不到
                    let anonymous = matches!(&init, ast::Expr::NewExpr(n) if n.body().is_some());
                    ty = match actual {
                        Ty::Null | Ty::Void => Ty::Unknown,
                        _ if anonymous => Ty::Unknown,
                        other => other,
                    };
                } else {
                    self.check_assignable(init.syntax().text_range(), &actual, &declared, &init);
                }
            }
            if let Some(name) = declarator.name() {
                self.declare(&name, ty, kind);
            }
        }
    }

    fn infer_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::Block(block) => self.infer_block(block),
            ast::Stmt::LocalVarDecl(decl) => self.infer_local_var_decl(decl, LocalKind::Local),
            ast::Stmt::ExprStmt(it) => {
                if let Some(expr) = it.expr() {
                    self.infer_expr(&expr, None);
                }
            }
            ast::Stmt::IfStmt(it) => {
                if let Some(cond) = it.condition() {
                    self.infer_condition(&cond);
                }
                if let Some(then) = it.then_branch() {
                    self.infer_stmt(&then);
                }
                if let Some(otherwise) = it.else_branch() {
                    self.infer_stmt(&otherwise);
                }
            }
            ast::Stmt::WhileStmt(it) => {
                if let Some(cond) = it.condition() {
                    self.infer_condition(&cond);
                }
                if let Some(body) = it.body() {
                    self.infer_stmt(&body);
                }
            }
            ast::Stmt::DoWhileStmt(it) => {
                if let Some(body) = it.body() {
                    self.infer_stmt(&body);
                }
                if let Some(cond) = it.condition() {
                    self.infer_condition(&cond);
                }
            }
            ast::Stmt::ForStmt(it) => {
                self.push_scope();
                if let Some(init) = it.init() {
                    if let Some(decl) = init.local_var_decl() {
                        self.infer_local_var_decl(&decl, LocalKind::Local);
                    } else {
                        for expr in init.exprs() {
                            self.infer_expr(&expr, None);
                        }
                    }
                }
                if let Some(cond) = it.condition() {
                    self.infer_condition(&cond);
                }
                if let Some(update) = it.update() {
                    for expr in update.exprs() {
                        self.infer_expr(&expr, None);
                    }
                }
                if let Some(body) = it.body() {
                    self.infer_stmt(&body);
                }
                self.pop_scope();
            }
            ast::Stmt::ForeachStmt(it) => {
                self.push_scope();
                let iterable = it
                    .iterable()
                    .map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                if let Some(param) = it.param() {
                    let ty = match param.ty() {
                        Some(t) if !lower_type(&t).is_var() => self.lower_ty(&t),
                        _ => self.iterable_element(&iterable),
                    };
                    if let Some(name) = param.name() {
                        self.declare(&name, ty, LocalKind::ForeachVar);
                    }
                }
                if let Some(body) = it.body() {
                    self.infer_stmt(&body);
                }
                self.pop_scope();
            }
            ast::Stmt::ReturnStmt(it) => {
                if let Some(expr) = it.expr() {
                    let expected = self.ret_expected.last().cloned().flatten();
                    let ty = self.infer_expr(&expr, expected.as_ref());
                    if let Some(returns) = self.return_tys.last_mut() {
                        returns.push(ty);
                    } else if let Some(expected) = &expected {
                        self.check_assignable(expr.syntax().text_range(), &ty, expected, &expr);
                    }
                }
            }
            ast::Stmt::ThrowStmt(it) => {
                if let Some(expr) = it.expr() {
                    self.infer_expr(&expr, None);
                }
            }
            ast::Stmt::YieldStmt(it) => {
                if let Some(expr) = it.expr() {
                    let ty = self.infer_expr(&expr, None);
                    if let Some(yields) = self.yield_tys.last_mut() {
                        yields.push(ty);
                    }
                }
            }
            ast::Stmt::AssertStmt(it) => {
                if let Some(cond) = it.condition() {
                    self.infer_condition(&cond);
                }
                if let Some(message) = it.message() {
                    self.infer_expr(&message, None);
                }
            }
            ast::Stmt::SynchronizedStmt(it) => {
                if let Some(lock) = it.lock() {
                    self.infer_expr(&lock, None);
                }
                if let Some(body) = it.body() {
                    self.infer_block(&body);
                }
            }
            ast::Stmt::TryStmt(it) => {
                self.push_scope();
                if let Some(resources) = it.resource_list() {
                    for resource in resources.resources() {
                        self.infer_resource(&resource);
                    }
                }
                if let Some(body) = it.body() {
                    self.infer_block(&body);
                }
                self.pop_scope();
                for catch in it.catch_clauses() {
                    self.push_scope();
                    if let Some(param) = catch.param() {
                        let ty = param.ty().map_or(Ty::Unknown, |t| self.lower_ty(&t));
                        if let Some(name) = param.name() {
                            self.declare(&name, ty, LocalKind::CatchParam);
                        }
                    }
                    if let Some(body) = catch.body() {
                        self.infer_block(&body);
                    }
                    self.pop_scope();
                }
                if let Some(body) = it.finally_clause().and_then(|f| f.body()) {
                    self.infer_block(&body);
                }
            }
            ast::Stmt::SwitchStmt(it) => {
                let scrutinee = it
                    .scrutinee()
                    .map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                if let Some(block) = it.block() {
                    self.infer_switch_block(&block, &scrutinee, false);
                }
            }
            ast::Stmt::LabeledStmt(it) => {
                if let Some(body) = it.body() {
                    self.infer_stmt(&body);
                }
            }
            ast::Stmt::BreakStmt(_) | ast::Stmt::ContinueStmt(_) | ast::Stmt::EmptyStmt(_) => {}
            // 局部类：成员就地推断，但类本身没有进入类型系统
            ast::Stmt::ClassDecl(_)
            | ast::Stmt::InterfaceDecl(_)
            | ast::Stmt::EnumDecl(_)
            | ast::Stmt::RecordDecl(_) => {
                if let Some(body) =
                    ast::TypeDecl::cast(stmt.syntax().clone()).and_then(|decl| decl.body())
                {
                    self.infer_class_body(&body, Ty::Unknown);
                }
            }
        }
    }

    fn infer_resource(&mut self, resource: &ast::Resource) {
        if let Some(declarator) = resource.declarator() {
            let ty_node = resource.ty();
            let is_var = ty_node.as_ref().is_some_and(|t| lower_type(t).is_var());
            let declared = match &ty_node {
                Some(t) if !is_var => self.lower_ty(t),
                _ => Ty::Unknown,
            };
            let mut ty = declared.clone();
            if let Some(init) = declarator.initializer() {
                let actual = self.infer_expr(&init, if is_var { None } else { Some(&declared) });
                if is_var {
                    ty = actual;
                }
            }
            if let Some(name) = declarator.name() {
                self.declare(&name, ty, LocalKind::Resource);
            }
        } else if let Some(expr) = resource.expr() {
            self.infer_expr(&expr, None);
        }
    }

    /// `is_expr` 为真时收集各分支的结果类型（压在 `yield_tys` 栈顶）。
    fn infer_switch_block(&mut self, block: &ast::SwitchBlock, scrutinee: &Ty, is_expr: bool) {
        // 旧式 `case X:` 的各组语句同属一个块：前面分支里声明的变量后面还能用
        let shared_scope = block.cases().any(|case| !case.is_arrow());
        if shared_scope {
            self.push_scope();
        }
        for case in block.cases() {
            if !shared_scope {
                self.push_scope();
            }
            if let Some(label) = case.label() {
                for expr in label.exprs() {
                    self.infer_case_constant(&expr, scrutinee);
                }
                for pattern in label.patterns() {
                    self.bind_pattern(&pattern, scrutinee);
                }
                if let Some(guard) = label.guard().and_then(|g| g.expr()) {
                    self.infer_expr(&guard, Some(&Ty::BOOLEAN));
                }
            }
            let is_arrow = case.is_arrow();
            for stmt in case.statements() {
                match &stmt {
                    // `case X -> expr;`：表达式的值就是该分支的结果
                    ast::Stmt::ExprStmt(it) if is_arrow && is_expr => {
                        if let Some(expr) = it.expr() {
                            let ty = self.infer_expr(&expr, None);
                            if let Some(yields) = self.yield_tys.last_mut() {
                                yields.push(ty);
                            }
                        }
                    }
                    _ => self.infer_stmt(&stmt),
                }
            }
            if !shared_scope {
                self.pop_scope();
            }
        }
        if shared_scope {
            self.pop_scope();
        }
    }

    /// `case RED:` 里的枚举常量按被匹配表达式的类型解析，不需要限定。
    fn infer_case_constant(&mut self, expr: &ast::Expr, scrutinee: &Ty) {
        if let (ast::Expr::NameExpr(name_expr), Ty::Class(class, _)) = (expr, scrutinee) {
            let is_enum =
                class_data(self.db, *class).is_some_and(|d| d.kind == hir_def::ClassKind::Enum);
            if let (true, Some(name_ref)) = (is_enum, name_expr.name_ref()) {
                if let Some(field) =
                    crate::members::lookup_field(self.db, scrutinee, &name_ref.text())
                {
                    let range = name_ref.syntax().text_range();
                    self.result
                        .resolutions
                        .insert(range, Resolution::Field(field.id));
                    self.result
                        .expr_types
                        .insert(expr.syntax().text_range(), field.ty);
                    return;
                }
            }
        }
        self.infer_expr(expr, Some(scrutinee));
    }

    /// 给模式里的绑定变量定型。`matched` 是被匹配值的类型。
    fn bind_pattern(&mut self, pattern: &ast::Pattern, matched: &Ty) {
        match pattern {
            ast::Pattern::TypePattern(it) => {
                let ty = match it.ty() {
                    Some(t) if !lower_type(&t).is_var() => self.lower_ty(&t),
                    _ => matched.clone(),
                };
                if let Some(name) = it.name() {
                    if name.text() != "_" {
                        self.declare(&name, ty, LocalKind::Pattern);
                    }
                }
            }
            ast::Pattern::RecordPattern(it) => {
                let record_ty = it.ty().map_or(Ty::Unknown, |t| self.lower_ty(&t));
                // 记录组件 = 该类按声明顺序的实例字段
                let components: Vec<Ty> = match &record_ty {
                    Ty::Class(class, args) => {
                        let subst = crate::ty::Subst::for_class(self.db, *class, args);
                        class_data(self.db, *class)
                            .map(|data| {
                                data.fields
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, f)| !f.flags.is_static())
                                    .map(|(idx, _)| {
                                        field_ty(
                                            self.db,
                                            FieldId {
                                                class: *class,
                                                idx: idx as u32,
                                            },
                                        )
                                        .subst(&subst)
                                    })
                                    .collect()
                            })
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                for (i, sub) in it.patterns().enumerate() {
                    let component = components.get(i).cloned().unwrap_or(Ty::Unknown);
                    self.bind_pattern(&sub, &component);
                }
            }
            ast::Pattern::UnnamedPattern(_) => {}
        }
    }

    /// 匿名类 / 局部类的类体。它自己声明的字段和方法登记为局部符号，
    /// 这样类体内部对它们的引用可以解析和跳转。
    fn infer_class_body(&mut self, body: &ast::ClassBody, this_ty: Ty) {
        self.frames.push(Frame { this_ty });
        self.frame_scope_base.push(self.scopes.len());
        self.push_scope();

        for member in body.members() {
            match &member {
                ast::Member::FieldDecl(field) => {
                    let base = field.ty().map_or(Ty::Unknown, |t| self.lower_ty(&t));
                    for declarator in field.declarators() {
                        let mut ty = base.clone();
                        for _ in 0..declarator.extra_dims() {
                            ty = Ty::array(ty);
                        }
                        if let Some(name) = declarator.name() {
                            self.declare(&name, ty, LocalKind::LocalClassField);
                        }
                    }
                }
                ast::Member::MethodDecl(method) => {
                    let ret = method
                        .return_type()
                        .map_or(Ty::Unknown, |t| self.lower_ty(&t));
                    if let Some(name) = method.name() {
                        self.declare(&name, ret, LocalKind::LocalClassMethod);
                    }
                }
                _ => {}
            }
        }

        for constant in body.enum_constants() {
            if let Some(args) = constant.arg_list() {
                for arg in args.args() {
                    self.infer_expr(&arg, None);
                }
            }
        }

        for member in body.members() {
            match &member {
                ast::Member::FieldDecl(field) => {
                    let base = field.ty().map_or(Ty::Unknown, |t| self.lower_ty(&t));
                    for declarator in field.declarators() {
                        if let Some(init) = declarator.initializer() {
                            self.infer_expr(&init, Some(&base));
                        }
                    }
                }
                ast::Member::MethodDecl(method) => {
                    let ret = method
                        .return_type()
                        .map_or(Ty::Unknown, |t| self.lower_ty(&t));
                    self.infer_local_function(method.param_list(), method.body(), Some(ret));
                }
                ast::Member::ConstructorDecl(ctor) => {
                    self.infer_local_function(ctor.param_list(), ctor.body(), Some(Ty::Void));
                }
                ast::Member::InitializerBlock(init) => {
                    if let Some(block) = init.body() {
                        self.infer_block(&block);
                    }
                }
                ast::Member::ClassDecl(_)
                | ast::Member::InterfaceDecl(_)
                | ast::Member::EnumDecl(_)
                | ast::Member::RecordDecl(_)
                | ast::Member::AnnotationTypeDecl(_) => {
                    if let Some(nested) =
                        ast::TypeDecl::cast(member.syntax().clone()).and_then(|d| d.body())
                    {
                        self.infer_class_body(&nested, Ty::Unknown);
                    }
                }
            }
        }

        self.pop_scope();
        self.frame_scope_base.pop();
        self.frames.pop();
    }

    /// 匿名类 / 局部类里的方法或构造器。
    fn infer_local_function(
        &mut self,
        params: Option<ast::ParamList>,
        body: Option<ast::Block>,
        ret: Option<Ty>,
    ) {
        self.push_scope();
        if let Some(params) = params {
            for param in params.params() {
                let ty = self.lower_ref(&hir_def::lower_param_type(&param));
                if let Some(name) = param.name() {
                    self.declare(&name, ty, LocalKind::Param);
                }
            }
        }
        // 嵌套函数体里的 return 不属于外层 lambda
        let outer_returns = std::mem::take(&mut self.return_tys);
        self.ret_expected.push(ret);
        if let Some(body) = body {
            self.infer_block(&body);
        }
        self.ret_expected.pop();
        self.return_tys = outer_returns;
        self.pop_scope();
    }
}
