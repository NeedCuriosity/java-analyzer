//! `.class` 文件解析：只提取签名级信息，方法体（Code 属性）除了参数名之外全部跳过。

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassFileError(pub &'static str);

impl fmt::Display for ClassFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "class 文件格式错误: {}", self.0)
    }
}

impl std::error::Error for ClassFileError {}

type Result<T> = std::result::Result<T, ClassFileError>;

pub mod access {
    pub const PUBLIC: u16 = 0x0001;
    pub const PRIVATE: u16 = 0x0002;
    pub const PROTECTED: u16 = 0x0004;
    pub const STATIC: u16 = 0x0008;
    pub const FINAL: u16 = 0x0010;
    pub const SYNCHRONIZED: u16 = 0x0020;
    pub const BRIDGE: u16 = 0x0040;
    pub const VOLATILE: u16 = 0x0040;
    pub const VARARGS: u16 = 0x0080;
    pub const TRANSIENT: u16 = 0x0080;
    pub const NATIVE: u16 = 0x0100;
    pub const INTERFACE: u16 = 0x0200;
    pub const ABSTRACT: u16 = 0x0400;
    pub const STRICT: u16 = 0x0800;
    pub const SYNTHETIC: u16 = 0x1000;
    pub const ANNOTATION: u16 = 0x2000;
    pub const ENUM: u16 = 0x4000;
    pub const MODULE: u16 = 0x8000;
}

/// 编译期常量的值。浮点数按位存储，保证整个结构可以 `Eq` / `Hash`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstValue {
    Int(i32),
    Long(i64),
    Float(u32),
    Double(u64),
    Str(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClassFile {
    pub major_version: u16,
    pub access: u16,
    /// 二进制名，例如 `java/util/Map$Entry`
    pub name: String,
    pub super_name: Option<String>,
    pub interfaces: Vec<String>,
    pub signature: Option<String>,
    pub fields: Vec<FieldInfo>,
    pub methods: Vec<MethodInfo>,
    pub inner_classes: Vec<InnerClassInfo>,
    pub record_components: Vec<RecordComponentInfo>,
    pub permitted_subclasses: Vec<String>,
    /// 注解的类型描述符，例如 `Ljava/lang/Deprecated;`
    pub annotations: Vec<String>,
    pub deprecated: bool,
    pub source_file: Option<String>,
    /// 有 EnclosingMethod 属性：局部类或匿名类
    pub is_local_or_anonymous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldInfo {
    pub access: u16,
    pub name: String,
    pub descriptor: String,
    pub signature: Option<String>,
    pub constant_value: Option<ConstValue>,
    pub annotations: Vec<String>,
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MethodInfo {
    pub access: u16,
    pub name: String,
    pub descriptor: String,
    pub signature: Option<String>,
    pub exceptions: Vec<String>,
    /// 参数名（来自 MethodParameters 或 LocalVariableTable）；拿不到时为空
    pub param_names: Vec<String>,
    pub annotations: Vec<String>,
    pub deprecated: bool,
    /// 注解元素是否带默认值
    pub has_annotation_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InnerClassInfo {
    pub inner: String,
    pub outer: Option<String>,
    pub inner_name: Option<String>,
    pub access: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordComponentInfo {
    pub name: String,
    pub descriptor: String,
    pub signature: Option<String>,
}

/// 只含类头信息的摘要，用于全量索引（子类型搜索、补全候选）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassHeader {
    pub access: u16,
    pub name: String,
    pub super_name: Option<String>,
    pub interfaces: Vec<String>,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(ClassFileError("长度溢出"))?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(ClassFileError("意外的文件结尾"))?;
        self.pos = end;
        Ok(slice)
    }

    fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
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

#[derive(Clone, Copy)]
enum Const<'a> {
    Unused,
    Utf8(&'a [u8]),
    Int(i32),
    Float(u32),
    Long(i64),
    Double(u64),
    Class(u16),
    Str(u16),
    /// `CONSTANT_Module` / `CONSTANT_Package`：名字的 Utf8 下标
    Module(u16),
    Package(u16),
    Other,
}

struct ConstPool<'a> {
    entries: Vec<Const<'a>>,
}

impl<'a> ConstPool<'a> {
    fn parse(r: &mut Reader<'a>) -> Result<ConstPool<'a>> {
        let count = r.u16()? as usize;
        let mut entries = Vec::with_capacity(count);
        entries.push(Const::Unused);
        while entries.len() < count {
            let tag = r.u8()?;
            let entry = match tag {
                1 => {
                    let len = r.u16()? as usize;
                    Const::Utf8(r.bytes(len)?)
                }
                3 => Const::Int(r.u32()? as i32),
                4 => Const::Float(r.u32()?),
                5 => Const::Long(r.u64()? as i64),
                6 => Const::Double(r.u64()?),
                7 => Const::Class(r.u16()?),
                8 => Const::Str(r.u16()?),
                9 | 10 | 11 | 12 | 17 | 18 => {
                    r.skip(4)?;
                    Const::Other
                }
                15 => {
                    r.skip(3)?;
                    Const::Other
                }
                16 => {
                    r.skip(2)?;
                    Const::Other
                }
                19 => Const::Module(r.u16()?),
                20 => Const::Package(r.u16()?),
                _ => return Err(ClassFileError("未知的常量池标签")),
            };
            entries.push(entry);
            // long / double 占两个槽位
            if matches!(tag, 5 | 6) {
                entries.push(Const::Unused);
            }
        }
        Ok(ConstPool { entries })
    }

    fn get(&self, idx: u16) -> Result<Const<'a>> {
        self.entries
            .get(idx as usize)
            .copied()
            .ok_or(ClassFileError("常量池下标越界"))
    }

    fn utf8(&self, idx: u16) -> Result<String> {
        match self.get(idx)? {
            Const::Utf8(bytes) => Ok(decode_modified_utf8(bytes)),
            _ => Err(ClassFileError("此处应为 Utf8 常量")),
        }
    }

    fn utf8_is(&self, idx: u16, expected: &str) -> bool {
        matches!(self.get(idx), Ok(Const::Utf8(bytes)) if bytes == expected.as_bytes())
    }

    fn class_name(&self, idx: u16) -> Result<String> {
        match self.get(idx)? {
            Const::Class(name) => self.utf8(name),
            _ => Err(ClassFileError("此处应为 Class 常量")),
        }
    }

    fn opt_class_name(&self, idx: u16) -> Result<Option<String>> {
        if idx == 0 {
            Ok(None)
        } else {
            self.class_name(idx).map(Some)
        }
    }

    fn const_value(&self, idx: u16) -> Result<ConstValue> {
        Ok(match self.get(idx)? {
            Const::Int(v) => ConstValue::Int(v),
            Const::Long(v) => ConstValue::Long(v),
            Const::Float(v) => ConstValue::Float(v),
            Const::Double(v) => ConstValue::Double(v),
            Const::Str(s) => ConstValue::Str(self.utf8(s)?),
            _ => return Err(ClassFileError("ConstantValue 指向了不支持的常量")),
        })
    }
}

/// JVM 的"改良 UTF-8"：绝大多数情况下就是合法 UTF-8；
/// 少数情况（NUL、辅助平面字符）需要单独解码。
fn decode_modified_utf8(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_owned();
    }
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
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
        } else {
            units.push(0xFFFD);
            i += 1;
        }
    }
    String::from_utf16_lossy(&units)
}

const MAGIC: u32 = 0xCAFE_BABE;

/// `module-info.class` 里的模块声明。包名是斜杠形式。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleDescriptor {
    pub name: String,
    /// `(模块名, 是否 transitive)`
    pub requires: Vec<(String, bool)>,
    /// `(包, 限定导出的目标模块)`；目标为空表示对所有模块导出
    pub exports: Vec<(String, Vec<String>)>,
    pub opens: Vec<(String, Vec<String>)>,
    /// 自动模块（普通 jar）：导出它的所有包
    pub automatic: bool,
}

const ACC_TRANSITIVE: u16 = 0x0020;

/// 解析 `module-info.class` 的 `Module` 属性。
pub fn parse_module_info(data: &[u8]) -> Result<ModuleDescriptor> {
    let mut r = Reader::new(data);
    if r.u32()? != MAGIC {
        return Err(ClassFileError("魔数不对"));
    }
    r.skip(4)?;
    let cp = ConstPool::parse(&mut r)?;
    r.skip(6)?; // access, this_class, super_class
    let n_interfaces = r.u16()? as usize;
    r.skip(n_interfaces * 2)?;
    // 字段和方法（module-info 里没有，但按格式跳过）
    for _ in 0..2 {
        let n = r.u16()?;
        for _ in 0..n {
            r.skip(6)?;
            let n_attrs = r.u16()?;
            for _ in 0..n_attrs {
                r.skip(2)?;
                let len = r.u32()? as usize;
                r.skip(len)?;
            }
        }
    }
    let named = |idx: u16| -> Result<String> {
        match cp.get(idx)? {
            Const::Module(name) | Const::Package(name) => cp.utf8(name),
            _ => Err(ClassFileError("此处应为 Module / Package 常量")),
        }
    };
    let n_attrs = r.u16()?;
    for _ in 0..n_attrs {
        let name_idx = r.u16()?;
        let len = r.u32()? as usize;
        let mut body = Reader::new(r.bytes(len)?);
        if !cp.utf8_is(name_idx, "Module") {
            continue;
        }
        let mut module = ModuleDescriptor {
            name: named(body.u16()?)?,
            ..ModuleDescriptor::default()
        };
        body.skip(4)?; // flags, version
        for _ in 0..body.u16()? {
            let name = named(body.u16()?)?;
            let flags = body.u16()?;
            body.skip(2)?;
            module.requires.push((name, flags & ACC_TRANSITIVE != 0));
        }
        for list in [&mut module.exports, &mut module.opens] {
            for _ in 0..body.u16()? {
                let package = named(body.u16()?)?;
                body.skip(2)?;
                let mut targets = Vec::new();
                for _ in 0..body.u16()? {
                    targets.push(named(body.u16()?)?);
                }
                list.push((package, targets));
            }
        }
        return Ok(module);
    }
    Err(ClassFileError("没有 Module 属性"))
}

/// 只解析类头（访问标志、名字、父类、接口），用于建立全量索引。
pub fn parse_header(data: &[u8]) -> Result<ClassHeader> {
    let mut r = Reader::new(data);
    if r.u32()? != MAGIC {
        return Err(ClassFileError("魔数不对"));
    }
    r.skip(4)?;
    let cp = ConstPool::parse(&mut r)?;
    let access = r.u16()?;
    let name = cp.class_name(r.u16()?)?;
    let super_name = cp.opt_class_name(r.u16()?)?;
    let n_interfaces = r.u16()?;
    let mut interfaces = Vec::with_capacity(n_interfaces as usize);
    for _ in 0..n_interfaces {
        interfaces.push(cp.class_name(r.u16()?)?);
    }
    Ok(ClassHeader {
        access,
        name,
        super_name,
        interfaces,
    })
}

pub fn parse_class(data: &[u8]) -> Result<ClassFile> {
    let mut r = Reader::new(data);
    if r.u32()? != MAGIC {
        return Err(ClassFileError("魔数不对"));
    }
    let _minor = r.u16()?;
    let major_version = r.u16()?;
    let cp = ConstPool::parse(&mut r)?;

    let mut class = ClassFile {
        major_version,
        ..ClassFile::default()
    };
    class.access = r.u16()?;
    class.name = cp.class_name(r.u16()?)?;
    class.super_name = cp.opt_class_name(r.u16()?)?;
    let n_interfaces = r.u16()?;
    for _ in 0..n_interfaces {
        class.interfaces.push(cp.class_name(r.u16()?)?);
    }

    let n_fields = r.u16()?;
    for _ in 0..n_fields {
        let mut field = FieldInfo {
            access: r.u16()?,
            name: cp.utf8(r.u16()?)?,
            descriptor: cp.utf8(r.u16()?)?,
            ..FieldInfo::default()
        };
        let n_attrs = r.u16()?;
        for _ in 0..n_attrs {
            let name_idx = r.u16()?;
            let len = r.u32()? as usize;
            let mut body = Reader::new(r.bytes(len)?);
            if cp.utf8_is(name_idx, "Signature") {
                field.signature = Some(cp.utf8(body.u16()?)?);
            } else if cp.utf8_is(name_idx, "ConstantValue") {
                field.constant_value = cp.const_value(body.u16()?).ok();
            } else if cp.utf8_is(name_idx, "Deprecated") {
                field.deprecated = true;
            } else if is_annotations_attr(&cp, name_idx) {
                read_annotations(&cp, &mut body, &mut field.annotations)?;
            }
        }
        class.fields.push(field);
    }

    let n_methods = r.u16()?;
    for _ in 0..n_methods {
        let mut method = MethodInfo {
            access: r.u16()?,
            name: cp.utf8(r.u16()?)?,
            descriptor: cp.utf8(r.u16()?)?,
            ..MethodInfo::default()
        };
        let mut lvt_names: Vec<(u16, String)> = Vec::new();
        let n_attrs = r.u16()?;
        for _ in 0..n_attrs {
            let name_idx = r.u16()?;
            let len = r.u32()? as usize;
            let mut body = Reader::new(r.bytes(len)?);
            if cp.utf8_is(name_idx, "Signature") {
                method.signature = Some(cp.utf8(body.u16()?)?);
            } else if cp.utf8_is(name_idx, "Exceptions") {
                let n = body.u16()?;
                for _ in 0..n {
                    method.exceptions.push(cp.class_name(body.u16()?)?);
                }
            } else if cp.utf8_is(name_idx, "MethodParameters") {
                let n = body.u8()?;
                let mut names = Vec::with_capacity(n as usize);
                let mut complete = true;
                for _ in 0..n {
                    let name = body.u16()?;
                    let _flags = body.u16()?;
                    if name == 0 {
                        complete = false;
                    } else {
                        names.push(cp.utf8(name)?);
                    }
                }
                if complete {
                    method.param_names = names;
                }
            } else if cp.utf8_is(name_idx, "Code") {
                // 读不出来就当没有参数名，不影响其余信息
                let _ = read_lvt_param_names(&cp, &mut body, &mut lvt_names);
            } else if cp.utf8_is(name_idx, "Deprecated") {
                method.deprecated = true;
            } else if cp.utf8_is(name_idx, "AnnotationDefault") {
                method.has_annotation_default = true;
            } else if is_annotations_attr(&cp, name_idx) {
                read_annotations(&cp, &mut body, &mut method.annotations)?;
            }
        }
        if method.param_names.is_empty() && !lvt_names.is_empty() {
            method.param_names = param_names_from_lvt(&method, &lvt_names);
        }
        class.methods.push(method);
    }

    let n_attrs = r.u16()?;
    for _ in 0..n_attrs {
        let name_idx = r.u16()?;
        let len = r.u32()? as usize;
        let mut body = Reader::new(r.bytes(len)?);
        if cp.utf8_is(name_idx, "Signature") {
            class.signature = Some(cp.utf8(body.u16()?)?);
        } else if cp.utf8_is(name_idx, "SourceFile") {
            class.source_file = Some(cp.utf8(body.u16()?)?);
        } else if cp.utf8_is(name_idx, "Deprecated") {
            class.deprecated = true;
        } else if cp.utf8_is(name_idx, "EnclosingMethod") {
            class.is_local_or_anonymous = true;
        } else if cp.utf8_is(name_idx, "InnerClasses") {
            let n = body.u16()?;
            for _ in 0..n {
                let inner = cp.class_name(body.u16()?)?;
                let outer = cp.opt_class_name(body.u16()?)?;
                let name_idx = body.u16()?;
                let inner_name = if name_idx == 0 {
                    None
                } else {
                    Some(cp.utf8(name_idx)?)
                };
                let access = body.u16()?;
                class.inner_classes.push(InnerClassInfo {
                    inner,
                    outer,
                    inner_name,
                    access,
                });
            }
        } else if cp.utf8_is(name_idx, "Record") {
            let n = body.u16()?;
            for _ in 0..n {
                let name = cp.utf8(body.u16()?)?;
                let descriptor = cp.utf8(body.u16()?)?;
                let mut signature = None;
                let n_attrs = body.u16()?;
                for _ in 0..n_attrs {
                    let attr_name = body.u16()?;
                    let attr_len = body.u32()? as usize;
                    let mut attr = Reader::new(body.bytes(attr_len)?);
                    if cp.utf8_is(attr_name, "Signature") {
                        signature = Some(cp.utf8(attr.u16()?)?);
                    }
                }
                class.record_components.push(RecordComponentInfo {
                    name,
                    descriptor,
                    signature,
                });
            }
        } else if cp.utf8_is(name_idx, "PermittedSubclasses") {
            let n = body.u16()?;
            for _ in 0..n {
                class.permitted_subclasses.push(cp.class_name(body.u16()?)?);
            }
        } else if is_annotations_attr(&cp, name_idx) {
            read_annotations(&cp, &mut body, &mut class.annotations)?;
        }
    }

    Ok(class)
}

fn is_annotations_attr(cp: &ConstPool<'_>, name_idx: u16) -> bool {
    cp.utf8_is(name_idx, "RuntimeVisibleAnnotations")
        || cp.utf8_is(name_idx, "RuntimeInvisibleAnnotations")
}

fn read_annotations(cp: &ConstPool<'_>, r: &mut Reader<'_>, out: &mut Vec<String>) -> Result<()> {
    let n = r.u16()?;
    for _ in 0..n {
        out.push(cp.utf8(r.u16()?)?);
        skip_annotation_pairs(r)?;
    }
    Ok(())
}

/// 已读过注解的 type_index，跳过它的键值对。
fn skip_annotation_pairs(r: &mut Reader<'_>) -> Result<()> {
    let n_pairs = r.u16()?;
    for _ in 0..n_pairs {
        r.skip(2)?;
        skip_element_value(r)?;
    }
    Ok(())
}

fn skip_element_value(r: &mut Reader<'_>) -> Result<()> {
    match r.u8()? {
        b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' | b's' | b'c' => r.skip(2),
        b'e' => r.skip(4),
        b'@' => {
            r.skip(2)?;
            skip_annotation_pairs(r)
        }
        b'[' => {
            let n = r.u16()?;
            for _ in 0..n {
                skip_element_value(r)?;
            }
            Ok(())
        }
        _ => Err(ClassFileError("未知的注解元素值标签")),
    }
}

/// 从 Code 属性里的 LocalVariableTable 找出起始于 pc 0 的局部变量（即 `this` 和参数）。
fn read_lvt_param_names(
    cp: &ConstPool<'_>,
    r: &mut Reader<'_>,
    out: &mut Vec<(u16, String)>,
) -> Result<()> {
    r.skip(4)?; // max_stack, max_locals
    let code_len = r.u32()? as usize;
    r.skip(code_len)?;
    let n_exceptions = r.u16()? as usize;
    r.skip(n_exceptions * 8)?;
    let n_attrs = r.u16()?;
    for _ in 0..n_attrs {
        let name_idx = r.u16()?;
        let len = r.u32()? as usize;
        let mut body = Reader::new(r.bytes(len)?);
        if cp.utf8_is(name_idx, "LocalVariableTable") {
            let n = body.u16()?;
            for _ in 0..n {
                let start_pc = body.u16()?;
                let _length = body.u16()?;
                let name = body.u16()?;
                let _descriptor = body.u16()?;
                let slot = body.u16()?;
                if start_pc == 0 {
                    out.push((slot, cp.utf8(name)?));
                }
            }
        }
    }
    Ok(())
}

/// 按描述符算出每个参数占用的局部变量槽位，再去 LVT 里取名字。
fn param_names_from_lvt(method: &MethodInfo, lvt: &[(u16, String)]) -> Vec<String> {
    let mut slot: u16 = if method.access & access::STATIC != 0 {
        0
    } else {
        1
    };
    let mut names = Vec::new();
    let desc = method.descriptor.as_bytes();
    let mut i = 1; // 跳过 '('
    while i < desc.len() && desc[i] != b')' {
        // long / double 占两个槽位；数组（以 `[` 开头）总是一个
        let width = match desc[i] {
            b'J' | b'D' => 2,
            _ => 1,
        };
        // 跳过这个参数的描述符
        while i < desc.len() && desc[i] == b'[' {
            i += 1;
        }
        if i < desc.len() && desc[i] == b'L' {
            while i < desc.len() && desc[i] != b';' {
                i += 1;
            }
        }
        i += 1;
        match lvt.iter().find(|(s, _)| *s == slot) {
            Some((_, name)) => names.push(name.clone()),
            None => return Vec::new(),
        }
        slot += width;
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工拼一个最小的 class 文件：`public class A extends java/lang/Object`，
    /// 带一个字段 `int x` 和一个方法 `void f(long, int[])`。
    fn tiny_class() -> Vec<u8> {
        fn utf8(out: &mut Vec<u8>, s: &str) {
            out.push(1);
            out.extend_from_slice(&(s.len() as u16).to_be_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 61]);
        out.extend_from_slice(&9u16.to_be_bytes()); // 常量池大小 = 条目数 + 1
        utf8(&mut out, "A"); // 1
        out.extend_from_slice(&[7, 0, 1]); // 2: Class A
        utf8(&mut out, "java/lang/Object"); // 3
        out.extend_from_slice(&[7, 0, 3]); // 4: Class Object
        utf8(&mut out, "x"); // 5
        utf8(&mut out, "I"); // 6
        utf8(&mut out, "f"); // 7
        utf8(&mut out, "(J[I)V"); // 8
        out.extend_from_slice(&access::PUBLIC.to_be_bytes());
        out.extend_from_slice(&[0, 2, 0, 4, 0, 0]); // this, super, 0 个接口
        out.extend_from_slice(&[0, 1]); // 1 个字段
        out.extend_from_slice(&[0, 2, 0, 5, 0, 6, 0, 0]);
        out.extend_from_slice(&[0, 1]); // 1 个方法
        out.extend_from_slice(&[0, 1, 0, 7, 0, 8, 0, 0]);
        out.extend_from_slice(&[0, 0]); // 0 个类属性
        out
    }

    #[test]
    fn parses_module_info() {
        fn utf8(out: &mut Vec<u8>, s: &str) {
            out.push(1);
            out.extend_from_slice(&(s.len() as u16).to_be_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 53]);
        out.extend_from_slice(&12u16.to_be_bytes());
        utf8(&mut out, "module-info"); // 1
        out.extend_from_slice(&[7, 0, 1]); // 2
        utf8(&mut out, "Module"); // 3
        utf8(&mut out, "m.a"); // 4
        out.extend_from_slice(&[19, 0, 4]); // 5: Module m.a
        utf8(&mut out, "java.base"); // 6
        out.extend_from_slice(&[19, 0, 6]); // 7: Module java.base
        utf8(&mut out, "p/q"); // 8
        out.extend_from_slice(&[20, 0, 8]); // 9: Package p/q
        utf8(&mut out, "m.b"); // 10
        out.extend_from_slice(&[19, 0, 10]); // 11: Module m.b
        out.extend_from_slice(&[0x80, 0x00, 0, 2, 0, 0, 0, 0]); // access, this, super, 0 个接口
        out.extend_from_slice(&[0, 0, 0, 0]); // 0 个字段、0 个方法
        out.extend_from_slice(&[0, 1, 0, 3]); // 1 个属性：Module
        let body: Vec<u8> = vec![
            0, 5, 0, 0, 0, 0, // 名字、flags、版本
            0, 1, 0, 7, 0x00, 0x20, 0, 0, // requires transitive java.base
            0, 1, 0, 9, 0, 0, 0, 1, 0, 11, // exports p.q to m.b
            0, 0, 0, 0, 0, 0, // opens、uses、provides
        ];
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        let module = parse_module_info(&out).unwrap();
        assert_eq!(module.name, "m.a");
        assert_eq!(module.requires, vec![("java.base".to_owned(), true)]);
        assert_eq!(
            module.exports,
            vec![("p/q".to_owned(), vec!["m.b".to_owned()])]
        );
        assert!(module.opens.is_empty());
    }

    #[test]
    fn parses_minimal_class() {
        let bytes = tiny_class();
        let class = parse_class(&bytes).unwrap();
        assert_eq!(class.name, "A");
        assert_eq!(class.super_name.as_deref(), Some("java/lang/Object"));
        assert_eq!(class.fields.len(), 1);
        assert_eq!(class.fields[0].descriptor, "I");
        assert_eq!(class.methods[0].descriptor, "(J[I)V");

        let header = parse_header(&bytes).unwrap();
        assert_eq!(header.name, "A");
        assert_eq!(header.access, access::PUBLIC);
    }

    #[test]
    fn truncated_input_is_an_error_not_a_panic() {
        let bytes = tiny_class();
        for len in 0..bytes.len() {
            assert!(parse_class(&bytes[..len]).is_err());
        }
    }

    #[test]
    fn lvt_slots_account_for_wide_types() {
        let method = MethodInfo {
            access: 0,
            descriptor: "(JLjava/lang/String;[DI)V".into(),
            ..MethodInfo::default()
        };
        let lvt = vec![
            (0, "this".to_string()),
            (1, "a".to_string()),
            (3, "b".to_string()),
            (4, "c".to_string()),
            (5, "d".to_string()),
        ];
        assert_eq!(
            param_names_from_lvt(&method, &lvt),
            vec!["a", "b", "c", "d"]
        );
    }
}
