//! 系统优化扫描器
//!
//! 定义系统优化任务列表（DNS 缓存刷新、QuickLook 重建、LaunchServices 重建等）。
//! 这些任务不会在扫描时执行，仅作为可用操作列展示给用户。
//! 实际执行由 UI 层触发。
//!
//! 参考 Mole (https://github.com/tw93/Mole) 的 optimize/tasks.sh 实现。

use std::time::Instant;

use super::{Recommend, ScanItem, ScanResult, Scanner};

/// 系统优化扫描器
#[derive(Debug, Default)]
pub struct OptimizeScanner;

impl OptimizeScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for OptimizeScanner {
    fn scan(&self) -> ScanResult {
        let start = Instant::now();

        // 6 项安全的系统优化任务
        // path 使用英文 key，UI 层通过 app.t("optimize_xxx") 翻译显示
        let items = vec![
            make_task(
                "dns_cache_flush",
                "刷新 DNS 缓存，修复网络解析问题",
            ),
            make_task(
                "quicklook_rebuild",
                "清理 QuickLook 缩略图缓存，修复预览问题",
            ),
            make_task(
                "launchservices_rebuild",
                "重建 LaunchServices 数据库，修复\"打开方式\"菜单问题",
            ),
            make_task(
                "saved_state_cleanup",
                "清理超过 30 天的应用保存状态",
            ),
            make_task(
                "gatekeeper_cleanup",
                "清理 Gatekeeper 下载追踪记录",
            ),
            make_task(
                "memory_pressure_release",
                "释放非活跃内存，提升系统响应速度",
            ),
        ];

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
//  辅助函数
// =========================================================================

/// 创建优化任务 ScanItem
///
/// 优化任务不是可删除的文件项，而是可执行的操作项:
/// - `path`: 任务名称（UI 层据此匹配并执行对应命令）
/// - `deletable`: false（操作项，非删除项）
/// - `size_bytes`: 0（操作项无大小）
/// - `recommend`: Safe（均为安全操作）
fn make_task(name: &str, description: &str) -> ScanItem {
    ScanItem {
        path: name.to_string(),
        size_bytes: 0,
        category: "系统优化".to_string(),
        selected: false,
        deletable: false,
        undeletable_reason: String::new(),
        batch_paths: Vec::new(),
        recommend: Recommend::Safe,
        description: description.to_string(),
    }
}
