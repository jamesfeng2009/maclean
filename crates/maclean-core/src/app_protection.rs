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

/// A-08：把表项规整为「域标签前缀」，统一小写后按「相等 / `前缀.` 边界」匹配。
///
/// 表项可能自带尾点（`com.apple.dt.`）也可能不带（`com.apple.FinalCut`），
/// 统一去掉尾点后比较，既让大小写混用的表项（`com.apple.Safari`）能命中
/// `com.apple.safari`，又不会让 `com.apple.finderfake` 被 `com.apple.finder` 越界命中。
fn label_matches(bundle_id: &str, prefix: &str) -> bool {
    let id = bundle_id.to_ascii_lowercase();
    let raw = prefix.to_ascii_lowercase();
    let p = raw.trim_end_matches('.');
    id == p || id.starts_with(&format!("{p}."))
}

/// A-08：bundle_id 是否命中前缀表任一项（统一走 label_matches）。
fn matches_any_prefix(bundle_id: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .copied()
        .any(|prefix| label_matches(bundle_id, prefix))
}

/// A-08：按优先级把四组命中标志收敛为保护级别（纯函数，生产与测试共用）。
///
/// 优先级：系统关键 > 安全/MDM > 可卸载白名单 > 数据保护 > 无。
/// 系统关键与安全/MDM **必须前置**：命中即保护，绝不能被「可卸载白名单」
/// 短路成 None（旧实现白名单在最前，`com.apple.dt.*` 直接 return None）。
/// 白名单仍保留覆盖数据保护的原相对行为，返回 None（与默认一致）。
fn classify_bundle(
    critical: bool,
    security: bool,
    uninstallable: bool,
    data_protected: bool,
) -> ProtectionLevel {
    if critical {
        return ProtectionLevel::Critical;
    }
    if security {
        return ProtectionLevel::RequiresOfficialUninstaller;
    }
    if uninstallable {
        return ProtectionLevel::None;
    }
    if data_protected {
        return ProtectionLevel::DataProtected;
    }
    ProtectionLevel::None
}

/// 检查 bundle ID 的保护级别
///
/// 返回对应的保护级别，供卸载扫描器决定是否允许删除。
/// A-08：Critical/Security 前置，匹配统一小写 + 边界（见 label_matches）。
pub fn check_bundle_protection(bundle_id: &str) -> ProtectionLevel {
    let critical = matches_any_prefix(bundle_id, CRITICAL_BUNDLE_PREFIXES);
    let security = SECURITY_APP_RULES
        .iter()
        .any(|rule| matches_any_prefix(bundle_id, rule.1));
    let uninstallable = matches_any_prefix(bundle_id, APPLE_UNINSTALLABLE_PREFIXES);
    let data_protected = matches_any_prefix(bundle_id, DATA_PROTECTED_PREFIXES);
    classify_bundle(critical, security, uninstallable, data_protected)
}

/// 获取安全/MDM 应用的厂商名称
///
/// 返回厂商名称用于显示"请使用 XXX 官方卸载工具"提示。
/// A-08：与保护判定共用 label_matches（统一小写 + 边界）。
pub fn get_security_vendor(bundle_id: &str) -> Option<&'static str> {
    for rule in SECURITY_APP_RULES.iter() {
        if matches_any_prefix(bundle_id, rule.1) {
            return Some(rule.0);
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

    #[test]
    fn label_matches_is_case_insensitive_and_boundary_aware() {
        // A-08：统一小写 + 相等/`prefix.` 边界。反向验证：去掉 to_ascii_lowercase
        // 或改回裸 starts_with，下面断言必红。
        assert!(label_matches("com.apple.safari", "com.apple.Safari"));
        assert!(label_matches("COM.APPLE.SAFARI", "com.apple.Safari"));
        assert!(label_matches("com.apple.Safari.helper", "com.apple.Safari"));
        assert!(label_matches("com.apple.dt.Xcode", "com.apple.dt.")); // 表项自带尾点
        assert!(label_matches(
            "com.cisco.anyconnect",
            "com.cisco.anyconnect"
        )); // 精确相等
            // 边界：兄弟名不得被前缀误命中
        assert!(!label_matches("com.apple.finderfake", "com.apple.finder"));
        assert!(!label_matches(
            "com.apple.FinalCutPro",
            "com.apple.FinalCut"
        ));
        assert!(!label_matches(
            "com.cisco.anyconnectX",
            "com.cisco.anyconnect"
        ));
    }

    #[test]
    fn check_bundle_protection_case_and_boundary_reverse_cases() {
        // A-08 反向用例（直接调生产 check_bundle_protection，改坏 label_matches 必红）：
        // - `com.apple.safari` 全小写过去因大小写敏感绕过 Critical，现必须 Critical；
        // - `com.apple.finderfake` 过去被裸前缀 `com.apple.finder` 误判 Critical，现必须 None。
        assert_eq!(
            check_bundle_protection("com.apple.safari"),
            ProtectionLevel::Critical
        );
        assert_eq!(
            check_bundle_protection("COM.APPLE.SAFARI"),
            ProtectionLevel::Critical
        );
        assert_eq!(
            check_bundle_protection("com.apple.Safari.evil"),
            ProtectionLevel::Critical
        );
        assert_eq!(
            check_bundle_protection("com.apple.finderfake"),
            ProtectionLevel::None
        );
    }

    #[test]
    fn classify_bundle_priority_puts_critical_and_security_first() {
        // A-08：Critical/Security 必须前置。反向验证：把 classify_bundle 里
        // `uninstallable` 分支挪到 `critical` 之前（退回旧优先级），两条断言必红。
        let cases = [
            (false, false, false, false, ProtectionLevel::None),
            (false, false, false, true, ProtectionLevel::DataProtected),
            (false, false, true, true, ProtectionLevel::None), // 白名单仍覆盖数据保护
            (true, false, true, false, ProtectionLevel::Critical), // 关键 > 白名单
            (
                false,
                true,
                true,
                false,
                ProtectionLevel::RequiresOfficialUninstaller,
            ), // 安全 > 白名单
            (true, true, true, true, ProtectionLevel::Critical), // 关键最高
        ];
        for (c, s, u, d, expected) in cases {
            assert_eq!(
                classify_bundle(c, s, u, d),
                expected,
                "critical={c} security={s} uninstallable={u} data_protected={d}"
            );
        }
    }

    #[test]
    fn test_security_vendor_is_case_insensitive() {
        // A-08：get_security_vendor 与保护判定共用 label_matches。
        assert_eq!(
            get_security_vendor("COM.CROWDSTRIKE.FALCON"),
            Some("CrowdStrike")
        );
        assert_eq!(get_security_vendor("Com.Jamf.Software"), Some("Jamf"));
        assert_eq!(get_security_vendor("com.crowdstrikeX"), None); // 边界：不得越界命中
    }
}
