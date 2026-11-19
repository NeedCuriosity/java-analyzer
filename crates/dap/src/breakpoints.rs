//! 行断点：源文件 → 类 → 方法行号表 → 代码位置。
//!
//! 设置断点时，已经加载的类直接查行号表装上断点；还没加载的类注册 `ClassPrepare` 事件
//! （类名模式 `pkg.Outer` 和 `pkg.Outer$*`，挂起事件线程），类准备好时再装，装好后发 `breakpoint`
//! 事件告诉客户端断点已生效。行上没有代码时挪到同一方法里的下一行代码。

use std::{path::Path, rc::Rc};

use base_db::{tr, trs};
use serde_json::{json, Value};

use crate::{
    eval,
    jdwp::{
        event_kind, signature_to_type_name, suspend_policy, LineTable, Location, Modifier, Vm,
        CLASS_STATUS_PREPARED,
    },
    session::Session,
    sources::SourceClasses,
};

#[derive(Debug)]
pub(crate) struct Breakpoint {
    pub id: i64,
    /// 请求的行（从 1 开始）
    pub line: i32,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
    pub hits: u64,
    /// 已设置的 JDWP 断点请求
    pub requests: Vec<i32>,
    /// 实际生效的行（可能因为请求的行没有代码而往后挪）
    pub actual_line: Option<i32>,
    /// 当前位置是挪过来的；之后加载的类里有这一行的代码时改用那里
    pub moved: bool,
}

impl Breakpoint {
    fn new(line: i32) -> Breakpoint {
        Breakpoint {
            id: 0,
            line,
            condition: None,
            hit_condition: None,
            log_message: None,
            hits: 0,
            requests: Vec::new(),
            actual_line: None,
            moved: false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SourceBreakpoints {
    pub classes: SourceClasses,
    pub prepare_requests: Vec<i32>,
    pub breakpoints: Vec<Breakpoint>,
    /// 已经处理过的类
    pub examined: Vec<u64>,
}

impl SourceBreakpoints {
    /// JVM 断开后清掉所有 JDWP 侧的状态。
    pub fn reset(&mut self) {
        self.prepare_requests.clear();
        self.examined.clear();
        for bp in &mut self.breakpoints {
            bp.requests.clear();
            bp.actual_line = None;
            bp.moved = false;
        }
    }
}

/// 某一行在行号表里对应的代码位置。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LineMatch {
    /// 实际的行
    pub line: i32,
    /// 就是请求的那一行（不是挪过来的）
    pub exact: bool,
    /// `(方法, 代码下标)`
    pub locations: Vec<(u64, u64)>,
}

/// 在行号表里找某一行的代码位置。
///
/// 请求的行有代码时，每个包含这一行代码的方法（例如方法本身和同一行上的 lambda）各取最小的代码下标。
/// 没有代码时，在行范围包含这一行（或者紧跟在这一行后面开始）的方法里找下一个有代码的行。
pub(crate) fn find_locations(tables: &[(u64, Rc<LineTable>)], line: i32) -> Option<LineMatch> {
    let first_index = |table: &LineTable, line: i32| {
        table
            .lines
            .iter()
            .filter(|(_, l)| *l == line)
            .map(|(code, _)| *code)
            .min()
    };
    let exact: Vec<(u64, u64)> = tables
        .iter()
        .filter_map(|(method, table)| first_index(table, line).map(|code| (*method, code)))
        .collect();
    if !exact.is_empty() {
        return Some(LineMatch {
            line,
            exact: true,
            locations: exact,
        });
    }
    let mut best: Option<(i32, u64)> = None;
    for (method, table) in tables {
        let Some((min, max)) = table.line_range() else {
            continue;
        };
        // 方法声明那一行（或者它上面的注解）：挪到方法体的第一行
        let contains = min <= line && line <= max;
        let just_before = min > line && min - line <= 2;
        if !contains && !just_before {
            continue;
        }
        let next = table
            .lines
            .iter()
            .map(|(_, l)| *l)
            .filter(|l| *l > line)
            .min();
        if let Some(next) = next {
            if best.is_none_or(|(b, _)| next < b) {
                best = Some((next, *method));
            }
        }
    }
    let (next, method) = best?;
    let table = &tables.iter().find(|(m, _)| *m == method)?.1;
    Some(LineMatch {
        line: next,
        exact: false,
        locations: vec![(method, first_index(table, next)?)],
    })
}

/// 在一个类里给这个源文件的断点装上 JDWP 断点请求，返回状态变化了的断点下标。
fn install_class(vm: &Vm, source: &mut SourceBreakpoints, class: u64) -> Vec<usize> {
    if source.examined.contains(&class) {
        return Vec::new();
    }
    source.examined.push(class);
    // 同名的类可能来自另一个源文件（比如测试目录里的同名类）
    if let Ok(Some(file)) = vm.source_file(class) {
        if file != source.classes.file_name {
            return Vec::new();
        }
    }
    let methods = match vm.methods(class) {
        Ok(methods) => methods,
        Err(err) => {
            tracing::debug!("读取类 {class} 的方法失败: {err:#}");
            return Vec::new();
        }
    };
    let tables: Vec<(u64, Rc<LineTable>)> = methods
        .iter()
        .filter_map(|m| Some((m.id, vm.line_table(class, m.id).ok()?)))
        .filter(|(_, table)| !table.lines.is_empty())
        .collect();
    let mut changed = Vec::new();
    for (i, bp) in source.breakpoints.iter_mut().enumerate() {
        let Some(LineMatch {
            line,
            exact,
            locations,
        }) = find_locations(&tables, bp.line)
        else {
            continue;
        };
        if !exact && !bp.requests.is_empty() {
            continue;
        }
        if exact && bp.moved {
            for request in bp.requests.drain(..) {
                let _ = vm.clear_event(event_kind::BREAKPOINT, request);
            }
            bp.moved = false;
            bp.actual_line = None;
        }
        let before = bp.requests.len();
        for (method, index) in locations {
            let location = Location {
                type_tag: 1,
                class,
                method,
                index,
            };
            match vm.set_event(
                event_kind::BREAKPOINT,
                suspend_policy::ALL,
                &[Modifier::LocationOnly(location)],
            ) {
                Ok(request) => bp.requests.push(request),
                Err(err) => tracing::warn!("设置断点失败: {err:#}"),
            }
        }
        if bp.requests.len() > before {
            if bp.actual_line.is_none() {
                bp.actual_line = Some(line);
                bp.moved = !exact;
            }
            changed.push(i);
        }
    }
    changed
}

impl Session {
    pub(crate) fn set_breakpoints(&mut self, args: &Value) -> anyhow::Result<Value> {
        let source = args.get("source").cloned().unwrap_or(Value::Null);
        let mut requested: Vec<Breakpoint> = match args.get("breakpoints").and_then(Value::as_array)
        {
            Some(list) => list
                .iter()
                .filter_map(|bp| {
                    let line = bp.get("line")?.as_i64()?;
                    let text = |key: &str| {
                        bp.get(key)
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                    };
                    let mut breakpoint = Breakpoint::new(self.line_from_client(line));
                    breakpoint.condition = text("condition");
                    breakpoint.hit_condition = text("hitCondition");
                    breakpoint.log_message = text("logMessage");
                    Some(breakpoint)
                })
                .collect(),
            None => args
                .get("lines")
                .and_then(Value::as_array)
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(Value::as_i64)
                        .map(|l| Breakpoint::new(self.line_from_client(l)))
                        .collect()
                })
                .unwrap_or_default(),
        };
        let Some(path) = source.get("path").and_then(Value::as_str) else {
            // 没有路径的源码（反编译、源码包里的）不能设置断点
            let breakpoints: Vec<Value> = requested
                .iter()
                .map(|_| {
                    json!({
                        "verified": false,
                        "message": trs!(
                            "只能在磁盘上的源文件里设置断点",
                            "Breakpoints can only be set in source files on disk"
                        ),
                    })
                })
                .collect();
            return Ok(json!({ "breakpoints": breakpoints }));
        };
        let path = std::path::PathBuf::from(path);

        if let Some(old) = self.breakpoints.remove(&path) {
            self.clear_source_requests(&old);
        }
        for bp in &mut requested {
            self.next_breakpoint_id += 1;
            bp.id = self.next_breakpoint_id;
        }
        let breakpoints = requested;
        if breakpoints.is_empty() {
            return Ok(json!({ "breakpoints": [] }));
        }
        self.breakpoints.insert(
            path.clone(),
            SourceBreakpoints {
                classes: SourceClasses::read(&path),
                prepare_requests: Vec::new(),
                breakpoints,
                examined: Vec::new(),
            },
        );
        self.install_source(&path);
        let source = &self.breakpoints[&path];
        let result: Vec<Value> = source
            .breakpoints
            .iter()
            .map(|bp| self.breakpoint_json(bp, &path))
            .collect();
        Ok(json!({ "breakpoints": result }))
    }

    pub(crate) fn breakpoint_json(&self, bp: &Breakpoint, path: &Path) -> Value {
        let verified = !bp.requests.is_empty();
        let mut value = json!({
            "id": bp.id,
            "verified": verified,
            "line": self.line_to_client(bp.actual_line.unwrap_or(bp.line)),
            "source": { "path": path, "name": path.file_name().map(|n| n.to_string_lossy().into_owned()) },
        });
        if !verified {
            value["message"] = json!(if self.vm.is_some() {
                trs!(
                    "类还没有加载，或者这一行没有可执行的代码",
                    "The class is not loaded yet, or this line has no executable code"
                )
            } else {
                trs!(
                    "调试目标还没有启动",
                    "The debuggee has not been started yet"
                )
            });
            value["reason"] = json!("pending");
        }
        value
    }

    fn clear_source_requests(&self, source: &SourceBreakpoints) {
        let Some(vm) = &self.vm else { return };
        for &request in &source.prepare_requests {
            let _ = vm.clear_event(event_kind::CLASS_PREPARE, request);
        }
        for bp in &source.breakpoints {
            for &request in &bp.requests {
                let _ = vm.clear_event(event_kind::BREAKPOINT, request);
            }
        }
    }

    /// 给一个源文件的断点注册类准备事件，并装到已经加载的类里。
    pub(crate) fn install_source(&mut self, path: &Path) {
        let Some(vm) = self.vm.clone() else { return };
        let Some(source) = self.breakpoints.get_mut(path) else {
            return;
        };
        if source.prepare_requests.is_empty() {
            // 先注册事件再列已加载的类，中间加载的类也不会漏掉（重复的由 `examined` 去掉）
            for pattern in source.classes.class_patterns() {
                match vm.set_event(
                    event_kind::CLASS_PREPARE,
                    suspend_policy::EVENT_THREAD,
                    &[Modifier::ClassMatch(pattern)],
                ) {
                    Ok(request) => source.prepare_requests.push(request),
                    Err(err) => tracing::warn!("注册类准备事件失败: {err:#}"),
                }
            }
        }
        let classes = match vm.all_classes() {
            Ok(classes) => classes,
            Err(err) => {
                tracing::warn!("列出已加载的类失败: {err:#}");
                return;
            }
        };
        for (class, signature, status) in classes {
            if status & CLASS_STATUS_PREPARED == 0 || !signature.starts_with('L') {
                continue;
            }
            if source
                .classes
                .contains_class(&signature_to_type_name(&signature))
            {
                vm.remember_signature(class, &signature);
                install_class(&vm, source, class);
            }
        }
    }

    /// 类被替换后重新装断点：JDWP 替换类时会删掉类里的断点，新版本的行号表也可能变了。
    /// 涉及这些类的源文件把断点全部重装一遍，并告诉客户端断点的新状态。
    pub(crate) fn reinstall_breakpoints(&mut self, classes: &[u64]) {
        let Some(vm) = self.vm.clone() else { return };
        let paths: Vec<std::path::PathBuf> = self
            .breakpoints
            .iter()
            .filter(|(_, source)| source.examined.iter().any(|c| classes.contains(c)))
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            let Some(source) = self.breakpoints.get_mut(&path) else {
                continue;
            };
            for bp in &mut source.breakpoints {
                for request in bp.requests.drain(..) {
                    let _ = vm.clear_event(event_kind::BREAKPOINT, request);
                }
                bp.actual_line = None;
                bp.moved = false;
            }
            source.examined.clear();
            self.install_source(&path);
            let source = &self.breakpoints[&path];
            let events: Vec<Value> = source
                .breakpoints
                .iter()
                .map(|bp| json!({ "reason": "changed", "breakpoint": self.breakpoint_json(bp, &path) }))
                .collect();
            for body in events {
                self.out.event("breakpoint", body);
            }
        }
    }

    pub(crate) fn on_class_prepare(&mut self, class: u64, class_name: &str) {
        let Some(vm) = self.vm.clone() else { return };
        let mut changed = Vec::new();
        for (path, source) in self.breakpoints.iter_mut() {
            if !source.classes.contains_class(class_name) {
                continue;
            }
            for i in install_class(&vm, source, class) {
                changed.push((path.clone(), i));
            }
        }
        for (path, i) in changed {
            let body = json!({
                "reason": "changed",
                "breakpoint": self.breakpoint_json(&self.breakpoints[&path].breakpoints[i], &path),
            });
            self.out.event("breakpoint", body);
        }
    }

    /// 断点命中：检查条件和命中次数，日志断点输出消息。需要停下时返回断点 ID。
    pub(crate) fn on_breakpoint(
        &mut self,
        request: i32,
        thread: u64,
        _location: &Location,
    ) -> Option<i64> {
        let (path, index) = self.breakpoints.iter().find_map(|(path, source)| {
            source
                .breakpoints
                .iter()
                .position(|bp| bp.requests.contains(&request))
                .map(|i| (path.clone(), i))
        })?;
        let bp = &self.breakpoints[&path].breakpoints[index];
        let (condition, hit_condition, log_message) = (
            bp.condition.clone(),
            bp.hit_condition.clone(),
            bp.log_message.clone(),
        );
        if let Some(condition) = condition {
            match self.eval_condition(thread, &condition) {
                Ok(true) => {}
                Ok(false) => return None,
                Err(err) => {
                    // 求值失败时停下来，让用户看到问题
                    self.out.output(
                        "console",
                        &tr!(
                            "条件断点 `{condition}` 求值失败: {err:#}\n",
                            "Failed to evaluate the breakpoint condition `{condition}`: {err:#}\n"
                        ),
                    );
                }
            }
        }
        let bp = &mut self.breakpoints.get_mut(&path)?.breakpoints[index];
        bp.hits += 1;
        let (hits, id) = (bp.hits, bp.id);
        if let Some(hit_condition) = hit_condition {
            match eval::hit_condition_met(&hit_condition, hits) {
                Ok(true) => {}
                Ok(false) => return None,
                Err(err) => self.out.output("console", &format!("{err}\n")),
            }
        }
        if let Some(message) = log_message {
            let text = self.interpolate_log_message(thread, &message);
            self.out.output("console", &format!("{text}\n"));
            return None;
        }
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(lines: &[(u64, i32)]) -> Rc<LineTable> {
        Rc::new(LineTable {
            lines: lines.to_vec(),
        })
    }

    #[test]
    fn exact_and_moved_lines() {
        // 方法 1：第 10~14 行；方法 2（lambda）：第 12 行
        let tables = vec![
            (1, table(&[(0, 10), (4, 12), (9, 14), (2, 12)])),
            (2, table(&[(0, 12), (5, 12)])),
        ];
        let found = |line, exact, locations: Vec<(u64, u64)>| {
            Some(LineMatch {
                line,
                exact,
                locations,
            })
        };
        assert_eq!(
            find_locations(&tables, 12),
            found(12, true, vec![(1, 2), (2, 0)])
        );
        // 第 11 行没有代码：挪到第 12 行
        assert_eq!(find_locations(&tables, 11), found(12, false, vec![(1, 2)]));
        // 方法声明行（方法体从第 10 行开始）
        assert_eq!(find_locations(&tables, 9), found(10, false, vec![(1, 0)]));
        // 远离任何方法
        assert_eq!(find_locations(&tables, 3), None);
        assert_eq!(find_locations(&tables, 20), None);
    }
}
