//! 类名的渲染：能用简单名的用简单名并记下需要的 import，冲突时退回全限定名。

use std::collections::BTreeSet;

use classfile::signature::{BaseType, JType, TypeArg};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::ClassEnv;

pub struct NameTable {
    /// 当前文件的包（斜杠形式）
    package: String,
    /// 简单名 → 它代表的最外层类（二进制名）
    bound: FxHashMap<String, String>,
    /// 不能再绑定的简单名（当前类的成员类型等）
    reserved: FxHashSet<String>,
    /// 当前文件的顶层类：它的成员类在整个文件里都能省掉外层类名
    top: Option<String>,
    imports: BTreeSet<String>,
}

pub fn base_name(b: BaseType) -> &'static str {
    match b {
        BaseType::Boolean => "boolean",
        BaseType::Byte => "byte",
        BaseType::Short => "short",
        BaseType::Char => "char",
        BaseType::Int => "int",
        BaseType::Long => "long",
        BaseType::Float => "float",
        BaseType::Double => "double",
    }
}

fn package_of(binary: &str) -> &str {
    binary.rfind('/').map_or("", |i| &binary[..i])
}

fn simple_of(binary: &str) -> &str {
    binary.rfind('/').map_or(binary, |i| &binary[i + 1..])
}

impl NameTable {
    pub fn new(package: &str) -> NameTable {
        NameTable {
            package: package.to_owned(),
            bound: FxHashMap::default(),
            reserved: FxHashSet::default(),
            top: None,
            imports: BTreeSet::new(),
        }
    }

    /// 已经由别处（签名部分）import 或声明的类。
    pub fn bind(&mut self, binary_outermost: &str) {
        let simple = simple_of(binary_outermost).to_owned();
        self.bound
            .entry(simple)
            .or_insert_with(|| binary_outermost.to_owned());
    }

    /// 当前文件的顶层类（也会绑定它的简单名）。
    pub fn set_top(&mut self, binary_outermost: &str) {
        self.bind(binary_outermost);
        self.top = Some(binary_outermost.to_owned());
    }

    /// 这个简单名在当前文件里另有所指（例如当前类的成员类型）。
    pub fn reserve(&mut self, simple: &str) {
        self.reserved.insert(simple.to_owned());
    }

    /// 需要新增的 import（点号形式）。
    pub fn imports(&self) -> impl Iterator<Item = &String> {
        self.imports.iter()
    }

    /// 把二进制名拆成最外层类和嵌套部分：`java/util/Map$Entry` → (`java/util/Map`, [`Entry`])。
    fn split(binary: &str, env: &dyn ClassEnv) -> (String, Vec<String>) {
        let simple_start = binary.rfind('/').map_or(0, |i| i + 1);
        let tail = &binary[simple_start..];
        if !tail.contains('$') {
            return (binary.to_owned(), Vec::new());
        }
        let parts: Vec<&str> = tail.split('$').collect();
        for i in 1..parts.len() {
            // 嵌套部分以数字开头（匿名类、局部类）或为空时不能用 `.` 引用
            if parts[i..]
                .iter()
                .any(|p| p.is_empty() || p.starts_with(|c: char| c.is_ascii_digit()))
            {
                break;
            }
            let outer = format!("{}{}", &binary[..simple_start], parts[..i].join("$"));
            if env.class_exists(&outer) {
                return (outer, parts[i..].iter().map(|s| (*s).to_owned()).collect());
            }
        }
        (binary.to_owned(), Vec::new())
    }

    /// 类在源码里的写法。
    pub fn class_name(&mut self, binary: &str, env: &dyn ClassEnv) -> String {
        let (outer, nested) = Self::split(binary, env);
        if !nested.is_empty() && self.top.as_deref() == Some(outer.as_str()) {
            return nested.join(".");
        }
        let simple = simple_of(&outer).to_owned();
        let pkg = package_of(&outer).to_owned();
        let head = match self.bound.get(&simple) {
            Some(b) if *b == outer => simple.clone(),
            Some(_) => outer.replace('/', "."),
            None if self.reserved.contains(&simple) => outer.replace('/', "."),
            None => {
                let conflict = (pkg != self.package
                    && !self.package.is_empty()
                    && env.class_exists(&format!("{}/{}", self.package, simple)))
                    || (pkg != "java/lang" && env.class_exists(&format!("java/lang/{simple}")));
                if conflict {
                    outer.replace('/', ".")
                } else {
                    self.bound.insert(simple.clone(), outer.clone());
                    if pkg != self.package && pkg != "java/lang" && !pkg.is_empty() {
                        self.imports.insert(outer.replace('/', "."));
                    }
                    simple.clone()
                }
            }
        };
        let mut out = head;
        for n in nested {
            out.push('.');
            out.push_str(&n);
        }
        out
    }

    pub fn ty(&mut self, ty: &JType, env: &dyn ClassEnv) -> String {
        match ty {
            JType::Base(b) => base_name(*b).to_owned(),
            JType::TypeVar(n) => n.clone(),
            JType::Array(e) => format!("{}[]", self.ty(e, env)),
            JType::Class { name, args } => {
                let mut out = self.class_name(name, env);
                if !args.is_empty() {
                    out.push('<');
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        match a {
                            TypeArg::Unbounded => out.push('?'),
                            TypeArg::Extends(t) => {
                                out.push_str("? extends ");
                                out.push_str(&self.ty(t, env));
                            }
                            TypeArg::Super(t) => {
                                out.push_str("? super ");
                                out.push_str(&self.ty(t, env));
                            }
                            TypeArg::Exact(t) => out.push_str(&self.ty(t, env)),
                        }
                    }
                    out.push('>');
                }
                out
            }
        }
    }
}

pub const KEYWORDS: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "void",
    "volatile",
    "while",
    "true",
    "false",
    "null",
    "var",
    "yield",
    "record",
    "_",
];

/// 是否能用作字段名或方法名。`var`、`record` 这些上下文关键字可以。
pub fn is_valid_member_name(name: &str) -> bool {
    matches!(name, "var" | "yield" | "record") || is_valid_identifier(name)
}

/// 是否能直接用作局部变量名。
pub fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_' || first == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        && !KEYWORDS.contains(&name)
}
