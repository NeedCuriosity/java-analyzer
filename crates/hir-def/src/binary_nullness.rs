//! class 文件里和空值默认值有关的注解：包（`package-info.class`）、模块（`module-info.class`）
//! 上的作用域注解，类和方法上带参数的作用域注解，以及方法和形参上的全部注解。
//!
//! 签名层（[`ClassData`](crate::ClassData)）只留下了认识的空值注解；空值分析给依赖库里的类
//! 套用 `@NullMarked` 一类的默认值时才按需读这些。

use std::sync::Arc;

use base_db::{workspace, Db};
use classfile::{access, signature, ClassFile};
use smol_str::SmolStr;

use crate::{
    data::ClassId,
    index::{class_origin, ClassOrigin},
};

/// 一个注解：二进制名（`org/jspecify/annotations/NullMarked`）和读出来的元素值
/// （只有 `@DefaultQualifier`、`@NonNullByDefault` 这类带参数的作用域注解才有）。
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct BinaryAnnotation {
    pub name: SmolStr,
    /// `(元素名, 取值)`：枚举常量名、类字面量的描述符、`true` / `false`，数组按元素展开
    pub elements: Vec<(SmolStr, Vec<SmolStr>)>,
}

/// class 文件里一个方法上的注解。
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct BinaryMethodAnnotations {
    /// class 文件里的名字（构造器是 `<init>`）
    pub name: SmolStr,
    /// 方法上的注解，加上返回类型上（含数组元素、嵌套类型）的类型注解
    pub annotations: Vec<BinaryAnnotation>,
    /// 各形参上的注解（含形参类型内部的数组元素、嵌套类型上的类型注解），和
    /// `MethodData::params` 一一对应
    pub params: Vec<Vec<SmolStr>>,
}

/// class 文件里的类上和它的方法上的注解。
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct BinaryNullAnnotations {
    /// 类上的注解
    pub class: Vec<BinaryAnnotation>,
    /// 和 `ClassData::methods` 一一对应
    pub methods: Vec<BinaryMethodAnnotations>,
}

fn binary_name_of(desc: &str) -> SmolStr {
    SmolStr::new(
        desc.strip_prefix('L')
            .and_then(|d| d.strip_suffix(';'))
            .unwrap_or(desc),
    )
}

/// 注解描述符的列表 + 读出来的元素值 → [`BinaryAnnotation`] 的列表。
fn annotations_of(descs: &[String], args: &[classfile::AnnotationArgs]) -> Vec<BinaryAnnotation> {
    descs
        .iter()
        .map(|desc| BinaryAnnotation {
            name: binary_name_of(desc),
            elements: args
                .iter()
                .find(|a| a.desc == *desc)
                .map(|a| {
                    a.elements
                        .iter()
                        .map(|(name, values)| {
                            (
                                SmolStr::new(name),
                                values.iter().map(SmolStr::new).collect(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// 类路径上的类在 class 文件里的注解。源码类（以及读不出 class 文件的类）返回 `None`。
pub fn binary_null_annotations(db: &dyn Db, class: ClassId) -> Option<Arc<BinaryNullAnnotations>> {
    binary_null_annotations_tracked(db, workspace(db), class)
}

#[salsa::tracked]
fn binary_null_annotations_tracked(
    db: &dyn Db,
    _ws: base_db::Workspace,
    class: ClassId,
) -> Option<Arc<BinaryNullAnnotations>> {
    let ClassOrigin::Binary { container } = class_origin(db, class)? else {
        return None;
    };
    let name = class.binary_name(db);
    let file = workspace(db).class_path(db).load_from(container, &name)?;
    Some(Arc::new(lower(&file)))
}

fn lower(file: &ClassFile) -> BinaryNullAnnotations {
    let self_inner = file.inner_classes.iter().find(|ic| ic.inner == file.name);
    let class_access = self_inner.map_or(file.access, |ic| ic.access);
    let is_inner_instance_class =
        self_inner.is_some_and(|ic| ic.outer.is_some()) && class_access & access::STATIC == 0;
    // 和 `lower_class_file` 用同样的条件挑方法，下标才能对上
    let methods = file
        .methods
        .iter()
        .filter(|m| m.access & (access::SYNTHETIC | access::BRIDGE) == 0 && m.name != "<clinit>")
        .filter_map(|m| {
            let generic = m.signature.as_deref().and_then(signature::parse_method_sig);
            let has_generic_sig = generic.is_some();
            let sig = generic.or_else(|| signature::parse_method_sig(&m.descriptor))?;
            let mut n_params = sig.params.len();
            if m.name == "<init>" && is_inner_instance_class && !has_generic_sig && n_params > 0 {
                n_params -= 1;
            }
            // 形参注解的个数可能和参数个数不同（编译器合成的参数），按末尾对齐
            let n_annotated = m.param_annotations.len();
            let params = (0..n_params)
                .map(|i| {
                    let mut names: Vec<SmolStr> = (i + n_annotated)
                        .checked_sub(n_params)
                        .and_then(|j| m.param_annotations.get(j))
                        .into_iter()
                        .chain(m.param_type_annotations.get(i))
                        .flatten()
                        .map(|d| binary_name_of(d))
                        .collect();
                    names.extend(
                        m.inner_type_annotations
                            .iter()
                            .filter(|(pos, _)| pos.map(usize::from) == Some(i))
                            .map(|(_, d)| binary_name_of(d)),
                    );
                    names
                })
                .collect();
            let mut annotations = annotations_of(&m.annotations, &m.annotation_args);
            annotations.extend(
                m.inner_type_annotations
                    .iter()
                    .filter(|(pos, _)| pos.is_none())
                    .map(|(_, d)| BinaryAnnotation {
                        name: binary_name_of(d),
                        elements: Vec::new(),
                    }),
            );
            Some(BinaryMethodAnnotations {
                name: SmolStr::new(&m.name),
                annotations,
                params,
            })
        })
        .collect();
    BinaryNullAnnotations {
        class: annotations_of(&file.annotations, &file.annotation_args),
        methods,
    }
}

/// 类路径上的类所在的包在同一个容器里的 `package-info.class` 上的注解。
pub fn binary_package_annotations(db: &dyn Db, class: ClassId) -> Arc<Vec<BinaryAnnotation>> {
    let Some(ClassOrigin::Binary { container }) = class_origin(db, class) else {
        return Arc::default();
    };
    let name = class.binary_name(db);
    let package = SmolStr::new(classfile::package_of(&name));
    package_annotations_tracked(db, workspace(db), container, package)
}

#[salsa::tracked]
fn package_annotations_tracked(
    db: &dyn Db,
    _ws: base_db::Workspace,
    container: u32,
    package: SmolStr,
) -> Arc<Vec<BinaryAnnotation>> {
    let info = if package.is_empty() {
        "package-info".to_owned()
    } else {
        format!("{package}/package-info")
    };
    let annotations = workspace(db)
        .class_path(db)
        .load_from(container, &info)
        .map(|file| annotations_of(&file.annotations, &file.annotation_args))
        .unwrap_or_default();
    Arc::new(annotations)
}

/// 类路径上的类所在的容器（模块化 jar）的 `module-info.class` 上的注解。
pub fn binary_module_annotations(db: &dyn Db, class: ClassId) -> Arc<Vec<BinaryAnnotation>> {
    let Some(ClassOrigin::Binary { container }) = class_origin(db, class) else {
        return Arc::default();
    };
    module_annotations_tracked(db, workspace(db), container)
}

#[salsa::tracked]
fn module_annotations_tracked(
    db: &dyn Db,
    _ws: base_db::Workspace,
    container: u32,
) -> Arc<Vec<BinaryAnnotation>> {
    let class_path = workspace(db).class_path(db);
    // 多版本 jar 常把 `module-info.class` 放在 `META-INF/versions/N/` 下
    let file = std::iter::once("module-info".to_owned())
        .chain((9..=30).map(|v| format!("META-INF/versions/{v}/module-info")))
        .find_map(|name| class_path.load_from(container, &name));
    let annotations = file
        .map(|file| annotations_of(&file.annotations, &file.annotation_args))
        .unwrap_or_default();
    Arc::new(annotations)
}
