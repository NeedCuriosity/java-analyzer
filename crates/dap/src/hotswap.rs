//! 热替换：把重新编译过的类通过 JDWP `VirtualMachine.RedefineClasses` 换进运行中的 JVM。
//!
//! 会话开始时记下编译输出目录里每个 class 文件的修改时间和大小。自定义请求 `redefineClasses`
//! 找出之后变化了的、而且 JVM 已经加载了的类，一次替换；一批里有替换不了的类（JVM 一般只允许改方法体）
//! 时逐个重试，能换的照样换上，换不了的给出原因。JDWP 替换类时会清掉类里的断点，替换后重新装上。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::Context;
use base_db::{tr, trs};
use serde_json::{json, Value};

use crate::{
    jdwp::{self, JdwpError},
    session::Session,
};

/// `CapabilitiesNew` 里 `canRedefineClasses` 的下标
const CAN_REDEFINE_CLASSES: usize = 7;

/// 编译输出目录里 class 文件的修改时间和大小。
#[derive(Debug, Default)]
pub(crate) struct ClassFiles {
    /// 会话的编译输出目录（`redefineClasses` 不带参数时检查这些）
    pub dirs: Vec<PathBuf>,
    /// 记录开始的时间：没有记录过的文件，在这之后写入的才算变化
    since: Option<SystemTime>,
    stamps: HashMap<PathBuf, (SystemTime, u64)>,
}

/// 文件系统记录 mtime 用的是粗粒度的时钟（Linux 上是时钟节拍），记录之后马上写入的文件，
/// mtime 可能比 `SystemTime::now()` 取到的记录时间还早一点，比较时留出这么多余量。
const MTIME_SLACK: Duration = Duration::from_secs(1);

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// 目录下（递归）所有的 class 文件。
fn class_files_in(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            class_files_in(&path, out);
        } else if path.extension().is_some_and(|e| e == "class") {
            out.push(path);
        }
    }
}

impl ClassFiles {
    /// 记下这些目录里 class 文件现在的状态。
    pub fn record(dirs: Vec<PathBuf>) -> ClassFiles {
        let mut files = ClassFiles {
            dirs,
            since: Some(SystemTime::now()),
            stamps: HashMap::new(),
        };
        let mut paths = Vec::new();
        for dir in &files.dirs {
            class_files_in(dir, &mut paths);
        }
        for path in paths {
            if let Some(stamp) = stamp(&path) {
                files.stamps.insert(path, stamp);
            }
        }
        files
    }

    /// 这些目录里记录之后变化了（或新出现）的 class 文件。
    pub fn changed(&self, dirs: &[PathBuf]) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for dir in dirs {
            class_files_in(dir, &mut paths);
        }
        paths.sort();
        paths.dedup();
        paths.retain(|path| {
            let Some(now) = stamp(path) else {
                return false;
            };
            match self.stamps.get(path) {
                Some(old) => *old != now,
                None => self.since.is_none_or(|since| now.0 + MTIME_SLACK >= since),
            }
        });
        paths
    }

    /// 文件已经处理过（换上了，或者类还没加载、以后会直接加载新版本）。
    pub fn mark_current(&mut self, path: &Path) {
        if let Some(stamp) = stamp(path) {
            self.stamps.insert(path.to_path_buf(), stamp);
        }
    }
}

/// 按顺序读 class 文件的字节。
struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(slice)
    }

    fn u2(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
}

/// class 文件里类的内部名（`demo/Outer$Inner`）：读常量池找到 `this_class`。
pub(crate) fn class_name_of(bytes: &[u8]) -> Option<String> {
    let mut r = Bytes {
        data: bytes,
        pos: 0,
    };
    if r.take(4)? != [0xCA, 0xFE, 0xBA, 0xBE] {
        return None;
    }
    r.take(4)?; // minor、major 版本
    let count = r.u2()? as usize;
    let mut utf8: HashMap<usize, String> = HashMap::new();
    let mut class_names: HashMap<usize, usize> = HashMap::new();
    let mut index = 1;
    while index < count {
        let tag = r.take(1)?[0];
        match tag {
            1 => {
                let len = r.u2()? as usize;
                let text = String::from_utf8_lossy(r.take(len)?).into_owned();
                utf8.insert(index, text);
            }
            7 => {
                class_names.insert(index, r.u2()? as usize);
            }
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => {
                r.take(4)?;
            }
            5 | 6 => {
                r.take(8)?;
                // long 和 double 占两个常量池位置
                index += 1;
            }
            8 | 16 | 19 | 20 => {
                r.take(2)?;
            }
            15 => {
                r.take(3)?;
            }
            _ => return None,
        }
        index += 1;
    }
    r.take(2)?; // access_flags
    let this_class = r.u2()? as usize;
    utf8.get(class_names.get(&this_class)?).cloned()
}

/// 替换失败的原因，给人看的。
fn failure_reason(err: &anyhow::Error) -> String {
    let Some(code) = err.downcast_ref::<JdwpError>() else {
        return format!("{err:#}");
    };
    let structure = trs!(
        "热替换只能修改方法体，需要重启调试会话",
        "hot swap can only change method bodies; restart the debug session"
    );
    let reason = match code.0 {
        60 => trs!("class 文件格式不正确", "The class file is malformed").to_owned(),
        61 => trs!(
            "类的继承关系出现了循环",
            "The class hierarchy has a cycle"
        )
        .to_owned(),
        62 => trs!(
            "新的字节码没有通过校验",
            "The new bytecode failed verification"
        )
        .to_owned(),
        63 => tr!("增加了方法；{structure}", "A method was added; {structure}"),
        64 => tr!(
            "增加、删除或修改了字段；{structure}",
            "A field was added, removed or changed; {structure}"
        ),
        66 => tr!(
            "改变了父类或实现的接口；{structure}",
            "The superclass or the implemented interfaces changed; {structure}"
        ),
        67 => tr!("删除了方法；{structure}", "A method was removed; {structure}"),
        68 => trs!(
            "class 文件的版本高于目标 JVM 支持的版本",
            "The class file version is newer than the target JVM supports"
        )
        .to_owned(),
        69 => trs!(
            "class 文件里的类名和要替换的类不一致",
            "The class name in the class file does not match the class being replaced"
        )
        .to_owned(),
        70 => tr!(
            "修改了类的修饰符；{structure}",
            "The class modifiers changed; {structure}"
        ),
        71 => tr!(
            "修改了方法的修饰符；{structure}",
            "The method modifiers changed; {structure}"
        ),
        72 => tr!(
            "修改了类的属性（例如嵌套类、记录组件、密封类的子类）；{structure}",
            "The class attributes changed (for example nested classes, record components or permitted subclasses); {structure}"
        ),
        jdwp::error_code::INVALID_CLASS => {
            trs!("类已经被卸载", "The class has been unloaded").to_owned()
        }
        99 => trs!(
            "目标 JVM 不支持热替换",
            "The target JVM does not support hot swapping"
        )
        .to_owned(),
        _ => return code.to_string(),
    };
    tr!("{reason}（{code}）", "{reason} ({code})")
}

/// 一个要替换的 class 文件。
struct Candidate {
    file: PathBuf,
    /// Java 写法的类名
    name: String,
    bytes: Vec<u8>,
    /// JVM 里这个名字的类（不同的类加载器各有一个）
    classes: Vec<u64>,
}

impl Session {
    fn can_redefine(&mut self) -> anyhow::Result<bool> {
        if let Some(can) = self.can_redefine {
            return Ok(can);
        }
        let can = self
            .vm()?
            .capabilities()?
            .get(CAN_REDEFINE_CLASSES)
            .copied()
            .unwrap_or(false);
        self.can_redefine = Some(can);
        Ok(can)
    }

    /// 自定义请求 `redefineClasses`。
    ///
    /// 参数（都可省略）：`classFiles` 指定要替换的 class 文件；否则检查 `outputDirs`
    /// （省略时是会话的编译输出目录）里会话开始（或上次替换）后变化了的 class 文件。
    /// 结果里每个类一项：`{ name, file, status: "redefined" | "failed", message? }`；
    /// 变化了但 JVM 还没有加载的类不用替换，只计数（`notLoaded`）。
    pub(crate) fn redefine_classes(&mut self, args: &Value) -> anyhow::Result<Value> {
        let vm = self.vm()?;
        anyhow::ensure!(
            self.can_redefine()?,
            trs!(
                "目标 JVM 不支持热替换（canRedefineClasses 为 false）",
                "The target JVM does not support hot swapping (canRedefineClasses is false)"
            )
        );
        let paths = |key: &str| -> Option<Vec<PathBuf>> {
            args.get(key).and_then(Value::as_array).map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect()
            })
        };
        let files = match paths("classFiles") {
            Some(files) => files,
            None => {
                let dirs = paths("outputDirs").unwrap_or_else(|| self.class_files.dirs.clone());
                self.class_files.changed(&dirs)
            }
        };

        let mut results: Vec<Value> = Vec::new();
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut not_loaded = 0usize;
        for file in files {
            let bytes = match std::fs::read(&file)
                .with_context(|| tr!("无法读取 {}", "Cannot read {}", file.display()))
            {
                Ok(bytes) => bytes,
                Err(err) => {
                    results.push(failed(&file, "", &format!("{err:#}")));
                    continue;
                }
            };
            let Some(internal) = class_name_of(&bytes) else {
                results.push(failed(
                    &file,
                    "",
                    trs!("不是有效的 class 文件", "Not a valid class file"),
                ));
                continue;
            };
            let name = internal.replace('/', ".");
            // module-info、package-info 不是能替换的类
            if name.ends_with("module-info") || name.ends_with("package-info") {
                self.class_files.mark_current(&file);
                continue;
            }
            let classes: Vec<u64> = vm
                .classes_by_signature(&format!("L{internal};"))?
                .into_iter()
                .map(|(_, class, _)| class)
                .collect();
            if classes.is_empty() {
                // 还没加载：以后加载时直接用的就是新版本
                self.class_files.mark_current(&file);
                not_loaded += 1;
                continue;
            }
            candidates.push(Candidate {
                file,
                name,
                bytes,
                classes,
            });
        }

        let outcomes: Vec<Result<(), String>> = if candidates.is_empty() {
            Vec::new()
        } else {
            let batch: Vec<(u64, &[u8])> = candidates
                .iter()
                .flat_map(|c| c.classes.iter().map(|&class| (class, c.bytes.as_slice())))
                .collect();
            match vm.redefine_classes(&batch) {
                Ok(()) => candidates.iter().map(|_| Ok(())).collect(),
                Err(err) if candidates.len() == 1 => vec![Err(failure_reason(&err))],
                // 一批里有换不了的：逐个替换，能换的照样换上
                Err(_) => candidates
                    .iter()
                    .map(|c| {
                        let one: Vec<(u64, &[u8])> = c
                            .classes
                            .iter()
                            .map(|&class| (class, c.bytes.as_slice()))
                            .collect();
                        vm.redefine_classes(&one)
                            .map_err(|err| failure_reason(&err))
                    })
                    .collect(),
            }
        };

        let mut redefined: Vec<u64> = Vec::new();
        let mut names: Vec<&str> = Vec::new();
        for (candidate, outcome) in candidates.iter().zip(&outcomes) {
            match outcome {
                Ok(()) => {
                    self.class_files.mark_current(&candidate.file);
                    redefined.extend(&candidate.classes);
                    names.push(&candidate.name);
                    results.push(json!({
                        "name": candidate.name,
                        "file": candidate.file,
                        "status": "redefined",
                    }));
                }
                Err(reason) => {
                    self.out.output(
                        "stderr",
                        &tr!(
                            "热替换 {} 失败：{reason}\n",
                            "Hot swap of {} failed: {reason}\n",
                            candidate.name
                        ),
                    );
                    results.push(failed(&candidate.file, &candidate.name, reason));
                }
            }
        }
        if !names.is_empty() {
            self.out.output(
                "console",
                &tr!(
                    "已热替换 {} 个类：{}\n",
                    "Hot swapped {} classes: {}\n",
                    names.len(),
                    names.join(", ")
                ),
            );
        }
        if !redefined.is_empty() {
            for class in &redefined {
                self.class_sources.remove(class);
            }
            self.reinstall_breakpoints(&redefined);
        }
        Ok(json!({ "classes": results, "notLoaded": not_loaded }))
    }
}

fn failed(file: &Path, name: &str, message: &str) -> Value {
    json!({ "name": name, "file": file, "status": "failed", "message": message })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手写一个最小的 class 文件：常量池里有 long、字符串、类，`this_class` 指向 `demo/Main$1`。
    #[test]
    fn reads_this_class_from_constant_pool() {
        let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 65];
        let utf8 = |text: &str| {
            let mut entry = vec![1];
            entry.extend((text.len() as u16).to_be_bytes());
            entry.extend(text.as_bytes());
            entry
        };
        let mut pool: Vec<u8> = Vec::new();
        pool.extend(utf8("java/lang/Object")); // #1
        pool.extend([7, 0, 1]); // #2 Class #1
        pool.extend([5, 0, 0, 0, 0, 0, 0, 0, 42]); // #3、#4 long
        pool.extend(utf8("demo/Main$1")); // #5
        pool.extend([7, 0, 5]); // #6 Class #5
        pool.extend([8, 0, 1]); // #7 String
        bytes.extend(8u16.to_be_bytes());
        bytes.extend(pool);
        bytes.extend([0, 0x21]); // access_flags
        bytes.extend([0, 6]); // this_class
        bytes.extend([0, 2]); // super_class
        assert_eq!(class_name_of(&bytes).as_deref(), Some("demo/Main$1"));
        assert_eq!(class_name_of(&bytes[..20]), None);
        assert_eq!(class_name_of(b"not a class"), None);
    }

    #[test]
    fn changed_files_since_recording() {
        let dir = std::env::temp_dir().join(format!("ja-dap-hotswap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("demo")).unwrap();
        let a = dir.join("demo/A.class");
        let b = dir.join("demo/B.class");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let mut files = ClassFiles::record(vec![dir.clone()]);
        assert!(files.changed(&files.dirs).is_empty());

        std::fs::write(&a, "a2").unwrap();
        let c = dir.join("demo/C.class");
        std::fs::write(&c, "c").unwrap();
        std::fs::write(dir.join("demo/notes.txt"), "x").unwrap();
        assert_eq!(
            files.changed(std::slice::from_ref(&dir)),
            vec![a.clone(), c.clone()]
        );

        files.mark_current(&a);
        files.mark_current(&c);
        assert!(files.changed(std::slice::from_ref(&dir)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readable_failure_reasons() {
        let reason = failure_reason(&anyhow::Error::new(JdwpError(63)));
        assert!(reason.contains("增加了方法"), "{reason}");
        assert!(reason.contains("ADD_METHOD_NOT_IMPLEMENTED"), "{reason}");
        let other = failure_reason(&anyhow::anyhow!("连接断开"));
        assert_eq!(other, "连接断开");
    }
}
