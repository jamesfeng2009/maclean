//! 安全防护模块
//!
//! 多层防护机制，确保不会误删除系统关键文件:
//! 1. 路径白名单 - 只允许删除已知安全路径下的文件
//! 2. 路径黑名单 - 明确禁止删除的系统关键路径
//! 3. 路径模式校验 - 检查路径是否匹配预期的安全模式
//! 4. 删除前二次校验 - 删除前再次检查路径合法性
//! 5. 删除日志 - 记录所有删除操作，可回溯

use std::path::{Path, PathBuf};

/// 安全检查结果
#[derive(Debug, Clone)]
pub enum SafetyCheck {
    /// 安全，可以删除
    Safe,
    /// 危险，拒绝删除，附带原因
    Danger(String),
    /// 警告，需要额外确认
    Warning(String),
}

/// 检查路径是否安全可删除
///
/// 这是删除前的最终安全屏障，即使扫描器有 bug 扫到了危险路径，
/// 这里的检查也会阻止删除。
///
/// 安全检查层（从外到内）:
/// 1. 空路径 / 非绝对路径检查
/// 2. 控制字符 / 路径遍历 `..` 防护
/// 3. 符号链接目标解析（拒绝指向保护目录的符号链接）
/// 4. 系统关键目录黑名单（/System, /usr, /bin 等 50+ 路径）
/// 5. 用户关键目录黑名单（Keychains, Mail, Messages 等）
/// 6. 白名单校验（只允许已知安全路径模式）
/// 7. 敏感文件名检测（.env, id_rsa, credentials 等）
pub fn check_path_safety(path: &str) -> SafetyCheck {
    // ================================================================
    //  第 0 层: 空路径检查
    // ================================================================
    if path.is_empty() {
        return SafetyCheck::Danger("路径为空".to_string());
    }

    // ================================================================
    //  第 1 层: 控制字符过滤 + 路径遍历防护
    // ================================================================
    // 拒绝包含控制字符的路径（防止注入攻击）
    if path.chars().any(|c| c.is_control() || c == '\n' || c == '\r' || c == '\0') {
        return SafetyCheck::Danger("路径包含控制字符".to_string());
    }

    // 路径遍历防护：拒绝 `..` 作为完整路径组件
    // 但允许文件名中包含 `..`（如 Firefox 的 name..files）
    let path_components: Vec<&str> = path.split('/').collect();
    if path_components.iter().any(|&c| c == "..") {
        return SafetyCheck::Danger("路径包含目录遍历 (..)".to_string());
    }

    // 非文件路径（APFS 快照等），跳过文件路径检查
    if path.starts_with("com.apple.TimeMachine.") || path.contains(" | ") {
        return SafetyCheck::Safe;
    }

    let p = Path::new(path);

    // ================================================================
    //  第 2 层: 符号链接目标解析
    // ================================================================
    // 如果是符号链接，解析目标并检查目标是否为保护路径
    if let Ok(meta) = p.symlink_metadata() {
        if meta.file_type().is_symlink() {
            match std::fs::canonicalize(p) {
                Ok(target) => {
                    let target_str = target.to_string_lossy().to_string();
                    // 检查符号链接目标是否指向系统保护目录
                    if is_critical_system_path(&target_str) {
                        return SafetyCheck::Danger(format!(
                            "符号链接指向系统保护目录: {} -> {}",
                            path, target_str
                        ));
                    }
                }
                Err(_) => {
                    return SafetyCheck::Danger(format!("符号链接目标无法解析: {}", path));
                }
            }
        }
    }

    let canonical = match p.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            return SafetyCheck::Danger(format!("路径无法解析: {}", path));
        }
    };

    let canonical_str = canonical.to_string_lossy().to_string();

    // ================================================================
    //  第 3 层: 系统关键目录黑名单（参考 Mole 的保护列表）
    // ================================================================
    if is_critical_system_path(&canonical_str) {
        return SafetyCheck::Danger(format!(
            "拒绝删除系统关键目录: {}",
            canonical_str
        ));
    }

    // 根目录本身绝对禁止
    if canonical == Path::new("/") {
        return SafetyCheck::Danger("拒绝删除根目录".to_string());
    }

    // ================================================================
    //  第 4 层: 用户主目录下的禁止路径
    // ================================================================
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/NONEXISTENT"));
    let home_str = home.to_string_lossy();

    // 用户主目录本身禁止删除
    if canonical == home {
        return SafetyCheck::Danger("拒绝删除用户主目录".to_string());
    }

    let forbidden_home_paths = [
        "Library/Preferences",           // 用户偏好设置
        "Library/Keychains",             // 钥匙串（密码）
        "Library/Accounts",              // 账户信息
        "Library/Mail",                  // 邮件数据
        "Library/Messages",              // 消息数据
        "Library/Cookies",               // Cookie
        "Library/Group Containers",      // Group Containers 根目录
        "Library/Containers",            // Containers 根目录
        "Library/Application Support/MobileSync",   // iOS 备份
        "Library/Application Support/AddressBook",  // 通讯录
        "Library/Application Support/CallHistoryDB", // 通话记录
        "Library/Application Support/CloudDocs",     // iCloud 文档
        "Library/Calendars",             // 日历
        "Library/Reminders",             // 提醒事项
        "Library/Notes",                 // 备忘录
        "Library/Safari",                // Safari 数据
        "Library/Assistants",            // Siri 数据
        "Library/Passwords",             // 密码
        "Library/Security",              // 安全数据
        "Library/Caches/Homebrew/Caskroom", // Homebrew 已安装应用
        ".ssh",                          // SSH 密钥
        ".gnupg",                        // GPG 密钥
        ".config/git",                   // Git 配置
    ];

    for forbidden_suffix in &forbidden_home_paths {
        let forbidden_path = home.join(forbidden_suffix);
        if canonical == forbidden_path {
            return SafetyCheck::Danger(format!(
                "拒绝删除用户关键目录: {}",
                canonical_str
            ));
        }
    }

    // ================================================================
    //  第 5 层: 白名单校验 - 只允许删除已知安全的路径模式
    // ================================================================
    let is_in_safe_zone = check_whitelist(&canonical, &home);

    if !is_in_safe_zone {
        return SafetyCheck::Danger(format!(
            "路径不在安全白名单内，拒绝删除: {}",
            canonical_str
        ));
    }

    // ================================================================
    //  第 6 层: 敏感文件名检测
    // ================================================================
    let file_name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let sensitive_names = [
        ".env",
        ".gitconfig",
        ".npmrc",
        ".cargo/credentials",
        "id_rsa",
        "id_ed25519",
        "known_hosts",
        "config.toml",
        "settings.json",
        "keychain",
        "password",
        "credential",
    ];

    for sensitive in &sensitive_names {
        if file_name.eq_ignore_ascii_case(sensitive)
            || canonical_str.contains(sensitive)
        {
            return SafetyCheck::Warning(format!(
                "路径包含敏感文件名 ({}): {}",
                sensitive, canonical_str
            ));
        }
    }

    SafetyCheck::Safe
}

/// 检查路径是否为系统关键保护路径
/// 参考 Mole 的 _mole_is_critical_deletion_path，包含 50+ 保护路径
fn is_critical_system_path(path: &str) -> bool {
    // 精确匹配的系统根路径
    let exact_match = [
        "/",
        "/bin",
        "/sbin",
        "/usr",
        "/System",
        "/Library",
        "/Applications",
        "/Volumes",
        "/opt",
        "/private",
        "/private/var",
        "/private/etc",
        "/private/tmp",
        "/private/var/db",
        "/private/var/log",
        "/private/var/audit",
        "/private/var/root",
        "/etc",
        "/var",
        "/dev",
        "/Users",
        "/Users/Shared",
        "/Users/Guest",
        "/cores",
    ];

    for &m in &exact_match {
        if path == m {
            return true;
        }
    }

    // 前缀匹配的系统保护路径
    let prefix_match = [
        "/System/",
        "/bin/",
        "/sbin/",
        "/usr/",
        "/Library/Apple/",
        "/Library/Application Support/",
        "/Library/Extensions/",
        "/Library/Keychains/",
        "/Library/Preferences/",
        "/Library/StartupItems/",
        "/Library/LaunchDaemons/",
        "/Library/LaunchAgents/",
        "/Library/Managed Preferences/",
        "/Library/ConfigurationProfiles/",
        "/private/var/db/",
        "/private/var/audit/",
        "/private/var/root/",
        "/private/etc/",
        "/var/db/",
        "/var/audit/",
        "/var/root/",
    ];

    for &p in &prefix_match {
        if path.starts_with(p) {
            return true;
        }
    }

    false
}

/// 检查路径是否在安全白名单内
///
/// 只有以下路径模式下的文件/目录才允许删除:
fn check_whitelist(canonical: &Path, home: &Path) -> bool {
    let canonical_str = canonical.to_string_lossy();
    let home_str = home.to_string_lossy();

    // 安全路径前缀列表
    let safe_prefixes: Vec<String> = vec![
        // Rust 编译产物
        format!("{}/.cargo/registry/", home_str),
        // Xcode
        format!("{}/Library/Developer/Xcode/DerivedData/", home_str),
        format!("{}/Library/Developer/Xcode/iOS DeviceSupport/", home_str),
        format!("{}/Library/Developer/Xcode/Archives/", home_str),
        // 模拟器缓存（不是运行时镜像）
        "/Library/Developer/CoreSimulator/Caches/".to_string(),
        // Node.js
        format!("{}/.npm/", home_str),
        format!("{}/Library/pnpm/store/", home_str),
        // Go
        format!("{}/go/pkg/mod/", home_str),
        // Homebrew 缓存
        format!("{}/Library/Caches/Homebrew/", home_str),
        // pip 缓存
        format!("{}/Library/Caches/pip/", home_str),
        // Gradle 缓存
        format!("{}/.gradle/caches/", home_str),
        format!("{}/.gradle/daemon/", home_str),
        format!("{}/.gradle/wrapper/dists/", home_str),
        // JetBrains 缓存
        format!("{}/Library/Caches/JetBrains/", home_str),
        // 系统日志
        format!("{}/Library/Logs/", home_str),
        // App 容器内的缓存
        format!("{}/Library/Containers/", home_str),
        format!("{}/Library/Group Containers/", home_str),
        // Application Support（需配合黑名单排除关键子目录）
        format!("{}/Library/Application Support/", home_str),
        // Library/Caches 下的子目录
        format!("{}/Library/Caches/", home_str),
        // Downloads 和 Desktop 下的文件
        format!("{}/Downloads/", home_str),
        format!("{}/Desktop/", home_str),
        // 项目目录下的 target/node_modules
        format!("{}/Downloads/myproject/workspace/", home_str),
        format!("{}/Downloads/myStudy/project/", home_str),
        format!("{}/FrontProject/", home_str),
    ];

    // 检查路径是否以任一安全前缀开头
    for prefix in &safe_prefixes {
        if canonical_str.starts_with(prefix) {
            // 额外检查：对于 Containers 和 Group Containers，
            // 只允许删除 Caches 子目录
            if canonical_str.contains("/Library/Containers/")
                && !canonical_str.contains("/Data/Library/Caches/")
                && !canonical_str.contains("/Documents/xwechat_files/")
            {
                return false;
            }
            if canonical_str.contains("/Library/Group Containers/")
                && !canonical_str.contains("/Library/Caches/")
            {
                return false;
            }

            // 对于 Application Support，排除黑名单目录
            if canonical_str.contains("/Library/Application Support/") {
                let forbidden_app_support = [
                    "MobileSync",
                    "AddressBook",
                    "CallHistoryDB",
                    "CloudDocs",
                    "iCloud",
                    "AppleShare",
                    "syncervices",
                ];
                for forbidden in &forbidden_app_support {
                    if canonical_str.contains(forbidden) {
                        return false;
                    }
                }
            }

            return true;
        }
    }

    false
}

/// 记录删除日志到文件
///
/// 所有删除操作都会记录到 ~/.maclean/delete.log，可回溯审计
pub fn log_deletion(path: &str, category: &str, success: bool, error: Option<&str>) {
    let log_dir = dirs::home_dir()
        .map(|h| h.join(".maclean"))
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let _ = std::fs::create_dir_all(&log_dir);

    let log_file = log_dir.join("delete.log");
    let timestamp = chrono_like_timestamp();

    let status = if success { "OK" } else { "FAIL" };
    let error_part = error.map(|e| format!(" error={}", e)).unwrap_or_default();

    let log_line = format!(
        "[{}] {} [{}] path={}{}",
        timestamp, status, category, path, error_part
    );

    // 追加写入日志文件
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
    {
        let _ = writeln!(file, "{}", log_line);
    }
}

/// 生成简单的时间戳字符串（不依赖 chrono crate）
fn chrono_like_timestamp() -> String {
    let output = std::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M:%S")
        .output();

    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => "unknown".to_string(),
    }
}
