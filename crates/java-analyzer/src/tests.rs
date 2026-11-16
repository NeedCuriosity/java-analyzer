//! 端到端测试：在进程内启动服务器，经内存通道走完整的
//! initialize → didOpen → 各请求 → shutdown → exit 流程。
//!
//! 这些测试不依赖 JDK：只用到工作区自己的类型。

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
    thread::JoinHandle,
    time::Duration,
};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{notification::Notification as _, request::Request as _, Url};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(60);

/// 每个测试一个独立的临时工程目录，结束时删除。
struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn new(files: &[(&str, &str)]) -> TempProject {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "java-analyzer-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        // 临时目录可能是符号链接（macOS 的 /var → /private/var）：统一成规范路径
        TempProject {
            root: root.canonicalize().unwrap(),
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn url(&self, relative: &str) -> Url {
        Url::from_file_path(self.path(relative)).unwrap()
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Client {
    connection: Connection,
    server: Option<JoinHandle<anyhow::Result<()>>>,
    next_id: i32,
    /// 等待响应期间收到的通知
    notifications: Vec<Notification>,
}

impl Client {
    fn start(root: &Path, capabilities: Value) -> Client {
        Client::start_with_options(root, capabilities, Value::Null)
    }

    fn start_with_options(root: &Path, capabilities: Value, options: Value) -> Client {
        Client::start_with_locale(root, capabilities, options, None)
    }

    /// `locale`：`initialize` 里客户端声明的界面语言。
    fn start_with_locale(
        root: &Path,
        capabilities: Value,
        options: Value,
        locale: Option<&str>,
    ) -> Client {
        let (server_side, client_side) = Connection::memory();
        let server = std::thread::spawn(move || crate::serve(server_side));
        let mut client = Client {
            connection: client_side,
            server: Some(server),
            next_id: 0,
            notifications: Vec::new(),
        };
        let result = client.request_raw(
            "initialize",
            json!({
                "processId": null,
                "rootUri": Url::from_file_path(root).unwrap(),
                "capabilities": capabilities,
                "initializationOptions": options,
                "locale": locale,
            }),
        );
        assert_eq!(result["serverInfo"]["name"], "java-analyzer");
        assert!(result["capabilities"]["completionProvider"].is_object());
        assert_eq!(result["capabilities"]["typeHierarchyProvider"], true);
        client.notify_raw("initialized", json!({}));
        client
    }

    fn notify_raw(&self, method: &str, params: Value) {
        self.connection
            .sender
            .send(Message::Notification(Notification {
                method: method.to_owned(),
                params,
            }))
            .unwrap();
    }

    fn send_request(&mut self, method: &str, params: Value) -> RequestId {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.connection
            .sender
            .send(Message::Request(Request {
                id: id.clone(),
                method: method.to_owned(),
                params,
            }))
            .unwrap();
        id
    }

    /// 等到指定请求的响应。期间服务器发来的请求一律回复成功，通知先存起来。
    fn wait_response(&mut self, id: &RequestId) -> Response {
        loop {
            let message = self
                .connection
                .receiver
                .recv_timeout(TIMEOUT)
                .expect("等待服务器响应超时");
            match message {
                Message::Response(response) if response.id == *id => return response,
                Message::Response(_) => {}
                Message::Request(request) => {
                    self.connection
                        .sender
                        .send(Message::Response(Response::new_ok(request.id, Value::Null)))
                        .unwrap();
                }
                Message::Notification(notification) => self.notifications.push(notification),
            }
        }
    }

    /// 发请求并取结果。服务器因并发的编辑回复 `ContentModified` 时按协议重发。
    fn request_raw(&mut self, method: &str, params: Value) -> Value {
        for _ in 0..20 {
            let id = self.send_request(method, params.clone());
            let response = self.wait_response(&id);
            match response.error {
                None => return response.result.unwrap_or(Value::Null),
                Some(err) if err.code == lsp_server::ErrorCode::ContentModified as i32 => continue,
                Some(err) => panic!("{method} 失败: {err:?}"),
            }
        }
        panic!("{method} 反复被取消");
    }

    fn open(&self, url: &Url, text: &str) {
        self.notify_raw(
            lsp_types::notification::DidOpenTextDocument::METHOD,
            json!({ "textDocument": { "uri": url, "languageId": "java", "version": 1, "text": text } }),
        );
    }

    /// 等到一条满足条件的通知（先看已经存下来的）。
    fn wait_notification(&mut self, method: &str, accept: impl Fn(&Value) -> bool) -> Value {
        if let Some(idx) = self
            .notifications
            .iter()
            .position(|n| n.method == method && accept(&n.params))
        {
            return self.notifications.remove(idx).params;
        }
        loop {
            let message = self
                .connection
                .receiver
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|_| panic!("等待通知 {method} 超时"));
            match message {
                Message::Notification(n) if n.method == method && accept(&n.params) => {
                    return n.params;
                }
                Message::Notification(n) => self.notifications.push(n),
                Message::Request(request) => {
                    self.connection
                        .sender
                        .send(Message::Response(Response::new_ok(request.id, Value::Null)))
                        .unwrap();
                }
                Message::Response(_) => {}
            }
        }
    }

    /// 等到服务器发来指定方法的请求（回复成功），返回它的参数。
    fn wait_server_request(&mut self, method: &str) -> Value {
        loop {
            let message = self
                .connection
                .receiver
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|_| panic!("等待服务器请求 {method} 超时"));
            match message {
                Message::Request(request) => {
                    self.connection
                        .sender
                        .send(Message::Response(Response::new_ok(
                            request.id,
                            json!({ "applied": true }),
                        )))
                        .unwrap();
                    if request.method == method {
                        return request.params;
                    }
                }
                Message::Notification(n) => self.notifications.push(n),
                Message::Response(_) => {}
            }
        }
    }

    fn shutdown(mut self) {
        let result = self.request_raw(lsp_types::request::Shutdown::METHOD, Value::Null);
        assert!(result.is_null());
        self.notify_raw(lsp_types::notification::Exit::METHOD, Value::Null);
        let server = self.server.take().unwrap();
        server
            .join()
            .expect("服务器线程 panic")
            .expect("服务器以错误退出");
    }
}

/// `text` 里 `needle` 第 `nth` 次出现处的 LSP 位置（再向右偏移 `delta` 列）。
fn position_of(text: &str, needle: &str, nth: usize, delta: u32) -> Value {
    let (offset, _) = text.match_indices(needle).nth(nth).expect("找不到标记");
    let line = text[..offset].matches('\n').count();
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let character = text[line_start..offset].encode_utf16().count() as u32 + delta;
    json!({ "line": line, "character": character })
}

const MANUAL_CONFIG: &str = "[[module]]\nname = \"app\"\nsources = [\"src\"]\n";

const GREETER: &str = "package app;\n\npublic class Greeter {\n    private final String name;\n\n    public Greeter(String name) {\n        this.name = name;\n    }\n\n    /** Says hello. */\n    public String greet() {\n        return name;\n    }\n}\n";

const MAIN: &str = "package app;\n\npublic class Main {\n    void run() {\n        Greeter greeter = new Greeter(\"x\");\n        greeter.greet();\n    }\n}\n";

fn manual_project() -> TempProject {
    TempProject::new(&[
        ("java-analyzer.toml", MANUAL_CONFIG),
        ("src/app/Greeter.java", GREETER),
        ("src/app/Main.java", MAIN),
    ])
}

/// 工程导入是异步的：反复请求直到另一个文件里的类能被找到。
fn wait_until_loaded(client: &mut Client, project: &TempProject) {
    let params = json!({
        "textDocument": { "uri": project.url("src/app/Main.java") },
        "position": position_of(MAIN, "Greeter greeter", 0, 1),
    });
    for _ in 0..600 {
        let result = client.request_raw("textDocument/definition", params.clone());
        if result
            .as_array()
            .is_some_and(|locations| !locations.is_empty())
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("工程迟迟没有导入完成");
}

#[test]
fn full_session_with_manual_project() {
    let project = manual_project();
    let mut client = Client::start(&project.root, json!({}));
    let main_url = project.url("src/app/Main.java");
    client.open(&main_url, MAIN);
    wait_until_loaded(&mut client, &project);

    // 跳转到另一个文件里的方法
    let result = client.request_raw(
        "textDocument/definition",
        json!({
            "textDocument": { "uri": main_url },
            "position": position_of(MAIN, "greet()", 0, 1),
        }),
    );
    let location = &result.as_array().unwrap()[0];
    assert_eq!(location["uri"], json!(project.url("src/app/Greeter.java")));
    assert_eq!(
        location["range"]["start"],
        position_of(GREETER, "greet()", 0, 0)
    );

    // 悬停：签名 + 文档
    let hover = client.request_raw(
        "textDocument/hover",
        json!({
            "textDocument": { "uri": main_url },
            "position": position_of(MAIN, "greet()", 0, 1),
        }),
    );
    let markdown = hover["contents"]["value"].as_str().unwrap();
    assert!(markdown.contains("greet()"), "{markdown}");
    assert!(markdown.contains("Says hello."), "{markdown}");

    // 成员补全
    let completion = client.request_raw(
        "textDocument/completion",
        json!({
            "textDocument": { "uri": main_url },
            "position": position_of(MAIN, "greeter.greet", 0, 8),
        }),
    );
    let labels: Vec<&str> = completion["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"greet"), "{labels:?}");
    assert!(
        !labels.contains(&"name"),
        "私有字段在别的类里不可见: {labels:?}"
    );

    // 查找引用（含声明）跨文件
    let references = client.request_raw(
        "textDocument/references",
        json!({
            "textDocument": { "uri": main_url },
            "position": position_of(MAIN, "greet()", 0, 1),
            "context": { "includeDeclaration": true },
        }),
    );
    assert_eq!(references.as_array().unwrap().len(), 2);

    // 文档符号（客户端未声明层级支持时是扁平列表）
    let symbols = client.request_raw(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": main_url } }),
    );
    let names: Vec<&str> = symbols
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert_eq!(names, ["Main", "run"]);

    // 工作区符号
    let symbols = client.request_raw("workspace/symbol", json!({ "query": "Greet" }));
    assert!(symbols
        .as_array()
        .unwrap()
        .iter()
        .any(|symbol| symbol["name"] == "Greeter"));

    // 重命名方法：声明和另一个文件里的调用一起改
    let edit = client.request_raw(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": main_url },
            "position": position_of(MAIN, "greet()", 0, 1),
            "newName": "hello",
        }),
    );
    let changes = edit["changes"].as_object().expect("应当返回 changes");
    assert_eq!(changes.len(), 2, "{edit}");
    assert!(changes.values().all(|edits| edits[0]["newText"] == "hello"));

    // 名字不合法时以错误回复，消息给用户看
    let id = client.send_request(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": main_url },
            "position": position_of(MAIN, "greet()", 0, 1),
            "newName": "class",
        }),
    );
    let response = client.wait_response(&id);
    assert_eq!(
        response.error.map(|e| e.code),
        Some(lsp_server::ErrorCode::RequestFailed as i32)
    );

    // 格式化：已经规整的文件没有改动
    let edits = client.request_raw(
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": main_url },
            "options": { "tabSize": 4, "insertSpaces": true },
        }),
    );
    assert!(edits.as_array().unwrap().is_empty(), "{edits}");

    // 代码操作：在类体里至少能生成 toString()
    let actions = client.request_raw(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": main_url },
            "range": { "start": position_of(MAIN, "void run", 0, 0), "end": position_of(MAIN, "void run", 0, 0) },
            "context": { "diagnostics": [] },
        }),
    );
    let titles: Vec<&str> = actions
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["title"].as_str())
        .collect();
    assert!(titles.contains(&"Generate toString()"), "{titles:?}");

    // 不支持的请求得到 MethodNotFound，而不是没有回应
    let id = client.send_request("textDocument/documentLink", json!({}));
    let response = client.wait_response(&id);
    assert_eq!(
        response.error.map(|e| e.code),
        Some(lsp_server::ErrorCode::MethodNotFound as i32)
    );

    client.shutdown();
}

#[test]
fn incremental_edits_and_pushed_diagnostics() {
    let project = manual_project();
    // 没有声明 pull 诊断能力：服务器应当主动推送
    let mut client = Client::start(&project.root, json!({}));
    let url = project.url("src/app/Main.java");
    client.open(&url, MAIN);

    let clean = client.wait_notification("textDocument/publishDiagnostics", |params| {
        params["uri"] == json!(url)
    });
    let syntax_errors = |params: &Value| {
        params["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["code"] == "syntax-error")
            .count()
    };
    assert_eq!(syntax_errors(&clean), 0);

    // 增量编辑：删掉 `greeter.greet();` 末尾的分号
    let semicolon = position_of(MAIN, "greet();", 0, 7);
    let after = json!({ "line": semicolon["line"], "character": semicolon["character"].as_u64().unwrap() + 1 });
    client.notify_raw(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": url, "version": 2 },
            "contentChanges": [{ "range": { "start": semicolon, "end": after }, "text": "" }],
        }),
    );
    let broken = client.wait_notification("textDocument/publishDiagnostics", |params| {
        params["uri"] == json!(url) && params["version"] == 2
    });
    assert!(syntax_errors(&broken) > 0, "{broken}");

    // 再补回去：错误消失
    client.notify_raw(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": url, "version": 3 },
            "contentChanges": [{ "range": { "start": semicolon, "end": semicolon }, "text": ";" }],
        }),
    );
    let fixed = client.wait_notification("textDocument/publishDiagnostics", |params| {
        params["uri"] == json!(url) && params["version"] == 3
    });
    assert_eq!(syntax_errors(&fixed), 0, "{fixed}");

    // 关闭文件时清空它的诊断
    client.notify_raw(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": url } }),
    );
    let cleared = client.wait_notification("textDocument/publishDiagnostics", |params| {
        params["uri"] == json!(url) && params["version"].is_null()
    });
    assert!(cleared["diagnostics"].as_array().unwrap().is_empty());

    client.shutdown();
}

#[test]
fn pull_diagnostics_and_semantic_token_delta() {
    let project = manual_project();
    let capabilities = json!({
        "general": { "positionEncodings": ["utf-8", "utf-16"] },
        "textDocument": { "diagnostic": {} },
    });
    let mut client = Client::start(&project.root, capabilities);
    let url = project.url("src/app/Main.java");
    client.open(&url, MAIN);
    wait_until_loaded(&mut client, &project);

    let report = client.request_raw(
        "textDocument/diagnostic",
        json!({ "textDocument": { "uri": url } }),
    );
    assert_eq!(report["kind"], "full");
    assert!(report["items"].is_array());

    let full = client.request_raw(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": url } }),
    );
    let result_id = full["resultId"].as_str().unwrap().to_owned();
    assert!(!full["data"].as_array().unwrap().is_empty());

    // 文本没变：增量为空
    let delta = client.request_raw(
        "textDocument/semanticTokens/full/delta",
        json!({ "textDocument": { "uri": url }, "previousResultId": result_id }),
    );
    assert!(delta["edits"].as_array().unwrap().is_empty(), "{delta}");

    // 过期的 result id：退回全量
    let stale = client.request_raw(
        "textDocument/semanticTokens/full/delta",
        json!({ "textDocument": { "uri": url }, "previousResultId": "no-such-id" }),
    );
    assert!(stale["data"].is_array(), "{stale}");

    client.shutdown();
}

#[test]
fn workspace_diagnostics_cover_closed_files() {
    let project = TempProject::new(&[
        ("java-analyzer.toml", MANUAL_CONFIG),
        ("src/app/Greeter.java", GREETER),
        ("src/app/Main.java", MAIN),
        (
            "src/app/Broken.java",
            "package app;\n\nclass Broken {\n    int x = \"s\";\n}\n",
        ),
    ]);
    let capabilities = json!({
        "general": { "positionEncodings": ["utf-8", "utf-16"] },
        "textDocument": { "diagnostic": {} },
        "workspace": { "diagnostic": { "refreshSupport": false } },
    });
    let mut client = Client::start(&project.root, capabilities);
    client.open(&project.url("src/app/Main.java"), MAIN);
    wait_until_loaded(&mut client, &project);
    let broken = project.url("src/app/Broken.java");

    let report = client.request_raw("workspace/diagnostic", json!({ "previousResultIds": [] }));
    let items = report["items"].as_array().unwrap();
    let entry = items
        .iter()
        .find(|i| i["uri"] == broken.as_str())
        .unwrap_or_else(|| panic!("没有打开的文件也要有诊断: {report}"));
    assert_eq!(entry["kind"], "full");
    assert!(!entry["items"].as_array().unwrap().is_empty(), "{entry}");
    // 没有问题的文件不出现
    assert!(
        !items
            .iter()
            .any(|i| i["uri"] == project.url("src/app/Main.java").as_str()),
        "{report}"
    );

    // 带上次的 result id 再问：没有变化
    let result_id = entry["resultId"].clone();
    let again = client.request_raw(
        "workspace/diagnostic",
        json!({ "previousResultIds": [{ "uri": broken, "value": result_id }] }),
    );
    let entry = again["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["uri"] == broken.as_str())
        .unwrap();
    assert_eq!(entry["kind"], "unchanged", "{again}");

    client.shutdown();
}

#[test]
fn maven_project_layout_is_recognized() {
    let pom = "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>g</groupId>\n  <artifactId>a</artifactId>\n  <version>1</version>\n</project>\n";
    let project = TempProject::new(&[
        ("pom.xml", pom),
        ("src/main/java/app/Greeter.java", GREETER),
        ("src/main/java/app/Main.java", MAIN),
    ]);
    let mut client = Client::start(&project.root, json!({}));
    let url = project.url("src/main/java/app/Main.java");
    client.open(&url, MAIN);

    // 另一个文件没有被打开过：能跳过去说明源码根是从 POM 的约定里识别出来的
    let params = json!({
        "textDocument": { "uri": url },
        "position": position_of(MAIN, "Greeter greeter", 0, 1),
    });
    let mut found = Value::Null;
    for _ in 0..600 {
        found = client.request_raw("textDocument/definition", params.clone());
        if found
            .as_array()
            .is_some_and(|locations| !locations.is_empty())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        found[0]["uri"],
        json!(project.url("src/main/java/app/Greeter.java")),
        "{found}"
    );
    client.shutdown();
}

/// Gradle 工程：不运行 Gradle，而是预先写好它会导出的模型（放进导入缓存），
/// 再验证服务器按模型里的源码目录加载——目录名故意不符合约定，约定兜底找不到它。
#[test]
fn gradle_project_is_imported_from_exported_model() {
    let project = TempProject::new(&[
        ("settings.gradle", "rootProject.name = 'demo'\n"),
        ("build.gradle", "plugins { id 'java' }\n"),
        ("code/java/app/Greeter.java", GREETER),
        ("code/java/app/Main.java", MAIN),
    ]);
    let model = json!([{
        "name": "demo",
        "dir": project.root,
        "buildDir": project.path("build"),
        "languageLevel": "17",
        "sourceSets": [{
            "name": "main",
            "srcDirs": [project.path("code/java")],
            "classpath": [],
            "outputDirs": [project.path("build/classes/java/main")],
        }],
    }]);
    let cache_file = project_model::gradle_cache_file(&project.root).expect("没有可用的缓存目录");
    std::fs::create_dir_all(cache_file.parent().unwrap()).unwrap();
    std::fs::write(&cache_file, model.to_string()).unwrap();

    // 关掉 Gradle：只能用缓存里的模型
    let mut client = Client::start_with_options(
        &project.root,
        json!({}),
        json!({ "gradle": { "enabled": false } }),
    );
    let url = project.url("code/java/app/Main.java");
    client.open(&url, MAIN);
    let params = json!({
        "textDocument": { "uri": url },
        "position": position_of(MAIN, "Greeter greeter", 0, 1),
    });
    let mut found = Value::Null;
    for _ in 0..600 {
        found = client.request_raw("textDocument/definition", params.clone());
        if found
            .as_array()
            .is_some_and(|locations| !locations.is_empty())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_file(&cache_file);
    assert_eq!(
        found[0]["uri"],
        json!(project.url("code/java/app/Greeter.java")),
        "{found}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 协议层的行为：补全项怎么插入、界面文案、code lens 的开关
// ---------------------------------------------------------------------------

/// `greeter.` 后面什么都没有：成员补全的现场。
const MAIN_MEMBER_ACCESS: &str = "package app;\n\npublic class Main {\n    void run() {\n        Greeter greeter = new Greeter(\"x\");\n        greeter.;\n    }\n}\n";

/// 有一处确定的错误（调用了不存在的方法）。
const MAIN_WITH_ERROR: &str = "package app;\n\npublic class Main {\n    void run() {\n        Greeter greeter = new Greeter(\"x\");\n        greeter.missing();\n    }\n}\n";

fn member_completion_items(client: &mut Client, project: &TempProject) -> Vec<Value> {
    let url = project.url("src/app/Main.java");
    client.open(&url, MAIN_MEMBER_ACCESS);
    wait_until_loaded(client, project);
    let completion = client.request_raw(
        "textDocument/completion",
        json!({
            "textDocument": { "uri": url },
            "position": position_of(MAIN_MEMBER_ACCESS, "greeter.;", 0, 8),
        }),
    );
    completion["items"].as_array().cloned().unwrap_or_default()
}

fn item<'a>(items: &'a [Value], label: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item["label"] == label)
        .unwrap_or_else(|| panic!("候选里没有 {label}"))
}

#[test]
fn method_completion_inserts_call_with_snippet_support() {
    let project = manual_project();
    let capabilities = json!({
        "textDocument": { "completion": { "completionItem": { "snippetSupport": true } } },
    });
    let mut client = Client::start(&project.root, capabilities);
    let items = member_completion_items(&mut client, &project);

    // 没有形参：直接补上一对括号
    let greet = item(&items, "greet");
    assert_eq!(greet["textEdit"]["newText"], "greet()", "{greet}");
    // 有形参：片段，光标停在括号里
    let equals = item(&items, "equals");
    assert_eq!(equals["textEdit"]["newText"], "equals($0)", "{equals}");
    assert_eq!(equals["insertTextFormat"], 2, "{equals}");
    // 有重载的方法也一样带括号
    let wait = item(&items, "wait");
    let text = wait["textEdit"]["newText"].as_str().unwrap();
    assert!(text.starts_with("wait("), "{wait}");

    // 排序：接收者自己的成员在 Object 的成员前面
    let rank = |label: &str| item(&items, label)["sortText"].as_str().unwrap().to_owned();
    assert!(rank("greet") < rank("hashCode"), "{items:?}");
    assert!(rank("greet") < rank("wait"), "{items:?}");
    client.shutdown();
}

#[test]
fn method_completion_without_snippet_support_is_plain_text() {
    let project = manual_project();
    let mut client = Client::start(&project.root, json!({}));
    let items = member_completion_items(&mut client, &project);
    for label in ["greet", "equals", "wait"] {
        let item = item(&items, label);
        let text = item["textEdit"]["newText"].as_str().unwrap();
        assert!(text.ends_with("()"), "{item}");
        assert!(!text.contains('$'), "{item}");
        assert_eq!(item["insertTextFormat"], 1, "{item}");
    }
    client.shutdown();
}

fn is_ascii_text(value: &Value) -> bool {
    value.as_str().is_some_and(str::is_ascii)
}

#[test]
fn messages_follow_client_locale() {
    let project = manual_project();
    let capabilities = json!({ "textDocument": { "diagnostic": {} } });
    let mut client =
        Client::start_with_locale(&project.root, capabilities, Value::Null, Some("en"));
    let main_url = project.url("src/app/Main.java");
    client.open(&main_url, MAIN_WITH_ERROR);
    wait_until_loaded(&mut client, &project);

    let report = client.request_raw(
        "textDocument/diagnostic",
        json!({ "textDocument": { "uri": main_url } }),
    );
    let diagnostics = report["items"].as_array().unwrap();
    assert!(!diagnostics.is_empty(), "{report}");
    for diagnostic in diagnostics {
        assert!(is_ascii_text(&diagnostic["message"]), "{diagnostic}");
    }

    let greeter_url = project.url("src/app/Greeter.java");
    client.open(&greeter_url, GREETER);
    let lenses = client.request_raw(
        "textDocument/codeLens",
        json!({ "textDocument": { "uri": greeter_url } }),
    );
    let lenses = lenses.as_array().unwrap();
    assert!(!lenses.is_empty());
    for lens in lenses {
        let resolved = client.request_raw("codeLens/resolve", lens.clone());
        assert!(is_ascii_text(&resolved["command"]["title"]), "{resolved}");
    }
    client.shutdown();
}

#[test]
fn code_lens_carries_the_configured_command() {
    let project = manual_project();
    let options = json!({ "codeLens": { "command": "test.showLocations" } });
    let mut client = Client::start_with_options(&project.root, json!({}), options);
    let main_url = project.url("src/app/Main.java");
    client.open(&main_url, MAIN);
    wait_until_loaded(&mut client, &project);

    let greeter_url = project.url("src/app/Greeter.java");
    client.open(&greeter_url, GREETER);
    let lenses = client.request_raw(
        "textDocument/codeLens",
        json!({ "textDocument": { "uri": greeter_url } }),
    );
    // `greet()` 在 Main 里被调用了一次
    let resolved: Vec<Value> = lenses
        .as_array()
        .unwrap()
        .iter()
        .map(|lens| client.request_raw("codeLens/resolve", lens.clone()))
        .collect();
    let with_refs = resolved
        .iter()
        .find(|lens| lens["command"]["command"] == "test.showLocations")
        .unwrap_or_else(|| panic!("{resolved:?}"));
    let locations = with_refs["command"]["arguments"][2].as_array().unwrap();
    assert!(!locations.is_empty(), "{with_refs}");
    assert!(
        locations.iter().any(|l| l["uri"] == main_url.as_str()),
        "{with_refs}"
    );
    client.shutdown();
}

#[test]
fn reference_code_lens_can_be_turned_off() {
    let project = manual_project();
    let options = json!({ "codeLens": { "references": false } });
    let mut client = Client::start_with_options(&project.root, json!({}), options);
    let main_url = project.url("src/app/Main.java");
    client.open(&main_url, MAIN);
    wait_until_loaded(&mut client, &project);

    let greeter_url = project.url("src/app/Greeter.java");
    client.open(&greeter_url, GREETER);
    let lenses = client.request_raw(
        "textDocument/codeLens",
        json!({ "textDocument": { "uri": greeter_url } }),
    );
    let references: Vec<&Value> = lenses
        .as_array()
        .unwrap()
        .iter()
        .filter(|lens| lens["data"]["implementations"] == false)
        .collect();
    assert!(references.is_empty(), "{references:?}");
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Spring 配置文件
// ---------------------------------------------------------------------------

const APP_PROPERTIES: &str = "package app;\n\nimport org.springframework.boot.context.properties.ConfigurationProperties;\n\n@ConfigurationProperties(prefix = \"app\")\npublic class AppProperties {\n    /** Request timeout in seconds. */\n    private int timeout = 30;\n    private boolean enabled;\n}\n";

const CONFIGURATION_PROPERTIES: &str = "package org.springframework.boot.context.properties;\n\npublic @interface ConfigurationProperties {\n    String value() default \"\";\n    String prefix() default \"\";\n}\n";

#[test]
fn spring_config_file_completion_hover_and_diagnostics() {
    let config = "app.ti\n";
    let project = TempProject::new(&[
        ("java-analyzer.toml", MANUAL_CONFIG),
        ("src/app/AppProperties.java", APP_PROPERTIES),
        (
            "src/org/springframework/boot/context/properties/ConfigurationProperties.java",
            CONFIGURATION_PROPERTIES,
        ),
        ("src/main/resources/application.properties", config),
    ]);
    let mut client = Client::start(&project.root, json!({}));
    let url = project.url("src/main/resources/application.properties");
    client.notify_raw(
        lsp_types::notification::DidOpenTextDocument::METHOD,
        json!({ "textDocument": { "uri": url, "languageId": "jproperties", "version": 1, "text": config } }),
    );

    // 工程导入是异步的：反复请求直到 `@ConfigurationProperties` 的键出现
    let params = json!({
        "textDocument": { "uri": url },
        "position": position_of(config, "app.ti", 0, 6),
    });
    let mut items = Vec::new();
    for _ in 0..600 {
        let completion = client.request_raw("textDocument/completion", params.clone());
        items = completion["items"].as_array().cloned().unwrap_or_default();
        if items.iter().any(|i| i["label"] == "app.timeout") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let timeout = item(&items, "app.timeout");
    assert_eq!(timeout["textEdit"]["newText"], "app.timeout=", "{timeout}");
    assert_eq!(
        timeout["textEdit"]["range"]["start"],
        json!({ "line": 0, "character": 0 }),
        "{timeout}"
    );
    assert!(
        !items.iter().any(|i| i["label"] == "app.enabled"),
        "前缀不匹配的键不该出现: {items:?}"
    );

    // 补全条目的文档来自字段的 Javadoc
    let resolved = client.request_raw("completionItem/resolve", timeout.clone());
    assert!(
        resolved["documentation"]["value"]
            .as_str()
            .is_some_and(|d| d.contains("Request timeout")),
        "{resolved}"
    );

    // 改成完整的键：悬停显示类型、默认值和说明
    let edited = "app.timeout=5\napp.timeot=1\n";
    client.notify_raw(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": url, "version": 2 },
            "contentChanges": [{ "text": edited }],
        }),
    );
    let hover = client.request_raw(
        "textDocument/hover",
        json!({
            "textDocument": { "uri": url },
            "position": position_of(edited, "timeout", 0, 2),
        }),
    );
    let value = hover["contents"]["value"].as_str().unwrap_or_default();
    assert!(value.contains("int app.timeout = 30"), "{hover}");
    assert!(value.contains("Request timeout"), "{hover}");

    // 拼错的键：`app` 是已知分组，推送警告
    let diagnostics = client.wait_notification("textDocument/publishDiagnostics", |params| {
        params["uri"] == json!(url) && params["version"] == 2
    });
    let unknown: Vec<&Value> = diagnostics["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "unknown-config-property")
        .collect();
    assert_eq!(unknown.len(), 1, "{diagnostics}");
    assert_eq!(unknown[0]["range"]["start"]["line"], 1, "{diagnostics}");
    client.shutdown();
}

#[test]
fn new_empty_file_gets_package_and_class() {
    let project = TempProject::new(&[
        ("java-analyzer.toml", MANUAL_CONFIG),
        ("src/app/Greeter.java", GREETER),
        ("src/app/Main.java", MAIN),
        ("src/app/util/Strings.java", ""),
    ]);
    let capabilities = json!({ "workspace": { "applyEdit": true } });
    let mut client = Client::start(&project.root, capabilities);
    client.open(&project.url("src/app/Main.java"), MAIN);
    wait_until_loaded(&mut client, &project);

    let url = project.url("src/app/util/Strings.java");
    client.open(&url, "");
    let params = client.wait_server_request("workspace/applyEdit");
    let edits = &params["edit"]["changes"][url.as_str()];
    assert_eq!(
        edits[0]["newText"], "package app.util;\n\npublic class Strings {\n}\n",
        "{params}"
    );
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );
    client.shutdown();
}
