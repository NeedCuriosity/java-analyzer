//! 语法树里所有 token 与节点的种类。
//!
//! 约定：token 种类全部排在 `__LAST_TOKEN` 之前，
//! 节点种类排在其后。本文件对应 `xtask codegen` 的输出，目前为手写维护。

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u16)]
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
pub enum SyntaxKind {
    // ---- 特殊 ----
    #[doc(hidden)]
    TOMBSTONE,
    #[doc(hidden)]
    EOF,

    // ---- 标点 ----
    L_PAREN,
    R_PAREN,
    L_CURLY,
    R_CURLY,
    L_BRACK,
    R_BRACK,
    SEMICOLON,
    COMMA,
    DOT,
    ELLIPSIS,
    AT,
    COLON_COLON,
    COLON,
    QUESTION,
    ARROW,
    EQ,
    EQ_EQ,
    NEQ,
    LT,
    GT,
    LT_EQ,
    GT_EQ,
    BANG,
    TILDE,
    AMP_AMP,
    PIPE_PIPE,
    PLUS_PLUS,
    MINUS_MINUS,
    PLUS,
    MINUS,
    STAR,
    SLASH,
    AMP,
    PIPE,
    CARET,
    PERCENT,
    SHL,
    /// `>>`，由 parser 用相邻的两个 `>` 拼成
    SHR,
    /// `>>>`，由 parser 用相邻的三个 `>` 拼成
    USHR,
    PLUS_EQ,
    MINUS_EQ,
    STAR_EQ,
    SLASH_EQ,
    AMP_EQ,
    PIPE_EQ,
    CARET_EQ,
    PERCENT_EQ,
    SHL_EQ,
    /// `>>=`，由 parser 拼成
    SHR_EQ,
    /// `>>>=`，由 parser 拼成
    USHR_EQ,

    // ---- 字面量 ----
    INT_LITERAL,
    LONG_LITERAL,
    FLOAT_LITERAL,
    DOUBLE_LITERAL,
    CHAR_LITERAL,
    STRING_LITERAL,
    TEXT_BLOCK,

    // ---- 保留关键字 ----
    ABSTRACT_KW,
    ASSERT_KW,
    BOOLEAN_KW,
    BREAK_KW,
    BYTE_KW,
    CASE_KW,
    CATCH_KW,
    CHAR_KW,
    CLASS_KW,
    CONST_KW,
    CONTINUE_KW,
    DEFAULT_KW,
    DO_KW,
    DOUBLE_KW,
    ELSE_KW,
    ENUM_KW,
    EXTENDS_KW,
    FINAL_KW,
    FINALLY_KW,
    FLOAT_KW,
    FOR_KW,
    GOTO_KW,
    IF_KW,
    IMPLEMENTS_KW,
    IMPORT_KW,
    INSTANCEOF_KW,
    INT_KW,
    INTERFACE_KW,
    LONG_KW,
    NATIVE_KW,
    NEW_KW,
    PACKAGE_KW,
    PRIVATE_KW,
    PROTECTED_KW,
    PUBLIC_KW,
    RETURN_KW,
    SHORT_KW,
    STATIC_KW,
    STRICTFP_KW,
    SUPER_KW,
    SWITCH_KW,
    SYNCHRONIZED_KW,
    THIS_KW,
    THROW_KW,
    THROWS_KW,
    TRANSIENT_KW,
    TRY_KW,
    VOID_KW,
    VOLATILE_KW,
    WHILE_KW,
    TRUE_KW,
    FALSE_KW,
    NULL_KW,

    // ---- 上下文关键字（lexer 产出 IDENT，parser 视上下文改写）----
    VAR_KW,
    YIELD_KW,
    RECORD_KW,
    SEALED_KW,
    NON_SEALED_KW,
    PERMITS_KW,
    WHEN_KW,
    MODULE_KW,
    OPEN_KW,
    REQUIRES_KW,
    TRANSITIVE_KW,
    EXPORTS_KW,
    OPENS_KW,
    TO_KW,
    USES_KW,
    PROVIDES_KW,
    WITH_KW,
    UNDERSCORE_KW,

    // ---- 其它 token ----
    IDENT,
    WHITESPACE,
    LINE_COMMENT,
    BLOCK_COMMENT,
    DOC_COMMENT,
    ERROR_TOKEN,

    #[doc(hidden)]
    __LAST_TOKEN,

    // ---- 节点：编译单元 ----
    COMPILATION_UNIT,
    PACKAGE_DECL,
    IMPORT_DECL,
    MODULE_DECL,
    MODULE_BODY,
    REQUIRES_DIRECTIVE,
    EXPORTS_DIRECTIVE,
    OPENS_DIRECTIVE,
    USES_DIRECTIVE,
    PROVIDES_DIRECTIVE,

    // ---- 节点：名字 ----
    NAME,
    NAME_REF,
    QUALIFIED_NAME,

    // ---- 节点：声明 ----
    CLASS_DECL,
    INTERFACE_DECL,
    ENUM_DECL,
    RECORD_DECL,
    ANNOTATION_TYPE_DECL,
    MODIFIER_LIST,
    ANNOTATION,
    ANNOTATION_ARG_LIST,
    ANNOTATION_ARG,
    TYPE_PARAM_LIST,
    TYPE_PARAM,
    EXTENDS_CLAUSE,
    IMPLEMENTS_CLAUSE,
    PERMITS_CLAUSE,
    CLASS_BODY,
    ENUM_CONSTANT,
    FIELD_DECL,
    METHOD_DECL,
    CONSTRUCTOR_DECL,
    INITIALIZER_BLOCK,
    PARAM_LIST,
    PARAM,
    THROWS_CLAUSE,
    VARIABLE_DECLARATOR,
    DEFAULT_VALUE,

    // ---- 节点：类型 ----
    PRIMITIVE_TYPE,
    CLASS_TYPE,
    ARRAY_TYPE,
    TYPE_ARG_LIST,
    WILDCARD_TYPE,
    INTERSECTION_TYPE,
    UNION_TYPE,

    // ---- 节点：语句 ----
    BLOCK,
    LOCAL_VAR_DECL,
    EXPR_STMT,
    IF_STMT,
    WHILE_STMT,
    DO_WHILE_STMT,
    FOR_STMT,
    FOR_INIT,
    FOR_UPDATE,
    FOREACH_STMT,
    RETURN_STMT,
    BREAK_STMT,
    CONTINUE_STMT,
    THROW_STMT,
    YIELD_STMT,
    ASSERT_STMT,
    SYNCHRONIZED_STMT,
    TRY_STMT,
    RESOURCE_LIST,
    RESOURCE,
    CATCH_CLAUSE,
    FINALLY_CLAUSE,
    SWITCH_STMT,
    SWITCH_BLOCK,
    SWITCH_CASE,
    SWITCH_LABEL,
    GUARD,
    LABELED_STMT,
    EMPTY_STMT,

    // ---- 节点：表达式 ----
    LITERAL,
    NAME_EXPR,
    THIS_EXPR,
    SUPER_EXPR,
    PAREN_EXPR,
    FIELD_ACCESS,
    METHOD_CALL,
    EXPLICIT_CONSTRUCTOR_CALL,
    ARG_LIST,
    NEW_EXPR,
    ARRAY_CREATION_EXPR,
    DIM_EXPR,
    ARRAY_INITIALIZER,
    ARRAY_ACCESS,
    UNARY_EXPR,
    POSTFIX_EXPR,
    BINARY_EXPR,
    ASSIGN_EXPR,
    CONDITIONAL_EXPR,
    INSTANCEOF_EXPR,
    CAST_EXPR,
    LAMBDA_EXPR,
    METHOD_REF,
    CLASS_LITERAL,
    SWITCH_EXPR,

    // ---- 节点：模式 ----
    TYPE_PATTERN,
    RECORD_PATTERN,
    UNNAMED_PATTERN,

    // ---- 节点：错误 ----
    ERROR,

    #[doc(hidden)]
    __LAST,
}

use SyntaxKind::*;

impl SyntaxKind {
    #[inline]
    pub fn from_u16(raw: u16) -> SyntaxKind {
        assert!(raw <= SyntaxKind::__LAST as u16);
        // SAFETY: `SyntaxKind` 是 `repr(u16)` 且判别值连续，上面已做范围检查。
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw) }
    }

    #[inline]
    pub fn is_token(self) -> bool {
        (self as u16) < (__LAST_TOKEN as u16)
    }

    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            WHITESPACE | LINE_COMMENT | BLOCK_COMMENT | DOC_COMMENT
        )
    }

    #[inline]
    pub fn is_comment(self) -> bool {
        matches!(self, LINE_COMMENT | BLOCK_COMMENT | DOC_COMMENT)
    }

    pub fn is_keyword(self) -> bool {
        (self as u16) >= (ABSTRACT_KW as u16) && (self as u16) <= (UNDERSCORE_KW as u16)
    }

    pub fn is_contextual_keyword(self) -> bool {
        (self as u16) >= (VAR_KW as u16) && (self as u16) <= (UNDERSCORE_KW as u16)
    }

    pub fn is_literal(self) -> bool {
        matches!(
            self,
            INT_LITERAL
                | LONG_LITERAL
                | FLOAT_LITERAL
                | DOUBLE_LITERAL
                | CHAR_LITERAL
                | STRING_LITERAL
                | TEXT_BLOCK
                | TRUE_KW
                | FALSE_KW
                | NULL_KW
        )
    }

    pub fn is_primitive_type_keyword(self) -> bool {
        matches!(
            self,
            BOOLEAN_KW | BYTE_KW | SHORT_KW | INT_KW | LONG_KW | CHAR_KW | FLOAT_KW | DOUBLE_KW
        )
    }

    pub fn is_modifier_keyword(self) -> bool {
        matches!(
            self,
            PUBLIC_KW
                | PROTECTED_KW
                | PRIVATE_KW
                | STATIC_KW
                | ABSTRACT_KW
                | FINAL_KW
                | NATIVE_KW
                | SYNCHRONIZED_KW
                | TRANSIENT_KW
                | VOLATILE_KW
                | STRICTFP_KW
                | DEFAULT_KW
                | SEALED_KW
                | NON_SEALED_KW
        )
    }

    /// 保留关键字（含 `true` / `false` / `null`）。
    pub fn from_keyword(text: &str) -> Option<SyntaxKind> {
        let kind = match text {
            "abstract" => ABSTRACT_KW,
            "assert" => ASSERT_KW,
            "boolean" => BOOLEAN_KW,
            "break" => BREAK_KW,
            "byte" => BYTE_KW,
            "case" => CASE_KW,
            "catch" => CATCH_KW,
            "char" => CHAR_KW,
            "class" => CLASS_KW,
            "const" => CONST_KW,
            "continue" => CONTINUE_KW,
            "default" => DEFAULT_KW,
            "do" => DO_KW,
            "double" => DOUBLE_KW,
            "else" => ELSE_KW,
            "enum" => ENUM_KW,
            "extends" => EXTENDS_KW,
            "final" => FINAL_KW,
            "finally" => FINALLY_KW,
            "float" => FLOAT_KW,
            "for" => FOR_KW,
            "goto" => GOTO_KW,
            "if" => IF_KW,
            "implements" => IMPLEMENTS_KW,
            "import" => IMPORT_KW,
            "instanceof" => INSTANCEOF_KW,
            "int" => INT_KW,
            "interface" => INTERFACE_KW,
            "long" => LONG_KW,
            "native" => NATIVE_KW,
            "new" => NEW_KW,
            "package" => PACKAGE_KW,
            "private" => PRIVATE_KW,
            "protected" => PROTECTED_KW,
            "public" => PUBLIC_KW,
            "return" => RETURN_KW,
            "short" => SHORT_KW,
            "static" => STATIC_KW,
            "strictfp" => STRICTFP_KW,
            "super" => SUPER_KW,
            "switch" => SWITCH_KW,
            "synchronized" => SYNCHRONIZED_KW,
            "this" => THIS_KW,
            "throw" => THROW_KW,
            "throws" => THROWS_KW,
            "transient" => TRANSIENT_KW,
            "try" => TRY_KW,
            "void" => VOID_KW,
            "volatile" => VOLATILE_KW,
            "while" => WHILE_KW,
            "true" => TRUE_KW,
            "false" => FALSE_KW,
            "null" => NULL_KW,
            _ => return None,
        };
        Some(kind)
    }

    /// 上下文关键字：只在特定语法位置才是关键字，其余位置是普通标识符。
    pub fn from_contextual_keyword(text: &str) -> Option<SyntaxKind> {
        let kind = match text {
            "var" => VAR_KW,
            "yield" => YIELD_KW,
            "record" => RECORD_KW,
            "sealed" => SEALED_KW,
            "permits" => PERMITS_KW,
            "when" => WHEN_KW,
            "module" => MODULE_KW,
            "open" => OPEN_KW,
            "requires" => REQUIRES_KW,
            "transitive" => TRANSITIVE_KW,
            "exports" => EXPORTS_KW,
            "opens" => OPENS_KW,
            "to" => TO_KW,
            "uses" => USES_KW,
            "provides" => PROVIDES_KW,
            "with" => WITH_KW,
            "_" => UNDERSCORE_KW,
            _ => return None,
        };
        Some(kind)
    }

    /// 用于错误信息的可读名字。
    pub fn describe(self) -> &'static str {
        match self {
            L_PAREN => "`(`",
            R_PAREN => "`)`",
            L_CURLY => "`{`",
            R_CURLY => "`}`",
            L_BRACK => "`[`",
            R_BRACK => "`]`",
            SEMICOLON => "`;`",
            COMMA => "`,`",
            DOT => "`.`",
            ELLIPSIS => "`...`",
            AT => "`@`",
            COLON_COLON => "`::`",
            COLON => "`:`",
            QUESTION => "`?`",
            ARROW => "`->`",
            EQ => "`=`",
            LT => "`<`",
            GT => "`>`",
            IDENT => "标识符",
            CLASS_KW => "`class`",
            INTERFACE_KW => "`interface`",
            WHILE_KW => "`while`",
            EOF => "文件结尾",
            _ => "token",
        }
    }
}

/// 把源码里的写法映射成 `SyntaxKind`，例如 `T![;]`、`T!['{']`、`T![class]`。
#[macro_export]
macro_rules! T {
    ['('] => { $crate::SyntaxKind::L_PAREN };
    [')'] => { $crate::SyntaxKind::R_PAREN };
    ['{'] => { $crate::SyntaxKind::L_CURLY };
    ['}'] => { $crate::SyntaxKind::R_CURLY };
    ['['] => { $crate::SyntaxKind::L_BRACK };
    [']'] => { $crate::SyntaxKind::R_BRACK };
    [;] => { $crate::SyntaxKind::SEMICOLON };
    [,] => { $crate::SyntaxKind::COMMA };
    [.] => { $crate::SyntaxKind::DOT };
    [...] => { $crate::SyntaxKind::ELLIPSIS };
    [@] => { $crate::SyntaxKind::AT };
    [::] => { $crate::SyntaxKind::COLON_COLON };
    [:] => { $crate::SyntaxKind::COLON };
    [?] => { $crate::SyntaxKind::QUESTION };
    [->] => { $crate::SyntaxKind::ARROW };
    [=] => { $crate::SyntaxKind::EQ };
    [==] => { $crate::SyntaxKind::EQ_EQ };
    [!=] => { $crate::SyntaxKind::NEQ };
    [<] => { $crate::SyntaxKind::LT };
    [>] => { $crate::SyntaxKind::GT };
    [<=] => { $crate::SyntaxKind::LT_EQ };
    [>=] => { $crate::SyntaxKind::GT_EQ };
    [!] => { $crate::SyntaxKind::BANG };
    [~] => { $crate::SyntaxKind::TILDE };
    [&&] => { $crate::SyntaxKind::AMP_AMP };
    [||] => { $crate::SyntaxKind::PIPE_PIPE };
    [+] => { $crate::SyntaxKind::PLUS };
    [-] => { $crate::SyntaxKind::MINUS };
    [*] => { $crate::SyntaxKind::STAR };
    [/] => { $crate::SyntaxKind::SLASH };
    [&] => { $crate::SyntaxKind::AMP };
    [|] => { $crate::SyntaxKind::PIPE };
    [^] => { $crate::SyntaxKind::CARET };
    [%] => { $crate::SyntaxKind::PERCENT };
    [<<] => { $crate::SyntaxKind::SHL };
    [>>] => { $crate::SyntaxKind::SHR };
    [+=] => { $crate::SyntaxKind::PLUS_EQ };
    [-=] => { $crate::SyntaxKind::MINUS_EQ };
    [*=] => { $crate::SyntaxKind::STAR_EQ };
    [/=] => { $crate::SyntaxKind::SLASH_EQ };
    [&=] => { $crate::SyntaxKind::AMP_EQ };
    [|=] => { $crate::SyntaxKind::PIPE_EQ };
    [^=] => { $crate::SyntaxKind::CARET_EQ };
    [%=] => { $crate::SyntaxKind::PERCENT_EQ };
    [<<=] => { $crate::SyntaxKind::SHL_EQ };
    [>>=] => { $crate::SyntaxKind::SHR_EQ };
    [abstract] => { $crate::SyntaxKind::ABSTRACT_KW };
    [assert] => { $crate::SyntaxKind::ASSERT_KW };
    [boolean] => { $crate::SyntaxKind::BOOLEAN_KW };
    [break] => { $crate::SyntaxKind::BREAK_KW };
    [case] => { $crate::SyntaxKind::CASE_KW };
    [catch] => { $crate::SyntaxKind::CATCH_KW };
    [class] => { $crate::SyntaxKind::CLASS_KW };
    [continue] => { $crate::SyntaxKind::CONTINUE_KW };
    [default] => { $crate::SyntaxKind::DEFAULT_KW };
    [do] => { $crate::SyntaxKind::DO_KW };
    [else] => { $crate::SyntaxKind::ELSE_KW };
    [enum] => { $crate::SyntaxKind::ENUM_KW };
    [extends] => { $crate::SyntaxKind::EXTENDS_KW };
    [final] => { $crate::SyntaxKind::FINAL_KW };
    [finally] => { $crate::SyntaxKind::FINALLY_KW };
    [for] => { $crate::SyntaxKind::FOR_KW };
    [if] => { $crate::SyntaxKind::IF_KW };
    [implements] => { $crate::SyntaxKind::IMPLEMENTS_KW };
    [import] => { $crate::SyntaxKind::IMPORT_KW };
    [instanceof] => { $crate::SyntaxKind::INSTANCEOF_KW };
    [interface] => { $crate::SyntaxKind::INTERFACE_KW };
    [new] => { $crate::SyntaxKind::NEW_KW };
    [package] => { $crate::SyntaxKind::PACKAGE_KW };
    [return] => { $crate::SyntaxKind::RETURN_KW };
    [static] => { $crate::SyntaxKind::STATIC_KW };
    [super] => { $crate::SyntaxKind::SUPER_KW };
    [switch] => { $crate::SyntaxKind::SWITCH_KW };
    [synchronized] => { $crate::SyntaxKind::SYNCHRONIZED_KW };
    [this] => { $crate::SyntaxKind::THIS_KW };
    [throw] => { $crate::SyntaxKind::THROW_KW };
    [throws] => { $crate::SyntaxKind::THROWS_KW };
    [try] => { $crate::SyntaxKind::TRY_KW };
    [void] => { $crate::SyntaxKind::VOID_KW };
    [while] => { $crate::SyntaxKind::WHILE_KW };
    [true] => { $crate::SyntaxKind::TRUE_KW };
    [false] => { $crate::SyntaxKind::FALSE_KW };
    [null] => { $crate::SyntaxKind::NULL_KW };
    [ident] => { $crate::SyntaxKind::IDENT };
}
