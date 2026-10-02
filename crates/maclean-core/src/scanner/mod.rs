//! 扫描器模块 - 负责扫描各类可清理的项目
//!
//! 本模块定义了扫描器的核心数据结构和 trait，
//! 并导出具体的子模块：开发者缓存、大文件、应用缓存、APFS 快照。

use std::path::{Path, PathBuf};
use std::time::Duration;

// 导出缓存模块
pub mod cache;

// 导出子模块
#[cfg(target_os = "macos")]
pub mod apfs;
// app_cache/app_data 扫描 macOS 专属路径（~/Library/Containers 等），
// Windows 上不支持，用 cfg 包装（调用点已确认全部在 cfg(macos) 内）。
// optimize 内部已按平台返回不同任务清单，无需限制平台。
#[cfg(target_os = "macos")]
pub mod app_cache;
#[cfg(target_os = "macos")]
pub mod app_data;
pub mod cache_registry;
pub mod dev_cache;
pub mod dup_files;
pub mod large_files;
pub mod optimize;
// startup/uninstall 刻意不加 cfg（2026-09-28）：cli/ui/app 里的调用点
// （App.startup_items 字段类型、CLI startup/uninstall 子命令、启动项面板）
// 是跨平台编译的，cfg(macos) 会直接砸断 Windows 构建（交叉 check 11 错）。
// 两模块自身只用 std::process::Command 与路径字符串，Windows 上可编译；
// 扫描目标（~/Library/LaunchAgents、/Applications 等）在 Windows 不存在，
// 天然返回空结果，不会误扫误删。
pub mod startup;
// 残留名称匹配：刻意不限定平台，见模块顶部注释。放在 scanner 根而非
// windows_apps 内，是为了让它（连同规定的删除保护）在开发机上可被编译和测试。
mod residual_match;
pub mod uninstall;
// Windows 专属模块（采集层：读注册表 / 枚举 UWP / 执行卸载）
#[cfg(target_os = "windows")]
pub mod windows_apps;
// Windows 应用保护**判定**：刻意不加 cfg。它原本在 windows_apps.rs 里，
// 结果在开发机上整段不编译、一行测试都跑不到 —— 而这是卸载前的最后一道
// 保护。判定只依赖 String/bool 和跨平台文件系统调用，必须本机可测。
// 详见 win_protection.rs 模块顶部注释。
pub mod win_protection;
// macOS 官方卸载器**识别**：同样刻意不加 cfg（理由同 win_protection）。
// 名字判定、搜索路径、启动命令构造全是纯逻辑，加 cfg 就等于在开发机上
// 零测试；只有真正 spawn 进程的几行放在 cfg(macos) 里。
pub mod official_uninstaller;

/// 推荐等级 - 帮助用户判断是否应该清理
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Recommend {
    /// 推荐清理 - 安全可删，重新构建/使用时会自动恢复
    Safe,
    /// 应用缓存 - 只含缓存/日志，删除后应用可正常运行并自动重建
    CacheOnly,
    /// 谨慎清理 - 删除后可能需要重新下载或配置
    Caution,
    /// 高级用户 - 需要了解风险后自行判断
    Advanced,
}

// 2026-09-18 删除了 `Recommend::label` / `Recommend::description`：
// 与 `i18n::translate_recommend` 重复且全仓零引用，UI 上的等级徽标统一走
// `theme::recommend_label`（文案按设计稿 02 节）。留两套会各自漂移。

impl Recommend {
    /// 是否默认被"智能选择"勾选
    pub fn default_selected(self) -> bool {
        matches!(self, Recommend::Safe | Recommend::CacheOnly)
    }
}

/// 扫描项 - 表示一个可清理的文件或目录
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanItem {
    /// 完整路径（或快照名称/UUID）
    pub path: String,
    /// 大小（字节）
    pub size_bytes: u64,
    /// 分类名称，如 "Rust编译"、"Xcode编译"、"APFS快照" 等
    pub category: String,
    /// 是否被用户选中（用于 UI 交互）
    pub selected: bool,
    /// 是否可删除（部分系统级目录不可直接删除）
    pub deletable: bool,
    /// 不可删除的原因（deletable=false 时显示给用户）
    pub undeletable_reason: String,
    /// 推荐等级
    pub recommend: Recommend,
    /// 该项的说明（告诉用户这是什么，删除后有什么影响）
    pub description: String,
    /// 批量删除的真实路径列表（用于 __pycache__ 等聚合项）
    /// 为空表示单项删除，使用 path 字段
    pub batch_paths: Vec<String>,
}

/// 扫描结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanResult {
    /// 所有扫描到的项目
    pub items: Vec<ScanItem>,
    /// 总大小（字节）
    pub total_size: u64,
    /// 扫描耗时（毫秒）
    pub scan_time_ms: u64,
}

/// 扫描器 trait - 所有具体扫描器都需实现此接口
pub trait Scanner {
    /// 执行扫描，返回扫描结果
    fn scan(&self) -> ScanResult;
}

/// 带超时的扫描执行器。
///
/// 文件系统异常（如损坏的 APFS 目录、挂起的网络卷）会让 `read_dir`
/// 永久阻塞，普通线程无法安全中断。本执行器把扫描放到独立线程，
/// 超时未返回即视为"跳过该扫描"，调用方继续后续流程；卡死的线程
/// 在进程退出时由系统回收。内部用 `catch_unwind` 兜底：扫描器 panic
/// 或超时一律返回 `None`，不炸线程。
///
/// 返回 `Some(result)` 表示正常完成；`None` 表示超时或 panic。
pub fn scan_with_timeout<R, F>(timeout: Duration, f: F) -> Option<R>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok();
        // sync_channel 容量 1：发送永不阻塞，recv_timeout 收到即返回
        let _ = tx.send(result);
    });
    rx.recv_timeout(timeout).ok().flatten()
}

/// 检测路径是否可被当前用户删除
/// 返回 (deletable, reason)
/// 跨平台实现：委托给 platform 模块
pub fn check_deletable(path: &str) -> (bool, String) {
    crate::platform::check_deletable(path)
}

/// 格式化字节大小为人类可读字符串
///
/// 返回格式如:
/// - "189.3G" (吉字节)
/// - "7.2G"
/// - "345.6M" (兆字节)
/// - "12.3K" (千字节)
/// - "512B"  (字节)
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;

    let size = bytes as f64;
    if size >= GB {
        format!("{:.1}G", size / GB)
    } else if size >= MB {
        format!("{:.1}M", size / MB)
    } else if size >= KB {
        format!("{:.1}K", size / KB)
    } else {
        format!("{}B", bytes)
    }
}

/// 单目录遍历超时：超过视为该目录磁盘 IO 卡死，跳过并记入黑名单
const DIR_SCAN_TIMEOUT: Duration = Duration::from_secs(10);

/// 遍历最大深度（与原 WalkDir 行为一致）
const DIR_MAX_DEPTH: usize = 50;

/// 黑名单有效期（秒）：7 天。过期后目录会被重新尝试扫描，
/// 避免磁盘恢复后永久误跳过。
const BLOCKED_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// 黑名单文件：上次遍历卡死的目录（~/.maclean/blocked_dirs.json）
fn blocked_dirs_file() -> PathBuf {
    home_dir().join(".maclean/blocked_dirs.json")
}

/// 读取黑名单（过滤过期条目）
pub fn load_blocked_dirs() -> std::collections::HashSet<PathBuf> {
    let mut dirs = std::collections::HashSet::new();
    let path = blocked_dirs_file();
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(list) = serde_json::from_str::<Vec<(String, u64)>>(&s) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            for (p, ts) in list {
                if now.saturating_sub(ts) < BLOCKED_TTL_SECS {
                    dirs.insert(PathBuf::from(p));
                }
            }
        }
    }
    dirs
}

/// 带超时的目录枚举：返回子路径列表。
///
/// 目录磁盘 IO 卡死（readdir 在内核长时间不返回）时，超时返回 None，
/// 调用方直接跳过该目录——单个坏目录不再阻塞整个扫描流程。
/// 与 dir_size 的目录级隔离共用 DIR_SCAN_TIMEOUT。
pub fn read_dir_with_timeout(dir: &Path) -> Option<Vec<PathBuf>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let d = dir.to_path_buf();
    std::thread::spawn(move || {
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&d) {
            for e in entries.flatten() {
                paths.push(e.path());
            }
        }
        let _ = tx.send(paths);
    });
    match rx.recv_timeout(DIR_SCAN_TIMEOUT) {
        Ok(paths) => Some(paths),
        Err(_) => {
            crate::logger::warn(&format!(
                "[read_dir] 目录遍历超时（IO 卡死），跳过: {}",
                dir.display()
            ));
            None
        }
    }
}

/// 将目录加入黑名单（持久化，7 天自动过期）
pub fn add_blocked_dir(path: &Path) {
    let mut dirs = load_blocked_dirs();
    if !dirs.insert(path.to_path_buf()) {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let list: Vec<(String, u64)> = dirs
        .iter()
        .map(|p| (p.to_string_lossy().to_string(), now))
        .collect();
    if let Ok(s) = serde_json::to_string(&list) {
        let file = blocked_dirs_file();
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&file, s);
    }
}

/// 清空黑名单（磁盘恢复后手动重试全部目录）
pub fn clear_blocked_dirs() {
    let _ = std::fs::remove_file(blocked_dirs_file());
}

/// 目录大小缓存文件：最近一次成功统计的大小（~/.maclean/dir_size_cache.json）
///
/// 黑名单目录（上次遍历卡死）不再返回 0，而是回退到最近一次成功统计的大小：
/// 跨扫描结果稳定，避免"这次统计到、下次跳过"造成的列表大小/排序抖动。
fn dir_size_cache_file() -> PathBuf {
    home_dir().join(".maclean/dir_size_cache.json")
}

/// 读取目录大小缓存（dir → 最近一次成功统计的字节数）
fn load_dir_size_cache() -> std::collections::HashMap<PathBuf, u64> {
    let mut cache = std::collections::HashMap::new();
    let path = dir_size_cache_file();
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(list) = serde_json::from_str::<Vec<(String, u64)>>(&s) {
            for (p, size) in list {
                cache.insert(PathBuf::from(p), size);
            }
        }
    }
    cache
}

/// 持久化目录大小缓存
///
/// 采用「写同目录临时文件 → rename 原子替换」：磁盘分析等扫描器会在 rayon
/// 线程里并行统计多个目录，非原子的直接覆写在并发/崩溃时可能留下被截断的
/// JSON（下次加载静默回退空缓存）。rename 在同一卷上是原子的，读者要么
/// 看到旧文件、要么看到完整新文件。
fn save_dir_size_cache(cache: &std::collections::HashMap<PathBuf, u64>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static WRITE_SEQ: AtomicU64 = AtomicU64::new(0);

    let list: Vec<(String, u64)> = cache
        .iter()
        .map(|(p, size)| (p.to_string_lossy().to_string(), *size))
        .collect();
    if let Ok(s) = serde_json::to_string(&list) {
        let file = dir_size_cache_file();
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("dir_size_cache.json");
        // 每个并发写者使用独立临时名，避免彼此截断；写完原子改名
        let tmp = file.with_file_name(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            WRITE_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        if std::fs::write(&tmp, s).is_ok() {
            if std::fs::rename(&tmp, &file).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
}

/// 递归计算目录大小（字节），并返回该次统计是否因黑名单/超时而不完整。
///
/// 返回 (大小, skipped)：skipped=true 表示统计过程中有黑名单目录被跳过或
/// 目录遍历超时，该大小可能是上次成功值或缺失。扫描器可用它给对应项
/// 追加"统计跳过（上次遍历卡死）"标记，让用户知道数值可能不准。
///
/// 目录级隔离：每个目录的枚举放到独立线程，主循环用超时等待。
/// 当某个目录磁盘 IO 卡死（APFS 异常 / 网络卷挂起，readdir/stat 在内核
/// 长时间不返回）时，超时后跳过该目录并记入黑名单，其余目录照常统计——
/// 一个坏目录不再拖垮整个扫描，也不会让 UI 长时间停在同一个路径。
///
/// 遇到无法访问的文件/目录时直接跳过，不报错。
pub fn dir_size_impl(path: &Path) -> (u64, bool) {
    use rayon::prelude::*;
    let blocked = load_blocked_dirs();
    let mut cache = load_dir_size_cache();
    let mut total: u64 = 0;
    let mut skipped = false;
    let mut entry_timeout = false;
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = std::collections::VecDeque::new();
    queue.push_back((path.to_path_buf(), 0));

    while !queue.is_empty() {
        let batch: Vec<(PathBuf, usize)> = queue.drain(..).collect();
        // 并行 BFS：同一批目录同时枚举（rayon 池并发 = CPU 核数）。
        // 每个目录仍是独立线程 + 超时 watchdog —— 一个坏目录只跳过自身，
        // 不阻塞同批其它目录（历史：Telegram 目录 readdir 卡死拖死整轮
        // 扫描、进度条冻结；串行逐目录等待是扫描慢的主因之一）。
        // 黑名单/缓存读取只做不可变借用，写入回到主循环串行处理。
        type DirScanResult = (
            usize,           // depth
            bool,            // skipped（深度/黑名单/photos/超时）
            u64,             // files_size
            Vec<PathBuf>,    // subdirs
            Option<PathBuf>, // 超时目录（需入黑名单）
            Option<PathBuf>, // 黑名单命中目录
        );
        let results: Vec<DirScanResult> = batch
            .into_par_iter()
            .map(|(dir, depth)| {
                if depth > DIR_MAX_DEPTH {
                    return (depth, true, 0, Vec::new(), None, None);
                }
                // 黑名单目录：上次遍历卡死，跳过并回退最近一次成功统计的大小
                // （无缓存则为 0）—— 结果跨扫描稳定，不再"这次有、下次没有"地跳。
                if blocked.contains(&dir) {
                    let cached = cache.get(&dir).copied().unwrap_or(0);
                    return (depth, true, cached, Vec::new(), None, Some(dir));
                }
                // 跳过 Photos Library 等问题 bundle（与原 WalkDir filter_entry 一致）
                if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
                    let lower = name.to_lowercase();
                    if lower.ends_with(".photoslibrary")
                        || lower.ends_with(".musiclibrary")
                        || lower.ends_with(".tvlibrary")
                    {
                        return (depth, true, 0, Vec::new(), None, None);
                    }
                }

                // 每个目录在独立线程枚举，超时等待（可中断的目录级 watchdog）
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                let dir_task = dir.clone();
                std::thread::spawn(move || {
                    let mut files_size: u64 = 0;
                    let mut subdirs: Vec<PathBuf> = Vec::new();
                    if let Ok(entries) = std::fs::read_dir(&dir_task) {
                        for e in entries.flatten() {
                            // file_type() 是 lstat 语义：目录符号链接 is_dir()=false，
                            // 天然不跟随链接（与 follow_links(false) 一致）
                            if let Ok(ft) = e.file_type() {
                                if ft.is_dir() {
                                    subdirs.push(e.path());
                                } else if ft.is_file() {
                                    if let Ok(meta) = e.metadata() {
                                        // 用 st_blocks×512（实际磁盘占用）而非 len()
                                        // （逻辑大小）：APFS 稀疏文件（VM 镜像、
                                        // Docker/OrbStack 磁盘）len() 会虚高——
                                        // 实测 OrbStack 容器目录按 len 算 995G，
                                        // 超过整机磁盘 926G，明显是稀疏文件误计。
                                        #[cfg(unix)]
                                        {
                                            use std::os::unix::fs::MetadataExt;
                                            files_size += meta.blocks() * 512;
                                        }
                                        #[cfg(not(unix))]
                                        {
                                            files_size += meta.len();
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let _ = tx.send((files_size, subdirs));
                });

                match rx.recv_timeout(DIR_SCAN_TIMEOUT) {
                    Ok((files_size, subdirs)) => (depth, false, files_size, subdirs, None, None),
                    Err(_) => (depth, true, 0, Vec::new(), Some(dir), None),
                }
            })
            .collect();

        for (depth, skipped_this, files_size, subdirs, timed_out, blocked_hit) in results {
            if let Some(bdir) = timed_out {
                // 该目录遍历超时（IO 卡死）：跳过并记入黑名单，保留旧缓存
                skipped = true;
                if bdir.as_path() == path {
                    entry_timeout = true;
                }
                crate::logger::warn(&format!(
                    "[dir_size] 目录遍历超时（IO 卡死），跳过并加入黑名单: {}",
                    bdir.display()
                ));
                add_blocked_dir(&bdir);
                continue;
            }
            if skipped_this {
                if let Some(bdir) = blocked_hit {
                    skipped = true;
                    let cached = cache.get(&bdir).copied().unwrap_or(0);
                    crate::logger::warn(&format!(
                        "[dir_size] 跳过黑名单目录（上次遍历卡死），使用上次统计大小 {}: {}",
                        cached,
                        bdir.display()
                    ));
                }
                // 深度超限 / Photos 跳过：无警告，正常继续
                continue;
            }
            total += files_size;
            for sub in subdirs {
                queue.push_back((sub, depth + 1));
            }
        }
    }

    // 入口成功统计（即使部分子目录被跳过）都记录最近值：黑名单清空前，
    // 后续扫描会得到同样的值，跨扫描结果稳定。入口本身超时不覆盖旧值。
    if !entry_timeout {
        cache.insert(path.to_path_buf(), total);
        save_dir_size_cache(&cache);
    }

    (total, skipped)
}

/// 递归计算目录大小（字节），见 [`dir_size_impl`]
pub fn dir_size(path: &Path) -> u64 {
    dir_size_impl(path).0
}

/// 获取当前用户的 home 目录
///
/// 使用 dirs crate 获取，若获取失败则返回**空路径**（而不是 "/"）。
///
/// 之所以不能回退到 "/"：各扫描器都拿它去 join("Library/...")，回退到 "/"
/// 会变成扫描 `/Library/Application Support`、`/Library/Caches` 这类系统目录，
/// 且扫描结果被标为可删除。
pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// home 目录是否有效（扫描器入口应先用它做守卫）
pub fn has_home() -> bool {
    !home_dir().as_os_str().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1024), "1.0K");
        assert_eq!(format_size(1024 * 1024), "1.0M");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0G");
        // 边界：不因浮点误差串位
        assert_eq!(format_size(1536 * 1024 * 1024), "1.5G");
    }

    #[test]
    fn recommend_default_selected_only_safe_levels() {
        // 只有 Safe / CacheOnly 会被"智能选择"默认勾选，
        // Caution / Advanced 必须保持未选（这是默认安全策略的核心）
        assert!(Recommend::Safe.default_selected());
        assert!(Recommend::CacheOnly.default_selected());
        assert!(!Recommend::Caution.default_selected());
        assert!(!Recommend::Advanced.default_selected());
    }

    #[test]
    fn recommend_labels_are_stable() {
        // 锁的是 `i18n::recommend_label`（自 theme 迁移，语义不变） —— UI 徽标真正调用的那个。
        // 此前这条用例锁的是 `Recommend::label()`（已删），后者零生产调用：
        // 锁在一个没人用的副本上，UI 文案改了这条用例照样绿。
        // 改动需同步 maclean-ui-design-v2.html。
        assert_eq!(
            crate::i18n::recommend_label(&Recommend::Safe, false),
            "安全"
        );
        assert_eq!(
            crate::i18n::recommend_label(&Recommend::CacheOnly, false),
            "仅缓存"
        );
        assert_eq!(
            crate::i18n::recommend_label(&Recommend::Caution, false),
            "谨慎"
        );
        assert_eq!(
            crate::i18n::recommend_label(&Recommend::Advanced, false),
            "高级"
        );
        // 英文侧
        assert_eq!(crate::i18n::recommend_label(&Recommend::Safe, true), "Safe");
        assert_eq!(
            crate::i18n::recommend_label(&Recommend::Advanced, true),
            "Advanced"
        );
    }

    #[test]
    fn recommend_label_has_two_diverging_copies() {
        // 同一套等级，仓里还剩两份文案源，且**不一致**：
        //   theme::recommend_label   → UI 徽标：安全 / 仅缓存 / 谨慎 / 高级
        //   i18n::translate_recommend → 日志等纯文本：推荐 / 仅缓存 / 谨慎 / 需确认
        // Safe 与 Advanced 两档对不上。不强行统一（改哪边都是产品决策），
        // 但把差异钉住：以后有人改其中一份，这条会红，逼他确认另一份。
        assert_eq!(
            crate::i18n::recommend_label(&Recommend::Safe, false),
            "安全"
        );
        assert_eq!(
            crate::i18n::translate_recommend(&Recommend::Safe, false),
            "推荐"
        );
        assert_eq!(
            crate::i18n::recommend_label(&Recommend::Advanced, false),
            "高级"
        );
        assert_eq!(
            crate::i18n::translate_recommend(&Recommend::Advanced, false),
            "需确认"
        );
    }

    #[test]
    fn scan_with_timeout_returns_result_when_fast() {
        let r = scan_with_timeout(Duration::from_secs(5), || ScanResult {
            items: Vec::new(),
            total_size: 42,
            scan_time_ms: 1,
        });
        assert!(r.is_some());
        assert_eq!(r.unwrap().total_size, 42);
    }

    #[test]
    fn scan_with_timeout_returns_none_on_timeout() {
        // 超时：卡住的扫描被放弃，返回 None（卡死线程 200ms 后自然结束）
        let r = scan_with_timeout(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_millis(200));
            ScanResult {
                items: Vec::new(),
                total_size: 0,
                scan_time_ms: 0,
            }
        });
        assert!(r.is_none());
    }

    #[test]
    fn scan_with_timeout_catches_panic() {
        // panic 的扫描器不炸线程，视为扫描失败返回 None
        let r = scan_with_timeout(Duration::from_secs(5), || {
            panic!("boom");
        });
        assert!(r.is_none());
    }
}
