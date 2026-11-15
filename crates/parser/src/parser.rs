//! parser 的核心状态机：只提供 token 级别的原语，语法规则在 `grammar` 模块里。

use std::cell::Cell;

use crate::{
    event::Event,
    input::Input,
    SyntaxKind::{self, *},
};

pub(crate) struct Parser<'t> {
    inp: &'t Input,
    pos: usize,
    events: Vec<Event>,
    steps: Cell<u32>,
    /// 正在解析 `case` 标签：这里的 `x ->` 是分支箭头，不是 lambda
    no_lambda: bool,
}

/// 防止语法规则写错导致死循环：连续这么多次前瞻都没有消费 token 就 panic。
const PARSER_STEP_LIMIT: u32 = 15_000_000;

impl<'t> Parser<'t> {
    pub(crate) fn new(inp: &'t Input) -> Parser<'t> {
        Parser {
            inp,
            pos: 0,
            events: Vec::new(),
            steps: Cell::new(0),
            no_lambda: false,
        }
    }

    pub(crate) fn no_lambda(&self) -> bool {
        self.no_lambda
    }

    /// 设置"此处不允许 lambda"，返回原来的值供调用方恢复。
    pub(crate) fn set_no_lambda(&mut self, value: bool) -> bool {
        std::mem::replace(&mut self.no_lambda, value)
    }

    pub(crate) fn finish(self) -> Vec<Event> {
        self.events
    }

    /// 当前已消费的输入 token 数，用于检测"没有前进"。
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    /// 前瞻第 `n` 个 token 的原始种类（上下文关键字仍是 `IDENT`）。
    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        let steps = self.steps.get();
        assert!(steps < PARSER_STEP_LIMIT, "parser 可能陷入了死循环");
        self.steps.set(steps + 1);
        self.inp.kind(self.pos + n)
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.nth_at(0, kind)
    }

    pub(crate) fn nth_at(&self, n: usize, kind: SyntaxKind) -> bool {
        match kind {
            SHR => self.at_composite2(n, GT, GT),
            SHR_EQ => self.at_composite2(n, GT, GT_EQ),
            USHR => self.at_composite3(n, GT, GT, GT),
            USHR_EQ => self.at_composite3(n, GT, GT, GT_EQ),
            NON_SEALED_KW => self.at_non_sealed(n),
            _ if kind.is_contextual_keyword() => self.inp.contextual_kind(self.pos + n) == kind,
            _ => self.inp.kind(self.pos + n) == kind,
        }
    }

    /// 当前 token 是否是给定的上下文关键字（文本匹配的 `IDENT`）。
    pub(crate) fn at_contextual(&self, kind: SyntaxKind) -> bool {
        self.nth_contextual(0, kind)
    }

    pub(crate) fn nth_contextual(&self, n: usize, kind: SyntaxKind) -> bool {
        debug_assert!(kind.is_contextual_keyword());
        self.nth_at(n, kind)
    }

    fn at_composite2(&self, n: usize, k1: SyntaxKind, k2: SyntaxKind) -> bool {
        let i = self.pos + n;
        self.inp.kind(i) == k1 && self.inp.is_joint(i) && self.inp.kind(i + 1) == k2
    }

    fn at_composite3(&self, n: usize, k1: SyntaxKind, k2: SyntaxKind, k3: SyntaxKind) -> bool {
        let i = self.pos + n;
        self.inp.kind(i) == k1
            && self.inp.is_joint(i)
            && self.inp.kind(i + 1) == k2
            && self.inp.is_joint(i + 1)
            && self.inp.kind(i + 2) == k3
    }

    /// `non-sealed` 在 lexer 里是 `non`、`-`、`sealed` 三个紧邻的 token。
    fn at_non_sealed(&self, n: usize) -> bool {
        let i = self.pos + n;
        self.inp.kind(i) == IDENT
            && self.inp.is_joint(i)
            && self.inp.kind(i + 1) == MINUS
            && self.inp.is_joint(i + 1)
            && self.inp.contextual_kind(i + 2) == SEALED_KW
    }

    pub(crate) fn start(&mut self) -> Marker {
        let pos = self.events.len() as u32;
        self.events.push(Event::tombstone());
        Marker { pos }
    }

    /// 消费当前 token，要求它就是 `kind`（支持拼接 token 和上下文关键字）。
    pub(crate) fn bump(&mut self, kind: SyntaxKind) {
        assert!(
            self.at(kind),
            "bump({kind:?}) 时当前 token 是 {:?}",
            self.current()
        );
        let n_raw_tokens = match kind {
            SHR | SHR_EQ => 2,
            USHR | USHR_EQ | NON_SEALED_KW => 3,
            _ => 1,
        };
        self.do_bump(kind, n_raw_tokens);
    }

    pub(crate) fn bump_any(&mut self) {
        let kind = self.current();
        if kind == EOF {
            return;
        }
        self.do_bump(kind, 1);
    }

    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if !self.at(kind) {
            return false;
        }
        self.bump(kind);
        true
    }

    pub(crate) fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(format!("此处应为 {}", kind.describe()));
        false
    }

    pub(crate) fn error(&mut self, msg: impl Into<String>) {
        self.events.push(Event::Error { msg: msg.into() });
    }

    /// 报错，并把当前 token 包进一个 `ERROR` 节点里跳过。
    pub(crate) fn err_and_bump(&mut self, msg: &str) {
        if self.current() == EOF {
            self.error(msg);
            return;
        }
        let m = self.start();
        self.error(msg);
        self.bump_any();
        m.complete(self, ERROR);
    }

    fn do_bump(&mut self, kind: SyntaxKind, n_raw_tokens: u8) {
        self.pos += n_raw_tokens as usize;
        self.steps.set(0);
        self.events.push(Event::Token { kind, n_raw_tokens });
    }
}

/// 一个尚未完成的节点。
pub(crate) struct Marker {
    pos: u32,
}

impl Marker {
    pub(crate) fn complete(self, p: &mut Parser<'_>, kind: SyntaxKind) -> CompletedMarker {
        match &mut p.events[self.pos as usize] {
            Event::Start { kind: slot, .. } => *slot = kind,
            _ => unreachable!(),
        }
        p.events.push(Event::Finish);
        CompletedMarker {
            pos: self.pos,
            kind,
        }
    }

    /// 放弃这个节点；它已经包含的子节点会直接挂到父节点上。
    pub(crate) fn abandon(self, p: &mut Parser<'_>) {
        if self.pos as usize == p.events.len() - 1 {
            match p.events.pop() {
                Some(Event::Start {
                    kind: TOMBSTONE,
                    forward_parent: None,
                }) => {}
                _ => unreachable!(),
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CompletedMarker {
    pos: u32,
    kind: SyntaxKind,
}

impl CompletedMarker {
    /// 开始一个新节点，并让它成为当前这个已完成节点的父节点。
    pub(crate) fn precede(self, p: &mut Parser<'_>) -> Marker {
        let new_pos = p.start();
        match &mut p.events[self.pos as usize] {
            Event::Start { forward_parent, .. } => {
                *forward_parent = Some(new_pos.pos - self.pos);
            }
            _ => unreachable!(),
        }
        new_pos
    }

    pub(crate) fn kind(&self) -> SyntaxKind {
        self.kind
    }
}
