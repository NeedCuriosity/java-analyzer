//! 字节码反编译：给没有源码的依赖库类生成可读的方法体。
//!
//! 流程：解码字节码 → 模拟操作数栈得到基本块里的语句（[`translate`]）→ 整理控制流图、
//! 求支配树和循环（[`cfg`]）→ 结构化成 if / 循环 / switch / try（[`structure`]）→
//! 各种还原和化简（[`transform`]）→ 变量声明和起名（[`vars`]）→ 输出源码（[`render`]）。
//!
//! 健壮性优先：任何一步做不到都只让这一个方法退回"没有方法体"，工作量有上限，
//! 输出的方法体都用 java-analyzer 自己的解析器检查过，保证没有语法错误。

// 图算法里按块编号同时访问好几个数组，下标循环更直白
#![allow(clippy::needless_range_loop, clippy::type_complexity)]

mod anon;
mod bytecode;
mod cfg;
mod class_text;
mod header;
mod ir;
mod lambda;
mod names;
mod render;
mod structure;
mod transform;
mod translate;
mod types;
mod vars;

#[cfg(test)]
mod tests;

use classfile::{
    access,
    code::{parse_class_code, CodeClass, CodeMethod},
    signature, ClassFileError,
};

pub use crate::{
    class_text::{decompile_class_text, DecompileStats},
    names::NameTable,
};
use crate::{
    ir::{LabelId, Stmt, VarId, Vars},
    render::{Printer, Scope},
};

/// 反编译失败的原因（只用于日志和统计）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fail(pub &'static str);

/// 一个方法最多的基本块数。
pub(crate) const MAX_BLOCKS: usize = 6000;
/// 表达式最深的嵌套。
pub(crate) const MAX_EXPR_DEPTH: usize = 400;
/// lambda / 匿名类最多嵌套几层。
pub(crate) const MAX_NESTING: u32 = 4;

/// 反编译需要从外部取得的信息。
pub trait ClassEnv {
    /// 类文件的字节（匿名类的类体要单独读）。
    fn class_bytes(&self, binary_name: &str) -> Option<Vec<u8>>;
    /// 类路径上有没有这个类（决定类名怎么写、要不要 import）。
    fn class_exists(&self, binary_name: &str) -> bool;
    /// 类是否带类型参数（`new ArrayList<>()` 要用到）。不知道时返回 `false`。
    fn is_generic(&self, _binary_name: &str) -> bool {
        false
    }
}

/// 工作量计数，用完就放弃，保证不会卡住。
pub(crate) struct Budget(usize);

impl Budget {
    pub(crate) fn spend(&mut self, n: usize) -> Result<(), Fail> {
        if self.0 < n {
            return Err(Fail("超出工作量上限"));
        }
        self.0 -= n;
        Ok(())
    }
}

pub(crate) struct MethodCx<'a> {
    pub class: &'a CodeClass,
    pub method: &'a CodeMethod,
    pub env: &'a dyn ClassEnv,
    pub vars: &'a mut Vars,
    pub budget: &'a mut Budget,
    pub labels: &'a mut LabelId,
    /// lambda / 匿名类的嵌套层数
    pub depth: u32,
    /// 匿名类构造器：保留编译器生成的语句，供分析捕获的变量
    pub raw_ctor: bool,
    /// 开头几个形参是编译器合成的（内部类的外层实例、枚举的名字和序号）
    pub synthetic_params: usize,
}

pub(crate) struct MethodAst {
    pub body: Vec<Stmt>,
    /// 按描述符顺序的形参
    pub params: Vec<VarId>,
}

/// 一个方法的完整流程（到声明变量为止，不起名）。
pub(crate) fn decompile_body(cx: &mut MethodCx<'_>) -> Result<MethodAst, Fail> {
    let method = cx.method;
    let code = method.code.as_ref().ok_or(Fail("没有方法体"))?;
    let translated = translate::Translator::new(cx, code)?.run()?;
    let mut cfg = translated.cfg;
    let params = translated.params;
    // 形参类型：泛型签名优先
    let (param_tys, _) = types::method_types(method);
    for (p, ty) in params.iter().zip(param_tys) {
        let info = cx.vars.get_mut(*p);
        info.is_param = true;
        if !(info.named_by_lvt && info.ty.as_ref().is_some_and(|t| matches!(t, classfile::signature::JType::Class { args, .. } if !args.is_empty()))) {
            info.ty = Some(ty);
        }
    }
    cfg::thread_jumps(&mut cfg);
    cfg::merge_conditions(&mut cfg);
    cx.budget.spend(cfg.blocks.len())?;
    let analysis = cfg::Analysis::new(&cfg)?;
    let mut body = {
        let mut s = structure::Structurer::new(&cfg, &analysis, cx.budget, cx.labels)?;
        s.run()?
    };
    let synthetic: Vec<VarId> = params.iter().take(cx.synthetic_params).copied().collect();
    transform::run(&mut body, cx, &synthetic)?;
    Ok(MethodAst { body, params })
}

/// 形参的名字：给定的源码名字（对齐到末尾）、MethodParameters、LocalVariableTable，都没有时 `argN`。
pub(crate) fn name_params(
    vars: &mut Vars,
    params: &[VarId],
    m: &CodeMethod,
    declared: Option<&[String]>,
) {
    let n = params.len();
    let k = declared.map_or(n, |d| d.len().min(n));
    let synthetic = n - k;
    let from_attr: Vec<Option<String>> = if m.parameter_names.len() == n {
        m.parameter_names.clone()
    } else {
        vec![None; n]
    };
    for (i, &p) in params.iter().enumerate() {
        let name = if i < synthetic {
            format!("$synthetic{i}")
        } else if let Some(d) = declared {
            d[i - synthetic].clone()
        } else if let Some(Some(a)) = from_attr
            .get(i)
            .filter(|a| a.as_deref().is_some_and(names::is_valid_identifier))
        {
            a.clone()
        } else if vars.get(p).named_by_lvt
            && vars
                .get(p)
                .name
                .as_deref()
                .is_some_and(names::is_valid_identifier)
        {
            vars.get(p).name.clone().unwrap()
        } else {
            format!("arg{}", i - synthetic)
        };
        vars.get_mut(p).name = Some(name);
    }
}

fn budget_for(m: &CodeMethod) -> usize {
    let len = m.code.as_ref().map_or(0, |c| c.bytecode.len());
    50_000 + len * 60
}

/// 输出的方法体要能被 java-analyzer 自己的解析器无错误地解析。
fn check_syntax(body: &str, ctor: bool) -> Result<(), Fail> {
    let text = if ctor {
        format!("class Check {{\n Check() {body}\n}}\n")
    } else {
        format!("class Check {{\n void check() {body}\n}}\n")
    };
    let parse = syntax::ast::CompilationUnit::parse(&text);
    if parse.errors().is_empty() {
        Ok(())
    } else {
        tracing::debug!("反编译结果有语法错误: {:?}\n{text}", parse.errors());
        Err(Fail("生成的代码有语法错误"))
    }
}

/// 一个类的反编译器。
pub struct ClassDecompiler {
    pub class: CodeClass,
}

impl ClassDecompiler {
    pub fn new(bytes: &[u8]) -> Result<ClassDecompiler, ClassFileError> {
        Ok(ClassDecompiler {
            class: parse_class_code(bytes)?,
        })
    }

    /// 和签名级模型（`hir`）里的方法列表一一对应的方法下标：去掉合成方法、桥接方法和静态初始化块。
    pub fn source_methods(&self) -> Vec<usize> {
        self.class
            .methods
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.access & (access::SYNTHETIC | access::BRIDGE) == 0
                    && m.name != "<clinit>"
                    && m.signature
                        .as_deref()
                        .and_then(signature::parse_method_sig)
                        .or_else(|| signature::parse_method_sig(&m.descriptor))
                        .is_some()
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn method(&self, idx: usize) -> &CodeMethod {
        &self.class.methods[idx]
    }

    /// 反编译第 `idx` 个方法，返回 `{ ... }` 形式的方法体文本。
    ///
    /// `declared_names` 是声明头里写出的形参名（对齐到描述符的末尾，前面多出来的是编译器合成的）；
    /// 方法体里的类名按 `names` 渲染并登记需要的 import。右花括号前缩进 `indent` 层。
    pub fn method_body(
        &self,
        idx: usize,
        declared_names: Option<&[String]>,
        names: &mut NameTable,
        env: &dyn ClassEnv,
        indent: usize,
    ) -> Result<String, Fail> {
        let method = self.class.methods.get(idx).ok_or(Fail("方法下标越界"))?;
        if method.code.is_none() {
            return Err(Fail("没有方法体"));
        }
        let mut vars = Vars::default();
        let mut budget = Budget(budget_for(method));
        let mut labels: LabelId = 0;
        let n_params = translate::method_desc(&method.descriptor)?.0.len();
        let synthetic = declared_names.map_or(0, |d| n_params.saturating_sub(d.len()));
        let ast = {
            let mut cx = MethodCx {
                class: &self.class,
                method,
                env,
                vars: &mut vars,
                budget: &mut budget,
                labels: &mut labels,
                depth: 0,
                raw_ctor: false,
                synthetic_params: synthetic,
            };
            decompile_body(&mut cx)?
        };
        name_params(&mut vars, &ast.params, method, declared_names);
        vars::name_all(&ast.body, &mut vars, &ast.params);
        let text = {
            let scope = Scope::new(&self.class, &vars, &[]);
            let mut printer = Printer::new(names, env, scope);
            printer.braced(&ast.body, indent);
            printer.out
        };
        check_syntax(&text, method.name == "<init>")?;
        Ok(text)
    }

    /// 静态初始化块的方法体（`{ ... }`）。没有或者为空时返回 `None`。
    pub fn static_init(
        &self,
        names: &mut NameTable,
        env: &dyn ClassEnv,
        indent: usize,
    ) -> Option<Result<String, Fail>> {
        let idx = self
            .class
            .methods
            .iter()
            .position(|m| m.name == "<clinit>" && m.code.is_some())?;
        let result = self.method_body(idx, Some(&[]), names, env, indent);
        match result {
            Ok(text) if text == "{}" => None,
            other => Some(other),
        }
    }
}
