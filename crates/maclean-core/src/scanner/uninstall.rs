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

// 模块刻意跨平台编译（cli/app 的引用点不做 cfg），但 Windows 的卸载走
// windows_apps 链路，本模块整体不被调用 —— 死代码告警是设计使然，不是回归。
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use rayon::prelude::*;

use crate::app_protection::{self, ProtectionLevel};

use super::{
    dir_size, dir_size_impl, home_dir, read_dir_with_timeout, Recommend, ScanItem, ScanResult,
    Scanner,
};

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

/// 系统服务缓存排除表
///
/// 这些目录归属于系统守护进程/框架（如 FamilyCircle 对应 familycircled
/// 守护进程，服务于"家人共享"），位于用户 Library 下但受 macOS 系统级
/// 访问控制保护 —— 任何权限（含管理员/Touch ID）都无法读取或删除，且
/// 属于系统服务在用数据，不是任何已卸载 App 的残留。
///
/// 扫描时对命中项直接 `continue`：不进 ScanItem、不进计数、不进统计。
/// 展示给用户只会造成"扫到了但永远删不掉"的困惑（曾导致用户反复
/// 勾选 → Touch ID 授权 → 删除失败 → 重试的挫败循环）。
pub fn is_system_service_cache(name: &str) -> bool {
    const SYSTEM_SERVICE_CACHES: &[&str] = &[
        // FamilyCircle.framework / familycircled 守护进程（家人共享/Family Sharing）
        "familycircle",
        "familycircled",
    ];
    SYSTEM_SERVICE_CACHES
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
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
        // `.map(|p| p.as_path())` 不是多余的：clippy::ptr_arg 要求 `scan_app`
        // 收 `&Path` 而不是 `&PathBuf`，而 par_iter 产出的是 `&PathBuf`。
        let mut items: Vec<ScanItem> = app_paths
            .par_iter()
            .map(|p| p.as_path())
            .flat_map(scan_app)
            .collect();

        // 扫描废纸篓和 Downloads 中的 .app 残留
        items.extend(scan_trash_apps());
        items.extend(scan_downloads_apps());

        // 扫描已卸载 App 在 Library 中的数据/缓存/配置残留
        scan_app_leftovers(&mut items);

        // 按大小降序排列
        items.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));

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
///
/// 注意：~/Applications/ 下存在子目录存放 PWA 快捷方式（如
/// `Chrome Apps.localized/`、Safari Web Apps），因此对用户 Applications
/// 目录额外递归一层，否则 Chrome/Safari 安装的网页应用永远扫不到。
/// 枚举 `/Applications` 与 `~/Applications`（含一层子目录的 PWA）下的全部 `.app`。
///
/// 供 IM 容器数据判定与语义化清理项（named_catalog）复用：不依赖 Spotlight，
/// 直接读文件系统。返回空 Vec 表示目录不可读（异常），调用方需保守处理。
pub(crate) fn collect_app_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let home = home_dir();

    // /Applications：仅顶层（保持原行为，避免扫进 Utilities 等系统工具）
    if let Some(entries) = read_dir_with_timeout(Path::new("/Applications")) {
        for path in entries {
            if is_app_bundle(&path) {
                paths.push(path);
            }
        }
    }

    // ~/Applications：顶层 + 一层子目录
    let user_apps = home.join("Applications");
    if let Some(entries) = read_dir_with_timeout(&user_apps) {
        for path in entries {
            if is_app_bundle(&path) {
                paths.push(path);
            } else if path.is_dir() {
                if let Some(sub) = read_dir_with_timeout(&path) {
                    for sub_path in sub {
                        if is_app_bundle(&sub_path) {
                            paths.push(sub_path);
                        }
                    }
                }
            }
        }
    }

    paths
}

/// 判断路径是否为 .app 包
fn is_app_bundle(path: &Path) -> bool {
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

    if let Some(entries) = read_dir_with_timeout(&trash_dir) {
        for path in entries {
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

    if let Some(entries) = read_dir_with_timeout(&downloads) {
        for path in entries {
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
/// （与 collect_app_paths 一致：~/Applications 递归一层，覆盖 PWA 目录）
fn collect_installed_app_names() -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    let home = home_dir();
    let dirs = [PathBuf::from("/Applications"), home.join("Applications")];
    for dir in &dirs {
        if let Some(entries) = read_dir_with_timeout(dir) {
            for path in entries {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    names.insert(name.to_string());
                }
            }
        }
        // ~/Applications 子目录（Chrome Apps.localized 等）递归一层
        if dir == &home.join("Applications") {
            if let Some(entries) = read_dir_with_timeout(dir) {
                for path in entries {
                    if path.is_dir() {
                        if let Some(sub) = read_dir_with_timeout(&path) {
                            for sub_path in sub {
                                if let Some(name) = sub_path.file_name().and_then(|n| n.to_str()) {
                                    names.insert(name.to_string());
                                }
                            }
                        }
                    }
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
fn scan_app(app_path: &Path) -> Vec<ScanItem> {
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
pub(crate) fn is_cache_like_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("library/caches/")
        || lower.contains("library/logs/")
        || lower.contains("library/httpstorages/")
}

/// 计算单个路径的大小（文件或目录）
pub(crate) fn path_size(path: &str) -> u64 {
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
pub(crate) fn get_bundle_id(app_path: &Path) -> Option<String> {
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

/// 枚举 `/Applications` 与 `~/Applications`（含一层子目录的 PWA）下全部
/// `.app` 的 bundle id 集合。
///
/// 供 IM 容器数据判定「对应 App 是否仍安装」使用：App 已卸载后，其沙盒
/// 容器里的聊天数据成为可清理的孤儿。这里基于**文件系统真相**（直接读
/// 各 `.app/Contents/Info.plist` 的 `CFBundleIdentifier`），不依赖
/// Spotlight 索引或 Finder 自动化授权，因此在关闭 Spotlight 的机器上、
/// 或无自动化权限时也可靠。
///
/// # 调用方的保守义务
/// 返回**空**集合意味着「一个应用都没枚举到」（目录不可读等异常），
/// 而不是「机器上没有任何应用」。调用方必须把空集合当作无法判定，
/// 一律按「仍安装」保护受保护数据，绝不据此把数据误判为孤儿。
pub(crate) fn installed_bundle_id_set() -> std::collections::HashSet<String> {
    collect_app_paths()
        .iter()
        .filter_map(|p| get_bundle_id(p))
        .collect()
}

/// 从 Info.plist 读取应用显示名称
///
/// 优先读取 CFBundleDisplayName，回退到 CFBundleName。
/// macOS `defaults read` 对中文字符会输出 \uXXXX 转义序列，这里做解码。
pub(crate) fn get_app_display_name(app_path: &Path) -> Option<String> {
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

/// 判断一个字符串是否可以安全地用作路径的**单个**组件
///
/// `CFBundleIdentifier` / `CFBundleName` 都来自应用的 Info.plist，作者可以填任意值。
///   若不校验就拼进 `home.join(format!("Library/Containers/{}", bid))`，
///   `../../../../Users/xxx/Documents` 这类值会让拼出的路径穿越到 home 之外，
///   从而把整个 Documents 目录当作"应用关联数据"列入删除列表（历史 bug）。
///
/// 这里只禁止真正危险的东西（分隔符、`..`、隐藏名、控制字符），
/// 不限制空格/括号/加号等合法字符，避免误伤 "Microsoft Edge"、
/// "Visual Studio Code" 这类真实应用名。
fn is_safe_path_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s != "."
        && s != ".."
        && !s.contains("..")
        && !s.contains('/')
        && !s.contains('\\')
        && !s.starts_with('.')
        && !s.chars().any(|c| c.is_control())
}

/// 关联文件候选路径的安全边界
///
/// `find_associated_files` 只允许产出两类路径：
/// - 当前用户 home 下的文件
/// - 系统级 LaunchAgents / LaunchDaemons（需 sudo 删除）
///
/// 其余一律拒绝，包括任何通过 `..` 或符号链接穿越出边界的路径。
/// 这是入口字符校验之外的第二道闸 —— 即使将来新增了拼接点忘了校验，
/// 出口这里也能兜住。
fn is_allowed_associated_path(path: &Path, home: &Path) -> bool {
    let raw_roots: [&Path; 3] = [
        home,
        Path::new("/Library/LaunchAgents"),
        Path::new("/Library/LaunchDaemons"),
    ];
    // 归一化为真实路径，便于按组件匹配（而非字符串前缀）
    let roots: Vec<PathBuf> = raw_roots
        .iter()
        .map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf()))
        .collect();

    // 显式 `..` 组件直接拒绝（无需 stat，最快）
    if path
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return false;
    }

    // 能解析就用真实路径比对（防符号链接穿越），否则退化为原路径
    let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    roots.iter().any(|r| target.starts_with(r))
}

/// 从 Info.plist 读取应用版本号
///
/// 优先 CFBundleShortVersionString，回退到 CFBundleVersion。
pub(crate) fn get_app_version(app_path: &Path) -> Option<String> {
    let plist = app_path.join("Contents/Info.plist");
    if !plist.exists() {
        return None;
    }

    for key in &["CFBundleShortVersionString", "CFBundleVersion"] {
        let output = Command::new("defaults")
            .arg("read")
            .arg(&plist)
            .arg(key)
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let version = decode_unicode_escapes(&raw);
                if !version.is_empty() {
                    return Some(version);
                }
            }
        }
    }

    None
}

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
///
/// `~/Library/Group Containers` 不再整目录枚举：改在 [`find_associated_files`]
/// 内按 `<bundle_id>` / `group.<bundle_id>` 定点 stat，避免卸载被大目录 read_dir 拖住。

pub(crate) fn find_associated_files(bundle_id: &str, app_name: &str) -> Vec<String> {
    let home = home_dir();
    let mut paths = Vec::new();

    // 关联发现总预算：正常机器下面这些 exists() 都只是几次 metadata 查询、亚毫秒级；
    // 磁盘极端拥塞时单次查询也可能排队。预算在**每个**定点 stat 循环的迭代级检查
    // （不再只是个别大步骤），到点立即用「已发现」的部分返回，绝不拖住一键卸载
    // （本体删除与残留清理相比，本体优先；少量残留下次可再清）。
    let started = Instant::now();
    const ASSOC_FIND_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);
    let over_budget = |started: &Instant| started.elapsed() >= ASSOC_FIND_BUDGET;

    // 生成应用名称变体（含版本后缀剥离）
    // 安全：这些变体会被拼进路径，先筛掉可用于穿越的变体
    let name_variants: Vec<String> = generate_name_variants(app_name)
        .into_iter()
        .filter(|n| is_safe_path_segment(n))
        .collect();

    // 生成 bundle ID 变体（剥离版本后缀）
    let bundle_id_variants: Vec<String> = generate_bundle_id_variants(bundle_id)
        .into_iter()
        .filter(|b| is_safe_path_segment(b))
        .collect();

    // ---- 按 bundle ID 匹配的路径 ----

    // 1. ~/Library/Containers/<bundle_id>/
    for bid in &bundle_id_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Containers/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 2. ~/Library/Group Containers/ —— 按 bundle_id **定点** stat，不再全量枚举。
    //
    // 历史实现对整个 `Group Containers`（常上百个共享容器）read_dir 一遍再做
    // 子串匹配；该目录在磁盘繁忙时单次枚举就能被看门狗退避重试到数十秒，是
    // 「点卸载后长时间卡在卸载中」的主要来源之一。共享容器目录名由配置描述
    // 文件里的 team id + group id 决定，无法仅从 bundle id 无损推出 team id
    // 前缀（`<TEAMID>.xxx`）；这里只定点检查两种最常见、可安全推出的命名：
    //   * `<bundle_id>`
    //   * `group.<bundle_id>`
    // 宁可漏掉带未知 team id 前缀的共享容器（少量残留、风险低），也不为卸载
    // 阻塞在整目录枚举上；命中的候选仍要过出口白名单。
    if !over_budget(&started) {
        for bid in &bundle_id_variants {
            if over_budget(&started) {
                return paths;
            }
            for tail in [bid.as_str(), &format!("group.{}", bid)] {
                let p = home.join(format!("Library/Group Containers/{}", tail));
                let ps = p.to_string_lossy().to_string();
                if p.exists() && !paths.contains(&ps) {
                    paths.push(ps);
                }
            }
        }
    }

    // 3. ~/Library/Caches/<bundle_id>/
    for bid in &bundle_id_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Caches/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 4. ~/Library/Application Support/<app_name>/ (含命名变体)
    for name in &name_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Application Support/{}", name));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 5. ~/Library/Preferences/<bundle_id>.plist
    for bid in &bundle_id_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Preferences/{}.plist", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 6. ~/Library/Preferences/<bundle_id>/
    for bid in &bundle_id_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Preferences/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 7. 浏览器 PWA 的「浏览器侧数据」刻意不在此枚举/删除：
    //    Chromium 系 PWA 的内部注册（Preferences / LevelDB 的 web_apps 注册表）与
    //    `Web Applications/Manifest Resources/<资源id>` 使用的是与快捷方式 extension id
    //    不同的独立标识，无法从 shim `.app` 安全映射（盲删可能误删共享资源），且浏览器
    //    运行中改写其配置会造成损坏。PWA 卸载只移除 shim `.app` 本体（见
    //    ops::uninstall_app）；浏览器下次启动自检到快捷方式缺失，会自行清除失效注册与
    //    资源 —— 这与 Finder「拖进废纸篓」的官方卸载效果一致。

    // 7. ~/Library/Logs/<app_name>/ (含命名变体)
    for name in &name_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Logs/{}", name));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 8. ~/Library/Saved Application State/<bundle_id>.savedState/
    for bid in &bundle_id_variants {
        if over_budget(&started) {
            return paths;
        }
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
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/HTTPStorages/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 10. ~/Library/Cookies/<bundle_id>.binarycookies
    for bid in &bundle_id_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Cookies/{}.binarycookies", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 11. ~/Library/WebKit/<bundle_id>/
    for bid in &bundle_id_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/WebKit/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 12. ~/Library/Application Scripts/<bundle_id>/
    for bid in &bundle_id_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Application Scripts/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 13. ~/Library/Metadata/<bundle_id>/
    for bid in &bundle_id_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Metadata/{}", bid));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 14. ~/Library/Caches/<app_name>/ — 按应用名匹配（含命名变体）
    for name in &name_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Caches/{}", name));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 15/16. LaunchAgents / LaunchDaemons 是仅剩的「需要 read_dir 目录」的步骤。
    // 磁盘拥塞且已超预算时跳过（可能残留启动项配置，风险低、可下次再清），优先
    // 保证本体与主要数据的卸载不被目录枚举拖住。
    if !over_budget(&started) {
        // 15. ~/Library/LaunchAgents/ — 扫描用户级启动代理
        scan_launch_agents(&home, &bundle_id_variants, &name_variants, &mut paths);

        // 16. /Library/LaunchAgents/ 和 /Library/LaunchDaemons/ — 系统级（需 sudo）
        scan_system_launch_agents(&bundle_id_variants, &name_variants, &mut paths);
    }

    // 17. ~/Library/Preferences/<app_name>/ (按应用名匹配的偏好设置目录)
    for name in &name_variants {
        if over_budget(&started) {
            return paths;
        }
        let p = home.join(format!("Library/Preferences/{}", name));
        if p.exists() && !paths.contains(&p.to_string_lossy().to_string()) {
            paths.push(p.to_string_lossy().to_string());
        }
    }

    // 18. Embedded bundle ID — 扫描 .app 包内嵌的 XPC/appex 的 bundle ID
    // 这些是应用插件/扩展，它们的关联文件需要一并清理
    // (在 scan_app 中调用时传入 app_path，这里通过额外参数实现)

    // 出口兜底：只保留落在允许根目录之内的路径。
    // 上面 18 个拼接点任何一个漏校验，这里都能拦住。
    paths
        .into_iter()
        .filter(|p| is_allowed_associated_path(Path::new(p), &home))
        .collect()
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
    let entries = match read_dir_with_timeout(dir) {
        Some(e) => e,
        None => return,
    };

    for path in entries {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let file_path = path.to_string_lossy().to_string();

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
        if let Some(entries) = read_dir_with_timeout(&app_support) {
            for path in entries {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // 跳过系统级和开发工具目录
                if is_system_library_name(&name) {
                    continue;
                }
                // 跳过系统服务缓存（FamilyCircle 等）：受系统级访问控制保护，
                // 任何权限都删不掉，展示只会造成困惑 —— 直接不扫描
                if is_system_service_cache(&name) {
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
                // ACL deny 规则保护的路径（如 FamilyCircle）：任何权限都删不掉，
                // 扫描时直接标记为不可删除，避免用户勾选后反复授权重试。
                let (size, size_skipped) = dir_size_impl(&path);
                let path_str = path.to_string_lossy().to_string();
                let sys_blocked = crate::safety::path_is_inaccessible(&path_str);
                let acl_blocked = crate::safety::path_is_acl_protected(&path_str);
                let undeletable_reason = if sys_blocked {
                    "系统保护: 此路径受系统访问控制保护，无法读取或删除（含管理员/Touch ID）"
                        .to_string()
                } else if acl_blocked {
                    "ACL保护: 系统规则禁止删除此路径，任何权限均无法删除".to_string()
                } else {
                    String::new()
                };
                let mut description = format!("{} 的残留数据（App 可能已卸载）", name);
                if size_skipped {
                    description.push_str("（上次遍历卡死，大小可能不准）");
                }
                items.push(ScanItem {
                    path: path_str,
                    size_bytes: size,
                    category: "App残留".to_string(),
                    selected: false,
                    deletable: !sys_blocked && !acl_blocked,
                    undeletable_reason,
                    batch_paths: Vec::new(),
                    recommend: Recommend::Advanced,
                    description,
                });
            }
        }
    }

    // 扫描 ~/Library/Caches/ 下的残留（大于 100MB 的）
    let caches = home.join("Library/Caches");
    if caches.is_dir() {
        if let Some(entries) = read_dir_with_timeout(&caches) {
            for path in entries {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // 跳过系统缓存
                if is_system_library_name(&name) {
                    continue;
                }
                // 跳过系统服务缓存（FamilyCircle 等）：受系统级访问控制保护，
                // 任何权限都删不掉，展示只会造成困惑 —— 直接不扫描
                if is_system_service_cache(&name) {
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
                // ACL deny 规则保护的路径：任何权限都删不掉，扫描时直接标记。
                let (size, size_skipped) = dir_size_impl(&path);
                let path_str = path.to_string_lossy().to_string();
                let sys_blocked = crate::safety::path_is_inaccessible(&path_str);
                let acl_blocked = crate::safety::path_is_acl_protected(&path_str);
                let undeletable_reason = if sys_blocked {
                    "系统保护: 此路径受系统访问控制保护，无法读取或删除（含管理员/Touch ID）"
                        .to_string()
                } else if acl_blocked {
                    "ACL保护: 系统规则禁止删除此路径，任何权限均无法删除".to_string()
                } else {
                    String::new()
                };
                let mut description =
                    format!("{} 的残留缓存，删除后无影响（App 可能已卸载）", name);
                if size_skipped {
                    description.push_str("（上次遍历卡死，大小可能不准）");
                }
                items.push(ScanItem {
                    path: path_str,
                    size_bytes: size,
                    category: "App残留缓存".to_string(),
                    selected: false,
                    deletable: !sys_blocked && !acl_blocked,
                    undeletable_reason,
                    batch_paths: Vec::new(),
                    recommend: Recommend::CacheOnly,
                    description,
                });
            }
        }
    }

    // 扫描 ~/Library/Preferences/ 下的残留 plist（每个文件独立展示）
    let prefs = home.join("Library/Preferences");
    if prefs.is_dir() {
        if let Some(entries) = read_dir_with_timeout(&prefs) {
            for path in entries {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // 只处理 .plist 文件
                if !name.ends_with(".plist") {
                    continue;
                }

                // 跳过系统级和 Apple 官方 plist
                if is_system_library_name(&name) {
                    continue;
                }

                // 跳过系统服务缓存对应的 plist（familycircled.plist 等）：
                // 系统守护进程在用数据，不属于任何已卸载 App 的残留
                if is_system_service_cache(name.trim_end_matches(".plist")) {
                    continue;
                }

                // 只展示已卸载 App 的 plist
                if is_app_installed(&name, &installed_apps) {
                    continue;
                }

                let (recommend, description) = classify_leftover_plist(&name);
                let size = path_size(path.to_string_lossy().as_ref());
                let path_str = path.to_string_lossy().to_string();
                let sys_blocked = crate::safety::path_is_inaccessible(&path_str);
                let acl_blocked = crate::safety::path_is_acl_protected(&path_str);
                let undeletable_reason = if sys_blocked {
                    "系统保护: 此路径受系统访问控制保护，无法读取或删除（含管理员/Touch ID）"
                        .to_string()
                } else if acl_blocked {
                    "ACL保护: 系统规则禁止删除此路径，任何权限均无法删除".to_string()
                } else {
                    String::new()
                };

                items.push(ScanItem {
                    path: path_str,
                    size_bytes: size,
                    category: "App残留配置".to_string(),
                    selected: false,
                    deletable: !sys_blocked && !acl_blocked,
                    undeletable_reason,
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
        if let Some(entries) = read_dir_with_timeout(&apps_path) {
            for path in entries {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
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
        if let Some(entries) = read_dir_with_timeout(&applications) {
            for path in entries {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
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
    if let Some(stripped) = name.split('.').next_back() {
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
        if !name_alnum.is_empty()
            && !app_alnum.is_empty()
            && (name_alnum.starts_with(&app_alnum) || app_alnum.starts_with(&name_alnum))
        {
            return true;
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

    let Some(entries) = read_dir_with_timeout(vendor_path) else {
        return items;
    };

    for sub_path in entries {
        let sub_name = sub_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

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

        let (size, size_skipped) = dir_size_impl(&sub_path);
        if size > min_size {
            let sub_str = sub_path.to_string_lossy().to_string();
            let sys_blocked = crate::safety::path_is_inaccessible(&sub_str);
            let acl_blocked = crate::safety::path_is_acl_protected(&sub_str);
            let undeletable_reason = if sys_blocked {
                "系统保护: 此路径受系统访问控制保护，无法读取或删除（含管理员/Touch ID）"
                    .to_string()
            } else if acl_blocked {
                "ACL保护: 系统规则禁止删除此路径，任何权限均无法删除".to_string()
            } else {
                String::new()
            };
            let mut description = format!(
                "{} 中 {} 的残留数据（App 可能已卸载）",
                vendor_name, sub_name
            );
            if size_skipped {
                description.push_str("（上次遍历卡死，大小可能不准）");
            }
            items.push(ScanItem {
                path: sub_str,
                size_bytes: size,
                category: category.to_string(),
                selected: false,
                deletable: !sys_blocked && !acl_blocked,
                undeletable_reason,
                batch_paths: Vec::new(),
                recommend,
                description,
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

// =========================================================================
//  应用清单（一键卸载数据源）
// =========================================================================

/// 已安装应用清单条目
///
/// 应用卸载页「一键卸载」卡片的数据源：每应用一条，包含展示用元数据
/// （名称/版本/体积/保护级别）与 PWA 标记。体积口径与扫描器一致：
/// - `app_size`：.app 包本体
/// - `data_size`：关联数据（Containers / Application Support / Preferences 等）
/// - `cache_size`：关联缓存（Caches / Logs / HTTPStorages 等）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InstalledApp {
    /// 展示名（CFBundleDisplayName / CFBundleName）
    pub name: String,
    /// .app 包路径
    pub path: String,
    pub bundle_id: String,
    /// CFBundleShortVersionString（回退 CFBundleVersion），可能为空
    pub version: String,
    pub app_size: u64,
    pub data_size: u64,
    pub cache_size: u64,
    /// 是否为浏览器安装的 PWA / Web App
    pub is_pwa: bool,
    /// PWA 来源：chrome / edge / brave / chromium / safari（非 PWA 为空串）
    pub pwa_kind: String,
    /// 是否允许一键卸载（保护级别 + 系统应用名双重判定）
    pub deletable: bool,
    pub undeletable_reason: String,
    /// 保护级别：none / critical / official / data
    pub protection: String,
    /// 应用包内或同级目录是否存在官方卸载器
    pub has_official_uninstaller: bool,
    /// 应用真实图标（PNG data URL）。轻量清单阶段提取；体积补算阶段为 `None`。
    /// 提取失败（无 icns / sips 失败）时为 `None`，前端回退为首字母色块。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// 浏览器安装 PWA 的 bundle id 前缀 → 来源名
///
/// Chrome 系 PWA 快捷方式的 bundle id 形如 `com.google.Chrome.app.<hash>`，
/// Edge / Brave / Chromium 同构。Safari 网页应用为 `com.apple.Safari.WebApp.*`。
const PWA_BUNDLE_PREFIXES: &[(&str, &str)] = &[
    ("com.google.Chrome.app.", "chrome"),
    ("com.microsoft.edgemac.app.", "edge"),
    ("com.brave.Browser.app.", "brave"),
    ("com.chromium.Chromium.app.", "chromium"),
    ("com.apple.Safari.WebApp.", "safari"),
];

/// 判断一个应用是否为浏览器安装的 PWA / Web App，并返回来源名
///
/// 双重判定：先按 bundle id 前缀精确匹配，再按路径回退
/// （`~/Applications/Chrome Apps.localized/`、`Safari Web Apps.localized`）。
pub fn pwa_kind_for(bundle_id: &str, path: &str) -> Option<&'static str> {
    for (prefix, kind) in PWA_BUNDLE_PREFIXES {
        if bundle_id.starts_with(prefix) {
            return Some(kind);
        }
    }
    if path.contains("Chrome Apps.localized") {
        return Some("chrome");
    }
    if path.contains("Safari Web Apps.localized") || path.contains("Web Apps.localized") {
        return Some("safari");
    }
    None
}

// ---- 通用 Chromium PWA shim 识别（不依赖硬编码浏览器清单）----

/// Chromium 宿主浏览器 bundle id → PWA 来源名；无法识别的内核浏览器统一归 `chromium`。
///
/// Chrome 系各发行版的 PWA 安装机制同构（都是 `app_mode_loader` shim），
/// Arc / Opera / Vivaldi / Yandex / 豆包等非清单内浏览器也走同一路径，
/// 因此未知宿主安全地归为 `chromium`（前端对未知来源统一显示「PWA」）。
fn chromium_host_kind(host_bundle: &str) -> &'static str {
    let h = host_bundle.to_ascii_lowercase();
    if h.starts_with("com.google.chrome") {
        "chrome"
    } else if h.contains("edgemac") || h.contains("microsoftedge") || h.contains("msedge") {
        "edge"
    } else if h.contains("brave") {
        "brave"
    } else {
        // 含 org.chromium.* 以及 Arc / Opera / Vivaldi / Yandex / 豆包等其它内核
        "chromium"
    }
}

/// 依据 Chromium shim 的 `Info.plist` 特征判定 PWA 来源（纯逻辑，便于单测）。
///
/// - `shortcut_id` = `CrAppModeShortcutID`（快捷方式/扩展 id，存在即 shim）；
/// - `executable` = `CFBundleExecutable`（Chromium shim 固定为 `app_mode_loader`）；
/// - `host_bundle` = `CrBundleIdentifier`（生成该 shim 的宿主浏览器 bundle id）。
fn pwa_kind_from_shim(
    shortcut_id: Option<&str>,
    executable: Option<&str>,
    host_bundle: Option<&str>,
) -> Option<&'static str> {
    let has_shortcut = matches!(shortcut_id, Some(s) if !s.is_empty());
    let is_loader = matches!(executable, Some(e) if e == "app_mode_loader");
    if !has_shortcut && !is_loader {
        return None;
    }
    Some(host_bundle.map(chromium_host_kind).unwrap_or("chromium"))
}

/// 路径是否位于 `~/Applications/<容器>.localized/<Name>.app`（浏览器 PWA 容器布局）。
///
/// 只对这种布局里的包读 plist，避免给 `/Applications` 与 `~/Applications` 顶层的
/// 普通应用增加额外的元数据 IO。
fn in_localized_app_container(path: &str) -> bool {
    let marker = "/Applications/";
    let Some(idx) = path.find(marker) else {
        return false;
    };
    let rest = &path[idx + marker.len()..];
    // rest 形如 "<container>.localized/<Name>.app"；顶层 .app（无第二段）直接排除。
    let Some((container, after)) = rest.split_once('/') else {
        return false;
    };
    container.ends_with(".localized") && after.ends_with(".app")
}

/// 从 `.app/Contents/Info.plist` 读取一个字符串键（`defaults read`，失败回退 `plutil`）。
fn read_plist_string(app_path: &Path, key: &str) -> Option<String> {
    let plist = app_path.join("Contents/Info.plist");
    if !plist.exists() {
        return None;
    }
    if let Ok(out) = Command::new("defaults")
        .arg("read")
        .arg(&plist)
        .arg(key)
        .output()
    {
        if out.status.success() {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    if let Ok(out) = Command::new("plutil")
        .arg("-extract")
        .arg(key)
        .arg("raw")
        .arg(&plist)
        .output()
    {
        if out.status.success() {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// 综合判定一个 `.app` 是否为浏览器安装的 PWA / Web App，并返回来源名。
///
/// 先走 [`pwa_kind_for`] 的 bundle id / 路径快速判定；未命中且包位于 PWA 容器
/// （`~/Applications/<容器>.localized/`）时，再读 `Info.plist` 用 shim 特征判定，
/// 从而覆盖硬编码清单之外的 Chromium 内核浏览器（豆包 / Arc / Opera 等）。
pub(crate) fn detect_pwa_kind(
    app_path: &Path,
    bundle_id: &str,
    path: &str,
) -> Option<&'static str> {
    if let Some(kind) = pwa_kind_for(bundle_id, path) {
        return Some(kind);
    }
    if !in_localized_app_container(path) {
        return None;
    }
    let shortcut = read_plist_string(app_path, "CrAppModeShortcutID");
    let executable = read_plist_string(app_path, "CFBundleExecutable");
    let host = read_plist_string(app_path, "CrBundleIdentifier");
    pwa_kind_from_shim(
        shortcut.as_deref(),
        executable.as_deref(),
        host.as_deref(),
    )
}

/// 扫描全部已安装应用，返回完整清单（按应用本体大小降序）
///
/// 与 `UninstallScanner::scan` 共享同一套路径收集 / plist 读取 / 关联文件
/// 查找逻辑，但不做废纸篓 / Downloads / Library 残留扫描 —— 它是给
/// 「一键卸载」用的只读视图，需要带版本号与体积。
///
/// 注意：本函数会对每个应用的本体及全部关联目录做完整体积递归（微信 / Telegram
/// 等可达数十 GB），冷跑可能耗时数十秒。**GUI「应用卸载」页不要直接调用它**：
/// 先用 [`list_installed_apps_light`] 秒出带图标的清单，再用
/// [`app_inventory_sizes_for`] 在后台补体积。本函数供 CLI / 卸载时按需使用。
pub fn list_installed_apps() -> Vec<InstalledApp> {
    let app_paths = collect_app_paths();

    let mut apps: Vec<InstalledApp> = app_paths
        .par_iter()
        .map(|p| p.as_path())
        .filter_map(inspect_app)
        .collect();

    apps.sort_by(|a, b| {
        b.app_size
            .cmp(&a.app_size)
            .then_with(|| a.name.cmp(&b.name))
    });
    apps
}

/// 轻量应用清单（GUI 打开「应用卸载」页时秒回）：
///
/// 只枚举 `.app`、读 plist 元数据、提取真实图标，**不做任何目录体积递归**
/// （`app_size/data_size/cache_size` 全为 0）。体积随后由
/// [`app_inventory_sizes_for`] 在后台统一补算。按名称稳定排序，
/// 避免体积补回后卡片整屏重排跳动。
pub fn list_installed_apps_light() -> Vec<InstalledApp> {
    let app_paths = collect_app_paths();

    let mut apps: Vec<InstalledApp> = app_paths
        .par_iter()
        .map(|p| p.as_path())
        .filter_map(|p| inspect_app_meta(p, true))
        .collect();

    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps
}

/// 读取单个 .app 的元数据（bundle id 读不到或位于 /System/ 下则跳过）。
///
/// `with_icon` 为真时额外用 `sips` 提取真实图标（PNG data URL）；体积字段恒为 0。
fn inspect_app_meta(app_path: &Path, with_icon: bool) -> Option<InstalledApp> {
    let path_str = app_path.to_string_lossy();

    // 跳过 /System/ 下的应用（系统只读区域，无法删除）
    if path_str.starts_with("/System/") {
        return None;
    }

    let bundle_id = get_bundle_id(app_path)?;

    let name = get_app_display_name(app_path).unwrap_or_else(|| {
        app_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("未知应用")
            .to_string()
    });
    let version = get_app_version(app_path).unwrap_or_default();

    // 保护级别（与 scan_app 同一套判定）
    let protection = app_protection::check_bundle_protection(&bundle_id);
    let is_system_by_name = is_system_app(&name);
    let is_critical = matches!(protection, ProtectionLevel::Critical) || is_system_by_name;

    let (deletable, undeletable_reason, protection_label) = if is_critical {
        (false, "protection_critical".to_string(), "critical")
    } else if matches!(protection, ProtectionLevel::RequiresOfficialUninstaller) {
        let vendor = app_protection::get_security_vendor(&bundle_id).unwrap_or("官方");
        (
            false,
            format!("protection_official_uninstaller:{}", vendor),
            "official",
        )
    } else {
        let label = if matches!(protection, ProtectionLevel::DataProtected) {
            "data"
        } else {
            "none"
        };
        (true, String::new(), label)
    };

    let has_official_uninstaller = !is_critical
        && crate::scanner::official_uninstaller::find_official_uninstaller(&path_str, &name)
            .is_some();

    let pwa_kind = detect_pwa_kind(app_path, &bundle_id, &path_str);

    let icon = if with_icon {
        extract_app_icon_data_url(app_path)
    } else {
        None
    };

    Some(InstalledApp {
        name,
        path: path_str.into_owned(),
        bundle_id,
        version,
        app_size: 0,
        data_size: 0,
        cache_size: 0,
        is_pwa: pwa_kind.is_some(),
        pwa_kind: pwa_kind.unwrap_or("").to_string(),
        deletable,
        undeletable_reason,
        protection: protection_label.to_string(),
        has_official_uninstaller,
        icon,
    })
}

/// 读取单个 .app 的完整信息（元数据 + 体积），供 CLI / 卸载流程使用。
fn inspect_app(app_path: &Path) -> Option<InstalledApp> {
    let mut app = inspect_app_meta(app_path, false)?;

    app.app_size = dir_size(app_path);

    // 关联文件：数据类 / 缓存类拆开，口径与扫描器一致
    let associated = find_associated_files(&app.bundle_id, &app.name);
    let (cache_paths, data_paths): (Vec<String>, Vec<String>) =
        associated.into_iter().partition(|p| is_cache_like_path(p));
    app.data_size = data_paths.iter().map(|p| path_size(p)).sum();
    app.cache_size = cache_paths.iter().map(|p| path_size(p)).sum();

    Some(app)
}

/// 单个应用的体积补算结果（与轻量清单按 `path` 对齐）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppInventorySize {
    pub path: String,
    pub app_size: u64,
    pub data_size: u64,
    pub cache_size: u64,
}

/// 单条路径的只读占用（带持久体积缓存）：目录走
/// [`sizecache::dir_size_accurate_cached`]，普通文件取元数据长度。
///
/// 仅供应用卸载页的**只读占用展示**。物理块口径（`st_blocks×512`）与删除
/// 链路的 [`path_size`] 一致；差别在于大目录结果会落盘缓存（TTL 7 天），
/// 机器空闲算过一次后，即便随后磁盘繁忙也能秒回。不完整结果不写缓存。
fn path_size_cached(path: &str) -> u64 {
    let p = Path::new(path);
    if p.is_dir() {
        crate::scanner::sizecache::dir_size_accurate_cached(p).0
    } else if let Ok(meta) = p.symlink_metadata() {
        meta.len()
    } else {
        0
    }
}

/// 删除前的体积估算：**只读**持久缓存，命中给值；未命中 / 已过期一律返回 0，
/// 绝不为了算大小而遍历目录（普通文件取一次元数据长度，廉价）。
///
/// 一键卸载不能被"先精确算出将释放多少字节"拖住——对几十 GB 的关联数据做全
/// 递归，正是用户点卸载后长时间卡在「卸载中」的根因。删除动作本身不依赖体积；
/// 缓存里若已有空闲时算好的值就展示，没有就当作未知，先把东西删掉。
pub(crate) fn path_size_fast(path: &str) -> u64 {
    let p = Path::new(path);
    if p.is_dir() {
        crate::scanner::sizecache::dir_size_peek_cached(p).unwrap_or(0)
    } else if let Ok(meta) = p.symlink_metadata() {
        meta.len()
    } else {
        0
    }
}

/// 计算单个应用的本体 / 数据 / 缓存体积（只读展示口径，带持久缓存）。
///
/// 关联路径发现 / 数据 / 缓存分类与 [`inspect_app`] 完全一致；体积统计改用
/// [`path_size_cached`] / `dir_size_accurate_cached`，使高负载机器二次打开
/// 秒出。`/System/` 或读不到 bundle id 时返回 `None`。
/// 单应用内无需去重（本体 `.app` 与 `~/Library` 关联路径不会重叠）。
pub fn app_inventory_size_one(raw: &str) -> Option<AppInventorySize> {
    let app_path = Path::new(raw);
    let path_str = app_path.to_string_lossy();
    if path_str.starts_with("/System/") {
        return None;
    }
    let bundle_id = get_bundle_id(app_path)?;
    let name = get_app_display_name(app_path).unwrap_or_else(|| {
        app_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("未知应用")
            .to_string()
    });

    let app_size = crate::scanner::sizecache::dir_size_accurate_cached(app_path).0;
    let associated = find_associated_files(&bundle_id, &name);
    let (cache_paths, data_paths): (Vec<String>, Vec<String>) =
        associated.into_iter().partition(|p| is_cache_like_path(p));

    Some(AppInventorySize {
        path: raw.to_string(),
        app_size,
        data_size: data_paths.iter().map(|p| path_size_cached(p)).sum(),
        cache_size: cache_paths.iter().map(|p| path_size_cached(p)).sum(),
    })
}

/// 后台并行补算一批应用的体积。
///
/// 每个应用独立走 [`app_inventory_size_one`]，可被上层逐条产出（GUI 据此做
/// 流式回填：先算完的小应用先显示，不等最慢的大目录）。关联路径由
/// [`find_associated_files`] 按 bundle_id 定点发现（不再整目录枚举
/// `~/Library/Group Containers`），体积与卸载同口径。
pub fn app_inventory_sizes_for(paths: &[String]) -> Vec<AppInventorySize> {
    paths
        .par_iter()
        .filter_map(|p| app_inventory_size_one(p))
        .collect()
}

/// 并行补算体积，并在**每个应用算完时立即回调**一次（供 GUI 流式回填）。
///
/// 与 [`app_inventory_sizes_bounded`] 不同，本函数无墙钟预算、会等所有应用
/// 算完（CLI / 测试用）。回调跨线程触发，要求 `Sync`。
pub fn app_inventory_sizes_emit<F: Fn(AppInventorySize) + Sync>(paths: &[String], on_item: F) {
    paths.par_iter().for_each(|p| {
        if let Some(item) = app_inventory_size_one(p) {
            on_item(item);
        }
    });
    crate::scanner::sizecache::flush();
}

/// 有界并发 + 墙钟预算地补算应用体积，**算完一个回调一个**（GUI 流式回填）。
///
/// 针对磁盘高负载是一等公民：用固定 `concurrency` 个 worker（过多线程只会在
/// IO 拥塞时互相拖累），每条结果经 `on_item` 立即推送。超过 `budget` 后停止
/// 领取新任务，已在跑的单个大目录不强行中断（其内部 `dir_size` 有看门狗，
/// 会在子树级超时后自然结束），detach 到后台自生自灭，本函数准时返回，
/// 因此 UI 绝不会无限停留在「统计中…」。返回预算内实际算完的条目。
pub fn app_inventory_sizes_bounded<F>(
    paths: Vec<String>,
    concurrency: usize,
    budget: std::time::Duration,
    on_item: F,
) -> Vec<AppInventorySize>
where
    F: Fn(AppInventorySize) + Send + Sync + 'static,
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    let start = Instant::now();
    let concurrency = concurrency.clamp(1, 16);
    let total = paths.len();
    let completed = std::sync::Arc::new(AtomicUsize::new(0));
    let done = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    let (job_tx, job_rx) = mpsc::channel::<String>();
    let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
    let on_item = std::sync::Arc::new(on_item);

    // feeder：投递全部任务后 drop sender，worker 在队列耗尽时自然退出
    let feeder = std::thread::spawn(move || {
        for p in paths {
            if job_tx.send(p).is_err() {
                break;
            }
        }
    });

    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let job_rx = std::sync::Arc::clone(&job_rx);
        let on_item = std::sync::Arc::clone(&on_item);
        let completed = std::sync::Arc::clone(&completed);
        let done = std::sync::Arc::clone(&done);
        // 普通线程（非 scope）：预算到仍在跑的 worker 被 detach，不阻塞返回
        workers.push(std::thread::spawn(move || loop {
            if start.elapsed() >= budget {
                break;
            }
            let Ok(guard) = job_rx.lock() else { break };
            let path = match guard.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(p) => p,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue, // 预算到由循环头判
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break, // 任务投递完毕
            };
            drop(guard);
            if let Some(item) = app_inventory_size_one(&path) {
                on_item(item.clone());
                if let Ok(mut g) = done.lock() {
                    g.push(item);
                }
                completed.fetch_add(1, Ordering::SeqCst);
            } else {
                completed.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    // 全部完成则提前返回，否则到预算准时返回（不 join，detach 慢 worker）
    while completed.load(Ordering::SeqCst) < total && start.elapsed() < budget {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = feeder;
    drop(workers); // 显式 detach：不等待慢任务

    // 本次完整算出的目录落盘：机器空闲算过一次后，随后即便磁盘繁忙也能秒回。
    crate::scanner::sizecache::flush();

    std::sync::Arc::try_unwrap(done)
        .ok()
        .and_then(|m| m.into_inner().ok())
        .unwrap_or_default()
}

// ---- 应用真实图标提取（.icns → PNG data URL，使用系统自带 sips，零新依赖）----

/// 标准 Base64 编码（不引第三方依赖；图标数据无需 url-safe）。
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut push = |n: u32| out.push(TABLE[(n & 63) as usize] as char);

    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        push(n >> 18);
        push(n >> 12);
        push(n >> 6);
        push(n);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        push(n >> 18);
        push(n >> 12);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        push(n >> 18);
        push(n >> 12);
        push(n >> 6);
        out.push('=');
    }
    out
}

/// 定位 `.app` 内的图标文件（`.icns`）。
///
/// 先读 `Info.plist` 的 `CFBundleIconFile`（可能带或不带 `.icns` 扩展名），
/// 在 `Contents/Resources/` 下解析；读不到则兜底取 Resources 下第一个图标，
/// 优先 `AppIcon*` / `app.icns`。
fn locate_app_icns(app_path: &Path) -> Option<PathBuf> {
    let resources = app_path.join("Contents/Resources");

    if let Some(name) = read_plist_string(app_path, "CFBundleIconFile") {
        let direct = resources.join(&name);
        if direct.is_file() {
            return Some(direct);
        }
        let with_ext = if name.to_ascii_lowercase().ends_with(".icns") {
            name
        } else {
            format!("{}.icns", name)
        };
        let candidate = resources.join(&with_ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // 兜底：Resources 下任一 .icns，AppIcon / app 优先
    let mut entries = read_dir_with_timeout(&resources)?;
    entries.retain(|p| p.extension().is_some_and(|e| e == "icns"));
    entries.sort_by_key(|p| {
        let n = p
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if n.starts_with("appicon") || n == "app.icns" {
            0
        } else {
            1
        }
    });
    entries.into_iter().next()
}

/// 提取应用真实图标为 PNG data URL（长边 128px）。
///
/// 使用 macOS 自带 `/usr/bin/sips` 把 `.icns` 转成 PNG，再 Base64 内联，
/// 前端直接 `<img src>`。任何一步失败都返回 `None`（前端回退首字母色块），
/// 绝不因图标问题影响清单加载。
fn extract_app_icon_data_url(app_path: &Path) -> Option<String> {
    let icns = locate_app_icns(app_path)?;

    // 以 app 路径哈希命名临时文件，保证并发提取不同应用时互不冲突
    let mut hasher = DefaultHasher::new();
    app_path.hash(&mut hasher);
    let tmp = std::env::temp_dir().join(format!("maclean_icon_{:016x}.png", hasher.finish()));

    let ok = Command::new("/usr/bin/sips")
        .args(["-s", "format", "png", "-Z", "128"])
        .arg(&icns)
        .arg("--out")
        .arg(&tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return None;
    }

    let bytes = std::fs::read(&tmp).ok()?;
    let _ = std::fs::remove_file(&tmp);
    if bytes.len() < 16 {
        return None;
    }
    Some(format!(
        "data:image/png;base64,{}",
        base64_encode(&bytes)
    ))
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
    fn test_is_system_service_cache() {
        // FamilyCircle / familycircled：系统守护进程缓存，必须命中排除表
        assert!(is_system_service_cache("FamilyCircle"));
        assert!(is_system_service_cache("familycircle"));
        assert!(is_system_service_cache("familycircled"));
        assert!(is_system_service_cache("FAMILYCIRCLED"));
        // 普通第三方缓存不得误伤
        assert!(!is_system_service_cache("google-chrome"));
        assert!(!is_system_service_cache("com.jetbrains.intellij"));
        assert!(!is_system_service_cache("vscode-cache"));
        assert!(!is_system_service_cache(""));
        assert!(!is_system_service_cache("FamilyCircleBackup"));
    }

    #[test]
    fn leftover_scan_skips_system_service_caches() {
        // 源码级断言：Library 残留扫描（Application Support + Caches）必须在
        // is_system_library_name 之后紧接着跳过系统服务缓存 —— 否则 FamilyCircle
        // 这类"扫到了但任何权限都删不掉"的项会再次出现在列表里，用户又会陷入
        // 勾选 → Touch ID → 失败 → 重试的循环。
        let src = include_str!("uninstall.rs");
        // 只统计生产代码：include_str! 会把下面这条断言自身也算进去
        let prod = &src[..src.find("mod tests").expect("测试模块")];
        // Application Support + Caches 两处目录扫描各应有一处排除表调用
        assert_eq!(
            prod.matches("is_system_service_cache(&name)").count(),
            2,
            "目录残留扫描（Application Support / Caches）漏了系统服务缓存排除"
        );
        // Preferences 扫描按 plist stem 匹配（familycircled.plist 等）
        assert!(
            src.contains("is_system_service_cache(name.trim_end_matches(\".plist\"))"),
            "Preferences 扫描没有跳过系统服务缓存 plist"
        );
    }

    // ---------- P0-4: bundle ID / 应用名不得用于路径穿越 ----------

    #[test]
    fn safe_path_segment_rejects_traversal() {
        // 合法应用名必须放行（含空格、点、连字符）
        assert!(is_safe_path_segment("com.example.app"));
        assert!(is_safe_path_segment("Microsoft Edge"));
        assert!(is_safe_path_segment("Visual Studio Code"));
        assert!(is_safe_path_segment("com.jetbrains.intellij"));

        // 危险值必须拦截
        assert!(!is_safe_path_segment("../.."));
        assert!(!is_safe_path_segment(".."));
        assert!(!is_safe_path_segment("."));
        assert!(!is_safe_path_segment("a/../b"));
        assert!(!is_safe_path_segment("/etc"));
        assert!(!is_safe_path_segment("Library/Caches"));
        assert!(!is_safe_path_segment(""));
        // 隐藏名（.ssh / .git 等）不得作为关联文件名
        assert!(!is_safe_path_segment(".ssh"));
    }

    #[test]
    fn allowed_associated_path_rejects_outside_home() {
        let home = home_dir();
        assert!(is_allowed_associated_path(
            &home.join("Library/Caches/Foo"),
            &home
        ));
        // 系统级 LaunchAgents 允许（需 sudo 删除）
        assert!(is_allowed_associated_path(
            Path::new("/Library/LaunchAgents/com.foo.plist"),
            &home
        ));
        // home 之外一律拒绝
        assert!(!is_allowed_associated_path(Path::new("/etc/passwd"), &home));
        assert!(!is_allowed_associated_path(
            Path::new("/Users/Shared/Evil"),
            &home
        ));
    }

    #[test]
    fn associated_files_reject_path_traversal_bundle_id() {
        // CFBundleIdentifier 来自 Info.plist，应用作者可填任意值。
        // 恶意值不得让 find_associated_files 产出 home 之外的路径 ——
        // 否则整个 Documents 会被当作"应用关联数据"列入删除。
        let home = home_dir();
        let home_str = home.to_string_lossy().to_string();
        for malicious in [
            "../../../../Users/Shared/Documents",
            "../../../..",
            "/etc",
            "..",
            "/Applications",
            "..%2f..",
            "Foo/../../etc",
        ] {
            let paths = find_associated_files(malicious, "Evil App");
            for p in &paths {
                // 独立断言：**不复用** is_allowed_associated_path，
                // 否则那个函数本身出错时这个测试会跟着一起失效。
                let in_allowed_root = p.starts_with(&home_str)
                    || p.starts_with("/Library/LaunchAgents")
                    || p.starts_with("/Library/LaunchDaemons");
                assert!(
                    in_allowed_root,
                    "恶意 bundle id {:?} 产出了越界路径: {}",
                    malicious, p
                );
                assert!(
                    !p.contains(".."),
                    "恶意 bundle id {:?} 产出了含 '..' 的路径: {}",
                    malicious,
                    p
                );
            }
        }
    }

    #[test]
    fn associated_files_still_finds_legitimate_paths() {
        // 收紧校验不能把正常功能改坏：合法 bundle ID 仍应产出候选路径
        // （这里只校验"不抛错 + 结果均在允许范围内"，不强依赖本机是否装了该应用）
        let home = home_dir();
        let paths = find_associated_files("com.apple.Safari", "Safari");
        for p in &paths {
            assert!(
                is_allowed_associated_path(Path::new(p), &home),
                "合法 bundle id 产出了越界路径: {}",
                p
            );
        }
        let _ = paths;
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

    // ---------- 一键卸载：PWA 识别 ----------

    #[test]
    fn pwa_kind_detects_chrome_family_by_bundle_id() {
        assert_eq!(
            pwa_kind_for("com.google.Chrome.app.abcdef1234", "/x/Chrome.app"),
            Some("chrome")
        );
        assert_eq!(
            pwa_kind_for("com.microsoft.edgemac.app.abcdef", "/x/Edge.app"),
            Some("edge")
        );
        assert_eq!(
            pwa_kind_for("com.brave.Browser.app.abcdef", "/x/Brave.app"),
            Some("brave")
        );
        assert_eq!(
            pwa_kind_for("com.chromium.Chromium.app.abcdef", "/x/Chromium.app"),
            Some("chromium")
        );
        assert_eq!(
            pwa_kind_for("com.apple.Safari.WebApp.uuid", "/x/S.webapp"),
            Some("safari")
        );
    }

    #[test]
    fn pwa_kind_falls_back_to_path_for_chrome_apps_folder() {
        // 目录名回退：bundle id 读不到或命名不标准时，仍能按路径认出 PWA
        assert_eq!(
            pwa_kind_for(
                "com.example.unknown",
                "/Users/u/Applications/Chrome Apps.localized/Notion.app"
            ),
            Some("chrome")
        );
        assert_eq!(
            pwa_kind_for(
                "com.example.unknown",
                "/Users/u/Applications/Safari Web Apps.localized/X.app"
            ),
            Some("safari")
        );
    }

    #[test]
    fn pwa_kind_rejects_regular_apps() {
        assert_eq!(
            pwa_kind_for("com.jetbrains.intellij", "/Applications/IntelliJ IDEA.app"),
            None
        );
        assert_eq!(
            pwa_kind_for("com.google.Chrome", "/Applications/Google Chrome.app"),
            None,
            "Chrome 本体不是 PWA"
        );
        assert_eq!(
            pwa_kind_for("com.apple.Safari", "/Applications/Safari.app"),
            None
        );
    }

    #[test]
    fn chromium_host_kind_maps_known_browsers_and_falls_back() {
        assert_eq!(chromium_host_kind("com.google.Chrome"), "chrome");
        assert_eq!(chromium_host_kind("com.google.Chrome.canary"), "chrome");
        assert_eq!(chromium_host_kind("com.microsoft.edgemac"), "edge");
        assert_eq!(chromium_host_kind("com.brave.Browser"), "brave");
        assert_eq!(chromium_host_kind("org.chromium.Chromium"), "chromium");
        // 清单之外的 Chromium 内核浏览器统一归 chromium，而不是误判为非 PWA
        assert_eq!(chromium_host_kind("company.thebrowser.Browser"), "chromium"); // Arc
        assert_eq!(chromium_host_kind("com.operasoftware.Opera"), "chromium");
        assert_eq!(chromium_host_kind("com.doubao.something"), "chromium");
    }

    #[test]
    fn pwa_kind_from_shim_requires_real_shim_signature() {
        // 有 CrAppModeShortcutID：按宿主浏览器归类
        assert_eq!(
            pwa_kind_from_shim(Some("abc"), None, Some("com.google.Chrome")),
            Some("chrome")
        );
        // 可执行固定为 app_mode_loader 也成立；宿主缺失时归 chromium
        assert_eq!(
            pwa_kind_from_shim(None, Some("app_mode_loader"), None),
            Some("chromium")
        );
        // Arc 等未知宿主的 shim
        assert_eq!(
            pwa_kind_from_shim(Some("x"), None, Some("company.thebrowser.Browser")),
            Some("chromium")
        );
        // 普通应用：无 shortcut id、可执行不是 loader → 不是 PWA（即使装了 Chrome）
        assert_eq!(
            pwa_kind_from_shim(None, Some("MyApp"), Some("com.google.Chrome")),
            None
        );
        assert_eq!(pwa_kind_from_shim(None, None, None), None);
        // 空字符串 shortcut id 不构成 shim 特征
        assert_eq!(pwa_kind_from_shim(Some(""), None, None), None);
    }

    #[test]
    fn localized_container_predicate_matches_only_pwa_layout() {
        assert!(in_localized_app_container(
            "/Users/u/Applications/Chrome Apps.localized/X.app"
        ));
        assert!(in_localized_app_container(
            "/Users/u/Applications/Doubao Apps.localized/Some.webapp.app"
        ));
        // /Applications 顶层、~/Applications 顶层普通应用都不应触发 plist 特征判定
        assert!(!in_localized_app_container("/Applications/Google Chrome.app"));
        assert!(!in_localized_app_container("/Users/u/Applications/Foo.app"));
    }

    // 以下测试依赖 macOS 的 `defaults read` 真正解析 Info.plist。
    #[cfg(target_os = "macos")]
    #[test]
    fn detect_pwa_kind_reads_shim_signature_from_unlisted_browser() {
        use std::fs;
        let root = std::env::temp_dir().join(format!(
            "maclean_pwa_detect_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let container = root.join("Applications").join("Doubao Apps.localized");

        // 1) 清单之外浏览器（Doubao）的 shim：靠 Info.plist 特征识别为 chromium 系 PWA
        let shim = container.join("Demo.app");
        fs::create_dir_all(shim.join("Contents")).unwrap();
        fs::write(
            shim.join("Contents/Info.plist"),
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
              <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
              \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
              <plist version=\"1.0\"><dict>\
              <key>CFBundleIdentifier</key><string>com.doubao.demo.pwa</string>\
              <key>CFBundleExecutable</key><string>app_mode_loader</string>\
              <key>CFBundleName</key><string>Demo PWA</string>\
              <key>CrAppModeShortcutID</key><string>zzzshortid</string>\
              <key>CrBundleIdentifier</key><string>com.doubao.browser</string>\
              </dict></plist>",
        )
        .unwrap();
        let shim_str = shim.to_string_lossy().to_string();
        // 该 bundle id 与路径都不在硬编码清单里：必须靠 shim 特征命中
        assert_eq!(
            pwa_kind_for("com.doubao.demo.pwa", &shim_str),
            None,
            "前置：快速路径不应识别该非清单浏览器"
        );
        assert_eq!(
            detect_pwa_kind(&shim, "com.doubao.demo.pwa", &shim_str),
            Some("chromium")
        );

        // 2) 同一容器里的普通 .app（无 Cr* 特征、可执行不是 loader）不得误判
        let plain = container.join("Plain.app");
        fs::create_dir_all(plain.join("Contents")).unwrap();
        fs::write(
            plain.join("Contents/Info.plist"),
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
              <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
              \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
              <plist version=\"1.0\"><dict>\
              <key>CFBundleIdentifier</key><string>com.example.plain</string>\
              <key>CFBundleExecutable</key><string>Plain</string>\
              </dict></plist>",
        )
        .unwrap();
        let plain_str = plain.to_string_lossy().to_string();
        assert_eq!(
            detect_pwa_kind(&plain, "com.example.plain", &plain_str),
            None,
            "容器内普通 app 不得被误判为 PWA"
        );

        fs::remove_dir_all(&root).ok();
    }

    // 若本机装有 Chrome PWA，端到端确认真实 shim 被识别，且能读到 shortcut id。
    #[cfg(target_os = "macos")]
    #[test]
    fn detect_pwa_kind_on_real_chrome_pwa_if_present() {
        let candidate = home_dir().join("Applications/Chrome Apps.localized/X.app");
        if !candidate.exists() {
            return; // 本机没有该 PWA，跳过
        }
        let path = candidate.to_string_lossy().to_string();
        assert_eq!(
            detect_pwa_kind(
                &candidate,
                "com.google.Chrome.app.lodlkdfmihgonocnmddehnfgiljnadcf",
                &path
            ),
            Some("chrome")
        );
        assert!(read_plist_string(&candidate, "CrAppModeShortcutID").is_some());
        assert_eq!(
            read_plist_string(&candidate, "CrBundleIdentifier").as_deref(),
            Some("com.google.Chrome")
        );
    }

    // ---- 图标 Base64 / 轻量清单 / 体积补算 ----

    #[test]
    fn base64_encode_matches_rfc4648_vectors() {
        // RFC 4648 标准测试向量
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // 任意字节往返长度自洽（输出长度为 4 的倍数，含 padding）
        let raw: Vec<u8> = (0u32..256).map(|i| (i % 256) as u8).collect();
        let enc = base64_encode(&raw);
        assert_eq!(enc.len(), raw.len().div_ceil(3) * 4);
        assert!(enc.ends_with('='));
    }

    #[test]
    fn app_inventory_sizes_empty_and_invalid_inputs() {
        // 空输入 → 空结果；非法 / /System 路径不 panic、被安全跳过
        assert!(app_inventory_sizes_for(&[]).is_empty());
        let out = app_inventory_sizes_for(&[
            "/definitely/not/an/app.app".to_string(),
            "/System/Applications/Safari.app".to_string(),
        ]);
        assert!(out.is_empty());
    }

    #[test]
    fn app_inventory_sizes_bounded_never_blocks_past_budget() {
        use std::time::Duration;
        // 空输入立即返回
        assert!(app_inventory_sizes_bounded(Vec::new(), 2, Duration::from_millis(50), |_| {}).is_empty());

        // 非法路径在 worker 内被快速跳过（completed 计数），必须很快结束、不挂死
        let t = Instant::now();
        let out = app_inventory_sizes_bounded(
            vec!["/definitely/not/an/app.app".to_string()],
            2,
            Duration::from_millis(300),
            |_| {},
        );
        assert!(out.is_empty());
        assert!(t.elapsed() < Duration::from_secs(3), "有界统计必须准时返回");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn icon_and_light_inventory_on_real_apps_if_present() {
        // Keynote（标准 .icns）若在，图标定位 + 轻量清单应秒返回且不做体积
        let keynote = PathBuf::from("/Applications/Keynote.app");
        if keynote.exists() {
            let icns = locate_app_icns(&keynote).expect("Keynote 应能定位到 .icns");
            assert!(icns.extension().is_some_and(|e| e == "icns"));
            let data_url = extract_app_icon_data_url(&keynote).expect("Keynote 图标应能转 PNG");
            assert!(data_url.starts_with("data:image/png;base64,"));
            assert!(data_url.len() > 100);
        }

        // 轻量清单：体积字段必须全为 0（保证不做重 IO），图标字段存在（成功或 None）
        let light = list_installed_apps_light();
        assert!(!light.is_empty(), "本机应至少能枚举出应用");
        for a in &light {
            assert_eq!(a.app_size, 0, "轻量清单不得统计体积: {}", a.name);
            assert_eq!(a.data_size, 0);
            assert_eq!(a.cache_size, 0);
            // 排序按名称稳定（体积补回后不重排）
        }
        for w in light.windows(2) {
            assert!(w[0].name <= w[1].name, "轻量清单应按名称排序");
        }

        // Chrome PWA X：应识别为 PWA，且能从 shim 提取到真实图标
        let x = home_dir().join("Applications/Chrome Apps.localized/X.app");
        if x.exists() {
            let xapp = light
                .iter()
                .find(|a| a.path == x.to_string_lossy().as_ref())
                .expect("轻量清单应包含 X PWA");
            assert!(xapp.is_pwa);
            assert_eq!(xapp.pwa_kind, "chrome");
            assert!(
                xapp.icon.as_deref().is_some_and(|s| s.starts_with("data:image/png;base64,")),
                "X PWA 应带真实图标 data URL"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_inventory_sizes_matches_full_scan_for_small_app() {
        // 体积补算与完整 inspect_app 口径一致：挑一个小真机应用比对（避免挑巨型应用拖慢测试）
        let candidate = home_dir().join("Applications/Chrome Apps.localized/X.app");
        let target = if candidate.exists() {
            candidate
        } else {
            let k = PathBuf::from("/Applications/Keynote.app");
            if k.exists() { k } else { return; }
        };
        let p = target.to_string_lossy().to_string();
        let sizes = app_inventory_sizes_for(&[p.clone()]);
        assert_eq!(sizes.len(), 1);
        assert_eq!(sizes[0].path, p);
        // 本体必然非零（.app 内含文件）
        assert!(sizes[0].app_size > 0);
        // 与完整 inspect_app 的本体体积逐字节一致（同一 dir_size 口径）
        if let Some(full) = inspect_app(&target) {
            assert_eq!(
                sizes[0].app_size, full.app_size,
                "体积补算口径必须与完整清单一致"
            );
        }
    }
}
