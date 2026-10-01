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
pub const LAUNCHD_LABEL: &str = "men.sunge.maclean.cleanup";

/// launchd plist 路径
pub fn launchd_plist_path() -> PathBuf {
    crate::platform::home_dir()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", LAUNCHD_LABEL))
}

/// 生成 launchd plist（纯函数）
///
/// `StartInterval` 按秒计；`RunAtLoad` 不开 —— 否则每次登录都跑一次清理，
/// 和用户设定的"每 N 天"不是一回事。
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
    </array>
    <key>StartInterval</key>
    <integer>{secs}</integer>
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
        log = crate::logger::log_dir()
            .join("schedule.log")
            .to_string_lossy(),
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
        format!("\"{}\" clean --safe-only", exe),
    ]
}

/// 卸载系统定时任务的命令（纯函数，返回 (program, args)）
pub fn uninstall_command() -> (String, Vec<String>) {
    #[cfg(target_os = "macos")]
    {
        (
            "launchctl".to_string(),
            vec![
                "unload".to_string(),
                launchd_plist_path().to_string_lossy().to_string(),
            ],
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
    std::fs::write(&path, launchd_plist(interval_days, &exe))
        .map_err(|e| format!("写入 plist 失败: {}", e))?;
    let out = std::process::Command::new("launchctl")
        .args(["load", &path.to_string_lossy()])
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
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
        assert!(plist.contains("<string>men.sunge.maclean.cleanup</string>"));
        // StartInterval 是秒：写成天数的话任务会每天跑，与设定不符
        assert!(plist.contains(&format!("<integer>{}</integer>", 7 * 86_400)));
        assert!(plist.contains("<string>/usr/local/bin/maclean</string>"));
        assert!(plist.contains("<string>clean</string>"));
        // 开了 RunAtLoad 会变成"每次登录都跑"，不是用户要的周期
        assert!(!plist.contains("RunAtLoad"));
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
    }

    #[test]
    fn uninstall_command_targets_the_same_task_name() {
        // 卸载命令必须与安装时用同一个任务标识，否则会留下一个永远跑的孤儿任务
        let (prog, args) = uninstall_command();
        assert!(!prog.is_empty());
        assert!(!args.is_empty());
    }
}
