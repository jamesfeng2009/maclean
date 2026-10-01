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

        // 平台专属优化任务
        let items = if cfg!(target_os = "windows") {
            windows_optimize_tasks()
        } else {
            macos_optimize_tasks()
        };

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        let scan_time_ms = start.elapsed().as_millis() as u64;

        ScanResult {
            items,
            total_size,
            scan_time_ms,
        }
    }
}

/// macOS 优化任务（14 项）
///
/// 前 8 项为常规优化；后 6 项为系统维护，对标 MangoDisk system_maintenance：
/// 低风险（icon-cache/finder-service/audio-service/legacy-overrides）直接可用，
/// 高风险（user-permissions/startup-disk）需用户确认后执行。
/// 当前平台的系统维护任务清单（Tauri/CLI 统一入口）。
pub fn current_platform_tasks() -> Vec<ScanItem> {
    #[cfg(target_os = "macos")]
    {
        macos_optimize_tasks()
    }
    #[cfg(target_os = "windows")]
    {
        windows_optimize_tasks()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

pub fn macos_optimize_tasks() -> Vec<ScanItem> {
    vec![
        make_task(
            "dns_cache_flush",
            "刷新 DNS 缓存，修复网络解析问题",
            Recommend::Safe,
        ),
        make_task(
            "quicklook_rebuild",
            "清理 QuickLook 缩略图缓存，修复预览问题",
            Recommend::Safe,
        ),
        make_task(
            "launchservices_rebuild",
            "重建 LaunchServices 数据库，修复\"打开方式\"菜单问题",
            Recommend::Safe,
        ),
        make_task(
            "saved_state_cleanup",
            "清理超过 30 天的应用保存状态",
            Recommend::Safe,
        ),
        make_task(
            "gatekeeper_cleanup",
            "清理 Gatekeeper 下载追踪记录",
            Recommend::Safe,
        ),
        make_task(
            "memory_pressure_release",
            "释放非活跃内存，提升系统响应速度",
            Recommend::Safe,
        ),
        make_task(
            "spotlight_reindex",
            "重建 Spotlight 搜索索引，修复搜索不到文件的问题",
            Recommend::Caution,
        ),
        make_task(
            "login_items_audit",
            &scan_login_items_description(),
            Recommend::Safe,
        ),
        // ---- 系统维护（P1，对标 MangoDisk system_maintenance）----
        make_task(
            "icon_cache_rebuild",
            "重建图标缓存，修复桌面/访达图标显示异常",
            Recommend::Safe,
        ),
        make_task(
            "finder_service_restart",
            "重启 Finder 服务，刷新文件关联与侧边栏",
            Recommend::Safe,
        ),
        make_task(
            "audio_service_restart",
            "重启音频服务，修复无声/卡顿（可能需要管理员权限）",
            Recommend::Safe,
        ),
        make_task(
            "legacy_overrides_clean",
            "清理旧版 LaunchServices 打开方式覆盖，恢复默认关联",
            Recommend::Safe,
        ),
        make_task(
            "user_permissions_repair",
            "修复用户 Library 目录权限，解决应用无法写入/打开异常",
            Recommend::Advanced,
        ),
        make_task(
            "startup_disk_verify",
            "校验启动盘卷健康（只读检查，不修改数据）",
            Recommend::Advanced,
        ),
    ]
}

/// Windows 优化任务（10 项）
///
/// 参考 Win11Debloat (https://github.com/Raphire/Win11Debloat) 设计，
/// 聚焦清理、隐私、性能三类安全优化，不涉及系统关键服务。
pub fn windows_optimize_tasks() -> Vec<ScanItem> {
    vec![
        make_task(
            "win_dns_flush",
            "刷新 DNS 缓存，修复网络解析问题",
            Recommend::Safe,
        ),
        make_task(
            "win_temp_cleanup",
            "清理 Windows 临时文件、缩略图缓存、交付优化缓存",
            Recommend::Safe,
        ),
        make_task(
            "win_disable_telemetry",
            "关闭 Windows 遥测与诊断数据收集，减少隐私泄露",
            Recommend::Caution,
        ),
        make_task(
            "win_disable_copilot",
            "禁用 Windows Copilot 与 AI 功能，释放内存与后台资源",
            Recommend::Caution,
        ),
        make_task(
            "win_disable_suggestions",
            "关闭开始菜单、设置、锁屏的推荐与广告内容",
            Recommend::Caution,
        ),
        make_task(
            "win_startup_audit",
            "审计开机启动项与计划任务，加快开机速度",
            Recommend::Safe,
        ),
        make_task(
            "win_disable_fast_startup",
            "关闭快速启动，减少休眠文件占用并避免驱动异常",
            Recommend::Caution,
        ),
        make_task(
            "win_restore_point",
            "创建系统还原点，优化前自动备份当前状态",
            Recommend::Safe,
        ),
        make_task(
            "win_restart_explorer",
            "重启资源管理器，刷新任务栏/开始菜单/桌面",
            Recommend::Safe,
        ),
        make_task(
            "win_trim_drives",
            "对 SSD 执行 TRIM 优化，对 HDD 执行碎片整理",
            Recommend::Caution,
        ),
    ]
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
fn make_task(name: &str, description: &str, recommend: Recommend) -> ScanItem {
    ScanItem {
        path: name.to_string(),
        size_bytes: 0,
        category: "系统优化".to_string(),
        selected: false,
        deletable: false,
        undeletable_reason: String::new(),
        batch_paths: Vec::new(),
        recommend,
        description: description.to_string(),
    }
}

/// 扫描登录项，生成包含项目数的描述文本
///
/// 统计来源：
/// - `~/Library/Preferences/com.apple.loginitems.plist`（用户登录项）
/// - `~/Library/LaunchAgents/`（用户 LaunchAgent）
/// - `/Library/LaunchAgents/`（系统 LaunchAgent）
/// - `/Library/LaunchDaemons/`（系统 LaunchDaemon）
fn scan_login_items_description() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut user_login = 0usize;
    // 延迟初始化：这三个变量在下面无条件赋值，给初值反而会被编译器判定为"写入未读"

    // 用户登录项（Login Items plist）
    let loginitems_plist = format!("{}/Library/Preferences/com.apple.loginitems.plist", home);
    if let Ok(output) = std::process::Command::new("defaults")
        .arg("read")
        .arg(&loginitems_plist)
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            // 统计 "Path" 键出现次数 = 登录项数量
            user_login = text.matches("Path =").count();
        }
    }

    // 用户 LaunchAgent
    let user_agents_dir = format!("{}/Library/LaunchAgents", home);
    let user_agents: usize = count_plist_files(&user_agents_dir);

    // 系统 LaunchAgent
    let system_agents: usize = count_plist_files("/Library/LaunchAgents");

    // 系统 LaunchDaemon
    let system_daemons: usize = count_plist_files("/Library/LaunchDaemons");

    let total = user_login + user_agents + system_agents + system_daemons;
    format!(
        "审计登录项与启动服务（当前 {} 项：登录项 {} / 用户服务 {} / 系统服务 {} / 系统守护进程 {}），点击打开系统设置管理",
        total, user_login, user_agents, system_agents, system_daemons
    )
}

/// 统计目录中的 .plist 文件数量
fn count_plist_files(dir: &str) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "plist")
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Windows 的优化代码（`execute_windows_optimize_task`）是 cfg(windows)，
    // 开发机上编译不到也就跑不到。所以这一组测试改用两个手段把覆盖面补回来：
    // 1. 清单构造函数 `windows_optimize_tasks()` 本身没有 cfg，可以直接验证
    // 2. 对无法编译的部分做源码级检查，至少保证"接口是对齐的"

    #[test]
    fn windows_task_list_is_populated() {
        let tasks = windows_optimize_tasks();
        assert!(!tasks.is_empty(), "Windows 优化清单为空");
        for t in &tasks {
            assert!(
                t.path.starts_with("win_"),
                "Windows 清单里混入了非 win_ 任务: {}",
                t.path
            );
            assert!(!t.description.is_empty(), "任务 {} 缺少说明文案", t.path);
        }
    }

    #[test]
    fn platform_task_lists_do_not_leak_into_each_other() {
        let win_items = windows_optimize_tasks();
        let mac_items = macos_optimize_tasks();
        let win: Vec<&str> = win_items.iter().map(|t| t.path.as_str()).collect();
        let mac: Vec<&str> = mac_items.iter().map(|t| t.path.as_str()).collect();
        for n in &mac {
            assert!(
                !n.starts_with("win_"),
                "macOS 清单里混入了 Windows 任务: {}",
                n
            );
        }
        // 两边都不应为空：任一边空掉意味着某个平台的优化 Tab 是块白板
        assert!(!win.is_empty());
        assert!(!mac.is_empty());
    }

    #[test]
    fn every_windows_task_has_an_executor_branch() {
        // UI 会把这份清单渲染成一排按钮；若执行器里没有对应分支，
        // 点下去会落到 `opt_unknown`，用户会以为点了但什么都没发生。
        // 这里做源码级检查就是为了拦住这类"清单和实现不同步"。
        let ops_src = include_str!("../ops/mod.rs");
        for item in windows_optimize_tasks() {
            assert!(
                ops_src.contains(&format!("\"{}\" =>", item.path)),
                "优化任务 {} 在 execute_windows_optimize_task 里没有对应分支",
                item.path
            );
        }
    }

    #[test]
    fn every_macos_task_has_an_executor_branch() {
        // 与 Windows 同理：macOS 清单里的每个任务都必须在
        // execute_macos_optimize_task 里有对应分支，否则点击无反应。
        let ops_src = include_str!("../ops/mod.rs");
        for item in macos_optimize_tasks() {
            assert!(
                ops_src.contains(&format!("\"{}\" =>", item.path)),
                "macOS 优化任务 {} 在 execute_macos_optimize_task 里没有对应分支",
                item.path
            );
        }
    }

    #[test]
    fn macos_maintenance_has_high_risk_confirm_items() {
        // P1：系统维护需确认项 —— Advanced 任务必须存在且至少 2 项
        let tasks = macos_optimize_tasks();
        let advanced: Vec<&ScanItem> = tasks
            .iter()
            .filter(|t| t.recommend == Recommend::Advanced)
            .collect();
        assert!(
            advanced.len() >= 2,
            "高风险维护任务应 >= 2，实际 {}",
            advanced.len()
        );
        for t in &advanced {
            assert!(!t.description.is_empty(), "高风险任务 {} 缺少说明", t.path);
        }
    }

    #[test]
    fn every_windows_task_has_an_i18n_label() {
        // 同理：i18n 翻译表里必须有 optimize_<任务名> 的中英文案。
        // 阶段 0 后翻译表随 i18n 模块一起在 maclean-core 内（原在 app.rs）。
        let i18n_src = include_str!("../i18n.rs");
        for item in windows_optimize_tasks() {
            let key = format!("optimize_{}", item.path);
            assert!(
                i18n_src.contains(&format!("\"{}\"", key)),
                "缺少文案 key: {}",
                key
            );
        }
    }
}
