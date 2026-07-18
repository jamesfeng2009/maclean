//! Windows 操作前备份模块（P1 系列）
//!
//! 借鉴 Win11Debloat (https://github.com/Raphire/Win11Debloat) 的安全策略：
//! - P1-1: 系统还原点 `ensure_restore_point()` —— 批次操作前自动创建（20h 频率限制）
//! - P1-2: 注册表备份 `backup_registry_key()` —— reg export 到备份目录
//! - P1-3: 备份清单 + 还原 `restore_last_backup()` —— 供设置页"还原上次修改"使用
//!
//! 备份目录: %APPDATA%\maclean\backup\
//!   ├── registry\yyyyMMdd_HHmmss_<source>_<idx>.reg   注册表备份文件
//!   ├── manifest.json                                  备份清单
//!   └── last_restore_point.txt                         上次还原点创建时间戳

use std::path::PathBuf;
use std::process::Command;

/// 还原点自动创建的最小间隔（秒）：20 小时
/// Windows 系统默认 24h 内只创建一个还原点，我们略小于该值避免无用调用
const RESTORE_POINT_INTERVAL_SECS: u64 = 20 * 60 * 60;

// =========================================================================
//  备份清单
// =========================================================================

/// 备份清单条目（一次注册表备份的记录）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackupEntry {
    /// 时间戳字符串（yyyy-MM-dd HH:mm:ss，用于显示）
    pub timestamp: String,
    /// 备份来源（如 win_disable_telemetry）
    pub source: String,
    /// .reg 备份文件完整路径
    pub reg_file: String,
    /// 原始注册表键路径（如 HKLM\SOFTWARE\...）
    pub reg_key: String,
}

/// 备份清单
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BackupManifest {
    pub entries: Vec<BackupEntry>,
}

/// 备份根目录：%APPDATA%\maclean\backup\
pub fn backup_dir() -> PathBuf {
    crate::platform::app_data_dir().join("backup")
}

/// 注册表备份目录：backup\registry\
fn registry_backup_dir() -> PathBuf {
    backup_dir().join("registry")
}

/// 清单文件路径
fn manifest_path() -> PathBuf {
    backup_dir().join("manifest.json")
}

/// 读取备份清单
pub fn load_manifest() -> BackupManifest {
    let path = manifest_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        BackupManifest::default()
    }
}

/// 保存备份清单
fn save_manifest(manifest: &BackupManifest) {
    let path = manifest_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(manifest) {
        let _ = std::fs::write(&path, json);
    }
}

/// 当前时间戳字符串（yyyy-MM-dd HH:mm:ss）
fn now_timestamp() -> String {
    // 用 PowerShell 获取本地时间格式化（避免引入 chrono 依赖）
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Date -Format 'yyyy-MM-dd HH:mm:ss'",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

/// 文件命名用时间戳（yyyyMMdd_HHmmss）
fn file_timestamp() -> String {
    now_timestamp().replace(['-', ':'], "").replace(' ', "_")
}

// =========================================================================
//  P1-1: 系统还原点
// =========================================================================

/// 确保系统还原点存在
///
/// - `force = false`（自动调用）：距上次创建 < 20h 则跳过
/// - `force = true`（用户手动触发）：跳过频率检查，强制创建
///
/// 流程：启用系统还原（尽力）→ Checkpoint-Computer → 记录时间戳
///
/// 返回 (created, message)
pub fn ensure_restore_point(force: bool) -> (bool, String) {
    let stamp_file = backup_dir().join("last_restore_point.txt");

    // 频率检查
    if !force {
        if let Ok(content) = std::fs::read_to_string(&stamp_file) {
            if let Ok(last) = content.trim().parse::<u64>() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if now.saturating_sub(last) < RESTORE_POINT_INTERVAL_SECS {
                    return (true, "skipped_recent".to_string());
                }
            }
        }
    }

    // 1. 启用系统还原（幂等，需管理员；失败不阻塞，让 Checkpoint-Computer 自己报错）
    let _ = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Enable-ComputerRestore -Drive $env:SystemDrive -ErrorAction SilentlyContinue",
        ])
        .output();

    // 2. 创建还原点
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Checkpoint-Computer -Description 'maclean auto backup' -RestorePointType 'MODIFY_SETTINGS' -ErrorAction Stop",
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            // 3. 记录时间戳
            if let Some(parent) = stamp_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = std::fs::write(&stamp_file, now.to_string());
            crate::logger::info("系统还原点已创建: maclean auto backup");
            (true, "created".to_string())
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            crate::logger::error(&format!("创建系统还原点失败: {}", stderr.trim()));
            (false, format!("failed: {}", stderr.trim()))
        }
        Err(e) => {
            crate::logger::error(&format!("执行 Checkpoint-Computer 失败: {}", e));
            (false, format!("failed: {}", e))
        }
    }
}

// =========================================================================
//  P1-2: 注册表备份
// =========================================================================

/// 备份注册表键到 .reg 文件
///
/// 执行 `reg export <key> <file> /y`，成功后追加到备份清单。
/// 键不存在时 reg export 会失败，此时跳过（不视为错误，仅记日志）。
///
/// 返回 Some(reg_file_path) 表示备份成功，None 表示键不存在或导出失败。
pub fn backup_registry_key(key_path: &str, source: &str) -> Option<String> {
    let dir = registry_backup_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }

    // 文件名：yyyyMMdd_HHmmss_<source>_<键名简化>.reg
    let key_short: String = key_path
        .rsplit('\\')
        .next()
        .unwrap_or("key")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    let file_name = format!("{}_{}_{}.reg", file_timestamp(), source, key_short);
    let file_path = dir.join(&file_name);

    let output = Command::new("reg")
        .args(["export", key_path, file_path.to_str()?, "/y"])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let path_str = file_path.to_string_lossy().to_string();
            // 追加到清单
            let mut manifest = load_manifest();
            manifest.entries.push(BackupEntry {
                timestamp: now_timestamp(),
                source: source.to_string(),
                reg_file: path_str.clone(),
                reg_key: key_path.to_string(),
            });
            save_manifest(&manifest);
            crate::logger::info(&format!("注册表已备份: {} -> {}", key_path, file_name));
            Some(path_str)
        }
        _ => {
            // 键不存在（reg export 返回非零），静默跳过
            crate::logger::info(&format!("注册表键不存在或导出失败（跳过）: {}", key_path));
            None
        }
    }
}

// =========================================================================
//  P1-3: 还原上次修改
// =========================================================================

/// 最近一次备份的摘要信息（供 UI 显示）
///
/// 返回 (备份时间, 来源, 备份键数)，无备份返回 None
pub fn last_backup_summary() -> Option<(String, String, usize)> {
    let manifest = load_manifest();
    if manifest.entries.is_empty() {
        return None;
    }
    // 按时间戳取最后一批（同一 source + 相邻时间视为一批，简化为取最后一个 source 的所有条目）
    let last_source = manifest.entries.last()?.source.clone();
    let last_time = manifest.entries.last()?.timestamp.clone();
    let count = manifest
        .entries
        .iter()
        .filter(|e| e.source == last_source)
        .count();
    Some((last_time, last_source, count))
}

/// 还原上次修改
///
/// 导入最近一批（同 source）的 .reg 备份文件。
/// 注意：reg import 只能恢复被覆盖的旧值；备份时不存在、后被新增的键值
/// 不会被移除（这是 .reg 格式的固有限制，UI 需向用户说明）。
///
/// 返回 (success, message)
pub fn restore_last_backup() -> (bool, String) {
    let manifest = load_manifest();
    if manifest.entries.is_empty() {
        return (false, "none".to_string());
    }

    let last_source = manifest.entries.last().map(|e| e.source.clone()).unwrap();
    let batch: Vec<&BackupEntry> = manifest
        .entries
        .iter()
        .filter(|e| e.source == last_source)
        .collect();

    let mut success_count = 0;
    let mut fail_count = 0;
    for entry in &batch {
        // 检查备份文件仍存在
        if !std::path::Path::new(&entry.reg_file).exists() {
            fail_count += 1;
            continue;
        }
        let output = Command::new("reg")
            .args(["import", &entry.reg_file])
            .output();
        match output {
            Ok(o) if o.status.success() => success_count += 1,
            _ => fail_count += 1,
        }
    }

    crate::logger::info(&format!(
        "还原备份 {}: 成功 {} / 失败 {}",
        last_source, success_count, fail_count
    ));

    if fail_count == 0 && success_count > 0 {
        (true, format!("{} ({})", last_source, success_count))
    } else if success_count > 0 {
        (
            false,
            format!("partial: {} ok / {} fail", success_count, fail_count),
        )
    } else {
        (false, "failed".to_string())
    }
}
