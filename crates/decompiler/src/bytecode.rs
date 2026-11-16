//! 字节码指令解码。

use crate::Fail;

/// 栈上值的类别（决定 `dup2` / `pop2` 这类指令的形态，也决定合成变量的类型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Int,
    Long,
    Float,
    Double,
    Ref,
}

impl Kind {
    pub fn is_wide(self) -> bool {
        matches!(self, Kind::Long | Kind::Double)
    }
}

/// 数组元素的读写指令的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayKind {
    Int,
    Long,
    Float,
    Double,
    Ref,
    /// `baload` / `bastore`：byte 或 boolean
    Byte,
    Char,
    Short,
}

impl ArrayKind {
    pub fn kind(self) -> Kind {
        match self {
            ArrayKind::Long => Kind::Long,
            ArrayKind::Float => Kind::Float,
            ArrayKind::Double => Kind::Double,
            ArrayKind::Ref => Kind::Ref,
            _ => Kind::Int,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    Ushr,
    And,
    Or,
    Xor,
}

/// 条件跳转的比较方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Eq,
    Ne,
    Lt,
    Ge,
    Gt,
    Le,
}

/// 基本类型转换的目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prim {
    Int,
    Long,
    Float,
    Double,
    Byte,
    Char,
    Short,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeKind {
    Virtual,
    Special,
    Static,
    Interface,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Nop,
    AConstNull,
    IConst(i32),
    LConst(i64),
    FConst(f32),
    DConst(f64),
    Ldc(u16),
    Load(Kind, u16),
    Store(Kind, u16),
    ArrayLoad(ArrayKind),
    ArrayStore(ArrayKind),
    Pop,
    Pop2,
    Dup,
    DupX1,
    DupX2,
    Dup2,
    Dup2X1,
    Dup2X2,
    Swap,
    Arith(Kind, Arith),
    Neg(Kind),
    Iinc(u16, i32),
    Convert(Kind, Prim),
    /// lcmp / fcmpl / fcmpg / dcmpl / dcmpg
    Compare(Kind),
    /// 和零比较（int）
    If(Cond, u32),
    IfCmp(Cond, u32),
    IfACmp(bool, u32),
    /// `true` 表示 ifnull
    IfNull(bool, u32),
    Goto(u32),
    Jsr,
    Ret,
    Switch {
        default: u32,
        cases: Vec<(i32, u32)>,
    },
    Return(Option<Kind>),
    GetStatic(u16),
    PutStatic(u16),
    GetField(u16),
    PutField(u16),
    Invoke(InvokeKind, u16),
    InvokeDynamic(u16),
    New(u16),
    /// `newarray` 的元素类型代码（4 = boolean … 11 = long）
    NewPrimArray(u8),
    ANewArray(u16),
    ArrayLength,
    AThrow,
    CheckCast(u16),
    InstanceOf(u16),
    MonitorEnter,
    MonitorExit,
    MultiANewArray(u16, u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Insn {
    pub pc: u32,
    /// 下一条指令的 pc
    pub next: u32,
    pub op: Op,
}

impl Insn {
    /// 跳转目标（不含顺序执行的下一条）。
    pub fn targets(&self) -> Vec<u32> {
        match &self.op {
            Op::If(_, t) | Op::IfCmp(_, t) | Op::IfACmp(_, t) | Op::IfNull(_, t) | Op::Goto(t) => {
                vec![*t]
            }
            Op::Switch { default, cases } => {
                let mut v = vec![*default];
                v.extend(cases.iter().map(|(_, t)| *t));
                v
            }
            _ => Vec::new(),
        }
    }

    /// 执行完之后不会顺序执行下一条。
    pub fn ends_flow(&self) -> bool {
        matches!(
            self.op,
            Op::Goto(_) | Op::Switch { .. } | Op::Return(_) | Op::AThrow | Op::Ret | Op::Jsr
        )
    }

    pub fn is_branch(&self) -> bool {
        matches!(
            self.op,
            Op::If(..) | Op::IfCmp(..) | Op::IfACmp(..) | Op::IfNull(..)
        ) || self.ends_flow()
    }
}

struct Bytes<'a> {
    code: &'a [u8],
    pos: usize,
}

impl Bytes<'_> {
    fn u8(&mut self) -> Result<u8, Fail> {
        let b = *self.code.get(self.pos).ok_or(Fail("字节码截断"))?;
        self.pos += 1;
        Ok(b)
    }
    fn i8(&mut self) -> Result<i8, Fail> {
        Ok(self.u8()? as i8)
    }
    fn u16(&mut self) -> Result<u16, Fail> {
        Ok(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }
    fn i16(&mut self) -> Result<i16, Fail> {
        Ok(self.u16()? as i16)
    }
    fn i32(&mut self) -> Result<i32, Fail> {
        Ok(i32::from_be_bytes([
            self.u8()?,
            self.u8()?,
            self.u8()?,
            self.u8()?,
        ]))
    }
}

fn target(pc: u32, offset: i32, len: usize) -> Result<u32, Fail> {
    let t = pc as i64 + offset as i64;
    if t < 0 || t >= len as i64 {
        return Err(Fail("跳转目标越界"));
    }
    Ok(t as u32)
}

const KINDS: [Kind; 5] = [Kind::Int, Kind::Long, Kind::Float, Kind::Double, Kind::Ref];

/// 把整段字节码解码成指令序列。
pub fn decode(code: &[u8]) -> Result<Vec<Insn>, Fail> {
    let mut insns = Vec::new();
    let mut b = Bytes { code, pos: 0 };
    while b.pos < code.len() {
        let pc = b.pos as u32;
        let opcode = b.u8()?;
        let op = match opcode {
            0x00 => Op::Nop,
            0x01 => Op::AConstNull,
            0x02..=0x08 => Op::IConst(opcode as i32 - 0x03),
            0x09 | 0x0a => Op::LConst((opcode - 0x09) as i64),
            0x0b..=0x0d => Op::FConst((opcode - 0x0b) as f32),
            0x0e | 0x0f => Op::DConst((opcode - 0x0e) as f64),
            0x10 => Op::IConst(b.i8()? as i32),
            0x11 => Op::IConst(b.i16()? as i32),
            0x12 => Op::Ldc(b.u8()? as u16),
            0x13 | 0x14 => Op::Ldc(b.u16()?),
            0x15..=0x19 => Op::Load(KINDS[(opcode - 0x15) as usize], b.u8()? as u16),
            0x1a..=0x2d => {
                let i = opcode - 0x1a;
                Op::Load(KINDS[(i / 4) as usize], (i % 4) as u16)
            }
            0x2e..=0x35 => Op::ArrayLoad(array_kind(opcode - 0x2e)),
            0x36..=0x3a => Op::Store(KINDS[(opcode - 0x36) as usize], b.u8()? as u16),
            0x3b..=0x4e => {
                let i = opcode - 0x3b;
                Op::Store(KINDS[(i / 4) as usize], (i % 4) as u16)
            }
            0x4f..=0x56 => Op::ArrayStore(array_kind(opcode - 0x4f)),
            0x57 => Op::Pop,
            0x58 => Op::Pop2,
            0x59 => Op::Dup,
            0x5a => Op::DupX1,
            0x5b => Op::DupX2,
            0x5c => Op::Dup2,
            0x5d => Op::Dup2X1,
            0x5e => Op::Dup2X2,
            0x5f => Op::Swap,
            0x60..=0x73 => {
                let i = opcode - 0x60;
                let arith =
                    [Arith::Add, Arith::Sub, Arith::Mul, Arith::Div, Arith::Rem][(i / 4) as usize];
                Op::Arith(KINDS[(i % 4) as usize], arith)
            }
            0x74..=0x77 => Op::Neg(KINDS[(opcode - 0x74) as usize]),
            0x78..=0x83 => {
                let i = opcode - 0x78;
                let arith = [
                    Arith::Shl,
                    Arith::Shr,
                    Arith::Ushr,
                    Arith::And,
                    Arith::Or,
                    Arith::Xor,
                ][(i / 2) as usize];
                Op::Arith(if i % 2 == 0 { Kind::Int } else { Kind::Long }, arith)
            }
            0x84 => {
                let slot = b.u8()? as u16;
                Op::Iinc(slot, b.i8()? as i32)
            }
            0x85 => Op::Convert(Kind::Int, Prim::Long),
            0x86 => Op::Convert(Kind::Int, Prim::Float),
            0x87 => Op::Convert(Kind::Int, Prim::Double),
            0x88 => Op::Convert(Kind::Long, Prim::Int),
            0x89 => Op::Convert(Kind::Long, Prim::Float),
            0x8a => Op::Convert(Kind::Long, Prim::Double),
            0x8b => Op::Convert(Kind::Float, Prim::Int),
            0x8c => Op::Convert(Kind::Float, Prim::Long),
            0x8d => Op::Convert(Kind::Float, Prim::Double),
            0x8e => Op::Convert(Kind::Double, Prim::Int),
            0x8f => Op::Convert(Kind::Double, Prim::Long),
            0x90 => Op::Convert(Kind::Double, Prim::Float),
            0x91 => Op::Convert(Kind::Int, Prim::Byte),
            0x92 => Op::Convert(Kind::Int, Prim::Char),
            0x93 => Op::Convert(Kind::Int, Prim::Short),
            0x94 => Op::Compare(Kind::Long),
            0x95 | 0x96 => Op::Compare(Kind::Float),
            0x97 | 0x98 => Op::Compare(Kind::Double),
            0x99..=0x9e => {
                let off = b.i16()? as i32;
                Op::If(cond(opcode - 0x99), target(pc, off, code.len())?)
            }
            0x9f..=0xa4 => {
                let off = b.i16()? as i32;
                Op::IfCmp(cond(opcode - 0x9f), target(pc, off, code.len())?)
            }
            0xa5 | 0xa6 => {
                let off = b.i16()? as i32;
                Op::IfACmp(opcode == 0xa5, target(pc, off, code.len())?)
            }
            0xa7 => {
                let off = b.i16()? as i32;
                Op::Goto(target(pc, off, code.len())?)
            }
            0xa8 => {
                b.i16()?;
                Op::Jsr
            }
            0xa9 => {
                b.u8()?;
                Op::Ret
            }
            0xaa => {
                while !b.pos.is_multiple_of(4) {
                    b.u8()?;
                }
                let default = target(pc, b.i32()?, code.len())?;
                let low = b.i32()?;
                let high = b.i32()?;
                if high < low || (high as i64 - low as i64) > 65535 {
                    return Err(Fail("tableswitch 范围不合法"));
                }
                let mut cases = Vec::new();
                for v in low..=high {
                    cases.push((v, target(pc, b.i32()?, code.len())?));
                }
                Op::Switch { default, cases }
            }
            0xab => {
                while !b.pos.is_multiple_of(4) {
                    b.u8()?;
                }
                let default = target(pc, b.i32()?, code.len())?;
                let n = b.i32()?;
                if !(0..=65535).contains(&n) {
                    return Err(Fail("lookupswitch 项数不合法"));
                }
                let mut cases = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let key = b.i32()?;
                    cases.push((key, target(pc, b.i32()?, code.len())?));
                }
                Op::Switch { default, cases }
            }
            0xac..=0xb0 => Op::Return(Some(KINDS[(opcode - 0xac) as usize])),
            0xb1 => Op::Return(None),
            0xb2 => Op::GetStatic(b.u16()?),
            0xb3 => Op::PutStatic(b.u16()?),
            0xb4 => Op::GetField(b.u16()?),
            0xb5 => Op::PutField(b.u16()?),
            0xb6 => Op::Invoke(InvokeKind::Virtual, b.u16()?),
            0xb7 => Op::Invoke(InvokeKind::Special, b.u16()?),
            0xb8 => Op::Invoke(InvokeKind::Static, b.u16()?),
            0xb9 => {
                let idx = b.u16()?;
                b.u8()?;
                b.u8()?;
                Op::Invoke(InvokeKind::Interface, idx)
            }
            0xba => {
                let idx = b.u16()?;
                b.u8()?;
                b.u8()?;
                Op::InvokeDynamic(idx)
            }
            0xbb => Op::New(b.u16()?),
            0xbc => Op::NewPrimArray(b.u8()?),
            0xbd => Op::ANewArray(b.u16()?),
            0xbe => Op::ArrayLength,
            0xbf => Op::AThrow,
            0xc0 => Op::CheckCast(b.u16()?),
            0xc1 => Op::InstanceOf(b.u16()?),
            0xc2 => Op::MonitorEnter,
            0xc3 => Op::MonitorExit,
            0xc4 => {
                let wide_op = b.u8()?;
                let slot = b.u16()?;
                match wide_op {
                    0x15..=0x19 => Op::Load(KINDS[(wide_op - 0x15) as usize], slot),
                    0x36..=0x3a => Op::Store(KINDS[(wide_op - 0x36) as usize], slot),
                    0x84 => Op::Iinc(slot, b.i16()? as i32),
                    0xa9 => Op::Ret,
                    _ => return Err(Fail("wide 后面跟了不支持的指令")),
                }
            }
            0xc5 => {
                let idx = b.u16()?;
                let dims = b.u8()?;
                Op::MultiANewArray(idx, dims)
            }
            0xc6 | 0xc7 => {
                let off = b.i16()? as i32;
                Op::IfNull(opcode == 0xc6, target(pc, off, code.len())?)
            }
            0xc8 => Op::Goto(target(pc, b.i32()?, code.len())?),
            0xc9 => {
                b.i32()?;
                Op::Jsr
            }
            _ => return Err(Fail("未知的操作码")),
        };
        insns.push(Insn {
            pc,
            next: b.pos as u32,
            op,
        });
    }
    Ok(insns)
}

fn array_kind(i: u8) -> ArrayKind {
    [
        ArrayKind::Int,
        ArrayKind::Long,
        ArrayKind::Float,
        ArrayKind::Double,
        ArrayKind::Ref,
        ArrayKind::Byte,
        ArrayKind::Char,
        ArrayKind::Short,
    ][i as usize]
}

fn cond(i: u8) -> Cond {
    [Cond::Eq, Cond::Ne, Cond::Lt, Cond::Ge, Cond::Gt, Cond::Le][i as usize]
}
