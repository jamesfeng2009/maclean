//! CLI 命令处理器（handler）。
//!
//! Phase 6（2026-10-08）从 `apps/maclean-free/src/cli.rs`（2695 行）迁入本 crate。
//! 职责：命令分发 + 输出渲染；业务逻辑全部调用 [`maclean_core`] 服务，
//! 参数处理器内**不含任何文件系统删除实现**（删除在 core::ops）。
//! 无子命令时返回帮助/错误信息（CLI 独立运行，不依赖任何 GUI 壳）。

use clap::{CommandFactory, FromArgMatches};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::args::{ApfsAction, Cli, Commands, DupIgnoreAction, OptimizeAction, StartupAction};
use crate::output::{
    emit_close_envelope, emit_error, emit_json, emit_json_status, set_command, set_request_id,
    status_for_exit_code, Status,
};
use maclean_types::cli::ScanTab;

#[cfg(target_os = "macos")]
use maclean_core::scanner::apfs::ApfsScanner;
#[cfg(target_os = "macos")]
use maclean_core::scanner::app_cache::AppCacheScanner;
#[cfg(target_os = "macos")]
use maclean_core::scanner::app_data::AppDataScanner;
use maclean_core::scanner::dev_cache::DevCacheScanner;
use maclean_core::scanner::large_files::LargeFileScanner;
use maclean_core::scanner::optimize::OptimizeScanner;
#[cfg(target_os = "macos")]
use maclean_core::scanner::startup::{
    disable_startup_item, enable_startup_item, restore_origin_from_backup, scan_startup_items,
    StartupItem,
};
#[cfg(target_os = "macos")]
use maclean_core::scanner::uninstall::UninstallScanner;
use maclean_core::scanner::{format_size, Scanner};
use maclean_core::scanner::{Recommend, ScanItem};

// =========================================================================
//  语义退出码（供脚本/CI 消费，稳定契约）
//
//  E-01：以 docs/cli-contract.md §4 为唯一事实来源：
//  0 成功；1 通用失败；2 参数错误（clap 默认，保留位）；
//  3 需确认（未带 --yes）；4 用户取消（Ctrl+C 收敛到此）；
//  5 带警告完成（clean 有失败/被拦截项）；6 输出序列化失败；
//  7 Ctrl+C 硬中断（保留位，无常量）。
//
//  Open Core 拆分后迁入 maclean-types::cli::exit_code，
//  此处 re-export 保持既有引用路径不变。
// =========================================================================
pub use maclean_types::cli::exit_code::*;

/// 取消标志：Ctrl+C 置位，扫描/删除循环检查后以 EXIT_CANCELLED(4) 退出
static CANCELLED: AtomicBool = AtomicBool::new(false);

pub fn cancelled() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}

/// 安装 Ctrl+C 处理器（CLI 专用；GUI 不安装，避免抢信号）
pub fn install_cancel_handler() {
    let _ = ctrlc::set_handler(|| {
        CANCELLED.store(true, Ordering::Relaxed);
    });
}

/// 输出格式 / 颜色控制 —— Open Core 拆分后迁入
/// `maclean-types::cli::output`，此处 re-export 保持引用路径不变。
pub use maclean_types::cli::output::{ColorMode, OutputFormat};

fn color_enabled(color: ColorMode, format: OutputFormat) -> bool {
    if format != OutputFormat::Human || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    match color {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => std::io::stdout().is_terminal(),
    }
}

/// ANSI 上色（仅 human 且启用时生效）
fn paint(enabled: bool, code: &str, s: &str) -> String {
    if enabled {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// 进度条是否启用：默认开，--no-progress 或 stderr 非终端时关
fn progress_enabled(no_progress: bool) -> bool {
    !no_progress && std::io::stderr().is_terminal()
}

/// 运行 CLI 命令，返回退出码（无子命令时返回 None 由 main 启动 GUI）
pub fn run_cli() -> Option<u8> {
    // 契约 crate 被独立使用（如测试）时没有上层二进制版本，退回自身版本
    run_cli_with_version(env!("CARGO_PKG_VERSION"))
}

/// macOS 发行二进制用：`--version` 必须显示 CLI 自身版本，而不是契约 crate 版本
pub fn run_cli_with_version(version: &'static str) -> Option<u8> {
    use clap::parser::ValueSource;
    let matches = Cli::command().version(version).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    match cli.command {
        None => None, // 无子命令，启动 GUI
        Some(cmd) => {
            install_cancel_handler();
            // E-09：用户显式传了 --format 时以它为准，不再被 --json 静默覆盖；
            // 两者同时给定时在 stderr 告警（不污染机器格式 stdout）。
            let format_explicit = matches.value_source("format") == Some(ValueSource::CommandLine);
            let format = if format_explicit {
                if cli.json {
                    eprintln!("警告：--json 与显式 --format 同时指定，以 --format 为准");
                }
                cli.format
            } else if cli.json {
                OutputFormat::Json // 兼容：仅 --json 时等价 --format json
            } else {
                cli.format
            };
            let color_on = color_enabled(cli.color, format);
            // envelope 输出上下文：一次请求一个 request_id
            set_request_id(uuid::Uuid::new_v4().to_string());
            Some(run_command(cmd, format, color_on, cli.no_progress))
        }
    }
}

/// 命令 → envelope.command 的稳定名称（JSON/JSONL 契约字段）
fn command_name(cmd: &Commands) -> &'static str {
    match cmd {
        Commands::Scan { .. } => "scan",
        Commands::Clean { .. } => "clean",
        Commands::Schedule { .. } => "schedule",
        Commands::SystemData => "system-data",
        Commands::Apfs { .. } => "apfs",
        Commands::CheckDisk { .. } => "check-disk",
        Commands::List => "list",
        Commands::Apps => "apps",
        Commands::Uninstall { .. } => "uninstall",
        Commands::Log { .. } => "log",
        Commands::Backups { .. } => "backups",
        Commands::Restore { .. } => "restore",
        Commands::Startup { .. } => "startup",
        Commands::Optimize { .. } => "optimize",
        Commands::DupIgnore { .. } => "dup-ignore",
        Commands::History => "history",
        Commands::Growth => "growth",
        Commands::Forecast => "forecast",
        Commands::Policy => "policy",
        Commands::Automation => "automation",
    }
}

fn run_command(cmd: Commands, format: OutputFormat, color: bool, no_progress: bool) -> u8 {
    let prog = progress_enabled(no_progress);
    set_command(command_name(&cmd));
    let code = match cmd {
        Commands::Scan {
            tab,
            deep,
            large_min_mb,
            dup_min_kb,
        } => cmd_scan(tab, deep, large_min_mb, dup_min_kb, format, color, prog),
        Commands::Clean {
            tab,
            safe_only,
            dry_run,
            scheduled,
            only_if_due,
            yes,
            privileged,
            large_min_mb,
            dup_min_kb,
        } => cmd_clean(
            tab,
            safe_only,
            dry_run,
            scheduled,
            only_if_due,
            yes,
            privileged,
            large_min_mb,
            dup_min_kb,
            format,
            color,
            prog,
        ),
        Commands::Schedule {
            install,
            remove,
            days,
        } => cmd_schedule(install, remove, days, format),
        Commands::SystemData => cmd_system_data(format),
        Commands::Apfs { action } => cmd_apfs(action, format),
        Commands::CheckDisk { breakdown } => cmd_check_disk(breakdown, format),
        Commands::List => cmd_list(format),
        Commands::Apps => cmd_apps(format),
        Commands::Uninstall { app, yes } => cmd_uninstall(&app, yes, format),
        // E-04：log 接收 format，机器模式输出 envelope 而非人类文本
        Commands::Log { tail, open } => cmd_log(tail, open, format),
        Commands::Backups { restorable_only } => cmd_backups(restorable_only, format),
        // 还原结果涉及逐个路径的成功/失败，JSON 更有用（脚本可据此重试）
        Commands::Restore { id } => cmd_restore(&id, format),
        Commands::Startup { action } => cmd_startup(action, format, color),
        Commands::Optimize { action } => cmd_optimize(action, format, color),
        Commands::DupIgnore { action } => cmd_dup_ignore(action, format),
        // Pro 命令：公开版仅契约占位（P6-3）
        Commands::History => pro_unavailable("history", format),
        Commands::Growth => pro_unavailable("growth", format),
        Commands::Forecast => pro_unavailable("forecast", format),
        Commands::Policy => pro_unavailable("policy", format),
        Commands::Automation => pro_unavailable("automation", format),
    };
    // E-08/E-10：机器格式补收尾信封。cancelled 单独补一个单对象；
    // jsonl 不论成功/警告/失败都补结束行（旧实现只在 EXIT_OK 补，
    // has_warnings 返回非 0 时缺结束信封行）。
    if format != OutputFormat::Human {
        if code == EXIT_CANCELLED {
            emit_close_envelope(format, Status::Cancelled);
        } else if format == OutputFormat::Jsonl {
            emit_close_envelope(format, status_for_exit_code(code));
        }
    }
    code
}

// =========================================================================
//  结构化输出（--json / --jsonl）
//
//  D-09/E-02/E-08/E-10：统一发射层已下沉到 crate::output，本文件只调用
//  emit_json / emit_json_status / emit_error / emit_close_envelope。
//    JSON  → 单个完整 envelope；
//    JSONL → 每行一个完整 envelope + 收尾信封行；
//    Human → 各命令自行 println（不经发射层）。
// =========================================================================

/// Pro 命令占位（P6-3）：公开版不可用，统一走 emit_error 错误信封。
/// 这是开源边界 DESIGN-INTENT：只返回错误信封 + 退出码，不"实现" Pro。
fn pro_unavailable(cmd: &'static str, format: OutputFormat) -> u8 {
    emit_error(
        format,
        EXIT_FAILURE,
        format!("Pro 命令 `{cmd}` 仅私有版可用"),
    )
}

// =========================================================================
//  Tab 名称映射
//
//  类别清单的唯一事实来源是 `maclean_types::cli::ScanTab`（2026-10-27
//  从这里原有的 `ALL_TABS` 字符串表迁走 —— 那张表让 `--tab` 只能是
//  `Option<String>`，拼错的值一路传到 `scan_tab` 的 `_ => Vec::new()`
//  分支，表现为"扫过了但没东西"）。这里只保留「当前平台有没有扫描器」
//  这层判断，因为它依赖 cfg，不属于契约层。
// =========================================================================

/// 该扫描类别在当前平台是否有对应扫描器
///
/// `scan_tab` 里 "apfs" 只有 `#[cfg(target_os = "macos")]` 分支，其它平台
/// 落到 `_ => Vec::new()`。`--deep` 和 `list` 若不过滤，Windows 上会打印一个
/// 恒为 0 项的「APFS快照」—— 用户以为扫过了没东西，其实是这个平台没有。
///
/// 穷举 match 而非 `_ => true`：新增类别时编译器会强制在这里表态。
fn tab_supported(tab: ScanTab) -> bool {
    match tab {
        ScanTab::Apfs => cfg!(target_os = "macos"),
        // dup-files 只实现了 macOS 扫描器；custom-rules 全平台
        ScanTab::DupFiles => cfg!(target_os = "macos"),
        ScanTab::DevCache
        | ScanTab::LargeFiles
        | ScanTab::AppCache
        | ScanTab::AppData
        | ScanTab::AppUninstall
        | ScanTab::Optimize
        | ScanTab::CustomRules => true,
        // 展开标记，不是具体类别 —— 它不该出现在候选列表里
        ScanTab::All => false,
    }
}

/// key → 人类可读标签；未知 key 原样返回（回放旧缓存时可能遇到）
fn tab_label_for(key: &str) -> &str {
    ScanTab::ALL_PUBLIC
        .iter()
        .find(|t| t.key() == key)
        .map(|t| t.label())
        .unwrap_or(key)
}

fn scan_tab(tab_name: &str) -> (Vec<ScanItem>, bool) {
    // A-07b：扫描前声明准入根。删除阶段的第 8 层只放行这些根下的路径；
    // 不注册会让所有清理项在删除时被拦，等于关停功能。
    maclean_core::provenance::register_standard_scan_roots();
    // watchdog：坏目录（APFS 异常等）会让 readdir 永久阻塞；超时返回空并提示，
    // 进程不再无限挂起（超时 tab 计入 EXIT_WARNINGS=5，E-01 契约口径）。
    const TAB_SCAN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
    // 转 String 再 move：scan_with_timeout 要求闭包 'static，不能捕获 &str
    let tab_key_owned = tab_name.to_string();
    // 阈值来自 config.json（CLI --large-min-mb / --dup-min-kb 写入）
    let cfg = maclean_core::config::load_config();
    let large_min = cfg.large_file_min_bytes;
    // dup-files 只有 macOS 扫描器；其它平台上该变量不会进闭包
    #[allow(unused_variables)]
    let dup_min = cfg.dup_file_min_bytes;
    let scan = move || match tab_key_owned.as_str() {
        "dev-cache" => DevCacheScanner::new().scan().items,
        "large-files" => LargeFileScanner::with_min_bytes(large_min).scan().items,
        #[cfg(target_os = "macos")]
        "app-cache" => AppCacheScanner::new().scan().items,
        #[cfg(target_os = "macos")]
        "app-data" => AppDataScanner::new().scan().items,
        #[cfg(target_os = "macos")]
        "app-uninstall" => UninstallScanner::new().scan().items,
        "optimize" => OptimizeScanner::new().scan().items,
        #[cfg(target_os = "macos")]
        "apfs" => ApfsScanner::new().scan().items,
        #[cfg(target_os = "macos")]
        "dup-files" => {
            maclean_core::scanner::dup_files::DuplicateFileScanner::with_min_bytes(dup_min)
                .scan()
                .items
        }
        "custom-rules" => maclean_core::rules::RuleScanner::new().scan().items,
        // Windows 扫描器
        #[cfg(target_os = "windows")]
        "app-cache" => {
            maclean_core::scanner::windows_apps::WindowsAppCacheScanner::new()
                .scan()
                .items
        }
        #[cfg(target_os = "windows")]
        "app-data" => {
            maclean_core::scanner::windows_apps::WindowsAppDataScanner::new()
                .scan()
                .items
        }
        #[cfg(target_os = "windows")]
        "app-uninstall" => {
            maclean_core::scanner::windows_apps::WindowsUninstallScanner::new()
                .scan()
                .items
        }
        _ => Vec::new(),
    };

    match maclean_core::scanner::scan_with_timeout(TAB_SCAN_TIMEOUT, scan) {
        Some(items) => (items, false),
        None => {
            eprintln!("\r  ⚠ {tab_name} 扫描超时（目录 IO 异常？），已跳过该 Tab");
            (Vec::new(), true)
        }
    }
}

// =========================================================================
//  子命令实现
// =========================================================================

#[derive(Clone, serde::Serialize)]
struct JsonItem {
    path: String,
    size_bytes: u64,
    category: String,
    description: String,
    deletable: bool,
    undeletable_reason: String,
    recommend: Recommend,
}

#[derive(Clone, serde::Serialize)]
struct JsonTab {
    key: String,
    label: String,
    count: usize,
    total_size: u64,
    safe_count: usize,
    safe_size: u64,
    elapsed_ms: u64,
    items: Vec<JsonItem>,
}

#[derive(serde::Serialize)]
struct JsonScan {
    command: &'static str,
    tabs: Vec<JsonTab>,
    total_count: usize,
    total_size: u64,
    /// E-07：可回收字节（各 Tab 的 safe_size 之和），契约示例
    /// `jq '.data.reclaimable'` 依赖此字段，与 total_size/safe_size 对齐。
    reclaimable: u64,
}

/// 把 CLI 阈值写入 config.json（GUI 与后续扫描共用同一份设置）
fn apply_scan_thresholds(large_min_mb: Option<u64>, dup_min_kb: Option<u64>) {
    if large_min_mb.is_none() && dup_min_kb.is_none() {
        return;
    }
    let mut cfg = maclean_core::config::load_config();
    if let Some(mb) = large_min_mb {
        cfg.large_file_min_bytes = mb.max(1) * 1024 * 1024;
    }
    if let Some(kb) = dup_min_kb {
        cfg.dup_file_min_bytes = kb.max(1) * 1024;
    }
    maclean_core::config::save_config(&cfg);
}

fn all_supported_tabs() -> Vec<(String, String)> {
    ScanTab::ALL_PUBLIC
        .iter()
        .filter(|t| tab_supported(**t))
        .map(|t| (t.key().to_string(), t.label().to_string()))
        .collect()
}

fn cmd_scan(
    tab: Option<ScanTab>,
    deep: bool,
    large_min_mb: Option<u64>,
    dup_min_kb: Option<u64>,
    format: OutputFormat,
    color: bool,
    prog: bool,
) -> u8 {
    apply_scan_thresholds(large_min_mb, dup_min_kb);
    let tabs: Vec<(String, String)> = if deep || tab == Some(ScanTab::All) {
        // `--deep` 与 `--tab all` 等价：扫描所有当前平台支持的 Tab
        all_supported_tabs()
    } else if let Some(t) = tab {
        // E-12：显式单 tab 也要过 tab_supported；不支持时给错误信封，
        // 不能静默返回 0 项 + exit 0（旧行为"扫过了但没东西"）。
        if !tab_supported(t) {
            return emit_error(
                format,
                EXIT_FAILURE,
                format!("当前平台不支持扫描类别 {}（{}）", t.key(), t.label()),
            );
        }
        vec![(t.key().to_string(), t.label().to_string())]
    } else {
        vec![(
            ScanTab::DevCache.key().to_string(),
            ScanTab::DevCache.label().to_string(),
        )]
    };

    let mut total_size: u64 = 0;
    let mut total_count: usize = 0;
    let mut total_reclaimable: u64 = 0;
    let mut has_warnings = false;
    // JSON 与表格共用同一份扫描结果，绝不各扫一遍
    let mut out_tabs: Vec<JsonTab> = Vec::new();
    let total_tabs = tabs.len();

    for (idx, (tab_key, tab_label)) in tabs.iter().enumerate() {
        if cancelled() {
            return EXIT_CANCELLED;
        }
        if prog {
            eprint!("\r  扫描 {}/{} — {} ...", idx + 1, total_tabs, tab_label);
        }
        let start = std::time::Instant::now();
        let (items, tab_timed_out) = scan_tab(tab_key);
        if tab_timed_out {
            has_warnings = true;
        }
        let elapsed = start.elapsed();

        // 保存扫描结果到本地缓存，便于 GUI 启动时直接加载
        if !items.is_empty() {
            let result = maclean_core::scanner::ScanResult {
                items: items.clone(),
                total_size: items.iter().map(|i| i.size_bytes).sum(),
                scan_time_ms: elapsed.as_millis() as u64,
            };
            maclean_core::scanner::cache::save_cache(tab_key, &result);
        }

        let tab_total: u64 = items.iter().map(|i| i.size_bytes).sum();
        let safe_count = items
            .iter()
            .filter(|i| i.recommend.default_selected() && i.deletable)
            .count();
        let safe_size: u64 = items
            .iter()
            .filter(|i| i.recommend.default_selected() && i.deletable)
            .map(|i| i.size_bytes)
            .sum();

        out_tabs.push(JsonTab {
            key: tab_key.clone(),
            label: tab_label.clone(),
            count: items.len(),
            total_size: tab_total,
            safe_count,
            safe_size,
            elapsed_ms: elapsed.as_millis() as u64,
            items: items
                .iter()
                .map(|i| JsonItem {
                    path: i.path.clone(),
                    size_bytes: i.size_bytes,
                    category: i.category.clone(),
                    description: i.description.clone(),
                    deletable: i.deletable,
                    undeletable_reason: i.undeletable_reason.clone(),
                    recommend: i.recommend,
                })
                .collect(),
        });

        total_size += tab_total;
        total_count += items.len();
        total_reclaimable += safe_size; // E-07

        if format == OutputFormat::Jsonl {
            // E-10：流式行也带真实汇总（旧实现把 total_count/total_size 硬编码 0，
            // 与 Json 模式口径不一致）；每行现在都是完整 envelope。
            emit_json(
                &JsonScan {
                    command: "scan",
                    tabs: vec![out_tabs.last().unwrap().clone()],
                    total_count,
                    total_size,
                    reclaimable: total_reclaimable,
                },
                format,
                false,
            );
            continue;
        }
        if format == OutputFormat::Json {
            continue;
        }

        println!("\n╔══════════════════════════════════════════╗");
        println!("║  扫描 — {} ({})", tab_label, tab_key);
        println!("╚══════════════════════════════════════════╝");
        println!(
            "  找到 {} 项，共 {}（安全 {} 项，{}）",
            items.len(),
            format_size(tab_total),
            safe_count,
            format_size(safe_size)
        );
        if items.is_empty() {
            println!("  （无）");
        }
        for item in &items {
            let mark = if item.recommend.default_selected() && item.deletable {
                paint(color, "32", "✓")
            } else {
                " ".to_string()
            };
            let prefix = if item.deletable {
                format!("[{}]", format_size(item.size_bytes))
            } else {
                format!(
                    "[-] {}",
                    if item.undeletable_reason.is_empty() {
                        "不可删除"
                    } else {
                        &item.undeletable_reason
                    }
                )
            };
            println!("  {} {} {}", mark, prefix, item.path);
        }
    }

    if prog {
        eprint!("\r\x1b[K");
    }

    // E-07/E-08：补 reclaimable，且 warnings 时信封 status 用 "warnings"
    // （旧实现恒 status:"ok" 却返回 EXIT_WARNINGS）。
    if format == OutputFormat::Json {
        let status = if has_warnings {
            Status::Warnings
        } else {
            Status::Ok
        };
        emit_json_status(
            &JsonScan {
                command: "scan",
                tabs: out_tabs,
                total_count,
                total_size,
                reclaimable: total_reclaimable,
            },
            format,
            true,
            status,
        );
    }
    if has_warnings {
        EXIT_WARNINGS
    } else {
        EXIT_OK
    }
}

#[derive(serde::Serialize)]
struct JsonPath {
    path: String,
    size_bytes: u64,
}

#[derive(serde::Serialize)]
struct JsonCleanTab {
    key: String,
    label: String,
    /// 预览模式下列出的计划删除项
    planned: Vec<JsonPath>,
    /// 实际删除的路径
    deleted: Vec<String>,
    failed: Vec<JsonFailure>,
    rejected: Vec<JsonFailure>,
    skipped_snapshots: usize,
    /// true = 本次是预览（未删除任何文件）
    preview: bool,
    /// 走废纸篓时生成的删除清单 id（可用 `maclean restore <id>` 还原）
    #[serde(skip_serializing_if = "Option::is_none")]
    backup_id: Option<String>,
}

#[derive(serde::Serialize)]
struct JsonFailure {
    path: String,
    reason: String,
}

#[derive(serde::Serialize)]
struct JsonClean {
    command: &'static str,
    /// 预览模式（未确认执行）
    preview: bool,
    dry_run: bool,
    yes: bool,
    scheduled: bool,
    /// 是否启用了权限失败自动提权（--privileged）
    privileged: bool,
    tabs: Vec<JsonCleanTab>,
    success: usize,
    failed: usize,
    rejected: usize,
}

/// 扫描项 → 实际要删除的路径列表
///
/// `batch_paths` 非空时真正的删除目标是它们（重复文件是"保留 path，删 batch"，
/// 聚合缓存是"批量路径全删"）；为空才是单个 `path`。旧实现只删 `path`，
/// 对重复文件来说删掉的是要保留的那份，属于必须修正的行为。
fn item_delete_targets(item: &ScanItem) -> Vec<(String, u64)> {
    if item.batch_paths.is_empty() {
        vec![(item.path.clone(), item.size_bytes)]
    } else {
        item.batch_paths.iter().map(|p| (p.clone(), 0)).collect()
    }
}

/// 去掉"已被同批其它项包含"的祖先目录
///
/// 大文件视图会同时列出目录和目录里的文件（磁盘分析需要两个视角），
/// 但清理时若两个都删：目录会被单独移进废纸篓、成功数虚高、体积重复计算，
/// 还原时还可能先还原子文件、父目录却已在废纸篓里 → 子项全部失败。
/// 这里保留更深的项（尊重用户勾选），父目录留空即可。
fn drop_covered_items(items: Vec<&ScanItem>) -> Vec<&ScanItem> {
    let targets: Vec<std::path::PathBuf> = items
        .iter()
        .flat_map(|i| item_delete_targets(i))
        .map(|(p, _)| std::path::PathBuf::from(p))
        .collect();
    items
        .into_iter()
        .filter(|item| {
            !item_delete_targets(item).iter().any(|(p, _)| {
                let cur = std::path::Path::new(p);
                targets
                    .iter()
                    .any(|other| other != cur && other.starts_with(cur))
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn cmd_clean(
    tab: Option<ScanTab>,
    safe_only: bool,
    dry_run: bool,
    scheduled: bool,
    only_if_due: bool,
    yes: bool,
    privileged: bool,
    large_min_mb: Option<u64>,
    dup_min_kb: Option<u64>,
    format: OutputFormat,
    color: bool,
    prog: bool,
) -> u8 {
    apply_scan_thresholds(large_min_mb, dup_min_kb);
    // 无人值守的定时清理永远只动默认安全项：即使 plist 被改掉 --safe-only，
    // 也不能让定时任务去删 Advanced/Caution 项。
    let safe_only = safe_only || scheduled;

    // 定时任务日志会无限增长（launchd 直接 append），每次跑之前先看是否需要轮转
    if scheduled {
        let _ = maclean_core::scheduler::rotate_log_if_needed();
    }

    // 登录补偿：launchd RunAtLoad / 任务计划登录触发会调用本命令。
    // 未到期就安静退出 —— 不写 last_run、不清理、不冒充成功。
    if scheduled && only_if_due {
        let cfg = maclean_core::config::load_config();
        if !maclean_core::scheduler::is_due(
            cfg.schedule_enabled,
            cfg.schedule_interval_days,
            cfg.schedule_last_run,
            now_secs(),
        ) {
            if format == OutputFormat::Human {
                println!(
                    "⏭️  定时清理未到期（每 {} 天），本次跳过",
                    cfg.schedule_interval_days
                );
            }
            return EXIT_OK;
        }
    }

    // 安全确认（P0）：
    // - 定时任务（--scheduled）视为用户已在 GUI 配置时确认 → 无人值守自动执行
    // - --yes 显式确认执行
    // - 否则一律预览（不删除）；非交互终端（stdin 非 TTY）时拒绝执行并退出码 3
    let confirmed = yes || scheduled;
    let preview = dry_run || !confirmed;
    // 只有真正执行了删除才记 last_run；预览/被确认保护挡下都不算"跑过"
    let ran_to_completion = !preview;
    if !confirmed && !dry_run && !std::io::stdin().is_terminal() {
        // 非交互环境：不给预览的机会直接要求 --yes（脚本必须显式确认删除意图）
        // E-02：统一走 emit_error，机器格式输出错误信封而非裸 stdout 文本
        return emit_error(
            format,
            EXIT_CONFIRM_REQUIRED,
            "非交互环境执行删除必须显式携带 --yes（仅预览请加 --dry-run）",
        );
    }

    // 定时清理不再只清开发缓存：无人值守时覆盖所有"安全默认项"的缓存类 Tab，
    // 但排除 app-uninstall（会把应用当清理项）与 apfs（快照需要授权）。
    const SCHEDULED_SAFE_TABS: &[ScanTab] = &[
        ScanTab::DevCache,
        ScanTab::AppCache,
        ScanTab::AppData,
        ScanTab::Optimize,
        ScanTab::CustomRules,
    ];
    let tabs: Vec<String> = if tab == Some(ScanTab::All) {
        all_supported_tabs().into_iter().map(|(k, _)| k).collect()
    } else if let Some(t) = tab {
        // E-12：单 tab 也过 tab_supported，平台不支持时给错误信封
        if !tab_supported(t) {
            return emit_error(
                format,
                EXIT_FAILURE,
                format!("当前平台不支持清理类别 {}（{}）", t.key(), t.label()),
            );
        }
        vec![t.key().to_string()]
    } else if scheduled {
        SCHEDULED_SAFE_TABS
            .iter()
            .filter(|t| tab_supported(**t))
            .map(|t| t.key().to_string())
            .collect()
    } else {
        vec![ScanTab::DevCache.key().to_string()]
    };

    let mut out: Vec<JsonCleanTab> = Vec::new();
    let mut g_success = 0usize;
    let mut g_failed = 0usize;
    let mut g_rejected = 0usize;

    for tab_key in &tabs {
        if cancelled() {
            return EXIT_CANCELLED;
        }
        let tab_label = tab_label_for(tab_key);

        let (items, _tab_timed_out) = scan_tab(tab_key);

        let to_clean: Vec<&ScanItem> = if safe_only {
            items
                .iter()
                .filter(|i| i.recommend.default_selected() && i.deletable)
                .collect()
        } else {
            items.iter().filter(|i| i.deletable).collect()
        };
        // 大文件视图同时给出目录与其内部文件：清理只能删一遍，保留更深的项
        let to_clean = drop_covered_items(to_clean);

        if to_clean.is_empty() {
            if format == OutputFormat::Human {
                println!("  （无可清理项目）");
            }
            continue;
        }

        let clean_size: u64 = to_clean.iter().map(|i| i.size_bytes).sum();
        maclean_core::logger::info(&format!(
            "[删除] 任务开始: {} — {} 项（{}）",
            tab_key,
            to_clean.len(),
            format_size(clean_size)
        ));

        if format == OutputFormat::Human {
            println!("\n🧹 清理 {} ({})", tab_label, tab_key);
            if preview {
                println!(
                    "  [预览] 将清理 {} 项，释放 {}（加 --yes 执行）",
                    to_clean.len(),
                    format_size(clean_size)
                );
            } else {
                println!(
                    "  将清理 {} 项，释放 {}",
                    to_clean.len(),
                    format_size(clean_size)
                );
            }
        }

        // 预览模式：列出计划项，不删除
        if preview {
            let rec = JsonCleanTab {
                key: tab_key.clone(),
                label: tab_label.to_string(),
                planned: to_clean
                    .iter()
                    .flat_map(|i| item_delete_targets(i))
                    .map(|(path, size_bytes)| JsonPath { path, size_bytes })
                    .collect(),
                deleted: Vec::new(),
                failed: Vec::new(),
                rejected: Vec::new(),
                skipped_snapshots: 0,
                preview: true,
                backup_id: None,
            };
            if format == OutputFormat::Jsonl {
                emit_json(&rec, format, false);
            } else if format == OutputFormat::Json {
                out.push(rec);
            } else {
                for item in &to_clean {
                    for (path, size) in item_delete_targets(item) {
                        let shown = if size == 0 {
                            "大小未知".to_string()
                        } else {
                            format_size(size)
                        };
                        println!("    - [{}] {}", shown, path);
                    }
                }
            }
            continue;
        }

        // 删除前复做安全校验（与 GUI 同一层 TOCTOU 防护）
        // 快照/模拟器运行时不是普通文件：best_effort_delete 只会失败。
        // CLI 的删除入口是 `maclean apfs delete`（快照会走管理员授权）。
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut skipped_snapshots = 0usize;
        for item in &to_clean {
            if item.path.starts_with("snapshot:")
                || item.category == "APFS快照"
                || item.category == "模拟器运行时"
            {
                skipped_snapshots += 1;
                continue;
            }
            for (path, _) in item_delete_targets(item) {
                pairs.push((path, item.category.clone()));
            }
        }

        // 删除策略：大文件/重复文件默认走废纸篓（可 restore），
        // 用户把设置改成 "trash" 时全部走废纸篓；其余沿用永久删除。
        let delete_cfg = maclean_core::config::load_config();
        let use_trash = matches!(tab_key.as_str(), "large-files" | "dup-files")
            || delete_cfg.settings_delete_strategy == "trash";

        let (allowed, rejected) = maclean_core::ops::sanitize_before_delete(pairs, false);
        g_rejected += rejected.len();

        if format == OutputFormat::Human {
            for (path, _category, reason) in &rejected {
                println!(
                    "  {} {} — {}",
                    paint(color, "33", "🛡️  已拦截"),
                    path,
                    reason
                );
            }
            if skipped_snapshots > 0 {
                println!(
                    "  ⏭️  跳过 {} 个 APFS 快照/模拟器运行时（用 `maclean apfs list` 查看，`maclean apfs delete <名称> --yes` 删除）",
                    skipped_snapshots
                );
            }
        }

        let mut rec = JsonCleanTab {
            key: tab_key.clone(),
            label: tab_label.to_string(),
            planned: Vec::new(),
            deleted: Vec::new(),
            failed: rejected
                .iter()
                .map(|(p, _c, r)| JsonFailure {
                    path: p.clone(),
                    reason: r.clone(),
                })
                .collect(),
            rejected: Vec::new(),
            skipped_snapshots,
            preview: false,
            backup_id: None,
        };

        // 实际删除（带进度与取消检查）
        let mut success = 0usize;
        let mut failed = 0usize;
        // 走废纸篓的项：与删除清单一起落盘，之后可用 `maclean restore <id>`
        let mut trash_entries: Vec<maclean_core::backup::BackupEntry> = Vec::new();
        // 权限类失败项单独收集：提权重试（--privileged）后再定成败，
        // 保证"失败计数只报真实再次失败项"（用户多次确认的口径）。
        let mut priv_needed: Vec<(String, String)> = Vec::new();
        let total = allowed.len();
        let mut done = 0usize;
        #[allow(clippy::explicit_counter_loop)]
        for (path, _category) in &allowed {
            if cancelled() {
                return EXIT_CANCELLED;
            }
            if prog && total > 0 {
                eprint!("\r  删除 {}/{} ...", done + 1, total);
            }
            done += 1;

            // 可还原路径：移入废纸篓并记录清单。移入失败**绝不降级成永久删除**，
            // 否则一次权限失败就会悄悄毁掉用户数据。
            if use_trash {
                let size = std::fs::symlink_metadata(path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                if maclean_core::ops::move_to_trash(path) {
                    rec.deleted.push(path.clone());
                    success += 1;
                    trash_entries.push(maclean_core::backup::BackupEntry {
                        path: path.clone(),
                        size_bytes: size,
                        category: _category.clone(),
                        restorable: maclean_core::backup::is_restorable_by_move(
                            true,
                            std::env::consts::OS,
                        ),
                        // B-04 新增字段：CLI 的 bool 版 move_to_trash 无法确认
                        // 废纸篓实际落点，留空 = 无法确认归属，还原时按 failed
                        // 处理（fail-closed，绝不退回"按 basename 猜一个搬过去"）。
                        trash_name: String::new(),
                    });
                    if format == OutputFormat::Jsonl {
                        emit_json(
                            &JsonlCleanEvent {
                                event: "deleted",
                                tab: tab_key.clone(),
                                path: path.clone(),
                                size_bytes: size,
                                reason: "trash".to_string(),
                            },
                            format,
                            false,
                        );
                    } else if format == OutputFormat::Human {
                        println!("  {} {}（已移入废纸篓）", paint(color, "32", "✅"), path);
                    }
                } else {
                    let reason = "移入废纸篓失败（未删除任何内容）".to_string();
                    maclean_core::logger::warn(&format!("[删除] 失败: {} — {}", path, reason));
                    rec.failed.push(JsonFailure {
                        path: path.clone(),
                        reason: reason.clone(),
                    });
                    failed += 1;
                    if format == OutputFormat::Jsonl {
                        emit_json(
                            &JsonlCleanEvent {
                                event: "failed",
                                tab: tab_key.clone(),
                                path: path.clone(),
                                size_bytes: 0,
                                reason: reason.clone(),
                            },
                            format,
                            false,
                        );
                    } else if format == OutputFormat::Human {
                        println!("  {} {} — {}", paint(color, "31", "❌"), path, reason);
                    }
                }
                continue;
            }

            // P6-4：删除实现下沉到 core::ops（best_effort_delete_with_reason，
            // 含 TOCTOU staging / rm -rf / chflags·chmod 重试 / 失败归因）。
            // CLI 参数处理器不包含任何文件系统删除实现。
            let result =
                maclean_core::ops::best_effort_delete_with_reason(std::path::Path::new(path));

            match result {
                Ok(()) => {
                    rec.deleted.push(path.clone());
                    success += 1;
                    if format == OutputFormat::Jsonl {
                        emit_json(
                            &JsonlCleanEvent {
                                event: "deleted",
                                tab: tab_key.clone(),
                                path: path.clone(),
                                size_bytes: 0,
                                reason: String::new(),
                            },
                            format,
                            false,
                        );
                    } else if format == OutputFormat::Human {
                        println!("  {} {}", paint(color, "32", "✅"), path);
                    }
                }
                Err(e) => {
                    if e == maclean_core::ops::DeleteFailure::PermissionDenied {
                        // 权限不足：先收集，等提权重试阶段统一处理（用户可见）
                        priv_needed.push((path.clone(), _category.clone()));
                        if format == OutputFormat::Human {
                            println!("  {} {} — 需要管理员权限", paint(color, "33", "⛔"), path);
                        }
                        continue;
                    }
                    let reason = format!("{:?}", e);
                    maclean_core::logger::warn(&format!("[删除] 失败: {} — {}", path, reason));
                    rec.failed.push(JsonFailure {
                        path: path.clone(),
                        reason: reason.clone(),
                    });
                    failed += 1;
                    if format == OutputFormat::Jsonl {
                        emit_json(
                            &JsonlCleanEvent {
                                event: "failed",
                                tab: tab_key.clone(),
                                path: path.clone(),
                                size_bytes: 0,
                                reason: reason.clone(),
                            },
                            format,
                            false,
                        );
                    } else if format == OutputFormat::Human {
                        println!("  {} {} — {}", paint(color, "31", "❌"), path, reason);
                    }
                }
            }
        }

        // 权限失败项处理：--privileged 自动提权（必须先让用户知晓，再触发系统授权）
        if !priv_needed.is_empty() {
            #[cfg(target_os = "macos")]
            {
                if privileged && std::io::stdin().is_terminal() {
                    // E-05：提权提示只在 human 打印；机器格式走 stderr，
                    // 绝不污染 stdout（否则 jq 解析失败）。
                    if format == OutputFormat::Human {
                        // 用户知晓：逐项列出即将提权删除的路径，然后才弹 Touch ID
                        println!(
                            "\n  🔐 以下 {} 项需要管理员权限，即将弹出系统授权（Touch ID / 密码）：",
                            priv_needed.len()
                        );
                        for (path, _) in &priv_needed {
                            println!("    - {}", path);
                        }
                        println!("  ⏳ 请在系统弹窗中完成授权（可随时取消）…\n");
                    } else {
                        eprintln!(
                            "以下 {} 项需要管理员权限，即将弹出系统授权（Touch ID / 密码）",
                            priv_needed.len()
                        );
                    }
                    let (ok_paths, fail_paths) =
                        maclean_core::ops::cli_sudo_delete_touchid(priv_needed);
                    for (path, category) in ok_paths {
                        rec.deleted.push(path.clone());
                        success += 1;
                        if format == OutputFormat::Jsonl {
                            emit_json(
                                &JsonlCleanEvent {
                                    event: "deleted",
                                    tab: tab_key.clone(),
                                    path: path.clone(),
                                    size_bytes: 0,
                                    reason: "privileged".to_string(),
                                },
                                format,
                                false,
                            );
                        } else if format == OutputFormat::Human {
                            println!("  {} {} (提权)", paint(color, "32", "✅"), path);
                            let _ = category;
                        }
                    }
                    for (path, reason) in fail_paths {
                        maclean_core::logger::warn(&format!(
                            "[删除] 失败(提权): {} — {}",
                            path, reason
                        ));
                        rec.failed.push(JsonFailure {
                            path: path.clone(),
                            reason: reason.clone(),
                        });
                        failed += 1;
                        if format == OutputFormat::Jsonl {
                            emit_json(
                                &JsonlCleanEvent {
                                    event: "failed",
                                    tab: tab_key.clone(),
                                    path: path.clone(),
                                    size_bytes: 0,
                                    reason,
                                },
                                format,
                                false,
                            );
                        } else if format == OutputFormat::Human {
                            println!("  {} {} — {}", paint(color, "31", "❌"), path, reason);
                        }
                    }
                } else if privileged {
                    // 非交互环境：无法弹窗授权，如实报告（脚本确定性）
                    for (path, _) in &priv_needed {
                        maclean_core::logger::warn(&format!(
                            "[删除] 失败(需提权): {} — 需要管理员权限（非交互环境无法弹窗授权）",
                            path
                        ));
                        rec.failed.push(JsonFailure {
                            path: path.clone(),
                            reason: "需要管理员权限（非交互环境无法弹窗授权）".to_string(),
                        });
                        failed += 1;
                        if format == OutputFormat::Jsonl {
                            emit_json(
                                &JsonlCleanEvent {
                                    event: "failed",
                                    tab: tab_key.clone(),
                                    path: path.clone(),
                                    size_bytes: 0,
                                    reason: "需要管理员权限（非交互环境无法弹窗授权）".to_string(),
                                },
                                format,
                                false,
                            );
                        } else if format == OutputFormat::Human {
                            println!(
                                "  {} {} — 需要管理员权限（非交互环境无法弹窗授权）",
                                paint(color, "31", "❌"),
                                path
                            );
                        }
                    }
                } else {
                    // 未开启 --privileged：给出明确指引
                    for (path, _) in &priv_needed {
                        maclean_core::logger::warn(&format!(
                            "[删除] 失败(需提权): {} — 需要管理员权限（可加 --privileged 自动提权）",
                            path
                        ));
                        rec.failed.push(JsonFailure {
                            path: path.clone(),
                            reason: "需要管理员权限（可加 --privileged 自动提权）".to_string(),
                        });
                        failed += 1;
                        if format == OutputFormat::Jsonl {
                            emit_json(
                                &JsonlCleanEvent {
                                    event: "failed",
                                    tab: tab_key.clone(),
                                    path: path.clone(),
                                    size_bytes: 0,
                                    reason: "需要管理员权限（可加 --privileged 自动提权）"
                                        .to_string(),
                                },
                                format,
                                false,
                            );
                        }
                    }
                    if format == OutputFormat::Human {
                        println!(
                            "\n  💡 {} 项需要管理员权限，可用 `--privileged` 自动提权删除（将弹出系统授权）",
                            priv_needed.len()
                        );
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                for (path, _) in &priv_needed {
                    maclean_core::logger::warn(&format!(
                        "[删除] 失败(需提权): {} — 当前平台 CLI 不支持自动提权",
                        path
                    ));
                    rec.failed.push(JsonFailure {
                        path: path.clone(),
                        reason: "需要管理员权限（当前平台 CLI 不支持自动提权）".to_string(),
                    });
                    failed += 1;
                }
                if format == OutputFormat::Human {
                    println!(
                        "\n  💡 {} 项需要管理员权限，当前平台 CLI 不支持自动提权",
                        priv_needed.len()
                    );
                }
            }
        }

        if prog {
            eprint!("\r\x1b[K");
        }

        // 废纸篓删除落一份清单：`maclean backups` 能看到，`maclean restore <id>`
        // 能把还在废纸篓里的项搬回去。清单写失败不阻断删除（但会明确提示）。
        if !trash_entries.is_empty() {
            let id = maclean_core::backup::record(trash_entries);
            rec.backup_id = id.clone();
            if format == OutputFormat::Human {
                match id {
                    Some(id) => println!("\n  📋 已移入废纸篓，可用 `maclean restore {}` 还原", id),
                    None => {
                        println!("\n  ⚠️  删除已完成，但删除清单写入失败（无法用 restore 还原）")
                    }
                }
            }
        }

        g_success += success;
        g_failed += failed;

        if format == OutputFormat::Human {
            println!(
                "\n  完成：成功 {}，失败 {}，安全拦截 {}",
                success,
                failed,
                rejected.len()
            );
        }
        out.push(rec);
        maclean_core::logger::info(&format!(
            "[删除] 任务完成: {} — 成功 {}，失败 {}，拦截 {}",
            tab_key,
            success,
            failed,
            rejected.len()
        ));
    }

    // E-10/E-08：Json 与 Jsonl 都输出真实汇总（旧实现只在 Json 输出），
    // 且部分失败/被拦截时信封 status 用 "warnings"，不再恒 "ok"。
    if format != OutputFormat::Human {
        let status = if g_failed > 0 || g_rejected > 0 {
            Status::Warnings
        } else {
            Status::Ok
        };
        emit_json_status(
            &JsonClean {
                command: "clean",
                preview,
                dry_run,
                yes,
                scheduled,
                privileged,
                tabs: out,
                success: g_success,
                failed: g_failed,
                rejected: g_rejected,
            },
            format,
            true,
            status,
        );
    }

    // C-3：只有真正执行过（不是预览、不是被确认保护挡下）才记账，
    // 设置页的"上次执行"才不会被误报成成功。
    if scheduled && ran_to_completion {
        let mut cfg = maclean_core::config::load_config();
        cfg.schedule_last_run = now_secs();
        maclean_core::config::save_config(&cfg);
        maclean_core::logger::info(&format!(
            "[定时] 清理完成，schedule_last_run={}",
            cfg.schedule_last_run
        ));
    }

    if cancelled() {
        EXIT_CANCELLED
    } else if g_failed > 0 || g_rejected > 0 {
        EXIT_WARNINGS
    } else {
        EXIT_OK
    }
}

#[derive(serde::Serialize)]
struct JsonlCleanEvent {
    event: &'static str,
    tab: String,
    path: String,
    size_bytes: u64,
    reason: String,
}

/// 递归统计目录大小（有界：最多 20 万条目，避免扫到 / 或坏目录时卡死）
///
/// 不跟随符号链接：否则 `~/Library` 里的链接可能把统计带进循环或别的卷。
fn dir_size_bounded(root: &std::path::Path) -> u64 {
    const MAX_ENTRIES: usize = 200_000;
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if seen >= MAX_ENTRIES {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            seen += 1;
            if seen >= MAX_ENTRIES {
                break;
            }
            let path = entry.path();
            let Ok(md) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if md.is_dir() {
                stack.push(path);
            } else if md.is_file() {
                total = total.saturating_add(md.len());
            }
        }
    }
    total
}

#[derive(serde::Serialize)]
struct JsonSystemDataRow {
    category: String,
    bytes: u64,
    detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    paths: Vec<String>,
}

#[derive(serde::Serialize)]
struct JsonSystemData {
    command: &'static str,
    total_bytes: u64,
    items: Vec<JsonSystemDataRow>,
}

/// System Data 聚合视图
///
/// Apple 的 “System Data” 是缓存、日志、本地快照、iOS 备份、App 支持文件等
/// 多类数据的总和，不对应单个目录。这里逐项拆开，让用户知道每块是什么、
/// 哪些能清、哪些只是展示。
fn cmd_system_data(format: OutputFormat) -> u8 {
    let home = maclean_core::platform::home_dir();
    let sum = |items: &[ScanItem]| -> u64 { items.iter().map(|i| i.size_bytes).sum() };

    let (dev, _) = scan_tab("dev-cache");
    let (app_cache, _) = scan_tab("app-cache");
    let (app_data, _) = scan_tab("app-data");
    let (apfs, _) = scan_tab("apfs");

    let logs = dir_size_bounded(&home.join("Library").join("Logs"))
        + dir_size_bounded(std::path::Path::new("/Library/Logs"));
    let ios_backup = dir_size_bounded(
        &home
            .join("Library")
            .join("Application Support")
            .join("MobileSync")
            .join("Backup"),
    );
    let containers = dir_size_bounded(&home.join("Library").join("Containers"))
        + dir_size_bounded(&home.join("Library").join("Group Containers"));

    let snapshot_count = apfs.iter().filter(|i| i.category == "APFS快照").count();
    let snapshot_known: u64 = apfs
        .iter()
        .filter(|i| i.category == "APFS快照")
        .map(|i| i.size_bytes)
        .sum();

    let rows = vec![
        JsonSystemDataRow {
            category: "缓存（应用）".to_string(),
            bytes: sum(&app_cache),
            detail: "~/Library/Caches 等应用缓存，可在 App 缓存页清理".to_string(),
            paths: vec![],
        },
        JsonSystemDataRow {
            category: "缓存（开发者）".to_string(),
            bytes: sum(&dev),
            detail: "Xcode / npm / Docker / AI 模型等开发缓存".to_string(),
            paths: vec![],
        },
        JsonSystemDataRow {
            category: "日志".to_string(),
            bytes: logs,
            detail: "~/Library/Logs 与 /Library/Logs（系统日志不建议手动删）".to_string(),
            paths: vec![
                home.join("Library/Logs").display().to_string(),
                "/Library/Logs".to_string(),
            ],
        },
        JsonSystemDataRow {
            category: "本地快照".to_string(),
            bytes: snapshot_known,
            detail: format!(
                "{} 个 APFS 本地快照（Time Machine 本地快照）；用 `maclean apfs list` 查看",
                snapshot_count
            ),
            paths: vec![],
        },
        JsonSystemDataRow {
            category: "iOS 备份".to_string(),
            bytes: ios_backup,
            detail: "~/Library/Application Support/MobileSync/Backup".to_string(),
            paths: vec![home
                .join("Library/Application Support/MobileSync/Backup")
                .display()
                .to_string()],
        },
        JsonSystemDataRow {
            category: "应用支持".to_string(),
            bytes: sum(&app_data),
            detail: "Application Support 中的应用数据".to_string(),
            paths: vec![],
        },
        JsonSystemDataRow {
            category: "容器与沙盒".to_string(),
            bytes: containers,
            detail: "~/Library/Containers 与 Group Containers（仅展示，清理需确认）".to_string(),
            paths: vec![],
        },
    ];

    let total: u64 = rows.iter().map(|r| r.bytes).sum();
    if format != OutputFormat::Human {
        emit_json(
            &JsonSystemData {
                command: "system-data",
                total_bytes: total,
                items: rows,
            },
            format,
            true,
        );
        return EXIT_OK;
    }

    println!("\nSystem Data 拆分（合计 {}）：\n", format_size(total));
    for r in &rows {
        let size = if r.bytes == 0 && r.category == "本地快照" {
            "大小未知".to_string()
        } else {
            format_size(r.bytes)
        };
        println!("  {:<14} {:>10}  {}", r.category, size, r.detail);
    }
    println!(
        "\n提示：缓存类可用 `maclean clean --tab all --safe-only --yes` 清理；\n\
         iOS 备份 / 容器数据请先确认不再需要；本地快照用 `maclean apfs` 管理。"
    );
    EXIT_OK
}

#[derive(serde::Serialize)]
struct JsonApfsItem {
    path: String,
    category: String,
    size_bytes: u64,
}

#[derive(serde::Serialize)]
struct JsonApfs {
    command: &'static str,
    action: &'static str,
    items: Vec<JsonApfsItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deleted: Option<String>,
}

/// APFS 本地快照 / 模拟器运行时：列出与删除
///
/// `clean --tab apfs` 会跳过 `snapshot:` 项（快照不是普通文件），
/// 这个命令是 CLI 唯一的快照删除入口。删除快照时非 root 会走
/// osascript 管理员授权（Touch ID / 密码）。
fn cmd_apfs(action: ApfsAction, format: OutputFormat) -> u8 {
    let (items, _timed_out) = scan_tab("apfs");

    match action {
        ApfsAction::List => {
            if format != OutputFormat::Human {
                emit_json(
                    &JsonApfs {
                        command: "apfs",
                        action: "list",
                        items: items
                            .iter()
                            .map(|i| JsonApfsItem {
                                path: i.path.clone(),
                                category: i.category.clone(),
                                size_bytes: i.size_bytes,
                            })
                            .collect(),
                        deleted: None,
                    },
                    format,
                    true,
                );
                return EXIT_OK;
            }
            println!("\nAPFS 快照 / 模拟器运行时：\n");
            if items.is_empty() {
                println!("  （无）");
            } else {
                for item in &items {
                    let size = if item.size_bytes == 0 {
                        "大小未知".to_string()
                    } else {
                        format_size(item.size_bytes)
                    };
                    println!("  - {}  [{}]  {}", item.path, item.category, size);
                }
                println!(
                    "\n删除：maclean apfs delete <名称> --yes\n（快照删除需要管理员权限，会弹系统授权）"
                );
            }
            EXIT_OK
        }
        ApfsAction::Delete { target, yes } => {
            if !yes {
                // E-02：需确认也是错误信封，退出码 EXIT_CONFIRM_REQUIRED(3)
                return emit_error(
                    format,
                    EXIT_CONFIRM_REQUIRED,
                    format!(
                        "删除前需确认：maclean apfs delete '{}' --yes（快照/模拟器运行时删除不可恢复）",
                        target
                    ),
                );
            }

            // 用扫描结果确认类别；扫描拿不到时按名称形态兜底
            let matched = items.iter().find(|i| {
                i.path == target
                    || i.path.ends_with(&format!("| {}", target))
                    || i.path.ends_with(target.as_str())
            });
            let category = matched.map(|i| i.category.as_str()).unwrap_or_else(|| {
                if target.contains("com.apple.TimeMachine.") {
                    "APFS快照"
                } else {
                    "模拟器运行时"
                }
            });

            let result: Result<(), String> = {
                #[cfg(target_os = "macos")]
                {
                    if category == "APFS快照" {
                        maclean_core::scanner::apfs::delete_snapshot(&target)
                    } else {
                        maclean_core::scanner::apfs::delete_simulator_runtime(&target)
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    // 非 macOS 没有 APFS 快照 / 模拟器运行时，明确拒绝而不是静默成功
                    let _ = category;
                    Err("APFS 快照 / 模拟器运行时管理仅支持 macOS".to_string())
                }
            };

            match result {
                Ok(()) => {
                    maclean_core::logger::info(&format!("[APFS] 已删除: {}", target));
                    if format != OutputFormat::Human {
                        emit_json(
                            &JsonApfs {
                                command: "apfs",
                                action: "delete",
                                items: Vec::new(),
                                deleted: Some(target.clone()),
                            },
                            format,
                            true,
                        );
                    } else {
                        println!("✅ 已删除 {}", target);
                    }
                    EXIT_OK
                }
                Err(e) => {
                    maclean_core::logger::warn(&format!("[APFS] 删除失败: {} — {}", target, e));
                    // E-02：失败路径统一错误信封，不再裸 eprintln
                    emit_error(format, EXIT_FAILURE, format!("❌ 删除失败：{}", e))
                }
            }
        }
    }
}

#[derive(serde::Serialize)]
struct JsonDisk {
    command: &'static str,
    total_bytes: u64,
    used_bytes: u64,
    free_bytes: u64,
    used_percent: f64,
    free_percent: f64,
    /// 告警等级：0 正常 / 1 注意 / 2 警告 / 3 危险
    alert_level: u8,
}

#[derive(serde::Serialize)]
struct JsonBreakdownEntry {
    category: String,
    size_bytes: u64,
    /// 占总扫描结果的比例（0-100）
    percent: f64,
    count: usize,
}

#[derive(serde::Serialize)]
struct JsonBreakdown {
    command: &'static str,
    tabs_scanned: Vec<String>,
    total_bytes: u64,
    total_count: usize,
    categories: Vec<JsonBreakdownEntry>,
}

fn cmd_check_disk(breakdown: bool, format: OutputFormat) -> u8 {
    if breakdown {
        return cmd_check_disk_breakdown(format);
    }
    let (total, free) = get_disk_info();
    if total == 0 {
        // E-02：拿不到磁盘信息必须输出完整错误信封（含 contract_version /
        // request_id / status / error_code），不能手写缺字段的 JSON 到 stderr，
        // 更不能静默成功。
        return emit_error(
            format,
            EXIT_FAILURE,
            "无法获取磁盘信息 / unable to read disk info",
        );
    }

    let used = total - free;
    let used_pct = used as f64 / total as f64 * 100.0;
    let free_pct = free as f64 / total as f64 * 100.0;

    // 告警等级
    let (level, icon, msg) = if free_pct < 5.0 {
        (3, "🔴", "危险！磁盘空间严重不足，建议立即清理")
    } else if free_pct < 10.0 {
        (2, "🟠", "警告：磁盘空间不足，建议清理")
    } else if free_pct < 20.0 {
        (1, "🟡", "注意：磁盘空间偏低，可考虑清理")
    } else {
        (0, "🟢", "正常：磁盘空间充足")
    };

    if format != OutputFormat::Human {
        emit_json(
            &JsonDisk {
                command: "check-disk",
                total_bytes: total,
                used_bytes: used,
                free_bytes: free,
                used_percent: used_pct,
                free_percent: free_pct,
                alert_level: level,
            },
            format,
            true,
        );
        return EXIT_OK;
    }

    println!("╔══════════════════════════════════════════╗");
    println!("║           磁盘空间检查                    ║");
    println!("╠══════════════════════════════════════════╣");
    println!("  总容量:  {}", format_size(total));
    println!("  已使用:  {} ({:.1}%)", format_size(used), used_pct);
    println!("  可用:    {} ({:.1}%)", format_size(free), free_pct);
    println!();

    println!("  {} {}", icon, msg);

    // 进度条
    let bar_len = 30;
    let filled = (used_pct / 100.0 * bar_len as f64).round() as usize;
    let bar: String = "█".repeat(filled) + &"░".repeat(bar_len - filled);
    println!("\n  [{}] {:.1}%", bar, used_pct);

    if level >= 2 {
        println!("\n  💡 建议：运行 `maclean scan --deep` 查看可清理项目");
    }

    println!("╚══════════════════════════════════════════╝");
    EXIT_OK
}

/// 分类占比总览（P2 口径，与 GUI 磁盘分析器的 aggregate_category_sizes 一致）
fn cmd_check_disk_breakdown(format: OutputFormat) -> u8 {
    // 扫描主要缓存类别（跳过 optimize 这类"操作型"Tab）
    let keys = [
        ScanTab::DevCache,
        ScanTab::LargeFiles,
        ScanTab::AppCache,
        ScanTab::AppData,
        ScanTab::CustomRules,
        ScanTab::DupFiles,
    ];
    // E-11：三元组 (category, size_bytes, count)，聚合时同步累加 count
    let mut categories: Vec<(String, u64, usize)> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut total_count: usize = 0;
    let mut scanned: Vec<String> = Vec::new();

    // 6 个 Tab 串行扫描在一台装满缓存的机器上会明显拖慢 breakdown。
    // 用 scoped thread 分组并行（每批 3 个，避免 6 路 IO 抢元数据反而更慢）。
    let mut results: Vec<(&str, Vec<ScanItem>, bool)> = Vec::new();
    std::thread::scope(|scope| {
        for chunk in keys.chunks(3) {
            let handles = chunk
                .iter()
                .filter(|t| tab_supported(**t))
                .map(|t| {
                    let key = t.key();
                    (key, scope.spawn(move || scan_tab(key)))
                })
                .collect::<Vec<_>>();
            for (key, handle) in handles {
                let (items, timed_out) = handle.join().unwrap_or_default();
                results.push((key, items, timed_out));
            }
        }
    });

    for (key, items, _timed_out) in results {
        scanned.push(key.to_string());
        total_count += items.len();
        for item in &items {
            total_bytes += item.size_bytes;
            if let Some(entry) = categories.iter_mut().find(|(c, _, _)| *c == item.category) {
                entry.1 += item.size_bytes;
                entry.2 += 1; // E-11：同步累加该类目的项数
            } else {
                categories.push((item.category.clone(), item.size_bytes, 1));
            }
        }
    }
    // 按占用降序；sort_by_key 同样稳定，同尺寸类别保持首次出现顺序
    categories.sort_by_key(|a| std::cmp::Reverse(a.1));
    categories.retain(|(_, s, _)| *s > 0);

    let entries: Vec<JsonBreakdownEntry> = categories
        .iter()
        .map(|(cat, size, count)| JsonBreakdownEntry {
            category: cat.clone(),
            size_bytes: *size,
            percent: if total_bytes > 0 {
                *size as f64 / total_bytes as f64 * 100.0
            } else {
                0.0
            },
            count: *count, // E-11：不再是恒 0
        })
        .collect();

    if format != OutputFormat::Human {
        emit_json(
            &JsonBreakdown {
                command: "check-disk",
                tabs_scanned: scanned,
                total_bytes,
                total_count,
                categories: entries,
            },
            format,
            true,
        );
        return EXIT_OK;
    }

    println!("╔══════════════════════════════════════════╗");
    println!("║        磁盘占用分类占比（扫描结果）        ║");
    println!("╚══════════════════════════════════════════╝");
    println!("  扫描类别: {}", scanned.join(", "));
    println!("  共 {} 项，{}", total_count, format_size(total_bytes));
    println!();
    if entries.is_empty() {
        println!("  （无分类数据）");
        return EXIT_OK;
    }
    for (i, e) in entries.iter().enumerate() {
        // 横向占比条
        let bar_len = 30;
        let filled = (e.percent / 100.0 * bar_len as f64).round() as usize;
        let bar: String = "█".repeat(filled) + &"░".repeat(bar_len - filled);
        println!(
            "  {:>2}. {:<14} {:>9} {:>5.1}%  [{}]",
            i + 1,
            e.category,
            format_size(e.size_bytes),
            e.percent,
            bar
        );
    }
    EXIT_OK
}

#[derive(serde::Serialize)]
struct JsonListEntry<'a> {
    key: &'a str,
    label: &'a str,
}

#[derive(serde::Serialize)]
struct JsonList<'a> {
    command: &'a str,
    tabs: Vec<JsonListEntry<'a>>,
}

fn cmd_list(format: OutputFormat) -> u8 {
    let supported: Vec<(&str, &str)> = ScanTab::ALL_PUBLIC
        .iter()
        .filter(|t| tab_supported(**t))
        .map(|t| (t.key(), t.label()))
        .collect();

    if format != OutputFormat::Human {
        emit_json(
            &JsonList {
                command: "list",
                tabs: supported
                    .iter()
                    .map(|(k, v)| JsonListEntry { key: k, label: v })
                    .collect(),
            },
            format,
            true,
        );
        return EXIT_OK;
    }

    println!("\nmaclean 可用扫描类别：\n");
    for (key, label) in &supported {
        println!("  {:<16}  {}", format!("--tab {}", key), label);
    }
    println!("\n用法示例：");
    println!("  maclean scan --tab dev-cache     # 扫描开发者缓存");
    println!("  maclean scan --deep              # 深度扫描所有类别");
    println!("  maclean clean --tab dev-cache --safe-only --dry-run  # 预览安全清理");
    println!("  maclean check-disk               # 检查磁盘空间");
    EXIT_OK
}

// =========================================================================
//  M-3 · 应用清单 / 一键卸载
// =========================================================================

/// `maclean apps`：列出已安装应用（含 PWA 标记与保护状态）
fn cmd_apps(format: OutputFormat) -> u8 {
    let apps = maclean_core::scanner::uninstall::list_installed_apps();

    if format != OutputFormat::Human {
        emit_json(&apps, format, true);
        return EXIT_OK;
    }

    if apps.is_empty() {
        println!("未发现已安装应用");
        return EXIT_OK;
    }

    println!();
    println!("{:<26} {:>9}  {:<9}  路径", "应用", "本体大小", "类型");
    for a in &apps {
        let kind = if a.is_pwa {
            format!("PWA({})", a.pwa_kind)
        } else if !a.deletable {
            "受保护".to_string()
        } else {
            "App".to_string()
        };
        let name: String = a.name.chars().take(24).collect();
        println!(
            "{:<26} {:>9}  {:<9}  {}",
            name,
            format_size(a.app_size),
            kind,
            a.path
        );
    }
    println!(
        "\n提示：`maclean uninstall <路径或名称> --yes` 一键卸载（本体+数据+缓存，移入废纸篓）"
    );
    EXIT_OK
}

/// 解析 `uninstall` 参数：接受完整 .app 路径，或按应用名在
/// /Applications 与 ~/Applications（含 PWA 目录）下解析。
fn resolve_app_arg(arg: &str) -> Result<String, String> {
    let p = std::path::Path::new(arg);
    if p.extension().and_then(|e| e.to_str()) == Some("app") && p.is_dir() {
        return Ok(arg.to_string());
    }

    let apps = maclean_core::scanner::uninstall::list_installed_apps();
    let lower = arg.to_lowercase();
    let mut hits: Vec<String> = apps
        .iter()
        .filter(|a| a.name.to_lowercase().contains(&lower))
        .map(|a| a.path.clone())
        .collect();
    match hits.len() {
        0 => Err(format!("找不到应用：{}（可用 `maclean apps` 查看）", arg)),
        1 => Ok(hits.remove(0)),
        _ => Err(format!(
            "应用名 {} 匹配到多个应用：{}\n请传入完整 .app 路径",
            arg,
            hits.join(", ")
        )),
    }
}

/// `maclean uninstall <app> [--yes]`：一键卸载
///
/// 安全口径与 GUI 完全一致（core ops::uninstall_app）：路径白名单、
/// 保护级别拦截、官方卸载器优先（跟随设置）、删除逐成员过闸门并移入废纸篓。
/// 默认只预览；非交互环境不带 --yes 直接拒绝（退出码 3，E-01）。
fn cmd_uninstall(app_arg: &str, yes: bool, format: OutputFormat) -> u8 {
    let path = match resolve_app_arg(app_arg) {
        Ok(p) => p,
        Err(e) => {
            // E-02：解析失败统一错误信封
            return emit_error(format, EXIT_FAILURE, e);
        }
    };

    // 展示用元数据（只读）
    let info = maclean_core::scanner::uninstall::list_installed_apps()
        .into_iter()
        .find(|a| a.path == path);
    let name = info.as_ref().map(|a| a.name.as_str()).unwrap_or(&path);
    let release = info
        .as_ref()
        .map(|a| a.app_size + a.data_size + a.cache_size)
        .unwrap_or(0);
    let is_pwa = info.as_ref().map(|a| a.is_pwa).unwrap_or(false);

    if !yes {
        if !std::io::stdin().is_terminal() {
            // E-02：需确认走错误信封（EXIT_CONFIRM_REQUIRED=3）
            return emit_error(
                format,
                EXIT_CONFIRM_REQUIRED,
                "非交互环境执行卸载必须显式携带 --yes（仅预览请直接查看输出）",
            );
        }
        let kind = if is_pwa { "PWA" } else { "应用" };
        println!(
            "[预览] 将卸载 {kind}「{}」（{}），释放 {}，移入废纸篓可还原",
            name,
            path,
            format_size(release)
        );
        println!("确认请加 --yes 执行");
        return EXIT_OK;
    }

    let prefer_official = maclean_core::config::load_config().settings_prefer_official_uninstaller;
    let rep = maclean_core::ops::uninstall_app(&path, false, prefer_official);

    // 先算退出码，再让信封 status 与之一致（E-02/E-08：不出现 status:"ok"
    // 配失败退出码，也不出现 status:"ok" 配 EXIT_WARNINGS）。
    let code = match rep.status.as_str() {
        "done" => {
            if rep.intercepted > 0 {
                EXIT_WARNINGS
            } else {
                EXIT_OK
            }
        }
        "delegated" => EXIT_OK,
        _ => EXIT_FAILURE,
    };

    if format == OutputFormat::Human {
        if rep.status == "blocked" {
            eprintln!("{}", rep.message);
        } else {
            println!("{}", rep.message);
            if rep.restorable > 0 {
                println!("可还原 {} 项（`maclean backups` 查看清单）", rep.restorable);
            }
        }
    } else if code == EXIT_FAILURE {
        emit_error(format, EXIT_FAILURE, rep.message.clone());
    } else {
        let status = if code == EXIT_WARNINGS {
            Status::Warnings
        } else {
            Status::Ok
        };
        emit_json_status(&rep, format, true, status);
    }
    code
}

// =========================================================================
//  C-3 · 定时清理
// =========================================================================

#[derive(serde::Serialize)]
struct JsonSchedule<'a> {
    command: &'a str,
    enabled: bool,
    /// 系统里是否真的注册了定时任务（`enabled` 只代表配置开关）
    registered: bool,
    interval_days: u32,
    last_run: u64,
    due: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<String>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 查看 / 注册 / 注销定时清理任务
fn cmd_schedule(install: bool, remove: bool, days: Option<u32>, format: OutputFormat) -> u8 {
    let mut cfg = maclean_core::config::load_config();
    let mut result: Option<String> = None;

    if let Some(d) = days {
        cfg.schedule_interval_days = maclean_core::scheduler::normalize_interval_days(d);
        maclean_core::config::save_config(&cfg);
        // E-13：单独 --days 只改配置、不注册系统任务，不能静默 exit 0
        // 让人以为"已注册"。显式给出提示（--install 组合才真正注册）。
        if !install {
            result = Some(format!(
                "已把间隔设为每 {} 天；未加 --install，系统任务未注册/未变更",
                cfg.schedule_interval_days
            ));
        }
    }

    if install {
        cfg.schedule_enabled = true;
        maclean_core::config::save_config(&cfg);
        result = Some(
            match maclean_core::scheduler::install(cfg.schedule_interval_days) {
                // 命令成功 ≠ 任务真的会跑（可能被系统策略挡下），如实回传原文
                Ok(_) => "定时任务已注册".to_string(),
                Err(e) => format!("注册失败：{}", e),
            },
        );
    } else if remove {
        cfg.schedule_enabled = false;
        maclean_core::config::save_config(&cfg);
        // macOS 上要同时 bootout 当前标签与旧标签，并删除 plist 文件；
        // 只跑一条 launchctl 命令会留下升级前的旧任务。
        result = Some(match maclean_core::scheduler::uninstall() {
            Ok(_) => "定时任务已移除".to_string(),
            Err(e) => format!("移除失败：{}", e),
        });
    }

    let registered = maclean_core::scheduler::is_registered();

    if format != OutputFormat::Human {
        emit_json(
            &JsonSchedule {
                command: "schedule",
                enabled: cfg.schedule_enabled,
                registered,
                interval_days: cfg.schedule_interval_days,
                last_run: cfg.schedule_last_run,
                due: maclean_core::scheduler::is_due(
                    cfg.schedule_enabled,
                    cfg.schedule_interval_days,
                    cfg.schedule_last_run,
                    now_secs(),
                ),
                result,
            },
            format,
            true,
        );
        return EXIT_OK;
    }

    println!("\n定时清理：\n");
    println!(
        "  状态：{}",
        match (cfg.schedule_enabled, registered) {
            (true, true) => "已开启（系统任务已注册）",
            // 只写 config 不算数：任务可能被 bootout 或 plist 被删，
            // 这时"已开启"会让用户误以为会自动清理。
            (true, false) => "已开启（配置），但系统里没有注册任务，不会自动执行",
            (false, _) => "未开启",
        }
    );
    if cfg.schedule_enabled && !registered {
        println!(
            "  提示：执行 `maclean schedule --install --days {}` 重新注册",
            cfg.schedule_interval_days
        );
    }
    println!("  间隔：每 {} 天", cfg.schedule_interval_days);
    println!(
        "  上次执行：{}",
        if cfg.schedule_last_run == 0 {
            "尚未执行过".to_string()
        } else {
            format_timestamp(cfg.schedule_last_run)
        }
    );
    if let Some(r) = result {
        println!("  {}", r);
    }
    println!("\n用法：");
    println!("  maclean schedule --install --days 7   # 注册每 7 天执行一次的任务");
    println!("  maclean schedule --remove             # 注销任务");
    EXIT_OK
}

// =========================================================================
//  M-2 · 删除清单与还原
// =========================================================================

#[derive(serde::Serialize)]
struct JsonBackupEntry<'a> {
    path: &'a str,
    size_bytes: u64,
    category: &'a str,
    restorable: bool,
}

#[derive(serde::Serialize)]
struct JsonBackup<'a> {
    id: &'a str,
    created_at: u64,
    platform: &'a str,
    total: usize,
    restorable: usize,
    total_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    entries: Option<Vec<JsonBackupEntry<'a>>>,
}

#[derive(serde::Serialize)]
struct JsonBackups<'a> {
    command: &'a str,
    manifests: Vec<JsonBackup<'a>>,
}

/// 列出历史删除清单
///
/// 顺带清掉超过保留期的旧清单：路径会复用，一个月前的"还原"很可能
/// 把旧文件搬到一个早就被新内容占掉的路径上，留着反而制造混乱。
fn cmd_backups(restorable_only: bool, format: OutputFormat) -> u8 {
    let pruned = maclean_core::backup::prune_old();
    let mut manifests = maclean_core::backup::list();
    if restorable_only {
        manifests.retain(|m| m.restorable_count() > 0);
    }

    if format != OutputFormat::Human {
        emit_json(
            &JsonBackups {
                command: "backups",
                manifests: manifests
                    .iter()
                    .map(|m| JsonBackup {
                        id: &m.id,
                        created_at: m.created_at,
                        platform: &m.platform,
                        total: m.entries.len(),
                        restorable: m.restorable_count(),
                        total_bytes: m.total_bytes(),
                        entries: None,
                    })
                    .collect(),
            },
            format,
            true,
        );
        return EXIT_OK;
    }

    println!("\n历史删除清单：\n");
    if manifests.is_empty() {
        println!("  （暂无）");
    } else {
        println!(
            "  {:<16} {:<20} {:>6} {:>8} {:>10}",
            "清单 ID", "时间", "项数", "可还原", "大小"
        );
        for m in &manifests {
            let ts = format_timestamp(m.created_at);
            println!(
                "  {:<16} {:<20} {:>6} {:>8} {:>10}",
                m.id,
                ts,
                m.entries.len(),
                m.restorable_count(),
                maclean_core::scanner::format_size(m.total_bytes()),
            );
        }
    }
    if pruned > 0 {
        println!("\n  已清理 {} 份超过保留期的旧清单", pruned);
    }
    println!("\n用法：");
    println!("  maclean restore <清单 ID>    # 还原该清单中仍可还原的项");
    println!("  maclean backups --restorable-only   # 只看还有救的清单");
    EXIT_OK
}

/// 把 Unix 秒渲染成固定宽度的时间串
///
/// 刻意不引入 chrono：这里只需要一个人类可读的时间戳，为此拖进一个
/// 日期库（连带时区表）不值得。格式固定为 UTC，避免不同机器显示不一致。
pub fn format_timestamp(secs: u64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    // 从 Unix 纪元推算年月日（平闰年累加，无时区概念）
    let mut y = 1970i64;
    let mut d = days as i64;
    loop {
        let len = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
            366
        } else {
            365
        };
        if d < len {
            break;
        }
        d -= len;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_len = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1usize;
    for len in month_len {
        if d < len {
            break;
        }
        d -= len;
        mo += 1;
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d + 1, h, m, s)
}

#[derive(serde::Serialize)]
struct JsonRestore<'a> {
    command: &'a str,
    manifest_id: &'a str,
    restored: &'a [String],
    not_restorable: &'a [String],
    failed: &'a [String],
}

/// 按清单还原
fn cmd_restore(id: &str, format: OutputFormat) -> u8 {
    let Some(manifest) = maclean_core::backup::load(id) else {
        // E-02：找不到清单统一错误信封，不再用 emit_json 造 status:"ok" 配 EXIT_FAILURE
        return emit_error(
            format,
            EXIT_FAILURE,
            format!("找不到清单 {}，用 `maclean backups` 查看可用清单", id),
        );
    };

    let report = maclean_core::backup::restore(id, &maclean_core::backup::trash_dir());
    // E-03：两种格式共用同一段退出码计算（旧实现机器分支提前 return EXIT_OK
    // 跳过 failed 判定，JSON 模式部分失败恒 exit 0，谎报成功）。
    let code = if !report.failed.is_empty() {
        EXIT_WARNINGS
    } else {
        EXIT_OK
    };
    let status = if code == EXIT_WARNINGS {
        Status::Warnings
    } else {
        Status::Ok
    };

    if format == OutputFormat::Human {
        println!("\n还原清单 {}（共 {} 项）：\n", id, manifest.entries.len());
        for p in &report.restored {
            println!("  ✓ 已还原  {}", p);
        }
        // 永久删除的项明确列出：这是事实陈述，不是失败
        for p in &report.not_restorable {
            println!("  ⦸ 不可还原（当时为永久删除）  {}", p);
        }
        for p in &report.failed {
            println!("  ✗ 还原失败  {}", p);
        }
        if report.restored.is_empty() {
            println!("\n  本次没有可还原的项。永久删除的数据无法通过清单找回。");
        }
    } else {
        emit_json_status(
            &JsonRestore {
                command: "restore",
                manifest_id: id,
                restored: &report.restored,
                not_restorable: &report.not_restorable,
                failed: &report.failed,
            },
            format,
            true,
            status,
        );
    }
    code
}

// =========================================================================
//  磁盘信息获取（跨平台，委托给 platform 模块）
// =========================================================================

fn get_disk_info() -> (u64, u64) {
    maclean_core::platform::disk_info()
}

// =========================================================================
//  日志命令
// =========================================================================

/// E-04：机器格式的 log 负载（旧实现全程 println!，--json 下 jq 必失败）
#[derive(serde::Serialize)]
struct JsonLog {
    command: &'static str,
    log_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_log: Option<String>,
    size_bytes: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tail: Vec<String>,
    opened: bool,
}

fn cmd_log(tail: Option<usize>, open: bool, format: OutputFormat) -> u8 {
    let log_dir = maclean_core::logger::log_dir();
    let machine = format != OutputFormat::Human;

    if open {
        // 在文件管理器中打开日志目录
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&log_dir).spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer").arg(&log_dir).spawn();
        }
        if machine {
            emit_json(
                &JsonLog {
                    command: "log",
                    log_dir: log_dir.display().to_string(),
                    latest_log: None,
                    size_bytes: 0,
                    tail: Vec::new(),
                    opened: true,
                },
                format,
                true,
            );
        } else {
            println!("已在文件管理器中打开: {}", log_dir.display());
        }
        return EXIT_OK;
    }

    let latest = maclean_core::logger::latest_log_file();
    let latest_str = latest.as_ref().map(|p| p.display().to_string());
    let size_bytes = latest
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);

    if let Some(n) = tail {
        let mut lines: Vec<String> = Vec::new();
        let mut read_failed = false;
        if let Some(log_file) = &latest {
            match std::fs::read_to_string(log_file) {
                Ok(content) => {
                    let all: Vec<&str> = content.lines().collect();
                    let start = all.len().saturating_sub(n);
                    lines = all[start..].iter().map(|s| s.to_string()).collect();
                }
                Err(_) => read_failed = true,
            }
        }
        if machine {
            emit_json(
                &JsonLog {
                    command: "log",
                    log_dir: log_dir.display().to_string(),
                    latest_log: latest_str,
                    size_bytes,
                    tail: lines,
                    opened: false,
                },
                format,
                true,
            );
        } else if latest.is_none() {
            println!("未找到日志文件");
        } else if read_failed {
            println!("无法读取日志文件: {}", latest.as_ref().unwrap().display());
        } else {
            for line in &lines {
                println!("{}", line);
            }
            println!("\n--- 日志文件: {} ---", latest.as_ref().unwrap().display());
        }
        return EXIT_OK;
    }

    // 显示日志路径和大小
    if machine {
        emit_json(
            &JsonLog {
                command: "log",
                log_dir: log_dir.display().to_string(),
                latest_log: latest_str,
                size_bytes,
                tail: Vec::new(),
                opened: false,
            },
            format,
            true,
        );
    } else {
        println!("日志目录: {}", log_dir.display());
        if let Some(log_file) = &latest {
            println!("最新日志: {}", log_file.display());
            println!(
                "日志大小: {}",
                maclean_core::scanner::format_size(size_bytes)
            );
        } else {
            println!("暂无日志文件");
        }
        println!("\n用法:");
        println!("  maclean log --tail 50    # 查看最近 50 行日志");
        println!("  maclean log --open       # 在文件管理器中打开日志目录");
    }
    EXIT_OK
}

// =========================================================================
//  Startup · macOS 启动项管理（P3 配套）
// =========================================================================

#[cfg(target_os = "macos")]
fn default_startup_backup_root() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".maclean/disabled_launchd")
}

// 三个 JSON 结构只被 macOS 版 cmd_startup 构造；刻意不加 cfg，
// 让它们在 Windows 上继续参与类型检查。
#[derive(serde::Serialize)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct JsonStartupItem {
    label: String,
    plist: String,
    scope: String,
    enabled: bool,
}

#[derive(serde::Serialize)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct JsonStartupList {
    command: &'static str,
    count: usize,
    enabled: usize,
    items: Vec<JsonStartupItem>,
}

#[derive(serde::Serialize)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct JsonStartupResult {
    command: &'static str,
    action: &'static str,
    label: String,
    plist: String,
    ok: bool,
    error: Option<String>,
}

fn cmd_dup_ignore(action: DupIgnoreAction, format: OutputFormat) -> u8 {
    let mut cfg = maclean_core::config::load_config();
    match action {
        DupIgnoreAction::Add { pattern } => {
            let pattern = pattern.trim().to_string();
            if pattern.is_empty() {
                // E-02：统一错误信封 + 语义退出码（不再返回裸字面量 2）
                return emit_error(format, EXIT_FAILURE, "⚠ 忽略模式不能为空");
            }
            if cfg.dup_ignore_patterns.contains(&pattern) {
                if format != OutputFormat::Human {
                    emit_json(
                        &serde_json::json!({
                            "command": "dup-ignore",
                            "action": "add",
                            "pattern": pattern,
                            "added": false,
                            "already": true,
                        }),
                        format,
                        false,
                    );
                } else {
                    println!("已存在（未改动）: {}", pattern);
                }
                return 0;
            }
            cfg.dup_ignore_patterns.push(pattern.clone());
            maclean_core::config::save_config(&cfg);
            if format != OutputFormat::Human {
                emit_json(
                    &serde_json::json!({
                        "command": "dup-ignore",
                        "action": "add",
                        "pattern": pattern,
                        "added": true,
                    }),
                    format,
                    false,
                );
            } else {
                println!("已添加忽略模式: {}", pattern);
                println!("（重复文件扫描命中该子串的路径将被跳过）");
            }
        }
        DupIgnoreAction::Remove { pattern } => {
            let before = cfg.dup_ignore_patterns.len();
            cfg.dup_ignore_patterns.retain(|p| p != &pattern);
            let removed = cfg.dup_ignore_patterns.len() != before;
            maclean_core::config::save_config(&cfg);
            if format != OutputFormat::Human {
                emit_json(
                    &serde_json::json!({
                        "command": "dup-ignore",
                        "action": "remove",
                        "pattern": pattern,
                        "removed": removed,
                    }),
                    format,
                    false,
                );
            } else if removed {
                println!("已移除忽略模式: {}", pattern);
            } else {
                println!("未找到该模式（无需移除）: {}", pattern);
            }
        }
        DupIgnoreAction::List => {
            if format != OutputFormat::Human {
                emit_json(
                    &serde_json::json!({
                        "command": "dup-ignore",
                        "action": "list",
                        "count": cfg.dup_ignore_patterns.len(),
                        "patterns": cfg.dup_ignore_patterns,
                    }),
                    format,
                    false,
                );
            } else if cfg.dup_ignore_patterns.is_empty() {
                println!("（当前无忽略模式 —— 重复文件扫描只受内置硬排除保护）");
            } else {
                for p in &cfg.dup_ignore_patterns {
                    println!("{}", p);
                }
            }
        }
    }
    EXIT_OK
}

#[cfg(target_os = "macos")]
#[derive(serde::Serialize)]
struct JsonLoginItems {
    command: &'static str,
    count: usize,
    items: Vec<String>,
}

/// 读取系统设置里的登录项（Login Items）
///
/// 这些项由 backgroundtaskmanagementagent 管理，不是 LaunchAgents plist，
/// 只能通过 System Events 查询。首次调用会触发"自动化"授权（TCC）。
#[cfg(target_os = "macos")]
fn read_login_items() -> Result<Vec<String>, String> {
    let output = std::process::Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to get the name of every login item"#,
        ])
        .output()
        .map_err(|e| format!("无法调用 osascript: {}", e))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "读取登录项失败（需在系统设置 > 隐私与安全性 > 自动化 中允许控制 System Events）: {}",
            err
        ));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    Ok(raw
        .trim()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// 打开系统设置 → 通用 → 登录项与扩展
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn open_login_items_settings() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.LoginItems-Settings.extension")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
fn cmd_startup(action: StartupAction, format: OutputFormat, color: bool) -> u8 {
    let backup_root = default_startup_backup_root();
    match action {
        StartupAction::List => {
            let items = scan_startup_items();
            let enabled = items.iter().filter(|i| i.enabled).count();
            if format != OutputFormat::Human {
                emit_json(
                    &JsonStartupList {
                        command: "startup",
                        count: items.len(),
                        enabled,
                        items: items
                            .iter()
                            .map(|i| JsonStartupItem {
                                label: i.label.clone(),
                                plist: i.plist.display().to_string(),
                                scope: i.scope.clone(),
                                enabled: i.enabled,
                            })
                            .collect(),
                    },
                    format,
                    true,
                );
                return EXIT_OK;
            }
            println!("\n启动项（{} 项，已加载 {}）：\n", items.len(), enabled);
            for it in &items {
                let mark = if it.enabled {
                    paint(color, "32", "已加载")
                } else {
                    paint(color, "90", "已禁用")
                };
                println!(
                    "  {:<6} {:<40} [{}] {}",
                    mark,
                    it.label,
                    it.scope,
                    it.plist.display()
                );
            }
            EXIT_OK
        }
        StartupAction::Disable { label } => {
            let items = scan_startup_items();
            let Some(item) = items.into_iter().find(|i| i.label == label) else {
                // E-02：错误路径统一错误信封，不再 emit_json 造 status:"ok"
                return emit_error(
                    format,
                    EXIT_FAILURE,
                    format!("未找到启动项: {label}（用 `maclean startup list` 查看可用 label）"),
                );
            };
            match disable_startup_item(&item, &backup_root) {
                Ok(target) => {
                    if format != OutputFormat::Human {
                        emit_json(
                            &JsonStartupResult {
                                command: "startup",
                                action: "disable",
                                label,
                                plist: target.clone(),
                                ok: true,
                                error: None,
                            },
                            format,
                            true,
                        );
                    } else {
                        println!("✅ 已禁用 {} → {}", label, target);
                    }
                    EXIT_OK
                }
                Err(e) => {
                    // E-02：禁用失败统一错误信封
                    emit_error(format, EXIT_FAILURE, format!("禁用失败: {e}"))
                }
            }
        }
        StartupAction::Enable { label } => {
            // 从备份目录中按 Label 找（新结构 <scope>/<dir>/<file>，旧结构 <scope>/<file>）
            let home = std::env::var("HOME").unwrap_or_default();
            let mut found: Option<StartupItem> = None;
            if backup_root.exists() {
                for entry in walkdir::WalkDir::new(&backup_root)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let path = entry.path();
                    if path.extension().map(|e| e == "plist").unwrap_or(false)
                        && maclean_core::scanner::startup::read_label(path).as_deref()
                            == Some(label.as_str())
                    {
                        match restore_origin_from_backup(path, &backup_root, &home) {
                            Ok(item) => {
                                found = Some(item);
                                break;
                            }
                            Err(e) => {
                                if format == OutputFormat::Human {
                                    eprintln!("跳过 {}: {e}", path.display());
                                }
                            }
                        }
                    }
                }
            }
            let Some(item) = found else {
                // E-02：备份中未找到统一错误信封
                return emit_error(
                    format,
                    EXIT_FAILURE,
                    format!(
                        "备份中未找到启动项: {label}（已禁用的项在 ~/.maclean/disabled_launchd）"
                    ),
                );
            };
            match enable_startup_item(&item, &backup_root) {
                Ok(restored) => {
                    if format != OutputFormat::Human {
                        emit_json(
                            &JsonStartupResult {
                                command: "startup",
                                action: "enable",
                                label,
                                plist: restored.clone(),
                                ok: true,
                                error: None,
                            },
                            format,
                            true,
                        );
                    } else {
                        println!("✅ 已启用 {} ← {}", label, restored);
                    }
                    EXIT_OK
                }
                Err(e) => {
                    // E-02：启用失败统一错误信封
                    emit_error(format, EXIT_FAILURE, format!("启用失败: {e}"))
                }
            }
        }
        StartupAction::LoginItems { open } => {
            #[cfg(target_os = "macos")]
            {
                // E-02：先定最终成败再选信封，杜绝"status:ok 配失败退出码"
                // 或重复发两条错误信封（旧实现 open 失败会覆盖 code）。
                let read = read_login_items();
                let open_ok = !open || open_login_items_settings();
                match (read, open_ok) {
                    (Err(e), _) => emit_error(format, EXIT_FAILURE, e),
                    (Ok(_), false) => emit_error(
                        format,
                        EXIT_FAILURE,
                        "无法打开系统设置，请手动前往：系统设置 > 通用 > 登录项与扩展",
                    ),
                    (Ok(items), true) => {
                        if format != OutputFormat::Human {
                            emit_json(
                                &JsonLoginItems {
                                    command: "startup",
                                    count: items.len(),
                                    items: items.clone(),
                                },
                                format,
                                true,
                            );
                        } else {
                            println!("\n登录项（{} 项）：", items.len());
                            if items.is_empty() {
                                println!("  （无）");
                            } else {
                                for name in &items {
                                    println!("  - {}", name);
                                }
                            }
                            println!(
                                "\n禁用/删除需在系统设置中操作：maclean startup login-items --open\n\
                                 （这些项不是 LaunchAgents plist，不能像 startup disable 那样搬走）"
                            );
                        }
                        EXIT_OK
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = open;
                emit_error(format, EXIT_FAILURE, "登录项管理仅支持 macOS")
            }
        }
    }
}

// =========================================================================
//  Optimize · 系统优化/维护任务（P1 配套）
// =========================================================================

/// 启动项（launchd plist）是 macOS 专属能力。CLI 表面跨平台编译，
/// 其它平台明确拒绝而不是静默空转，避免脚本误以为"已处理"。
#[cfg(not(target_os = "macos"))]
fn cmd_startup(action: StartupAction, format: OutputFormat, color: bool) -> u8 {
    let _ = (action, color);
    // E-02：非 macOS 明确拒绝，机器格式输出错误信封
    emit_error(format, EXIT_FAILURE, "startup 管理仅 macOS 支持")
}

#[derive(serde::Serialize)]
struct JsonOptimizeTask {
    id: String,
    description: String,
    recommend: Recommend,
    /// 非 Safe 任务执行需要 --yes
    needs_confirmation: bool,
}

#[derive(serde::Serialize)]
struct JsonOptimizeList {
    command: &'static str,
    tasks: Vec<JsonOptimizeTask>,
}

#[derive(serde::Serialize)]
struct JsonOptimizeRun {
    command: &'static str,
    task: String,
    ok: bool,
    output: String,
    error: Option<String>,
}

fn platform_tasks() -> Vec<ScanItem> {
    // 任务名约定：macOS 用短横线命名（dns_cache_flush 等），
    // Windows 统一 win_ 前缀（win_disable_telemetry 等）。
    OptimizeScanner::new()
        .scan()
        .items
        .into_iter()
        .filter(|i| {
            if cfg!(target_os = "windows") {
                i.path.starts_with("win_")
            } else {
                !i.path.starts_with("win_")
            }
        })
        .collect()
}

fn cmd_optimize(action: OptimizeAction, format: OutputFormat, color: bool) -> u8 {
    match action {
        OptimizeAction::ListTasks => {
            let tasks = platform_tasks();
            if format != OutputFormat::Human {
                emit_json(
                    &JsonOptimizeList {
                        command: "optimize",
                        tasks: tasks
                            .iter()
                            .map(|t| JsonOptimizeTask {
                                id: t.path.clone(),
                                description: t.description.clone(),
                                recommend: t.recommend,
                                needs_confirmation: !matches!(t.recommend, Recommend::Safe),
                            })
                            .collect(),
                    },
                    format,
                    true,
                );
                return EXIT_OK;
            }
            println!("\n系统优化/维护任务（{} 项）：\n", tasks.len());
            for t in &tasks {
                let risk = match t.recommend {
                    Recommend::Safe => paint(color, "32", "低风险"),
                    Recommend::CacheOnly => paint(color, "36", "缓存"),
                    Recommend::Caution => paint(color, "33", "需确认"),
                    Recommend::Advanced => paint(color, "31", "高级"),
                };
                println!("  {:<24} {:<10} {}", t.path, risk, t.description);
            }
            println!(
                "\n执行示例：maclean optimize run --task dns_cache_flush --yes（非低风险任务需 --yes）"
            );
            EXIT_OK
        }
        OptimizeAction::Run { task, yes } => {
            let tasks = platform_tasks();
            let Some(item) = tasks.into_iter().find(|i| i.path == task) else {
                // E-02：未找到任务统一错误信封，不再 emit_json 造 status:"ok"
                return emit_error(
                    format,
                    EXIT_FAILURE,
                    format!("未找到任务: {task}（用 `maclean optimize list-tasks` 查看）"),
                );
            };
            // 非 Safe 任务需要显式 --yes（与 GUI 高危险项确认弹窗同口径）
            if !matches!(item.recommend, Recommend::Safe) && !yes {
                // E-02：需确认走错误信封（EXIT_CONFIRM_REQUIRED=3）
                return emit_error(
                    format,
                    EXIT_CONFIRM_REQUIRED,
                    format!("任务 {} 非低风险，需 --yes 确认", item.path),
                );
            }

            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                let lang_en = maclean_core::config::load_config().lang_en;
                #[cfg(target_os = "macos")]
                let output = maclean_core::ops::execute_macos_optimize_task(&item.path, lang_en);
                #[cfg(target_os = "windows")]
                let output = maclean_core::ops::execute_windows_optimize_task(&item.path, lang_en);

                if format != OutputFormat::Human {
                    emit_json(
                        &JsonOptimizeRun {
                            command: "optimize",
                            task,
                            ok: true,
                            output: output.clone(),
                            error: None,
                        },
                        format,
                        true,
                    );
                } else {
                    println!("{}", output);
                }
                EXIT_OK
            }
            // E-06：非 mac/win 平台不得"假绿"（旧实现 ok:true + EXIT_OK）。
            // 统一错误信封 + EXIT_FAILURE，真实反映"未执行"。
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                let _ = (item, yes, task);
                emit_error(format, EXIT_FAILURE, "当前平台不支持优化任务")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn scan_item(path: &str, size: u64) -> ScanItem {
        ScanItem {
            batch_mtimes: vec![],
            path: path.to_string(),
            size_bytes: size,
            category: "文件".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Advanced,
            description: String::new(),
        }
    }

    #[test]
    fn clean_drops_parent_dir_when_children_are_selected() {
        // 大文件视图同时列出目录和目录内文件：清理只删子项，父目录留空，
        // 避免成功数虚高、体积重复计算，以及还原时父目录后到导致子项失败。
        let dir = scan_item("/tmp/big", 300);
        let a = scan_item("/tmp/big/a.bin", 150);
        let b = scan_item("/tmp/big/b.bin", 120);
        let kept = drop_covered_items(vec![&dir, &a, &b]);
        let paths: Vec<&str> = kept.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, vec!["/tmp/big/a.bin", "/tmp/big/b.bin"]);
    }

    // -----------------------------------------------------------------
    //  C-4 · 结构化输出（--json）
    // -----------------------------------------------------------------

    #[test]
    fn json_output_has_a_stable_shape() {
        // schema 是对外契约：脚本照字段名取值，改名等于破坏集成。
        let scan = JsonScan {
            command: "scan",
            tabs: vec![JsonTab {
                key: "dev-cache".to_string(),
                label: "开发者缓存".to_string(),
                count: 1,
                total_size: 10,
                safe_count: 1,
                safe_size: 10,
                elapsed_ms: 3,
                items: vec![JsonItem {
                    path: "/tmp/a".to_string(),
                    size_bytes: 10,
                    category: "Rust编译".to_string(),
                    description: String::new(),
                    deletable: true,
                    undeletable_reason: String::new(),
                    recommend: Recommend::Safe,
                }],
            }],
            total_count: 1,
            total_size: 10,
            reclaimable: 10, // E-07：契约示例 jq '.data.reclaimable' 依赖此字段
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&scan).expect("scan 序列化失败"))
                .expect("scan JSON 非法");
        assert_eq!(v["reclaimable"], 10, "scan 缺 reclaimable 字段");
        let tab = &v["tabs"][0];
        for field in [
            "key",
            "label",
            "count",
            "total_size",
            "safe_count",
            "safe_size",
            "elapsed_ms",
        ] {
            assert!(!tab[field].is_null(), "tabs[] 缺字段 {}", field);
        }
        let item = &tab["items"][0];
        for field in [
            "path",
            "size_bytes",
            "category",
            "description",
            "deletable",
            "undeletable_reason",
            "recommend",
        ] {
            assert!(!item[field].is_null(), "items[] 缺字段 {}", field);
        }
    }

    #[test]
    fn clean_json_reports_rejections_and_failures() {
        // 被安全闸门拦下的项必须出现在 JSON 里，不能只打在人类可读输出里 ——
        // 脚本据此判断"为什么没删掉"。
        let c = JsonClean {
            command: "clean",
            preview: true,
            dry_run: true,
            yes: false,
            scheduled: false,
            privileged: false,
            tabs: vec![JsonCleanTab {
                key: "dev-cache".to_string(),
                label: "开发者缓存".to_string(),
                planned: vec![JsonPath {
                    path: "/tmp/a".to_string(),
                    size_bytes: 1,
                }],
                deleted: Vec::new(),
                failed: Vec::new(),
                rejected: vec![JsonFailure {
                    path: "/tmp/b".to_string(),
                    reason: "符号链接".to_string(),
                }],
                skipped_snapshots: 0,
                preview: true,
                backup_id: None,
            }],
            success: 0,
            failed: 0,
            rejected: 1,
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(v["rejected"], 1);
        assert_eq!(v["tabs"][0]["rejected"][0]["path"], "/tmp/b");
        assert_eq!(v["dry_run"], true);
    }

    #[test]
    fn every_command_accepts_the_json_flag() {
        // --json 是 global arg：任何子命令上都能用，包括放在子命令之后。
        // clap 的 global 容易漏，漏了就只有部分命令能脚本化。
        use clap::CommandFactory;
        let cmd = Cli::command();
        for sub in cmd.get_subcommands() {
            let name = sub.get_name().to_string();
            // `restore` 有必填位置参数，只给 --json 会因"缺参数"失败 ——
            // 那是参数没给全，不是 global 失效。补占位值再断言，
            // 否则这条用例会把新命令误判成回归。
            let mut argv: Vec<String> = vec!["maclean".to_string(), name.clone()];
            // 嵌套子命令（startup list / optimize list-tasks）：补第一个子命令名，
            // 否则父命令因缺 action 报错，会误判成 global 失效。
            if let Some(inner) = sub.get_subcommands().next() {
                argv.push(inner.get_name().to_string());
            }
            for _ in sub.get_arguments().filter(|a| a.is_required_set()) {
                argv.push("x".to_string());
            }
            argv.push("--json".to_string());
            let parsed = Cli::try_parse_from(argv);
            assert!(
                parsed.is_ok(),
                "子命令 {} 不接受 --json（global 没生效）",
                name
            );
        }
    }

    #[test]
    fn json_flag_is_accepted_after_the_subcommand() {
        // 用户会写 `maclean scan --json`，也会写 `maclean --json scan`；
        // global=true 保证两种都行。
        let a = Cli::try_parse_from(["maclean", "scan", "--json"]);
        assert!(a.is_ok(), "`scan --json` 解析失败");
        let b = Cli::try_parse_from(["maclean", "--json", "scan"]);
        assert!(b.is_ok(), "`--json scan` 解析失败");
    }

    #[test]
    fn text_output_still_prints_the_table() {
        // 加 JSON 不能破坏人类可读输出：--json 分支必须早于任何 println，
        // 反之文本模式也不得混入 JSON。用源码钉住这一点。
        let src = include_str!("handler.rs");
        let check = src[src.find("fn cmd_check_disk(").expect("cmd_check_disk")..]
            .split("\nfn cmd_list(")
            .next()
            .unwrap();
        let json_at = check
            .find("format != OutputFormat::Human")
            .expect("cmd_check_disk 没有机器输出分支");
        let first_print = check.find("println!(\"╔").expect("没有表格输出");
        assert!(
            json_at < first_print,
            "JSON 分支在表格输出之后，两种输出会混在一起"
        );
    }

    // -----------------------------------------------------------------
    //  M-2 · 删除清单与还原
    // -----------------------------------------------------------------

    #[test]
    fn timestamp_rendering_matches_utc_conventions() {
        // 基准值用系统 date（UTC）独立算出来的，不是拿本函数自证。
        assert_eq!(format_timestamp(0), "1970-01-01 00:00:00");
        // 闰年 2 月 29 日：手搓日历最容易错的就是这里
        assert_eq!(format_timestamp(1709164800), "2024-02-29 00:00:00");
        assert_eq!(format_timestamp(1789831800), "2026-09-19 15:30:00");
    }

    #[test]
    fn timestamp_rendering_survives_the_century_rule() {
        // 2100 能被 4 整除但**不是**闰年（百年不闰、四百年再闰）。
        // 拿 1970 + 366 天推进过去，若闰年判定漏了百年规则，这里会漂一天。
        let mut secs: u64 = 0;
        for y in 1970..2100i64 {
            let len: u64 = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                366
            } else {
                365
            };
            secs += len * 86400;
        }
        // 1970..2100 累计后落在 2100-01-01
        assert_eq!(format_timestamp(secs), "2100-01-01 00:00:00");
    }

    #[test]
    fn backup_commands_are_wired_into_the_cli() {
        // 清单模块全部函数都是 pub，但 pub 不等于可达 —— 必须能从命令行走到。
        // 拆掉 Commands::Backups / Commands::Restore 任一变体，这里立刻红。
        assert!(Cli::try_parse_from(["maclean", "backups"]).is_ok());
        assert!(Cli::try_parse_from(["maclean", "backups", "--restorable-only"]).is_ok());
        let r = Cli::try_parse_from(["maclean", "restore", "abc"]).unwrap();
        assert!(matches!(r.command, Some(Commands::Restore { id }) if id == "abc"));
        // restore 缺 id 必须报错，不能默默什么都不做
        assert!(Cli::try_parse_from(["maclean", "restore"]).is_err());
    }

    #[test]
    fn schedule_command_is_wired_with_its_flags() {
        // 定时清理没有命令行入口的话，"应用没开"这个主场景就完全覆盖不到。
        assert!(Cli::try_parse_from(["maclean", "schedule"]).is_ok());
        assert!(Cli::try_parse_from(["maclean", "schedule", "--install", "--days", "7"]).is_ok());
        assert!(Cli::try_parse_from(["maclean", "schedule", "--remove"]).is_ok());
        let c = Cli::try_parse_from(["maclean", "clean", "--scheduled"]).unwrap();
        assert!(
            matches!(
                c.command,
                Some(Commands::Clean {
                    scheduled: true,
                    ..
                })
            ),
            "clean 缺少 --scheduled：定时任务跑完无法记账，设置页永远显示未执行过"
        );
    }

    #[test]
    fn log_command_emits_envelope_in_machine_format() {
        // E-04：旧行为「log 全程 println!，--json 下 jq 必失败」已被推翻。
        // 现在 cmd_log 接收 format，机器模式走 emit_json 输出 envelope。
        // 断言按 "\nmod tests" 切分生产段，避免测试文本自己命中。
        let src = include_str!("handler.rs");
        let prod = src.split("\nmod tests").next().unwrap();
        let body = prod[prod.find("fn cmd_log").expect("cmd_log 不见了")..]
            .split("\nfn ")
            .next()
            .unwrap();
        assert!(
            body.contains("format: OutputFormat"),
            "cmd_log 未接收 format 参数"
        );
        assert!(body.contains("emit_json("), "机器模式未输出 envelope");
        // global --json / --format 仍能在 log 上解析（未破坏 args.rs 的 global）
        assert!(Cli::try_parse_from(["maclean", "log", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["maclean", "log", "--format", "jsonl"]).is_ok());
    }

    // -----------------------------------------------------------------
    //  #33 · CLI 删除路径的安全校验
    //
    //  `cmd_clean` 曾经只按 `item.deletable` 过滤就 `remove_dir_all`，
    //  全程没有任何 safety 调用，也没有软链复查 —— GUI 侧一直在
    //  `ops::sanitize_before_delete` 里做这件事，CLI 整层跳过。
    //
    //  cmd_clean 要真扫一遍磁盘才跑得起来，没法在测试里端到端执行，
    //  所以这里用源码级断言钉住接线，防止有人把校验删回去。
    // -----------------------------------------------------------------

    #[test]
    fn clean_command_runs_the_same_pre_delete_gate_as_the_gui() {
        let src = include_str!("handler.rs");
        let body = src[src.find("fn cmd_clean").expect("cmd_clean 不见了")..]
            .split("\nfn ")
            .next()
            .unwrap();

        assert!(
            body.contains("sanitize_before_delete("),
            "CLI 删除前没有复做安全校验，等于绕过保护直接删"
        );
        // 删除循环必须遍历校验放行后的列表，而不是原始扫描结果。
        // 注意不能简单断言 "不出现 to_clean" —— dry-run 分支要用它列清单，
        // 那是合法的。
        assert!(
            body.contains("for (path, _category) in &allowed"),
            "删除循环没有走校验放行后的 allowed 列表"
        );
        let deletion_loop = body[body.find("for (path, _category) in &allowed").unwrap()..]
            .split("\n        }")
            .next()
            .unwrap();
        assert!(
            !deletion_loop.contains("&to_clean"),
            "删除循环又直接用上了未校验的 to_clean"
        );
    }

    #[test]
    fn clean_command_reports_intercepted_items_instead_of_deleting_them() {
        // 被拦截的项必须显式告知用户，不能静默跳过 —— 否则用户以为删干净了
        let src = include_str!("handler.rs");
        let body = src[src.find("fn cmd_clean").expect("cmd_clean 不见了")..]
            .split("\nfn ")
            .next()
            .unwrap();
        assert!(
            body.contains("已拦截"),
            "被安全校验拦下的项没有打印出来，用户无从得知"
        );
    }
}
