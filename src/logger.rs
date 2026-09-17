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

/// 获取日志目录
pub fn log_dir() -> PathBuf {
    LOG_DIR
        .get_or_init(|| crate::platform::app_data_dir().join("logs"))
        .clone()
}

/// 初始化日志系统
///
/// 在 main() 开头调用。创建日志目录，打开当天的日志文件，
/// 设置 panic hook，清理过期日志。
pub fn init() {
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
            let mutex = Mutex::new(Some(f));
            let _ = LOGGER.set(mutex);

            // 写入启动分隔
            info(&format!(
                "========== maclean v{} 启动 ==========",
                env!("CARGO_PKG_VERSION")
            ));
            info(&format!("日志文件: {}", log_file.display()));
            info(&format!("平台: {}", std::env::consts::OS));
            info(&format!("架构: {}", std::env::consts::ARCH));

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

/// 格式化日期: YYYY-MM-DD
fn format_date(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (year, month, day) = secs_to_date(secs);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// 格式化时间戳: YYYY-MM-DD HH:MM:SS
fn format_timestamp(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (year, month, day) = secs_to_date(secs);
    let (hour, minute, second) = secs_to_time(secs);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hour, minute, second
    )
}

/// Unix 时间戳转日期 (年, 月, 日)
///
/// 使用算法：https://howardhinnant.github.io/date_algorithms.html
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

/// Unix 时间戳转时间 (时, 分, 秒) - UTC
///
/// 注意：这里使用 UTC 时间。对于日志来说，UTC 是可以接受的，
/// 且避免了本地时区转换的复杂性。
fn secs_to_time(secs: u64) -> (u32, u32, u32) {
    let tod = secs % 86400;
    let hour = (tod / 3600) as u32;
    let minute = ((tod % 3600) / 60) as u32;
    let second = (tod % 60) as u32;
    (hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (h, m, s) = secs_to_time(1720828800); // 00:00:00 UTC
        assert_eq!(h, 0);
        assert_eq!(m, 0);
        assert_eq!(s, 0);
    }

    #[test]
    fn test_format_date() {
        let date = format_date(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1720828800));
        assert_eq!(date, "2024-07-13");
    }
}
