//! 真机验证（P3 收尾 + review 表格「死亡方式 / 容错边界」）。
//!
//! 与 `crash_recovery.rs` 的区别：那边验证「线索是否还在」，这里验证
//! **真 SIGKILL 之后空间是否真的回到文件系统、日志页是否真有收尾记录**。
//!
//! 默认 `#[ignore]`：要写 ~128MB 并读 `df -k`，不适合每次 `cargo test` 都跑。
//! 由 `scripts/verify-gc-real-machine.sh` 显式 `--ignored --nocapture` 触发。
//!
//! 关于 `mdfind`：Spotlight 默认不索引以 `.` 开头的隐藏项，且索引是异步的，
//! 所以 `mdfind` 查不到 `.maclean_stage_*` 是**恒真**，拿它做「残留清零」的
//! 断言是假绿。这里只把它当观察项打印，硬断言用 `read_dir` + `symlink_metadata`。

#![cfg(all(target_os = "macos", debug_assertions))]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CHILD_ENV: &str = "MACLEAN_TEST_GC_REAL_CHILD";
const TEST_NAME: &str = "kill9_releases_disk_space_and_writes_recovery_log";

/// 造在真实用户主目录下（不是 /tmp）：df 与实际卷一致，且允许 Spotlight 观察。
/// 用 RAII 保证 panic 时也不把 128MB 垃圾留在用户目录里。
struct WorkDir(PathBuf);

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// `df -k <path>` 的 Available（字节）。比 statvfs 更贴近用户在「关于本机」里看到的数字。
fn df_available(path: &Path) -> u64 {
    let out = Command::new("df")
        .arg("-k")
        .arg(path)
        .output()
        .expect("df -k 应可执行");
    assert!(
        out.status.success(),
        "df -k 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1).expect("df -k 应有数据行");
    let kb: u64 = line
        .split_whitespace()
        .nth(3)
        .expect("df -k 第 4 列是 Available")
        .parse()
        .expect("Available 应可解析为整数");
    kb.saturating_mul(1024)
}

fn staging_leftovers(parent: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(parent)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(".maclean_stage_"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn journal_counts(home: &Path) -> (usize, usize) {
    let text =
        std::fs::read_to_string(home.join(".maclean").join("staging.journal")).unwrap_or_default();
    (
        text.lines().filter(|l| l.contains("\"path\"")).count(),
        text.lines().filter(|l| l.contains("\"done\"")).count(),
    )
}

/// ~128MB：64 个 2MB 文件。用非零内容填充，避免稀疏/压缩让 df 看不出变化。
fn make_big_victim(work: &Path) -> PathBuf {
    let victim = work.join("victim");
    std::fs::create_dir_all(&victim).expect("创建待删目录");
    let chunk = vec![0x5au8; 2 * 1024 * 1024];
    for i in 0..64 {
        std::fs::write(victim.join(format!("chunk-{i:02}.bin")), &chunk).expect("写入 2MB 数据块");
    }
    victim
}

/// 子进程在 `stage` 崩点对自己发真 SIGKILL：不跑 atexit、不 flush 用户态缓冲。
fn kill9_child(home: &Path, victim: &Path) {
    let status = Command::new(std::env::current_exe().expect("测试二进制路径"))
        .args(["--exact", TEST_NAME, "--nocapture", "--ignored"])
        .env(CHILD_ENV, victim)
        .env("HOME", home)
        .env("MACLEAN_TEST_KILL_AT", "stage")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("启动子进程");
    assert!(
        !status.success(),
        "子进程应在 stage 崩点被 SIGKILL，而不是正常退出: {status:?}"
    );
}

#[test]
#[ignore = "真机：写 128MB + 读 df -k，由 scripts/verify-gc-real-machine.sh 触发"]
fn kill9_releases_disk_space_and_writes_recovery_log() {
    // ---- 子进程：真删那个 128MB 目录，删到一半被 SIGKILL ----
    if let Ok(victim) = std::env::var(CHILD_ENV) {
        let _ = maclean_core::ops::best_effort_delete_with_reason(&PathBuf::from(victim));
        std::process::exit(97); // 崩点没生效 → 让父进程判失败
    }

    let real_home = PathBuf::from(std::env::var("HOME").expect("原始 HOME"));
    let work = WorkDir(real_home.join(format!(
        "maclean-gc-verify-{}-{nanos}",
        std::process::id(),
        nanos = nanos()
    )));
    std::fs::create_dir_all(&work.0).expect("创建真机工作目录");
    let home = work.0.join("home");
    std::fs::create_dir_all(&home).expect("创建隔离 HOME");

    // 日志目录取自 app_data_dir() = $HOME/.maclean，必须在任何 logger 调用前设好。
    std::env::set_var("HOME", &home);
    maclean_core::logger::init();

    let victim = make_big_victim(&work.0);
    println!("[真机] 待删目录: {} (约 128MB)", victim.display());

    kill9_child(&home, &victim);

    let left = staging_leftovers(&work.0);
    assert_eq!(left.len(), 1, "SIGKILL 后应恰好留 1 个 staging: {left:?}");
    assert!(victim.symlink_metadata().is_err(), "原路径应已被改名走");
    assert_eq!(
        journal_counts(&home),
        (1, 0),
        "kill -9 后开始记录必须仍在（write(2) 已进内核）"
    );

    let before = df_available(&work.0);
    println!("[真机] GC 前 df Available = {before} B");

    let st = maclean_core::gc::run_startup_cleanup();
    println!("[真机] GcStatus = {st:?}");

    // ---- 硬断言 1：残留清零（用文件系统真相，不用 mdfind） ----
    assert!(
        staging_leftovers(&work.0).is_empty(),
        "收尾后 staging 残留必须清零: {:?}",
        staging_leftovers(&work.0)
    );
    assert!(left[0].symlink_metadata().is_err(), "残留目录应已不存在");
    assert_eq!(journal_counts(&home).0, 0, "压缩后不该还有未完成记录");
    assert!(st.cleaned >= 1, "至少要真回收 1 条: {st:?}");
    assert_eq!(st.failed, 0, "真机正常目录不该有失败: {st:?}");
    assert!(!st.running);
    assert!(st.bytes > 0, "statvfs 实测回收字节应 > 0: {st:?}");

    // ---- 硬断言 2：df -k 可用量上升 ----
    let after = df_available(&work.0);
    println!(
        "[真机] GC 后 df Available = {after} B，增量 = {}",
        after.saturating_sub(before)
    );
    let freed = after.saturating_sub(before);
    assert!(
        freed >= 64 * 1024 * 1024,
        "df -k 可用量应至少上升 64MB，实际只上升 {} B。\
         若本机有 Time Machine 本地快照，已删块会被快照占住而不释放，\
         请用 `tmutil listlocalsnapshots /` 确认后再判读。",
        freed
    );

    // ---- 硬断言 3：日志页能看到的收尾记录 ----
    let log = maclean_core::logger::latest_log_file().expect("应有当天日志文件");
    let log_text = std::fs::read_to_string(&log).expect("日志文件应可读");
    println!("[真机] 日志文件: {}", log.display());
    assert!(
        log_text.contains("[gc] 已收尾残留 staging"),
        "日志页应能看到逐条收尾记录"
    );
    assert!(
        log_text.contains("[gc] 收尾完成"),
        "日志页应能看到收尾汇总（含清理/失败/回收字节/耗时）"
    );

    // ---- 观察项：mdfind 仅打印，不作断言 ----
    let md = Command::new("mdfind")
        .arg("-onlyin")
        .arg(&work.0)
        .arg(".maclean_stage_")
        .output();
    match md {
        Ok(o) => println!(
            "[真机] mdfind 输出（仅观察）：{:?}",
            String::from_utf8_lossy(&o.stdout).trim()
        ),
        Err(e) => println!("[真机] mdfind 不可用（仅观察）：{e}"),
    }
    println!(
        "[真机] 说明：Spotlight 默认不索引 `.maclean_stage_*` 这类隐藏项，且索引异步，\
         mdfind 查不到是恒真；因此「残留清零」用 read_dir + symlink_metadata 硬断言。"
    );
}
