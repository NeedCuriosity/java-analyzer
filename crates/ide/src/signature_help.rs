//! 签名帮助：光标在实参列表里时，列出可用的重载并标出当前实参。

use base_db::Db;
use hir::{class_data, method_sig, Definition, Semantics};
use ide_db::{javadoc::javadoc_to_markdown, render::method_params};
use syntax::{
    ast::{self, AstNode},
    SyntaxKind,
};

use crate::{navigation::position_file, FilePosition};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    pub label: String,
    /// 各形参在 `label` 里的字节范围
    pub params: Vec<(u32, u32)>,
    pub docs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInfo>,
    pub active_signature: usize,
    pub active_parameter: Option<usize>,
}

pub(crate) fn signature_help(db: &dyn Db, position: FilePosition) -> Option<SignatureHelp> {
    let file = position_file(db, position)?;
    let sema = Semantics::new(db);
    let root = base_db::parse(db, file).syntax_node();
    let offset = position.offset;

    // 包含光标的最内层实参列表（光标必须在括号之内）
    let token = root.token_at_offset(offset).left_biased()?;
    let arg_list = token
        .parent_ancestors()
        .filter_map(ast::ArgList::cast)
        .find(|list| {
            let after_open = list
                .l_paren_token()
                .is_some_and(|t| t.text_range().end() <= offset);
            let before_close = list
                .r_paren_token()
                .is_none_or(|t| offset <= t.text_range().start());
            after_open && before_close
        })?;

    let active_arg = arg_list
        .syntax()
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|t| t.kind() == SyntaxKind::COMMA && t.text_range().end() <= offset)
        .count();

    let body = sema.body_of(file, arg_list.syntax())?;
    let result = hir::infer(db, body);
    let info = result.call_info(arg_list.syntax().text_range())?;
    if info.candidates.is_empty() {
        return None;
    }

    let mut signatures = Vec::with_capacity(info.candidates.len());
    for candidate in &info.candidates {
        let data = class_data(db, candidate.id.class)?;
        let method = data.methods.get(candidate.id.idx as usize)?;
        let params = method_params(db, candidate.id, &candidate.subst);

        let mut label = String::new();
        if !method.is_constructor {
            label.push_str(&candidate.ret_ty(db).display(db));
            label.push(' ');
        }
        label.push_str(&method.name);
        label.push('(');
        let mut ranges = Vec::with_capacity(params.len());
        for (idx, param) in params.iter().enumerate() {
            if idx > 0 {
                label.push_str(", ");
            }
            let start = label.len() as u32;
            label.push_str(param);
            ranges.push((start, label.len() as u32));
        }
        label.push(')');

        let docs = Definition::Method(candidate.id)
            .doc_comment(db)
            .map(|raw| javadoc_to_markdown(&raw))
            .filter(|docs| !docs.is_empty());
        signatures.push(SignatureInfo {
            label,
            params: ranges,
            docs,
        });
    }

    // 当前重载：已选中的那个；否则第一个形参个数够用的
    let accepts = |idx: usize| {
        let candidate = &info.candidates[idx];
        let sig = method_sig(db, candidate.id);
        sig.varargs || sig.params.len() > active_arg || (sig.params.is_empty() && active_arg == 0)
    };
    let active_signature = info
        .chosen
        .and_then(|chosen| info.candidates.iter().position(|c| c.id == chosen))
        .filter(|idx| accepts(*idx))
        .or_else(|| (0..info.candidates.len()).find(|idx| accepts(*idx)))
        .unwrap_or(0);

    let param_count = signatures[active_signature].params.len();
    let active_parameter = if param_count == 0 {
        None
    } else {
        // 变长参数：超出的实参都落在最后一个形参上
        Some(active_arg.min(param_count - 1))
    };
    Some(SignatureHelp {
        signatures,
        active_signature,
        active_parameter,
    })
}
