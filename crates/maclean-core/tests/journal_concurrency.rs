//! 表格「并发」维度：CLI 与 App 共用同一个 `~/.maclean/staging.journal`，
//! 两个真进程各自 append 时靠 flock 串行化，不能丢行、也不能交错成坏行。
//!
//! 单进程内的 Mutex 盖不到这里，所以必须是真子进程。Windows 没有 flock，
//! 该场景在 Windows 上由 O_APPEND + 逐行容错降级，故本文件只在 unix 编译。

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};

const CHILD_ENV: &str = "MACLEAN_JOURNAL_CONCURRENCY_CHILD";
const TEST_NAME: &str = "two_processes_append_without_losing_lines";
const LINES_PER_CHILD: usize = 200;

#[test]
fn two_processes_append_without_losing_lines() {
    // ---- 子进程模式：往同一个 journal 写一批开始记录 ----
    if let Ok(tag) = std::env::var(CHILD_ENV) {
        for i in 0..LINES_PER_CHILD {
            let p = PathBuf::from(format!("/virtual/{tag}/.maclean_stage_1_{i:x}_0"));
            maclean_core::staging_journal::record_staged(&p, 0, 0);
        }
        maclean_core::staging_journal::flush();
        std::process::exit(0);
    }

    // ---- 父进程：同时起两个进程写同一个 HOME 下的 journal ----
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let root =
        std::env::temp_dir().join(format!("maclean-journal-cc-{}-{nanos}", std::process::id()));
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("创建临时 HOME");
    std::env::set_var("HOME", &home);

    let exe = std::env::current_exe().expect("测试二进制路径");
    let mut children = Vec::new();
    for tag in ["a", "b"] {
        children.push(
            Command::new(&exe)
                .args(["--exact", TEST_NAME, "--nocapture"])
                .env(CHILD_ENV, tag)
                .env("HOME", &home)
                .stdout(Stdio::null())
                .spawn()
                .expect("启动子进程"),
        );
    }
    for c in &mut children {
        assert!(c.wait().unwrap().success(), "子进程写 journal 不应失败");
    }

    // read_lines() 只返回能解析的完整行：少一行或交错出坏行都会让计数不足
    let lines = maclean_core::staging_journal::read_lines();
    assert_eq!(
        lines.len(),
        2 * LINES_PER_CHILD,
        "两进程各写 {LINES_PER_CHILD} 行，不能丢行，也不能交错成半行"
    );
    let mut seen = std::collections::HashSet::new();
    for l in &lines {
        let p = l.path.as_deref().unwrap_or("");
        seen.insert(if p.contains("/a/") {
            "a"
        } else if p.contains("/b/") {
            "b"
        } else {
            "?"
        });
    }
    assert_eq!(seen.len(), 2, "两个进程各自的行都必须保留：{seen:?}");

    let _ = std::fs::remove_dir_all(&root);
}
