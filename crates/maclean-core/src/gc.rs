//! 启动收尾（GC）：把上次删除中断留下的 staging 残留清掉。
//!
//! 方案见 `docs/PLAN-clean-crash-recovery.md` §2.3。两条承接线索：
//!
//! 1. **权威层** —— [`crate::staging_journal`] 里「改了名但没删完」的记录；
//! 2. **兜底层** —— [`crate::ops::sweep_stale_staging`] 在下次删除同父目录时按
//!    名字格式 + 死 pid + 60s 年龄补收（覆盖「rename 成功、journal 未落盘」窗口）。
//!
//! 硬约束：本模块的每一处失败都只能「记日志 + 留待下次重试」，
//! 绝不允许 panic、绝不允许因为收尾失败影响启动。
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ops::remove_staging_tree;
use crate::staging_journal::{self, JournalLine};

/// 收尾总预算（方案 §2.3 第 8 步）。
///
/// 超预算后不再**新开**一条删除，已开始的那条照常跑完：既不把启动拖成
/// 无上限，也不会留下删到一半的对象。
pub const GC_TOTAL_BUDGET: Duration = Duration::from_secs(30);

/// 本轮允许的总预算。
///
/// 只有 debug 构建会读 `MACLEAN_TEST_GC_BUDGET_MS`（毫秒）；release 产物永远
/// 返回 [`GC_TOTAL_BUDGET`]，环境变量不产生任何行为变化。`0` 是明确的
/// 「预算已耗尽」哨兵，让超时分支可被确定性测试覆盖，不必空跑 30 秒。
fn budget_for_this_run() -> Duration {
    #[cfg(debug_assertions)]
    {
        if let Ok(raw) = std::env::var("MACLEAN_TEST_GC_BUDGET_MS") {
            if let Ok(ms) = raw.parse::<u64>() {
                return Duration::from_millis(ms);
            }
        }
    }
    GC_TOTAL_BUDGET
}

/// 是否已超出本轮预算。抽成纯函数：超时分支不必靠 30 秒长跑夹具验证。
fn out_of_budget(started: Instant, budget: Duration) -> bool {
    budget.is_zero() || started.elapsed() > budget
}

/// 启动收尾的结果，同时也是 `cleanup-recovered` 事件与 `gc_status()` 的载荷。
///
/// 字段与方案 §3 一致：`cleaned` = 真删掉的条数，`failed` = 这次没删成功的
/// 条数（已留在 journal 里等下次），`bytes` = `statvfs` 前后差值实测的回收量
/// （不是条目自报大小），`took_ms` = 本轮耗时。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GcStatus {
    pub running: bool,
    pub cleaned: u64,
    pub failed: u64,
    pub bytes: u64,
    pub took_ms: u64,
}

static STATUS: Mutex<Option<GcStatus>> = Mutex::new(None);

/// 只允许一个收尾实例在跑（与 `CLEAN_IN_FLIGHT` 完全分离，互不阻塞用户清理）。
static GC_LOCK: Mutex<()> = Mutex::new(());

/// 当前/最近一轮收尾状态。前端挂载时读它决定是否显示「收尾中…」。
pub fn status() -> GcStatus {
    STATUS
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

fn set_status(s: GcStatus) {
    if let Ok(mut g) = STATUS.lock() {
        *g = Some(s);
    }
}

/// 执行一轮启动收尾（同步、阻塞调用线程）。
///
/// 前端在 `setup()` 里通过 [`spawn_startup_cleanup`] 后台跑，不阻塞 UI。
#[must_use]
pub fn run_startup_cleanup() -> GcStatus {
    // 已有实例在跑：不重复开工，直接回报当前状态。
    let guard = match GC_LOCK.try_lock() {
        Ok(g) => g,
        Err(_) => return status(),
    };

    let started = Instant::now();
    let mut running = status();
    running.running = true;
    set_status(running);

    let mut cleaned = 0u64;
    let mut failed = 0u64;
    let mut bytes = 0u64;
    let mut keep: Vec<JournalLine> = Vec::new();

    // 读 journal 时顺带落实方案 §4 的 20k 行强制截断。
    let open = staging_journal::reconcile_open();
    if !open.is_empty() {
        crate::logger::info(&format!(
            "[gc] 启动收尾开始：{} 条未完成 staging 记录",
            open.len()
        ));
    }

    let budget = budget_for_this_run();
    for rec in open {
        if out_of_budget(started, budget) {
            // 超预算：这条留给下次启动，记录原样保留。
            keep.push(rec);
            continue;
        }
        let Some(pstr) = rec.path.clone() else {
            continue; // 没有 path 的行不是开始记录
        };
        let path = PathBuf::from(&pstr);

        // 1. 名字必须严格是 staging 格式 —— 防误删用户同名文件的第一道闸。
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            abandon(&path, "bad_name");
            continue;
        };
        let Some(parsed) = staging_journal::parse_staging_name(name) else {
            crate::logger::warn(&format!(
                "[gc] 记录里的名字不符合 staging 格式，跳过不删: {pstr}"
            ));
            abandon(&path, "bad_name");
            continue;
        };

        // 2. 造它的进程还活着（双开 / 另一实例正在删）→ 本轮不动。
        let pid = rec.pid.unwrap_or(parsed.pid);
        if pid == std::process::id() {
            keep.push(rec);
            continue;
        }
        if staging_journal::pid_alive(pid) {
            crate::logger::info(&format!(
                "[gc] staging 属于仍存活的进程 pid={pid}，本轮跳过: {pstr}"
            ));
            keep.push(rec);
            continue;
        }

        // 3. 不跟随符号链接地看目标。
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 卷被拔掉 / 已被并发清掉：目标已不可达，没有可回收的东西，
                // 标 done 免得每次启动都重试（方案 §4）。
                staging_journal::record_done_with_why(&path, Some("gone"));
                continue;
            }
            Err(e) => {
                crate::logger::warn(&format!(
                    "[gc] 读取 staging 元数据失败，保留记录待下次重试: {pstr} ({e})"
                ));
                failed += 1;
                keep.push(rec);
                continue;
            }
        };

        // 4. (dev, ino) 必须与记录一致；不一致说明名字被复用 / 对象被换过。
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Some(dev), Some(ino)) = (rec.dev, rec.ino) {
                if (dev != 0 || ino != 0) && (meta.dev() != dev || meta.ino() != ino) {
                    crate::logger::warn(&format!(
                        "[gc] staging 物理身份与记录不一致（记录 {dev}/{ino}，实际 {}/{}），\
                         防误删直接放弃: {pstr}",
                        meta.dev(),
                        meta.ino()
                    ));
                    // 放弃这条记录：对象已不是当初改名的那一个，再挂着重试只会
                    // 每次启动都刷一条 warn。真正的残留交给 Layer 2 同目录扫描。
                    abandon(&path, "identity_mismatch");
                    continue;
                }
            }
        }
        #[cfg(not(unix))]
        {
            // Windows 拿不到跨重启稳定的 (dev, ino)：防误删降级为
            // 「名字严格格式 + pid 已退出」，meta 仅用于确认对象仍然存在。
            let _ = &meta;
        }

        // 5. 删。statvfs 前后差值才是「真实回收字节」。
        let before = volume_available(&path);
        match remove_staging_tree(&path, &path) {
            Ok(()) => {
                cleaned += 1;
                if let (Some((d0, a0)), Some((d1, a1))) = (before, volume_available(&path)) {
                    if d0 == d1 {
                        bytes = bytes.saturating_add(a1.saturating_sub(a0));
                    }
                }
                staging_journal::record_done(&path);
                crate::logger::info(&format!("[gc] 已收尾残留 staging: {pstr}"));
            }
            Err(f) => {
                failed += 1;
                crate::logger::warn(&format!(
                    "[gc] 收尾失败（保留记录，下次启动重试）: {pstr} ({})",
                    f.label_key()
                ));
                keep.push(rec);
            }
        }
    }

    // 压缩：只保留此刻仍未完成项。内部会重读 journal，不能用循环开始时
    // 的快照（GC 期间用户可能正在删除，旧快照会抹掉新写入的记录）。
    if let Err(e) = staging_journal::compact() {
        crate::logger::warn(&format!("[gc] journal 压缩失败（原文件保持可用）: {e}"));
    }
    staging_journal::flush();

    // `.pending.json` 对账：只改名留证，不删（审计证据）。
    let marked = reconcile_pending();
    if marked > 0 {
        crate::logger::info(&format!(
            "[gc] 对账完成：{} 份中断待删快照已标记为 .interrupted.json",
            marked
        ));
    }

    let next_retry = keep.len() as u64;
    let took_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let st = GcStatus {
        running: false,
        cleaned,
        failed,
        bytes,
        took_ms,
    };
    set_status(st.clone());
    drop(guard);
    if cleaned > 0 || failed > 0 {
        crate::logger::info(&format!(
            "[gc] 收尾完成：清理 {cleaned} 条，失败 {failed} 条，回收 {bytes} 字节，\
             耗时 {took_ms}ms，待重试 {next_retry} 条"
        ));
    }
    st
}

/// 后台跑一轮收尾，结束后把结果交给回调（pro 侧用它发 `cleanup-recovered`）。
pub fn spawn_startup_cleanup<F>(on_finished: F) -> std::thread::JoinHandle<()>
where
    F: FnOnce(GcStatus) + Send + 'static,
{
    std::thread::spawn(move || {
        let st = run_startup_cleanup();
        on_finished(st);
    })
}

/// 短生命周期进程（CLI）的收尾入口：在命令入口**同步**调用一次。
///
/// 与 GUI 不同，CLI 不能 spawn 后台线程 —— 命令跑完进程就退出，线程可能
/// 根本没执行完。业内通用形态是「下次启动同步做一次幂等 recovery」：
/// n8n CLI 启动时检查 crash.journal、dpkg 的 `--configure -a`、LevelDB
/// 打开 DB 时 replay WAL，都是这个模式。
///
/// 快速路径：journal 为空时只做 `.pending.json` 对账，不重写任何文件，
/// 所以 `maclean scan` 这类只读命令的开销近似为一次小文件读。
pub fn recover_if_needed() -> GcStatus {
    if staging_journal::line_count() == 0 {
        reconcile_pending();
        return GcStatus::default();
    }
    run_startup_cleanup()
}

/// `.pending.json` 对账。单测走显式目录，不碰（也不依赖）进程级备份目录钩子。
fn reconcile_pending() -> usize {
    #[cfg(test)]
    {
        if let Some(dir) = test_pending_dir() {
            return crate::backup::mark_interrupted_pending_in(&dir);
        }
    }
    crate::backup::mark_interrupted_pending()
}

#[cfg(test)]
thread_local! {
    static TEST_PENDING_DIR: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn test_pending_dir() -> Option<PathBuf> {
    TEST_PENDING_DIR.with(|d| d.borrow().clone())
}

/// 放弃一条记录（对象已不是我们改名的那个 / 名字不合法）。
fn abandon(path: &Path, why: &str) {
    staging_journal::record_done_with_why(path, Some(why));
}

/// 卷可用字节：(st_dev, available)。取不到时返回 None（不猜、不编数字）。
#[cfg(unix)]
fn volume_available(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    // 用父目录探测：目标可能刚被删掉，父目录还在同一个卷上。
    let probe = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(path);
    let c = CString::new(probe.as_os_str().as_bytes()).ok()?;
    let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut buf) } != 0 {
        return None;
    }
    let frsize = if buf.f_frsize > 0 {
        buf.f_frsize as u64
    } else {
        buf.f_bsize as u64
    };
    let dev = std::fs::metadata(probe).ok()?.dev();
    Some((dev, (buf.f_bavail as u64).saturating_mul(frsize)))
}

#[cfg(not(unix))]
fn volume_available(_path: &Path) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::staging_journal::{
        read_lines, record_staged_by, set_test_journal_path, TEST_FS_GUARD,
    };
    use std::sync::MutexGuard;

    /// gc 的单测共用全局 test 钩子（journal 路径 / 备份目录），必须串行跑。
    struct Fixture {
        root: PathBuf,
        _guard: MutexGuard<'static, ()>,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let root = std::env::temp_dir()
                .join(format!("maclean-gc-{tag}-{}-{nonce}", std::process::id()));
            std::fs::create_dir_all(root.join("work")).unwrap();
            std::fs::create_dir_all(root.join("backups")).unwrap();
            let guard = TEST_FS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            set_test_journal_path(Some(root.join("staging.journal")));
            TEST_PENDING_DIR.with(|d| *d.borrow_mut() = Some(root.join("backups")));
            Self {
                root,
                _guard: guard,
            }
        }

        /// 造一个 staging 残留（目录 + 一个 64KB 文件），返回路径。
        fn staged_dir(&self, name: &str) -> PathBuf {
            let p = self.root.join("work").join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("payload.bin"), vec![0u8; 64 * 1024]).unwrap();
            p
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            set_test_journal_path(None);
            TEST_PENDING_DIR.with(|d| *d.borrow_mut() = None);
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn dev_ino(p: &Path) -> (u64, u64) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::symlink_metadata(p).unwrap();
            (m.dev(), m.ino())
        }
        #[cfg(not(unix))]
        {
            let _ = p;
            (0, 0)
        }
    }

    /// macOS 的 PID_MAX 是 99998，999999 一定不存在 → 稳定地「死进程」。
    const DEAD_PID: u32 = 999_999;

    #[test]
    fn cleans_forged_staging_and_leaves_journal_empty() {
        let f = Fixture::new("forged");
        let staging = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_abc_0"));
        let (dev, ino) = dev_ino(&staging);
        record_staged_by(DEAD_PID, &staging, dev, ino);
        staging_journal::flush();

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 1, "伪造的残留必须被收掉");
        assert_eq!(st.failed, 0);
        assert!(!st.running);
        assert!(staging.symlink_metadata().is_err(), "残留目录应已消失");
        assert!(
            read_lines().is_empty(),
            "全部收尾后 journal 应被压缩成 0 行（done 行一并清掉）"
        );
    }

    /// 物理身份校验只在 unix 实现（Windows 没有跨重启稳定的 dev/ino）。
    /// 不加 cfg 的话 Windows runner 上会「真删掉再断言没删」，必然变红；
    /// Windows 侧的防误删由「名字严格格式 + pid 已退出」两道闸承担。
    #[cfg(unix)]
    #[test]
    fn keeps_staging_when_physical_identity_differs() {
        let f = Fixture::new("identity");
        let staging = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_bbb_1"));
        let (dev, ino) = dev_ino(&staging);
        // 记录里的 inode 与真实对象不一致：名字被复用 / 对象被换过 → 防误删
        record_staged_by(DEAD_PID, &staging, dev, ino.saturating_add(1));
        staging_journal::flush();

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 0, "身份不符绝不能删");
        assert_eq!(st.failed, 0, "放弃不是失败，不该计入失败数");
        assert!(
            staging.symlink_metadata().is_ok(),
            "身份不符的目录必须原地保留"
        );
        assert!(
            read_lines().is_empty(),
            "放弃的记录也要从 journal 清掉，避免每次启动重复 warn"
        );
    }

    #[test]
    fn skips_staging_owned_by_live_process_and_keeps_record() {
        let f = Fixture::new("alive");
        let me = std::process::id();
        let staging = f.staged_dir(&format!(".maclean_stage_{me}_ccc_0"));
        let (dev, ino) = dev_ino(&staging);
        record_staged_by(std::process::id(), &staging, dev, ino); // pid = 当前进程
        staging_journal::flush();

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 0);
        assert!(
            staging.symlink_metadata().is_ok(),
            "活进程的 staging 不能动"
        );
        let open = staging_journal::unfinished();
        assert_eq!(open.len(), 1, "跳过项必须留在 journal 里，下次再收");
        assert_eq!(
            open[0].path.as_deref(),
            Some(staging.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn marks_missing_path_done_without_counting_as_cleaned() {
        let f = Fixture::new("gone");
        let gone = f.root.join("work").join(".maclean_stage_99999_ddd_2");
        record_staged_by(DEAD_PID, &gone, 1, 2);
        staging_journal::flush();

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 0);
        assert_eq!(st.failed, 0);
        assert!(
            read_lines().is_empty(),
            "路径不存在 → 标 done（卷已不可达），不该无限重试"
        );
    }

    #[test]
    fn never_deletes_a_name_that_does_not_match_the_strict_format() {
        let f = Fixture::new("badname");
        let user_file = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_ABC_0")); // 大写十六进制
        let (dev, ino) = dev_ino(&user_file);
        record_staged_by(DEAD_PID, &user_file, dev, ino);
        staging_journal::flush();

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 0, "名字不严格匹配的一律不动");
        assert!(user_file.symlink_metadata().is_ok());
    }

    #[test]
    fn pending_snapshot_is_marked_not_deleted() {
        let f = Fixture::new("pending");
        let pending = f.root.join("backups").join("20260101-120000.pending.json");
        std::fs::write(&pending, "{\"id\":\"20260101-120000\"}").unwrap();

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 0);
        assert!(!pending.exists(), "pending 应被改名");
        assert!(
            f.root
                .join("backups")
                .join("20260101-120000.interrupted.json")
                .exists(),
            "审计证据必须保留（只改名不删）"
        );
        // 幂等：第二轮不会再重复处理
        let again = run_startup_cleanup();
        assert_eq!(again.cleaned, 0);
    }

    /// 表格「防误删」：staging 名指向一个符号链接时，只能删链接本身，
    /// 绝不能跟随它把用户目录里的内容一起删掉。
    #[cfg(unix)]
    #[test]
    fn symlink_staging_is_removed_without_touching_its_target() {
        let f = Fixture::new("symlink");
        let target = f.root.join("work").join("user_data");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), b"important").unwrap();

        let link = f
            .root
            .join("work")
            .join(format!(".maclean_stage_{DEAD_PID}_abc_0"));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (dev, ino) = dev_ino(&link); // symlink_metadata → 链接自身的身份
        record_staged_by(DEAD_PID, &link, dev, ino);
        staging_journal::flush();

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 1, "链接本身应被收掉");
        assert!(link.symlink_metadata().is_err(), "链接应已消失");
        assert!(
            target.join("keep.txt").exists(),
            "绝不能跟随符号链接删到用户数据"
        );
    }

    /// 表格「防误删」：pid 存活判定以 journal 记录为准，不能被名字里的
    /// 死 pid 骗过去（pid 复用 / 伪造名字场景）。
    #[test]
    fn journal_pid_not_name_pid_decides_liveness() {
        let f = Fixture::new("pid-source");
        let staging = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_aaa_0"));
        let (dev, ino) = dev_ino(&staging);
        record_staged_by(std::process::id(), &staging, dev, ino);
        staging_journal::flush();

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 0, "记录里的 pid 还活着，名字里的死 pid 不算数");
        assert!(staging.symlink_metadata().is_ok());
        assert_eq!(staging_journal::unfinished().len(), 1, "记录留待下次");
    }

    /// 表格「防误删」：用户手工造的同名目录，只要 journal 没有线索就不碰
    /// （方案明确不做全盘扫描）。
    #[test]
    fn gc_never_touches_a_lookalike_without_a_journal_record() {
        let f = Fixture::new("no-record");
        let lookalike = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_abc_0"));
        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 0);
        assert!(lookalike.exists(), "GC 只按 journal 行动，不猜名字");
    }

    /// 表格「容错边界」：外置卷拔掉时 GC 会按方案标 done，线索随之消失；
    /// 卷重新插回后残留「复活」，必须仍能被 Layer 2 同目录扫描收掉，
    /// 否则就成了永久残留。
    #[cfg(target_os = "macos")]
    #[test]
    fn reappearing_volume_leftover_is_still_collectable_by_layer2() {
        let f = Fixture::new("volume");
        let name = format!(".maclean_stage_{DEAD_PID}_ccc_0");
        let absent = f.root.join("work").join(&name);
        record_staged_by(DEAD_PID, &absent, 1, 2);
        staging_journal::flush();

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 0, "路径不存在不算真回收");
        assert!(read_lines().is_empty(), "已标 done 并压缩，权威线索消失");

        // 卷重新挂载：残留重新出现
        let reborn = f.staged_dir(&name);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
        std::fs::File::open(&reborn)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let trigger = f.root.join("work").join("trigger.bin");
        std::fs::write(&trigger, b"x").unwrap();
        crate::ops::best_effort_delete_with_reason(&trigger).unwrap();
        assert!(!reborn.exists(), "Layer 2 必须能收掉重新出现的残留");
    }

    /// 表格「并发」：GC 后台跑的同时用户发起清理，两把锁互不阻塞，
    /// 且 GC 的压缩不会把新写入的记录抹掉（精确断言见
    /// `staging_journal::tests::compaction_preserves_entries_appended_after_caller_snapshot`）。
    #[test]
    fn gc_and_a_concurrent_delete_do_not_block_or_panic() {
        let f = Fixture::new("concurrent");
        for i in 0..8 {
            let p = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_{i:x}_0"));
            let (d, n) = dev_ino(&p);
            record_staged_by(DEAD_PID, &p, d, n);
        }
        staging_journal::flush();

        let work = f.root.join("work");
        let handle = std::thread::spawn(move || {
            for i in 0..8 {
                let victim = work.join(format!("victim-{i}.bin"));
                std::fs::write(&victim, b"x").unwrap();
                let _ = crate::ops::best_effort_delete_with_reason(&victim);
            }
        });

        let st = run_startup_cleanup();
        handle.join().expect("并发删除线程不能 panic");
        assert!(!st.running, "GC 结束后 running 必须复位");
        assert!(!status().running);
    }

    /// 表格「权限失败路径 · chflags immutable」。macOS 上 `uchg` 普通用户就能
    /// 给自己造的目录设置，`/bin/rm -rf` 会返回 EPERM —— 这类场景不需要 sudo，
    /// 所以直接进 CI。GC 必须软失败：failed+1、记录留在 journal、对象保留。
    #[cfg(target_os = "macos")]
    #[test]
    fn immutable_staging_fails_softly_and_keeps_record() {
        use std::process::Command;

        let f = Fixture::new("immutable");
        let staging = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_fff_0"));
        let (dev, ino) = dev_ino(&staging);
        record_staged_by(DEAD_PID, &staging, dev, ino);
        staging_journal::flush();

        let locked = Command::new("chflags")
            .arg("uchg")
            .arg(&staging)
            .status()
            .is_ok_and(|s| s.success());
        if !locked {
            eprintln!("SKIP: 当前文件系统不支持 chflags uchg，权限失败路径未执行");
            return;
        }
        // RAII：断言 panic 时也要解锁，否则 Fixture::drop 清不掉临时目录。
        struct Unlock(PathBuf);
        impl Drop for Unlock {
            fn drop(&mut self) {
                let _ = std::process::Command::new("chflags")
                    .arg("nouchg")
                    .arg(&self.0)
                    .status();
            }
        }
        let _unlock = Unlock(staging.clone());

        let st = run_startup_cleanup();
        assert_eq!(st.cleaned, 0, "删不掉绝不能算清理成功");
        assert_eq!(st.failed, 1, "权限失败必须计入 failed，供日志页展示");
        assert!(!st.running);
        assert!(staging.symlink_metadata().is_ok(), "对象必须原地保留");
        assert_eq!(
            staging_journal::unfinished().len(),
            1,
            "失败记录必须留在 journal 里等下次启动重试"
        );
    }

    /// 表格「权限失败路径 · root 拥有项」的无 sudo 代理：父目录只给读+执行
    /// 权限时，非 root 用户删不掉其中的条目（EACCES），失败语义与 root 拥有
    /// 项一致。真正 `chown root` 的场景见 `scripts/verify-gc-real-machine.sh`。
    #[cfg(unix)]
    #[test]
    fn unwritable_parent_fails_softly_and_keeps_record() {
        use std::os::unix::fs::PermissionsExt;

        let f = Fixture::new("unwritable");
        let staging = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_aaa_1"));
        let (dev, ino) = dev_ino(&staging);
        record_staged_by(DEAD_PID, &staging, dev, ino);
        staging_journal::flush();

        let parent = f.root.join("work");
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        let _restore = Restore(parent);

        let st = run_startup_cleanup();
        assert_eq!(st.failed, 1, "EACCES 必须是一次软失败");
        assert_eq!(st.cleaned, 0);
        assert!(staging.symlink_metadata().is_ok());
        assert_eq!(staging_journal::unfinished().len(), 1);
    }

    /// 表格「容错边界 · 30s 预算」：预算边界是纯函数，0 是明确的耗尽哨兵；
    /// 未注入时必须严格等于方案规定的 30s，注入后按毫秒解析。
    #[test]
    fn budget_boundary_is_pure_and_injectable() {
        let _g = TEST_FS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        assert!(out_of_budget(now, Duration::ZERO), "0 必须是耗尽哨兵");
        assert!(!out_of_budget(now, GC_TOTAL_BUDGET), "刚起跑不该超预算");
        assert_eq!(budget_for_this_run(), GC_TOTAL_BUDGET, "默认必须 30s");

        std::env::set_var("MACLEAN_TEST_GC_BUDGET_MS", "1500");
        assert_eq!(budget_for_this_run(), Duration::from_millis(1500));
        std::env::remove_var("MACLEAN_TEST_GC_BUDGET_MS");
        assert_eq!(budget_for_this_run(), GC_TOTAL_BUDGET);
    }

    /// 表格「容错边界 · 30s 预算」：预算耗尽时不再新开删除，未处理项必须
    /// 原样留在 journal 里，且下一轮预算恢复后能真正收掉（不是"测了跳过就完"）。
    #[test]
    fn exhausted_budget_defers_every_record_to_next_run() {
        let f = Fixture::new("budget");
        let mut paths = Vec::new();
        for i in 0..3 {
            let p = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_{i:x}_0"));
            let (dev, ino) = dev_ino(&p);
            record_staged_by(DEAD_PID, &p, dev, ino);
            paths.push(p);
        }
        staging_journal::flush();

        std::env::set_var("MACLEAN_TEST_GC_BUDGET_MS", "0");
        let st = run_startup_cleanup();
        std::env::remove_var("MACLEAN_TEST_GC_BUDGET_MS");

        assert_eq!(st.cleaned, 0, "预算耗尽不该再新开删除");
        assert_eq!(st.failed, 0, "延后不是失败，不能虚报失败数");
        assert!(!st.running, "running 必须复位");
        assert_eq!(
            staging_journal::unfinished().len(),
            3,
            "3 条未处理记录必须留在 journal 供下次启动继续"
        );
        assert!(paths.iter().all(|p| p.symlink_metadata().is_ok()));

        let again = run_startup_cleanup();
        assert_eq!(again.cleaned, 3, "下轮预算恢复后必须能收完");
        assert!(staging_journal::unfinished().is_empty());
        assert!(paths.iter().all(|p| p.symlink_metadata().is_err()));
    }

    #[test]
    fn status_is_running_while_work_is_in_flight() {
        let f = Fixture::new("status");
        let staging = f.staged_dir(&format!(".maclean_stage_{DEAD_PID}_eee_0"));
        let (dev, ino) = dev_ino(&staging);
        record_staged_by(DEAD_PID, &staging, dev, ino);
        staging_journal::flush();

        // 进入前是"未在跑"
        assert!(!status().running);
        let st = run_startup_cleanup();
        assert!(!st.running, "结束后必须复位 running");
        assert!(!status().running);
    }
}
