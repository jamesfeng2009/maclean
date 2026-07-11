//! APFS 快照管理模块
//!
//! 负责扫描和管理 macOS 上的 APFS 本地快照和 iOS 模拟器运行时:
//! - 使用 `tmutil listlocalsnapshots /` 列出 Time Machine 本地快照
//! - 使用 `xcrun simctl runtime list` 列出模拟器运行时
//! - 提供删除快照和运行时的方法

use std::process::Command;
use std::time::Instant;

use super::{Recommend, ScanItem, ScanResult, Scanner};

/// APFS 快照扫描器
#[derive(Debug, Default)]
pub struct ApfsScanner;

impl ApfsScanner {
    /// 创建新的 APFS 快照扫描器实例
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for ApfsScanner {
    fn scan(&self) -> ScanResult {
        let start = Instant::now();
        let mut items = Vec::new();

        // 扫描 Time Machine 本地快照
        items.extend(scan_local_snapshots());

        // 扫描 iOS 模拟器运行时
        items.extend(scan_simulator_runtimes());

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        let scan_time_ms = start.elapsed().as_millis() as u64;

        ScanResult {
            items,
            total_size,
            scan_time_ms,
        }
    }
}

// =========================================================================
//  Time Machine 本地快照
// =========================================================================

/// 扫描 Time Machine 本地快照
///
/// 执行 `tmutil listlocalsnapshots /` 命令，解析输出中的快照名称。
/// 由于无法直接获取快照占用大小，size_bytes 设为 0（UI 上标注"未知"）。
fn scan_local_snapshots() -> Vec<ScanItem> {
    let mut items = Vec::new();

    // 执行 tmutil 命令列出本地快照
    let output = Command::new("tmutil")
        .arg("listlocalsnapshots")
        .arg("/")
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => return items, // tmutil 不可用，直接返回空列表
    };

    // 解析命令输出
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        // 快照名称格式如: com.apple.TimeMachine.2024-01-15-100000.local
        // 有时输出行可能包含前缀，如: "Snapshot of volume Macintosh HD: com.apple.TimeMachine...."
        if line.contains("com.apple.TimeMachine.") {
            // 提取快照名称（从 "com.apple.TimeMachine." 开始到行尾）
            let name = if let Some(idx) = line.find("com.apple.TimeMachine.") {
                line[idx..].to_string()
            } else {
                line.to_string()
            };

            items.push(ScanItem {
                path: name,
                size_bytes: 0, // 快照大小无法直接获取，UI 上标注"未知"
                category: "APFS快照".to_string(),
                selected: false,
                deletable: true,
                recommend: Recommend::Safe,
                description: "Time Machine 本地快照，可安全删除".to_string(),
            });
        }
    }

    items
}

/// 删除 Time Machine 本地快照
///
/// 从快照名称中提取时间戳，执行 `tmutil deletelocalsnapshots <timestamp>`。
///
/// # 参数
/// - `name`: 快照名称，如 "com.apple.TimeMachine.2024-01-15-100000.local"
///
/// # 返回
/// - `Ok(())`: 删除成功
/// - `Err(String)`: 删除失败，包含错误信息
pub fn delete_snapshot(name: &str) -> Result<(), String> {
    // 从快照名称中提取时间戳
    // 名称格式: com.apple.TimeMachine.2024-01-15-100000.local
    // 时间戳: 2024-01-15-100000
    let timestamp = name
        .strip_prefix("com.apple.TimeMachine.")
        .and_then(|s| s.strip_suffix(".local"))
        .unwrap_or(name);

    // 执行 tmutil deletelocalsnapshots 命令
    let output = Command::new("tmutil")
        .arg("deletelocalsnapshots")
        .arg(timestamp)
        .output()
        .map_err(|e| format!("执行 tmutil 失败: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("删除快照失败: {}", stderr.trim()))
    }
}

// =========================================================================
//  iOS 模拟器运行时
// =========================================================================

/// 扫描 iOS 模拟器运行时
///
/// 执行 `xcrun simctl runtime list` 命令，解析输出中的运行时 UUID。
/// 由于无法直接获取运行时占用大小，size_bytes 设为 0。
fn scan_simulator_runtimes() -> Vec<ScanItem> {
    let mut items = Vec::new();

    // 执行 xcrun simctl runtime list 命令
    let output = Command::new("xcrun")
        .args(["simctl", "runtime", "list"])
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => return items, // xcrun 不可用，直接返回空列表
    };

    // 解析命令输出，查找 UUID
    let stdout = String::from_utf8_lossy(&output.stdout);

    // 记录当前上下文（如 "== Runtime: iOS 17.2 ==" 这样的行）
    // 用于在 UUID 附近提取运行时名称信息
    let mut current_context = String::new();
    for line in stdout.lines() {
        let trimmed = line.trim();

        // 更新上下文（遇到 "== " 开头或包含 "Runtime:" 的行时更新）
        if trimmed.starts_with("== ") || trimmed.contains("Runtime:") {
            current_context = trimmed.to_string();
        }

        // 在当前行中查找 UUID
        if let Some(uuid) = extract_uuid(trimmed) {
            // path 字段存储 UUID（删除时需要用 UUID）
            // 如果有上下文信息，将其附加在 UUID 前面以便用户识别
            let display_path = if current_context.is_empty() {
                uuid.clone()
            } else {
                format!("{} | {}", current_context, uuid)
            };

            items.push(ScanItem {
                path: display_path,
                size_bytes: 0, // 运行时大小无法直接获取
                category: "模拟器运行时".to_string(),
                selected: false,
                deletable: true,
                recommend: Recommend::Caution,
                description: "iOS 模拟器运行时，删除后需重新下载".to_string(),
            });
        }
    }

    items
}

/// 删除 iOS 模拟器运行时
///
/// 执行 `xcrun simctl runtime delete <uuid>` 命令。
///
/// # 参数
/// - `uuid`: 运行时的 UUID
///
/// # 返回
/// - `Ok(())`: 删除成功
/// - `Err(String)`: 删除失败，包含错误信息
pub fn delete_simulator_runtime(uuid: &str) -> Result<(), String> {
    // 如果 path 中包含了上下文信息（格式为 "context | uuid"），提取 UUID 部分
    let uuid = uuid.rsplit(" | ").next().unwrap_or(uuid);

    let output = Command::new("xcrun")
        .args(["simctl", "runtime", "delete", uuid])
        .output()
        .map_err(|e| format!("执行 xcrun 失败: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("删除运行时失败: {}", stderr.trim()))
    }
}

// =========================================================================
//  辅助函数
// =========================================================================

/// 从文本中提取 UUID
///
/// UUID 格式: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx (8-4-4-4-12 十六进制字符)
/// 在文本中搜索匹配此模式的子串。
fn extract_uuid(text: &str) -> Option<String> {
    let len = text.len();
    if len < 36 {
        return None;
    }

    for i in 0..=(len - 36) {
        // 使用 get 方法避免 UTF-8 边界 panic
        if let Some(candidate) = text.get(i..i + 36) {
            if is_valid_uuid(candidate) {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

/// 验证字符串是否为有效的 UUID 格式
///
/// UUID 格式: 8-4-4-4-12 十六进制字符，用连字符分隔
/// 如: 12345678-1234-1234-1234-123456789abc
fn is_valid_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    // 各部分长度应为 8, 4, 4, 4, 12
    let lengths = [8, 4, 4, 4, 12];
    parts
        .iter()
        .zip(lengths.iter())
        .all(|(part, &expected_len)| {
            part.len() == expected_len && part.chars().all(|c| c.is_ascii_hexdigit())
        })
}
