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

/// 单目录遍历超时：超过视为该目录本次枚举磁盘 IO 卡死，**仅本次统计**跳过其子树。
/// 不落盘、不跨扫描记忆——下次扫描会无条件重新尝试（慢盘唤醒 / 外置盘重新挂载
/// 后即可自然恢复，避免一次偶发卡顿把目录长期判小）。
const DIR_SCAN_TIMEOUT: Duration = Duration::from_secs(10);

/// 磁盘分析「大目录」准确统计的单目录枚举超时：比清理扫描更宽松，给慢盘 /
/// 休眠唤醒 / 刚挂载的外置磁盘足够时间，尽量一次就统计准确。
const ACCURATE_DIR_TIMEOUT: Duration = Duration::from_secs(20);

/// 快速统计（清理扫描）的遍历最大深度；磁盘分析的准确统计不受此限。
const DIR_MAX_DEPTH: usize = 50;

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

/// 递归计算目录大小（字节），并返回该次统计是否因超时而不完整。
///
/// 返回 (大小, skipped)：skipped=true 表示统计过程中有目录枚举超时（或触达深度
/// 上限 / 命中资源库 bundle）而有子树没能计入，该大小是「至少」占用，真实值可能
/// 更大。磁盘分析等可用它给对应项追加"统计不完整"提示。
///
/// 目录级隔离：每个目录的枚举放到独立线程，主循环用超时等待。当某个目录磁盘 IO
/// 卡死（APFS 异常 / 网络卷挂起，readdir/stat 在内核长时间不返回）时，**仅本次
/// 统计**跳过该子树，其余目录照常——一个坏目录不拖垮整轮扫描，也不让 UI 停住。
///
/// **不做任何持久记忆**：超时不落盘、不写黑名单，下次扫描会无条件重新尝试。慢盘
/// 唤醒、外置盘重新挂载、系统负载恢复后，下一次即可统计到真实体量，不会因一次偶发
/// 卡顿被长期低估为 0（同一棵遍历树内每个目录本就只访问一次，剪枝即天然避免在
/// 同一轮里重复踩中同一个坏目录，无需额外的内存表）。
///
/// 遇到无法访问的文件/目录时直接跳过，不报错。
pub fn dir_size_impl(path: &Path) -> (u64, bool) {
    use rayon::prelude::*;
    let mut total: u64 = 0;
    let mut skipped = false;
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = std::collections::VecDeque::new();
    queue.push_back((path.to_path_buf(), 0));

    while !queue.is_empty() {
        let batch: Vec<(PathBuf, usize)> = queue.drain(..).collect();
        // 并行 BFS：同一批目录同时枚举（rayon 池并发 = CPU 核数）。每个目录在独立
        // 线程 + 超时 watchdog —— 一个坏目录只在本次跳过自身子树，不阻塞同批其它
        // 目录（历史：Telegram 目录 readdir 卡死拖死整轮扫描、进度条冻结）。
        type DirScanResult = (
            usize,           // depth
            bool,            // skipped（深度 / 资源库 bundle）
            u64,             // files_size
            Vec<PathBuf>,    // subdirs
            Option<PathBuf>, // 本次枚举超时的目录（仅本次剪枝，不落盘）
        );
        let results: Vec<DirScanResult> = batch
            .into_par_iter()
            .map(|(dir, depth)| {
                if depth > DIR_MAX_DEPTH {
                    return (depth, true, 0, Vec::new(), None);
                }
                // 跳过 Photos Library 等问题 bundle（与原 WalkDir filter_entry 一致）
                if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
                    let lower = name.to_lowercase();
                    if lower.ends_with(".photoslibrary")
                        || lower.ends_with(".musiclibrary")
                        || lower.ends_with(".tvlibrary")
                    {
                        return (depth, true, 0, Vec::new(), None);
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
                    Ok((files_size, subdirs)) => (depth, false, files_size, subdirs, None),
                    Err(_) => (depth, true, 0, Vec::new(), Some(dir)),
                }
            })
            .collect();

        for (_depth, skipped_this, files_size, subdirs, timed_out) in results {
            if let Some(bdir) = timed_out {
                // 该目录本次枚举超时（IO 卡死/忙）：本次只跳过它的子树并标记
                // 不完整，不落盘、不记忆——下次扫描无条件重试。
                skipped = true;
                crate::logger::warn(&format!(
                    "[dir_size] 目录本次枚举超时（IO 忙/卡死），本轮跳过其子树、下次扫描重试: {}",
                    bdir.display()
                ));
                continue;
            }
            if skipped_this {
                // 深度超限 / 资源库 bundle：正常剪枝，不计入警告
                continue;
            }
            total += files_size;
            for sub in subdirs {
                queue.push_back((sub, _depth + 1));
            }
        }
    }

    (total, skipped)
}

/// 递归计算目录大小（字节），见 [`dir_size_impl`]
pub fn dir_size(path: &Path) -> u64 {
    dir_size_impl(path).0
}

/// 磁盘分析「大目录」专用的**尽力准确**目录统计。
///
/// 返回 `(占用字节数, incomplete)`：`incomplete == true` 表示存在因磁盘 IO 卡死 /
/// 忙而**本次没能统计到**的子树，此时字节数是「至少」占用，真实值可能更大，调用方
/// 应向用户如实标注，而不是把它当成精确的小数字展示。
///
/// 相比为「快速、绝不卡死」的清理扫描设计的 [`dir_size_impl`]，这里做两点放宽：
/// - **不做深度截断**，再深的目录也一直走到文件；
/// - 单目录枚举超时放宽到 [`ACCURATE_DIR_TIMEOUT`]（20s），给慢磁盘 / 休眠唤醒 /
///   刚挂载的外置盘足够时间，尽量一次统计准确。
///
/// 与 [`dir_size_impl`] 一样**不做任何持久记忆**：超时只在本次统计中剪枝并标记
/// incomplete，不落盘、不写黑名单；下次扫描无条件重新尝试，慢盘恢复 / 外置盘重新
/// 挂载后自然能统计到真实体量，绝不因一次偶发卡顿把大目录长期记 0。
///
/// 用 `st_blocks×512`（实际磁盘块占用）而非逻辑长度，APFS 稀疏文件（虚拟机磁盘）
/// 不会被虚高计；符号链接不跟随（`file_type` 为 lstat 语义），无环风险。
pub fn dir_size_accurate(path: &Path) -> (u64, bool) {
    use rayon::prelude::*;
    use std::collections::VecDeque;
    use std::sync::mpsc::sync_channel;

    let mut total: u64 = 0;
    let mut incomplete = false;
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((path.to_path_buf(), 0));

    // (depth, skipped(资源库 bundle), files_size, subdirs, 本次超时目录)
    type Row = (usize, bool, u64, Vec<PathBuf>, Option<PathBuf>);

    while !queue.is_empty() {
        let batch: Vec<(PathBuf, usize)> = queue.drain(..).collect();
        // 同层目录并行枚举（rayon），每个目录独立线程 + 宽松 watchdog：
        // 一个坏目录最多让这一项标 incomplete，不阻塞同层其它目录。
        let results: Vec<Row> = batch
            .into_par_iter()
            .map(|(dir, depth)| {
                if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
                    let lower = name.to_lowercase();
                    if lower.ends_with(".photoslibrary")
                        || lower.ends_with(".musiclibrary")
                        || lower.ends_with(".tvlibrary")
                    {
                        // 资源库 bundle 有意整体跳过，不计入 incomplete
                        return (depth, true, 0, Vec::new(), None);
                    }
                }

                let (tx, rx) = sync_channel(1);
                let task = dir.clone();
                std::thread::spawn(move || {
                    let mut files_size: u64 = 0;
                    let mut subdirs: Vec<PathBuf> = Vec::new();
                    if let Ok(entries) = std::fs::read_dir(&task) {
                        for e in entries.flatten() {
                            if let Ok(ft) = e.file_type() {
                                if ft.is_dir() {
                                    subdirs.push(e.path());
                                } else if ft.is_file() {
                                    if let Ok(meta) = e.metadata() {
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

                match rx.recv_timeout(ACCURATE_DIR_TIMEOUT) {
                    Ok((files_size, subdirs)) => {
                        (depth, false, files_size, subdirs, None)
                    }
                    Err(_) => (depth, true, 0, Vec::new(), Some(dir)),
                }
            })
            .collect();

        for (depth, skipped, files_size, subdirs, timed_out) in results {
            if let Some(bdir) = timed_out {
                // 该项本次 IO 卡死/忙：标 incomplete（体量未知）、本次跳过其子树，
                // 不落盘、不记忆——下次扫描会重新真正尝试。
                incomplete = true;
                crate::logger::warn(&format!(
                    "[dir_size_accurate] 目录枚举超过 {ACCURATE_DIR_TIMEOUT:?}（IO 忙/卡死），\
                     本项标记为统计不完整并跳过子树、下次扫描重试: {}",
                    bdir.display()
                ));
                continue;
            }
            if skipped {
                // 资源库 bundle：files_size=0，正常忽略
                continue;
            }
            total += files_size;
            for sub in subdirs {
                queue.push_back((sub, depth + 1));
            }
        }
    }

    (total, incomplete)
}

/// 取一个路径占用的磁盘大小：普通文件取其长度，目录/其它递归统计。
/// 仅用于在“从聚合项中剔除个别危险成员”时回减大小（罕见路径）。
fn path_disk_size(p: &Path) -> u64 {
    match p.symlink_metadata() {
        Ok(m) if m.is_file() => m.len(),
        Ok(_) => dir_size(p),
        Err(_) => 0,
    }
}

/// 把字符串中**第一段**连续阿拉伯数字替换为 `n`（用于同步聚合项描述里的成员计数，
/// 如 “~/ 下的 .DS_Store 文件 (11 个)” → “(10 个)”）。没有数字则原样不动。
fn replace_first_usize(s: &mut String, n: usize) {
    let bytes = s.as_bytes();
    let Some(start) = bytes.iter().position(|c| c.is_ascii_digit()) else {
        return;
    };
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    s.replace_range(start..end, &n.to_string());
}

/// 净化聚合扫描项：只保留能通过 `safety` 安全闸门的 `batch_paths` 成员。
///
/// 这是“安全清理零拦截”不变量在扫描阶段的落点——凡是会被默认勾选的安全聚合项
/// （.DS_Store / __pycache__ / Monorepo 子包等），其展示数量、大小与真实删除成员
/// 必须全部已通过安全检查；命中保护规则的个别成员在扫描期就剔除，而不是先放进
/// 篮子、等到删除阶段再弹“已安全拦截 N 项”。
///
/// - 有成员被剔除时：回减该项大小，并把描述中的计数同步为新成员数；
/// - 净化后一个可删成员都不剩：返回 `false`，调用方应整条移除。
///
/// 注意：单项（`batch_paths` 为空）不在此处理，原样返回其 `deletable`。
pub fn retain_safe_batch_members(item: &mut ScanItem) -> bool {
    if item.batch_paths.is_empty() {
        return item.deletable;
    }
    let before = item.batch_paths.len();
    let mut removed_bytes: u64 = 0;
    item.batch_paths.retain(|bp| {
        match crate::safety::check_path_safety_with_category(bp, &item.category) {
            crate::safety::SafetyCheck::Safe => true,
            // Danger / Warning 在执行层都会被跳过；扫描期就不应把它们算作可清理成员
            _ => {
                removed_bytes = removed_bytes.saturating_add(path_disk_size(Path::new(bp)));
                false
            }
        }
    });
    if item.batch_paths.is_empty() {
        return false;
    }
    if item.batch_paths.len() != before {
        item.size_bytes = item.size_bytes.saturating_sub(removed_bytes);
        replace_first_usize(&mut item.path, item.batch_paths.len());
    }
    true
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
    fn retain_safe_batch_members_drops_guarded_and_resyncs() {
        let root = home_dir().join(format!("maclean_retain_test_{}", std::process::id()));
        let safe1 = root.join("a/.DS_Store");
        let safe2 = root.join("b/.DS_Store");
        // node_modules 属“清单管理目录”，其内成员会被第 4.6 层拦（与 go/pkg/mod 同源）
        let bad = root.join("proj/node_modules/.DS_Store");
        for (p, n) in [(&safe1, 10u64), (&safe2, 20), (&bad, 30)] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![b'x'; n as usize]).unwrap();
        }

        let mk = |paths: Vec<&std::path::Path>, size: u64| ScanItem {
            path: format!("~/ 下的 .DS_Store 文件 ({} 个)", paths.len()),
            size_bytes: size,
            category: "DS_Store".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            recommend: Recommend::Safe,
            description: String::new(),
            batch_paths: paths.iter().map(|p| p.to_string_lossy().to_string()).collect(),
        };

        // 3 个成员（2 安全 + 1 受保护）→ 剔除受保护成员，计数/大小同步
        let mut item = mk(vec![&safe1, &bad, &safe2], 60);
        assert!(retain_safe_batch_members(&mut item), "仍有安全成员应保留该项");
        assert_eq!(item.batch_paths.len(), 2);
        assert!(
            !item.batch_paths.iter().any(|p| p.contains("node_modules")),
            "受保护成员必须被剔除"
        );
        assert_eq!(item.size_bytes, 30, "应回减被剔除成员的 30 字节");
        assert!(item.path.contains("(2 个)"), "计数应同步为 2: {}", item.path);

        // 全部受保护 → 返回 false，调用方整条移除
        let mut all_bad = mk(vec![&bad], 30);
        assert!(!retain_safe_batch_members(&mut all_bad), "无安全成员应整条丢弃");

        let _ = std::fs::remove_dir_all(&root);
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

    // ---- 大目录准确统计 / 黑名单 TTL ----

    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn unique_temp(tag: &str) -> PathBuf {
        let n = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("{tag}_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    /// 写 8KiB 高熵内容：不可压缩、非稀疏，确保在盘上实打实占 >=8192 字节块。
    fn write_8k(path: &Path) {
        let data: Vec<u8> = (0..8192u32).map(|i| (i * 73 + 17) as u8).collect();
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn dir_size_accurate_counts_regular_tree() {
        let base = unique_temp("mcl_acc_tree");
        std::fs::create_dir_all(base.join("x/sub")).unwrap();
        std::fs::create_dir_all(base.join("y")).unwrap();
        write_8k(&base.join("x/f1"));
        write_8k(&base.join("x/sub/f2"));
        write_8k(&base.join("y/f3"));

        let (size, incomplete) = dir_size_accurate(&base);
        assert!(!incomplete, "常规可读目录不应被标记为统计不完整");
        assert!(
            size >= 3 * 8192,
            "三个 8KiB 文件应按实际磁盘块计入（>=24KiB），实际 {size}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dir_size_accurate_has_no_depth_truncation() {
        // 55 层深链（超过快速统计的 DIR_MAX_DEPTH=50），末端放唯一的 8KiB 文件
        let base = unique_temp("mcl_acc_deep");
        let mut leaf = base.clone();
        for _ in 0..55 {
            leaf.push("d");
        }
        std::fs::create_dir_all(&leaf).unwrap();
        write_8k(&leaf.join("deep.bin"));

        // 准确统计：不做深度截断，能走到 55 层下的文件
        let (acc, incomplete) = dir_size_accurate(&base);
        assert!(!incomplete);
        assert!(acc >= 8192, "准确统计应覆盖深度>50 的文件，实际 {acc}");

        // 快速统计：深度 50 以上被整段截断，链上唯一文件不被计入
        let fast = dir_size_impl(&base).0;
        assert_eq!(fast, 0, "快速统计受深度截断，深链唯一文件应为 0，实际 {fast}");
        assert!(fast < acc, "准确统计必须不小于被深度截断的快速统计");
        let _ = std::fs::remove_dir_all(&base);
    }
}
