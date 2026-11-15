// 由 `cargo xtask codegen` 根据 `crates/syntax/ast.def` 生成，请勿手工修改。

ast_node!(CompilationUnit, COMPILATION_UNIT);
ast_node!(PackageDecl, PACKAGE_DECL);
ast_node!(ImportDecl, IMPORT_DECL);
ast_node!(ModuleDecl, MODULE_DECL);
ast_node!(Name, NAME);
ast_node!(NameRef, NAME_REF);
ast_node!(QualifiedName, QUALIFIED_NAME);
ast_node!(ModifierList, MODIFIER_LIST);
ast_node!(Annotation, ANNOTATION);
ast_node!(AnnotationArgList, ANNOTATION_ARG_LIST);
ast_node!(AnnotationArg, ANNOTATION_ARG);
ast_node!(ClassDecl, CLASS_DECL);
ast_node!(InterfaceDecl, INTERFACE_DECL);
ast_node!(EnumDecl, ENUM_DECL);
ast_node!(RecordDecl, RECORD_DECL);
ast_node!(AnnotationTypeDecl, ANNOTATION_TYPE_DECL);
ast_enum!(TypeDecl { ClassDecl, InterfaceDecl, EnumDecl, RecordDecl, AnnotationTypeDecl });
ast_node!(TypeParamList, TYPE_PARAM_LIST);
ast_node!(TypeParam, TYPE_PARAM);
ast_node!(ExtendsClause, EXTENDS_CLAUSE);
ast_node!(ImplementsClause, IMPLEMENTS_CLAUSE);
ast_node!(PermitsClause, PERMITS_CLAUSE);
ast_node!(ClassBody, CLASS_BODY);
ast_node!(EnumConstant, ENUM_CONSTANT);
ast_node!(FieldDecl, FIELD_DECL);
ast_node!(MethodDecl, METHOD_DECL);
ast_node!(ConstructorDecl, CONSTRUCTOR_DECL);
ast_node!(InitializerBlock, INITIALIZER_BLOCK);
ast_enum!(Member {
    FieldDecl,
    MethodDecl,
    ConstructorDecl,
    InitializerBlock,
    ClassDecl,
    InterfaceDecl,
    EnumDecl,
    RecordDecl,
    AnnotationTypeDecl,
});
ast_node!(ParamList, PARAM_LIST);
ast_node!(Param, PARAM);
ast_node!(ThrowsClause, THROWS_CLAUSE);
ast_node!(VariableDeclarator, VARIABLE_DECLARATOR);
ast_node!(DefaultValue, DEFAULT_VALUE);
ast_node!(PrimitiveType, PRIMITIVE_TYPE);
ast_node!(ClassType, CLASS_TYPE);
ast_node!(ArrayType, ARRAY_TYPE);
ast_node!(WildcardType, WILDCARD_TYPE);
ast_node!(IntersectionType, INTERSECTION_TYPE);
ast_node!(UnionType, UNION_TYPE);
ast_enum!(Type { PrimitiveType, ClassType, ArrayType, WildcardType, IntersectionType, UnionType });
ast_node!(TypeArgList, TYPE_ARG_LIST);
ast_node!(Block, BLOCK);
ast_node!(LocalVarDecl, LOCAL_VAR_DECL);
ast_node!(ExprStmt, EXPR_STMT);
ast_node!(IfStmt, IF_STMT);
ast_node!(WhileStmt, WHILE_STMT);
ast_node!(DoWhileStmt, DO_WHILE_STMT);
ast_node!(ForStmt, FOR_STMT);
ast_node!(ForInit, FOR_INIT);
ast_node!(ForUpdate, FOR_UPDATE);
ast_node!(ForeachStmt, FOREACH_STMT);
ast_node!(ReturnStmt, RETURN_STMT);
ast_node!(BreakStmt, BREAK_STMT);
ast_node!(ContinueStmt, CONTINUE_STMT);
ast_node!(ThrowStmt, THROW_STMT);
ast_node!(YieldStmt, YIELD_STMT);
ast_node!(AssertStmt, ASSERT_STMT);
ast_node!(SynchronizedStmt, SYNCHRONIZED_STMT);
ast_node!(TryStmt, TRY_STMT);
ast_node!(ResourceList, RESOURCE_LIST);
ast_node!(Resource, RESOURCE);
ast_node!(CatchClause, CATCH_CLAUSE);
ast_node!(FinallyClause, FINALLY_CLAUSE);
ast_node!(SwitchStmt, SWITCH_STMT);
ast_node!(SwitchBlock, SWITCH_BLOCK);
ast_node!(SwitchCase, SWITCH_CASE);
ast_node!(SwitchLabel, SWITCH_LABEL);
ast_node!(Guard, GUARD);
ast_node!(LabeledStmt, LABELED_STMT);
ast_node!(EmptyStmt, EMPTY_STMT);
ast_enum!(Stmt {
    Block,
    LocalVarDecl,
    ExprStmt,
    IfStmt,
    WhileStmt,
    DoWhileStmt,
    ForStmt,
    ForeachStmt,
    ReturnStmt,
    BreakStmt,
    ContinueStmt,
    ThrowStmt,
    YieldStmt,
    AssertStmt,
    SynchronizedStmt,
    TryStmt,
    SwitchStmt,
    LabeledStmt,
    EmptyStmt,
    ClassDecl,
    InterfaceDecl,
    EnumDecl,
    RecordDecl,
});
ast_node!(Literal, LITERAL);
ast_node!(NameExpr, NAME_EXPR);
ast_node!(ThisExpr, THIS_EXPR);
ast_node!(SuperExpr, SUPER_EXPR);
ast_node!(ParenExpr, PAREN_EXPR);
ast_node!(FieldAccess, FIELD_ACCESS);
ast_node!(MethodCall, METHOD_CALL);
ast_node!(ExplicitConstructorCall, EXPLICIT_CONSTRUCTOR_CALL);
ast_node!(ArgList, ARG_LIST);
ast_node!(NewExpr, NEW_EXPR);
ast_node!(ArrayCreationExpr, ARRAY_CREATION_EXPR);
ast_node!(DimExpr, DIM_EXPR);
ast_node!(ArrayInitializer, ARRAY_INITIALIZER);
ast_node!(ArrayAccess, ARRAY_ACCESS);
ast_node!(UnaryExpr, UNARY_EXPR);
ast_node!(PostfixExpr, POSTFIX_EXPR);
ast_node!(BinaryExpr, BINARY_EXPR);
ast_node!(AssignExpr, ASSIGN_EXPR);
ast_node!(ConditionalExpr, CONDITIONAL_EXPR);
ast_node!(InstanceofExpr, INSTANCEOF_EXPR);
ast_node!(CastExpr, CAST_EXPR);
ast_node!(LambdaExpr, LAMBDA_EXPR);
ast_node!(MethodRef, METHOD_REF);
ast_node!(ClassLiteral, CLASS_LITERAL);
ast_node!(SwitchExpr, SWITCH_EXPR);
ast_enum!(Expr {
    Literal,
    NameExpr,
    ThisExpr,
    SuperExpr,
    ParenExpr,
    FieldAccess,
    MethodCall,
    ExplicitConstructorCall,
    NewExpr,
    ArrayCreationExpr,
    ArrayInitializer,
    ArrayAccess,
    UnaryExpr,
    PostfixExpr,
    BinaryExpr,
    AssignExpr,
    ConditionalExpr,
    InstanceofExpr,
    CastExpr,
    LambdaExpr,
    MethodRef,
    ClassLiteral,
    SwitchExpr,
});
ast_node!(TypePattern, TYPE_PATTERN);
ast_node!(RecordPattern, RECORD_PATTERN);
ast_node!(UnnamedPattern, UNNAMED_PATTERN);
ast_enum!(Pattern { TypePattern, RecordPattern, UnnamedPattern });
