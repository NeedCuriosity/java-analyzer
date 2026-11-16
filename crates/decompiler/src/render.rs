//! 语句树 → Java 源码文本。表达式按优先级只加必要的括号。

use std::fmt::Write as _;

use classfile::{access, code::CodeClass, signature::JType};
use rustc_hash::FxHashSet;

use crate::{bytecode::InvokeKind, ir::*, names::NameTable, types::owner_ty, ClassEnv};

/// 渲染时的上下文：当前类、变量表、匿名类捕获的外层值。
pub struct Scope<'a> {
    pub class: &'a CodeClass,
    pub vars: &'a Vars,
    pub captured: &'a [Expr],
    /// 方法里所有局部变量的名字（同名字段要写 `this.`）
    pub var_names: FxHashSet<String>,
}

impl<'a> Scope<'a> {
    pub fn new(class: &'a CodeClass, vars: &'a Vars, captured: &'a [Expr]) -> Scope<'a> {
        let var_names = vars.vars.iter().filter_map(|v| v.name.clone()).collect();
        Scope {
            class,
            vars,
            captured,
            var_names,
        }
    }
}

pub struct Printer<'p, 'a> {
    pub names: &'p mut NameTable,
    pub env: &'p dyn ClassEnv,
    pub out: String,
    pub scopes: Vec<Scope<'a>>,
    indent: usize,
}

const PREC_ASSIGN: u8 = 0;
const PREC_COND: u8 = 1;
const PREC_UNARY: u8 = 13;
const PREC_POSTFIX: u8 = 14;
const PREC_PRIMARY: u8 = 15;

fn prec(e: &Expr) -> u8 {
    match e {
        Expr::Assign(..) | Expr::CompoundAssign(..) | Expr::Lambda { .. } => PREC_ASSIGN,
        Expr::Cond(..) => PREC_COND,
        Expr::Binary(op, ..) => op.precedence(),
        Expr::InstanceOf(..) => 9,
        Expr::Unary(..) | Expr::Cast(..) => PREC_UNARY,
        Expr::IncDec { prefix: true, .. } => PREC_UNARY,
        Expr::IncDec { prefix: false, .. } => PREC_POSTFIX,
        Expr::Lit(Lit::Int(v)) if *v < 0 => PREC_UNARY,
        Expr::Lit(Lit::Long(v)) if *v < 0 => PREC_UNARY,
        Expr::Lit(Lit::Float(b)) if f32::from_bits(*b).is_sign_negative() => PREC_UNARY,
        Expr::Lit(Lit::Double(b)) if f64::from_bits(*b).is_sign_negative() => PREC_UNARY,
        // `new int[n]` 后面不能直接跟 `[i]`
        Expr::NewArray { .. } => PREC_POSTFIX,
        _ => PREC_PRIMARY,
    }
}

pub fn escape_char(c: u16, quote: char, out: &mut String) {
    match c {
        0x08 => out.push_str("\\b"),
        0x09 => out.push_str("\\t"),
        0x0A => out.push_str("\\n"),
        0x0C => out.push_str("\\f"),
        0x0D => out.push_str("\\r"),
        0x5C => out.push_str("\\\\"),
        0x22 if quote == '"' => out.push_str("\\\""),
        0x27 if quote == '\'' => out.push_str("\\'"),
        0..=0x1F => {
            let _ = write!(out, "\\{c:03o}");
        }
        0x20..=0x7E => out.push(c as u8 as char),
        _ => {
            let special = (0x7F..=0x9F).contains(&c)
                || (0xD800..=0xDFFF).contains(&c)
                || (0x2000..=0x200F).contains(&c)
                || (0x2028..=0x202F).contains(&c)
                || c == 0xFEFF
                || c >= 0xFFF0
                || c == 0x00AD;
            match char::from_u32(c as u32) {
                Some(ch) if !special => out.push(ch),
                _ => {
                    let _ = write!(out, "\\u{c:04X}");
                }
            }
        }
    }
}

pub fn string_literal(units: &[u16]) -> String {
    let mut out = String::from("\"");
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        // 成对的代理项按字符输出
        if (0xD800..0xDC00).contains(&u)
            && i + 1 < units.len()
            && (0xDC00..0xE000).contains(&units[i + 1])
        {
            let cp = 0x10000 + (((u as u32) - 0xD800) << 10) + ((units[i + 1] as u32) - 0xDC00);
            match char::from_u32(cp) {
                Some(ch) => out.push(ch),
                None => {
                    let _ = write!(out, "\\u{u:04X}\\u{:04X}", units[i + 1]);
                }
            }
            i += 2;
            continue;
        }
        escape_char(u, '"', &mut out);
        i += 1;
    }
    out.push('"');
    out
}

fn float_text(v: f32) -> String {
    if v.is_nan() {
        return "Float.NaN".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "Float.POSITIVE_INFINITY"
        } else {
            "Float.NEGATIVE_INFINITY"
        }
        .to_owned();
    }
    format!("{v:?}F")
}

fn double_text(v: f64) -> String {
    if v.is_nan() {
        return "Double.NaN".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "Double.POSITIVE_INFINITY"
        } else {
            "Double.NEGATIVE_INFINITY"
        }
        .to_owned();
    }
    let s = format!("{v:?}");
    if s.contains(['.', 'e', 'E']) {
        s
    } else {
        format!("{s}.0")
    }
}

pub fn label_name(l: LabelId) -> String {
    format!("label{l}")
}

impl<'p, 'a> Printer<'p, 'a> {
    pub fn new(names: &'p mut NameTable, env: &'p dyn ClassEnv, scope: Scope<'a>) -> Self {
        Printer {
            names,
            env,
            out: String::new(),
            scopes: vec![scope],
            indent: 0,
        }
    }

    fn scope(&self) -> &Scope<'a> {
        self.scopes.last().unwrap()
    }

    fn pad(&mut self, indent: usize) {
        for _ in 0..indent {
            self.out.push_str("    ");
        }
    }

    pub fn ty(&mut self, ty: &JType) -> String {
        self.names.ty(ty, self.env)
    }

    fn class_name(&mut self, binary: &str) -> String {
        self.names.class_name(binary, self.env)
    }

    fn var_ty(&mut self, v: VarId) -> String {
        let info = self.scope().vars.get(v);
        let ty = info.ty.clone().unwrap_or_else(|| ty_of_kind(info.kind));
        self.ty(&ty)
    }

    fn var_name(&self, v: VarId) -> String {
        self.scope().vars.name(v).to_owned()
    }

    // -----------------------------------------------------------------------
    // 表达式

    pub fn expr(&mut self, e: &'a Expr, min: u8) {
        let p = prec(e);
        let paren = p < min;
        if paren {
            self.out.push('(');
        }
        self.expr_inner(e);
        if paren {
            self.out.push(')');
        }
    }

    fn args(&mut self, args: &'a [Expr]) {
        self.out.push('(');
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.expr(a, PREC_ASSIGN);
        }
        self.out.push(')');
    }

    fn lit(&mut self, l: &Lit) {
        match l {
            Lit::Int(v) => {
                let _ = write!(self.out, "{v}");
            }
            Lit::Long(v) => {
                let _ = write!(self.out, "{v}L");
            }
            Lit::Float(b) => self.out.push_str(&float_text(f32::from_bits(*b))),
            Lit::Double(b) => self.out.push_str(&double_text(f64::from_bits(*b))),
            Lit::Bool(b) => self.out.push_str(if *b { "true" } else { "false" }),
            Lit::Char(c) => {
                self.out.push('\'');
                let mut s = String::new();
                escape_char(*c, '\'', &mut s);
                self.out.push_str(&s);
                self.out.push('\'');
            }
            Lit::Str(units) => self.out.push_str(&string_literal(units)),
            Lit::Null => self.out.push_str("null"),
            Lit::Class(t) => {
                let t = self.ty(&erase(t));
                self.out.push_str(&t);
                self.out.push_str(".class");
            }
        }
    }

    fn is_interface_of_current(&self, owner: &str) -> bool {
        self.scope().class.interfaces.iter().any(|i| i == owner)
    }

    fn expr_inner(&mut self, e: &'a Expr) {
        match e {
            Expr::Lit(l) => self.lit(l),
            Expr::Var(v) => {
                let n = self.var_name(*v);
                self.out.push_str(&n);
            }
            Expr::This => self.out.push_str("this"),
            Expr::OuterThis(c) => {
                let n = self.class_name(c);
                self.out.push_str(&n);
                self.out.push_str(".this");
            }
            Expr::Caught(_) | Expr::Uninit(..) => self.out.push_str("null"),
            Expr::Captured(k) => {
                let cap: &'a [Expr] = self.scope().captured;
                match cap.get(*k as usize) {
                    Some(o) if self.scopes.len() > 1 => {
                        let inner = self.scopes.pop().unwrap();
                        // 外层的 this 在匿名类体里要写成 `Outer.this`
                        if *o == Expr::This {
                            let c = self.scope().class.name.clone();
                            let n = self.class_name(&c);
                            self.out.push_str(&n);
                            self.out.push_str(".this");
                        } else {
                            self.expr(o, PREC_PRIMARY);
                        }
                        self.scopes.push(inner);
                    }
                    _ => self.out.push_str("null"),
                }
            }
            Expr::Field {
                target,
                owner,
                name,
                ..
            } => match target {
                None => {
                    let cur = self.scope().class.name.clone();
                    if *owner == cur && !self.scope().var_names.contains(name) {
                        self.out.push_str(name);
                    } else {
                        let o = self.class_name(owner);
                        self.out.push_str(&o);
                        self.out.push('.');
                        self.out.push_str(name);
                    }
                }
                Some(t) => {
                    if **t == Expr::This && !self.scope().var_names.contains(name) {
                        self.out.push_str(name);
                    } else {
                        self.expr(t, PREC_PRIMARY);
                        self.out.push('.');
                        self.out.push_str(name);
                    }
                }
            },
            Expr::Invoke {
                kind,
                target,
                owner,
                name,
                args,
                ..
            } => {
                let cur = self.scope().class.name.clone();
                if name == "<init>" {
                    self.out
                        .push_str(if *owner == cur { "this" } else { "super" });
                    self.args(args);
                    return;
                }
                match target {
                    None => {
                        if *owner != cur {
                            let o = self.ty(&owner_ty(owner));
                            self.out.push_str(&o);
                            self.out.push('.');
                        }
                    }
                    Some(t) if **t == Expr::This => {
                        if *kind == InvokeKind::Special && *owner != cur {
                            if self.is_interface_of_current(owner) {
                                let o = self.class_name(owner);
                                self.out.push_str(&o);
                                self.out.push('.');
                            }
                            self.out.push_str("super.");
                        }
                    }
                    Some(t) => {
                        self.expr(t, PREC_PRIMARY);
                        self.out.push('.');
                    }
                }
                self.out.push_str(name);
                self.args(args);
            }
            Expr::New { class, args, .. } => {
                let mut args: &'a [Expr] = args;
                // 内部类的构造器第一个实参是外层实例，源码里不写
                let cur = self.scope().class.name.clone();
                if args.first() == Some(&Expr::This) {
                    let inner_of_cur = self.scope().class.inner_classes.iter().any(|ic| {
                        ic.inner == *class
                            && ic.outer.as_deref() == Some(cur.as_str())
                            && ic.access & access::STATIC == 0
                    });
                    if inner_of_cur {
                        args = &args[1..];
                    }
                }
                self.out.push_str("new ");
                let c = self.class_name(class);
                self.out.push_str(&c);
                self.args(args);
            }
            Expr::AnonNew(anon) => self.anon(anon),
            Expr::NewArray {
                elem,
                dims,
                extra,
                init,
            } => {
                self.out.push_str("new ");
                let t = self.ty(&erase(elem));
                self.out.push_str(&t);
                match init {
                    Some(values) => {
                        for _ in 0..=*extra {
                            self.out.push_str("[]");
                        }
                        self.out.push('{');
                        for (i, v) in values.iter().enumerate() {
                            if i > 0 {
                                self.out.push_str(", ");
                            }
                            self.expr(v, PREC_ASSIGN);
                        }
                        self.out.push('}');
                    }
                    None => {
                        for d in dims {
                            self.out.push('[');
                            self.expr(d, PREC_ASSIGN);
                            self.out.push(']');
                        }
                        for _ in 0..*extra {
                            self.out.push_str("[]");
                        }
                    }
                }
            }
            Expr::ArrayLen(a) => {
                self.expr(a, PREC_PRIMARY);
                self.out.push_str(".length");
            }
            Expr::Index(a, i) => {
                self.expr(a, PREC_PRIMARY);
                self.out.push('[');
                self.expr(i, PREC_ASSIGN);
                self.out.push(']');
            }
            Expr::Unary(op, x) => {
                let sym = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                    UnOp::BitNot => "~",
                };
                self.out.push_str(sym);
                // `- -x`、`-(-1)`
                let needs_paren = *op == UnOp::Neg
                    && (prec(x) == PREC_UNARY || matches!(**x, Expr::IncDec { prefix: true, .. }));
                if needs_paren {
                    self.out.push('(');
                    self.expr(x, PREC_ASSIGN);
                    self.out.push(')');
                } else {
                    self.expr(x, PREC_UNARY);
                }
            }
            Expr::Binary(op, a, b) => {
                let p = op.precedence();
                // `a || b && c` 加括号更易读
                let left_min = if *op == BinOp::LOr && matches!(**a, Expr::Binary(BinOp::LAnd, ..))
                {
                    p + 2
                } else {
                    p
                };
                let right_min = if *op == BinOp::LOr && matches!(**b, Expr::Binary(BinOp::LAnd, ..))
                {
                    p + 2
                } else {
                    p + 1
                };
                self.expr(a, left_min);
                self.out.push(' ');
                self.out.push_str(op.text());
                self.out.push(' ');
                self.expr(b, right_min);
            }
            Expr::Cmp(a, b) => {
                // 不应出现：结构化之后的检查会拒绝
                self.expr(a, PREC_PRIMARY);
                self.out.push_str(" - ");
                self.expr(b, PREC_PRIMARY);
            }
            Expr::Cast(t, x) => {
                self.out.push('(');
                let ts = self.ty(t);
                self.out.push_str(&ts);
                self.out.push_str(") ");
                // 引用类型的强制转换后面不能直接跟 `+` / `-` 开头的表达式
                let reference = !matches!(t, JType::Base(_));
                let starts_with_sign = matches!(**x, Expr::Unary(UnOp::Neg, _))
                    || prec(x) == PREC_UNARY && matches!(**x, Expr::Lit(_));
                if reference && starts_with_sign {
                    self.out.push('(');
                    self.expr(x, PREC_ASSIGN);
                    self.out.push(')');
                } else {
                    self.expr(x, PREC_UNARY);
                }
            }
            Expr::InstanceOf(x, t) => {
                self.expr(x, 9);
                self.out.push_str(" instanceof ");
                let ts = self.ty(&erase(t));
                self.out.push_str(&ts);
            }
            Expr::Cond(c, a, b) => {
                self.expr(c, PREC_COND + 2);
                self.out.push_str(" ? ");
                self.expr(a, PREC_COND + 1);
                self.out.push_str(" : ");
                self.expr(b, PREC_COND);
            }
            Expr::Assign(l, r) => {
                self.expr(l, PREC_POSTFIX);
                self.out.push_str(" = ");
                self.expr(r, PREC_ASSIGN);
            }
            Expr::CompoundAssign(op, l, r) => {
                self.expr(l, PREC_POSTFIX);
                self.out.push(' ');
                self.out.push_str(op.text());
                self.out.push_str("= ");
                self.expr(r, PREC_ASSIGN);
            }
            Expr::IncDec {
                target,
                inc,
                prefix,
            } => {
                let sym = if *inc { "++" } else { "--" };
                if *prefix {
                    self.out.push_str(sym);
                    self.expr(target, PREC_POSTFIX);
                } else {
                    self.expr(target, PREC_POSTFIX);
                    self.out.push_str(sym);
                }
            }
            Expr::Lambda { params, body } => {
                if params.len() == 1 {
                    let n = self.var_name(params[0]);
                    self.out.push_str(&n);
                } else {
                    self.out.push('(');
                    for (i, p) in params.iter().enumerate() {
                        if i > 0 {
                            self.out.push_str(", ");
                        }
                        let n = self.var_name(*p);
                        self.out.push_str(&n);
                    }
                    self.out.push(')');
                }
                self.out.push_str(" -> ");
                match &**body {
                    LambdaBody::Expr(b) => self.expr(b, PREC_ASSIGN),
                    LambdaBody::Block(stmts) => {
                        let indent = self.indent;
                        self.braced(stmts, indent);
                    }
                }
            }
            Expr::MethodRef { target, name } => {
                match target {
                    MethodRefTarget::Type(t) => {
                        let ts = self.ty(&erase(t));
                        self.out.push_str(&ts);
                    }
                    MethodRefTarget::Expr(x) => self.expr(x, PREC_PRIMARY),
                }
                self.out.push_str("::");
                self.out.push_str(name);
            }
        }
    }

    fn anon(&mut self, anon: &'a AnonClass) {
        self.out.push_str("new ");
        let t = self.ty(&anon.super_ty);
        self.out.push_str(&t);
        self.args(&anon.super_args);
        self.out.push_str(" {\n");
        let indent = self.indent;
        crate::anon::render_members(self, anon, indent + 1);
        self.pad(indent);
        self.out.push('}');
    }

    // -----------------------------------------------------------------------
    // 语句

    /// `{ ... }`，左花括号接在当前行末尾，右花括号单独一行。
    pub fn braced(&mut self, stmts: &'a [Stmt], indent: usize) {
        if stmts.is_empty() {
            self.out.push_str("{}");
            return;
        }
        self.out.push_str("{\n");
        for s in stmts {
            self.stmt(s, indent + 1);
        }
        self.pad(indent);
        self.out.push('}');
    }

    fn local_decl(&mut self, var: VarId, init: Option<&'a Expr>) {
        let info = self.scope().vars.get(var);
        let ty = info.ty.clone().unwrap_or_else(|| ty_of_kind(info.kind));
        let ts = self.ty(&ty);
        let name = self.var_name(var);
        let _ = write!(self.out, "{ts} {name}");
        if let Some(e) = init {
            self.out.push_str(" = ");
            // 声明类型带类型实参、右边是同一个类的 new：写成 `<>`
            match (&ty, e) {
                (
                    JType::Class { name, args },
                    Expr::New {
                        class, args: cargs, ..
                    },
                ) if !args.is_empty() && (name == class || self.env.is_generic(class)) => {
                    let c = self.class_name(class);
                    let _ = write!(self.out, "new {c}<>");
                    self.args(cargs);
                }
                _ => self.expr(e, PREC_ASSIGN),
            }
        }
    }

    pub fn stmt(&mut self, s: &'a Stmt, indent: usize) {
        self.indent = indent;
        self.pad(indent);
        match s {
            Stmt::Expr(e) => {
                self.expr(e, PREC_ASSIGN);
                self.out.push_str(";\n");
            }
            Stmt::Local { var, init } => {
                self.local_decl(*var, init.as_ref());
                self.out.push_str(";\n");
            }
            Stmt::Return(e) => {
                self.out.push_str("return");
                if let Some(e) = e {
                    self.out.push(' ');
                    self.expr(e, PREC_ASSIGN);
                }
                self.out.push_str(";\n");
            }
            Stmt::Throw(e) => {
                self.out.push_str("throw ");
                self.expr(e, PREC_ASSIGN);
                self.out.push_str(";\n");
            }
            Stmt::If { .. } => {
                self.if_chain(s, indent);
                self.out.push('\n');
            }
            Stmt::Loop { label, kind, body } => {
                if let Some(l) = label {
                    let _ = write!(self.out, "{}: ", label_name(*l));
                }
                match kind {
                    LoopKind::While(c) => {
                        self.out.push_str("while (");
                        self.expr(c, PREC_ASSIGN);
                        self.out.push_str(") ");
                        self.braced(body, indent);
                    }
                    LoopKind::DoWhile(c) => {
                        self.out.push_str("do ");
                        self.braced(body, indent);
                        self.out.push_str(" while (");
                        self.indent = indent;
                        self.expr(c, PREC_ASSIGN);
                        self.out.push_str(");");
                    }
                    LoopKind::For { init, cond, update } => {
                        self.out.push_str("for (");
                        for (i, st) in init.iter().enumerate() {
                            if i > 0 {
                                self.out.push_str(", ");
                            }
                            match st {
                                Stmt::Local { var, init } => self.local_decl(*var, init.as_ref()),
                                Stmt::Expr(e) => self.expr(e, PREC_ASSIGN),
                                _ => {}
                            }
                        }
                        self.out.push_str("; ");
                        if let Some(c) = cond {
                            self.expr(c, PREC_ASSIGN);
                        }
                        self.out.push_str("; ");
                        for (i, u) in update.iter().enumerate() {
                            if i > 0 {
                                self.out.push_str(", ");
                            }
                            self.expr(u, PREC_ASSIGN);
                        }
                        self.out.push_str(") ");
                        self.braced(body, indent);
                    }
                    LoopKind::ForEach { var, iterable } => {
                        let t = self.var_ty(*var);
                        let n = self.var_name(*var);
                        let _ = write!(self.out, "for ({t} {n} : ");
                        self.expr(iterable, PREC_ASSIGN);
                        self.out.push_str(") ");
                        self.braced(body, indent);
                    }
                }
                self.out.push('\n');
            }
            Stmt::Switch { label, expr, cases } => {
                if let Some(l) = label {
                    let _ = write!(self.out, "{}: ", label_name(*l));
                }
                self.out.push_str("switch (");
                self.expr(expr, PREC_ASSIGN);
                self.out.push_str(") {\n");
                for c in cases {
                    for l in &c.labels {
                        self.pad(indent + 1);
                        match l {
                            None => self.out.push_str("default:\n"),
                            Some(CaseLabel::Int(v)) => {
                                let _ = writeln!(self.out, "case {v}:");
                            }
                            Some(CaseLabel::Char(ch)) => {
                                let mut s = String::new();
                                escape_char(*ch, '\'', &mut s);
                                let _ = writeln!(self.out, "case '{s}':");
                            }
                        }
                    }
                    for st in &c.body {
                        self.stmt(st, indent + 2);
                    }
                }
                self.pad(indent);
                self.out.push_str("}\n");
            }
            Stmt::Try {
                body,
                catches,
                finally,
            } => {
                self.out.push_str("try ");
                self.braced(body, indent);
                for c in catches {
                    self.out.push_str(" catch (");
                    if c.types.is_empty() {
                        let t = self.class_name("java/lang/Throwable");
                        self.out.push_str(&t);
                    } else {
                        for (i, t) in c.types.iter().enumerate() {
                            if i > 0 {
                                self.out.push_str(" | ");
                            }
                            let t = self.class_name(t);
                            self.out.push_str(&t);
                        }
                    }
                    let n = self.var_name(c.var);
                    let _ = write!(self.out, " {n}) ");
                    self.indent = indent;
                    self.braced(&c.body, indent);
                }
                if let Some(f) = finally {
                    self.out.push_str(" finally ");
                    self.braced(f, indent);
                }
                self.out.push('\n');
            }
            Stmt::Sync { lock, body } => {
                self.out.push_str("synchronized (");
                self.expr(lock, PREC_ASSIGN);
                self.out.push_str(") ");
                self.braced(body, indent);
                self.out.push('\n');
            }
            Stmt::Break(l) => {
                self.out.push_str("break");
                if let Some(l) = l {
                    let _ = write!(self.out, " {}", label_name(*l));
                }
                self.out.push_str(";\n");
            }
            Stmt::Continue(l) => {
                self.out.push_str("continue");
                if let Some(l) = l {
                    let _ = write!(self.out, " {}", label_name(*l));
                }
                self.out.push_str(";\n");
            }
            Stmt::Assert { cond, msg } => {
                self.out.push_str("assert ");
                self.expr(cond, PREC_ASSIGN);
                if let Some(m) = msg {
                    self.out.push_str(" : ");
                    self.expr(m, PREC_ASSIGN);
                }
                self.out.push_str(";\n");
            }
            Stmt::MonitorEnter(_) | Stmt::MonitorExit(_) => {
                // 结构化之后的检查会拒绝；这里只保证语法合法
                self.out.push_str(";\n");
            }
        }
    }

    fn if_chain(&mut self, s: &'a Stmt, indent: usize) {
        let Stmt::If { cond, then, els } = s else {
            return;
        };
        self.out.push_str("if (");
        self.indent = indent;
        self.expr(cond, PREC_ASSIGN);
        self.out.push_str(") ");
        self.braced(then, indent);
        match els.as_slice() {
            [] => {}
            [inner @ Stmt::If { .. }] => {
                self.out.push_str(" else ");
                self.if_chain(inner, indent);
            }
            _ => {
                self.out.push_str(" else ");
                self.braced(els, indent);
            }
        }
    }
}
