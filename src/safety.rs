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
pub fn check_path_safety(path: &str) -> SafetyCheck {
    let p = Path::new(path);
    let canonical = match p.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            // 路径不存在或无法解析，可能是 APFS 快照名称等非文件路径
            // 对于非文件路径（如快照名），跳过文件路径安全检查
            if path.starts_with("com.apple.TimeMachine.")
                || path.contains(" | ")
            {
                return SafetyCheck::Safe;
            }
            return SafetyCheck::Danger(format!("路径无法解析: {}", path));
        }
    };

    let canonical_str = canonical.to_string_lossy().to_string();

    // ================================================================
    //  第一层: 绝对禁止路径（系统关键目录）
    // ================================================================
    let forbidden_prefixes = [
        "/",
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/var",
        "/private/var/db",      // 系统数据库
        "/private/var/log",      // 系统日志（不是 ~/Library/Logs）
        "/etc",
        "/dev",
        "/Library/Preferences",  // 系统偏好设置
        "/Library/StartupItems",
        "/Library/LaunchDaemons",
        "/Library/LaunchAgents",
    ];

    for forbidden in &forbidden_prefixes {
        if canonical_str == *forbidden {
            return SafetyCheck::Danger(format!(
                "拒绝删除系统关键目录: {}",
                canonical_str
            ));
        }
    }

    // 根目录本身绝对禁止
    if canonical == Path::new("/") {
        return SafetyCheck::Danger("拒绝删除根目录".to_string());
    }

    // ================================================================
    //  第二层: 用户主目录下的禁止路径
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
        "Library/Group Containers",      // Group Containers 根目录（只允许删其下的 Caches）
        "Library/Containers",            // Containers 根目录（只允许删其下的 Caches）
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
        "Library/Caches/Homebrew/Caskroom", // Homebrew 已安装应用（不是缓存）
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
    //  第三层: 白名单校验 - 只允许删除已知安全的路径模式
    // ================================================================
    let is_in_safe_zone = check_whitelist(&canonical, &home);

    if !is_in_safe_zone {
        return SafetyCheck::Danger(format!(
            "路径不在安全白名单内，拒绝删除: {}",
            canonical_str
        ));
    }

    // ================================================================
    //  第四层: 敏感文件名检测
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
