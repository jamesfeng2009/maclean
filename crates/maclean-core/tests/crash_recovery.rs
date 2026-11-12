//! 方案 §6 的崩溃注入验证（P3）＋ review 表格的「崩溃窗口 / 死亡方式」两维。
//!
//! 子进程在删除链路的三个不同时刻死亡，父进程按真实启动路径恢复：
//!
//! | 窗口     | 崩溃现场                          | 谁能收尾                    |
//! | -------- | --------------------------------- | --------------------------- |
//! | `rename` | staging 在、journal **没有记录**  | 只有 Layer 2 同目录扫描     |
//! | `stage`  | staging 在、journal 有 start      | 启动 GC replay              |
//! | `rm`     | staging 已删完、journal 仍 open   | GC 标 done("gone") 不计清理 |
//!
//! `stage` 再跑一遍真 SIGKILL（不是 abort）：验证 journal 的 `write(2)` 不依赖
//! 进程善后，kill -9 之后开始记录仍在内核里。
//!
//! 只在 macOS + debug 构建下验证：改名走 `renameatx_np` 分支，注入埋点也只在
//! debug 编译（发布包绝不允许自杀）。

#![cfg(all(target_os = "macos", debug_assertions))]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CHILD_ENV: &str = "MACLEAN_TEST_GC_CRASH_CHILD";
const TEST_NAME: &str = "crash_windows_recover_cleanly";

fn scratch_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "maclean-gc-crash-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn staging_leftovers(parent: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".maclean_stage_"))
        })
        .collect()
}

fn journal_path(home: &Path) -> PathBuf {
    home.join(".maclean").join("staging.journal")
}

/// (开始记录数, 完成记录数)
fn journal_counts(home: &Path) -> (usize, usize) {
    let text = std::fs::read_to_string(journal_path(home)).unwrap_or_default();
    (
        text.lines().filter(|l| l.contains("\"path\"")).count(),
        text.lines().filter(|l| l.contains("\"done\"")).count(),
    )
}

fn make_victim(work: &Path) -> PathBuf {
    let victim = work.join("victim");
    std::fs::create_dir_all(victim.join("nested")).expect("创建待删目录");
    std::fs::write(victim.join("a.bin"), vec![0u8; 4096]).expect("写入文件");
    std::fs::write(victim.join("nested").join("b.bin"), "x").expect("写入文件");
    victim
}

fn age_dir(p: &Path, secs: u64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    std::fs::File::open(p)
        .expect("打开残留目录以改 mtime")
        .set_modified(t)
        .expect("把残留 mtime 推过 60s 保护线");
}

/// 起一个子进程，让它在指定崩点死亡。`kill9=true` 用真 SIGKILL。
fn crash_child(window: &str, kill9: bool, home: &Path, victim: &Path) {
    let mut cmd = Command::new(std::env::current_exe().expect("测试二进制路径"));
    cmd.args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_ENV, victim)
        .env("HOME", home)
        .stdout(Stdio::null());
    if kill9 {
        cmd.env("MACLEAN_TEST_KILL_AT", window);
    } else {
        cmd.env("MACLEAN_TEST_CRASH_AT", window);
    }
    let status = cmd.status().expect("启动子进程");
    assert!(
        !status.success(),
        "子进程应在 {window} 崩点死亡，而不是正常退出：{status:?}"
    );
}

fn scenario(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = scratch_dir(tag);
    let home = root.join("home");
    let work = root.join("work");
    std::fs::create_dir_all(&home).expect("创建临时 HOME");
    std::fs::create_dir_all(&work).expect("创建工作目录");
    (root, home, work)
}

#[test]
fn crash_windows_recover_cleanly() {
    // ---- 子进程模式：真删一个目录，由环境变量决定的崩点杀死自己 ----
    if let Ok(victim) = std::env::var(CHILD_ENV) {
        let _ = maclean_core::ops::best_effort_delete_with_reason(&PathBuf::from(victim));
        // 走到这里说明崩点没生效（或 rename 前就失败了），非 0 退出让父进程失败
        std::process::exit(97);
    }

    // ---- 窗口 1：rename 成功、journal 未落盘 → 只有 Layer 2 能兜 ----
    {
        let (root, home, work) = scenario("rename");
        let victim = make_victim(&work);
        crash_child("rename", false, &home, &victim);

        let left = staging_leftovers(&work);
        assert_eq!(
            left.len(),
            1,
            "rename 后崩溃应恰好留 1 个 staging：{left:?}"
        );
        assert!(victim.symlink_metadata().is_err(), "原路径应已被改名走");
        assert_eq!(
            journal_counts(&home).0,
            0,
            "这个窗口 journal 不该有开始记录 —— 正是 Layer 2 存在的理由"
        );

        std::env::set_var("HOME", &home);
        let st = maclean_core::gc::run_startup_cleanup();
        assert_eq!(st.cleaned, 0, "没有任何线索时 GC 不该乱删");
        assert_eq!(staging_leftovers(&work).len(), 1, "GC 单独收不掉这个窗口");

        // 模拟「上次崩溃」：mtime 推过 60s 保护线，再删同父目录的兄弟项
        age_dir(&left[0], 120);
        let sibling = work.join("sibling.bin");
        std::fs::write(&sibling, b"x").unwrap();
        maclean_core::ops::best_effort_delete_with_reason(&sibling).unwrap();
        assert!(
            staging_leftovers(&work).is_empty(),
            "Layer 2 同目录兜底必须清掉无 journal 线索的残留：{:?}",
            staging_leftovers(&work)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- 窗口 2 / 死亡方式：stage 崩点，abort 与真 kill -9 各跑一遍 ----
    for (tag, kill9) in [("stage-abort", false), ("stage-kill9", true)] {
        let (root, home, work) = scenario(tag);
        let victim = make_victim(&work);
        crash_child("stage", kill9, &home, &victim);

        assert_eq!(staging_leftovers(&work).len(), 1, "崩溃后应有 1 个残留");
        assert_eq!(
            journal_counts(&home),
            (1, 0),
            "kill -9 也不能丢掉已 write(2) 的开始记录：{tag}"
        );

        std::env::set_var("HOME", &home);
        let st = maclean_core::gc::run_startup_cleanup();
        assert!(st.cleaned >= 1, "GC 应至少收掉 1 条残留：{tag} {st:?}");
        assert!(
            staging_leftovers(&work).is_empty(),
            "{tag} 收尾后残留应清零"
        );
        let after = std::fs::read_to_string(journal_path(&home)).unwrap_or_default();
        assert!(
            !after.lines().any(|l| l.contains("\"path\"")),
            "压缩后不该还有未完成记录：{tag} {after}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- 窗口 3：rm 完成、done 未写 → GC 识别对象已不存在并标 done ----
    {
        let (root, home, work) = scenario("rm");
        let victim = make_victim(&work);
        crash_child("rm", false, &home, &victim);

        assert!(
            staging_leftovers(&work).is_empty(),
            "staging 应已被删完，只是没来得及写 done"
        );
        assert_eq!(journal_counts(&home), (1, 0), "journal 仍留着未完成记录");

        std::env::set_var("HOME", &home);
        let st = maclean_core::gc::run_startup_cleanup();
        assert_eq!(st.cleaned, 0, "对象已不存在，不该计入真回收条数");
        assert_eq!(st.failed, 0, "也不该算失败");
        let after = std::fs::read_to_string(journal_path(&home)).unwrap_or_default();
        assert!(
            !after.lines().any(|l| l.contains("\"path\"")),
            "不可达的记录应标 done 并压掉，避免每次启动重试：{after}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
