//! 变量的显示、展开、修改和简单表达式求值。

use anyhow::Context;
use serde_json::{json, Value as Json};

use crate::{
    eval::{self, BinOp, Expr, Literal, Segment},
    jdwp::{self, signature_to_type_name, simple_name, tag, FieldInfo, Value, Vm},
    session::{FrameRef, Session, VarRef},
};

/// 一次最多返回的数组元素（客户端没有分页时）
const MAX_ARRAY_ELEMENTS: i64 = 1000;

const BOXED: &[&str] = &[
    "java.lang.Integer",
    "java.lang.Long",
    "java.lang.Short",
    "java.lang.Byte",
    "java.lang.Character",
    "java.lang.Boolean",
    "java.lang.Float",
    "java.lang.Double",
];

/// 值在界面上的样子。
struct Described {
    value: String,
    type_name: String,
    reference: Option<VarRef>,
    indexed: Option<i64>,
}

/// 求值结果：目标 JVM 里的值（带声明类型的签名），或者表达式里的字面量。
enum Evaluated {
    Jdwp(Value, Option<String>),
    Literal(Literal),
}

/// 比较用的值。
#[derive(Debug, PartialEq)]
enum Cmp {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Null,
    Ref(u64),
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn char_display(unit: u16) -> String {
    match char::from_u32(unit as u32) {
        Some('\n') => "'\\n'".to_owned(),
        Some('\t') => "'\\t'".to_owned(),
        Some('\'') => "'\\''".to_owned(),
        Some(c) if !c.is_control() => format!("'{c}'"),
        _ => format!("'\\u{unit:04x}'"),
    }
}

fn primitive_display(value: &Value) -> Option<(String, &'static str)> {
    Some(match *value {
        Value::Byte(v) => (v.to_string(), "byte"),
        Value::Char(v) => (char_display(v), "char"),
        Value::Short(v) => (v.to_string(), "short"),
        Value::Int(v) => (v.to_string(), "int"),
        Value::Long(v) => (v.to_string(), "long"),
        Value::Float(v) => (format!("{v:?}"), "float"),
        Value::Double(v) => (format!("{v:?}"), "double"),
        Value::Boolean(v) => (v.to_string(), "boolean"),
        Value::Void => ("void".to_owned(), "void"),
        Value::Object { .. } => return None,
    })
}

/// 在类和它的父类里按名字找字段：`(声明它的类, 字段)`。
fn find_field(vm: &Vm, class: u64, name: &str) -> anyhow::Result<Option<(u64, FieldInfo)>> {
    for class in vm.class_chain(class)? {
        if let Some(field) = vm.fields(class)?.iter().find(|f| f.name == name) {
            return Ok(Some((class, field.clone())));
        }
    }
    Ok(None)
}

fn literal_display(literal: &Literal) -> (String, &'static str) {
    match literal {
        Literal::Int(n) => (n.to_string(), "long"),
        Literal::Float(f) => (format!("{f:?}"), "double"),
        Literal::Bool(b) => (b.to_string(), "boolean"),
        Literal::Null => ("null".to_owned(), "null"),
        Literal::Str(s) => (quote(s), "java.lang.String"),
        Literal::Char(c) => (char_display(*c), "char"),
    }
}

impl Session {
    fn describe(&self, vm: &Vm, value: &Value, declared: Option<&str>) -> Described {
        match self.try_describe(vm, value, declared) {
            Ok(described) => described,
            Err(err) => Described {
                value: format!("<{err:#}>"),
                type_name: declared.map(signature_to_type_name).unwrap_or_default(),
                reference: None,
                indexed: None,
            },
        }
    }

    fn try_describe(
        &self,
        vm: &Vm,
        value: &Value,
        declared: Option<&str>,
    ) -> anyhow::Result<Described> {
        if let Some((text, type_name)) = primitive_display(value) {
            return Ok(Described {
                value: text,
                type_name: type_name.to_owned(),
                reference: None,
                indexed: None,
            });
        }
        let Value::Object { tag: value_tag, id } = *value else {
            unreachable!("基本类型已经处理过")
        };
        if id == 0 {
            return Ok(Described {
                value: "null".to_owned(),
                type_name: declared.map(signature_to_type_name).unwrap_or_default(),
                reference: None,
                indexed: None,
            });
        }
        if value_tag == tag::STRING {
            return Ok(Described {
                value: quote(&vm.string_value(id)?),
                type_name: "java.lang.String".to_owned(),
                reference: None,
                indexed: None,
            });
        }
        let class = vm.object_type(id)?;
        let signature = vm.class_signature(class)?;
        let type_name = signature_to_type_name(&signature);
        if value_tag == tag::ARRAY || signature.starts_with('[') {
            let len = vm.array_length(id)?;
            let display = type_name.replacen("[]", &format!("[{len}]"), 1);
            return Ok(Described {
                value: simple_name(&display).to_owned(),
                type_name,
                reference: Some(VarRef::Array { id, signature }),
                indexed: Some(len as i64),
            });
        }
        if BOXED.contains(&type_name.as_str()) {
            if let Some((_, field)) = find_field(vm, class, "value")? {
                let inner = vm.object_values(id, &[field.id])?;
                if let Some((text, _)) = inner.first().and_then(primitive_display) {
                    return Ok(Described {
                        value: text,
                        type_name,
                        reference: None,
                        indexed: None,
                    });
                }
            }
        }
        let chain = vm.class_chain(class)?;
        let mut display = format!("{}@{id}", simple_name(&type_name));
        let is_enum = chain
            .iter()
            .skip(1)
            .any(|&c| vm.class_signature(c).is_ok_and(|s| s == "Ljava/lang/Enum;"));
        if is_enum {
            if let Some((_, field)) = find_field(vm, class, "name")? {
                if let Some(name) = vm
                    .object_values(id, &[field.id])?
                    .first()
                    .and_then(Value::object_id)
                {
                    display = vm.string_value(name)?;
                }
            }
        } else if type_name.starts_with("java.util.") {
            // 集合只显示字段，但在摘要里带上大小
            if let Some((_, field)) = find_field(vm, class, "size")? {
                if field.signature == "I" && !field.is_static() {
                    if let Some(Value::Int(size)) = vm.object_values(id, &[field.id])?.first() {
                        display = format!("{} (size={size})", simple_name(&type_name));
                    }
                }
            }
        }
        Ok(Described {
            value: display,
            type_name,
            reference: Some(VarRef::Object(id)),
            indexed: None,
        })
    }

    fn variable_json(
        &mut self,
        vm: &Vm,
        name: &str,
        value: &Value,
        declared: Option<&str>,
        evaluate_name: Option<String>,
    ) -> Json {
        let described = self.describe(vm, value, declared);
        let mut json = json!({
            "name": name,
            "value": described.value,
            "type": described.type_name,
            "variablesReference": 0,
        });
        if let Some(reference) = described.reference {
            let handle = self.new_handle();
            self.vars.insert(handle, reference);
            json["variablesReference"] = json!(handle);
        }
        if let Some(indexed) = described.indexed {
            json["indexedVariables"] = json!(indexed);
        }
        if let Some(evaluate_name) = evaluate_name {
            json["evaluateName"] = json!(evaluate_name);
        }
        json
    }

    fn frame_arg(&self, args: &Json) -> anyhow::Result<FrameRef> {
        let id = args
            .get("frameId")
            .and_then(Json::as_i64)
            .context("缺少 frameId")?;
        self.frames
            .get(&id)
            .copied()
            .context("栈帧已经失效（程序已经继续运行）")
    }

    /// 线程的栈顶帧。
    fn top_frame(&self, vm: &Vm, thread: u64) -> anyhow::Result<FrameRef> {
        let (frame, location) = vm
            .frames(thread, 0, 1)?
            .into_iter()
            .next()
            .context("线程没有栈帧")?;
        Ok(FrameRef {
            thread,
            frame,
            location,
        })
    }

    pub(crate) fn scopes(&mut self, args: &Json) -> anyhow::Result<Json> {
        let vm = self.vm()?;
        let frame = self.frame_arg(args)?;
        let locals = self.new_handle();
        self.vars.insert(locals, VarRef::Locals(frame));
        let mut scopes = vec![json!({
            "name": "Locals",
            "presentationHint": "locals",
            "variablesReference": locals,
            "expensive": false,
        })];
        let class = frame.location.class;
        if vm
            .fields(class)
            .is_ok_and(|fields| fields.iter().any(FieldInfo::is_static))
        {
            let statics = self.new_handle();
            self.vars.insert(statics, VarRef::Statics(class));
            scopes.push(json!({
                "name": "Static",
                "variablesReference": statics,
                "expensive": false,
            }));
        }
        Ok(json!({ "scopes": scopes }))
    }

    /// 栈帧里当前可见的局部变量 `(名字, 签名, 槽位)`；没有局部变量表时返回 `None`。
    fn visible_locals(
        &self,
        vm: &Vm,
        frame: &FrameRef,
    ) -> anyhow::Result<Option<Vec<jdwp::Variable>>> {
        let location = frame.location;
        let table = match vm.variable_table(location.class, location.method) {
            Ok(table) => table,
            Err(err) if jdwp::is_error(&err, jdwp::error_code::ABSENT_INFORMATION) => {
                return Ok(None)
            }
            Err(err) => return Err(err),
        };
        let mut vars: Vec<jdwp::Variable> = table
            .iter()
            .filter(|v| v.visible_at(location.index) && v.name != "this")
            .cloned()
            .collect();
        vars.sort_by_key(|v| (v.code_index, v.slot));
        Ok(Some(vars))
    }

    pub(crate) fn variables(&mut self, args: &Json) -> anyhow::Result<Json> {
        let vm = self.vm()?;
        let reference = args
            .get("variablesReference")
            .and_then(Json::as_i64)
            .context("缺少 variablesReference")?;
        let target = self
            .vars
            .get(&reference)
            .cloned()
            .context("变量已经失效（程序已经继续运行）")?;
        let mut result = Vec::new();
        match target {
            VarRef::Locals(frame) => {
                let this = vm.this_object(frame.thread, frame.frame)?;
                if !this.is_null() {
                    result.push(self.variable_json(&vm, "this", &this, None, Some("this".into())));
                }
                match self.visible_locals(&vm, &frame)? {
                    Some(vars) => {
                        let slots: Vec<(i32, u8)> = vars
                            .iter()
                            .map(|v| (v.slot, v.signature.as_bytes()[0]))
                            .collect();
                        let values = if slots.is_empty() {
                            Vec::new()
                        } else {
                            vm.frame_values(frame.thread, frame.frame, &slots)?
                        };
                        for (var, value) in vars.iter().zip(values) {
                            result.push(self.variable_json(
                                &vm,
                                &var.name,
                                &value,
                                Some(&var.signature),
                                Some(var.name.clone()),
                            ));
                        }
                    }
                    None => result.push(json!({
                        "name": "<局部变量>",
                        "value": "没有局部变量信息（编译时加上 -g）",
                        "variablesReference": 0,
                    })),
                }
            }
            VarRef::Statics(class) => {
                let fields: Vec<FieldInfo> = vm
                    .fields(class)?
                    .iter()
                    .filter(|f| f.is_static())
                    .cloned()
                    .collect();
                let ids: Vec<u64> = fields.iter().map(|f| f.id).collect();
                let values = if ids.is_empty() {
                    Vec::new()
                } else {
                    vm.static_values(class, &ids)?
                };
                for (field, value) in fields.iter().zip(values) {
                    result.push(self.variable_json(
                        &vm,
                        &field.name,
                        &value,
                        Some(&field.signature),
                        None,
                    ));
                }
            }
            VarRef::Object(id) => {
                let class = vm.object_type(id)?;
                for class in vm.class_chain(class)? {
                    let fields: Vec<FieldInfo> = vm
                        .fields(class)?
                        .iter()
                        .filter(|f| !f.is_static())
                        .cloned()
                        .collect();
                    if fields.is_empty() {
                        continue;
                    }
                    let ids: Vec<u64> = fields.iter().map(|f| f.id).collect();
                    let values = vm.object_values(id, &ids)?;
                    for (field, value) in fields.iter().zip(values) {
                        result.push(self.variable_json(
                            &vm,
                            &field.name,
                            &value,
                            Some(&field.signature),
                            None,
                        ));
                    }
                }
            }
            VarRef::Array { id, signature } => {
                let len = vm.array_length(id)? as i64;
                let start = args
                    .get("start")
                    .and_then(Json::as_i64)
                    .unwrap_or(0)
                    .clamp(0, len);
                let count = args
                    .get("count")
                    .and_then(Json::as_i64)
                    .filter(|&c| c > 0)
                    .unwrap_or(MAX_ARRAY_ELEMENTS)
                    .min(len - start);
                let element = signature
                    .strip_prefix('[')
                    .unwrap_or("Ljava/lang/Object;")
                    .to_owned();
                if count > 0 {
                    let values = vm.array_values(id, start as i32, count as i32)?;
                    for (i, value) in values.iter().enumerate() {
                        let name = format!("[{}]", start + i as i64);
                        result.push(self.variable_json(&vm, &name, value, Some(&element), None));
                    }
                }
            }
        }
        Ok(json!({ "variables": result }))
    }

    // ---------------------------------------------------------------
    // 修改变量
    // ---------------------------------------------------------------

    pub(crate) fn set_variable(&mut self, args: &Json) -> anyhow::Result<Json> {
        let vm = self.vm()?;
        let reference = args
            .get("variablesReference")
            .and_then(Json::as_i64)
            .context("缺少 variablesReference")?;
        let name = args
            .get("name")
            .and_then(Json::as_str)
            .context("缺少 name")?;
        let text = args
            .get("value")
            .and_then(Json::as_str)
            .context("缺少 value")?;
        let target = self
            .vars
            .get(&reference)
            .cloned()
            .context("变量已经失效（程序已经继续运行）")?;
        let context_frame = match &target {
            VarRef::Locals(frame) => Some(*frame),
            _ => self.last_frame(&vm),
        };
        let (new_value, signature) = match target {
            VarRef::Locals(frame) => {
                let var = self
                    .visible_locals(&vm, &frame)?
                    .context("没有局部变量信息（编译时加上 -g）")?
                    .into_iter()
                    .find(|v| v.name == name)
                    .with_context(|| format!("不能修改 `{name}`"))?;
                let value =
                    self.parse_new_value(&vm, context_frame.as_ref(), text, &var.signature)?;
                vm.set_frame_value(frame.thread, frame.frame, var.slot, &value)?;
                (value, var.signature)
            }
            VarRef::Object(id) => {
                let class = vm.object_type(id)?;
                let (declaring, field) = find_field(&vm, class, name)?
                    .with_context(|| format!("找不到字段 `{name}`"))?;
                let value =
                    self.parse_new_value(&vm, context_frame.as_ref(), text, &field.signature)?;
                if field.is_static() {
                    vm.set_static_value(declaring, field.id, &value)?;
                } else {
                    vm.set_object_value(id, field.id, &value)?;
                }
                (value, field.signature)
            }
            VarRef::Statics(class) => {
                let field = vm
                    .fields(class)?
                    .iter()
                    .find(|f| f.name == name && f.is_static())
                    .cloned()
                    .with_context(|| format!("找不到静态字段 `{name}`"))?;
                let value =
                    self.parse_new_value(&vm, context_frame.as_ref(), text, &field.signature)?;
                vm.set_static_value(class, field.id, &value)?;
                (value, field.signature)
            }
            VarRef::Array { id, signature } => {
                let index: i32 = name
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse()
                    .with_context(|| format!("`{name}` 不是数组下标"))?;
                let element = signature
                    .strip_prefix('[')
                    .unwrap_or("Ljava/lang/Object;")
                    .to_owned();
                let value = self.parse_new_value(&vm, context_frame.as_ref(), text, &element)?;
                vm.set_array_value(id, index, &value)?;
                (value, element)
            }
        };
        let described = self.describe(&vm, &new_value, Some(&signature));
        let mut result = json!({
            "value": described.value,
            "type": described.type_name,
            "variablesReference": 0,
        });
        if let Some(reference) = described.reference {
            let handle = self.new_handle();
            self.vars.insert(handle, reference);
            result["variablesReference"] = json!(handle);
        }
        Ok(result)
    }

    /// 最近停下的线程的栈顶帧。
    fn last_frame(&self, vm: &Vm) -> Option<FrameRef> {
        self.last_stopped_thread
            .and_then(|thread| self.top_frame(vm, thread).ok())
    }

    /// 把用户输入的文本转成声明类型为 `signature` 的值。
    fn parse_new_value(
        &self,
        vm: &Vm,
        frame: Option<&FrameRef>,
        text: &str,
        signature: &str,
    ) -> anyhow::Result<Value> {
        let text = text.trim();
        let number = text.trim_end_matches(['L', 'l', 'f', 'F', 'd', 'D']);
        let bad = || {
            anyhow::anyhow!(
                "`{text}` 不是 {} 类型的值",
                signature_to_type_name(signature)
            )
        };
        Ok(match signature.as_bytes().first() {
            Some(b'I') => Value::Int(number.parse().map_err(|_| bad())?),
            Some(b'J') => Value::Long(number.parse().map_err(|_| bad())?),
            Some(b'S') => Value::Short(number.parse().map_err(|_| bad())?),
            Some(b'B') => Value::Byte(number.parse().map_err(|_| bad())?),
            Some(b'F') => Value::Float(number.parse().map_err(|_| bad())?),
            Some(b'D') => Value::Double(number.parse().map_err(|_| bad())?),
            Some(b'Z') => Value::Boolean(text.parse().map_err(|_| bad())?),
            Some(b'C') => {
                let inner = text
                    .strip_prefix('\'')
                    .and_then(|t| t.strip_suffix('\''))
                    .unwrap_or(text);
                let units: Vec<u16> = inner.encode_utf16().collect();
                match units.as_slice() {
                    [unit] => Value::Char(*unit),
                    _ => return Err(bad()),
                }
            }
            _ if text == "null" => Value::Object {
                tag: tag::OBJECT,
                id: 0,
            },
            _ if signature == "Ljava/lang/String;"
                || signature == "Ljava/lang/Object;"
                || signature == "Ljava/lang/CharSequence;" =>
            {
                match text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                    Some(inner) => Value::Object {
                        tag: tag::STRING,
                        id: vm.create_string(&unescape(inner))?,
                    },
                    None => self.evaluate_object(vm, frame, text)?,
                }
            }
            _ => self.evaluate_object(vm, frame, text)?,
        })
    }

    fn evaluate_object(
        &self,
        vm: &Vm,
        frame: Option<&FrameRef>,
        text: &str,
    ) -> anyhow::Result<Value> {
        let frame = frame.context("没有可用的栈帧来求值")?;
        let expr = eval::parse(text).map_err(anyhow::Error::msg)?;
        match self.eval_expr(vm, frame, &expr)? {
            Evaluated::Jdwp(value, _) => Ok(value),
            Evaluated::Literal(_) => anyhow::bail!("`{text}` 的类型不对"),
        }
    }

    // ---------------------------------------------------------------
    // 求值
    // ---------------------------------------------------------------

    pub(crate) fn evaluate(&mut self, args: &Json) -> anyhow::Result<Json> {
        let vm = self.vm()?;
        let expression = args
            .get("expression")
            .and_then(Json::as_str)
            .context("缺少 expression")?;
        let frame = match args.get("frameId").and_then(Json::as_i64) {
            Some(_) => self.frame_arg(args)?,
            None => self.last_frame(&vm).context("程序没有停下，不能求值")?,
        };
        let expr = eval::parse(expression).map_err(anyhow::Error::msg)?;
        match self.eval_expr(&vm, &frame, &expr)? {
            Evaluated::Jdwp(value, signature) => {
                let mut json =
                    self.variable_json(&vm, expression, &value, signature.as_deref(), None);
                let mut result = json!({
                    "result": json["value"].take(),
                    "type": json["type"].take(),
                    "variablesReference": json["variablesReference"].take(),
                });
                if let Some(indexed) = json.get("indexedVariables") {
                    result["indexedVariables"] = indexed.clone();
                }
                Ok(result)
            }
            Evaluated::Literal(literal) => {
                let (text, type_name) = literal_display(&literal);
                Ok(json!({ "result": text, "type": type_name, "variablesReference": 0 }))
            }
        }
    }

    pub(crate) fn eval_condition(&mut self, thread: u64, condition: &str) -> anyhow::Result<bool> {
        let vm = self.vm()?;
        let frame = self.top_frame(&vm, thread)?;
        let expr = eval::parse(condition).map_err(anyhow::Error::msg)?;
        let value = self.eval_expr(&vm, &frame, &expr)?;
        self.truthy(&vm, &value)
    }

    /// 日志断点的消息：`{表达式}` 换成求值结果。
    pub(crate) fn interpolate_log_message(&mut self, thread: u64, message: &str) -> String {
        let Ok(vm) = self.vm() else {
            return message.to_owned();
        };
        let frame = self.top_frame(&vm, thread).ok();
        let mut out = String::new();
        let mut rest = message;
        while let Some(start) = rest.find('{') {
            out.push_str(&rest[..start]);
            let Some(len) = rest[start..].find('}') else {
                out.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let expression = &rest[start + 1..start + len];
            let text = match &frame {
                Some(frame) => match eval::parse(expression)
                    .map_err(anyhow::Error::msg)
                    .and_then(|expr| self.eval_expr(&vm, frame, &expr))
                {
                    Ok(Evaluated::Jdwp(value, signature)) => {
                        self.describe(&vm, &value, signature.as_deref()).value
                    }
                    Ok(Evaluated::Literal(literal)) => literal_display(&literal).0,
                    Err(err) => format!("<{err:#}>"),
                },
                None => format!("{{{expression}}}"),
            };
            out.push_str(&text);
            rest = &rest[start + len + 1..];
        }
        out.push_str(rest);
        out
    }

    /// 异常对象的 `detailMessage`。
    pub(crate) fn throwable_message(&self, exception: u64) -> Option<String> {
        let vm = self.vm.as_ref()?;
        let class = vm.object_type(exception).ok()?;
        let (_, field) = find_field(vm, class, "detailMessage").ok()??;
        let value = *vm.object_values(exception, &[field.id]).ok()?.first()?;
        vm.string_value(value.object_id()?).ok()
    }

    fn eval_expr(&self, vm: &Vm, frame: &FrameRef, expr: &Expr) -> anyhow::Result<Evaluated> {
        Ok(match expr {
            Expr::Literal(literal) => Evaluated::Literal(literal.clone()),
            Expr::Path(name, segments) => {
                let (value, signature) = self.eval_path(vm, frame, name, segments)?;
                Evaluated::Jdwp(value, signature)
            }
            Expr::Not(inner) => {
                let value = self.eval_expr(vm, frame, inner)?;
                Evaluated::Literal(Literal::Bool(!self.truthy(vm, &value)?))
            }
            Expr::Binary(lhs, op @ (BinOp::And | BinOp::Or), rhs) => {
                let left = self.truthy(vm, &self.eval_expr(vm, frame, lhs)?)?;
                let result = match op {
                    BinOp::And => left && self.truthy(vm, &self.eval_expr(vm, frame, rhs)?)?,
                    _ => left || self.truthy(vm, &self.eval_expr(vm, frame, rhs)?)?,
                };
                Evaluated::Literal(Literal::Bool(result))
            }
            Expr::Binary(lhs, op, rhs) => {
                let left = self.comparable(vm, &self.eval_expr(vm, frame, lhs)?)?;
                let right = self.comparable(vm, &self.eval_expr(vm, frame, rhs)?)?;
                Evaluated::Literal(Literal::Bool(compare(&left, *op, &right)?))
            }
        })
    }

    fn truthy(&self, vm: &Vm, value: &Evaluated) -> anyhow::Result<bool> {
        match self.comparable(vm, value)? {
            Cmp::Bool(b) => Ok(b),
            other => anyhow::bail!("条件的结果不是 boolean: {other:?}"),
        }
    }

    fn comparable(&self, vm: &Vm, value: &Evaluated) -> anyhow::Result<Cmp> {
        let value = match value {
            Evaluated::Literal(literal) => {
                return Ok(match literal {
                    Literal::Int(n) => Cmp::Int(*n),
                    Literal::Float(f) => Cmp::Float(*f),
                    Literal::Bool(b) => Cmp::Bool(*b),
                    Literal::Null => Cmp::Null,
                    Literal::Str(s) => Cmp::Str(s.clone()),
                    Literal::Char(c) => Cmp::Int(*c as i64),
                })
            }
            Evaluated::Jdwp(value, _) => *value,
        };
        Ok(match value {
            Value::Byte(v) => Cmp::Int(v as i64),
            Value::Char(v) => Cmp::Int(v as i64),
            Value::Short(v) => Cmp::Int(v as i64),
            Value::Int(v) => Cmp::Int(v as i64),
            Value::Long(v) => Cmp::Int(v),
            Value::Float(v) => Cmp::Float(v as f64),
            Value::Double(v) => Cmp::Float(v),
            Value::Boolean(v) => Cmp::Bool(v),
            Value::Void => anyhow::bail!("void 不能比较"),
            Value::Object { id: 0, .. } => Cmp::Null,
            Value::Object {
                tag: tag::STRING,
                id,
            } => Cmp::Str(vm.string_value(id)?),
            Value::Object { id, .. } => {
                // 包装类型拆箱
                let class = vm.object_type(id)?;
                let name = vm.class_name(class)?;
                if BOXED.contains(&name.as_str()) {
                    if let Some((_, field)) = find_field(vm, class, "value")? {
                        if let Some(inner) = vm.object_values(id, &[field.id])?.first() {
                            return self.comparable(vm, &Evaluated::Jdwp(*inner, None));
                        }
                    }
                }
                Cmp::Ref(id)
            }
        })
    }

    fn eval_path(
        &self,
        vm: &Vm,
        frame: &FrameRef,
        name: &str,
        segments: &[Segment],
    ) -> anyhow::Result<(Value, Option<String>)> {
        let (mut value, mut signature) = self.resolve_name(vm, frame, name)?;
        let mut path = name.to_owned();
        for segment in segments {
            match segment {
                Segment::Field(field_name) => {
                    let id = match value {
                        Value::Object { id: 0, .. } => anyhow::bail!("`{path}` 是 null"),
                        Value::Object { id, .. } => id,
                        _ => anyhow::bail!("`{path}` 是基本类型，没有字段 `{field_name}`"),
                    };
                    if value.tag() == tag::ARRAY && field_name == "length" {
                        value = Value::Int(vm.array_length(id)?);
                        signature = Some("I".to_owned());
                    } else {
                        let class = vm.object_type(id)?;
                        let (declaring, field) = find_field(vm, class, field_name)?
                            .with_context(|| format!("`{path}` 没有字段 `{field_name}`"))?;
                        let values = if field.is_static() {
                            vm.static_values(declaring, &[field.id])?
                        } else {
                            vm.object_values(id, &[field.id])?
                        };
                        value = *values.first().context("JVM 没有返回字段值")?;
                        signature = Some(field.signature.clone());
                    }
                    path = format!("{path}.{field_name}");
                }
                Segment::Index(index) => {
                    let id = match value {
                        Value::Object {
                            tag: tag::ARRAY,
                            id,
                        } if id != 0 => id,
                        Value::Object { id: 0, .. } => anyhow::bail!("`{path}` 是 null"),
                        _ => anyhow::bail!("`{path}` 不是数组"),
                    };
                    let len = vm.array_length(id)? as i64;
                    anyhow::ensure!(
                        (0..len).contains(index),
                        "下标 {index} 超出数组 `{path}` 的长度 {len}"
                    );
                    value = *vm
                        .array_values(id, *index as i32, 1)?
                        .first()
                        .context("JVM 没有返回数组元素")?;
                    signature = signature.and_then(|s| s.strip_prefix('[').map(str::to_owned));
                    path = format!("{path}[{index}]");
                }
            }
        }
        Ok((value, signature))
    }

    /// 名字依次在局部变量、`this` 的字段、当前类的静态字段里找。
    fn resolve_name(
        &self,
        vm: &Vm,
        frame: &FrameRef,
        name: &str,
    ) -> anyhow::Result<(Value, Option<String>)> {
        let this = vm.this_object(frame.thread, frame.frame)?;
        if name == "this" {
            anyhow::ensure!(!this.is_null(), "静态方法里没有 this");
            return Ok((this, None));
        }
        if let Some(vars) = self.visible_locals(vm, frame)? {
            if let Some(var) = vars.iter().rev().find(|v| v.name == name) {
                let values = vm.frame_values(
                    frame.thread,
                    frame.frame,
                    &[(var.slot, var.signature.as_bytes()[0])],
                )?;
                let value = *values.first().context("JVM 没有返回变量值")?;
                return Ok((value, Some(var.signature.clone())));
            }
        }
        if let Some(id) = this.object_id() {
            let class = vm.object_type(id)?;
            if let Some((declaring, field)) = find_field(vm, class, name)? {
                let values = if field.is_static() {
                    vm.static_values(declaring, &[field.id])?
                } else {
                    vm.object_values(id, &[field.id])?
                };
                let value = *values.first().context("JVM 没有返回字段值")?;
                return Ok((value, Some(field.signature)));
            }
        }
        if let Some((declaring, field)) = find_field(vm, frame.location.class, name)? {
            if field.is_static() {
                let value = *vm
                    .static_values(declaring, &[field.id])?
                    .first()
                    .context("JVM 没有返回字段值")?;
                return Ok((value, Some(field.signature)));
            }
        }
        anyhow::bail!("找不到名字 `{name}`")
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn compare(left: &Cmp, op: BinOp, right: &Cmp) -> anyhow::Result<bool> {
    use std::cmp::Ordering;
    let ordering: Option<Ordering> = match (left, right) {
        (Cmp::Int(a), Cmp::Int(b)) => Some(a.cmp(b)),
        (Cmp::Int(a), Cmp::Float(b)) => (*a as f64).partial_cmp(b),
        (Cmp::Float(a), Cmp::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Cmp::Float(a), Cmp::Float(b)) => a.partial_cmp(b),
        (Cmp::Str(a), Cmp::Str(b)) => Some(a.cmp(b)),
        _ => None,
    };
    if let Some(ordering) = ordering {
        return Ok(match op {
            BinOp::Eq => ordering == Ordering::Equal,
            BinOp::Ne => ordering != Ordering::Equal,
            BinOp::Lt => ordering == Ordering::Less,
            BinOp::Le => ordering != Ordering::Greater,
            BinOp::Gt => ordering == Ordering::Greater,
            BinOp::Ge => ordering != Ordering::Less,
            BinOp::And | BinOp::Or => unreachable!("逻辑运算单独处理"),
        });
    }
    let equal = match (left, right) {
        (Cmp::Bool(a), Cmp::Bool(b)) => a == b,
        (Cmp::Null, Cmp::Null) => true,
        (Cmp::Ref(a), Cmp::Ref(b)) => a == b,
        (Cmp::Null, Cmp::Ref(_) | Cmp::Str(_)) | (Cmp::Ref(_) | Cmp::Str(_), Cmp::Null) => false,
        _ => anyhow::bail!("不能比较 {left:?} 和 {right:?}"),
    };
    match op {
        BinOp::Eq => Ok(equal),
        BinOp::Ne => Ok(!equal),
        _ => anyhow::bail!("{left:?} 和 {right:?} 不能比较大小"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparisons() {
        assert!(compare(&Cmp::Int(2), BinOp::Eq, &Cmp::Int(2)).unwrap());
        assert!(compare(&Cmp::Int(2), BinOp::Lt, &Cmp::Float(2.5)).unwrap());
        assert!(compare(&Cmp::Str("a".into()), BinOp::Ne, &Cmp::Str("b".into())).unwrap());
        assert!(compare(&Cmp::Null, BinOp::Eq, &Cmp::Null).unwrap());
        assert!(compare(&Cmp::Ref(1), BinOp::Ne, &Cmp::Null).unwrap());
        assert!(compare(&Cmp::Bool(true), BinOp::Lt, &Cmp::Bool(false)).is_err());
        assert!(compare(&Cmp::Int(1), BinOp::Eq, &Cmp::Str("1".into())).is_err());
    }

    #[test]
    fn displays() {
        assert_eq!(quote("a\"b\n"), "\"a\\\"b\\n\"");
        assert_eq!(char_display('x' as u16), "'x'");
        assert_eq!(char_display(0), "'\\u0000'");
        assert_eq!(unescape("a\\nb\\\\"), "a\nb\\");
        assert_eq!(primitive_display(&Value::Double(1.0)).unwrap().0, "1.0");
    }
}
