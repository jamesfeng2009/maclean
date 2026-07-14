//! 应用保护列表
//!
//! 参考 Mole 的 app_protection_data.sh，提供三级应用保护：
//! 1. 系统关键应用 — 禁止卸载（Finder、Dock、Safari 等）
//! 2. 安全/MDM 应用 — 需使用官方卸载工具（CrowdStrike、Jamf 等）
//! 3. 数据保护应用 — 可卸载但警告数据丢失（密码管理器、输入法等）
//!
//! 保护策略：
//! - Critical: 标记为不可删除（deletable=false），显示保护原因
//! - RequiresOfficialUninstaller: 标记为不可删除，提示使用官方卸载工具
//! - DataProtected: 允许删除但描述中包含数据丢失警告
//! - None: 正常扫描和删除

/// 应用保护级别
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectionLevel {
    /// 无保护，可正常卸载
    None,
    /// 系统关键，禁止卸载
    Critical,
    /// 需官方卸载工具（安全/MDM 应用）
    RequiresOfficialUninstaller,
    /// 数据保护，警告数据丢失
    DataProtected,
}

/// 系统关键 bundle ID 前缀/精确匹配列表
///
/// 这些是 macOS 系统核心组件，删除会导致系统无法正常运行。
/// 注意：不使用 "com.apple.*" 通配符，因为 Apple 也发布可卸载的用户级应用
/// （如 Xcode、Final Cut Pro、Logic Pro 等）。
const CRITICAL_BUNDLE_PREFIXES: &[&str] = &[
    // 核心系统应用
    "com.apple.finder",
    "com.apple.dock",
    "com.apple.Safari",
    "com.apple.mail",
    "com.apple.systempreferences",
    "com.apple.SystemSettings",
    "com.apple.Settings",
    "com.apple.controlcenter",
    "com.apple.Spotlight",
    "com.apple.notificationcenterui",
    "com.apple.loginwindow",
    // 系统服务和守护进程
    "com.apple.SecurityAgent",
    "com.apple.CoreServices",
    "com.apple.SystemUIServer",
    "com.apple.backgroundtaskmanagement",
    "com.apple.loginitems",
    "com.apple.sharedfilelist",
    "com.apple.sfl",
    "com.apple.coreservices",
    "com.apple.metadata",
    "com.apple.MobileSoftwareUpdate",
    "com.apple.SoftwareUpdate",
    "com.apple.installer",
    "com.apple.frameworks",
    "com.apple.security",
    "com.apple.keychain",
    "com.apple.trustd",
    "com.apple.securityd",
    "com.apple.cloudd",
    "com.apple.iCloud",
    "com.apple.WiFi",
    "com.apple.airport",
    "com.apple.Bluetooth",
    // 输入法（系统内置）
    "com.apple.inputmethod",
    "com.apple.inputsource",
    "com.apple.TextInput",
    "com.apple.CharacterPicker",
    "com.apple.PressAndHold",
];

/// 可卸载的 Apple 应用 bundle ID 前缀
///
/// 这些是 Apple 发布但用户可自行安装/卸载的应用。
const APPLE_UNINSTALLABLE_PREFIXES: &[&str] = &[
    "com.apple.dt.",      // Xcode, Instruments, FileMerge
    "com.apple.FinalCut", // Final Cut Pro
    "com.apple.Motion",
    "com.apple.Compressor",
    "com.apple.logic",      // Logic Pro
    "com.apple.garageband", // GarageBand
    "com.apple.iMovie",
    "com.apple.iWork.", // Pages, Numbers, Keynote
    "com.apple.MainStage",
    "com.apple.server.",     // macOS Server
    "com.apple.Playgrounds", // Swift Playgrounds
];

/// 安全/MDM 应用规则：厂商名称 + bundle ID 前缀
///
/// 这些应用必须使用厂商提供的官方卸载工具，
/// 直接删除可能触发 tamper 检测或导致系统管理功能失效。
const SECURITY_APP_RULES: &[(&str, &[&str])] = &[
    ("ESET", &["com.eset."]),
    ("Jamf", &["com.jamf.", "com.jamfsoftware."]),
    ("CrowdStrike", &["com.crowdstrike."]),
    ("SentinelOne", &["com.sentinelone.", "com.sentinel-labs."]),
    ("GlobalProtect", &["com.paloaltonetworks."]),
    ("Cisco", &["com.cisco.anyconnect", "com.cisco.secureclient"]),
];

/// 数据保护应用 bundle ID 前缀
///
/// 这些应用包含用户敏感数据（密码、输入法词库等），
/// 卸载前应提醒用户备份。
const DATA_PROTECTED_PREFIXES: &[&str] = &[
    // 密码管理器
    "com.1password.",
    "com.agilebits.", // 1Password 旧 bundle ID
    "com.lastpass.",
    "com.dashlane.",
    "com.bitwarden.",
    "com.keepassx.",
    "org.keepassx.",
    "org.keepassxc.",
    // 输入法（第三方）
    "com.tencent.inputmethod.QQInput",
    "com.sogou.inputmethod.",
    "com.baidu.inputmethod.",
    "com.googlecode.rimeime.",
    "im.rime.",
    // 磁盘/清理工具（含扫描历史等数据）
    "com.nektony.",
    "com.macpaw.",
    "com.freemacsoft.AppCleaner",
    "com.daisydiskapp.",
];

/// 检查 bundle ID 的保护级别
///
/// 返回对应的保护级别，供卸载扫描器决定是否允许删除。
pub fn check_bundle_protection(bundle_id: &str) -> ProtectionLevel {
    // 1. 检查是否为可卸载的 Apple 应用（Xcode 等）
    //    这类应用虽然是 com.apple.* 但允许卸载，需在 Critical 检查之前判断
    for prefix in APPLE_UNINSTALLABLE_PREFIXES {
        if bundle_id.starts_with(prefix) {
            return ProtectionLevel::None;
        }
    }

    // 2. 检查是否为系统关键应用
    for prefix in CRITICAL_BUNDLE_PREFIXES {
        if bundle_id.starts_with(prefix) {
            return ProtectionLevel::Critical;
        }
    }

    // 3. 检查是否为安全/MDM 应用
    for (_, prefixes) in SECURITY_APP_RULES {
        for prefix in *prefixes {
            if bundle_id.starts_with(prefix) {
                return ProtectionLevel::RequiresOfficialUninstaller;
            }
        }
    }

    // 4. 检查是否为数据保护应用
    for prefix in DATA_PROTECTED_PREFIXES {
        if bundle_id.starts_with(prefix) {
            return ProtectionLevel::DataProtected;
        }
    }

    ProtectionLevel::None
}

/// 获取安全/MDM 应用的厂商名称
///
/// 返回厂商名称用于显示"请使用 XXX 官方卸载工具"提示。
pub fn get_security_vendor(bundle_id: &str) -> Option<&'static str> {
    for (vendor, prefixes) in SECURITY_APP_RULES {
        for prefix in *prefixes {
            if bundle_id.starts_with(prefix) {
                return Some(vendor);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_critical_finder() {
        assert_eq!(
            check_bundle_protection("com.apple.finder"),
            ProtectionLevel::Critical
        );
    }

    #[test]
    fn test_critical_dock() {
        assert_eq!(
            check_bundle_protection("com.apple.dock"),
            ProtectionLevel::Critical
        );
    }

    #[test]
    fn test_critical_safari() {
        assert_eq!(
            check_bundle_protection("com.apple.Safari"),
            ProtectionLevel::Critical
        );
    }

    #[test]
    fn test_apple_uninstallable_xcode() {
        // Xcode 是 Apple 应用但可以卸载
        assert_eq!(
            check_bundle_protection("com.apple.dt.Xcode"),
            ProtectionLevel::None
        );
    }

    #[test]
    fn test_apple_uninstallable_finalcut() {
        assert_eq!(
            check_bundle_protection("com.apple.FinalCutPro"),
            ProtectionLevel::None
        );
    }

    #[test]
    fn test_security_crowdstrike() {
        assert_eq!(
            check_bundle_protection("com.crowdstrike.falcon"),
            ProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn test_security_jamf() {
        assert_eq!(
            check_bundle_protection("com.jamf.selfservice"),
            ProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn test_security_sentinelone() {
        assert_eq!(
            check_bundle_protection("com.sentinelone.agent"),
            ProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn test_security_cisco() {
        assert_eq!(
            check_bundle_protection("com.cisco.anyconnect"),
            ProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn test_security_vendor_name() {
        assert_eq!(
            get_security_vendor("com.crowdstrike.falcon"),
            Some("CrowdStrike")
        );
        assert_eq!(get_security_vendor("com.jamf.management"), Some("Jamf"));
        assert_eq!(get_security_vendor("com.apple.finder"), None);
    }

    #[test]
    fn test_data_protected_1password() {
        assert_eq!(
            check_bundle_protection("com.1password.1password"),
            ProtectionLevel::DataProtected
        );
    }

    #[test]
    fn test_data_protected_sogou_input() {
        assert_eq!(
            check_bundle_protection("com.sogou.inputmethod.sogou"),
            ProtectionLevel::DataProtected
        );
    }

    #[test]
    fn test_normal_third_party() {
        assert_eq!(
            check_bundle_protection("com.google.Chrome"),
            ProtectionLevel::None
        );
        assert_eq!(
            check_bundle_protection("com.microsoft.VSCode"),
            ProtectionLevel::None
        );
    }

    #[test]
    fn test_apple_uninstallable_logic() {
        assert_eq!(
            check_bundle_protection("com.apple.logic.pro"),
            ProtectionLevel::None
        );
    }

    #[test]
    fn test_apple_uninstallable_iwork() {
        assert_eq!(
            check_bundle_protection("com.apple.iWork.Pages"),
            ProtectionLevel::None
        );
    }
}
