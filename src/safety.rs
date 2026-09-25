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

// 2026-09-18 删除了 `check_path_safety`（无 category 的兼容版本）：全仓零调用点，
// 留着只会让人以为"不传 category 也行"。所有入口必须显式传分类。

/// 取当前用户的**真实** home 目录（读 passwd 库，不受 `$HOME` 影响）
///
/// `dirs::home_dir()` 在 unix 上优先读 `$HOME` 环境变量。把它指到别处，
/// 第 4 层的保护前缀就会整体偏移，真实的 `~/Library/Mail`、`~/.ssh`
/// 反而不再命中黑名单。这里用 `getpwuid` 取 passwd 库里的 `pw_dir` 兜底。
#[cfg(unix)]
fn real_home_dir() -> Option<PathBuf> {
    use std::ffi::CStr;
    // SAFETY: getpwuid 返回的静态指针只在本次读取期间有效，这里立刻把
    // pw_dir 拷成 String 后再无别名读写，不存在跨线程共享。
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        let dir = CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
        if dir.is_empty() {
            None
        } else {
            Some(PathBuf::from(dir))
        }
    }
}

/// 需要保护的用户主目录候选集（去重）
///
/// 只保护其中一份是不够的：攻击者改 `$HOME` 就能绕过 env 那份，
/// 而恶意缓存文件加载绝对路径时用的可能是真实 home。
/// Windows 侧的真实 profile 路径
///
/// `dirs::home_dir()` 在 Windows 上读 `%USERPROFILE%`，而在当前进程里改这么
/// 一个环境变量，就能让整份 home 黑名单的保护前缀整体偏移（macOS 侧把 item #15
/// 列为 P0 修的正是同一类问题）。这里改从注册表
/// `HKCU\Volatile Environment\USERPROFILE` 取 —— 该值由系统在登录时写入，
/// 进程内改环境变量影响不到它，因此可作为独立的第二重来源。
#[cfg(target_os = "windows")]
fn real_home_dir_windows() -> Option<PathBuf> {
    let out = std::process::Command::new("reg")
        .args(["query", r"HKCU\Volatile Environment", "/v", "USERPROFILE"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    extract_reg_value(&text, "USERPROFILE").map(PathBuf::from)
}

/// 从 `reg query` 输出中取出某个值
///
/// 输出形如：`    USERPROFILE    REG_SZ    C:\Users\John Doe`
///
/// 两个坑：
/// - 路径可能含空格，所以不能按空白切完就去取最后一列
/// - 名字要按整词匹配，否则查 `HOME` 会命中 `HOMEDRIVE`
///
/// 抽成纯函数（不限 cfg）是为了能在开发机上测这段解析：一旦错位，第二重
/// 保护就会静默失效，而且从现象上极难发现。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn extract_reg_value(output: &str, name: &str) -> Option<String> {
    for raw in output.lines() {
        let line = raw.trim();
        // 名字后必须紧跟空白，避免 HOME 匹配到 HOMEDRIVE
        let Some(rest) = line
            .strip_prefix(name)
            .filter(|r| r.chars().next().map(|c| c.is_whitespace()).unwrap_or(false))
        else {
            continue;
        };
        // 第二列是注册表类型：REG_SZ / REG_EXPAND_SZ / REG_MULTI_SZ / REG_DWORD
        let Some((_ty, value)) = rest.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn protected_homes() -> Vec<PathBuf> {
    #[cfg(unix)]
    #[cfg(unix)]
    let real = real_home_dir();
    #[cfg(target_os = "windows")]
    let real = real_home_dir_windows();
    #[cfg(not(any(unix, target_os = "windows")))]
    let real: Option<PathBuf> = None;
    homes_from(dirs::home_dir(), real)
}

/// 合并两份 home 候选（去重、去空）
///
/// 单独抽出来是为了可测：`protected_homes()` 依赖进程环境，写不出
/// "$HOME 被篡改" 这个场景；这里可以直接喂两组不同的值。
fn homes_from(env_home: Option<PathBuf>, real_home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = Vec::new();
    for candidate in [env_home, real_home].into_iter().flatten() {
        if !candidate.as_os_str().is_empty() && !homes.contains(&candidate) {
            homes.push(candidate);
        }
    }
    homes
}

/// 第 4 层：用户主目录下的禁止路径
///
/// `homes` 由调用方注入（生产环境是 `protected_homes()`）。
/// 之所以不在这个函数里自己去取 home：那样就只能测到进程当前的 home，
/// 写不出「$HOME 被指到别处、真实 home 失防」这个场景。
fn check_home_paths(
    canonical: &Path,
    canonical_str: &str,
    homes: &[PathBuf],
) -> Option<SafetyCheck> {
    check_home_paths_for_platform(canonical, canonical_str, homes, cfg!(target_os = "windows"))
}

/// macOS 侧 home 黑名单：精确匹配（只禁目录自身，子项可删）
const MACOS_HOMES_EXACT: &[&str] = &[
    "Library/Preferences",             // 偏好设置根目录（子项 plist 可删）
    "Library/Containers",              // Containers 根目录（子项可删）
    "Library/Group Containers",        // Group Containers 根目录（子项可删）
    "Library/Saved Application State", // Saved State 根目录（子项可删）
    "Library/HTTPStorages",            // HTTPStorages 根目录（子项可删）
    "Library/Caches",                  // Caches 根目录（子项可删）
    "Library/Logs",                    // Logs 根目录（子项可删）
    "Library/Application Support",     // Application Support 根目录（子项可删）
    "Library/Cookies",                 // Cookies 根目录（子项 .binarycookies 可删）
    "Library/WebKit",                  // WebKit 根目录（子项可删）
    "Library/Application Scripts",     // Application Scripts 根目录（子项可删）
    "OneDrive",                        // 云同步盘根（子缓存可删）
    "OneDrive - ",                     // 企业 OneDrive 根
    "Dropbox",                         // 云同步盘根
    "Google Drive",                    // 云同步盘根
    "iCloud Drive",                    // iCloud Drive 根（Mounted 位置）
    "Nextcloud",                       // 自建云同步根
    "OwnCloud",                        // 自建云同步根
    "Box",                             // Box 同步根
];

/// macOS 侧 home 黑名单：前缀匹配（自身与整个子树都禁）
const MACOS_HOMES_PREFIX: &[&str] = &[
    "Library/Keychains",                         // 钥匙串（密码）
    "Library/Accounts",                          // 账户信息
    "Library/Mail",                              // 邮件数据
    "Library/Messages",                          // 消息数据
    "Library/Application Support/MobileSync",    // iOS 备份
    "Library/Application Support/AddressBook",   // 通讯录
    "Library/Application Support/CallHistoryDB", // 通话记录
    "Library/Application Support/CloudDocs",     // iCloud 文档
    "Library/Calendars",                         // 日历
    "Library/Reminders",                         // 提醒事项
    "Library/Notes",                             // 备忘录
    "Library/Safari",                            // Safari 数据
    "Library/Assistants",                        // Siri 数据
    "Library/Passwords",                         // 密码
    "Library/Security",                          // 安全数据
    "Library/Caches/Homebrew/Caskroom",          // Homebrew 已安装应用
    ".ssh",                                      // SSH 密钥
    ".gnupg",                                    // GPG 密钥
    ".config/git",                               // Git 配置
    ".aws",                                      // AWS 凭证与配置
    ".azure",                                    // Azure 凭证
    ".gcloud",                                   // Google Cloud 凭证
    ".kube",                                     // Kubernetes 凭证
    ".netrc",                                    // 网络认证凭据
    ".password-store",                           // pass 密码库
    ".git-credentials",                          // Git 明文凭证
    ".docker",                                   // Docker 配置与登录态
    ".env",                                      // 环境变量（常含密钥）
    "Library/Application Support/FileProvider",  // FileProvider 同步状态
    "Library/Caches/CloudKit",                   // CloudKit 元数据库
    "Library/Caches/com.apple.bird",             // iCloud Drive 同步状态
    "Library/Caches/com.apple.cloudd",           // CloudDocs 守护同步库
    "Library/Caches/com.apple.clouddocs",
    "Library/Caches/com.apple.fileprovider",
];

/// Windows 侧 home 黑名单：精确匹配（只禁目录自身，子项可删）
///
/// 这正是 Windows 上"看起来最危险但其实必须放行"的一类目录：`AppData\Local`
/// 既是系统缓存的根，也装着大量真实清理目标（npm/pip/Chrome 缓存都在其下）。
/// 一刀切禁掉整棵子树会让 Windows 端几乎无事可做，所以与 macOS 的
/// `Library/Caches` 采取同样策略：只禁根、放行子项。
const WINDOWS_HOMES_EXACT: &[&str] = &[
    "AppData",
    "AppData/Local",
    "AppData/LocalLow",
    "AppData/Roaming",
    "AppData/Local/Packages",           // UWP 应用数据容器根目录
    "AppData/Roaming/Firefox/Profiles", // Firefox profile 列表根
    "AppData/Local/Google/Chrome/User Data/Default", // Chrome profile（内含密码/历史）
    "AppData/Local/Microsoft/Edge/User Data/Default", // Edge profile
    "Documents/Outlook Files",          // Outlook 本地数据文件
    "OneDrive",                         // 同步根目录（删了会连带云端）
];

/// Windows 侧 home 黑名单：前缀匹配（自身与整个子树都禁）
///
/// 凭据/证书/邮件数据一类：这些目录下的任何内容都不能由清理工具处理，
/// 误删会导致用户无法登录、邮件丢失或触发企业安全策略告警。
const WINDOWS_HOMES_PREFIX: &[&str] = &[
    "AppData/Local/Microsoft/Credentials", // Windows 凭据管理器
    "AppData/Local/Microsoft/Vault",       // Web 凭据保管库
    "AppData/Local/Microsoft/Protect",     // DPAPI 用户密钥（含加密用主密钥）
    "AppData/Roaming/Microsoft/Credentials",
    "AppData/Roaming/Microsoft/Protect",
    "AppData/Roaming/Microsoft/SystemCertificates", // 用户根证书存储
    "AppData/Local/Microsoft/Outlook",              // OST/PST 邮件数据
    "AppData/Roaming/Microsoft/Outlook",
    ".ssh",             // SSH 密钥
    ".gnupg",           // GPG 密钥
    ".aws",             // AWS 凭证
    ".azure",           // Azure 凭证
    ".gcloud",          // Google Cloud 凭证
    ".kube",            // Kubernetes 凭证
    ".netrc",           // 网络认证凭据
    ".password-store",  // pass 密码库
    ".git-credentials", // Git 明文凭证
    ".docker",          // Docker 配置与登录态
];

/// 按平台取 home 黑名单
///
/// 平台从参数传入而不是在函数里 `cfg!`：两套名单因此都能在开发机（macOS）上
/// 跑测试。否则 Windows 这半张表要等到真机删了用户 Outlook 数据才发现是空的。
fn home_blacklist(windows: bool) -> (&'static [&'static str], &'static [&'static str]) {
    if windows {
        (WINDOWS_HOMES_EXACT, WINDOWS_HOMES_PREFIX)
    } else {
        (MACOS_HOMES_EXACT, MACOS_HOMES_PREFIX)
    }
}

/// 第 4 层（平台无关实现）
///
/// `windows` 参数决定用哪套黑名单，由调用方注入以便测试。
fn check_home_paths_for_platform(
    canonical: &Path,
    canonical_str: &str,
    homes: &[PathBuf],
    windows: bool,
) -> Option<SafetyCheck> {
    // 用户主目录本身禁止删除
    if homes.iter().any(|h| canonical == *h) {
        return Some(SafetyCheck::Danger("拒绝删除用户主目录".to_string()));
    }

    let (forbidden_exact, forbidden_prefix) = home_blacklist(windows);

    // 4a. 精确匹配：只保护目录本身，允许删除其子项
    for home in homes {
        for forbidden_suffix in forbidden_exact {
            let forbidden_path = home.join(forbidden_suffix);
            if canonical == forbidden_path {
                return Some(SafetyCheck::Danger(format!(
                    "拒绝删除用户关键目录: {}",
                    canonical_str
                )));
            }
        }
    }

    // 4b. 前缀匹配：保护目录本身及其所有子内容
    //
    // 改用 `Path::starts_with`（按路径组件比较）。原实现拼字符串
    // `starts_with(format!("{}/", forbidden_path))` 硬编码了 `/`，
    // Windows 的 `\` 路径永远匹配不上 —— 这一层在那边一直是虚设。
    for home in homes {
        for forbidden_suffix in forbidden_prefix {
            let forbidden_path = home.join(forbidden_suffix);
            if canonical == forbidden_path || canonical.starts_with(&forbidden_path) {
                return Some(SafetyCheck::Danger(format!(
                    "拒绝删除用户关键目录: {}",
                    canonical_str
                )));
            }
        }
    }

    None
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
    // 当前策略对所有分类一视同仁（category 预留给后续按场景细化，如"大文件"放宽白名单）。
    // 先显式消费掉，避免误删参数后调用方悄悄失配。
    let _ = category;
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
    if path
        .chars()
        .any(|c| c.is_control() || c == '\n' || c == '\r' || c == '\0')
    {
        return SafetyCheck::Danger("路径包含控制字符".to_string());
    }

    // 路径遍历防护：拒绝 `..` 作为完整路径组件
    // 但允许文件名中包含 `..`（如 Firefox 的 name..files）
    //
    // 必须**同时**按 `/` 与 `\` 切分：Windows 路径用反斜杠，只按 `/` 切时
    // `C:\Users\x\..\..\Windows` 会整体落成一个组件，`..` 检测被完全绕过。
    // canonicalize 失败时会回退到原始字符串继续检查，这一层不能失守。
    let has_traversal = path.split(['/', '\\']).any(|c| c == "..");
    if has_traversal {
        return SafetyCheck::Danger("路径包含目录遍历 (..)".to_string());
    }

    // 非文件路径（APFS 快照等），跳过文件路径检查
    //
    // 这里必须用 `&&` 同时约束前缀与分隔符。原实现是
    // `starts_with("com.apple.TimeMachine.") || contains(" | ")`，
    // `||` 让第二个条件独立生效 —— 任何含「空格|空格」的路径
    // （如 `/System/Library | x`、`/Users/u/Library/Mail | x`）
    // 会在下面全部 7 层黑名单之前被无条件放行（历史 bug）。
    if path.starts_with("com.apple.TimeMachine.") && path.contains(" | ") {
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
            // canonicalize 失败不一定是路径不存在，可能是 TCC 保护导致 realpath() 无权限。
            // 回退到原始路径（规范化斜杠但不解析符号链接），继续做安全检查。
            // 如果路径确实不存在，删除时自然会失败，不需要在安全检查阶段拒绝。
            PathBuf::from(path)
        }
    };

    let canonical_str = canonical.to_string_lossy().to_string();

    // ================================================================
    //  第 3 层: 系统关键目录黑名单（参考 Mole 的保护列表）
    // ================================================================
    if is_critical_system_path(&canonical_str) {
        return SafetyCheck::Danger(format!("拒绝删除系统关键目录: {}", canonical_str));
    }

    // 根目录本身绝对禁止
    if canonical == Path::new("/") {
        return SafetyCheck::Danger("拒绝删除根目录".to_string());
    }

    // ================================================================
    //  第 4 层: 用户主目录下的禁止路径
    // ================================================================
    // 必须同时保护 `$HOME` 与真实 home：`dirs::home_dir()` 在 unix 上读
    // `$HOME` 环境变量，把它指到别处就能让整个第 4 层的前缀整体偏移，
    // 真实的 ~/Library/Mail、~/.ssh 反而全部失防。

    let homes = protected_homes();
    if let Some(danger) = check_home_paths(&canonical, &canonical_str, &homes) {
        return danger;
    }

    // ================================================================
    //  第 4.4 层: 仓库元数据组件保护（任意深度）
    // ================================================================
    // 对标 MangoDisk PROTECTED_REPOSITORY_COMPONENTS：.git/.hg/.svn/.bzr 是仓库
    // 状态而非构建产物，在任何深度出现都拒绝删除 —— 误删 .git 会毁掉整个版本历史。
    // 缓存清理目标（DerivedData/Caches/node_modules 等）从不落在这些组件上。
    {
        const REPO_COMPONENTS: [&str; 4] = [".git", ".hg", ".svn", ".bzr"];
        let has_repo_component = canonical.components().any(|comp| {
            matches!(comp, std::path::Component::Normal(n) if {
                let name = n.to_string_lossy();
                REPO_COMPONENTS.contains(&name.as_ref())
            })
        });
        if has_repo_component {
            return SafetyCheck::Danger(format!(
                "仓库元数据目录（.git/.hg/.svn/.bzr），拒绝删除: {}",
                canonical_str
            ));
        }
    }

    // ================================================================
    //  第 4.5 层: EDR / Endpoint Security 代理保护
    // ================================================================
    // 检测 CrowdStrike、SentinelOne、ESET、Jamf 等企业安全代理的缓存路径。
    // 误删这些路径会触发安全代理的 tamper 检测（MITRE T1562.001），
    // 可能导致设备被隔离或告警。这些路径通常位于 /private/var/folders/ 或
    // /var/folders/ 下，以厂商前缀命名（如 com.crowdstrike.、com.sentinelone.）。
    if is_endpoint_security_cache_path(&canonical_str) {
        return SafetyCheck::Danger(format!(
            "EDR 安全代理缓存路径，删除会触发篡改告警: {}",
            canonical_str
        ));
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
    let file_name = canonical.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // 已卸载 App 的偏好设置 plist（~/Library/Preferences/<name>.plist）可安全删除，
    // 即使文件名包含 credential/password 等关键字（如 git-credential-manager.plist）。
    let is_leftover_pref = file_name.ends_with(".plist")
        && homes.iter().any(|h| {
            canonical_str.starts_with(&format!("{}/Library/Preferences/", h.to_string_lossy()))
        });

    if !is_leftover_pref {
        if let Some(sensitive) = sensitive_name_hit(&canonical_str, file_name) {
            return SafetyCheck::Warning(format!(
                "路径包含敏感文件名 ({}): {}",
                sensitive, canonical_str
            ));
        }
    }

    // ================================================================
    //  第 7 层: 运行中应用保护
    // ================================================================
    // 如果要删除的是 /Applications 下的 .app 包，检查该应用是否正在运行。
    // 运行中的应用不应被删除（可能导致系统不稳定）。
    if canonical_str.ends_with(".app")
        && (canonical_str.starts_with("/Applications/")
            || homes.iter().any(|h| {
                canonical_str.starts_with(&format!("{}/Applications/", h.to_string_lossy()))
            }))
    {
        if let Some(app_name) = canonical
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.trim_end_matches(".app"))
        {
            if is_app_running(app_name) {
                return SafetyCheck::Warning(format!(
                    "应用 {} 正在运行，请先退出后再删除",
                    app_name
                ));
            }
        }
    }

    SafetyCheck::Safe
}

/// 第 6 层：敏感文件名检测（纯函数）
///
/// 抽出来是为了能直接喂 Windows 形态的路径做测试 —— 详见下面
/// `backslashed_cargo_credentials_is_detected` 那条用例。
fn sensitive_name_hit(canonical_str: &str, file_name: &str) -> Option<&'static str> {
    const SENSITIVE_NAMES: &[&str] = &[
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

    // 分隔符归一：名单里 ".cargo/credentials" 是 POSIX 写法，
    // 直接用原串 contains 在 Windows 的 `\` 路径上永远匹配不上。
    let normalized = canonical_str.replace('\\', "/");

    SENSITIVE_NAMES
        .iter()
        .find(|&sensitive| {
            file_name.eq_ignore_ascii_case(sensitive) || normalized.contains(sensitive)
        })
        .map(|v| v as _)
}

/// 检查模拟器相关服务是否正在运行
///
/// 包括用户主动打开的前台应用（Xcode、Simulator.app）以及常驻后台的
/// 模拟器核心服务（CoreSimulatorService、simdiskimaged）。这些服务运行
/// 时会锁定 /Library/Developer/CoreSimulator/Volumes 下的 runtime 镜像，
/// 导致 xcrun simctl runtime delete 无法删除。此时不应展示或删除这些项。
pub fn is_simulator_running() -> bool {
    #[cfg(target_os = "macos")]
    {
        let processes = [
            "Xcode",
            "Simulator",
            "CoreSimulatorService",
            "simdiskimaged",
        ];
        for proc in &processes {
            if let Ok(output) = std::process::Command::new("/usr/bin/pgrep")
                .arg("-x")
                .arg(proc)
                .output()
            {
                if output.status.success() && !output.stdout.is_empty() {
                    return true;
                }
            }
        }
        // 额外检查 com.apple.CoreSimulator
        if let Ok(output) = std::process::Command::new("/usr/bin/pgrep")
            .arg("-f")
            .arg("com.apple.CoreSimulator")
            .output()
        {
            if output.status.success() && !output.stdout.is_empty() {
                return true;
            }
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows/Linux 无 iOS 模拟器
        false
    }
}

/// 检查指定应用是否正在运行
///
/// macOS: 通过 `pgrep -f` 搜索进程列表
/// Windows: 通过 `tasklist` 搜索进程列表
fn is_app_running(app_name: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/bin/pgrep")
            .args(["-f", app_name])
            .output();
        if let Ok(out) = output {
            return out.status.success() && !out.stdout.is_empty();
        }
        false
    }
    #[cfg(target_os = "windows")]
    {
        // Windows: tasklist + findstr
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {}", app_name), "/NH"])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            return !stdout.contains("INFO: No tasks") && !stdout.trim().is_empty();
        }
        false
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = app_name;
        false
    }
}

/// 检查 MacBook 是否处于合盖状态（clamshell mode）
///
/// macOS: 通过 `ioreg` 读取 AppleClamshellState 属性判断屏幕开合状态。
/// 合盖时 Touch ID 传感器不可用（电源按钮 Touch ID 在合盖时无法触达），
/// 需要回退到密码输入流程。
///
/// Windows/Linux: 无合盖概念，始终返回 false。
pub fn is_clamshell_closed() -> bool {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("ioreg")
            .arg("-r")
            .arg("-k")
            .arg("AppleClamshellState")
            .arg("-d")
            .arg("4")
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("AppleClamshellState") && line.contains("Yes") {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows/Linux 无合盖概念
        false
    }
}

/// EDR / Endpoint Security 代理的 bundle ID 前缀
///
/// 这些是企业安全代理的标识前缀，其缓存文件通常位于
/// /private/var/folders/ 或 /var/folders/ 下。
/// 误删会触发 tamper 检测（MITRE T1562.001）。
const EDR_BUNDLE_PREFIXES: &[&str] = &[
    "com.crowdstrike.",
    "com.sentinelone.",
    "com.sentinel-labs.",
    "com.eset.",
    "com.jamf.",
    "com.jamfsoftware.",
    "com.paloaltonetworks.",
    "com.cisco.anyconnect",
    "com.cisco.secureclient",
];

/// 检测路径是否为 EDR / Endpoint Security 代理的缓存路径
///
/// EDR 代理的临时缓存通常位于 macOS 的临时目录下：
/// - `/private/var/folders/<XX>/<YYYY...>/T/` (用户级临时目录)
/// - `/var/folders/<XX>/<YYYY...>/T/` (同上，符号链接)
/// - `/private/var/folders/<XX>/<YYYY...>/C/` (用户级缓存目录)
///
/// 这些目录下以 `com.crowdstrike.`、`com.sentinelone.` 等前缀命名的
/// 子目录是安全代理的运行时缓存，删除后会触发 tamper 检测。
///
/// 注意：此检查不依赖 HOME 环境变量，防止 `env -u HOME` 绕过。
pub fn is_endpoint_security_cache_path(path: &str) -> bool {
    // 只检查 /private/var/folders/ 和 /var/folders/ 下的路径
    let is_var_folders =
        path.starts_with("/private/var/folders/") || path.starts_with("/var/folders/");

    if !is_var_folders {
        return false;
    }

    // 检查路径中是否包含 EDR 厂商前缀
    for prefix in EDR_BUNDLE_PREFIXES {
        if path.contains(prefix) {
            return true;
        }
    }

    false
}

/// Windows 系统关键路径 / 网络位置判定
///
/// 与 macOS 那张 POSIX 黑名单分开维护：Windows 的形态完全不同 —— 盘符、
/// 反斜杠、大小写不敏感，还有 `\\?\` 设备前缀和 `\\server\share` UNC。
/// 硬塞进同一张表只会让两边都看不清。
///
/// 这里**故意不加** `#[cfg(target_os = "windows")]`：macOS/Linux 的路径以 `/`
/// 开头，形态上命中不了下面任何一条规则，运行时成本可以忽略；换来的是这张
/// 表能在开发机（macOS）上跑回归测试。这条防线只在真出事那一刻起作用，
/// 在那之前必须有测试证明它拦得住。
fn is_windows_critical_path(path: &str) -> bool {
    // 分隔符归一：用户输入里可能混用 / 与 \
    let normalized = path.replace('/', "\\");
    let lower = normalized.to_lowercase();
    // `\\?\` 是本地设备路径前缀，剥掉后语义不变（不剥会让盘符判断失配）
    let lower = lower.strip_prefix(r"\\?\").unwrap_or(&lower);

    // UNC（`\\server\share`）：大概率落在网络位置上。删远端共享既不可控、
    // 也不该是清理工具的职责，一律拒绝。
    if lower.starts_with(r"\\") {
        return true;
    }

    // 非盘符路径（POSIX 形态 / 相对路径等）不适用这张表，交其它层判断
    let bytes = lower.as_bytes();
    if lower.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return false;
    }

    // 盘符之后的部分，形如 `\Windows\System32`
    let rest = &lower[2..];

    // 盘符根本身：`C:` / `C:\` / `C:\\`
    if rest.trim_matches('\\').is_empty() {
        return true;
    }

    // 精确匹配：目录自身危险，其内内容另有规则
    //
    // 刻意**不**把 `\Users` 整棵列为前缀：用户数据全在 Users\<name> 之下，
    // 一刀切会让清理功能彻底失效。macOS 侧同理 —— 只禁 `/Users` 自身，
    // 不禁子树；个人敏感目录由第 4 层的 home 黑名单负责。
    const WIN_EXACT: &[&str] = &[
        "\\windows",
        "\\windows.old",
        "\\program files",
        "\\program files (x86)",
        "\\programdata",
        "\\users",
        "\\users\\public",
        "\\users\\default",
        "\\users\\default user",
        "\\users\\all users",
        "\\recovery",
        "\\system volume information",
        "\\$recycle.bin",
        "\\efi",
        "\\boot",
        "\\perflogs",
        "\\documents and settings",
        "\\config.msi",
    ];

    for &e in WIN_EXACT {
        if rest.trim_end_matches('\\') == e {
            return true;
        }
    }

    // 前缀匹配：这些目录自身及其全部子树都不允许删除
    const WIN_PREFIX: &[&str] = &[
        "\\windows\\",
        "\\windows.old\\",
        "\\program files\\",
        "\\program files (x86)\\",
        "\\programdata\\",
        "\\recovery\\",
        "\\system volume information\\",
        "\\$recycle.bin\\",
        "\\efi\\",
        "\\boot\\",
        "\\perflogs\\",
        "\\documents and settings\\",
        "\\config.msi\\",
        "\\users\\public\\",
        "\\users\\default\\",
    ];

    WIN_PREFIX.iter().any(|p| rest.starts_with(p))
}

/// 检查路径是否为系统关键保护路径
/// 参考 Mole 的 _mole_is_critical_deletion_path，包含 50+ 保护路径
///
/// 跨平台：POSIX 黑名单（macOS/Linux）之外，先过一遍 Windows 黑名单。
pub(crate) fn is_critical_system_path(path: &str) -> bool {
    // Windows 形态先交给 Windows 那张表（POSIX 路径命中不了它，故此处不限 cfg）
    if is_windows_critical_path(path) {
        return true;
    }

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
        // CoreSimulator 全局 runtime 镜像受 SIP 保护：即使管理员权限也无法删除，
        // 扫描期直接标记为不可删（第 4 层系统黑名单），避免删除失败后再弹窗引导。
        "/Library/Developer/CoreSimulator/",
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
        format!(
            "{}/Library/Application Support/JetBrains/Toolbox/apps",
            home_str
        ),
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
        // 或 Container 内的 Caches/Documents（缓存/数据清理场景）
        return canonical_str.contains("/Data/Library/Caches/")
            || canonical_str.contains("/Data/Documents/")
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

// ================================================================
//  P0：规则根模板校验 + 受保护根判定（为声明式规则引擎前置）
// ================================================================
//
// 设计目标（对齐 MangoDisk 的 root_validation，但叠加 maclean 已有优势）：
// - 规则里声明的 roots 支持模板变量（$HOME 等），解析失败/为空必须 fail-closed，
//   否则 "$HOME/Library/Caches" 在 HOME 为空时会退化成 "/Library/Caches" 造成越权删除
// - 解析后的根必须绝对、不能落在受保护根本身（/、home、系统目录、卷根）
// - 组件数上限：防止"根模板过浅"（如规则根恰好等于 home）通过校验
// - 本模块是 P2 声明式规则的强制门禁，也是删除前二次校验的最后一道兜底

/// 模板变量解析：把规则根里的 `$HOME` / `%USERPROFILE%` 替换为真实值
///
/// 返回 None 表示变量缺失或为空 —— 调用方必须拒绝该规则（fail-closed）。
/// 环境变量可能被进程内篡改，因此优先用 passwd/注册表真实 home 兜底。
fn resolve_root_variable(root: &str) -> Option<String> {
    let homes = protected_homes();
    let home = homes.first()?;
    let home_str = home.to_string_lossy();
    let mut out = root.to_string();
    for (var, val) in [
        ("$HOME", home_str.as_ref()),
        ("%USERPROFILE%", home_str.as_ref()),
    ] {
        if out.contains(var) {
            out = out.replace(var, val);
        }
    }
    // 还残留模板变量说明遇到了不认识的变量（如 $APPDATA），拒绝
    if out.contains('$') || out.contains('%') {
        return None;
    }
    Some(out)
}

/// 受保护根：永远不允许作为删除根目标的本体路径
///
/// 与黑名单（保护目录本身/子项可删）不同，这里保护的是**根本身**：
/// 待删路径等于这些路径时必须拒绝 —— 规则根解析若退化到这里，直接 fail-closed。
#[cfg(target_os = "macos")]
fn macos_protected_roots() -> Vec<&'static str> {
    vec![
        "/",
        "/Users",
        "/System",
        "/Library",
        "/usr",
        "/bin",
        "/sbin",
        "/private",
        "/etc",
        "/var",
        "/Applications",
        "/Volumes",
        "/cores",
        "/opt",
        "/dev",
        "/tmp",
    ]
}

#[cfg(target_os = "windows")]
fn windows_protected_roots() -> Vec<String> {
    let mut roots: Vec<String> = vec![
        "C:\\".into(),
        "C:\\Windows".into(),
        "C:\\Program Files".into(),
        "C:\\Program Files (x86)".into(),
        "C:\\Users".into(),
        "C:\\ProgramData".into(),
        "C:\\Recovery".into(),
        "C:\\System Volume Information".into(),
    ];
    // 盘符根一律保护：D:\、E:\ ... 防止规则解析到任一盘的根
    for drive in b'C'..=b'Z' {
        let letter = (drive as char).to_ascii_uppercase();
        roots.push(format!("{}:\\", letter));
    }
    roots
}

/// 判断 path（已解析、规范化后）是否为受保护根本身
///
/// 供两个场景使用：
/// 1. 规则根校验（validate_rule_root）：解析后的根不能是这里任一值
/// 2. 删除前二次校验（ops::sanitize_before_delete）：待删路径不能是这里任一值
#[cfg(target_os = "macos")]
pub fn is_protected_root(path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    let canonical = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string());
    // home 根本身
    if protected_homes().iter().any(|h| {
        h.as_os_str() == std::ffi::OsStr::new(&canonical)
            || h.as_os_str() == std::ffi::OsStr::new(path)
    }) {
        return true;
    }
    macos_protected_roots()
        .iter()
        .any(|r| canonical == *r || path == *r)
}

#[cfg(target_os = "windows")]
pub fn is_protected_root(path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    let normalized = path.replace('/', "\\").trim_end_matches('\\').to_string();
    if protected_homes()
        .iter()
        .any(|h| h.to_string_lossy().as_ref() == normalized)
    {
        return true;
    }
    windows_protected_roots()
        .iter()
        .any(|r| normalized == r.trim_end_matches('\\'))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn is_protected_root(path: &str) -> bool {
    path.is_empty() || path == "/"
}

/// 规则根组件数上限：防止"根模板过浅"通过校验
#[cfg_attr(not(test), allow(dead_code))] // P2 声明式规则将消费
pub const MAX_RULE_ROOT_COMPONENTS: usize = 3;

/// 校验声明式规则的根模板，返回解析后的安全绝对路径
///
/// fail-closed：任何一步失败都返回 Err，调用方必须拒绝该规则。
/// - 模板变量缺失/未知 → Err
/// - 解析后非绝对路径 → Err
/// - 解析后是受保护根本身 → Err
/// - 解析后组件数 ≤ 3（如 $HOME 本身、/ 等浅根直接拒绝）→ Err
#[cfg(target_os = "macos")]
#[cfg_attr(not(test), allow(dead_code))] // P2 声明式规则将消费
pub fn validate_rule_root(root: &str) -> Result<String, String> {
    let resolved = resolve_root_variable(root)
        .ok_or_else(|| format!("规则根含未解析模板变量或为空: {root}"))?;
    let p = std::path::Path::new(&resolved);
    if !p.is_absolute() {
        return Err(format!("规则根必须为绝对路径: {root}"));
    }
    if resolved.contains("..") || resolved.chars().any(|c| c.is_control()) {
        return Err(format!("规则根含目录遍历或控制字符: {root}"));
    }
    if is_protected_root(&resolved) {
        return Err(format!("规则根是受保护根本身: {root}"));
    }
    let components = resolved.split('/').filter(|c| !c.is_empty()).count();
    if components < MAX_RULE_ROOT_COMPONENTS {
        return Err(format!(
            "规则根过浅（{components} 个组件 < {MAX_RULE_ROOT_COMPONENTS}），拒绝: {root}"
        ));
    }
    Ok(resolved)
}

#[cfg(target_os = "windows")]
pub fn validate_rule_root(root: &str) -> Result<String, String> {
    let resolved = resolve_root_variable(root)
        .ok_or_else(|| format!("规则根含未解析模板变量或为空: {root}"))?;
    let p = std::path::Path::new(&resolved);
    if !p.is_absolute() {
        return Err(format!("规则根必须为绝对路径: {root}"));
    }
    if resolved.contains("..") || resolved.chars().any(|c| c.is_control()) {
        return Err(format!("规则根含目录遍历或控制字符: {root}"));
    }
    if is_protected_root(&resolved) {
        return Err(format!("规则根是受保护根本身: {root}"));
    }
    // Windows：至少 3 个组件，如 C:\Users\Name 起步
    let normalized = resolved.replace('/', "\\");
    let components = normalized.split('\\').filter(|c| !c.is_empty()).count();
    if components < MAX_RULE_ROOT_COMPONENTS {
        return Err(format!(
            "规则根过浅（{components} 个组件 < {MAX_RULE_ROOT_COMPONENTS}），拒绝: {root}"
        ));
    }
    Ok(resolved)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn validate_rule_root(root: &str) -> Result<String, String> {
    if root.is_empty() || root == "/" {
        return Err("规则根无效".to_string());
    }
    Ok(root.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- P0: 规则根模板校验 + 受保护根判定 ----------

    #[test]
    fn rule_root_validation_accepts_normal_cache_roots() {
        // $HOME/Library/Caches 这类正常规则根必须通过
        let resolved = validate_rule_root("$HOME/Library/Caches").expect("正常规则根应通过校验");
        assert!(
            resolved.starts_with('/'),
            "解析后的根应为绝对路径: {resolved}"
        );
        assert!(resolved.ends_with("Library/Caches"));
    }

    #[test]
    fn rule_root_validation_rejects_protected_roots() {
        // 受保护根本身 → 拒绝
        for bad in ["/", "/Library", "/System", "/Users", "/usr"] {
            assert!(validate_rule_root(bad).is_err(), "受保护根应被拒绝: {bad}");
        }
    }

    #[test]
    fn rule_root_validation_rejects_home_itself_and_shallow_roots() {
        // $HOME 本身（组件过浅）→ 拒绝
        assert!(validate_rule_root("$HOME").is_err(), "$HOME 本身不应通过");
        // 未知模板变量 → fail-closed
        assert!(
            validate_rule_root("$APPDATA/Foo").is_err(),
            "未知变量应被拒绝"
        );
        // 相对路径 → 拒绝
        assert!(
            validate_rule_root("Library/Caches").is_err(),
            "相对路径应被拒绝"
        );
    }

    #[test]
    fn protected_root_detection_blocks_system_and_home_roots() {
        let homes = protected_homes();
        let home = homes.first().expect("有 home");
        let home_str = home.to_string_lossy().to_string();
        for bad in ["/", "/Library", "/System", "/Users", "/usr", &home_str] {
            assert!(is_protected_root(bad), "应识别为受保护根: {bad}");
        }
        // 普通可删路径不是受保护根
        assert!(!is_protected_root("/Users/me/Library/Caches/foo"));
        assert!(!is_protected_root(&format!(
            "{}/Library/Caches/foo",
            home_str
        )));
    }

    #[test]
    fn rule_root_validation_rejects_traversal_and_control_chars() {
        assert!(
            validate_rule_root("$HOME/Library/../..").is_err(),
            "目录遍历应被拒绝"
        );
        assert!(
            validate_rule_root("$HOME/Library/Caches\n").is_err(),
            "控制字符应被拒绝"
        );
    }

    // ---------- W-5: Windows 用户关键目录保护 ----------
    //
    // 这里刻意用**正斜杠**书写 Windows 路径（C:/Users/Bob/...）。
    // 开发机是 macOS，反斜杠不算路径分隔符，整条路径会被当成一个组件，
    // `Path::starts_with` 自然失效。改用正斜杠后组件语义成立，测到的仍是
    // 同一套 join + starts_with 逻辑；真跑在 Windows 上时两种斜杠等价。

    #[test]
    fn windows_home_credentials_and_mail_are_blocked() {
        let homes = vec![PathBuf::from("C:/Users/Bob")];
        for p in [
            "C:/Users/Bob/AppData/Local/Microsoft/Credentials",
            "C:/Users/Bob/AppData/Local/Microsoft/Credentials/deadbeef",
            "C:/Users/Bob/AppData/Roaming/Microsoft/Protect/S-1-5-21-1",
            "C:/Users/Bob/AppData/Local/Microsoft/Vault/4BF4C442",
            "C:/Users/Bob/AppData/Local/Microsoft/Outlook/mail.ost",
            "C:/Users/Bob/AppData/Roaming/Microsoft/SystemCertificates/My/x",
            "C:/Users/Bob/.ssh/id_ed25519",
            "C:/Users/Bob/.gnupg/private-keys-v1.d/key",
        ] {
            let verdict = check_home_paths_for_platform(Path::new(p), p, &homes, true);
            assert!(verdict.is_some(), "Windows 用户敏感目录未被拦截: {}", p);
        }
    }

    #[test]
    fn windows_home_roots_blocked_but_cache_targets_allowed() {
        let homes = vec![PathBuf::from("C:/Users/Bob")];

        // 应用数据根目录自身禁止删除
        for p in [
            "C:/Users/Bob/AppData",
            "C:/Users/Bob/AppData/Local",
            "C:/Users/Bob/AppData/LocalLow",
            "C:/Users/Bob/AppData/Roaming",
            "C:/Users/Bob/AppData/Local/Packages",
            "C:/Users/Bob/OneDrive",
            "C:/Users/Bob/AppData/Local/Google/Chrome/User Data/Default",
        ] {
            assert!(
                check_home_paths_for_platform(Path::new(p), p, &homes, true).is_some(),
                "应用数据根目录本应被拦截: {}",
                p
            );
        }

        // 但根以下的真实清理目标必须放行 —— 否则 Windows 端几乎无事可做。
        // 这条用例同时兜住"加了黑名单会不会把功能一起废掉"这件事。
        for p in [
            "C:/Users/Bob/AppData/Local/npm-cache",
            "C:/Users/Bob/AppData/Local/pip/Cache",
            "C:/Users/Bob/AppData/Local/Google/Chrome/User Data/Default/Cache",
            "C:/Users/Bob/AppData/Local/Temp/junk",
            "C:/Users/Bob/AppData/Roaming/npm-cache",
        ] {
            assert!(
                check_home_paths_for_platform(Path::new(p), p, &homes, true).is_none(),
                "正常缓存清理目标被误拦: {}",
                p
            );
        }
    }

    #[test]
    fn windows_home_blacklist_covers_every_injected_home() {
        // 与 macOS 侧同类修复一致：不能只护第一个候选 home
        let homes = vec![
            PathBuf::from("C:/Users/Fake"),
            PathBuf::from("C:/Users/Bob"),
        ];
        let p = "C:/Users/Bob/AppData/Local/Microsoft/Credentials";
        assert!(
            check_home_paths_for_platform(Path::new(p), p, &homes, true).is_some(),
            "第二个 home（真实 profile）未被保护: {}",
            p
        );
    }

    #[test]
    fn windows_home_of_the_user_itself_is_blocked() {
        let homes = vec![PathBuf::from("C:/Users/Bob")];
        assert!(
            check_home_paths_for_platform(Path::new("C:/Users/Bob"), "C:/Users/Bob", &homes, true)
                .is_some(),
            "用户主目录自身必须禁止删除"
        );
    }

    #[test]
    fn windows_lists_never_leak_into_macos_judgement() {
        // 同一条 Windows 路径，按 macOS 规则判断时不应命中 macOS 那份表
        let homes = vec![PathBuf::from("C:/Users/Bob")];
        let p = "C:/Users/Bob/AppData/Local/Microsoft/Credentials";
        assert!(
            check_home_paths_for_platform(Path::new(p), p, &homes, false).is_none(),
            "平台选择没有生效：macOS 模式下命中了 Windows 名单"
        );
    }

    // ---------- W-5: 注册表解析（Windows 第二重 home 来源） ----------

    #[test]
    fn reg_value_parsing_handles_spaces_and_lookalike_names() {
        let out = "HKEY_CURRENT_USER\\Volatile Environment\n    USERPROFILE    REG_SZ    C:\\Users\\John Doe\nEnd of search: 1 match(es) found.\n";
        // 路径含空格，取"最后一列"的那种写法会只拿到 "Doe"
        assert_eq!(
            extract_reg_value(out, "USERPROFILE"),
            Some(r"C:\Users\John Doe".to_string())
        );

        // HOMEDRIVE 不能因为前缀相同被误当成 HOME
        let out2 = "    HOMEDRIVE    REG_SZ    C:\n    HOME    REG_SZ    D:\\home\n";
        assert_eq!(
            extract_reg_value(out2, "HOME"),
            Some(r"D:\home".to_string())
        );

        // 查不到就返回 None，不许随手抓一行充数
        assert_eq!(extract_reg_value(out2, "NOSUCHKEY"), None);
        assert_eq!(extract_reg_value("", "USERPROFILE"), None);
    }

    // ---------- W-10: 敏感文件名检测支持 Windows 分隔符 ----------

    #[test]
    fn backslashed_cargo_credentials_is_detected() {
        // 名单里 ".cargo/credentials" 是 POSIX 写法。不归一化分隔符时，
        // 这条 Windows 路径会漏到更后面的裸词 "credential" 才命中 ——
        // 所以断言命中项**必须**是 ".cargo/credentials"，这条用例才有效。
        let hit = sensitive_name_hit(r"C:\Users\Bob\.cargo\credentials", "credentials");
        assert_eq!(hit, Some(".cargo/credentials"));
    }

    #[test]
    fn posix_cargo_credentials_unchanged() {
        let hit = sensitive_name_hit("/Users/Bob/.cargo/credentials", "credentials");
        assert_eq!(hit, Some(".cargo/credentials"));
    }

    #[test]
    fn sensitive_name_hit_returns_none_for_plain_paths() {
        assert_eq!(
            sensitive_name_hit("/Users/Bob/project/src/main.rs", "main.rs"),
            None
        );
        assert_eq!(
            sensitive_name_hit(r"C:\Users\Bob\project\main.rs", "main.rs"),
            None
        );
    }

    #[test]
    fn sensitive_filename_match_is_case_insensitive() {
        // 只针对 file_name 的完整相等比较，文件系统可能大小写不敏感
        assert_eq!(sensitive_name_hit("/tmp/ID_RSA", "ID_RSA"), Some("id_rsa"));
        assert_eq!(sensitive_name_hit("/tmp/id_rsa", "id_rsa"), Some("id_rsa"));
    }

    // ---------- W-3: Windows 系统关键路径黑名单 ----------
    //
    // 这组用例在开发机（macOS）上跑。`is_windows_critical_path` 故意不加
    // `cfg(target_os = "windows")`，就是为了在这里能被验证 —— 否则这条防线
    // 要等到真在 Windows 上删了系统目录才发现它是空的。

    #[test]
    fn windows_system_dirs_are_blocked() {
        for p in [
            r"C:\Windows",
            r"C:\Windows\System32",
            r"C:\windows\system32\drivers\etc",
            r"C:\Windows.old\Users\Bob",
            r"D:\Windows", // 任何盘符上的 Windows 都要拦
            r"C:\Program Files",
            r"C:\Program Files\Some App\bin\app.exe",
            r"C:\Program Files (x86)\Vendor",
            r"C:\ProgramData\Microsoft",
            r"C:\ProgramData",
            r"C:\Recovery",
            r"C:\Recovery\OEM",
            r"C:\System Volume Information\tracking.log",
            r"C:\$Recycle.Bin\S-1-5-21-xxx",
            r"C:\EFI\Boot",
            r"C:\Boot\BCD",
            r"C:\PerfLogs\Admin",
            r"C:\Documents and Settings\Bob",
            r"C:\config.msi\cache",
        ] {
            assert!(
                is_windows_critical_path(p),
                "Windows 系统路径本应被拦截，实际放行了: {}",
                p
            );
        }
    }

    #[test]
    fn windows_drive_roots_are_blocked() {
        for p in [r"C:", r"C:\", r"c:\", r"D:\", r"Z:\\", r"d:/"] {
            assert!(
                is_windows_critical_path(p),
                "盘符根本应被拦截，实际放行了: {}",
                p
            );
        }
    }

    #[test]
    fn windows_users_root_and_profiles_blocked_but_home_subpaths_allowed() {
        // Users 自身、默认/公共配置必须拦
        for p in [
            r"C:\Users",
            r"C:\Users\",
            r"C:\Users\Public\Desktop",
            r"C:\Users\Default\AppData",
            r"C:\Users\Default User",
            r"C:\Users\All Users",
        ] {
            assert!(
                is_windows_critical_path(p),
                "用户配置根目录本应被拦截，实际放行了: {}",
                p
            );
        }

        // 但普通用户的子树整体不能被一刀切，否则清理功能彻底失效。
        // （个人敏感目录由第 4 层 home 黑名单负责，不在这一层。）
        for p in [
            r"C:\Users\Bob\AppData\Local\npm\Cache",
            r"C:\Users\Bob\AppData\Local\Temp\junk",
            r"C:\Users\Bob\Documents",
        ] {
            assert!(
                !is_windows_critical_path(p),
                "个人目录下的清理目标不应被这一层拦死: {}",
                p
            );
        }
    }

    #[test]
    fn windows_device_prefix_and_case_insensitivity() {
        // `\\?\` 设备前缀剥掉后规则照样生效
        assert!(is_windows_critical_path(r"\\?\C:\Windows\System32"));
        assert!(is_windows_critical_path(r"\\?\C:\"));
        // 大小写不敏感（Windows 文件系统语义）
        assert!(is_windows_critical_path(r"c:\WINDOWS\SYSTEM32"));
        assert!(is_windows_critical_path(r"c:\PROGRAM FILES\x"));
        // 混用正斜杠的输入也要识别
        assert!(is_windows_critical_path("C:/Windows/System32"));
    }

    #[test]
    fn windows_unc_paths_are_blocked() {
        // 网络共享 deleting 不可控，且不该由本机清理工具触及
        assert!(is_windows_critical_path(r"\\server\share\dir"));
        assert!(is_windows_critical_path(r"\\nas\backup\Users\Bob"));
    }

    #[test]
    fn posix_paths_unaffected_by_windows_rules() {
        // macOS/Linux 路径不得误命中 Windows 规则，否则会误伤本机
        for p in [
            "/System",
            "/usr/bin",
            "/Users/Bob/Library/Caches/com.foo.bar",
            "/private/var/folders/xx",
            "/opt/homebrew/Cellar/node",
        ] {
            assert!(
                !is_windows_critical_path(p),
                "POSIX 路径被 Windows 规则误命中: {}",
                p
            );
        }
    }

    #[test]
    fn windows_blocklist_reaches_critical_system_path_entrypoint() {
        // 端到端：走的是第 3 层实际调用的那个函数，而不是绕过它直接调内部表
        assert!(is_critical_system_path(r"C:\Windows\System32"));
        assert!(is_critical_system_path(r"D:\ProgramData"));
        assert!(is_critical_system_path(r"\\server\share\x"));
        // macOS 既有规则没有被这次改动破坏
        assert!(is_critical_system_path("/System"));
        assert!(is_critical_system_path("/usr/bin"));
    }

    // ---------- W-4: 路径遍历检测必须支持反斜杠 ----------

    #[test]
    fn backslash_traversal_is_rejected() {
        // 旧实现只 split('/')，下面这条在 Windows 上全会成了一个普通组件
        let dangerous = r"C:\Users\Bob\AppData\..\..\..\Windows";
        match check_path_safety_with_category(dangerous, "") {
            SafetyCheck::Danger(reason) => {
                assert!(
                    reason.contains("目录遍历"),
                    "应当报告目录遍历，实际原因: {}",
                    reason
                );
            }
            other => panic!(
                "含 .. 的 Windows 路径未被拦截: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn posix_traversal_still_rejected() {
        assert!(matches!(
            check_path_safety_with_category("/Users/Bob/../../System", ""),
            SafetyCheck::Danger(_)
        ));
    }

    #[test]
    fn dotted_filename_still_allowed() {
        // 文件名里含 .. 是合法的（Firefox 的 xxx..files），不能被误伤。
        // 这里只验证"不会被当作目录遍历拒绝"，是否为 Danger 取决于其它层。
        for p in [
            r"C:\Users\Bob\AppData\Roaming\Mozilla\foo..files",
            "/Users/Bob/Library/Caches/foo..files",
        ] {
            if let SafetyCheck::Danger(reason) = check_path_safety_with_category(p, "") {
                assert!(
                    !reason.contains("目录遍历"),
                    "含 .. 的合法文件名被误判为目录遍历: {} -> {}",
                    p,
                    reason
                );
            }
        }
    }

    // ---------- P0-5: 「 | 」不得再绕过黑名单 ----------

    #[test]
    fn pipe_separator_no_longer_bypasses_blacklist() {
        // 原实现：`starts_with("com.apple.TimeMachine.") || contains(" | ")`。
        // `||` 让第二个条件独立生效 —— 任何含「空格|空格」的路径都会在
        // 下面全部 7 层黑名单之前被无条件 return Safe。
        //
        // 下面这些路径前缀都命中 prefix_match，只因尾部带了 " | x" 就被放行。
        for p in [
            "/System/ | x",
            "/System/Library/Caches/ | x",
            "/usr/bin/ | x",
            "/private/var/db/ | x",
        ] {
            let r = check_path_safety_with_category(p, "");
            assert!(
                !matches!(r, SafetyCheck::Safe),
                "含 ' | ' 的系统路径不应被放行: {} -> {:?}",
                p,
                r
            );
        }
    }

    #[test]
    fn time_machine_snapshot_still_allowed() {
        // 真正的 APFS 快照名（前缀 + 分隔符同时满足）仍应放行，
        // 收紧条件不能把正常功能改坏。
        let r = check_path_safety_with_category(
            "com.apple.TimeMachine.2024-01-01-120000 | Macintosh HD",
            "",
        );
        assert!(
            matches!(r, SafetyCheck::Safe),
            "APFS 快照名应被放行, 实际: {:?}",
            r
        );
    }

    #[test]
    fn test_edr_detection_crowdstrike() {
        assert!(is_endpoint_security_cache_path(
            "/private/var/folders/ab/com.crowdstrike.falcon.T/abc"
        ));
    }

    #[test]
    fn test_edr_detection_sentinelone() {
        assert!(is_endpoint_security_cache_path(
            "/var/folders/xy/com.sentinelone.agent/C/xyz"
        ));
    }

    #[test]
    fn test_edr_detection_jamf() {
        assert!(is_endpoint_security_cache_path(
            "/private/var/folders/cd/com.jamf.management/C/data"
        ));
    }

    #[test]
    fn test_edr_detection_non_edr_path() {
        // 普通 var/folders 路径不应被标记为 EDR
        assert!(!is_endpoint_security_cache_path(
            "/private/var/folders/ab/abc123/T/com.apple.something"
        ));
        assert!(!is_endpoint_security_cache_path(
            "/Users/test/Library/Caches/com.crowdstrike.falcon"
        ));
        assert!(!is_endpoint_security_cache_path("/tmp/test"));
    }

    #[test]
    fn test_edr_detection_blocks_deletion() {
        // EDR 路径应返回 Danger
        let result = check_path_safety_with_category(
            "/private/var/folders/ab/com.crowdstrike.falcon.T/abc",
            "",
        );
        assert!(matches!(result, SafetyCheck::Danger(_)));
    }

    // ---------- P1-15: 第 4 层必须同时覆盖 $HOME 与真实 home ----------

    #[test]
    fn homes_from_covers_both_env_and_real_home() {
        // 只保护 $HOME 是不够的：把它指到别处，第 4 层的保护前缀整体偏移，
        // 真实的 ~/Library/Mail、~/.ssh 就全部失防。
        let env_home = PathBuf::from("/Users/fakehome");
        let real_home = PathBuf::from("/Users/realhome");

        let homes = homes_from(Some(env_home.clone()), Some(real_home.clone()));
        assert_eq!(
            homes,
            vec![env_home.clone(), real_home.clone()],
            "$HOME 与真实 home 必须同时在保护集合里"
        );

        // 去重：两者相同时只留一份
        assert_eq!(
            homes_from(Some(env_home.clone()), Some(env_home.clone())),
            vec![env_home.clone()]
        );
        // 缺失的一侧不应塞进占位值
        assert_eq!(
            homes_from(None, Some(real_home.clone())),
            vec![real_home.clone()]
        );
        assert_eq!(
            homes_from(Some(env_home.clone()), None),
            vec![env_home.clone()]
        );
        assert!(homes_from(None, None).is_empty());
        // 空路径不代表「根目录」，不能入列（否则扫描会退化为扫 /）
        assert!(homes_from(Some(PathBuf::new()), None).is_empty());
    }

    #[test]
    fn home_protection_covers_every_injected_home_not_just_the_first() {
        // 这是 #15 的**确定性**测试。
        //
        // homes_from 那条测试有个缺口：它直接调纯函数，抓不到
        // 「第 4 层只遍历单一 home」这类回归。这里注入两份不同的 home，
        // 只要第四层少遍历任何一份就会被发现 —— 不依赖本机 $HOME 是否等于真实 home。
        let env_home = PathBuf::from("/Users/attacker-controlled");
        let real_home = PathBuf::from("/Users/realhome");
        let homes = [env_home.clone(), real_home.clone()];

        for home in &homes {
            // 4a: 目录本身（精确匹配）
            for exact in [
                "Library/Preferences",
                "Library/Caches",
                "Library/Containers",
            ] {
                let p = home.join(exact);
                assert!(
                    check_home_paths(&p, &p.to_string_lossy(), &homes).is_some(),
                    "4a 未拦截: {}",
                    p.display()
                );
            }
            // 4b: 前缀匹配（含子路径）
            for (prefix, child) in [
                ("Library/Keychains", "login.keychain-db"),
                ("Library/Mail", "V10/INBOX.mbox"),
                (".ssh", "id_rsa"),
            ] {
                let base = home.join(prefix);
                let deep = base.join(child);
                for target in [base, deep] {
                    assert!(
                        check_home_paths(&target, &target.to_string_lossy(), &homes).is_some(),
                        "4b 未拦截: {}",
                        target.display()
                    );
                }
            }
            // 主目录本身
            assert!(
                check_home_paths(home, &home.to_string_lossy(), &homes).is_some(),
                "主目录本身必须被拦截: {}",
                home.display()
            );
        }

        // 对照组：两份 home 之外的同名目录不在保护范围（确认没有过度拦截）
        let unrelated = PathBuf::from("/Volumes/OtherDisk/Library/Mail");
        assert!(
            check_home_paths(&unrelated, &unrelated.to_string_lossy(), &homes).is_none(),
            "不应越界拦截保护名单之外的路径"
        );
    }

    #[test]
    fn critical_home_subpath_blocked_for_every_protected_home() {
        // 端到端验证：protected_homes() 里的每一份 home，其关键子目录都要被拦。
        // 这条能抓住「第 4 层只遍历单一 home」的回归。
        let homes = protected_homes();
        assert!(!homes.is_empty(), "至少要有一份 home，否则用户数据完全失防");
        for home in homes {
            for sub in ["Library/Mail", "Library/Keychains", ".ssh"] {
                let p = home.join(sub);
                let r = check_path_safety_with_category(p.to_string_lossy().as_ref(), "");
                assert!(
                    matches!(r, SafetyCheck::Danger(_)),
                    "未被拦截: {}",
                    p.display()
                );
            }
        }
    }

    #[test]
    fn test_leftover_plist_with_credential_allowed() {
        // ~/Library/Preferences/ 下含 credential 关键字的 plist 应允许删除
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/NONEXISTENT"));
        let path = home.join("Library/Preferences/git-credential-manager.plist");
        let result = check_path_safety_with_category(path.to_string_lossy().as_ref(), "");
        assert!(matches!(result, SafetyCheck::Safe));
    }

    #[test]
    fn home_credential_and_sync_dirs_are_protected() {
        // 本轮增强：凭证目录、密码库、同步盘根（对标 MangoDisk PROTECTED_HOME_ROOTS）
        let home = std::env::temp_dir().join(format!("maclean_home_cred_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for sub in [
            ".aws",
            ".azure",
            ".gcloud",
            ".kube",
            ".netrc",
            ".password-store",
            ".git-credentials",
            ".docker",
            ".env",
            "OneDrive",
            "Dropbox",
            "Google Drive",
            "iCloud Drive",
            "Nextcloud",
            "Box",
        ] {
            let p = home.join(sub);
            std::fs::create_dir_all(&p).unwrap();
            // 用可注入 home 的纯函数检查（真实 $HOME 不包含测试目录）
            let s = check_home_paths(
                &p,
                p.to_string_lossy().as_ref(),
                std::slice::from_ref(&home),
            );
            assert!(
                matches!(s, Some(SafetyCheck::Danger(_))),
                "{} 应被拒绝删除，实际 {:?}",
                sub,
                s
            );
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn repo_metadata_components_protected_at_any_depth() {
        // 对标 MangoDisk PROTECTED_REPOSITORY_COMPONENTS：任意深度的 .git 均拒绝
        let tmp = std::env::temp_dir().join(format!("maclean_repo_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for (label, p) in [
            ("git_root", tmp.join("proj/.git")),
            ("git_deep", tmp.join("a/b/c/.git")),
            ("hg", tmp.join("proj/.hg")),
            ("svn", tmp.join("proj/.svn")),
            ("bzr", tmp.join("proj/.bzr")),
        ] {
            std::fs::create_dir_all(&p).unwrap();
            let s = check_path_safety_with_category(p.to_string_lossy().as_ref(), "");
            assert!(
                matches!(s, SafetyCheck::Danger(_)),
                "{} 应被拒绝删除，实际 {:?}",
                label,
                s
            );
        }
        // 普通缓存目录不受影响
        let ok_dir = tmp.join("proj/DerivedData");
        std::fs::create_dir_all(&ok_dir).unwrap();
        let s = check_path_safety_with_category(ok_dir.to_string_lossy().as_ref(), "");
        assert!(
            matches!(s, SafetyCheck::Safe),
            "普通缓存目录应放行: {:?}",
            s
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
