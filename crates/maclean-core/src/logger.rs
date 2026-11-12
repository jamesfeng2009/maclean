//! 日志系统
//!
//! 跨平台文件日志，支持日志轮转和 panic 捕获。
//!
//! 日志路径:
//! - macOS:   ~/.maclean/logs/maclean_YYYY-MM-DD.log
//! - Windows: %APPDATA%\maclean\logs\maclean_YYYY-MM-DD.log
//!
//! 日志级别: INFO / WARN / ERROR / PANIC
//! 日志轮转: 每天一个文件，自动清理 7 天前的日志

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 日志级别
#[derive(Debug, Clone, Copy)]
enum LogLevel {
    Info,
    Warn,
    Error,
    Panic,
}

impl LogLevel {
    fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Panic => "PANIC",
        }
    }
}

/// 全局日志文件
static LOGGER: OnceLock<Mutex<Option<File>>> = OnceLock::new();

/// 日志目录
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 日志目录环境变量覆盖（测试 / 排障用）
///
/// 优先级高于默认目录，且**不写入 `LOG_DIR` 缓存**：CLI 集成测试会派生多个
/// 子进程，每个进程各自读一次环境变量即可，不会互相污染。
pub const LOG_DIR_ENV: &str = "MACLEAN_LOG_DIR";

/// 获取日志目录
pub fn log_dir() -> PathBuf {
    // 环境变量优先于缓存：让测试能在进程内把日志重定向到临时目录，
    // 即使更早的调用已经把默认路径缓存进 LOG_DIR 也依然生效。
    if let Some(custom) = std::env::var_os(LOG_DIR_ENV) {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    LOG_DIR
        .get_or_init(|| crate::platform::app_data_dir().join("logs"))
        .clone()
}

/// 初始化日志系统
///
/// 在 main() 开头调用。创建日志目录，打开当天的日志文件，
/// 设置 panic hook，清理过期日志。
pub fn init() {
    init_with_version(env!("CARGO_PKG_VERSION"));
}

/// 初始化日志系统，并指定启动横幅中展示的版本号
///
/// 供 CLI 等上层二进制传入自身版本（`env!("CARGO_PKG_VERSION")`），
/// 避免横幅显示的是本库的版本号。
pub fn init_with_version(version: &str) {
    // 幂等：重复调用只保留第一次的横幅与 panic hook。
    //
    // 旧实现把 `LOGGER.set` 失败的返回值丢掉（`let _ =`），却照写横幅、照装
    // panic hook：库被多次 init 时横幅会重复；更麻烦的是 `set_panic_hook`
    // 每次 `take_hook` 再 `set_hook`，N 次 init 叠出 N 层 hook —— 一次 panic
    // 会被记 N 遍、backtrace 抓 N 次。
    if LOGGER.get().is_some() {
        return;
    }

    let log_dir = log_dir();

    // 创建日志目录
    if fs::create_dir_all(&log_dir).is_err() {
        eprintln!("警告: 无法创建日志目录 {:?}", log_dir);
        return;
    }

    // 清理 7 天前的日志
    clean_old_logs(&log_dir);

    // 打开今天的日志文件
    let today = format_date(SystemTime::now());
    let log_file = log_dir.join(format!("maclean_{}.log", today));

    let file = OpenOptions::new().create(true).append(true).open(&log_file);

    match file {
        Ok(f) => {
            // 先占位再写横幅：并发 init 时 set 只会成功一次，抢输的线程直接返回，
            // 不会再写第二份横幅（上面的 is_some 检查只挡得住顺序重复）。
            if LOGGER.set(Mutex::new(Some(f))).is_err() {
                return;
            }

            // 启动信息压成一行：原来 4 行（横幅 / 日志路径 / 平台 / 架构）在
            // 每次启动都占 4 行，而真正排障需要的版本、平台、日志位置一个不少。
            info(&format!(
                "========== maclean v{} 启动 | {} {} | 日志 {} ==========",
                version,
                std::env::consts::OS,
                std::env::consts::ARCH,
                log_file.display()
            ));

            // 设置 panic hook
            set_panic_hook();
        }
        Err(e) => {
            eprintln!("警告: 无法打开日志文件 {:?}: {}", log_file, e);
        }
    }
}

/// 设置 panic hook，捕获崩溃信息写入日志
fn set_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 写入日志
        let panic_msg = format!("PANIC: {}", info);
        let location = info
            .location()
            .map(|l| format!(" at {}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        log(LogLevel::Panic, &format!("{}{}", panic_msg, location));

        // 获取 backtrace
        let backtrace = std::backtrace::Backtrace::capture();
        if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
            log(LogLevel::Panic, &format!("Backtrace:\n{}", backtrace));
        }

        // 调用默认 hook（输出到 stderr）
        default_hook(info);
    }));
}

/// 写入 INFO 级别日志
pub fn info(msg: &str) {
    log(LogLevel::Info, msg);
}

/// 写入 WARN 级别日志
pub fn warn(msg: &str) {
    log(LogLevel::Warn, msg);
}

/// 写入 ERROR 级别日志
pub fn error(msg: &str) {
    log(LogLevel::Error, msg);
}

/// 写入日志
fn log(level: LogLevel, msg: &str) {
    // 同时输出到 stderr（方便调试）
    eprintln!("[{}] {}", level.as_str(), msg);

    // 写入文件
    if let Some(mutex) = LOGGER.get() {
        if let Ok(mut guard) = mutex.lock() {
            if let Some(file) = guard.as_mut() {
                let timestamp = format_timestamp(SystemTime::now());
                let line = format!("[{}] [{}] {}\n", timestamp, level.as_str(), msg);
                let _ = file.write_all(line.as_bytes());
                let _ = file.flush();
            }
        }
    }
}

/// 获取最近的日志文件路径
pub fn latest_log_file() -> Option<PathBuf> {
    let dir = log_dir();
    if !dir.is_dir() {
        return None;
    }

    let mut logs: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("maclean_") && name.ends_with(".log") {
                logs.push((name, entry.path()));
            }
        }
    }

    if logs.is_empty() {
        return None;
    }

    // 按文件名排序（文件名包含日期）
    logs.sort_by(|a, b| b.0.cmp(&a.0));
    Some(logs[0].1.clone())
}

// 2026-09-18 删除了 `total_log_size`：零引用。

/// 清理过期日志（保留 7 天）
fn clean_old_logs(log_dir: &PathBuf) {
    let cutoff = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(7 * 24 * 60 * 60); // 7 天前

    if let Ok(entries) = fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // 只清理 maclean_YYYY-MM-DD.log 文件
            if !name.starts_with("maclean_") || !name.ends_with(".log") {
                continue;
            }

            // 按修改时间判断
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if let Ok(age) = modified.duration_since(UNIX_EPOCH) {
                        if age.as_secs() < cutoff {
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
            }
        }
    }
}

// =========================================================================
//  时间格式化（不依赖 chrono，手动计算）
// =========================================================================

/// 本地时区时间分量（Unix 用 localtime_r；其他平台回退 UTC）
///
/// 返回 (年, 月, 日, 时, 分, 秒)。日志时间戳必须用本地时间，
/// 否则用户看到的日志比真实时间慢 8 小时（UTC 无偏移），
/// 会被误读为"日志没在写/停在某时刻"。
fn localtime_fields(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    #[cfg(unix)]
    {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t = secs as i64;
        unsafe {
            libc::localtime_r(&t, &mut tm);
        }
        let year = (tm.tm_year + 1900) as u32;
        let month = (tm.tm_mon + 1) as u32;
        (
            year,
            month,
            tm.tm_mday as u32,
            tm.tm_hour as u32,
            tm.tm_min as u32,
            tm.tm_sec as u32,
        )
    }
    #[cfg(not(unix))]
    {
        let (y, m, d) = secs_to_date(secs);
        let tod = secs % 86400;
        (
            y,
            m,
            d,
            (tod / 3600) as u32,
            ((tod % 3600) / 60) as u32,
            (tod % 60) as u32,
        )
    }
}

/// 格式化日期: YYYY-MM-DD
fn format_date(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (year, month, day, _, _, _) = localtime_fields(secs);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// 格式化时间戳: YYYY-MM-DD HH:MM:SS
fn format_timestamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (year, month, day, hour, minute, second) = localtime_fields(secs);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hour, minute, second
    )
}

/// Unix 时间戳转日期 (年, 月, 日)
///
/// 使用算法：https://howardhinnant.github.io/date_algorithms.html
/// 仅非 Unix 平台回退路径与测试引用，Unix 走 localtime_r。
#[cfg_attr(unix, allow(dead_code))]
fn secs_to_date(secs: u64) -> (u32, u32, u32) {
    let days = (secs / 86400) as i64;

    // Howard Hinnant's algorithm
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]

    let year = if m <= 2 { y + 1 } else { y };
    (year as u32, m as u32, d as u32)
}

/// Unix 时间戳转时间 (时, 分, 秒) - 本地时区
///
/// 注意：使用本地时区（localtime_r）。日志时间戳若用 UTC，
/// 用户看到的日志会比真实时间慢 8 小时，误以为日志未写入。
/// 仅非 Unix 平台回退路径与测试引用，Unix 走 localtime_r。
#[cfg_attr(unix, allow(dead_code))]
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn secs_to_time(secs: u64) -> (u32, u32, u32) {
    let (_, _, _, hour, minute, second) = localtime_fields(secs);
    (hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 串行化「改 LOG_DIR_ENV 的用例」：env 是进程级共享状态。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_secs_to_date() {
        // 2026-01-01 00:00:00 UTC = 1767225600
        let (y, m, d) = secs_to_date(1767225600);
        assert_eq!(y, 2026);
        assert_eq!(m, 1);
        assert_eq!(d, 1);
    }

    #[test]
    fn test_secs_to_date_known() {
        // 2024-07-13 00:00:00 UTC = 1720828800
        let (y, m, d) = secs_to_date(1720828800);
        assert_eq!(y, 2024);
        assert_eq!(m, 7);
        assert_eq!(d, 13);
    }

    #[test]
    fn test_secs_to_time() {
        // 本地时区：具体时分随机器 TZ 变化，只验证分量合法
        let (h, m, s) = secs_to_time(1720828800);
        assert!(h < 24 && m < 60 && s < 60);
    }

    #[test]
    fn test_format_date() {
        // 本地时区：日期可能随 TZ 前后漂移一天，只验证格式
        let date = format_date(SystemTime::now());
        assert_eq!(date.len(), 10);
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
    }

    /// 幂等契约：`init_with_version` 被调多次也只留一条横幅、一套 panic hook。
    ///
    /// 真实触发场景不是 App 启动（那是每次进程一次），而是库被上层重复初始化：
    /// 旧实现把 `LOGGER.set` 失败的返回值丢掉、横幅照写，于是每次多余的 init
    /// 都会多一条横幅，并多叠一层 panic hook（一次 panic 记 N 遍）。
    #[test]
    fn repeated_init_writes_one_banner_and_one_hook() {
        // 环境变量是进程级共享状态：串行化，避免与另一个改 env 的用例互相踩。
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("maclean_logger_idem_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        std::env::set_var(LOG_DIR_ENV, &dir);

        init_with_version("9.9.9");
        init_with_version("9.9.9");
        init_with_version("9.9.9");

        let file = dir.join(format!("maclean_{}.log", format_date(SystemTime::now())));
        let text = fs::read_to_string(&file).expect("应当写出日志文件");
        assert_eq!(
            text.matches("maclean v9.9.9 启动").count(),
            1,
            "重复 init 不得重复写横幅，实际日志:\n{text}"
        );

        // 横幅压成一行，但排障要的信息一项都不能少：版本 / 平台 / 架构 / 日志位置。
        let banner = text
            .lines()
            .find(|l| l.contains("maclean v9.9.9 启动"))
            .expect("应有横幅行");
        assert!(
            banner.contains(std::env::consts::OS),
            "横幅缺平台: {banner}"
        );
        assert!(
            banner.contains(std::env::consts::ARCH),
            "横幅缺架构: {banner}"
        );
        assert!(
            banner.contains(&dir.display().to_string()),
            "横幅缺日志路径: {banner}"
        );

        std::env::remove_var(LOG_DIR_ENV);
        fs::remove_dir_all(&dir).ok();
    }

    /// 环境变量必须能覆盖已经缓存过的默认路径，否则测试无法把日志调离真实目录。
    #[test]
    fn log_dir_env_overrides_cached_default() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = log_dir(); // 先把默认路径缓存进去
        let dir = std::env::temp_dir().join(format!("maclean_logdir_env_{}", std::process::id()));
        std::env::set_var(LOG_DIR_ENV, &dir);
        assert_eq!(log_dir(), dir);
        std::env::remove_var(LOG_DIR_ENV);
    }
}
