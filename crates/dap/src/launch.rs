//! `launch` / `attach` 的参数、运行时类路径的计算、启动调试目标 JVM 并转发它的输出。

use std::{
    collections::HashMap,
    io::Read,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::Context;
use base_db::{ModuleId, ProjectGraph, SourceRootKind};
use crossbeam_channel::{Receiver, Sender};
use serde::Deserialize;
use serde_json::Value;

use crate::protocol::Output;

/// 字符串（按空白拆分，支持引号）或字符串数组。
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum StringOrList {
    String(String),
    List(Vec<String>),
}

impl StringOrList {
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            StringOrList::String(s) => split_args(s),
            StringOrList::List(list) => list.clone(),
        }
    }
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchArgs {
    pub main_class: Option<String>,
    pub project_root: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub class_paths: Option<Vec<String>>,
    pub module_paths: Option<Vec<String>>,
    pub vm_args: Option<StringOrList>,
    pub args: Option<StringOrList>,
    pub env: HashMap<String, Option<String>>,
    pub java_exec: Option<PathBuf>,
    pub source_paths: Vec<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub struct AttachArgs {
    pub host_name: String,
    pub port: u16,
    pub project_root: Option<PathBuf>,
    pub source_paths: Vec<PathBuf>,
    pub timeout: Duration,
}

impl AttachArgs {
    pub fn parse(args: &Value) -> anyhow::Result<AttachArgs> {
        let host_name = args
            .get("hostName")
            .and_then(Value::as_str)
            .filter(|h| !h.is_empty())
            .unwrap_or("localhost")
            .to_owned();
        // 端口可以写成数字或字符串
        let port = match args.get("port") {
            Some(Value::Number(n)) => n.as_u64(),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        }
        .and_then(|p| u16::try_from(p).ok())
        .filter(|&p| p != 0)
        .context("attach 需要 port（目标 JVM 的 JDWP 端口）")?;
        let path = |key: &str| args.get(key).and_then(Value::as_str).map(PathBuf::from);
        let source_paths = args
            .get("sourcePaths")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        let timeout = args
            .get("timeout")
            .and_then(Value::as_u64)
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(10));
        Ok(AttachArgs {
            host_name,
            port,
            project_root: path("projectRoot").or_else(|| path("cwd")),
            source_paths,
            timeout,
        })
    }
}

/// 按空白拆分命令行参数，单双引号里的空白不拆。
pub fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut has_token = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                has_token = true;
            }
            None if c.is_whitespace() => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            None => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    args
}

/// 导入工程。`full` 为假时不调用 mvn / gradle，只取源码根和已知的构件（用于显示源码）。
pub fn load_project(root: &Path, full: bool) -> project_model::LoadedProject {
    let options = project_model::LoadOptions {
        jdk_home: None,
        maven_use_cli: full,
        gradle_allow_run: full,
    };
    project_model::load_projects(&[root.to_path_buf()], &options)
}

/// 主类在哪个模块、是不是测试代码。先看编译输出目录里有没有类文件，再看源码根里有没有源文件。
fn module_of_main_class(graph: &ProjectGraph, main_class: &str) -> Option<(ModuleId, bool)> {
    let class = main_class.rsplit('/').next().unwrap_or(main_class);
    let class_file = format!("{}.class", class.replace('.', "/"));
    let source_file = format!("{}.java", class.replace('.', "/"));
    for (i, module) in graph.modules.iter().enumerate() {
        if let Some(dir) = module
            .output_dirs
            .iter()
            .find(|dir| dir.path.join(&class_file).is_file())
        {
            return Some((ModuleId(i as u32), dir.kind == SourceRootKind::Test));
        }
    }
    for (i, module) in graph.modules.iter().enumerate() {
        if let Some(root) = module
            .source_roots
            .iter()
            .find(|root| root.path.join(&source_file).is_file())
        {
            return Some((ModuleId(i as u32), root.kind == SourceRootKind::Test));
        }
    }
    None
}

/// 从工程模型算主类运行时的类路径：主类所在模块的类路径（见 [`project_model::runtime_class_path`]），
/// 主类在测试代码里时按测试的算。找不到主类所在的模块时，带上所有模块的主输出目录和所有依赖库。
pub fn runtime_class_path(graph: &ProjectGraph, main_class: &str) -> anyhow::Result<Vec<PathBuf>> {
    let Some((start, is_test)) = module_of_main_class(graph, main_class) else {
        let has_output = graph
            .modules
            .iter()
            .flat_map(|m| &m.output_dirs)
            .any(|dir| dir.path.is_dir());
        anyhow::ensure!(
            has_output,
            "工程里没有找到编译输出目录（target/classes、build/classes/java/main 等）。\
             调试器不负责构建，请先编译工程（例如 `mvn compile` 或 `gradle classes`），或者在 launch 参数里给出 classPaths"
        );
        let mut entries: Vec<PathBuf> = Vec::new();
        let candidates = graph
            .modules
            .iter()
            .flat_map(|module| &module.output_dirs)
            .filter(|d| d.kind == SourceRootKind::Main)
            .map(|d| &d.path)
            .chain(graph.libraries.iter().map(|library| &library.jar));
        for path in candidates {
            if !entries.contains(path) {
                entries.push(path.clone());
            }
        }
        entries.retain(|e| e.exists());
        return Ok(entries);
    };

    let class_path = project_model::runtime_class_path(graph, start, is_test);
    anyhow::ensure!(
        class_path.output_dirs.iter().any(|dir| dir.is_dir()),
        "模块 {} 的编译输出目录不存在（{}）。调试器不负责构建，请先编译工程",
        graph.module(start).name,
        class_path
            .output_dirs
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(class_path.entries)
}

/// 选 `java` 可执行文件：显式配置 → 工程的 JDK → `JAVA_HOME` / 系统里找到的 JDK → `PATH` 里的 `java`。
pub fn java_executable(configured: Option<&Path>, project_jdk: Option<&Path>) -> PathBuf {
    if let Some(path) = configured {
        return path.to_path_buf();
    }
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let from_home = |home: &Path| Some(home.join("bin").join(exe)).filter(|p| p.is_file());
    project_jdk
        .and_then(from_home)
        .or_else(|| project_model::discover_jdk(None).and_then(|jdk| from_home(&jdk.home)))
        .unwrap_or_else(|| PathBuf::from(exe))
}

/// 由调试器启动的 JVM 进程。
pub struct Debuggee {
    child: Child,
    output_done: Receiver<()>,
    output_threads: usize,
    exit_code: Option<i32>,
}

impl Debuggee {
    /// 启动进程并开始把它的标准输出 / 标准错误转成 `output` 事件。
    pub fn spawn(mut command: Command, out: &Output) -> anyhow::Result<Debuggee> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let program = command.get_program().to_string_lossy().into_owned();
        let mut child = command
            .spawn()
            .with_context(|| format!("无法启动 {program}"))?;
        let (done_tx, done_rx) = crossbeam_channel::unbounded();
        let mut threads = 0;
        if let Some(stdout) = child.stdout.take() {
            forward(stdout, "stdout", out.clone(), done_tx.clone());
            threads += 1;
        }
        if let Some(stderr) = child.stderr.take() {
            forward(stderr, "stderr", out.clone(), done_tx);
            threads += 1;
        }
        Ok(Debuggee {
            child,
            output_done: done_rx,
            output_threads: threads,
            exit_code: None,
        })
    }

    pub fn try_exit_code(&mut self) -> Option<i32> {
        if self.exit_code.is_none() {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.exit_code = Some(status.code().unwrap_or(-1));
            }
        }
        self.exit_code
    }

    /// 等进程退出，最多等 `timeout`；超时就杀掉。
    pub fn wait_or_kill(&mut self, timeout: Duration) -> i32 {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(code) = self.try_exit_code() {
                return code;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        self.kill()
    }

    pub fn kill(&mut self) -> i32 {
        if let Some(code) = self.try_exit_code() {
            return code;
        }
        let _ = self.child.kill();
        let code = self.child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        self.exit_code = Some(code);
        code
    }

    /// 等转发输出的线程读完管道（最多等 `timeout`），保证 `exited` 之前输出都发出去了。
    pub fn drain_output(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.output_threads > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if self.output_done.recv_timeout(left).is_err() {
                break;
            }
            self.output_threads -= 1;
        }
    }
}

impl Drop for Debuggee {
    fn drop(&mut self) {
        // 编辑器异常退出时也不留下孤儿 JVM
        if self.try_exit_code().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn forward(
    mut pipe: impl Read + Send + 'static,
    category: &'static str,
    out: Output,
    done: Sender<()>,
) {
    let spawned = std::thread::Builder::new()
        .name(format!("debuggee-{category}"))
        .spawn({
            let done = done.clone();
            move || {
                let mut buf = [0u8; 8192];
                // 上次读到的不完整 UTF-8 序列
                let mut pending: Vec<u8> = Vec::new();
                loop {
                    let n = match pipe.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    pending.extend_from_slice(&buf[..n]);
                    let valid = match std::str::from_utf8(&pending) {
                        Ok(_) => pending.len(),
                        Err(e) if e.error_len().is_none() => e.valid_up_to(),
                        // 真正的非法字节：整段按有损方式发出去
                        Err(_) => pending.len(),
                    };
                    let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
                    pending.drain(..valid);
                    out.output(category, &text);
                }
                if !pending.is_empty() {
                    out.output(category, &String::from_utf8_lossy(&pending));
                }
                let _ = done.send(());
            }
        });
    if spawned.is_err() {
        let _ = done.send(());
    }
}

/// 等调试目标 JVM 连回来。进程提前退出或超时都报错。
pub fn accept_debuggee(
    listener: &TcpListener,
    debuggee: &mut Debuggee,
    timeout: Duration,
) -> anyhow::Result<TcpStream> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                return Ok(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e).context("等待调试目标连接失败"),
        }
        if let Some(code) = debuggee.try_exit_code() {
            anyhow::bail!("JVM 在连上调试器之前就退出了（退出码 {code}），详见输出");
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "JVM 在 {timeout:?} 内没有连上调试器"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 拼出启动 JVM 的命令。`address` 是调试器监听的 `127.0.0.1:端口`。
pub fn build_command(
    java: &Path,
    args: &LaunchArgs,
    main_class: &str,
    class_path: &[String],
    address: &str,
    cwd: &Path,
) -> anyhow::Result<Command> {
    let mut command = Command::new(java);
    command.current_dir(cwd);
    for (key, value) in &args.env {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    command.args(
        args.vm_args
            .as_ref()
            .map(StringOrList::to_vec)
            .unwrap_or_default(),
    );
    command.arg(format!(
        "-agentlib:jdwp=transport=dt_socket,server=n,suspend=y,address={address}"
    ));
    let module_path: Vec<&String> = args.module_paths.iter().flatten().collect();
    if !module_path.is_empty() {
        let joined = std::env::join_paths(module_path.iter().map(|p| p.as_str()))
            .context("modulePaths 里有不能出现在路径列表里的字符")?;
        command.arg("--module-path").arg(joined);
    }
    if !class_path.is_empty() {
        let joined = std::env::join_paths(class_path.iter())
            .context("classPaths 里有不能出现在路径列表里的字符")?;
        command.arg("-cp").arg(joined);
    }
    // `模块/主类` 的写法按模块启动
    if main_class.contains('/') {
        command.arg("-m").arg(main_class);
    } else {
        command.arg(main_class);
    }
    command.args(
        args.args
            .as_ref()
            .map(StringOrList::to_vec)
            .unwrap_or_default(),
    );
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base_db::{
        Dependency, DependencyTarget, LibraryData, LibraryId, ModuleData, Scope, SourceRoot,
    };

    #[test]
    fn splits_arguments() {
        assert_eq!(
            split_args("  -Xmx1g  -Da=\"b c\" 'd e'"),
            ["-Xmx1g", "-Da=b c", "d e"]
        );
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args("\"\""), [""]);
    }

    #[test]
    fn parses_launch_arguments() {
        let args: LaunchArgs = serde_json::from_value(serde_json::json!({
            "type": "java", "request": "launch", "name": "x",
            "mainClass": "demo.Main", "vmArgs": "-Xmx64m -ea", "args": ["a b", "c"],
            "env": {"A": "1", "B": null}, "classPaths": ["out"],
        }))
        .unwrap();
        assert_eq!(args.main_class.as_deref(), Some("demo.Main"));
        assert_eq!(args.vm_args.unwrap().to_vec(), ["-Xmx64m", "-ea"]);
        assert_eq!(args.args.unwrap().to_vec(), ["a b", "c"]);
        assert_eq!(args.env.get("B"), Some(&None));
        assert_eq!(args.class_paths.unwrap(), ["out"]);

        let attach = AttachArgs::parse(&serde_json::json!({"port": "5005"})).unwrap();
        assert_eq!(
            (attach.host_name.as_str(), attach.port),
            ("localhost", 5005)
        );
        assert!(AttachArgs::parse(&serde_json::json!({"hostName": "h"})).is_err());
    }

    #[test]
    fn class_path_from_project_model() {
        let dir = std::env::temp_dir().join(format!("dap-cp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mkdir = |p: &str| {
            let path = dir.join(p);
            std::fs::create_dir_all(&path).unwrap();
            path
        };
        let app_classes = mkdir("app/target/classes");
        let app_tests = mkdir("app/target/test-classes");
        let core_classes = mkdir("core/target/classes");
        std::fs::create_dir_all(app_classes.join("demo")).unwrap();
        std::fs::write(app_classes.join("demo/Main.class"), b"").unwrap();
        std::fs::create_dir_all(app_tests.join("demo")).unwrap();
        std::fs::write(app_tests.join("demo/MainTest.class"), b"").unwrap();
        let lib = dir.join("lib.jar");
        let test_lib = dir.join("junit.jar");
        std::fs::write(&lib, b"").unwrap();
        std::fs::write(&test_lib, b"").unwrap();

        let outputs = |main: PathBuf, test: PathBuf| {
            vec![
                SourceRoot {
                    path: main,
                    kind: SourceRootKind::Main,
                },
                SourceRoot {
                    path: test,
                    kind: SourceRootKind::Test,
                },
            ]
        };
        let graph = ProjectGraph {
            modules: vec![
                ModuleData {
                    name: "app".into(),
                    root: dir.join("app"),
                    source_roots: Vec::new(),
                    language_level: None,
                    api_release: None,
                    preview: false,
                    dependencies: vec![
                        Dependency {
                            target: DependencyTarget::Module(ModuleId(1)),
                            scope: Scope::Compile,
                        },
                        Dependency {
                            target: DependencyTarget::Library(LibraryId(1)),
                            scope: Scope::Test,
                        },
                    ],
                    output_dirs: outputs(app_classes.clone(), app_tests.clone()),
                },
                ModuleData {
                    name: "core".into(),
                    root: dir.join("core"),
                    source_roots: Vec::new(),
                    language_level: None,
                    api_release: None,
                    preview: false,
                    dependencies: vec![Dependency {
                        target: DependencyTarget::Library(LibraryId(0)),
                        scope: Scope::Compile,
                    }],
                    output_dirs: outputs(
                        core_classes.clone(),
                        dir.join("core/target/test-classes"),
                    ),
                },
            ],
            libraries: vec![
                LibraryData {
                    name: "lib".into(),
                    jar: lib.clone(),
                    sources: None,
                },
                LibraryData {
                    name: "junit".into(),
                    jar: test_lib.clone(),
                    sources: None,
                },
            ],
            jdk: None,
        };
        assert_eq!(
            runtime_class_path(&graph, "demo.Main").unwrap(),
            [app_classes.clone(), core_classes.clone(), lib.clone()]
        );
        assert_eq!(
            runtime_class_path(&graph, "demo.MainTest").unwrap(),
            [app_tests, app_classes, core_classes, test_lib, lib]
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
