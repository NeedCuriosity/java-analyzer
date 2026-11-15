//! AST 节点：声明是生成的（见 `generated.rs`），访问器是手写的。

use smol_str::SmolStr;

use super::{doc_comment, support, AstChildren, AstNode};
use crate::{SyntaxKind, SyntaxNode, SyntaxToken};

macro_rules! ast_node {
    ($name:ident, $kind:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name {
            syntax: SyntaxNode,
        }

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == SyntaxKind::$kind
            }

            fn cast(syntax: SyntaxNode) -> Option<Self> {
                if Self::can_cast(syntax.kind()) {
                    Some(Self { syntax })
                } else {
                    None
                }
            }

            fn syntax(&self) -> &SyntaxNode {
                &self.syntax
            }
        }
    };
}

macro_rules! ast_enum {
    ($name:ident { $($variant:ident),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant($variant),)*
        }

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                $(<$variant as AstNode>::can_cast(kind))||*
            }

            fn cast(syntax: SyntaxNode) -> Option<Self> {
                let kind = syntax.kind();
                $(
                    if <$variant as AstNode>::can_cast(kind) {
                        return <$variant as AstNode>::cast(syntax).map($name::$variant);
                    }
                )*
                None
            }

            fn syntax(&self) -> &SyntaxNode {
                match self {
                    $($name::$variant(it) => it.syntax(),)*
                }
            }
        }

        $(
            impl From<$variant> for $name {
                fn from(it: $variant) -> Self {
                    $name::$variant(it)
                }
            }
        )*
    };
}

// 节点结构体和枚举的声明由 `cargo xtask codegen` 从 `crates/syntax/ast.def` 生成；
// 本文件只保留宏定义和手写的访问器。
include!("generated.rs");

// ===================================================================
// 编译单元
// ===================================================================

impl CompilationUnit {
    pub fn package(&self) -> Option<PackageDecl> {
        support::child(&self.syntax)
    }
    pub fn imports(&self) -> AstChildren<ImportDecl> {
        support::children(&self.syntax)
    }
    pub fn type_decls(&self) -> AstChildren<TypeDecl> {
        support::children(&self.syntax)
    }
    pub fn module(&self) -> Option<ModuleDecl> {
        support::child(&self.syntax)
    }
}

impl PackageDecl {
    pub fn name(&self) -> Option<QualifiedName> {
        support::child(&self.syntax)
    }
    pub fn modifier_list(&self) -> Option<ModifierList> {
        support::child(&self.syntax)
    }
}

impl ImportDecl {
    pub fn name(&self) -> Option<QualifiedName> {
        support::child(&self.syntax)
    }
    pub fn is_static(&self) -> bool {
        support::has_token(&self.syntax, SyntaxKind::STATIC_KW)
    }
    /// `import a.b.*;`
    pub fn is_on_demand(&self) -> bool {
        support::has_token(&self.syntax, SyntaxKind::STAR)
    }
}

impl ModuleDecl {
    pub fn name(&self) -> Option<QualifiedName> {
        support::child(&self.syntax)
    }
}

// ===================================================================
// 名字
// ===================================================================

impl Name {
    pub fn ident_token(&self) -> Option<SyntaxToken> {
        support::first_significant_token(&self.syntax)
    }
    pub fn text(&self) -> SmolStr {
        self.ident_token()
            .map(|t| SmolStr::new(t.text()))
            .unwrap_or_default()
    }
}

impl NameRef {
    pub fn ident_token(&self) -> Option<SyntaxToken> {
        support::first_significant_token(&self.syntax)
    }
    pub fn text(&self) -> SmolStr {
        self.ident_token()
            .map(|t| SmolStr::new(t.text()))
            .unwrap_or_default()
    }
}

impl QualifiedName {
    pub fn qualifier(&self) -> Option<QualifiedName> {
        support::child(&self.syntax)
    }
    pub fn name_ref(&self) -> Option<NameRef> {
        support::child(&self.syntax)
    }
    /// 从左到右的各段。
    pub fn segments(&self) -> Vec<SmolStr> {
        let mut res = Vec::new();
        let mut cur = Some(self.clone());
        while let Some(q) = cur {
            if let Some(name) = q.name_ref() {
                res.push(name.text());
            }
            cur = q.qualifier();
        }
        res.reverse();
        res
    }
    /// 点分形式的全名，例如 `java.util.List`。
    pub fn text(&self) -> String {
        self.segments().join(".")
    }
}

// ===================================================================
// 修饰符与注解
// ===================================================================

impl ModifierList {
    pub fn annotations(&self) -> AstChildren<Annotation> {
        support::children(&self.syntax)
    }
    pub fn has(&self, kind: SyntaxKind) -> bool {
        support::has_token(&self.syntax, kind)
    }
    /// 所有修饰符关键字 token（不含注解）。
    pub fn keywords(&self) -> impl Iterator<Item = SyntaxToken> {
        self.syntax
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .filter(|t| t.kind().is_modifier_keyword())
    }
}

impl Annotation {
    pub fn name(&self) -> Option<QualifiedName> {
        support::child(&self.syntax)
    }
    pub fn arg_list(&self) -> Option<AnnotationArgList> {
        support::child(&self.syntax)
    }
}

impl AnnotationArgList {
    pub fn args(&self) -> AstChildren<AnnotationArg> {
        support::children(&self.syntax)
    }
}

impl AnnotationArg {
    pub fn name_ref(&self) -> Option<NameRef> {
        support::child(&self.syntax)
    }
    pub fn value_expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn value_annotation(&self) -> Option<Annotation> {
        support::child(&self.syntax)
    }
}

// ===================================================================
// 类型声明
// ===================================================================

impl TypeDecl {
    pub fn name(&self) -> Option<Name> {
        support::child(self.syntax())
    }
    pub fn modifier_list(&self) -> Option<ModifierList> {
        support::child(self.syntax())
    }
    pub fn type_param_list(&self) -> Option<TypeParamList> {
        support::child(self.syntax())
    }
    pub fn extends_clause(&self) -> Option<ExtendsClause> {
        support::child(self.syntax())
    }
    pub fn implements_clause(&self) -> Option<ImplementsClause> {
        support::child(self.syntax())
    }
    pub fn permits_clause(&self) -> Option<PermitsClause> {
        support::child(self.syntax())
    }
    /// 记录的组件列表。
    pub fn record_components(&self) -> Option<ParamList> {
        match self {
            TypeDecl::RecordDecl(it) => support::child(it.syntax()),
            _ => None,
        }
    }
    pub fn body(&self) -> Option<ClassBody> {
        support::child(self.syntax())
    }
    pub fn doc_comment(&self) -> Option<SyntaxToken> {
        doc_comment(self.syntax())
    }
}

impl TypeParamList {
    pub fn type_params(&self) -> AstChildren<TypeParam> {
        support::children(&self.syntax)
    }
}

impl TypeParam {
    pub fn name(&self) -> Option<Name> {
        support::child(&self.syntax)
    }
    pub fn bounds(&self) -> AstChildren<Type> {
        support::children(&self.syntax)
    }
}

impl ExtendsClause {
    pub fn types(&self) -> AstChildren<Type> {
        support::children(&self.syntax)
    }
}

impl ImplementsClause {
    pub fn types(&self) -> AstChildren<Type> {
        support::children(&self.syntax)
    }
}

impl PermitsClause {
    pub fn types(&self) -> AstChildren<Type> {
        support::children(&self.syntax)
    }
}

impl ClassBody {
    pub fn members(&self) -> AstChildren<Member> {
        support::children(&self.syntax)
    }
    pub fn enum_constants(&self) -> AstChildren<EnumConstant> {
        support::children(&self.syntax)
    }
}

impl EnumConstant {
    pub fn name(&self) -> Option<Name> {
        support::child(&self.syntax)
    }
    pub fn annotations(&self) -> AstChildren<Annotation> {
        support::children(&self.syntax)
    }
    pub fn arg_list(&self) -> Option<ArgList> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<ClassBody> {
        support::child(&self.syntax)
    }
    pub fn doc_comment(&self) -> Option<SyntaxToken> {
        doc_comment(&self.syntax)
    }
}

// ===================================================================
// 成员
// ===================================================================

impl FieldDecl {
    pub fn modifier_list(&self) -> Option<ModifierList> {
        support::child(&self.syntax)
    }
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn declarators(&self) -> AstChildren<VariableDeclarator> {
        support::children(&self.syntax)
    }
    pub fn doc_comment(&self) -> Option<SyntaxToken> {
        doc_comment(&self.syntax)
    }
}

impl MethodDecl {
    pub fn modifier_list(&self) -> Option<ModifierList> {
        support::child(&self.syntax)
    }
    pub fn type_param_list(&self) -> Option<TypeParamList> {
        support::child(&self.syntax)
    }
    pub fn return_type(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn name(&self) -> Option<Name> {
        support::child(&self.syntax)
    }
    pub fn param_list(&self) -> Option<ParamList> {
        support::child(&self.syntax)
    }
    /// `int f()[]` 这种写在参数列表之后的数组维度个数。
    pub fn extra_dims(&self) -> usize {
        support::count_tokens(&self.syntax, SyntaxKind::L_BRACK)
    }
    pub fn throws_clause(&self) -> Option<ThrowsClause> {
        support::child(&self.syntax)
    }
    pub fn default_value(&self) -> Option<DefaultValue> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<Block> {
        support::child(&self.syntax)
    }
    pub fn doc_comment(&self) -> Option<SyntaxToken> {
        doc_comment(&self.syntax)
    }
}

impl ConstructorDecl {
    pub fn modifier_list(&self) -> Option<ModifierList> {
        support::child(&self.syntax)
    }
    pub fn type_param_list(&self) -> Option<TypeParamList> {
        support::child(&self.syntax)
    }
    pub fn name(&self) -> Option<Name> {
        support::child(&self.syntax)
    }
    /// 记录的紧凑构造器没有参数列表。
    pub fn param_list(&self) -> Option<ParamList> {
        support::child(&self.syntax)
    }
    pub fn throws_clause(&self) -> Option<ThrowsClause> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<Block> {
        support::child(&self.syntax)
    }
    pub fn doc_comment(&self) -> Option<SyntaxToken> {
        doc_comment(&self.syntax)
    }
}

impl InitializerBlock {
    pub fn is_static(&self) -> bool {
        support::child::<ModifierList>(&self.syntax).is_some_and(|m| m.has(SyntaxKind::STATIC_KW))
    }
    pub fn body(&self) -> Option<Block> {
        support::child(&self.syntax)
    }
}

impl ParamList {
    pub fn params(&self) -> AstChildren<Param> {
        support::children(&self.syntax)
    }
}

impl Param {
    pub fn modifier_list(&self) -> Option<ModifierList> {
        support::child(&self.syntax)
    }
    /// lambda 的隐式类型参数没有类型。
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn name(&self) -> Option<Name> {
        support::child(&self.syntax)
    }
    pub fn is_varargs(&self) -> bool {
        support::has_token(&self.syntax, SyntaxKind::ELLIPSIS)
    }
    pub fn extra_dims(&self) -> usize {
        support::count_tokens(&self.syntax, SyntaxKind::L_BRACK)
    }
}

impl ThrowsClause {
    pub fn types(&self) -> AstChildren<Type> {
        support::children(&self.syntax)
    }
}

impl VariableDeclarator {
    pub fn name(&self) -> Option<Name> {
        support::child(&self.syntax)
    }
    pub fn extra_dims(&self) -> usize {
        support::count_tokens(&self.syntax, SyntaxKind::L_BRACK)
    }
    pub fn initializer(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl DefaultValue {
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

// ===================================================================
// 类型
// ===================================================================

impl PrimitiveType {
    /// `int`、`boolean`、`void` 等关键字 token。
    pub fn keyword(&self) -> Option<SyntaxToken> {
        self.syntax
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|t| t.kind().is_keyword())
    }
}

impl ClassType {
    pub fn qualifier(&self) -> Option<ClassType> {
        support::child(&self.syntax)
    }
    pub fn name_ref(&self) -> Option<NameRef> {
        support::child(&self.syntax)
    }
    pub fn type_arg_list(&self) -> Option<TypeArgList> {
        support::child(&self.syntax)
    }
    pub fn annotations(&self) -> AstChildren<Annotation> {
        support::children(&self.syntax)
    }
    /// 从最外层限定符到自身的各段名字。
    pub fn segments(&self) -> Vec<SmolStr> {
        let mut res = Vec::new();
        let mut cur = Some(self.clone());
        while let Some(t) = cur {
            if let Some(name) = t.name_ref() {
                res.push(name.text());
            }
            cur = t.qualifier();
        }
        res.reverse();
        res
    }
}

impl ArrayType {
    pub fn element(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
}

impl WildcardType {
    pub fn bound(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn is_extends(&self) -> bool {
        support::has_token(&self.syntax, SyntaxKind::EXTENDS_KW)
    }
    pub fn is_super(&self) -> bool {
        support::has_token(&self.syntax, SyntaxKind::SUPER_KW)
    }
}

impl IntersectionType {
    pub fn types(&self) -> AstChildren<Type> {
        support::children(&self.syntax)
    }
}

impl UnionType {
    pub fn types(&self) -> AstChildren<Type> {
        support::children(&self.syntax)
    }
}

impl TypeArgList {
    pub fn args(&self) -> AstChildren<Type> {
        support::children(&self.syntax)
    }
}

// ===================================================================
// 语句
// ===================================================================

impl Block {
    pub fn statements(&self) -> AstChildren<Stmt> {
        support::children(&self.syntax)
    }
    pub fn r_curly_token(&self) -> Option<SyntaxToken> {
        support::token(&self.syntax, SyntaxKind::R_CURLY)
    }
}

impl LocalVarDecl {
    pub fn modifier_list(&self) -> Option<ModifierList> {
        support::child(&self.syntax)
    }
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn declarators(&self) -> AstChildren<VariableDeclarator> {
        support::children(&self.syntax)
    }
}

impl ExprStmt {
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl IfStmt {
    pub fn condition(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn then_branch(&self) -> Option<Stmt> {
        support::nth_child(&self.syntax, 0)
    }
    pub fn else_branch(&self) -> Option<Stmt> {
        support::token(&self.syntax, SyntaxKind::ELSE_KW)?;
        support::nth_child(&self.syntax, 1)
    }
}

impl WhileStmt {
    pub fn condition(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<Stmt> {
        support::child(&self.syntax)
    }
}

impl DoWhileStmt {
    pub fn body(&self) -> Option<Stmt> {
        support::child(&self.syntax)
    }
    pub fn condition(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl ForStmt {
    pub fn init(&self) -> Option<ForInit> {
        support::child(&self.syntax)
    }
    pub fn condition(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn update(&self) -> Option<ForUpdate> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<Stmt> {
        support::child(&self.syntax)
    }
}

impl ForInit {
    pub fn local_var_decl(&self) -> Option<LocalVarDecl> {
        support::child(&self.syntax)
    }
    pub fn exprs(&self) -> AstChildren<Expr> {
        support::children(&self.syntax)
    }
}

impl ForUpdate {
    pub fn exprs(&self) -> AstChildren<Expr> {
        support::children(&self.syntax)
    }
}

impl ForeachStmt {
    pub fn param(&self) -> Option<Param> {
        support::child(&self.syntax)
    }
    pub fn iterable(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<Stmt> {
        support::child(&self.syntax)
    }
}

impl ReturnStmt {
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl BreakStmt {
    pub fn label(&self) -> Option<NameRef> {
        support::child(&self.syntax)
    }
}

impl ContinueStmt {
    pub fn label(&self) -> Option<NameRef> {
        support::child(&self.syntax)
    }
}

impl ThrowStmt {
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl YieldStmt {
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl AssertStmt {
    pub fn condition(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 0)
    }
    pub fn message(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 1)
    }
}

impl SynchronizedStmt {
    pub fn lock(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<Block> {
        support::child(&self.syntax)
    }
}

impl TryStmt {
    pub fn resource_list(&self) -> Option<ResourceList> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<Block> {
        support::child(&self.syntax)
    }
    pub fn catch_clauses(&self) -> AstChildren<CatchClause> {
        support::children(&self.syntax)
    }
    pub fn finally_clause(&self) -> Option<FinallyClause> {
        support::child(&self.syntax)
    }
}

impl ResourceList {
    pub fn resources(&self) -> AstChildren<Resource> {
        support::children(&self.syntax)
    }
}

impl Resource {
    pub fn modifier_list(&self) -> Option<ModifierList> {
        support::child(&self.syntax)
    }
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn declarator(&self) -> Option<VariableDeclarator> {
        support::child(&self.syntax)
    }
    /// 直接引用已有变量的资源：`try (existing) {}`
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl CatchClause {
    pub fn param(&self) -> Option<Param> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<Block> {
        support::child(&self.syntax)
    }
}

impl FinallyClause {
    pub fn body(&self) -> Option<Block> {
        support::child(&self.syntax)
    }
}

impl SwitchStmt {
    pub fn scrutinee(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn block(&self) -> Option<SwitchBlock> {
        support::child(&self.syntax)
    }
}

impl SwitchBlock {
    pub fn cases(&self) -> AstChildren<SwitchCase> {
        support::children(&self.syntax)
    }
}

impl SwitchCase {
    pub fn label(&self) -> Option<SwitchLabel> {
        support::child(&self.syntax)
    }
    pub fn is_arrow(&self) -> bool {
        support::has_token(&self.syntax, SyntaxKind::ARROW)
    }
    pub fn statements(&self) -> AstChildren<Stmt> {
        support::children(&self.syntax)
    }
}

impl SwitchLabel {
    pub fn is_default(&self) -> bool {
        support::has_token(&self.syntax, SyntaxKind::DEFAULT_KW)
            && !support::has_token(&self.syntax, SyntaxKind::CASE_KW)
    }
    pub fn exprs(&self) -> AstChildren<Expr> {
        support::children(&self.syntax)
    }
    pub fn patterns(&self) -> AstChildren<Pattern> {
        support::children(&self.syntax)
    }
    pub fn guard(&self) -> Option<Guard> {
        support::child(&self.syntax)
    }
}

impl Guard {
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl LabeledStmt {
    pub fn label(&self) -> Option<Name> {
        support::child(&self.syntax)
    }
    pub fn body(&self) -> Option<Stmt> {
        support::child(&self.syntax)
    }
}

// ===================================================================
// 表达式
// ===================================================================

/// 节点里第一个非空白的直接子 token（运算符节点用它取运算符）。
fn operator_token(syntax: &SyntaxNode) -> Option<SyntaxToken> {
    support::first_significant_token(syntax)
}

impl Literal {
    pub fn token(&self) -> Option<SyntaxToken> {
        support::first_significant_token(&self.syntax)
    }
    pub fn kind(&self) -> Option<SyntaxKind> {
        self.token().map(|t| t.kind())
    }
}

impl NameExpr {
    pub fn name_ref(&self) -> Option<NameRef> {
        support::child(&self.syntax)
    }
}

impl ThisExpr {
    /// `Outer.this` 里的 `Outer`
    pub fn qualifier(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl SuperExpr {
    pub fn qualifier(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl ParenExpr {
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl FieldAccess {
    pub fn receiver(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn name_ref(&self) -> Option<NameRef> {
        support::child(&self.syntax)
    }
    pub fn dot_token(&self) -> Option<SyntaxToken> {
        support::token(&self.syntax, SyntaxKind::DOT)
    }
}

impl MethodCall {
    pub fn receiver(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn type_arg_list(&self) -> Option<TypeArgList> {
        support::child(&self.syntax)
    }
    pub fn name_ref(&self) -> Option<NameRef> {
        support::child(&self.syntax)
    }
    pub fn arg_list(&self) -> Option<ArgList> {
        support::child(&self.syntax)
    }
}

impl ExplicitConstructorCall {
    /// `this(...)` 还是 `super(...)`，由这个表达式的种类区分。
    pub fn target(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn is_super(&self) -> bool {
        matches!(self.target(), Some(Expr::SuperExpr(_)))
    }
    pub fn arg_list(&self) -> Option<ArgList> {
        support::child(&self.syntax)
    }
}

impl ArgList {
    pub fn args(&self) -> AstChildren<Expr> {
        support::children(&self.syntax)
    }
    pub fn l_paren_token(&self) -> Option<SyntaxToken> {
        support::token(&self.syntax, SyntaxKind::L_PAREN)
    }
    pub fn r_paren_token(&self) -> Option<SyntaxToken> {
        support::token(&self.syntax, SyntaxKind::R_PAREN)
    }
}

impl NewExpr {
    /// `outer.new Inner()` 里的 `outer`
    pub fn qualifier(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn arg_list(&self) -> Option<ArgList> {
        support::child(&self.syntax)
    }
    /// 匿名类的类体
    pub fn body(&self) -> Option<ClassBody> {
        support::child(&self.syntax)
    }
}

impl ArrayCreationExpr {
    pub fn element_type(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn dims(&self) -> AstChildren<DimExpr> {
        support::children(&self.syntax)
    }
    pub fn initializer(&self) -> Option<ArrayInitializer> {
        support::child(&self.syntax)
    }
}

impl DimExpr {
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl ArrayInitializer {
    pub fn exprs(&self) -> AstChildren<Expr> {
        support::children(&self.syntax)
    }
}

impl ArrayAccess {
    pub fn array(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 0)
    }
    pub fn index(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 1)
    }
}

impl UnaryExpr {
    pub fn op_token(&self) -> Option<SyntaxToken> {
        operator_token(&self.syntax)
    }
    pub fn operand(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl PostfixExpr {
    pub fn op_token(&self) -> Option<SyntaxToken> {
        operator_token(&self.syntax)
    }
    pub fn operand(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl BinaryExpr {
    pub fn lhs(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 0)
    }
    pub fn rhs(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 1)
    }
    pub fn op_token(&self) -> Option<SyntaxToken> {
        operator_token(&self.syntax)
    }
    pub fn op_kind(&self) -> Option<SyntaxKind> {
        self.op_token().map(|t| t.kind())
    }
}

impl AssignExpr {
    pub fn lhs(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 0)
    }
    pub fn rhs(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 1)
    }
    pub fn op_token(&self) -> Option<SyntaxToken> {
        operator_token(&self.syntax)
    }
}

impl ConditionalExpr {
    pub fn condition(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 0)
    }
    pub fn then_expr(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 1)
    }
    pub fn else_expr(&self) -> Option<Expr> {
        support::nth_child(&self.syntax, 2)
    }
}

impl InstanceofExpr {
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn pattern(&self) -> Option<Pattern> {
        support::child(&self.syntax)
    }
}

impl CastExpr {
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl LambdaExpr {
    pub fn param_list(&self) -> Option<ParamList> {
        support::child(&self.syntax)
    }
    pub fn body_expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn body_block(&self) -> Option<Block> {
        support::child(&self.syntax)
    }
    pub fn arrow_token(&self) -> Option<SyntaxToken> {
        support::token(&self.syntax, SyntaxKind::ARROW)
    }
}

impl MethodRef {
    pub fn receiver_expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn receiver_type(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn name_ref(&self) -> Option<NameRef> {
        support::child(&self.syntax)
    }
    /// `Foo::new`
    pub fn is_constructor(&self) -> bool {
        support::has_token(&self.syntax, SyntaxKind::NEW_KW)
    }
}

impl ClassLiteral {
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    /// `Foo.class` / `a.b.Foo.class` 里按表达式解析出来的类型名
    pub fn expr(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
}

impl SwitchExpr {
    pub fn scrutinee(&self) -> Option<Expr> {
        support::child(&self.syntax)
    }
    pub fn block(&self) -> Option<SwitchBlock> {
        support::child(&self.syntax)
    }
}

// ===================================================================
// 模式
// ===================================================================

impl TypePattern {
    pub fn modifier_list(&self) -> Option<ModifierList> {
        support::child(&self.syntax)
    }
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn name(&self) -> Option<Name> {
        support::child(&self.syntax)
    }
}

impl RecordPattern {
    pub fn ty(&self) -> Option<Type> {
        support::child(&self.syntax)
    }
    pub fn patterns(&self) -> AstChildren<Pattern> {
        support::children(&self.syntax)
    }
}
