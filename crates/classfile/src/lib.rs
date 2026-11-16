//! 二进制类（jar / JDK）的读取与索引。
//!
//! - [`parse_class`]：把单个 `.class` 解析成签名级的 [`ClassFile`]。
//! - [`ClassContainer`]：类的来源（jar、`ct.sym`、jimage）。
//! - [`ClassPath`]：把若干容器合并成一个按二进制名查找的索引。每个容器的"类头摘要"
//!   首次解析后写入磁盘缓存，之后启动直接读取，不再解压任何 class 文件；
//!   完整的 [`ClassFile`] 则在真正用到某个类时才按需读取。

mod cache;
mod container;
mod jimage;
mod reader;
pub mod signature;
mod sources;

use std::sync::{Arc, OnceLock};

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

pub use crate::sources::read_source;
pub use crate::{
    container::{ClassContainer, CtSymContainer, JImageContainer, JarContainer},
    reader::{
        access, parse_class, parse_header, parse_module_info, ClassFile, ClassFileError,
        ClassHeader, ConstValue, FieldInfo, InnerClassInfo, MethodInfo, ModuleDescriptor,
        RecordComponentInfo,
    },
};

/// 类头摘要：足以回答"有哪些类""谁继承了谁"，不需要读完整的 class 文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassSummary {
    pub name: SmolStr,
    pub access: u16,
    pub super_name: Option<SmolStr>,
    pub interfaces: Vec<SmolStr>,
}

impl ClassSummary {
    /// 匿名类、局部类和编译器合成类：名字里某一段以数字开头（`Outer$1`、`Outer$1Local`）。
    pub fn is_anonymous_or_local(&self) -> bool {
        self.name
            .split('$')
            .skip(1)
            .any(|segment| segment.chars().next().is_some_and(|c| c.is_ascii_digit()))
    }

    pub fn is_public(&self) -> bool {
        self.access & access::PUBLIC != 0
    }

    /// 简单名（最后一个 `/` 或 `$` 之后的部分）。
    pub fn simple_name(&self) -> &str {
        simple_name(&self.name)
    }
}

/// 二进制名的简单名部分。
pub fn simple_name(binary_name: &str) -> &str {
    let start = binary_name.rfind(['/', '$']).map_or(0, |i| i + 1);
    &binary_name[start..]
}

/// 二进制名的包部分（斜杠形式），默认包为空串。
pub fn package_of(binary_name: &str) -> &str {
    binary_name.rfind('/').map_or("", |i| &binary_name[..i])
}

#[derive(Clone, Copy)]
struct ClassEntry {
    container: u32,
    summary: u32,
}

/// 合并后的类路径索引。靠前的容器优先（同名类取第一个）。
#[derive(Default)]
pub struct ClassPath {
    containers: Vec<Arc<dyn ClassContainer>>,
    classes: FxHashMap<SmolStr, ClassEntry>,
    summaries: Vec<ClassSummary>,
    /// 包（斜杠形式）→ 该包下的顶层类简单名
    package_classes: FxHashMap<SmolStr, Vec<SmolStr>>,
    /// 包 → 直接子包的最后一段名字
    subpackages: FxHashMap<SmolStr, Vec<SmolStr>>,
    packages: FxHashSet<SmolStr>,
    /// 父类型二进制名 → 直接子类型在 `summaries` 里的下标
    subtypes: FxHashMap<SmolStr, Vec<u32>>,
    /// 各容器里的模块声明（同名的取靠前的），第一次用到时才读
    modules: OnceLock<Vec<ModuleDescriptor>>,
}

impl std::fmt::Debug for ClassPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClassPath")
            .field("containers", &self.containers.len())
            .field("classes", &self.classes.len())
            .finish()
    }
}

impl ClassPath {
    /// 并行索引所有容器（命中磁盘缓存的直接读取），然后合并。
    pub fn build(containers: Vec<Arc<dyn ClassContainer>>) -> ClassPath {
        let indexed: Vec<Vec<ClassSummary>> = containers
            .par_iter()
            .map(|c| cache::load_or_index(c.as_ref()))
            .collect();

        let mut res = ClassPath {
            containers,
            ..ClassPath::default()
        };
        for (container_idx, summaries) in indexed.into_iter().enumerate() {
            for summary in summaries {
                if res.classes.contains_key(&summary.name) {
                    continue;
                }
                let summary_idx = res.summaries.len() as u32;
                res.classes.insert(
                    summary.name.clone(),
                    ClassEntry {
                        container: container_idx as u32,
                        summary: summary_idx,
                    },
                );
                res.register_package(&summary.name);
                if !summary.is_anonymous_or_local() {
                    for parent in summary.super_name.iter().chain(summary.interfaces.iter()) {
                        res.subtypes
                            .entry(parent.clone())
                            .or_default()
                            .push(summary_idx);
                    }
                }
                res.summaries.push(summary);
            }
        }
        for names in res.package_classes.values_mut() {
            names.sort_unstable();
        }
        for names in res.subpackages.values_mut() {
            names.sort_unstable();
            names.dedup();
        }
        res
    }

    fn register_package(&mut self, binary_name: &str) {
        let package = package_of(binary_name);
        let simple = &binary_name[if package.is_empty() {
            0
        } else {
            package.len() + 1
        }..];
        if !simple.contains('$') {
            self.package_classes
                .entry(SmolStr::new(package))
                .or_default()
                .push(SmolStr::new(simple));
        }
        // 登记包及其所有祖先包
        let mut current = package;
        while !current.is_empty() && self.packages.insert(SmolStr::new(current)) {
            let (parent, last) = match current.rfind('/') {
                Some(i) => (&current[..i], &current[i + 1..]),
                None => ("", current),
            };
            self.subpackages
                .entry(SmolStr::new(parent))
                .or_default()
                .push(SmolStr::new(last));
            current = parent;
        }
    }

    pub fn len(&self) -> usize {
        self.summaries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.summaries.is_empty()
    }

    pub fn contains(&self, binary_name: &str) -> bool {
        self.classes.contains_key(binary_name)
    }

    pub fn summary(&self, binary_name: &str) -> Option<&ClassSummary> {
        self.classes
            .get(binary_name)
            .map(|e| &self.summaries[e.summary as usize])
    }

    pub fn summaries(&self) -> &[ClassSummary] {
        &self.summaries
    }

    /// 读取并解析完整的 class 文件。
    pub fn load(&self, binary_name: &str) -> Option<ClassFile> {
        let entry = self.classes.get(binary_name)?;
        let bytes = self.containers[entry.container as usize].read_class(binary_name)?;
        match parse_class(&bytes) {
            Ok(class) => Some(class),
            Err(err) => {
                tracing::warn!("解析 {binary_name} 失败: {err}");
                None
            }
        }
    }

    /// 每个容器里名为 `entry` 的资源（按容器顺序），附带容器的磁盘路径。
    pub fn read_resources(&self, entry: &str) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        self.containers
            .par_iter()
            .filter_map(|c| Some((c.path().to_path_buf(), c.read_resource(entry)?)))
            .collect()
    }

    /// 类所在容器对应的磁盘路径（jar 文件等）。
    pub fn container_path(&self, binary_name: &str) -> Option<&std::path::Path> {
        let entry = self.classes.get(binary_name)?;
        Some(self.containers[entry.container as usize].path())
    }

    pub fn has_package(&self, package: &str) -> bool {
        self.packages.contains(package)
    }

    /// 包下的顶层类简单名（已排序）。
    pub fn package_classes(&self, package: &str) -> &[SmolStr] {
        self.package_classes.get(package).map_or(&[], Vec::as_slice)
    }

    /// 直接子包的最后一段名字（已排序）；`""` 表示根。
    pub fn subpackages(&self, package: &str) -> &[SmolStr] {
        self.subpackages.get(package).map_or(&[], Vec::as_slice)
    }

    /// 类路径上的模块：JDK 的各个模块、模块化 jar、普通 jar 对应的自动模块。
    pub fn modules(&self) -> &[ModuleDescriptor] {
        self.modules.get_or_init(|| {
            let mut res: Vec<ModuleDescriptor> = Vec::new();
            let per_container: Vec<Vec<ModuleDescriptor>> =
                self.containers.par_iter().map(|c| c.modules()).collect();
            for module in per_container.into_iter().flatten() {
                if !res.iter().any(|m| m.name == module.name) {
                    res.push(module);
                }
            }
            res
        })
    }

    /// 直接继承 / 实现了给定类型的类。
    pub fn direct_subtypes(&self, binary_name: &str) -> impl Iterator<Item = &ClassSummary> {
        self.subtypes
            .get(binary_name)
            .into_iter()
            .flatten()
            .map(|idx| &self.summaries[*idx as usize])
    }
}

/// 不经过缓存，直接读容器里每个类的类头。
pub(crate) fn index_container(container: &dyn ClassContainer) -> Vec<ClassSummary> {
    let mut res = Vec::new();
    for name in container.class_names() {
        let Some(bytes) = container.read_class(&name) else {
            continue;
        };
        match parse_header(&bytes) {
            Ok(header) => res.push(ClassSummary {
                name: SmolStr::new(&header.name),
                access: header.access,
                super_name: header.super_name.as_deref().map(SmolStr::new),
                interfaces: header.interfaces.iter().map(SmolStr::new).collect(),
            }),
            Err(err) => tracing::debug!("跳过 {name}: {err}"),
        }
    }
    res.sort_by(|a, b| a.name.cmp(&b.name));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_helpers() {
        assert_eq!(simple_name("java/util/Map$Entry"), "Entry");
        assert_eq!(simple_name("Foo"), "Foo");
        assert_eq!(package_of("java/util/Map$Entry"), "java/util");
        assert_eq!(package_of("Foo"), "");
    }

    #[test]
    fn automatic_module_names() {
        use crate::container::automatic_module_name;
        assert_eq!(
            automatic_module_name("commons-lang3-3.12.0.jar", None).as_deref(),
            Some("commons.lang3")
        );
        assert_eq!(
            automatic_module_name("guava-33.0-jre.jar", None).as_deref(),
            Some("guava")
        );
        assert_eq!(
            automatic_module_name(
                "x.jar",
                Some("Manifest-Version: 1.0\r\nAutomatic-Module-Name: org.example.x\r\n")
            )
            .as_deref(),
            Some("org.example.x")
        );
    }

    #[test]
    fn anonymous_detection() {
        let mk = |name: &str| ClassSummary {
            name: SmolStr::new(name),
            access: 0,
            super_name: None,
            interfaces: vec![],
        };
        assert!(mk("a/B$1").is_anonymous_or_local());
        assert!(mk("a/B$1Local").is_anonymous_or_local());
        assert!(!mk("a/B$Inner").is_anonymous_or_local());
        assert!(!mk("a/B").is_anonymous_or_local());
    }
}
