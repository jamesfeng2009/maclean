//! 有界阻塞 IO 池。
//!
//! 把可能无限期阻塞在内核的目录枚举 / `stat` 等操作，隔离到**固定数量**的
//! 专用线程里执行；调用方只做带超时的等待。
//!
//! ## 为什么需要它
//!
//! 旧实现对「每个目录」都 `std::thread::spawn` 一个线程去跑 `readdir`，再用
//! `sync_channel.recv_timeout(10s)` 等结果。超时后主流程确实会跳过该目录，
//! 但**阻塞在内核里的那个线程无法取消、只能 detach 放任泄漏**。在 NFS 掉线 /
//! TCC 容器上，一个批次（可能成百上千个目录）会瞬间 spawn 出同等数量的僵尸
//! 线程，最终把调度 / 文件描述符 / 内存拖垮，表现为整个扫描永久挂起。
//!
//! ## 现在的做法
//!
//! - 一个进程级、线程数固定（按 CPU 核数在 16–32 间取值）的专用阻塞线程池
//!   （独立于 rayon 的
//!   CPU 线程池），所有可能卡住的文件 IO 都提交到这里。
//! - 调用方通过 `mpsc` 回传通道 + `recv_timeout` 等待；超时只放弃**取结果**，
//!   不去（也无法）取消内核 IO。
//! - 最坏情况下卡死的 worker 也只占满固定名额（数量等于池大小的有限个僵尸），
//!   不会无上限增长；排队任务超时后被放弃，主流程永远继续推进。
//! - 配合 [`super::fs_guard`] 的远程卷 / TCC 容器快跳，绝大多数阻塞根本不会
//!   发生，僵尸 worker 在真实环境里应趋近于零。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// 一个阻塞 IO 任务：无参、一次性、可跨线程发送。
type Task = Box<dyn FnOnce() + Send + 'static>;

struct Pool {
    tx: Sender<Task>,
    /// 仅需持有句柄以表达生命周期；worker 随进程存活，不主动 join。
    _workers: Vec<JoinHandle<()>>,
}

/// 常驻 worker 数取负载自适应的**上限**（见 [`super::load::max_concurrency`]）：
/// 这是「最多能同时在途多少」的天花板，不代表一开始就满并发。真正的有效并发由
/// [`super::load`] 的闸门在 2～上限之间动态调节——繁忙时只放行 2–4 个，空闲才平滑
/// 升满。这些线程跑磁盘 / 网络元数据 IO 而非 CPU 计算，常驻数按 CPU 核数 2×、在
/// [16, 32] 收敛并封顶，避免在机械盘 / 网络卷上过度争用；真正永久阻塞的枚举已由
/// fs_guard 前置剪枝，能卡死 worker 的任务极少，最坏也只占满这个有界名额。
static POOL: OnceLock<Pool> = OnceLock::new();

fn pool() -> &'static Pool {
    POOL.get_or_init(|| {
        let worker_count = super::load::max_concurrency();
        let (tx, rx) = channel::<Task>();
        // Receiver 不是 Sync，用 Mutex 共享给固定数量的 worker。
        // 进程生命周期内只建一次，Box::leak 得到 'static 引用（可接受的一次性泄漏）。
        let rx: &'static Mutex<Receiver<Task>> = Box::leak(Box::new(Mutex::new(rx)));

        let mut workers = Vec::with_capacity(worker_count);
        for id in 0..worker_count {
            let builder = thread::Builder::new().name(format!("maclean-io-{id}"));
            let handle = builder
                .spawn(move || {
                    // IO 工作线程整体降到后台 QoS，不与前台 App / WindowServer 平权抢盘。
                    super::load::apply_bg_priority();
                    loop {
                        // 仅在「取下一个任务」期间持锁；recv 返回后立刻离开作用域释放锁，
                        // 再执行任务体 —— 这样多个 worker 能真正并行处理任务。
                        let next = {
                            let guard = match rx.lock() {
                                Ok(g) => g,
                                // 锁中毒说明某个持锁线程 panic，退出本 worker。
                                Err(_) => break,
                            };
                            guard.recv()
                        };
                        match next {
                            Ok(task) => {
                                // gated_run 负责自适应并发闸门（繁忙时限在途 IO）与执行
                                // 延迟反馈；catch_unwind 兜底确保任务 panic 不杀死 worker
                                //（否则池容量会悄悄缩小）。
                                let _ = catch_unwind(AssertUnwindSafe(|| {
                                    super::load::gated_run(task)
                                }));
                            }
                            // 所有发送端释放（进程退出）时 recv 报错，worker 干净退出。
                            Err(_) => break,
                        }
                    }
                })
                .expect("failed to spawn maclean io worker");
            workers.push(handle);
        }

        Pool {
            tx,
            _workers: workers,
        }
    })
}

/// 在有界 IO 池中执行阻塞操作 `f`，最多等待 `timeout`。
///
/// - 正常完成：`Some(result)`。
/// - 超时 / 池关闭 / `f` panic：`None`。超时后底层操作可能仍在某个 worker
///   内阻塞（不可取消的内核 IO），但受线程池上界约束，不影响主流程。
///
/// 回传通道使用**无界** `mpsc::channel`：即便调用方已超时放弃接收，worker
/// 完成后 `send` 也不会因此阻塞（结果留在缓冲区里随通道一起丢弃），避免
/// worker 被"没人要的结果"反卡死。
pub fn run_with_timeout<F, R>(f: F, timeout: Duration) -> Option<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (tx, rx) = channel::<R>();
    let task: Task = Box::new(move || {
        if let Ok(result) = catch_unwind(AssertUnwindSafe(f)) {
            let _ = tx.send(result);
        }
    });
    if pool().tx.send(task).is_err() {
        return None;
    }
    match rx.recv_timeout(timeout) {
        Ok(r) => Some(r),
        Err(_) => {
            // 到点仍未拿到结果：可能是池内排队（繁忙期限流）也可能是内核 IO 卡死。
            // 两种都应被当成拥塞信号反馈，驱动自适应并发下降；底层任务即便之后才完成
            // 也只是被丢弃结果，受有界池上界约束，不影响主流程。
            super::load::note_wait_timeout();
            None
        }
    }
}
