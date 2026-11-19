//! 到目标 JVM 的 JDWP 连接：握手、按 ID 匹配回复、把事件转给会话。
//!
//! 一个读线程不停地读包：回复交给等待它的请求方，`Event.Composite` 命令转成 [`JdwpEvent`]。
//! 请求方在 [`Connection::request`] 里阻塞等回复，超时就报错，保证编辑器不会被卡死。

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use base_db::{tr, trs};
use crossbeam_channel::{bounded, Receiver, Sender};

use super::{JdwpError, Packet};

/// 连接已经断开时的错误文案。
pub fn disconnected() -> &'static str {
    trs!(
        "与目标 JVM 的连接已断开",
        "The connection to the target JVM is closed"
    )
}

const HANDSHAKE: &[u8] = b"JDWP-Handshake";

/// 读线程交给会话的消息。
#[derive(Debug)]
pub enum JdwpEvent {
    /// `Event.Composite` 命令的数据部分，等会话知道 ID 长度后再解析
    Composite(Vec<u8>),
    /// 连接断开（目标 JVM 退出或者网络断了）
    Closed,
}

/// 回复的数据，或者 JDWP 错误码。
pub type Reply = Result<Vec<u8>, u16>;

/// 已经发出、还没收到回复的命令。
pub struct PendingReply {
    pub id: u32,
    pub rx: Receiver<Reply>,
}

/// 把回复转成结果：错误码变成 [`JdwpError`]。
pub fn reply_result(reply: Reply) -> anyhow::Result<Vec<u8>> {
    reply.map_err(|code| JdwpError(code).into())
}

pub struct Connection {
    writer: Mutex<TcpStream>,
    pending: Arc<Mutex<HashMap<u32, Sender<Reply>>>>,
    closed: Arc<AtomicBool>,
    next_id: AtomicU32,
    timeout: Duration,
}

impl Connection {
    /// 在已建立的 TCP 连接上握手，然后启动读线程。事件发到 `events`。
    pub fn handshake(
        stream: TcpStream,
        events: Sender<JdwpEvent>,
        timeout: Duration,
    ) -> anyhow::Result<Arc<Connection>> {
        stream.set_nodelay(true).ok();
        let mut stream = stream;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.write_all(HANDSHAKE)?;
        let mut reply = [0u8; HANDSHAKE.len()];
        stream.read_exact(&mut reply).map_err(|e| {
            anyhow::anyhow!(tr!("JDWP 握手失败: {e}", "JDWP handshake failed: {e}"))
        })?;
        anyhow::ensure!(
            reply == HANDSHAKE,
            trs!(
                "JDWP 握手失败: 对端回复不是 JDWP-Handshake",
                "JDWP handshake failed: the peer did not reply with JDWP-Handshake"
            )
        );
        stream.set_read_timeout(None)?;

        let reader = stream.try_clone()?;
        let pending: Arc<Mutex<HashMap<u32, Sender<Reply>>>> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let conn = Arc::new(Connection {
            writer: Mutex::new(stream),
            pending: pending.clone(),
            closed: closed.clone(),
            next_id: AtomicU32::new(1),
            timeout,
        });
        std::thread::Builder::new()
            .name("jdwp-reader".to_owned())
            .spawn(move || read_loop(reader, pending, closed, events))?;
        Ok(conn)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// 发一个命令并等回复。JDWP 错误码转成 [`JdwpError`]。
    pub fn request(&self, set: u8, cmd: u8, data: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        self.request_with_timeout(set, cmd, data, self.timeout)
    }

    pub fn request_with_timeout(
        &self,
        set: u8,
        cmd: u8,
        data: Vec<u8>,
        timeout: Duration,
    ) -> anyhow::Result<Vec<u8>> {
        let PendingReply { id, rx } = self.send_command(set, cmd, data)?;
        match rx.recv_timeout(timeout) {
            Ok(reply) => reply_result(reply),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                self.forget(id);
                anyhow::bail!(tr!(
                    "目标 JVM 在 {timeout:?} 内没有回复命令 {set}/{cmd}",
                    "The target JVM did not reply to command {set}/{cmd} within {timeout:?}"
                ))
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                anyhow::bail!(disconnected())
            }
        }
    }

    /// 发一个命令，不等回复：回复到达时放进返回的通道。等不到时用 [`Connection::forget`] 撤销登记。
    pub fn send_command(&self, set: u8, cmd: u8, data: Vec<u8>) -> anyhow::Result<PendingReply> {
        anyhow::ensure!(!self.is_closed(), disconnected());
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = bounded(1);
        self.pending.lock().unwrap().insert(id, tx);
        // 读线程先置关闭标志再清空等待者：登记之后再看一次，避免和断开擦肩而过、白等到超时
        if self.is_closed() {
            self.pending.lock().unwrap().remove(&id);
            anyhow::bail!(disconnected());
        }
        let bytes = Packet::Command { id, set, cmd, data }.encode();
        let written = self.writer.lock().unwrap().write_all(&bytes);
        if let Err(err) = written {
            self.pending.lock().unwrap().remove(&id);
            anyhow::bail!(tr!(
                "向目标 JVM 发送命令失败: {err}",
                "Failed to send a command to the target JVM: {err}"
            ));
        }
        Ok(PendingReply { id, rx })
    }

    /// 不再等这个命令的回复（之后到达的回复直接丢掉）。
    pub fn forget(&self, id: u32) {
        self.pending.lock().unwrap().remove(&id);
    }

    /// 关闭连接；读线程随之退出。
    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(stream) = self.writer.lock() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn read_loop(
    mut stream: TcpStream,
    pending: Arc<Mutex<HashMap<u32, Sender<Reply>>>>,
    closed: Arc<AtomicBool>,
    events: Sender<JdwpEvent>,
) {
    loop {
        match Packet::read_from(&mut stream) {
            Ok(Some(Packet::Reply { id, error, data })) => {
                let waiter = pending.lock().unwrap().remove(&id);
                if let Some(waiter) = waiter {
                    let _ = waiter.send(if error == 0 { Ok(data) } else { Err(error) });
                }
            }
            Ok(Some(Packet::Command { set, cmd, data, .. })) => {
                // 目标 JVM 只会发 `Event.Composite`
                if set == 64 && cmd == 100 {
                    if events.send(JdwpEvent::Composite(data)).is_err() {
                        break;
                    }
                } else {
                    tracing::debug!("忽略目标 JVM 发来的命令 {set}/{cmd}");
                }
            }
            Ok(None) => break,
            Err(err) => {
                if !closed.load(Ordering::SeqCst) {
                    tracing::debug!("读取 JDWP 包失败: {err}");
                }
                break;
            }
        }
    }
    closed.store(true, Ordering::SeqCst);
    // 丢掉所有等待者：它们会立刻收到“连接已断开”
    pending.lock().unwrap().clear();
    let _ = events.send(JdwpEvent::Closed);
}
