//! `completions` 请求：求值输入框、监视表达式和调试控制台里的补全。
//!
//! 补全光标前正在输入的名字（前缀）：
//! - 前面没有 `.`：当前栈帧可见的局部变量和参数、`this`（和外层实例）的字段和方法、当前类和
//!   外层类的静态成员、静态 import 的成员、能用简单名引用的类型（成员类、import、同一个包、
//!   `java.lang`）和关键字；
//! - `值.`：值的运行时类型的字段和方法（含继承的），数组是 `length`；
//! - `类型.`：静态字段、静态方法和成员类；
//! - `包.`：子包和类型。
//!
//! 接收者按 `evaluate` 的规则求值（名字解析完全相同，补全出来的表达式可以原样求值），但和悬停
//! 一样只读：不调用方法、不新建对象、不加载类、不恢复线程。只有调用方法才算得出来的接收者
//! （`a.b().`、`new T().`、强制转换）用声明的类型：方法的返回类型、字段的类型，都是擦除后的。
//! 类型和包只列目标 JVM 里已经加载的类（`AllClasses`，每次停下最多取一次）。

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    rc::Rc,
};

use anyhow::Context;
use serde_json::{json, Value as Json};
use syntax::ast;

use crate::{
    eval,
    evaluator::{enclosing_classes, loaded_class, outer_instances, Cx, Val, OBJECT, STRING},
    jdwp::{self, signature_to_type_name, MethodInfo, Value, Vm},
    session::Session,
    variables::find_field,
};

const ACC_PUBLIC: i32 = 0x0001;
const ACC_PRIVATE: i32 = 0x0002;
const ACC_PROTECTED: i32 = 0x0004;
const ACC_SYNTHETIC: i32 = 0x1000;

const KEYWORDS: &[&str] = &[
    "this",
    "super",
    "new",
    "true",
    "false",
    "null",
    "instanceof",
];

/// 排序分组：同样的匹配程度下，局部变量在前，关键字在最后。
const GROUP_LOCAL: u8 = 0;
const GROUP_FIELD: u8 = 1;
const GROUP_METHOD: u8 = 2;
const GROUP_TYPE: u8 = 3;
const GROUP_PACKAGE: u8 = 4;
const GROUP_KEYWORD: u8 = 5;

/// 补全的位置：接收者表达式（`a.b.c` 里的 `a.b`；没有 `.` 时为 `None`）和已经输入的前缀。
#[derive(Debug, PartialEq)]
struct Site<'a> {
    receiver: Option<&'a str>,
    prefix: &'a str,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// 光标之前的文本。`line` 从 0 开始，`column` 是这一行里从 0 开始的 UTF-16 位置；超出范围时取到
/// 行尾 / 文本末尾。
fn text_before_cursor(text: &str, line: usize, column: usize) -> &str {
    let mut offset = 0;
    for (i, current) in text.split('\n').enumerate() {
        if i == line {
            let mut units = 0;
            for (pos, c) in current.char_indices() {
                if units >= column {
                    return &text[..offset + pos];
                }
                units += c.len_utf16();
            }
            return &text[..offset + current.len()];
        }
        offset += current.len() + 1;
    }
    text
}

/// 文本是否停在字符串或字符字面量里面。
fn inside_literal(text: &str) -> bool {
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(_), '\\') => {
                chars.next();
            }
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            _ => {}
        }
    }
    quote.is_some()
}

fn completion_site(before: &str) -> Option<Site<'_>> {
    if inside_literal(before) {
        return None;
    }
    let prefix_start = before
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_ident(c))
        .last()
        .map_or(before.len(), |(i, _)| i);
    let prefix = &before[prefix_start..];
    // 数字字面量（`1.5`）不补全
    if prefix.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let Some(head) = before[..prefix_start].trim_end().strip_suffix('.') else {
        return Some(Site {
            receiver: None,
            prefix,
        });
    };
    let head = head.trim_end();
    let receiver = head[receiver_start(head)?..].trim();
    if receiver.is_empty() || receiver.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(Site {
        receiver: Some(receiver),
        prefix,
    })
}

/// `head` 末尾那个接收者表达式的起点：从后往前跨过名字、`.`、成对的括号和字面量，
/// 遇到运算符、逗号、没配对的左括号等就停；`new T(...)` 把 `new` 也算进来。
fn receiver_start(head: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = head.char_indices().collect();
    let mut depth = 0usize;
    let mut start = head.len();
    let mut i = chars.len();
    while i > 0 {
        let (pos, c) = chars[i - 1];
        match c {
            ')' | ']' => depth += 1,
            '(' | '[' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            '"' | '\'' => {
                // 往回跳过整个字面量
                let mut j = i - 1;
                loop {
                    if j == 0 {
                        return None;
                    }
                    j -= 1;
                    if chars[j].1 == c && (j == 0 || chars[j - 1].1 != '\\') {
                        break;
                    }
                }
                start = chars[j].0;
                i = j;
                continue;
            }
            _ if depth > 0 => {}
            c if is_ident(c) || c == '.' => {}
            _ => break,
        }
        start = pos;
        i -= 1;
    }
    if depth > 0 {
        return None;
    }
    let before = &head[..start];
    let trimmed = before.trim_end();
    if trimmed.len() < before.len() {
        if let Some(rest) = trimmed.strip_suffix("new") {
            if !rest.ends_with(is_ident) {
                start = rest.len();
            }
        }
    }
    Some(start)
}

/// 名字和前缀的匹配程度：0 区分大小写的前缀，1 不区分大小写的前缀，2 首字符相同、按顺序
/// 包含前缀的每个字符；不匹配时为 `None`。
fn match_rank(label: &str, prefix: &str) -> Option<u8> {
    if label.starts_with(prefix) {
        return Some(0);
    }
    let label = label.to_lowercase();
    let prefix = prefix.to_lowercase();
    if label.starts_with(&prefix) {
        return Some(1);
    }
    let mut rest = label.chars();
    let mut wanted = prefix.chars();
    if rest.next() != wanted.next() {
        return None;
    }
    wanted.all(|c| rest.any(|x| x == c)).then_some(2)
}

/// 类型签名的简短写法：`Ljava/util/Map$Entry;` → `Map.Entry`，`[I` → `int[]`。
fn short_type(signature: &str) -> String {
    let name = signature_to_type_name(signature);
    jdwp::simple_name(&name).replace('$', ".")
}

fn method_detail(method: &MethodInfo) -> String {
    match eval::split_method_signature(&method.signature) {
        Some((params, ret)) => {
            let params: Vec<String> = params.iter().map(|p| short_type(p)).collect();
            let mut params = params.join(", ");
            if method.is_varargs() {
                if let Some(head) = params.strip_suffix("[]") {
                    params = format!("{head}...");
                }
            }
            format!("{} {}({params})", short_type(&ret), method.name)
        }
        None => method.name.clone(),
    }
}

fn package_of(name: &str) -> &str {
    name.rfind('.').map_or("", |i| &name[..i])
}

/// 最外层的类：`demo.Outer$Inner` → `demo.Outer`。
fn outermost(name: &str) -> &str {
    let start = name.rfind('.').map_or(0, |i| i + 1);
    match name[start..].find('$') {
        Some(i) => &name[..start + i],
        None => name,
    }
}

/// 收集补全项：按前缀过滤，同名的只留第一个（先收集的在名字解析里优先）。
struct Targets<'a> {
    prefix: &'a str,
    seen: HashSet<String>,
    items: Vec<Json>,
}

impl<'a> Targets<'a> {
    fn new(prefix: &'a str) -> Targets<'a> {
        Targets {
            prefix,
            seen: HashSet::new(),
            items: Vec::new(),
        }
    }

    /// 方法和字段可以同名，分开去重。
    fn key(label: &str, kind: &str) -> String {
        if kind == "method" {
            format!("{label}()")
        } else {
            label.to_owned()
        }
    }

    fn wants(&self, label: &str, kind: &str) -> bool {
        !self.seen.contains(&Self::key(label, kind)) && match_rank(label, self.prefix).is_some()
    }

    fn push(
        &mut self,
        label: &str,
        kind: &str,
        group: u8,
        detail: Option<String>,
        text: Option<String>,
    ) {
        let Some(rank) = match_rank(label, self.prefix) else {
            return;
        };
        if !self.seen.insert(Self::key(label, kind)) {
            return;
        }
        let mut item = json!({
            "label": label,
            "type": kind,
            "sortText": format!("{rank}{group}{label}"),
        });
        if let Some(detail) = detail {
            item["detail"] = json!(detail);
        }
        if let Some(text) = text {
            item["text"] = json!(text);
        }
        self.items.push(item);
    }

    fn finish(mut self) -> Vec<Json> {
        self.items.sort_by(|a, b| {
            a["sortText"]
                .as_str()
                .unwrap_or_default()
                .cmp(b["sortText"].as_str().unwrap_or_default())
        });
        self.items
    }
}

/// 从当前栈帧所在的类看，成员能不能访问（`private` 按最外层类，`protected` 按父类近似）。
struct Access {
    package: String,
    outermost: String,
    supers: HashSet<u64>,
}

impl Access {
    fn new(vm: &Vm, class: u64) -> anyhow::Result<Access> {
        let name = vm.class_name(class)?;
        let mut supers = HashSet::new();
        for enclosing in enclosing_classes(vm, class) {
            supers.extend(vm.class_chain(enclosing)?);
        }
        Ok(Access {
            package: package_of(&name).to_owned(),
            outermost: outermost(&name).to_owned(),
            supers,
        })
    }

    fn allows(&self, vm: &Vm, declaring: u64, modifiers: i32) -> anyhow::Result<bool> {
        if modifiers & ACC_PUBLIC != 0 {
            return Ok(true);
        }
        let name = vm.class_name(declaring)?;
        if modifiers & ACC_PRIVATE != 0 {
            return Ok(outermost(&name) == self.outermost);
        }
        Ok(package_of(&name) == self.package
            || (modifiers & ACC_PROTECTED != 0 && self.supers.contains(&declaring)))
    }

    /// 能用名字引用的类型：`public` 的，或者在同一个包里。
    fn type_visible(&self, vm: &Vm, class: u64, name: &str) -> bool {
        package_of(name) == self.package
            || vm
                .class_modifiers(class)
                .is_ok_and(|modifiers| modifiers & ACC_PUBLIC != 0)
    }
}

/// `.` 前面的东西。
enum Receiver {
    /// 值：按这个类型（运行时类型，算不出来时是声明的类型）列成员
    Instance(u64),
    /// 类型名：列静态成员和成员类
    Static(u64),
    /// 包名
    Package(String),
}

/// 类型和它所有的父类、接口（自身在前，然后是父类链，最后是接口）。
fn supertypes(vm: &Vm, class: u64) -> anyhow::Result<Vec<u64>> {
    let mut types = vm.class_chain(class)?;
    let mut seen: HashSet<u64> = types.iter().copied().collect();
    let mut i = 0;
    while i < types.len() {
        for &interface in vm.interfaces(types[i])?.iter() {
            if seen.insert(interface) {
                types.push(interface);
            }
        }
        i += 1;
    }
    Ok(types)
}

impl Session {
    pub(crate) fn completions(&mut self, args: &Json) -> anyhow::Result<Json> {
        // 程序没有停下、栈帧已经失效、目标 JVM 已经断开：没有可补全的，不算错误
        let targets = self.completion_targets(args).unwrap_or_else(|err| {
            tracing::debug!("补全失败: {err:#}");
            Vec::new()
        });
        Ok(json!({ "targets": targets }))
    }

    fn completion_targets(&mut self, args: &Json) -> anyhow::Result<Vec<Json>> {
        let text = args
            .get("text")
            .and_then(Json::as_str)
            .context("缺少 text")?;
        if !self.is_stopped() {
            return Ok(Vec::new());
        }
        let vm = self.vm()?;
        let frame = match args.get("frameId").and_then(Json::as_i64) {
            Some(_) => self.frame_arg(args)?,
            None => match self.last_frame(&vm) {
                Some(frame) => frame,
                None => return Ok(Vec::new()),
            },
        };
        let line = args
            .get("line")
            .and_then(Json::as_i64)
            .map_or(0, |line| (self.line_from_client(line) - 1).max(0) as usize);
        let column = match args.get("column").and_then(Json::as_i64) {
            Some(column) if self.columns_start_at1 => (column - 1).max(0) as usize,
            Some(column) => column.max(0) as usize,
            None => usize::MAX,
        };
        let before = text_before_cursor(text, line, column);
        let Some(site) = completion_site(before) else {
            return Ok(Vec::new());
        };
        let mut cx = Cx::new(vm, frame, false);
        let mut targets = Targets::new(site.prefix);
        match site.receiver {
            None => self.complete_names(&mut cx, &mut targets)?,
            Some(receiver) => self.complete_members(&mut cx, receiver, &mut targets)?,
        }
        Ok(targets.finish())
    }

    // ---------------------------------------------------------------
    // 没有接收者
    // ---------------------------------------------------------------

    /// 按 `evaluate` 解析名字的顺序收集：局部变量、`this` 和外层实例的成员、当前类和外层类的
    /// 静态成员、静态 import、类型、关键字。
    fn complete_names(&mut self, cx: &mut Cx, out: &mut Targets) -> anyhow::Result<()> {
        let vm = cx.vm.clone();
        let frame = cx.frame;
        let access = Access::new(&vm, frame.location.class)?;
        if let Some(vars) = self.visible_locals(&vm, &frame)? {
            // 后声明的同名变量遮住先声明的
            for var in vars.iter().rev() {
                out.push(
                    &var.name,
                    "variable",
                    GROUP_LOCAL,
                    Some(short_type(&var.signature)),
                    None,
                );
            }
        }
        let this = vm.this_object(frame.thread, frame.frame)?.object_id();
        if let Some(this) = this {
            let mut objects = vec![this];
            objects.extend(outer_instances(&vm, this));
            for object in objects {
                let class = vm.object_type(object)?;
                add_fields(&vm, &access, class, false, None, out)?;
                add_methods(&vm, &access, class, false, None, out)?;
            }
        }
        for class in enclosing_classes(&vm, frame.location.class) {
            add_fields(&vm, &access, class, true, None, out)?;
            add_methods(&vm, &access, class, true, None, out)?;
        }
        let imports = self.imports(cx);
        for (ty, member) in &imports.static_single {
            if !out.wants(member, "field") && !out.wants(member, "method") {
                continue;
            }
            if let Some((class, _)) = self.class_by_name(cx, ty)? {
                add_fields(&vm, &access, class, true, Some(member), out)?;
                add_methods(&vm, &access, class, true, Some(member), out)?;
            }
        }
        for ty in &imports.static_on_demand {
            if let Some((class, _)) = self.class_by_name(cx, ty)? {
                add_fields(&vm, &access, class, true, None, out)?;
                add_methods(&vm, &access, class, true, None, out)?;
            }
        }

        // 类型：和 `resolve_simple_type` 的顺序一致
        let names = self.loaded_names(&vm)?;
        for class in enclosing_classes(&vm, frame.location.class) {
            let outer = vm.class_name(class)?;
            add_nested_types(&vm, &names, &outer, out);
        }
        for single in &imports.single {
            let simple = jdwp::simple_name(single);
            if out.wants(simple, "class") {
                out.push(simple, "class", GROUP_TYPE, Some(single.clone()), None);
            }
        }
        add_package_types(&vm, &access, &names, &access.package, out);
        for package in &imports.on_demand {
            add_package_types(&vm, &access, &names, package, out);
        }
        add_package_types(&vm, &access, &names, "java.lang", out);

        for keyword in KEYWORDS {
            if this.is_none() && matches!(*keyword, "this" | "super") {
                continue;
            }
            out.push(keyword, "keyword", GROUP_KEYWORD, None, None);
        }
        Ok(())
    }

    // ---------------------------------------------------------------
    // `接收者.`
    // ---------------------------------------------------------------

    fn complete_members(
        &mut self,
        cx: &mut Cx,
        receiver: &str,
        out: &mut Targets,
    ) -> anyhow::Result<()> {
        let expr = eval::parse(receiver).map_err(anyhow::Error::msg)?;
        let vm = cx.vm.clone();
        let access = Access::new(&vm, cx.frame.location.class)?;
        match self.completion_receiver(cx, &expr)? {
            Some(Receiver::Instance(class)) => {
                if vm.class_signature(class)?.starts_with('[') {
                    out.push("length", "field", GROUP_FIELD, Some("int".to_owned()), None);
                    if let Some(object) = loaded_class(&vm, OBJECT)? {
                        add_methods(&vm, &access, object, false, None, out)?;
                    }
                } else {
                    add_fields(&vm, &access, class, false, None, out)?;
                    add_methods(&vm, &access, class, false, None, out)?;
                }
            }
            Some(Receiver::Static(class)) => {
                add_fields(&vm, &access, class, true, None, out)?;
                add_methods(&vm, &access, class, true, None, out)?;
                let names = self.loaded_names(&vm)?;
                add_nested_types(&vm, &names, &vm.class_name(class)?, out);
                out.push("class", "keyword", GROUP_KEYWORD, None, None);
            }
            Some(Receiver::Package(package)) => {
                let names = self.loaded_names(&vm)?;
                add_package_types(&vm, &access, &names, &package, out);
                let prefix = format!("{package}.");
                let subpackages: BTreeSet<&str> = names_with_prefix(&names, &prefix)
                    .filter_map(|(name, _)| name[prefix.len()..].split_once('.'))
                    .map(|(sub, _)| sub)
                    .collect();
                for sub in subpackages {
                    out.push(
                        sub,
                        "module",
                        GROUP_PACKAGE,
                        Some(format!("{package}.{sub}")),
                        None,
                    );
                }
            }
            None => {}
        }
        Ok(())
    }

    /// 接收者是什么：先按 `evaluate` 的规则只读地求值，要调用方法才算得出来时用声明的类型。
    fn completion_receiver(
        &mut self,
        cx: &mut Cx,
        expr: &ast::Expr,
    ) -> anyhow::Result<Option<Receiver>> {
        let vm = cx.vm.clone();
        if let Ok(val) = self.eval(cx, expr) {
            return Ok(match val {
                Val::Jdwp(Value::Object { id: 0, .. }, Some(signature)) => {
                    declared_receiver(&vm, &signature)?
                }
                Val::Jdwp(Value::Object { id, .. }, _) if id != 0 => {
                    Some(Receiver::Instance(vm.object_type(id)?))
                }
                Val::Jdwp(..) => None,
                Val::Str(_) => loaded_class(&vm, STRING)?.map(Receiver::Instance),
                Val::Type { class, .. } => Some(Receiver::Static(class)),
                Val::Package(name) => Some(Receiver::Package(name)),
            });
        }
        match self.declared_type(cx, expr)? {
            Some(signature) => declared_receiver(&vm, &signature),
            None => Ok(None),
        }
    }

    /// 表达式的类型签名：求得出值时是运行时类型，否则是声明的类型。
    fn expr_signature(&mut self, cx: &mut Cx, expr: &ast::Expr) -> anyhow::Result<Option<String>> {
        let vm = cx.vm.clone();
        match self.eval(cx, expr) {
            Ok(Val::Jdwp(Value::Object { id, .. }, _)) if id != 0 => {
                Ok(Some(vm.class_signature(vm.object_type(id)?)?))
            }
            Ok(Val::Jdwp(_, signature)) => Ok(signature),
            Ok(Val::Str(_)) => Ok(Some(STRING.to_owned())),
            Ok(_) => Ok(None),
            Err(_) => self.declared_type(cx, expr),
        }
    }

    /// 不求值、按声明推出来的类型签名（擦除后的）：方法的返回类型、字段的类型、`new` 和强制
    /// 转换的类型、数组元素的类型。
    fn declared_type(&mut self, cx: &mut Cx, expr: &ast::Expr) -> anyhow::Result<Option<String>> {
        use ast::Expr as E;
        let vm = cx.vm.clone();
        Ok(match expr {
            E::ParenExpr(paren) => match paren.expr() {
                Some(inner) => self.expr_signature(cx, &inner)?,
                None => None,
            },
            E::CastExpr(cast) => match cast.ty() {
                Some(ty) => self.resolve_type(cx, &ty).ok(),
                None => None,
            },
            E::NewExpr(new) => match new.ty() {
                Some(ty) => self.resolve_type(cx, &ty).ok(),
                None => None,
            },
            E::ArrayAccess(access) => match access.array() {
                Some(array) => self
                    .expr_signature(cx, &array)?
                    .and_then(|s| s.strip_prefix('[').map(str::to_owned)),
                None => None,
            },
            E::FieldAccess(access) => {
                let (Some(name), Some(receiver)) = (access.name_ref(), access.receiver()) else {
                    return Ok(None);
                };
                let name = name.text().to_string();
                match self.completion_receiver(cx, &receiver)? {
                    Some(Receiver::Instance(class) | Receiver::Static(class)) => {
                        if name == "length" && vm.class_signature(class)?.starts_with('[') {
                            Some("I".to_owned())
                        } else {
                            find_field(&vm, class, &name)?.map(|(_, field)| field.signature)
                        }
                    }
                    _ => None,
                }
            }
            E::MethodCall(call) => self.call_return_type(cx, call)?,
            _ => None,
        })
    }

    /// 方法调用的返回类型：按接收者的类型找同名、实参个数对得上的方法（和 `evaluate` 找方法的
    /// 范围相同），取第一个的返回类型。
    fn call_return_type(
        &mut self,
        cx: &mut Cx,
        call: &ast::MethodCall,
    ) -> anyhow::Result<Option<String>> {
        let vm = cx.vm.clone();
        let Some(name) = call.name_ref().map(|n| n.text().to_string()) else {
            return Ok(None);
        };
        let arity = call.arg_list().map_or(0, |list| list.args().count());
        // (类型, 是否只看静态方法)
        let mut scopes: Vec<(u64, bool)> = Vec::new();
        match call.receiver() {
            None => {
                let this = vm.this_object(cx.frame.thread, cx.frame.frame)?;
                if let Some(this) = this.object_id() {
                    scopes.push((vm.object_type(this)?, false));
                    for outer in outer_instances(&vm, this) {
                        scopes.push((vm.object_type(outer)?, false));
                    }
                }
                for class in enclosing_classes(&vm, cx.frame.location.class) {
                    scopes.push((class, true));
                }
            }
            Some(ast::Expr::SuperExpr(_)) => {
                if let Some(superclass) = vm.superclass(cx.frame.location.class)? {
                    scopes.push((superclass, false));
                }
            }
            Some(receiver) => match self.completion_receiver(cx, &receiver)? {
                Some(Receiver::Instance(class)) => scopes.push((class, false)),
                Some(Receiver::Static(class)) => scopes.push((class, true)),
                _ => {}
            },
        }
        for (class, static_only) in scopes {
            let found = self
                .methods_named(&vm, class, &name)?
                .into_iter()
                .filter(|(_, m)| !static_only || m.is_static())
                .find_map(|(_, m)| {
                    let (params, ret) = eval::split_method_signature(&m.signature)?;
                    let fits =
                        params.len() == arity || (m.is_varargs() && arity + 1 >= params.len());
                    fits.then_some(ret)
                });
            if let Some(ret) = found {
                return Ok(Some(ret));
            }
        }
        Ok(None)
    }

    /// 目标 JVM 里已经加载好的类的名字（按名字排序）。每次停下最多取一次。
    fn loaded_names(&mut self, vm: &Vm) -> anyhow::Result<Rc<Vec<(String, u64)>>> {
        if let Some(names) = &self.loaded_names {
            return Ok(names.clone());
        }
        let mut names: Vec<(String, u64)> = vm
            .all_classes()?
            .into_iter()
            .filter(|(_, signature, status)| {
                status & jdwp::CLASS_STATUS_PREPARED != 0 && signature.starts_with('L')
            })
            .map(|(id, signature, _)| (signature_to_type_name(&signature), id))
            .collect();
        names.sort();
        let names = Rc::new(names);
        self.loaded_names = Some(names.clone());
        Ok(names)
    }
}

/// 值的声明类型对应的接收者；基本类型和没有加载的类型没有成员可列。
fn declared_receiver(vm: &Vm, signature: &str) -> anyhow::Result<Option<Receiver>> {
    if !signature.starts_with(['L', '[']) {
        return Ok(None);
    }
    Ok(loaded_class(vm, signature)?.map(Receiver::Instance))
}

/// 名字以 `prefix` 开头的已加载类。
fn names_with_prefix<'n>(
    names: &'n [(String, u64)],
    prefix: &'n str,
) -> impl Iterator<Item = &'n (String, u64)> {
    let start = names.partition_point(|(name, _)| name.as_str() < prefix);
    names[start..]
        .iter()
        .take_while(move |(name, _)| name.starts_with(prefix))
}

/// 类型的字段（含父类和接口的）。`static_only` 时只要静态字段，`member` 给出时只要这个名字。
fn add_fields(
    vm: &Vm,
    access: &Access,
    class: u64,
    static_only: bool,
    member: Option<&str>,
    out: &mut Targets,
) -> anyhow::Result<()> {
    for ty in supertypes(vm, class)? {
        for field in vm.fields(ty)?.iter() {
            if field.is_synthetic()
                || (static_only && !field.is_static())
                || member.is_some_and(|m| m != field.name)
                || !out.wants(&field.name, "field")
                || !access.allows(vm, ty, field.modifiers)?
            {
                continue;
            }
            out.push(
                &field.name,
                "field",
                GROUP_FIELD,
                Some(short_type(&field.signature)),
                None,
            );
        }
    }
    Ok(())
}

/// 类型的方法（含父类和接口的），同名的重载合成一项。接口的静态方法只在接口自己身上。
fn add_methods(
    vm: &Vm,
    access: &Access,
    class: u64,
    static_only: bool,
    member: Option<&str>,
    out: &mut Targets,
) -> anyhow::Result<()> {
    let mut signatures = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut overloads: HashMap<String, Vec<MethodInfo>> = HashMap::new();
    for ty in supertypes(vm, class)? {
        for method in vm.methods(ty)?.iter() {
            if method.name.starts_with('<')
                || method.name.contains('$')
                || method.is_bridge()
                || method.modifiers & ACC_SYNTHETIC != 0
                || (static_only && !method.is_static())
                || member.is_some_and(|m| m != method.name)
            {
                continue;
            }
            if method.is_static() && ty != class && vm.is_interface(ty)? {
                continue;
            }
            if !signatures.insert((method.name.clone(), method.signature.clone())) {
                continue;
            }
            if !out.wants(&method.name, "method") || !access.allows(vm, ty, method.modifiers)? {
                continue;
            }
            let entry = overloads.entry(method.name.clone()).or_default();
            if entry.is_empty() {
                order.push(method.name.clone());
            }
            entry.push(method.clone());
        }
    }
    for name in order {
        let methods = &overloads[&name];
        let mut detail = method_detail(&methods[0]);
        if methods.len() > 1 {
            detail.push_str(&format!(" (+{} overloads)", methods.len() - 1));
        }
        let no_args = methods.iter().all(|m| m.signature.starts_with("()"));
        let text = if no_args {
            format!("{name}()")
        } else {
            format!("{name}(")
        };
        out.push(&name, "method", GROUP_METHOD, Some(detail), Some(text));
    }
    Ok(())
}

fn type_kind(vm: &Vm, class: u64) -> &'static str {
    if vm.is_interface(class).unwrap_or(false) {
        "interface"
    } else {
        "class"
    }
}

/// `outer` 的成员类（已加载的；匿名类和局部类除外）。
fn add_nested_types(vm: &Vm, names: &[(String, u64)], outer: &str, out: &mut Targets) {
    let prefix = format!("{outer}$");
    for (name, class) in names_with_prefix(names, &prefix) {
        let simple = &name[prefix.len()..];
        if simple.contains('$') || simple.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        if out.wants(simple, "class") {
            out.push(
                simple,
                type_kind(vm, *class),
                GROUP_TYPE,
                Some(name.replace('$', ".")),
                None,
            );
        }
    }
}

/// 包里（已加载的）顶层类型。
fn add_package_types(
    vm: &Vm,
    access: &Access,
    names: &[(String, u64)],
    package: &str,
    out: &mut Targets,
) {
    let prefix = if package.is_empty() {
        String::new()
    } else {
        format!("{package}.")
    };
    for (name, class) in names_with_prefix(names, &prefix) {
        let simple = &name[prefix.len()..];
        if simple.contains(['.', '$'])
            || !out.wants(simple, "class")
            || !access.type_visible(vm, *class, name)
        {
            continue;
        }
        out.push(
            simple,
            type_kind(vm, *class),
            GROUP_TYPE,
            Some(name.clone()),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(text: &str) -> Option<(Option<&str>, &str)> {
        completion_site(text).map(|site| (site.receiver, site.prefix))
    }

    #[test]
    fn finds_receiver_and_prefix() {
        assert_eq!(site("ar"), Some((None, "ar")));
        assert_eq!(site(""), Some((None, "")));
        assert_eq!(site("a + li"), Some((None, "li")));
        assert_eq!(site("list."), Some((Some("list"), "")));
        assert_eq!(site("list.si"), Some((Some("list"), "si")));
        assert_eq!(site("a + b.c"), Some((Some("b"), "c")));
        assert_eq!(site("java.util."), Some((Some("java.util"), "")));
        assert_eq!(
            site("foo(bar.baz(1, 2).x"),
            Some((Some("bar.baz(1, 2)"), "x"))
        );
        assert_eq!(site("items[i + 1]."), Some((Some("items[i + 1]"), "")));
        assert_eq!(
            site("((String) o).len"),
            Some((Some("((String) o)"), "len"))
        );
        assert_eq!(site("x = new Foo(1)."), Some((Some("new Foo(1)"), "")));
        assert_eq!(site("renew Foo()."), Some((Some("Foo()"), "")));
        assert_eq!(site("\"a.b(\".len"), Some((Some("\"a.b(\""), "len")));
        assert_eq!(site("Outer.this.f"), Some((Some("Outer.this"), "f")));
        assert_eq!(site("list . "), Some((Some("list"), "")));
        // 字面量里、数字、没配对的括号
        assert_eq!(site("\"abc."), None);
        assert_eq!(site("1.5"), None);
        assert_eq!(site("x.)."), None);
        assert_eq!(site("."), None);
    }

    #[test]
    fn cursor_position() {
        assert_eq!(text_before_cursor("list.size", 0, 5), "list.");
        assert_eq!(text_before_cursor("a\nlist.si", 1, 7), "a\nlist.si");
        assert_eq!(text_before_cursor("a\nlist.si", 1, 4), "a\nlist");
        assert_eq!(text_before_cursor("a\nb", 5, 0), "a\nb");
        assert_eq!(text_before_cursor("ab", 0, 99), "ab");
        // UTF-16：`中` 占一个单位，`😀` 占两个
        assert_eq!(text_before_cursor("中x", 0, 1), "中");
        assert_eq!(text_before_cursor("😀x", 0, 2), "😀");
    }

    #[test]
    fn prefix_matching() {
        assert_eq!(match_rank("args", ""), Some(0));
        assert_eq!(match_rank("args", "ar"), Some(0));
        assert_eq!(match_rank("ArrayList", "ar"), Some(1));
        assert_eq!(match_rank("arrayList", "aL"), Some(2));
        assert_eq!(match_rank("currentTimeMillis", "ctm"), Some(2));
        assert_eq!(match_rank("size", "ar"), None);
        assert_eq!(match_rank("bar", "ar"), None);
    }

    #[test]
    fn type_names() {
        assert_eq!(short_type("Ljava/util/Map$Entry;"), "Map.Entry");
        assert_eq!(short_type("[Ljava/lang/String;"), "String[]");
        assert_eq!(short_type("I"), "int");
        assert_eq!(outermost("demo.Outer$Inner$Deep"), "demo.Outer");
        assert_eq!(outermost("Top"), "Top");
        assert_eq!(package_of("demo.Outer$Inner"), "demo");
        assert_eq!(package_of("Top"), "");
        let method = MethodInfo {
            id: 0,
            name: "of".to_owned(),
            signature: "([Ljava/lang/Object;)Ljava/util/List;".to_owned(),
            modifiers: 0x0080,
        };
        assert_eq!(method_detail(&method), "List of(Object...)");
    }
}
