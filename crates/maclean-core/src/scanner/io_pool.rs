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
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// C-14 修订 · 一个阻塞 IO 任务。
///
/// 早期版本给任务带「提交时刻截止时间」，worker 取出时一旦过期就直接跳过不执行。
/// 在繁忙机器上这会造成**误判**：任务先要在自适应并发闸门（繁忙时只放行 2 个）
/// 后面排队，排队几十秒后到点被丢弃，调用方却把它记成「该目录枚举超时」，于是整段
/// 子树被剪枝（实测 `/Applications` 顶层因此返回 0 项，应用模块直接空结果）。
/// 现在任务不再带提交截止时间：**排队不是卡死**，看门狗只衡量真正执行的那一段时间。
struct Task {
    job: Box<dyn FnOnce() + Send + 'static>,
}

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
                                let Task { job } = task;
                                // gated_run 负责自适应并发闸门（繁忙时限在途 IO）与执行
                                // 延迟反馈；catch_unwind 兜底确保任务 panic 不杀死 worker
                                //（否则池容量会悄悄缩小）。
                                let _ =
                                    catch_unwind(AssertUnwindSafe(|| super::load::gated_run(job)));
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

/// 等「真正开始执行」的排队上限。超过它说明系统已经饱和到任务根本排不上，
/// 此时放弃并上报拥塞，而不是无限期挂住调用方所在的扫描线程。
const QUEUE_WAIT_LIMIT: Duration = Duration::from_secs(30);

/// 在有界 IO 池中执行阻塞操作 `f`。
///
/// 计时口径（C-14 修订）：`timeout` 只约束**执行**这一段，从 worker 拿到并发名额、
/// 真正开始跑 `f` 起算；任务的排队等待另用 [`QUEUE_WAIT_LIMIT`] 约束。这样繁忙机器上
/// 「排在闸门后面」不会被误记成「目录枚举超时」而剪掉整段子树。
///
/// - 正常完成：`Some(result)`。
/// - 执行超时 / 排队超限 / 池关闭 / `f` panic：`None`。超时后底层操作可能仍在某个
///   worker 内阻塞（不可取消的内核 IO），但受线程池上界约束，不影响主流程。
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
    // 「已开始执行」信号：worker 拿到闸门名额后才发，调用方据此才开始计时。
    let (started_tx, started_rx) = channel::<()>();
    let task = Task {
        job: Box::new(move || {
            let _ = started_tx.send(());
            if let Ok(result) = catch_unwind(AssertUnwindSafe(f)) {
                let _ = tx.send(result);
            }
        }),
    };
    if pool().tx.send(task).is_err() {
        return None;
    }
    // 1) 排队阶段：受闸门限流时可能要等，超过上限才放弃（这是真饱和，不是卡死）。
    if started_rx.recv_timeout(QUEUE_WAIT_LIMIT).is_err() {
        super::load::note_wait_timeout();
        return None;
    }
    // 2) 执行阶段：看门狗预算从此刻起算。
    match rx.recv_timeout(timeout) {
        Ok(r) => Some(r),
        Err(err) => {
            // C-14：只有真正的等待超时才是拥塞信号；Disconnected 说明 sender
            // 全被释放却没发结果（任务 panic 被 catch_unwind 吞掉），与 IO
            // 拥塞无关，不能再误报给负载自适应。
            if should_note_congestion(&err) {
                super::load::note_wait_timeout();
            }
            None
        }
    }
}

/// C-14 · 是否应把 recv 错误当拥塞上报：Timeout 才是，Disconnected 不是。
fn should_note_congestion(err: &RecvTimeoutError) -> bool {
    matches!(err, RecvTimeoutError::Timeout)
}

// =========================================================================
//  C-12 · 统一外部命令执行器（带超时 + 退出码检查）
//
//  旧代码里 `Command::output()` 既无超时、多数分支又只判 `Ok(out)` 不判
//  `out.status.success()`，于是「命令失败」和「确实没有结果」不可区分。
//  这里把两者分开，并复用 run_with_timeout（有界 IO 池 + 截止时间）。
// =========================================================================

/// C-12 · 外部命令执行结果
#[derive(Debug)]
pub enum CommandOutcome {
    /// 启动成功且退出码为 0
    Success(std::process::Output),
    /// 启动成功但退出码非 0（失败，不是「没有结果」）
    Failed(std::process::Output),
    /// 命令根本没启动起来
    SpawnError(String),
    /// 等待超时（底层任务带截止时间，worker 取出后会判过期跳过）
    Timeout,
}

impl CommandOutcome {
    /// 仅在真正成功时取出 stdout；失败/未启动/超时一律为 None。
    pub fn ok_output(&self) -> Option<&std::process::Output> {
        match self {
            CommandOutcome::Success(out) => Some(out),
            _ => None,
        }
    }
}

/// C-12 · 带超时与退出码检查地执行外部命令。
pub fn command_output_checked(program: &str, args: &[&str], timeout: Duration) -> CommandOutcome {
    let program = program.to_string();
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    match run_with_timeout(
        move || std::process::Command::new(&program).args(&owned).output(),
        timeout,
    ) {
        None => CommandOutcome::Timeout,
        Some(Err(e)) => CommandOutcome::SpawnError(e.to_string()),
        Some(Ok(out)) if out.status.success() => CommandOutcome::Success(out),
        Some(Ok(out)) => CommandOutcome::Failed(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c14_only_timeout_is_reported_as_congestion() {
        // 反向验证：把 should_note_congestion 改成恒 true，第二条断言必红。
        assert!(should_note_congestion(&RecvTimeoutError::Timeout));
        assert!(!should_note_congestion(&RecvTimeoutError::Disconnected));
    }

    #[test]
    fn c14_execution_timeout_still_enforced() {
        // 执行期预算仍须生效：任务本身睡 200ms、预算 100ms → None（判为本次不可枚举）。
        // 反向验证：把 recv_timeout 换成无界 recv，本用例必红。
        let slow = run_with_timeout(
            || {
                std::thread::sleep(Duration::from_millis(200));
                1u32
            },
            Duration::from_millis(100),
        );
        assert!(slow.is_none(), "执行超过预算必须判超时");
        // 快任务照常拿到结果：预留 Gates 排队不影响小预算下的正常结果。
        assert_eq!(
            run_with_timeout(|| 7u32, Duration::from_millis(100)),
            Some(7)
        );
    }

    #[cfg(unix)]
    #[test]
    fn c12_command_outcome_distinguishes_failure_from_empty_result() {
        // 用一个必定非 0 退出的命令验证「失败」与「没有结果」可区分
        let failed = command_output_checked("sh", &["-c", "exit 7"], Duration::from_secs(5));
        assert!(matches!(failed, CommandOutcome::Failed(_)));
        assert!(failed.ok_output().is_none());

        let ok = command_output_checked("sh", &["-c", "printf hello"], Duration::from_secs(5));
        match ok {
            CommandOutcome::Success(ref out) => {
                assert_eq!(String::from_utf8_lossy(&out.stdout), "hello");
            }
            other => panic!("期望 Success，得到 {:?}", other),
        }
        assert!(ok.ok_output().is_some());
    }
}
