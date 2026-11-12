//! CLI 端到端集成测试（真进程 + 非交互 stdin）
//!
//! 覆盖此前"只有单测、没有集成测试"的缺口：
//! 1. 定时任务在无 TTY 环境下不被确认保护挡下（`--scheduled --dry-run` 退出 0）
//! 2. 非交互环境未带 `--yes` 时明确退出 3（E-01：契约 §4 需确认 = 3，旧代码误用 4）
//! 3. APFS 快照名注入被拦截（含 `;` 的名字不得进入 tmutil/AppleScript）
//! 4. `apfs list` 的 JSON envelope 可解析
//! 5. `schedule` 状态区分"配置已开启"与"系统里已注册"
//!
//! H-08：新增表驱动矩阵，覆盖命令 × {json, jsonl} × 成功/失败，
//! 断言 envelope 字段与退出码一致；并补 --json / E-03 / E-07 / E-09 用例。

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

/// 解析单个 json envelope 并断言字段形状 + status + 退出码。
fn assert_json_envelope(out: &Output, expected_code: i32, expected_status: &str, command: &str) {
    assert_eq!(
        code(out),
        expected_code,
        "退出码不符，stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout 不是合法 JSON 单对象: {e}\n{stdout}"));
    assert_eq!(v["contract_version"], 1, "缺 contract_version");
    assert!(
        !v["request_id"].as_str().unwrap_or("").is_empty(),
        "缺 request_id"
    );
    assert_eq!(v["command"], command);
    assert_eq!(v["status"], expected_status);
}

/// jsonl：每行都是完整 envelope，末行为收尾信封（data 省略）。
fn assert_jsonl_envelopes(out: &Output, expected_code: i32, command: &str) {
    assert_eq!(code(out), expected_code);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "JSONL 至少一行");
    for (i, line) in lines.iter().enumerate() {
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("第 {i} 行不是完整 envelope: {e}\n{line}"));
        assert_eq!(v["contract_version"], 1, "每行都必须是完整 envelope");
        assert_eq!(v["command"], command);
        assert!(v.get("status").is_some(), "每行都要有 status");
    }
    // 末行收尾信封：data 省略或为 null
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert!(
        last.get("data").map(|d| d.is_null()).unwrap_or(true),
        "末行应为收尾信封（data 省略/null）"
    );
}

#[test]
fn scheduled_clean_dry_run_runs_without_terminal() {
    // launchd/schtasks 都是非交互环境：带 --scheduled 时不得以退出码 3 退出，
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
fn clean_without_yes_in_non_interactive_exits_3() {
    // E-01：契约 §4 需确认 = 3（旧代码/旧测试误用 4）
    let out = run(&["clean", "--tab", "dev-cache"]);
    assert_eq!(
        code(&out),
        3,
        "非交互未带 --yes 必须退出 3，stdout={}",
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

// =========================================================================
//  H-08 · 表驱动覆盖矩阵
// =========================================================================

#[test]
fn readonly_commands_emit_consistent_envelopes() {
    // 只读命令 × {json, jsonl} 成功矩阵：envelope 字段与退出码一致。
    let cases: &[(&[&str], &str)] = &[
        (&["list"], "list"),
        (&["backups"], "backups"),
        (&["dup-ignore", "list"], "dup-ignore"),
        (&["log"], "log"),
        (&["optimize", "list-tasks"], "optimize"),
        // 注：`scan` 不进本矩阵。scan 扫描真实文件系统，custom-rules 等 tab 在
        // 负载下可能触发 90s watchdog 超时，合法返回 status:"warnings" + 退出码 5，
        // 不满足本矩阵"成功即 status:ok/0"的严格契约。其 envelope 契约由
        // `scan_json_exposes_reclaimable_field` 单独、且对超时鲁棒地覆盖。
        (&["schedule"], "schedule"),
    ];
    for (args, command) in cases {
        let mut json_args = args.to_vec();
        json_args.extend(["--format", "json"]);
        assert_json_envelope(&run(&json_args), 0, "ok", command);

        let mut jsonl_args = args.to_vec();
        jsonl_args.extend(["--format", "jsonl"]);
        assert_jsonl_envelopes(&run(&jsonl_args), 0, command);
    }
}

#[test]
fn error_commands_emit_error_envelopes_with_matching_exit_codes() {
    // 失败 / 需确认路径：机器格式必须是错误信封，且退出码与 error_code 一致。
    // E-02：绝不出现 status:"ok" 配失败退出码。
    let cases: &[(&[&str], i32, &str, u64)] = &[
        // (args, 退出码, command, error_code)
        (
            &["clean", "--tab", "custom-rules", "--format", "json"],
            3,
            "clean",
            3,
        ),
        (
            &["restore", "no-such-manifest", "--format", "json"],
            1,
            "restore",
            1,
        ),
        (&["history", "--format", "json"], 1, "history", 1),
        (&["apfs", "delete", "bad", "--format", "json"], 3, "apfs", 3),
        // task 是位置参数（非 --task），用不存在任务触发"未找到"错误信封
        (
            &["optimize", "run", "nope", "--yes", "--format", "json"],
            1,
            "optimize",
            1,
        ),
    ];
    for (args, expected_code, command, error_code) in cases {
        let out = run(args);
        assert_eq!(code(&out), *expected_code, "{args:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let v: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("{args:?} 无合法错误信封: {e}\n{stdout}"));
        assert_eq!(v["status"], "error", "{args:?}");
        assert_eq!(v["command"], *command);
        assert_eq!(v["error_code"], *error_code, "{args:?}");
        assert!(
            !v["error_message"].as_str().unwrap_or("").is_empty(),
            "{args:?} 缺 error_message"
        );
    }
}

#[test]
fn error_envelope_is_emitted_in_jsonl_too() {
    // E-02/E-10：jsonl 错误路径每行仍是完整 envelope，且含 error 行
    let out = run(&["restore", "no-such", "--format", "jsonl"]);
    assert_eq!(code(&out), 1);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "jsonl 错误路径无输出");
    for l in &lines {
        let v: serde_json::Value = serde_json::from_str(l).unwrap();
        assert_eq!(v["contract_version"], 1);
    }
    assert!(
        lines.iter().any(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["status"] == "error"
        }),
        "jsonl 缺少 error 信封行"
    );
}

#[test]
fn global_json_flag_equals_format_json() {
    // --json 是 global arg：等价 --format json
    let out = run(&["list", "--json"]);
    assert_json_envelope(&out, 0, "ok", "list");
}

#[test]
fn explicit_format_wins_over_json_flag() {
    // E-09：用户显式 --format jsonl 时优先于 --json（旧实现静默覆盖成 json）
    let out = run(&["list", "--json", "--format", "jsonl"]);
    assert_eq!(code(&out), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        lines.len() >= 2,
        "显式 --format jsonl 应优先：得到多行 jsonl，而非单个 json 对象\n{stdout}"
    );
    for l in &lines {
        let _: serde_json::Value = serde_json::from_str(l).unwrap();
    }
}

#[test]
fn scan_json_exposes_reclaimable_field() {
    // E-07：契约 §7 示例 `jq '.data.reclaimable'` 必须能取到真实字段。
    // custom-rules 扫描真实文件系统，可能因 watchdog 超时返回 status:"warnings"
    // + 退出码 5（EXIT_WARNINGS），这是合法状态而非契约失败 —— 本测试只验
    // reclaimable 字段确实暴露且为整数，对 ok(0)/warnings(5) 两种退出码都鲁棒。
    let out = run(&["scan", "--tab", "custom-rules", "--format", "json"]);
    let c = code(&out);
    assert!(
        c == 0 || c == 5,
        "scan 仅允许 ok(0) 或 warnings(5)，得到 {c}；stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["contract_version"], 1, "缺 contract_version");
    assert_eq!(v["command"], "scan");
    assert!(
        matches!(v["status"], serde_json::Value::String(ref s) if s == "ok" || s == "warnings"),
        "scan 状态应为 ok 或 warnings：{}",
        v["status"]
    );
    assert!(
        v["data"]["reclaimable"].is_u64(),
        "data.reclaimable 缺失或不是整数：{}",
        v["data"]
    );
}

#[test]
fn check_disk_envelope_status_matches_exit_code() {
    // 磁盘信息可得 → ok/0；不可得（容器/CI）→ error/1，二者必须一致
    let out = run(&["check-disk", "--format", "json"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("check-disk 无合法信封: {e}\n{stdout}"));
    assert_eq!(v["contract_version"], 1);
    assert_eq!(v["command"], "check-disk");
    if code(&out) == 0 {
        assert_eq!(v["status"], "ok");
    } else {
        assert_eq!(v["status"], "error", "失败退出码不得配 status:ok");
        assert_eq!(v["error_code"], 1);
    }
}

#[test]
fn check_disk_breakdown_category_counts_are_real() {
    // E-11：categories[].count 不再恒为 0
    let out = run(&["check-disk", "--breakdown", "--format", "json"]);
    // 拿不到磁盘信息时会返回错误信封（非 0），此时跳过
    if code(&out) != 0 {
        return;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let cats = v["data"]["categories"]
        .as_array()
        .expect("categories 应为数组");
    for c in cats {
        if c["size_bytes"].as_u64().unwrap_or(0) > 0 {
            assert!(
                c["count"].as_u64().unwrap_or(0) > 0,
                "E-11：有 size_bytes 的 category 其 count 不应为 0: {c}"
            );
        }
    }
}

#[test]
fn scheduled_only_if_due_skips_without_error() {
    // H-08：--scheduled --only-if-due 组合在机器格式下不得报错退出
    let out = run(&[
        "clean",
        "--safe-only",
        "--scheduled",
        "--only-if-due",
        "--dry-run",
        "--tab",
        "custom-rules",
        "--format",
        "json",
    ]);
    assert_eq!(
        code(&out),
        0,
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// CLI 子进程里 `app_data_dir()` 的落点：Windows 读 `%APPDATA%\maclean`
/// （测试把 APPDATA 设成临时 home，所以还要再拼一层 `maclean`），
/// macOS/Linux 读 `$HOME/.maclean`。
///
/// 2026-11-12 Windows runner 实证：只注入 HOME 不注入 APPDATA，CLI 会去读
/// runner 本机的真实 `%APPDATA%`，夹具写的 journal 根本没被读到 —— 测试在本机
/// 全绿、Windows 必红。跨平台夹具必须把平台实际读取的变量一起注入。
fn cli_test_app_data_dir(home: &std::path::Path) -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    {
        home.join("maclean")
    }
    #[cfg(not(target_os = "windows"))]
    {
        home.join(".maclean")
    }
}

/// 崩溃收尾（GC）：CLI 是短生命周期进程，必须在命令入口**同步** replay，
/// 不能像 GUI 那样 spawn 后台线程（命令跑完进程就退出，线程可能没执行完）。
///
/// 造一个「上次进程死在 rename 之后」的残留 + 一条未完成 journal 记录，
/// 跑一个只读命令，断言残留被清掉，且 stdout 仍是干净的 envelope
/// （收尾日志只允许走 stderr / 日志文件）。
#[test]
fn cli_recovers_stale_staging_at_command_startup() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let root = std::env::temp_dir().join(format!("maclean-cli-gc-{}-{nanos}", std::process::id()));
    let home = root.join("home");
    let work = root.join("work");
    let app_data = cli_test_app_data_dir(&home);
    std::fs::create_dir_all(&app_data).expect("建临时 app data 目录");
    std::fs::create_dir_all(&work).expect("建工作目录");

    let staging = work.join(".maclean_stage_999999_abc_0");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("payload.bin"), vec![0u8; 4096]).unwrap();

    // 手写一条未完成记录：pid 999999 在 macOS 必死；dev/ino 全 0 时 GC
    // 跳过物理身份比对，让夹具只聚焦「CLI 到底有没有来收」。
    let journal = app_data.join("staging.journal");
    let line = serde_json::json!({
        "v": 1,
        "ts": 1,
        "pid": 999999,
        "path": staging.to_string_lossy(),
        "dev": 0,
        "ino": 0
    });
    std::fs::write(&journal, format!("{line}\n")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_maclean"))
        .args(["list", "--format", "json"])
        .env("HOME", &home)
        .env("APPDATA", &home)
        .env("USERPROFILE", &home)
        .env("MACLEAN_NO_TELEMETRY", "1")
        .stdin(Stdio::null())
        .output()
        .expect("maclean 二进制应可执行");
    assert_eq!(
        code(&out),
        0,
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let envelope: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("收尾不能污染 stdout: {e}\n{stdout}"));
    assert_eq!(envelope["command"], "list");

    assert!(
        staging.symlink_metadata().is_err(),
        "CLI 启动时必须同步收掉上次崩溃的 staging 残留"
    );
    let after = std::fs::read_to_string(&journal).unwrap_or_default();
    assert!(
        !after.lines().any(|l| l.contains("\"path\"")),
        "journal 应压缩掉已完成的记录: {after}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
