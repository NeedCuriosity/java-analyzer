//! 变量的显示、展开和修改，`evaluate` 请求、条件断点和日志断点的入口（求值本身在 `evaluator.rs`）。

use std::rc::Rc;

use anyhow::Context;
use base_db::{tr, trs};
use serde_json::{json, Value as Json};

use crate::{
    evaluator::{Cx, Val},
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
pub(crate) struct Described {
    pub(crate) value: String,
    type_name: String,
    reference: Option<VarRef>,
    indexed: Option<i64>,
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
pub(crate) fn find_field(
    vm: &Vm,
    class: u64,
    name: &str,
) -> anyhow::Result<Option<(u64, FieldInfo)>> {
    for class in vm.class_chain(class)? {
        if let Some(field) = vm.fields(class)?.iter().find(|f| f.name == name) {
            return Ok(Some((class, field.clone())));
        }
    }
    Ok(None)
}

impl Session {
    pub(crate) fn describe(&self, vm: &Vm, value: &Value, declared: Option<&str>) -> Described {
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

    pub(crate) fn frame_arg(&self, args: &Json) -> anyhow::Result<FrameRef> {
        let id = args
            .get("frameId")
            .and_then(Json::as_i64)
            .context(trs!("缺少 frameId", "Missing frameId"))?;
        self.frames.get(&id).copied().context(trs!(
            "栈帧已经失效（程序已经继续运行）",
            "The stack frame is no longer valid (the program has resumed)"
        ))
    }

    /// 线程的栈顶帧。
    fn top_frame(&self, vm: &Vm, thread: u64) -> anyhow::Result<FrameRef> {
        let (frame, location) = vm
            .frames(thread, 0, 1)?
            .into_iter()
            .next()
            .context(trs!("线程没有栈帧", "The thread has no stack frames"))?;
        Ok(FrameRef {
            thread,
            frame,
            location,
            depth: 0,
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
    pub(crate) fn visible_locals(
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
            .context(trs!(
                "缺少 variablesReference",
                "Missing variablesReference"
            ))?;
        let target = self.vars.get(&reference).cloned().context(trs!(
            "变量已经失效（程序已经继续运行）",
            "The variable is no longer valid (the program has resumed)"
        ))?;
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
                        "name": trs!("<局部变量>", "<local variables>"),
                        "value": trs!(
                            "没有局部变量信息（编译时加上 -g）",
                            "No local variable information (compile with -g)"
                        ),
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
            .context(trs!(
                "缺少 variablesReference",
                "Missing variablesReference"
            ))?;
        let name = args
            .get("name")
            .and_then(Json::as_str)
            .context(trs!("缺少 name", "Missing name"))?;
        let text = args
            .get("value")
            .and_then(Json::as_str)
            .context(trs!("缺少 value", "Missing value"))?;
        let target = self.vars.get(&reference).cloned().context(trs!(
            "变量已经失效（程序已经继续运行）",
            "The variable is no longer valid (the program has resumed)"
        ))?;
        let context_frame = match &target {
            VarRef::Locals(frame) => Some(*frame),
            _ => self.last_frame(&vm),
        };
        let (new_value, signature) = match target {
            VarRef::Locals(frame) => {
                let var = self
                    .visible_locals(&vm, &frame)?
                    .context(trs!(
                        "没有局部变量信息（编译时加上 -g）",
                        "No local variable information (compile with -g)"
                    ))?
                    .into_iter()
                    .find(|v| v.name == name)
                    .with_context(|| tr!("不能修改 `{name}`", "Cannot modify `{name}`"))?;
                let value = self.parse_new_value(&vm, context_frame, text, &var.signature)?;
                // 求值时调用过方法的话帧 ID 换过了，用刷新后的
                let frame = match self.vars.get(&reference) {
                    Some(VarRef::Locals(frame)) => *frame,
                    _ => frame,
                };
                vm.set_frame_value(frame.thread, frame.frame, var.slot, &value)?;
                (value, var.signature)
            }
            VarRef::Object(id) => {
                let class = vm.object_type(id)?;
                let (declaring, field) = find_field(&vm, class, name)?
                    .with_context(|| tr!("找不到字段 `{name}`", "Cannot find field `{name}`"))?;
                let value = self.parse_new_value(&vm, context_frame, text, &field.signature)?;
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
                    .with_context(|| {
                        tr!(
                            "找不到静态字段 `{name}`",
                            "Cannot find static field `{name}`"
                        )
                    })?;
                let value = self.parse_new_value(&vm, context_frame, text, &field.signature)?;
                vm.set_static_value(class, field.id, &value)?;
                (value, field.signature)
            }
            VarRef::Array { id, signature } => {
                let index: i32 = name
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse()
                    .with_context(|| {
                        tr!("`{name}` 不是数组下标", "`{name}` is not an array index")
                    })?;
                let element = signature
                    .strip_prefix('[')
                    .unwrap_or("Ljava/lang/Object;")
                    .to_owned();
                let value = self.parse_new_value(&vm, context_frame, text, &element)?;
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
    pub(crate) fn last_frame(&self, vm: &Vm) -> Option<FrameRef> {
        self.last_stopped_thread
            .and_then(|thread| self.top_frame(vm, thread).ok())
    }

    /// 把用户输入的文本转成声明类型为 `signature` 的值：基本类型的字面量、带引号的字符串和
    /// `null` 直接转换，别的按表达式在栈帧上求值。
    fn parse_new_value(
        &mut self,
        vm: &Rc<Vm>,
        frame: Option<FrameRef>,
        text: &str,
        signature: &str,
    ) -> anyhow::Result<Value> {
        let text = text.trim();
        if let Some(value) = self.quick_value(vm, text, signature)? {
            return Ok(value);
        }
        let frame = frame.with_context(|| {
            tr!(
                "`{text}` 不是 {} 类型的值",
                "`{text}` is not a value of type {}",
                signature_to_type_name(signature)
            )
        })?;
        let mut cx = Cx::new(vm.clone(), frame, true);
        self.eval_for_assignment(&mut cx, text, signature)
    }

    /// 不用求值就能直接转换的输入。
    fn quick_value(&self, vm: &Vm, text: &str, signature: &str) -> anyhow::Result<Option<Value>> {
        let number = text.trim_end_matches(['L', 'l', 'f', 'F', 'd', 'D']);
        Ok(match signature.as_bytes().first() {
            Some(b'I') => number.parse().ok().map(Value::Int),
            Some(b'J') => number.parse().ok().map(Value::Long),
            Some(b'S') => number.parse().ok().map(Value::Short),
            Some(b'B') => number.parse().ok().map(Value::Byte),
            Some(b'F') => number.parse().ok().map(Value::Float),
            Some(b'D') => number.parse().ok().map(Value::Double),
            Some(b'Z') => text.parse().ok().map(Value::Boolean),
            Some(b'C') => {
                let inner = text
                    .strip_prefix('\'')
                    .and_then(|t| t.strip_suffix('\''))
                    .unwrap_or(text);
                match inner.encode_utf16().collect::<Vec<u16>>().as_slice() {
                    [unit] => Some(Value::Char(*unit)),
                    _ => None,
                }
            }
            _ if text == "null" => Some(Value::Object {
                tag: tag::OBJECT,
                id: 0,
            }),
            _ if signature == "Ljava/lang/String;"
                || signature == "Ljava/lang/Object;"
                || signature == "Ljava/lang/CharSequence;" =>
            {
                match text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                    Some(inner) if !inner.contains('"') => Some(Value::Object {
                        tag: tag::STRING,
                        id: vm.create_string(&unescape(inner))?,
                    }),
                    _ => None,
                }
            }
            _ => None,
        })
    }

    // ---------------------------------------------------------------
    // 求值
    // ---------------------------------------------------------------

    pub(crate) fn evaluate(&mut self, args: &Json) -> anyhow::Result<Json> {
        let vm = self.vm()?;
        let expression = args
            .get("expression")
            .and_then(Json::as_str)
            .context(trs!("缺少 expression", "Missing expression"))?;
        let frame = match args.get("frameId").and_then(Json::as_i64) {
            Some(_) => self.frame_arg(args)?,
            None => self.last_frame(&vm).context(trs!(
                "程序没有停下，不能求值",
                "The program is not stopped; cannot evaluate"
            ))?,
        };
        // 悬停时只读：不调用方法、不新建对象
        let allow_invoke = args.get("context").and_then(Json::as_str) != Some("hover");
        let mut cx = Cx::new(vm.clone(), frame, allow_invoke);
        let (value, signature) = match self.eval_text(&mut cx, expression)? {
            Val::Str(text) => {
                return Ok(json!({
                    "result": quote(&text),
                    "type": "java.lang.String",
                    "variablesReference": 0,
                }))
            }
            val => self.materialize(&cx, val)?,
        };
        let mut json = self.variable_json(&vm, expression, &value, signature.as_deref(), None);
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

    pub(crate) fn eval_condition(&mut self, thread: u64, condition: &str) -> anyhow::Result<bool> {
        let vm = self.vm()?;
        let frame = self.top_frame(&vm, thread)?;
        let mut cx = Cx::new(vm, frame, true);
        let value = self.eval_text(&mut cx, condition)?;
        self.truthy(&cx, &value)
    }

    /// 日志断点的消息：`{表达式}` 换成求值结果。
    pub(crate) fn interpolate_log_message(&mut self, thread: u64, message: &str) -> String {
        let Ok(vm) = self.vm() else {
            return message.to_owned();
        };
        let mut cx = self
            .top_frame(&vm, thread)
            .ok()
            .map(|frame| Cx::new(vm.clone(), frame, true));
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
            let text = match cx.as_mut() {
                Some(cx) => match self.eval_text(cx, expression).and_then(|val| match val {
                    Val::Str(text) => Ok(quote(&text)),
                    val => {
                        let (value, signature) = self.materialize(cx, val)?;
                        Ok(self.describe(&vm, &value, signature.as_deref()).value)
                    }
                }) {
                    Ok(text) => text,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays() {
        assert_eq!(quote("a\"b\n"), "\"a\\\"b\\n\"");
        assert_eq!(char_display('x' as u16), "'x'");
        assert_eq!(char_display(0), "'\\u0000'");
        assert_eq!(unescape("a\\nb\\\\"), "a\nb\\");
        assert_eq!(primitive_display(&Value::Double(1.0)).unwrap().0, "1.0");
    }
}
