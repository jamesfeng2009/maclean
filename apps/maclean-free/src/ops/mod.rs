//! 后台操作层（egui 壳）
//!
//! 阶段 0 后本模块是薄壳：
//!
//! 1. glob 再导出 [`maclean_core::ops`] 的全部公共项（删除 / sudo / 优化 / 消息契约），
//!    bin 内历史调用路径 `crate::ops::xxx` 保持不变；
//! 2. 保留与 egui [`crate::app::App`] 状态耦合的扫描编排（start_scan 等）。
//!
//! 删除安全闸门只有 core 一份实现，这里不做任何文件删除决策。

pub(crate) use maclean_core::ops::*;

use std::sync::mpsc;

use crate::app::{App, ScanState, Tab};
use maclean_core::scanner;
use maclean_core::scanner::Scanner;

/// 启动后台扫描
pub(crate) fn start_scan(app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
    let tab = app.tab;
    let tab_idx = app.tab_index();
    // 扫描开始时清空删除日志：扫描中不再展示上次的「已删除 …」记录
    app.logs.clear();
    app.scan_states[tab_idx] = ScanState::Scanning;
    app.scan_progress = 0.0;
    // 清空上一次扫描结果，为增量显示做准备
    app.results[tab_idx].clear();

    // 重置用户取消标志
    app.scan_cancel
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let cancel = app.scan_cancel.clone();

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
    let cancel_progress = cancel.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 用户取消：提前退出进度线程
            if cancel_progress.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
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
            Tab::CustomRules => None, // 每次全扫，不缓存
            Tab::DuplicateFiles => Some("dup_files"),
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
            Tab::StartupItems => None, // 同步扫描
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
                Tab::CustomRules => crate::rules::RuleScanner::new().scan(),
                Tab::DuplicateFiles => scanner::dup_files::DuplicateFileScanner::new().scan(),
                // StartupItems 不走统一扫描管道：同步扫描由 UI 直接调用
                #[cfg(target_os = "macos")]
                Tab::StartupItems => scanner::ScanResult {
                    items: Vec::new(),
                    total_size: 0,
                    scan_time_ms: 0,
                },
                #[cfg(not(target_os = "macos"))]
                Tab::StartupItems => scanner::ScanResult {
                    items: Vec::new(),
                    total_size: 0,
                    scan_time_ms: 0,
                },
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
    // 扫描开始时清空删除日志，扫描中不展示上次的「已删除 …」记录
    app.logs.clear();
    app.reset_for_full_scan();

    // 重置用户取消标志（全量扫描可被用户取消）
    app.scan_cancel
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let cancel = app.scan_cancel.clone();

    let (tx, rx) = mpsc::channel();
    *scan_rx = Some(rx);

    // 进度估算线程
    let tx_progress = tx.clone();
    let cancel_progress = cancel.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 用户取消：提前退出进度线程
            if cancel_progress.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
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
            // 用户取消：停止后续扫描
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                crate::logger::warn("[扫描] 用户点击取消，终止全量扫描");
                let _ = tx.send(ScanMessage::Skipped(tab_idx, "用户取消扫描".to_string()));
                break;
            }
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
                Tab::CustomRules => None, // 每次全扫，不缓存
                Tab::DuplicateFiles => Some("dup_files"),
                Tab::StartupItems => None, // 同步扫描
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

            // watchdog：坏目录（如 APFS 异常）会让 readdir 永久阻塞，
            // 超时跳过该 Tab，不让一个 IO 异常拖死整个全量扫描。
            let timeout = scan_timeout_for(tab);
            let scan_fn = move || match tab {
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
                Tab::CustomRules => crate::rules::RuleScanner::new().scan(),
                Tab::DuplicateFiles => scanner::dup_files::DuplicateFileScanner::new().scan(),
                Tab::StartupItems => scanner::ScanResult {
                    items: Vec::new(),
                    total_size: 0,
                    scan_time_ms: 0,
                },
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
            };

            let result = scanner::scan_with_timeout(timeout, scan_fn);

            match result {
                Some(scan_result) => {
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
                None => {
                    crate::logger::warn(&format!(
                        "[扫描] {} 扫描超时被跳过（目录 IO 异常）",
                        cache_name.unwrap_or("tab")
                    ));
                    let _ = tx.send(ScanMessage::Skipped(
                        tab_idx,
                        "该 Tab 扫描超时（目录 IO 异常），已跳过。可重启 Mac 后重扫。".to_string(),
                    ));
                }
            }
        }

        let _ = tx.send(ScanMessage::AllDone);
    });
}

/// 每个 Tab 扫描的 watchdog 超时。
///
/// AppUninstall / DuplicateFiles 遍历面大（全量 Containers / 全盘大文件），
/// 给更宽裕的时限；其余 Tab 60 秒足够。超时只发生在目录 IO 异常（如损坏
/// 的 APFS 目录导致 readdir 永久阻塞）时，正常机器不会走到。
fn scan_timeout_for(tab: Tab) -> std::time::Duration {
    match tab {
        Tab::AppUninstall => std::time::Duration::from_secs(240),
        Tab::DuplicateFiles => std::time::Duration::from_secs(120),
        _ => std::time::Duration::from_secs(60),
    }
}
