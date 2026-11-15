//! 分三档的线程池。
//!
//! - `High`：延迟敏感的请求（补全、签名帮助）。有专属线程，永远不会排在别的任务后面。
//! - `Normal`：普通请求（悬停、跳转、高亮、诊断）。
//! - `Background`：索引预热、引用搜索之类的长任务。只占用一小部分线程，
//!   因此不会饿死前两档。
//!
//! 每个工作线程按固定的优先级顺序取任务：先看高档队列，再看自己被允许处理的低档队列。

use std::thread;

use crossbeam_channel::{unbounded, Receiver, Select, Sender};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    High,
    Normal,
    Background,
}

type Job = Box<dyn FnOnce() + Send + 'static>;

pub struct TaskPool {
    high: Sender<Job>,
    normal: Sender<Job>,
    background: Sender<Job>,
}

fn worker(queues: Vec<Receiver<Job>>) {
    loop {
        // 先按优先级非阻塞地取
        let mut job = None;
        for queue in &queues {
            if let Ok(found) = queue.try_recv() {
                job = Some(found);
                break;
            }
        }
        let job = match job {
            Some(job) => job,
            None => {
                // 都空了：阻塞等任意一个队列
                let mut select = Select::new();
                for queue in &queues {
                    select.recv(queue);
                }
                let operation = select.select();
                let index = operation.index();
                match operation.recv(&queues[index]) {
                    Ok(job) => job,
                    // 发送端全部关闭：进程正在退出
                    Err(_) => return,
                }
            }
        };
        job();
    }
}

impl TaskPool {
    pub fn new() -> TaskPool {
        let threads = thread::available_parallelism()
            .map_or(4, |n| n.get())
            .max(3);
        let (high, high_rx) = unbounded::<Job>();
        let (normal, normal_rx) = unbounded::<Job>();
        let (background, background_rx) = unbounded::<Job>();

        let background_threads = (threads / 4).max(1);
        let high_only_threads = 1;
        let normal_threads = threads
            .saturating_sub(background_threads + high_only_threads)
            .max(1);

        let spawn = |name: String, queues: Vec<Receiver<Job>>| {
            thread::Builder::new()
                .name(name)
                // 类型推断的递归可能很深
                .stack_size(16 * 1024 * 1024)
                .spawn(move || worker(queues))
                .expect("无法创建工作线程");
        };
        for i in 0..high_only_threads {
            spawn(format!("ja-high-{i}"), vec![high_rx.clone()]);
        }
        for i in 0..normal_threads {
            spawn(
                format!("ja-normal-{i}"),
                vec![high_rx.clone(), normal_rx.clone()],
            );
        }
        for i in 0..background_threads {
            spawn(
                format!("ja-background-{i}"),
                vec![high_rx.clone(), normal_rx.clone(), background_rx.clone()],
            );
        }
        TaskPool {
            high,
            normal,
            background,
        }
    }

    pub fn spawn(&self, tier: Tier, job: impl FnOnce() + Send + 'static) {
        let queue = match tier {
            Tier::High => &self.high,
            Tier::Normal => &self.normal,
            Tier::Background => &self.background,
        };
        // 工作线程只在进程退出时结束，发送不会失败
        let _ = queue.send(Box::new(job));
    }
}
