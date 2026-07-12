//! 扫描器模块 - 负责扫描各类可清理的项目
//!
//! 本模块定义了扫描器的核心数据结构和 trait，
//! 并导出具体的子模块：开发者缓存、大文件、应用缓存、APFS 快照。

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// 导出缓存模块
pub mod cache;

// 导出子模块
pub mod apfs;
pub mod app_cache;
pub mod app_data;
pub mod dev_cache;
pub mod large_files;
pub mod optimize;
pub mod uninstall;

/// 推荐等级 - 帮助用户判断是否应该清理
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Recommend {
    /// 推荐清理 - 安全可删，重新构建/使用时会自动恢复
    Safe,
    /// 谨慎清理 - 删除后可能需要重新下载或配置
    Caution,
    /// 高级用户 - 需要了解风险后自行判断
    Advanced,
}

impl Recommend {
    /// 返回等级的中文描述
    pub fn label(self) -> &'static str {
        match self {
            Recommend::Safe => "推荐",
            Recommend::Caution => "谨慎",
            Recommend::Advanced => "高级",
        }
    }

    /// 返回详细说明
    pub fn description(self) -> &'static str {
        match self {
            Recommend::Safe => "安全可删，自动恢复",
            Recommend::Caution => "删除后需重新下载",
            Recommend::Advanced => "请确认后再删除",
        }
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

/// 检测路径是否可被当前用户删除
/// 返回 (deletable, reason)
/// 不可删除的情况:
/// 1. 路径位于系统 SIP 保护目录下，即使 root 也无法修改
/// 可删除但需要 sudo 的情况:
/// 2. 文件属主为 root (需要管理员权限)
pub fn check_deletable(path: &str) -> (bool, String) {
    let p = std::path::Path::new(path);

    // APFS 快照由专门的删除逻辑处理，总是可删除
    if path.starts_with("snapshot:") {
        return (true, String::new());
    }

    // CoreSimulator 运行时镜像 — 通过 xcrun simctl runtime delete 安全删除
    if path.starts_with("/Library/Developer/CoreSimulator/Volumes") {
        return (true, String::new());
    }

    // CoreSimulator/Caches — root 属主，sudo rm -rf 可删
    if path.starts_with("/Library/Developer/CoreSimulator/Caches") {
        return (true, String::new());
    }

    // 检查属主和扩展属性
    if let Ok(meta) = p.symlink_metadata() {
        use std::os::unix::fs::MetadataExt;

        let uid = meta.uid();

        // 属主为 root → 需要 sudo，但允许尝试删除
        // com.apple.provenance 只是下载来源标记，不代表 SIP 保护
        if uid == 0 {
            return (true, String::new());
        }
    }

    (true, String::new())
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

/// 递归计算目录大小（字节）
///
/// 使用 walkdir 遍历目录下所有文件，累加文件大小。
/// 遇到无法访问的文件/目录时直接跳过，不报错。
pub fn dir_size(path: &Path) -> u64 {
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
                // 跳过 Photos Library 等问题 bundle
                if let Some(name) = e.file_name().to_str() {
                    let lower = name.to_lowercase();
                    if lower.ends_with(".photoslibrary")
                        || lower.ends_with(".musiclibrary")
                        || lower.ends_with(".tvlibrary")
                    {
                        return false;
                    }
                }
            }
            true
        })
    {
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

/// 获取当前用户的 home 目录
///
/// 使用 dirs crate 获取，若获取失败则回退到 "/"。
pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}
