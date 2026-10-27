//! C-3 · 定时清理
//!
//! # 缺口
//!
//! 此前没有任何定时能力：想定期清理只能自己记得打开应用点一遍。
//! 清理工具的价值恰恰在于"不用记得"。
//!
//! # 两条腿
//!
//! 1. **应用内**：启动时和运行时定期检查，到期就跑一次安全清理。
//!    覆盖"应用开着"的情况。
//! 2. **系统级**：launchd（macOS）/ 任务计划（Windows）注册一个周期性任务。
//!    覆盖"应用没开"的情况 —— 这才是定时清理的主场景，
//!    否则"定时"只在你恰好打开了应用时才成立。
//!
//! # 为什么没有"每天 20:00"这种时刻设置
//!
//! 只靠 std 拿不到可靠的本地时区偏移（跨平台要碰 `localtime_r` /
//! `localtime_s` 两套 API），为此引入 chrono/time 只为取一个小时数不划算。
//! 所以这里做成"每 N 天"：到期就用，不到期不跑。诚实说明局限，
//! 好过做一个在部分机器上算错时区的时刻表。
//!
//! # 平台无关
//!
//! 到期判定与命令构造都是纯函数，不加 cfg；真正 spawn 进程的几行才分平台。

use std::path::PathBuf;

/// 可选的清理间隔（天）
///
/// 只给三档：太少选择没用，太多选择是噪音。其它值一律夹到最近的合法档。
pub const INTERVAL_OPTIONS: [u32; 3] = [1, 7, 30];

/// 归一化间隔：把任意输入夹到合法档位
///
/// 配置文件是手写的 JSON，不夹一下会出现 "间隔 0 天" 这种每帧都触发的怪物。
pub fn normalize_interval_days(days: u32) -> u32 {
    INTERVAL_OPTIONS
        .iter()
        .copied()
        .min_by_key(|d| days.abs_diff(*d))
        .unwrap_or(7)
}

/// 是否到期（纯函数）
///
/// - 未开启 → 永不触发
/// - 从未跑过（last_run == 0）→ 立即跑一次。这是"开启即生效"的意图，
///   用户设了定时不该等 N 天才看到它工作。
/// - 时钟回拨（now < last_run）→ 不跑。回拨后 `now - last_run` 会下溢，
///   在 release 下 saturating 成 0，在 debug 下直接 panic。
pub fn is_due(enabled: bool, interval_days: u32, last_run: u64, now: u64) -> bool {
    if !enabled {
        return false;
    }
    if last_run == 0 {
        return true;
    }
    let Some(elapsed) = now.checked_sub(last_run) else {
        return false;
    };
    elapsed >= interval_days as u64 * 86_400
}

/// launchd 任务标签
///
/// 旧版本用的是 `men.sunge.maclean.cleanup`（反向域名不是我们的域名）。
/// 安装/卸载时会一并清理旧标签，避免升级后留下两个任务同时跑。
pub const LAUNCHD_LABEL: &str = "app.maclean.cleanup";

/// 旧版 launchd 任务标签（仅用于升级清理）
pub const LEGACY_LAUNCHD_LABEL: &str = "men.sunge.maclean.cleanup";

fn launchd_agents_dir() -> PathBuf {
    crate::platform::home_dir()
        .join("Library")
        .join("LaunchAgents")
}

/// launchd plist 路径
pub fn launchd_plist_path() -> PathBuf {
    launchd_agents_dir().join(format!("{}.plist", LAUNCHD_LABEL))
}

/// 旧版 plist 路径（升级时删除）
pub fn legacy_launchd_plist_path() -> PathBuf {
    launchd_agents_dir().join(format!("{}.plist", LEGACY_LAUNCHD_LABEL))
}

/// 定时任务是否真的注册在系统里
///
/// `config.json` 里的 `schedule_enabled=true` 只代表开关被打开过：plist 被删、
/// 任务被 bootout，或配置来自另一台机器时，任务并不会执行。状态展示必须区分
/// "配置已开启"和"系统里确实有任务"，否则会误报成功。
pub fn is_registered() -> bool {
    #[cfg(target_os = "macos")]
    {
        if !launchd_plist_path().exists() {
            return false;
        }
        std::process::Command::new("launchctl")
            .arg("list")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(LAUNCHD_LABEL))
            .unwrap_or(false)
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("schtasks")
            .args(["/query", "/tn", "MacleanCleanup"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// 定时任务日志路径（launchd 的 stdout/stderr 落这里）
pub fn schedule_log_path() -> PathBuf {
    crate::logger::log_dir().join("schedule.log")
}

/// 定时任务日志轮转阈值（5 MB）
const SCHEDULE_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// 轮转定时任务日志
///
/// launchd 的 stdout/stderr 直接 append 到同一个文件，不轮转的话它会
/// 无限增长。超过阈值就改名为 `schedule.log.1`（只保留一份旧日志）。
/// 返回是否发生了轮转。
pub fn rotate_log_if_needed() -> bool {
    let log = schedule_log_path();
    let Ok(meta) = std::fs::metadata(&log) else {
        return false;
    };
    if meta.len() < SCHEDULE_LOG_MAX_BYTES {
        return false;
    }
    let rotated = log.with_extension("log.1");
    let _ = std::fs::remove_file(&rotated);
    std::fs::rename(&log, &rotated).is_ok()
}

/// 生成 launchd plist（纯函数）
///
/// `StartInterval` 按秒计。`RunAtLoad` 开 + `--only-if-due`：登录时补跑
/// 一次错过的清理（机器关机/休眠错过定时点时，launchd 不会补），
/// 但命令自己会先判断是否真的到期，未到期直接退出，不会每次登录都清。
pub fn launchd_plist(interval_days: u32, exe: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>clean</string>
        <string>--safe-only</string>
        <string>--scheduled</string>
        <string>--only-if-due</string>
    </array>
    <key>StartInterval</key>
    <integer>{secs}</integer>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        label = LAUNCHD_LABEL,
        exe = exe,
        secs = interval_days as u64 * 86_400,
        log = schedule_log_path().to_string_lossy(),
    )
}

/// Windows 任务计划程序：`schtasks /create` 的参数（纯函数）
///
/// `#[cfg_attr]` 说明：这个函数只在 Windows 构建里被调用，但刻意不在函数上
/// 加 cfg —— 加了的话 macOS 开发机上整段不编译，`/sc daily /mo N` 拼错也
/// 没人会发现。允许它在非 Windows 构建里"未使用"，换来的测试覆盖是值的。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn schtasks_create_args(interval_days: u32, exe: &str) -> Vec<String> {
    vec![
        "/create".to_string(),
        "/f".to_string(),
        "/tn".to_string(),
        "MacleanCleanup".to_string(),
        "/sc".to_string(),
        "daily".to_string(),
        "/mo".to_string(),
        interval_days.to_string(),
        "/tr".to_string(),
        // 必须带 --scheduled：schtasks 是非交互环境，缺少它 CLI 会以
        // 退出码 4（需要确认）直接退出，定时清理永远不会落盘 last_run。
        // --only-if-due 让登录补偿执行时先判断是否真的到期。
        format!("\"{}\" clean --safe-only --scheduled --only-if-due", exe),
    ]
}

/// 卸载系统定时任务的命令（纯函数，返回 (program, args)）
///
/// macOS 用 `launchctl bootout`（现代接口，`unload` 已废弃）；Windows 用
/// `schtasks /delete`。完整的 macOS 卸载（含旧标签与 plist 文件清理）见
/// [`uninstall`]。
pub fn uninstall_command() -> (String, Vec<String>) {
    #[cfg(target_os = "macos")]
    {
        (
            "launchctl".to_string(),
            macos_bootout_args(LAUNCHD_LABEL, &launchd_plist_path()),
        )
    }
    #[cfg(target_os = "windows")]
    {
        (
            "schtasks".to_string(),
            vec![
                "/delete".to_string(),
                "/f".to_string(),
                "/tn".to_string(),
                "MacleanCleanup".to_string(),
            ],
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        (String::new(), Vec::new())
    }
}

/// macOS `launchctl bootout gui/<uid> <plist>` 参数
#[cfg(target_os = "macos")]
fn macos_bootout_args(label: &str, plist: &std::path::Path) -> Vec<String> {
    let uid = unsafe { libc::getuid() };
    vec![
        "bootout".to_string(),
        format!("gui/{}/{}", uid, label),
        plist.to_string_lossy().to_string(),
    ]
}

/// 卸载系统定时任务
///
/// macOS：先 bootout 当前标签与旧标签（升级清理），再删除对应的 plist 文件。
/// 任何一步失败都继续尝试下一步，最后把第一个错误返回给调用方 ——
/// 不能因为旧任务不存在就报"移除失败"。
#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<(), String> {
    let mut first_err: Option<String> = None;
    for (label, plist) in [
        (LAUNCHD_LABEL, launchd_plist_path()),
        (LEGACY_LAUNCHD_LABEL, legacy_launchd_plist_path()),
    ] {
        if plist.exists() || label == LAUNCHD_LABEL {
            if let Ok(o) = std::process::Command::new("launchctl")
                .args(macos_bootout_args(label, &plist))
                .output()
            {
                if !o.status.success() && first_err.is_none() {
                    let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    // "Could not find service" 说明本来就没装，不算失败
                    if !msg.contains("Could not find service")
                        && !msg.contains("No such process")
                        && !msg.is_empty()
                    {
                        first_err = Some(msg);
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&plist);
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall() -> Result<(), String> {
    let (prog, args) = uninstall_command();
    let out = std::process::Command::new(prog).args(args).output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// 安装系统定时任务
///
/// 返回 Ok(()) 仅表示"命令发出去了"。launchd / schtasks 都可能因为权限
/// 或策略失败，调用方要把这一点如实告诉用户，不能说"已开启定时清理"。
#[cfg(target_os = "macos")]
pub fn install(interval_days: u32) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "maclean".to_string());
    let path = launchd_plist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 LaunchAgents 目录失败: {}", e))?;
    }
    // 先清掉旧标签：升级后旧 plist 仍会被 launchd 加载，两个任务会重复清理
    let legacy = legacy_launchd_plist_path();
    if legacy.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(macos_bootout_args(LEGACY_LAUNCHD_LABEL, &legacy))
            .output();
        let _ = std::fs::remove_file(&legacy);
    }
    // 重装前先 bootout 当前标签，否则 bootstrap 会报 "service already loaded"
    let _ = std::process::Command::new("launchctl")
        .args(macos_bootout_args(LAUNCHD_LABEL, &path))
        .output();

    rotate_log_if_needed();
    std::fs::write(&path, launchd_plist(interval_days, &exe))
        .map_err(|e| format!("写入 plist 失败: {}", e))?;

    let uid = unsafe { libc::getuid() };
    let out = std::process::Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{}", uid),
            &path.to_string_lossy(),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            // 老系统可能不支持 bootstrap：退回 load，仍失败则如实回传
            let bootstrap_err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            match std::process::Command::new("launchctl")
                .args(["load", &path.to_string_lossy()])
                .output()
            {
                Ok(fallback) if fallback.status.success() => Ok(()),
                Ok(fallback) => {
                    let fallback_err = String::from_utf8_lossy(&fallback.stderr).trim().to_string();
                    Err(if bootstrap_err.is_empty() {
                        fallback_err
                    } else {
                        format!("{} / {}", bootstrap_err, fallback_err)
                    })
                }
                Err(e) => Err(format!("{} / {}", bootstrap_err, e)),
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(target_os = "windows")]
pub fn install(interval_days: u32) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "maclean.exe".to_string());
    let out = std::process::Command::new("schtasks")
        .args(schtasks_create_args(interval_days, &exe))
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn install(_interval_days: u32) -> Result<(), String> {
    Err("当前平台不支持注册系统定时任务".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_means_never_due() {
        // 反向验证：开关关掉时，即使早就超期也不得触发
        assert!(!is_due(false, 1, 0, u64::MAX));
        assert!(!is_due(false, 1, 100, 100 + 86_400 * 10));
    }

    #[test]
    fn first_run_fires_immediately() {
        // 设了定时却要等 N 天才第一次生效，用户会以为功能坏了
        assert!(is_due(true, 30, 0, 1_000));
    }

    #[test]
    fn due_only_after_the_interval_elapses() {
        let last = 1_000_000;
        assert!(!is_due(true, 7, last, last + 86_400 * 6));
        assert!(is_due(true, 7, last, last + 86_400 * 7));
        assert!(is_due(true, 1, last, last + 86_400));
    }

    #[test]
    fn a_clock_rollback_never_triggers_a_run() {
        // 回拨防护：不加这条，debug 构建下 now - last_run 会直接 panic
        assert!(!is_due(true, 1, 5_000, 1_000));
        assert!(!is_due(true, 1, 5_000, 4_999));
    }

    #[test]
    fn odd_intervals_snap_to_a_supported_value() {
        assert_eq!(normalize_interval_days(0), 1);
        assert_eq!(normalize_interval_days(2), 1);
        assert_eq!(normalize_interval_days(5), 7);
        assert_eq!(normalize_interval_days(9), 7);
        assert_eq!(normalize_interval_days(365), 30);
        // 合法档位必须原样返回，不能被"归一化"挪走
        for d in INTERVAL_OPTIONS {
            assert_eq!(normalize_interval_days(d), d);
        }
    }

    #[test]
    fn launchd_plist_carries_the_interval_in_seconds() {
        let plist = launchd_plist(7, "/usr/local/bin/maclean");
        assert!(plist.contains(&format!("<string>{}</string>", LAUNCHD_LABEL)));
        // 旧标签不能出现在新 plist 里（只用于升级清理）
        assert!(!plist.contains(LEGACY_LAUNCHD_LABEL));
        // StartInterval 是秒：写成天数的话任务会每天跑，与设定不符
        assert!(plist.contains(&format!("<integer>{}</integer>", 7 * 86_400)));
        assert!(plist.contains("<string>/usr/local/bin/maclean</string>"));
        assert!(plist.contains("<string>clean</string>"));
        // P0 回归：缺 --scheduled 时非交互 launchd 会以退出码 4 直接退出，
        // 定时清理永远不执行、schedule_last_run 永远不落盘。
        assert!(plist.contains("<string>--scheduled</string>"));
        assert!(plist.contains("<string>--only-if-due</string>"));
        // RunAtLoad + --only-if-due：登录时补跑错过的清理，但未到期会跳过
        assert!(plist.contains("<key>RunAtLoad</key>"));
    }

    #[test]
    fn schtasks_uses_a_daily_schedule_with_a_multiplier() {
        let args = schtasks_create_args(7, r"C:\Program Files\maclean.exe");
        assert_eq!(args[0], "/create");
        // /sc daily + /mo N 才是"每 N 天"；只写 daily 是每天
        let sc = args.iter().position(|a| a == "/sc").unwrap();
        assert_eq!(args[sc + 1], "daily");
        let mo = args.iter().position(|a| a == "/mo").unwrap();
        assert_eq!(args[mo + 1], "7");
        // 同样必须带 --scheduled，否则 schtasks 跑起来会被确认保护挡掉
        let tr = args.iter().position(|a| a == "/tr").unwrap();
        let cmd = &args[tr + 1];
        assert!(
            cmd.contains("--scheduled"),
            "schtasks 命令缺少 --scheduled: {cmd}"
        );
        assert!(cmd.contains("--only-if-due"));
        assert!(cmd.contains("--safe-only"));
    }

    #[test]
    fn uninstall_command_targets_the_same_task_name() {
        // 卸载命令必须与安装时用同一个任务标识，否则会留下一个永远跑的孤儿任务
        let (prog, args) = uninstall_command();
        assert!(!prog.is_empty());
        assert!(!args.is_empty());
        #[cfg(target_os = "macos")]
        {
            assert_eq!(prog, "launchctl");
            assert!(args.iter().any(|a| a.contains(LAUNCHD_LABEL)));
            assert!(args.iter().any(|a| a == "bootout"));
        }
        #[cfg(target_os = "windows")]
        {
            assert_eq!(prog, "schtasks");
            assert!(args.iter().any(|a| a == "MacleanCleanup"));
        }
    }
}
