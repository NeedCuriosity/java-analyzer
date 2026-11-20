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
use base_db::{tr, trs, ModuleId, ProjectGraph, SourceRootKind};
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
    #[serde(flatten)]
    pub settings: ProjectSettings,
}

/// 导入工程用的设置：调试适配器是单独的进程，看不到语言服务器收到的编辑器配置，
/// 由客户端放在 `launch` / `attach` 参数里传过来。和语言服务器一样，`java-analyzer.toml`
/// 里的 `[jdk]` / `[maven]` 优先。
#[derive(Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectSettings {
    /// JDK 的安装目录（相当于编辑器配置的 `jdkHome`）
    pub jdk_home: Option<PathBuf>,
    /// 代替 `~/.m2/settings.xml` 的 Maven 设置文件（`maven.settings`）
    pub maven_settings: Option<PathBuf>,
    /// Maven 本地仓库的目录（`maven.localRepository`）
    pub maven_local_repository: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub struct AttachArgs {
    pub host_name: String,
    pub port: u16,
    pub project_root: Option<PathBuf>,
    pub source_paths: Vec<PathBuf>,
    pub timeout: Duration,
    pub settings: ProjectSettings,
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
        .context(trs!(
            "attach 需要 port（目标 JVM 的 JDWP 端口）",
            "attach needs port (the JDWP port of the target JVM)"
        ))?;
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
        let settings = serde_json::from_value(args.clone())
            .context(trs!("attach 参数格式不对", "Malformed attach arguments"))?;
        Ok(AttachArgs {
            host_name,
            port,
            project_root: path("projectRoot").or_else(|| path("cwd")),
            source_paths,
            timeout,
            settings,
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
/// `settings` 是客户端传来的 JDK 和 Maven 设置，`java-analyzer.toml` 里写了的优先。
pub fn load_project(
    root: &Path,
    full: bool,
    settings: &ProjectSettings,
) -> project_model::LoadedProject {
    // 编辑器里没填的设置可能以空字符串传过来
    let given = |path: &Option<PathBuf>| path.clone().filter(|p| !p.as_os_str().is_empty());
    project_model::set_maven_setup(project_model::MavenSetup {
        settings: given(&settings.maven_settings),
        local_repository: given(&settings.maven_local_repository),
    });
    let options = project_model::LoadOptions {
        jdk_home: given(&settings.jdk_home),
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
            trs!(
                "工程里没有找到编译输出目录（target/classes、build/classes/java/main 等）。\
                 调试器不负责构建，请先编译工程（例如 `mvn compile` 或 `gradle classes`），或者在 launch 参数里给出 classPaths",
                "No compiled output directory (target/classes, build/classes/java/main, ...) was found in the project. \
                 The debugger does not build the project: compile it first (for example with `mvn compile` or `gradle classes`), \
                 or give classPaths in the launch arguments"
            )
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
        tr!(
            "模块 {} 的编译输出目录不存在（{}）。调试器不负责构建，请先编译工程",
            "The compiled output directory of module {} does not exist ({}). \
             The debugger does not build the project: compile it first",
            graph.module(start).name,
            class_path
                .output_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    );
    Ok(class_path.entries)
}

/// 启动 JVM 时给 `file.encoding` 的值：主类所在模块的源文件编码。找不到主类所在的模块时，
/// 各模块设置的编码一致就用它。工程没有设置编码时为 `None`。
pub fn project_encoding(graph: &ProjectGraph, main_class: &str) -> Option<String> {
    if let Some((module, _)) = module_of_main_class(graph, main_class) {
        if let Some(encoding) = &graph.module(module).encoding {
            return Some(encoding.clone());
        }
    }
    let mut encodings = graph.modules.iter().filter_map(|m| m.encoding.as_deref());
    let first = encodings.next()?;
    encodings
        .all(|e| e.eq_ignore_ascii_case(first))
        .then(|| first.to_owned())
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
            .with_context(|| tr!("无法启动 {program}", "Cannot start {program}"))?;
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
            Err(e) => {
                return Err(e).context(trs!(
                    "等待调试目标连接失败",
                    "Failed while waiting for the debuggee to connect"
                ))
            }
        }
        if let Some(code) = debuggee.try_exit_code() {
            anyhow::bail!(tr!(
                "JVM 在连上调试器之前就退出了（退出码 {code}），详见输出",
                "The JVM exited before connecting to the debugger (exit code {code}); see the output"
            ));
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            tr!(
                "JVM 在 {timeout:?} 内没有连上调试器",
                "The JVM did not connect to the debugger within {timeout:?}"
            )
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 拼出启动 JVM 的命令。`address` 是调试器监听的 `127.0.0.1:端口`。
/// `encoding` 是工程的源文件编码：`vmArgs` 里没有 `-Dfile.encoding=` 时加上它，
/// 否则 JVM 用平台默认编码（例如中文 Windows 上 JDK 17 及以前的 GBK）。
pub fn build_command(
    java: &Path,
    args: &LaunchArgs,
    main_class: &str,
    class_path: &[String],
    address: &str,
    cwd: &Path,
    encoding: Option<&str>,
) -> anyhow::Result<Command> {
    let mut command = Command::new(java);
    command.current_dir(cwd);
    for (key, value) in &args.env {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let vm_args = args
        .vm_args
        .as_ref()
        .map(StringOrList::to_vec)
        .unwrap_or_default();
    if let Some(encoding) = encoding {
        if !vm_args.iter().any(|a| a.starts_with("-Dfile.encoding=")) {
            command.arg(format!("-Dfile.encoding={encoding}"));
        }
    }
    command.args(vm_args);
    command.arg(format!(
        "-agentlib:jdwp=transport=dt_socket,server=n,suspend=y,address={address}"
    ));
    let module_path: Vec<&String> = args.module_paths.iter().flatten().collect();
    if !module_path.is_empty() {
        let joined = std::env::join_paths(module_path.iter().map(|p| p.as_str())).context(trs!(
            "modulePaths 里有不能出现在路径列表里的字符",
            "modulePaths contains a character that is not allowed in a path list"
        ))?;
        command.arg("--module-path").arg(joined);
    }
    if !class_path.is_empty() {
        let joined = std::env::join_paths(class_path.iter()).context(trs!(
            "classPaths 里有不能出现在路径列表里的字符",
            "classPaths contains a character that is not allowed in a path list"
        ))?;
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
        assert_eq!(attach.settings, ProjectSettings::default());
    }

    #[test]
    fn parses_project_settings() {
        let args: LaunchArgs = serde_json::from_value(serde_json::json!({
            "type": "java-analyzer", "request": "launch", "name": "x",
            "mainClass": "demo.Main", "vmArgs": "-ea",
            "jdkHome": "/opt/jdk17", "mavenSettings": "/opt/m2/settings.xml",
            "mavenLocalRepository": null,
        }))
        .unwrap();
        assert_eq!(args.main_class.as_deref(), Some("demo.Main"));
        assert_eq!(args.vm_args.unwrap().to_vec(), ["-ea"]);
        assert_eq!(
            args.settings,
            ProjectSettings {
                jdk_home: Some(PathBuf::from("/opt/jdk17")),
                maven_settings: Some(PathBuf::from("/opt/m2/settings.xml")),
                maven_local_repository: None,
            }
        );

        let attach = AttachArgs::parse(&serde_json::json!({
            "port": 5005, "mavenLocalRepository": "/data/m2",
        }))
        .unwrap();
        assert_eq!(
            attach.settings.maven_local_repository,
            Some(PathBuf::from("/data/m2"))
        );
        assert!(attach.settings.jdk_home.is_none());
    }

    /// 假的 JDK：`release` 文件、空的 `lib/modules` 和 `bin/java`。
    fn fake_jdk(home: &Path, version: &str) {
        std::fs::create_dir_all(home.join("lib")).unwrap();
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(
            home.join("release"),
            format!("JAVA_VERSION=\"{version}\"\n"),
        )
        .unwrap();
        std::fs::write(home.join("lib/modules"), b"").unwrap();
        let exe = if cfg!(windows) { "java.exe" } else { "java" };
        std::fs::write(home.join("bin").join(exe), b"").unwrap();
    }

    #[test]
    fn jdk_home_attribute_selects_the_jdk() {
        let dir = std::env::temp_dir().join(format!("dap-jdk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let project = dir.join("project");
        std::fs::create_dir_all(project.join("src/demo")).unwrap();
        std::fs::write(project.join("src/demo/Main.java"), b"package demo;").unwrap();
        let config = "[[module]]\nname = \"app\"\nsources = [\"src\"]\n";
        std::fs::write(project.join("java-analyzer.toml"), config).unwrap();
        let attribute_jdk = dir.join("jdk-attr");
        fake_jdk(&attribute_jdk, "11.0.2");

        let args: LaunchArgs = serde_json::from_value(serde_json::json!({
            "mainClass": "demo.Main",
            "jdkHome": attribute_jdk,
        }))
        .unwrap();
        let graph = load_project(&project, false, &args.settings).graph;
        let jdk = graph.jdk.as_ref().unwrap();
        assert_eq!(jdk.home, attribute_jdk);
        assert_eq!(jdk.version, 11);
        // 启动调试目标用的就是这个 JDK 的 java
        let exe = if cfg!(windows) { "java.exe" } else { "java" };
        assert_eq!(
            java_executable(args.java_exec.as_deref(), Some(&jdk.home)),
            attribute_jdk.join("bin").join(exe)
        );

        // `java-analyzer.toml` 里的 `[jdk]` 优先
        let toml_jdk = dir.join("jdk-toml");
        fake_jdk(&toml_jdk, "17.0.1");
        let config = format!("[jdk]\nhome = {:?}\n\n{config}", toml_jdk.to_string_lossy());
        std::fs::write(project.join("java-analyzer.toml"), config).unwrap();
        let graph = load_project(&project, false, &args.settings).graph;
        assert_eq!(graph.jdk.unwrap().home, toml_jdk);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_encoding_from_project() {
        let args = |vm_args: Option<&str>| LaunchArgs {
            vm_args: vm_args.map(|a| StringOrList::String(a.to_owned())),
            ..LaunchArgs::default()
        };
        let command_line = |args: &LaunchArgs, encoding: Option<&str>| -> Vec<String> {
            build_command(
                Path::new("java"),
                args,
                "demo.Main",
                &[],
                "127.0.0.1:1",
                Path::new("."),
                encoding,
            )
            .unwrap()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
        };
        let encodings = |line: &[String]| -> Vec<String> {
            line.iter()
                .filter(|a| a.starts_with("-Dfile.encoding="))
                .cloned()
                .collect()
        };

        let line = command_line(&args(Some("-Xmx64m")), Some("UTF-8"));
        assert_eq!(encodings(&line), ["-Dfile.encoding=UTF-8"]);
        assert!(line.contains(&"-Xmx64m".to_owned()));
        // 用户自己写了就不加
        let line = command_line(&args(Some("-Dfile.encoding=GBK -ea")), Some("UTF-8"));
        assert_eq!(encodings(&line), ["-Dfile.encoding=GBK"]);
        // 工程没有设置编码
        assert!(encodings(&command_line(&args(None), None)).is_empty());
        assert_eq!(
            encodings(&command_line(&args(None), Some("UTF-8"))),
            ["-Dfile.encoding=UTF-8"]
        );

        // 编码取主类所在模块的；找不到模块时各模块一致才用
        let dir = std::env::temp_dir().join(format!("dap-enc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("b/src/demo")).unwrap();
        std::fs::write(dir.join("b/src/demo/Main.java"), b"").unwrap();
        let module = |name: &str, encoding: Option<&str>| ModuleData {
            name: name.into(),
            root: dir.join(name),
            source_roots: vec![SourceRoot {
                path: dir.join(name).join("src"),
                kind: SourceRootKind::Main,
            }],
            language_level: None,
            api_release: None,
            test_language_level: None,
            preview: false,
            dependencies: Vec::new(),
            output_dirs: Vec::new(),
            encoding: encoding.map(str::to_owned),
        };
        let graph = |a: Option<&str>, b: Option<&str>| ProjectGraph {
            modules: vec![module("a", a), module("b", b)],
            libraries: Vec::new(),
            jdk: None,
        };
        let mixed = graph(Some("UTF-8"), Some("GBK"));
        assert_eq!(
            project_encoding(&mixed, "demo.Main").as_deref(),
            Some("GBK")
        );
        assert_eq!(project_encoding(&mixed, "demo.Other"), None);
        let same = graph(Some("UTF-8"), None);
        assert_eq!(
            project_encoding(&same, "demo.Other").as_deref(),
            Some("UTF-8")
        );
        assert_eq!(project_encoding(&graph(None, None), "demo.Main"), None);
        std::fs::remove_dir_all(&dir).ok();
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
                    test_language_level: None,
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
                    encoding: None,
                },
                ModuleData {
                    name: "core".into(),
                    root: dir.join("core"),
                    source_roots: Vec::new(),
                    language_level: None,
                    api_release: None,
                    test_language_level: None,
                    preview: false,
                    dependencies: vec![Dependency {
                        target: DependencyTarget::Library(LibraryId(0)),
                        scope: Scope::Compile,
                    }],
                    output_dirs: outputs(
                        core_classes.clone(),
                        dir.join("core/target/test-classes"),
                    ),
                    encoding: None,
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
