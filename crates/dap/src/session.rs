//! 调试会话：处理 DAP 请求，管理到目标 JVM 的连接和它发来的事件。
//!
//! 会话跑在一个线程上，同时等两路输入：客户端的请求和 JDWP 读线程转来的事件。
//! 发给 JVM 的命令都是同步的（带超时），所以处理请求期间到达的事件会排队，处理完请求后再处理。

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::Context;
use base_db::{tr, trs};
use crossbeam_channel::{Receiver, Sender};
use serde_json::{json, Value};

use crate::{
    breakpoints::SourceBreakpoints,
    hotswap::ClassFiles,
    jdwp::{
        self, event_kind, signature_to_type_name, simple_name, suspend_policy, Connection, Event,
        EventSet, JdwpEvent, Location, Modifier, StepDepth, Vm,
    },
    launch::{self, AttachArgs, Debuggee, LaunchArgs},
    protocol::{Output, Request},
    sources::{relative_source_path, FoundSource, SourceLocator},
};

/// 等 JDWP 回复的时间。目标 JVM 卡住时请求最多等这么久，不会让编辑器一直挂着
pub(crate) const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// 启动的 JVM 连回调试器的最长等待时间
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// 单步时跳过的类：JDK 自己的代码
const STEP_EXCLUDES: &[&str] = &["java.*", "javax.*", "jdk.*", "sun.*", "com.sun.*"];

/// 一个栈帧（挂起期间有效）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameRef {
    pub thread: u64,
    pub frame: u64,
    pub location: Location,
    /// 在调用栈里的位置（栈顶为 0）：线程恢复过一次（比如求值时调用了方法）帧 ID 就会失效，
    /// 按位置重新取
    pub depth: u32,
}

/// `variablesReference` 指向的内容（挂起期间有效）。
#[derive(Debug, Clone)]
pub(crate) enum VarRef {
    Locals(FrameRef),
    Statics(u64),
    Object(u64),
    Array { id: u64, signature: String },
}

/// 线程的 JDWP 对象 ID 和 DAP 线程号之间的对应。DAP 的数字在 JS 客户端里是双精度浮点，
/// 不能直接用 64 位 ID。
#[derive(Default)]
struct ThreadIds {
    to_dap: HashMap<u64, i64>,
    to_jdwp: HashMap<i64, u64>,
    next: i64,
}

impl ThreadIds {
    fn dap(&mut self, thread: u64) -> i64 {
        if let Some(&id) = self.to_dap.get(&thread) {
            return id;
        }
        self.next += 1;
        self.to_dap.insert(thread, self.next);
        self.to_jdwp.insert(self.next, thread);
        self.next
    }

    fn jdwp(&self, id: i64) -> Option<u64> {
        self.to_jdwp.get(&id).copied()
    }
}

/// 停下来的原因。
struct Stop {
    reason: &'static str,
    thread: u64,
    description: Option<String>,
    text: Option<String>,
    hit_breakpoints: Vec<i64>,
}

impl Stop {
    fn priority(&self) -> u8 {
        match self.reason {
            "breakpoint" => 3,
            "exception" => 2,
            _ => 1,
        }
    }
}

/// 最近一次异常停下时的信息（`exceptionInfo` 用）。
struct ExceptionStop {
    exception: u64,
    type_name: String,
    message: Option<String>,
    caught: bool,
}

pub struct Session {
    pub(crate) out: Output,
    events_tx: Sender<JdwpEvent>,
    pub(crate) events_rx: Receiver<JdwpEvent>,
    /// 求值调用方法期间收到的、和被调用线程无关的事件：调用结束后再处理
    pub(crate) deferred: VecDeque<JdwpEvent>,
    /// 正在执行求值调用的线程：这期间它触发的断点、单步、异常事件不停下
    pub(crate) invoking: Option<u64>,
    /// 打断超时调用用的异常对象（第一次调用方法前创建，整个会话复用）
    pub(crate) abort_exception: Option<u64>,
    /// 求值时新建的对象：禁止回收，程序继续运行时放开
    pub(crate) pinned: Vec<u64>,
    pub(crate) vm: Option<Rc<Vm>>,
    debuggee: Option<Debuggee>,
    /// JVM 是调试器启动的（断开时默认结束它）
    launched: bool,
    lines_start_at1: bool,
    pub(crate) columns_start_at1: bool,
    pub(crate) sources: SourceLocator,
    pub(crate) breakpoints: BTreeMap<PathBuf, SourceBreakpoints>,
    pub(crate) next_breakpoint_id: i64,
    exception_filters: (bool, bool),
    exception_request: Option<i32>,
    step_request: Option<i32>,
    threads: ThreadIds,
    pub(crate) frames: HashMap<i64, FrameRef>,
    pub(crate) vars: HashMap<i64, VarRef>,
    next_handle: i64,
    /// 因为停下（事件或 pause）而挂起全部线程的次数；继续运行时要恢复同样多次
    suspends: u32,
    /// 收到了 VMStart，目标 JVM 挂起着等 `configurationDone`
    vm_start_pending: bool,
    configured: bool,
    pub(crate) last_stopped_thread: Option<u64>,
    exceptions: HashMap<u64, ExceptionStop>,
    /// 类 ID → 栈帧里显示的源码（缓存）
    pub(crate) class_sources: HashMap<u64, Option<Value>>,
    /// 编译输出目录里 class 文件的状态（热替换时找变化了的类）
    pub(crate) class_files: ClassFiles,
    /// 目标 JVM 是否支持替换类（第一次用时查询）
    pub(crate) can_redefine: Option<bool>,
    /// 已加载的类：`(Java 名字, 类 ID)`，按名字排序（补全类型和包用）。停下或继续运行时清空
    pub(crate) loaded_names: Option<Rc<Vec<(String, u64)>>>,
    terminated_sent: bool,
    done: bool,
}

impl Session {
    pub fn new(out: Output) -> Session {
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        Session {
            out,
            events_tx,
            events_rx,
            deferred: VecDeque::new(),
            invoking: None,
            abort_exception: None,
            pinned: Vec::new(),
            vm: None,
            debuggee: None,
            launched: false,
            lines_start_at1: true,
            columns_start_at1: true,
            sources: SourceLocator::default(),
            breakpoints: BTreeMap::new(),
            next_breakpoint_id: 0,
            exception_filters: (false, false),
            exception_request: None,
            step_request: None,
            threads: ThreadIds::default(),
            frames: HashMap::new(),
            vars: HashMap::new(),
            next_handle: 0,
            suspends: 0,
            vm_start_pending: false,
            configured: false,
            last_stopped_thread: None,
            exceptions: HashMap::new(),
            class_sources: HashMap::new(),
            class_files: ClassFiles::default(),
            can_redefine: None,
            loaded_names: None,
            terminated_sent: false,
            done: false,
        }
    }

    /// 主循环：处理请求和 JDWP 事件，直到客户端断开（`disconnect` 或输入结束）。
    pub fn run(mut self, requests: Receiver<Value>) {
        let events = self.events_rx.clone();
        while !self.done {
            if let Some(event) = self.deferred.pop_front() {
                self.handle_jdwp(event);
                continue;
            }
            crossbeam_channel::select! {
                recv(requests) -> message => match message {
                    Ok(message) => self.handle_message(&message),
                    Err(_) => break,
                },
                recv(events) -> event => {
                    if let Ok(event) = event {
                        self.handle_jdwp(event);
                    }
                }
            }
        }
        // 客户端没打招呼就走了：由调试器启动的 JVM 一并结束，附加的 JVM 让它继续运行
        let terminate = self.launched;
        self.shutdown(terminate);
    }

    pub(crate) fn vm(&self) -> anyhow::Result<Rc<Vm>> {
        self.vm.clone().context(trs!(
            "还没有连接到目标 JVM",
            "Not connected to the target JVM"
        ))
    }

    /// 程序停在断点、单步、异常或暂停处（全部线程挂起着）。
    pub(crate) fn is_stopped(&self) -> bool {
        self.suspends > 0
    }

    pub(crate) fn new_handle(&mut self) -> i64 {
        self.next_handle += 1;
        self.next_handle
    }

    pub(crate) fn line_to_client(&self, line: i32) -> i64 {
        if self.lines_start_at1 {
            line as i64
        } else {
            line as i64 - 1
        }
    }

    pub(crate) fn line_from_client(&self, line: i64) -> i32 {
        if self.lines_start_at1 {
            line as i32
        } else {
            line as i32 + 1
        }
    }

    // ---------------------------------------------------------------
    // 请求分发
    // ---------------------------------------------------------------

    fn handle_message(&mut self, message: &Value) {
        let Some(request) = Request::parse(message) else {
            return;
        };
        let args = &request.arguments;
        let result = match request.command.as_str() {
            "initialize" => self.initialize(args),
            "launch" => self.launch(args),
            "attach" => self.attach(args),
            "configurationDone" => self.configuration_done(),
            "setBreakpoints" => self.set_breakpoints(args),
            "setExceptionBreakpoints" => self.set_exception_breakpoints(args),
            "threads" => self.threads(),
            "stackTrace" => self.stack_trace(args),
            "scopes" => self.scopes(args),
            "variables" => self.variables(args),
            "setVariable" => self.set_variable(args),
            "evaluate" => self.evaluate(args),
            "completions" => self.completions(args),
            "continue" => self.continue_(),
            "next" => self.step(args, StepDepth::Over),
            "stepIn" => self.step(args, StepDepth::Into),
            "stepOut" => self.step(args, StepDepth::Out),
            "pause" => self.pause(args),
            "source" => self.source(args),
            "exceptionInfo" => self.exception_info(args),
            "redefineClasses" => self.redefine_classes(args),
            "terminate" => self.terminate(),
            "disconnect" => self.disconnect(args),
            other => Err(anyhow::anyhow!(tr!(
                "不支持的请求 `{other}`",
                "Unsupported request `{other}`"
            ))),
        };
        let success = result.is_ok();
        self.out
            .response(&request, result.map_err(|err| format!("{err:#}")));
        match request.command.as_str() {
            // 连上 JVM 之后才能接受断点配置
            "launch" | "attach" if success => self.out.event("initialized", json!({})),
            "disconnect" => self.done = true,
            _ => {}
        }
    }

    fn initialize(&mut self, args: &Value) -> anyhow::Result<Value> {
        // 文案的语言跟随客户端；语言是进程级的设置，一个适配器进程只服务一个会话
        let locale = args.get("locale").and_then(Value::as_str);
        base_db::set_lang(base_db::lang_of_locale(locale));
        self.lines_start_at1 = args
            .get("linesStartAt1")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        self.columns_start_at1 = args
            .get("columnsStartAt1")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        Ok(json!({
            "supportsConfigurationDoneRequest": true,
            "supportsConditionalBreakpoints": true,
            "supportsHitConditionalBreakpoints": true,
            "supportsLogPoints": true,
            "supportsEvaluateForHovers": true,
            "supportsCompletionsRequest": true,
            "completionTriggerCharacters": ["."],
            "supportsSetVariable": true,
            "supportsTerminateRequest": true,
            "supportsExceptionInfoRequest": true,
            "supportTerminateDebuggee": true,
            "supportsDelayedStackTraceLoading": true,
            // 自定义请求 `redefineClasses`（热替换）
            "supportsRedefineClasses": true,
            "exceptionBreakpointFilters": [
                { "filter": "caught", "label": "Caught Exceptions", "default": false },
                { "filter": "uncaught", "label": "Uncaught Exceptions", "default": false },
            ],
        }))
    }

    // ---------------------------------------------------------------
    // 启动和附加
    // ---------------------------------------------------------------

    fn launch(&mut self, args: &Value) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.vm.is_none(),
            trs!("已经在调试中", "A debug session is already running")
        );
        let args: LaunchArgs = serde_json::from_value(args.clone())
            .context(trs!("launch 参数格式不对", "Malformed launch arguments"))?;
        let main_class = args
            .main_class
            .clone()
            .filter(|c| !c.trim().is_empty())
            .context(trs!(
                "launch 需要 mainClass（主类的全限定名）",
                "launch needs mainClass (the fully qualified name of the main class)"
            ))?;
        let cwd_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let root = args
            .project_root
            .clone()
            .or_else(|| args.cwd.clone())
            .unwrap_or_else(|| cwd_dir.clone());
        let root = project_model::canonicalize(&root).unwrap_or(root);
        let cwd = args.cwd.clone().unwrap_or_else(|| root.clone());

        // 没有给类路径时从工程模型算，这时允许调用构建工具解析依赖
        let explicit = args.class_paths.is_some() || args.module_paths.is_some();
        let project = launch::load_project(&root, !explicit, &args.settings);
        for message in &project.messages {
            tracing::info!("导入工程: {message}");
        }
        let class_path: Vec<String> = match &args.class_paths {
            Some(paths) => paths.clone(),
            None if args.module_paths.is_some() => Vec::new(),
            None => launch::runtime_class_path(&project.graph, &main_class)?
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        };
        self.configure_sources(&project.graph, &args.source_paths);
        // 类路径和模块路径里的目录：热替换时在这里找重新编译过的类
        let mut output_dirs: Vec<PathBuf> = class_path.iter().map(PathBuf::from).collect();
        output_dirs.extend(args.module_paths.iter().flatten().map(PathBuf::from));
        output_dirs.retain(|dir| dir.is_dir());
        output_dirs.dedup();
        self.class_files = ClassFiles::record(output_dirs);

        let java = launch::java_executable(
            args.java_exec.as_deref(),
            project.graph.jdk.as_ref().map(|jdk| jdk.home.as_path()),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .context(trs!("无法监听本地端口", "Cannot listen on a local port"))?;
        let address = listener.local_addr()?.to_string();
        let encoding = launch::project_encoding(&project.graph, &main_class);
        let command = launch::build_command(
            &java,
            &args,
            &main_class,
            &class_path,
            &address,
            &cwd,
            encoding.as_deref(),
        )?;
        tracing::info!("启动调试目标: {:?}", command.command);
        if let Some(file) = &command.arg_file {
            tracing::info!(
                "类路径在参数文件 {} 里: {}",
                file.path.display(),
                file.class_path
            );
        }
        let mut debuggee = Debuggee::spawn(command, &self.out)?;
        let stream = match launch::accept_debuggee(&listener, &mut debuggee, CONNECT_TIMEOUT) {
            Ok(stream) => stream,
            Err(err) => {
                debuggee.kill();
                debuggee.drain_output(Duration::from_secs(1));
                return Err(err);
            }
        };
        self.debuggee = Some(debuggee);
        self.launched = true;
        if let Err(err) = self.connect(stream, Duration::from_secs(10)) {
            self.shutdown(true);
            return Err(err);
        }
        Ok(Value::Null)
    }

    fn attach(&mut self, args: &Value) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.vm.is_none(),
            trs!("已经在调试中", "A debug session is already running")
        );
        let args = AttachArgs::parse(args)?;
        let root = args
            .project_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let project = launch::load_project(&root, false, &args.settings);
        self.configure_sources(&project.graph, &args.source_paths);
        let output_dirs: Vec<PathBuf> = project
            .graph
            .modules
            .iter()
            .flat_map(|m| &m.output_dirs)
            .map(|dir| dir.path.clone())
            .filter(|dir| dir.is_dir())
            .collect();
        self.class_files = ClassFiles::record(output_dirs);

        let addrs: Vec<_> = (args.host_name.as_str(), args.port)
            .to_socket_addrs()
            .with_context(|| {
                tr!(
                    "无法解析主机名 {}",
                    "Cannot resolve the host name {}",
                    args.host_name
                )
            })?
            .collect();
        let mut last_err = None;
        let mut stream = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, args.timeout) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(err) => last_err = Some(err),
            }
        }
        let stream = match stream {
            Some(stream) => stream,
            None => anyhow::bail!(tr!(
                "无法连接到 {}:{}{}",
                "Cannot connect to {}:{}{}",
                args.host_name,
                args.port,
                last_err.map(|e| format!(": {e}")).unwrap_or_default()
            )),
        };
        self.launched = false;
        // 以 suspend=y 启动、等着调试器的 JVM 会立刻发 VMStart；已经在运行的不会
        if let Err(err) = self.connect(stream, Duration::from_millis(500)) {
            self.shutdown(false);
            return Err(err);
        }
        Ok(Value::Null)
    }

    fn configure_sources(&mut self, graph: &base_db::ProjectGraph, extra: &[PathBuf]) {
        let mut roots: Vec<PathBuf> = extra.to_vec();
        roots.extend(graph.all_source_roots());
        let mut archives: Vec<PathBuf> = graph
            .libraries
            .iter()
            .filter_map(|lib| lib.sources.clone())
            .collect();
        let jdk = graph
            .jdk
            .clone()
            .or_else(|| project_model::discover_jdk(None));
        if let Some(src_zip) = jdk.and_then(|jdk| jdk.src_zip) {
            archives.push(src_zip);
        }
        self.sources = SourceLocator::new(roots, archives);
        self.class_sources.clear();
    }

    /// 握手、读 ID 长度、注册线程事件，然后（最多 `start_wait`）等 VMStart，最后装上已有的断点。
    fn connect(&mut self, stream: TcpStream, start_wait: Duration) -> anyhow::Result<()> {
        let conn = Connection::handshake(stream, self.events_tx.clone(), REPLY_TIMEOUT)?;
        let vm = Rc::new(Vm::new(conn)?);
        self.can_redefine = None;
        match vm.version() {
            Ok(version) => tracing::info!("已连接到 {version}"),
            Err(err) => tracing::warn!("读取 JVM 版本失败: {err:#}"),
        }
        vm.set_event(event_kind::THREAD_START, suspend_policy::NONE, &[])?;
        vm.set_event(event_kind::THREAD_DEATH, suspend_policy::NONE, &[])?;
        self.vm = Some(vm);
        self.abort_exception = None;
        self.pinned.clear();

        let deadline = Instant::now() + start_wait;
        while !self.vm_start_pending {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events_rx.recv_timeout(left) {
                Ok(JdwpEvent::Closed) => {
                    self.handle_jdwp(JdwpEvent::Closed);
                    anyhow::bail!(trs!(
                        "目标 JVM 在调试开始前就退出了",
                        "The target JVM exited before debugging started"
                    ));
                }
                Ok(event) => self.handle_jdwp(event),
                Err(_) => break,
            }
        }

        let sources: Vec<PathBuf> = self.breakpoints.keys().cloned().collect();
        for path in sources {
            self.install_source(&path);
        }
        self.apply_exception_request()?;
        Ok(())
    }

    fn configuration_done(&mut self) -> anyhow::Result<Value> {
        self.configured = true;
        if self.vm_start_pending {
            self.vm_start_pending = false;
            self.vm()?.resume()?;
        }
        Ok(Value::Null)
    }

    // ---------------------------------------------------------------
    // 结束
    // ---------------------------------------------------------------

    fn terminate(&mut self) -> anyhow::Result<Value> {
        match self.vm.clone() {
            Some(vm) => {
                // JVM 退出后连接断开，那时再发 exited / terminated
                if vm.exit(0).is_err() {
                    if let Some(debuggee) = &mut self.debuggee {
                        debuggee.kill();
                    }
                }
            }
            None => {
                if let Some(debuggee) = &mut self.debuggee {
                    debuggee.kill();
                }
                self.send_terminated();
            }
        }
        Ok(Value::Null)
    }

    fn disconnect(&mut self, args: &Value) -> anyhow::Result<Value> {
        let terminate = args
            .get("terminateDebuggee")
            .and_then(Value::as_bool)
            .unwrap_or(self.launched);
        self.shutdown(terminate);
        Ok(Value::Null)
    }

    /// 断开与 JVM 的连接。`terminate` 时结束 JVM，否则让它继续运行。
    fn shutdown(&mut self, terminate: bool) {
        if let Some(vm) = self.vm.take() {
            let result = if terminate { vm.exit(1) } else { vm.dispose() };
            if let Err(err) = result {
                tracing::debug!("断开 JVM 时出错: {err:#}");
            }
            vm.connection().shutdown();
        }
        self.invalidate_handles();
        if let Some(mut debuggee) = self.debuggee.take() {
            if terminate {
                debuggee.wait_or_kill(Duration::from_secs(2));
                debuggee.drain_output(Duration::from_millis(500));
            } else {
                // 让它继续运行：不要在 drop 时杀掉
                std::mem::forget(debuggee);
            }
        }
    }

    fn send_terminated(&mut self) {
        if !self.terminated_sent {
            self.terminated_sent = true;
            self.out.event("terminated", json!({}));
        }
    }

    /// 连接断开：JVM 退出了（或者网络断了）。
    fn on_vm_closed(&mut self) {
        if self.vm.take().is_none() && self.terminated_sent {
            return;
        }
        self.invalidate_handles();
        self.vm_start_pending = false;
        self.suspends = 0;
        self.abort_exception = None;
        self.pinned.clear();
        for source in self.breakpoints.values_mut() {
            source.reset();
        }
        if let Some(mut debuggee) = self.debuggee.take() {
            let code = debuggee.wait_or_kill(Duration::from_secs(5));
            debuggee.drain_output(Duration::from_secs(2));
            self.out.event("exited", json!({ "exitCode": code }));
        }
        self.send_terminated();
    }

    // ---------------------------------------------------------------
    // JDWP 事件
    // ---------------------------------------------------------------

    fn handle_jdwp(&mut self, event: JdwpEvent) {
        match event {
            JdwpEvent::Closed => self.on_vm_closed(),
            JdwpEvent::Composite(data) => {
                let Some(vm) = self.vm.clone() else { return };
                match vm.parse_events(&data) {
                    Ok(set) => self.handle_event_set(&vm, set),
                    Err(err) => tracing::warn!("无法解析 JDWP 事件: {err:#}"),
                }
            }
        }
    }

    pub(crate) fn handle_event_set(&mut self, vm: &Rc<Vm>, set: EventSet) {
        let mut stop: Option<Stop> = None;
        let mut hold = false;
        let merge = |stop: &mut Option<Stop>, new: Stop| {
            if stop.as_ref().is_none_or(|s| new.priority() > s.priority()) {
                *stop = Some(new);
            }
        };
        for event in &set.events {
            // 求值调用方法期间，被调用的方法碰到断点、单步或异常都不停下，接着执行
            if self.invoking.is_some()
                && matches!(
                    event,
                    Event::Breakpoint { .. } | Event::SingleStep { .. } | Event::Exception { .. }
                )
            {
                continue;
            }
            match event {
                Event::VmStart { .. } => {
                    if set.suspend_policy != suspend_policy::NONE {
                        self.vm_start_pending = true;
                        hold = true;
                    }
                }
                Event::VmDeath => {}
                Event::ThreadStart { thread } => {
                    let id = self.threads.dap(*thread);
                    self.out
                        .event("thread", json!({ "reason": "started", "threadId": id }));
                }
                Event::ThreadDeath { thread } => {
                    let id = self.threads.dap(*thread);
                    self.out
                        .event("thread", json!({ "reason": "exited", "threadId": id }));
                }
                Event::ClassPrepare {
                    class, signature, ..
                } => {
                    vm.remember_signature(*class, signature);
                    self.on_class_prepare(*class, &signature_to_type_name(signature));
                }
                Event::Breakpoint {
                    request,
                    thread,
                    location,
                } => {
                    if let Some(hit) = self.on_breakpoint(*request, *thread, location) {
                        merge(
                            &mut stop,
                            Stop {
                                reason: "breakpoint",
                                thread: *thread,
                                description: None,
                                text: None,
                                hit_breakpoints: vec![hit],
                            },
                        );
                    }
                }
                Event::SingleStep {
                    request, thread, ..
                } => {
                    if self.step_request == Some(*request) {
                        merge(
                            &mut stop,
                            Stop {
                                reason: "step",
                                thread: *thread,
                                description: None,
                                text: None,
                                hit_breakpoints: Vec::new(),
                            },
                        );
                    }
                }
                Event::Exception {
                    thread,
                    exception,
                    catch_location,
                    ..
                } => {
                    if let Some(id) = exception.object_id() {
                        let type_name = vm
                            .object_type(id)
                            .and_then(|class| vm.class_name(class))
                            .unwrap_or_else(|_| "java.lang.Throwable".to_owned());
                        let message = self.throwable_message(id);
                        let caught = catch_location.is_some();
                        let text = match &message {
                            Some(message) => format!("{type_name}: {message}"),
                            None => type_name.clone(),
                        };
                        self.exceptions.insert(
                            *thread,
                            ExceptionStop {
                                exception: id,
                                type_name: type_name.clone(),
                                message,
                                caught,
                            },
                        );
                        merge(
                            &mut stop,
                            Stop {
                                reason: "exception",
                                thread: *thread,
                                description: Some(format!(
                                    "{} {}",
                                    if caught {
                                        "Exception"
                                    } else {
                                        "Uncaught exception"
                                    },
                                    simple_name(&type_name)
                                )),
                                text: Some(text),
                                hit_breakpoints: Vec::new(),
                            },
                        );
                    }
                }
                Event::Other { .. } => {}
            }
        }

        if let Some(stop) = stop {
            if set.suspend_policy == suspend_policy::ALL {
                self.suspends += 1;
            }
            self.report_stop(stop);
        } else if !hold {
            let result = match set.suspend_policy {
                suspend_policy::ALL => vm.resume(),
                suspend_policy::EVENT_THREAD => match set.events.iter().find_map(Event::thread) {
                    Some(thread) => vm.thread_resume(thread),
                    None => Ok(()),
                },
                _ => Ok(()),
            };
            if let Err(err) = result {
                tracing::debug!("恢复运行失败: {err:#}");
            }
        }
    }

    fn report_stop(&mut self, stop: Stop) {
        self.clear_step_request();
        self.invalidate_handles();
        self.last_stopped_thread = Some(stop.thread);
        let thread_id = self.threads.dap(stop.thread);
        let mut body = json!({
            "reason": stop.reason,
            "threadId": thread_id,
            "allThreadsStopped": true,
        });
        if let Some(description) = stop.description {
            body["description"] = json!(description);
        }
        if let Some(text) = stop.text {
            body["text"] = json!(text);
        }
        if !stop.hit_breakpoints.is_empty() {
            body["hitBreakpointIds"] = json!(stop.hit_breakpoints);
        }
        self.out.event("stopped", body);
    }

    fn clear_step_request(&mut self) {
        if let Some(request) = self.step_request.take() {
            if let Some(vm) = &self.vm {
                let _ = vm.clear_event(event_kind::SINGLE_STEP, request);
            }
        }
    }

    pub(crate) fn invalidate_handles(&mut self) {
        self.frames.clear();
        self.vars.clear();
        self.loaded_names = None;
    }

    /// 恢复所有线程：把停下时的挂起一次性抵消掉。
    fn resume_all(&mut self) -> anyhow::Result<()> {
        let vm = self.vm()?;
        self.invalidate_handles();
        self.vm_start_pending = false;
        for object in self.pinned.drain(..) {
            let _ = vm.enable_collection(object);
        }
        let times = self.suspends.max(1);
        self.suspends = 0;
        for _ in 0..times {
            vm.resume()?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------
    // 执行控制
    // ---------------------------------------------------------------

    fn thread_arg(&self, args: &Value) -> anyhow::Result<u64> {
        let id = args
            .get("threadId")
            .and_then(Value::as_i64)
            .context(trs!("缺少 threadId", "Missing threadId"))?;
        self.threads
            .jdwp(id)
            .with_context(|| tr!("未知的线程 {id}", "Unknown thread {id}"))
    }

    fn continue_(&mut self) -> anyhow::Result<Value> {
        self.clear_step_request();
        self.resume_all()?;
        Ok(json!({ "allThreadsContinued": true }))
    }

    fn step(&mut self, args: &Value, depth: StepDepth) -> anyhow::Result<Value> {
        let vm = self.vm()?;
        let thread = self.thread_arg(args)?;
        self.clear_step_request();
        let mut modifiers = vec![Modifier::Step { thread, depth }];
        modifiers.extend(
            STEP_EXCLUDES
                .iter()
                .map(|pattern| Modifier::ClassExclude((*pattern).to_owned())),
        );
        // 计数放在最后：只数通过了前面过滤的事件
        modifiers.push(Modifier::Count(1));
        let request = vm.set_event(event_kind::SINGLE_STEP, suspend_policy::ALL, &modifiers)?;
        self.step_request = Some(request);
        self.resume_all()?;
        Ok(Value::Null)
    }

    fn pause(&mut self, args: &Value) -> anyhow::Result<Value> {
        let vm = self.vm()?;
        if self.suspends == 0 && !self.vm_start_pending {
            vm.suspend()?;
            self.suspends += 1;
        }
        let thread = match self.thread_arg(args) {
            Ok(thread) => thread,
            Err(_) => self.default_thread(&vm)?,
        };
        self.report_stop(Stop {
            reason: "pause",
            thread,
            description: None,
            text: None,
            hit_breakpoints: Vec::new(),
        });
        Ok(Value::Null)
    }

    /// 没有指定线程时用的线程：`main`，没有就取第一个。
    fn default_thread(&mut self, vm: &Vm) -> anyhow::Result<u64> {
        let threads = vm.all_threads()?;
        let main = threads
            .iter()
            .copied()
            .find(|&t| vm.thread_name(t).is_ok_and(|name| name == "main"));
        main.or_else(|| threads.first().copied())
            .context(trs!("目标 JVM 里没有线程", "The target JVM has no threads"))
    }

    fn threads(&mut self) -> anyhow::Result<Value> {
        let Some(vm) = self.vm.clone() else {
            return Ok(json!({ "threads": [] }));
        };
        let mut threads = Vec::new();
        for thread in vm.all_threads()? {
            // 线程可能刚好结束了
            let Ok(name) = vm.thread_name(thread) else {
                continue;
            };
            let id = self.threads.dap(thread);
            threads.push(json!({ "id": id, "name": name }));
        }
        Ok(json!({ "threads": threads }))
    }

    // ---------------------------------------------------------------
    // 调用栈
    // ---------------------------------------------------------------

    fn stack_trace(&mut self, args: &Value) -> anyhow::Result<Value> {
        let vm = self.vm()?;
        let thread = self.thread_arg(args)?;
        let frames = match vm.frames(thread, 0, -1) {
            Ok(frames) => frames,
            Err(err) if jdwp::is_error(&err, jdwp::error_code::THREAD_NOT_SUSPENDED) => {
                anyhow::bail!(trs!(
                    "线程正在运行，先暂停才能查看调用栈",
                    "The thread is running; pause it to see its call stack"
                ))
            }
            Err(err) => return Err(err),
        };
        let total = frames.len();
        let start = args.get("startFrame").and_then(Value::as_u64).unwrap_or(0) as usize;
        let levels = args
            .get("levels")
            .and_then(Value::as_u64)
            .filter(|&l| l > 0)
            .map_or(total, |l| l as usize);
        let column = if self.columns_start_at1 { 1 } else { 0 };
        let mut result = Vec::new();
        for (depth, (frame, location)) in frames.into_iter().enumerate().skip(start).take(levels) {
            let id = self.new_handle();
            self.frames.insert(
                id,
                FrameRef {
                    thread,
                    frame,
                    location,
                    depth: depth as u32,
                },
            );
            let class_name = vm
                .class_name(location.class)
                .unwrap_or_else(|_| "?".to_owned());
            let method = vm.method(location.class, location.method).ok().flatten();
            let method_name = method.as_ref().map_or("?", |m| m.name.as_str());
            let line = vm
                .line_table(location.class, location.method)
                .ok()
                .and_then(|table| table.line_of(location.index))
                .unwrap_or(0);
            let mut entry = json!({
                "id": id,
                "name": format!("{}.{method_name}", simple_name(&class_name)),
                "line": if line > 0 { self.line_to_client(line) } else { 0 },
                "column": column,
            });
            match self.source_of_class(&vm, location.class) {
                Some(source) => entry["source"] = source,
                None => entry["presentationHint"] = json!("subtle"),
            }
            result.push(entry);
        }
        Ok(json!({ "stackFrames": result, "totalFrames": total }))
    }

    /// 栈帧里显示的源码：源码根下的文件，或者源码包里的条目（按 `sourceReference` 取）。
    fn source_of_class(&mut self, vm: &Vm, class: u64) -> Option<Value> {
        if let Some(source) = self.class_sources.get(&class) {
            return source.clone();
        }
        let source = (|| {
            let class_name = vm.class_name(class).ok()?;
            let source_file = vm.source_file(class).ok().flatten();
            let relative = relative_source_path(&class_name, source_file.as_deref());
            match self.sources.find(&relative)? {
                FoundSource::File(path) => Some(json!({
                    "name": path.file_name().map(|n| n.to_string_lossy().into_owned()),
                    "path": path,
                })),
                FoundSource::Archive { reference, name } => {
                    let origin = self
                        .sources
                        .reference_origin(reference)
                        .and_then(Path::file_name)
                        .map(|n| n.to_string_lossy().into_owned());
                    Some(json!({
                        "name": name,
                        "sourceReference": reference,
                        "origin": origin,
                        "presentationHint": "deemphasize",
                    }))
                }
            }
        })();
        self.class_sources.insert(class, source.clone());
        source
    }

    fn source(&mut self, args: &Value) -> anyhow::Result<Value> {
        let reference = args
            .get("sourceReference")
            .or_else(|| args.get("source").and_then(|s| s.get("sourceReference")))
            .and_then(Value::as_i64)
            .filter(|&r| r > 0)
            .context(trs!("缺少 sourceReference", "Missing sourceReference"))?;
        let content = self.sources.read_reference(reference)?;
        Ok(json!({ "content": content, "mimeType": "text/x-java" }))
    }

    fn exception_info(&mut self, args: &Value) -> anyhow::Result<Value> {
        let thread = self.thread_arg(args)?;
        let info = self.exceptions.get(&thread).context(trs!(
            "这个线程没有因为异常停下",
            "This thread did not stop on an exception"
        ))?;
        Ok(json!({
            "exceptionId": info.type_name,
            "description": info.message,
            "breakMode": if info.caught { "always" } else { "unhandled" },
            "details": {
                "message": info.message,
                "typeName": simple_name(&info.type_name),
                "fullTypeName": info.type_name,
                "evaluateName": null,
                "objectId": info.exception,
            },
        }))
    }

    // ---------------------------------------------------------------
    // 异常断点
    // ---------------------------------------------------------------

    fn set_exception_breakpoints(&mut self, args: &Value) -> anyhow::Result<Value> {
        let mut filters: Vec<String> = args
            .get("filters")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(options) = args.get("filterOptions").and_then(Value::as_array) {
            filters.extend(
                options
                    .iter()
                    .filter_map(|o| o.get("filterId").and_then(Value::as_str))
                    .map(str::to_owned),
            );
        }
        let caught = filters.iter().any(|f| f == "caught");
        let uncaught = filters.iter().any(|f| f == "uncaught");
        self.exception_filters = (caught, uncaught);
        if self.vm.is_some() {
            self.apply_exception_request()?;
        }
        let breakpoints: Vec<Value> = filters
            .iter()
            .map(|_| json!({ "verified": true }))
            .collect();
        Ok(json!({ "breakpoints": breakpoints }))
    }

    fn apply_exception_request(&mut self) -> anyhow::Result<()> {
        let vm = self.vm()?;
        if let Some(request) = self.exception_request.take() {
            let _ = vm.clear_event(event_kind::EXCEPTION, request);
        }
        let (caught, uncaught) = self.exception_filters;
        if caught || uncaught {
            let request = vm.set_event(
                event_kind::EXCEPTION,
                suspend_policy::ALL,
                &[Modifier::ExceptionOnly {
                    class: 0,
                    caught,
                    uncaught,
                }],
            )?;
            self.exception_request = Some(request);
        }
        Ok(())
    }
}
