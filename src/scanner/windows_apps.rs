//! Windows 应用扫描器
//!
//! 通过 Windows 注册表扫描已安装应用，支持：
//! - App 缓存扫描（%LOCALAPPDATA%/%APPDATA% 下的 Cache 目录）
//! - App 数据扫描（%LOCALAPPDATA%/%APPDATA% 下的应用数据目录）
//! - App 卸载扫描（注册表 Uninstall 键 + UWP 应用）
//!
//! 注册表位置：
//! - HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall (系统级 64位)
//! - HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall (系统级 32位)
//! - HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall (用户级)

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{dir_size, home_dir, Recommend, ScanItem, ScanResult, Scanner};

/// Windows 已安装应用信息
#[derive(Debug, Clone)]
pub struct WindowsAppInfo {
    /// 显示名称
    pub name: String,
    /// 安装路径
    pub install_location: Option<String>,
    /// 卸载命令
    pub uninstall_string: Option<String>,
    /// 静默卸载命令
    pub quiet_uninstall_string: Option<String>,
    /// 估计大小（KB）
    pub estimated_size: Option<u64>,
    /// 发布者
    pub publisher: Option<String>,
    /// 注册表键名（ProductCode 或唯一标识）
    pub key_name: String,
    /// 是否为 UWP/Store 应用
    pub is_uwp: bool,
}

/// Windows 应用保护列表
///
/// 这些是 Windows 系统核心组件，删除会导致系统无法正常运行。
const CRITICAL_APP_NAMES: &[&str] = &[
    "Windows",
    "Microsoft Windows",
    "Windows Defender",
    "Windows Security",
    "Microsoft .NET",
    "Microsoft Visual C++",
    "Microsoft Edge",
    "Microsoft Edge Update",
    "Windows Installer",
    "Windows Software Development Kit",
    "Microsoft Store",
    "Windows Subsystem for Linux",
    "WSL",
];

/// 需要官方卸载工具的安全/MDM 应用
const REQUIRES_OFFICIAL_UNINSTALLER: &[&str] = &[
    "CrowdStrike",
    "SentinelOne",
    "Sentinel Labs",
    "ESET",
    "Kaspersky",
    "McAfee",
    "Norton",
    "Bitdefender",
    "Trend Micro",
    "Sophos",
    "Carbon Black",
    "Cylance",
    "Trellix",
    "Jamf",
];

/// 数据保护应用（卸载时警告数据丢失）
const DATA_PROTECTED_APPS: &[&str] = &[
    "1Password",
    "LastPass",
    "Bitwarden",
    "KeePass",
    "Microsoft Authenticator",
    "Google Authenticator",
    "微信",
    "WeChat",
    "QQ",
    "WhatsApp",
    "Telegram",
    "Signal",
    "钉钉",
    "DingTalk",
    "飞书",
    "Feishu",
    "Lark",
    "Slack",
    "Microsoft Teams",
    "Zoom",
    "输入法",
    "Input Method",
    "搜狗输入法",
    "百度输入法",
    "RIME",
    "小狼毫",
];

/// 应用保护级别
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WinProtectionLevel {
    None,
    Critical,
    RequiresOfficialUninstaller,
    DataProtected,
}

/// 检查应用保护级别
fn check_protection(name: &str) -> WinProtectionLevel {
    for &critical in CRITICAL_APP_NAMES {
        if name.eq_ignore_ascii_case(critical) || name.contains(critical) {
            return WinProtectionLevel::Critical;
        }
    }
    for &official in REQUIRES_OFFICIAL_UNINSTALLER {
        if name.contains(official) {
            return WinProtectionLevel::RequiresOfficialUninstaller;
        }
    }
    for &data_app in DATA_PROTECTED_APPS {
        if name.contains(data_app) {
            return WinProtectionLevel::DataProtected;
        }
    }
    WinProtectionLevel::None
}

// =========================================================================
//  注册表扫描
// =========================================================================

/// 查询注册表 Uninstall 键下所有应用
///
/// 使用 `reg query` 命令查询三个注册表位置。
fn scan_registry_uninstall() -> Vec<WindowsAppInfo> {
    let mut apps = Vec::new();

    let reg_paths = [
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    for reg_path in &reg_paths {
        // 列出所有子键
        let output = Command::new("reg")
            .args(["query", reg_path])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let line = line.trim();
                if !line.starts_with("HKEY") {
                    continue;
                }
                // 提取键名（最后一段）
                let key_name = line.rsplit('\\').next().unwrap_or("").to_string();
                if key_name.is_empty() {
                    continue;
                }
                // 查询该键的值
                if let Some(app) = query_app_info(reg_path, &key_name) {
                    apps.push(app);
                }
            }
        }
    }

    apps
}

/// 查询单个注册表键的应用信息
fn query_app_info(parent_path: &str, key_name: &str) -> Option<WindowsAppInfo> {
    let full_path = format!("{}\\{}", parent_path, key_name);
    let output = Command::new("reg")
        .args(["query", &full_path])
        .output();
    let out = output.ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);

    let mut name = None;
    let mut install_location = None;
    let mut uninstall_string = None;
    let mut quiet_uninstall_string = None;
    let mut estimated_size = None;
    let mut publisher = None;

    for line in stdout.lines() {
        let line = line.trim();
        // reg query 输出格式: "ValueName    REG_SZ    ValueData"
        if let Some(val) = parse_reg_line(line, "DisplayName") {
            name = Some(val);
        } else if let Some(val) = parse_reg_line(line, "InstallLocation") {
            if !val.is_empty() {
                install_location = Some(val);
            }
        } else if let Some(val) = parse_reg_line(line, "UninstallString") {
            uninstall_string = Some(val);
        } else if let Some(val) = parse_reg_line(line, "QuietUninstallString") {
            quiet_uninstall_string = Some(val);
        } else if let Some(val) = parse_reg_line(line, "EstimatedSize") {
            estimated_size = val.parse::<u64>().ok();
        } else if let Some(val) = parse_reg_line(line, "Publisher") {
            publisher = Some(val);
        }
    }

    let name = name?;
    // 跳过没有名称或名称为空的应用
    if name.trim().is_empty() {
        return None;
    }

    Some(WindowsAppInfo {
        name,
        install_location,
        uninstall_string,
        quiet_uninstall_string,
        estimated_size, // KB
        publisher,
        key_name: key_name.to_string(),
        is_uwp: false,
    })
}

/// 解析 reg query 输出行
///
/// 格式: "    DisplayName    REG_SZ    My Application"
/// 或:   "    EstimatedSize    REG_DWORD    0x1234"
fn parse_reg_line(line: &str, field: &str) -> Option<String> {
    if !line.to_lowercase().contains(&field.to_lowercase()) {
        return None;
    }
    // 分割: 字段名 | 类型 | 值
    let parts: Vec<&str> = line.splitn(3, "    ").collect();
    if parts.len() < 3 {
        // 尝试用多个空格分割
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }
        // 找到 REG_ 类型后的内容
        let type_idx = parts.iter().position(|p| p.starts_with("REG_"))?;
        let value = parts[type_idx + 1..].join(" ");
        // REG_DWORD 的值是 0x 开头的十六进制
        if parts[type_idx].contains("DWORD") {
            return Some(value);
        }
        return Some(value);
    }
    let value = parts[2].trim().to_string();
    if value.is_empty() {
        return None;
    }
    // REG_DWORD: 0x1234 -> 转十进制
    if parts[1].contains("DWORD") && value.starts_with("0x") {
        if let Ok(n) = u64::from_str_radix(&value[2..], 16) {
            return Some(n.to_string());
        }
    }
    Some(value)
}

/// 扫描 UWP/Store 应用（通过 PowerShell）
fn scan_uwp_apps() -> Vec<WindowsAppInfo> {
    let ps_cmd = "Get-AppxPackage | Select-Object Name, PackageFullName, InstallLocation | ConvertTo-Json";
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_cmd])
        .output();
    let mut apps = Vec::new();
    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        // 解析 JSON（简化处理，PowerShell 可能返回数组或单对象）
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            let arr = if json.is_array() {
                json.as_array().unwrap().clone()
            } else {
                vec![json]
            };
            for item in arr {
                let name = item.get("Name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let full_name = item.get("PackageFullName").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let install = item.get("InstallLocation").and_then(|v| v.as_str()).map(|s| s.to_string());
                if name.is_empty() {
                    continue;
                }
                apps.push(WindowsAppInfo {
                    name,
                    install_location: install,
                    uninstall_string: Some(format!("Remove-AppxPackage -Package {}", full_name)),
                    quiet_uninstall_string: Some(format!("Remove-AppxPackage -Package {}", full_name)),
                    estimated_size: None,
                    publisher: None,
                    key_name: full_name,
                    is_uwp: true,
                });
            }
        }
    }
    apps
}

/// 获取所有已安装应用（注册表 + UWP）
pub fn get_installed_apps() -> Vec<WindowsAppInfo> {
    let mut apps = scan_registry_uninstall();
    apps.extend(scan_uwp_apps());
    // 去重（按名称）
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    apps
}

// =========================================================================
//  App 缓存扫描器
// =========================================================================

/// Windows App 缓存扫描器
///
/// 扫描 %LOCALAPPDATA% 和 %APPDATA% 下的应用 Cache 目录
pub struct WindowsAppCacheScanner;

impl WindowsAppCacheScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for WindowsAppCacheScanner {
    fn scan(&self) -> ScanResult {
        let start = std::time::Instant::now();
        let home = home_dir();
        let mut items = Vec::new();

        // 扫描 %LOCALAPPDATA% 下的缓存目录
        let local_appdata = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Local"));
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Roaming"));

        // 缓存目录名模式
        let cache_dir_names = ["Cache", "cache", "Caches", "caches", "GPUCache", "Code Cache", "Service Worker"];

        for base in [&local_appdata, &appdata] {
            if !base.is_dir() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let app_dir = entry.path();
                    if !app_dir.is_dir() {
                        continue;
                    }
                    let app_name = entry.file_name().to_string_lossy().to_string();

                    // 检查子目录是否包含缓存
                    if let Ok(sub_entries) = std::fs::read_dir(&app_dir) {
                        for sub in sub_entries.flatten() {
                            let sub_name = sub.file_name().to_string_lossy().to_string();
                            if cache_dir_names.iter().any(|&cn| sub_name.eq_ignore_ascii_case(cn)) {
                                let cache_path = sub.path();
                                let size = dir_size(&cache_path);
                                if size > 0 {
                                    let (deletable, reason, recommend, desc) = if is_safe_cache(&app_name) {
                                        (true, String::new(), Recommend::Safe,
                                         format!("{} 应用的缓存文件，可安全删除", app_name))
                                    } else {
                                        (true, String::new(), Recommend::Caution,
                                         format!("{} 应用的缓存，删除后可能需重新配置", app_name))
                                    };
                                    items.push(ScanItem {
                                        path: cache_path.to_string_lossy().to_string(),
                                        size_bytes: size,
                                        category: format!("{} 缓存", app_name),
                                        selected: false,
                                        deletable,
                                        undeletable_reason: reason,
                                        batch_paths: Vec::new(),
                                        recommend,
                                        description: desc,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        ScanResult {
            items,
            total_size,
            scan_time_ms: start.elapsed().as_millis() as u64,
        }
    }
}

/// 判断应用缓存是否安全删除
fn is_safe_cache(app_name: &str) -> bool {
    // 浏览器和编辑器缓存通常安全
    let safe_apps = [
        "Google\\Chrome", "Microsoft\\Edge", "Mozilla\\Firefox",
        "Code", "Cursor", "JetBrains", "Postman",
        "Discord", "Slack", "Teams",
    ];
    safe_apps.iter().any(|a| app_name.contains(a))
}

// =========================================================================
//  App 数据扫描器
// =========================================================================

/// Windows App 数据扫描器
///
/// 扫描 %LOCALAPPDATA% 和 %APPDATA% 下的应用数据目录
pub struct WindowsAppDataScanner;

impl WindowsAppDataScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for WindowsAppDataScanner {
    fn scan(&self) -> ScanResult {
        let start = std::time::Instant::now();
        let home = home_dir();
        let mut items = Vec::new();

        let local_appdata = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Local"));
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Roaming"));

        // 系统保护目录（不扫描）
        let skip_dirs = [
            "Microsoft", "Packages", "ConnectedDevicesPlatform", "Temp",
            "CrashDumps", "D3DSCache", "DXCache", "NVIDIA", "AMD",
            "IconCache.db", "cache", "Cache",
        ];

        for base in [&local_appdata, &appdata] {
            if !base.is_dir() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let app_dir = entry.path();
                    if !app_dir.is_dir() {
                        continue;
                    }
                    let app_name = entry.file_name().to_string_lossy().to_string();

                    // 跳过系统目录
                    if skip_dirs.iter().any(|&s| app_name.eq_ignore_ascii_case(s)) {
                        continue;
                    }

                    let size = dir_size(&app_dir);
                    if size > 10 * 1024 * 1024 { // > 10MB
                        items.push(ScanItem {
                            path: app_dir.to_string_lossy().to_string(),
                            size_bytes: size,
                            category: format!("{} 数据", app_name),
                            selected: false,
                            deletable: false, // App 数据默认不可删除（高风险）
                            undeletable_reason: "应用数据删除可能导致应用配置丢失，请确认后手动删除".to_string(),
                            batch_paths: Vec::new(),
                            recommend: Recommend::Advanced,
                            description: format!(
                                "{} 应用的本地数据目录。\n⚠️ 删除可能导致应用配置、登录状态丢失，建议先备份。",
                                app_name
                            ),
                        });
                    }
                }
            }
        }

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        ScanResult {
            items,
            total_size,
            scan_time_ms: start.elapsed().as_millis() as u64,
        }
    }
}

// =========================================================================
//  App 卸载扫描器
// =========================================================================

/// Windows App 卸载扫描器
///
/// 扫描注册表中的已安装应用，支持卸载操作
pub struct WindowsUninstallScanner;

impl WindowsUninstallScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for WindowsUninstallScanner {
    fn scan(&self) -> ScanResult {
        let start = std::time::Instant::now();
        let apps = get_installed_apps();
        let mut items = Vec::new();

        for app in apps {
            let protection = check_protection(&app.name);
            let (deletable, reason, recommend, desc_suffix) = match protection {
                WinProtectionLevel::Critical => (
                    false,
                    "系统关键应用，禁止卸载".to_string(),
                    Recommend::Advanced,
                    " [系统保护]",
                ),
                WinProtectionLevel::RequiresOfficialUninstaller => (
                    false,
                    "安全软件，请使用官方卸载工具".to_string(),
                    Recommend::Advanced,
                    " [需官方卸载工具]",
                ),
                WinProtectionLevel::DataProtected => (
                    true,
                    String::new(),
                    Recommend::Caution,
                    " [⚠️ 数据保护]",
                ),
                WinProtectionLevel::None => (
                    true,
                    String::new(),
                    Recommend::Caution,
                    "",
                ),
            };

            // 计算大小
            let size = if let Some(ref loc) = app.install_location {
                let p = Path::new(loc);
                if p.is_dir() {
                    dir_size(p)
                } else {
                    app.estimated_size.unwrap_or(0) * 1024
                }
            } else {
                app.estimated_size.unwrap_or(0) * 1024
            };

            // 卸载路径标记（删除时识别）
            let path = if app.is_uwp {
                format!("uwp:{}", app.key_name)
            } else if app.uninstall_string.is_some() {
                format!("uninstall:{}", app.key_name)
            } else {
                continue; // 没有卸载命令的应用跳过
            };

            let display_size = if size > 0 {
                super::format_size(size)
            } else {
                "未知".to_string()
            };

            items.push(ScanItem {
                path,
                size_bytes: size,
                category: "应用名称".to_string(),
                selected: false,
                deletable,
                undeletable_reason: reason,
                batch_paths: Vec::new(),
                recommend,
                description: format!(
                    "{} ({}){} - 大小: {}",
                    app.name,
                    if app.is_uwp { "UWP" } else { "Win32" },
                    desc_suffix,
                    display_size
                ),
            });
        }

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        ScanResult {
            items,
            total_size,
            scan_time_ms: start.elapsed().as_millis() as u64,
        }
    }
}

// =========================================================================
//  应用卸载执行
// =========================================================================

/// 卸载 Windows 应用
///
/// 根据 path 前缀决定卸载方式：
/// - `uwp:<PackageFullName>` -> PowerShell Remove-AppxPackage
/// - `uninstall:<KeyName>` -> 查注册表执行 UninstallString
///
/// 返回 (success, output_message)
pub fn uninstall_app(path: &str) -> (bool, String) {
    if path.starts_with("uwp:") {
        let package_full_name = &path[4..];
        uninstall_uwp(package_full_name)
    } else if path.starts_with("uninstall:") {
        let key_name = &path[10..];
        uninstall_win32(key_name)
    } else {
        (false, "未知的卸载路径格式".to_string())
    }
}

/// 卸载 UWP/Store 应用
fn uninstall_uwp(package_full_name: &str) -> (bool, String) {
    let ps_cmd = format!("Remove-AppxPackage -Package '{}'", package_full_name);
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
        .output();
    match output {
        Ok(out) => {
            if out.status.success() {
                (true, format!("UWP 应用 {} 已卸载", package_full_name))
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                (false, format!("UWP 卸载失败: {}", stderr))
            }
        }
        Err(e) => (false, format!("执行 PowerShell 失败: {}", e)),
    }
}

/// 卸载 Win32 应用（通过注册表 UninstallString）
fn uninstall_win32(key_name: &str) -> (bool, String) {
    // 在注册表中查找该应用
    let reg_paths = [
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    for reg_path in &reg_paths {
        let full_path = format!("{}\\{}", reg_path, key_name);
        let output = Command::new("reg")
            .args(["query", &full_path])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);

            // 优先使用 QuietUninstallString（静默卸载）
            let mut quiet_cmd = None;
            let mut uninstall_cmd = None;

            for line in stdout.lines() {
                if let Some(val) = parse_reg_line(line, "QuietUninstallString") {
                    quiet_cmd = Some(val);
                } else if let Some(val) = parse_reg_line(line, "UninstallString") {
                    uninstall_cmd = Some(val);
                }
            }

            // 执行卸载命令
            if let Some(cmd) = quiet_cmd.or(uninstall_cmd) {
                return execute_uninstall_command(&cmd);
            }
        }
    }

    (false, format!("未找到应用 {} 的卸载命令", key_name))
}

/// 执行卸载命令
///
/// 解析 UninstallString 并执行，支持：
/// - msiexec /x {ProductCode} -> 加 /quiet /norestart
/// - "C:\path\uninstall.exe" /S -> 直接执行
/// - "C:\path\uninstall.exe" -> 尝试加 /S
fn execute_uninstall_command(cmd_str: &str) -> (bool, String) {
    let cmd_str = cmd_str.trim();

    // MSI 安装的应用: msiexec /x{ProductCode}
    if cmd_str.to_lowercase().contains("msiexec") {
        // 提取 ProductCode
        if let Some(start) = cmd_str.find('{') {
            if let Some(end) = cmd_str[start..].find('}') {
                let product_code = &cmd_str[start..=start + end];
                let full_cmd = format!("msiexec /x {} /quiet /norestart", product_code);
                let parts: Vec<&str> = full_cmd.split_whitespace().collect();
                if parts.len() >= 4 {
                    let output = Command::new(parts[0])
                        .args(&parts[1..])
                        .output();
                    return match output {
                        Ok(out) => {
                            if out.status.success() {
                                (true, format!("MSI 应用 {} 已静默卸载", product_code))
                            } else {
                                // 静默卸载失败，尝试交互式
                                let interactive_cmd = format!("msiexec /x {} /passive", product_code);
                                let parts: Vec<&str> = interactive_cmd.split_whitespace().collect();
                                let output = Command::new(parts[0]).args(&parts[1..]).output();
                                match output {
                                    Ok(o) if o.status.success() => (true, format!("MSI 应用 {} 已卸载", product_code)),
                                    Ok(o) => (false, format!("MSI 卸载失败: {}", String::from_utf8_lossy(&o.stderr))),
                                    Err(e) => (false, format!("执行 msiexec 失败: {}", e)),
                                }
                            }
                        }
                        Err(e) => (false, format!("执行 msiexec 失败: {}", e)),
                    };
                }
            }
        }
    }

    // 普通 exe 卸载程序
    // 尝试解析命令和参数
    let (exe_path, args) = parse_command(cmd_str);

    // 如果没有静默参数，尝试加 /S 或 /quiet
    let mut full_args = args.clone();
    if !full_args.iter().any(|a| {
        let lower = a.to_lowercase();
        lower == "/s" || lower == "/silent" || lower == "/quiet" || lower == "--silent"
    }) {
        full_args.push("/S".to_string());
    }

    let output = Command::new(&exe_path)
        .args(&full_args)
        .output();

    match output {
        Ok(out) => {
            if out.status.success() {
                (true, format!("应用 {} 已卸载", exe_path))
            } else {
                // 静默卸载失败，尝试原始命令（交互式）
                let output = Command::new(&exe_path)
                    .args(&args) // 不加 /S
                    .output();
                match output {
                    Ok(o) if o.status.success() => (true, format!("应用 {} 已卸载", exe_path)),
                    Ok(o) => (false, format!(
                        "卸载失败: {}",
                        String::from_utf8_lossy(&o.stderr)
                    )),
                    Err(e) => (false, format!("执行卸载程序失败: {}", e)),
                }
            }
        }
        Err(e) => (false, format!("执行卸载程序失败: {}", e)),
    }
}

/// 解析命令字符串为 (exe_path, args)
///
/// 处理带引号的路径：'"C:\Program Files\App\uninstall.exe" /S' -> ("C:\Program Files\App\uninstall.exe", ["/S"])
fn parse_command(cmd: &str) -> (String, Vec<String>) {
    let cmd = cmd.trim();

    if cmd.starts_with('"') {
        // 带引号的路径
        if let Some(end) = cmd[1..].find('"') {
            let exe = cmd[1..=end].to_string();
            let rest = cmd[end + 2..].trim();
            let args: Vec<String> = if rest.is_empty() {
                Vec::new()
            } else {
                rest.split_whitespace().map(String::from).collect()
            };
            return (exe, args);
        }
    }

    // 不带引号
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return (String::new(), Vec::new());
    }
    (
        parts[0].to_string(),
        parts[1..].iter().map(|s| s.to_string()).collect(),
    )
}

/// 卸载后清理残留文件
///
/// 扫描 %APPDATA% 和 %LOCALAPPDATA% 下与应用名匹配的残留目录
pub fn clean_app_residual(app_name: &str) -> Vec<(String, u64, bool)> {
    let home = home_dir();
    let mut residuals = Vec::new();

    let local_appdata = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Local"));
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Roaming"));

    for base in [&local_appdata, &appdata] {
        if !base.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let dir_name = entry.file_name().to_string_lossy().to_string();
                // 部分名称匹配（类似 macOS 的 App 残留检测）
                if dir_name.to_lowercase().contains(&app_name.to_lowercase()) {
                    let path = entry.path();
                    let size = dir_size(&path);
                    if size > 0 {
                        let success = std::fs::remove_dir_all(&path).is_ok();
                        residuals.push((path.to_string_lossy().to_string(), size, success));
                    }
                }
            }
        }
    }

    residuals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_quoted() {
        let (exe, args) = parse_command(r#""C:\Program Files\App\uninstall.exe" /S"#);
        assert_eq!(exe, r"C:\Program Files\App\uninstall.exe");
        assert_eq!(args, vec!["/S"]);
    }

    #[test]
    fn test_parse_command_unquoted() {
        let (exe, args) = parse_command("uninstall.exe /quiet");
        assert_eq!(exe, "uninstall.exe");
        assert_eq!(args, vec!["/quiet"]);
    }

    #[test]
    fn test_protection_critical() {
        assert_eq!(check_protection("Windows Defender"), WinProtectionLevel::Critical);
        assert_eq!(check_protection("Microsoft Edge"), WinProtectionLevel::Critical);
    }

    #[test]
    fn test_protection_official_uninstaller() {
        assert_eq!(check_protection("CrowdStrike Falcon"), WinProtectionLevel::RequiresOfficialUninstaller);
    }

    #[test]
    fn test_protection_data_protected() {
        assert_eq!(check_protection("1Password"), WinProtectionLevel::DataProtected);
        assert_eq!(check_protection("WeChat"), WinProtectionLevel::DataProtected);
    }

    #[test]
    fn test_protection_none() {
        assert_eq!(check_protection("Visual Studio Code"), WinProtectionLevel::None);
    }
}
