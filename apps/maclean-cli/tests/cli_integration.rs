//! CLI 端到端集成测试（真进程 + 非交互 stdin）
//!
//! 覆盖此前"只有单测、没有集成测试"的缺口：
//! 1. 定时任务在无 TTY 环境下不被确认保护挡下（`--scheduled --dry-run` 退出 0）
//! 2. 非交互环境未带 `--yes` 时明确退出 4（不静默删除）
//! 3. APFS 快照名注入被拦截（含 `;` 的名字不得进入 tmutil/AppleScript）
//! 4. `apfs list` 的 JSON envelope 可解析
//! 5. `schedule` 状态区分"配置已开启"与"系统里已注册"

use std::process::{Command, Output, Stdio};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maclean"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("maclean 二进制应可执行")
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

#[test]
fn scheduled_clean_dry_run_runs_without_terminal() {
    // launchd/schtasks 都是非交互环境：带 --scheduled 时不得以退出码 4 退出，
    // 否则定时清理永远跑不起来、last_run 也永远不会落盘。
    // 用 custom-rules 缩小扫描范围：确认逻辑与 Tab 无关，全量 Tab 会让测试慢一分钟。
    let out = run(&[
        "clean",
        "--safe-only",
        "--scheduled",
        "--dry-run",
        "--tab",
        "custom-rules",
    ]);
    assert_eq!(
        code(&out),
        0,
        "定时任务预览应正常完成，stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn clean_without_yes_in_non_interactive_exits_4() {
    let out = run(&["clean", "--tab", "dev-cache"]);
    assert_eq!(
        code(&out),
        4,
        "非交互未带 --yes 必须退出 4，stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn apfs_delete_rejects_unsafe_snapshot_name() {
    // 快照名来自 CLI 参数，进入 shell/osascript 前必须校验；
    // 这里的名字若被放行就会变成任意命令执行。
    let out = run(&[
        "apfs",
        "delete",
        "com.apple.TimeMachine.2024-01-15-100000.local; rm -rf /tmp/maclean-inject",
        "--yes",
    ]);
    assert_ne!(code(&out), 0, "非法快照名必须失败");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("非法") || stderr.contains("invalid"),
        "应明确提示快照名非法，stderr={stderr}"
    );
    assert!(
        !std::path::Path::new("/tmp/maclean-inject").exists(),
        "注入内容不得被执行"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn apfs_list_returns_parseable_json() {
    let out = run(&["apfs", "list", "--format", "json"]);
    assert_eq!(
        code(&out),
        0,
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("输出必须是合法 JSON");
    assert_eq!(v["command"], "apfs");
    assert!(v["data"]["items"].is_array(), "items 必须是数组");
}

#[test]
fn schedule_json_exposes_registered_flag() {
    let out = run(&["schedule", "--format", "json"]);
    assert_eq!(
        code(&out),
        0,
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("输出必须是合法 JSON");
    let data = &v["data"];
    assert_eq!(data["command"], "schedule");
    assert!(data["enabled"].is_boolean());
    // 配置开关与系统注册是两个概念，必须分别暴露
    assert!(data["registered"].is_boolean(), "缺少 registered 字段");
    assert!(data["due"].is_boolean());
    assert!(data["interval_days"].is_u64());
}
