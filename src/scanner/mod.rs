//! 扫描器模块 - 负责扫描各类可清理的项目
//!
//! 本模块定义了扫描器的核心数据结构和 trait，
//! 并导出具体的子模块：开发者缓存、大文件、应用缓存、APFS 快照。

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// 导出缓存模块
pub mod cache;

// 导出子模块
#[cfg(target_os = "macos")]
pub mod apfs;
// app_cache/app_data/uninstall 扫描 macOS 专属路径（~/Library/Containers 等），
// Windows 上不支持，用 cfg 包装。optimize 内部已按平台返回不同任务清单，
// 无需限制平台（Windows 有自己的 Win11Debloat 风格任务）。
#[cfg(target_os = "macos")]
pub mod app_cache;
#[cfg(target_os = "macos")]
pub mod app_data;
pub mod cache_registry;
pub mod dev_cache;
pub mod large_files;
pub mod optimize;
// 残留名称匹配：刻意不限定平台，见模块顶部注释。放在 scanner 根而非
// windows_apps 内，是为了让它（连同规定的删除保护）在开发机上可被编译和测试。
mod residual_match;
#[cfg(target_os = "macos")]
pub mod uninstall;
// Windows 专属模块（采集层：读注册表 / 枚举 UWP / 执行卸载）
#[cfg(target_os = "windows")]
pub mod windows_apps;
// Windows 应用保护**判定**：刻意不加 cfg。它原本在 windows_apps.rs 里，
// 结果在开发机上整段不编译、一行测试都跑不到 —— 而这是卸载前的最后一道
// 保护。判定只依赖 String/bool 和跨平台文件系统调用，必须本机可测。
// 详见 win_protection.rs 模块顶部注释。
pub mod win_protection;

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
                if e.file_type().is_dir() && std::fs::metadata(e.path()).is_err() {
                    return false;
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
        // 锁的是 `theme::recommend_label` —— UI 徽标真正调用的那个。
        // 此前这条用例锁的是 `Recommend::label()`（已删），后者零生产调用：
        // 锁在一个没人用的副本上，UI 文案改了这条用例照样绿。
        // 改动需同步 maclean-ui-design-v2.html。
        assert_eq!(
            crate::theme::recommend_label(&Recommend::Safe, false),
            "安全"
        );
        assert_eq!(
            crate::theme::recommend_label(&Recommend::CacheOnly, false),
            "仅缓存"
        );
        assert_eq!(
            crate::theme::recommend_label(&Recommend::Caution, false),
            "谨慎"
        );
        assert_eq!(
            crate::theme::recommend_label(&Recommend::Advanced, false),
            "高级"
        );
        // 英文侧
        assert_eq!(
            crate::theme::recommend_label(&Recommend::Safe, true),
            "Safe"
        );
        assert_eq!(
            crate::theme::recommend_label(&Recommend::Advanced, true),
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
            crate::theme::recommend_label(&Recommend::Safe, false),
            "安全"
        );
        assert_eq!(
            crate::i18n::translate_recommend(&Recommend::Safe, false),
            "推荐"
        );
        assert_eq!(
            crate::theme::recommend_label(&Recommend::Advanced, false),
            "高级"
        );
        assert_eq!(
            crate::i18n::translate_recommend(&Recommend::Advanced, false),
            "需确认"
        );
    }
}
