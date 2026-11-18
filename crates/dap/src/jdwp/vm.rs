//! 常用 JDWP 命令的类型化封装，以及类、方法、行号表等只读信息的缓存。

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use super::{
    error_code, event_kind, signature_to_type_name, Connection, IdSizes, Location, PendingReply,
    Reader, Value, Writer,
};

mod cmd {
    // (命令集, 命令)
    pub const VM_VERSION: (u8, u8) = (1, 1);
    pub const VM_CLASSES_BY_SIGNATURE: (u8, u8) = (1, 2);
    pub const VM_ALL_CLASSES: (u8, u8) = (1, 3);
    pub const VM_ALL_THREADS: (u8, u8) = (1, 4);
    pub const VM_DISPOSE: (u8, u8) = (1, 6);
    pub const VM_ID_SIZES: (u8, u8) = (1, 7);
    pub const VM_SUSPEND: (u8, u8) = (1, 8);
    pub const VM_RESUME: (u8, u8) = (1, 9);
    pub const VM_EXIT: (u8, u8) = (1, 10);
    pub const VM_CREATE_STRING: (u8, u8) = (1, 11);
    pub const VM_CAPABILITIES_NEW: (u8, u8) = (1, 17);
    pub const VM_REDEFINE_CLASSES: (u8, u8) = (1, 18);
    pub const RT_SIGNATURE: (u8, u8) = (2, 1);
    pub const RT_CLASS_LOADER: (u8, u8) = (2, 2);
    pub const RT_MODIFIERS: (u8, u8) = (2, 3);
    pub const RT_FIELDS: (u8, u8) = (2, 4);
    pub const RT_METHODS: (u8, u8) = (2, 5);
    pub const RT_GET_VALUES: (u8, u8) = (2, 6);
    pub const RT_SOURCE_FILE: (u8, u8) = (2, 7);
    pub const RT_INTERFACES: (u8, u8) = (2, 10);
    pub const RT_CLASS_OBJECT: (u8, u8) = (2, 11);
    pub const CT_SUPERCLASS: (u8, u8) = (3, 1);
    pub const CT_SET_VALUES: (u8, u8) = (3, 2);
    pub const CT_INVOKE_METHOD: (u8, u8) = (3, 3);
    pub const CT_NEW_INSTANCE: (u8, u8) = (3, 4);
    pub const AT_NEW_INSTANCE: (u8, u8) = (4, 1);
    pub const IT_INVOKE_METHOD: (u8, u8) = (5, 1);
    pub const M_LINE_TABLE: (u8, u8) = (6, 1);
    pub const M_VARIABLE_TABLE: (u8, u8) = (6, 2);
    pub const OR_REFERENCE_TYPE: (u8, u8) = (9, 1);
    pub const OR_GET_VALUES: (u8, u8) = (9, 2);
    pub const OR_SET_VALUES: (u8, u8) = (9, 3);
    pub const OR_INVOKE_METHOD: (u8, u8) = (9, 6);
    pub const OR_DISABLE_COLLECTION: (u8, u8) = (9, 7);
    pub const OR_ENABLE_COLLECTION: (u8, u8) = (9, 8);
    pub const SR_VALUE: (u8, u8) = (10, 1);
    pub const TR_NAME: (u8, u8) = (11, 1);
    pub const TR_SUSPEND: (u8, u8) = (11, 2);
    pub const TR_RESUME: (u8, u8) = (11, 3);
    pub const TR_STOP: (u8, u8) = (11, 10);
    pub const TR_FRAMES: (u8, u8) = (11, 6);
    pub const AR_LENGTH: (u8, u8) = (13, 1);
    pub const AR_GET_VALUES: (u8, u8) = (13, 2);
    pub const AR_SET_VALUES: (u8, u8) = (13, 3);
    pub const ER_SET: (u8, u8) = (15, 1);
    pub const ER_CLEAR: (u8, u8) = (15, 2);
    pub const SF_GET_VALUES: (u8, u8) = (16, 1);
    pub const SF_SET_VALUES: (u8, u8) = (16, 2);
    pub const SF_THIS_OBJECT: (u8, u8) = (16, 3);
    pub const COR_REFLECTED_TYPE: (u8, u8) = (17, 1);
}

/// 方法调用的选项：只恢复调用所在的线程；不按虚方法分派（`super.m()`）
const INVOKE_SINGLE_THREADED: i32 = 0x01;
const INVOKE_NONVIRTUAL: i32 = 0x02;

/// 类的状态位：已准备好（方法、字段可以查询）
pub const CLASS_STATUS_PREPARED: i32 = 2;
const ACC_STATIC: i32 = 0x0008;
const ACC_BRIDGE: i32 = 0x0040;
const ACC_VARARGS: i32 = 0x0080;
const ACC_INTERFACE: i32 = 0x0200;

#[derive(Debug, Clone)]
pub struct MethodInfo {
    pub id: u64,
    pub name: String,
    pub signature: String,
    pub modifiers: i32,
}

impl MethodInfo {
    pub fn is_static(&self) -> bool {
        self.modifiers & ACC_STATIC != 0
    }

    /// 编译器为泛型擦除生成的桥接方法
    pub fn is_bridge(&self) -> bool {
        self.modifiers & ACC_BRIDGE != 0
    }

    pub fn is_varargs(&self) -> bool {
        self.modifiers & ACC_VARARGS != 0
    }
}

/// 在目标 JVM 里执行的调用。都只恢复 `thread` 一个线程（`INVOKE_SINGLE_THREADED`）。
#[derive(Debug, Clone)]
pub enum Invoke {
    /// 静态方法；`interface` 为真时用 `InterfaceType.InvokeMethod`
    Static {
        class: u64,
        interface: bool,
        thread: u64,
        method: u64,
        args: Vec<Value>,
    },
    /// 实例方法；`nonvirtual` 时不按运行时类型分派（`super.m()`）
    Instance {
        object: u64,
        class: u64,
        thread: u64,
        method: u64,
        args: Vec<Value>,
        nonvirtual: bool,
    },
    /// 构造器
    New {
        class: u64,
        thread: u64,
        method: u64,
        args: Vec<Value>,
    },
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub id: u64,
    pub name: String,
    pub signature: String,
    pub modifiers: i32,
}

impl FieldInfo {
    pub fn is_static(&self) -> bool {
        self.modifiers & ACC_STATIC != 0
    }

    /// 编译器生成的字段（`this$0`、`val$x` 等）
    pub fn is_synthetic(&self) -> bool {
        self.modifiers & 0x1000 != 0 || self.modifiers & 0xF000_0000u32 as i32 != 0
    }
}

/// `Method.LineTable`：代码下标 → 行号。
#[derive(Debug, Clone, Default)]
pub struct LineTable {
    pub lines: Vec<(u64, i32)>,
}

impl LineTable {
    /// 代码下标所在的行：取下标不超过它的最后一个条目。
    pub fn line_of(&self, index: u64) -> Option<i32> {
        self.lines
            .iter()
            .filter(|(code, _)| *code <= index)
            .max_by_key(|(code, _)| *code)
            .map(|(_, line)| *line)
    }

    pub fn line_range(&self) -> Option<(i32, i32)> {
        let min = self.lines.iter().map(|(_, l)| *l).min()?;
        let max = self.lines.iter().map(|(_, l)| *l).max()?;
        Some((min, max))
    }
}

/// `Method.VariableTable` 的一项。
#[derive(Debug, Clone)]
pub struct Variable {
    pub code_index: u64,
    pub name: String,
    pub signature: String,
    pub length: u32,
    pub slot: i32,
}

impl Variable {
    pub fn visible_at(&self, index: u64) -> bool {
        self.code_index <= index && index < self.code_index + self.length as u64
    }
}

/// 一个引用类型的缓存信息。
#[derive(Debug, Default)]
pub struct ClassInfo {
    pub signature: Option<String>,
    pub source_file: Option<Option<String>>,
    pub methods: Option<Rc<Vec<MethodInfo>>>,
    pub fields: Option<Rc<Vec<FieldInfo>>>,
    pub superclass: Option<Option<u64>>,
    pub interfaces: Option<Rc<Vec<u64>>>,
    pub modifiers: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepDepth {
    Into = 0,
    Over = 1,
    Out = 2,
}

/// `EventRequest.Set` 的修饰条件。
#[derive(Debug, Clone)]
pub enum Modifier {
    Count(i32),
    ThreadOnly(u64),
    ClassMatch(String),
    ClassExclude(String),
    LocationOnly(Location),
    ExceptionOnly {
        class: u64,
        caught: bool,
        uncaught: bool,
    },
    Step {
        thread: u64,
        depth: StepDepth,
    },
}

/// 复合事件里的一个事件。
#[derive(Debug, Clone)]
pub enum Event {
    VmStart {
        thread: u64,
    },
    VmDeath,
    SingleStep {
        request: i32,
        thread: u64,
        location: Location,
    },
    Breakpoint {
        request: i32,
        thread: u64,
        location: Location,
    },
    Exception {
        request: i32,
        thread: u64,
        location: Location,
        exception: Value,
        catch_location: Option<Location>,
    },
    ThreadStart {
        thread: u64,
    },
    ThreadDeath {
        thread: u64,
    },
    ClassPrepare {
        request: i32,
        thread: u64,
        class: u64,
        signature: String,
    },
    Other {
        kind: u8,
    },
}

impl Event {
    pub fn thread(&self) -> Option<u64> {
        match self {
            Event::VmStart { thread }
            | Event::SingleStep { thread, .. }
            | Event::Breakpoint { thread, .. }
            | Event::Exception { thread, .. }
            | Event::ThreadStart { thread }
            | Event::ThreadDeath { thread }
            | Event::ClassPrepare { thread, .. } => Some(*thread),
            Event::VmDeath | Event::Other { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventSet {
    pub suspend_policy: u8,
    pub events: Vec<Event>,
}

/// `(类, 方法)`
type MethodKey = (u64, u64);

/// 一个已连接的目标 JVM。只在会话线程上使用，缓存用 `RefCell`。
pub struct Vm {
    conn: Arc<Connection>,
    pub sizes: IdSizes,
    classes: RefCell<HashMap<u64, ClassInfo>>,
    line_tables: RefCell<HashMap<(u64, u64), Rc<LineTable>>>,
    variable_tables: RefCell<HashMap<MethodKey, Rc<Vec<Variable>>>>,
}

impl Vm {
    pub fn new(conn: Arc<Connection>) -> anyhow::Result<Vm> {
        let data = conn.request(cmd::VM_ID_SIZES.0, cmd::VM_ID_SIZES.1, Vec::new())?;
        let mut r = Reader::new(&data, IdSizes::default());
        let mut size = || -> anyhow::Result<usize> {
            let n = r.i32()?;
            anyhow::ensure!((1..=8).contains(&n), "不支持的 JDWP ID 长度 {n}");
            Ok(n as usize)
        };
        let sizes = IdSizes {
            field: size()?,
            method: size()?,
            object: size()?,
            reference_type: size()?,
            frame: size()?,
        };
        Ok(Vm {
            conn,
            sizes,
            classes: RefCell::default(),
            line_tables: RefCell::default(),
            variable_tables: RefCell::default(),
        })
    }

    pub fn connection(&self) -> &Arc<Connection> {
        &self.conn
    }

    fn writer(&self) -> Writer {
        Writer::new(self.sizes)
    }

    fn send(&self, command: (u8, u8), data: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        self.conn.request(command.0, command.1, data)
    }

    // ---------------------------------------------------------------
    // VirtualMachine
    // ---------------------------------------------------------------

    pub fn version(&self) -> anyhow::Result<String> {
        let data = self.send(cmd::VM_VERSION, Vec::new())?;
        let mut r = Reader::new(&data, self.sizes);
        let description = r.string()?;
        Ok(description.lines().next().unwrap_or_default().to_owned())
    }

    /// 已加载的、签名完全相同的类：`(类型标签, 类型 ID, 状态)`。
    pub fn classes_by_signature(&self, signature: &str) -> anyhow::Result<Vec<(u8, u64, i32)>> {
        let mut w = self.writer();
        w.string(signature);
        let data = self.send(cmd::VM_CLASSES_BY_SIGNATURE, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push((r.u8()?, r.ref_type()?, r.i32()?));
        }
        Ok(out)
    }

    /// 所有已加载的类：`(类型 ID, 签名, 状态)`。
    pub fn all_classes(&self) -> anyhow::Result<Vec<(u64, String, i32)>> {
        let data = self.send(cmd::VM_ALL_CLASSES, Vec::new())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let _tag = r.u8()?;
            let id = r.ref_type()?;
            let signature = r.string()?;
            let status = r.i32()?;
            out.push((id, signature, status));
        }
        Ok(out)
    }

    pub fn all_threads(&self) -> anyhow::Result<Vec<u64>> {
        let data = self.send(cmd::VM_ALL_THREADS, Vec::new())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        (0..n).map(|_| r.object()).collect()
    }

    pub fn suspend(&self) -> anyhow::Result<()> {
        self.send(cmd::VM_SUSPEND, Vec::new()).map(drop)
    }

    pub fn resume(&self) -> anyhow::Result<()> {
        self.send(cmd::VM_RESUME, Vec::new()).map(drop)
    }

    pub fn exit(&self, code: i32) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.i32(code);
        self.send(cmd::VM_EXIT, w.finish()).map(drop)
    }

    /// 断开调试：目标 JVM 取消所有事件请求、恢复被挂起的线程，然后继续运行。
    pub fn dispose(&self) -> anyhow::Result<()> {
        self.send(cmd::VM_DISPOSE, Vec::new()).map(drop)
    }

    /// `VirtualMachine.CapabilitiesNew` 的 32 个能力位，下标见 JDWP 规范。
    pub fn capabilities(&self) -> anyhow::Result<Vec<bool>> {
        let data = self.send(cmd::VM_CAPABILITIES_NEW, Vec::new())?;
        let mut r = Reader::new(&data, self.sizes);
        let mut out = Vec::with_capacity(32);
        while r.remaining() > 0 {
            out.push(r.bool()?);
        }
        Ok(out)
    }

    /// 用新的 class 文件替换已加载的类：`(类型 ID, class 文件内容)`。一批里有一个失败时都不替换。
    pub fn redefine_classes(&self, classes: &[(u64, &[u8])]) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.i32(classes.len() as i32);
        for (class, bytes) in classes {
            w.ref_type(*class).i32(bytes.len() as i32).bytes(bytes);
        }
        self.send(cmd::VM_REDEFINE_CLASSES, w.finish())?;
        for (class, _) in classes {
            self.forget_class(*class);
        }
        Ok(())
    }

    /// 类被替换后，方法、字段、行号表和局部变量表都可能变了：丢掉缓存（签名不变，保留）。
    pub fn forget_class(&self, class: u64) {
        if let Some(info) = self.classes.borrow_mut().get_mut(&class) {
            info.source_file = None;
            info.methods = None;
            info.fields = None;
            info.superclass = None;
        }
        self.line_tables
            .borrow_mut()
            .retain(|(c, _), _| *c != class);
        self.variable_tables
            .borrow_mut()
            .retain(|(c, _), _| *c != class);
    }

    pub fn create_string(&self, text: &str) -> anyhow::Result<u64> {
        let mut w = self.writer();
        w.string(text);
        let data = self.send(cmd::VM_CREATE_STRING, w.finish())?;
        Reader::new(&data, self.sizes).object()
    }

    // ---------------------------------------------------------------
    // 事件请求
    // ---------------------------------------------------------------

    pub fn set_event(
        &self,
        kind: u8,
        suspend_policy: u8,
        modifiers: &[Modifier],
    ) -> anyhow::Result<i32> {
        let mut w = self.writer();
        w.u8(kind).u8(suspend_policy).i32(modifiers.len() as i32);
        for modifier in modifiers {
            match modifier {
                Modifier::Count(n) => {
                    w.u8(1).i32(*n);
                }
                Modifier::ThreadOnly(thread) => {
                    w.u8(3).object(*thread);
                }
                Modifier::ClassMatch(pattern) => {
                    w.u8(5).string(pattern);
                }
                Modifier::ClassExclude(pattern) => {
                    w.u8(6).string(pattern);
                }
                Modifier::LocationOnly(loc) => {
                    w.u8(7).location(loc);
                }
                Modifier::ExceptionOnly {
                    class,
                    caught,
                    uncaught,
                } => {
                    w.u8(8).ref_type(*class).bool(*caught).bool(*uncaught);
                }
                Modifier::Step { thread, depth } => {
                    // 步长固定按行
                    w.u8(10).object(*thread).i32(1).i32(*depth as i32);
                }
            }
        }
        let data = self.send(cmd::ER_SET, w.finish())?;
        Reader::new(&data, self.sizes).i32()
    }

    pub fn clear_event(&self, kind: u8, request: i32) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.u8(kind).i32(request);
        self.send(cmd::ER_CLEAR, w.finish()).map(drop)
    }

    /// 解析 `Event.Composite` 的数据。遇到不认识的事件种类时停下，返回已解析的部分。
    pub fn parse_events(&self, data: &[u8]) -> anyhow::Result<EventSet> {
        let mut r = Reader::new(data, self.sizes);
        let suspend_policy = r.u8()?;
        let n = r.count()?;
        let mut events = Vec::with_capacity(n);
        for _ in 0..n {
            let kind = r.u8()?;
            let request = r.i32()?;
            let event = match kind {
                event_kind::VM_START => Event::VmStart {
                    thread: r.object()?,
                },
                event_kind::VM_DEATH => Event::VmDeath,
                event_kind::SINGLE_STEP => Event::SingleStep {
                    request,
                    thread: r.object()?,
                    location: r.location()?,
                },
                event_kind::BREAKPOINT => Event::Breakpoint {
                    request,
                    thread: r.object()?,
                    location: r.location()?,
                },
                event_kind::EXCEPTION => {
                    let thread = r.object()?;
                    let location = r.location()?;
                    let exception = r.value()?;
                    let catch = r.location()?;
                    Event::Exception {
                        request,
                        thread,
                        location,
                        exception,
                        catch_location: (catch.class != 0).then_some(catch),
                    }
                }
                event_kind::THREAD_START => Event::ThreadStart {
                    thread: r.object()?,
                },
                event_kind::THREAD_DEATH => Event::ThreadDeath {
                    thread: r.object()?,
                },
                event_kind::CLASS_PREPARE => {
                    let thread = r.object()?;
                    let _tag = r.u8()?;
                    let class = r.ref_type()?;
                    let signature = r.string()?;
                    let _status = r.i32()?;
                    Event::ClassPrepare {
                        request,
                        thread,
                        class,
                        signature,
                    }
                }
                event_kind::CLASS_UNLOAD => {
                    let _signature = r.string()?;
                    Event::Other { kind }
                }
                other => {
                    events.push(Event::Other { kind: other });
                    break;
                }
            };
            events.push(event);
        }
        Ok(EventSet {
            suspend_policy,
            events,
        })
    }

    // ---------------------------------------------------------------
    // 线程和栈帧
    // ---------------------------------------------------------------

    pub fn thread_name(&self, thread: u64) -> anyhow::Result<String> {
        let mut w = self.writer();
        w.object(thread);
        let data = self.send(cmd::TR_NAME, w.finish())?;
        Reader::new(&data, self.sizes).string()
    }

    pub fn thread_suspend(&self, thread: u64) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.object(thread);
        self.send(cmd::TR_SUSPEND, w.finish()).map(drop)
    }

    /// 让线程抛出 `throwable`（`Thread.stop` 的效果），用来打断超时的方法调用。
    pub fn thread_stop(&self, thread: u64, throwable: u64) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.object(thread).object(throwable);
        self.send(cmd::TR_STOP, w.finish()).map(drop)
    }

    pub fn thread_resume(&self, thread: u64) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.object(thread);
        self.send(cmd::TR_RESUME, w.finish()).map(drop)
    }

    /// 线程的栈帧 `(帧 ID, 位置)`；`length` 为 -1 时取到栈底。线程必须已挂起。
    pub fn frames(
        &self,
        thread: u64,
        start: i32,
        length: i32,
    ) -> anyhow::Result<Vec<(u64, Location)>> {
        let mut w = self.writer();
        w.object(thread).i32(start).i32(length);
        let data = self.send(cmd::TR_FRAMES, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push((r.frame()?, r.location()?));
        }
        Ok(out)
    }

    pub fn frame_values(
        &self,
        thread: u64,
        frame: u64,
        slots: &[(i32, u8)],
    ) -> anyhow::Result<Vec<Value>> {
        let mut w = self.writer();
        w.object(thread).frame(frame).i32(slots.len() as i32);
        for (slot, tag) in slots {
            w.i32(*slot).u8(*tag);
        }
        let data = self.send(cmd::SF_GET_VALUES, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        (0..n).map(|_| r.value()).collect()
    }

    pub fn set_frame_value(
        &self,
        thread: u64,
        frame: u64,
        slot: i32,
        value: &Value,
    ) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.object(thread).frame(frame).i32(1).i32(slot).value(value);
        self.send(cmd::SF_SET_VALUES, w.finish()).map(drop)
    }

    pub fn this_object(&self, thread: u64, frame: u64) -> anyhow::Result<Value> {
        let mut w = self.writer();
        w.object(thread).frame(frame);
        let data = self.send(cmd::SF_THIS_OBJECT, w.finish())?;
        Reader::new(&data, self.sizes).value()
    }

    // ---------------------------------------------------------------
    // 类型信息（带缓存）
    // ---------------------------------------------------------------

    pub fn class_signature(&self, class: u64) -> anyhow::Result<String> {
        if let Some(sig) = self
            .classes
            .borrow()
            .get(&class)
            .and_then(|c| c.signature.clone())
        {
            return Ok(sig);
        }
        let mut w = self.writer();
        w.ref_type(class);
        let data = self.send(cmd::RT_SIGNATURE, w.finish())?;
        let sig = Reader::new(&data, self.sizes).string()?;
        self.remember_signature(class, &sig);
        Ok(sig)
    }

    /// 事件里已经带了签名时直接记下，省一次往返。
    pub fn remember_signature(&self, class: u64, signature: &str) {
        self.classes
            .borrow_mut()
            .entry(class)
            .or_default()
            .signature = Some(signature.to_owned());
    }

    /// 类的 Java 名字：`demo.Outer$Inner`。
    pub fn class_name(&self, class: u64) -> anyhow::Result<String> {
        Ok(signature_to_type_name(&self.class_signature(class)?))
    }

    /// 类文件的 `SourceFile` 属性；编译时没有带上时为 `None`。
    pub fn source_file(&self, class: u64) -> anyhow::Result<Option<String>> {
        if let Some(file) = self
            .classes
            .borrow()
            .get(&class)
            .and_then(|c| c.source_file.clone())
        {
            return Ok(file);
        }
        let mut w = self.writer();
        w.ref_type(class);
        let file = match self.send(cmd::RT_SOURCE_FILE, w.finish()) {
            Ok(data) => Some(Reader::new(&data, self.sizes).string()?),
            Err(err) if super::is_error(&err, error_code::ABSENT_INFORMATION) => None,
            Err(err) => return Err(err),
        };
        self.classes
            .borrow_mut()
            .entry(class)
            .or_default()
            .source_file = Some(file.clone());
        Ok(file)
    }

    pub fn methods(&self, class: u64) -> anyhow::Result<Rc<Vec<MethodInfo>>> {
        if let Some(methods) = self
            .classes
            .borrow()
            .get(&class)
            .and_then(|c| c.methods.clone())
        {
            return Ok(methods);
        }
        let mut w = self.writer();
        w.ref_type(class);
        let data = self.send(cmd::RT_METHODS, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        let mut methods = Vec::with_capacity(n);
        for _ in 0..n {
            methods.push(MethodInfo {
                id: r.method()?,
                name: r.string()?,
                signature: r.string()?,
                modifiers: r.i32()?,
            });
        }
        let methods = Rc::new(methods);
        self.classes.borrow_mut().entry(class).or_default().methods = Some(methods.clone());
        Ok(methods)
    }

    pub fn method(&self, class: u64, method: u64) -> anyhow::Result<Option<MethodInfo>> {
        Ok(self
            .methods(class)?
            .iter()
            .find(|m| m.id == method)
            .cloned())
    }

    /// 类自己声明的字段（不含父类的）。
    pub fn fields(&self, class: u64) -> anyhow::Result<Rc<Vec<FieldInfo>>> {
        if let Some(fields) = self
            .classes
            .borrow()
            .get(&class)
            .and_then(|c| c.fields.clone())
        {
            return Ok(fields);
        }
        let mut w = self.writer();
        w.ref_type(class);
        let data = self.send(cmd::RT_FIELDS, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        let mut fields = Vec::with_capacity(n);
        for _ in 0..n {
            fields.push(FieldInfo {
                id: r.field()?,
                name: r.string()?,
                signature: r.string()?,
                modifiers: r.i32()?,
            });
        }
        let fields = Rc::new(fields);
        self.classes.borrow_mut().entry(class).or_default().fields = Some(fields.clone());
        Ok(fields)
    }

    /// 父类；接口、`java.lang.Object` 和数组类型返回 `None`。
    pub fn superclass(&self, class: u64) -> anyhow::Result<Option<u64>> {
        if let Some(sup) = self.classes.borrow().get(&class).and_then(|c| c.superclass) {
            return Ok(sup);
        }
        let mut w = self.writer();
        w.ref_type(class);
        let sup = match self.send(cmd::CT_SUPERCLASS, w.finish()) {
            Ok(data) => {
                let id = Reader::new(&data, self.sizes).ref_type()?;
                (id != 0).then_some(id)
            }
            // 接口和数组类型不是 ClassType
            Err(_) => None,
        };
        self.classes
            .borrow_mut()
            .entry(class)
            .or_default()
            .superclass = Some(sup);
        Ok(sup)
    }

    /// 类直接实现（接口则是直接继承）的接口。
    pub fn interfaces(&self, class: u64) -> anyhow::Result<Rc<Vec<u64>>> {
        if let Some(interfaces) = self
            .classes
            .borrow()
            .get(&class)
            .and_then(|c| c.interfaces.clone())
        {
            return Ok(interfaces);
        }
        let mut w = self.writer();
        w.ref_type(class);
        let data = self.send(cmd::RT_INTERFACES, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        let interfaces: Rc<Vec<u64>> = Rc::new(
            (0..n)
                .map(|_| r.ref_type())
                .collect::<anyhow::Result<_>>()?,
        );
        self.classes
            .borrow_mut()
            .entry(class)
            .or_default()
            .interfaces = Some(interfaces.clone());
        Ok(interfaces)
    }

    pub fn class_modifiers(&self, class: u64) -> anyhow::Result<i32> {
        if let Some(modifiers) = self.classes.borrow().get(&class).and_then(|c| c.modifiers) {
            return Ok(modifiers);
        }
        let mut w = self.writer();
        w.ref_type(class);
        let data = self.send(cmd::RT_MODIFIERS, w.finish())?;
        let modifiers = Reader::new(&data, self.sizes).i32()?;
        self.classes
            .borrow_mut()
            .entry(class)
            .or_default()
            .modifiers = Some(modifiers);
        Ok(modifiers)
    }

    pub fn is_interface(&self, class: u64) -> anyhow::Result<bool> {
        Ok(self.class_modifiers(class)? & ACC_INTERFACE != 0)
    }

    /// 加载这个类的类加载器；启动类加载器为 0。
    pub fn class_loader(&self, class: u64) -> anyhow::Result<u64> {
        let mut w = self.writer();
        w.ref_type(class);
        let data = self.send(cmd::RT_CLASS_LOADER, w.finish())?;
        Reader::new(&data, self.sizes).object()
    }

    /// 类型对应的 `java.lang.Class` 对象。
    pub fn class_object(&self, class: u64) -> anyhow::Result<u64> {
        let mut w = self.writer();
        w.ref_type(class);
        let data = self.send(cmd::RT_CLASS_OBJECT, w.finish())?;
        Reader::new(&data, self.sizes).object()
    }

    /// `java.lang.Class` 对象表示的类型。
    pub fn reflected_type(&self, class_object: u64) -> anyhow::Result<u64> {
        let mut w = self.writer();
        w.object(class_object);
        let data = self.send(cmd::COR_REFLECTED_TYPE, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let _tag = r.u8()?;
        r.ref_type()
    }

    /// 发出方法调用，不等结果：调用期间目标 JVM 可能发来事件，由调用方边等边处理。
    /// 结果用 [`Vm::invoke_result`] 解析。
    pub fn start_invoke(&self, invoke: &Invoke) -> anyhow::Result<PendingReply> {
        let mut w = self.writer();
        let write_args = |w: &mut Writer, args: &[Value]| {
            w.i32(args.len() as i32);
            for arg in args {
                w.value(arg);
            }
        };
        let command = match invoke {
            Invoke::Static {
                class,
                interface,
                thread,
                method,
                args,
            } => {
                w.ref_type(*class).object(*thread).method(*method);
                write_args(&mut w, args);
                w.i32(INVOKE_SINGLE_THREADED);
                if *interface {
                    cmd::IT_INVOKE_METHOD
                } else {
                    cmd::CT_INVOKE_METHOD
                }
            }
            Invoke::Instance {
                object,
                class,
                thread,
                method,
                args,
                nonvirtual,
            } => {
                w.object(*object)
                    .object(*thread)
                    .ref_type(*class)
                    .method(*method);
                write_args(&mut w, args);
                let options = if *nonvirtual {
                    INVOKE_SINGLE_THREADED | INVOKE_NONVIRTUAL
                } else {
                    INVOKE_SINGLE_THREADED
                };
                w.i32(options);
                cmd::OR_INVOKE_METHOD
            }
            Invoke::New {
                class,
                thread,
                method,
                args,
            } => {
                w.ref_type(*class).object(*thread).method(*method);
                write_args(&mut w, args);
                w.i32(INVOKE_SINGLE_THREADED);
                cmd::CT_NEW_INSTANCE
            }
        };
        self.conn.send_command(command.0, command.1, w.finish())
    }

    /// 方法调用的回复：`(返回值或新对象, 抛出的异常对象 ID（没有时为 0）)`。
    pub fn invoke_result(&self, data: &[u8]) -> anyhow::Result<(Value, u64)> {
        let mut r = Reader::new(data, self.sizes);
        let value = r.value()?;
        let exception = r.value()?;
        Ok((value, exception.object_id().unwrap_or(0)))
    }

    /// 新建长度为 `length` 的数组，`array_type` 是数组类型的 ID。
    pub fn new_array(&self, array_type: u64, length: i32) -> anyhow::Result<Value> {
        let mut w = self.writer();
        w.ref_type(array_type).i32(length);
        let data = self.send(cmd::AT_NEW_INSTANCE, w.finish())?;
        Reader::new(&data, self.sizes).value()
    }

    /// 不让垃圾回收收走这个对象（求值时新建的对象在显示之前不能被回收）。
    pub fn disable_collection(&self, object: u64) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.object(object);
        self.send(cmd::OR_DISABLE_COLLECTION, w.finish()).map(drop)
    }

    pub fn enable_collection(&self, object: u64) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.object(object);
        self.send(cmd::OR_ENABLE_COLLECTION, w.finish()).map(drop)
    }

    /// 类和它的所有父类（自身在前）。
    pub fn class_chain(&self, class: u64) -> anyhow::Result<Vec<u64>> {
        let mut chain = vec![class];
        let mut current = class;
        while let Some(sup) = self.superclass(current)? {
            if chain.contains(&sup) || chain.len() > 64 {
                break;
            }
            chain.push(sup);
            current = sup;
        }
        Ok(chain)
    }

    pub fn line_table(&self, class: u64, method: u64) -> anyhow::Result<Rc<LineTable>> {
        if let Some(table) = self.line_tables.borrow().get(&(class, method)) {
            return Ok(table.clone());
        }
        let mut w = self.writer();
        w.ref_type(class).method(method);
        let table = match self.send(cmd::M_LINE_TABLE, w.finish()) {
            Ok(data) => {
                let mut r = Reader::new(&data, self.sizes);
                let _start = r.u64()?;
                let _end = r.u64()?;
                let n = r.count()?;
                let mut lines = Vec::with_capacity(n);
                for _ in 0..n {
                    lines.push((r.u64()?, r.i32()?));
                }
                LineTable { lines }
            }
            // 没有行号信息、native / abstract 方法：当作空表
            Err(_) => LineTable::default(),
        };
        let table = Rc::new(table);
        self.line_tables
            .borrow_mut()
            .insert((class, method), table.clone());
        Ok(table)
    }

    /// 局部变量表。编译时没有 `-g`（或 `-g:vars`）时返回 `ABSENT_INFORMATION` 错误。
    pub fn variable_table(&self, class: u64, method: u64) -> anyhow::Result<Rc<Vec<Variable>>> {
        if let Some(table) = self.variable_tables.borrow().get(&(class, method)) {
            return Ok(table.clone());
        }
        let mut w = self.writer();
        w.ref_type(class).method(method);
        let data = self.send(cmd::M_VARIABLE_TABLE, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let _arg_count = r.i32()?;
        let n = r.count()?;
        let mut vars = Vec::with_capacity(n);
        for _ in 0..n {
            vars.push(Variable {
                code_index: r.u64()?,
                name: r.string()?,
                signature: r.string()?,
                length: r.i32()?.max(0) as u32,
                slot: r.i32()?,
            });
        }
        let vars = Rc::new(vars);
        self.variable_tables
            .borrow_mut()
            .insert((class, method), vars.clone());
        Ok(vars)
    }

    pub fn static_values(&self, class: u64, fields: &[u64]) -> anyhow::Result<Vec<Value>> {
        let mut w = self.writer();
        w.ref_type(class).i32(fields.len() as i32);
        for field in fields {
            w.field(*field);
        }
        let data = self.send(cmd::RT_GET_VALUES, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        (0..n).map(|_| r.value()).collect()
    }

    pub fn set_static_value(&self, class: u64, field: u64, value: &Value) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.ref_type(class).i32(1).field(field).untagged_value(value);
        self.send(cmd::CT_SET_VALUES, w.finish()).map(drop)
    }

    // ---------------------------------------------------------------
    // 对象
    // ---------------------------------------------------------------

    /// 对象的运行时类型。
    pub fn object_type(&self, object: u64) -> anyhow::Result<u64> {
        let mut w = self.writer();
        w.object(object);
        let data = self.send(cmd::OR_REFERENCE_TYPE, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let _tag = r.u8()?;
        r.ref_type()
    }

    pub fn object_values(&self, object: u64, fields: &[u64]) -> anyhow::Result<Vec<Value>> {
        let mut w = self.writer();
        w.object(object).i32(fields.len() as i32);
        for field in fields {
            w.field(*field);
        }
        let data = self.send(cmd::OR_GET_VALUES, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let n = r.count()?;
        (0..n).map(|_| r.value()).collect()
    }

    pub fn set_object_value(&self, object: u64, field: u64, value: &Value) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.object(object).i32(1).field(field).untagged_value(value);
        self.send(cmd::OR_SET_VALUES, w.finish()).map(drop)
    }

    pub fn string_value(&self, object: u64) -> anyhow::Result<String> {
        let mut w = self.writer();
        w.object(object);
        let data = self.send(cmd::SR_VALUE, w.finish())?;
        Reader::new(&data, self.sizes).string()
    }

    pub fn array_length(&self, array: u64) -> anyhow::Result<i32> {
        let mut w = self.writer();
        w.object(array);
        let data = self.send(cmd::AR_LENGTH, w.finish())?;
        Reader::new(&data, self.sizes).i32()
    }

    pub fn array_values(&self, array: u64, first: i32, length: i32) -> anyhow::Result<Vec<Value>> {
        let mut w = self.writer();
        w.object(array).i32(first).i32(length);
        let data = self.send(cmd::AR_GET_VALUES, w.finish())?;
        let mut r = Reader::new(&data, self.sizes);
        let tag = r.u8()?;
        let n = r.i32()?.max(0) as usize;
        let mut out = Vec::with_capacity(n.min(r.remaining()));
        for _ in 0..n {
            out.push(if super::tag::is_primitive(tag) {
                r.untagged_value(tag)?
            } else {
                r.value()?
            });
        }
        Ok(out)
    }

    pub fn set_array_value(&self, array: u64, index: i32, value: &Value) -> anyhow::Result<()> {
        self.set_array_values(array, index, std::slice::from_ref(value))
    }

    pub fn set_array_values(&self, array: u64, first: i32, values: &[Value]) -> anyhow::Result<()> {
        let mut w = self.writer();
        w.object(array).i32(first).i32(values.len() as i32);
        for value in values {
            w.untagged_value(value);
        }
        self.send(cmd::AR_SET_VALUES, w.finish()).map(drop)
    }
}
