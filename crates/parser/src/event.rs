//! parser 先产出扁平的事件流，再转换成 `Output`。
//!
//! 事件流允许"事后给已完成的节点套一层父节点"（`forward_parent`），
//! 这是处理左递归结构（`a.b.c`、二元表达式）的关键。

use std::mem;

use crate::{output::Output, SyntaxKind};

#[derive(Debug)]
pub(crate) enum Event {
    Start {
        kind: SyntaxKind,
        forward_parent: Option<u32>,
    },
    Finish,
    Token {
        kind: SyntaxKind,
        n_raw_tokens: u8,
    },
    Error {
        msg: String,
    },
}

impl Event {
    pub(crate) fn tombstone() -> Event {
        Event::Start {
            kind: SyntaxKind::TOMBSTONE,
            forward_parent: None,
        }
    }
}

pub(crate) fn process(mut events: Vec<Event>) -> Output {
    let mut res = Output::default();
    let mut forward_parents = Vec::new();

    for i in 0..events.len() {
        match mem::replace(&mut events[i], Event::tombstone()) {
            Event::Start {
                kind,
                forward_parent,
            } => {
                // 沿着 forward_parent 链收集所有祖先，最外层的最后入栈
                forward_parents.push(kind);
                let mut idx = i;
                let mut fp = forward_parent;
                while let Some(fwd) = fp {
                    idx += fwd as usize;
                    fp = match mem::replace(&mut events[idx], Event::tombstone()) {
                        Event::Start {
                            kind,
                            forward_parent,
                        } => {
                            forward_parents.push(kind);
                            forward_parent
                        }
                        _ => unreachable!("forward_parent 必须指向 Start 事件"),
                    };
                }
                for kind in forward_parents.drain(..).rev() {
                    if kind != SyntaxKind::TOMBSTONE {
                        res.enter_node(kind);
                    }
                }
            }
            Event::Finish => res.leave_node(),
            Event::Token { kind, n_raw_tokens } => res.token(kind, n_raw_tokens),
            Event::Error { msg } => res.error(msg),
        }
    }
    res
}
