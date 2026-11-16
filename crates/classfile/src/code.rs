//! 带方法体的完整解析：常量池全部保留，方法的 `Code` 属性（字节码、异常表、
//! LocalVariableTable / LocalVariableTypeTable）和类的 `BootstrapMethods` 都读出来。
//!
//! 只有反编译用得到；索引和签名级的分析走 [`crate::parse_class`]，不读方法体。

use crate::reader::{access, ClassFileError, InnerClassInfo};

type Result<T> = std::result::Result<T, ClassFileError>;

/// 常量池里的一项。引用关系保留下标，用 [`ConstantPool`] 的方法解析。
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Unused,
    Utf8(String),
    Int(i32),
    /// 按位存储
    Float(u32),
    Long(i64),
    Double(u64),
    Class(u16),
    /// 字符串常量的内容按 UTF-16 码元保存，单独的代理项也不会丢
    Str(Vec<u16>),
    FieldRef(u16, u16),
    MethodRef(u16, u16),
    InterfaceMethodRef(u16, u16),
    NameAndType(u16, u16),
    MethodHandle(u8, u16),
    MethodType(u16),
    Dynamic(u16, u16),
    InvokeDynamic(u16, u16),
    Module(u16),
    Package(u16),
}

/// 字段 / 方法引用解析后的样子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRef {
    /// 所属类的二进制名；数组类型的方法（`clone`）是数组描述符，如 `[I`
    pub owner: String,
    pub name: String,
    pub descriptor: String,
    /// 是否来自 `CONSTANT_InterfaceMethodref`
    pub interface: bool,
}

/// `CONSTANT_MethodHandle` 解析后的样子。`kind` 是 JVMS 表 5.4.3.5-A 里的 1..=9。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodHandle {
    pub kind: u8,
    pub member: MemberRef,
}

pub mod handle_kind {
    pub const GET_FIELD: u8 = 1;
    pub const GET_STATIC: u8 = 2;
    pub const PUT_FIELD: u8 = 3;
    pub const PUT_STATIC: u8 = 4;
    pub const INVOKE_VIRTUAL: u8 = 5;
    pub const INVOKE_STATIC: u8 = 6;
    pub const INVOKE_SPECIAL: u8 = 7;
    pub const NEW_INVOKE_SPECIAL: u8 = 8;
    pub const INVOKE_INTERFACE: u8 = 9;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConstantPool {
    pub entries: Vec<Constant>,
}

impl ConstantPool {
    pub fn get(&self, idx: u16) -> Option<&Constant> {
        self.entries.get(idx as usize)
    }

    pub fn utf8(&self, idx: u16) -> Option<&str> {
        match self.get(idx)? {
            Constant::Utf8(s) => Some(s),
            _ => None,
        }
    }

    pub fn class_name(&self, idx: u16) -> Option<&str> {
        match self.get(idx)? {
            Constant::Class(name) => self.utf8(*name),
            _ => None,
        }
    }

    pub fn name_and_type(&self, idx: u16) -> Option<(&str, &str)> {
        match self.get(idx)? {
            Constant::NameAndType(name, desc) => Some((self.utf8(*name)?, self.utf8(*desc)?)),
            _ => None,
        }
    }

    /// 字段、方法、接口方法引用。
    pub fn member(&self, idx: u16) -> Option<MemberRef> {
        let (class, nat, interface) = match self.get(idx)? {
            Constant::FieldRef(c, n) | Constant::MethodRef(c, n) => (*c, *n, false),
            Constant::InterfaceMethodRef(c, n) => (*c, *n, true),
            _ => return None,
        };
        let owner = self.class_name(class)?.to_owned();
        let (name, descriptor) = self.name_and_type(nat)?;
        Some(MemberRef {
            owner,
            name: name.to_owned(),
            descriptor: descriptor.to_owned(),
            interface,
        })
    }

    pub fn method_handle(&self, idx: u16) -> Option<MethodHandle> {
        match self.get(idx)? {
            Constant::MethodHandle(kind, reference) => Some(MethodHandle {
                kind: *kind,
                member: self.member(*reference)?,
            }),
            _ => None,
        }
    }

    pub fn method_type(&self, idx: u16) -> Option<&str> {
        match self.get(idx)? {
            Constant::MethodType(desc) => self.utf8(*desc),
            _ => None,
        }
    }

    /// `invokedynamic` / `ldc` 动态常量：(引导方法下标, 名字, 描述符)
    pub fn dynamic(&self, idx: u16) -> Option<(u16, &str, &str)> {
        match self.get(idx)? {
            Constant::InvokeDynamic(bsm, nat) | Constant::Dynamic(bsm, nat) => {
                let (name, desc) = self.name_and_type(*nat)?;
                Some((*bsm, name, desc))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceptionEntry {
    pub start: u16,
    pub end: u16,
    pub handler: u16,
    /// `None` 表示捕获一切（`finally` / `synchronized` 生成的）
    pub catch_type: Option<String>,
}

/// LocalVariableTable（`descriptor` 是描述符）或 LocalVariableTypeTable（是泛型签名）的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVariable {
    pub start: u16,
    pub length: u16,
    pub name: String,
    pub descriptor: String,
    pub slot: u16,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Code {
    pub max_stack: u16,
    pub max_locals: u16,
    pub bytecode: Vec<u8>,
    pub exception_table: Vec<ExceptionEntry>,
    pub local_variables: Vec<LocalVariable>,
    pub local_variable_types: Vec<LocalVariable>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodeMethod {
    pub access: u16,
    pub name: String,
    pub descriptor: String,
    pub signature: Option<String>,
    pub exceptions: Vec<String>,
    /// MethodParameters 里的名字（缺名的为 `None`）；没有这个属性时为空
    pub parameter_names: Vec<Option<String>>,
    pub code: Option<Code>,
}

impl CodeMethod {
    pub fn is_static(&self) -> bool {
        self.access & access::STATIC != 0
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodeField {
    pub access: u16,
    pub name: String,
    pub descriptor: String,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootstrapMethod {
    /// 指向 `CONSTANT_MethodHandle`
    pub method: u16,
    pub arguments: Vec<u16>,
}

/// 带方法体的类。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodeClass {
    pub major_version: u16,
    pub access: u16,
    pub name: String,
    pub super_name: Option<String>,
    pub interfaces: Vec<String>,
    pub signature: Option<String>,
    pub pool: ConstantPool,
    pub fields: Vec<CodeField>,
    pub methods: Vec<CodeMethod>,
    pub bootstrap_methods: Vec<BootstrapMethod>,
    pub inner_classes: Vec<InnerClassInfo>,
    /// EnclosingMethod 属性：(外层类, 外层方法的名字和描述符)
    pub enclosing_method: Option<(String, Option<(String, String)>)>,
}

impl CodeClass {
    pub fn is_interface(&self) -> bool {
        self.access & access::INTERFACE != 0
    }

    pub fn find_method(&self, name: &str, descriptor: &str) -> Option<&CodeMethod> {
        self.methods
            .iter()
            .find(|m| m.name == name && m.descriptor == descriptor)
    }

    pub fn find_field(&self, name: &str) -> Option<&CodeField> {
        self.fields.iter().find(|f| f.name == name)
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(ClassFileError("长度溢出"))?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(ClassFileError("意外的文件结尾"))?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.bytes(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

/// 改良 UTF-8 解码成 UTF-16 码元（保留单独的代理项）。
pub fn decode_modified_utf8_units(bytes: &[u8]) -> Vec<u16> {
    let mut units = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b & 0x80 == 0 {
            units.push(b as u16);
            i += 1;
        } else if b & 0xE0 == 0xC0 && i + 1 < bytes.len() {
            units.push(((b as u16 & 0x1F) << 6) | (bytes[i + 1] as u16 & 0x3F));
            i += 2;
        } else if b & 0xF0 == 0xE0 && i + 2 < bytes.len() {
            units.push(
                ((b as u16 & 0x0F) << 12)
                    | ((bytes[i + 1] as u16 & 0x3F) << 6)
                    | (bytes[i + 2] as u16 & 0x3F),
            );
            i += 3;
        } else if b & 0xF8 == 0xF0 && i + 3 < bytes.len() {
            // 标准 UTF-8 的四字节形式（改良 UTF-8 里不会出现，宽容处理）
            let cp = ((b as u32 & 0x07) << 18)
                | ((bytes[i + 1] as u32 & 0x3F) << 12)
                | ((bytes[i + 2] as u32 & 0x3F) << 6)
                | (bytes[i + 3] as u32 & 0x3F);
            match char::from_u32(cp) {
                Some(c) => {
                    let mut buf = [0u16; 2];
                    units.extend_from_slice(c.encode_utf16(&mut buf));
                }
                None => units.push(0xFFFD),
            }
            i += 4;
        } else {
            units.push(0xFFFD);
            i += 1;
        }
    }
    units
}

fn decode_string(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => String::from_utf16_lossy(&decode_modified_utf8_units(bytes)),
    }
}

fn parse_pool(r: &mut Reader<'_>) -> Result<ConstantPool> {
    let count = r.u16()? as usize;
    let mut entries = Vec::with_capacity(count);
    entries.push(Constant::Unused);
    while entries.len() < count {
        let tag = r.u8()?;
        let entry = match tag {
            1 => {
                let len = r.u16()? as usize;
                // 先按原始字节存，字符串常量再按 UTF-16 解一遍
                Constant::Utf8(decode_string(r.bytes(len)?))
            }
            3 => Constant::Int(r.u32()? as i32),
            4 => Constant::Float(r.u32()?),
            5 => Constant::Long(r.u64()? as i64),
            6 => Constant::Double(r.u64()?),
            7 => Constant::Class(r.u16()?),
            8 => {
                // 暂存下标，解析完整个常量池后替换成内容
                let idx = r.u16()?;
                Constant::Str(vec![idx])
            }
            9 => Constant::FieldRef(r.u16()?, r.u16()?),
            10 => Constant::MethodRef(r.u16()?, r.u16()?),
            11 => Constant::InterfaceMethodRef(r.u16()?, r.u16()?),
            12 => Constant::NameAndType(r.u16()?, r.u16()?),
            15 => Constant::MethodHandle(r.u8()?, r.u16()?),
            16 => Constant::MethodType(r.u16()?),
            17 => Constant::Dynamic(r.u16()?, r.u16()?),
            18 => Constant::InvokeDynamic(r.u16()?, r.u16()?),
            19 => Constant::Module(r.u16()?),
            20 => Constant::Package(r.u16()?),
            _ => return Err(ClassFileError("未知的常量池标签")),
        };
        entries.push(entry);
        if matches!(tag, 5 | 6) {
            entries.push(Constant::Unused);
        }
    }
    Ok(ConstantPool { entries })
}

/// 字符串常量需要原始字节才能保留单独的代理项：重新扫一遍常量池拿到 Utf8 的原始字节。
fn raw_utf8_bytes(data: &[u8]) -> Result<Vec<Option<(usize, usize)>>> {
    let mut r = Reader { data, pos: 8 };
    let count = r.u16()? as usize;
    let mut res = vec![None; count.max(1)];
    let mut idx = 1;
    while idx < count {
        let tag = r.u8()?;
        match tag {
            1 => {
                let len = r.u16()? as usize;
                res[idx] = Some((r.pos, len));
                r.bytes(len)?;
            }
            3 | 4 => {
                r.bytes(4)?;
            }
            5 | 6 => {
                r.bytes(8)?;
                idx += 1;
            }
            7 | 8 | 16 | 19 | 20 => {
                r.bytes(2)?;
            }
            9..=12 | 17 | 18 => {
                r.bytes(4)?;
            }
            15 => {
                r.bytes(3)?;
            }
            _ => return Err(ClassFileError("未知的常量池标签")),
        }
        idx += 1;
    }
    Ok(res)
}

fn attr_name(pool: &ConstantPool, idx: u16) -> &str {
    pool.utf8(idx).unwrap_or("")
}

fn read_local_variables(pool: &ConstantPool, r: &mut Reader<'_>) -> Result<Vec<LocalVariable>> {
    let n = r.u16()?;
    let mut res = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let start = r.u16()?;
        let length = r.u16()?;
        let name = r.u16()?;
        let descriptor = r.u16()?;
        let slot = r.u16()?;
        let (Some(name), Some(descriptor)) = (pool.utf8(name), pool.utf8(descriptor)) else {
            continue;
        };
        res.push(LocalVariable {
            start,
            length,
            name: name.to_owned(),
            descriptor: descriptor.to_owned(),
            slot,
        });
    }
    Ok(res)
}

fn read_code(pool: &ConstantPool, r: &mut Reader<'_>) -> Result<Code> {
    let max_stack = r.u16()?;
    let max_locals = r.u16()?;
    let len = r.u32()? as usize;
    let bytecode = r.bytes(len)?.to_vec();
    let n = r.u16()?;
    let mut exception_table = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let start = r.u16()?;
        let end = r.u16()?;
        let handler = r.u16()?;
        let catch_idx = r.u16()?;
        let catch_type = if catch_idx == 0 {
            None
        } else {
            Some(
                pool.class_name(catch_idx)
                    .ok_or(ClassFileError("异常表的类型不是 Class 常量"))?
                    .to_owned(),
            )
        };
        exception_table.push(ExceptionEntry {
            start,
            end,
            handler,
            catch_type,
        });
    }
    let mut code = Code {
        max_stack,
        max_locals,
        bytecode,
        exception_table,
        ..Code::default()
    };
    let n_attrs = r.u16()?;
    for _ in 0..n_attrs {
        let name = r.u16()?;
        let len = r.u32()? as usize;
        let mut body = Reader {
            data: r.bytes(len)?,
            pos: 0,
        };
        match attr_name(pool, name) {
            // 读不出来就当没有，不影响字节码本身
            "LocalVariableTable" => {
                if let Ok(vars) = read_local_variables(pool, &mut body) {
                    code.local_variables.extend(vars);
                }
            }
            "LocalVariableTypeTable" => {
                if let Ok(vars) = read_local_variables(pool, &mut body) {
                    code.local_variable_types.extend(vars);
                }
            }
            _ => {}
        }
    }
    Ok(code)
}

/// 完整解析一个 class 文件，连同方法体。
pub fn parse_class_code(data: &[u8]) -> Result<CodeClass> {
    let mut r = Reader { data, pos: 0 };
    if r.u32()? != 0xCAFE_BABE {
        return Err(ClassFileError("魔数不对"));
    }
    let _minor = r.u16()?;
    let major_version = r.u16()?;
    let mut pool = parse_pool(&mut r)?;
    // 字符串常量换成 UTF-16 内容
    let raw = raw_utf8_bytes(data)?;
    for i in 0..pool.entries.len() {
        if let Constant::Str(idx) = &pool.entries[i] {
            let idx = idx[0] as usize;
            let units = match raw.get(idx).copied().flatten() {
                Some((pos, len)) => decode_modified_utf8_units(&data[pos..pos + len]),
                None => return Err(ClassFileError("字符串常量指向了非 Utf8 常量")),
            };
            pool.entries[i] = Constant::Str(units);
        }
    }

    let mut class = CodeClass {
        major_version,
        ..CodeClass::default()
    };
    class.access = r.u16()?;
    class.name = pool
        .class_name(r.u16()?)
        .ok_or(ClassFileError("this_class 不是 Class 常量"))?
        .to_owned();
    let super_idx = r.u16()?;
    class.super_name = if super_idx == 0 {
        None
    } else {
        Some(
            pool.class_name(super_idx)
                .ok_or(ClassFileError("super_class 不是 Class 常量"))?
                .to_owned(),
        )
    };
    for _ in 0..r.u16()? {
        let idx = r.u16()?;
        class.interfaces.push(
            pool.class_name(idx)
                .ok_or(ClassFileError("接口不是 Class 常量"))?
                .to_owned(),
        );
    }

    for _ in 0..r.u16()? {
        let access = r.u16()?;
        let name = r.u16()?;
        let desc = r.u16()?;
        let mut field = CodeField {
            access,
            name: pool.utf8(name).unwrap_or("").to_owned(),
            descriptor: pool.utf8(desc).unwrap_or("").to_owned(),
            signature: None,
        };
        for _ in 0..r.u16()? {
            let attr = r.u16()?;
            let len = r.u32()? as usize;
            let mut body = Reader {
                data: r.bytes(len)?,
                pos: 0,
            };
            if attr_name(&pool, attr) == "Signature" {
                field.signature = pool.utf8(body.u16()?).map(str::to_owned);
            }
        }
        class.fields.push(field);
    }

    for _ in 0..r.u16()? {
        let access = r.u16()?;
        let name = r.u16()?;
        let desc = r.u16()?;
        let mut method = CodeMethod {
            access,
            name: pool.utf8(name).unwrap_or("").to_owned(),
            descriptor: pool.utf8(desc).unwrap_or("").to_owned(),
            ..CodeMethod::default()
        };
        for _ in 0..r.u16()? {
            let attr = r.u16()?;
            let len = r.u32()? as usize;
            let mut body = Reader {
                data: r.bytes(len)?,
                pos: 0,
            };
            match attr_name(&pool, attr) {
                "Code" => method.code = Some(read_code(&pool, &mut body)?),
                "Signature" => method.signature = pool.utf8(body.u16()?).map(str::to_owned),
                "Exceptions" => {
                    for _ in 0..body.u16()? {
                        if let Some(name) = pool.class_name(body.u16()?) {
                            method.exceptions.push(name.to_owned());
                        }
                    }
                }
                "MethodParameters" => {
                    for _ in 0..body.u8()? {
                        let name = body.u16()?;
                        let _flags = body.u16()?;
                        method
                            .parameter_names
                            .push(pool.utf8(name).map(str::to_owned));
                    }
                }
                _ => {}
            }
        }
        class.methods.push(method);
    }

    for _ in 0..r.u16()? {
        let attr = r.u16()?;
        let len = r.u32()? as usize;
        let mut body = Reader {
            data: r.bytes(len)?,
            pos: 0,
        };
        match attr_name(&pool, attr) {
            "Signature" => class.signature = pool.utf8(body.u16()?).map(str::to_owned),
            "BootstrapMethods" => {
                for _ in 0..body.u16()? {
                    let method = body.u16()?;
                    let n = body.u16()?;
                    let mut arguments = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        arguments.push(body.u16()?);
                    }
                    class
                        .bootstrap_methods
                        .push(BootstrapMethod { method, arguments });
                }
            }
            "InnerClasses" => {
                for _ in 0..body.u16()? {
                    let inner = body.u16()?;
                    let outer = body.u16()?;
                    let inner_name = body.u16()?;
                    let access = body.u16()?;
                    let Some(inner) = pool.class_name(inner) else {
                        continue;
                    };
                    class.inner_classes.push(InnerClassInfo {
                        inner: inner.to_owned(),
                        outer: pool.class_name(outer).map(str::to_owned),
                        inner_name: pool.utf8(inner_name).map(str::to_owned),
                        access,
                    });
                }
            }
            "EnclosingMethod" => {
                let outer = body.u16()?;
                let method = body.u16()?;
                if let Some(outer) = pool.class_name(outer) {
                    let method = pool
                        .name_and_type(method)
                        .map(|(n, d)| (n.to_owned(), d.to_owned()));
                    class.enclosing_method = Some((outer.to_owned(), method));
                }
            }
            _ => {}
        }
    }
    class.pool = pool;
    Ok(class)
}
