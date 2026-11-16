//! DAP 消息的发送：给每条消息编号，响应、事件、调试目标输出都从这里走。
//! 可以在多个线程间共享（转发调试目标标准输出的线程也用它）。

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

pub type Sink = Box<dyn FnMut(&Value) + Send>;

struct Inner {
    seq: i64,
    sink: Sink,
}

#[derive(Clone)]
pub struct Output(Arc<Mutex<Inner>>);

impl Output {
    pub fn new(sink: Sink) -> Output {
        Output(Arc::new(Mutex::new(Inner { seq: 0, sink })))
    }

    fn send(&self, mut message: Value) {
        let mut inner = match self.0.lock() {
            Ok(inner) => inner,
            Err(poisoned) => poisoned.into_inner(),
        };
        inner.seq += 1;
        message["seq"] = json!(inner.seq);
        (inner.sink)(&message);
    }

    pub fn event(&self, event: &str, body: Value) {
        self.send(json!({ "type": "event", "event": event, "body": body }));
    }

    pub fn response(&self, request: &Request, result: Result<Value, String>) {
        let mut message = json!({
            "type": "response",
            "request_seq": request.seq,
            "command": request.command,
        });
        match result {
            Ok(body) => {
                message["success"] = json!(true);
                if !body.is_null() {
                    message["body"] = body;
                }
            }
            Err(error) => {
                message["success"] = json!(false);
                message["message"] = json!(error);
                message["body"] =
                    json!({ "error": { "id": 1, "format": error, "showUser": true } });
            }
        }
        self.send(message);
    }

    /// `output` 事件；`category` 为 `stdout` / `stderr` / `console` / `important`。
    pub fn output(&self, category: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        self.event("output", json!({ "category": category, "output": text }));
    }
}

/// 客户端发来的请求。
#[derive(Debug, Clone)]
pub struct Request {
    pub seq: i64,
    pub command: String,
    pub arguments: Value,
}

impl Request {
    pub fn parse(message: &Value) -> Option<Request> {
        if message.get("type").and_then(Value::as_str) != Some("request") {
            return None;
        }
        Some(Request {
            seq: message.get("seq").and_then(Value::as_i64).unwrap_or(0),
            command: message.get("command")?.as_str()?.to_owned(),
            arguments: message.get("arguments").cloned().unwrap_or(Value::Null),
        })
    }
}
