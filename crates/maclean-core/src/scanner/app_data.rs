//! App 数据扫描器
//!
//! 扫描 ~/Library/Application Support/、~/Library/Containers/ 与
//! ~/Library/Group Containers/ 下的应用数据目录。
//! 与 App 缓存不同，这些是应用的完整数据目录，删除后可能导致：
//! - 丢失登录状态
//! - 丢失用户配置
//! - 丢失本地数据
//! - App 无法启动
//!
//! 因此所有项标记为 Advanced（高级用户），需要用户明确确认风险。

use std::path::PathBuf;
use std::time::Instant;

use rayon::prelude::*;

use super::{
    dir_size, finalize_scan_result, has_home, home_dir, Recommend, ScanItem, ScanResult, Scanner,
};

/// 100MB 阈值（只有大于此值的目录才展示）
const APP_DATA_MIN: u64 = 100 * 1024 * 1024;

/// App 数据扫描器
#[derive(Debug, Default)]
pub struct AppDataScanner;

impl AppDataScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for AppDataScanner {
    fn scan(&self) -> ScanResult {
        let start = Instant::now();
        let mut items = Vec::new();

        // home 获取不到时不要硬扫（否则会退化为扫描系统目录）
        if !has_home() {
            return finalize_scan_result(Vec::new(), start.elapsed().as_millis() as u64);
        }

        items.extend(scan_application_support());
        items.extend(scan_containers());

        // C-17 · 排序 / 去重 / 体积汇总统一走共享 helper
        finalize_scan_result(items, start.elapsed().as_millis() as u64)
    }
}

/// 不应展示的系统级目录（在 Application Support 下）
const SYSTEM_DIRS: &[&str] = &[
    "Apple",
    "AppleSetup",
    "com.apple",
    "CrashReporter",
    "Dock",
    "FaceTime",
    "iCloud",
    "KeyboardServices",
    "MobileSync",
    "SyncServices",
    "AddressBook",
    "Calendar",
    "Mail",
    "Messages",
    "Notes",
    "Reminders",
    "Safari",
    "Siri",
    "Spotlight",
    "System Preferences",
    "TelephonyUtilities",
    "WebKit",
    "CloudDocs",
    "Caches",
];

/// 扫描 ~/Library/Application Support/ 下的应用数据目录
///
/// **高风险警告**：这些目录包含应用的完整数据，
/// 删除后可能导致 App 丢失配置、登录状态或无法启动。
fn scan_application_support() -> Vec<ScanItem> {
    let home = home_dir();
    let app_support = home.join("Library/Application Support");
    let mut items = Vec::new();

    let entries = match std::fs::read_dir(&app_support) {
        Ok(e) => e,
        Err(e) => {
            // C-17 · 与 scan_containers 同口径：读取失败要如实标注「不可读」，
            // 不能静默返回空让用户以为「确实没有数据」。
            items.push(ScanItem {
                batch_mtimes: vec![],
                path: app_support.to_string_lossy().to_string(),
                size_bytes: 0,
                category: "应用数据".to_string(),
                selected: false,
                deletable: false,
                undeletable_reason: if e.kind() == std::io::ErrorKind::PermissionDenied {
                    "需要「完全磁盘访问权限」读取 Application Support".to_string()
                } else {
                    format!("无法读取目录: {}", e)
                },
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: "应用数据目录（未能读取）".to_string(),
            });
            return items;
        }
    };

    let paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    let sized: Vec<(PathBuf, u64, String)> = paths
        .par_iter()
        .filter_map(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("未知")
                .to_string();

            // 跳过系统级目录
            if is_system_dir(&name) {
                return None;
            }

            let size = dir_size(path);
            if size >= APP_DATA_MIN {
                Some((path.clone(), size, name))
            } else {
                None
            }
        })
        .collect();

    for (path, size, name) in sized {
        items.push(ScanItem {
            batch_mtimes: vec![],
            path: path.to_string_lossy().to_string(),
            size_bytes: size,
            category: name,
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Advanced,
            description:
                "⚠️ 高风险：应用数据目录，删除后可能丢失配置、登录状态或本地数据，App 可能无法启动"
                    .to_string(),
        });
    }

    items
}

/// 扫描沙盒容器：~/Library/Containers 与 ~/Library/Group Containers
///
/// 官网把 App 数据描述成“容器 + 沙盒 + 扩展”，只扫 Application Support
/// 与文案不符。这里补上两个容器目录；读取被 TCC 拒绝时给出明确引导，
/// 而不是静默返回空列表让用户以为没有数据。
fn scan_containers() -> Vec<ScanItem> {
    let home = home_dir();
    let mut out = Vec::new();
    for (rel, label) in [
        ("Library/Containers", "容器"),
        ("Library/Group Containers", "共享容器"),
    ] {
        let base = home.join(rel);
        if !base.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(&base) {
            Ok(e) => e,
            Err(e) => {
                // 典型情况：没有“完全磁盘访问权限”，read_dir 返回 PermissionDenied
                out.push(ScanItem {
                    batch_mtimes: vec![],
                    path: base.to_string_lossy().to_string(),
                    size_bytes: 0,
                    category: label.to_string(),
                    selected: false,
                    deletable: false,
                    undeletable_reason: if e.kind() == std::io::ErrorKind::PermissionDenied {
                        "需要“完全磁盘访问权限”：系统设置 > 隐私与安全性 > 完全磁盘访问权限"
                            .to_string()
                    } else {
                        format!("无法读取目录: {}", e)
                    },
                    batch_paths: Vec::new(),
                    recommend: Recommend::Caution,
                    description: "沙盒容器目录（未能读取）".to_string(),
                });
                continue;
            }
        };
        let paths: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();

        let sized: Vec<(PathBuf, u64, String)> = paths
            .par_iter()
            .filter_map(|path| {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("未知")
                    .to_string();
                // com.apple.* 与系统级目录不动
                if is_system_dir(&name) || name.eq_ignore_ascii_case("com.apple") {
                    return None;
                }
                let size = dir_size(path);
                if size >= APP_DATA_MIN {
                    Some((path.clone(), size, name))
                } else {
                    None
                }
            })
            .collect();

        for (path, size, name) in sized {
            out.push(ScanItem {
                batch_mtimes: vec![],
                path: path.to_string_lossy().to_string(),
                size_bytes: size,
                category: format!("{}: {}", label, name),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Advanced,
                description:
                    "⚠️ 高风险：沙盒容器内的应用本地数据，删除后应用会丢失配置/缓存/登录状态"
                        .to_string(),
            });
        }
    }
    out
}

/// 判断是否为系统级目录（不应展示给用户清理）
fn is_system_dir(name: &str) -> bool {
    for sys in SYSTEM_DIRS {
        if name.eq_ignore_ascii_case(sys) || name.starts_with(&format!("{}.", sys)) {
            return true;
        }
    }
    false
}
