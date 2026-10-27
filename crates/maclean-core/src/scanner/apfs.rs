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
                batch_mtimes: vec![],
                path: name,
                size_bytes: 0, // 快照大小无法直接获取，UI 上标注"未知"
                category: "APFS快照".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                // P2 修复：Time Machine 本地快照删除后不可恢复，且 size 未知（显示 0），
                // 标成 Safe 会被"智能选择"默认勾选，用户在不知道删了什么的情况下失去快照。
                // 降为 Caution：不默认勾选，UI 上明确提示不可恢复。
                recommend: Recommend::Caution,
                description: "Time Machine 本地快照，删除后不可恢复（大小未知，系统未报告）"
                    .to_string(),
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
    let timestamp = extract_snapshot_timestamp(name);
    // 快照名来自 tmutil 输出或 CLI 参数：进入 shell/AppleScript 之前必须校验，
    // 否则一个带引号/分号的"名字"就能变成任意命令执行。
    if !is_safe_snapshot_timestamp(timestamp) {
        return Err(format!("非法的快照时间戳: {}", timestamp));
    }

    let output = Command::new("tmutil")
        .arg("deletelocalsnapshots")
        .arg(timestamp)
        .output()
        .map_err(|e| format!("执行 tmutil 失败: {}", e))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if is_permission_denied(&stderr) {
        #[cfg(target_os = "macos")]
        {
            // 非 root 时 tmutil 会拒绝删除本地快照（即使文件属主是当前用户）。
            // 走系统授权弹窗（Touch ID / 密码），成功后仍是同一个 tmutil 命令。
            if !is_euid_root() {
                return delete_snapshot_with_privileges(timestamp);
            }
        }
    }
    Err(format!(
        "删除快照失败: {}（可执行 sudo tmutil deletelocalsnapshots {} 重试）",
        stderr, timestamp
    ))
}

/// 从快照名提取 tmutil 需要的时间戳
///
/// 输入 `com.apple.TimeMachine.2024-01-15-100000.local` → `2024-01-15-100000`。
/// 已经是时间戳的原样返回（幂等）。
pub fn extract_snapshot_timestamp(name: &str) -> &str {
    name.strip_prefix("com.apple.TimeMachine.")
        .and_then(|s| s.strip_suffix(".local"))
        .unwrap_or(name)
}

/// 快照时间戳白名单：数字、字母、`-`、`_`、`.`
///
/// tmutil 时间戳形如 `2024-01-15-100000`；不允许任何 shell/AppleScript 元字符。
pub fn is_safe_snapshot_timestamp(ts: &str) -> bool {
    !ts.is_empty()
        && ts.len() <= 64
        && ts
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 判断 tmutil 的报错是否属于"需要管理员权限"
pub fn is_permission_denied(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("operation not permitted")
        || s.contains("permission denied")
        || s.contains("must be run as root")
        || s.contains("not authorized")
        || s.contains("authorization")
}

#[cfg(target_os = "macos")]
fn is_euid_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// 通过系统授权弹窗执行 `tmutil deletelocalsnapshots`
///
/// `osascript ... with administrator privileges` 会弹 Touch ID / 密码框，
/// 等价于用户手动执行 `sudo tmutil deletelocalsnapshots <ts>`。
#[cfg(target_os = "macos")]
fn delete_snapshot_with_privileges(timestamp: &str) -> Result<(), String> {
    let shell_cmd = format!("/usr/bin/tmutil deletelocalsnapshots {}", timestamp);
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        shell_cmd.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("无法发起管理员授权: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "删除快照需要管理员权限，授权未完成或被取消: {}（也可执行 sudo tmutil deletelocalsnapshots {}）",
            err, timestamp
        ))
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
    // -j 输出稳定的 JSON 映射（UUID -> 元数据）。旧实现按行抓 UUID，
    // 会把 parentIdentifier 之类的无关 UUID 一起当成 runtime；JSON 解析
    // 还能直接拿到系统标注的 deletable 与 sizeBytes。
    let output = Command::new("xcrun")
        .args(["simctl", "runtime", "list", "-j"])
        .output();

    let Ok(output) = output else {
        return Vec::new(); // xcrun 不可用
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_simulator_runtimes_json(&output.stdout)
}

/// `xcrun simctl runtime list -j` 的单条元数据
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimRuntimeMeta {
    #[serde(default)]
    runtime_identifier: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    deletable: Option<bool>,
    #[serde(default)]
    size_bytes: Option<u64>,
    #[serde(default)]
    mount_path: Option<String>,
}

/// 解析 `xcrun simctl runtime list -j`（纯函数，便于单测）
///
/// 只保留系统标记为可删除的运行时候选；正在使用/被依赖的 runtime 不列出，
/// 避免用户勾选后删除失败。`path` 格式为 `显示名 | UUID`，删除时取右半段。
pub fn parse_simulator_runtimes_json(raw: &[u8]) -> Vec<ScanItem> {
    let Ok(map) = serde_json::from_slice::<std::collections::BTreeMap<String, SimRuntimeMeta>>(raw)
    else {
        return Vec::new();
    };

    map.into_iter()
        .filter(|(_, m)| m.deletable.unwrap_or(false))
        .map(|(uuid, m)| {
            let label = match (m.platform.as_deref(), m.version.as_deref()) {
                (Some(p), Some(v)) => format!("{} {}", p, v),
                _ => m
                    .runtime_identifier
                    .clone()
                    .unwrap_or_else(|| "iOS Simulator Runtime".to_string()),
            };
            let display_path = format!("{} | {}", label, uuid);
            ScanItem {
                batch_mtimes: vec![],
                path: display_path,
                // 有 sizeBytes 就用真实大小；老版本没有该字段时仍标 0（未知）
                size_bytes: m.size_bytes.unwrap_or(0),
                category: "模拟器运行时".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: format!(
                    "iOS 模拟器运行时{}，删除后需重新下载",
                    m.mount_path
                        .as_deref()
                        .map(|p| format!("（{}）", p))
                        .unwrap_or_default()
                ),
            }
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_timestamp_is_extracted_idempotently() {
        let name = "com.apple.TimeMachine.2024-01-15-100000.local";
        assert_eq!(extract_snapshot_timestamp(name), "2024-01-15-100000");
        assert_eq!(
            extract_snapshot_timestamp("2024-01-15-100000"),
            "2024-01-15-100000"
        );
    }

    #[test]
    fn dangerous_snapshot_names_are_rejected() {
        // 进入 osascript/shell 前必须挡住命令注入
        assert!(is_safe_snapshot_timestamp("2024-01-15-100000"));
        assert!(!is_safe_snapshot_timestamp("2024-01-15-100000; rm -rf /"));
        assert!(!is_safe_snapshot_timestamp("2024-01-15\"quote"));
        assert!(!is_safe_snapshot_timestamp(""));
        assert!(!is_safe_snapshot_timestamp(&"a".repeat(65)));
    }

    #[test]
    fn permission_errors_are_classified() {
        assert!(is_permission_denied("tmutil: Operation not permitted"));
        assert!(is_permission_denied("Permission denied"));
        assert!(is_permission_denied("This command must be run as root"));
        assert!(!is_permission_denied("No snapshot found"));
    }

    #[test]
    fn simulator_runtimes_parse_from_json_not_line_scanning() {
        let raw = br#"{
          "0A1B2C3D-0000-1111-2222-333344445555": {
            "platform": "iOS",
            "version": "17.2",
            "deletable": true,
            "sizeBytes": 1234567890,
            "mountPath": "/Library/Developer/CoreSimulator/Volumes/iOS_22F77"
          },
          "FFFFFFFF-AAAA-BBBB-CCCC-DDDDDDDDDDDD": {
            "platform": "iOS",
            "version": "16.4",
            "deletable": false
          }
        }"#;
        let items = parse_simulator_runtimes_json(raw);
        assert_eq!(items.len(), 1, "不可删除的 runtime 不应列出");
        assert_eq!(items[0].size_bytes, 1_234_567_890);
        assert!(items[0]
            .path
            .ends_with("0A1B2C3D-0000-1111-2222-333344445555"));
        assert_eq!(items[0].category, "模拟器运行时");
    }

    #[test]
    fn invalid_simulator_json_yields_empty_not_garbage() {
        assert!(parse_simulator_runtimes_json(b"not json").is_empty());
    }
}
