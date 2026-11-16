//! 只依据 class 文件把整个类（连同成员类）反编译成一份 Java 源码。
//! 用于命令行调试和测试；语言服务器里的桩代码由 `ide` 按签名级模型生成，只借用这里的方法体。

use std::collections::BTreeMap;

use classfile::{access, signature, ClassFileError};

use crate::{
    header::{self, method_header, modifiers, simple_class_name},
    names::{is_valid_identifier, NameTable},
    ClassDecompiler, ClassEnv,
};

#[derive(Debug, Clone, Default)]
pub struct DecompileStats {
    pub classes: usize,
    /// 有方法体的方法
    pub methods: usize,
    pub decompiled: usize,
    pub failures: BTreeMap<&'static str, usize>,
}

impl DecompileStats {
    pub fn merge(&mut self, other: &DecompileStats) {
        self.classes += other.classes;
        self.methods += other.methods;
        self.decompiled += other.decompiled;
        for (k, v) in &other.failures {
            *self.failures.entry(k).or_default() += v;
        }
    }
}

fn outermost(binary: &str) -> &str {
    let start = binary.rfind('/').map_or(0, |i| i + 1);
    match binary[start..].find('$') {
        Some(i) => &binary[..start + i],
        None => binary,
    }
}

/// 源码里写出来的形参名。
fn declared_names(d: &ClassDecompiler, idx: usize) -> Vec<String> {
    let m = d.method(idx);
    let n = crate::translate::method_desc(&m.descriptor)
        .map(|x| x.0.len())
        .unwrap_or(0);
    let declared = header::declared_param_count(&d.class, m);
    let skip = n.saturating_sub(declared);
    // 每个形参的槽位
    let (tys, _) = crate::translate::method_desc(&m.descriptor).unwrap_or_default();
    let mut slot: u16 = if m.is_static() { 0 } else { 1 };
    let mut slots = Vec::new();
    for t in &tys {
        slots.push(slot);
        slot += if crate::ir::kind_of(t).is_wide() {
            2
        } else {
            1
        };
    }
    (skip..n)
        .map(|i| {
            if m.parameter_names.len() == n {
                if let Some(Some(name)) = m.parameter_names.get(i) {
                    if is_valid_identifier(name) {
                        return name.clone();
                    }
                }
            }
            if let Some(code) = &m.code {
                if let Some(v) = code
                    .local_variables
                    .iter()
                    .find(|v| v.start == 0 && v.slot == slots[i])
                {
                    if is_valid_identifier(&v.name) {
                        return v.name.clone();
                    }
                }
            }
            format!("arg{}", i - skip)
        })
        .collect()
}

fn render_class(
    d: &ClassDecompiler,
    env: &dyn ClassEnv,
    names: &mut NameTable,
    indent: usize,
    out: &mut String,
    stats: &mut DecompileStats,
    depth: u32,
) {
    stats.classes += 1;
    let class = &d.class;
    let pad = "    ".repeat(indent);
    let inner_pad = "    ".repeat(indent + 1);
    let own_access = class
        .inner_classes
        .iter()
        .find(|ic| ic.inner == class.name)
        .map_or(class.access, |ic| ic.access);
    let is_enum = class.access & access::ENUM != 0;
    let is_annotation = class.access & access::ANNOTATION != 0;
    let is_interface = class.is_interface();
    let mut acc =
        own_access & (access::PUBLIC | access::PRIVATE | access::PROTECTED | access::STATIC);
    if !is_interface && !is_enum {
        acc |= own_access & (access::ABSTRACT | access::FINAL);
    }
    out.push_str(&pad);
    let mut mods = modifiers(acc, false);
    if acc & access::ABSTRACT != 0 {
        mods.push_str("abstract ");
    }
    out.push_str(&mods);
    out.push_str(if is_annotation {
        "@interface "
    } else if is_interface {
        "interface "
    } else if is_enum {
        "enum "
    } else {
        "class "
    });
    out.push_str(&simple_class_name(class));
    let sig = class
        .signature
        .as_deref()
        .and_then(signature::parse_class_sig);
    if let Some(sig) = &sig {
        out.push_str(&header::type_params(names, env, &sig.type_params));
    }
    let super_ty = sig
        .as_ref()
        .map(|s| s.super_class.clone())
        .or_else(|| class.super_name.as_deref().map(crate::ir::class_ty));
    let interfaces = sig
        .as_ref()
        .filter(|s| s.interfaces.len() == class.interfaces.len())
        .map(|s| s.interfaces.clone())
        .unwrap_or_else(|| {
            class
                .interfaces
                .iter()
                .map(|i| crate::ir::class_ty(i))
                .collect()
        });
    if !is_interface && !is_enum {
        if let Some(t) = &super_ty {
            if !matches!(t, signature::JType::Class { name, .. } if name == "java/lang/Object") {
                out.push_str(" extends ");
                out.push_str(&names.ty(t, env));
            }
        }
    }
    let interfaces: Vec<_> = interfaces
        .iter()
        .filter(|t| !(is_annotation && matches!(t, signature::JType::Class { name, .. } if name == "java/lang/annotation/Annotation")))
        .collect();
    if !interfaces.is_empty() {
        out.push_str(if is_interface {
            " extends "
        } else {
            " implements "
        });
        let list: Vec<String> = interfaces.iter().map(|t| names.ty(t, env)).collect();
        out.push_str(&list.join(", "));
    }
    out.push_str(" {\n");

    if is_enum {
        let constants: Vec<&str> = class
            .fields
            .iter()
            .filter(|f| f.access & access::ENUM != 0)
            .map(|f| f.name.as_str())
            .collect();
        out.push_str(&inner_pad);
        out.push_str(&constants.join(", "));
        out.push_str(";\n");
    }
    for f in &class.fields {
        if f.access & (access::SYNTHETIC | access::ENUM) != 0 {
            continue;
        }
        let Some(ty) = f
            .signature
            .as_deref()
            .and_then(signature::parse_field_type)
            .or_else(|| signature::parse_field_type(&f.descriptor))
        else {
            continue;
        };
        out.push_str(&inner_pad);
        out.push_str(&modifiers(f.access, false));
        out.push_str(&names.ty(&ty, env));
        out.push(' ');
        out.push_str(&f.name);
        out.push_str(";\n");
    }
    if !is_interface && !is_enum {
        stats.methods += usize::from(
            class
                .methods
                .iter()
                .any(|m| m.name == "<clinit>" && m.code.is_some()),
        );
        match d.static_init(names, env, indent + 1) {
            Some(Ok(text)) => {
                stats.decompiled += 1;
                out.push('\n');
                out.push_str(&inner_pad);
                out.push_str("static ");
                out.push_str(&text);
                out.push('\n');
            }
            Some(Err(e)) => *stats.failures.entry(e.0).or_default() += 1,
            None => {
                if class
                    .methods
                    .iter()
                    .any(|m| m.name == "<clinit>" && m.code.is_some())
                {
                    stats.decompiled += 1;
                }
            }
        }
    }
    for idx in d.source_methods() {
        let m = d.method(idx);
        if is_enum
            && ((m.name == "values" && m.descriptor.starts_with("()"))
                || (m.name == "valueOf" && m.descriptor.starts_with("(Ljava/lang/String;)")))
        {
            continue;
        }
        let param_names = declared_names(d, idx);
        out.push('\n');
        out.push_str(&inner_pad);
        out.push_str(&method_header(names, env, class, m, &param_names));
        if m.code.is_none() {
            out.push_str(";\n");
            continue;
        }
        stats.methods += 1;
        match d.method_body(idx, Some(&param_names), names, env, indent + 1) {
            Ok(body) => {
                stats.decompiled += 1;
                out.push(' ');
                out.push_str(&body);
                out.push('\n');
            }
            Err(e) => {
                *stats.failures.entry(e.0).or_default() += 1;
                out.push_str(&format!(" {{ /* 无法反编译：{} */ }}\n", e.0));
            }
        }
    }
    if depth < 8 {
        for ic in &class.inner_classes {
            if ic.outer.as_deref() != Some(class.name.as_str()) || ic.inner_name.is_none() {
                continue;
            }
            let Some(bytes) = env.class_bytes(&ic.inner) else {
                continue;
            };
            let Ok(nested) = ClassDecompiler::new(&bytes) else {
                continue;
            };
            out.push('\n');
            render_class(&nested, env, names, indent + 1, out, stats, depth + 1);
        }
    }
    out.push_str(&pad);
    out.push_str("}\n");
}

/// 反编译整个类（`bytes` 是最外层类的 class 文件），输出一份完整的源码。
pub fn decompile_class_text(
    bytes: &[u8],
    env: &dyn ClassEnv,
    stats: &mut DecompileStats,
) -> Result<String, ClassFileError> {
    let d = ClassDecompiler::new(bytes)?;
    let package = classfile::package_of(&d.class.name).to_owned();
    let mut names = NameTable::new(&package);
    names.set_top(outermost(&d.class.name));
    for ic in &d.class.inner_classes {
        if let Some(n) = &ic.inner_name {
            if ic.outer.is_some() {
                names.reserve(n);
            }
        }
    }
    let mut body = String::new();
    render_class(&d, env, &mut names, 0, &mut body, stats, 0);
    let mut out = String::new();
    if !package.is_empty() {
        out.push_str(&format!("package {};\n\n", package.replace('/', ".")));
    }
    let imports: Vec<&String> = names.imports().collect();
    for imp in &imports {
        out.push_str(&format!("import {imp};\n"));
    }
    if !imports.is_empty() {
        out.push('\n');
    }
    out.push_str(&body);
    Ok(out)
}
