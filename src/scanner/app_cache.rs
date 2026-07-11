//! App 容器缓存扫描器
//!
//! 扫描 ~/Library/Containers、~/Library/Group Containers、
//! ~/Library/Application Support、~/Library/Caches、~/Library/Logs
//! 下的应用缓存数据。

use std::path::PathBuf;
use std::time::Instant;

use rayon::prelude::*;

use super::{dir_size, home_dir, ScanItem, ScanResult, Scanner};

/// 50MB 阈值
const CONTAINER_MIN: u64 = 50 * 1024 * 1024;
/// 100MB 阈值
const CACHE_MIN: u64 = 100 * 1024 * 1024;
/// 500MB 阈值（Application Support）
const APP_SUPPORT_MIN: u64 = 500 * 1024 * 1024;

/// App 缓存扫描器
#[derive(Debug, Default)]
pub struct AppCacheScanner;

impl AppCacheScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for AppCacheScanner {
    fn scan(&self) -> ScanResult {
        let start = Instant::now();
        let mut items = Vec::new();

        items.extend(scan_containers());
        items.extend(scan_group_containers());
        items.extend(scan_app_support());
        items.extend(scan_system_caches());
        items.extend(scan_logs());

        // 按大小降序排列
        items.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        let scan_time_ms = start.elapsed().as_millis() as u64;

        ScanResult {
            items,
            total_size,
            scan_time_ms,
        }
    }
}

// =========================================================================
//  ~/Library/Containers
// =========================================================================

/// 扫描应用容器缓存
fn scan_containers() -> Vec<ScanItem> {
    let home = home_dir();
    let containers_dir = home.join("Library/Containers");
    let mut items = Vec::new();

    let entries = match std::fs::read_dir(&containers_dir) {
        Ok(e) => e,
        Err(_) => return items,
    };

    // 收集所有容器路径
    let container_paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    // 并行计算每个容器的缓存大小
    let sized: Vec<(PathBuf, PathBuf, u64, String, bool)> = container_paths
        .par_iter()
        .filter_map(|container_path| {
            let caches_dir = container_path.join("Data/Library/Caches");
            let total_size = if caches_dir.is_dir() {
                dir_size(&caches_dir)
            } else {
                0
            };

            // 微信特殊处理：检查 xwechat_files
            let wechat_data = container_path.join("Data/Documents/xwechat_files");
            let wechat_size = if wechat_data.is_dir() {
                dir_size(&wechat_data)
            } else {
                0
            };

            let container_name = container_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            if total_size >= CONTAINER_MIN {
                let app_name = container_display_name(container_name);
                Some((container_path.clone(), caches_dir, total_size, app_name, false))
            } else if wechat_size >= CONTAINER_MIN {
                let app_name = "微信数据".to_string();
                Some((container_path.clone(), wechat_data, wechat_size, app_name, true))
            } else {
                None
            }
        })
        .collect();

    for (_, cache_path, size, app_name, is_wechat_data) in sized {
        items.push(ScanItem {
            path: cache_path.to_string_lossy().to_string(),
            size_bytes: size,
            category: app_name,
            selected: false,
            deletable: !is_wechat_data, // 微信数据标记为不可直接删除
        });
    }

    items
}

/// 将容器目录名映射为友好的应用名
fn container_display_name(container_id: &str) -> String {
    match container_id {
        "com.tencent.xinWeChat" => "微信缓存".to_string(),
        "com.bytedance.macos.feishu" => "飞书".to_string(),
        "com.tencent.QQMusicMac" => "QQ音乐".to_string(),
        "com.youku.mac" => "优酷".to_string(),
        "com.iqiyi.player" => "爱奇艺".to_string(),
        "com.tencent.qq" => "QQ".to_string(),
        "com.tencent.qqexdoc" => "QQ文档".to_string(),
        "com.tencent.meeting" => "腾讯会议".to_string(),
        "com.kingsoft.wpsoffice.mac" => "WPS".to_string(),
        "com.microsoft.Excel" => "Excel".to_string(),
        "com.microsoft.Word" => "Word".to_string(),
        "com.microsoft.Powerpoint" => "PowerPoint".to_string(),
        "com.apple.mail" => "邮件".to_string(),
        "com.apple.Safari" => "Safari".to_string(),
        "com.googlecode.iterm2" => "iTerm".to_string(),
        "com.tinyspeck.slackmacgap" => "Slack".to_string(),
        "com.spotify.client" => "Spotify".to_string(),
        _ => {
            // 截取最后一部分作为名称
            container_id
                .split('.')
                .last()
                .unwrap_or(container_id)
                .to_string()
        }
    }
}

// =========================================================================
//  ~/Library/Group Containers
// =========================================================================

/// 扫描 Group Containers
fn scan_group_containers() -> Vec<ScanItem> {
    let home = home_dir();
    let group_dir = home.join("Library/Group Containers");
    let mut items = Vec::new();

    let entries = match std::fs::read_dir(&group_dir) {
        Ok(e) => e,
        Err(_) => return items,
    };

    let paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    let sized: Vec<(PathBuf, u64, String)> = paths
        .par_iter()
        .filter_map(|path| {
            let caches = path.join("Library/Caches");
            let size = if caches.is_dir() {
                dir_size(&caches)
            } else {
                0
            };

            if size >= CONTAINER_MIN {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                let display = group_container_display_name(name);
                Some((caches, size, display))
            } else {
                None
            }
        })
        .collect();

    for (cache_path, size, display) in sized {
        items.push(ScanItem {
            path: cache_path.to_string_lossy().to_string(),
            size_bytes: size,
            category: display,
            selected: false,
            deletable: true,
        });
    }

    items
}

/// Group Containers 名称映射
fn group_container_display_name(id: &str) -> String {
    if id.contains("Telegram") {
        return "Telegram".to_string();
    }
    if id.contains("orbstack") {
        return "OrbStack".to_string();
    }
    if id.contains("whatsapp") {
        return "WhatsApp".to_string();
    }
    id.to_string()
}

// =========================================================================
//  ~/Library/Application Support
// =========================================================================

/// 扫描 Application Support 下的应用数据
fn scan_app_support() -> Vec<ScanItem> {
    let home = home_dir();
    let app_support = home.join("Library/Application Support");
    let mut items = Vec::new();

    let entries = match std::fs::read_dir(&app_support) {
        Ok(e) => e,
        Err(_) => return items,
    };

    let paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    let sized: Vec<(PathBuf, u64, String)> = paths
        .par_iter()
        .filter_map(|path| {
            let size = dir_size(path);
            if size >= APP_SUPPORT_MIN {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("未知")
                    .to_string();
                Some((path.clone(), size, name))
            } else {
                None
            }
        })
        .collect();

    for (path, size, name) in sized {
        items.push(ScanItem {
            path: path.to_string_lossy().to_string(),
            size_bytes: size,
            category: name,
            selected: false,
            deletable: true,
        });
    }

    items
}

// =========================================================================
//  ~/Library/Caches
// =========================================================================

/// 扫描系统缓存目录
fn scan_system_caches() -> Vec<ScanItem> {
    let home = home_dir();
    let caches_dir = home.join("Library/Caches");
    let mut items = Vec::new();

    let entries = match std::fs::read_dir(&caches_dir) {
        Ok(e) => e,
        Err(_) => return items,
    };

    let paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    let sized: Vec<(PathBuf, u64)> = paths
        .par_iter()
        .filter_map(|path| {
            let size = dir_size(path);
            if size >= CACHE_MIN {
                Some((path.clone(), size))
            } else {
                None
            }
        })
        .collect();

    for (path, size) in sized {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("缓存")
            .to_string();
        items.push(ScanItem {
            path: path.to_string_lossy().to_string(),
            size_bytes: size,
            category: format!("系统缓存-{}", name),
            selected: false,
            deletable: true,
        });
    }

    items
}

// =========================================================================
//  ~/Library/Logs
// =========================================================================

/// 扫描系统日志
fn scan_logs() -> Vec<ScanItem> {
    let home = home_dir();
    let logs_dir = home.join("Library/Logs");
    let mut items = Vec::new();

    if logs_dir.is_dir() {
        let size = dir_size(&logs_dir);
        if size >= CACHE_MIN {
            items.push(ScanItem {
                path: logs_dir.to_string_lossy().to_string(),
                size_bytes: size,
                category: "系统日志".to_string(),
                selected: false,
                deletable: true,
            });
        }
    }

    items
}
