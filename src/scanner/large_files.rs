//! 大文件/目录扫描器
//!
//! 扫描用户主目录下的顶层大目录和大文件，
//! 以及 Downloads 和 Desktop 下的散落大文件。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use walkdir::WalkDir;

use super::{home_dir, Recommend, ScanItem, ScanResult, Scanner};

/// 100MB 阈值
const MIN_SIZE: u64 = 100 * 1024 * 1024;
/// 500MB 阈值（用于 Downloads）
const DOWNLOAD_MIN_SIZE: u64 = 500 * 1024 * 1024;
/// 单目录遍历超时 10 秒
const DIR_TIMEOUT: Duration = Duration::from_secs(10);

/// 已知会导致崩溃或极慢的目录/文件（Photos Library、Music Library 等 bundle）
fn is_problematic_path(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".photoslibrary")
        || lower.ends_with(".musiclibrary")
        || lower.ends_with(".tvlibrary")
        || lower.ends_with(".podcastlibrary")
        || lower == "photos library.photoslibrary"
        || lower.contains(".photoslibrary")
        || lower == "photo library"
}

/// 大文件扫描器
#[derive(Debug, Default)]
pub struct LargeFileScanner;

impl LargeFileScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for LargeFileScanner {
    fn scan(&self) -> ScanResult {
        scan_with_min_size(MIN_SIZE)
    }
}

/// 扫描大文件，可自定义最小大小阈值
pub fn scan_with_min_size(min_size: u64) -> ScanResult {
    let start = Instant::now();
    let home = home_dir();
    let mut items = Vec::new();

    // 1. 扫描主目录顶层目录和文件
    if let Ok(entries) = std::fs::read_dir(&home) {
        let paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                // 跳过隐藏目录和 Library
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with('.') {
                        return false;
                    }
                }
                true
            })
            .collect();

        // 并行计算每个路径的大小（用 catch_unwind 防止崩溃）
        let sized: Vec<(PathBuf, u64, bool)> = paths
            .par_iter()
            .filter_map(|path| {
                let is_dir = path.is_dir();

                // 跳过 Photos Library 等会导致崩溃的 bundle
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if is_problematic_path(name) {
                        return None;
                    }
                }

                // 用 catch_unwind 防止遍历过程中 panic
                let size = catch_unwind(AssertUnwindSafe(|| {
                    if is_dir {
                        dir_size_with_timeout(path)
                    } else {
                        path.symlink_metadata()
                            .map(|m| m.len())
                            .unwrap_or(0)
                    }
                }))
                .unwrap_or(0);

                Some((path.clone(), size, is_dir))
            })
            .filter(|(_, size, _)| *size >= min_size)
            .collect();

        for (path, size, is_dir) in sized {
            let category = if is_dir { "大目录" } else { "大文件" };
            items.push(ScanItem {
                path: path.to_string_lossy().to_string(),
                size_bytes: size,
                category: category.to_string(),
                selected: false,
                deletable: true,
                recommend: Recommend::Advanced,
                description: "主目录下的大文件/目录，请确认无需保留".to_string(),
            });
        }
    }

    // 2. 扫描 ~/Downloads 顶层大文件（不递归子目录）
    let downloads = home.join("Downloads");
    if let Ok(entries) = std::fs::read_dir(&downloads) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                if let Ok(meta) = path.symlink_metadata() {
                    if meta.len() >= DOWNLOAD_MIN_SIZE {
                        items.push(ScanItem {
                            path: path.to_string_lossy().to_string(),
                            size_bytes: meta.len(),
                            category: "下载文件".to_string(),
                            selected: false,
                            deletable: true,
                            recommend: Recommend::Advanced,
                            description: "Downloads 中的大文件，请确认无需保留".to_string(),
                        });
                    }
                }
            }
        }
    }

    // 3. 扫描 ~/Desktop 顶层大文件
    let desktop = home.join("Desktop");
    if let Ok(entries) = std::fs::read_dir(&desktop) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                if let Ok(meta) = path.symlink_metadata() {
                    if meta.len() >= MIN_SIZE {
                        items.push(ScanItem {
                            path: path.to_string_lossy().to_string(),
                            size_bytes: meta.len(),
                            category: "桌面文件".to_string(),
                            selected: false,
                            deletable: true,
                            recommend: Recommend::Advanced,
                            description: "桌面上的大文件，请确认无需保留".to_string(),
                        });
                    }
                }
            }
        }
    }

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

/// 带超时的目录大小计算
///
/// 使用 walkdir 遍历，如果超过 10 秒则返回 0。
/// 跳过权限不足的目录、符号链接和已知的问题 bundle。
fn dir_size_with_timeout(path: &std::path::Path) -> u64 {
    let start = Instant::now();
    let mut total: u64 = 0;

    for entry in WalkDir::new(path)
        .follow_links(false)
        .max_depth(50) // 限制深度防止无限递归
        .into_iter()
        .filter_entry(|e| {
            if e.depth() > 0 {
                // 跳过不可读目录
                if e.file_type().is_dir() {
                    if std::fs::metadata(e.path()).is_err() {
                        return false;
                    }
                }
                // 跳过 Photos Library 等问题 bundle
                if let Some(name) = e.file_name().to_str() {
                    if is_problematic_path(name) {
                        return false;
                    }
                }
            }
            true
        })
    {
        // 检查超时
        if start.elapsed() > DIR_TIMEOUT {
            return total;
        }

        match entry {
            Ok(entry) => {
                if entry.file_type().is_file() {
                    if let Ok(metadata) = entry.metadata() {
                        total += metadata.len();
                    }
                }
            }
            Err(_) => continue,
        }
    }

    total
}
