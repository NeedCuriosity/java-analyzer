//! 表达式的推断。

use hir_def::{
    all_supers, class_data, class_source_file, item_tree, lookup_class, lower_type, member_type,
    package_exists, resolve_qualified_class, resolve_simple_type, ClassId, GenericOwner, MethodId,
    Modifiers, PrimitiveType, TypeNs,
};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, TextRange,
};

use super::{CallInfo, InferCtx, InferDiagnosticKind, LocalKind, Resolution};
use crate::{
    lower::{class_supertypes, method_sig, this_ty},
    members::{constructors, functional_method, lookup_field, lookup_methods, MethodCandidate},
    method_resolution::{resolve_call, Arg, Call},
    subtype::{binary_numeric_promotion, is_subtype},
    ty::{Subst, Ty},
};

/// 表达式位置上的一个名字可能是值，也可能只是类型名或包名（作为限定符）。
pub(super) enum Value {
    Ty(Ty),
    Type(ClassId),
    /// 斜杠形式的包名
    Package(String),
}

/// 可以带着目标类型安全地重新推断一遍的实参：方法调用或对象创建，且里面不声明任何局部变量。
fn is_retargetable(arg: &ast::Expr) -> bool {
    matches!(arg, ast::Expr::MethodCall(_) | ast::Expr::NewExpr(_))
        && !arg.syntax().descendants().any(|n| {
            matches!(
                n.kind(),
                SyntaxKind::LAMBDA_EXPR
                    | SyntaxKind::CLASS_BODY
                    | SyntaxKind::SWITCH_EXPR
                    | SyntaxKind::TYPE_PATTERN
                    | SyntaxKind::RECORD_PATTERN
            )
        })
}

fn method_flags(cx: &InferCtx<'_>, method: MethodId) -> Modifiers {
    class_data(cx.db, method.class)
        .and_then(|data| data.methods.get(method.idx as usize).map(|m| m.flags))
        .unwrap_or_default()
}

impl InferCtx<'_> {
    pub(super) fn infer_expr(&mut self, expr: &ast::Expr, expected: Option<&Ty>) -> Ty {
        match self.infer_value(expr, expected) {
            Value::Ty(ty) => ty,
            Value::Type(_) | Value::Package(_) => Ty::Unknown,
        }
    }

    fn infer_value(&mut self, expr: &ast::Expr, expected: Option<&Ty>) -> Value {
        let value = self.infer_value_inner(expr, expected);
        if let Value::Ty(ty) = &value {
            self.result
                .expr_types
                .insert(expr.syntax().text_range(), ty.clone());
        }
        value
    }

    fn infer_value_inner(&mut self, expr: &ast::Expr, expected: Option<&Ty>) -> Value {
        let db = self.db;
        let ty = match expr {
            ast::Expr::Literal(it) => match it.kind() {
                Some(SyntaxKind::INT_LITERAL) => Ty::INT,
                Some(SyntaxKind::LONG_LITERAL) => Ty::Primitive(PrimitiveType::Long),
                Some(SyntaxKind::FLOAT_LITERAL) => Ty::Primitive(PrimitiveType::Float),
                Some(SyntaxKind::DOUBLE_LITERAL) => Ty::Primitive(PrimitiveType::Double),
                Some(SyntaxKind::CHAR_LITERAL) => Ty::Primitive(PrimitiveType::Char),
                Some(SyntaxKind::STRING_LITERAL | SyntaxKind::TEXT_BLOCK) => Ty::string(db),
                Some(SyntaxKind::TRUE_KW | SyntaxKind::FALSE_KW) => Ty::BOOLEAN,
                Some(SyntaxKind::NULL_KW) => Ty::Null,
                _ => Ty::Unknown,
            },
            ast::Expr::NameExpr(it) => {
                return match it.name_ref() {
                    Some(name_ref) => self.resolve_simple_name(&name_ref),
                    None => Value::Ty(Ty::Unknown),
                };
            }
            ast::Expr::ThisExpr(it) => match it.qualifier() {
                Some(qualifier) => match self.infer_value(&qualifier, None) {
                    Value::Type(class) => this_ty(db, class),
                    _ => Ty::Unknown,
                },
                None => self.this_ty(),
            },
            ast::Expr::SuperExpr(it) => match it.qualifier() {
                // `Iface.super.method()`
                Some(qualifier) => match self.infer_value(&qualifier, None) {
                    Value::Type(class) => this_ty(db, class),
                    _ => Ty::Unknown,
                },
                None => self.super_ty(),
            },
            ast::Expr::ParenExpr(it) => it
                .expr()
                .map_or(Ty::Unknown, |e| self.infer_expr(&e, expected)),
            ast::Expr::FieldAccess(it) => {
                let Some(receiver) = it.receiver() else {
                    return Value::Ty(Ty::Unknown);
                };
                let receiver = self.infer_value(&receiver, None);
                let Some(name_ref) = it.name_ref() else {
                    return Value::Ty(Ty::Unknown);
                };
                return self.resolve_member(receiver, &name_ref);
            }
            ast::Expr::MethodCall(it) => self.infer_method_call(it, expected),
            ast::Expr::ExplicitConstructorCall(it) => {
                let key = it.target().map(|t| t.syntax().text_range());
                let ty = if it.is_super() {
                    self.super_ty()
                } else {
                    self.this_ty()
                };
                if let Some(args) = it.arg_list() {
                    match &ty {
                        Ty::Class(class, _) => {
                            self.infer_constructor_call(*class, &ty, false, &args, None, key);
                        }
                        _ => self.infer_args_untargeted(&args),
                    }
                }
                Ty::Void
            }
            ast::Expr::NewExpr(it) => {
                let qualifier_ty = it
                    .qualifier()
                    .map(|qualifier| self.infer_expr(&qualifier, None));
                let Some(ty_node) = it.ty() else {
                    if let Some(args) = it.arg_list() {
                        self.infer_args_untargeted(&args);
                    }
                    return Value::Ty(Ty::Unknown);
                };
                // `outer.new Inner()`：`Inner` 是 `outer` 的类型的成员类，不按词法作用域解析
                let inner_of_qualifier = match (&qualifier_ty, &ty_node) {
                    (Some(Ty::Class(outer, _)), ast::Type::ClassType(class_ty)) => class_ty
                        .name_ref()
                        .and_then(|name| member_type(db, *outer, &name.text()))
                        .map(Ty::class),
                    (Some(_), _) => Some(Ty::Unknown),
                    _ => None,
                };
                let ty = inner_of_qualifier.unwrap_or_else(|| self.lower_ty(&ty_node));
                let (diamond, key) = match &ty_node {
                    ast::Type::ClassType(class_ty) => (
                        class_ty
                            .type_arg_list()
                            .is_some_and(|list| list.args().next().is_none()),
                        class_ty.name_ref().map(|n| n.syntax().text_range()),
                    ),
                    _ => (false, None),
                };
                let result = match (&ty, it.arg_list()) {
                    (Ty::Class(class, _), Some(args)) => {
                        self.infer_constructor_call(*class, &ty, diamond, &args, expected, key)
                    }
                    (_, Some(args)) => {
                        self.infer_args_untargeted(&args);
                        ty.clone()
                    }
                    _ => ty.clone(),
                };
                if let Some(body) = it.body() {
                    self.infer_class_body(&body, result.clone());
                }
                result
            }
            ast::Expr::ArrayCreationExpr(it) => {
                let mut ty = it.element_type().map_or(Ty::Unknown, |t| self.lower_ty(&t));
                for dim in it.dims() {
                    if let Some(size) = dim.expr() {
                        self.infer_expr(&size, Some(&Ty::INT));
                    }
                    ty = Ty::array(ty);
                }
                if let Some(init) = it.initializer() {
                    self.infer_array_initializer(&init, &ty);
                }
                ty
            }
            ast::Expr::ArrayInitializer(it) => {
                let ty = expected.cloned().unwrap_or(Ty::Unknown);
                self.infer_array_initializer(it, &ty);
                ty
            }
            ast::Expr::ArrayAccess(it) => {
                let array = it
                    .array()
                    .map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                if let Some(index) = it.index() {
                    self.infer_expr(&index, Some(&Ty::INT));
                }
                match array {
                    Ty::Array(element) => (*element).clone(),
                    _ => Ty::Unknown,
                }
            }
            ast::Expr::UnaryExpr(it) => {
                let operand = it
                    .operand()
                    .map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                let op = it.op_token();
                if let Some(op) = &op {
                    let ok = match op.kind() {
                        SyntaxKind::BANG => {
                            operand.unboxed(self.db) == Some(PrimitiveType::Boolean)
                        }
                        _ => operand.unboxed(self.db).is_some_and(|p| p.is_numeric()),
                    };
                    if !ok && self.is_simple_ty(&operand) {
                        self.diagnostic(
                            it.syntax().text_range(),
                            InferDiagnosticKind::BadOperand {
                                op: op.text().to_string(),
                                operands: operand.display(self.db),
                            },
                        );
                    }
                }
                match op.map(|t| t.kind()) {
                    Some(SyntaxKind::BANG) => Ty::BOOLEAN,
                    Some(SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS) => operand,
                    _ => self.unary_promote(&operand),
                }
            }
            ast::Expr::PostfixExpr(it) => it
                .operand()
                .map_or(Ty::Unknown, |e| self.infer_expr(&e, None)),
            ast::Expr::BinaryExpr(it) => {
                let lhs = it.lhs().map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                let rhs = it.rhs().map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                self.check_operands(it, &lhs, &rhs);
                self.binary_result(it.op_kind(), &lhs, &rhs)
            }
            ast::Expr::AssignExpr(it) => {
                let lhs = it.lhs().map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                if let Some(rhs) = it.rhs() {
                    self.infer_expr(&rhs, Some(&lhs));
                }
                lhs
            }
            ast::Expr::ConditionalExpr(it) => {
                if let Some(cond) = it.condition() {
                    self.infer_condition(&cond);
                }
                let then = it
                    .then_expr()
                    .map_or(Ty::Unknown, |e| self.infer_expr(&e, expected));
                let otherwise = it
                    .else_expr()
                    .map_or(Ty::Unknown, |e| self.infer_expr(&e, expected));
                self.merge_branches(&then, &otherwise)
            }
            ast::Expr::InstanceofExpr(it) => {
                let matched = it.expr().map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                if let Some(pattern) = it.pattern() {
                    self.bind_pattern(&pattern, &matched);
                }
                Ty::BOOLEAN
            }
            ast::Expr::CastExpr(it) => {
                let ty = it.ty().map_or(Ty::Unknown, |t| self.lower_ty(&t));
                if let Some(operand) = it.expr() {
                    self.infer_expr(&operand, Some(&ty));
                }
                ty
            }
            ast::Expr::LambdaExpr(it) => {
                self.infer_lambda(it, expected);
                expected.cloned().unwrap_or(Ty::Unknown)
            }
            ast::Expr::MethodRef(it) => {
                self.infer_method_ref(it, expected);
                expected.cloned().unwrap_or(Ty::Unknown)
            }
            ast::Expr::ClassLiteral(it) => {
                let inner = if let Some(t) = it.ty() {
                    self.lower_ty(&t)
                } else if let Some(e) = it.expr() {
                    match self.infer_value(&e, None) {
                        Value::Type(class) => Ty::class(class),
                        _ => Ty::Unknown,
                    }
                } else {
                    Ty::Unknown
                };
                match lookup_class(db, "java/lang/Class") {
                    Some(class) => Ty::class_with_args(class, vec![inner.boxed(db)]),
                    None => Ty::Unknown,
                }
            }
            ast::Expr::SwitchExpr(it) => {
                let scrutinee = it
                    .scrutinee()
                    .map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                self.yield_tys.push(Vec::new());
                if let Some(block) = it.block() {
                    self.infer_switch_block(&block, &scrutinee, true);
                }
                let results = self.yield_tys.pop().unwrap_or_default();
                // 各分支结果按条件表达式的规则合并（`1` 和 `2L` 得到 long）
                results
                    .into_iter()
                    .filter(|ty| !matches!(ty, Ty::Unknown | Ty::Null))
                    .reduce(|a, b| self.merge_branches(&a, &b))
                    .or_else(|| expected.cloned())
                    .unwrap_or(Ty::Unknown)
            }
        };
        Value::Ty(ty)
    }

    // ---- 名字 ----

    fn outer_class(&self, class: ClassId) -> Option<ClassId> {
        let data = class_data(self.db, class)?;
        lookup_class(self.db, data.outer.as_deref()?)
    }

    /// 可能通过 static import 提供名为 `name` 的静态成员的类。
    /// 单个成员的 static import 遮蔽按需的（JLS §6.4.1），所以排在前面。
    fn static_import_classes(&self, name: &str) -> Vec<ClassId> {
        let mut imports: Vec<_> = self
            .tree
            .imports
            .iter()
            .filter(|import| import.is_static)
            .collect();
        imports.sort_by_key(|import| import.is_on_demand);
        imports
            .into_iter()
            .filter_map(|import| {
                if import.is_on_demand {
                    resolve_qualified_class(self.db, &import.segments)
                } else {
                    let (last, prefix) = import.segments.split_last()?;
                    (last == name)
                        .then(|| resolve_qualified_class(self.db, prefix))
                        .flatten()
                }
            })
            .collect()
    }

    fn resolve_simple_name(&mut self, name_ref: &ast::NameRef) -> Value {
        let db = self.db;
        let name = name_ref.text();
        let range = name_ref.syntax().text_range();

        if let Some(local) = self.lookup_local(&name) {
            self.result
                .resolutions
                .insert(range, Resolution::Local(local));
            return Value::Ty(self.result.locals[local as usize].ty.clone());
        }

        // 各层"当前类"的字段，然后是外层具名类的字段
        let mut receivers: Vec<Ty> = self
            .frames
            .iter()
            .rev()
            .map(|f| f.this_ty.clone())
            .collect();
        let mut outer = self.outer_class(self.class);
        while let Some(class) = outer {
            receivers.push(this_ty(db, class));
            outer = self.outer_class(class);
        }
        receivers.extend(self.static_import_classes(&name).into_iter().map(Ty::class));
        for receiver in &receivers {
            // 别的顶层类的私有字段不被继承，简单名看不到它
            let visible = lookup_field(db, receiver, &name).filter(|field| {
                class_data(db, field.id.class)
                    .and_then(|data| data.fields.get(field.id.idx as usize).map(|f| f.flags))
                    .is_none_or(|flags| {
                        crate::members::is_accessible(db, flags, field.id.class, Some(self.class))
                            || flags.visibility() != hir_def::Visibility::Private
                    })
            });
            if let Some(field) = visible {
                self.result
                    .resolutions
                    .insert(range, Resolution::Field(field.id));
                return Value::Ty(field.ty);
            }
        }

        if let Some(TypeNs::Class(class)) = resolve_simple_type(db, &self.scope, &name) {
            self.result
                .resolutions
                .insert(range, Resolution::Class(class));
            return Value::Type(class);
        }
        if package_exists(db, &name) {
            self.result
                .resolutions
                .insert(range, Resolution::Package(name.clone()));
            return Value::Package(name.to_string());
        }

        // 匿名类 / 局部类从父类型继承来的成员类型
        let inherited = self
            .frames
            .iter()
            .skip(1)
            .rev()
            .find_map(|frame| match &frame.this_ty {
                Ty::Class(class, _) => member_type(db, *class, &name),
                _ => None,
            });
        if let Some(class) = inherited {
            self.result
                .resolutions
                .insert(range, Resolution::Class(class));
            return Value::Type(class);
        }
        // 方法体里声明的局部类 / 记录 / 接口：没有 `ClassId`，只知道名字存在
        if self.local_type_names.contains(&name) {
            return Value::Ty(Ty::Unknown);
        }
        if self.can_report_unqualified() {
            self.diagnostic(range, InferDiagnosticKind::UnresolvedName { name });
        }
        Value::Ty(Ty::Unknown)
    }

    fn resolve_member(&mut self, receiver: Value, name_ref: &ast::NameRef) -> Value {
        let db = self.db;
        let name = name_ref.text();
        let range = name_ref.syntax().text_range();
        match receiver {
            Value::Ty(ty) => {
                if name == "length" && matches!(ty, Ty::Array(_)) {
                    self.result
                        .resolutions
                        .insert(range, Resolution::ArrayLength);
                    return Value::Ty(Ty::INT);
                }
                if let Some(field) = lookup_field(db, &ty, &name) {
                    self.result
                        .resolutions
                        .insert(range, Resolution::Field(field.id));
                    return Value::Ty(field.ty);
                }
                // 匿名类 / 局部类里的 `this.x`：`this` 的类型记成了父类型，
                // 类体自己声明的字段登记在局部符号里
                if self.in_local_class_with_this(&ty) {
                    if let Some(local) = self.lookup_local(&name) {
                        if self.result.locals[local as usize].kind == LocalKind::LocalClassField {
                            self.result
                                .resolutions
                                .insert(range, Resolution::Local(local));
                            return Value::Ty(self.result.locals[local as usize].ty.clone());
                        }
                    }
                    return Value::Ty(Ty::Unknown);
                }
                if self.can_report_missing_member(&ty) {
                    let receiver = ty.display(db);
                    self.diagnostic(
                        range,
                        InferDiagnosticKind::UnresolvedField { name, receiver },
                    );
                }
                Value::Ty(Ty::Unknown)
            }
            Value::Type(class) => {
                let ty = Ty::class(class);
                // 字段和成员类型同名时字段优先；但字段在这里不可访问的话，
                // 名字指的是同名的成员类型（javac 的行为）
                let field = lookup_field(db, &ty, &name).filter(|field| {
                    member_type(db, class, &name).is_none()
                        || class_data(db, field.id.class)
                            .and_then(|data| {
                                data.fields.get(field.id.idx as usize).map(|f| f.flags)
                            })
                            .is_none_or(|flags| {
                                crate::members::is_accessible(
                                    db,
                                    flags,
                                    field.id.class,
                                    Some(self.class),
                                )
                            })
                });
                if let Some(field) = field {
                    self.result
                        .resolutions
                        .insert(range, Resolution::Field(field.id));
                    return Value::Ty(field.ty);
                }
                if let Some(nested) = member_type(db, class, &name) {
                    self.result
                        .resolutions
                        .insert(range, Resolution::Class(nested));
                    return Value::Type(nested);
                }
                if self.can_report_missing_member(&ty) {
                    let receiver = ty.display(db);
                    self.diagnostic(
                        range,
                        InferDiagnosticKind::UnresolvedField { name, receiver },
                    );
                }
                Value::Ty(Ty::Unknown)
            }
            Value::Package(package) => {
                let qualified = format!("{package}/{name}");
                let class = lookup_class(db, &qualified)
                    .filter(|class| hir_def::is_visible_from(db, self.scope.file, *class));
                if let Some(class) = class {
                    self.result
                        .resolutions
                        .insert(range, Resolution::Class(class));
                    Value::Type(class)
                } else if package_exists(db, &qualified) {
                    self.result
                        .resolutions
                        .insert(range, Resolution::Package(qualified.as_str().into()));
                    Value::Package(qualified)
                } else {
                    Value::Ty(Ty::Unknown)
                }
            }
        }
    }

    // ---- 诊断的前提条件 ----

    /// 类及其所有祖先是否都解析成功，并且都不依赖会凭空生成成员的工具。
    /// 只有这样，"找不到成员"才是确定的。
    fn hierarchy_is_reliable(&self, class: ClassId) -> bool {
        let db = self.db;
        std::iter::once(class)
            .chain(all_supers(db, class))
            .all(|c| {
                let Some(data) = class_data(db, c) else {
                    return false;
                };
                let declared = data.superclass.iter().count() + data.interfaces.len();
                if class_supertypes(db, c).len() != declared {
                    return false;
                }
                if data.annotations.iter().any(|a| a.contains("lombok")) {
                    return false;
                }
                match class_source_file(db, c) {
                    Some(file) => !item_tree(db, file)
                        .imports
                        .iter()
                        .any(|i| i.segments.first().is_some_and(|s| s == "lombok")),
                    None => true,
                }
            })
    }

    /// 当前在匿名类 / 局部类的类体里，并且 `ty` 就是这一层 `this` 的类型。
    /// 这种类自己声明的成员不在父类型上，按类型查不到是正常的。
    fn in_local_class_with_this(&self, ty: &Ty) -> bool {
        self.frames.len() > 1 && self.frames.last().is_some_and(|frame| frame.this_ty == *ty)
    }

    fn can_report_missing_member(&self, receiver: &Ty) -> bool {
        if self.suppress_unresolved || receiver.contains_unknown() {
            return false;
        }
        match receiver {
            Ty::Class(class, _) => self.hierarchy_is_reliable(*class),
            _ => false,
        }
    }

    /// 不带限定符的名字找不到时能否报错：所有可能提供它的地方都必须是确定的。
    fn can_report_unqualified(&self) -> bool {
        if self.suppress_unresolved {
            return false;
        }
        let frames_ok = self.frames.iter().all(|frame| match &frame.this_ty {
            Ty::Class(class, _) => self.hierarchy_is_reliable(*class),
            _ => false,
        });
        if !frames_ok {
            return false;
        }
        let mut outer = self.outer_class(self.class);
        while let Some(class) = outer {
            if !self.hierarchy_is_reliable(class) {
                return false;
            }
            outer = self.outer_class(class);
        }
        // 有解析不了的 static import 时，名字可能就来自它
        self.tree
            .imports
            .iter()
            .filter(|i| i.is_static)
            .all(|import| {
                let class_path = if import.is_on_demand {
                    &import.segments[..]
                } else {
                    &import.segments[..import.segments.len().saturating_sub(1)]
                };
                resolve_qualified_class(self.db, class_path).is_some()
            })
    }

    // ---- 调用 ----

    fn infer_args_untargeted(&mut self, args: &ast::ArgList) {
        for arg in args.args() {
            self.infer_expr(&arg, None);
        }
    }

    /// 去掉从这里访问不到的私有方法（别的顶层类的私有方法不被继承，也不参与重载决议）。
    fn without_inaccessible_private(&self, found: Vec<MethodCandidate>) -> Vec<MethodCandidate> {
        found
            .into_iter()
            .filter(|candidate| {
                let flags = method_flags(self, candidate.id);
                flags.visibility() != hir_def::Visibility::Private
                    || crate::members::is_accessible(
                        self.db,
                        flags,
                        candidate.id.class,
                        Some(self.class),
                    )
            })
            .collect()
    }

    fn unqualified_methods(&self, name: &str) -> Vec<MethodCandidate> {
        let db = self.db;
        for frame in self.frames.iter().rev() {
            let found = self.without_inaccessible_private(lookup_methods(db, &frame.this_ty, name));
            if !found.is_empty() {
                return found;
            }
        }
        let mut outer = self.outer_class(self.class);
        while let Some(class) = outer {
            let found =
                self.without_inaccessible_private(lookup_methods(db, &this_ty(db, class), name));
            if !found.is_empty() {
                return found;
            }
            outer = self.outer_class(class);
        }
        for class in self.static_import_classes(name) {
            let found: Vec<MethodCandidate> = lookup_methods(db, &Ty::class(class), name)
                .into_iter()
                .filter(|c| method_flags(self, c.id).is_static())
                .collect();
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }

    fn infer_method_call(&mut self, call: &ast::MethodCall, expected: Option<&Ty>) -> Ty {
        let db = self.db;
        let receiver = call.receiver().map(|r| self.infer_value(&r, None));
        let Some(name_ref) = call.name_ref() else {
            if let Some(args) = call.arg_list() {
                self.infer_args_untargeted(&args);
            }
            return Ty::Unknown;
        };
        let name = name_ref.text();
        let name_range = name_ref.syntax().text_range();
        let explicit: Vec<Ty> = call
            .type_arg_list()
            .map(|list| list.args().map(|t| self.lower_ty(&t)).collect())
            .unwrap_or_default();

        // (候选, 用于诊断的接收者类型)
        let (candidates, receiver_ty) = match receiver {
            Some(Value::Ty(ty)) => (lookup_methods(db, &ty, &name), Some(ty)),
            Some(Value::Type(class)) => {
                let ty = Ty::class(class);
                (lookup_methods(db, &ty, &name), Some(ty))
            }
            Some(Value::Package(_)) => {
                if let Some(args) = call.arg_list() {
                    self.infer_args_untargeted(&args);
                }
                return Ty::Unknown;
            }
            None => {
                // 匿名类 / 局部类自己声明的方法
                if let Some(local) = self.lookup_local_method(&name) {
                    let data = &self.result.locals[local as usize];
                    if data.kind == LocalKind::LocalClassMethod {
                        let ty = data.ty.clone();
                        self.result
                            .resolutions
                            .insert(name_range, Resolution::Local(local));
                        if let Some(args) = call.arg_list() {
                            self.infer_args_untargeted(&args);
                        }
                        return ty;
                    }
                }
                (self.unqualified_methods(&name), None)
            }
        };

        // 带限定符的调用：有访问得到的重载时，访问不到的私有重载不参与决议
        let candidates = if receiver_ty.is_some() {
            let accessible = self.without_inaccessible_private(candidates.clone());
            if accessible.is_empty() {
                candidates
            } else {
                accessible
            }
        } else {
            candidates
        };

        let Some(args) = call.arg_list() else {
            return Ty::Unknown;
        };
        // 数组的 `clone()` 返回数组本身的类型，也不抛受检异常（JLS §10.7）
        if name == "clone" && args.args().next().is_none() {
            if let Some(array @ Ty::Array(_)) = &receiver_ty {
                return array.clone();
            }
        }
        if candidates.is_empty() {
            self.infer_args_untargeted(&args);
            let can_report = match &receiver_ty {
                Some(ty) if self.in_local_class_with_this(ty) => false,
                Some(ty) => self.can_report_missing_member(ty),
                None => self.can_report_unqualified(),
            };
            if can_report {
                let receiver = receiver_ty.map(|ty| ty.display(db));
                self.diagnostic(
                    name_range,
                    InferDiagnosticKind::UnresolvedMethod { name, receiver },
                );
            }
            return Ty::Unknown;
        }

        let candidates_complete = match &receiver_ty {
            Some(ty) if self.in_local_class_with_this(ty) => false,
            Some(ty) => self.can_report_missing_member(ty),
            None => self.can_report_unqualified(),
        };
        self.report_inapplicable = candidates_complete.then(|| name.clone());
        match self.finish_call(candidates, &args, &explicit, &[], Some(name_range)) {
            Some(mut resolved) => resolved.return_ty(db, expected),
            None => Ty::Unknown,
        }
    }

    /// 构造器调用（`new`、`this(...)`、`super(...)`、枚举常量）。返回被构造对象的类型。
    pub(super) fn infer_constructor_call(
        &mut self,
        class: ClassId,
        class_ty: &Ty,
        diamond: bool,
        args: &ast::ArgList,
        expected: Option<&Ty>,
        key: Option<TextRange>,
    ) -> Ty {
        let db = self.db;
        // 菱形推断不出来时：期望类型正好是同一个类就用它，否则退回原始类型
        let diamond_fallback = || match expected {
            Some(expected) if expected.as_class() == Some(class) => expected.clone(),
            _ => Ty::class(class),
        };

        let is_interface = class_data(db, class).is_some_and(|d| d.is_interface());
        let subst = if diamond {
            Subst::empty()
        } else {
            Subst::for_receiver(db, class_ty)
        };
        let candidates = if is_interface {
            Vec::new()
        } else {
            constructors(db, class, &subst)
        };
        if candidates.is_empty() {
            self.infer_args_untargeted(args);
            return if diamond {
                diamond_fallback()
            } else {
                class_ty.clone()
            };
        }

        // 菱形构造时，被构造类的类型参数也是待推断的
        let class_owner = [GenericOwner::Class(class)];
        let owners: &[GenericOwner] = if diamond { &class_owner } else { &[] };
        // class 文件里内部类的构造器带着合成的外层实例形参，只对源码里的类下结论
        let candidates_complete = !self.suppress_unresolved
            && class_source_file(db, class).is_some()
            && self.hierarchy_is_reliable(class);
        self.report_inapplicable = candidates_complete
            .then(|| class_data(db, class).map(|d| d.name.clone()))
            .flatten();
        match self.finish_call(candidates, args, &[], owners, key) {
            Some(mut resolved) if diamond => resolved.finish_ty(db, &this_ty(db, class), expected),
            Some(_) => class_ty.clone(),
            None if diamond => diamond_fallback(),
            None => class_ty.clone(),
        }
    }

    /// 重载决议 + 实参推断。lambda 和方法引用延后到选定方法之后，以形参类型为目标类型来推断。
    fn finish_call(
        &mut self,
        candidates: Vec<MethodCandidate>,
        arg_list: &ast::ArgList,
        explicit_type_args: &[Ty],
        extra_owners: &[GenericOwner],
        key: Option<TextRange>,
    ) -> Option<Call> {
        let db = self.db;
        let arg_exprs: Vec<ast::Expr> = arg_list.args().collect();
        let args: Vec<Arg> = arg_exprs
            .iter()
            .map(|arg| match arg {
                ast::Expr::LambdaExpr(lambda) => Arg::Lambda {
                    arity: lambda.param_list().map_or(0, |p| p.params().count()),
                },
                ast::Expr::MethodRef(_) => Arg::MethodRef,
                other => Arg::Ty(self.infer_expr(other, None)),
            })
            .collect();

        let resolved = resolve_call(db, &candidates, &args, explicit_type_args, extra_owners);
        if let Some(name) = self.report_inapplicable.take() {
            if resolved.as_ref().is_some_and(|call| !call.applicable)
                && self.inapplicable_is_certain(&candidates, &args)
            {
                let shown: Vec<String> = args
                    .iter()
                    .map(|arg| match arg {
                        Arg::Ty(ty) => ty.display(db),
                        Arg::Lambda { .. } => "lambda".to_owned(),
                        Arg::MethodRef => base_db::tr!("方法引用", "method reference"),
                    })
                    .collect();
                self.diagnostic(
                    key.unwrap_or_else(|| arg_list.syntax().text_range()),
                    InferDiagnosticKind::ArgumentMismatch {
                        name,
                        args: shown.join(", "),
                    },
                );
            }
        }
        self.result.calls.insert(
            arg_list.syntax().text_range(),
            CallInfo {
                chosen: resolved.as_ref().map(|c| c.method),
                candidates,
            },
        );

        let Some(mut call) = resolved else {
            for arg in &arg_exprs {
                if matches!(arg, ast::Expr::LambdaExpr(_) | ast::Expr::MethodRef(_)) {
                    self.infer_expr(arg, None);
                }
            }
            return None;
        };
        if let Some(key) = key {
            self.result
                .resolutions
                .insert(key, Resolution::Method(call.method));
        }

        // 实参本身是泛型调用、且它的类型参数只能由目标类型确定时（`collect(Collectors.toList())`、
        // `add(new ArrayList<>())`），带着形参类型再推一遍，然后用更精确的结果重新约束本次调用。
        for (idx, arg) in arg_exprs.iter().enumerate() {
            let Arg::Ty(first) = &args[idx] else {
                continue;
            };
            if !first.contains_unknown() || !is_retargetable(arg) {
                continue;
            }
            let target = call.finish_ty(db, &call.param_ty(idx), None);
            if target.is_unknown() {
                continue;
            }
            let n_diagnostics = self.result.diagnostics.len();
            let refined = self.infer_expr(arg, Some(&target));
            self.result.diagnostics.truncate(n_diagnostics);
            if !refined.is_unknown() {
                call.refine_arg(db, idx, &refined);
            }
        }

        for (idx, arg) in arg_exprs.iter().enumerate() {
            let produced = match arg {
                ast::Expr::LambdaExpr(lambda) => {
                    let target = call.param_ty(idx);
                    Some((self.infer_lambda(lambda, Some(&target)), target))
                }
                ast::Expr::MethodRef(method_ref) => {
                    let target = call.param_ty(idx);
                    Some((self.infer_method_ref(method_ref, Some(&target)), target))
                }
                _ => None,
            };
            if let Some((produced_ty, target)) = produced {
                // 函数体的结果类型反过来约束方法的类型参数（`map(x -> x.length())` 推出 R）
                if !matches!(produced_ty, Ty::Void | Ty::Unknown) {
                    if let Some((_, functional_ret)) = self.functional_sig(&target) {
                        call.inference.unify(db, &functional_ret, &produced_ty);
                    }
                }
                let arg_ty = call.finish_ty(db, &target, None);
                self.result
                    .expr_types
                    .insert(arg.syntax().text_range(), arg_ty);
            }
        }
        Some(call)
    }

    /// 重载决议失败是不是确定的：要么所有候选的参数个数都对不上，
    /// 要么实参和所有候选的形参都是简单类型（不涉及泛型推断、lambda）。
    fn inapplicable_is_certain(&self, candidates: &[MethodCandidate], args: &[Arg]) -> bool {
        let db = self.db;
        let sigs: Vec<_> = candidates
            .iter()
            .map(|candidate| (candidate, method_sig(db, candidate.id)))
            .collect();
        let arity_never_matches = sigs
            .iter()
            .all(|(_, sig)| !sig.varargs && sig.params.len() != args.len());
        if arity_never_matches {
            return true;
        }
        let args_simple = args.iter().all(|arg| match arg {
            Arg::Ty(ty) => self.is_simple_ty(ty),
            _ => false,
        });
        args_simple
            && sigs.iter().all(|(candidate, sig)| {
                sig.n_type_params == 0
                    && sig.params.iter().all(|param| {
                        let param = param.subst(&candidate.subst);
                        match &param {
                            Ty::Array(element) if sig.varargs => self.is_simple_ty(element),
                            other => self.is_simple_ty(other),
                        }
                    })
            })
    }

    /// 二元运算符的操作数类型检查（只看两边都是简单类型的情形）。
    fn check_operands(&mut self, expr: &ast::BinaryExpr, lhs: &Ty, rhs: &Ty) {
        use SyntaxKind::*;
        let Some(op) = expr.op_token() else { return };
        if !self.is_simple_ty(lhs) || !self.is_simple_ty(rhs) {
            return;
        }
        let db = self.db;
        let numeric = |ty: &Ty| ty.unboxed(db).is_some_and(|p| p.is_numeric());
        let boolean = |ty: &Ty| ty.unboxed(db) == Some(PrimitiveType::Boolean);
        let ok = match op.kind() {
            PLUS => self.is_string(lhs) || self.is_string(rhs) || (numeric(lhs) && numeric(rhs)),
            MINUS | STAR | SLASH | PERCENT | LT | GT | LT_EQ | GT_EQ | SHL | SHR | USHR => {
                numeric(lhs) && numeric(rhs)
            }
            AMP_AMP | PIPE_PIPE => boolean(lhs) && boolean(rhs),
            AMP | PIPE | CARET => (boolean(lhs) && boolean(rhs)) || (numeric(lhs) && numeric(rhs)),
            EQ_EQ | NEQ => {
                let is_concrete_class = |ty: &Ty| match ty {
                    Ty::Class(class, _) => {
                        class_data(db, *class).is_some_and(|d| !d.is_interface())
                    }
                    _ => false,
                };
                if lhs.is_primitive() || rhs.is_primitive() {
                    // 基本类型之间：数值和 boolean 不能比较；和引用类型比较时由装箱 / 拆箱规则决定，不判断
                    !(lhs.is_primitive() && rhs.is_primitive() && (numeric(lhs) != numeric(rhs)))
                } else if is_concrete_class(lhs) && is_concrete_class(rhs) {
                    is_subtype(db, lhs, rhs) || is_subtype(db, rhs, lhs)
                } else {
                    true
                }
            }
            _ => true,
        };
        if !ok {
            self.diagnostic(
                expr.syntax().text_range(),
                InferDiagnosticKind::BadOperand {
                    op: op.text().to_string(),
                    operands: format!("{}, {}", lhs.display(db), rhs.display(db)),
                },
            );
        }
    }

    /// 函数式接口类型的抽象方法签名：(形参类型, 返回类型)。通配符实参取其界。
    fn functional_sig(&self, target: &Ty) -> Option<(Vec<Ty>, Ty)> {
        let db = self.db;
        let method = functional_method(db, target)?;
        let subst = method.subst.map_types(|ty| ty.wildcard_bound(db));
        let sig = method_sig(db, method.id);
        Some((
            sig.params.iter().map(|p| p.subst(&subst)).collect(),
            sig.ret.subst(&subst),
        ))
    }

    /// 推断 lambda，返回函数体的结果类型（块体没有 `return 值` 时为 `Void`）。
    fn infer_lambda(&mut self, lambda: &ast::LambdaExpr, target: Option<&Ty>) -> Ty {
        let functional = target.and_then(|t| self.functional_sig(t));
        self.push_scope();
        if let Some(params) = lambda.param_list() {
            for (idx, param) in params.params().enumerate() {
                let ty = match param.ty() {
                    Some(t) if !lower_type(&t).is_var() => self.lower_ty(&t),
                    _ => functional
                        .as_ref()
                        .and_then(|(params, _)| params.get(idx).cloned())
                        .unwrap_or(Ty::Unknown),
                };
                if let Some(name) = param.name() {
                    self.declare(&name, ty, LocalKind::LambdaParam);
                }
            }
        }

        // 返回类型还是待推断的类型参数时，不拿它当期望类型
        let expected_ret = functional
            .as_ref()
            .map(|(_, ret)| ret.clone())
            .filter(|ret| !matches!(ret, Ty::Void | Ty::TypeParam(_) | Ty::Unknown));
        self.ret_expected.push(expected_ret.clone());
        self.return_tys.push(Vec::new());

        let mut result = Ty::Void;
        if let Some(block) = lambda.body_block() {
            self.infer_block(&block);
        } else if let Some(body) = lambda.body_expr() {
            result = self.infer_expr(&body, expected_ret.as_ref());
        }

        let returns = self.return_tys.pop().unwrap_or_default();
        self.ret_expected.pop();
        self.pop_scope();

        if lambda.body_block().is_some() {
            result = if returns.is_empty() {
                Ty::Void
            } else {
                returns
                    .into_iter()
                    .find(|ty| !matches!(ty, Ty::Unknown | Ty::Null))
                    .unwrap_or(Ty::Unknown)
            };
        }
        result
    }

    /// 解析方法引用，返回被引用方法的返回类型（构造器引用返回被构造的类型）。
    fn infer_method_ref(&mut self, method_ref: &ast::MethodRef, target: Option<&Ty>) -> Ty {
        let db = self.db;
        let arity = target
            .and_then(|t| self.functional_sig(t))
            .map(|(params, _)| params.len());

        // (接收者类型, 接收者是否是类型名)
        let (receiver, is_type) = if let Some(t) = method_ref.receiver_type() {
            (self.lower_ty(&t), true)
        } else if let Some(e) = method_ref.receiver_expr() {
            match self.infer_value(&e, None) {
                Value::Ty(ty) => (ty, false),
                Value::Type(class) => (Ty::class(class), true),
                Value::Package(_) => return Ty::Unknown,
            }
        } else {
            return Ty::Unknown;
        };

        if method_ref.is_constructor() {
            return if is_type { receiver } else { Ty::Unknown };
        }
        let Some(name_ref) = method_ref.name_ref() else {
            return Ty::Unknown;
        };
        let candidates = lookup_methods(db, &receiver, &name_ref.text());

        let matches_arity = |candidate: &MethodCandidate| {
            let Some(arity) = arity else { return true };
            let n = method_sig(db, candidate.id).params.len();
            if is_type {
                // `Type::staticMethod` 或 `Type::instanceMethod`（第一个实参当接收者）
                if method_flags(self, candidate.id).is_static() {
                    n == arity
                } else {
                    n + 1 == arity
                }
            } else {
                n == arity
            }
        };
        let chosen = candidates
            .iter()
            .find(|c| matches_arity(c))
            .or(candidates.first())
            .cloned();
        match chosen {
            Some(candidate) => {
                self.result.resolutions.insert(
                    name_ref.syntax().text_range(),
                    Resolution::Method(candidate.id),
                );
                // `Map.Entry::getKey`：接收者写的是不带实参的泛型类，真正的类型实参要靠
                // 目标类型推断，这里得不到，按未知处理（否则会当成原始类型擦除成 Object）
                let raw_generic_receiver = is_type
                    && matches!(&receiver, Ty::Class(class, args) if args.is_empty()
                        && hir_def::class_data(db, *class).is_some_and(|d| !d.type_params.is_empty()));
                if raw_generic_receiver {
                    return Ty::Unknown;
                }
                candidate
                    .ret_ty(db)
                    .erase_params_of(GenericOwner::Method(candidate.id))
            }
            None => Ty::Unknown,
        }
    }

    // ---- 运算 ----

    fn infer_array_initializer(&mut self, init: &ast::ArrayInitializer, array_ty: &Ty) {
        let element = match array_ty {
            Ty::Array(element) => Some((**element).clone()),
            _ => None,
        };
        for expr in init.exprs() {
            self.infer_expr(&expr, element.as_ref());
        }
    }

    fn unary_promote(&self, ty: &Ty) -> Ty {
        match ty.unboxed(self.db) {
            Some(
                PrimitiveType::Byte
                | PrimitiveType::Short
                | PrimitiveType::Char
                | PrimitiveType::Int,
            ) => Ty::INT,
            Some(PrimitiveType::Boolean) | None => Ty::Unknown,
            Some(other) => Ty::Primitive(other),
        }
    }

    fn is_string(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Class(class, _) if class.binary_name(self.db) == "java/lang/String")
    }

    fn numeric_result(&self, lhs: &Ty, rhs: &Ty) -> Ty {
        match (lhs.unboxed(self.db), rhs.unboxed(self.db)) {
            (Some(a), Some(b)) if a.is_numeric() && b.is_numeric() => {
                Ty::Primitive(binary_numeric_promotion(a, b))
            }
            _ => Ty::Unknown,
        }
    }

    fn binary_result(&self, op: Option<SyntaxKind>, lhs: &Ty, rhs: &Ty) -> Ty {
        use SyntaxKind::*;
        match op {
            Some(EQ_EQ | NEQ | LT | GT | LT_EQ | GT_EQ | AMP_AMP | PIPE_PIPE) => Ty::BOOLEAN,
            Some(PLUS) if self.is_string(lhs) || self.is_string(rhs) => Ty::string(self.db),
            Some(AMP | PIPE | CARET) => {
                let is_bool = |ty: &Ty| ty.unboxed(self.db) == Some(PrimitiveType::Boolean);
                if is_bool(lhs) && is_bool(rhs) {
                    Ty::BOOLEAN
                } else {
                    self.numeric_result(lhs, rhs)
                }
            }
            Some(SHL | SHR | USHR) => self.unary_promote(lhs),
            Some(PLUS | MINUS | STAR | SLASH | PERCENT) => self.numeric_result(lhs, rhs),
            _ => Ty::Unknown,
        }
    }

    /// 条件表达式两个分支的合并类型（JLS §15.25 的简化版）。
    fn merge_branches(&self, a: &Ty, b: &Ty) -> Ty {
        let db = self.db;
        if a == b {
            return a.clone();
        }
        match (a, b) {
            (Ty::Unknown, other) | (other, Ty::Unknown) => other.clone(),
            (Ty::Null, other) | (other, Ty::Null) => other.boxed(db),
            _ => {
                if a.is_primitive() || b.is_primitive() {
                    let numeric = self.numeric_result(a, b);
                    if !numeric.is_unknown() {
                        return numeric;
                    }
                }
                let (a, b) = (a.boxed(db), b.boxed(db));
                if is_subtype(db, &a, &b) {
                    b
                } else {
                    a
                }
            }
        }
    }
}
