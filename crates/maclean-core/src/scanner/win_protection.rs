//! Windows 应用保护判定（跨平台模块，不加 `cfg(windows)`）
//!
//! # 为什么这个模块不能放进 `windows_apps.rs`
//!
//! 这套判定是 Windows 侧卸载的**最后一道保护** —— 判定结果直接决定
//! `deletable` 与"是否执行 uninstall"。它原本写在 `windows_apps.rs` 里，
//! 而该模块整体是 `#[cfg(target_os = "windows")]`：在开发机（macOS）上
//! **整段不参与编译**，因此那套判定一行测试都跑不到。
//!
//! 这是本项目已经踩过一次的坑（见 `residual_match.rs` 的模块注释）：把逻辑
//! 写在 `cfg(windows)` 模块里，会得到"看起来有保护、实际 0 覆盖"的假象。
//! clippy 的 dead_code 也不会报 —— 因为在 Windows 目标上它们确实被调用了。
//!
//! 所以这里把 **判定所需的全部东西**搬出来：结构体、常量、纯函数，全部不加
//! cfg。它们只依赖 `String` / `bool` / 跨平台的文件系统调用，本机可编译可测。
//! `windows_apps.rs` 只负责**采集** `WindowsAppInfo`，判定一律走这里。
//!
//! 判定依赖的三个外部函数（`dir_size` / `home_dir` / `resembles_app_dir`）
//! 都是跨平台实现，不是 Windows 专属，可以直接用。

// 代价：不加 cfg 意味着在 macOS 目标上这些符号没有生产调用点，clippy 会把它们
// 全报成 dead_code。这里是刻意的 —— 宁可挂一条 allow 换"本机跑得到测试"，
// 也不要回到"只有 Windows 目标才编译、因此永远没测过"的状态。
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::path::PathBuf;

use super::residual_match::resembles_app_dir;
use super::{dir_size, home_dir};

/// Windows 已安装应用信息
///
/// 纯数据结构，不依赖任何 Windows API —— 这样才能在 macOS 开发机上构造
/// 测试用例。采集它（`get_installed_apps`）才是 Windows 专属的部分。
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
    /// 注册表 SystemComponent 标志（=1 表示系统组件，Windows 自带的隐藏标记）
    pub system_component: bool,
}

/// 应用保护级别
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WinProtectionLevel {
    None,
    Critical,
    RequiresOfficialUninstaller,
    DataProtected,
}

impl WinProtectionLevel {
    /// 该级别是否允许本工具执行卸载
    ///
    /// 这是**删除出口的判定依据**（`uninstall_app` 的闸门）。只有
    /// `None` / `DataProtected` 允许执行；后者的风险由 UI 侧警告承担。
    pub fn allows_uninstall(&self) -> bool {
        matches!(
            self,
            WinProtectionLevel::None | WinProtectionLevel::DataProtected
        )
    }

    /// 拒绝执行时给用户的理由（中文）
    pub fn block_reason(&self) -> Option<&'static str> {
        match self {
            WinProtectionLevel::Critical => Some("系统关键应用，禁止卸载"),
            WinProtectionLevel::RequiresOfficialUninstaller => {
                Some("安全/MDM 软件，请使用厂商官方卸载工具")
            }
            WinProtectionLevel::None | WinProtectionLevel::DataProtected => None,
        }
    }
}

/// 系统安装路径前缀（安装在这些目录下的应用视为系统级）
///
/// 注意：不包含裸 "C:\Program Files\"，因为该目录下大部分是普通第三方应用。
const SYSTEM_INSTALL_PATH_PREFIXES: &[&str] = &[
    "c:\\windows\\",
    "c:\\program files\\windowsapps\\",
    "c:\\program files (x86)\\windowsapps\\",
    "c:\\program files\\microsoft\\",
    "c:\\program files (x86)\\microsoft\\",
    "c:\\program files\\windows\\",
    "c:\\program files (x86)\\windows\\",
    "c:\\programdata\\microsoft\\",
];

/// 系统应用名称关键词（仅当发布者为 Microsoft 时才匹配）
///
/// 这些是功能类别词，不是具体应用名，可覆盖同一类别的所有应用。
const SYSTEM_NAME_KEYWORDS: &[&str] = &[
    "windows ",
    "microsoft .net",
    "microsoft visual c++",
    "microsoft edge",
    "windows defender",
    "windows security",
    "microsoft store",
    "windows installer",
    "windows sdk",
    "windows subsystem",
    "wsl",
    "microsoft azure",
    "directx",
];

/// 安全软件厂商关键词
///
/// 匹配应用名或发布者。厂商名稳定，比应用名少很多
/// （一家厂商可能发布多款安全产品，如 ESET NOD32 / ESET Internet Security）。
const SECURITY_VENDOR_KEYWORDS: &[&str] = &[
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
    "Palo Alto",
    "Fortinet",
    "Symantec",
    "Webroot",
    "Malwarebytes",
    "Tanium",
];

/// 安全软件类别关键词（匹配应用名，覆盖未列出的厂商）
const SECURITY_CATEGORY_KEYWORDS: &[&str] = &[
    "antivirus",
    "anti-virus",
    "anti malware",
    "anti-malware",
    "endpoint protection",
    "endpoint security",
    "internet security",
    "total security",
    "firewall",
    "mdm agent",
];

/// 数据保护功能类别关键词（按功能类别，不枚举具体应用名）
///
/// 密码管理器、输入法、认证器等有明确类别词的应用用关键词匹配。
/// IM/云盘等应用通过数据目录大小启发式检测，无需枚举。
const DATA_PROTECTED_KEYWORDS: &[&str] = &[
    // 密码/认证
    "password",
    "authenticator",
    "wallet",
    "vault",
    "keepass",
    // 输入法
    "输入法",
    "input method",
    "小狼毫",
    "rime",
];

/// 数据保护的数据目录大小阈值（字节）
///
/// 超过此值的应用数据目录（在 %APPDATA% 或 %LOCALAPPDATA% 下），
/// 视为含有用户重要数据（聊天记录、配置等），标记为 DataProtected。
const DATA_PROTECTED_SIZE_THRESHOLD: u64 = 50 * 1024 * 1024; // 50MB

/// 检查应用保护级别（通用规则检测）
///
/// 不依赖具体应用名枚举，而是通过：
/// 1. 注册表 SystemComponent 标志 + 系统路径 + Microsoft 发布者 → Critical
/// 2. 厂商关键词 + 安全类别关键词 → RequiresOfficialUninstaller
/// 3. 功能类别关键词 + 数据目录大小启发式 → DataProtected
pub fn check_protection(app: &WindowsAppInfo) -> WinProtectionLevel {
    // 1. Critical: 系统关键应用
    if is_critical_system_app(app) {
        return WinProtectionLevel::Critical;
    }
    // 2. Security: 需官方卸载工具
    if is_security_app(app) {
        return WinProtectionLevel::RequiresOfficialUninstaller;
    }
    // 3. DataProtected: 数据保护
    if is_data_protected_app(app) {
        return WinProtectionLevel::DataProtected;
    }
    WinProtectionLevel::None
}

/// 判断是否为系统关键应用
///
/// 检测规则（任一命中即视为系统应用）：
/// 1. 注册表 SystemComponent 标志 = 1（Windows 自带的系统组件标记）
/// 2. 安装路径在系统目录下（C:\Windows\、WindowsApps 等）
/// 3. 发布者为 Microsoft 且名称含系统关键词（windows、.net、defender 等）
pub fn is_critical_system_app(app: &WindowsAppInfo) -> bool {
    // 规则 1: 注册表 SystemComponent 标志
    if app.system_component {
        return true;
    }

    // 规则 2: 安装在系统目录
    if let Some(ref loc) = app.install_location {
        let loc_lower = loc.to_lowercase();
        if SYSTEM_INSTALL_PATH_PREFIXES
            .iter()
            .any(|p| loc_lower.starts_with(p))
        {
            return true;
        }
    }

    // 规则 3: Microsoft 发布 + 系统关键词
    let is_microsoft = app
        .publisher
        .as_deref()
        .map(|p| p.to_lowercase().contains("microsoft"))
        .unwrap_or(false);
    if is_microsoft {
        let name_lower = app.name.to_lowercase();
        if SYSTEM_NAME_KEYWORDS
            .iter()
            .any(|kw| name_lower.contains(kw))
        {
            return true;
        }
    }

    false
}

/// 判断是否为安全/MDM 软件
///
/// 检测规则（任一命中即视为需官方卸载工具）：
/// 1. 厂商关键词匹配 name 或 publisher
/// 2. 安全类别关键词匹配 name（覆盖未列出的厂商）
pub fn is_security_app(app: &WindowsAppInfo) -> bool {
    // 规则 1: 厂商关键词（匹配 name 或 publisher）
    for &vendor in SECURITY_VENDOR_KEYWORDS {
        let vendor_lower = vendor.to_lowercase();
        if app.name.to_lowercase().contains(&vendor_lower) {
            return true;
        }
        if let Some(ref pub_name) = app.publisher {
            if pub_name.to_lowercase().contains(&vendor_lower) {
                return true;
            }
        }
    }

    // 规则 2: 安全类别关键词（仅匹配 name）
    let name_lower = app.name.to_lowercase();
    if SECURITY_CATEGORY_KEYWORDS
        .iter()
        .any(|kw| name_lower.contains(kw))
    {
        return true;
    }

    false
}

/// 判断是否为数据保护应用
///
/// 检测规则（任一命中即视为数据保护）：
/// 1. 应用名包含功能类别关键词（password、输入法、rime 等）
/// 2. 数据目录大小启发式：应用在 %APPDATA% 或 %LOCALAPPDATA% 下的数据目录
///    超过 50MB，视为含有用户重要数据（聊天记录、文档等）
pub fn is_data_protected_app(app: &WindowsAppInfo) -> bool {
    // 规则 1: 功能类别关键词
    let name_lower = app.name.to_lowercase();
    if DATA_PROTECTED_KEYWORDS
        .iter()
        .any(|kw| name_lower.contains(kw))
    {
        return true;
    }

    // 规则 2: 数据目录大小启发式
    has_large_user_data(&app.name)
}

/// 检查应用是否有大量用户数据（>50MB）
///
/// 在 %APPDATA% 和 %LOCALAPPDATA% 下查找与应用名匹配的目录，
/// 如果目录大小超过阈值，返回 true。
///
/// 保护性启发式：命中就"不让卸载"，方向和删除相反 —— 这边漏报是少了一层
/// 保护（fail-open），所以刻意用宽松近似匹配。
///
/// 在 macOS 开发机上 `LOCALAPPDATA` / `APPDATA` 不存在，两个 base 目录都不
/// 是目录，会直接返回 false —— 这条路径本机测不到行为，只能测"不 panic"。
/// 大小阈值判定本身抽成 [`user_data_dir_exceeds_threshold`] 后可测。
fn has_large_user_data(app_name: &str) -> bool {
    let home = home_dir();
    let local_appdata = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Local"));
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Roaming"));

    let trimmed_name = app_name
        .trim_end_matches(" for windows")
        .trim_end_matches(" desktop")
        .trim_end_matches(" beta")
        .trim_end_matches(" (64-bit)");

    for base in [&local_appdata, &appdata] {
        if !base.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let dir_name = entry.file_name().to_string_lossy().to_string();
                if resembles_app_dir(&dir_name, trimmed_name) {
                    let path = entry.path();
                    if path.is_dir() && dir_size(&path) >= DATA_PROTECTED_SIZE_THRESHOLD {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// 去掉应用名尾部的平台/渠道后缀
///
/// 抽出来是因为 `has_large_user_data` 本体依赖真实文件系统，本机（macOS）
/// 没有 `%APPDATA%`，行为测不到；但"后缀剥离"是纯字符串逻辑，必须能测。
fn trimmed_app_name(app_name: &str) -> &str {
    // 原实现是 app_name.trim_end_matches(" for windows")...，那是**区分大小写**的：
    // "WeChat for Windows" 里的 'W' 是大写，后缀永远剥不掉，后面的目录名匹配
    // 就永远对不上 —— 保护性启发式静默失效（fail-open 到了"不保护"那一侧）。
    // 这里改成不区分大小写。用 to_ascii_lowercase 而不是 to_lowercase：后者对
    // 非 ASCII 可能改变字节长度（如 'İ'），会让下面的按字节切分 panic。
    const SUFFIXES: [&str; 4] = [" for windows", " desktop", " beta", " (64-bit)"];
    let mut s = app_name;
    loop {
        let lower = s.to_ascii_lowercase();
        match SUFFIXES.iter().find(|suf| lower.ends_with(*suf)) {
            // 后缀本身全是 ASCII，按字节切一定落在字符边界上
            Some(suf) => s = &s[..s.len() - suf.len()],
            None => break,
        }
    }
    s
}

/// 给定目录列表（目录名 → 大小），判断是否命中"大量用户数据"阈值
///
/// 把 `has_large_user_data` 中依赖文件系统的部分剥离后剩下的纯判定，
/// 这样 50MB 阈值与名称匹配这两条规则在本机也能被测试覆盖。
fn user_data_dir_exceeds_threshold(app_name: &str, dirs: &[(String, u64)]) -> bool {
    let trimmed = trimmed_app_name(app_name);
    dirs.iter().any(|(dir_name, size)| {
        resembles_app_dir(dir_name, trimmed) && *size >= DATA_PROTECTED_SIZE_THRESHOLD
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用 AppInfo：只填判定需要的字段
    fn app(name: &str, publisher: Option<&str>, loc: Option<&str>) -> WindowsAppInfo {
        WindowsAppInfo {
            name: name.to_string(),
            install_location: loc.map(|s| s.to_string()),
            uninstall_string: Some("MsiExec.exe /X{test}".to_string()),
            quiet_uninstall_string: None,
            estimated_size: None,
            publisher: publisher.map(|s| s.to_string()),
            key_name: "test-key".to_string(),
            is_uwp: false,
            system_component: false,
        }
    }

    // --- Critical ---

    #[test]
    fn system_component_flag_is_critical() {
        let mut a = app("Some App", Some("Acme"), Some(r"C:\Program Files\Some App"));
        a.system_component = true;
        assert_eq!(check_protection(&a), WinProtectionLevel::Critical);
    }

    #[test]
    fn system_install_path_is_critical() {
        let a = app("Whatever", Some("Acme"), Some(r"C:\Windows\System32\foo"));
        assert_eq!(check_protection(&a), WinProtectionLevel::Critical);
    }

    #[test]
    fn windowsapps_path_is_critical() {
        let a = app("Calculator", Some("Microsoft"), None);
        let mut a = a;
        a.install_location = Some(r"C:\Program Files\WindowsApps\Calc".to_string());
        assert_eq!(check_protection(&a), WinProtectionLevel::Critical);
    }

    #[test]
    fn microsoft_plus_system_keyword_is_critical() {
        let a = app(
            "Microsoft Visual C++ 2015 Redistributable",
            Some("Microsoft Corporation"),
            Some(r"C:\Program Files\VC"),
        );
        assert_eq!(check_protection(&a), WinProtectionLevel::Critical);
    }

    #[test]
    fn microsoft_without_system_keyword_is_not_critical() {
        // Microsoft 发布但不是系统组件 —— 不能因为是 Microsoft 就一律禁删
        let a = app(
            "Microsoft Visual Studio Code",
            Some("Microsoft Corporation"),
            Some(r"C:\Program Files\Microsoft VS Code"),
        );
        assert_ne!(check_protection(&a), WinProtectionLevel::Critical);
    }

    #[test]
    fn plain_program_files_is_not_critical() {
        // 关键回归：裸 "C:\Program Files\" 不在系统路径列表里，
        // 否则所有第三方应用都会被误判为系统关键
        let a = app(
            "7-Zip",
            Some("Igor Pavlov"),
            Some(r"C:\Program Files\7-Zip"),
        );
        assert_eq!(check_protection(&a), WinProtectionLevel::None);
    }

    // --- RequiresOfficialUninstaller ---

    #[test]
    fn security_vendor_in_name_is_protected() {
        let a = app("CrowdStrike Falcon", Some("CrowdStrike"), None);
        assert_eq!(
            check_protection(&a),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn security_vendor_in_publisher_is_protected() {
        // 应用名里没有厂商名，只有 Publisher 里有 —— 必须也能命中
        let a = app("Falcon Sensor", Some("CrowdStrike, Inc."), None);
        assert_eq!(
            check_protection(&a),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn unknown_security_vendor_via_category_keyword() {
        // 未列入厂商表的杀软，靠类别词兜住
        let a = app("ACME Antivirus 2024", Some("ACME"), None);
        assert_eq!(
            check_protection(&a),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn security_vendor_match_is_case_insensitive() {
        let a = app("crowdstrike falcon", Some("crowdstrike"), None);
        assert_eq!(
            check_protection(&a),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn firewall_is_treated_as_security() {
        let a = app("ZoneAlarm Firewall", Some("Check Point"), None);
        assert_eq!(
            check_protection(&a),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
    }

    // --- DataProtected ---

    #[test]
    fn password_manager_is_data_protected() {
        let a = app("KeePass Password Safe", Some("Dominik Reichl"), None);
        assert_eq!(check_protection(&a), WinProtectionLevel::DataProtected);
    }

    #[test]
    fn input_method_is_data_protected() {
        let a = app("搜狗输入法", Some("Sogou"), None);
        assert_eq!(check_protection(&a), WinProtectionLevel::DataProtected);
    }

    #[test]
    fn rime_is_data_protected() {
        let a = app("小狼毫 Rime", Some("Rime"), None);
        assert_eq!(check_protection(&a), WinProtectionLevel::DataProtected);
    }

    // --- 阈值判定（剥离文件系统依赖后可测） ---

    #[test]
    fn user_data_over_threshold_is_protected() {
        assert!(user_data_dir_exceeds_threshold(
            "WeChat",
            &[("WeChat".to_string(), 80 * 1024 * 1024)]
        ));
    }

    #[test]
    fn user_data_under_threshold_is_not_protected() {
        assert!(!user_data_dir_exceeds_threshold(
            "WeChat",
            &[("WeChat".to_string(), 10 * 1024 * 1024)]
        ));
    }

    #[test]
    fn threshold_boundary_is_inclusive() {
        // 恰好 50MB：阈值语义是 >=，应命中
        assert!(user_data_dir_exceeds_threshold(
            "WeChat",
            &[("WeChat".to_string(), DATA_PROTECTED_SIZE_THRESHOLD)]
        ));
    }

    #[test]
    fn unrelated_dir_name_does_not_match() {
        assert!(!user_data_dir_exceeds_threshold(
            "WeChat",
            &[("SomethingElse".to_string(), 999 * 1024 * 1024)]
        ));
    }

    #[test]
    fn platform_suffix_is_trimmed_before_matching() {
        assert_eq!(trimmed_app_name("WeChat for Windows"), "WeChat");
        assert_eq!(trimmed_app_name("Slack desktop"), "Slack");
        assert_eq!(trimmed_app_name("Discord beta"), "Discord");
        assert_eq!(trimmed_app_name("VS Code (64-bit)"), "VS Code");
    }

    // --- 出口闸门语义 ---

    #[test]
    fn critical_and_security_block_uninstall() {
        assert!(!WinProtectionLevel::Critical.allows_uninstall());
        assert!(!WinProtectionLevel::RequiresOfficialUninstaller.allows_uninstall());
    }

    #[test]
    fn none_and_data_protected_allow_uninstall() {
        assert!(WinProtectionLevel::None.allows_uninstall());
        assert!(WinProtectionLevel::DataProtected.allows_uninstall());
    }

    #[test]
    fn block_reason_only_for_blocked_levels() {
        assert!(WinProtectionLevel::Critical.block_reason().is_some());
        assert!(WinProtectionLevel::RequiresOfficialUninstaller
            .block_reason()
            .is_some());
        assert!(WinProtectionLevel::None.block_reason().is_none());
        assert!(WinProtectionLevel::DataProtected.block_reason().is_none());
    }

    // --- 优先级 ---

    #[test]
    fn critical_takes_priority_over_security() {
        // 一个既是系统组件、又像杀软的应用，应按更严格的 Critical 处理
        let mut a = app("Windows Defender Antivirus", Some("Microsoft"), None);
        a.system_component = true;
        assert_eq!(check_protection(&a), WinProtectionLevel::Critical);
    }

    #[test]
    fn security_takes_priority_over_data_protected() {
        // 名字既含密码词又是杀软 —— 安全等级更高者胜
        let a = app("Norton Password Vault", Some("Norton"), None);
        assert_eq!(
            check_protection(&a),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn common_apps_are_unprotected() {
        for name in ["Google Chrome", "Visual Studio Code", "7-Zip", "Spotify"] {
            let a = app(name, Some("Various"), Some(r"C:\Program Files\x"));
            assert_eq!(
                check_protection(&a),
                WinProtectionLevel::None,
                "{} 不应被保护",
                name
            );
        }
    }
}
