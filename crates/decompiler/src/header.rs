//! 声明头的渲染（修饰符、方法签名），供匿名类体和独立的整类反编译使用。

use classfile::{
    access,
    code::{CodeClass, CodeMethod},
    signature::{self, JType, TypeParamSig},
};

use crate::{names::NameTable, types, ClassEnv};

pub fn modifiers(acc: u16, method: bool) -> String {
    let mut s = String::new();
    let table: &[(u16, &str)] = if method {
        &[
            (access::PUBLIC, "public "),
            (access::PRIVATE, "private "),
            (access::PROTECTED, "protected "),
            (access::ABSTRACT, "abstract "),
            (access::STATIC, "static "),
            (access::FINAL, "final "),
            (access::SYNCHRONIZED, "synchronized "),
            (access::NATIVE, "native "),
        ]
    } else {
        &[
            (access::PUBLIC, "public "),
            (access::PRIVATE, "private "),
            (access::PROTECTED, "protected "),
            (access::STATIC, "static "),
            (access::FINAL, "final "),
            (access::TRANSIENT, "transient "),
            (access::VOLATILE, "volatile "),
        ]
    };
    for (flag, text) in table {
        if acc & flag != 0 {
            s.push_str(text);
        }
    }
    s
}

pub fn type_params(names: &mut NameTable, env: &dyn ClassEnv, params: &[TypeParamSig]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let mut out = String::from("<");
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&p.name);
        let bounds: Vec<&JType> = p
            .bounds
            .iter()
            .filter(|b| !matches!(b, JType::Class { name, args } if name == "java/lang/Object" && args.is_empty()))
            .collect();
        for (j, b) in bounds.iter().enumerate() {
            out.push_str(if j == 0 { " extends " } else { " & " });
            out.push_str(&names.ty(b, env));
        }
    }
    out.push('>');
    out
}

/// 源码里写出来的形参个数（去掉内部类构造器的外层实例、枚举构造器的名字和序号）。
pub fn declared_param_count(class: &CodeClass, m: &CodeMethod) -> usize {
    let desc = types::method_types(m);
    let n = crate::translate::method_desc(&m.descriptor)
        .map(|d| d.0.len())
        .unwrap_or(0);
    if m.name != "<init>" {
        return n;
    }
    if let Some(sig) = m.signature.as_deref().and_then(signature::parse_method_sig) {
        return sig.params.len().min(n);
    }
    let _ = desc;
    if class.access & access::ENUM != 0 {
        return n.saturating_sub(2);
    }
    let inner = class
        .inner_classes
        .iter()
        .any(|ic| ic.inner == class.name && ic.access & access::STATIC == 0 && ic.outer.is_some());
    if inner {
        n.saturating_sub(1)
    } else {
        n
    }
}

pub fn simple_class_name(class: &CodeClass) -> String {
    class
        .inner_classes
        .iter()
        .find(|ic| ic.inner == class.name)
        .and_then(|ic| ic.inner_name.clone())
        .unwrap_or_else(|| classfile::simple_name(&class.name).to_owned())
}

/// `public <T> List<T> name(T a, int... b) throws IOException`（不含方法体）。
/// `param_names` 是源码里写出来的那些形参的名字（对齐到末尾）。
pub fn method_header(
    names: &mut NameTable,
    env: &dyn ClassEnv,
    class: &CodeClass,
    m: &CodeMethod,
    param_names: &[String],
) -> String {
    let mut out = String::new();
    let mut acc = m.access;
    if class.is_interface() {
        acc &= !(access::PUBLIC | access::ABSTRACT);
        if m.access & (access::ABSTRACT | access::STATIC | access::PRIVATE) == 0 {
            out.push_str("default ");
        }
    }
    out.insert_str(0, &modifiers(acc, true));
    let sig = m.signature.as_deref().and_then(signature::parse_method_sig);
    if let Some(sig) = &sig {
        let tp = type_params(names, env, &sig.type_params);
        if !tp.is_empty() {
            out.push_str(&tp);
            out.push(' ');
        }
    }
    let (params, ret) = types::method_types(m);
    let ctor = m.name == "<init>";
    if ctor {
        out.push_str(&simple_class_name(class));
    } else {
        match &ret {
            Some(t) => out.push_str(&names.ty(t, env)),
            None => out.push_str("void"),
        }
        out.push(' ');
        out.push_str(&m.name);
    }
    out.push('(');
    let declared = declared_param_count(class, m);
    let skip = params.len().saturating_sub(declared);
    let shown = &params[skip..];
    let varargs = m.access & access::VARARGS != 0;
    for (i, ty) in shown.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let last = i + 1 == shown.len();
        match (ty, varargs && last) {
            (JType::Array(e), true) => {
                out.push_str(&names.ty(e, env));
                out.push_str("...");
            }
            _ => out.push_str(&names.ty(ty, env)),
        }
        out.push(' ');
        let k = (param_names.len() + i).checked_sub(shown.len());
        match k.and_then(|k| param_names.get(k)) {
            Some(n) => out.push_str(n),
            None => out.push_str(&format!("arg{i}")),
        }
    }
    out.push(')');
    let throws: Vec<JType> = match &sig {
        Some(s) if !s.throws.is_empty() => s.throws.clone(),
        _ => m
            .exceptions
            .iter()
            .map(|e| crate::ir::class_ty(e))
            .collect(),
    };
    if !throws.is_empty() {
        out.push_str(" throws ");
        let list: Vec<String> = throws.iter().map(|t| names.ty(t, env)).collect();
        out.push_str(&list.join(", "));
    }
    out
}
