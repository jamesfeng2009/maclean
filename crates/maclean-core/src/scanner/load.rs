//! 扫描负载自适应：后台优先级 + 负载信号 + 自适应并发闸门。
//!
//! ## 要解决的问题
//!
//! 真实用户的机器常常是「忙」的：爱奇艺在读盘、Spotlight 在建索引、浏览器 / 微信
//! 占着大量内存与 IO。早期实现为空闲机器调参——一上来就用 16–32 个线程满并发打
//! 内核，且扫描线程与前台 App **平权**抢盘。饱和态下几十个线程挤同一条元数据队列，
//! 比 2–4 个线程顺序跑更慢；固定的 10s 目录看门狗又把「只是排队慢」误判成「坏目录」
//! 成片剪枝，最终扫描器顶层预算耗尽、整模块返回 0。
//!
//! ## 本模块三件事
//!
//! 1. [`apply_bg_priority`]：把扫描 / IO / rayon 工作线程降到 macOS `UTILITY` QoS，
//!    不再与 WindowServer / 用户正在用的 App 平权抢 CPU 与 IO。
//! 2. [`Load`]：按「任务真实执行延迟」的指数移动平均（EWMA）与近期超时率，用
//!    加性增 / 乘性减（类 TCP 拥塞控制）动态决定有效并发；繁忙时降到 2–4，空闲时
//!    平滑升回满速。
//! 3. 看门狗参数（[`is_busy`] 等）：让目录枚举在繁忙时拿到更长预算、本地卷给一次
//!    退避重试，而远程卷 / TCC 沙盒仍按「不可达 / 无权限」立即快速失败。
//!
//! 所有状态都是进程内、单次运行的内存态：不持久化、不跨运行记忆，重启即回到温和的
//! 初始并发，再在运行中自适应。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// QoS class `UTILITY`（macOS `<sys/qos.h>`）：低于用户主动发起的工作，系统会在
/// 资源紧张时压制其 CPU 调度与 IO 发放，但又不像 BACKGROUND 那样可能被强烈挂起。
#[cfg(target_os = "macos")]
const QOS_CLASS_UTILITY: u32 = 0x11;

/// 有效并发下限：即便极度繁忙也保留少量在途，保证扫描能缓慢前进、不是完全停摆。
const MIN_CONCURRENCY: usize = 2;

/// 把「有效并发降到这个值或更低」视为繁忙（用于放宽看门狗 / 允许本地重试）。
const BUSY_CONCURRENCY: usize = 4;

/// 首次 / 刚启动时的温和并发，避免在尚未测得负载前就满并发把繁忙机器压垮；
/// 之后由 [`Load`] 在运行中自适应升降。
fn start_concurrency() -> usize {
    available_cores().clamp(4, 8)
}

/// 有效并发上限（与 io_pool 常驻 worker 数一致）：SSD/APFS 空闲态能吃下的并发。
pub(crate) fn max_concurrency() -> usize {
    available_cores().saturating_mul(2).clamp(16, 32)
}

fn available_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8)
}

/// 把**当前线程**降为后台优先级。必须在每个扫描 / IO / rayon 工作线程的入口调用
/// （QoS 是线程属性，不随线程池传播）。非 macOS 平台为 no-op。
pub fn apply_bg_priority() {
    #[cfg(target_os = "macos")]
    unsafe {
        // 自己声明，避免依赖特定 libc 版本是否导出该符号；它在 macOS 10.10+ 始终可用。
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        let _ = pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
    }
}

/// 一次性安装一个**全局后台 rayon 线程池**：所有 `par_iter` 的工作线程都带 UTILITY
/// QoS。必须在任何并行迭代之前调用；重复 / 过晚调用会被忽略（rayon 全局池只允许
/// 初始化一次），返回是否由本次成功安装。
pub fn install_rayon_bg_pool() -> bool {
    rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("maclean-rayon-{i}"))
        .spawn_handler(|thread| {
            // 自定义 spawn：std::thread::Builder::spawn 返回 io::Result（spawn_handler 要求）。
            // 在 rayon 的工作线程入口 run() 之前，先把本线程降为后台 QoS。
            std::thread::Builder::new()
                .name(format!("maclean-rayon-{}", thread.index()))
                .spawn(move || {
                    apply_bg_priority();
                    thread.run();
                })
                .map(|_| ())
        })
        .build_global()
        .is_ok()
}

/// 并发闸门：控制「正在执行 IO 任务」的线程数不超过动态 `limit`。
struct Gate {
    active: usize,
    limit: usize,
}

struct Load {
    gate: Mutex<Gate>,
    cv: Condvar,
    /// 成功完成任务的执行延迟 EWMA（微秒），仅统计**执行**时间、不含主动限流排队。
    ewma_us: AtomicU64,
    /// 自上次调节窗口以来的成功 / 超时计数。
    win_ok: AtomicU64,
    win_timeout: AtomicU64,
    last_adjust: Mutex<Instant>,
}

static LOAD: OnceLock<Load> = OnceLock::new();

fn load() -> &'static Load {
    LOAD.get_or_init(|| Load {
        gate: Mutex::new(Gate {
            active: 0,
            limit: start_concurrency(),
        }),
        cv: Condvar::new(),
        ewma_us: AtomicU64::new(0),
        win_ok: AtomicU64::new(0),
        win_timeout: AtomicU64::new(0),
        last_adjust: Mutex::new(Instant::now()),
    })
}

/// EWMA 认为「执行延迟偏高」的阈值：300ms（空闲本地 APFS 元数据调用通常是毫秒级）。
const LATENCY_BUSY_US: u64 = 300_000;
/// EWMA 认为「已经很空闲、可以加并发」的阈值：80ms。
const LATENCY_IDLE_US: u64 = 80_000;
/// 加并发前窗口内至少要有这么多成功样本，避免凭一两个快样本盲目拉满。
const IDLE_MIN_SAMPLES: u64 = 8;
/// 常规（成功驱动的）升降调节之间的最小间隔，避免抖动。
const ADJUST_INTERVAL: Duration = Duration::from_millis(1200);

impl Load {
    fn current_limit(&self) -> usize {
        self.gate.lock().expect("load gate poisoned").limit
    }

    /// 工作线程在执行 IO 任务前获取一个有效并发名额；繁忙（名额用尽）时在此等待，
    /// 从而把内核实际在途 IO 压到自适应上限以内。
    fn acquire(&self) {
        let mut g = self.gate.lock().expect("load gate poisoned");
        while g.active >= g.limit {
            g = self.cv.wait(g).expect("load condvar poisoned");
        }
        g.active += 1;
    }

    fn release(&self) {
        let mut g = self.gate.lock().expect("load gate poisoned");
        if g.active > 0 {
            g.active -= 1;
        }
        // 唤醒一个在等名额的 worker（提并发或某个任务腾出名额时）。
        self.cv.notify_one();
    }

    /// 记录一次成功完成的任务（仅执行耗时），并按节流做常规调节。
    fn record_ok(&self, exec_us: u64) {
        let prev = self.ewma_us.load(Ordering::Relaxed);
        let next = if prev == 0 {
            exec_us
        } else {
            (prev * 4 + exec_us) / 5 // α=0.2 的 EWMA
        };
        self.ewma_us.store(next, Ordering::Relaxed);
        self.win_ok.fetch_add(1, Ordering::Relaxed);
        self.maybe_adjust(false);
    }

    /// 记录一次任务超时（调用方等待到点仍未返回）。超时是最强的拥塞信号，
    /// 立即触发一次乘性降并发（不等节流间隔）。
    fn record_timeout(&self) {
        self.win_timeout.fetch_add(1, Ordering::Relaxed);
        self.maybe_adjust(true);
    }

    fn maybe_adjust(&self, force: bool) {
        // 取窗口样本并决定是否到调节时刻。
        let mut last = self.last_adjust.lock().expect("adjust lock poisoned");
        if !force && last.elapsed() < ADJUST_INTERVAL {
            return;
        }
        let ok = self.win_ok.swap(0, Ordering::Relaxed);
        let to = self.win_timeout.swap(0, Ordering::Relaxed);
        let ewma = self.ewma_us.load(Ordering::Relaxed);
        let total = ok + to;
        let timeout_rate = if total > 0 {
            to as f64 / total as f64
        } else {
            0.0
        };

        let mut g = self.gate.lock().expect("load gate poisoned");
        let old = g.limit;
        // 乘性减：窗口内有明显超时比例（≥12%）或执行延迟持续偏高 → 立刻减半（封底）。
        let congested = timeout_rate >= 0.12 || ewma >= LATENCY_BUSY_US;
        // 加性增：窗口零超时、延迟足够低且样本充分 → 每次 +2（封顶）。
        let idle = to == 0 && ewma <= LATENCY_IDLE_US && ok >= IDLE_MIN_SAMPLES;
        if congested {
            g.limit = MIN_CONCURRENCY.max(old / 2);
        } else if idle {
            g.limit = max_concurrency().min(old + 2);
        }
        *last = Instant::now();
        drop(g);
        // 提并发后唤醒可能在等名额的 worker；降并发不需要唤醒（在途任务做完自然受限）。
        if old < self.current_limit() {
            self.cv.notify_all();
        }
    }
}

/// 当前是否判定为磁盘 / 系统繁忙：有效并发已降到繁忙线及以下、EWMA 延迟偏高，
/// 或本调节窗口刚出现过超时。看门狗据此放宽预算、对本地卷给一次重试。
pub fn is_busy() -> bool {
    let l = load();
    l.current_limit() <= BUSY_CONCURRENCY
        || l.ewma_us.load(Ordering::Relaxed) >= LATENCY_BUSY_US
        || l.win_timeout.load(Ordering::Relaxed) > 0
}

/// 当前自适应有效并发（主要用于日志 / 诊断）。
pub fn current_concurrency() -> usize {
    load().current_limit()
}

/// 在自适应并发闸门内执行闭包 `f`，并把**执行段**耗时 / 超时反馈给负载信号。
///
/// 这是给 io_pool worker 调用的包装：先取名额（繁忙时阻塞等待，从而限制在途 IO），
/// 执行成功回报执行延迟；panic 不使 worker 死亡（由 io_pool 兜底），但释放名额。
pub(crate) fn gated_run<F>(f: F)
where
    F: FnOnce(),
{
    let l = load();
    l.acquire();
    let t0 = Instant::now();
    // catch_unwind 由 io_pool 负责；这里无论正常 / panic 都要释放名额。
    struct Guard<'a>(&'a Load);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let _g = Guard(l);
    f();
    l.record_ok(t0.elapsed().as_micros() as u64);
}

/// 由调用方在「等待任务结果超时」时上报一次超时（拥塞信号）。
pub(crate) fn note_wait_timeout() {
    load().record_timeout();
}
