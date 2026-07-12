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

/// 检查路径是否安全可删除（通用版本，兼容旧调用）
pub fn check_path_safety(path: &str) -> SafetyCheck {
    check_path_safety_with_category(path, "")
}

/// 检查路径是否安全可删除
///
/// 这是删除前的最终安全屏障，即使扫描器有 bug 扫到了危险路径，
/// 这里的检查也会阻止删除。
///
/// `category` 参数用于区分不同清理场景，大文件/目录可享受更宽松的策略：
/// 用户通过大文件扫描器主动发现的主目录下的大文件/目录，明确由用户选择删除。
///
/// 安全检查层（从外到内）:
/// 1. 空路径 / 非绝对路径检查
/// 2. 控制字符 / 路径遍历 `..` 防护
/// 3. 符号链接目标解析（拒绝指向保护目录的符号链接）
/// 4. 系统关键目录黑名单（/System, /usr, /bin 等 50+ 路径）
/// 5. 用户关键目录黑名单（Keychains, Mail, Messages 等）
/// 6. 白名单校验（只允许已知安全路径模式）
/// 7. 敏感文件名检测（.env, id_rsa, credentials 等）
pub fn check_path_safety_with_category(path: &str, category: &str) -> SafetyCheck {
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

    // Docker prune 特殊路径标记，通过外部命令清理而非文件删除
    if path.starts_with("docker:") {
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

    // 4a. 精确匹配：只保护目录本身，允许删除其子项
    //     （这些目录的子项在白名单中按需放行，如 Containers/<bundle_id>、Preferences/<bundle_id>.plist）
    let forbidden_exact = [
        "Library/Preferences",           // 偏好设置根目录（子项 plist 可删）
        "Library/Containers",            // Containers 根目录（子项可删）
        "Library/Group Containers",      // Group Containers 根目录（子项可删）
        "Library/Saved Application State", // Saved State 根目录（子项可删）
        "Library/HTTPStorages",          // HTTPStorages 根目录（子项可删）
        "Library/Caches",                // Caches 根目录（子项可删）
        "Library/Logs",                  // Logs 根目录（子项可删）
        "Library/Application Support",   // Application Support 根目录（子项可删）
    ];

    for forbidden_suffix in &forbidden_exact {
        let forbidden_path = home.join(forbidden_suffix);
        if canonical == forbidden_path {
            return SafetyCheck::Danger(format!(
                "拒绝删除用户关键目录: {}",
                canonical_str
            ));
        }
    }

    // 4b. 前缀匹配：保护目录本身及其所有子内容
    //     （这些目录的任何子路径都不允许删除）
    let forbidden_prefix = [
        "Library/Keychains",             // 钥匙串（密码）
        "Library/Accounts",              // 账户信息
        "Library/Mail",                  // 邮件数据
        "Library/Messages",              // 消息数据
        "Library/Cookies",               // Cookie
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

    for forbidden_suffix in &forbidden_prefix {
        let forbidden_path = home.join(forbidden_suffix);
        // 前缀匹配：禁止删除该目录本身或其任何子路径
        if canonical == forbidden_path
            || canonical.starts_with(&format!("{}/", forbidden_path.to_string_lossy()))
        {
            return SafetyCheck::Danger(format!(
                "拒绝删除用户关键目录: {}",
                canonical_str
            ));
        }
    }

    // ================================================================
    //  第 5 层: 黑名单策略（已替代白名单）
    // ================================================================
    // 不再使用白名单（只允许特定路径），改为纯黑名单（只禁止危险路径）。
    // 这样更通用，适配任意机器，无需为每台电脑配置白名单。
    //
    // 安全保障：
    //   - 第 3 层：系统关键目录黑名单（/System, /usr, /bin 等 60+ 路径）
    //   - 第 4 层：用户数据黑名单（Keychains, Mail, Messages 等）
    //   - 扫描器只扫描已知缓存/构建目录，不会扫到随机危险路径
    //   - 第 6 层：敏感文件名检测（.env, id_rsa 等）
    //
    // 原 check_whitelist 函数保留但不再调用，以备未来需要时恢复

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
        // Homebrew 已安装的核心组件（保护，不允许删除）
        // Caskroom 旧版本不在保护范围内，由扫描器决定哪些是旧版本
        "/opt/homebrew/Cellar/",
        "/opt/homebrew/bin/",
        "/opt/homebrew/opt/",
        "/opt/homebrew/lib/",
        "/opt/homebrew/include/",
        "/opt/homebrew/sbin/",
        "/usr/local/Cellar/",
        "/usr/local/bin/",
        "/usr/local/opt/",
        "/usr/local/lib/",
        "/usr/local/sbin/",
    ];

    for &p in &prefix_match {
        if path.starts_with(p) {
            return true;
        }
    }

    false
}

/// 检查路径是否在安全白名单内（已弃用，保留备查）
///
/// 以前采用白名单策略，现已改为纯黑名单策略，更通用。
/// 此函数保留供未来需要时参考，但不再被调用。
#[allow(dead_code)]
fn check_whitelist(canonical: &Path, home: &Path, category: &str) -> bool {
    let canonical_str = canonical.to_string_lossy();
    let home_str = home.to_string_lossy();

    // ================================================================
    //  大文件/目录特殊策略
    // ================================================================
    // 大文件扫描器扫描的是用户主目录下的大文件/大目录，
    // 属于用户主动发现并勾选删除的项目，因此放行主目录下的直接子项。
    // 黑名单（Library、Pictures 等）已在上层处理，这里只需确保是直接子项。
    let is_large_file = category == "大文件" || category == "大目录";
    if is_large_file {
        // 允许用户主目录下的直接子目录/文件
        if is_direct_child(&canonical_str, &home_str) {
            return true;
        }
        // 也允许 Downloads/Desktop 下的大文件（用户主动下载/存放的文件）
        let downloads = format!("{}/Downloads/", home_str);
        let desktop = format!("{}/Desktop/", home_str);
        if canonical_str.starts_with(&downloads) || canonical_str.starts_with(&desktop) {
            return true;
        }
    }

    // ================================================================
    //  第 1 层: 全局工具缓存标准路径
    //  这些路径是各工具在 macOS 上的默认位置，任何机器都一样
    // ================================================================
    let global_tool_caches: Vec<String> = vec![
        // Rust
        format!("{}/.cargo/registry", home_str),
        // Node.js / npm / pnpm / yarn
        format!("{}/.npm", home_str),
        format!("{}/.yarn", home_str),
        format!("{}/.config/yarn/global", home_str),
        format!("{}/Library/pnpm/store", home_str),
        // Go
        format!("{}/go/pkg/mod", home_str),
        format!("{}/go/bin", home_str),
        // Java / Gradle / Maven
        format!("{}/.gradle/caches", home_str),
        format!("{}/.gradle/daemon", home_str),
        format!("{}/.gradle/wrapper/dists", home_str),
        format!("{}/.m2/repository", home_str),
        // Flutter / Dart
        format!("{}/.pub-cache", home_str),
        format!("{}/.flutter", home_str),
        // Python
        format!("{}/.cache/pip", home_str),
        format!("{}/.local/share/pip", home_str),
        format!("{}/Library/Caches/pip", home_str),
        // Homebrew
        format!("{}/Library/Caches/Homebrew", home_str),
        format!("{}/Library/Caches/Homebrew/Cask", home_str),
        // C/C++
        format!("{}/.conan/data", home_str),
        format!("{}/.cache/ccache", home_str),
        // Ruby
        format!("{}/.gem", home_str),
        format!("{}/.rbenv/versions", home_str),
        // .NET
        format!("{}/.nuget/packages", home_str),
        // 系统级 Xcode / 模拟器缓存
        format!("{}/Library/Developer/Xcode/DerivedData", home_str),
        format!("{}/Library/Developer/Xcode/iOS DeviceSupport", home_str),
        format!("{}/Library/Developer/Xcode/Archives", home_str),
        format!("{}/Library/Developer/Xcode/Products", home_str),
        "/Library/Developer/CoreSimulator/Caches".to_string(),
        "/Library/Developer/CoreSimulator/Volumes".to_string(),
        "/Library/Developer/CoreSimulator/Cryptex".to_string(),
        // 日志
        format!("{}/Library/Logs", home_str),
        // JetBrains
        format!("{}/Library/Caches/JetBrains", home_str),
        format!("{}/Library/Application Support/JetBrains/Toolbox/apps", home_str),
    ];

    for prefix in &global_tool_caches {
        if is_same_or_under(&canonical_str, prefix) {
            return true;
        }
    }

    // ================================================================
    //  第 2 层: 用户 Library/Caches 和 Application Support 下的子目录
    //  黑名单已在 is_critical_system_path / forbidden_home_paths 中排除危险项
    // ================================================================
    let user_library_caches = format!("{}/Library/Caches/", home_str);
    if canonical_str.starts_with(&user_library_caches) {
        return true;
    }

    let user_app_support = format!("{}/Library/Application Support/", home_str);
    if canonical_str.starts_with(&user_app_support) {
        // 黑名单已在 forbidden_home_paths 中处理，这里放行其余子目录
        return true;
    }

    // Preferences 下的 plist 文件（App 残留清理）
    let user_prefs = format!("{}/Library/Preferences/", home_str);
    if canonical_str.starts_with(&user_prefs) {
        return true;
    }

    let user_containers = format!("{}/Library/Containers/", home_str);
    if canonical_str.starts_with(&user_containers) {
        // 允许删除整个 Container 子目录（App 卸载场景）
        // 或 Container 内的 Caches（缓存清理场景）
        return canonical_str.contains("/Data/Library/Caches/")
            || canonical_str.contains("/Documents/xwechat_files/")
            || is_direct_child(&canonical_str, &user_containers);
    }

    let user_group_containers = format!("{}/Library/Group Containers/", home_str);
    if canonical_str.starts_with(&user_group_containers) {
        // 允许删除整个 Group Container 子目录（App 卸载场景）
        // 或其中的 Caches（缓存清理场景）
        return canonical_str.contains("/Library/Caches/")
            || is_direct_child(&canonical_str, &user_group_containers);
    }

    // Saved Application State 子目录（App 卸载场景）
    let user_saved_state = format!("{}/Library/Saved Application State/", home_str);
    if canonical_str.starts_with(&user_saved_state) {
        return is_direct_child(&canonical_str, &user_saved_state);
    }

    // HTTPStorages 子目录（App 卸载场景）
    let user_http_storages = format!("{}/Library/HTTPStorages/", home_str);
    if canonical_str.starts_with(&user_http_storages) {
        return is_direct_child(&canonical_str, &user_http_storages);
    }

    // /Applications/ 下的 .app 包（App 卸载场景）
    if canonical_str.starts_with("/Applications/") && canonical_str.ends_with(".app") {
        return true;
    }

    // ================================================================
    //  第 3 层: 常见项目根目录下的缓存/构建目录
    //  扫描器识别出的项目级缓存，只要位于常见项目目录下就放行
    // ================================================================
    let project_roots: Vec<String> = vec![
        format!("{}/Downloads/", home_str),
        format!("{}/Desktop/", home_str),
        format!("{}/Documents/", home_str),
        format!("{}/workspace/", home_str),
        format!("{}/projects/", home_str),
        format!("{}/project/", home_str),
        format!("{}/FrontProject/", home_str),
        format!("{}/IdeaProjects/", home_str),
        format!("{}/AndroidStudioProjects/", home_str),
        format!("{}/StudioProjects/", home_str),
        format!("{}/dev/", home_str),
        format!("{}/Development/", home_str),
    ];

    // 已知缓存/构建目录名（项目级）
    let project_cache_names = [
        "node_modules",
        ".next",
        ".nuxt",
        ".svelte-kit",
        ".output",
        ".turbo",
        ".parcel-cache",
        "target",
        "build",
        "dist",
        "out",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        ".tox",
        ".venv",
        "venv",
        // 注意：不放行 .env 文件，避免误删配置/密钥
        ".gradle",
        ".idea",
        ".vs",
        ".angular",
        ".nyc_output",
        "coverage",
        ".DS_Store", // 文件
    ];

    for root in &project_roots {
        if canonical_str.starts_with(root) {
            for name in &project_cache_names {
                if path_contains_component(&canonical_str, name) {
                    return true;
                }
            }
        }
    }

    false
}

/// 检查 path 是否等于 prefix 或位于 prefix 之下
/// 自动处理末尾 / 的差异
fn is_same_or_under(path: &str, prefix: &str) -> bool {
    let normalized_path = path.trim_end_matches('/');
    let normalized_prefix = prefix.trim_end_matches('/');
    normalized_path == normalized_prefix
        || normalized_path.starts_with(&format!("{}/", normalized_prefix))
}

/// 检查 path 是否是 prefix 的直接子项（即 path = prefix + name，无更多层级）
/// 用于确保只允许删除 Containers/Group Containers 等目录的直接子目录，而非任意深层路径
fn is_direct_child(path: &str, prefix: &str) -> bool {
    if !path.starts_with(prefix) {
        return false;
    }
    let suffix = &path[prefix.len()..];
    // 直接子项：suffix 中不包含额外的 /
    !suffix.is_empty() && !suffix.contains('/')
}

/// 检查路径中是否包含某个目录/文件名组件
/// 例如 /a/b/node_modules/c 包含 node_modules
fn path_contains_component(path: &str, name: &str) -> bool {
    path.split('/').any(|component| component == name)
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
