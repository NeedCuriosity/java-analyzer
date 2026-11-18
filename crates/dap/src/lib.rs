//! Java 调试适配器（Debug Adapter Protocol）。
//!
//! 通过标准输入输出和编辑器说 DAP（和 LSP 一样用 `Content-Length` 分帧），
//! 直接用 JDWP 协议经 socket 连接目标 JVM，不依赖任何 Java 侧的辅助程序。
//!
//! - `launch`：用 `-agentlib:jdwp=transport=dt_socket,server=n,suspend=y` 启动 JVM，让它连回
//!   调试器监听的本地端口；没有给 `classPaths` 时从工程模型（Maven / Gradle / 手动配置）算出
//!   运行时类路径。调试器不负责构建，工程要先编译好。
//! - `attach`：连接已经带着 `-agentlib:jdwp=...,server=y` 运行的 JVM。
//! - 自定义请求 `redefineClasses`：把重新编译过的类热替换进 JVM（见 `hotswap`）。

mod breakpoints;
mod completions;
mod eval;
mod evaluator;
mod hotswap;
pub mod jdwp;
mod launch;
mod protocol;
mod session;
mod sources;
pub mod transport;
mod variables;

use std::io;

use crossbeam_channel::Receiver;
use serde_json::Value;

pub use crate::launch::{load_project, runtime_class_path};
pub use crate::protocol::Output;
use crate::session::Session;

/// 在标准输入输出上运行调试适配器，直到客户端断开。
pub fn run_stdio() -> anyhow::Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("dap-stdin".to_owned())
        .spawn(move || {
            let mut stdin = io::stdin().lock();
            loop {
                match transport::read_message(&mut stdin) {
                    Ok(Some(message)) => {
                        if tx.send(message).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        tracing::error!("读取 DAP 消息失败: {err}");
                        break;
                    }
                }
            }
        })?;
    serve(rx, move |message: &Value| {
        let mut stdout = io::stdout().lock();
        if let Err(err) = transport::write_message(&mut stdout, message) {
            tracing::error!("写出 DAP 消息失败: {err}");
        }
    });
    Ok(())
}

/// 处理 `requests` 里的请求，响应和事件交给 `sink`；请求通道关闭或收到 `disconnect` 后返回。
/// 测试和嵌入时使用，不经过分帧。
pub fn serve(requests: Receiver<Value>, sink: impl FnMut(&Value) + Send + 'static) {
    Session::new(Output::new(Box::new(sink))).run(requests);
}
