//! App 卸载扫描器
//!
//! 扫描 /Applications/ 和 ~/Applications/ 下的 .app 应用包，
//! 计算应用大小并查找关联文件（容器、缓存、偏好设置等）。
//!
//! 卸载是不可逆操作，所有项标记为 Advanced（高级用户），
//! 需用户明确确认风险后才会执行。
//!
//! 关联文件查找范围（基于 bundle ID 和应用名）:
//! - ~/Library/Containers/<bundle_id>/
//! - ~/Library/Group Containers/*<bundle_id>*/  (通配匹配)
//! - ~/Library/Caches/<bundle_id>/
//! - ~/Library/Application Support/<app_name>/
//! - ~/Library/Preferences/<bundle_id>.plist
//! - ~/Library/Preferences/<bundle_id>/
//! - ~/Library/Logs/<app_name>/
//! - ~/Library/Saved Application State/<bundle_id>.savedState/
//! - ~/Library/HTTPStorages/<bundle_id>/

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use rayon::prelude::*;

use super::{dir_size, home_dir, Recommend, ScanItem, ScanResult, Scanner};

/// macOS 系统自带应用名称（不应卸载）
const SYSTEM_APP_NAMES: &[&str] = &[
    "Safari", "Mail", "Notes", "Calendar", "Messages", "FaceTime",
    "Maps", "News", "Stocks", "Weather", "Reminders", "Contacts",
    "Preview", "TextEdit", "Calculator", "Chess", "Stickies",
    "Time Machine", "System Preferences", "System Settings",
    "Photo Booth", "Dictionary", "Font Book", "Grapher",
    "Terminal", "Activity Monitor", "Disk Utility", "Keychain Access",
    "Migration Assistant", "Console", "Automator", "Script Editor",
    "Image Capture", "Screenshot", "QuickTime Player",
    "VoiceOver Utility", "Audio MIDI Setup", "Digital Color Meter",
    "ColorSync Utility", "AirPort Utility", "Bluetooth File Exchange",
    "Tips", "Home", "App Store", "System Information", "Launchpad",
    "Mission Control", "Find My", "Podcasts", "Music", "TV",
    "Photos", "Books", "Freeform", "Shortcuts", "Numbers",
    "Keynote", "Pages", "GarageBand", "iMovie",
];

/// App 卸载扫描器
#[derive(Debug, Default)]
pub struct UninstallScanner;

impl UninstallScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for UninstallScanner {
    fn scan(&self) -> ScanResult {
        let start = Instant::now();

        // 收集所有 .app 路径
        let app_paths = collect_app_paths();

        // 并行扫描每个应用
        let mut items: Vec<ScanItem> = app_paths
            .par_iter()
            .filter_map(|app_path| scan_app(app_path))
            .collect();

        // 按大小降序排列
        items.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

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
//  应用收集
// =========================================================================

/// 收集 /Applications/ 和 ~/Applications/ 下的所有 .app 路径
fn collect_app_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let home = home_dir();

    let search_dirs = [
        PathBuf::from("/Applications"),
        home.join("Applications"),
    ];

    for dir in &search_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if is_app_bundle(&path) {
                    paths.push(path);
                }
            }
        }
    }

    paths
}

/// 判断路径是否为 .app 包
fn is_app_bundle(path: &PathBuf) -> bool {
    if !path.is_dir() {
        return false;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        return name.ends_with(".app");
    }
    false
}

// =========================================================================
//  单个应用扫描
// =========================================================================

/// 扫描单个应用，返回 ScanItem
///
/// 跳过系统应用（/System/ 下、com.apple.* bundle ID、已知系统应用名）。
fn scan_app(app_path: &PathBuf) -> Option<ScanItem> {
    let path_str = app_path.to_string_lossy();

    // 跳过 /System/ 下的应用
    if path_str.starts_with("/System/") {
        return None;
    }

    // 获取 bundle ID
    let bundle_id = get_bundle_id(app_path)?;

    // 跳过 Apple 系统应用
    if bundle_id.starts_with("com.apple.") {
        return None;
    }

    // 获取应用显示名称
    let app_name = get_app_display_name(app_path).unwrap_or_else(|| {
        app_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("未知应用")
            .to_string()
    });

    // 跳过 macOS 系统自带应用
    if is_system_app(&app_name) {
        return None;
    }

    // 计算 .app 包大小
    let app_size = dir_size(app_path);

    // 查找关联文件
    let mut batch_paths = find_associated_files(&bundle_id, &app_name);

    // 计算关联文件总大小
    let mut associated_size: u64 = 0;
    for p in &batch_paths {
        let p_path = std::path::Path::new(p);
        if p_path.is_dir() {
            associated_size += dir_size(p_path);
        } else if p_path.is_file() {
            if let Ok(meta) = p_path.symlink_metadata() {
                associated_size += meta.len();
            }
        }
    }

    // 将 .app 本身加入 batch_paths 末尾，确保卸载时 .app 包也被删除
    // （删除流程中 batch_paths 非空时只删 batch_paths 并 continue，不删主路径）
    batch_paths.push(app_path.to_string_lossy().to_string());

    let total_size = app_size + associated_size;
    let file_count = batch_paths.len();

    Some(ScanItem {
        path: app_path.to_string_lossy().to_string(),
        size_bytes: total_size,
        category: app_name,
        selected: false,
        deletable: true,
        undeletable_reason: String::new(),
        batch_paths,
        recommend: Recommend::Advanced,
        description: format!(
            "应用大小 {}，关联文件 {} 项",
            format_size_local(app_size),
            file_count.saturating_sub(1)
        ),
    })
}

// =========================================================================
//  Bundle 信息读取
// =========================================================================

/// 从 Info.plist 读取 bundle ID
///
/// 优先使用 `defaults read`，失败时回退到 `plutil`。
fn get_bundle_id(app_path: &PathBuf) -> Option<String> {
    let plist = app_path.join("Contents/Info.plist");
    if !plist.exists() {
        return None;
    }

    // 尝试 defaults read
    let output = Command::new("defaults")
        .arg("read")
        .arg(&plist)
        .arg("CFBundleIdentifier")
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }

    // 尝试 plutil 作为后备
    let output = Command::new("plutil")
        .arg("-extract")
        .arg("CFBundleIdentifier")
        .arg("raw")
        .arg(&plist)
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }

    None
}

/// 从 Info.plist 读取应用显示名称
///
/// 优先读取 CFBundleDisplayName，回退到 CFBundleName。
fn get_app_display_name(app_path: &PathBuf) -> Option<String> {
    let plist = app_path.join("Contents/Info.plist");
    if !plist.exists() {
        return None;
    }

    for key in &["CFBundleDisplayName", "CFBundleName"] {
        let output = Command::new("defaults")
            .arg("read")
            .arg(&plist)
            .arg(key)
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }

    None
}

// =========================================================================
//  关联文件查找
// =========================================================================

/// 查找应用的关联文件
///
/// 基于 bundle ID 和应用名搜索 ~/Library/ 下的各类关联路径。
/// 返回所有存在的关联文件/目录路径列表。
fn find_associated_files(bundle_id: &str, app_name: &str) -> Vec<String> {
    let home = home_dir();
    let mut paths = Vec::new();

    // ~/Library/Containers/<bundle_id>/
    let p = home.join(format!("Library/Containers/{}", bundle_id));
    if p.exists() {
        paths.push(p.to_string_lossy().to_string());
    }

    // ~/Library/Group Containers/*<bundle_id>*/  (通配匹配)
    let group_dir = home.join("Library/Group Containers");
    if let Ok(entries) = std::fs::read_dir(&group_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(bundle_id) {
                paths.push(entry.path().to_string_lossy().to_string());
            }
        }
    }

    // ~/Library/Caches/<bundle_id>/
    let p = home.join(format!("Library/Caches/{}", bundle_id));
    if p.exists() {
        paths.push(p.to_string_lossy().to_string());
    }

    // ~/Library/Application Support/<app_name>/
    let p = home.join(format!("Library/Application Support/{}", app_name));
    if p.exists() {
        paths.push(p.to_string_lossy().to_string());
    }

    // ~/Library/Preferences/<bundle_id>.plist
    let p = home.join(format!("Library/Preferences/{}.plist", bundle_id));
    if p.exists() {
        paths.push(p.to_string_lossy().to_string());
    }

    // ~/Library/Preferences/<bundle_id>/
    let p = home.join(format!("Library/Preferences/{}", bundle_id));
    if p.exists() {
        paths.push(p.to_string_lossy().to_string());
    }

    // ~/Library/Logs/<app_name>/
    let p = home.join(format!("Library/Logs/{}", app_name));
    if p.exists() {
        paths.push(p.to_string_lossy().to_string());
    }

    // ~/Library/Saved Application State/<bundle_id>.savedState/
    let p = home.join(format!(
        "Library/Saved Application State/{}.savedState",
        bundle_id
    ));
    if p.exists() {
        paths.push(p.to_string_lossy().to_string());
    }

    // ~/Library/HTTPStorages/<bundle_id>/
    let p = home.join(format!("Library/HTTPStorages/{}", bundle_id));
    if p.exists() {
        paths.push(p.to_string_lossy().to_string());
    }

    paths
}

// =========================================================================
//  辅助函数
// =========================================================================

/// 判断是否为 macOS 系统自带应用
fn is_system_app(app_name: &str) -> bool {
    for name in SYSTEM_APP_NAMES {
        if app_name.eq_ignore_ascii_case(name) {
            return true;
        }
    }
    false
}

/// 格式化文件大小（本地辅助函数）
fn format_size_local(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}
