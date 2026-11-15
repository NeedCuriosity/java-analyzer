//! JDK 运行时镜像（jimage，`$JAVA_HOME/lib/modules`）的只读解析。
//!
//! 文件布局（按 OpenJDK `imageFile.hpp`，本实现尚未对着真实镜像验证）：
//!
//! ```text
//! header        7 × u32：magic, version, flags, resource_count, table_length,
//!                         locations_size, strings_size
//! redirect      table_length × i32
//! offsets       table_length × u32   每个资源的 location 属性在 locations 区的偏移
//! locations     locations_size 字节  属性流
//! strings       strings_size 字节    以 NUL 结尾的字符串表
//! resources     紧随其后
//! ```
//!
//! location 属性流：每个属性一个头字节 `(kind << 3) | (len - 1)`，后跟 `len` 字节大端数值；
//! kind 为 0 表示结束。

use std::{fs::File, io, path::Path};

use memmap2::Mmap;
use rustc_hash::FxHashMap;

const MAGIC: u32 = 0xCAFE_DADA;
const HEADER_SIZE: usize = 28;

const ATTR_END: u8 = 0;
const ATTR_MODULE: u8 = 1;
const ATTR_PARENT: u8 = 2;
const ATTR_BASE: u8 = 3;
const ATTR_EXTENSION: u8 = 4;
const ATTR_OFFSET: u8 = 5;
const ATTR_COMPRESSED: u8 = 6;
const ATTR_UNCOMPRESSED: u8 = 7;

#[derive(Clone, Copy)]
struct Resource {
    offset: u64,
    compressed: u64,
    uncompressed: u64,
}

pub struct JImage {
    mmap: Mmap,
    /// 资源区在文件里的起始偏移
    data_start: usize,
    classes: FxHashMap<String, Resource>,
    /// 各模块的 `module-info.class`
    module_infos: Vec<Resource>,
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl JImage {
    pub fn open(path: &Path) -> io::Result<JImage> {
        let file = File::open(path)?;
        // SAFETY: 镜像文件是 JDK 安装的一部分，运行期间不会被修改；
        // 即便被外部截断，后续访问也都经过边界检查的切片。
        let mmap = unsafe { Mmap::map(&file)? };
        let data: &[u8] = &mmap;
        if data.len() < HEADER_SIZE {
            return Err(invalid("jimage 文件过短"));
        }

        let raw_magic = [data[0], data[1], data[2], data[3]];
        let little_endian = if u32::from_le_bytes(raw_magic) == MAGIC {
            true
        } else if u32::from_be_bytes(raw_magic) == MAGIC {
            false
        } else {
            return Err(invalid("jimage 魔数不对"));
        };
        let u32_at = |pos: usize| -> io::Result<u32> {
            let b = data
                .get(pos..pos + 4)
                .ok_or_else(|| invalid("jimage 越界"))?;
            let b = [b[0], b[1], b[2], b[3]];
            Ok(if little_endian {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            })
        };

        let table_length = u32_at(16)? as usize;
        let locations_size = u32_at(20)? as usize;
        let strings_size = u32_at(24)? as usize;

        let redirect_start = HEADER_SIZE;
        let offsets_start = redirect_start + table_length * 4;
        let locations_start = offsets_start + table_length * 4;
        let strings_start = locations_start + locations_size;
        let data_start = strings_start + strings_size;
        if data_start > data.len() {
            return Err(invalid("jimage 索引区越界"));
        }
        let locations = &data[locations_start..strings_start];
        let strings = &data[strings_start..data_start];

        let string_at = |offset: u64| -> Option<&str> {
            let tail = strings.get(offset as usize..)?;
            let end = tail.iter().position(|b| *b == 0)?;
            std::str::from_utf8(&tail[..end]).ok()
        };

        let mut classes = FxHashMap::default();
        let mut module_infos = Vec::new();
        for i in 0..table_length {
            let loc_offset = u32_at(offsets_start + i * 4)? as usize;
            let Some(mut attrs) = locations.get(loc_offset..) else {
                continue;
            };

            let mut values = [0u64; 8];
            while let Some((&head, rest)) = attrs.split_first() {
                let kind = head >> 3;
                if kind == ATTR_END || kind as usize >= values.len() {
                    break;
                }
                let len = (head & 0x7) as usize + 1;
                let Some(bytes) = rest.get(..len) else { break };
                values[kind as usize] = bytes.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64);
                attrs = &rest[len..];
            }

            let (Some(module), Some(parent), Some(base), Some(extension)) = (
                string_at(values[ATTR_MODULE as usize]),
                string_at(values[ATTR_PARENT as usize]),
                string_at(values[ATTR_BASE as usize]),
                string_at(values[ATTR_EXTENSION as usize]),
            ) else {
                continue;
            };
            // `/modules`、`/packages` 下的是元数据条目，不是类
            if extension != "class"
                || module.is_empty()
                || module == "modules"
                || module == "packages"
            {
                continue;
            }
            let resource = Resource {
                offset: values[ATTR_OFFSET as usize],
                compressed: values[ATTR_COMPRESSED as usize],
                uncompressed: values[ATTR_UNCOMPRESSED as usize],
            };
            if parent.is_empty() && base == "module-info" {
                module_infos.push(resource);
                continue;
            }
            let binary_name = if parent.is_empty() {
                base.to_owned()
            } else {
                format!("{parent}/{base}")
            };
            classes.entry(binary_name).or_insert(resource);
        }

        Ok(JImage {
            mmap,
            data_start,
            classes,
            module_infos,
        })
    }

    /// 各模块 `module-info.class` 的内容。
    pub fn module_infos(&self) -> Vec<Vec<u8>> {
        self.module_infos
            .iter()
            .filter_map(|resource| self.read_resource(resource))
            .collect()
    }

    pub fn class_names(&self) -> impl Iterator<Item = &str> {
        self.classes.keys().map(String::as_str)
    }

    pub fn read_class(&self, binary_name: &str) -> Option<Vec<u8>> {
        let resource = self.classes.get(binary_name)?;
        self.read_resource(resource)
    }

    fn read_resource(&self, resource: &Resource) -> Option<Vec<u8>> {
        if resource.compressed != 0 {
            // 用 `jlink --compress` 生成的镜像才会压缩；标准 JDK 发行版不压缩。
            tracing::debug!("跳过被压缩的 jimage 资源");
            return None;
        }
        let start = self.data_start.checked_add(resource.offset as usize)?;
        let end = start.checked_add(resource.uncompressed as usize)?;
        self.mmap.get(start..end).map(<[u8]>::to_vec)
    }
}
