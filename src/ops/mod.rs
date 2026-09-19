//! 后台操作层
//!
//! 扫描 / 删除 / sudo / Touch ID / 系统优化执行。这一层不碰 egui，
//! 只跟进程、文件系统和线程打交道，可以脱离 UI 单独测试。
//!
//! 2026-09 从 main.rs 拆出，内容与拆分前逐行一致。

use std::sync::mpsc;

use crate::app::{App, ScanState, Tab};
// 模块路径本身也要引入：代码里大量写成 `scanner::Foo` / `logger::info(..)`
use crate::scanner::{ScanItem, Scanner};
use crate::{logger, platform, safety, scanner};

/// 后台扫描消息
pub(crate) enum ScanMessage {
    /// 扫描进度更新
    Progress(f32),
    /// 增量结果（扫描中部分项）— (items, tab_index)
    PartialItems(Vec<ScanItem>, u64),
    /// 当前扫描路径
    CurrentPath(String),
    /// 单个 Tab 扫描完成
    Done(Vec<ScanItem>, u64, u64), // (items, scan_time_ms, tab_index)
    /// 全部扫描完成（用于批量扫描）
    AllDone,
}

/// 后台删除消息
pub(crate) enum DeleteMessage {
    /// 单项删除结果（日志, 路径, 类别, 是否成功）
    Log(String, String, String, bool),
    /// 进度信息（不计入成功/失败统计）
    Info(String),
    /// 普通删除完成，部分项需要管理员权限
    NeedPassword(Vec<(String, String)>),
    /// 全部删除完成
    Done,
    /// 本次删除已写入备份清单（M-2）
    ///
    /// 携带清单 id 与"可还原 / 总数"，让 UI 能如实告诉用户有多少项真的
    /// 有后悔药。永久删除的项不在这两个数里撒谎 —— restorable 只统计
    /// 走废纸篓的那部分。
    BackupRecorded {
        id: String,
        restorable: usize,
        total: usize,
    },
    /// Windows: 卸载后检测到残留，弹出残留清理弹窗
    #[cfg(target_os = "windows")]
    ResidualFound(scanner::windows_apps::UninstallResidual),
    /// Windows: 残留清理完成
    #[cfg(target_os = "windows")]
    ResidualCleaned(usize, usize, usize), // (注册表, 环境变量, 文件)
}

/// 启动后台扫描
pub(crate) fn start_scan(app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
    let tab = app.tab;
    let tab_idx = app.tab_index();
    app.scan_states[tab_idx] = ScanState::Scanning;
    app.scan_progress = 0.0;
    // 清空上一次扫描结果，为增量显示做准备
    app.results[tab_idx].clear();

    // 磁盘分析器：获取当前浏览路径
    let disk_path = if tab == Tab::LargeFiles {
        Some(app.disk_analyzer_current_path())
    } else {
        None
    };

    let (tx, rx) = mpsc::channel();
    *scan_rx = Some(rx);

    // 进度估算线程：每 200ms 发送进度更新
    // 扫描通常 2-8 秒完成，但大文件扫描可能更久，用渐近曲线估算进度
    let tx_progress = tx.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let elapsed = start.elapsed().as_secs_f32();
            // 分段渐近曲线：
            //   0-5s: 快速上升到约 90%
            //   5-15s: 缓慢上升到约 99%
            //   15s+: 极缓慢逼近 99.5%，避免长时间卡在同一个百分比
            let progress = if elapsed < 5.0 {
                0.9 * (1.0 - (-elapsed / 2.5).exp())
            } else if elapsed < 15.0 {
                0.9 + 0.09 * (1.0 - (-(elapsed - 5.0) / 10.0).exp())
            } else {
                0.99 + 0.005 * (1.0 - (-(elapsed - 15.0) / 10.0).exp())
            };
            // 如果通道关闭（扫描已完成），退出
            if tx_progress
                .send(ScanMessage::Progress(progress.min(0.995)))
                .is_err()
            {
                break;
            }
        }
    });

    // 实际扫描线程
    std::thread::spawn(move || {
        // 获取 Tab 名称用于缓存
        // 磁盘分析器在非主目录浏览时不使用缓存（路径不同，结果不同）
        let tab_name = match tab {
            Tab::Overview | Tab::Settings => None,
            Tab::DevCache => Some("dev_cache"),
            Tab::LargeFiles => {
                // 仅在主目录时缓存
                if disk_path
                    .as_ref()
                    .map(|p| *p == scanner::home_dir())
                    .unwrap_or(true)
                {
                    Some("large_files")
                } else {
                    None
                }
            }
            Tab::AppCache => Some("app_cache"),
            Tab::AppData => Some("app_data"),
            Tab::AppUninstall => Some("app_uninstall"),
            Tab::SystemOptimize => None, // 不缓存
            Tab::Apfs => Some("apfs"),
        };

        // 尝试从磁盘缓存加载
        if let Some(name) = tab_name {
            if let Some(cached) = scanner::cache::load_cache(name) {
                // 缓存命中：直接使用缓存结果
                let mut items = cached.items;
                // 后处理：重新检测可删除性（路径可能已变化）
                for item in &mut items {
                    let (deletable, reason) = scanner::check_deletable(&item.path);
                    item.deletable = deletable && item.deletable;
                    if !item.deletable && !reason.is_empty() {
                        item.undeletable_reason = reason;
                    }
                }
                // 缓存命中也按批次发送，提供增量显示体验
                send_items_in_batches(&tx, &items, tab_idx as u64);
                let _ = tx.send(ScanMessage::Done(
                    items,
                    cached.scan_time_ms,
                    tab_idx as u64,
                ));
                let _ = tx.send(ScanMessage::AllDone);
                return;
            }
        }

        // 缓存未命中：执行全量扫描
        // 用 catch_unwind 兜底，防止扫描 panic 后 UI 卡死
        let result = std::panic::catch_unwind(|| {
            match tab {
                Tab::Overview | Tab::Settings => scanner::ScanResult {
                    items: Vec::new(),
                    total_size: 0,
                    scan_time_ms: 0,
                },
                Tab::DevCache => scanner::dev_cache::DevCacheScanner::new().scan(),
                Tab::LargeFiles => {
                    // 磁盘分析器：扫描指定目录（默认为主目录）
                    if let Some(ref path) = disk_path {
                        scanner::large_files::scan_directory(path)
                    } else {
                        scanner::large_files::LargeFileScanner::new().scan()
                    }
                }
                #[cfg(target_os = "macos")]
                Tab::AppCache => scanner::app_cache::AppCacheScanner::new().scan(),
                #[cfg(target_os = "macos")]
                Tab::AppData => scanner::app_data::AppDataScanner::new().scan(),
                #[cfg(target_os = "macos")]
                Tab::AppUninstall => scanner::uninstall::UninstallScanner::new().scan(),
                // OptimizeScanner 内部已按平台分流（macOS 8 项 / Windows 10 项），
                // 这里绝不能加 cfg(macos)：加了之后 Windows 上这个 Tab 恒为空，
                // 而 windows_optimize_tasks() 那 10 项永远跑不到。
                Tab::SystemOptimize => scanner::optimize::OptimizeScanner::new().scan(),
                #[cfg(target_os = "macos")]
                Tab::Apfs => scanner::apfs::ApfsScanner::new().scan(),
                // Windows/Linux: 这些 Tab 返回空结果
                #[cfg(not(target_os = "macos"))]
                Tab::AppCache => {
                    #[cfg(target_os = "windows")]
                    {
                        scanner::windows_apps::WindowsAppCacheScanner::new().scan()
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        scanner::ScanResult {
                            items: Vec::new(),
                            total_size: 0,
                            scan_time_ms: 0,
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                Tab::AppData => {
                    #[cfg(target_os = "windows")]
                    {
                        scanner::windows_apps::WindowsAppDataScanner::new().scan()
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        scanner::ScanResult {
                            items: Vec::new(),
                            total_size: 0,
                            scan_time_ms: 0,
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                Tab::AppUninstall => {
                    #[cfg(target_os = "windows")]
                    {
                        scanner::windows_apps::WindowsUninstallScanner::new().scan()
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        scanner::ScanResult {
                            items: Vec::new(),
                            total_size: 0,
                            scan_time_ms: 0,
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                Tab::Apfs => scanner::ScanResult {
                    items: Vec::new(),
                    total_size: 0,
                    scan_time_ms: 0,
                },
            }
        });

        match result {
            Ok(scan_result) => {
                // 保存到磁盘缓存
                if let Some(name) = tab_name {
                    scanner::cache::save_cache(name, &scan_result);
                }

                // 后处理：检测每个 item 的可删除性
                let mut items = scan_result.items;
                for item in &mut items {
                    let (deletable, reason) = scanner::check_deletable(&item.path);
                    item.deletable = deletable && item.deletable;
                    if !item.deletable && !reason.is_empty() {
                        item.undeletable_reason = reason;
                    }
                }

                // 增量显示：按大小降序排序后分批发送，最后 Done 发送完整列表替换
                send_items_in_batches(&tx, &items, tab_idx as u64);
                let _ = tx.send(ScanMessage::Done(
                    items,
                    scan_result.scan_time_ms,
                    tab_idx as u64,
                ));
            }
            Err(_) => {
                // 扫描 panic，发送空结果让 UI 恢复正常
                let _ = tx.send(ScanMessage::Done(Vec::new(), 0, tab_idx as u64));
            }
        }
        let _ = tx.send(ScanMessage::AllDone);
    });
}

/// 启动所有 Tab 的后台扫描（用于概览页"扫描全部"）
pub(crate) fn start_scan_all(app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
    // 标记所有 Tab 为扫描中，并清空上一次结果
    //
    // 必须与 start_scan 一样 clear()：PartialItems 走 extend，
    // 不清空的话二次「扫描全部」时列表是旧+新叠加，概览统计翻倍；
    // 若某 Tab 扫描 panic 没发 Done，重复项还会固化下来。
    app.reset_for_full_scan();

    let (tx, rx) = mpsc::channel();
    *scan_rx = Some(rx);

    // 进度估算线程
    let tx_progress = tx.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let elapsed = start.elapsed().as_secs_f32();
            let progress = if elapsed < 10.0 {
                0.9 * (1.0 - (-elapsed / 5.0).exp())
            } else if elapsed < 30.0 {
                0.9 + 0.09 * (1.0 - (-(elapsed - 10.0) / 20.0).exp())
            } else {
                0.99 + 0.005 * (1.0 - (-(elapsed - 30.0) / 20.0).exp())
            };
            if tx_progress
                .send(ScanMessage::Progress(progress.min(0.995)))
                .is_err()
            {
                break;
            }
        }
    });

    std::thread::spawn(move || {
        let tabs_to_scan: Vec<(Tab, u64)> = Tab::scannable()
            .into_iter()
            .map(|(tab, idx)| (tab, idx as u64))
            .collect();

        for (tab, tab_idx) in tabs_to_scan {
            // 检查缓存
            let cache_name = match tab {
                Tab::Overview | Tab::Settings => None,
                Tab::DevCache => Some("dev_cache"),
                Tab::LargeFiles => Some("large_files"),
                Tab::AppCache => Some("app_cache"),
                Tab::AppData => Some("app_data"),
                Tab::AppUninstall => Some("app_uninstall"),
                Tab::SystemOptimize => None,
                Tab::Apfs => Some("apfs"),
            };

            if let Some(name) = cache_name {
                if let Some(cached) = scanner::cache::load_cache(name) {
                    let mut items = cached.items;
                    for item in &mut items {
                        let (deletable, reason) = scanner::check_deletable(&item.path);
                        item.deletable = deletable && item.deletable;
                        if !item.deletable && !reason.is_empty() {
                            item.undeletable_reason = reason;
                        }
                    }
                    send_items_in_batches(&tx, &items, tab_idx);
                    let _ = tx.send(ScanMessage::Done(items, cached.scan_time_ms, tab_idx));
                    continue;
                }
            }

            let result = std::panic::catch_unwind(|| match tab {
                Tab::Overview | Tab::Settings => scanner::ScanResult {
                    items: Vec::new(),
                    total_size: 0,
                    scan_time_ms: 0,
                },
                Tab::DevCache => scanner::dev_cache::DevCacheScanner::new().scan(),
                Tab::LargeFiles => scanner::large_files::LargeFileScanner::new().scan(),
                #[cfg(target_os = "macos")]
                Tab::AppCache => scanner::app_cache::AppCacheScanner::new().scan(),
                #[cfg(target_os = "macos")]
                Tab::AppData => scanner::app_data::AppDataScanner::new().scan(),
                #[cfg(target_os = "macos")]
                Tab::AppUninstall => scanner::uninstall::UninstallScanner::new().scan(),
                // OptimizeScanner 内部已按平台分流（macOS 8 项 / Windows 10 项），
                // 这里绝不能加 cfg(macos)：加了之后 Windows 上这个 Tab 恒为空，
                // 而 windows_optimize_tasks() 那 10 项永远跑不到。
                Tab::SystemOptimize => scanner::optimize::OptimizeScanner::new().scan(),
                #[cfg(target_os = "macos")]
                Tab::Apfs => scanner::apfs::ApfsScanner::new().scan(),
                #[cfg(not(target_os = "macos"))]
                Tab::AppCache => {
                    #[cfg(target_os = "windows")]
                    {
                        scanner::windows_apps::WindowsAppCacheScanner::new().scan()
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        scanner::ScanResult {
                            items: Vec::new(),
                            total_size: 0,
                            scan_time_ms: 0,
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                Tab::AppData => {
                    #[cfg(target_os = "windows")]
                    {
                        scanner::windows_apps::WindowsAppDataScanner::new().scan()
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        scanner::ScanResult {
                            items: Vec::new(),
                            total_size: 0,
                            scan_time_ms: 0,
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                Tab::AppUninstall => {
                    #[cfg(target_os = "windows")]
                    {
                        scanner::windows_apps::WindowsUninstallScanner::new().scan()
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        scanner::ScanResult {
                            items: Vec::new(),
                            total_size: 0,
                            scan_time_ms: 0,
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                Tab::Apfs => scanner::ScanResult {
                    items: Vec::new(),
                    total_size: 0,
                    scan_time_ms: 0,
                },
            });

            match result {
                Ok(scan_result) => {
                    if let Some(name) = cache_name {
                        scanner::cache::save_cache(name, &scan_result);
                    }
                    let mut items = scan_result.items;
                    for item in &mut items {
                        let (deletable, reason) = scanner::check_deletable(&item.path);
                        item.deletable = deletable && item.deletable;
                        if !item.deletable && !reason.is_empty() {
                            item.undeletable_reason = reason;
                        }
                    }
                    send_items_in_batches(&tx, &items, tab_idx);
                    let _ = tx.send(ScanMessage::Done(items, scan_result.scan_time_ms, tab_idx));
                }
                Err(_) => {
                    let _ = tx.send(ScanMessage::Done(Vec::new(), 0, tab_idx));
                }
            }
        }

        let _ = tx.send(ScanMessage::AllDone);
    });
}

/// 将扫描结果按大小降序分批发送，实现增量显示
///
/// 排序后按每批最多 10 项分割，每批之间 sleep 50ms，
/// 让 UI 有机会渲染已发现的项。发送完毕后调用方再发送 Done
/// （Done 携带完整列表，会覆盖累积的部分项，保证最终结果一致）。
pub(crate) fn send_items_in_batches(
    tx: &mpsc::Sender<ScanMessage>,
    items: &[ScanItem],
    tab_idx: u64,
) {
    // 按大小降序排序，让用户先看到最大的项
    let mut sorted: Vec<ScanItem> = items.to_vec();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));

    const BATCH_SIZE: usize = 10;
    for chunk in sorted.chunks(BATCH_SIZE) {
        // 报告当前扫描路径
        if let Some(last) = chunk.last() {
            let path = if last.path.is_empty() {
                last.category.clone()
            } else {
                last.path.clone()
            };
            let _ = tx.send(ScanMessage::CurrentPath(path));
        }
        let _ = tx.send(ScanMessage::PartialItems(chunk.to_vec(), tab_idx));
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 尽力删除：优先用系统命令（对 node_modules 等大目录更快），
/// 失败则尝试解除只读标志后再删，再失败就放弃
pub(crate) fn best_effort_delete(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy().to_string();

    #[cfg(target_os = "macos")]
    {
        // macOS: 优先用系统 rm -rf
        if std::process::Command::new("/bin/rm")
            .arg("-rf")
            .arg(&path_str)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
            && !path.exists()
        {
            return true;
        }

        // rm -rf 失败，尝试解除可能存在的 immutable/只读标志后再删除
        let _ = std::process::Command::new("/usr/bin/chflags")
            .arg("-R")
            .arg("nouchg")
            .arg(&path_str)
            .output();
        let _ = std::process::Command::new("/bin/chmod")
            .arg("-R")
            .arg("u+w")
            .arg(&path_str)
            .output();

        let _ = std::process::Command::new("/bin/rm")
            .arg("-rf")
            .arg(&path_str)
            .output();

        !path.exists() && path.symlink_metadata().is_err()
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: 用 rd /s /q 删除目录，del /f /q 删除文件
        let success = if path.is_dir() {
            std::process::Command::new("cmd")
                .arg("/C")
                .arg("rd")
                .arg("/S")
                .arg("/Q")
                .arg(&path_str)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        } else {
            std::process::Command::new("cmd")
                .arg("/C")
                .arg("del")
                .arg("/F")
                .arg("/Q")
                .arg(&path_str)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };

        if success {
            return true;
        }

        // fallback: Rust API
        if path.is_dir() {
            std::fs::remove_dir_all(path).is_ok()
        } else {
            std::fs::remove_file(path).is_ok()
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if path.is_dir() {
            std::fs::remove_dir_all(path).is_ok()
        } else {
            std::fs::remove_file(path).is_ok()
        }
    }
}

/// 获取系统所有挂载点 (macOS 专属)
///
/// 用 `getfsstat` 直接取内核的挂载表，而不是解析 `/sbin/mount` 的文本输出。
/// 文本解析有两个致命问题：
/// 1. 挂载点含空格（如 `/Volumes/My Disk`）时被 `split_whitespace` 截断，
///    导致路径匹配失败 → 误判"未挂载"而放行删除正在使用的卷；
/// 2. `mount` 输出是给人看的，含转义字符（`\040`），不是可靠的数据接口。
#[cfg(target_os = "macos")]
pub(crate) fn get_mount_points() -> Vec<String> {
    use std::ffi::CStr;

    unsafe {
        // 先取缓冲区大小，再分配
        let count = libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT);
        if count <= 0 {
            return Vec::new();
        }
        let cap = (count as usize).saturating_add(8);
        let mut buf: Vec<libc::statfs> = vec![std::mem::zeroed(); cap];
        let n = libc::getfsstat(
            buf.as_mut_ptr(),
            (cap * std::mem::size_of::<libc::statfs>()) as libc::c_int,
            libc::MNT_NOWAIT,
        );
        if n <= 0 {
            return Vec::new();
        }

        let mut mounts = Vec::new();
        for entry in buf.iter().take(n as usize) {
            let cstr = CStr::from_ptr(entry.f_mntonname.as_ptr());
            if let Ok(s) = cstr.to_str() {
                if !s.is_empty() {
                    mounts.push(s.to_string());
                }
            }
        }
        mounts
    }
}

/// 检查路径是否处于某个挂载点之内（macOS 专属）
///
/// 双向判断：
/// - 挂载点就是该路径，或位于该路径之下（原来的方向）；
/// - **该路径本身位于某个挂载点之内**（新增）—— 例如传入的目录位于一个
///   已挂载的 DMG 里，原来会被判为"未挂载"从而放行删除。
#[cfg(target_os = "macos")]
pub(crate) fn is_path_mounted(path: &str, mount_points: &[String]) -> bool {
    let normalized = path.trim_end_matches('/');
    for mp in mount_points {
        let m = mp.trim_end_matches('/');
        if m.is_empty() {
            continue;
        }
        // 挂载点 == 路径，或挂载点在路径之下
        if m == normalized || m.starts_with(&format!("{}/", normalized)) {
            return true;
        }
        // 路径本身在某个挂载点之内（且不是 "/" 这种根挂载）
        if m != "/" && normalized.starts_with(&format!("{}/", m)) {
            return true;
        }
    }
    false
}

/// 移动文件/目录到废纸篓（可恢复）
/// 用于 Caution/Advanced 级别的文件，给用户后悔的机会
/// 移动文件到废纸篓（跨平台，委托给 platform 模块）
pub(crate) fn move_to_trash(path: &str) -> bool {
    platform::move_to_trash(path)
}

/// `xcrun simctl runtime list -j` 中的单个 runtime 条目
#[cfg(target_os = "macos")]
#[derive(serde::Deserialize)]
pub(crate) struct SimRuntimeInfo {
    /// 挂载路径，如 /Library/Developer/CoreSimulator/Volumes/iOS_22F77
    #[serde(default, rename = "mountPath")]
    mount_path: Option<String>,
    /// Cryptex 镜像的父挂载路径
    #[serde(default, rename = "parentMountPath")]
    parent_mount_path: Option<String>,
    /// 系统是否标记该项可删除（正在使用/被依赖的会是 false）
    #[serde(default)]
    deletable: bool,
}

/// 解析出「归属于 path」且「系统允许删除」的模拟器 runtime UUID
///
/// 旧实现用正则从 `runtime list` 的整段文本里抓所有 UUID 再逐个删除，问题：
/// 1. 会连 `parentIdentifier` 这类无关 UUID 一起抓；
/// 2. 完全忽略 path —— 勾选一项却把机器上所有模拟器运行时连锅端；
/// 3. 不看系统给的 `deletable` 标记，正在使用的 runtime 也会被尝试删除。
///
/// 现在：JSON 解析 + 按 mountPath/parentMountPath 归属过滤 + 只取 deletable。
/// 解析失败时**拒绝删除**（fail-closed），不再退化为"全删"。
#[cfg(target_os = "macos")]
pub(crate) fn resolve_simulator_uuids(path: &str) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("xcrun")
        .args(["simctl", "runtime", "list", "-j"])
        .output()
        .map_err(|e| format!("xcrun simctl runtime list -j: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "xcrun simctl runtime list -j: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let map: std::collections::HashMap<String, SimRuntimeInfo> =
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("解析模拟器 runtime 列表失败: {}", e))?;

    let prefix = format!("{}/", path);
    let belongs = |candidate: Option<&String>| match candidate {
        Some(p) => *p == path || p.starts_with(&prefix),
        None => false,
    };

    let mut uuids: Vec<String> = map
        .iter()
        .filter(|(_, v)| v.deletable)
        .filter(|(_, v)| belongs(v.mount_path.as_ref()) || belongs(v.parent_mount_path.as_ref()))
        .map(|(k, _)| k.clone())
        .collect();
    uuids.sort();

    Ok(uuids)
}

/// 安全删除模拟器运行时镜像（/Library/Developer/CoreSimulator/Volumes）
///
/// 安全检查层：
/// 1. 进程检测：Xcode/Simulator/CoreSimulatorService 运行时拒绝删除
/// 2. 挂载点检测：正在挂载使用的运行时跳过，只删 UNUSED 的
/// 3. 使用 `xcrun simctl runtime delete <uuid>` 安全删除
/// 4. 如果 xcrun 失败，返回 Err 让调用方 fallback 到 sudo rm -rf（仅 UNUSED 项）
#[cfg(target_os = "macos")]
pub(crate) fn delete_simulator_volumes(path: &str, lang_en: bool) -> Result<String, String> {
    // 1. 进程检测：模拟器运行中时拒绝删除
    if safety::is_simulator_running() {
        return Err(App::t_lang(lang_en, "log_skip_running")
            .replace("[{}]", "")
            .trim()
            .to_string());
    }

    // 2. 挂载点检测：如果路径被挂载使用，跳过
    let mount_points = get_mount_points();
    if mount_points.is_empty() {
        // mount 命令失败，无法确认安全，拒绝删除
        return Err(App::t_lang(lang_en, "log_cannot_get_mount").to_string());
    }
    if is_path_mounted(path, &mount_points) {
        return Err(App::t_lang(lang_en, "log_mount_in_use").to_string());
    }

    // 3. 只解析归属于该路径、且系统允许删除的 runtime
    let uuids: Vec<String> = resolve_simulator_uuids(path)?;

    if uuids.is_empty() {
        return Err(App::t_lang(lang_en, "log_no_sim_runtimes").to_string());
    }

    // 4. 逐个删除（只删上面筛出来的这些，不再"全删"）
    let mut success_count = 0;
    let mut fail_msgs: Vec<String> = Vec::new();

    for uuid in &uuids {
        let del_output = std::process::Command::new("xcrun")
            .args(["simctl", "runtime", "delete", uuid])
            .output();

        match del_output {
            Ok(o) if o.status.success() => {
                success_count += 1;
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                // 如果是 "not found" 或 "already deleted" 类错误，算成功
                if stderr.contains("not found") || stderr.contains("No such") {
                    success_count += 1;
                } else {
                    fail_msgs.push(format!("{}: {}", uuid, stderr.trim()));
                }
            }
            Err(e) => {
                fail_msgs.push(format!("{}: {}", uuid, e));
            }
        }
    }

    if success_count > 0 {
        let suffix = if !fail_msgs.is_empty() {
            App::tf_lang(
                lang_en,
                "log_sim_failed_suffix",
                &[&fail_msgs.len().to_string()],
            )
        } else {
            String::new()
        };
        let msg = App::tf_lang(
            lang_en,
            "log_sim_deleted",
            &[&success_count.to_string(), &suffix],
        );
        Ok(msg)
    } else {
        Err(format!(
            "{}: {}",
            App::t_lang(lang_en, "log_unknown"),
            fail_msgs.join("; ")
        ))
    }
}

/// 执行 Docker 系统清理
///
/// 运行 `docker system prune -a -f` 清理:
/// - 所有已停止的容器
/// - 所有未被容器使用的网络
/// - 所有未被容器引用的镜像（dangling + unused）
/// - 所有构建缓存
///
/// 注意：**不清理数据卷**。卷通常存放数据库、上传文件等不可重建的数据，
/// 且 `prune --volumes` 无确认、不可恢复。需要清卷请让用户在 Docker Desktop
/// 或 `docker volume prune`（交互式列出卷名）中自行操作。
pub(crate) fn run_docker_prune(lang_en: bool) -> Result<String, String> {
    // 先检查 Docker 是否运行
    let info_check = std::process::Command::new("docker")
        .arg("info")
        .output()
        .map_err(|e| format!("{}: {}", App::t_lang(lang_en, "log_unknown"), e))?;

    if !info_check.status.success() {
        return Err(App::t_lang(lang_en, "log_docker_not_running").to_string());
    }

    // 执行 prune（-f 跳过交互确认，-a 删除所有未使用镜像）
    //
    // P1-3：这里**不带 --volumes**。数据卷里常放数据库、上传文件等用户数据，
    // 而 `prune --volumes` 是静默全删且不可恢复；界面上也没有让用户确认卷清单的环节。
    // 需要清理卷时请用 Docker Desktop 或 `docker volume prune`（它会列出卷并交互式确认）。
    let output = std::process::Command::new("docker")
        .args(["system", "prune", "-a", "-f"])
        .output()
        .map_err(|e| format!("{}: {}", App::t_lang(lang_en, "log_unknown"), e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(format!("docker prune: {}", stderr.trim()));
    }

    // 解析输出中的释放空间
    // 输出包含: "Total reclaimed space: 1.2GB"
    let reclaimed = stdout
        .lines()
        .find(|line| line.contains("Total reclaimed space"))
        .map(|line| line.split(':').nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_else(|| App::t_lang(lang_en, "log_unknown").to_string());

    Ok(App::tf_lang(lang_en, "log_docker_done", &[&reclaimed]))
}

/// 启动后台删除线程（两阶段自动删除）
/// 阶段1: 普通删除（多线程并行 rm -rf）
/// 阶段2: 对失败项自动 sudo 批量删除（后台并发，只弹一次密码框）
pub(crate) fn start_delete(
    to_delete: Vec<(String, String, Vec<String>, bool, u64)>,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
    auto_restore: bool,
    // 卸载 .app 时优先交给厂商自带的官方卸载器（M-1，仅 macOS 有意义）
    prefer_official_uninstaller: bool,
) {
    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        let failed_items: std::sync::Mutex<Vec<(String, String)>> =
            std::sync::Mutex::new(Vec::new());
        // M-2：本次删除实际删掉的项，结束后落一份清单。
        // 只记"真删掉了的"—— 不存在/被拒绝/失败的项不进清单，
        // 否则用户会以为还能还原一个从没被删过的东西。
        let backup_entries: std::sync::Mutex<Vec<crate::backup::BackupEntry>> =
            std::sync::Mutex::new(Vec::new());

        logger::info(&format!("删除任务开始: {} 项", to_delete.len()));

        // P1-1: Windows 批次删除前自动创建系统还原点（20h 频率限制，开关控制）
        #[cfg(not(target_os = "windows"))]
        let _ = auto_restore;
        #[cfg(target_os = "windows")]
        if auto_restore {
            let (ok, msg) = platform::windows_backup::ensure_restore_point(false);
            let text = match (ok, msg.as_str()) {
                (true, "created") => App::t_lang(lang_en, "restore_point_created").to_string(),
                (true, _) => App::t_lang(lang_en, "restore_point_skipped").to_string(),
                (false, _) => App::t_lang(lang_en, "restore_point_failed").to_string(),
            };
            let _ = tx.send(DeleteMessage::Info(text));
        }

        // ========== 阶段1: 普通删除（多线程并行） ==========
        let worker_count = std::cmp::min(4, to_delete.len().max(1));
        let idx = std::sync::atomic::AtomicUsize::new(0);

        std::thread::scope(|s| {
            for _ in 0..worker_count {
                s.spawn(|| {
                    loop {
                        let i = idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= to_delete.len() { break; }
                        let (path, category, batch_paths, use_trash, size_bytes) = &to_delete[i];
                        let path = path.to_string();
                        let category = category.to_string();
                        let batch_paths = batch_paths.clone();
                        let use_trash = *use_trash;
                        let size_bytes = *size_bytes;

                        // M-1：应用包优先交给官方卸载器。
                        // 删目录≠卸载软件：launchd 任务、pkgutil 收据、系统扩展
                        // 授权都清不掉，卸载完还会每分钟拉起一个已不存在的二进制。
                        // 启动失败不阻断 —— 回落到正常删除，用户至少还能卸掉文件。
                        #[cfg(target_os = "macos")]
                        if prefer_official_uninstaller && path.ends_with(".app") {
                            let name =
                                crate::scanner::official_uninstaller::app_display_name(&path);
                            if let Some(u) =
                                crate::scanner::official_uninstaller::find_official_uninstaller(
                                    &path, &name,
                                )
                            {
                                logger::info(&format!("交由官方卸载器处理: {} -> {}", path, u.path));
                                match crate::scanner::official_uninstaller::launch(&u) {
                                    Ok(_) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("↗ {} [{}] {}",
                                                App::t_lang(lang_en, "log_official_uninstaller"),
                                                category, path),
                                            path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                        continue;
                                    }
                                    Err(e) => {
                                        logger::warn(&format!(
                                            "官方卸载器启动失败，回落到删除目录: {} ({})",
                                            path, e
                                        ));
                                    }
                                }
                            }
                        }

                        // Windows 应用卸载特殊处理（干净卸载：卸载程序 + 扫描残留，不自动清理）
                        #[cfg(target_os = "windows")]
                        if path.starts_with("uwp:") || path.starts_with("uninstall:") {
                            logger::info(&format!("开始卸载应用: {}", path));

                            // 查找应用信息以支持干净卸载
                            let key_name = if path.starts_with("uninstall:") {
                                &path[10..]
                            } else {
                                ""
                            };
                            let app_info = scanner::windows_apps::find_app_by_key(key_name);
                            let app_name = app_info.as_ref().map(|a| a.name.as_str()).unwrap_or("");
                            let install_path = app_info.as_ref().and_then(|a| a.install_location.as_deref());

                            // 执行干净卸载（只卸载 + 扫描残留，不自动清理）
                            // 用户选择权：残留信息返回给用户，由用户决定是否清理
                            let (success, msg, residual) = scanner::windows_apps::clean_uninstall(
                                &path,
                                app_name,
                                install_path,
                            );

                            if success {
                                logger::info(&format!("卸载成功: {}", msg));
                            } else {
                                logger::error(&format!("卸载失败: {}", msg));
                            }

                            // 记录残留扫描详情（不自动清理，等待用户确认）
                            if !residual.is_empty() {
                                let fs_size: u64 = residual.filesystem.iter().map(|f| f.size).sum();
                                logger::info(&format!(
                                    "残留扫描结果: 注册表 {} 项 (可删 {}), 环境变量 {} 项 (可删 {}), 文件 {} 项 (可删 {}, 共 {}) — 等待用户选择",
                                    residual.registry.len(),
                                    residual.registry.iter().filter(|r| r.deletable).count(),
                                    residual.env_vars.len(),
                                    residual.env_vars.iter().filter(|e| e.deletable).count(),
                                    residual.filesystem.len(),
                                    residual.filesystem.iter().filter(|f| f.deletable).count(),
                                    scanner::format_size(fs_size),
                                ));
                                // 发送残留信息到 UI，弹出残留清理弹窗让用户选择
                                let _ = tx.send(DeleteMessage::ResidualFound(residual));
                            }

                            let _ = tx.send(DeleteMessage::Log(
                                if success { format!("✓ {}", msg) } else { format!("✗ {}", msg) },
                                path.clone(), category.clone(), success));
                            safety::log_deletion(&path, &category, success, None);
                            if !success {
                                failed_items.lock().unwrap_or_else(|e| e.into_inner()).push((path.clone(), category.clone()));
                            }
                            continue;
                        }

                        // 批量删除模式（如 __pycache__）：逐个安全删除
                        if !batch_paths.is_empty() {
                            let mut success_count = 0;
                            let mut fail_count = 0;
                            for bp in &batch_paths {
                                match safety::check_path_safety_with_category(bp, &category) {
                                    safety::SafetyCheck::Danger(reason) => {
                                        fail_count += 1;
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("⛔ {}", App::tf_lang(lang_en, "log_intercepted", &[bp, &reason])), bp.clone(), category.clone(), false));
                                        safety::log_deletion(bp, &category, false, Some(&reason));
                                        continue;
                                    }
                                    safety::SafetyCheck::Warning(reason) => {
                                        fail_count += 1;
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("⚠️ {}", App::tf_lang(lang_en, "log_skipped", &[bp, &reason])), bp.clone(), category.clone(), false));
                                        safety::log_deletion(bp, &category, false, Some(&reason));
                                        continue;
                                    }
                                    safety::SafetyCheck::Safe => {}
                                }
                                let p = std::path::Path::new(bp.as_str());
                                if best_effort_delete(p) {
                                    success_count += 1;
                                } else {
                                    fail_count += 1;
                                    failed_items.lock().unwrap_or_else(|e| e.into_inner()).push((bp.clone(), category.clone()));
                                }
                            }
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ {}", App::tf_lang(lang_en, "log_deleted", &[&category, &path, &success_count.to_string(), &fail_count.to_string()])),
                                path.clone(), category.clone(), fail_count == 0));
                            safety::log_deletion(&path, &category, fail_count == 0, None);
                            continue;
                        }

                        // 安全校验
                        match safety::check_path_safety_with_category(&path, &category) {
                            safety::SafetyCheck::Danger(reason) => {
                                let _ = tx.send(DeleteMessage::Log(
                                    format!("⛔ {}", App::tf_lang(lang_en, "log_intercepted", &[&path, &reason])), path.clone(), category.clone(), false));
                                safety::log_deletion(&path, &category, false, Some(&reason));
                                continue;
                            }
                            safety::SafetyCheck::Warning(reason) => {
                                let _ = tx.send(DeleteMessage::Log(
                                    format!("⚠️ {}", App::tf_lang(lang_en, "log_skipped", &[&path, &reason])), path.clone(), category.clone(), false));
                                safety::log_deletion(&path, &category, false, Some(&reason));
                                continue;
                            }
                            safety::SafetyCheck::Safe => {}
                        }

                        // APFS 快照特殊处理 (macOS 专属)
                        if cfg!(target_os = "macos") && category == "APFS快照" {
                            #[cfg(target_os = "macos")]
                            {
                                match scanner::apfs::delete_snapshot(&path) {
                                    Ok(_) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✓ {}", App::tf_lang(lang_en, "log_snapshot_deleted", &[&path])), path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                    }
                                    Err(e) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✗ {}", App::tf_lang(lang_en, "log_delete_failed", &[&path, &e])), path.clone(), category.clone(), false));
                                        safety::log_deletion(&path, &category, false, Some(&e));
                                    }
                                }
                            }
                            continue;
                        }

                        if cfg!(target_os = "macos") && category == "模拟器运行时" {
                            #[cfg(target_os = "macos")]
                            {
                                match scanner::apfs::delete_simulator_runtime(&path) {
                                    Ok(_) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✓ {}", App::tf_lang(lang_en, "log_runtime_deleted", &[&path])), path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                    }
                                    Err(e) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✗ {}", App::tf_lang(lang_en, "log_delete_failed", &[&path, &e])), path.clone(), category.clone(), false));
                                        safety::log_deletion(&path, &category, false, Some(&e));
                                    }
                                }
                            }
                            continue;
                        }

                        // 模拟器镜像/Cryptex — 通过 xcrun simctl runtime delete 安全删除 (macOS 专属)
                        if cfg!(target_os = "macos") && (category == "模拟器镜像" || category == "模拟器Cryptex") {
                            #[cfg(target_os = "macos")]
                            {
                                match delete_simulator_volumes(&path, lang_en) {
                                    Ok(msg) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✓ {}", msg), path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                    }
                                    Err(e) => {
                                        // xcrun 失败，加入 sudo 重试列表
                                        failed_items.lock().unwrap_or_else(|e| e.into_inner()).push((path.clone(), category.clone()));
                                        let _ = tx.send(DeleteMessage::Info(
                                            format!("🔄 {}", App::tf_lang(lang_en, "log_xcrun_failed", &[&e])),
                                        ));
                                    }
                                }
                            }
                            continue;
                        }

                        // 模拟器缓存 — 进程检测后删除 (macOS 专属)
                        if cfg!(target_os = "macos") && category == "模拟器缓存" {
                            #[cfg(target_os = "macos")]
                            {
                                if safety::is_simulator_running() {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("⏭️ {}", App::tf_lang(lang_en, "log_skip_running", &[&path])), path.clone(), category.clone(), false));
                                    safety::log_deletion(&path, &category, false, Some(App::t_lang(lang_en, "log_skip_running").replace("[{}]", "").trim()));
                                    continue;
                                }
                            }
                            // 走普通删除流程（会自动 fallback 到 sudo）
                        }

                        // Docker 清理 — 通过 docker system prune 命令清理
                        if category == "Docker清理" {
                            match run_docker_prune(lang_en) {
                                Ok(msg) => {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("✓ {}", msg), path.clone(), category.clone(), true));
                                    safety::log_deletion(&path, &category, true, None);
                                }
                                Err(e) => {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("✗ {}", App::tf_lang(lang_en, "log_docker_failed", &[&e])), path.clone(), category.clone(), false));
                                    safety::log_deletion(&path, &category, false, Some(&e));
                                }
                            }
                            continue;
                        }

                        // 普通文件/目录删除 - 尽力删除模式
                        let p = std::path::Path::new(path.as_str());

                        if !p.exists() && p.symlink_metadata().is_err() {
                            // 路径已不存在：视为删除成功（幂等性）。
                            // 用户想要的结果就是该路径消失，现在目标已经达成，
                            // 无需因缓存过期或外部已删除而报错。
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ {} ({})", App::tf_lang(lang_en, "log_path_not_exist", &[&path]),
                                    App::t_lang(lang_en, "already_cleaned")), path.clone(), category.clone(), true));
                            safety::log_deletion(&path, &category, true, None);
                            continue;
                        }

                        // 拒绝删除符号链接
                        if let Ok(meta) = p.symlink_metadata() {
                            if meta.file_type().is_symlink() {
                                let _ = tx.send(DeleteMessage::Log(
                                    format!("⛔ {}", App::tf_lang(lang_en, "log_symlink_rejected", &[&path])), path.clone(), category.clone(), false));
                                safety::log_deletion(&path, &category, false, Some(App::t_lang(lang_en, "log_symlink_rejected").replace("{}", "").trim()));
                                continue;
                            }
                        }

                        // 尽力删除：废纸篓模式或永久删除
                        // P0-3：use_trash 项在废纸篓失败时**不得**降级为永久删除，
                        // 也不得进入 sudo 重试列表（sudo rm -rf 会让它彻底不可恢复）
                        let deleted_ok = if use_trash {
                            move_to_trash(&path)
                        } else {
                            best_effort_delete(p)
                        };

                        if deleted_ok {
                            let action = App::t_lang(lang_en, if use_trash { "log_action_trashed" } else { "log_action_deleted" });
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ {} [{}] {}", action, category, path), path.clone(), category.clone(), true));
                            safety::log_deletion(&path, &category, true, None);

                            // M-2：真删掉一项，就往清单里记一笔。
                            // restorable 由平台判定：macOS 废纸篓可搬回，
                            // Windows 回收站没有稳定路径，一律 false（那边靠还原点）。
                            let restorable = crate::backup::is_restorable_by_move(
                                use_trash,
                                std::env::consts::OS,
                            );
                            if let Ok(mut guard) = backup_entries.lock() {
                                guard.push(crate::backup::BackupEntry {
                                    path: path.clone(),
                                    size_bytes,
                                    category: category.clone(),
                                    restorable,
                                });
                            }
                        } else if use_trash {
                            // 废纸篓失败：保留文件 + 明确告知，不进 sudo 重试列表
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✗ {}", App::tf_lang(lang_en, "log_trash_failed", &[&path])), path.clone(), category.clone(), false));
                            safety::log_deletion(&path, &category, false, Some(App::t_lang(lang_en, "log_trash_failed").replace("{}", "").trim()));
                        } else {
                            // 普通删除失败，加入待 sudo 列表
                            failed_items.lock().unwrap_or_else(|e| e.into_inner()).push((path, category));
                        }
                    }
                });
            }
        });

        // 毒化保护：若某个 worker panic，Mutex 会被 poison，这里必须仍能取出数据，
        // 否则整条删除链路连锁 panic，UI 永久卡在 Deleting 态
        let failed_items: Vec<(String, String)> =
            failed_items.into_inner().unwrap_or_else(|e| e.into_inner());
        let backup_entries: Vec<crate::backup::BackupEntry> = backup_entries
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());

        logger::info(&format!("阶段1删除完成, 失败 {} 项", failed_items.len()));

        // M-2：落清单。写盘失败只记日志、绝不影响删除结果 ——
        // 清单是事后追溯手段，不能因为它存不进去就把已删掉的东西说成没删。
        //
        // 边界说清楚：这份清单只覆盖本函数删掉的项。需要提权的那批走
        // 另一条链路（sudo 阶段），不进这份清单；它们本来就是永久删除，
        // 不可还原，进不进清单不改变"能不能后悔"这个结论。
        if let Some(id) = crate::backup::record(backup_entries.clone()) {
            let restorable = backup_entries.iter().filter(|e| e.restorable).count();
            let _ = tx.send(DeleteMessage::BackupRecorded {
                id,
                restorable,
                total: backup_entries.len(),
            });
        }

        // 普通删除完成后，若还有失败项，通知 GUI 弹出 egui 内置密码输入框
        if !failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(format!(
                "🔐 {}",
                App::tf_lang(lang_en, "log_need_sudo", &[&failed_items.len().to_string()])
            )));
            let _ = tx.send(DeleteMessage::NeedPassword(failed_items));
            return;
        }

        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 创建名称不可预测、仅属主可访问的临时文件，返回 (路径, 文件句柄)
///
/// 安全说明（P0-2）：旧实现把即将被 `sudo` 以 root 执行的脚本写在固定路径
/// （`/tmp/maclean_sudo_delete.sh` 等）。本地任意进程可以预先占位，或在写入后
/// 替换内容 → 以 root 执行任意代码，是标准的本地提权路径。
///
/// 对策：
/// - 文件名含纳秒时间戳 + pid + 尝试序号，不可预测
/// - `O_CREAT|O_EXCL`：已存在就换名重试，不覆盖已有文件
/// - `O_NOFOLLOW`：目标是符号链接则失败，防止被导向别处
/// - 权限 0600，属主外不可读写
#[cfg(unix)]
pub(crate) fn create_private_temp_file(
    prefix: &str,
    ext: &str,
) -> Option<(std::path::PathBuf, std::fs::File)> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    for attempt in 0..64u64 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(attempt as u128);
        let mixed = nanos ^ (((std::process::id() as u128) << 40) ^ ((attempt as u128) << 100));
        let name = format!("{}_{:032x}.{}", prefix, mixed, ext);
        let path = std::env::temp_dir().join(name);

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(mut file) => {
                if file.flush().is_ok() {
                    return Some((path, file));
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => continue,
        }
    }
    None
}

/// 写入一个受保护的临时文本文件，返回路径（见 create_private_temp_file 安全说明）
#[cfg(unix)]
pub(crate) fn write_private_temp_file(
    prefix: &str,
    ext: &str,
    content: &str,
) -> Option<std::path::PathBuf> {
    use std::io::Write;
    let (path, mut file) = create_private_temp_file(prefix, ext)?;
    file.write_all(content.as_bytes()).ok()?;
    file.flush().ok()?;
    Some(path)
}

/// 创建名称不可预测、独占打开的临时文件（Windows 实现）
///
/// Windows 没有 unix 的 `O_NOFOLLOW` / `mode(0o600)`，这里用等价手段达到同样的
/// 保护强度（设计目标与 unix 版一致）：
/// - `create_new(true)` → `CREATE_NEW`：目标已存在（**包括**那里已有一个符号链接
///   或 junction 占位）就直接失败，既不覆盖也不跟随，等价于 `O_EXCL | O_NOFOLLOW`
/// - `share_mode(0)`：独占打开，同机其它进程无法再读写该文件，等价于 0600 的作用
/// - 文件名混入纳秒时间戳 + pid + 尝试序号，不可用"猜下一个名字"的方式占位
///
/// 这条防线是为了让随后的 UAC 提权删除脚本不会被本地其它进程预先占位 / 替换，
/// 否则就是标准的本地提权路径（参见 unix 版注释里的 P0-2 说明）。
#[cfg(target_os = "windows")]
pub(crate) fn create_private_temp_file(
    prefix: &str,
    ext: &str,
) -> Option<(std::path::PathBuf, std::fs::File)> {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;

    const SHARE_NONE: u32 = 0;

    for attempt in 0..64u64 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(attempt as u128);
        let mixed = nanos ^ (((std::process::id() as u128) << 40) ^ ((attempt as u128) << 100));
        let name = format!("{}_{:032x}.{}", prefix, mixed, ext);
        let path = std::env::temp_dir().join(name);

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(SHARE_NONE)
            .open(&path)
        {
            Ok(mut file) => {
                if file.flush().is_ok() {
                    return Some((path, file));
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => continue,
        }
    }
    None
}

/// 写入一个受保护的临时文本文件，返回路径（见 create_private_temp_file 安全说明）
#[cfg(target_os = "windows")]
pub(crate) fn write_private_temp_file(
    prefix: &str,
    ext: &str,
    content: &str,
) -> Option<std::path::PathBuf> {
    use std::io::Write;
    let (path, mut file) = create_private_temp_file(prefix, ext)?;
    file.write_all(content.as_bytes()).ok()?;
    file.flush().ok()?;
    Some(path)
}

/// **不可逆删除之前的二次安全校验**（P0-1）
///
/// 名字里不再写 "sudo"：GUI 的提权删除和 CLI 的 `clean` 都要过这一层，
/// 语义是「执行删除的那一刻之前重做校验」，跟要不要提权无关。
///
/// 为什么必须复做：扫描时做过 `safety` 校验，但到真正删除之间有时间差，
/// 路径可能已被替换成别的东西（TOCTOU），也可能被换成了符号链接。
/// 这一层的删除一旦放行不可恢复，所以必须逐项重做。
/// 待删除项：(路径, 分类)
pub(crate) type DeleteItem = (String, String);
/// 被拦截项：(路径, 分类, 拦截原因)
pub(crate) type RejectedItem = (String, String, String);

/// 返回：`(允许放行的项, 被拦截的项及原因)`
pub(crate) fn sanitize_before_delete(
    items: Vec<DeleteItem>,
    lang_en: bool,
) -> (Vec<DeleteItem>, Vec<RejectedItem>) {
    let mut allowed: Vec<DeleteItem> = Vec::with_capacity(items.len());
    let mut rejected: Vec<RejectedItem> = Vec::new();

    for (path, category) in items {
        // 1. 重做安全校验（与阶段一相同规则）
        match safety::check_path_safety_with_category(&path, &category) {
            safety::SafetyCheck::Danger(reason) | safety::SafetyCheck::Warning(reason) => {
                rejected.push((path, category, reason));
                continue;
            }
            safety::SafetyCheck::Safe => {}
        }

        // 2. 符号链接复查：阶段一之后路径可能已被替换成软链，
        //    对软链执行 rm -rf 可能顺着链接删到链接目标之外的内容
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if meta.file_type().is_symlink() {
                rejected.push((
                    path,
                    category,
                    App::t_lang(lang_en, "log_sudo_symlink_rejected")
                        .replace("{}", "")
                        .trim()
                        .to_string(),
                ));
                continue;
            }
        }

        // 3. 拒绝含危险字符的路径：sudo 脚本用单引号包裹参数，
        //    换行 / 反引号 / $( 都可能突破引号造成命令注入
        if path.contains('\n') || path.contains('\r') || path.contains('`') || path.contains("$(") {
            rejected.push((
                path,
                category,
                App::t_lang(lang_en, "log_sudo_unsafe_path")
                    .replace("{}", "")
                    .trim()
                    .to_string(),
            ));
            continue;
        }

        allowed.push((path, category));
    }

    (allowed, rejected)
}

/// 启动后台 sudo 删除线程
/// 使用 egui 内置输入框收集到的密码，通过 sudo -S 的 stdin 传入，
/// 避免调用 System Events / osascript 触发钥匙串弹窗。
///
/// macOS 专属：`sudo -S` + `/bin/bash` 脚本 + `xcrun simctl` 三者共同构成这条链路，
/// 离开 macOS 都不成立。Windows 的实现走 UAC 提权，见下方同名函数。
#[cfg(target_os = "macos")]
pub(crate) fn start_sudo_delete(
    failed_items: Vec<(String, String)>,
    password: String,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    if failed_items.is_empty() {
        return;
    }

    // P0-1：sudo 以 root 执行 rm -rf，放行前必须重做安全校验
    let (failed_items, rejected) = sanitize_before_delete(failed_items, lang_en);

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        // 先播报被安全校验拦截的项，避免用户以为软件没干活
        for (path, category, reason) in &rejected {
            let _ = tx.send(DeleteMessage::Log(
                format!(
                    "⛔ {}",
                    App::tf_lang(lang_en, "log_sudo_rejected", &[path, reason])
                ),
                path.clone(),
                category.clone(),
                false,
            ));
            safety::log_deletion(path, category, false, Some(reason));
        }

        if failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(
                App::t_lang(lang_en, "log_sudo_phase").to_string(),
            ));
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(format!(
            "🔐 {}",
            App::t_lang(lang_en, "log_sudo_phase")
        )));

        // 日志文件也用受保护的随机名（避免被符号链接导向用户目录）
        let sudo_debug_log: Option<std::path::PathBuf> =
            write_private_temp_file("maclean_sudo_dbg", "log", "");
        let mut debug_entries: Vec<String> = Vec::new();
        debug_entries.push(format!(
            "[sudo phase] started, {} items",
            failed_items.len()
        ));
        for (p, c) in &failed_items {
            debug_entries.push(format!("  item: [{}] {}", c, p));
        }

        // ========== 预处理：模拟器镜像通过 sudo xcrun simctl runtime delete 删除 ==========
        let mut remaining_items: Vec<(String, String)> = Vec::new();
        for (path, category) in &failed_items {
            if category == "模拟器镜像" || category == "模拟器Cryptex" {
                // 用 sudo xcrun simctl runtime delete 删除所有运行时
                // P1：只删归属于本项、且系统标记 deletable 的 runtime。
                // 旧脚本在 root 下遍历"所有" runtime 逐个删除，勾选一项会连锅端。
                let sim_uuids: Vec<String> = resolve_simulator_uuids(path)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|u| u.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
                    .collect();
                if sim_uuids.is_empty() {
                    // 没有可删目标：保留该项，交给后续 sudo rm 清理残留目录
                    remaining_items.push((path.clone(), category.clone()));
                    continue;
                }
                let uuid_list = sim_uuids
                    .iter()
                    .map(|u| format!("'{}'", u))
                    .collect::<Vec<_>>()
                    .join(" ");
                let script = format!(
                    r#"#!/bin/bash
set +e
count=0
fail=0
for uuid in {}; do
    xcrun simctl runtime delete "$uuid" 2>/dev/null
    rc=$?
    if [ $rc -eq 0 ]; then
        count=$((count + 1))
    else
        fail=$((fail + 1))
    fi
done
echo "xcrun_deleted:$count"
echo "xcrun_failed:$fail"
exit 0
"#,
                    uuid_list
                );
                // P0-2：固定路径脚本可被本地进程预先占位或替换 → sudo 以 root 执行任意内容。
                // 改用不可预测文件名 + O_EXCL + 0600；脚本交给 /bin/bash 执行，不需要 +x。
                let Some(xcrun_script) = write_private_temp_file("maclean_xcrun", "sh", &script)
                else {
                    crate::logger::error("无法安全创建临时脚本，跳过 xcrun 删除");
                    remaining_items.push((path.clone(), category.clone()));
                    continue;
                };

                let xcrun_result = std::process::Command::new("/usr/bin/sudo")
                    .arg("-S")
                    .arg("/bin/bash")
                    .arg(&xcrun_script)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        if let Some(mut stdin) = child.stdin.take() {
                            use std::io::Write;
                            let _ = writeln!(stdin, "{}", password);
                        }
                        child.wait_with_output()
                    });

                let mut xcrun_success = false;
                match &xcrun_result {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        debug_entries.push(format!("xcrun sudo stdout: {}", stdout));
                        debug_entries.push(format!("xcrun sudo stderr: {}", stderr));

                        // 检查是否密码错误
                        if stderr.contains("incorrect password") || stderr.contains("3 incorrect") {
                            let _ = tx.send(DeleteMessage::Info(format!(
                                "🔒 {}",
                                App::t_lang(lang_en, "log_password_wrong")
                            )));
                            let _ = tx.send(DeleteMessage::NeedPassword(failed_items.clone()));
                            if let Some(p) = &sudo_debug_log {
                                let _ = std::fs::write(p, debug_entries.join("\n"));
                            }
                            let _ = std::fs::remove_file(&xcrun_script);
                            return;
                        }

                        // 检查 Volumes 目录是否已清空
                        let p = std::path::Path::new(path);
                        if !p.exists()
                            || std::fs::read_dir(p)
                                .map(|mut d| d.next().is_none())
                                .unwrap_or(true)
                        {
                            xcrun_success = true;
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✓ {}",
                                    App::tf_lang(
                                        lang_en,
                                        "log_touchid_runtime_deleted_path",
                                        &[path]
                                    )
                                ),
                                path.clone(),
                                category.clone(),
                                true,
                            ));
                            safety::log_deletion(path, category, true, None);
                        }
                    }
                    Err(e) => {
                        debug_entries.push(format!("xcrun sudo error: {}", e));
                    }
                }

                let _ = std::fs::remove_file(&xcrun_script);

                if !xcrun_success {
                    // xcrun 删除后仍有残留，用 sudo rm -rf 清理
                    remaining_items.push((path.clone(), category.clone()));
                }
            } else {
                remaining_items.push((path.clone(), category.clone()));
            }
        }

        let failed_items = remaining_items;

        if failed_items.is_empty() {
            if let Some(p) = &sudo_debug_log {
                let _ = std::fs::write(p, debug_entries.join("\n"));
            }
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        // 写临时删除脚本：所有目录后台并行删除
        let current_user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "root".to_string());
        let mut script_content = String::from("#!/bin/bash\nset +e\n");
        script_content.push_str("workdir=$(/usr/bin/mktemp -d)\n");
        script_content.push_str("trap \"/bin/rm -rf \\\"$workdir\\\"\" EXIT\n\n");
        script_content.push_str("process_one() {\n");
        script_content.push_str("  local idx=\"$1\"\n");
        script_content.push_str("  local path=\"$2\"\n");
        script_content.push_str("  local out=\"$workdir/${idx}.out\"\n");
        script_content.push_str("  echo \">MACLEAN_BEGIN:$path\" > \"$out\"\n");
        script_content.push_str("  /usr/bin/chflags -R nouchg \"$path\" 2>/dev/null\n");
        script_content.push_str("  /usr/sbin/chown -R '");
        script_content.push_str(&current_user.replace("'", "'\\''"));
        script_content.push_str(":staff' \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/chmod -R u+w \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/rm -rf \"$path\" 2>&1 >> \"$out\"\n");
        script_content.push_str("  echo \">MACLEAN_EXIT:$path:$?\" >> \"$out\"\n");
        script_content.push_str("}\n\n");

        for (i, (path, _)) in failed_items.iter().enumerate() {
            let escaped = path.replace("'", "'\\''");
            script_content.push_str(&format!("process_one {} '{}' &\n", i, escaped));
        }
        script_content.push_str("\nwait\n");
        script_content
            .push_str("for f in \"$workdir\"/*.out; do [ -f \"$f\" ] && /bin/cat \"$f\"; done\n");
        script_content.push_str("exit 0\n");
        // P0-2：随机名 + O_EXCL + 0600，杜绝"本地进程替换脚本 → root 执行任意内容"
        let Some(tmp_script) = write_private_temp_file("maclean_sudo_del", "sh", &script_content)
        else {
            crate::logger::error("无法安全创建删除脚本，放弃 sudo 删除");
            for (path, category) in &failed_items {
                let _ = tx.send(DeleteMessage::Log(
                    format!(
                        "✗ {}",
                        App::tf_lang(
                            lang_en,
                            "log_delete_failed",
                            &[path, "cannot create temp script securely"]
                        )
                    ),
                    path.clone(),
                    category.clone(),
                    false,
                ));
            }
            let _ = tx.send(DeleteMessage::Done);
            return;
        };

        debug_entries.push("--- delete script content ---".to_string());
        debug_entries.push(script_content.clone());

        // 使用 sudo -S，通过 stdin 传入密码，避免钥匙串弹窗
        let sudo_result = std::process::Command::new("/usr/bin/sudo")
            .arg("-S")
            .arg("/bin/bash")
            .arg(&tmp_script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = writeln!(stdin, "{}", password);
                }
                child.wait_with_output()
            });

        // 解析脚本输出，按路径收集错误信息
        let mut rm_stderr: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        match &sudo_result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                debug_entries.push(format!("sudo exit code: {:?}", output.status.code()));
                debug_entries.push(format!("sudo stdout:\n{}", stdout));
                debug_entries.push(format!("sudo stderr:\n{}", stderr));
                let mut current_path = String::new();
                for line in stdout.lines() {
                    if let Some(p) = line.strip_prefix(">MACLEAN_BEGIN:") {
                        current_path = p.to_string();
                    } else if let Some(_rest) = line.strip_prefix(">MACLEAN_EXIT:") {
                        current_path.clear();
                    } else if !line.is_empty() && !current_path.is_empty() {
                        rm_stderr
                            .entry(current_path.clone())
                            .or_default()
                            .push_str(line);
                        rm_stderr
                            .entry(current_path.clone())
                            .or_default()
                            .push('\n');
                    }
                }
            }
            Err(e) => {
                debug_entries.push(format!("sudo spawn error: {}", e));
            }
        }

        // 写入调试日志
        if let Some(p) = &sudo_debug_log {
            let _ = std::fs::write(p, debug_entries.join("\n"));
        }

        // 逐项验证删除结果
        match sudo_result {
            Ok(output) => {
                let stderr_all = String::from_utf8_lossy(&output.stderr);
                let sudo_failed = !output.status.success();
                let password_error = stderr_all.contains("sudo: 3 incorrect password attempts")
                    || stderr_all.contains("incorrect password");
                let user_cancelled = !password_error
                    && (stderr_all.contains("sudo: a password is required")
                        || stderr_all.contains("User canceled"));

                if password_error {
                    let _ = tx.send(DeleteMessage::Info(format!(
                        "🔒 {}",
                        App::t_lang(lang_en, "log_password_wrong")
                    )));
                    // 密码错误时 sudo 不会执行任何删除，全部项都需要重试
                    let _ = tx.send(DeleteMessage::NeedPassword(failed_items));
                    let _ = std::fs::remove_file(&tmp_script);
                    return;
                }

                for (path, category) in &failed_items {
                    let p = std::path::Path::new(path.as_str());
                    if !p.exists() && p.symlink_metadata().is_err() {
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✓ {}",
                                App::tf_lang(lang_en, "log_deleted_sudo", &[category, path])
                            ),
                            path.clone(),
                            category.clone(),
                            true,
                        ));
                        safety::log_deletion(path, category, true, None);
                        continue;
                    }

                    // 仍在：判断是 SIP 保护还是普通权限/占用问题
                    let err_text = rm_stderr
                        .get(path)
                        .map(|s| s.as_str())
                        .unwrap_or(&stderr_all);
                    let is_sip = err_text.contains("Operation not permitted")
                        || std::process::Command::new("/usr/bin/xattr")
                            .arg(path)
                            .output()
                            .map(|o| {
                                String::from_utf8_lossy(&o.stdout).contains("com.apple.provenance")
                            })
                            .unwrap_or(false)
                        || path.starts_with("/Library/Developer/CoreSimulator/Caches");

                    if is_sip {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("🔒 {}", App::tf_lang(lang_en, "log_sip_protected", &[path])),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(
                            path,
                            category,
                            false,
                            Some(App::t_lang(lang_en, "log_sip_reason")),
                        );
                    } else if user_cancelled {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ {}", App::tf_lang(lang_en, "log_cancelled_auth", &[path])),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(
                            path,
                            category,
                            false,
                            Some(App::t_lang(lang_en, "log_cancel_reason")),
                        );
                    } else if sudo_failed {
                        let detail = if stderr_all.is_empty() {
                            App::tf_lang(
                                lang_en,
                                "log_exit_code",
                                &[&output.status.code().unwrap_or(-1).to_string()],
                            )
                        } else {
                            stderr_all.trim().to_string()
                        };
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✗ {}",
                                App::tf_lang(lang_en, "log_delete_failed", &[path, &detail])
                            ),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(path, category, false, Some(&detail));
                    } else {
                        let detail = if err_text.is_empty() {
                            App::t_lang(lang_en, "log_still_exists").to_string()
                        } else {
                            err_text.trim().to_string()
                        };
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✗ {}",
                                App::tf_lang(lang_en, "log_delete_failed", &[path, &detail])
                            ),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(path, category, false, Some(&detail));
                    }
                }
            }
            Err(e) => {
                for (path, category) in &failed_items {
                    let _ = tx.send(DeleteMessage::Log(
                        format!(
                            "✗ {}",
                            App::tf_lang(lang_en, "log_cannot_start_sudo", &[path, &e.to_string()])
                        ),
                        path.clone(),
                        category.clone(),
                        false,
                    ));
                    safety::log_deletion(path, category, false, Some(&e.to_string()));
                }
            }
        }

        // 清理临时脚本
        let _ = std::fs::remove_file(&tmp_script);

        // 如果全部失败项仍在且不是因为密码错误，正常结束
        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 使用 Touch ID 的 sudo 删除（不需要密码，sudo 自动触发 Touch ID）
/// 启动 Touch ID 提权的删除线程
///
/// 返回是否真的拉起了线程 —— 调用方据此决定要不要保留 `delete_rx`。
/// （空 `failed_items` 时直接返回 false，不碰 `delete_rx`。）
///
/// macOS 专属：依赖 pam_tid / sudo_local 这套 Touch ID 提权机制。
#[cfg(target_os = "macos")]
pub(crate) fn start_sudo_delete_touchid(
    failed_items: Vec<(String, String)>,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) -> bool {
    if failed_items.is_empty() {
        return false;
    }

    // P0-1：与密码路径同样，放行前必须重做安全校验
    let (failed_items, rejected) = sanitize_before_delete(failed_items, lang_en);

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);
    let started = true;

    std::thread::spawn(move || {
        for (path, category, reason) in &rejected {
            let _ = tx.send(DeleteMessage::Log(
                format!(
                    "⛔ {}",
                    App::tf_lang(lang_en, "log_sudo_rejected", &[path, reason])
                ),
                path.clone(),
                category.clone(),
                false,
            ));
            safety::log_deletion(path, category, false, Some(reason));
        }

        if failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(
            App::t_lang(lang_en, "log_touchid_verifying").to_string(),
        ));

        let sudo_debug_log: Option<std::path::PathBuf> =
            write_private_temp_file("maclean_sudo_tid_dbg", "log", "");
        let mut debug_entries: Vec<String> = Vec::new();
        debug_entries.push(format!(
            "[touchid sudo] started, {} items",
            failed_items.len()
        ));

        // 预处理：模拟器镜像通过 xcrun simctl runtime delete
        let mut remaining_items: Vec<(String, String)> = Vec::new();
        for (path, category) in &failed_items {
            if category == "模拟器镜像" || category == "模拟器Cryptex" {
                let _ = tx.send(DeleteMessage::Info(format!(
                    "🔄 {}",
                    App::tf_lang(lang_en, "log_touchid_prepare_xcrun", &[category])
                )));

                let script = r#"#!/bin/bash
set +e
uuids=$(xcrun simctl runtime list 2>/dev/null | grep -oE '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}' | sort -u)
count=0
fail=0
for uuid in $uuids; do
    xcrun simctl runtime delete "$uuid" 2>/dev/null
    rc=$?
    if [ $rc -eq 0 ]; then
        count=$((count + 1))
    else
        fail=$((fail + 1))
    fi
done
echo "xcrun_deleted:$count"
echo "xcrun_failed:$fail"
exit 0
"#;
                // P0-2：固定路径脚本可被本地进程预先占位或替换 → sudo 以 root 执行任意内容。
                // 改用不可预测文件名 + O_EXCL + 0600；脚本交给 /bin/bash 执行，不需要 +x。
                let Some(xcrun_script) = write_private_temp_file("maclean_xcrun", "sh", script)
                else {
                    crate::logger::error("无法安全创建临时脚本，跳过 xcrun 删除");
                    remaining_items.push((path.clone(), category.clone()));
                    continue;
                };

                // 先清除 sudo 票据，确保能触发 Touch ID
                let _ = std::process::Command::new("/usr/bin/sudo")
                    .arg("-k")
                    .output();

                let _ = tx.send(DeleteMessage::Info(format!(
                    "⏳ {}",
                    App::t_lang(lang_en, "log_wait_touchid")
                )));

                // P0-2：随机名 + O_EXCL + 0600，避免日志被符号链接劫持
                let (xcrun_log, xcrun_log_file) =
                    match create_private_temp_file("maclean_xcrun_tid", "log") {
                        Some(v) => v,
                        None => {
                            remaining_items.push((path.clone(), category.clone()));
                            continue;
                        }
                    };
                let xcrun_log_file = Some(xcrun_log_file);

                // 不用 -S，sudo 会自动弹出 Touch ID；stdout 重定向到文件避免 pipe 死锁
                let mut cmd = std::process::Command::new("/usr/bin/sudo");
                cmd.arg("/bin/bash").arg(&xcrun_script);
                if let Some(file) = xcrun_log_file {
                    cmd.stdout(file);
                }
                let xcrun_result = cmd.status();

                let mut xcrun_success = false;
                let xcrun_stdout = std::fs::read_to_string(&xcrun_log).unwrap_or_default();
                debug_entries.push(format!("xcrun touchid stdout:\n{}", xcrun_stdout));

                let _ = tx.send(DeleteMessage::Info(format!(
                    "✅ {}",
                    App::tf_lang(
                        lang_en,
                        "log_xcrun_done",
                        &[&xcrun_stdout.trim().replace('\n', " ")]
                    )
                )));

                match &xcrun_result {
                    Ok(status) => {
                        if !status.success() {
                            // Touch ID 可能被取消
                            if xcrun_stdout.contains("canceled")
                                || xcrun_stdout.contains("cancelled")
                            {
                                let _ = tx.send(DeleteMessage::Info(format!(
                                    "🔒 {}",
                                    App::t_lang(lang_en, "log_touchid_cancel")
                                )));
                                for (p, c) in &failed_items {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!(
                                            "✗ {}: {}",
                                            App::t_lang(lang_en, "log_touchid_cancel"),
                                            p
                                        ),
                                        p.clone(),
                                        c.clone(),
                                        false,
                                    ));
                                    safety::log_deletion(
                                        p,
                                        c,
                                        false,
                                        Some(App::t_lang(lang_en, "log_touchid_cancel")),
                                    );
                                }
                                if let Some(p) = &sudo_debug_log {
                                    let _ = std::fs::write(p, debug_entries.join("\n"));
                                }
                                let _ = std::fs::remove_file(&xcrun_script);
                                let _ = std::fs::remove_file(&xcrun_log);
                                let _ = tx.send(DeleteMessage::Done);
                                return;
                            }
                        }

                        // xcrun 删除成功即可认为该模拟器镜像项已清理
                        // 不必要求 Volumes 目录为空（mount point 会残留，且受 SIP 保护）
                        let deleted_count = xcrun_stdout
                            .lines()
                            .find(|l| l.starts_with("xcrun_deleted:"))
                            .and_then(|l| l.strip_prefix("xcrun_deleted:"))
                            .and_then(|n| n.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        let failed_count = xcrun_stdout
                            .lines()
                            .find(|l| l.starts_with("xcrun_failed:"))
                            .and_then(|l| l.strip_prefix("xcrun_failed:"))
                            .and_then(|n| n.trim().parse::<usize>().ok())
                            .unwrap_or(0);

                        if deleted_count > 0 && failed_count == 0 {
                            xcrun_success = true;
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✓ {}",
                                    App::tf_lang(
                                        lang_en,
                                        "log_touchid_xcrun_deleted",
                                        &[&deleted_count.to_string()]
                                    )
                                ),
                                path.clone(),
                                category.clone(),
                                true,
                            ));
                            safety::log_deletion(path, category, true, None);
                        }
                    }
                    Err(e) => {
                        debug_entries.push(format!("xcrun touchid error: {}", e));
                    }
                }

                let _ = std::fs::remove_file(&xcrun_script);
                let _ = std::fs::remove_file(&xcrun_log);

                if !xcrun_success {
                    remaining_items.push((path.clone(), category.clone()));
                }
            } else {
                remaining_items.push((path.clone(), category.clone()));
            }
        }

        let failed_items = remaining_items;

        if failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(format!(
                "✅ {}",
                App::t_lang(lang_en, "log_no_sudo_needed")
            )));
            if let Some(p) = &sudo_debug_log {
                let _ = std::fs::write(p, debug_entries.join("\n"));
            }
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(format!(
            "🔄 {}",
            App::tf_lang(
                lang_en,
                "log_sudo_execute",
                &[&failed_items.len().to_string()]
            )
        )));

        // 写临时删除脚本：并行删除
        let current_user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "root".to_string());
        let mut script_content = String::from("#!/bin/bash\nset +e\n");
        script_content.push_str("workdir=$(/usr/bin/mktemp -d)\n");
        script_content.push_str("trap \"/bin/rm -rf \\\"$workdir\\\"\" EXIT\n\n");
        script_content.push_str("process_one() {\n");
        script_content.push_str("  local idx=\"$1\"\n");
        script_content.push_str("  local path=\"$2\"\n");
        script_content.push_str("  local out=\"$workdir/${idx}.out\"\n");
        script_content.push_str("  echo \">MACLEAN_BEGIN:$path\" > \"$out\"\n");
        script_content.push_str("  /usr/bin/chflags -R nouchg \"$path\" 2>/dev/null\n");
        script_content.push_str("  /usr/sbin/chown -R '");
        script_content.push_str(&current_user.replace("'", "'\\''"));
        script_content.push_str(":staff' \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/chmod -R u+w \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/rm -rf \"$path\" 2>&1 >> \"$out\"\n");
        script_content.push_str("  echo \">MACLEAN_EXIT:$path:$?\" >> \"$out\"\n");
        script_content.push_str("}\n\n");

        for (i, (path, _)) in failed_items.iter().enumerate() {
            let escaped = path.replace("'", "'\\''");
            script_content.push_str(&format!("process_one {} '{}' &\n", i, escaped));
        }
        script_content.push_str("\nwait\n");
        script_content
            .push_str("for f in \"$workdir\"/*.out; do [ -f \"$f\" ] && /bin/cat \"$f\"; done\n");
        script_content.push_str("exit 0\n");
        // P0-2：随机名 + O_EXCL + 0600
        let Some(tmp_script) = write_private_temp_file("maclean_sudo_tid", "sh", &script_content)
        else {
            crate::logger::error("无法安全创建删除脚本，放弃 Touch ID 删除");
            let _ = tx.send(DeleteMessage::Done);
            return;
        };
        debug_entries.push(format!("delete script: {}", tmp_script.display()));

        // 日志文件同样用受保护的随机名，避免被符号链接导向别处
        let Some((sudo_log, sudo_log_file)) =
            create_private_temp_file("maclean_sudo_tid_out", "log")
        else {
            crate::logger::error("无法安全创建日志文件，放弃 Touch ID 删除");
            let _ = tx.send(DeleteMessage::Done);
            return;
        };
        let sudo_log_file = Some(sudo_log_file);

        let _ = tx.send(DeleteMessage::Info(format!(
            "⏳ {}",
            App::t_lang(lang_en, "log_wait_touchid")
        )));

        // 不用 -S，sudo 自动触发 Touch ID；stdout 重定向到文件避免 pipe 死锁
        let mut sudo_cmd = std::process::Command::new("/usr/bin/sudo");
        sudo_cmd.arg("/bin/bash").arg(&tmp_script);
        if let Some(file) = sudo_log_file {
            sudo_cmd.stdout(file);
        }
        let sudo_result = sudo_cmd.status();

        let _ = tx.send(DeleteMessage::Info(format!(
            "✅ {}",
            App::t_lang(lang_en, "log_sudo_done2")
        )));

        // 解析输出
        let sudo_stdout = std::fs::read_to_string(&sudo_log).unwrap_or_default();
        debug_entries.push(format!(
            "sudo touchid exit code: {:?}",
            sudo_result.as_ref().ok().and_then(|s| s.code())
        ));
        debug_entries.push(format!("sudo touchid stdout:\n{}", sudo_stdout));

        let mut rm_stderr: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut current_path = String::new();
        for line in sudo_stdout.lines() {
            if let Some(p) = line.strip_prefix(">MACLEAN_BEGIN:") {
                current_path = p.to_string();
            } else if let Some(_rest) = line.strip_prefix(">MACLEAN_EXIT:") {
                current_path.clear();
            } else if !line.is_empty() && !current_path.is_empty() {
                rm_stderr
                    .entry(current_path.clone())
                    .or_default()
                    .push_str(line);
                rm_stderr
                    .entry(current_path.clone())
                    .or_default()
                    .push('\n');
            }
        }

        if let Some(p) = &sudo_debug_log {
            let _ = std::fs::write(p, debug_entries.join("\n"));
        }
        let _ = std::fs::remove_file(&sudo_log);

        // 逐项验证
        match sudo_result {
            Ok(_) => {
                let user_cancelled = sudo_stdout.contains("canceled")
                    || sudo_stdout.contains("cancelled")
                    || sudo_stdout.contains("User canceled");

                if user_cancelled {
                    let _ = tx.send(DeleteMessage::Info(format!(
                        "🔒 {}",
                        App::t_lang(lang_en, "log_touchid_cancel")
                    )));
                    for (path, category) in &failed_items {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ {}: {}", App::t_lang(lang_en, "log_touchid_cancel"), path),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(
                            path,
                            category,
                            false,
                            Some(App::t_lang(lang_en, "log_touchid_cancel")),
                        );
                    }
                    let _ = std::fs::remove_file(&tmp_script);
                    let _ = tx.send(DeleteMessage::Done);
                    return;
                }

                for (path, category) in &failed_items {
                    let p = std::path::Path::new(path.as_str());
                    if !p.exists() && p.symlink_metadata().is_err() {
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✓ {}",
                                App::tf_lang(lang_en, "log_deleted_touchid", &[category, path])
                            ),
                            path.clone(),
                            category.clone(),
                            true,
                        ));
                        safety::log_deletion(path, category, true, None);
                    } else {
                        let err_text = rm_stderr.get(path).map(|s| s.as_str()).unwrap_or("");
                        let is_sip = err_text.contains("Operation not permitted")
                            || path.starts_with("/Library/Developer/CoreSimulator/Caches");

                        if is_sip {
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "🔒 {}",
                                    App::tf_lang(lang_en, "log_sip_protected", &[path])
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            ));
                            safety::log_deletion(
                                path,
                                category,
                                false,
                                Some(App::t_lang(lang_en, "log_sip_reason")),
                            );
                        } else {
                            let detail = if err_text.is_empty() {
                                App::t_lang(lang_en, "log_still_exists").to_string()
                            } else {
                                err_text.trim().to_string()
                            };
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✗ {}",
                                    App::tf_lang(lang_en, "log_delete_failed", &[path, &detail])
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            ));
                            safety::log_deletion(path, category, false, Some(&detail));
                        }
                    }
                }
            }
            Err(e) => {
                for (path, category) in &failed_items {
                    let _ = tx.send(DeleteMessage::Log(
                        format!(
                            "✗ {}",
                            App::tf_lang(lang_en, "log_cannot_start_sudo", &[path, &e.to_string()])
                        ),
                        path.clone(),
                        category.clone(),
                        false,
                    ));
                    safety::log_deletion(path, category, false, Some(&e.to_string()));
                }
            }
        }

        let _ = std::fs::remove_file(&tmp_script);
        let _ = tx.send(DeleteMessage::Done);
    });

    started
}

/// 跨平台在默认浏览器打开 URL
pub(crate) fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// 执行单个优化任务
pub(crate) fn execute_optimize_task(task_name: &str, lang_en: bool) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Windows 平台：路由到 Windows 专属实现
    #[cfg(target_os = "windows")]
    let result = execute_windows_optimize_task(task_name, lang_en);

    // macOS 及其他平台：原有 macOS 实现
    #[cfg(not(target_os = "windows"))]
    let result = execute_macos_optimize_task(task_name, lang_en);

    format!("[{}] {}", timestamp, result)
}

/// macOS 优化任务执行
#[cfg(not(target_os = "windows"))]
pub(crate) fn execute_macos_optimize_task(task_name: &str, lang_en: bool) -> String {
    match task_name {
        "dns_cache_flush" => {
            // dscacheutil 和 killall 在现代 macOS 上需要 sudo
            let r1 = std::process::Command::new("dscacheutil")
                .arg("-flushcache")
                .output();
            let r2 = std::process::Command::new("killall")
                .arg("-HUP")
                .arg("mDNSResponder")
                .output();
            let success = r1.map(|o| o.status.success()).unwrap_or(false)
                && r2.map(|o| o.status.success()).unwrap_or(false);
            if success {
                App::t_lang(lang_en, "opt_dns_success").to_string()
            } else {
                App::t_lang(lang_en, "opt_dns_fail").to_string()
            }
        }
        "quicklook_rebuild" => {
            let r = std::process::Command::new("qlmanage")
                .arg("-r")
                .arg("cache")
                .output();
            match r {
                Ok(_) => App::t_lang(lang_en, "opt_quicklook_success").to_string(),
                Err(_) => App::t_lang(lang_en, "opt_quicklook_fail").to_string(),
            }
        }
        "launchservices_rebuild" => {
            // lsregister 路径在 macOS 10.0-15 上一致，但加 fallback 更稳健
            let lsregister_candidates = [
                "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister",
                "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
            ];
            let lsregister = lsregister_candidates
                .iter()
                .find(|p| std::path::Path::new(p).exists())
                .unwrap_or(&lsregister_candidates[0]);
            let r = std::process::Command::new(lsregister).arg("-gc").output();
            match r {
                Ok(_) => App::t_lang(lang_en, "opt_launchservices_success").to_string(),
                Err(_) => App::t_lang(lang_en, "opt_launchservices_fail").to_string(),
            }
        }
        "saved_state_cleanup" => {
            let home = std::env::var("HOME").unwrap_or_default();
            let state_dir = format!("{}/Library/Saved Application State", home);
            let mut count = 0;
            if let Ok(entries) = std::fs::read_dir(&state_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_dir() {
                        // 检查修改时间是否超过 30 天
                        if let Ok(meta) = path.metadata() {
                            if let Ok(mtime) = meta.modified() {
                                if let Ok(age) = mtime.elapsed() {
                                    if age.as_secs() > 30 * 86400 {
                                        let _ = std::fs::remove_dir_all(&path);
                                        count += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            App::tf_lang(lang_en, "opt_saved_state_success", &[&count.to_string()])
        }
        "gatekeeper_cleanup" => {
            let home = std::env::var("HOME").unwrap_or_default();
            let db_path = format!(
                "{}/Library/Preferences/com.apple.LaunchServices.QuarantineEventsV2",
                home
            );
            if std::path::Path::new(&db_path).exists() {
                let r = std::process::Command::new("sqlite3")
                    .arg(&db_path)
                    .arg("DELETE FROM LSQuarantineEvent; VACUUM;")
                    .output();
                match r {
                    Ok(_) => App::t_lang(lang_en, "opt_gatekeeper_success").to_string(),
                    Err(_) => App::t_lang(lang_en, "opt_gatekeeper_fail").to_string(),
                }
            } else {
                App::t_lang(lang_en, "opt_gatekeeper_empty").to_string()
            }
        }
        "memory_pressure_release" => {
            // purge 在所有 macOS 版本上都需要 sudo
            let r = std::process::Command::new("purge").output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_memory_success").to_string()
                }
                _ => App::t_lang(lang_en, "opt_memory_fail").to_string(),
            }
        }
        "spotlight_reindex" => {
            // mdutil -E / 重建根卷的 Spotlight 索引
            let r = std::process::Command::new("mdutil")
                .arg("-E")
                .arg("/")
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_spotlight_success").to_string()
                }
                _ => App::t_lang(lang_en, "opt_spotlight_fail").to_string(),
            }
        }
        "login_items_audit" => {
            // 打开系统设置 > 通用 > 登录项
            // macOS 13+ 使用 "x-apple.systempreferences:com.apple.LoginItems-Settings.extension"
            // macOS 12 及以下使用 "com.apple.preference.users"
            let url = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";
            let r = std::process::Command::new("open").arg(url).output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_login_items_opened").to_string()
                }
                _ => App::t_lang(lang_en, "opt_login_items_fail").to_string(),
            }
        }
        _ => App::tf_lang(lang_en, "opt_unknown", &[task_name]),
    }
}

/// Windows 优化任务执行
///
/// 参考 Win11Debloat (https://github.com/Raphire/Win11Debloat) 实现。
/// 注册表操作统一通过 `reg.exe` 命令执行，避免引入 winreg 等额外依赖。
#[cfg(target_os = "windows")]
pub(crate) fn execute_windows_optimize_task(task_name: &str, lang_en: bool) -> String {
    // P1-2: 注册表修改类任务执行前自动备份相关键（reg export 到备份目录）
    let reg_keys: &[&str] = match task_name {
        "win_disable_telemetry" => &[r"HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection"],
        "win_disable_copilot" => &[r"HKCU\Software\Policies\Microsoft\Windows\WindowsCopilot"],
        "win_disable_suggestions" => &[
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager",
        ],
        "win_disable_fast_startup" => {
            &[r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Power"]
        }
        _ => &[],
    };
    for key in reg_keys {
        platform::windows_backup::backup_registry_key(key, task_name);
    }

    match task_name {
        "win_dns_flush" => {
            let r = std::process::Command::new("ipconfig")
                .arg("/flushdns")
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_win_dns_success").to_string()
                }
                _ => App::t_lang(lang_en, "opt_win_dns_fail").to_string(),
            }
        }
        "win_temp_cleanup" => {
            // 清理临时目录、缩略图缓存、交付优化缓存
            let mut total_cleaned = 0u64;

            // 1. 用户临时文件
            if let Ok(temp) = std::env::var("TEMP") {
                total_cleaned += clean_dir_size(&temp);
            }
            if let Ok(tmp) = std::env::var("TMP") {
                total_cleaned += clean_dir_size(&tmp);
            }
            // 2. Windows 临时文件
            total_cleaned += clean_dir_size(r"C:\Windows\Temp");
            // 3. 缩略图缓存
            if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
                let thumb_cache = format!(r"{}\Microsoft\Windows\Explorer", localappdata);
                total_cleaned += clean_dir_size(&thumb_cache);
            }
            // 4. 交付优化缓存
            let _ = std::process::Command::new("cleanmgr")
                .args(["/sagerun:1", "/d", "C:"])
                .output();

            App::tf_lang(
                lang_en,
                "opt_win_temp_success",
                &[&crate::scanner::format_size(total_cleaned)],
            )
        }
        "win_disable_telemetry" => {
            // HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection AllowTelemetry = 0
            let r = std::process::Command::new("reg")
                .args([
                    "add",
                    r"HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection",
                    "/v",
                    "AllowTelemetry",
                    "/t",
                    "REG_DWORD",
                    "/d",
                    "0",
                    "/f",
                ])
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_win_telemetry_success").to_string()
                }
                _ => App::t_lang(lang_en, "opt_win_telemetry_fail").to_string(),
            }
        }
        "win_disable_copilot" => {
            // HKCU\Software\Policies\Microsoft\Windows\WindowsCopilot TurnOffWindowsCopilot = 1
            let r = std::process::Command::new("reg")
                .args([
                    "add",
                    r"HKCU\Software\Policies\Microsoft\Windows\WindowsCopilot",
                    "/v",
                    "TurnOffWindowsCopilot",
                    "/t",
                    "REG_DWORD",
                    "/d",
                    "1",
                    "/f",
                ])
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_win_copilot_success").to_string()
                }
                _ => App::t_lang(lang_en, "opt_win_copilot_fail").to_string(),
            }
        }
        "win_disable_suggestions" => {
            // 关闭开始菜单建议、锁屏广告、设置建议
            let keys = [
                (
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
                    "Start_IrisRecommendations",
                ),
                (
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager",
                    "SubscribedContent-338388Enabled",
                ),
                (
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager",
                    "SubscribedContent-338389Enabled",
                ),
                (
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager",
                    "SystemPaneSuggestionsEnabled",
                ),
            ];
            let mut success = true;
            for (key, value) in keys {
                let r = std::process::Command::new("reg")
                    .args(["add", key, "/v", value, "/t", "REG_DWORD", "/d", "0", "/f"])
                    .output();
                if !r.map(|o| o.status.success()).unwrap_or(false) {
                    success = false;
                }
            }
            if success {
                App::t_lang(lang_en, "opt_win_suggestions_success").to_string()
            } else {
                App::t_lang(lang_en, "opt_win_suggestions_fail").to_string()
            }
        }
        "win_startup_audit" => {
            // 打开任务管理器启动页
            let r = std::process::Command::new("taskmgr").arg("/4").spawn();
            match r {
                Ok(_) => App::t_lang(lang_en, "opt_win_startup_opened").to_string(),
                Err(_) => App::t_lang(lang_en, "opt_win_startup_fail").to_string(),
            }
        }
        "win_disable_fast_startup" => {
            // HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Power HiberbootEnabled = 0
            let r = std::process::Command::new("reg")
                .args([
                    "add",
                    r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Power",
                    "/v",
                    "HiberbootEnabled",
                    "/t",
                    "REG_DWORD",
                    "/d",
                    "0",
                    "/f",
                ])
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_win_faststartup_success").to_string()
                }
                _ => App::t_lang(lang_en, "opt_win_faststartup_fail").to_string(),
            }
        }
        "win_restore_point" => {
            // 手动触发：强制创建还原点（跳过 20h 频率限制）
            let (ok, _msg) = platform::windows_backup::ensure_restore_point(true);
            if ok {
                App::t_lang(lang_en, "opt_win_restore_success").to_string()
            } else {
                App::t_lang(lang_en, "opt_win_restore_fail").to_string()
            }
        }
        "win_restart_explorer" => {
            // taskkill /f /im explorer.exe && start explorer.exe
            let r1 = std::process::Command::new("taskkill")
                .args(["/f", "/im", "explorer.exe"])
                .output();
            let r2 = std::process::Command::new("explorer.exe").spawn();
            let success = r1.map(|o| o.status.success()).unwrap_or(false) && r2.is_ok();
            if success {
                App::t_lang(lang_en, "opt_win_explorer_success").to_string()
            } else {
                App::t_lang(lang_en, "opt_win_explorer_fail").to_string()
            }
        }
        "win_trim_drives" => {
            // 对系统盘执行 TRIM / 碎片整理
            let r = std::process::Command::new("defrag")
                .args(["C:", "/O", "/H"])
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_win_trim_success").to_string()
                }
                _ => App::t_lang(lang_en, "opt_win_trim_fail").to_string(),
            }
        }
        _ => App::tf_lang(lang_en, "opt_unknown", &[task_name]),
    }
}

/// 计算并清空目录，返回清理的字节数
#[cfg(target_os = "windows")]
pub(crate) fn clean_dir_size(dir: &str) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if let Ok(meta) = path.metadata() {
                total += meta.len();
            }
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    total
}

// =========================================================================
//  Windows UAC 提权删除
// =========================================================================
//
//  macOS 侧用 `sudo -S` 复用系统授权；Windows 没有 sudo，正确做法是 UAC 提权
//  （`Start-Process -Verb RunAs`），由系统弹窗完成授权。
//
//  下面的 `escape_ps_single_quoted` / `build_uac_delete_script` 故意不加
//  `#[cfg(target_os = "windows")]`：它们是纯字符串处理，跨平台可编译，
//  因此能在开发机（macOS）上跑真正的单元测试。命令构造是这个链路里唯一
//  可确定性测试的部分，也是最容易被注入突破的部分，必须有测试兜。

/// PowerShell 单引号字符串内的转义：`'` → `''`
///
/// 删除脚本里的路径一律用单引号字符串字面量承载（单引号串不做变量展开、
/// 不解释反引号，语义最接近"原样传参"）。唯一能突破它的是路径里自带的
/// 单引号，所以只需要把 `'` 翻倍。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn escape_ps_single_quoted(s: &str) -> String {
    s.replace('\'', "''")
}

/// 生成 UAC 提权删除脚本的内容
///
/// `items` 为 (路径, 类别)，`result_path` 为脚本写回逐项结果的位置。
/// 脚本对每项输出一行 `OK\t<path>` 或 `FAIL\t<path>\t<原因>`，主进程据此上报。
///
/// 注意 `Remove-Item` 必须走 `-LiteralPath`：默认的 `-Path` 会把 `[` `]`
/// 当通配符解释，含方括号的目录（如 `foo[1]`）会被匹配错或直接报错。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn build_uac_delete_script(items: &[(String, String)], result_path: &str) -> String {
    let mut s = String::new();
    s.push_str("$ErrorActionPreference = 'Stop'\n");
    s.push_str("$lines = New-Object System.Collections.Generic.List[string]\n");
    s.push_str("$paths = @(\n");
    for (path, _category) in items {
        // 路径中含 `;` 等字符在单引号字面量里是安全的，只需转义单引号
        s.push_str(&format!("'{}'\n", escape_ps_single_quoted(path)));
    }
    s.push_str(")\n");
    s.push_str(
        r#"foreach ($p in $paths) {
    if ([string]::IsNullOrWhiteSpace($p)) { continue }
    try {
        if (Test-Path -LiteralPath $p) {
            Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction Stop
        }
        if (Test-Path -LiteralPath $p) {
            [void]$lines.Add("FAIL`t$p`tdelete reported no error but target still exists")
        } else {
            [void]$lines.Add("OK`t$p")
        }
    } catch {
        [void]$lines.Add("FAIL`t$p`t$($_.Exception.Message)")
    }
}
"#,
    );
    s.push_str(&format!(
        "[System.IO.File]::WriteAllLines('{}', $lines, [System.Text.UTF8Encoding]::new($false))\n",
        escape_ps_single_quoted(result_path)
    ));
    s
}

/// 启动 UAC 提权删除线程（Windows 实现）
///
/// 与 macOS 版 `start_sudo_delete` 同签名（方便 UI 层不分平台调用）：
/// 普通删除失败的项目走到这里，用管理员权限重试一次。
///
/// 安全约束（与 macOS 侧对齐）：
/// - 放行前用 `sanitize_before_delete` 逐项重做 safety 校验，防阶段间的 TOCTOU
/// - 脚本与结果文件都写在不可预测、独占打开的临时文件里（见各自的
///   `create_private_temp_file` 说明），避免被本地进程占位后以管理员执行
/// - 路径经单引号转义后嵌入脚本
///
/// `password` 参数在 Windows 上不使用：授权由系统 UAC 弹窗完成，
/// 自绘输入框收集密码既不安全也无处可用。这里显式消费掉避免误用。
#[cfg(target_os = "windows")]
pub(crate) fn start_sudo_delete(
    failed_items: Vec<(String, String)>,
    password: String,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    // Windows 不做命令行密码，交给 UAC
    let _ = password;

    if failed_items.is_empty() {
        return;
    }

    // P0-1：以管理员权限删除前必须重做安全校验
    let (failed_items, rejected) = sanitize_before_delete(failed_items, lang_en);

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        // 先播报被安全校验拦截的项，避免用户以为软件没干活
        for (path, category, reason) in rejected {
            let line = App::tf_lang(lang_en, "log_sudo_rejected", &[&path, &reason]);
            let _ = tx.send(DeleteMessage::Log(line, path, category, false));
        }

        if failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(
            App::t_lang(lang_en, "log_sudo_phase").to_string(),
        ));

        // path -> category，结果文件里只有路径，需要回填类别来发日志
        let category_of: std::collections::HashMap<String, String> = failed_items
            .iter()
            .map(|(p, c)| (p.clone(), c.clone()))
            .collect();

        // 结果文件：脚本以管理员身份写回逐项结果
        let Some(result_path) = write_private_temp_file("maclean_uac_res", "log", "") else {
            crate::logger::error("无法安全创建结果文件，放弃 UAC 提权删除");
            let _ = tx.send(DeleteMessage::Done);
            return;
        };
        let result_path_str = result_path.to_string_lossy().to_string();

        let Some(script_path) = write_private_temp_file(
            "maclean_uac_del",
            "ps1",
            &build_uac_delete_script(&failed_items, &result_path_str),
        ) else {
            crate::logger::error("无法安全创建删除脚本，放弃 UAC 提权删除");
            let _ = std::fs::remove_file(&result_path);
            let _ = tx.send(DeleteMessage::Done);
            return;
        };
        let script_path_str = script_path.to_string_lossy().to_string();

        // Start-Process -Verb RunAs 触发 UAC；-Wait 保证本次删除跑完才继续。
        // 用户在 UAC 弹窗点"否"会抛异常 → catch 分支返回非零，据此区分"被拒绝"
        // 与"删除失败"，不要用同一句报错糊过去。
        let launcher = format!(
            "try {{ $null = Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File','{}') -Verb RunAs -Wait -PassThru; exit 0 }} catch {{ exit 1 }}",
            escape_ps_single_quoted(&script_path_str)
        );
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &launcher])
            .status();

        let uac_failed = match status {
            Ok(s) => !s.success(),
            Err(e) => {
                crate::logger::error(&format!("无法启动 UAC 提权进程: {}", e));
                true
            }
        };

        let Ok(results) = std::fs::read_to_string(&result_path) else {
            crate::logger::error("未能读回 UAC 删除结果");
            let _ = std::fs::remove_file(&script_path);
            let _ = std::fs::remove_file(&result_path);
            let _ = tx.send(DeleteMessage::Done);
            return;
        };

        let mut reported = 0usize;
        for raw_line in results.lines() {
            let line = raw_line.trim_end_matches(['\r', '\n']);
            if line.trim().is_empty() {
                continue;
            }
            let mut parts = line.splitn(3, '\t');
            let status = parts.next().unwrap_or("");
            let path = parts.next().unwrap_or("").to_string();
            if path.is_empty() {
                continue;
            }
            let category = category_of.get(&path).cloned().unwrap_or_default();
            let message = parts.next().unwrap_or("").to_string();
            reported += 1;
            match status {
                "OK" => {
                    // 复用 macOS 侧同一句话：Deleted [<类别>] <路径> (admin privileges)
                    let _ = tx.send(DeleteMessage::Log(
                        App::tf_lang(lang_en, "log_deleted_sudo", &[&category, &path]),
                        path,
                        category,
                        true,
                    ));
                }
                _ => {
                    // log_delete_failed 有两个占位符：路径 + 失败原因
                    let reason = if message.is_empty() {
                        App::t_lang(lang_en, "log_still_exists").to_string()
                    } else {
                        message
                    };
                    let _ = tx.send(DeleteMessage::Log(
                        App::tf_lang(lang_en, "log_delete_failed", &[&path, &reason]),
                        path,
                        category,
                        false,
                    ));
                }
            }
        }

        // UAC 被拒绝时脚本根本没执行，结果文件是空的，这里要明确告诉用户
        if uac_failed && reported == 0 {
            let _ = tx.send(DeleteMessage::Info(
                App::t_lang(lang_en, "sudo_cancelled_log").to_string(),
            ));
        }

        let _ = std::fs::remove_file(&script_path);
        let _ = std::fs::remove_file(&result_path);
        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 其它平台（Linux 等）的提权删除
///
/// 既没有 macOS 的 `sudo -S`，也没有 Windows 的 UAC，所谓"提权重试"无从谈起。
/// 这里退化为再删一次并**如实上报结果**：不做 sudo 幻想，也不把失败粉饰成成功。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn start_sudo_delete(
    failed_items: Vec<(String, String)>,
    password: String,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    let _ = password;
    if failed_items.is_empty() {
        return;
    }

    let (failed_items, rejected) = sanitize_before_delete(failed_items, lang_en);

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        for (path, category, reason) in rejected {
            let line = App::tf_lang(lang_en, "log_sudo_rejected", &[&path, &reason]);
            let _ = tx.send(DeleteMessage::Log(line, path, category, false));
        }

        for (path, category) in failed_items {
            let ok = best_effort_delete(std::path::Path::new(&path));
            let line = if ok {
                App::tf_lang(lang_en, "log_deleted_sudo", &[&category, &path])
            } else {
                App::tf_lang(
                    lang_en,
                    "log_delete_failed",
                    &[&path, App::t_lang(lang_en, "log_still_exists")],
                )
            };
            let _ = tx.send(DeleteMessage::Log(line, path, category, ok));
        }

        let _ = tx.send(DeleteMessage::Done);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_single_quote_is_doubled() {
        assert_eq!(escape_ps_single_quoted("plain"), "plain");
        assert_eq!(escape_ps_single_quoted("Bob's App"), "Bob''s App");
        assert_eq!(escape_ps_single_quoted("a'b'c"), "a''b''c");
    }

    #[test]
    fn ps_escape_neutralises_quote_breakout() {
        // 恶意路径试图提前闭合单引号再拼一条命令出去
        let evil = "C:\\x'; Remove-Item -LiteralPath C:\\Windows -Recurse -Force; echo '";
        let escaped = escape_ps_single_quoted(evil);
        // 转义后不应再出现"单个单引号跟着命令"的结构
        assert!(
            !escaped.contains("' Remove-Item"),
            "注入串没有被转义: {}",
            escaped
        );
        assert_eq!(
            escaped.matches('\'').count(),
            evil.matches('\'').count() * 2
        );
    }

    #[test]
    fn uac_script_uses_literalpath_and_escapes_paths() {
        let items = vec![
            (
                "C:\\Users\\Bob\\AppData\\Local\\Temp\\x".to_string(),
                "临时文件".to_string(),
            ),
            ("C:\\Users\\Bob's\\cache".to_string(), "Cache".to_string()),
        ];
        let script = build_uac_delete_script(&items, "C:\\tmp\\result.log");

        // -LiteralPath 而非 -Path：否则含 [] 的目录会被当通配符
        assert!(script.contains("Remove-Item -LiteralPath $p -Recurse -Force"));
        assert!(script.contains("Test-Path -LiteralPath $p"));
        // 两条路径都被写入，且带撇号的那条做了转义
        assert!(script.contains("'C:\\Users\\Bob\\AppData\\Local\\Temp\\x'"));
        assert!(script.contains("'C:\\Users\\Bob''s\\cache'"));
        // 结果文件落盘
        assert!(script.contains("[System.IO.File]::WriteAllLines('C:\\tmp\\result.log'"));
    }

    #[test]
    fn uac_script_result_line_protocol() {
        let items = vec![("C:\\a".to_string(), "A".to_string())];
        let script = build_uac_delete_script(&items, "C:\\r");
        assert!(script.contains("OK`t$p"), "缺少成功行协议");
        assert!(script.contains("FAIL`t$p"), "缺少失败行协议");
    }

    #[test]
    fn uac_script_empty_items_still_valid() {
        let script = build_uac_delete_script(&[], "C:\\r");
        assert!(script.contains("$paths = @(\n)"));
        assert!(script.contains("WriteAllLines"));
    }
}
