//! parser 的输出：一串建树指令，与具体的语法树实现无关。

use crate::SyntaxKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Enter {
        kind: SyntaxKind,
    },
    /// `n_input_tokens` 个输入 token 合并为一个 `kind` 的 token
    Token {
        kind: SyntaxKind,
        n_input_tokens: u8,
    },
    Exit,
    Error {
        msg: String,
    },
}

#[derive(Default, Debug)]
pub struct Output {
    steps: Vec<Step>,
}

impl Output {
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub fn into_steps(self) -> Vec<Step> {
        self.steps
    }

    pub(crate) fn enter_node(&mut self, kind: SyntaxKind) {
        self.steps.push(Step::Enter { kind });
    }

    pub(crate) fn leave_node(&mut self) {
        self.steps.push(Step::Exit);
    }

    pub(crate) fn token(&mut self, kind: SyntaxKind, n_input_tokens: u8) {
        self.steps.push(Step::Token {
            kind,
            n_input_tokens,
        });
    }

    pub(crate) fn error(&mut self, msg: String) {
        self.steps.push(Step::Error { msg });
    }
}
