//! 磁盘分析器 - 目录钻取式磁盘浏览器
//!
//! 从用户主目录开始，列出所有子项并按大小排序。
//! 支持进入子目录继续浏览，实现 Mole/DaisyDisk 式的目录钻取体验。
//!
//! 与旧版大文件扫描器的区别：
//! - 旧版：只列出主目录下 >100MB 的顶层大文件
//! - 新版：列出当前目录下所有子项（不论大小），支持进入子目录继续浏览
//!
//! 安全机制：
//! - 跳过 TCC 保护目录（Library/Pictures/Music/Movies/Documents/Desktop/Downloads）
//! - 跳过 Media Library bundles（.photoslibrary 等）
//! - 跳过 .app/.bundle/.pkg/.dmg 等特殊 bundle
//! - 单目录遍历超时 10 秒

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use walkdir::WalkDir;

use super::{home_dir, Recommend, ScanItem, ScanResult, Scanner};

/// 单目录遍历超时 10 秒
const DIR_TIMEOUT: Duration = Duration::from_secs(10);
/// 最小展示大小：1MB（小于此值的文件不展示，避免列表过长）
const MIN_DISPLAY_SIZE: u64 = 1024 * 1024;

/// 已知会导致崩溃、极慢或权限问题的目录/文件
fn is_problematic_path(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".photoslibrary")
        || lower.ends_with(".musiclibrary")
        || lower.ends_with(".tvlibrary")
        || lower.ends_with(".podcastlibrary")
        || lower.ends_with(".aplibrary")
        || lower.ends_with(".fcpbundle")
        || lower.ends_with(".logicx")
        || lower.ends_with(".band")
        || lower.ends_with(".app")
        || lower.ends_with(".xcworkspace")
        || lower.ends_with(".xcodeproj")
        || lower.ends_with(".bundle")
        || lower.ends_with(".pkg")
        || lower.ends_with(".dmg")
        || lower.ends_with(".iso")
}

/// 主目录下应跳过的目录名（TCC 保护，访问会触发权限弹窗）
fn should_skip_home_dir(name: &str) -> bool {
    matches!(
        name,
        "Library"
            | "Pictures"
            | "Music"
            | "Movies"
            | "Public"
            | "Applications"
            | "Sites"
            | "Documents"
            | "Desktop"
            | "Downloads"
    )
}

/// 磁盘分析器扫描器
#[derive(Debug, Default)]
pub struct LargeFileScanner;

impl LargeFileScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for LargeFileScanner {
    /// 默认扫描用户主目录
    fn scan(&self) -> ScanResult {
        scan_directory(&home_dir())
    }
}

/// 扫描指定目录下的所有子项（文件 + 目录），按大小降序排列
///
/// 这是磁盘分析器的核心函数。对当前目录的每个子项：
/// - 文件：直接获取大小
/// - 目录：递归计算大小（带超时保护）
///
/// 结果按大小降序排列，便于用户快速定位占用空间的目录/文件。
pub fn scan_directory(path: &Path) -> ScanResult {
    catch_unwind(AssertUnwindSafe(|| scan_directory_impl(path))).unwrap_or_else(|_| ScanResult {
        items: Vec::new(),
        total_size: 0,
        scan_time_ms: 0,
    })
}

fn scan_directory_impl(path: &Path) -> ScanResult {
    let start = Instant::now();
    let mut items = Vec::new();
    crate::log_scan_step(&format!("磁盘分析: 扫描目录 {}", path.display()));

    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => {
            return ScanResult {
                items: Vec::new(),
                total_size: 0,
                scan_time_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    // 收集所有子项路径
    let paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            // 跳过隐藏文件/目录（以 . 开头）
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    return false;
                }
                // 如果是主目录的直接子项，跳过 TCC 保护目录
                if path == home_dir().as_path() && should_skip_home_dir(name) {
                    return false;
                }
                // 跳过问题 bundle
                if is_problematic_path(name) {
                    return false;
                }
            }
            true
        })
        .collect();

    crate::log_scan_step(&format!("磁盘分析: {} 个子项待计算", paths.len()));

    // 并行计算每个子项的大小
    let sized: Vec<(PathBuf, u64, bool)> = paths
        .par_iter()
        .filter_map(|path| {
            let is_dir = path.is_dir();

            let size = catch_unwind(AssertUnwindSafe(|| {
                if is_dir {
                    dir_size_with_timeout(path)
                } else {
                    path.symlink_metadata().map(|m| m.len()).unwrap_or(0)
                }
            }))
            .unwrap_or(0);

            Some((path.clone(), size, is_dir))
        })
        .filter(|(_, size, _)| *size >= MIN_DISPLAY_SIZE)
        .collect();

    let total_size: u64 = sized.iter().map(|(_, s, _)| *s).sum();

    for (path, size, is_dir) in sized {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let category = if is_dir { "目录" } else { "文件" };

        items.push(ScanItem {
            path: path.to_string_lossy().to_string(),
            size_bytes: size,
            category: category.to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Advanced,
            description: if is_dir {
                format!(
                    "📁 {} — {}（可进入查看详情）",
                    name,
                    super::format_size(size)
                )
            } else {
                format!("📄 {} — {}", name, super::format_size(size))
            },
        });
    }

    // 按大小降序排列
    items.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    crate::log_scan_step(&format!(
        "磁盘分析: 完成, {} 项, 总计 {}",
        items.len(),
        super::format_size(total_size)
    ));

    let scan_time_ms = start.elapsed().as_millis() as u64;

    ScanResult {
        items,
        total_size,
        scan_time_ms,
    }
}

/// 带超时的目录大小计算
fn dir_size_with_timeout(path: &Path) -> u64 {
    let start = Instant::now();
    let mut total: u64 = 0;

    for entry in WalkDir::new(path)
        .follow_links(false)
        .max_depth(50)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() > 0 {
                if e.file_type().is_dir() {
                    if std::fs::metadata(e.path()).is_err() {
                        return false;
                    }
                }
                if let Some(name) = e.file_name().to_str() {
                    if is_problematic_path(name) {
                        return false;
                    }
                }
            }
            true
        })
    {
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
