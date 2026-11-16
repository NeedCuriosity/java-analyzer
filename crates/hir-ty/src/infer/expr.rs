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
    members::{
        constructors, functional_method, lookup_field, lookup_methods, FieldCandidate,
        MethodCandidate,
    },
    method_resolution::{certainly_violates_bounds, mentions_type_param, resolve_call, Arg, Call},
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

/// 诊断里展示的方法：`Math.abs(int)`。
fn describe_method(db: &dyn base_db::Db, method: MethodId) -> String {
    let class = class_data(db, method.class);
    let class_name = class
        .as_ref()
        .map(|d| d.name.to_string())
        .unwrap_or_default();
    let name = class
        .as_ref()
        .and_then(|d| {
            d.methods
                .get(method.idx as usize)
                .map(|m| m.name.to_string())
        })
        .unwrap_or_default();
    let params: Vec<String> = method_sig(db, method)
        .params
        .iter()
        .map(|p| p.display(db))
        .collect();
    format!("{class_name}.{name}({})", params.join(", "))
}

/// lambda 的体是否可能与返回 `void`（`returns_value` 为假）或返回值的函数类型兼容
/// （JLS 15.12.2.1 的"潜在兼容"）。拿不准时返回 `None`。
fn lambda_shape_fits(lambda: &ast::LambdaExpr, returns_value: bool) -> Option<bool> {
    if let Some(body) = lambda.body_expr() {
        let statement_expr = matches!(
            body,
            ast::Expr::MethodCall(_) | ast::Expr::NewExpr(_) | ast::Expr::AssignExpr(_)
        ) || matches!(&body, ast::Expr::PostfixExpr(_))
            || matches!(&body, ast::Expr::UnaryExpr(u) if u.syntax().children_with_tokens().any(|t| matches!(t.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS)));
        return Some(returns_value || statement_expr);
    }
    let block = lambda.body_block()?;
    // 只看最外层这个 lambda 自己的 return（嵌套的 lambda、类体里的不算）
    let mut with_value = false;
    let mut without_value = false;
    let mut walk = block.syntax().preorder();
    while let Some(event) = walk.next() {
        let syntax::WalkEvent::Enter(node) = event else {
            continue;
        };
        match node.kind() {
            SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY => walk.skip_subtree(),
            SyntaxKind::RETURN_STMT => {
                if node.children().any(|c| ast::Expr::can_cast(c.kind())) {
                    with_value = true;
                } else {
                    without_value = true;
                }
            }
            // 可能不能正常结束的块对两种函数类型都兼容，判断起来太细，放弃
            SyntaxKind::THROW_STMT
            | SyntaxKind::WHILE_STMT
            | SyntaxKind::FOR_STMT
            | SyntaxKind::DO_WHILE_STMT => return None,
            _ => {}
        }
    }
    match (with_value, without_value) {
        (true, false) => Some(returns_value),
        (false, _) => Some(!returns_value),
        (true, true) => None,
    }
}

/// 块体 lambda 自己的 `return 值;` 里的值（嵌套的 lambda、类体里的不算）。
fn own_return_values(block: &ast::Block) -> Vec<ast::Expr> {
    let mut values = Vec::new();
    let mut walk = block.syntax().preorder();
    while let Some(event) = walk.next() {
        let syntax::WalkEvent::Enter(node) = event else {
            continue;
        };
        match node.kind() {
            SyntaxKind::LAMBDA_EXPR | SyntaxKind::CLASS_BODY => walk.skip_subtree(),
            SyntaxKind::RETURN_STMT => {
                values.extend(ast::ReturnStmt::cast(node).and_then(|r| r.expr()));
            }
            _ => {}
        }
    }
    values
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
                let Some(body) = it.body() else {
                    return Value::Ty(result);
                };
                self.infer_class_body(&body, result.clone());
                // `new T() { ... }.member`：直接访问匿名类自己声明的成员时按匿名类本身的类型；
                // 别处仍按父类型（避免泛型推断把匿名类型传播出去）
                let mut outer = it.syntax().clone();
                while let Some(parent) = outer
                    .parent()
                    .filter(|p| p.kind() == SyntaxKind::PAREN_EXPR)
                {
                    outer = parent;
                }
                let is_receiver = outer.parent().is_some_and(|p| {
                    matches!(p.kind(), SyntaxKind::FIELD_ACCESS | SyntaxKind::METHOD_CALL)
                        && p.first_child().is_some_and(|first| first == outer)
                });
                match self.class_of_body(&body) {
                    Some(class) if is_receiver => this_ty(db, class),
                    _ => result,
                }
            }
            ast::Expr::ArrayCreationExpr(it) => {
                let mut ty = it.element_type().map_or(Ty::Unknown, |t| self.lower_ty(&t));
                for dim in it.dims() {
                    if let Some(size) = dim.expr() {
                        let size_ty = self.infer_expr(&size, Some(&Ty::INT));
                        self.check_index(&size, &size_ty);
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
            ast::Expr::ArrayAccess(it) => self.infer_array_access(it),
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
            ast::Expr::PostfixExpr(it) => {
                let operand = it
                    .operand()
                    .map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                // `s++`：只能用于数值类型
                let op = it.syntax().children_with_tokens().find_map(|e| {
                    e.into_token().filter(|t| {
                        matches!(t.kind(), SyntaxKind::PLUS_PLUS | SyntaxKind::MINUS_MINUS)
                    })
                });
                if let Some(op) = op {
                    let numeric = operand.unboxed(self.db).is_some_and(|p| p.is_numeric());
                    if !numeric && self.is_simple_ty(&operand) {
                        self.diagnostic(
                            it.syntax().text_range(),
                            InferDiagnosticKind::BadOperand {
                                op: op.text().to_string(),
                                operands: operand.display(self.db),
                            },
                        );
                    }
                }
                operand
            }
            ast::Expr::BinaryExpr(it) => {
                let lhs = it.lhs().map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                let rhs = it.rhs().map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                self.check_operands(it, &lhs, &rhs);
                self.binary_result(it.op_kind(), &lhs, &rhs)
            }
            ast::Expr::AssignExpr(it) => {
                let lhs = it.lhs().map_or(Ty::Unknown, |e| self.infer_expr(&e, None));
                if let Some(rhs) = it.rhs() {
                    let actual = self.infer_expr(&rhs, Some(&lhs));
                    // 复合赋值（`+=` 等）带隐式的强制转换，只检查简单赋值
                    let op = it.op_token();
                    let simple = op.as_ref().is_some_and(|op| op.kind() == SyntaxKind::EQ);
                    if simple && !lhs.is_unknown() {
                        self.check_assignable(rhs.syntax().text_range(), &actual, &lhs, &rhs);
                    } else if let Some(op) = op.filter(|_| !simple) {
                        if self.has_target_free_type(&rhs) {
                            self.check_compound_assign(it, &op, &lhs, &actual);
                        }
                    }
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
            ast::Expr::InstanceofExpr(it) => self.infer_instanceof(it),
            ast::Expr::CastExpr(it) => self.infer_cast(it),
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

    // 下面几种表达式单独成函数（不内联）：`infer_value_inner` 随表达式的嵌套层层递归，
    // 它的栈帧要尽量小

    #[inline(never)]
    fn infer_array_access(&mut self, it: &ast::ArrayAccess) -> Ty {
        let db = self.db;
        let array_expr = it.array();
        let array = array_expr
            .as_ref()
            .map_or(Ty::Unknown, |e| self.infer_expr(e, None));
        if let Some(index) = it.index() {
            let index_ty = self.infer_expr(&index, Some(&Ty::INT));
            self.check_index(&index, &index_ty);
        }
        // `s[0]`（`String s`）
        if let Some(array_expr) = &array_expr {
            let known_class = matches!(array, Ty::Class(..))
                && !array.contains_unknown()
                && !mentions_type_param(&array);
            if (array.is_primitive() || known_class)
                && !self.suppress_unresolved
                && self.has_target_free_type(array_expr)
            {
                self.diagnostic(
                    array_expr.syntax().text_range(),
                    InferDiagnosticKind::NotAnArray {
                        ty: array.display(db),
                    },
                );
            }
        }
        match array {
            Ty::Array(element) => (*element).clone(),
            _ => Ty::Unknown,
        }
    }

    #[inline(never)]
    fn infer_instanceof(&mut self, it: &ast::InstanceofExpr) -> Ty {
        let operand = it.expr();
        let matched = operand
            .as_ref()
            .map_or(Ty::Unknown, |e| self.infer_expr(e, None));
        let tested = match (it.ty(), it.pattern()) {
            (Some(t), _) => Some(self.lower_ty(&t)),
            (None, Some(ast::Pattern::TypePattern(p))) => p
                .ty()
                .filter(|t| !lower_type(t).is_var())
                .map(|t| self.lower_ty(&t)),
            _ => None,
        };
        if let (Some(operand), Some(tested)) = (&operand, &tested) {
            // 基本类型的 instanceof 是较新的语言特性，只看引用类型
            if matched.is_reference() && tested.is_reference() {
                self.check_castable(operand, &matched, tested);
            }
        }
        if let Some(pattern) = it.pattern() {
            self.bind_pattern(&pattern, &matched);
        }
        Ty::BOOLEAN
    }

    #[inline(never)]
    fn infer_cast(&mut self, it: &ast::CastExpr) -> Ty {
        let ty = it.ty().map_or(Ty::Unknown, |t| self.lower_ty(&t));
        if let Some(operand) = it.expr() {
            let actual = self.infer_expr(&operand, Some(&ty));
            self.check_castable(&operand, &actual, &ty);
        }
        ty
    }

    // ---- 名字 ----

    pub(super) fn outer_class(&self, class: ClassId) -> Option<ClassId> {
        let data = class_data(self.db, class)?;
        lookup_class(self.db, data.outer.as_deref()?)
    }

    /// 可能通过 static import 提供名为 `name` 的静态成员的类。
    /// 单个成员的 static import 遮蔽按需的（JLS §6.4.1），所以排在前面。
    fn static_import_classes(&self, name: &str) -> Vec<ClassId> {
        self.static_import_classes_with_kind(name)
            .into_iter()
            .map(|(class, _)| class)
            .collect()
    }

    /// 可能带进静态成员 `name` 的类，以及是否来自按需导入（单个导入在前）。
    fn static_import_classes_with_kind(&self, name: &str) -> Vec<(ClassId, bool)> {
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
                let class = if import.is_on_demand {
                    resolve_qualified_class(self.db, &import.segments)
                } else {
                    let (last, prefix) = import.segments.split_last()?;
                    (last == name)
                        .then(|| resolve_qualified_class(self.db, prefix))
                        .flatten()
                }?;
                Some((class, import.is_on_demand))
            })
            .collect()
    }

    /// 简单名能看到的 `receiver` 的字段 `name`：别的顶层类的私有字段不被继承，看不到；
    /// `strict` 时别的包里的包私有字段也看不到。
    fn visible_field(&self, receiver: &Ty, name: &str, strict: bool) -> Option<FieldCandidate> {
        let db = self.db;
        lookup_field(db, receiver, name).filter(|field| {
            class_data(db, field.id.class)
                .and_then(|data| data.fields.get(field.id.idx as usize).map(|f| f.flags))
                .is_none_or(|flags| {
                    let checked = match flags.visibility() {
                        hir_def::Visibility::Private => true,
                        hir_def::Visibility::Package => strict,
                        _ => false,
                    };
                    !checked
                        || crate::members::is_accessible(
                            db,
                            flags,
                            field.id.class,
                            Some(self.class),
                        )
                })
        })
    }

    fn resolve_simple_name(&mut self, name_ref: &ast::NameRef) -> Value {
        let db = self.db;
        let name = name_ref.text();
        let range = name_ref.syntax().text_range();

        // 由内到外逐层：这一层类体里的局部符号，然后是这一层类的字段（含继承来的）。
        // 局部类 / 匿名类的成员遮蔽外层方法的同名局部变量（JLS §6.4.1）
        for depth in (0..self.frames.len()).rev() {
            let lo = self.frame_scope_base.get(depth).copied().unwrap_or(0);
            let hi = self
                .frame_scope_base
                .get(depth + 1)
                .copied()
                .unwrap_or(self.scopes.len());
            if let Some(local) = self.lookup_local_in(lo, hi, &name, range.start()) {
                self.result
                    .resolutions
                    .insert(range, Resolution::Local(local));
                return Value::Ty(self.result.locals[local as usize].ty.clone());
            }
            let receiver = self.frames[depth].this_ty.clone();
            // 内层类继承不到的字段（别的包里包私有的）不遮蔽外层的局部变量
            if let Some(field) = self.visible_field(&receiver, &name, depth > 0) {
                self.result
                    .resolutions
                    .insert(range, Resolution::Field(field.id));
                return Value::Ty(field.ty);
            }
        }

        // 外层具名类的字段，然后是静态导入的
        let mut receivers: Vec<Ty> = Vec::new();
        let mut outer = self.outer_class(self.class);
        while let Some(class) = outer {
            receivers.push(this_ty(db, class));
            outer = self.outer_class(class);
        }
        receivers.extend(self.static_import_classes(&name).into_iter().map(Ty::class));
        for receiver in &receivers {
            if let Some(field) = self.visible_field(receiver, &name, false) {
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
        // 方法体里声明的局部类 / 记录 / 接口：在当前作用域里没找到对应的类，只知道名字存在
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
                if matches!(ty, Ty::Primitive(_)) {
                    if !self.suppress_unresolved {
                        self.diagnostic(
                            range,
                            InferDiagnosticKind::PrimitiveDereference {
                                ty: ty.display(db),
                                method: false,
                            },
                        );
                    }
                    return Value::Ty(Ty::Unknown);
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
        self.frames.len() > 1
            && self
                .frames
                .last()
                .is_some_and(|frame| frame.anonymous && frame.this_ty == *ty)
    }

    fn can_report_missing_member(&self, receiver: &Ty) -> bool {
        if self.suppress_unresolved || receiver.contains_unknown() {
            return false;
        }
        match receiver {
            Ty::Class(class, _) => self.hierarchy_is_reliable(*class),
            // 数组的成员是确定的：`length`、`clone()` 和 `Object` 的方法
            Ty::Array(_) => true,
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
        // 单个静态导入带进来的同名方法合在一起参与重载决议（来自不同的类也一样）；
        // 都没有时才看按需导入
        let imported = self.static_import_classes_with_kind(name);
        for on_demand in [false, true] {
            let mut found: Vec<MethodCandidate> = Vec::new();
            for (class, _) in imported.iter().filter(|(_, kind)| *kind == on_demand) {
                for candidate in lookup_methods(db, &Ty::class(*class), name) {
                    if method_flags(self, candidate.id).is_static()
                        && !found.iter().any(|c| c.id == candidate.id)
                    {
                        found.push(candidate);
                    }
                }
            }
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }

    fn infer_method_call(&mut self, call: &ast::MethodCall, expected: Option<&Ty>) -> Ty {
        let db = self.db;
        let receiver_expr = call.receiver();
        let receiver = receiver_expr.as_ref().map(|r| self.infer_value(r, None));
        let Some(name_ref) = call.name_ref() else {
            if let Some(args) = call.arg_list() {
                self.infer_args_untargeted(&args);
            }
            return Ty::Unknown;
        };
        let name = name_ref.text();
        let name_range = name_ref.syntax().text_range();
        // `x.foo()`（`int x`）、`voidCall().toString()`
        if let Some(Value::Ty(ty)) = &receiver {
            if self.check_primitive_receiver(ty, receiver_expr.as_ref(), name_range) {
                if let Some(args) = call.arg_list() {
                    self.infer_args_untargeted(&args);
                }
                return Ty::Unknown;
            }
        }
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
            Some(mut resolved) => {
                if let Some(expected) = expected {
                    self.check_target_typed_call(
                        &resolved,
                        &args,
                        expected,
                        call.syntax().text_range(),
                    );
                }
                resolved.return_ty(db, expected)
            }
            None => Ty::Unknown,
        }
    }

    /// 方法调用的接收者是基本类型或 void 值时报错，返回是否是这种情形。
    #[inline(never)]
    fn check_primitive_receiver(
        &mut self,
        ty: &Ty,
        receiver_expr: Option<&ast::Expr>,
        name_range: TextRange,
    ) -> bool {
        let void_call = *ty == Ty::Void && matches!(receiver_expr, Some(ast::Expr::MethodCall(_)));
        if !matches!(ty, Ty::Primitive(_)) && !void_call {
            return false;
        }
        if !self.suppress_unresolved {
            self.diagnostic(
                name_range,
                InferDiagnosticKind::PrimitiveDereference {
                    ty: ty.display(self.db),
                    method: true,
                },
            );
        }
        true
    }

    /// 泛型方法调用的结果交给一个带类型实参的目标类型（`List<String> xs = List.of(1);`）：
    /// 目标类型把返回类型里的类型参数固定成某个类型，直接以这个类型参数为形参类型的实参
    /// 必须能赋给它。只检查完全确定的情形：目标类型和实参类型都已知、都不含类型变量。
    fn check_target_typed_call(
        &mut self,
        call: &Call,
        arg_list: &ast::ArgList,
        expected: &Ty,
        range: TextRange,
    ) {
        let db = self.db;
        if !call.applicable || call.sig.n_type_params == 0 {
            return;
        }
        let Ty::Class(expected_class, expected_args) = expected else {
            return;
        };
        if expected_args.is_empty() || expected.contains_unknown() || mentions_type_param(expected)
        {
            return;
        }
        let ret = call.sig.ret.subst(&call.receiver_subst);
        let lifted = match &ret {
            Ty::Class(class, _) if class == expected_class => Some(ret.clone()),
            Ty::Class(..) => crate::subtype::find_super(db, &ret, *expected_class),
            _ => None,
        };
        let Some(lifted) = lifted else {
            return;
        };
        let owner = GenericOwner::Method(call.method);
        // 方法类型参数 → 目标类型要求它等于的类型
        let mut target = Subst::empty();
        for (arg, expected_arg) in lifted.type_args().iter().zip(expected_args.iter()) {
            if let Ty::TypeParam(param) = arg {
                if param.owner == owner && matches!(expected_arg, Ty::Class(..) | Ty::Array(_)) {
                    target.insert(*param, expected_arg.clone());
                }
            }
        }
        if target.is_empty() {
            return;
        }
        for (idx, arg) in arg_list.args().enumerate() {
            let Ty::TypeParam(param) = call.formal_param_ty(idx) else {
                continue;
            };
            let Some(required) = target.get(param) else {
                continue;
            };
            let Some(actual) = self
                .result
                .expr_types
                .get(&arg.syntax().text_range())
                .cloned()
            else {
                continue;
            };
            if !self.is_simple_ty(&actual) || crate::subtype::is_assignable(db, &actual, required) {
                continue;
            }
            let mut with_actual = Subst::empty();
            with_actual.insert(param, actual.boxed(db));
            let found = ret
                .subst(&with_actual)
                .subst(&call.inference.bindings)
                .erase_params_of(owner);
            self.diagnostic(
                range,
                InferDiagnosticKind::TypeMismatch {
                    expected: expected.display(db),
                    actual: found.display(db),
                },
            );
            return;
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
        // 先取走：推断实参时里面嵌套的调用会改写这个字段
        let report_inapplicable = self.report_inapplicable.take();
        let arg_exprs: Vec<ast::Expr> = arg_list.args().collect();
        let args: Vec<Arg> = arg_exprs
            .iter()
            .map(|arg| match arg {
                ast::Expr::LambdaExpr(lambda) => Arg::Lambda {
                    arity: lambda.param_list().map_or(0, |p| p.params().count()),
                    implicit: lambda.param_list().is_some_and(|p| {
                        p.params().next().is_some() && p.params().all(|param| param.ty().is_none())
                    }),
                },
                ast::Expr::MethodRef(_) => Arg::MethodRef,
                other => Arg::Ty(self.infer_expr(other, None)),
            })
            .collect();

        let resolved = resolve_call(db, &candidates, &args, explicit_type_args, extra_owners);
        // lambda 实参的目标类型确定：候选完整、参数个数合得上的只有选中的这一个（选择不可能
        // 取决于 lambda 的形状），且它的形参类型不涉及要推断的类型参数
        let lambda_targets_certain = report_inapplicable.is_some()
            && extra_owners.is_empty()
            && resolved.as_ref().is_some_and(|call| {
                let arity_fits = |candidate: &MethodCandidate| {
                    let sig = method_sig(db, candidate.id);
                    if sig.varargs {
                        args.len() + 1 >= sig.params.len()
                    } else {
                        sig.params.len() == args.len()
                    }
                };
                let mut fitting = candidates.iter().filter(|c| arity_fits(c));
                call.sig.n_type_params == 0
                    && fitting.next().is_some_and(|c| c.id == call.method)
                    && fitting.next().is_none()
            });
        if let (Some(name), Some(call)) = (&report_inapplicable, &resolved) {
            let lambdas_fit = |method: MethodId| {
                let sig = method_sig(db, method);
                arg_exprs.iter().enumerate().all(|(idx, arg)| {
                    let ast::Expr::LambdaExpr(lambda) = arg else {
                        return true;
                    };
                    let Some(functional) =
                        sig.params.get(idx).and_then(|p| functional_method(db, p))
                    else {
                        return false;
                    };
                    let returns_value = method_sig(db, functional.id).ret != Ty::Void;
                    lambda_shape_fits(lambda, returns_value) == Some(true)
                })
            };
            if let Some(other) = call
                .ambiguous
                .filter(|other| lambdas_fit(call.method) && lambdas_fit(*other))
            {
                self.diagnostic(
                    key.unwrap_or_else(|| arg_list.syntax().text_range()),
                    InferDiagnosticKind::AmbiguousCall {
                        name: name.clone(),
                        first: describe_method(db, call.method),
                        second: describe_method(db, other),
                    },
                );
            }
        }
        if let Some(name) = report_inapplicable {
            if resolved.as_ref().is_some_and(|call| !call.applicable)
                && self.inapplicable_is_certain(&candidates, &args, &arg_exprs)
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
                    let produced = self.infer_lambda(lambda, Some(&target));
                    if lambda_targets_certain && !mentions_type_param(&target) {
                        self.check_lambda(lambda, &target);
                    }
                    Some((produced, target))
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
    fn inapplicable_is_certain(
        &self,
        candidates: &[MethodCandidate],
        args: &[Arg],
        arg_exprs: &[ast::Expr],
    ) -> bool {
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
        // 每个候选要么参数个数对不上，要么按实参推出的类型参数确定违反了上界
        let violates_bounds = sigs.iter().all(|(candidate, sig)| {
            (!sig.varargs && sig.params.len() != args.len())
                || certainly_violates_bounds(db, candidate, args)
        });
        if violates_bounds {
            return true;
        }
        // 每个候选都有一个实参确定传不进对应的形参：只看擦除后的类，类型实参（包括通配符和
        // 方法自己的类型参数）不影响结论（`addAll("x")` 对 `Collection<? extends E>`）
        let certainly_inapplicable =
            |candidate: &MethodCandidate, sig: &crate::lower::MethodSig| {
                let fixed = if sig.varargs {
                    if args.len() + 1 < sig.params.len() {
                        return true;
                    }
                    sig.params.len() - 1
                } else if sig.params.len() != args.len() {
                    return true;
                } else {
                    sig.params.len()
                };
                (0..fixed).any(|idx| {
                    let Arg::Ty(arg) = &args[idx] else {
                        return false;
                    };
                    let Some(arg) = self.erased_arg(arg, &arg_exprs[idx]) else {
                        return false;
                    };
                    let param = sig.params[idx].subst(&candidate.subst);
                    let param = match &param {
                        Ty::Primitive(_) => param.clone(),
                        Ty::Class(class, _) if self.hierarchy_is_reliable(*class) => {
                            Ty::class(*class)
                        }
                        _ => return false,
                    };
                    !crate::subtype::is_assignable(db, &arg, &param)
                })
            };
        if sigs
            .iter()
            .all(|(candidate, sig)| certainly_inapplicable(candidate, sig))
        {
            return true;
        }
        // 形参都是非泛型类型时，能不能赋值只取决于实参类型擦除后的类：实参可以是
        // 参数化类型（`List<String>` 传给 `String`）或原始类型。但这类实参的类型必须是
        // 声明出来的（变量、字段、强制转换、显式写出类型的 `new`），泛型方法调用推出的
        // 类型可能依赖目标类型，不可靠
        let args_simple = args.iter().zip(arg_exprs).all(|(arg, expr)| match arg {
            Arg::Ty(ty) => {
                self.is_simple_ty(ty)
                    || matches!(ty, Ty::Class(..))
                        && !ty.contains_unknown()
                        && has_declared_type(expr)
            }
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

    /// 实参擦除后的类型，只在它确定、且能不能传给某个类只取决于这个擦除时给出：基本类型、
    /// 非泛型类，或者类型是声明出来的参数化类型；类的层次结构要可靠。
    fn erased_arg(&self, ty: &Ty, expr: &ast::Expr) -> Option<Ty> {
        match ty {
            Ty::Primitive(_) if self.has_target_free_type(expr) => Some(ty.clone()),
            // `Object` 常常是推断不出来时的退路，不拿它下结论
            Ty::Class(class, _)
                if !ty.contains_unknown()
                    && class.binary_name(self.db) != "java/lang/Object"
                    && self.has_target_free_type(expr)
                    && self.hierarchy_is_reliable(*class) =>
            {
                Some(Ty::class(*class))
            }
            _ => None,
        }
    }

    /// 二元运算符的操作数类型检查（只看两边都是简单类型的情形）。
    #[inline(never)]
    fn check_operands(&mut self, expr: &ast::BinaryExpr, lhs: &Ty, rhs: &Ty) {
        let Some(op) = expr.op_token() else { return };
        let db = self.db;
        // `"" + voidCall()`
        let void_call = |e: Option<ast::Expr>, ty: &Ty| {
            *ty == Ty::Void && matches!(e, Some(ast::Expr::MethodCall(_)))
        };
        let has_void = void_call(expr.lhs(), lhs) || void_call(expr.rhs(), rhs);
        // `count == null`（`int count`）
        let primitive_vs_null = matches!(op.kind(), SyntaxKind::EQ_EQ | SyntaxKind::NEQ)
            && matches!(
                (lhs, rhs),
                (Ty::Primitive(_), Ty::Null) | (Ty::Null, Ty::Primitive(_))
            );
        let ok = if has_void || primitive_vs_null {
            false
        } else if self.is_simple_ty(lhs) && self.is_simple_ty(rhs) {
            self.binary_operands_ok(op.kind(), lhs, rhs)
        } else {
            true
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

    /// 两边都是简单类型时，二元运算符能否用在这对操作数上。
    fn binary_operands_ok(&self, op: SyntaxKind, lhs: &Ty, rhs: &Ty) -> bool {
        use SyntaxKind::*;
        let db = self.db;
        let numeric = |ty: &Ty| ty.unboxed(db).is_some_and(|p| p.is_numeric());
        let integral = |ty: &Ty| self.is_integral(ty);
        let boolean = |ty: &Ty| ty.unboxed(db) == Some(PrimitiveType::Boolean);
        match op {
            PLUS => self.is_string(lhs) || self.is_string(rhs) || (numeric(lhs) && numeric(rhs)),
            MINUS | STAR | SLASH | PERCENT | LT | GT | LT_EQ | GT_EQ => {
                numeric(lhs) && numeric(rhs)
            }
            SHL | SHR | USHR => integral(lhs) && integral(rhs),
            AMP_AMP | PIPE_PIPE => boolean(lhs) && boolean(rhs),
            AMP | PIPE | CARET => {
                (boolean(lhs) && boolean(rhs)) || (integral(lhs) && integral(rhs))
            }
            EQ_EQ | NEQ => {
                let class_info = |ty: &Ty| match ty {
                    Ty::Class(class, _) => class_data(db, *class).map(|d| d.is_interface()),
                    _ => None,
                };
                let is_concrete_class = |ty: &Ty| class_info(ty) == Some(false);
                match (lhs, rhs) {
                    // 基本类型之间：数值和 boolean 不能比较
                    (Ty::Primitive(_), Ty::Primitive(_)) => numeric(lhs) == numeric(rhs),
                    // 基本类型和引用类型：引用类型要能拆箱成同一类（数值 / boolean）的基本类型。
                    // 只对不是包装类的 final 类（`String`）和包装类下结论
                    (Ty::Primitive(_), Ty::Class(class, _))
                    | (Ty::Class(class, _), Ty::Primitive(_)) => {
                        let (prim, other) = if lhs.is_primitive() {
                            (lhs, rhs)
                        } else {
                            (rhs, lhs)
                        };
                        match other.unboxed(db) {
                            Some(_) => numeric(prim) == numeric(other),
                            None => !class_data(db, *class)
                                .is_some_and(|d| !d.is_interface() && d.flags.is_final()),
                        }
                    }
                    _ if is_concrete_class(lhs) && is_concrete_class(rhs) => {
                        is_subtype(db, lhs, rhs) || is_subtype(db, rhs, lhs)
                    }
                    _ => true,
                }
            }
            _ => true,
        }
    }

    /// 复合赋值（`b += 1`、`x &= true`）：对应的二元运算本身不成立，或者把字符串拼接的结果
    /// 赋回基本类型的变量（`int x; x += "a"`）。
    #[inline(never)]
    pub(super) fn check_compound_assign(
        &mut self,
        expr: &ast::AssignExpr,
        op: &syntax::SyntaxToken,
        lhs: &Ty,
        rhs: &Ty,
    ) {
        use SyntaxKind::*;
        if !self.is_simple_ty(lhs) || !self.is_simple_ty(rhs) {
            return;
        }
        let binary = match op.kind() {
            PLUS_EQ => PLUS,
            MINUS_EQ => MINUS,
            STAR_EQ => STAR,
            SLASH_EQ => SLASH,
            PERCENT_EQ => PERCENT,
            AMP_EQ => AMP,
            PIPE_EQ => PIPE,
            CARET_EQ => CARET,
            SHL_EQ => SHL,
            SHR_EQ => SHR,
            USHR_EQ => USHR,
            _ => return,
        };
        let ok = if binary == PLUS && self.is_string(lhs) {
            true
        } else if binary == PLUS && self.is_string(rhs) {
            lhs.unboxed(self.db).is_none()
        } else {
            self.binary_operands_ok(binary, lhs, rhs)
        };
        if !ok {
            self.diagnostic(
                expr.syntax().text_range(),
                InferDiagnosticKind::BadOperand {
                    op: op.text().to_string(),
                    operands: format!("{}, {}", lhs.display(self.db), rhs.display(self.db)),
                },
            );
        }
    }

    /// 表达式的类型不依赖目标类型：声明出来的类型、字面量、运算结果、非泛型方法的调用。
    pub(super) fn has_target_free_type(&self, expr: &ast::Expr) -> bool {
        match expr {
            ast::Expr::Literal(_) | ast::Expr::BinaryExpr(_) | ast::Expr::UnaryExpr(_) => true,
            ast::Expr::ParenExpr(paren) => {
                paren.expr().is_some_and(|e| self.has_target_free_type(&e))
            }
            _ => has_declared_type(expr) || self.is_certain_call(expr),
        }
    }

    /// 非泛型方法的调用，并且参数个数合得上的候选只有一个：返回类型不取决于重载决议的细节。
    pub(super) fn is_certain_call(&self, expr: &ast::Expr) -> bool {
        let ast::Expr::MethodCall(call) = expr else {
            return false;
        };
        let Some(args) = call.arg_list() else {
            return false;
        };
        if !self.is_non_generic_call(expr) {
            return false;
        }
        let n = args.args().count();
        self.result
            .calls
            .get(&args.syntax().text_range())
            .is_some_and(|info| {
                info.candidates
                    .iter()
                    .filter(|candidate| {
                        let sig = method_sig(self.db, candidate.id);
                        if sig.varargs {
                            n + 1 >= sig.params.len()
                        } else {
                            sig.params.len() == n
                        }
                    })
                    .count()
                    == 1
            })
    }

    /// 擦除后的类（类型完全已知、层次结构可靠时）。
    pub(super) fn reliable_class(&self, ty: &Ty) -> Option<ClassId> {
        match ty {
            Ty::Class(class, _)
                if !ty.contains_unknown()
                    && !mentions_type_param(ty)
                    && self.hierarchy_is_reliable(*class) =>
            {
                Some(*class)
            }
            _ => None,
        }
    }

    /// `from` 类型的值确定不能强制转换成 `to`（JLS §5.5）。只对基本类型和类型完全已知的类下结论；
    /// 数组、类型变量、交叉类型一律放过。
    fn cast_is_impossible(&self, from: &Ty, to: &Ty) -> bool {
        let db = self.db;
        match (from, to) {
            (Ty::Primitive(a), Ty::Primitive(b)) => {
                (*a == PrimitiveType::Boolean) != (*b == PrimitiveType::Boolean)
            }
            // 装箱后拓宽：`(Object) 1`、`(Number) 1`
            (Ty::Primitive(_), Ty::Class(..)) => match self.reliable_class(to) {
                Some(class) => !is_subtype(db, &from.boxed(db), &Ty::class(class)),
                None => false,
            },
            // 拆箱后拓宽（`(long) integer`），或者先收窄到包装类再拆箱（`(int) object`）
            (Ty::Class(..), Ty::Primitive(target)) => match self.reliable_class(from) {
                Some(class) => match from.unboxed(db) {
                    Some(unboxed) => !crate::subtype::primitive_widens(unboxed, *target),
                    None => !is_subtype(db, &to.boxed(db), &Ty::class(class)),
                },
                None => false,
            },
            (Ty::Class(..), Ty::Class(..)) => {
                let (Some(a), Some(b)) = (self.reliable_class(from), self.reliable_class(to))
                else {
                    return false;
                };
                let (raw_a, raw_b) = (Ty::class(a), Ty::class(b));
                if is_subtype(db, &raw_a, &raw_b) || is_subtype(db, &raw_b, &raw_a) {
                    return false;
                }
                let (Some(data_a), Some(data_b)) = (class_data(db, a), class_data(db, b)) else {
                    return false;
                };
                match (data_a.is_interface(), data_b.is_interface()) {
                    (false, false) => true,
                    (true, false) => data_b.flags.is_final(),
                    (false, true) => data_a.flags.is_final(),
                    (true, true) => false,
                }
            }
            _ => false,
        }
    }

    /// 引用的是 catch 形参：多重 catch 的形参类型是各个备选的最小上界，这里只记了第一个。
    fn is_catch_param(&self, expr: &ast::Expr) -> bool {
        let ast::Expr::NameExpr(name) = expr else {
            return false;
        };
        name.name_ref().is_some_and(|name_ref| {
            matches!(
                self.result.resolution(name_ref.syntax().text_range()),
                Some(Resolution::Local(local))
                    if self.result.locals[*local as usize].kind == LocalKind::CatchParam
            )
        })
    }

    /// 强制转换 / `instanceof`：值的类型确定转换不过去时报类型不匹配。
    fn check_castable(&mut self, operand: &ast::Expr, from: &Ty, to: &Ty) {
        if self.suppress_unresolved
            || !self.has_target_free_type(operand)
            || self.is_catch_param(operand)
            || !self.cast_is_impossible(from, to)
        {
            return;
        }
        self.diagnostic(
            operand.syntax().text_range(),
            InferDiagnosticKind::TypeMismatch {
                expected: to.display(self.db),
                actual: from.display(self.db),
            },
        );
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

    /// 目标类型确定的 lambda（已经按这个目标类型推断过）：检查形参个数、函数体和 void /
    /// 返回值的兼容性，以及返回值的类型。只在目标是层次结构完整、没有未知部分的函数式接口时下结论。
    pub(super) fn check_lambda(&mut self, lambda: &ast::LambdaExpr, target: &Ty) {
        let db = self.db;
        let Ty::Class(class, _) = target else {
            return;
        };
        if target.contains_unknown()
            || !self.hierarchy_is_reliable(*class)
            || lambda
                .syntax()
                .descendants()
                .any(|n| n.kind() == SyntaxKind::ERROR)
        {
            return;
        }
        let Some(body_range) = lambda
            .body_expr()
            .map(|e| e.syntax().text_range())
            .or_else(|| lambda.body_block().map(|b| b.syntax().text_range()))
        else {
            return;
        };
        let Some(method) = functional_method(db, target) else {
            return;
        };
        let sig = method_sig(db, method.id);
        // 泛型的函数式方法不能用 lambda 实现，那是另一种错误
        if sig.n_type_params != 0 {
            return;
        }
        let arity = lambda.param_list().map_or(0, |p| p.params().count());
        if arity != sig.params.len() {
            self.diagnostic(
                lambda.syntax().text_range(),
                InferDiagnosticKind::LambdaArity {
                    target: target.display(db),
                    expected: sig.params.len(),
                    found: arity,
                },
            );
            return;
        }
        let Some((_, ret)) = self.functional_sig(target) else {
            return;
        };
        let returns_value = ret != Ty::Void;
        if lambda_shape_fits(lambda, returns_value) == Some(false) {
            let kind = if returns_value {
                InferDiagnosticKind::LambdaMissingValue {
                    target: target.display(db),
                    expected: ret.display(db),
                }
            } else {
                InferDiagnosticKind::LambdaReturnsValue {
                    target: target.display(db),
                }
            };
            self.diagnostic(body_range, kind);
            return;
        }
        if !returns_value {
            return;
        }
        let values = match lambda.body_expr() {
            Some(body) => vec![body],
            None => lambda
                .body_block()
                .map(|block| own_return_values(&block))
                .unwrap_or_default(),
        };
        for value in values {
            let range = value.syntax().text_range();
            if let Some(ty) = self.result.type_of(range).cloned() {
                self.check_assignable(range, &ty, &ret, &value);
            }
        }
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
            let actual = self.infer_expr(&expr, element.as_ref());
            if let Some(element) = &element {
                if !matches!(expr, ast::Expr::ArrayInitializer(_)) && !element.contains_unknown() {
                    self.check_assignable(expr.syntax().text_range(), &actual, element, &expr);
                }
            }
        }
    }

    /// 数组下标和数组长度要能提升成 int（`a["x"]`、`new int[1L]`）。
    #[inline(never)]
    fn check_index(&mut self, index: &ast::Expr, ty: &Ty) {
        let integral = matches!(
            ty.unboxed(self.db),
            Some(
                PrimitiveType::Byte
                    | PrimitiveType::Short
                    | PrimitiveType::Char
                    | PrimitiveType::Int
            )
        );
        if !integral && self.is_simple_ty(ty) && self.has_target_free_type(index) {
            self.diagnostic(
                index.syntax().text_range(),
                InferDiagnosticKind::TypeMismatch {
                    expected: Ty::INT.display(self.db),
                    actual: ty.display(self.db),
                },
            );
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

    fn is_integral(&self, ty: &Ty) -> bool {
        matches!(
            ty.unboxed(self.db),
            Some(
                PrimitiveType::Byte
                    | PrimitiveType::Short
                    | PrimitiveType::Char
                    | PrimitiveType::Int
                    | PrimitiveType::Long
            )
        )
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
                } else if self.is_integral(lhs) && self.is_integral(rhs) {
                    self.numeric_result(lhs, rhs)
                } else {
                    Ty::Unknown
                }
            }
            // 移位的结果是左操作数提升后的类型；浮点操作数本身就是错误，结果不再往下传
            Some(SHL | SHR | USHR) => {
                let non_integral = |ty: &Ty| ty.unboxed(self.db).is_some() && !self.is_integral(ty);
                if non_integral(lhs) || non_integral(rhs) {
                    Ty::Unknown
                } else {
                    self.unary_promote(lhs)
                }
            }
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

/// 表达式的类型直接来自声明，不依赖推断：变量、字段、`this`、强制转换、显式写出类型实参
/// （或非泛型）的 `new`。
pub(crate) fn has_declared_type(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::NameExpr(_)
        | ast::Expr::FieldAccess(_)
        | ast::Expr::ThisExpr(_)
        | ast::Expr::CastExpr(_)
        | ast::Expr::ArrayAccess(_) => true,
        ast::Expr::ParenExpr(paren) => paren.expr().is_some_and(|e| has_declared_type(&e)),
        ast::Expr::NewExpr(new) => {
            new.body().is_none()
                && new
                    .ty()
                    .is_some_and(|t| !t.syntax().text().to_string().contains("<>"))
        }
        _ => false,
    }
}
