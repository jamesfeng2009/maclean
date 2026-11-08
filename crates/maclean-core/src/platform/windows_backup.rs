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

// F-09 / B-06：清单结构定义移到无 cfg 的 reg_safety 模块，便于本机行为测试；
// 这里保留原公开名字，外部引用不变。
pub use super::reg_safety::{RegBackupEntry as BackupEntry, RegBackupManifest as BackupManifest};

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

/// 读取备份清单（F-09）
///
/// 损坏文件返回 Err，绝不 `unwrap_or_default` 静默清空 —— manifest.json 位于
/// 用户可写的 %APPDATA%，解析失败被当空清单会让所有备份条目凭空消失。
pub fn load_manifest() -> Result<BackupManifest, String> {
    let path = manifest_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => super::reg_safety::parse_reg_manifest(&content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BackupManifest::default()),
        Err(e) => Err(format!("读取备份清单失败: {}", e)),
    }
}

/// 保存备份清单（F-09）：复用共用原子写，避免半截 JSON 与并发交叉写。
fn save_manifest(manifest: &BackupManifest) {
    let path = manifest_path();
    match serde_json::to_string_pretty(manifest) {
        Ok(json) => {
            if let Err(e) = crate::atomic_write::atomic_write(&path, json.as_bytes()) {
                crate::logger::error(&format!("保存备份清单失败: {}", e));
            }
        }
        Err(e) => crate::logger::error(&format!("序列化备份清单失败: {}", e)),
    }
}

/// F-09：跨进程串行化清单读改写（create_new 占位锁，争用超时则尽力而为）。
fn with_manifest_lock<T>(f: impl FnOnce() -> T) -> T {
    let _ = std::fs::create_dir_all(backup_dir());
    let lock = backup_dir().join("manifest.lock");
    for _ in 0..50 {
        if std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .is_ok()
        {
            let out = f();
            let _ = std::fs::remove_file(&lock);
            return out;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    crate::logger::warn("备份清单进程锁争用超时，仍按原子写尽力而为");
    f()
}

/// 当前时间戳字符串（yyyy-MM-dd HH:mm:ss）
///
/// F-09：powershell 失败或输出为空时，用 SystemTime 回退且**永不返回空串**，
/// 否则文件名会退化成 `_<source>_<key>.reg` 被 `reg export /y` 覆盖旧备份。
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
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                super::reg_safety::fallback_file_stamp()
            } else {
                s
            }
        }
        _ => super::reg_safety::fallback_file_stamp(),
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
    let base = format!("{}_{}_{}", file_timestamp(), source, key_short);
    // F-09：文件名唯一化，绝不覆盖已存在的 .reg 备份（即使时间戳退化）。
    let mut file_name = format!("{}.reg", base);
    for i in 1..1000u32 {
        if !dir.join(&file_name).exists() {
            break;
        }
        file_name = format!("{}_{}.reg", base, i);
    }
    let file_path = dir.join(&file_name);

    let output = Command::new("reg")
        .args(["export", key_path, file_path.to_str()?, "/y"])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let path_str = file_path.to_string_lossy().to_string();
            let stamp = now_timestamp();
            // F-09：读改写串行化；损坏清单绝不能被当成空清单继续追加。
            let ok = with_manifest_lock(|| {
                let mut manifest = match load_manifest() {
                    Ok(m) => m,
                    Err(e) => {
                        crate::logger::error(&format!("{}，拒绝追加备份条目", e));
                        return false;
                    }
                };
                manifest.entries.push(BackupEntry {
                    timestamp: stamp.clone(),
                    source: source.to_string(),
                    reg_file: path_str.clone(),
                    reg_key: key_path.to_string(),
                });
                save_manifest(&manifest);
                true
            });
            if !ok {
                return None;
            }
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
    // F-09：清单损坏时返回 None，不按空清单静默处理。
    let manifest = load_manifest().ok()?;
    if manifest.entries.is_empty() {
        return None;
    }
    // B-06：按 (timestamp, source) 分组，只统计最后一批的数量。
    let keys: Vec<(String, String)> = manifest
        .entries
        .iter()
        .map(|e| (e.timestamp.clone(), e.source.clone()))
        .collect();
    let batch = super::reg_safety::last_backup_batch(&keys);
    let last = *batch.last()?;
    let e = &manifest.entries[last];
    Some((e.timestamp.clone(), e.source.clone(), batch.len()))
}

/// 还原上次修改
///
/// 导入最近一批（同 source）的 .reg 备份文件。
/// 注意：reg import 只能恢复被覆盖的旧值；备份时不存在、后被新增的键值
/// 不会被移除（这是 .reg 格式的固有限制，UI 需向用户说明）。
///
/// 安全防范：
/// - .reg 文件必须位于本工具的备份目录内（canonicalize 后判定，挡 `..` 与软链）
/// - 文件内容声明的键必须全部落在该项记录的 reg_key 子树内
/// - reg_key 的根 hive 必须在预期集合里，避免子树检查被退化成恒真
///
/// 返回 (success, message)
pub fn restore_last_backup() -> (bool, String) {
    let manifest = match load_manifest() {
        Ok(m) => m,
        // F-09：清单损坏时如实报错，绝不按空清单静默"无备份/还原成功"。
        Err(e) => return (false, e),
    };
    if manifest.entries.is_empty() {
        return (false, "none".to_string());
    }

    // B-06：只取最后一批（同 timestamp + source），不再把历史上所有同
    // source 的 .reg 全 import —— 那会把更早的旧值一起写回。
    let keys: Vec<(String, String)> = manifest
        .entries
        .iter()
        .map(|e| (e.timestamp.clone(), e.source.clone()))
        .collect();
    let batch_idx = super::reg_safety::last_backup_batch(&keys);
    let Some(&last) = batch_idx.last() else {
        return (false, "none".to_string());
    };
    let last_source = manifest.entries[last].source.clone();
    let batch: Vec<&BackupEntry> = batch_idx.iter().map(|&i| &manifest.entries[i]).collect();

    let registry_dir = registry_backup_dir();
    // canonicalize 一次作为比较基准；取不到就直接放弃本次还原，
    // 而不是退回"按原始字符串比较"那种可被 ../ 绕过的判断。
    let Ok(canonical_root) = registry_dir.canonicalize() else {
        crate::logger::warn("备份目录不可用，放弃还原");
        return (false, "backup_dir_missing".to_string());
    };

    let mut success_count = 0;
    let mut fail_count = 0;
    for entry in &batch {
        let file = std::path::Path::new(&entry.reg_file);

        // ① .reg 文件必须躺在我们的备份目录里，且 canonicalize 后仍在（挡 ../ 与软链）
        let Ok(canonical_file) = file.canonicalize() else {
            fail_count += 1;
            continue;
        };
        if !super::reg_safety::file_is_within_backup_dir(&canonical_file, &canonical_root) {
            crate::logger::warn(&format!(
                "备份文件不在备份目录内，拒绝导入: {}",
                entry.reg_file
            ));
            fail_count += 1;
            continue;
        }

        // ② 文件内容必须只触碰 entry 声明的那棵子树。
        //    manifest.json 在用户可写的 %APPDATA% 下，不校验的话改写它或换掉
        //    .reg 文件就能借"还原"往任意注册表位置写东西。
        let Ok(content) = std::fs::read_to_string(&canonical_file) else {
            fail_count += 1;
            continue;
        };
        if let Err(reason) =
            super::reg_safety::validate_backup_entry_content(&entry.reg_key, &content)
        {
            crate::logger::warn(&format!(
                "拒绝导入不安全的备份 {}: {}",
                entry.reg_file, reason
            ));
            fail_count += 1;
            continue;
        }

        // B-08：canonicalize / 目录内判定 / 内容校验全部基于 canonical_file，
        // 执行 reg import 也必须用同一个路径，不能用 manifest 里的原始字符串
        // （那会绕过所有校验，指向任意文件）。
        let canonical_str = canonical_file.to_string_lossy().to_string();
        let output = Command::new("reg")
            .args(["import", &canonical_str])
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
