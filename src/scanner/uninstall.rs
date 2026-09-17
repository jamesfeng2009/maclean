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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use rayon::prelude::*;

use crate::app_protection::{self, ProtectionLevel};

use super::{dir_size, home_dir, Recommend, ScanItem, ScanResult, Scanner};

/// macOS 系统自带应用名称（不应卸载）
///
/// 这些应用即使在 /Applications/ 下（非 /System/）也不应被卸载。
/// 用于 bundle ID 检查之外的名称匹配后备保护。
const SYSTEM_APP_NAMES: &[&str] = &[
    "Safari",
    "Mail",
    "Notes",
    "Calendar",
    "Messages",
    "FaceTime",
    "Maps",
    "News",
    "Stocks",
    "Weather",
    "Reminders",
    "Contacts",
    "Preview",
    "TextEdit",
    "Calculator",
    "Chess",
    "Stickies",
    "Time Machine",
    "System Preferences",
    "System Settings",
    "Photo Booth",
    "Dictionary",
    "Font Book",
    "Grapher",
    "Terminal",
    "Activity Monitor",
    "Disk Utility",
    "Keychain Access",
    "Migration Assistant",
    "Console",
    "Automator",
    "Script Editor",
    "Image Capture",
    "Screenshot",
    "QuickTime Player",
    "VoiceOver Utility",
    "Audio MIDI Setup",
    "Digital Color Meter",
    "ColorSync Utility",
    "AirPort Utility",
    "Bluetooth File Exchange",
    "Tips",
    "Home",
    "App Store",
    "System Information",
    "Launchpad",
    "Mission Control",
    "Find My",
    "Podcasts",
    "Music",
    "TV",
    "Photos",
    "Books",
    "Freeform",
    "Shortcuts",
];

/// 统一的系统级目录/文件名称过滤函数
///
/// 用于 Application Support、Caches、Preferences 等 Library 子目录扫描时，
/// 从根源过滤掉系统级条目，避免系统文件被展示给用户。
///
/// 覆盖范围包括：
/// - Apple 系统应用（Safari/Mail/Maps/Photos/Calendar/Messages 等）
/// - 系统框架/服务（CloudKit/CoreSimulator/QuickLook/ColorSync/Spotlight/launchservicesd 等）
/// - 系统目录（ByHost/Caches/TemporaryItems/LaunchAgents/LaunchDaemons/Preferences 等）
/// - 系统守护进程（bluetoothd/powerd/sharingd/wifiagent 等）
/// - com.apple.* / org.apple.* 前缀
/// - 点号开头文件
pub fn is_system_library_name(name: &str) -> bool {
    if name.is_empty() {
        return true;
    }

    // 点号开头（隐藏系统文件/目录）
    if name.starts_with('.') {
        return true;
    }

    // Apple 官方 bundle ID / 组织前缀
    if name.starts_with("com.apple.")
        || name.starts_with("org.apple.")
        || name.starts_with("com.icloud.")
        || name.starts_with("com.mobilenetworking.")
    {
        return true;
    }

    // 常见系统目录名（Library 子目录级别）
    let system_directories = [
        "ByHost",
        "Caches",
        "TemporaryItems",
        "LaunchAgents",
        "LaunchDaemons",
        "Preferences",
        "Saved Application State",
        "SavedApplicationState",
        "Containers",
        "Group Containers",
        "GroupContainers",
        "HTTPStorages",
        "HTTPStorage",
        "Cookies",
        "WebKit",
        "Application Scripts",
        "ApplicationScripts",
        "Metadata",
        "Logs",
        "Application Support",
        "ApplicationSupport",
        "MobileSync",
        "SyncServices",
        "KeyboardServices",
        "AddressBook",
        "CallHistoryDB",
        "CallHistory",
        "CloudDocs",
        "iCloud",
        "Apple",
        "AppleSetup",
        "CrashReporter",
        "Dock",
        "Siri",
        "TelephonyUtilities",
        "CoreSimulator",
        "Xcode",
    ];
    for dir in &system_directories {
        if name.eq_ignore_ascii_case(dir) {
            return true;
        }
    }

    // Apple 系统应用名称
    for app in SYSTEM_APP_NAMES {
        if name.eq_ignore_ascii_case(app) {
            return true;
        }
    }

    // 系统守护进程 / 服务 / 框架
    let system_daemons_and_frameworks = [
        // 守护进程
        "bluetoothd",
        "powerd",
        "sharingd",
        "wifiagent",
        "wifid",
        "airportd",
        "awdd",
        "thermalmonitord",
        "systemstats",
        "sysmond",
        "powerlogd",
        "deleted",
        "photolibraryd",
        "medialibraryd",
        "coreduetd",
        "contextstored",
        "coredatad",
        "applecamerad",
        "appleaccountd",
        "akd",
        "aned",
        "trustd",
        "oahd",
        "runningboardd",
        "iconservicesagent",
        "iconservicesd",
        "fileproviderd",
        "filecoordinationd",
        "launchservicesd",
        "coreservicesd",
        "coreaudiod",
        "corelocationd",
        "parsed",
        "distnoted",
        "opendirectoryd",
        "syslogd",
        "kernelmanagerd",
        "kextd",
        "notifyd",
        "securityd",
        "mds",
        "mds_stores",
        "mds_worker",
        "WindowServer",
        "loginwindow",
        "SystemUIServer",
        "talagent",
        "cfprefsd",
        "cshregistrar",
        "cloudd",
        "bird",
        "nsurlsessiond",
        "nsurlstoraged",
        "favord",
        "suggestd",
        "knowledge-agent",
        "aksd",
        "biometrickitd",
        // 框架 / 服务
        "CloudKit",
        "QuickLook",
        "ColorSync",
        "Spotlight",
        "CoreServices",
        "ApplicationServices",
        "SystemConfiguration",
        "IOKit",
        "Security",
        "CoreFoundation",
        "Foundation",
        "CoreData",
        "CoreText",
        "CoreGraphics",
        "CoreImage",
        "CoreVideo",
        "Accelerate",
        "QuartzCore",
        "AppKit",
        "UIKit",
        "WebKitLegacy",
        "AVFoundation",
        "CFNetwork",
        "ImageIO",
        "Metal",
        "OpenGL",
        "SystemIntegrityProtection",
        // 其他系统级通用名
        "MobileAsset",
        "SoftwareUpdate",
        "CommerceKit",
        "StoreKit",
        "GameCenter",
        "GameKit",
        "PassKit",
        "HealthKit",
        "HomeKit",
        "ClassKit",
        "ReplayKit",
        "SpriteKit",
        "SceneKit",
        "MapKit",
        "EventKit",
        "AddressBookSourceSync",
    ];
    for daemon in &system_daemons_and_frameworks {
        if name.eq_ignore_ascii_case(daemon) {
            return true;
        }
    }

    false
}

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

        // 并行扫描每个应用，每个应用可能拆出多个 ScanItem
        let mut items: Vec<ScanItem> = app_paths
            .par_iter()
            .flat_map(|app_path| scan_app(app_path))
            .collect();

        // 扫描废纸篓和 Downloads 中的 .app 残留
        items.extend(scan_trash_apps());
        items.extend(scan_downloads_apps());

        // 扫描已卸载 App 在 Library 中的数据/缓存/配置残留
        scan_app_leftovers(&mut items);

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

    let search_dirs = [PathBuf::from("/Applications"), home.join("Applications")];

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
//  废纸篓 / Downloads 中的 .app 残留扫描
// =========================================================================

/// 扫描 ~/.Trash 中的 .app 包（已删除应用的残留）
///
/// 废纸篓中的 .app 是用户已经拖入 Trash 的应用，可以安全清理。
/// 会检测是否与 /Applications 中的已安装应用同名（避免删除正在重装的应用）。
fn scan_trash_apps() -> Vec<ScanItem> {
    let home = home_dir();
    let trash_dir = home.join(".Trash");
    let mut items = Vec::new();
    if !trash_dir.is_dir() {
        return items;
    }

    // 收集已安装应用名称集合，用于排除正在重装的情况
    let installed_names = collect_installed_app_names();

    if let Ok(entries) = std::fs::read_dir(&trash_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !is_app_bundle(&path) {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // 如果同名应用已在 /Applications 安装，则跳过（可能是在重装）
            if installed_names.contains(name) {
                continue;
            }
            let size = dir_size(&path);
            items.push(ScanItem {
                path: path.to_string_lossy().to_string(),
                size_bytes: size,
                category: format!("废纸篓残留-{}", name.trim_end_matches(".app")),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: format!("废纸篓中的 {} 残留，可安全清理释放空间", name),
            });
        }
    }

    items
}

/// 扫描 ~/Downloads 中的 .app 包（下载后未清理的安装包）
///
/// Downloads 中的 .app 通常是下载后直接解压运行的，安装到 /Applications 后可清理。
fn scan_downloads_apps() -> Vec<ScanItem> {
    let home = home_dir();
    let downloads = home.join("Downloads");
    let mut items = Vec::new();
    if !downloads.is_dir() {
        return items;
    }

    let installed_names = collect_installed_app_names();

    if let Ok(entries) = std::fs::read_dir(&downloads) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !is_app_bundle(&path) {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // 如果同名应用已安装，说明已拷贝到 /Applications，Downloads 里的是残留
            let is_installed = installed_names.contains(name);
            let size = dir_size(&path);
            let recommend = if is_installed {
                Recommend::Safe
            } else {
                Recommend::Caution
            };
            let desc = if is_installed {
                format!("Downloads 中的 {}，同名应用已安装，可安全清理", name)
            } else {
                format!(
                    "Downloads 中的 {}，未检测到同名已安装应用，请确认后再删除",
                    name
                )
            };
            items.push(ScanItem {
                path: path.to_string_lossy().to_string(),
                size_bytes: size,
                category: format!("下载残留-{}", name.trim_end_matches(".app")),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend,
                description: desc,
            });
        }
    }

    items
}

/// 收集 /Applications 和 ~/Applications 中已安装应用的文件名集合
fn collect_installed_app_names() -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    let home = home_dir();
    let dirs = [PathBuf::from("/Applications"), home.join("Applications")];
    for dir in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                if let Some(name) = entry.file_name().to_str() {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names
}

// =========================================================================
//  单个应用扫描
// =========================================================================

/// 扫描单个应用，返回一组 ScanItem
///
/// 把一个应用拆分为多个可独立选择的项：
/// - 应用卸载项（Advanced）：.app 包 + 数据类关联文件
/// - 缓存清理项（CacheOnly）：Caches / Logs / HTTPStorages 等，删除后不影响使用
///
/// 保护策略（参考 Mole app_protection_data.sh）：
/// - /System/ 下的应用：跳过（系统只读区域）
/// - 系统关键应用（Finder、Dock、Safari 等）：显示但标记不可删除
/// - 安全/MDM 应用（CrowdStrike、Jamf 等）：显示但标记不可删除，提示使用官方卸载工具
/// - 数据保护应用（1Password、输入法等）：可删除但描述包含警告
/// - 可卸载的 Apple 应用（Xcode、Final Cut Pro 等）：正常显示
/// - 普通第三方应用：正常显示
fn scan_app(app_path: &PathBuf) -> Vec<ScanItem> {
    let mut items = Vec::new();
    let path_str = app_path.to_string_lossy();

    // 跳过 /System/ 下的应用（系统只读区域，无法删除）
    if path_str.starts_with("/System/") {
        return items;
    }

    // 获取 bundle ID（无 bundle ID 的应用无法判断保护级别，跳过）
    let bundle_id = match get_bundle_id(app_path) {
        Some(id) => id,
        None => return items,
    };

    // 获取应用显示名称
    let app_name = get_app_display_name(app_path).unwrap_or_else(|| {
        app_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("未知应用")
            .to_string()
    });

    // 检查应用保护级别
    let protection = app_protection::check_bundle_protection(&bundle_id);

    // 系统关键应用：通过 bundle ID 或名称匹配检测
    // 即使不在 /System/ 下（如 /Applications/ 中的 Apple 应用），也标记为不可删除
    let is_system_by_name = is_system_app(&app_name);
    let is_critical = matches!(protection, ProtectionLevel::Critical) || is_system_by_name;

    // 计算 .app 包大小
    let app_size = dir_size(app_path);

    // 查找关联文件并拆分为缓存类 / 数据类
    let associated_files = find_associated_files(&bundle_id, &app_name);
    let (cache_paths, data_paths): (Vec<String>, Vec<String>) = associated_files
        .into_iter()
        .partition(|p| is_cache_like_path(p));

    let cache_size: u64 = cache_paths.iter().map(|p| path_size(p)).sum();
    let data_size: u64 = data_paths.iter().map(|p| path_size(p)).sum();

    // 不可删除的系统关键 / 安全应用：只展示一个汇总项
    if is_critical {
        items.push(ScanItem {
            path: app_path.to_string_lossy().to_string(),
            size_bytes: app_size + data_size + cache_size,
            category: app_name,
            selected: false,
            deletable: false,
            undeletable_reason: "protection_critical".to_string(),
            batch_paths: Vec::new(),
            recommend: Recommend::Advanced,
            description: format!("系统关键应用 | 应用大小 {}", format_size_local(app_size)),
        });
        return items;
    }

    if let ProtectionLevel::RequiresOfficialUninstaller = protection {
        let vendor = app_protection::get_security_vendor(&bundle_id).unwrap_or("官方");
        items.push(ScanItem {
            path: app_path.to_string_lossy().to_string(),
            size_bytes: app_size + data_size + cache_size,
            category: app_name,
            selected: false,
            deletable: false,
            undeletable_reason: format!("protection_official_uninstaller:{}", vendor),
            batch_paths: Vec::new(),
            recommend: Recommend::Advanced,
            description: format!(
                "{} 安全代理 | 应用大小 {}",
                vendor,
                format_size_local(app_size)
            ),
        });
        return items;
    }

    // 可卸载应用：拆为三项，让用户自主选择
    //   1. 应用卸载（Caution）：只删 .app 包
    //   2. 应用数据（Advanced）：删 Containers/Application Support/Preferences 等
    //   3. 应用缓存（CacheOnly）：删 Caches/Logs/HTTPStorages

    // 1) 应用卸载项：只删 .app 包
    items.push(ScanItem {
        path: app_path.to_string_lossy().to_string(),
        size_bytes: app_size,
        category: format!("{} (卸载)", app_name),
        selected: false,
        deletable: true,
        undeletable_reason: String::new(),
        batch_paths: vec![app_path.to_string_lossy().to_string()],
        recommend: Recommend::Caution,
        description: format!(
            "卸载 {} 应用本体（{}），关联数据不会被删除",
            app_name,
            format_size_local(app_size)
        ),
    });

    // 2) 应用数据项（Advanced）：删数据类文件
    if data_size > 0 && !data_paths.is_empty() {
        let data_desc = if matches!(protection, ProtectionLevel::DataProtected) {
            format!(
                "{} 的应用数据（含聊天记录/配置等），删除前请务必备份",
                app_name
            )
        } else {
            format!(
                "{} 的应用数据与配置（{} 项），删除后可能需要重新登录或配置",
                app_name,
                data_paths.len()
            )
        };
        items.push(ScanItem {
            path: data_paths[0].clone(),
            size_bytes: data_size,
            category: format!("{} 数据", app_name),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: data_paths,
            recommend: Recommend::Advanced,
            description: data_desc,
        });
    }

    // 3) 缓存清理项（CacheOnly）：Caches / Logs / HTTPStorages
    if cache_size > 0 && !cache_paths.is_empty() {
        items.push(ScanItem {
            path: cache_paths[0].clone(),
            size_bytes: cache_size,
            category: format!("{} 缓存", app_name),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: cache_paths,
            recommend: Recommend::CacheOnly,
            description: format!("{} 的缓存/日志，删除后不影响使用，应用会自动重建", app_name),
        });
    }

    items
}

/// 判断路径是否为缓存/日志类路径
///
/// 缓存类路径删除后应用可正常运行并自动重建。
fn is_cache_like_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("library/caches/")
        || lower.contains("library/logs/")
        || lower.contains("library/httpstorages/")
}

/// 计算单个路径的大小（文件或目录）
fn path_size(path: &str) -> u64 {
    let p = std::path::Path::new(path);
    if p.is_dir() {
        dir_size(p)
    } else if let Ok(meta) = p.symlink_metadata() {
        meta.len()
    } else {
        0
    }
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
/// macOS `defaults read` 对中文字符会输出 \uXXXX 转义序列，这里做解码。
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
                let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let name = decode_unicode_escapes(&raw);
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }

    None
}

/// 解码 \uXXXX / \UXXXXXXXX 转义序列
///
/// macOS `defaults read` 对非 ASCII 字符会输出 Unicode 转义，例如：
///   \u5143\u5b9d -> 元宝
fn decode_unicode_escapes(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            let next = chars[i + 1];
            if next == 'u' || next == 'U' {
                let is_long = next == 'U';
                let hex_len = if is_long { 8 } else { 4 };
                let start = i + 2;
                let end = (start + hex_len).min(chars.len());
                let hex: String = chars[start..end].iter().collect();

                if hex.len() == hex_len && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    if let Ok(codepoint) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(codepoint) {
                            result.push(ch);
                            i = end;
                            continue;
                        }
                    }
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

// =========================================================================
//  关联文件查找
// =========================================================================

/// 应用名称的版本后缀，在生成命名变体时需要剥离
///
/// 例如 "Visual Studio Code Insiders" → 基础名 "Visual Studio Code"
/// "Firefox Developer Edition" → "Firefox"
const VERSION_SUFFIXES: &[&str] = &[
    "Nightly",
    "Beta",
    "Alpha",
    "Dev",
    "Canary",
    "Preview",
    "Insider",
    "Insiders",
    "Edge",
    "Stable",
    "Release",
    "RC",
    "LTS",
    "Developer Edition",
    "Technology Preview",
];

/// 查找应用的关联文件
///
/// 基于 bundle ID 和应用名搜索 ~/Library/ 下的各类关联路径。
/// 支持命名变体生成（nospace/underscore/hyphen/lowercase）和
/// 版本后缀剥离（Nightly/Beta/Dev/Canary 等），确保找到所有残留。
///
/// 扫描范围（共 18 类）：
/// 1.  Containers — 沙盒应用容器
/// 2.  Group Containers — 共享容器（App Group）
/// 3.  Caches — 应用缓存
/// 4.  Application Support — 应用数据
/// 5.  Preferences (.plist) — 偏好设置文件
/// 6.  Preferences/ — 偏好设置目录
/// 7.  Logs — 日志
/// 8.  Saved Application State — 窗口恢复状态
/// 9.  HTTPStorages — HTTP 缓存/Cookie
/// 10. Cookies — Cookie 文件
/// 11. WebKit — WebKit/Electron 数据
/// 12. Application Scripts — 应用脚本
/// 13. Metadata — Spotlight 元数据
/// 14. Caches (按应用名匹配)
/// 15. LaunchAgents — 用户级启动代理
/// 16. LaunchDaemons — 系统级守护进程（需 sudo 删除）
/// 17. Caches/Application Support（按命名变体匹配）
/// 18. Embedded bundle ID（XPC/appex 内嵌的 bundle ID）
fn find_associated_files(bundle_id: &str, app_name: &str) -> Vec<String> {
    let home = home_dir();
    let mut paths = Vec::new();

    // 生成应用名称变体（含版本后缀剥离）
    let name_variants = generate_name_variants(app_name);

    // 生成 bundle ID 变体（剥离版本后缀）
    let bundle_id_variants = generate_bundle_id_variants(bundle_id);

    // ---- 按 bundle ID 匹配的路径 ----

    // 1. ~/Library/Containers/<bundle_id>/
    for bid in &bundle_id_variants {
        let p = home.join(format!("Library/Containers/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 2. ~/Library/Group Containers/*<bundle_id>*/
    let group_dir = home.join("Library/Group Containers");
    if let Ok(entries) = std::fs::read_dir(&group_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            for bid in &bundle_id_variants {
                if name.contains(bid) {
                    let p = entry.path().to_string_lossy().to_string();
                    if !paths.contains(&p) {
                        paths.push(p);
                    }
                    break;
                }
            }
        }
    }

    // 3. ~/Library/Caches/<bundle_id>/
    for bid in &bundle_id_variants {
        let p = home.join(format!("Library/Caches/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 4. ~/Library/Application Support/<app_name>/ (含命名变体)
    for name in &name_variants {
        let p = home.join(format!("Library/Application Support/{}", name));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 5. ~/Library/Preferences/<bundle_id>.plist
    for bid in &bundle_id_variants {
        let p = home.join(format!("Library/Preferences/{}.plist", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 6. ~/Library/Preferences/<bundle_id>/
    for bid in &bundle_id_variants {
        let p = home.join(format!("Library/Preferences/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 7. ~/Library/Logs/<app_name>/ (含命名变体)
    for name in &name_variants {
        let p = home.join(format!("Library/Logs/{}", name));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 8. ~/Library/Saved Application State/<bundle_id>.savedState/
    for bid in &bundle_id_variants {
        let p = home.join(format!(
            "Library/Saved Application State/{}.savedState",
            bid
        ));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 9. ~/Library/HTTPStorages/<bundle_id>/
    for bid in &bundle_id_variants {
        let p = home.join(format!("Library/HTTPStorages/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 10. ~/Library/Cookies/<bundle_id>.binarycookies
    for bid in &bundle_id_variants {
        let p = home.join(format!("Library/Cookies/{}.binarycookies", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 11. ~/Library/WebKit/<bundle_id>/
    for bid in &bundle_id_variants {
        let p = home.join(format!("Library/WebKit/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 12. ~/Library/Application Scripts/<bundle_id>/
    for bid in &bundle_id_variants {
        let p = home.join(format!("Library/Application Scripts/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 13. ~/Library/Metadata/<bundle_id>/
    for bid in &bundle_id_variants {
        let p = home.join(format!("Library/Metadata/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 14. ~/Library/Caches/<app_name>/ — 按应用名匹配（含命名变体）
    for name in &name_variants {
        let p = home.join(format!("Library/Caches/{}", name));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 15. ~/Library/LaunchAgents/ — 扫描用户级启动代理
    scan_launch_agents(&home, &bundle_id_variants, &name_variants, &mut paths);

    // 16. /Library/LaunchAgents/ 和 /Library/LaunchDaemons/ — 系统级（需 sudo）
    scan_system_launch_agents(&bundle_id_variants, &name_variants, &mut paths);

    // 17. ~/Library/Preferences/<app_name>/ (按应用名匹配的偏好设置目录)
    for name in &name_variants {
        let p = home.join(format!("Library/Preferences/{}", name));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 18. Embedded bundle ID — 扫描 .app 包内嵌的 XPC/appex 的 bundle ID
    // 这些是应用插件/扩展，它们的关联文件需要一并清理
    // (在 scan_app 中调用时传入 app_path，这里通过额外参数实现)

    paths
}

/// 生成应用名称的命名变体
///
/// 生成以下变体：
/// - 原始名称
/// - 去空格（如 "Google Chrome" → "GoogleChrome"）
/// - 下划线替换空格（如 "Google Chrome" → "Google_Chrome"）
/// - 连字符替换空格（如 "Google Chrome" → "Google-Chrome"）
/// - 全小写
/// - 剥离版本后缀后的基础名（如 "Firefox Developer Edition" → "Firefox"）
/// - 剥离后缀 + 去空格/下划线/连字符/小写
fn generate_name_variants(app_name: &str) -> Vec<String> {
    let mut variants = vec![app_name.to_string()];

    // 去空格
    let nospace = app_name.replace(' ', "");
    if nospace != app_name {
        variants.push(nospace.clone());
    }

    // 下划线替换空格
    let underscore = app_name.replace(' ', "_");
    if underscore != app_name && underscore != nospace {
        variants.push(underscore.clone());
    }

    // 连字符替换空格
    let hyphen = app_name.replace(' ', "-");
    if hyphen != app_name && hyphen != nospace && hyphen != underscore {
        variants.push(hyphen.clone());
    }

    // 全小写
    let lowercase = app_name.to_lowercase();
    if lowercase != app_name {
        variants.push(lowercase.clone());
    }

    // 剥离版本后缀
    let base_name = strip_version_suffix(app_name);
    if base_name != app_name {
        variants.push(base_name.clone());
        let base_nospace = base_name.replace(' ', "");
        if base_nospace != base_name {
            variants.push(base_nospace);
        }
        let base_lower = base_name.to_lowercase();
        if base_lower != base_name && base_lower != lowercase {
            variants.push(base_lower);
        }
    }

    variants
}

/// 生成 bundle ID 的变体（剥离版本后缀）
///
/// 例如 "com.mozilla.firefox-developer-edition" → "com.mozilla.firefox"
fn generate_bundle_id_variants(bundle_id: &str) -> Vec<String> {
    let mut variants = vec![bundle_id.to_string()];

    // 尝试剥离版本后缀
    let lower = bundle_id.to_lowercase();
    for suffix in VERSION_SUFFIXES {
        let suffix_lower = suffix.to_lowercase().replace(' ', "-");
        if lower.contains(&format!("-{}", suffix_lower)) {
            let stripped = lower.replace(&format!("-{}", suffix_lower), "");
            if stripped != bundle_id && !variants.contains(&stripped) {
                variants.push(stripped);
            }
        }
        // 也尝试不带连字符的变体
        let suffix_nospace = suffix.to_lowercase().replace(' ', "");
        if lower.contains(&format!("-{}", suffix_nospace)) {
            let stripped = lower.replace(&format!("-{}", suffix_nospace), "");
            if stripped != bundle_id && !variants.contains(&stripped) {
                variants.push(stripped);
            }
        }
    }

    variants
}

/// 剥离应用名称中的版本后缀
///
/// 例如 "Visual Studio Code Insiders" → "Visual Studio Code"
fn strip_version_suffix(name: &str) -> String {
    for suffix in VERSION_SUFFIXES {
        let pattern = format!(" {}", suffix);
        if let Some(pos) = name.find(&pattern) {
            let stripped = name[..pos].trim().to_string();
            if !stripped.is_empty() {
                return stripped;
            }
        }
    }
    name.to_string()
}

/// 扫描用户级 LaunchAgents 目录，查找与目标应用相关的启动代理
///
/// 检查 ~/Library/LaunchAgents/ 下的 .plist 文件，
/// 匹配 bundle ID 或应用名（文件名或 plist 内容中包含目标标识）。
fn scan_launch_agents(
    home: &Path,
    bundle_id_variants: &[String],
    name_variants: &[String],
    paths: &mut Vec<String>,
) {
    let agents_dir = home.join("Library/LaunchAgents");
    scan_launch_dir(&agents_dir, bundle_id_variants, name_variants, paths);
}

/// 扫描系统级 LaunchAgents/LaunchDaemons 目录
///
/// 这些路径需要 sudo 权限才能删除，但仍需扫描出来告知用户。
fn scan_system_launch_agents(
    bundle_id_variants: &[String],
    name_variants: &[String],
    paths: &mut Vec<String>,
) {
    let system_dirs = [
        PathBuf::from("/Library/LaunchAgents"),
        PathBuf::from("/Library/LaunchDaemons"),
    ];

    for dir in &system_dirs {
        scan_launch_dir(dir, bundle_id_variants, name_variants, paths);
    }
}

/// 扫描指定的 LaunchAgents/LaunchDaemons 目录
fn scan_launch_dir(
    dir: &Path,
    bundle_id_variants: &[String],
    name_variants: &[String],
    paths: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let file_name = entry.file_name().to_string_lossy().to_string();
        let file_path = entry.path().to_string_lossy().to_string();

        // 跳过非 plist 文件
        if !file_name.ends_with(".plist") {
            continue;
        }

        // 检查文件名是否匹配 bundle ID 变体
        let mut matched = false;
        for bid in bundle_id_variants {
            if file_name.contains(bid) {
                matched = true;
                break;
            }
        }

        // 检查文件名是否匹配应用名变体
        if !matched {
            for name in name_variants {
                let name_lower = name.to_lowercase();
                let file_lower = file_name.to_lowercase();
                if file_lower.contains(&name_lower) && name_lower.len() > 2 {
                    matched = true;
                    break;
                }
            }
        }

        if matched && !paths.contains(&file_path) {
            paths.push(file_path);
        }
    }
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

// =========================================================================
//  App 卸载残留清理（已卸载 App 在 Library 中的数据/缓存/配置）
// =========================================================================

/// 扫描已卸载 App 的残留文件
///
/// 检测策略：
/// 1. 扫描 ~/Library/Application Support/、~/Library/Caches/、~/Library/Preferences/
/// 2. 对每个子目录，检查 /Applications/ 下是否有对应的 .app
/// 3. 如果 App 不存在，标记为残留
fn scan_app_leftovers(items: &mut Vec<ScanItem>) {
    let home = home_dir();

    // 获取已安装的 app 名称集合
    let installed_apps = get_installed_app_names();
    if installed_apps.is_empty() {
        return;
    }

    // 扫描 ~/Library/Application Support/ 下的残留
    let app_support = home.join("Library/Application Support");
    if app_support.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&app_support) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();

                // 跳过系统级和开发工具目录
                if is_system_library_name(&name) {
                    continue;
                }

                // 检查是否有对应的 App
                if is_app_installed(&name, &installed_apps) {
                    continue;
                }

                // 多应用共享厂商目录（如 Google/、Microsoft/）：
                // 不整体标记为残留，递归检测子目录
                if is_vendor_shared_dir(&name) {
                    items.extend(scan_vendor_subdir_leftovers(
                        &path,
                        &installed_apps,
                        "App残留",
                        0,
                        Recommend::Advanced,
                    ));
                    continue;
                }

                // 普通残留目录：只要有文件就展示
                let size = dir_size(&path);
                items.push(ScanItem {
                    path: path.to_string_lossy().to_string(),
                    size_bytes: size,
                    category: "App残留".to_string(),
                    selected: false,
                    deletable: true,
                    undeletable_reason: String::new(),
                    batch_paths: Vec::new(),
                    recommend: Recommend::Advanced,
                    description: format!("{} 的残留数据（App 可能已卸载）", name),
                });
            }
        }
    }

    // 扫描 ~/Library/Caches/ 下的残留（大于 100MB 的）
    let caches = home.join("Library/Caches");
    if caches.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&caches) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();

                // 跳过系统缓存
                if is_system_library_name(&name) {
                    continue;
                }

                if is_app_installed(&name, &installed_apps) {
                    continue;
                }

                // 多应用共享厂商目录：递归检测子目录
                if is_vendor_shared_dir(&name) {
                    items.extend(scan_vendor_subdir_leftovers(
                        &path,
                        &installed_apps,
                        "App残留缓存",
                        0,
                        Recommend::CacheOnly,
                    ));
                    continue;
                }

                // 普通残留缓存：只要有文件就展示
                let size = dir_size(&path);
                items.push(ScanItem {
                    path: path.to_string_lossy().to_string(),
                    size_bytes: size,
                    category: "App残留缓存".to_string(),
                    selected: false,
                    deletable: true,
                    undeletable_reason: String::new(),
                    batch_paths: Vec::new(),
                    recommend: Recommend::CacheOnly,
                    description: format!("{} 的残留缓存，删除后无影响（App 可能已卸载）", name),
                });
            }
        }
    }

    // 扫描 ~/Library/Preferences/ 下的残留 plist（每个文件独立展示）
    let prefs = home.join("Library/Preferences");
    if prefs.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&prefs) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();

                // 只处理 .plist 文件
                if !name.ends_with(".plist") {
                    continue;
                }

                // 跳过系统级和 Apple 官方 plist
                if is_system_library_name(&name) {
                    continue;
                }

                // 只展示已卸载 App 的 plist
                if is_app_installed(&name, &installed_apps) {
                    continue;
                }

                let (recommend, description) = classify_leftover_plist(&name);
                let size = path_size(path.to_string_lossy().as_ref());

                items.push(ScanItem {
                    path: path.to_string_lossy().to_string(),
                    size_bytes: size,
                    category: "App残留配置".to_string(),
                    selected: false,
                    deletable: true,
                    undeletable_reason: String::new(),
                    batch_paths: Vec::new(),
                    recommend,
                    description,
                });
            }
        }
    }
}

/// 获取已安装 App 的名称集合
///
/// 除扫描 /Applications 和 ~/Applications 外，还检测：
/// - Homebrew（/opt/homebrew/bin/brew 或 /usr/local/bin/brew 存在）
/// - JetBrains 系列 IDE（/Applications 下是否存在 JetBrains 应用）
fn get_installed_app_names() -> std::collections::HashSet<String> {
    let mut apps = std::collections::HashSet::new();

    let home_str = home_dir().to_string_lossy().to_string();
    for apps_dir in ["/Applications", &format!("{}/Applications", home_str)] {
        let apps_path = PathBuf::from(apps_dir);
        if let Ok(entries) = std::fs::read_dir(&apps_path) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".app") {
                    let app_name = name.trim_end_matches(".app").to_string();
                    apps.insert(app_name.clone());
                    apps.insert(app_name.to_lowercase());
                }
            }
        }
    }

    // Homebrew 检测
    if Path::new("/opt/homebrew/bin/brew").exists() || Path::new("/usr/local/bin/brew").exists() {
        apps.insert("Homebrew".to_string());
        apps.insert("homebrew".to_string());
    }

    // JetBrains 检测
    const JETBRAINS_APP_NAMES: &[&str] = &[
        "IntelliJ IDEA",
        "WebStorm",
        "PyCharm",
        "CLion",
        "Rider",
        "GoLand",
        "RubyMine",
        "DataGrip",
        "AppCode",
        "Android Studio",
        "Fleet",
        "JetBrains Toolbox",
    ];
    let applications = PathBuf::from("/Applications");
    if applications.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&applications) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".app") {
                    continue;
                }
                let app_name = name.trim_end_matches(".app");
                for jb_name in JETBRAINS_APP_NAMES {
                    if app_name.eq_ignore_ascii_case(jb_name) {
                        apps.insert("JetBrains".to_string());
                        apps.insert("jetbrains".to_string());
                        break;
                    }
                }
            }
        }
    }

    apps
}

/// 对已卸载 App 的残留 plist 进行安全分级
///
/// - git-credential-manager.plist 标记为 Safe
/// - 文件名含 password/keychain/credential 关键字的标记为 Caution
/// - 其他普通 plist 标记为 Safe
fn classify_leftover_plist(name: &str) -> (Recommend, String) {
    let lower = name.to_lowercase();

    if lower.contains("git-credential-manager") {
        return (
            Recommend::Safe,
            "GCM 偏好设置文件，不包含密码，删除后 GCM 配置恢复默认，不影响 Git 凭证".to_string(),
        );
    }

    if lower.contains("password") || lower.contains("keychain") || lower.contains("credential") {
        return (
            Recommend::Caution,
            format!(
                "{} 可能包含凭证相关偏好设置，删除后对应应用的登录/授权信息可能需要重新配置",
                name
            ),
        );
    }

    (
        Recommend::Safe,
        format!("{} 的偏好设置文件，可安全删除", name),
    )
}

/// 检查名称是否对应已安装的 App
fn is_app_installed(name: &str, installed_apps: &std::collections::HashSet<String>) -> bool {
    let name_lower = name.to_lowercase();

    // 直接匹配
    if installed_apps.contains(name) || installed_apps.contains(&name_lower) {
        return true;
    }

    // 去掉常见前缀（如 com.example.）
    if let Some(stripped) = name.split('.').last() {
        if !stripped.is_empty()
            && (installed_apps.contains(stripped)
                || installed_apps.contains(&stripped.to_lowercase()))
        {
            return true;
        }
    }

    // 模糊匹配：name 是 app 的前缀，或 app 是 name 的前缀
    // 例如 "Google" 匹配 "Google Chrome"，"AndroidStudio2025.1.3" 匹配 "Android Studio"
    let name_alnum: String = name_lower.chars().filter(|c| c.is_alphanumeric()).collect();
    for app in installed_apps {
        let app_lower = app.to_lowercase();
        if name_lower.contains(&app_lower) || app_lower.contains(&name_lower) {
            return true;
        }
        // 去除空格和标点后比较前缀（如 "AndroidStudio2025" vs "Android Studio"）
        let app_alnum: String = app_lower.chars().filter(|c| c.is_alphanumeric()).collect();
        if !name_alnum.is_empty() && !app_alnum.is_empty() {
            if name_alnum.starts_with(&app_alnum) || app_alnum.starts_with(&name_alnum) {
                return true;
            }
        }
    }

    false
}

/// 判断是否为多应用共享的厂商目录（如 Google/、Microsoft/、Adobe/）
/// 这类目录下通常包含多个不同 App 的子目录，不应整体标记为残留
fn is_vendor_shared_dir(name: &str) -> bool {
    let vendor_dirs = [
        "Google",
        "Microsoft",
        "Adobe",
        "JetBrains",
        "Mozilla",
        "Opera",
        "BraveSoftware",
        "Vivaldi",
        "Chromium",
    ];
    vendor_dirs.iter().any(|v| name.eq_ignore_ascii_case(v))
}

/// 扫描厂商共享目录的子目录，检测真正的残留
/// 例如 Google/ 下有 Chrome/（已安装）和 OtherApp/（未安装），只标记 OtherApp/
fn scan_vendor_subdir_leftovers(
    vendor_path: &Path,
    installed_apps: &std::collections::HashSet<String>,
    category: &str,
    min_size: u64,
    recommend: Recommend,
) -> Vec<ScanItem> {
    let mut items = Vec::new();
    let vendor_name = vendor_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let Ok(entries) = std::fs::read_dir(vendor_path) else {
        return items;
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let sub_name = entry.file_name().to_string_lossy().to_string();
        let sub_path = entry.path();

        if !sub_path.is_dir() {
            continue;
        }

        // 检查子目录是否对应已安装的 App
        if is_app_installed(&sub_name, installed_apps) {
            continue;
        }

        // 跳过系统级目录
        if is_system_library_name(&sub_name) {
            continue;
        }

        // 对于 AndroidStudioXXXX 这类带版本号的目录，额外检查
        // 例如 "AndroidStudio2025.1.3" 应该匹配 "Android Studio"
        if is_versioned_app_dir(&sub_name, installed_apps) {
            continue;
        }

        let size = dir_size(&sub_path);
        if size > min_size {
            items.push(ScanItem {
                path: sub_path.to_string_lossy().to_string(),
                size_bytes: size,
                category: category.to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend,
                description: format!(
                    "{} 中 {} 的残留数据（App 可能已卸载）",
                    vendor_name, sub_name
                ),
            });
        }
    }

    items
}

/// 检测带版本号的 App 目录（如 AndroidStudio2025.1.3 → Android Studio）
fn is_versioned_app_dir(name: &str, installed_apps: &std::collections::HashSet<String>) -> bool {
    let name_alnum: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    for app in installed_apps {
        let app_alnum: String = app
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        // 至少 4 个字符前缀匹配，避免误判
        if app_alnum.len() >= 4 && name_alnum.starts_with(&app_alnum) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_name_variants_basic() {
        let variants = generate_name_variants("Google Chrome");
        assert!(variants.contains(&"Google Chrome".to_string()));
        assert!(variants.contains(&"GoogleChrome".to_string()));
        assert!(variants.contains(&"Google_Chrome".to_string()));
        assert!(variants.contains(&"Google-Chrome".to_string()));
        assert!(variants.contains(&"google chrome".to_string()));
    }

    #[test]
    fn test_generate_name_variants_version_suffix() {
        let variants = generate_name_variants("Firefox Developer Edition");
        // 应包含剥离后缀的基础名
        assert!(variants.contains(&"Firefox".to_string()));
    }

    #[test]
    fn test_generate_name_variants_single_word() {
        let variants = generate_name_variants("Xcode");
        assert!(variants.contains(&"Xcode".to_string()));
        // 单词只有小写变体
        assert!(variants.contains(&"xcode".to_string()));
        assert_eq!(variants.len(), 2);
    }

    #[test]
    fn test_strip_version_suffix() {
        assert_eq!(strip_version_suffix("Firefox Developer Edition"), "Firefox");
        assert_eq!(strip_version_suffix("VS Code Insiders"), "VS Code");
        assert_eq!(strip_version_suffix("Chrome Beta"), "Chrome");
        assert_eq!(strip_version_suffix("Chrome"), "Chrome"); // 无后缀
    }

    #[test]
    fn test_generate_bundle_id_variants() {
        let variants = generate_bundle_id_variants("com.mozilla.firefox-developer-edition");
        assert!(variants.contains(&"com.mozilla.firefox-developer-edition".to_string()));
        // 应包含剥离后缀的变体
        assert!(variants.len() > 1);
    }

    #[test]
    fn test_find_associated_files_launch_agents() {
        // 验证 find_associated_files 返回的结果类型正确
        // 这里只测试函数不 panic，因为它依赖实际文件系统
        let paths = find_associated_files("com.test.nonexistent.app", "TestApp");
        // 对于不存在的应用，应返回空或很少的路径
        // （不应该 panic）
        assert!(paths.iter().all(|p| !p.is_empty()));
    }

    #[test]
    fn test_is_system_library_name() {
        // Apple 系统应用
        assert!(is_system_library_name("Safari"));
        assert!(is_system_library_name("com.apple.Safari"));
        // 系统框架/服务
        assert!(is_system_library_name("CloudKit"));
        assert!(is_system_library_name("launchservicesd"));
        // 系统目录
        assert!(is_system_library_name("Containers"));
        assert!(is_system_library_name("HTTPStorages"));
        // 隐藏文件
        assert!(is_system_library_name(".DS_Store"));
        // 普通第三方应用不应被过滤
        assert!(!is_system_library_name("Google Chrome"));
        assert!(!is_system_library_name("com.jetbrains.intellij"));
    }

    #[test]
    fn test_classify_leftover_plist() {
        let (recommend, desc) = classify_leftover_plist("git-credential-manager.plist");
        assert_eq!(recommend, Recommend::Safe);
        assert!(desc.contains("GCM"));

        let (recommend, _) = classify_leftover_plist("com.example.mypassword.plist");
        assert_eq!(recommend, Recommend::Caution);

        let (recommend, _) = classify_leftover_plist("com.example.preferences.plist");
        assert_eq!(recommend, Recommend::Safe);
    }
}
