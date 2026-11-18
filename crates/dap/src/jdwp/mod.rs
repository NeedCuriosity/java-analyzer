//! Java Debug Wire Protocol：包的编解码、连接和常用命令。
//!
//! 包格式（大端）：
//! - 命令包：`length:u32 id:u32 flags:u8 command_set:u8 command:u8 data...`
//! - 回复包：`length:u32 id:u32 flags:u8(0x80) error_code:u16 data...`
//!
//! 对象、类型、方法、字段、栈帧的 ID 长度由 `VirtualMachine.IDSizes` 决定（HotSpot 上都是 8）。

mod connection;
mod vm;

use std::{
    fmt,
    io::{self, Read},
};

pub use connection::{reply_result, Connection, JdwpEvent, PendingReply, Reply};
pub use vm::{
    Event, EventSet, FieldInfo, Invoke, LineTable, MethodInfo, Modifier, StepDepth, Variable, Vm,
    CLASS_STATUS_PREPARED,
};

pub const HEADER_LEN: usize = 11;
pub const FLAG_REPLY: u8 = 0x80;
/// 单个包允许的最大长度，防止对端发来的垃圾长度导致巨量分配
const MAX_PACKET_LEN: usize = 256 * 1024 * 1024;

pub mod tag {
    pub const ARRAY: u8 = b'[';
    pub const BYTE: u8 = b'B';
    pub const CHAR: u8 = b'C';
    pub const OBJECT: u8 = b'L';
    pub const FLOAT: u8 = b'F';
    pub const DOUBLE: u8 = b'D';
    pub const INT: u8 = b'I';
    pub const LONG: u8 = b'J';
    pub const SHORT: u8 = b'S';
    pub const VOID: u8 = b'V';
    pub const BOOLEAN: u8 = b'Z';
    pub const STRING: u8 = b's';
    pub const THREAD: u8 = b't';
    pub const THREAD_GROUP: u8 = b'g';
    pub const CLASS_LOADER: u8 = b'l';
    pub const CLASS_OBJECT: u8 = b'c';

    /// 基本类型的值在数组区域里不带标签
    pub fn is_primitive(tag: u8) -> bool {
        matches!(
            tag,
            BYTE | CHAR | FLOAT | DOUBLE | INT | LONG | SHORT | BOOLEAN | VOID
        )
    }
}

pub mod event_kind {
    pub const SINGLE_STEP: u8 = 1;
    pub const BREAKPOINT: u8 = 2;
    pub const EXCEPTION: u8 = 4;
    pub const THREAD_START: u8 = 6;
    pub const THREAD_DEATH: u8 = 7;
    pub const CLASS_PREPARE: u8 = 8;
    pub const CLASS_UNLOAD: u8 = 9;
    pub const VM_START: u8 = 90;
    pub const VM_DEATH: u8 = 99;
}

pub mod suspend_policy {
    pub const NONE: u8 = 0;
    pub const EVENT_THREAD: u8 = 1;
    pub const ALL: u8 = 2;
}

pub mod error_code {
    pub const INVALID_THREAD: u16 = 10;
    pub const THREAD_NOT_SUSPENDED: u16 = 13;
    pub const INVALID_OBJECT: u16 = 20;
    pub const INVALID_CLASS: u16 = 21;
    pub const ABSENT_INFORMATION: u16 = 101;
    pub const VM_DEAD: u16 = 112;
}

/// JDWP 回复里的错误码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JdwpError(pub u16);

impl fmt::Display for JdwpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.0 {
            10 => "INVALID_THREAD",
            11 => "INVALID_THREAD_GROUP",
            13 => "THREAD_NOT_SUSPENDED",
            20 => "INVALID_OBJECT",
            21 => "INVALID_CLASS",
            22 => "CLASS_NOT_PREPARED",
            23 => "INVALID_METHODID",
            24 => "INVALID_LOCATION",
            25 => "INVALID_FIELDID",
            30 => "INVALID_FRAMEID",
            34 => "TYPE_MISMATCH",
            35 => "INVALID_SLOT",
            41 => "NOT_FOUND",
            60 => "INVALID_CLASS_FORMAT",
            61 => "CIRCULAR_CLASS_DEFINITION",
            62 => "FAILS_VERIFICATION",
            63 => "ADD_METHOD_NOT_IMPLEMENTED",
            64 => "SCHEMA_CHANGE_NOT_IMPLEMENTED",
            65 => "INVALID_TYPESTATE",
            66 => "HIERARCHY_CHANGE_NOT_IMPLEMENTED",
            67 => "DELETE_METHOD_NOT_IMPLEMENTED",
            68 => "UNSUPPORTED_VERSION",
            69 => "NAMES_DONT_MATCH",
            70 => "CLASS_MODIFIERS_CHANGE_NOT_IMPLEMENTED",
            71 => "METHOD_MODIFIERS_CHANGE_NOT_IMPLEMENTED",
            72 => "CLASS_ATTRIBUTE_CHANGE_NOT_IMPLEMENTED",
            99 => "NOT_IMPLEMENTED",
            101 => "ABSENT_INFORMATION",
            102 => "INVALID_EVENT_TYPE",
            112 => "VM_DEAD",
            502 => "INTERNAL",
            511 => "NATIVE_METHOD",
            _ => "",
        };
        write!(f, "JDWP error {}", self.0)?;
        if !name.is_empty() {
            write!(f, " ({name})")?;
        }
        Ok(())
    }
}

impl std::error::Error for JdwpError {}

/// 错误是不是给定的 JDWP 错误码。
pub fn is_error(err: &anyhow::Error, code: u16) -> bool {
    err.downcast_ref::<JdwpError>().is_some_and(|e| e.0 == code)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    Command {
        id: u32,
        set: u8,
        cmd: u8,
        data: Vec<u8>,
    },
    Reply {
        id: u32,
        error: u16,
        data: Vec<u8>,
    },
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let (id, data) = match self {
            Packet::Command { id, data, .. } | Packet::Reply { id, data, .. } => (*id, data),
        };
        let mut out = Vec::with_capacity(HEADER_LEN + data.len());
        out.extend_from_slice(&((HEADER_LEN + data.len()) as u32).to_be_bytes());
        out.extend_from_slice(&id.to_be_bytes());
        match self {
            Packet::Command { set, cmd, .. } => {
                out.push(0);
                out.push(*set);
                out.push(*cmd);
            }
            Packet::Reply { error, .. } => {
                out.push(FLAG_REPLY);
                out.extend_from_slice(&error.to_be_bytes());
            }
        }
        out.extend_from_slice(data);
        out
    }

    /// 解码一个完整的包（含包头）。
    pub fn decode(bytes: &[u8]) -> io::Result<Packet> {
        if bytes.len() < HEADER_LEN {
            return Err(invalid("JDWP 包太短"));
        }
        let len = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize;
        if len != bytes.len() {
            return Err(invalid("JDWP 包长度与内容不符"));
        }
        let id = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
        let flags = bytes[8];
        let data = bytes[HEADER_LEN..].to_vec();
        Ok(if flags & FLAG_REPLY != 0 {
            Packet::Reply {
                id,
                error: u16::from_be_bytes([bytes[9], bytes[10]]),
                data,
            }
        } else {
            Packet::Command {
                id,
                set: bytes[9],
                cmd: bytes[10],
                data,
            }
        })
    }

    /// 从流里读一个包。流在包边界处结束时返回 `Ok(None)`。
    pub fn read_from(reader: &mut impl Read) -> io::Result<Option<Packet>> {
        let mut header = [0u8; HEADER_LEN];
        let mut filled = 0;
        while filled < HEADER_LEN {
            match reader.read(&mut header[filled..]) {
                Ok(0) if filled == 0 => return Ok(None),
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let len = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
        if !(HEADER_LEN..=MAX_PACKET_LEN).contains(&len) {
            return Err(invalid("JDWP 包长度不合法"));
        }
        let mut bytes = vec![0u8; len];
        bytes[..HEADER_LEN].copy_from_slice(&header);
        reader.read_exact(&mut bytes[HEADER_LEN..])?;
        Packet::decode(&bytes).map(Some)
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// `VirtualMachine.IDSizes` 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdSizes {
    pub field: usize,
    pub method: usize,
    pub object: usize,
    pub reference_type: usize,
    pub frame: usize,
}

impl Default for IdSizes {
    fn default() -> Self {
        IdSizes {
            field: 8,
            method: 8,
            object: 8,
            reference_type: 8,
            frame: 8,
        }
    }
}

/// 代码位置：`typeTag classID methodID index`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Location {
    pub type_tag: u8,
    pub class: u64,
    pub method: u64,
    pub index: u64,
}

/// 带标签的值。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Void,
    Byte(i8),
    Char(u16),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Boolean(bool),
    /// 对象引用：标签区分字符串、数组、线程等；`id == 0` 表示 null
    Object {
        tag: u8,
        id: u64,
    },
}

impl Value {
    pub fn tag(&self) -> u8 {
        match self {
            Value::Void => tag::VOID,
            Value::Byte(_) => tag::BYTE,
            Value::Char(_) => tag::CHAR,
            Value::Short(_) => tag::SHORT,
            Value::Int(_) => tag::INT,
            Value::Long(_) => tag::LONG,
            Value::Float(_) => tag::FLOAT,
            Value::Double(_) => tag::DOUBLE,
            Value::Boolean(_) => tag::BOOLEAN,
            Value::Object { tag, .. } => *tag,
        }
    }

    pub fn object_id(&self) -> Option<u64> {
        match self {
            Value::Object { id, .. } if *id != 0 => Some(*id),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Object { id: 0, .. })
    }
}

/// 往命令数据里写字段。
pub struct Writer {
    buf: Vec<u8>,
    sizes: IdSizes,
}

impl Writer {
    pub fn new(sizes: IdSizes) -> Writer {
        Writer {
            buf: Vec::new(),
            sizes,
        }
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(bytes);
        self
    }

    pub fn string(&mut self, s: &str) -> &mut Self {
        self.i32(s.len() as i32);
        self.buf.extend_from_slice(s.as_bytes());
        self
    }

    fn id(&mut self, v: u64, size: usize) -> &mut Self {
        let bytes = v.to_be_bytes();
        let size = size.clamp(1, 8);
        self.buf.extend_from_slice(&bytes[8 - size..]);
        self
    }

    pub fn object(&mut self, v: u64) -> &mut Self {
        self.id(v, self.sizes.object)
    }

    pub fn ref_type(&mut self, v: u64) -> &mut Self {
        self.id(v, self.sizes.reference_type)
    }

    pub fn method(&mut self, v: u64) -> &mut Self {
        self.id(v, self.sizes.method)
    }

    pub fn field(&mut self, v: u64) -> &mut Self {
        self.id(v, self.sizes.field)
    }

    pub fn frame(&mut self, v: u64) -> &mut Self {
        self.id(v, self.sizes.frame)
    }

    pub fn location(&mut self, loc: &Location) -> &mut Self {
        self.u8(loc.type_tag);
        self.ref_type(loc.class);
        self.method(loc.method);
        self.u64(loc.index)
    }

    /// 不带标签的值（`ObjectReference.SetValues` 等用）。
    pub fn untagged_value(&mut self, value: &Value) -> &mut Self {
        match *value {
            Value::Void => self,
            Value::Byte(v) => self.u8(v as u8),
            Value::Char(v) => {
                self.buf.extend_from_slice(&v.to_be_bytes());
                self
            }
            Value::Short(v) => {
                self.buf.extend_from_slice(&v.to_be_bytes());
                self
            }
            Value::Int(v) => self.i32(v),
            Value::Long(v) => self.u64(v as u64),
            Value::Float(v) => self.i32(v.to_bits() as i32),
            Value::Double(v) => self.u64(v.to_bits()),
            Value::Boolean(v) => self.bool(v),
            Value::Object { id, .. } => self.object(id),
        }
    }

    pub fn value(&mut self, value: &Value) -> &mut Self {
        self.u8(value.tag());
        self.untagged_value(value)
    }
}

/// 从回复数据里读字段。
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    sizes: IdSizes,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], sizes: IdSizes) -> Reader<'a> {
        Reader {
            data,
            pos: 0,
            sizes,
        }
    }

    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| anyhow::anyhow!("JDWP 回复数据不完整"))?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> anyhow::Result<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn u16(&mut self) -> anyhow::Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> anyhow::Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// 计数字段：负数当作 0
    pub fn count(&mut self) -> anyhow::Result<usize> {
        let n = self.i32()?.max(0) as usize;
        // 每个元素至少一个字节；数量明显超过剩余数据时说明数据坏了
        anyhow::ensure!(n <= self.remaining(), "JDWP 回复里的计数不合法");
        Ok(n)
    }

    pub fn u64(&mut self) -> anyhow::Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn string(&mut self) -> anyhow::Result<String> {
        let len = self.i32()?.max(0) as usize;
        let bytes = self.take(len)?;
        Ok(decode_modified_utf8(bytes))
    }

    fn id(&mut self, size: usize) -> anyhow::Result<u64> {
        let bytes = self.take(size.clamp(1, 8))?;
        let mut buf = [0u8; 8];
        buf[8 - bytes.len()..].copy_from_slice(bytes);
        Ok(u64::from_be_bytes(buf))
    }

    pub fn object(&mut self) -> anyhow::Result<u64> {
        self.id(self.sizes.object)
    }

    pub fn ref_type(&mut self) -> anyhow::Result<u64> {
        self.id(self.sizes.reference_type)
    }

    pub fn method(&mut self) -> anyhow::Result<u64> {
        self.id(self.sizes.method)
    }

    pub fn field(&mut self) -> anyhow::Result<u64> {
        self.id(self.sizes.field)
    }

    pub fn frame(&mut self) -> anyhow::Result<u64> {
        self.id(self.sizes.frame)
    }

    pub fn location(&mut self) -> anyhow::Result<Location> {
        Ok(Location {
            type_tag: self.u8()?,
            class: self.ref_type()?,
            method: self.method()?,
            index: self.u64()?,
        })
    }

    pub fn untagged_value(&mut self, tag: u8) -> anyhow::Result<Value> {
        Ok(match tag {
            tag::VOID => Value::Void,
            tag::BYTE => Value::Byte(self.u8()? as i8),
            tag::CHAR => Value::Char(self.u16()?),
            tag::SHORT => Value::Short(self.u16()? as i16),
            tag::INT => Value::Int(self.i32()?),
            tag::LONG => Value::Long(self.u64()? as i64),
            tag::FLOAT => Value::Float(f32::from_bits(self.i32()? as u32)),
            tag::DOUBLE => Value::Double(f64::from_bits(self.u64()?)),
            tag::BOOLEAN => Value::Boolean(self.bool()?),
            tag::ARRAY
            | tag::OBJECT
            | tag::STRING
            | tag::THREAD
            | tag::THREAD_GROUP
            | tag::CLASS_LOADER
            | tag::CLASS_OBJECT => Value::Object {
                tag,
                id: self.object()?,
            },
            other => anyhow::bail!("未知的 JDWP 值标签 {other}"),
        })
    }

    pub fn value(&mut self) -> anyhow::Result<Value> {
        let tag = self.u8()?;
        self.untagged_value(tag)
    }
}

/// JVM 的“修改版 UTF-8”：`\0` 编成两个字节，补充平面字符编成两个代理对。
/// 先按标准 UTF-8 解，失败时再按 CESU-8 的方式拼代理对。
fn decode_modified_utf8(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_owned();
    }
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            units.push(b as u16);
            i += 1;
        } else if b & 0xE0 == 0xC0 && i + 1 < bytes.len() {
            units.push((((b & 0x1F) as u16) << 6) | (bytes[i + 1] & 0x3F) as u16);
            i += 2;
        } else if b & 0xF0 == 0xE0 && i + 2 < bytes.len() {
            units.push(
                (((b & 0x0F) as u16) << 12)
                    | (((bytes[i + 1] & 0x3F) as u16) << 6)
                    | (bytes[i + 2] & 0x3F) as u16,
            );
            i += 3;
        } else {
            units.push(0xFFFD);
            i += 1;
        }
    }
    String::from_utf16_lossy(&units)
}

/// 把 JVM 类型签名转成 Java 写法：`I` → `int`，`[Ljava/lang/String;` → `java.lang.String[]`。
pub fn signature_to_type_name(sig: &str) -> String {
    let dims = sig.bytes().take_while(|&b| b == b'[').count();
    let base = &sig[dims..];
    let name = match base.as_bytes().first() {
        Some(b'B') => "byte".to_owned(),
        Some(b'C') => "char".to_owned(),
        Some(b'D') => "double".to_owned(),
        Some(b'F') => "float".to_owned(),
        Some(b'I') => "int".to_owned(),
        Some(b'J') => "long".to_owned(),
        Some(b'S') => "short".to_owned(),
        Some(b'Z') => "boolean".to_owned(),
        Some(b'V') => "void".to_owned(),
        Some(b'L') => base[1..].trim_end_matches(';').replace('/', ".").to_owned(),
        _ => base.to_owned(),
    };
    let mut out = name;
    for _ in 0..dims {
        out.push_str("[]");
    }
    out
}

/// 类型名去掉包名：`java.util.ArrayList` → `ArrayList`，`demo.Outer$Inner` → `Outer$Inner`。
pub fn simple_name(type_name: &str) -> &str {
    type_name.rsplit('.').next().unwrap_or(type_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_packet_round_trip() {
        let packet = Packet::Command {
            id: 7,
            set: 1,
            cmd: 7,
            data: vec![1, 2, 3],
        };
        let bytes = packet.encode();
        assert_eq!(bytes, [0, 0, 0, 14, 0, 0, 0, 7, 0, 1, 7, 1, 2, 3]);
        assert_eq!(Packet::decode(&bytes).unwrap(), packet);
    }

    #[test]
    fn reply_packet_round_trip() {
        let packet = Packet::Reply {
            id: 0x01020304,
            error: 101,
            data: Vec::new(),
        };
        let bytes = packet.encode();
        assert_eq!(bytes, [0, 0, 0, 11, 1, 2, 3, 4, 0x80, 0, 101]);
        assert_eq!(Packet::decode(&bytes).unwrap(), packet);
    }

    #[test]
    fn read_packets_from_stream() {
        let mut stream = Vec::new();
        let a = Packet::Command {
            id: 1,
            set: 64,
            cmd: 100,
            data: vec![2, 0, 0, 0, 0],
        };
        let b = Packet::Reply {
            id: 2,
            error: 0,
            data: vec![9; 20],
        };
        stream.extend(a.encode());
        stream.extend(b.encode());
        let mut cursor = io::Cursor::new(stream);
        assert_eq!(Packet::read_from(&mut cursor).unwrap(), Some(a));
        assert_eq!(Packet::read_from(&mut cursor).unwrap(), Some(b));
        assert_eq!(Packet::read_from(&mut cursor).unwrap(), None);

        // 包头不完整、长度不合法
        let mut truncated = io::Cursor::new(vec![0, 0, 0, 20, 0]);
        assert!(Packet::read_from(&mut truncated).is_err());
        let mut bad = io::Cursor::new(vec![0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0]);
        assert!(Packet::read_from(&mut bad).is_err());
    }

    #[test]
    fn writer_and_reader_agree() {
        let sizes = IdSizes {
            field: 4,
            method: 8,
            object: 8,
            reference_type: 4,
            frame: 8,
        };
        let loc = Location {
            type_tag: 1,
            class: 0xABCD,
            method: 0x1234_5678_9ABC,
            index: 42,
        };
        let mut w = Writer::new(sizes);
        w.u8(5)
            .bool(true)
            .i32(-3)
            .u64(1 << 40)
            .string("héllo")
            .object(77)
            .ref_type(88)
            .field(99)
            .location(&loc)
            .value(&Value::Int(12))
            .value(&Value::Double(1.5))
            .value(&Value::Object {
                tag: tag::STRING,
                id: 5,
            })
            .value(&Value::Char('x' as u16));
        let data = w.finish();
        let mut r = Reader::new(&data, sizes);
        assert_eq!(r.u8().unwrap(), 5);
        assert!(r.bool().unwrap());
        assert_eq!(r.i32().unwrap(), -3);
        assert_eq!(r.u64().unwrap(), 1 << 40);
        assert_eq!(r.string().unwrap(), "héllo");
        assert_eq!(r.object().unwrap(), 77);
        assert_eq!(r.ref_type().unwrap(), 88);
        assert_eq!(r.field().unwrap(), 99);
        assert_eq!(r.location().unwrap(), loc);
        assert_eq!(r.value().unwrap(), Value::Int(12));
        assert_eq!(r.value().unwrap(), Value::Double(1.5));
        assert_eq!(
            r.value().unwrap(),
            Value::Object {
                tag: tag::STRING,
                id: 5
            }
        );
        assert_eq!(r.value().unwrap(), Value::Char('x' as u16));
        assert_eq!(r.remaining(), 0);
        assert!(r.u8().is_err());
    }

    #[test]
    fn modified_utf8() {
        // U+1F600 按 CESU-8 编成两个代理对
        let bytes = [0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80, b'!'];
        assert_eq!(decode_modified_utf8(&bytes), "😀!");
        // `\0` 的两字节形式
        assert_eq!(decode_modified_utf8(&[b'a', 0xC0, 0x80, b'b']), "a\0b");
    }

    #[test]
    fn signatures() {
        assert_eq!(signature_to_type_name("I"), "int");
        assert_eq!(
            signature_to_type_name("[[Ljava/lang/String;"),
            "java.lang.String[][]"
        );
        assert_eq!(
            signature_to_type_name("Ldemo/Outer$Inner;"),
            "demo.Outer$Inner"
        );
        assert_eq!(simple_name("java.util.ArrayList"), "ArrayList");
    }
}
