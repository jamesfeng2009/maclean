//! 安全防护模块
//!
//! 多层防护机制，确保不会误删除系统关键文件:
//! 1. 路径白名单 - 只允许删除已知安全路径下的文件
//! 2. 路径黑名单 - 明确禁止删除的系统关键路径
//! 3. 路径模式校验 - 检查路径是否匹配预期的安全模式
//! 4. 删除前二次校验 - 删除前再次检查路径合法性
//! 5. 删除日志 - 记录所有删除操作，可回溯

use std::path::{Path, PathBuf};

use maclean_types::domain::{CleanupAction, CleanupCandidate, StorageEntity};
use maclean_types::safety::SafetyGate;

/// 安全检查结果 —— Open Core 拆分后迁移至公开契约层
/// `maclean-types::safety`（2026-10-07，tasks.md P3-1/P4-3）。
///
/// 这里只做 re-export：既有 `crate::safety::SafetyCheck` 引用路径不变。
pub use maclean_types::safety::SafetyCheck;

/// 契约层 `SafetyGate` 的生产实现（D-10）。
///
/// 之前 `types/safety.rs` 里只有定义与单测，删除链全走自由函数，trait 形同
/// 虚设。现在把它接到唯一闸门上：所有破坏性出口调 [`gate_allows`]，而
/// `gate_allows` 必须经由本实现，契约层的「validate 非 Safe 不得删除」才真正成立。
pub struct DefaultSafetyGate;

impl SafetyGate for DefaultSafetyGate {
    fn validate(&self, candidate: &CleanupCandidate) -> SafetyCheck {
        check_path_safety_with_category(&candidate.entity.path, &candidate.entity.category)
    }
}

/// 删除出口的**唯一**放行判定（A-07a）。
///
/// 不要在各出口自己 `match SafetyCheck`：历史上有出口只判 `Danger` 就把
/// `Warning` 放行了，同一路径在不同删除链得到相反结论。统一走这里，
/// `is_safe()` 只认 `Safe`，`Warning` 与 `Danger` 一律拒绝。
///
/// `Warning` 的「需要额外确认」语义此前零生产消费者，本实现按最保守口径
/// 收敛为「不放行」；若将来要接确认流，必须同时改这里与所有出口。
pub fn gate_decision(path: &str, category: &str) -> SafetyCheck {
    let candidate = CleanupCandidate {
        entity: StorageEntity {
            path: path.to_string(),
            category: category.to_string(),
            ..Default::default()
        },
        safety: SafetyCheck::Safe,
        reclaimable_bytes: 0,
        action: CleanupAction::Delete,
    };
    DefaultSafetyGate.validate(&candidate)
}

pub fn gate_allows(path: &str, category: &str) -> bool {
    gate_decision(path, category).is_safe()
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

pub(crate) fn protected_homes() -> Vec<PathBuf> {
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

/// 路径比较的宽松键（A-09）。
///
/// macOS APFS 默认大小写不敏感，且非 ASCII 常以 NFD（分解）存储，而
/// `$HOME` / 用户输入的路径可能是 NFC（合成）。直接用 `Path` 的字节比较时
/// `~/library/mail` 与 `~/Library/Mail` 会指向同一目录却不命中黑名单。
///
/// 这里做两件事：先 NFC 归一化（把 `e\u{301}` 合成 `é`），再转小写。
/// 早期版本是「丢弃组合标记」，那只会让 NFD 变成 `e`、NFC 仍是 `é`，
/// 两者依然不相等 —— 保护并没有真正生效。
fn loose_component_key(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfc().flat_map(char::to_lowercase).collect()
}

fn loose_components(p: &Path) -> Vec<String> {
    // 复用 provenance 的跨平台组件键：Windows canonicalize 的 `\\?\` verbatim
    // 前缀与 home 的普通盘符前缀不相等，直接用 Path::components() 会让整个
    // 第 4 层 home 黑名单在 Windows 上静默失防。
    crate::provenance::path_component_key(p)
        .iter()
        .map(|c| loose_component_key(c))
        .collect()
}

fn loose_starts_with(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && path[..prefix.len()] == prefix[..]
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
    // 云同步盘根：Windows 客户端同样会把这些目录建在 home 下，删根会连带云端。
    // 之前只在 macOS 表里有，Windows  runner 上实测整批漏保护。
    "OneDrive",
    "OneDrive - ",
    "Dropbox",
    "Google Drive",
    "iCloud Drive",
    "Nextcloud",
    "OwnCloud",
    "Box",
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
    ".env",             // 环境变量文件（常含密钥），Windows 项目里同样常见
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

/// 凭据/配置点目录整体受前缀保护，但其中**纯可再生的客户端缓存**子目录可安全整目录
/// 清理。此处是精确相对路径白名单（相对 home），只放行这些目录本身：
/// - `.kube/cache`：kubectl/helm 的服务端发现缓存与 RESTMapper 缓存（
///   `api`/`discovery`/`http` 等 json），下次执行 kubectl 时自动重建；真正的集群凭证
///   在 `~/.kube/config`（一个文件，不在本名单）与其它子目录，仍受保护。
///
/// 刻意只精确到目录本身、不做前缀：缓存目录的更深层路径也不放行，避免任何放宽蔓延。
const RECLAIMABLE_INSIDE_PROTECTED_HOME: &[&str] = &[".kube/cache"];

/// 第 4 层（平台无关实现）
///
/// `windows` 参数决定用哪套黑名单，由调用方注入以便测试。
fn check_home_paths_for_platform(
    canonical: &Path,
    canonical_str: &str,
    homes: &[PathBuf],
    windows: bool,
) -> Option<SafetyCheck> {
    // 组件级、大小写与 Unicode 规范化无关的比较（A-09）。
    // 注意不能直接用 `Path::starts_with`：它按原始字节比，APFS 上
    // `~/library/mail` 与 `~/Library/Mail` 是同一目录却匹配不上。
    let candidate = loose_components(canonical);

    // 用户主目录本身禁止删除
    if homes.iter().any(|h| candidate == loose_components(h)) {
        return Some(SafetyCheck::Danger("拒绝删除用户主目录".to_string()));
    }

    // 凭据点目录内的精确可再生缓存子目录（如 ~/.kube/cache）放行：
    // 只对白名单目录本身生效，其兄弟项（.kube/config 等）与其更深层路径仍走下面的保护。
    for home in homes {
        let home_comps = loose_components(home);
        if loose_starts_with(&candidate, &home_comps) {
            let rel_comps = &candidate[home_comps.len()..];
            let rel = rel_comps.join("/");
            if RECLAIMABLE_INSIDE_PROTECTED_HOME.contains(&rel.as_str()) {
                return None;
            }
        }
    }

    let (forbidden_exact, forbidden_prefix) = home_blacklist(windows);

    // 4a. 精确匹配：只保护目录本身，允许删除其子项
    for home in homes {
        for forbidden_suffix in forbidden_exact {
            if candidate == loose_components(&home.join(forbidden_suffix)) {
                return Some(SafetyCheck::Danger(format!(
                    "拒绝删除用户关键目录: {}",
                    canonical_str
                )));
            }
        }
    }

    // 4b. 前缀匹配：保护目录本身及其所有子内容（组件级，跨平台）
    for home in homes {
        for forbidden_suffix in forbidden_prefix {
            let forbidden = loose_components(&home.join(forbidden_suffix));
            if loose_starts_with(&candidate, &forbidden) {
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
///
/// 路径任意层级是否命中清单管理的包目录标记（site-packages / dist-packages /
/// node_modules / .venv* / .terraform / go/pkg/mod / *.app/Contents）。
///
/// 这些目录由包管理器清单（RECORD / package-lock.json / go.sum 等）管理：
/// 删任一份副本都会破坏完整性校验，属于「项目文件」而非缓存。
/// 任意层级出现即命中（不依赖 home 相对前缀），供 safety 第 4.6 层与
/// 重复文件候选收集共用，保证两处判定永远一致。
pub fn is_manifest_managed_path(path: &Path) -> bool {
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (0..comps.len()).any(|i| manifest_marker_hit(&comps, i))
}

/// 路径是否位于某个 git 项目工作区内（向上找 `.git`，以受保护 home 为界）。
///
/// 供第 4.7 层（重复文件类目删除硬闸门）使用：项目代码/数据文件
/// （.git 祖先）在重复组里只能是保留方，绝不删除。home 自身不算项目根。
pub fn is_inside_git_project(path: &Path, homes: &[PathBuf]) -> bool {
    let mut cur = path.parent();
    while let Some(dir) = cur {
        if dir == Path::new("/") || homes.iter().any(|h| dir == h.as_path()) {
            break;
        }
        if dir.join(".git").exists() {
            return true;
        }
        cur = dir.parent();
    }
    false
}

/// P2-2：进程级 git 判定缓存。删除期每个子项都过 4.7 层向上找 `.git`，
/// 几百项时重复磁盘 stat；同一删除任务内路径集合稳定，缓存按 canonical
/// 路径缓存"是否在 git 项目内"，命中即免 IO。缓存失败（锁毒化）退化为
/// 无缓存直算，绝不影响判定正确性。
static GIT_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, bool>>> =
    std::sync::OnceLock::new();

fn git_cache() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, bool>> {
    GIT_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub fn is_inside_git_project_cached(path: &Path, homes: &[PathBuf]) -> bool {
    if let Ok(mut cache) = git_cache().lock() {
        if let Some(&v) = cache.get(path) {
            return v;
        }
        let v = is_inside_git_project(path, homes);
        cache.insert(path.to_path_buf(), v);
        v
    } else {
        is_inside_git_project(path, homes)
    }
}

/// 重复文件类目"候选可删"判定（扫描期与删除期 4.8 层共用的唯一判定源）。
///
/// 语义 = 第 4.8 层 Danger 的精确补集：
/// `allowed = 位于缓存目录白名单 && 扩展名不在不可再生列表`。
/// 扫描期用它过滤候选（不满足的不进列表），删除期用它拦截（不满足的
/// Danger）—— 两处共用同一函数，杜绝"扫得出、删不掉"或"删得掉、扫不出"
/// 的规则漂移。
pub(crate) fn duplicate_candidate_allowed(path: &Path) -> bool {
    if !is_repeat_cache_dir(path) {
        return false;
    }
    if let Some(ext) = path.extension() {
        let ext_l = ext.to_string_lossy().to_ascii_lowercase();
        if NON_RECREATABLE_EXTS.contains(&ext_l.as_str()) {
            return false;
        }
    }
    true
}

/// 重复文件类目（扫描 + 删除共用）的**目录白名单**：只处理明确缓存目录
/// 内的副本。
///
/// 历史事故（9-30）：用户文档（Downloads）、项目数据（onlineStudy）、
/// ComfyUI 生成图、IDE 扩展组件只要内容重复就被移入废纸篓。第 4.7 层
/// 拦截 git 项目内文件，但无 git 的用户数据（docx/pdf/jpg 等）仍漏网。
/// 本函数把"重复文件"的扫描与删除都钉死在缓存目录家族 —— 缓存目录外
/// 一律拒绝，宁可少省空间也不误删（用户明确接受的保守原则）。
pub(crate) fn is_repeat_cache_dir(path: &Path) -> bool {
    // A-03：一律用组件级 `Path::starts_with`。旧实现是裸字符串前缀，
    // `~/Library/CachesEvil`、`/Library/CachesX` 都会被当成缓存目录。
    for h in protected_homes() {
        if path.starts_with(h.join("Library/Caches")) || path.starts_with(h.join(".cache")) {
            return true;
        }
    }
    if path.starts_with(Path::new("/Library/Caches")) {
        return true;
    }

    // 沙盒容器缓存：~/Library/Containers/<bundle-id>/Data/Library/Caches
    // 旧实现用两段独立 `contains`，任何路径只要凑齐这两段（顺序不限）就放行。
    // 这里按组件顺序窗口匹配，且要求中间确实隔着一个 bundle 组件。
    let comps: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    comps.windows(6).any(|w| {
        w[0] == "Library"
            && w[1] == "Containers"
            && !w[2].is_empty()
            && w[3] == "Data"
            && w[4] == "Library"
            && w[5] == "Caches"
    })
}

/// 缓存目录内也拒绝删除的"用户数据/不可再生"扩展名（仅重复文件类目）。
///
/// 文档、代码、数据库、压缩包、字体即使出现在缓存目录中，也极可能是被
/// 同步/备份工具误放的用户数据，删任一份即破坏。图片/音视频/无扩展名
/// 二进制不入列 —— 它们是缓存清理的价值所在，且目录白名单已保护用户区。
const NON_RECREATABLE_EXTS: &[&str] = &[
    // 文档
    "doc", "docx", "pdf", "ppt", "pptx", "xls", "xlsx", "txt", "md",
    // 代码 / 配置 / 数据
    "ts", "tsx", "js", "jsx", "mjs", "py", "go", "rs", "java", "c", "cc", "cpp", "h", "hpp", "sql",
    "json", "yaml", "yml", "toml", "ipynb", "sh", "vue", "css", "scss", // 数据库
    "db", "sqlite", "sqlite3", // 压缩包 / 字体
    "zip", "rar", "7z", "tar", "gz", "tgz", "ttf", "otf", "woff", "woff2",
];

/// 路径组件中是否出现指定名称（任意层级，组件精确相等）。
///
/// 用于 4.6 层对 Monorepo依赖 类目的 node_modules 精确豁免 —— 只放行
/// 组件就叫 node_modules 的目录，`node_modulesx` 之类相似名不命中。
fn canonical_has_component(path: &Path, name: &str) -> bool {
    path.components().any(|c| match c {
        std::path::Component::Normal(n) => n.to_string_lossy() == name,
        _ => false,
    })
}

/// 单组件清单标记判定（第 4.6 层与后代深扫共用）。
/// `comps[i]` 为当前组件，部分标记需结合后续组件（go/pkg/mod、
/// *.app/Contents、.venv+数字）。
fn manifest_marker_hit(comps: &[String], i: usize) -> bool {
    let c = &comps[i];
    if c == "site-packages" || c == "dist-packages" || c == "node_modules" || c == ".terraform"
        // 无点前缀的 venv 命名（python -m venv venv）与 virtualenv 聚合目录
        // （virtualenvs / .virtualenvs）：事故后必须补上的常见形态
        || c == "venv" || c == "virtualenvs" || c == ".virtualenvs"
    {
        return true;
    }
    // .venv* 只匹配虚拟环境命名（.venv / .venv2 / .venv311 等数字后缀），
    // 避免把 .venvista 这类普通目录误伤。
    if c.starts_with(".venv") && (c.len() == 5 || c[5..].chars().all(|ch| ch.is_ascii_digit())) {
        return true;
    }
    if c == "go"
        && comps.get(i + 1).map(|s| s.as_str()) == Some("pkg")
        && comps.get(i + 2).map(|s| s.as_str()) == Some("mod")
    {
        return true;
    }
    if c.ends_with(".app") && comps.get(i + 1).map(|s| s.as_str()) == Some("Contents") {
        return true;
    }
    false
}

/// 仅按目录名判断清单标记（不含需要后续组件的形态）。
/// 供后代深扫使用：那里只能看到单个条目名。
fn manifest_marker_name_only(name: &str) -> bool {
    name == "site-packages"
        || name == "dist-packages"
        || name == "node_modules"
        || name == ".terraform"
        || name == "venv"
        || name == "virtualenvs"
        || name == ".virtualenvs"
        || (name.starts_with(".venv")
            && (name.len() == 5 || name[5..].chars().all(|ch| ch.is_ascii_digit())))
}

/// 目录的**后代**中是否存在清单管理结构 —— 拦截"删清单树祖先目录"。
///
/// 第 4.6 层原本只看被删路径自身的组件，删 `~/Library/Caches/pypoetry`
/// （Poetry 默认虚拟环境就在其 virtualenvs/ 子树下）完全绕防。判定信号：
/// 子目录名命中清单标记，或出现 `pyvenv.cfg`（虚拟环境签名文件）。
///
/// 刻意**不含** `*.app/Contents` 与 `go/pkg/mod`：否则"删一个含 app bundle
/// 的目录"（卸载功能的主路径）会被误伤。有界遍历（深度 ≤ 8、条目 ≤ 2 万）
/// 控制删除链延迟；配合"默认全部进废纸篓"，预算耗尽漏判仍可挽回。
pub fn contains_manifest_managed_descendant(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    const MAX_DEPTH: usize = 8;
    const MAX_ENTRIES: usize = 20_000;
    let mut visited = 0usize;
    for entry in walkdir::WalkDir::new(path)
        .min_depth(1)
        .max_depth(MAX_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        visited += 1;
        if visited > MAX_ENTRIES {
            break;
        }
        let ft = entry.file_type();
        if ft.is_file() && entry.file_name() == "pyvenv.cfg" {
            return true;
        }
        if ft.is_dir() && manifest_marker_name_only(&entry.file_name().to_string_lossy()) {
            return true;
        }
    }
    false
}

/// 公认可再生缓存根的**精确相对路径**（相对 home）。
///
/// 这些目录本质是包管理器的下载 / 内容寻址缓存，整目录删除等同于
/// `npm cache clean --force`、`pip cache purge`、清空 pnpm store 等标准操作，
/// 随时可重新下载生成；其内部出现 node_modules / site-packages / 打包依赖是
/// 常态而非"项目环境"。因此第 4.6 层的「后代含清单结构就拒删祖先」对它们豁免。
const RECLAIMABLE_CACHE_ROOT_REL: &[&str] = &[
    ".npm",                    // npm 内容寻址缓存
    ".cache/pip",              // pip 缓存（XDG）
    "Library/Caches/pip",      // pip 缓存（macOS 常见位置）
    "Library/Caches/Yarn",     // Yarn 缓存（macOS）
    ".cache/yarn",             // Yarn 缓存（XDG）
    "Library/pnpm/store",      // pnpm content-addressable store（macOS）
    ".local/share/pnpm/store", // pnpm store（XDG）
];

/// 判定某路径是否为可整目录回收的缓存根（home 由调用方注入，便于单测）。
///
/// 刻意保持精确白名单，**不**做宽泛前缀：
/// - 只接受白名单中的精确相对路径；
/// - Squirrel / Mac AutoUpdate 更新器残留只认 `Library/Caches/<name>.ShipIt`
///   这一个直接子项（中断的升级临时目录，整目录是垃圾，如
///   `com.microsoft.VSCode.ShipIt`）。
///
/// `~/Library/Caches/pypoetry/virtualenvs` 这类真实虚拟环境宿主**不在**名单，
/// 仍由后代深扫拦截。系统关键目录 / 容器数据 / .git 等红线也不受本豁免影响。
fn reclaimable_cache_root_with_homes(path: &Path, homes: &[PathBuf]) -> bool {
    for home in homes {
        let Ok(rel) = path.strip_prefix(home) else {
            continue;
        };
        // 白名单是 POSIX 相对路径；Windows 的 `strip_prefix` 结果用 `\`，
        // 必须先归一分隔符，否则整份白名单在 Windows 上形同虚设。
        let rel = crate::provenance::path_key_string(rel);
        if RECLAIMABLE_CACHE_ROOT_REL.contains(&rel.as_str()) {
            return true;
        }
        // 更新器残留：Library/Caches 下的直接子项且名字以 .ShipIt 结尾
        if let Some(name) = rel.strip_prefix("Library/Caches/") {
            if !name.contains('/') && name.ends_with(".ShipIt") {
                return true;
            }
        }
    }
    false
}

/// 判定某路径是否为可整目录回收的缓存根（生产环境用受保护 home 集合）。
fn is_reclaimable_cache_root(path: &Path) -> bool {
    reclaimable_cache_root_with_homes(path, &protected_homes())
}

/// 判定被删路径是否为「应用数据根」：`~/Library/Application Support/<App>`
/// （home 由调用方注入，便于单测）。只认 Application Support 的**直接子项**，
/// 更深的嵌套目录不匹配。
///
/// 为什么需要它：第 4.6 层「祖先目录含 venv/node_modules/site-packages 就拒删」
/// 保护的是**用户自己的项目依赖 / Python 虚拟环境**。但同样的目录名出现在应用
/// 数据根里时，几乎都是**应用自带的捆绑运行时 / 插件 / 类型存根**，例如：
///   - PyCharm: `Application Support/JetBrains/PyCharm*/plugins/python-ce/
///     helpers/typeshed/stdlib/venv`（标准库类型存根，目录名叫 venv，并非虚拟环境）
///   - TRAE:   `Application Support/TRAE SOLO CN/ModularData/ai-agent/vm/tools/
///     lib/node_modules` 与 `lib/python3.10/site-packages`（内置 node / Python VM）
///
/// 这些随应用可重装、删除统一移入废纸篓可恢复，不属于要保护的用户项目环境。
/// 用户在「应用数据」分类勾选整目录、且经高级风险二次确认后删除，是设计内的
/// 「重装级清理」。因此仅对这**一个位置**豁免第 4.6 层的后代深扫；系统关键目录、
/// 容器数据（Docker/OrbStack）、仓库 .git、敏感文件名等其它红线一律不受影响。
fn app_support_data_root_with_homes(path: &Path, homes: &[PathBuf]) -> bool {
    for home in homes {
        let Ok(rel) = path.strip_prefix(home) else {
            continue;
        };
        // 归一分隔符：Windows 上 `Library\Application Support\<App>` 用
        // 字面量 "Library/Application Support/" 永远 strip 不掉。
        let rel = crate::provenance::path_key_string(rel);
        let rel = rel.trim_start_matches('/');
        // 必须恰好是 Library/Application Support/<一个直接子项>：
        // Application Support 根本身、以及再深一层的嵌套目录都不放行。
        if let Some(name) = rel.strip_prefix("Library/Application Support/") {
            if !name.is_empty() && !name.contains('/') {
                return true;
            }
        }
    }
    false
}

/// 判定被删路径是否为应用数据根（生产环境用受保护 home 集合）。
fn is_app_support_data_root(path: &Path) -> bool {
    app_support_data_root_with_homes(path, &protected_homes())
}

/// 伪路径标记的格式白名单（A-10）。
///
/// 只接受「已知前缀 + 由安全字符组成的标识」，绝不允许 `/`、`\`、`..`，
/// 也不允许把绝对路径藏在 ` | ` 之后。APFS 快照卷名允许空格。
fn is_well_formed_pseudo_path(path: &str) -> bool {
    fn safe_label(s: &str) -> bool {
        !s.is_empty()
            && !s.contains("..")
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_' | ':'))
    }

    if let Some(rest) = path.strip_prefix("com.apple.TimeMachine.") {
        let Some((snapshot, volume)) = rest.split_once(" | ") else {
            return false;
        };
        return safe_label(snapshot) && safe_label(volume);
    }
    if let Some(rest) = path
        .strip_prefix("docker:")
        .or_else(|| path.strip_prefix("orbstack:"))
    {
        return safe_label(rest);
    }
    false
}

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

    // 非文件路径（APFS 快照 / docker prune 标记）由外部命令处理，不删文件。
    //
    // A-10：旧实现在这里无条件 `return Safe`，位于符号链接解析、系统黑名单、
    // home 黑名单之前 —— 拼一个 `com.apple.TimeMachine.x | /System/...`
    // 就能绕过全部 7 层。现在只记录标记并收窄格式，让它继续走完所有检查，
    // 到函数末尾（第 8 层之前）才放行；注入的绝对路径会在第 3 层被拦下。
    let has_pseudo_prefix = path.starts_with("com.apple.TimeMachine.")
        || path.starts_with("docker:")
        || path.starts_with("orbstack:");
    let pseudo_path = is_well_formed_pseudo_path(path);
    if has_pseudo_prefix && !pseudo_path {
        return SafetyCheck::Danger("伪路径标记格式非法（含路径分隔符或目录遍历）".to_string());
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
        // Windows 的 `\\?\C:\...` verbatim 前缀与 home / 注册表给出的 `C:\...`
        // 不是同一个 Prefix 组件，会让下面按组件比较的白名单、home 黑名单、
        // provenance 准入整片失效。剥掉后再进入 7 层检查，形态与普通路径一致。
        Ok(c) => crate::provenance::strip_verbatim(c),
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

    // ================================================================
    //  第 3.5 层: 容器数据目录（Docker Desktop / OrbStack）绝对红线
    // ================================================================
    // Docker Desktop 的镜像层/容器/卷是数据库结构，直接删文件 = 破坏镜像
    // 元数据（9-30 事故：maclean 删了 Docker 容器内文件，Docker Desktop
    // 直接损坏）。OrbStack 同理。任何类目、任何删除入口一律 Danger——
    // 只允许通过官方 CLI（docker prune / orbctl delete）清理，绝不直接
    // 删容器数据文件。与删除端同源，扫描期由 precheck_deletability 过滤。
    if is_container_data_path(&canonical) {
        return SafetyCheck::Danger(format!(
            "容器数据目录（Docker/OrbStack 镜像与卷），禁止直接删除，请通过官方命令清理: {}",
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
    //  第 4.6 层: 清单管理目录保护（任意深度）
    // ================================================================
    // site-packages / dist-packages / node_modules / .venv* / .terraform /
    // go/pkg/mod / *.app/Contents 由包管理器清单管理，删任一份副本都会
    // 破坏完整性校验（用户实际误删过：delete.log 累计 350+ 条落在
    // site-packages、波及 20+ 个 venv）。这些是「项目文件」而非缓存，
    // 任何类目、任何删除入口都硬阻断 —— 与候选收集共用同一判定，
    // 保证「扫不进」与「删不掉」永远一致。
    //
    // 唯一例外（精确豁免）：Monorepo依赖 类目的目标就是 node_modules
    // —— 它由 npm/pnpm/yarn 清单管理、`npm install` 可再生，删除走废纸篓
    // 可恢复，是开发者缓存清理的设计意图，不是误删。仅当「类目 ==
    // Monorepo依赖」且路径组件含 node_modules 时放行；重复文件等其它
    // 类目遇到 node_modules 仍无条件拦截。
    if is_manifest_managed_path(&canonical)
        && !(category == "Monorepo依赖" && canonical_has_component(&canonical, "node_modules"))
    {
        return SafetyCheck::Danger(format!(
            "清单管理目录（site-packages/.venv/node_modules 等），拒绝删除: {}",
            canonical_str
        ));
    }
    // 删清单树的祖先目录同样阻断（例：~/Library/Caches/pypoetry，其
    // virtualenvs/ 子树下就是各项目的完整环境）。Monorepo依赖 豁免
    // node_modules 时一并豁免此层：npm 嵌套 node_modules 是常态，删
    // 整棵 node_modules 树正是该类目的目标。
    //
    // 再一个精确豁免：公认可再生的包管理器/更新器缓存根（~/.npm、
    // ~/Library/Caches/*.ShipIt、pip/yarn/pnpm store 等）——这些目录内部
    // 本就含解包依赖，整目录删除就是清缓存、可重新生成；不豁免会把
    // `npm cache clean` 同类的标准清理目标（实测 ~/.npm 与 VSCode.ShipIt
    // 更新残留共约 0.9GB）误判为危险。名单精确到路径，pypoetry virtualenvs
    // 等真实环境宿主不在其列、仍被拦截。
    if !(!contains_manifest_managed_descendant(&canonical)
        || is_reclaimable_cache_root(&canonical)
        // 应用数据根（~/Library/Application Support/<App>）里出现的
        // node_modules/site-packages/venv 多为应用自带运行时/插件/类型存根，
        // 不是用户项目环境；整目录删除经高级风险确认、且走废纸篓可恢复，放行。
        // 系统关键 / 容器数据 / .git / 敏感文件等其它层的红线不受影响。
        || is_app_support_data_root(&canonical)
        || category == "Monorepo依赖" && canonical_has_component(&canonical, "node_modules"))
    {
        return SafetyCheck::Danger(format!(
            "目录内部含清单管理结构（venv/site-packages 等），拒绝删除祖先目录: {}",
            canonical_str
        ));
    }

    // ================================================================
    //  第 4.7 层: git 项目工作区文件保护（仅"重复文件"类目）
    // ================================================================
    // 重复文件扫描把"内容相同"的文件当副本删除，会误删项目代码/数据文件
    // （历史事故：onlineStudy 21 个、9-30 用户文档/镜像/扩展 76 个）。
    // 删除期硬闸门：项目工作区文件（.git 祖先，home 为界）在重复组里
    // 只能是保留方，绝不删除 —— 即使扫描结果来自旧缓存/旧版本也拦得住。
    // 其它类目不启用（node 依赖清理等本就清理项目内 node_modules，
    // 由第 4.6 层按清单目录另行判定）。
    if category == "重复文件" && is_inside_git_project_cached(&canonical, &homes) {
        return SafetyCheck::Danger(format!(
            "git 项目工作区文件，重复文件类目拒绝删除: {}",
            canonical_str
        ));
    }

    // ================================================================
    //  第 4.8 层: 重复文件类目 —— 目录白名单 + 不可再生扩展名保护
    // ================================================================
    // 第 4.7 层只保护 git 项目内文件；无 git 的用户文档（docx/pdf/jpg）、
    // 项目外代码、数据库在旧缓存/旧扫描结果进入删除队列时仍会被删。
    // 双闸门：目录白名单（只删缓存目录内副本）+ 扩展名黑名单（缓存
    // 目录内命中用户数据扩展名也拒删）。与扫描期共用
    // duplicate_candidate_allowed —— 保证"扫不进"与"删不掉"永远一致。
    if category == "重复文件" && !duplicate_candidate_allowed(&canonical) {
        if !is_repeat_cache_dir(&canonical) {
            return SafetyCheck::Danger(format!(
                "非缓存目录（Library/Caches 等），重复文件类目拒绝删除: {}",
                canonical_str
            ));
        }
        if let Some(ext) = canonical.extension() {
            let ext_l = ext.to_string_lossy().to_ascii_lowercase();
            if NON_RECREATABLE_EXTS.contains(&ext_l.as_str()) {
                return SafetyCheck::Danger(format!(
                    "用户数据文件（*.{}），重复文件类目拒绝删除: {}",
                    ext_l, canonical_str
                ));
            }
        }
        return SafetyCheck::Danger(format!(
            "重复文件类目拒绝删除（目录白名单/扩展名保护）: {}",
            canonical_str
        ));
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
    // 安全模型（2026-11-08 起，A-07）：**排除 + 准入** 双层，fail-closed。
    //
    // 第 1~7 层是「排除」：系统关键目录、home 黑名单、容器数据、仓库元数据、
    // 敏感文件名、运行中应用等。但它们只回答「明确不能删」，没命中不代表能删。
    // 第 8 层是「准入」：路径必须落在扫描器显式声明的根（provenance）或静态
    // 可再生缓存准入表内，否则一律 Danger。
    //
    // 早期版本在这里直接 `return Safe`，防御力等价于黑名单完备性：漏一条规则
    // 就是一次误删。改成准入制后，漏一个根只是「少删一点」，不会误删。
    // 详见 `docs/safety/SAFETY_MODEL.md` 与 `provenance.rs`。

    // ================================================================
    //  第 6 层: 敏感文件名检测
    // ================================================================
    let file_name = canonical.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // 已卸载 App 的偏好设置 plist（~/Library/Preferences/<name>.plist）可安全删除，
    // 即使文件名包含 credential/password 等关键字（如 git-credential-manager.plist）。
    // 分隔符归一后再比：Windows 上 `C:\Users\...` 与字面量 `/Library/...`
    // 拼在一起永远对不上，plist 会被敏感名检测误判成 Warning。
    let canonical_key = crate::provenance::path_key_string(&canonical);
    let is_leftover_pref = file_name.ends_with(".plist")
        && homes.iter().any(|h| {
            canonical_key.starts_with(&format!(
                "{}/Library/Preferences/",
                crate::provenance::path_key_string(h)
            ))
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

    // 伪路径不是文件系统路径：上面 7 层排除检查已全部跑过，在此放行，
    // 不参与第 8 层的来源准入（它们由外部命令处理，不经过文件删除链）。
    if pseudo_path {
        return SafetyCheck::Safe;
    }

    // ================================================================
    //  第 8 层: 来源准入（fail-closed）
    // ================================================================
    // 前面 7 层只排除了「明确不能删」的路径；未命中黑名单不等于可以删。
    // 这里要求路径来自扫描器显式声明的根，或落在静态可再生缓存准入表内。
    // 漏注册只会导致少删，不会导致误删 —— 这是准入制相对纯黑名单的关键。
    if homes
        .iter()
        .any(|home| check_whitelist(&canonical, home, category))
        || crate::provenance::is_vetted(&canonical)
    {
        SafetyCheck::Safe
    } else {
        SafetyCheck::Danger(format!(
            "路径未通过来源准入：不在任何已声明扫描根下，拒绝删除: {}",
            canonical_str
        ))
    }
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
/// 容器数据目录判定（Docker Desktop / OrbStack）。
///
/// Docker Desktop 的镜像层/容器/卷在 `~/Library/Containers/com.docker.docker/
/// Data` 下，OrbStack 在 `~/.orbstack` 与
/// `~/Library/Group Containers/HUAQ24HBR6.dev.orbstack` 下 —— 都是数据库
/// 结构，直接删文件 = 破坏镜像元数据（9-30 事故）。任何类目任何入口一律
/// Danger，只允许通过官方 CLI（docker prune / orbctl delete）清理。
/// 与删除端同源：precheck_deletability 扫描期即标不可删。
pub(crate) fn is_container_data_path(path: &Path) -> bool {
    let p = path.to_string_lossy();
    for h in protected_homes() {
        let h = h.to_string_lossy();
        if p.starts_with(&format!("{}/Library/Containers/com.docker.docker/Data", h))
            || p.starts_with(&format!("{}/.orbstack", h))
            || p.starts_with(&format!(
                "{}/Library/Group Containers/HUAQ24HBR6.dev.orbstack",
                h
            ))
        {
            return true;
        }
    }
    false
}

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

/// 静态「可再生缓存根」准入表（A-07d，2026-11-08 重新接线）。
///
/// 与 `provenance` 的动态注册根取并集：本表覆盖已知工具缓存 / 应用数据根，
/// 动态注册覆盖各扫描器运行时枚举的根。任一命中才允许第 8 层放行。
///
/// 所有前缀判定必须走组件级 `is_same_or_under` / `is_direct_child`；
/// 裸 `starts_with` 会让 `~/Library/CachesEvil` 被当成缓存目录（见 A-03）。
fn check_whitelist(canonical: &Path, home: &Path, category: &str) -> bool {
    // 准入表用 `format!("{}/...", home)` 的 POSIX 斜杠形态书写。Windows 上
    // canonical/home 都是 `\`，直接比字符串永远不命中；统一归一分隔符并剥掉
    // verbatim 前缀后再比，两个平台共用同一张表。
    let canonical_str = crate::provenance::path_key_string(canonical);
    let home_str = crate::provenance::path_key_string(home);

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
        // 或 Container 内的 Caches/Documents 子内容（缓存/数据清理场景）。
        // "/Data/Documents/" 带尾斜杠，只匹配 Documents **内** 的条目，匹配不到
        // Documents 目录本身；这里仅对受支持 IM（微信/QQ/企微）额外放行其
        // Documents 根——用于「App 已卸载后的孤儿数据」清理（在位 IM 在扫描期
        // 即 deletable=false，根本不会走到删除）。
        let im_docs_root = crate::im_data::IM_APPS.iter().any(|a| {
            canonical_str.ends_with(&format!("/Library/Containers/{}/Data/Documents", a.bundle))
        });
        return canonical_str.contains("/Data/Library/Caches/")
            || canonical_str.contains("/Data/Documents/")
            || im_docs_root
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
    // 分隔符归一：Windows 路径用 `\`，只 trim `/` 会让尾斜杠规则失效。
    let p = path.replace('\\', "/");
    let q = prefix.replace('\\', "/");
    let normalized_path = p.trim_end_matches('/');
    let normalized_prefix = q.trim_end_matches('/');
    normalized_path == normalized_prefix
        || normalized_path.starts_with(&format!("{}/", normalized_prefix))
}

/// 检查 path 是否是 prefix 的直接子项（即 path = prefix + name，无更多层级）
/// 用于确保只允许删除 Containers/Group Containers 等目录的直接子目录，而非任意深层路径
///
/// CORE-07 / A-03：一律用组件级 `Path::starts_with` + 组件数差恰好 1。
/// 旧实现是裸字符串前缀：`/Users/fooEvil` 的前缀命中 `/Users/foo`、
/// 后缀又是 "Evil"（不含 '/'），于是被当成 home 的直接子项放行 ——
/// 第 8 层来源准入因此能被同名前缀的兄弟目录整个绕过。
fn is_direct_child(path: &str, prefix: &str) -> bool {
    let p = std::path::Path::new(path);
    let base = std::path::Path::new(prefix);
    p.starts_with(base) && p.components().count() == base.components().count() + 1
}

/// 检查路径中是否包含某个目录/文件名组件
/// 例如 /a/b/node_modules/c 包含 node_modules
fn path_contains_component(path: &str, name: &str) -> bool {
    // Windows 分隔符同样要切：否则 `node_modules`、`target` 这类项目级
    // 缓存名在 Windows 上永远匹配不到，第 8 层准入会把正常清理全拦掉。
    path.split(['/', '\\']).any(|component| component == name)
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

/// 模板变量解析：把规则根里的 `$HOME` / `%USERPROFILE%` / `%APPDATA%` /
/// `%LOCALAPPDATA%` 替换为真实值
///
/// 返回 None 表示变量缺失或为空 —— 调用方必须拒绝该规则（fail-closed）。
/// 环境变量可能被进程内篡改，因此优先用 passwd/注册表真实 home 兜底。
fn resolve_root_variable_for(
    root: &str,
    home: &std::path::Path,
    windows_style: bool,
) -> Option<String> {
    let home_str = home.to_string_lossy();
    // %APPDATA% / %LOCALAPPDATA% 按平台映射：Windows 走 AppData 目录，
    // macOS 无 Roaming/Local 之分，统一落在 ~/Library/Application Support
    // （与 Windows 侧语义最接近；规则根里的这些变量在另一平台扫描不到，
    // 由 validate 的"至少一个可用根"兜底，不会误删）。
    //
    // `windows_style` 与 `home` 都由入参注入：这样「喂 macOS 策略 + macOS 风格
    // home」在 Windows runner 上也能真跑，不会因为本机 HOME 是 `C:\Users\...`
    // 就整条 macOS 分支测不到（CORE-06 收敛的同一条约定）。
    let (appdata, localappdata) = if windows_style {
        let roaming = format!("{}\\AppData\\Roaming", home_str);
        let local = format!("{}\\AppData\\Local", home_str);
        (roaming, local)
    } else {
        let support = format!("{}/Library/Application Support", home_str);
        (support.clone(), support)
    };
    let mut out = root.to_string();
    for (var, val) in [
        ("$HOME", home_str.as_ref()),
        ("%USERPROFILE%", home_str.as_ref()),
        ("%APPDATA%", appdata.as_str()),
        ("%LOCALAPPDATA%", localappdata.as_str()),
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

// =========================================================================
//  受保护根（CORE-06 后续）：三平台共用一份纯逻辑，平台差异只剩「数据」
// =========================================================================
//
// 修复前这里有三份 #[cfg] 实现：macOS 用 canonicalize + 字符串相等、
// Windows 用 replace('/', "\\") + trim_end_matches、其它平台只认空串与 "/"。
// Linux 那份等于没有防护，而且在本机 0 编译 0 测试。现在所有判定都收敛到
// 组件级纯函数，平台只提供三张根表 + 分隔符/大小写规则（由 os 入参决定），
// 于是 macos / windows / linux 三个分支都能在开发机上真跑真测。

/// 受保护根**本体**：待删路径等于其中任一项即拒绝（相等语义）。
///
/// 含 home 容器根（/Users、/home、C:\Users）与盘符根 —— 这些只挡本体，
/// 因为用户缓存根全在 home 之下，扩大成前缀语义会把正常清理全拦掉。
const MACOS_PROTECTED_ROOTS: &[&str] = &[
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
];

/// Linux / 其它 Unix：FHS 系统根。修复前这张表根本不存在。
const LINUX_PROTECTED_ROOTS: &[&str] = &[
    "/",
    "/home",
    "/root",
    "/usr",
    "/usr/local",
    "/usr/lib",
    "/usr/lib64",
    "/usr/bin",
    "/usr/sbin",
    "/usr/share",
    "/usr/include",
    "/usr/src",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/bin",
    "/sbin",
    "/etc",
    "/var",
    "/var/lib",
    "/var/log",
    "/var/cache",
    "/opt",
    "/boot",
    "/boot/efi",
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/srv",
    "/mnt",
    "/media",
    "/tmp",
    "/var/tmp",
];

const WINDOWS_PROTECTED_ROOTS: &[&str] = &[
    "C:\\",
    "C:\\Windows",
    "C:\\Program Files",
    "C:\\Program Files (x86)",
    "C:\\Users",
    "C:\\ProgramData",
    "C:\\Recovery",
    "C:\\System Volume Information",
];

/// 规则根不得落在这些目录的**本体或任意子孙**下（组件级前缀）。
///
/// 与 `*_PROTECTED_ROOTS` 的区别：那条管「待删路径等于根本身」，这条管
/// 「声明式规则把扫描根设在系统目录里」。`/usr/local/lib` 组件数 3，只靠
/// 「组件数 ≥ 3」和「不等于受保护根本体」两道检查会放行 —— 这里补上第三道。
///
/// 三张表都**不含** `/`、`home 容器根`、`盘符根`：否则任何绝对路径、
/// `$HOME/Library/Caches`、`%LOCALAPPDATA%/Cache` 都会被一刀切拒掉
///（内置 44 条规则的根全部落在 home 之下，已核对 builtin_rules.json）。
const MACOS_FORBIDDEN_SUBTREES: &[&str] = &[
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
];

const LINUX_FORBIDDEN_SUBTREES: &[&str] = &[
    "/usr", "/lib", "/lib32", "/lib64", "/libx32", "/bin", "/sbin", "/etc", "/var", "/opt",
    "/boot", "/proc", "/sys", "/dev", "/run", "/srv", "/mnt", "/media", "/root", "/tmp",
];

const WINDOWS_FORBIDDEN_SUBTREES: &[&str] = &[
    "C:\\Windows",
    "C:\\Program Files",
    "C:\\Program Files (x86)",
    "C:\\ProgramData",
    "C:\\Recovery",
    "C:\\System Volume Information",
];

#[derive(Debug, Clone)]
struct RootPolicy {
    protected_roots: Vec<String>,
    forbidden_subtrees: Vec<String>,
    homes: Vec<PathBuf>,
    /// 分隔符与大小写规则：Windows 用 '\' 且大小写不敏感
    windows_style: bool,
}

impl RootPolicy {
    /// `os` 由入参传入，不是 cfg —— 三张表在开发机上都能取到、都能测。
    fn for_os(os: &str, homes: Vec<PathBuf>) -> Self {
        let windows_style = os == "windows";
        let (protected_roots, forbidden_subtrees): (Vec<String>, Vec<String>) = if windows_style {
            let mut roots: Vec<String> = WINDOWS_PROTECTED_ROOTS
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            // 盘符根一律保护：D:\、E:\ ... 防止规则解析到任一盘的根
            for drive in b'C'..=b'Z' {
                roots.push(format!("{}:\\", (drive as char).to_ascii_uppercase()));
            }
            (
                roots,
                WINDOWS_FORBIDDEN_SUBTREES
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            )
        } else if os == "macos" {
            (
                MACOS_PROTECTED_ROOTS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                MACOS_FORBIDDEN_SUBTREES
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            )
        } else {
            (
                LINUX_PROTECTED_ROOTS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                LINUX_FORBIDDEN_SUBTREES
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            )
        };
        Self {
            protected_roots,
            forbidden_subtrees,
            homes,
            windows_style,
        }
    }

    /// 空策略：证明共用校验逻辑不依赖根表完备性（CORE-06 的验收口径）。
    /// 只作测试夹具，生产二进制里不该存在这个构造入口。
    ///
    /// 刻意保留当前平台的分隔符规则与 homes：这两项不属于「根表」，清空
    /// 会让 `$HOME` 模板解析在 Windows 上必然错配，测到的只是环境噪声，
    /// 而不是「空表也不 fail-open」这件事本身。
    #[cfg(test)]
    fn empty() -> Self {
        let mut p = Self::for_os(std::env::consts::OS, protected_homes());
        p.protected_roots.clear();
        p.forbidden_subtrees.clear();
        p
    }
}

/// A-03：一律组件级比较，绝不用裸字符串前缀（CORE-07 的教训）。
/// 同时吃掉尾斜杠、重复分隔符、Windows 的反斜杠与大小写差异。
fn root_components(path: &str, windows_style: bool) -> Vec<String> {
    let normalized = if windows_style {
        path.replace('/', "\\")
    } else {
        path.to_string()
    };
    let sep = if windows_style { '\\' } else { '/' };
    normalized
        .split(sep)
        .filter(|c| !c.is_empty())
        .map(|c| {
            if windows_style {
                c.to_ascii_lowercase()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// `path` 是否等于 `root` 或落在其子树下（组件级）。
/// `!r.is_empty()` 很关键："/" 切完是空组件，若允许会匹配所有绝对路径。
fn path_is_under(path: &str, root: &str, windows_style: bool) -> bool {
    let p = root_components(path, windows_style);
    let r = root_components(root, windows_style);
    !r.is_empty() && p.len() >= r.len() && r.iter().zip(p.iter()).all(|(a, b)| a == b)
}

/// 三平台共用的受保护根本体判定（相等语义，纯函数）。
///
/// 刻意**不**改成前缀语义：`ops::sanitize_before_delete` 用它拦「待删路径
/// 就是 / 或 home 本身」，若把 `/Users/me/Library/Caches/foo` 也判成受保护
/// 根，正常缓存清理会被全拦。`canonical` 作为第二个候选一起比（macOS 原实现
/// 的口径），只可能让判定更严，不会更松。
fn is_protected_root_checked(path: &str, canonical: Option<&str>, policy: &RootPolicy) -> bool {
    if path.is_empty() {
        return true;
    }
    let candidates: Vec<Vec<String>> = std::iter::once(path)
        .chain(canonical)
        .map(|p| root_components(p, policy.windows_style))
        .collect();
    let forbidden: Vec<Vec<String>> = policy
        .protected_roots
        .iter()
        .map(|r| root_components(r, policy.windows_style))
        .chain(
            policy
                .homes
                .iter()
                .map(|h| root_components(&h.to_string_lossy(), policy.windows_style)),
        )
        .collect();
    candidates.iter().any(|c| forbidden.iter().any(|r| r == c))
}

/// 绝对路径判定也平台无关、由入参决定。
///
/// 不能直接用 `std::path::Path::is_absolute()`：在开发机（Unix）上喂
/// `C:\Windows\System32` 会返回 false，Windows 分支的测试就会「因为别的
/// 原因变绿」，测不到真正想测的子树拦截。
fn root_is_absolute(resolved: &str, windows_style: bool) -> bool {
    if windows_style {
        let b = resolved.as_bytes();
        (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/'))
            || resolved.starts_with("\\\\")
    } else {
        resolved.starts_with('/')
    }
}

fn current_root_policy() -> RootPolicy {
    RootPolicy::for_os(std::env::consts::OS, protected_homes())
}

/// 判断 path（已解析、规范化后）是否为受保护根本身
///
/// 供两个场景使用：
/// 1. 规则根校验（validate_rule_root）：解析后的根不能是这里任一值
/// 2. 删除前二次校验（ops::sanitize_before_delete）：待删路径不能是这里任一值
///
/// 平台差异全部收敛在 `current_root_policy()` 返回的三张表里；这里只剩
/// 一次 `canonicalize` IO，判定逻辑本身是上面那份无 cfg 的纯函数。
pub fn is_protected_root(path: &str) -> bool {
    let policy = current_root_policy();
    let canonical = std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    is_protected_root_checked(path, canonical.as_deref(), &policy)
}

/// 规则根组件数上限：防止"根模板过浅"通过校验
pub const MAX_RULE_ROOT_COMPONENTS: usize = 3;

/// 校验声明式规则的根模板，返回解析后的安全绝对路径（CORE-06 起三平台共用一份）。
///
/// fail-closed：任何一步失败都返回 Err，调用方必须拒绝该规则。
/// - 模板变量缺失/未知 → Err
/// - 解析后非绝对路径 → Err
/// - 解析后是受保护根本身 → Err
/// - 解析后落在系统受保护目录的子树下（CORE-06 后续）→ Err
/// - 解析后组件数 < 3（如 $HOME 本身、/ 等浅根直接拒绝）→ Err
///
/// 平台差异只由 `policy` 携带（三张根表 + 分隔符/大小写规则）。测试可以喂
/// `RootPolicy::for_os("linux" | "windows", ...)`，把「本机 0 编译」的分支
/// 在开发机上真跑一遍；也可以喂 `RootPolicy::empty()` 证明共用逻辑本身
/// 不依赖根表完备性。历史上 `#[cfg(not(any(macos, windows)))]` 另有一份
/// 只校验空串与 "/" 的实现，Linux 上 $HOME / 相对路径 / `..` 全部放行
/// —— 已删除，不再有第二份。
fn validate_rule_root_checked(root: &str, policy: &RootPolicy) -> Result<String, String> {
    // 模板变量解析必须跟随被测策略的 home 与平台分隔符规则，否则「拿 macOS
    // 策略测 $HOME」在 Windows 上会被本机 `C:\Users\...` 污染成必然失败。
    let home = policy
        .homes
        .first()
        .cloned()
        .or_else(|| protected_homes().into_iter().next())
        .ok_or_else(|| "无法确定用户主目录，规则根拒绝（fail-closed）".to_string())?;
    let resolved = resolve_root_variable_for(root, &home, policy.windows_style)
        .ok_or_else(|| format!("规则根含未解析模板变量或为空: {root}"))?;
    if !root_is_absolute(&resolved, policy.windows_style) {
        return Err(format!("规则根必须为绝对路径: {root}"));
    }
    if resolved.contains("..") || resolved.chars().any(|c| c.is_control()) {
        return Err(format!("规则根含目录遍历或控制字符: {root}"));
    }
    if is_protected_root_checked(&resolved, None, policy) {
        return Err(format!("规则根是受保护根本身: {root}"));
    }
    // CORE-06 后续：组件数达标（≥3）不代表安全 —— /usr/local/lib 恰好 3 个组件，
    // 却落在系统目录下。按组件级前缀再拦一道，绝不退化成字符串 starts_with。
    if let Some(hit) = policy
        .forbidden_subtrees
        .iter()
        .find(|r| path_is_under(&resolved, r, policy.windows_style))
    {
        return Err(format!("规则根落在系统受保护目录 {hit} 下: {root}"));
    }
    // macOS 用 '/'、Windows 用 '\'：两种分隔符都切开，组件数与旧两版逐一对齐
    //（macOS "$HOME/Library/Caches" = 4；Windows "C:\Users\Name" = 3）。
    let components = resolved
        .split(['/', '\\'])
        .filter(|c| !c.is_empty())
        .count();
    if components < MAX_RULE_ROOT_COMPONENTS {
        return Err(format!(
            "规则根过浅（{components} 个组件 < {MAX_RULE_ROOT_COMPONENTS}），拒绝: {root}"
        ));
    }
    Ok(resolved)
}

/// 生产入口：平台差异全部收敛在 `current_root_policy()` 返回的数据里。
pub fn validate_rule_root(root: &str) -> Result<String, String> {
    validate_rule_root_checked(root, &current_root_policy())
}

/// 记录删除日志到文件
///
/// 所有删除操作都会记录到 ~/.maclean/delete.log，可回溯审计
/// 检测路径是否被 macOS ACL deny 规则保护（如父目录带 `deny delete`）。
///
/// macOS 的 ACL deny 优先级高于一切（包括 root 的 sudo/Touch ID 授权），
/// 这类路径「任何权限都无法删除」，属于系统保护而非权限不足。
/// 用于把 Touch ID 提权后的 EPERM 失败归类为「ACL 保护」，避免误导用户反复授权。
#[cfg(target_os = "macos")]
pub fn path_is_acl_protected(path: &str) -> bool {
    // 只检查目标自身的扩展 ACL 是否含 deny 规则。
    // macOS 会给用户主目录 / Library / Caches 等父目录加
    // `0: group:everyone deny delete`，但该 deny 只阻止"删除目录本身"，
    // 不阻止删除目录内的子项（实测 owner 可正常删除其中缓存），
    // 因此不能沿祖先链逐层检查，否则会把所有子项误判为不可删除。
    dir_has_acl_deny(path)
}

/// 非 macOS 平台没有"扩展 ACL deny 优先于 root"这一概念：
/// Windows 的权限不足由删除时的 EPERM 失败归类兜住，这里一律判否。
#[cfg(not(target_os = "macos"))]
pub fn path_is_acl_protected(_path: &str) -> bool {
    false
}

/// 检测路径是否受系统级访问控制保护（如 TCC 隐私保护目录）。
/// 这类目录即使权限位正常、无 ACL deny，非系统进程也无法读取/遍历
/// （read_dir 报 EPERM），因此任何权限（含 root / Touch ID）都无法删除。
pub fn path_is_inaccessible(path: &str) -> bool {
    let p = std::path::Path::new(path);
    // 目录：能否列举子项决定能否删除（TCC 类系统保护连 ls 都 EPERM）
    if p.is_dir() {
        return std::fs::read_dir(p).is_err();
    }
    // 文件：能否打开读取决定能否删除（Preferences 下 plist 等）。
    // 注意：read_dir 对普通文件必然失败，绝不能用来探测文件 ——
    // 否则所有 plist 都会被误判为"系统保护不可删除"。
    if p.is_file() {
        return std::fs::File::open(p).is_err();
    }
    // 不存在或其他：fail-closed 视为不可访问（与旧行为一致）
    true
}

/// 检查单个目录的扩展 ACL 中是否含 deny 规则（`ls -lde` 输出解析）
#[cfg(target_os = "macos")]
fn dir_has_acl_deny(dir: &str) -> bool {
    let out = std::process::Command::new("/bin/ls")
        .args(["-lde", dir])
        .output();
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            // ACL 块形如 ` 0: group:everyone deny delete`
            text.lines().any(|l| {
                let t = l.trim();
                t.contains("deny") && t.contains(':')
            })
        }
        Err(_) => false,
    }
}

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

    /// 与生产判定同源的 home。Windows runner 上 `$HOME` 环境变量常常是
    /// `D:\Users\u` 之类，与 `dirs::home_dir()` / 注册表给出的真实 home 不同；
    /// 测试一旦用 `std::env::var("HOME")` 或硬编码 `/Users/u` 构造路径，
    /// 第 4 层与第 8 层的组件比较就会因为 home 不匹配而整片失效。
    fn test_home_str() -> String {
        protected_homes()
            .into_iter()
            .next()
            .expect("有 home")
            .to_string_lossy()
            .into_owned()
    }

    // ---------- 第 4.6 层: 清单管理目录保护 ----------

    #[test]
    fn monorepo_category_node_modules_is_precisely_exempted() {
        // 第 8 层来源准入按当前机器的 home 判定。硬编码 /Users/fengyu/... 在
        // CI（HOME=/Users/runner 或 /home/runner）必然落到「不在任何已声明
        // 扫描根下」，开发机却全绿 —— 典型的环境假绿。用生产同一份 homes 构造。
        let home = protected_homes().first().expect("有 home").clone();
        let project = home.join("Downloads/myproject/workspace/onlineStudy");
        let nm = project.join("node_modules");
        let nm_s = nm.to_string_lossy().to_string();
        // Monorepo依赖 类目的目标就是 node_modules（npm install 可再生、
        // 删除走废纸篓可恢复）—— 4.6 层精确豁免，删除不再"扫得出删不掉"。
        let r = check_path_safety_with_category(&nm_s, "Monorepo依赖");
        assert!(
            matches!(r, SafetyCheck::Safe),
            "Monorepo依赖 + node_modules 应放行: {:?}",
            r
        );
        // 子包 node_modules 同样放行
        let nm2 = project.join("voice-tutor-worker/node_modules");
        assert!(matches!(
            check_path_safety_with_category(&nm2.to_string_lossy(), "Monorepo依赖"),
            SafetyCheck::Safe
        ));
        // 精确性：豁免只匹配组件名为 node_modules 的目录；相似名
        // （node_modulesx）不命中清单标记，走普通路径、不享受豁免。
        let nm3 = project.join("node_modulesx");
        assert!(!canonical_has_component(&nm3, "node_modules"));
        assert!(canonical_has_component(&nm, "node_modules"));
        // 非 Monorepo 类目遇到 node_modules 仍无条件拦截（防重复文件误删）
        assert!(matches!(
            check_path_safety_with_category(&nm_s, "重复文件"),
            SafetyCheck::Danger(_)
        ));
        // Monorepo依赖 遇到其它清单目录（site-packages 等）仍拦截
        let sp = project.join(".venv/lib/python3.13/site-packages/x");
        assert!(matches!(
            check_path_safety_with_category(&sp.to_string_lossy(), "Monorepo依赖"),
            SafetyCheck::Danger(_)
        ));
    }

    #[test]
    #[ignore = "真实路径验证，手动运行（cargo test --bin maclean monorepo_real -- --ignored）"]
    fn monorepo_real_path_is_exempted() {
        // 端到端验证：用户机器上真实存在的 monorepo node_modules，
        // Monorepo依赖 类目下删除期判定必须放行（可删、走废纸篓）。
        for p in [
            "/Users/fengyu/Downloads/myproject/workspace/onlineStudy/node_modules",
            "/Users/fengyu/Downloads/myproject/workspace/onlineStudy/voice-tutor-worker/node_modules",
        ] {
            if !std::path::Path::new(p).exists() {
                continue;
            }
            let r = check_path_safety_with_category(p, "Monorepo依赖");
            assert!(
                matches!(r, SafetyCheck::Safe),
                "真实 monorepo node_modules 应放行: {} (got {:?})",
                p,
                r
            );
        }
    }

    #[test]
    fn container_data_paths_are_never_deletable() {
        // 容器数据目录（Docker Desktop / OrbStack 镜像与卷）是数据库结构，
        // 直接删文件破坏镜像元数据（9-30 事故）。任何类目任何入口都 Danger。
        let home = test_home_str();
        for p in [
            format!(
                "{}/Library/Containers/com.docker.docker/Data/vms/0/docker.raw",
                home
            ),
            format!("{}/Library/Containers/com.docker.docker/Data/cache", home),
            format!("{}/.orbstack/vm/0.img", home),
            format!(
                "{}/Library/Group Containers/HUAQ24HBR6.dev.orbstack/config.json",
                home
            ),
        ] {
            for cat in ["重复文件", "Docker虚拟机", "大文件", "安装包"] {
                let r = check_path_safety_with_category(&p, cat);
                assert!(
                    matches!(r, SafetyCheck::Danger(_)),
                    "容器数据目录应拒绝删除: {} ({}), got {:?}",
                    p,
                    cat,
                    r
                );
            }
        }
        // Docker 构建缓存（~/.docker/buildx/cache）不是容器数据目录，
        // 但即使被更早的用户关键目录层拦截也是既有保护行为，此处只确认
        // 容器闸门本身不误伤普通缓存路径（~/.npm 等常规可删缓存）。
        let npm_cache = format!("{}/.npm/_cacache/data", home);
        let r = check_path_safety_with_category(&npm_cache, "npm缓存");
        assert!(
            !matches!(r, SafetyCheck::Danger(_)),
            "普通缓存路径不应被容器闸门拦截: {:?}",
            r
        );
        // Docker 命令伪路径不拦（走命令删除通道）
        let cmd = "docker:system-prune";
        assert!(matches!(
            check_path_safety_with_category(cmd, "Docker清理"),
            SafetyCheck::Safe
        ));
        let orb = "orbstack:system-clean";
        assert!(matches!(
            check_path_safety_with_category(orb, "OrbStack清理"),
            SafetyCheck::Safe
        ));
    }

    #[test]
    fn manifest_managed_dirs_are_never_deletable() {
        // A-07b：本测试用的是假 home 路径，声明它为扫描根后才能检验
        // 「4.6/4.8 不误伤其它类目」；否则第 8 层准入会先把它拦下。
        crate::provenance::register_root(Path::new("/Users/u"));
        // 根因修复：delete.log 曾累计 350+ 条误删落在 site-packages、波及
        // 20+ 个 venv。清单管理目录在任何删除入口都必须被硬阻断。
        for p in [
            "/Users/u/proj/.venv/lib/python3.13/site-packages/x/y.bin",
            "/Users/u/proj/node_modules/pkg/b.bin",
            "/Users/u/a/b/dist-packages/c.bin",
            "/Users/u/ops/.terraform/e.bin",
            "/Users/u/gopath/go/pkg/mod/f.bin",
            "/Users/u/App.app/Contents/Resources/g.bin",
            "/Users/u/tools/.venv2/bin/h.bin",
        ] {
            let r = check_path_safety_with_category(p, "重复文件");
            assert!(
                matches!(r, SafetyCheck::Danger(_)),
                "应拒绝删除: {} (got {:?})",
                p,
                r
            );
        }
        // P0-1 4.8 层语义（取代旧的"普通路径不拒"回归）：重复文件类目只允许
        // 删除缓存目录内的副本 —— 缓存目录外的用户数据/项目文件/散落文件
        // 即使内容重复也一律拒绝（宁可少省空间也不误删）。
        for p in [
            "/Users/u/Downloads/a.bin",
            "/Users/u/proj/src/h.bin",
            "/Users/u/.venvista/i.bin",
        ] {
            let r = check_path_safety_with_category(p, "重复文件");
            assert!(
                matches!(r, SafetyCheck::Danger(_)),
                "非缓存目录在重复文件类目下应被拒绝: {} (got {:?})",
                p,
                r
            );
        }
        // 其它类目不受 4.6/4.8 影响（回归：保护不能误伤其它类目的正常路径）
        for p in ["/Users/u/proj/src/h.bin", "/Users/u/.venvista/i.bin"] {
            let r = check_path_safety_with_category(p, "App残留");
            assert!(
                !matches!(r, SafetyCheck::Danger(_)),
                "非重复文件类目不应被 4.6/4.8 误拦: {} (got {:?})",
                p,
                r
            );
        }
    }

    // ---------- 第 4.8 层: 重复文件类目目录白名单 + 扩展名保护 ----------

    #[test]
    fn duplicate_category_denies_non_cache_dir_without_git() {
        // 4.8 层兜底：无 git 的用户文档/项目外代码一旦进入删除队列必须被拦。
        // 目录白名单是"非缓存目录全拒"，不依赖 .git 祖先是否存在。
        let tmp = std::env::temp_dir().join(format!("maclean_safety_48a_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        crate::provenance::register_root(&tmp); // A-07b：声明本用例的扫描根
        let user_doc = tmp.join("docs/report.pdf");
        std::fs::create_dir_all(user_doc.parent().unwrap()).unwrap();
        std::fs::write(&user_doc, b"%PDF-1.4 fake").unwrap();
        let r = check_path_safety_with_category(user_doc.to_str().unwrap(), "重复文件");
        assert!(
            matches!(r, SafetyCheck::Danger(_)),
            "非缓存目录的重复候选必须被拒: {:?}",
            r
        );
        // 同一路径在其它类目不受 4.8 影响
        let r2 = check_path_safety_with_category(user_doc.to_str().unwrap(), "系统缓存");
        assert!(
            !matches!(r2, SafetyCheck::Danger(_)),
            "其它类目不应被 4.8 误拦: {:?}",
            r2
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn duplicate_category_allows_cache_dir_plain_file() {
        // 缓存目录内、无"用户数据扩展名"的副本允许删除（清理价值保留）。
        // is_repeat_cache_dir 用 protected_homes（真实 HOME）判定，
        // 测试路径必须以真实 home 构造，否则目录白名单命中不了。
        let home = test_home_str();
        // 4.8 目录白名单形态判定：
        let cache_p1 = Path::new(&home).join("Library/Caches/app/video.mp4");
        assert!(
            is_repeat_cache_dir(&cache_p1),
            "~/Library/Caches 应命中白名单"
        );
        let cache_p2 = Path::new(&home).join(".cache/tool/x.bin");
        assert!(is_repeat_cache_dir(&cache_p2), "~/.cache 应命中白名单");
        assert!(
            is_repeat_cache_dir(Path::new("/Library/Caches/sys/x.bin")),
            "/Library/Caches 应命中白名单"
        );
        let cache_p3 =
            Path::new(&home).join("Library/Containers/com.app/Data/Library/Caches/x.bin");
        assert!(is_repeat_cache_dir(&cache_p3), "沙盒容器缓存应命中白名单");
        let cache_p4 = Path::new(&home).join("Library/Application Support/app/x.bin");
        assert!(
            !is_repeat_cache_dir(&cache_p4),
            "App Support 不应命中白名单"
        );
        let cache_p5 = Path::new(&home).join("Downloads/x.bin");
        assert!(!is_repeat_cache_dir(&cache_p5), "Downloads 不应命中白名单");
        // mp4 不在不可再生扩展名列表 → 缓存目录内可删（无 Danger 命中即非 Danger）
        let cache_p = Path::new(&home)
            .join("Library/Caches/app/video.mp4")
            .to_string_lossy()
            .into_owned();
        let r = check_path_safety_with_category(&cache_p, "重复文件");
        assert!(
            !matches!(r, SafetyCheck::Danger(_)),
            "缓存目录内普通媒体副本应允许删除: {:?}",
            r
        );
    }

    #[test]
    fn duplicate_category_denies_user_doc_extension_in_cache() {
        // 缓存目录内命中"用户数据扩展名"（文档/代码/数据库/压缩包/字体）
        // 也拒绝 —— 极可能是同步/备份工具误放的用户数据。
        let home = test_home_str();
        let cases = [
            ("report.pdf", "pdf"),
            ("code.ts", "ts"),
            ("data.json", "json"),
            ("db.sqlite", "sqlite"),
            ("backup.zip", "zip"),
            ("font.ttf", "ttf"),
            ("REPORT.DOCX", "DOCX 大写"),
        ];
        for (fname, label) in cases {
            let p = Path::new(&home)
                .join(format!("Library/Caches/x/{}", fname))
                .to_string_lossy()
                .into_owned();
            let r = check_path_safety_with_category(&p, "重复文件");
            assert!(
                matches!(r, SafetyCheck::Danger(_)),
                "缓存目录内 {} 也应被拒: {} (got {:?})",
                label,
                p,
                r
            );
        }
    }

    #[test]
    fn candidate_allowed_equals_layer48_complement() {
        // 扫描允许判定（duplicate_candidate_allowed）与删除期 4.8 层必须
        // 精确互补：allowed=false ⟺ 重复文件类目 Danger。扫描按它过滤，
        // 删除按它拦截 —— "扫得出的都能删、删不掉的都不扫"。
        let home = test_home_str();
        // 缓存目录内、无用户扩展名 → allowed，删除期不 Danger
        let ok = Path::new(&home)
            .join("Library/Caches/app/video.mp4")
            .to_string_lossy()
            .into_owned();
        assert!(duplicate_candidate_allowed(Path::new(&ok)));
        assert!(!matches!(
            check_path_safety_with_category(&ok, "重复文件"),
            SafetyCheck::Danger(_)
        ));
        // 缓存目录内、命中不可再生扩展名 → 不允许，删除期 Danger
        let code = Path::new(&home)
            .join("Library/Caches/x/code.ts")
            .to_string_lossy()
            .into_owned();
        assert!(!duplicate_candidate_allowed(Path::new(&code)));
        assert!(matches!(
            check_path_safety_with_category(&code, "重复文件"),
            SafetyCheck::Danger(_)
        ));
        // 非缓存目录 → 不允许，删除期 Danger
        let user = Path::new(&home)
            .join("Documents/report.pdf")
            .to_string_lossy()
            .into_owned();
        assert!(!duplicate_candidate_allowed(Path::new(&user)));
        assert!(matches!(
            check_path_safety_with_category(&user, "重复文件"),
            SafetyCheck::Danger(_)
        ));
    }

    #[test]
    fn git_cache_matches_uncached_judgement() {
        // P2-2：缓存版判定与无缓存版一致（缓存只是 IO 优化，不能改变结论）。
        let tmp = std::env::temp_dir().join(format!("maclean_safety_48c_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("p/x")).unwrap();
        std::fs::write(tmp.join("p/.git"), b"").unwrap();
        let homes = vec![tmp.clone()];
        let in_proj = tmp.join("p/x/a.bin");
        let outside = tmp.join("other/a.bin");
        // 第一次（写缓存）
        assert!(is_inside_git_project_cached(&in_proj, &homes));
        assert!(!is_inside_git_project_cached(&outside, &homes));
        // 第二次（读缓存）
        assert!(is_inside_git_project_cached(&in_proj, &homes));
        assert!(!is_inside_git_project_cached(&outside, &homes));
        // 与无缓存版一致
        assert_eq!(
            is_inside_git_project_cached(&in_proj, &homes),
            is_inside_git_project(&in_proj, &homes)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn git_project_worktree_never_deleted_in_duplicate_category() {
        // 第 4.7 层：重复文件类目删除 git 项目工作区文件必须被硬阻断。
        // 历史事故：onlineStudy 21 个 + 9-30 误删 76 个（用户文档/镜像/扩展）。
        // 即使扫描结果来自旧缓存/旧版本，删除期也拦得住。
        let tmp = std::env::temp_dir().join(format!("maclean_safety_git_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        crate::provenance::register_root(&tmp); // A-07b：声明本用例的扫描根
        let proj = tmp.join("proj");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::write(proj.join(".git"), b"").unwrap();
        let f = proj.join("src/data.ts");
        std::fs::write(&f, b"x").unwrap();
        let r = check_path_safety_with_category(f.to_str().unwrap(), "重复文件");
        assert!(
            matches!(r, SafetyCheck::Danger(_)),
            "重复文件类目必须拒绝项目工作区文件: {:?}",
            r
        );
        // 其它类目不启用 4.7：项目文件本身不是危险路径，由各自类目规则负责
        let r2 = check_path_safety_with_category(f.to_str().unwrap(), "App残留");
        assert!(
            !matches!(r2, SafetyCheck::Danger(_)),
            "其它类目不应被 4.7 误拦: {:?}",
            r2
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn git_project_detection_stops_at_home() {
        let tmp = std::env::temp_dir().join(format!("maclean_safety_git2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("p/x")).unwrap();
        std::fs::write(tmp.join("p/.git"), b"").unwrap();
        let homes = vec![tmp.clone()];
        assert!(is_inside_git_project(&tmp.join("p/x/a.bin"), &homes));
        assert!(!is_inside_git_project(&tmp.join("other/a.bin"), &homes));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn manifest_and_candidate_collection_share_one_judgement() {
        // is_manifest_managed_path 是唯一判定源：候选收集（dup_files）与
        // 删除阶段（safety 4.6）必须走同一函数，防止两处规则漂移。
        let paths = [
            "/Users/u/.venv/lib/python3.13/site-packages/pkg/a.bin",
            "/Users/u/proj/node_modules/pkg/b.bin",
            "/Users/u/gopath/go/pkg/mod/f.bin",
        ];
        for p in paths {
            assert!(
                is_manifest_managed_path(Path::new(p)),
                "判定源应命中: {}",
                p
            );
        }
    }

    #[test]
    fn manifest_markers_cover_dotless_venv_and_virtualenvs_roots() {
        // python -m venv venv / ~/.virtualenvs / Poetry 的 virtualenvs/ 都是
        // 事故后补上的常见形态；此前只认 .venv* 导致这些树整棵失防。
        for p in [
            "/Users/u/proj/venv/lib/python3.11/site-packages/x.bin",
            "/Users/u/.virtualenvs/envA/bin/y.bin",
            "/Users/u/proj/envs/../virtualenvs/envB/z.bin",
        ] {
            assert!(
                is_manifest_managed_path(Path::new(p)),
                "判定源应命中: {}",
                p
            );
        }
        // 名字里带 venv 字样的普通目录不受影响（virtualenvs 是精确组件匹配）
        assert!(!is_manifest_managed_path(Path::new(
            "/Users/u/venvtools/a.bin"
        )));
    }

    #[test]
    fn deleting_ancestor_of_manifest_tree_is_blocked() {
        // 4.6 层的历史缺口：被删路径自身不含标记组件、但整棵树是清单树。
        // 复现事故同型场景：~/Library/Caches/pypoetry 下挂 virtualenvs/。
        let tmp = std::env::temp_dir().join(format!("maclean_46_{}", std::process::id()));
        let venv_root = tmp.join("virtualenvs/proj-py311");
        std::fs::create_dir_all(venv_root.join("lib/python3.11/site-packages/pkg")).unwrap();
        std::fs::write(venv_root.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        assert!(
            contains_manifest_managed_descendant(&tmp),
            "祖先目录含 virtualenvs/pyvenv.cfg 必须被识别"
        );

        // 纯缓存目录不受影响；含 .app bundle 的目录刻意不拦（卸载主路径）
        let clean = tmp.join("clean_cache");
        std::fs::create_dir_all(clean.join("some/app/Contents/MacOS")).unwrap();
        assert!(!contains_manifest_managed_descendant(&clean));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn app_support_data_root_boundaries() {
        use std::path::PathBuf;
        let h = PathBuf::from("/Users/tester");
        let f =
            |rel: &str| app_support_data_root_with_homes(&h.join(rel), std::slice::from_ref(&h));
        // Application Support 的直接子项才识别为应用数据根
        assert!(f("Library/Application Support/JetBrains"));
        assert!(
            f("Library/Application Support/TRAE SOLO CN"),
            "含空格 App 名"
        );
        // 根本身、再深一层、Toolbox 深层、其它 home 子树都不放行
        assert!(
            !f("Library/Application Support"),
            "Application Support 根不放行"
        );
        assert!(
            !f("Library/Application Support/JetBrains/PyCharmCE2025.2"),
            "嵌套不放行"
        );
        assert!(
            !f("Library/Application Support/JetBrains/Toolbox/apps"),
            "深层不放行"
        );
        assert!(!f("Library/Caches/pypoetry"), "Caches 不是应用数据根");
        assert!(!f("Documents/proj"), "其它 home 子树不是");
        assert!(!f("Library/Containers/com.x/Data"), "沙盒容器本次不放开");
    }

    #[test]
    fn orphan_im_documents_root_allowed_but_other_apps_docs_root_blocked() {
        use std::path::PathBuf;
        // 准入表按 home 前缀比对，硬编码 Unix home 在 Windows 上永远不命中。
        let h = PathBuf::from(test_home_str());
        let wl = |rel: &str| check_whitelist(&h.join(rel), &h, "应用缓存");
        // IM（微信 / QQ / 企微）Documents 根：App 已卸载后的孤儿数据可清理
        assert!(wl(
            "Library/Containers/com.tencent.xinWeChat/Data/Documents"
        ));
        assert!(wl("Library/Containers/com.tencent.qq/Data/Documents"));
        assert!(wl(
            "Library/Containers/com.tencent.WeWorkMac/Data/Documents"
        ));
        // IM Documents 内的子项：尾斜杠规则，保持原有放行
        assert!(wl(
            "Library/Containers/com.tencent.xinWeChat/Data/Documents/x/video.dat"
        ));
        // 非 IM 沙盒 App 的 Documents 根不放行（最小权限，避免扩大删除面）
        assert!(!wl("Library/Containers/com.some.editor/Data/Documents"));
        // IM 容器的 Data 层本身、其它层级不放行
        assert!(!wl("Library/Containers/com.tencent.qq/Data"));
    }

    #[test]
    fn app_support_root_bundled_runtime_not_blocked_but_redlines_hold() {
        use std::path::Path;
        let home = test_home_str();
        let pid = std::process::id();

        // 1) App 根内捆绑、仅与虚拟环境同名的目录（PyCharm typeshed 存根 venv）
        //    不应锁死整个应用数据根 —— 复现 JetBrains 4.1GB 无法清理的误判。
        let app = Path::new(&home).join(format!(
            "Library/Application Support/maclean_test_app_{pid}"
        ));
        let stub = app.join("plugins/python-ce/helpers/typeshed/stdlib/venv");
        std::fs::create_dir_all(&stub).unwrap();
        std::fs::write(stub.join("__init__.pyi"), "").unwrap();
        let ap = app.to_string_lossy().into_owned();
        let r1 = check_path_safety_with_category(&ap, "maclean_test_app");
        assert!(
            !matches!(r1, SafetyCheck::Danger(_)),
            "应用自带类型存根(venv 同名)不应误拦整个 App 根: {:?}",
            r1
        );
        let _ = std::fs::remove_dir_all(&app);

        // 2) 豁免只挂在第 4.6 层：更早的第 4 层 home 黑名单不能被它盖过。
        //    通讯录（macOS 表）形状上同样是「Application Support 直接子项」，
        //    正好与 4.6 豁免重叠；Windows 表用凭据管理器作对应红线。
        //    两套表都由入参注入、在开发机上真跑，不是 cfg 跳过（Windows runner
        //    上原本只有 4.6 误拦让这条断言碰巧变绿，修好误拦后才暴露出来）。
        let protected = Path::new(&home).join("Library/Application Support/AddressBook");
        assert!(
            is_app_support_data_root(&protected),
            "通讯录目录形状上也是 Application Support 直接子项"
        );
        let homes = protected_homes();
        for windows in [false, true] {
            let (p, label) = if windows {
                (
                    Path::new(&home).join("AppData/Local/Microsoft/Credentials"),
                    "Windows 凭据管理器",
                )
            } else {
                (protected.clone(), "通讯录")
            };
            let s = p.to_string_lossy().into_owned();
            assert!(
                check_home_paths_for_platform(&p, &s, &homes, windows).is_some(),
                "{label} 必须在第 4 层被拦（windows={windows}）"
            );
        }
        // 生产入口：当前平台自己的红线目录仍必须返回 Danger。
        let (pp, label) = if cfg!(target_os = "windows") {
            (
                Path::new(&home)
                    .join("AppData/Local/Microsoft/Credentials")
                    .to_string_lossy()
                    .into_owned(),
                "Credentials",
            )
        } else {
            (protected.to_string_lossy().into_owned(), "AddressBook")
        };
        let r2 = check_path_safety_with_category(&pp, label);
        assert!(
            matches!(r2, SafetyCheck::Danger(_)),
            "系统受保护目录必须在 4.6 之前的层被拦: {:?}",
            r2
        );

        // 3) 非应用数据根（Caches 下祖先含虚拟环境）仍受 4.6 层保护。
        let cache = Path::new(&home).join(format!("Library/Caches/maclean_test_venv_{pid}"));
        std::fs::create_dir_all(cache.join("some/virtualenvs/proj")).unwrap();
        let cp = cache.to_string_lossy().into_owned();
        let r3 = check_path_safety_with_category(&cp, "maclean_test_venv");
        assert!(
            matches!(r3, SafetyCheck::Danger(_)),
            "Caches 祖先含虚拟环境仍应拦截: {:?}",
            r3
        );
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn reclaimable_cache_roots_whitelist_boundaries() {
        use std::path::PathBuf;
        let h = PathBuf::from("/Users/tester");
        let f =
            |rel: &str| reclaimable_cache_root_with_homes(&h.join(rel), std::slice::from_ref(&h));

        // 公认可再生缓存根：放行（内部含 node_modules/site-packages 也不拦）
        assert!(f(".npm"), "~/.npm 必须放行（npm cache clean）");
        assert!(f(".cache/pip"));
        assert!(f("Library/Caches/pip"));
        assert!(f("Library/Caches/Yarn"));
        assert!(f(".cache/yarn"));
        assert!(f("Library/pnpm/store"));
        assert!(f(".local/share/pnpm/store"));
        // Squirrel 更新器残留：仅 Caches 直接子项、.ShipIt 结尾
        assert!(f("Library/Caches/com.microsoft.VSCode.ShipIt"));

        // 真实虚拟环境宿主 / 普通目录：不在名单，继续受后代深扫保护
        assert!(!f("Library/Caches/pypoetry/virtualenvs"));
        assert!(!f("Library/Caches/pypoetry"));
        // .ShipIt 必须是 Caches 直接子项；嵌套或其它后缀不放行
        assert!(!f("Library/Caches/com.microsoft.VSCode.ShipIt/sub"));
        assert!(!f("Library/Caches/SomeShipIt"));
        assert!(!f("Downloads/x.ShipIt"));
        // home 之外的同名目录不放行
        assert!(!reclaimable_cache_root_with_homes(
            &PathBuf::from("/Volumes/External/.npm"),
            std::slice::from_ref(&h)
        ));
    }

    #[test]
    fn kube_cache_reclaimable_but_credentials_still_protected() {
        let h = PathBuf::from("/Users/tester");
        let f = |rel: &str| {
            let p = h.join(rel);
            let s = p.to_string_lossy().to_string();
            check_home_paths_for_platform(&p, &s, std::slice::from_ref(&h), false)
        };

        // kubectl 发现缓存：整目录可再生，放行进后续层
        assert!(f(".kube/cache").is_none(), "~/.kube/cache 应放行");
        // 凭证与目录本身：必须继续被第 4 层拦
        assert!(f(".kube").is_some(), "~/.kube 目录本身仍受保护");
        assert!(f(".kube/config").is_some(), "~/.kube/config 凭证必须保护");
        assert!(
            f(".kube/cache/discovery/apps.json").is_some(),
            "缓存目录更深层路径不做前缀放行，仍由保护层兜底"
        );
        assert!(
            f(".kube/cache2").is_some(),
            "只有精确 .kube/cache 放行，近似目录不允许"
        );
    }

    // ---------- P0: 规则根模板校验 + 受保护根判定 ----------

    #[test]
    fn rule_root_validation_accepts_normal_cache_roots() {
        // $HOME/Library/Caches 这类正常规则根必须通过
        let resolved = validate_rule_root("$HOME/Library/Caches").expect("正常规则根应通过校验");
        assert!(
            root_is_absolute(&resolved, cfg!(target_os = "windows")),
            "解析后的根应为绝对路径: {resolved}"
        );
        assert!(resolved.replace('\\', "/").ends_with("Library/Caches"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn acl_protected_path_detection() {
        let dir = std::env::temp_dir().join(format!("maclean_acl_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let add = std::process::Command::new("/bin/chmod")
            .args(["+a", "everyone deny delete"])
            .arg(&dir)
            .output()
            .unwrap();
        assert!(add.status.success(), "chmod +a 应成功");

        assert!(
            path_is_acl_protected(&dir.to_string_lossy()),
            "目标自身带 deny delete 时应被识别为 ACL 保护"
        );

        let _ = std::process::Command::new("/bin/chmod")
            .args(["-a", "everyone deny delete"])
            .arg(&dir)
            .output();
        assert!(
            !path_is_acl_protected(&dir.to_string_lossy()),
            "移除 ACL 后不应再被识别"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inaccessible_path_detection() {
        // 不存在的路径应视为不可访问（read_dir 失败）
        let missing = std::env::temp_dir().join(format!("maclean_missing_{}", std::process::id()));
        assert!(path_is_inaccessible(&missing.to_string_lossy()));
        // 存在的普通目录应可访问
        let dir = std::env::temp_dir().join(format!("maclean_ok_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert!(!path_is_inaccessible(&dir.to_string_lossy()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inaccessible_path_detection_does_not_misjudge_regular_files() {
        // 回归：read_dir 对普通文件必然失败 —— 老实现会把所有 plist 文件
        // 误判为"系统保护不可删除"，导致 Preferences 下几十项全变灰色。
        let file = std::env::temp_dir().join(format!("maclean_file_ok_{}", std::process::id()));
        let _ = std::fs::write(&file, b"x");
        assert!(
            !path_is_inaccessible(&file.to_string_lossy()),
            "普通可读文件不应被判定为系统保护"
        );
        let _ = std::fs::remove_file(&file);
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
        let home_str = homes
            .first()
            .expect("有 home")
            .to_string_lossy()
            .to_string();
        // os 由入参传入（不是 cfg）：macOS / Linux 两张表在开发机与 Linux
        // runner 上都能真跑。/Library 只是 macOS 根表里的项，拿它当跨平台
        // 通用断言必然在 Linux 假红。
        let macos = RootPolicy::for_os("macos", homes.clone());
        for bad in ["/", "/Library", "/System", "/Users", "/usr"] {
            assert!(
                is_protected_root_checked(bad, None, &macos),
                "macos 应识别为受保护根: {bad}"
            );
        }
        let linux = RootPolicy::for_os("linux", homes.clone());
        for bad in ["/", "/etc", "/usr", "/bin", "/home", "/var"] {
            assert!(
                is_protected_root_checked(bad, None, &linux),
                "linux 应识别为受保护根: {bad}"
            );
        }
        // home 本体在两种策略下都受保护
        for policy in [&macos, &linux] {
            assert!(
                is_protected_root_checked(&home_str, None, policy),
                "home 本体应识别为受保护根: {home_str}"
            );
        }
        // 普通可删路径不是受保护根（相等语义，不能扩成前缀）
        assert!(!is_protected_root_checked(
            "/Users/me/Library/Caches/foo",
            None,
            &macos
        ));
        assert!(!is_protected_root_checked(
            &format!("{home_str}/Library/Caches/foo"),
            None,
            &macos
        ));
        // 生产入口（当前平台）再直打一次，防止纯函数与接线脱节
        assert!(is_protected_root(&home_str), "生产入口应拦下 home 本体");
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

    // ---------- CORE-06：三平台共用一份校验，Linux 不再 fail-open ----------

    #[test]
    fn c06_shared_validator_holds_with_empty_root_policy() {
        // 用「一张根表都没有」的空策略喂共用实现：三个历史 fail-open 用例
        // 仍必须被拒绝 —— 证明防护不依赖受保护根表的完备性，且这条逻辑
        // 在开发机（macOS）上真跑真测。本轮收敛后策略改为结构体而非闭包。
        let weak = RootPolicy::empty();
        let home = protected_homes()
            .into_iter()
            .next()
            .expect("有 home")
            .to_string_lossy()
            .into_owned();
        for bad in [
            "$HOME".to_string(),
            "Library/Caches".to_string(),
            format!("{home}/Library/../.."),
        ] {
            assert!(
                validate_rule_root_checked(&bad, &weak).is_err(),
                "共用实现在空策略下仍应拒绝: {bad}"
            );
        }
        // 正常规则根不该被误伤
        assert!(validate_rule_root_checked("$HOME/Library/Caches", &weak).is_ok());
        // 生产入口与共用实现同口径（报告要求的 $HOME 验收）
        assert!(validate_rule_root("$HOME").is_err(), "$HOME 本身不应通过");
    }

    // ---------- CORE-06 后续：受保护根表三平台彻底收敛 ----------

    #[test]
    fn rp_three_platform_policies_are_distinct_and_reachable() {
        // os 是入参，不是 cfg：三张表在开发机（macOS）上都能真取真测。
        let mac = RootPolicy::for_os("macos", vec![]);
        let win = RootPolicy::for_os("windows", vec![]);
        let linux = RootPolicy::for_os("linux", vec![]);
        assert!(!mac.windows_style && !linux.windows_style);
        assert!(win.windows_style);

        // Linux 不再退化成「只认空串和 /」：系统根表必须是实打实的一张表。
        assert!(
            linux.protected_roots.len() >= 20,
            "Linux 受保护根表仍是弱化实现: {:?}",
            linux.protected_roots
        );
        assert!(linux.forbidden_subtrees.contains(&"/usr".to_string()));
        assert!(linux.protected_roots.contains(&"/usr/local".to_string()));
        // 三张表的子树名单必须各自独立，不能沿用 macOS 那份
        assert!(mac
            .forbidden_subtrees
            .contains(&"/Applications".to_string()));
        assert!(!linux
            .forbidden_subtrees
            .contains(&"/Applications".to_string()));
        assert!(win.forbidden_subtrees.contains(&"C:\\Windows".to_string()));
        // 盘符根只挡本体，绝不进 forbidden_subtrees
        assert!(win.protected_roots.contains(&"Z:\\".to_string()));
        assert!(!win.forbidden_subtrees.contains(&"C:\\".to_string()));
    }

    #[test]
    fn rp_linux_rejects_three_component_system_roots() {
        let linux = RootPolicy::for_os("linux", vec![]);
        // 报告点名的 fail-open：/usr/local/lib 组件数恰好 3，旧实现只查
        // 「不是受保护根本体 + 组件数 ≥ 3」就会放行。
        for bad in [
            "/usr/local/lib",
            "/usr/local",
            "/usr/lib/debug",
            "/var/cache/apt",
            "/opt/homebrew/lib",
            "/etc/opt/foo",
            "/boot/efi/EFI",
            "/root/.cache",
            "/tmp/work/cache",
        ] {
            assert!(
                validate_rule_root_checked(bad, &linux).is_err(),
                "Linux 不应允许把规则根设在系统目录下: {bad}（得到 {:?}）",
                validate_rule_root_checked(bad, &linux)
            );
        }
        // home 容器根只挡本体：用户缓存根全在 /home 下，不能被一刀切
        assert!(validate_rule_root_checked("/home", &linux).is_err());
        assert!(validate_rule_root_checked("/home/me/.cache/pip", &linux).is_ok());
    }

    #[test]
    fn rp_subtree_guard_is_component_level_not_string_prefix() {
        let linux = RootPolicy::for_os("linux", vec![]);
        // /usrEvil 是 /usr 的同名前缀兄弟目录，组件级判定不能误伤
        assert!(validate_rule_root_checked("/usrEvil/lib/cache", &linux).is_ok());
        assert!(validate_rule_root_checked("/usre/lib/cache", &linux).is_ok());
        assert!(!path_is_under("/usrEvil/lib", "/usr", false));
        assert!(path_is_under("/usr/local/lib", "/usr", false));
    }

    #[test]
    fn rp_subtree_guard_reverse_check_is_not_fake_green() {
        // 反向验证：把 forbidden_subtrees 清空，/usr/local/lib 必须立刻
        // 变成 Ok —— 否则上面那些「is_err()」可能只是被组件数或别的原因挡住。
        let mut linux = RootPolicy::for_os("linux", vec![]);
        assert!(validate_rule_root_checked("/usr/local/lib", &linux).is_err());
        linux.forbidden_subtrees.clear();
        assert!(
            validate_rule_root_checked("/usr/local/lib", &linux).is_ok(),
            "清空子树名单后仍被拒，说明拦截来自别处，c11 的断言是假绿"
        );
    }

    #[test]
    fn rp_windows_policy_rejects_system_subtree_but_not_user_cache() {
        let win = RootPolicy::for_os("windows", vec![]);
        assert!(validate_rule_root_checked(r"C:\Windows\System32\drivers", &win).is_err());
        assert!(validate_rule_root_checked(r"C:\Program Files\Common Files", &win).is_err());
        // 大小写不敏感 + 正斜杠也要能识别
        assert!(validate_rule_root_checked("c:/windows/temp", &win).is_err());
        // 用户缓存根不能被盘符根 / C:\Users 误伤
        assert!(validate_rule_root_checked(r"C:\Users\me\AppData\Local\Cache", &win).is_ok());
        // Windows 绝对路径判定必须由入参决定，不能依赖开发机的 Path::is_absolute
        assert!(root_is_absolute(r"D:\Data\Cache", true));
        assert!(!root_is_absolute("Data/Cache", true));
    }

    #[test]
    fn rp_macos_policy_rejects_system_subtree_but_not_home_cache() {
        // 注入 macOS 风格 home：模板解析不再读本机 HOME，Windows runner 上
        // 也能真跑 macOS 分支（否则 $HOME 会被解析成 C:\Users\... 而必然错配）。
        let mac = RootPolicy::for_os("macos", vec![PathBuf::from("/Users/tester")]);
        assert!(validate_rule_root_checked("/System/Library/Caches", &mac).is_err());
        assert!(validate_rule_root_checked("/Library/Caches/Homebrew", &mac).is_err());
        // 内置 44 条规则的根全在 $HOME 下，不能被 /Users 误伤
        assert!(validate_rule_root_checked("$HOME/Library/Caches", &mac).is_ok());
    }

    #[test]
    fn rp_is_protected_root_stays_equality_only() {
        // 相等语义是删除闸门的契约：ops::sanitize_before_delete 用它拦
        // 「待删路径就是 / 或 home 本身」。扩大成前缀会把正常清理全拦。
        let linux = RootPolicy::for_os("linux", vec![]);
        assert!(is_protected_root_checked("", None, &linux));
        assert!(is_protected_root_checked("/", None, &linux));
        assert!(is_protected_root_checked("/usr", None, &linux));
        assert!(!is_protected_root_checked("/usr/local/lib", None, &linux));
        assert!(!is_protected_root_checked(
            "/home/me/.cache/pip",
            None,
            &linux
        ));
        // canonicalize 作为第二候选：只可能让判定更严
        assert!(is_protected_root_checked(
            "/some/link",
            Some("/usr"),
            &linux
        ));
        // home 候选由入参注入，不依赖进程环境
        let with_home = RootPolicy::for_os("linux", vec![PathBuf::from("/home/me")]);
        assert!(is_protected_root_checked("/home/me", None, &with_home));
        assert!(!is_protected_root_checked(
            "/home/me/.cache",
            None,
            &with_home
        ));
    }

    #[test]
    fn rp_production_entry_rejects_three_component_system_root() {
        // 纯逻辑先跑三张表：Windows 分支在 macOS 开发机上同样真跑，避免
        // 「喂 Unix 绝对路径给 Windows 生产入口、因为非绝对而假红」。
        let cases: &[(&str, &str, &str)] = &[
            ("macos", "/usr/local/lib", "/usrEvil/lib/cache"),
            ("linux", "/usr/local/lib", "/usrEvil/lib/cache"),
            (
                "windows",
                r"C:\Windows\System32",
                r"C:\WindowsEvil\lib\cache",
            ),
        ];
        for &(os, forbidden, benign) in cases {
            let policy = RootPolicy::for_os(os, vec![]);
            assert!(
                validate_rule_root_checked(forbidden, &policy).is_err(),
                "{os} 策略仍允许把规则根设在系统目录的子树下"
            );
            assert!(
                validate_rule_root_checked(benign, &policy).is_ok(),
                "{os} 同名前缀兄弟不应被误伤: {benign}"
            );
        }
        // 生产入口：按当前平台取等价样本，确认接线没退化（报告点名的
        // Linux fail-open 在 macOS 根表同样含 /usr，也必须被挡住）。
        let (bad, ok) = if cfg!(target_os = "windows") {
            (r"C:\Windows\System32", r"C:\WindowsEvil\lib\cache")
        } else {
            ("/usr/local/lib", "/usrEvil/lib/cache")
        };
        assert!(validate_rule_root(bad).is_err());
        assert!(validate_rule_root(ok).is_ok());
        // 用户缓存根不能因这次收紧被误伤
        assert!(validate_rule_root("$HOME/Library/Caches").is_ok());
    }

    #[test]
    fn rp_production_entries_are_wired_to_shared_policy() {
        // 源码级接线断言：按 \nmod tests 切掉测试段，避免自命中。
        let src = include_str!("safety.rs");
        let prod = src.split("\nmod tests").next().unwrap_or(src);
        assert!(
            prod.contains("is_protected_root_checked(path, canonical.as_deref(), &policy)"),
            "is_protected_root 没有委托给共用纯函数"
        );
        assert!(
            prod.contains("validate_rule_root_checked(root, &current_root_policy())"),
            "validate_rule_root 没有用可注入策略"
        );
        // 三份 #[cfg] 实现必须真的消失，而不是加了一份共用实现却留着旧分支
        let cfg_split = format!(
            "#[cfg(not(any(target_os = \"macos\", target_os = \"windows\")))]\n{} is_protected_root",
            "pub fn"
        );
        assert!(
            !prod.contains(&cfg_split),
            "仍存在 Linux 专用的 is_protected_root cfg 分身"
        );
        assert!(
            !prod.contains("path.is_empty() || path == \"/\"\n}"),
            "Linux 弱化实现仍在生产段里"
        );
    }

    // ---------- CORE-07：is_direct_child 组件级边界 ----------

    #[test]
    fn c07_direct_child_rejects_sibling_with_same_string_prefix() {
        // A-03：裸字符串前缀会把 /Users/fooEvil 当成 /Users/foo 的直接子项。
        assert!(!is_direct_child("/Users/fooEvil", "/Users/foo"));
        assert!(!is_direct_child("/Users/foo", "/Users/foo"));
        assert!(!is_direct_child("/Users/foo/a/b", "/Users/foo"));
        assert!(is_direct_child("/Users/foo/Downloads", "/Users/foo"));
        // 调用点用的是带尾斜杠的前缀（Containers / HTTPStorages 等），
        // 换实现后语义必须保持一致
        assert!(is_direct_child(
            "/Users/foo/Library/Containers/com.x",
            "/Users/foo/Library/Containers/"
        ));
        assert!(!is_direct_child(
            "/Users/foo/Library/ContainersX",
            "/Users/foo/Library/Containers/"
        ));
        // 第 8 层白名单：大文件类目不得靠兄弟前缀绕过来源准入
        assert!(!check_whitelist(
            std::path::Path::new("/Users/fooEvil"),
            std::path::Path::new("/Users/foo"),
            "大文件"
        ));
    }

    #[test]
    fn c07_large_file_category_cannot_escape_home_end_to_end() {
        let home = protected_homes().into_iter().next().expect("有 home");
        let evil = format!("{}Evil", home.to_string_lossy());
        // 路径不存在 → canonicalize 回退到原字符串，正好模拟被污染的
        // 清单 / 缓存项。修复前会在第 8 层被 is_direct_child 放行成 Safe。
        let r = check_path_safety_with_category(&evil, "大文件");
        let verdict = match &r {
            SafetyCheck::Safe => "Safe",
            SafetyCheck::Warning(_) => "Warning",
            SafetyCheck::Danger(_) => "Danger",
        };
        assert!(
            matches!(r, SafetyCheck::Danger(_)),
            "大文件类目把 home 外的同名前缀兄弟目录放行了: {evil}（判定={verdict}）"
        );
    }

    #[test]
    fn c06_only_one_rule_root_validator_is_wired() {
        // 反向验证：有人再按 cfg 复制出第二份 `pub fn validate_rule_root`，
        // 这条断言必红 —— 那份弱化实现正是 CORE-06 的根因。
        let src = include_str!("safety.rs");
        let prod = src.split("\nmod tests").next().unwrap_or(src);
        assert_eq!(
            prod.matches("pub fn validate_rule_root(").count(),
            1,
            "规则根校验出现第二份平台实现，Linux fail-open 回归"
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

        // 平台差异由入参注入：macOS 与 Windows 两张黑名单在开发机上都要真跑。
        // 原先直接调生产入口 `check_home_paths`（按当前 OS 选表），Windows runner
        // 上喂 macOS 形态的路径必然不命中 —— 那测的是环境，不是「遍历每份 home」。
        for windows in [false, true] {
            let exact: &[&str] = if windows {
                &["AppData/Local", "AppData/Roaming", "OneDrive"]
            } else {
                &[
                    "Library/Preferences",
                    "Library/Caches",
                    "Library/Containers",
                ]
            };
            let prefixes: &[(&str, &str)] = if windows {
                &[
                    ("AppData/Local/Microsoft/Credentials", "cred"),
                    ("AppData/Roaming/Microsoft/Protect", "key"),
                    (".ssh", "id_rsa"),
                ]
            } else {
                &[
                    ("Library/Keychains", "login.keychain-db"),
                    ("Library/Mail", "V10/INBOX.mbox"),
                    (".ssh", "id_rsa"),
                ]
            };
            let check = |p: &std::path::Path| {
                check_home_paths_for_platform(p, &p.to_string_lossy(), &homes, windows)
            };
            for home in &homes {
                // 4a: 目录本身（精确匹配）
                for &rel in exact {
                    let p = home.join(rel);
                    assert!(
                        check(&p).is_some(),
                        "4a 未拦截(windows={windows}): {}",
                        p.display()
                    );
                }
                // 4b: 前缀匹配（含子路径）
                for &(prefix, child) in prefixes {
                    let base = home.join(prefix);
                    let deep = base.join(child);
                    for target in [base, deep] {
                        assert!(
                            check(&target).is_some(),
                            "4b 未拦截(windows={windows}): {}",
                            target.display()
                        );
                    }
                }
                // 主目录本身
                assert!(
                    check(home).is_some(),
                    "主目录本身必须被拦截(windows={windows}): {}",
                    home.display()
                );
            }
            // 对照组：两份 home 之外的同名目录不在保护范围（确认没有过度拦截）
            let unrelated = PathBuf::from("/Volumes/OtherDisk/Library/Mail");
            assert!(
                check(&unrelated).is_none(),
                "不应越界拦截保护名单之外的路径(windows={windows})"
            );
        }
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
        let home = PathBuf::from(test_home_str());
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
        crate::provenance::register_root(&tmp); // A-07b：声明本用例的扫描根
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

    // ---------- A-07：fail-closed 准入层的端到端反向验证 ----------

    #[test]
    fn fail_closed_gate_denies_unvetted_path_and_allows_registered_root() {
        // 用一个不可能被其它用例或静态准入表登记的根，避免全局注册表互相干扰。
        // 这条测试同时是「把第 8 层改回无条件 Safe 就必须变红」的反向验证：
        // 只要有人让准入层恒真，下面第一个断言立刻失败。
        let sentinel = Path::new("/__maclean_unvetted_sentinel_2026__");
        let candidate = sentinel.join("Library/Caches/sub/file.bin");
        let s = candidate.to_string_lossy().to_string();

        let before = gate_decision(&s, "系统缓存");
        assert!(
            matches!(before, SafetyCheck::Danger(_)),
            "未声明来源根的路径必须默认拒绝，实际 {:?}",
            before
        );
        assert!(
            before.reason().unwrap_or("").contains("来源准入"),
            "拒绝原因必须可诊断，实际 {:?}",
            before
        );

        crate::provenance::register_root(sentinel);
        let after = gate_decision(&s, "系统缓存");
        assert!(
            matches!(after, SafetyCheck::Safe),
            "声明扫描根后应放行，实际 {:?}",
            after
        );

        // 组件级边界：同名前缀的兄弟目录不得被顺带放行
        let sibling = "/__maclean_unvetted_sentinel_2026__Evil/Library/Caches/file.bin";
        assert!(
            matches!(gate_decision(sibling, "系统缓存"), SafetyCheck::Danger(_)),
            "准入根必须按组件比较，不能退化为字符串前缀"
        );
    }

    #[test]
    fn warning_is_never_treated_as_allowed_by_the_gate() {
        // 闸门只认 Safe。若有人给 `Warning` 开后门，这条会变红。
        let warning = SafetyCheck::Warning("需要确认".into());
        assert!(!warning.is_safe(), "Warning 在统一闸门下不得放行");
        assert!(!gate_allows("", "任意类目"), "空路径必须被拒");
    }

    #[test]
    fn pseudo_path_injection_cannot_skip_blacklists() {
        // A-10：把绝对路径藏在 ` | ` 之后不得绕过第 3 层系统黑名单。
        let injected = "com.apple.TimeMachine.2024-01-01-120000 | /System/Library";
        assert!(matches!(
            check_path_safety_with_category(injected, ""),
            SafetyCheck::Danger(_)
        ));
        // 正常快照名仍可用
        assert!(matches!(
            check_path_safety_with_category(
                "com.apple.TimeMachine.2024-01-01-120000 | Macintosh HD",
                ""
            ),
            SafetyCheck::Safe
        ));
    }

    #[test]
    fn home_blacklist_is_case_and_unicode_insensitive() {
        // A-09：APFS 大小写不敏感 / NFD 存储，黑名单比较必须归一化。
        let canonical = Path::new("/Users/u/library/mail/message.emlx");
        let forbidden = Path::new("/Users/u/Library/Mail");
        assert!(loose_starts_with(
            &loose_components(canonical),
            &loose_components(forbidden)
        ));
        assert_eq!(
            loose_component_key("Cafe\u{301}"),
            loose_component_key("cafÉ")
        );
    }

    #[test]
    fn repeat_cache_dir_rejects_sibling_prefix_and_reordered_containers() {
        // A-03：裸字符串前缀与两段独立 contains 的历史漏洞。
        assert!(!is_repeat_cache_dir(Path::new(
            "/Library/CachesEvil/duplicate.bin"
        )));
        assert!(!is_repeat_cache_dir(Path::new(
            "/tmp/Data/Library/Caches/Library/Containers/x/duplicate.bin"
        )));
        assert!(is_repeat_cache_dir(Path::new(
            "/Users/u/Library/Containers/com.example.app/Data/Library/Caches/a.bin"
        )));
    }
}
