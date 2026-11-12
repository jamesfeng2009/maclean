//! staging 残留的持久记录（GC 的权威层）。
//!
//! 背景：删除一个对象时，`ops::best_effort_delete_with_reason` 会先把它原子改名为
//! 同目录下的 `.maclean_stage_{pid}_{nanos:x}_{attempt}`，再删这个改名后的对象。
//! 进程一旦在这两步之间死掉（kill / 崩溃 / 断电），staging 目录就留在了被删项的
//! 原父目录里 —— 散落各处、名字隐藏、没有任何持久线索指向它。
//!
//! 本模块就是那条线索：`~/.maclean/staging.journal`（JSONL，一行一条）
//!
//! ```text
//! {"v":1,"ts":1699999999,"pid":1234,"path":"/Users/x/Library/Caches/.maclean_stage_1234_1a2b_0","dev":16777220,"ino":123456}
//! {"v":1,"ts":1700000001,"done":"/Users/x/Library/Caches/.maclean_stage_1234_1a2b_0"}
//! ```
//!
//! 设计约束（对齐 docs/PLAN-clean-crash-recovery.md §2.1）：
//! - **只在 rename 成功之后写**，写失败绝不影响删除本身；
//! - 行缓冲 + 每 [`FSYNC_EVERY`] 条或显式 [`flush`] 落一次 fsync —— 一次清理可能
//!   删上万项，逐条 fsync 会把删除拖慢一个量级；进程被杀时数据已在 kernel 里，
//!   真正会丢的只有「rename 后、写入前」和断电这两种极小窗口，由 Layer 2 兜底；
//! - App 内用一把 [`Mutex`] 串行写，跨进程（极少，双开）由 GC 的 pid 存活检查兜底。

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// journal 文件名（位于 `app_data_dir()`，即 macOS 的 `~/.maclean`）
pub const JOURNAL_FILE_NAME: &str = "staging.journal";

/// 每写多少条做一次 fsync（方案 §2.1：不为每个 inode 付一次 fsync）
pub const FSYNC_EVERY: usize = 200;

/// 方案 §4「journal 巨大」：超过这个行数，启动时强制截断（只保留未完成项）。
pub const MAX_JOURNAL_LINES: usize = 20_000;

/// 跨进程互斥用的**独立**锁文件。绝不能锁 journal 自身 —— 它会被压缩替换，
/// 锁会跟着旧 inode 走而失效。这个文件永不 rename。
pub const JOURNAL_LOCK_NAME: &str = "staging.journal.lock";

/// staging 目录名前缀
pub const STAGING_PREFIX: &str = ".maclean_stage_";

/// 当前行格式版本
pub const JOURNAL_VERSION: u8 = 1;

/// journal 的一行。开始记录带 `path/dev/ino/pid`，完成记录只带 `done`。
///
/// 所有字段都 `default` 且空值不落盘 —— 半行 / 未来版本多出的字段都能容错解析。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct JournalLine {
    #[serde(default)]
    pub v: u8,
    #[serde(default)]
    pub ts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// 开始记录的 staging 绝对路径
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// 开始记录：rename 之后对象的 st_dev
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<u64>,
    /// 开始记录：rename 之后对象的 st_ino
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ino: Option<u64>,
    /// 完成记录：同一条 `path` 的 staging 已被处理（删除成功 / 已不可达 / 主动放弃）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    /// 放弃原因（仅审计用，`done` 的补充说明）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

impl JournalLine {
    fn start(pid: u32, ts: u64, path: &str, dev: u64, ino: u64) -> Self {
        Self {
            v: JOURNAL_VERSION,
            ts,
            pid: Some(pid),
            path: Some(path.to_string()),
            dev: Some(dev),
            ino: Some(ino),
            done: None,
            why: None,
        }
    }

    /// 是否是「还没收尾」的开始记录
    pub fn is_open(&self) -> bool {
        self.path.is_some() && self.done.is_none()
    }
}

/// 解析出的 staging 目录名
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagingName {
    pub pid: u32,
    pub nanos: u64,
    pub attempt: u32,
}

/// 严格匹配 `.maclean_stage_{pid}_{nanos:x}_{attempt}`。
///
/// 严格是安全要求：宁可漏收，也不能把用户自己的同名文件当残留删掉。
/// 分段必须恰好 3 段、pid/attempt 全十进制数字、nanos 全小写十六进制。
pub fn parse_staging_name(name: &str) -> Option<StagingName> {
    let rest = name.strip_prefix(STAGING_PREFIX)?;
    let mut parts = rest.split('_');
    let pid_s = parts.next()?;
    let nanos_s = parts.next()?;
    let attempt_s = parts.next()?;
    if parts.next().is_some() {
        return None; // 多出的分段（如 .maclean_stage_1_2_3_4）
    }
    if pid_s.is_empty() || !pid_s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if nanos_s.is_empty()
        || !nanos_s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    if attempt_s.is_empty() || !attempt_s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(StagingName {
        pid: pid_s.parse().ok()?,
        nanos: u64::from_str_radix(nanos_s, 16).ok()?,
        attempt: attempt_s.parse().ok()?,
    })
}

/// journal 路径
pub fn journal_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(p) = test_override_path() {
            return p;
        }
    }
    crate::platform::app_data_dir().join(JOURNAL_FILE_NAME)
}

// 单测钩子必须是 **线程局部** 的：cargo test 默认多线程并行。若用进程级
// static，A 用例刚把路径指向自己的临时文件，B 用例（哪怕根本不想写 journal）
// 的删除就会写进 A 的 journal，出现「应该是 1 条却读到 5 条」的假失败。
#[cfg(test)]
thread_local! {
    static TEST_JOURNAL_PATH: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// 备份目录仍是进程级 static（无法线程隔离），触碰它的用例共用这把锁。
#[cfg(test)]
pub(crate) static TEST_FS_GUARD: Mutex<()> = Mutex::new(());

#[cfg(test)]
fn test_override_path() -> Option<PathBuf> {
    TEST_JOURNAL_PATH.with(|p| p.borrow().clone())
}

/// 单测里只有显式设了 journal 路径的用例才写盘。
///
/// 其余用例（删除链路的回归测试）只关心删除本身，不该顺手污染真实的
/// `~/.maclean/staging.journal`，也不该让它们被 journal 的 IO 拖慢。
#[cfg(test)]
fn journal_overridden() -> bool {
    test_override_path().is_some()
}

/// 单测钩子：把 journal 指向临时文件，绝不碰真实 `~/.maclean/staging.journal`。
#[cfg(test)]
pub(crate) fn set_test_journal_path(p: Option<PathBuf>) {
    drop_writer();
    TEST_JOURNAL_PATH.with(|slot| *slot.borrow_mut() = p);
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

struct Writer {
    path: PathBuf,
    file: File,
    since_sync: usize,
}

static WRITER: Mutex<Option<Writer>> = Mutex::new(None);

/// 跨进程锁的 fd 缓存（只在 unix 有 flock）。CLI 与 App 共用一个 journal，
/// 两个进程各持一把进程内 `WRITER` 锁，互不知情，必须再靠 flock 串行化。
#[cfg(unix)]
static LOCK_FILE: Mutex<Option<File>> = Mutex::new(None);

/// 在 flock 独占锁内执行 `f`。锁的是固定路径的 `.lock` 文件（永不 rename），
/// 所以 journal 被压缩时锁不会失效。
///
/// 拿不到锁文件 / flock 失败时降级为「不锁」继续执行：journal 是恢复线索，
/// 绝不能因为锁问题让删除本身失败或卡住。
#[cfg(unix)]
fn with_journal_lock<T>(f: impl FnOnce() -> T) -> T {
    use std::os::fd::AsRawFd;
    let mut slot = LOCK_FILE.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        let mut p = journal_path();
        p.set_file_name(JOURNAL_LOCK_NAME);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&p)
        {
            *slot = Some(file);
        }
    }
    if let Some(file) = slot.as_ref() {
        let fd = file.as_raw_fd();
        let locked = unsafe { libc::flock(fd, libc::LOCK_EX) } == 0;
        let out = f();
        if locked {
            unsafe {
                libc::flock(fd, libc::LOCK_UN);
            }
        }
        return out;
    }
    f()
}

/// Windows 没有 flock：进程内 `WRITER` 锁仍生效，跨进程降级为 O_APPEND +
/// 逐行容错（双开本就是方案里标注的极少场景）。
#[cfg(not(unix))]
fn with_journal_lock<T>(f: impl FnOnce() -> T) -> T {
    f()
}

fn lock_writer() -> std::sync::MutexGuard<'static, Option<Writer>> {
    // 写 journal 失败不该让删除失败；锁中毒时直接取回内部值继续。
    WRITER.lock().unwrap_or_else(|e| e.into_inner())
}

/// 测试切换 journal 路径时关掉旧 fd。生产路径的 compact 自己会在锁内
/// flush + 置空，不再需要这个 helper，故只在 cfg(test) 编译。
#[cfg(test)]
fn drop_writer() {
    let mut guard = lock_writer();
    if let Some(w) = guard.as_mut() {
        let _ = w.file.flush();
        let _ = w.file.sync_data();
    }
    *guard = None;
}

/// 追加一行。锁顺序固定为 **flock → WRITER**（compact 走同一顺序，不会死锁）。
fn append_line(line: &JournalLine) -> std::io::Result<()> {
    #[cfg(test)]
    if !journal_overridden() {
        return Ok(());
    }
    with_journal_lock(|| append_line_locked(line))
}

fn append_line_locked(line: &JournalLine) -> std::io::Result<()> {
    let path = journal_path();
    let text = serde_json::to_string(line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut guard = lock_writer();
    let need_open = match guard.as_ref() {
        Some(w) => w.path != path,
        None => true,
    };
    if need_open {
        if let Some(w) = guard.take() {
            drop(w);
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        *guard = Some(Writer {
            path,
            file,
            since_sync: 0,
        });
    }
    let w = match guard.as_mut() {
        Some(w) => w,
        None => return Err(std::io::Error::other("journal writer unavailable")),
    };
    w.file.write_all(text.as_bytes())?;
    w.file.write_all(b"\n")?;
    w.since_sync += 1;
    if w.since_sync >= FSYNC_EVERY {
        let _ = w.file.sync_data();
        w.since_sync = 0;
    }
    Ok(())
}

/// 记录「已改名为 staging，即将删除」。写失败只记 warn，不影响删除。
pub fn record_staged(path: &Path, dev: u64, ino: u64) {
    record_staged_by(std::process::id(), path, dev, ino);
}

/// 与 [`record_staged`] 相同，但显式指定「造它的 pid」。
///
/// 生产只有 `record_staged` 一条路径；这里单独存在是为了让 GC 的单测能伪造
/// 「某个早已退出的进程留下的记录」——否则记录里的 pid 永远是当前进程，
/// 会被「pid 仍存活」规则正确但无用地跳过。
pub(crate) fn record_staged_by(pid: u32, path: &Path, dev: u64, ino: u64) {
    let line = JournalLine::start(pid, now_secs(), &path.to_string_lossy(), dev, ino);
    if let Err(e) = append_line(&line) {
        crate::logger::warn(&format!(
            "[gc] staging journal 写入失败（删除继续）: {} ({e})",
            path.display()
        ));
    }
}

/// 记录「这条 staging 已收尾」。
pub fn record_done(path: &Path) {
    record_done_with_why(path, None);
}

/// 记录「这条 staging 已收尾」，并附原因（仅审计）。
pub fn record_done_with_why(path: &Path, why: Option<&str>) {
    let line = JournalLine {
        v: JOURNAL_VERSION,
        ts: now_secs(),
        done: Some(path.to_string_lossy().to_string()),
        why: why.map(|s| s.to_string()),
        ..Default::default()
    };
    if let Err(e) = append_line(&line) {
        crate::logger::warn(&format!(
            "[gc] staging journal done 记录写入失败: {} ({e})",
            path.display()
        ));
    }
}

/// 强制把缓冲区刷到磁盘（每批删除结束 / GC 结束时调用）。
pub fn flush() {
    #[cfg(test)]
    if !journal_overridden() {
        return;
    }
    let mut guard = lock_writer();
    if let Some(w) = guard.as_mut() {
        let _ = w.file.flush();
        let _ = w.file.sync_data();
        w.since_sync = 0;
    }
}

/// 容错逐行解析。返回 (有效行, 丢弃的坏行数)。
///
/// 半行（断电写了半截）只丢那一行，绝不让整个 journal 失效 ——
/// 丢掉的记录顶多让这条残留交给 Layer 2 的同目录扫描。
pub fn read_lines_with_stats() -> (Vec<JournalLine>, usize) {
    #[cfg(test)]
    if !journal_overridden() {
        return (Vec::new(), 0);
    }
    let path = journal_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return (Vec::new(), 0);
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut out = Vec::new();
    let mut bad = 0usize;
    for raw in text.lines() {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        match serde_json::from_str::<JournalLine>(t) {
            Ok(line) => out.push(line),
            Err(_) => bad += 1,
        }
    }
    (out, bad)
}

/// 读全部有效行（测试/诊断用）
pub fn read_lines() -> Vec<JournalLine> {
    read_lines_with_stats().0
}

/// 构建 `path → 最后状态`，返回所有还没收尾的开始记录（按 ts 升序）。
pub fn unfinished() -> Vec<JournalLine> {
    let (lines, bad) = read_lines_with_stats();
    if bad > 0 {
        crate::logger::warn(&format!("[gc] staging journal 有 {bad} 行损坏，已忽略"));
    }
    let mut latest: std::collections::HashMap<String, JournalLine> =
        std::collections::HashMap::new();
    for line in lines {
        if let Some(p) = line.path.as_ref() {
            latest.insert(p.clone(), line);
        } else if let Some(d) = line.done.as_ref() {
            latest.remove(d);
        }
    }
    let mut open: Vec<JournalLine> = latest.into_values().filter(|l| l.is_open()).collect();
    open.sort_by_key(|l| l.ts);
    open
}

/// 压缩 journal：只保留**此刻**仍未完成项。
///
/// 修 review 发现的两处并发丢记录：
/// 1. **不用调用方循环开始时算出的 `keep` 快照**。GC 后台跑的同时用户可能
///    正在清理，旧快照会把这期间新 append 的 start 记录一起抹掉。压缩时重新
///    读一遍文件即可 —— GC 每处理完一条都把 done 追加进文件了，重读就是最新
///    状态；跳过 / 失败的条目本就没写 done，自然还在。
/// 2. **原地截断重写，不再 tmp + rename**。rename 会换 inode：CLI 与 App 共用
///    同一 journal，另一个进程缓存的写 fd 会继续写进已 unlink 的孤儿文件，
///    之后整批记录凭空消失。JSONL 逐行容错，原地写最坏只丢一行。
pub fn compact() -> std::io::Result<()> {
    #[cfg(test)]
    if !journal_overridden() {
        return Ok(());
    }
    with_journal_lock(|| {
        {
            let mut guard = lock_writer();
            if let Some(w) = guard.as_mut() {
                let _ = w.file.flush();
                let _ = w.file.sync_data();
            }
            *guard = None;
        }
        // 重读当前文件，而不是用任何旧快照。
        let keep = unfinished();
        let mut body = String::new();
        for line in keep {
            // 落盘时统一写成「开始记录」，不带 done/why
            let start = JournalLine {
                v: JOURNAL_VERSION,
                ts: line.ts,
                pid: line.pid,
                path: line.path.clone(),
                dev: line.dev,
                ino: line.ino,
                done: None,
                why: None,
            };
            body.push_str(&serde_json::to_string(&start).map_err(std::io::Error::other)?);
            body.push('\n');
        }
        rewrite_in_place(&journal_path(), body.as_bytes())
    })
}

/// 原地截断 + 重写，保持 inode 不变（理由见 [`compact`]）。
fn rewrite_in_place(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(content)?;
    file.sync_all()
}

/// journal 当前行数（只数换行，不解析 JSON）。给 CLI 快速判断「有没有活要干」。
pub fn line_count() -> usize {
    #[cfg(test)]
    if !journal_overridden() {
        return 0;
    }
    std::fs::read(journal_path())
        .map(|b| b.iter().filter(|&&c| c == b'\n').count())
        .unwrap_or(0)
}

/// 取未完成记录，并落实方案 §4「journal 巨大 → 超过 20k 行强制截断（保留未完成项）」。
///
/// 截断放在这里而不是 `unfinished()` 内：`compact()` 自己会调 `unfinished()`，
/// 放里面就成了递归压缩。GC 与 CLI 两个入口都走本函数。
pub fn reconcile_open() -> Vec<JournalLine> {
    if line_count() > MAX_JOURNAL_LINES {
        crate::logger::warn(&format!(
            "[gc] staging journal 超过 {MAX_JOURNAL_LINES} 行，强制截断（保留未完成项）"
        ));
        if let Err(e) = compact() {
            crate::logger::warn(&format!("[gc] journal 超限时压缩失败: {e}"));
        }
    }
    unfinished()
}

/// 一批删除结束：落盘 + 压缩到只剩未完成项（方案 §6.4「正常清理一轮，
/// journal 最终为 0 行」）。删失败的项保留 open 记录，下次启动继续收。
pub fn compact_finished() {
    flush();
    if let Err(e) = compact() {
        crate::logger::warn(&format!("[gc] 批结束 journal 压缩失败: {e}"));
    }
}

/// pid 是否仍存活（`kill(pid, 0)`：ESRCH = 已退出；EPERM = 存在但无权限 → 仍算活）。
///
/// pid 0（kill 语义里表示「当前进程组」）一律按「不可判定」处理为不存活 ——
/// 但调用方另有 `pid == 当前进程` 的显式跳过，不会误删自己正在删的东西。
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    #[cfg(unix)]
    {
        let r = unsafe { libc::kill(pid as i32, 0) };
        if r == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(target_os = "windows")]
    {
        // 方案 §2.3 的第 2 道闸「造 staging 的进程还活着就别动」在 Windows 上
        // 曾被降级成恒 false —— 双开 App 时会误删另一实例正在删的对象。
        // 用 OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) 只需查询权限，
        // 不需要 PROCESS_ALL_ACCESS，句柄立即关掉。
        use std::ffi::c_void;

        #[link(name = "kernel32")]
        #[allow(non_snake_case)]
        extern "system" {
            fn OpenProcess(
                dwDesiredAccess: u32,
                bInheritHandle: i32,
                dwProcessId: u32,
            ) -> *mut c_void;
            fn CloseHandle(hObject: *mut c_void) -> i32;
        }
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        // SAFETY: 只查询句柄是否存在，不读写目标进程内存；返回前立即 CloseHandle。
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            CloseHandle(handle);
            true
        }
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!(
                "maclean-journal-{tag}-{}-{}",
                std::process::id(),
                now_secs()
            ));
            let _ = std::fs::create_dir_all(&d);
            Self(d)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn with_journal<T>(tag: &str, f: impl FnOnce(&Path) -> T) -> T {
        let _guard = TEST_FS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new(tag);
        let j = tmp.0.join(JOURNAL_FILE_NAME);
        set_test_journal_path(Some(j.clone()));
        let out = f(&j);
        drop_writer();
        set_test_journal_path(None);
        out
    }

    #[test]
    fn parses_only_strict_staging_names() {
        let ok = parse_staging_name(".maclean_stage_4242_1a2b3c_7").expect("合法名应解析成功");
        assert_eq!(ok.pid, 4242);
        assert_eq!(ok.nanos, 0x1a2b3c);
        assert_eq!(ok.attempt, 7);

        for bad in [
            ".maclean_stage_4242_1A2B_7",                 // 大写十六进制
            ".maclean_stage_4242_1a2b_7_x",               // 多一段
            ".maclean_stage__1a2b_7",                     // pid 空
            ".maclean_stage_4242__7",                     // nanos 空
            ".maclean_stage_4242_1a2b_",                  // attempt 空
            ".maclean_stage_4242_1a2b_-1",                // attempt 非数字
            ".maclean_stage-4242-1a2b-7",                 // 分隔符不对
            "maclean_stage_4242_1a2b_7",                  // 少了前导点
            ".maclean_stage_4242_1a2b_7 (copy)",          // 尾部多余内容
            ".maclean_stage_99999999999999999999_1a2b_7", // pid 溢出
        ] {
            assert!(parse_staging_name(bad).is_none(), "不该被当成残留: {bad}");
        }
    }

    #[test]
    fn journal_records_start_then_done_and_compacts_to_empty() {
        with_journal("lifecycle", |path| {
            let p = PathBuf::from("/Users/u/Caches/.maclean_stage_1_aa_0");
            record_staged(&p, 16777220, 123456);
            flush();
            assert_eq!(unfinished().len(), 1, "开始记录应出现在未完成项里");

            record_done(&p);
            flush();
            assert!(unfinished().is_empty(), "done 之后不该还有未完成项");

            compact().expect("压缩应成功");
            let lines = read_lines();
            assert_eq!(lines.len(), 0, "全部收尾后 journal 应为 0 行");
            assert!(path.exists(), "压缩只替换文件，不删文件本身");
        });
    }

    #[test]
    fn done_for_unknown_path_is_ignored_and_bad_lines_are_dropped() {
        with_journal("tolerant", |path| {
            let mut body = String::new();
            body.push_str("{\"v\":1,\"ts\":1,\"pid\":2,\"path\":\"/a/.maclean_stage_2_ff_0\",\"dev\":1,\"ino\":2}\n");
            body.push_str("{\"v\":1,\"ts\":2,\"done\":\"/a/.maclean_stage_2_ff_0\"}\n");
            body.push_str("{\"v\":1,\"ts\":3,\"pid\":2,\"path\":\"/b/"); // 断电半行
            body.push('\n');
            body.push_str("{\"v\":1,\"ts\":4,\"done\":\"/never/staged/.maclean_stage_9_ff_0\"}\n");
            std::fs::write(path, body).unwrap();

            let (lines, bad) = read_lines_with_stats();
            assert_eq!(bad, 1, "半行应被记为一处损坏");
            assert_eq!(lines.len(), 3);
            assert!(
                unfinished().is_empty(),
                "已 done 的与未知 done 都不算未完成"
            );
        });
    }

    #[test]
    fn compaction_keeps_only_the_open_entries() {
        with_journal("compact", |path| {
            let a = PathBuf::from("/a/.maclean_stage_1_aa_0");
            let b = PathBuf::from("/b/.maclean_stage_2_bb_0");
            record_staged(&a, 10, 11);
            record_staged(&b, 12, 13);
            record_done(&a);
            flush();

            let keep = unfinished();
            assert_eq!(keep.len(), 1);
            compact().expect("压缩应成功");
            flush();

            let lines = read_lines();
            assert_eq!(lines.len(), 1, "压缩后只剩未完成项");
            assert_eq!(lines[0].path.as_deref(), Some(b.to_string_lossy().as_ref()));
            assert_eq!(lines[0].dev, Some(12));
            assert_eq!(lines[0].ino, Some(13));
            assert!(path.exists());
        });
    }

    #[test]
    fn missing_journal_reads_as_empty_without_error() {
        with_journal("missing", |path| {
            assert!(!path.exists());
            assert!(read_lines().is_empty());
            assert!(unfinished().is_empty());
        });
    }

    /// 反向验证：把 `compact()` 改回「用调用方循环开始时算出的 keep 快照」，
    /// 这条必须变红 —— GC 后台跑时用户新写的 start 记录会被整批抹掉。
    #[test]
    fn compaction_preserves_entries_appended_after_caller_snapshot() {
        with_journal("compact-race", |_| {
            let during_gc = PathBuf::from("/a/.maclean_stage_1_aa_0");
            let user_new = PathBuf::from("/b/.maclean_stage_2_bb_0");
            record_staged_by(9, &during_gc, 1, 1);
            flush();

            // 模拟 GC 循环开始：读一次未完成项快照
            let snapshot = unfinished();
            assert_eq!(snapshot.len(), 1);

            // GC 运行期间用户又发起清理 → 新的 start 落盘
            record_staged_by(9, &user_new, 2, 2);
            flush();

            // GC 结束压缩（若用 snapshot，user_new 就丢了）
            compact().expect("压缩应成功");
            let kept: Vec<String> = unfinished().into_iter().filter_map(|l| l.path).collect();
            assert!(
                kept.contains(&during_gc.to_string_lossy().to_string()),
                "跳过/待重试项必须保留: {kept:?}"
            );
            assert!(
                kept.contains(&user_new.to_string_lossy().to_string()),
                "压缩期间新写入的记录绝不能被抹掉: {kept:?}"
            );
        });
    }

    /// tmp + rename 会换 inode：CLI 与 App 共用一个 journal，另一进程缓存的
    /// 写 fd 会继续写进已 unlink 的孤儿文件。改回 rename 这条必须变红。
    #[cfg(unix)]
    #[test]
    fn compaction_keeps_inode_so_other_process_fd_still_writes_live_file() {
        use std::os::unix::fs::MetadataExt;

        with_journal("compact-inode", |path| {
            let p = PathBuf::from("/x/.maclean_stage_1_aa_0");
            record_staged_by(9, &p, 1, 1);
            flush();

            let ino_before = std::fs::metadata(path).unwrap().ino();
            // 模拟另一个进程（CLI/App）在压缩前已打开并持有 append fd
            let mut other = OpenOptions::new().append(true).open(path).unwrap();

            compact().expect("压缩应成功");
            assert_eq!(
                std::fs::metadata(path).unwrap().ino(),
                ino_before,
                "压缩不能换 inode，否则并发进程的写会落进孤儿文件"
            );

            other
                .write_all(
                    b"{\"v\":1,\"ts\":9,\"pid\":9,\"path\":\"/y/.maclean_stage_9_cc_0\",\"dev\":1,\"ino\":1}\n",
                )
                .unwrap();
            other.flush().unwrap();
            let open = unfinished();
            assert_eq!(open.len(), 2, "旧 fd 写入的记录必须还在活文件里: {open:?}");
        });
    }

    /// 方案 §4：journal 巨大 → 强制截断，但**未完成项一条都不能丢**。
    #[test]
    fn reconcile_open_truncates_oversized_journal_but_keeps_open_entries() {
        with_journal("oversized", |path| {
            let mut body = String::new();
            // 一半的「开始 + 完成」配对就足够超过 20k 行上限
            for i in 0..(MAX_JOURNAL_LINES / 2 + 5) {
                let p = format!("/done/.maclean_stage_1_{i:x}_0");
                body.push_str(&format!(
                    "{{\"v\":1,\"ts\":1,\"pid\":9,\"path\":\"{p}\",\"dev\":1,\"ino\":1}}\n"
                ));
                body.push_str(&format!("{{\"v\":1,\"ts\":2,\"done\":\"{p}\"}}\n"));
            }
            let keep_path = "/open/.maclean_stage_9_ff_0";
            body.push_str(&format!(
                "{{\"v\":1,\"ts\":3,\"pid\":9,\"path\":\"{keep_path}\",\"dev\":1,\"ino\":1}}\n"
            ));
            std::fs::write(path, body).unwrap();
            assert!(line_count() > MAX_JOURNAL_LINES, "夹具必须真的超限");

            let open = reconcile_open();
            assert_eq!(open.len(), 1, "未完成项不能被截断掉");
            assert_eq!(open[0].path.as_deref(), Some(keep_path));
            assert!(
                line_count() <= MAX_JOURNAL_LINES,
                "超限 journal 必须被压回上限内"
            );
            assert_eq!(line_count(), 1, "压缩后只该剩未完成项");
        });
    }

    /// 方案 §6.4：正常清理一轮（全部 done）后 journal 最终为 0 行。
    #[test]
    fn compact_finished_leaves_zero_lines_after_a_clean_batch() {
        with_journal("batch-zero", |path| {
            let p = PathBuf::from("/a/.maclean_stage_1_aa_0");
            record_staged_by(9, &p, 1, 1);
            record_done(&p);
            flush();
            assert_eq!(line_count(), 2, "夹具：一轮删除写 start + done 两行");

            compact_finished();
            assert_eq!(line_count(), 0, "批结束后 journal 必须归零");
            assert!(path.exists(), "压缩只重写文件，不删文件本身");
        });
    }

    #[test]
    fn pid_alive_detects_current_process_and_nonsense_pids() {
        assert!(pid_alive(std::process::id()), "当前进程必须判为存活");
        assert!(!pid_alive(0), "pid 0 不可判定，按不存活处理");
        assert!(
            !pid_alive(u32::MAX),
            "溢出 i32 的 pid 不可判定，按不存活处理"
        );
    }
}
