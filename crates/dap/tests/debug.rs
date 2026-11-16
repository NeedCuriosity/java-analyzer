//! 端到端测试：用 javac 编译小程序，在进程内跑调试适配器，按 DAP 请求驱动一次完整的调试。
//! 找不到带 javac 的 JDK 时跳过。

use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::JoinHandle,
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(60);

fn jdk_home() -> Option<PathBuf> {
    let has_javac = |home: &Path| home.join("bin").join("javac").is_file();
    std::env::var_os("JAVA_HOME")
        .map(PathBuf::from)
        .filter(|home| has_javac(home))
        .or_else(|| {
            project_model::discover_jdk(None)
                .map(|jdk| jdk.home)
                .filter(|home| has_javac(home))
        })
}

macro_rules! require_jdk {
    () => {
        match jdk_home() {
            Some(home) => home,
            None => {
                eprintln!("没有找到带 javac 的 JDK，跳过");
                return;
            }
        }
    };
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ja-dap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn javac(jdk: &Path, out: &Path, sources: &[&Path]) {
    let status = Command::new(jdk.join("bin").join("javac"))
        .arg("-g")
        .arg("-d")
        .arg(out)
        .args(sources)
        .status()
        .expect("无法运行 javac");
    assert!(status.success(), "javac 失败");
}

fn java(jdk: &Path) -> String {
    jdk.join("bin").join("java").to_string_lossy().into_owned()
}

/// 进程内的调试适配器加一个简单的 DAP 客户端。
struct Client {
    tx: Option<Sender<Value>>,
    rx: Receiver<Value>,
    seq: i64,
    backlog: Vec<Value>,
    output: String,
    thread: Option<JoinHandle<()>>,
}

impl Client {
    fn start() -> Client {
        let (tx, requests) = crossbeam_channel::unbounded();
        let (out_tx, rx) = crossbeam_channel::unbounded();
        let thread = std::thread::spawn(move || {
            dap::serve(requests, move |message: &Value| {
                let _ = out_tx.send(message.clone());
            })
        });
        let mut client = Client {
            tx: Some(tx),
            rx,
            seq: 0,
            backlog: Vec::new(),
            output: String::new(),
            thread: Some(thread),
        };
        let caps = client.request(
            "initialize",
            json!({"adapterID": "java", "linesStartAt1": true, "columnsStartAt1": true}),
        );
        assert_eq!(caps["supportsConfigurationDoneRequest"], json!(true));
        client
    }

    fn next(&mut self) -> Value {
        let message = self.rx.recv_timeout(TIMEOUT).unwrap_or_else(|_| {
            panic!(
                "等待调试适配器的消息超时。已收到但未处理的消息: {:#?}\n输出: {}",
                self.backlog, self.output
            )
        });
        if message["event"] == "output" {
            self.output
                .push_str(message["body"]["output"].as_str().unwrap_or_default());
        }
        message
    }

    fn send(&mut self, command: &str, arguments: Value) -> i64 {
        self.seq += 1;
        let message =
            json!({"seq": self.seq, "type": "request", "command": command, "arguments": arguments});
        self.tx.as_ref().unwrap().send(message).unwrap();
        self.seq
    }

    fn response(&mut self, command: &str, arguments: Value) -> Value {
        let seq = self.send(command, arguments);
        loop {
            let message = self.next();
            if message["type"] == "response" && message["request_seq"] == json!(seq) {
                return message;
            }
            self.backlog.push(message);
        }
    }

    fn request(&mut self, command: &str, arguments: Value) -> Value {
        let response = self.response(command, arguments);
        assert_eq!(
            response["success"],
            json!(true),
            "{command} 失败: {response}\n输出: {}",
            self.output
        );
        response.get("body").cloned().unwrap_or(Value::Null)
    }

    fn wait_event(&mut self, event: &str) -> Value {
        if let Some(i) = self.backlog.iter().position(|m| m["event"] == event) {
            return self.backlog.remove(i)["body"].clone();
        }
        loop {
            let message = self.next();
            if message["event"] == event {
                return message["body"].clone();
            }
            self.backlog.push(message);
        }
    }

    fn wait_stopped(&mut self, reason: &str) -> i64 {
        let body = self.wait_event("stopped");
        assert_eq!(
            body["reason"],
            json!(reason),
            "{body}\n输出: {}",
            self.output
        );
        body["threadId"].as_i64().unwrap()
    }

    /// 栈顶帧：(帧 ID, 名字, 行, 源文件路径)
    fn top_frame(&mut self, thread: i64) -> (i64, String, i64, String) {
        let body = self.request("stackTrace", json!({"threadId": thread}));
        let frame = &body["stackFrames"][0];
        (
            frame["id"].as_i64().unwrap(),
            frame["name"].as_str().unwrap().to_owned(),
            frame["line"].as_i64().unwrap(),
            frame["source"]["path"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    }

    fn variables(&mut self, reference: i64) -> Vec<(String, String, i64)> {
        let body = self.request("variables", json!({"variablesReference": reference}));
        body["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["name"].as_str().unwrap().to_owned(),
                    v["value"].as_str().unwrap().to_owned(),
                    v["variablesReference"].as_i64().unwrap(),
                )
            })
            .collect()
    }

    fn locals(&mut self, frame: i64) -> Vec<(String, String, i64)> {
        let scopes = self.request("scopes", json!({"frameId": frame}));
        assert_eq!(scopes["scopes"][0]["name"], json!("Locals"));
        let reference = scopes["scopes"][0]["variablesReference"].as_i64().unwrap();
        self.variables(reference)
    }

    fn evaluate(&mut self, frame: i64, expression: &str) -> String {
        let body = self.request(
            "evaluate",
            json!({"expression": expression, "frameId": frame, "context": "watch"}),
        );
        body["result"].as_str().unwrap().to_owned()
    }

    fn finish(mut self) {
        let _ = self.response("disconnect", json!({}));
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn value_of<'a>(vars: &'a [(String, String, i64)], name: &str) -> &'a (String, String, i64) {
    vars.iter()
        .find(|(n, _, _)| n == name)
        .unwrap_or_else(|| panic!("没有变量 {name}: {vars:?}"))
}

const MAIN_JAVA: &str = r#"package demo;

public class Main {
    private int counter = 7;
    private String label = "hello";

    public static void main(String[] args) {
        Main m = new Main();
        int total = 0;
        for (int i = 0; i < 3; i++) {
            total += m.step(i);
        }
        System.out.println("total=" + total);
        m.fail();
    }

    int step(int i) {
        String text = "value" + i;
        int doubled = i * 2;
        return doubled + counter + text.length();
    }

    void fail() {
        throw new IllegalStateException("boom");
    }
}
"#;

#[test]
fn launch_breakpoints_stepping_variables_and_exceptions() {
    let jdk = require_jdk!();
    let dir = temp_dir("launch");
    let source = dir.join("src/demo/Main.java");
    write(&source, MAIN_JAVA);
    let out = dir.join("out");
    javac(&jdk, &out, &[&source]);

    let mut client = Client::start();
    client.request(
        "launch",
        json!({
            "mainClass": "demo.Main",
            "projectRoot": dir,
            "classPaths": [out],
            "javaExec": java(&jdk),
        }),
    );
    client.wait_event("initialized");
    let set = client.request(
        "setBreakpoints",
        json!({"source": {"path": source}, "breakpoints": [{"line": 11}]}),
    );
    // 类还没有加载：先报告未生效
    assert_eq!(set["breakpoints"][0]["verified"], json!(false));
    let bp_id = set["breakpoints"][0]["id"].as_i64().unwrap();
    client.request("setExceptionBreakpoints", json!({"filters": ["uncaught"]}));
    client.request("configurationDone", json!({}));

    // 类加载后断点生效
    let changed = client.wait_event("breakpoint");
    assert_eq!(changed["breakpoint"]["id"], json!(bp_id));
    assert_eq!(changed["breakpoint"]["verified"], json!(true));

    let thread = client.wait_stopped("breakpoint");
    let threads = client.request("threads", json!({}));
    assert!(threads["threads"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == json!(thread) && t["name"] == "main"));
    let (frame, name, line, path) = client.top_frame(thread);
    assert_eq!((name.as_str(), line), ("Main.main", 11));
    assert!(path.ends_with("demo/Main.java"), "{path}");

    let locals = client.locals(frame);
    let names: Vec<&str> = locals.iter().map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(names, ["args", "m", "total", "i"]);
    assert_eq!(value_of(&locals, "total").1, "0");
    assert_eq!(value_of(&locals, "i").1, "0");
    assert!(
        value_of(&locals, "args").1.starts_with("String[0]"),
        "{locals:?}"
    );
    let m = value_of(&locals, "m");
    assert!(m.1.starts_with("Main@"), "{m:?}");
    let fields = client.variables(m.2);
    assert_eq!(value_of(&fields, "counter").1, "7");
    assert_eq!(value_of(&fields, "label").1, "\"hello\"");

    assert_eq!(client.evaluate(frame, "m.counter"), "7");
    assert_eq!(client.evaluate(frame, "m.label"), "\"hello\"");
    assert_eq!(client.evaluate(frame, "args.length"), "0");
    assert_eq!(
        client.evaluate(frame, "i == 0 && m.label == \"hello\""),
        "true"
    );
    let error = client.response(
        "evaluate",
        json!({"expression": "nothing", "frameId": frame}),
    );
    assert_eq!(error["success"], json!(false));

    // 修改变量
    let set_var = client.request(
        "setVariable",
        json!({"variablesReference": m.2, "name": "counter", "value": "8"}),
    );
    assert_eq!(set_var["value"], json!("8"));
    assert_eq!(client.evaluate(frame, "m.counter"), "8");
    client.request(
        "setVariable",
        json!({"variablesReference": m.2, "name": "counter", "value": "7"}),
    );

    // 步入 step()
    client.request("stepIn", json!({"threadId": thread}));
    let thread = client.wait_stopped("step");
    let (_, name, line, _) = client.top_frame(thread);
    assert_eq!((name.as_str(), line), ("Main.step", 18));

    // 单步跳过
    client.request("next", json!({"threadId": thread}));
    let thread = client.wait_stopped("step");
    let (frame, name, line, _) = client.top_frame(thread);
    assert_eq!((name.as_str(), line), ("Main.step", 19));
    let locals = client.locals(frame);
    assert_eq!(value_of(&locals, "text").1, "\"value0\"");
    assert_eq!(value_of(&locals, "i").1, "0");
    let this = value_of(&locals, "this").2;
    let fields = client.variables(this);
    assert_eq!(value_of(&fields, "counter").1, "7");
    assert_eq!(client.evaluate(frame, "counter"), "7");
    assert_eq!(client.evaluate(frame, "this.label"), "\"hello\"");

    // 步出回到 main
    client.request("stepOut", json!({"threadId": thread}));
    let thread = client.wait_stopped("step");
    let (_, name, line, _) = client.top_frame(thread);
    assert_eq!((name.as_str(), line), ("Main.main", 11));

    // 条件断点：只在 i == 2 时停
    let set = client.request(
        "setBreakpoints",
        json!({"source": {"path": source}, "breakpoints": [{"line": 19, "condition": "i == 2"}]}),
    );
    assert_eq!(set["breakpoints"][0]["verified"], json!(true));
    client.request("continue", json!({"threadId": thread}));
    let thread = client.wait_stopped("breakpoint");
    let (frame, _, line, _) = client.top_frame(thread);
    assert_eq!(line, 19);
    assert_eq!(client.evaluate(frame, "i"), "2");

    // 去掉断点，停在未捕获的异常上
    client.request(
        "setBreakpoints",
        json!({"source": {"path": source}, "breakpoints": []}),
    );
    client.request("continue", json!({"threadId": thread}));
    let stopped = client.wait_event("stopped");
    assert_eq!(stopped["reason"], json!("exception"), "{stopped}");
    assert!(
        stopped["text"].as_str().unwrap().contains("boom"),
        "{stopped}"
    );
    let thread = stopped["threadId"].as_i64().unwrap();
    let info = client.request("exceptionInfo", json!({"threadId": thread}));
    assert_eq!(
        info["exceptionId"],
        json!("java.lang.IllegalStateException")
    );
    assert_eq!(info["breakMode"], json!("unhandled"));
    let (_, name, line, _) = client.top_frame(thread);
    assert_eq!((name.as_str(), line), ("Main.fail", 24));

    // 运行到结束
    client.request("continue", json!({"threadId": thread}));
    let exited = client.wait_event("exited");
    assert_eq!(exited["exitCode"], json!(1));
    client.wait_event("terminated");
    assert!(client.output.contains("total=45"), "{}", client.output);
    assert!(
        client.output.contains("IllegalStateException"),
        "{}",
        client.output
    );
    client.finish();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn launch_maven_project_without_class_paths() {
    let jdk = require_jdk!();
    let dir = temp_dir("maven");
    write(
        &dir.join("pom.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>demo</groupId>
  <artifactId>app</artifactId>
  <version>1.0</version>
</project>
"#,
    );
    let source = dir.join("src/main/java/demo/App.java");
    write(
        &source,
        r#"package demo;

public class App {
    public static void main(String[] args) {
        String greeting = "hi from maven";
        System.out.println(greeting);
    }
}
"#,
    );
    javac(&jdk, &dir.join("target/classes"), &[&source]);

    let mut client = Client::start();
    client.request(
        "launch",
        json!({"mainClass": "demo.App", "projectRoot": dir, "javaExec": java(&jdk)}),
    );
    client.wait_event("initialized");
    client.request(
        "setBreakpoints",
        json!({"source": {"path": source}, "breakpoints": [{"line": 6}]}),
    );
    client.request("configurationDone", json!({}));
    let thread = client.wait_stopped("breakpoint");
    let (frame, name, line, path) = client.top_frame(thread);
    assert_eq!((name.as_str(), line), ("App.main", 6));
    assert!(path.ends_with("src/main/java/demo/App.java"), "{path}");
    let locals = client.locals(frame);
    assert_eq!(value_of(&locals, "greeting").1, "\"hi from maven\"");
    client.request("continue", json!({"threadId": thread}));
    let exited = client.wait_event("exited");
    assert_eq!(exited["exitCode"], json!(0));
    client.wait_event("terminated");
    assert!(client.output.contains("hi from maven"), "{}", client.output);
    client.finish();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn launch_without_compiled_output_reports_an_error() {
    let _jdk = require_jdk!();
    let dir = temp_dir("uncompiled");
    write(
        &dir.join("pom.xml"),
        "<project><modelVersion>4.0.0</modelVersion><groupId>d</groupId><artifactId>x</artifactId><version>1</version></project>",
    );
    write(
        &dir.join("src/main/java/demo/X.java"),
        "package demo; class X { public static void main(String[] a) {} }",
    );
    let mut client = Client::start();
    let response = client.response("launch", json!({"mainClass": "demo.X", "projectRoot": dir}));
    assert_eq!(response["success"], json!(false));
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("target/classes"),
        "{response}"
    );
    client.finish();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn launch_missing_main_class_exits() {
    let jdk = require_jdk!();
    let dir = temp_dir("missing");
    let mut client = Client::start();
    client.request(
        "launch",
        json!({"mainClass": "demo.Missing", "projectRoot": dir, "classPaths": [dir], "javaExec": java(&jdk)}),
    );
    client.wait_event("initialized");
    client.request("configurationDone", json!({}));
    let exited = client.wait_event("exited");
    assert_eq!(exited["exitCode"], json!(1));
    client.wait_event("terminated");
    assert!(client.output.contains("demo.Missing"), "{}", client.output);
    client.finish();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn attach_to_running_jvm() {
    let jdk = require_jdk!();
    let dir = temp_dir("attach");
    let source = dir.join("src/demo/Loop.java");
    write(
        &source,
        r#"package demo;

public class Loop {
    public static void main(String[] args) throws Exception {
        for (int n = 0; n < 2; n++) {
            Thread.sleep(10);
            System.out.println("tick " + n);
        }
    }
}
"#,
    );
    let out = dir.join("out");
    javac(&jdk, &out, &[&source]);
    let mut child = Command::new(jdk.join("bin/java"))
        .arg("-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=127.0.0.1:0")
        .arg("-cp")
        .arg(&out)
        .arg("demo.Loop")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // JVM 先打印 `Listening for transport dt_socket at address: <端口>`
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let port: u16 = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or_else(|| panic!("无法从 `{line}` 读出端口"));

    let mut client = Client::start();
    client.request(
        "attach",
        json!({"hostName": "127.0.0.1", "port": port, "projectRoot": dir}),
    );
    client.wait_event("initialized");
    client.request(
        "setBreakpoints",
        json!({"source": {"path": source}, "breakpoints": [{"line": 7, "hitCondition": "2"}]}),
    );
    client.request("configurationDone", json!({}));
    let thread = client.wait_stopped("breakpoint");
    let (frame, name, line, path) = client.top_frame(thread);
    assert_eq!((name.as_str(), line), ("Loop.main", 7));
    assert!(path.ends_with("demo/Loop.java"), "{path}");
    // 命中次数条件：第 2 次才停
    assert_eq!(client.evaluate(frame, "n"), "1");
    client.request("continue", json!({"threadId": thread}));
    client.wait_event("terminated");
    let status = child.wait().unwrap();
    assert!(status.success());
    // 读完剩下的输出，确认程序跑完了
    let rest: Vec<String> = stdout.lines().map_while(Result::ok).collect();
    assert!(rest.iter().any(|l| l == "tick 1"), "{rest:?}");
    client.finish();
    std::fs::remove_dir_all(&dir).ok();
}
