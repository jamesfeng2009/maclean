//! CLI 命令行接口
//!
//! 支持 `maclean scan`、`maclean clean`、`maclean check-disk` 等子命令。
//! 无参数时返回 None，由 main 启动 GUI。

use clap::{Parser, Subcommand, ValueEnum};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "macos")]
use crate::scanner::apfs::ApfsScanner;
#[cfg(target_os = "macos")]
use crate::scanner::app_cache::AppCacheScanner;
#[cfg(target_os = "macos")]
use crate::scanner::app_data::AppDataScanner;
use crate::scanner::dev_cache::DevCacheScanner;
use crate::scanner::large_files::LargeFileScanner;
use crate::scanner::optimize::OptimizeScanner;
#[cfg(target_os = "macos")]
use crate::scanner::startup::{
    disable_startup_item, enable_startup_item, restore_origin_from_backup, scan_startup_items,
    StartupItem,
};
use crate::scanner::uninstall::UninstallScanner;
use crate::scanner::{format_size, Scanner};
use crate::scanner::{Recommend, ScanItem};

// =========================================================================
//  语义退出码（供脚本/CI 消费，稳定契约）
//
//  0 成功；1 通用失败；2 JSON 序列化失败（历史保留）；
//  4 需确认（非交互环境执行删除未带 --yes）；
//  7 用户取消（Ctrl+C）；8 带警告完成（clean 有失败/被拦截项）。
// =========================================================================
pub const EXIT_OK: u8 = 0;
pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_SERIALIZE: u8 = 2;
pub const EXIT_CONFIRM_REQUIRED: u8 = 4;
pub const EXIT_CANCELLED: u8 = 7;
pub const EXIT_WARNINGS: u8 = 8;

/// 取消标志：Ctrl+C 置位，扫描/删除循环检查后以 7 退出
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

/// 输出格式（机器可读输出永不包含 ANSI 颜色）
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
    Jsonl,
}

/// 人类可读输出的颜色控制
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

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

/// maclean — macOS 磁盘清理工具
#[derive(Parser, Debug)]
#[command(name = "maclean", version, about = "macOS 磁盘清理工具", long_about = None)]
pub struct Cli {
    /// 子命令（无子命令时启动 GUI）
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// 以 JSON 输出（供脚本/监控系统消费；等价 --format json）
    ///
    /// 结构化输出是稳定契约：字段名与语义不随文案翻译改变。
    #[arg(long, global = true)]
    pub json: bool,

    /// 输出格式：human / json / jsonl（jsonl 流式，每行一个对象）
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,

    /// 人类可读输出的颜色：auto / always / never（机器格式恒无色）
    #[arg(long, global = true, value_enum, default_value_t = ColorMode::Auto)]
    pub color: ColorMode,

    /// 关闭 stderr 进度条（脚本/日志场景）
    #[arg(long, global = true)]
    pub no_progress: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 扫描可清理项目
    Scan {
        /// 指定扫描的 Tab（dev-cache, large-files, app-cache, app-data, app-uninstall, optimize, apfs, dup-files, custom-rules）
        #[arg(long)]
        tab: Option<String>,

        /// 深度扫描（扫描所有 Tab）
        #[arg(long)]
        deep: bool,
    },

    /// 清理安全可删除的项目
    ///
    /// 默认只预览不删除；加 --yes 才实际执行。定时任务（--scheduled）
    /// 视为用户已确认（无人值守自动清理，行为不变）。
    Clean {
        /// 指定清理的 Tab
        #[arg(long)]
        tab: Option<String>,

        /// 仅清理推荐安全项（Safe）
        #[arg(long)]
        safe_only: bool,

        /// 只预览将清理的项目，不实际删除
        #[arg(long, conflicts_with = "yes")]
        dry_run: bool,

        /// 确认执行删除（非交互环境必须显式携带，否则拒绝执行）
        #[arg(long, conflicts_with = "dry_run")]
        yes: bool,

        /// 由系统定时任务调用（C-3）：执行后把 schedule_last_run 记为当前时间
        ///
        /// 定时任务（launchd / 任务计划）跑的就是这条命令。带上它才知道
        /// "上次定时清理是什么时候"，否则设置页永远显示"尚未执行过"。
        #[arg(long)]
        scheduled: bool,
    },

    /// 查看 / 注册 / 注销定时清理任务（C-3）
    Schedule {
        /// 注册系统定时任务
        #[arg(long)]
        install: bool,

        /// 注销系统定时任务
        #[arg(long)]
        remove: bool,

        /// 设置间隔（天）：1 / 7 / 30，其它值会被夹到最近的档位
        #[arg(long)]
        days: Option<u32>,
    },

    /// 检查磁盘空间使用情况；--breakdown 输出分类占比
    CheckDisk {
        /// 扫描各缓存类别并按分类输出磁盘占用占比（P2 口径）
        #[arg(long)]
        breakdown: bool,
    },

    /// 列出所有可用的扫描类别
    List,

    /// 查看日志文件路径或输出最近日志
    Log {
        /// 输出最近 N 行日志（默认显示路径）
        #[arg(long)]
        tail: Option<usize>,

        /// 在资源管理器/Finder 中打开日志目录
        #[arg(long)]
        open: bool,
    },

    /// 列出历史删除清单（M-2）
    ///
    /// 每次删除都会落一份清单，记录删了哪些路径、多大、其中多少项还救得回来。
    Backups {
        /// 只看还有可还原项的清单
        #[arg(long)]
        restorable_only: bool,
    },

    /// 按清单还原（M-2）
    ///
    /// 只还原当时走废纸篓的项。永久删除的缓存字节已经不在了，
    /// 命令会明确列出它们为"不可还原"，不会假装成功。
    Restore {
        /// 清单 id（`maclean backups` 输出的第一列）
        id: String,
    },

    /// macOS 启动项管理（LaunchAgents / LaunchDaemons）
    Startup {
        #[command(subcommand)]
        action: StartupAction,
    },

    /// 系统优化/维护任务
    Optimize {
        #[command(subcommand)]
        action: OptimizeAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum StartupAction {
    /// 列出启动项（含已加载状态）
    List,
    /// 禁用启动项（plist 移入备份目录，可逆）
    Disable {
        /// launchd Label（`maclean startup list` 第一列）
        label: String,
    },
    /// 启用（从备份目录恢复 plist）
    Enable {
        /// launchd Label
        label: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum OptimizeAction {
    /// 列出全部优化/维护任务（含风险等级）
    ListTasks,
    /// 执行指定任务（非 Safe 任务需 --yes 确认）
    Run {
        /// 任务 id（`maclean optimize list-tasks` 第一列）
        task: String,
        /// 确认执行非 Safe 任务
        #[arg(long)]
        yes: bool,
    },
}

/// 运行 CLI 命令，返回退出码（无子命令时返回 None 由 main 启动 GUI）
pub fn run_cli() -> Option<u8> {
    let cli = Cli::parse();

    match cli.command {
        None => None, // 无子命令，启动 GUI
        Some(cmd) => {
            install_cancel_handler();
            // 兼容：--json 等价 --format json
            let format = if cli.json {
                OutputFormat::Json
            } else {
                cli.format
            };
            let color_on = color_enabled(cli.color, format);
            Some(run_command(cmd, format, color_on, cli.no_progress))
        }
    }
}

fn run_command(cmd: Commands, format: OutputFormat, color: bool, no_progress: bool) -> u8 {
    let prog = progress_enabled(no_progress);
    match cmd {
        Commands::Scan { tab, deep } => cmd_scan(tab, deep, format, color, prog),
        Commands::Clean {
            tab,
            safe_only,
            dry_run,
            scheduled,
            yes,
        } => cmd_clean(tab, safe_only, dry_run, scheduled, yes, format, color, prog),
        Commands::Schedule {
            install,
            remove,
            days,
        } => cmd_schedule(install, remove, days, format),
        Commands::CheckDisk { breakdown } => cmd_check_disk(breakdown, format),
        Commands::List => cmd_list(format),
        // 日志是给人看的，不做 JSON
        Commands::Log { tail, open } => cmd_log(tail, open),
        Commands::Backups { restorable_only } => cmd_backups(restorable_only, format),
        // 还原结果涉及逐个路径的成功/失败，JSON 更有用（脚本可据此重试）
        Commands::Restore { id } => cmd_restore(&id, format),
        Commands::Startup { action } => cmd_startup(action, format, color),
        Commands::Optimize { action } => cmd_optimize(action, format, color),
    }
}

// =========================================================================
//  结构化输出（--json）
//
//  所有命令共用这一层：先算出数据，再决定渲染成表格还是 JSON。
//  绝不能"表格和 JSON 各扫一遍" —— 那两份结果可能对不上。
// =========================================================================

fn emit_json<T: serde::Serialize>(value: &T, format: OutputFormat, pretty: bool) {
    let s = if format == OutputFormat::Jsonl {
        serde_json::to_string(value)
    } else if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    };
    match s {
        Ok(s) => println!("{}", s),
        Err(e) => {
            // 序列化失败不能静默：调用方拿不到任何输出会以为"没有可清理项"
            eprintln!(
                "{{\"error\": \"json serialization failed\", \"detail\": \"{}\"}}",
                e
            );
            std::process::exit(EXIT_SERIALIZE as i32);
        }
    }
}

// =========================================================================
//  Tab 名称映射
// =========================================================================

const ALL_TABS: &[(&str, &str)] = &[
    ("dev-cache", "开发者缓存"),
    ("large-files", "大文件"),
    ("app-cache", "App缓存"),
    ("app-data", "App数据"),
    ("app-uninstall", "App卸载"),
    ("optimize", "系统优化"),
    ("apfs", "APFS快照"),
    ("dup-files", "重复文件"),
    ("custom-rules", "自定义规则"),
];

/// 该扫描类别在当前平台是否有对应扫描器
///
/// `scan_tab` 里 "apfs" 只有 `#[cfg(target_os = "macos")]` 分支，其它平台
/// 落到 `_ => Vec::new()`。`--deep` 和 `list` 若不过滤，Windows 上会打印一个
/// 恒为 0 项的「APFS快照」—— 用户以为扫过了没东西，其实是这个平台没有。
fn tab_supported(name: &str) -> bool {
    match name {
        "apfs" => cfg!(target_os = "macos"),
        // dup-files 只实现了 macOS 扫描器；custom-rules 全平台
        "dup-files" => cfg!(target_os = "macos"),
        _ => true,
    }
}

fn scan_tab(tab_name: &str) -> Vec<ScanItem> {
    match tab_name {
        "dev-cache" => DevCacheScanner::new().scan().items,
        "large-files" => LargeFileScanner::new().scan().items,
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
            crate::scanner::dup_files::DuplicateFileScanner::new()
                .scan()
                .items
        }
        "custom-rules" => crate::rules::RuleScanner::new().scan().items,
        // Windows 扫描器
        #[cfg(target_os = "windows")]
        "app-cache" => {
            crate::scanner::windows_apps::WindowsAppCacheScanner::new()
                .scan()
                .items
        }
        #[cfg(target_os = "windows")]
        "app-data" => {
            crate::scanner::windows_apps::WindowsAppDataScanner::new()
                .scan()
                .items
        }
        #[cfg(target_os = "windows")]
        "app-uninstall" => {
            crate::scanner::windows_apps::WindowsUninstallScanner::new()
                .scan()
                .items
        }
        _ => Vec::new(),
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
}

fn cmd_scan(tab: Option<String>, deep: bool, format: OutputFormat, color: bool, prog: bool) -> u8 {
    let tabs: Vec<(String, String)> = if deep {
        ALL_TABS
            .iter()
            .filter(|(k, _)| tab_supported(k))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    } else if let Some(t) = &tab {
        vec![(
            t.clone(),
            ALL_TABS
                .iter()
                .find(|(k, _)| *k == t.as_str())
                .map(|(_, v)| v.to_string())
                .unwrap_or_else(|| t.clone()),
        )]
    } else {
        vec![("dev-cache".to_string(), "开发者缓存".to_string())]
    };

    let mut total_size: u64 = 0;
    let mut total_count: usize = 0;
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
        let items = scan_tab(tab_key);
        let elapsed = start.elapsed();

        // 保存扫描结果到本地缓存，便于 GUI 启动时直接加载
        if !items.is_empty() {
            let result = crate::scanner::ScanResult {
                items: items.clone(),
                total_size: items.iter().map(|i| i.size_bytes).sum(),
                scan_time_ms: elapsed.as_millis() as u64,
            };
            crate::scanner::cache::save_cache(tab_key, &result);
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

        if format == OutputFormat::Jsonl {
            // 流式：每个 Tab 扫完立即输出一行，脚本可边收边处理
            emit_json(
                &JsonScan {
                    command: "scan",
                    tabs: vec![out_tabs.last().unwrap().clone()],
                    total_count: 0,
                    total_size: 0,
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

    if format == OutputFormat::Json {
        emit_json(
            &JsonScan {
                command: "scan",
                tabs: out_tabs,
                total_count,
                total_size,
            },
            format,
            true,
        );
    }
    EXIT_OK
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
    tabs: Vec<JsonCleanTab>,
    success: usize,
    failed: usize,
    rejected: usize,
}

#[allow(clippy::too_many_arguments)]
fn cmd_clean(
    tab: Option<String>,
    safe_only: bool,
    dry_run: bool,
    scheduled: bool,
    yes: bool,
    format: OutputFormat,
    color: bool,
    prog: bool,
) -> u8 {
    // C-3：定时任务跑完就记账，设置页才显示得出"上次执行"时间
    if scheduled {
        let mut cfg = crate::config::load_config();
        cfg.schedule_last_run = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        crate::config::save_config(&cfg);
    }

    // 安全确认（P0）：
    // - 定时任务（--scheduled）视为用户已在 GUI 配置时确认 → 无人值守自动执行
    // - --yes 显式确认执行
    // - 否则一律预览（不删除）；非交互终端（stdin 非 TTY）时拒绝执行并退出码 4
    let confirmed = yes || scheduled;
    let preview = dry_run || !confirmed;
    if !confirmed && !dry_run && !std::io::stdin().is_terminal() {
        // 非交互环境：不给预览的机会直接要求 --yes（脚本必须显式确认删除意图）
        eprintln!("非交互环境执行删除必须显式携带 --yes（仅预览请加 --dry-run）");
        return EXIT_CONFIRM_REQUIRED;
    }

    let tabs: Vec<String> = if let Some(t) = &tab {
        vec![t.clone()]
    } else {
        vec!["dev-cache".to_string()]
    };

    let mut out: Vec<JsonCleanTab> = Vec::new();
    let mut g_success = 0usize;
    let mut g_failed = 0usize;
    let mut g_rejected = 0usize;

    for tab_key in &tabs {
        if cancelled() {
            return EXIT_CANCELLED;
        }
        let tab_label = ALL_TABS
            .iter()
            .find(|(k, _)| *k == tab_key.as_str())
            .map(|(_, v)| *v)
            .unwrap_or(tab_key);

        let items = scan_tab(tab_key);

        let to_clean: Vec<&ScanItem> = if safe_only {
            items
                .iter()
                .filter(|i| i.recommend.default_selected() && i.deletable)
                .collect()
        } else {
            items.iter().filter(|i| i.deletable).collect()
        };

        if to_clean.is_empty() {
            if format == OutputFormat::Human {
                println!("  （无可清理项目）");
            }
            continue;
        }

        let clean_size: u64 = to_clean.iter().map(|i| i.size_bytes).sum();

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
                    .map(|i| JsonPath {
                        path: i.path.clone(),
                        size_bytes: i.size_bytes,
                    })
                    .collect(),
                deleted: Vec::new(),
                failed: Vec::new(),
                rejected: Vec::new(),
                skipped_snapshots: 0,
                preview: true,
            };
            if format == OutputFormat::Jsonl {
                emit_json(&rec, format, false);
            } else if format == OutputFormat::Json {
                out.push(rec);
            } else {
                for item in &to_clean {
                    println!("    - [{}] {}", format_size(item.size_bytes), item.path);
                }
            }
            continue;
        }

        // 删除前复做安全校验（与 GUI 同一层 TOCTOU 防护）
        let pairs: Vec<(String, String)> = to_clean
            .iter()
            .filter(|i| !i.path.starts_with("snapshot:"))
            .map(|i| (i.path.clone(), i.category.clone()))
            .collect();
        let skipped_snapshots = to_clean.len() - pairs.len();

        let (allowed, rejected) = crate::ops::sanitize_before_delete(pairs, false);
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
                    "  ⏭️  跳过 {} 个 APFS 快照（需特殊处理）",
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
        };

        // 实际删除（带进度与取消检查）
        let mut success = 0usize;
        let mut failed = 0usize;
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
            let result = if std::path::Path::new(path).is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };

            match result {
                Ok(_) => {
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
                    rec.failed.push(JsonFailure {
                        path: path.clone(),
                        reason: e.to_string(),
                    });
                    failed += 1;
                    if format == OutputFormat::Jsonl {
                        emit_json(
                            &JsonlCleanEvent {
                                event: "failed",
                                tab: tab_key.clone(),
                                path: path.clone(),
                                size_bytes: 0,
                                reason: e.to_string(),
                            },
                            format,
                            false,
                        );
                    } else if format == OutputFormat::Human {
                        println!("  {} {} — {}", paint(color, "31", "❌"), path, e);
                    }
                }
            }
        }
        if prog {
            eprint!("\r\x1b[K");
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
    }

    if format == OutputFormat::Json {
        emit_json(
            &JsonClean {
                command: "clean",
                preview,
                dry_run,
                yes,
                scheduled,
                tabs: out,
                success: g_success,
                failed: g_failed,
                rejected: g_rejected,
            },
            format,
            true,
        );
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
        if format != OutputFormat::Human {
            // 拿不到磁盘信息时必须是**结构化的错误**，不能静默成功：
            // 监控脚本看到 exit 0 且字段全 0 会以为"磁盘空了"
            eprintln!("{{\"command\": \"check-disk\", \"error\": \"unable to read disk info\"}}");
            return EXIT_FAILURE;
        }
        println!("❌ 无法获取磁盘信息");
        return EXIT_FAILURE;
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
        "dev-cache",
        "large-files",
        "app-cache",
        "app-data",
        "custom-rules",
        "dup-files",
    ];
    let mut categories: Vec<(String, u64)> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut total_count: usize = 0;
    let mut scanned: Vec<String> = Vec::new();

    for key in keys {
        if !tab_supported(key) {
            continue;
        }
        let items = scan_tab(key);
        scanned.push(key.to_string());
        total_count += items.len();
        for item in &items {
            total_bytes += item.size_bytes;
            if let Some(entry) = categories.iter_mut().find(|(c, _)| *c == item.category) {
                entry.1 += item.size_bytes;
            } else {
                categories.push((item.category.clone(), item.size_bytes));
            }
        }
    }
    categories.sort_by(|a, b| b.1.cmp(&a.1));
    categories.retain(|(_, s)| *s > 0);

    let entries: Vec<JsonBreakdownEntry> = categories
        .iter()
        .map(|(cat, size)| JsonBreakdownEntry {
            category: cat.clone(),
            size_bytes: *size,
            percent: if total_bytes > 0 {
                *size as f64 / total_bytes as f64 * 100.0
            } else {
                0.0
            },
            count: 0,
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
    let supported: Vec<(&str, &str)> = ALL_TABS
        .iter()
        .filter(|(k, _)| tab_supported(k))
        .map(|(k, v)| (*k, *v))
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
//  C-3 · 定时清理
// =========================================================================

#[derive(serde::Serialize)]
struct JsonSchedule<'a> {
    command: &'a str,
    enabled: bool,
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
    let mut cfg = crate::config::load_config();

    if let Some(d) = days {
        cfg.schedule_interval_days = crate::scheduler::normalize_interval_days(d);
        crate::config::save_config(&cfg);
    }

    let mut result: Option<String> = None;

    if install {
        cfg.schedule_enabled = true;
        crate::config::save_config(&cfg);
        result = Some(
            match crate::scheduler::install(cfg.schedule_interval_days) {
                // 命令成功 ≠ 任务真的会跑（可能被系统策略挡下），如实回传原文
                Ok(_) => "定时任务已注册".to_string(),
                Err(e) => format!("注册失败：{}", e),
            },
        );
    } else if remove {
        cfg.schedule_enabled = false;
        crate::config::save_config(&cfg);
        let (prog, args) = crate::scheduler::uninstall_command();
        result = Some(match std::process::Command::new(prog).args(args).output() {
            Ok(o) if o.status.success() => "定时任务已移除".to_string(),
            Ok(o) => format!("移除失败：{}", String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => format!("移除失败：{}", e),
        });
    }

    if format != OutputFormat::Human {
        emit_json(
            &JsonSchedule {
                command: "schedule",
                enabled: cfg.schedule_enabled,
                interval_days: cfg.schedule_interval_days,
                last_run: cfg.schedule_last_run,
                due: crate::scheduler::is_due(
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
        if cfg.schedule_enabled {
            "已开启"
        } else {
            "未开启"
        }
    );
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
    let pruned = crate::backup::prune_old();
    let mut manifests = crate::backup::list();
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
                crate::scanner::format_size(m.total_bytes()),
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
pub(crate) fn format_timestamp(secs: u64) -> String {
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
    let Some(manifest) = crate::backup::load(id) else {
        if format != OutputFormat::Human {
            emit_json(
                &serde_json::json!({
                    "command": "restore",
                    "manifest_id": id,
                    "error": "manifest not found",
                }),
                format,
                true,
            );
        } else {
            eprintln!("找不到清单 {}，用 `maclean backups` 查看可用清单", id);
        }
        return EXIT_FAILURE;
    };

    let report = crate::backup::restore(id, &crate::backup::trash_dir());

    if format != OutputFormat::Human {
        emit_json(
            &JsonRestore {
                command: "restore",
                manifest_id: id,
                restored: &report.restored,
                not_restorable: &report.not_restorable,
                failed: &report.failed,
            },
            format,
            true,
        );
        return EXIT_OK;
    }

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
    if !report.failed.is_empty() {
        EXIT_WARNINGS
    } else {
        EXIT_OK
    }
}

// =========================================================================
//  磁盘信息获取（跨平台，委托给 platform 模块）
// =========================================================================

fn get_disk_info() -> (u64, u64) {
    crate::platform::disk_info()
}

// =========================================================================
//  日志命令
// =========================================================================

fn cmd_log(tail: Option<usize>, open: bool) -> u8 {
    let log_dir = crate::logger::log_dir();

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
        println!("已在文件管理器中打开: {}", log_dir.display());
        return EXIT_OK;
    }

    if let Some(n) = tail {
        // 输出最近 N 行日志
        if let Some(log_file) = crate::logger::latest_log_file() {
            if let Ok(content) = std::fs::read_to_string(&log_file) {
                let lines: Vec<&str> = content.lines().collect();
                let start = if lines.len() > n { lines.len() - n } else { 0 };
                for line in &lines[start..] {
                    println!("{}", line);
                }
                println!("\n--- 日志文件: {} ---", log_file.display());
            } else {
                println!("无法读取日志文件: {}", log_file.display());
            }
        } else {
            println!("未找到日志文件");
        }
    } else {
        // 显示日志路径和大小
        println!("日志目录: {}", log_dir.display());
        if let Some(log_file) = crate::logger::latest_log_file() {
            let size = std::fs::metadata(&log_file).map(|m| m.len()).unwrap_or(0);
            println!("最新日志: {}", log_file.display());
            println!("日志大小: {}", crate::scanner::format_size(size));
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

fn default_startup_backup_root() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".maclean/disabled_launchd")
}

#[derive(serde::Serialize)]
struct JsonStartupItem {
    label: String,
    plist: String,
    scope: String,
    enabled: bool,
}

#[derive(serde::Serialize)]
struct JsonStartupList {
    command: &'static str,
    count: usize,
    enabled: usize,
    items: Vec<JsonStartupItem>,
}

#[derive(serde::Serialize)]
struct JsonStartupResult {
    command: &'static str,
    action: &'static str,
    label: String,
    plist: String,
    ok: bool,
    error: Option<String>,
}

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
                if format != OutputFormat::Human {
                    emit_json(
                        &JsonStartupResult {
                            command: "startup",
                            action: "disable",
                            label: label.clone(),
                            plist: String::new(),
                            ok: false,
                            error: Some(format!("未找到启动项: {label}")),
                        },
                        format,
                        true,
                    );
                } else {
                    eprintln!("未找到启动项: {}", label);
                    eprintln!("用 `maclean startup list` 查看可用 label");
                }
                return EXIT_FAILURE;
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
                    if format != OutputFormat::Human {
                        emit_json(
                            &JsonStartupResult {
                                command: "startup",
                                action: "disable",
                                label,
                                plist: String::new(),
                                ok: false,
                                error: Some(e.clone()),
                            },
                            format,
                            true,
                        );
                    } else {
                        eprintln!("禁用失败: {e}");
                    }
                    EXIT_FAILURE
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
                        && crate::scanner::startup::read_label(path).as_deref()
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
                if format != OutputFormat::Human {
                    emit_json(
                        &JsonStartupResult {
                            command: "startup",
                            action: "enable",
                            label: label.clone(),
                            plist: String::new(),
                            ok: false,
                            error: Some(format!("备份中未找到启动项: {label}")),
                        },
                        format,
                        true,
                    );
                } else {
                    eprintln!("备份中未找到启动项: {}", label);
                    eprintln!("用 `maclean startup list` 查看当前启动项；已禁用的项在 ~/.maclean/disabled_launchd");
                }
                return EXIT_FAILURE;
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
                    if format != OutputFormat::Human {
                        emit_json(
                            &JsonStartupResult {
                                command: "startup",
                                action: "enable",
                                label,
                                plist: String::new(),
                                ok: false,
                                error: Some(e.clone()),
                            },
                            format,
                            true,
                        );
                    } else {
                        eprintln!("启用失败: {e}");
                    }
                    EXIT_FAILURE
                }
            }
        }
    }
}

// =========================================================================
//  Optimize · 系统优化/维护任务（P1 配套）
// =========================================================================

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
                if format != OutputFormat::Human {
                    emit_json(
                        &JsonOptimizeRun {
                            command: "optimize",
                            task,
                            ok: false,
                            output: String::new(),
                            error: Some(
                                "未找到任务，用 `maclean optimize list-tasks` 查看".to_string(),
                            ),
                        },
                        format,
                        true,
                    );
                } else {
                    eprintln!("未找到任务: {}", task);
                    eprintln!("用 `maclean optimize list-tasks` 查看可用任务");
                }
                return EXIT_FAILURE;
            };
            // 非 Safe 任务需要显式 --yes（与 GUI 高危险项确认弹窗同口径）
            if !matches!(item.recommend, Recommend::Safe) && !yes {
                if format != OutputFormat::Human {
                    emit_json(
                        &JsonOptimizeRun {
                            command: "optimize",
                            task,
                            ok: false,
                            output: String::new(),
                            error: Some(format!("任务 {} 非低风险，需 --yes 确认", item.path)),
                        },
                        format,
                        true,
                    );
                } else {
                    eprintln!("任务 {} 非低风险，需 --yes 确认", item.path);
                }
                return EXIT_CONFIRM_REQUIRED;
            }

            let lang_en = crate::config::load_config().lang_en;
            #[cfg(target_os = "macos")]
            let output = crate::ops::execute_macos_optimize_task(&item.path, lang_en);
            #[cfg(target_os = "windows")]
            let output = crate::ops::execute_windows_optimize_task(&item.path, lang_en);
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let output = "当前平台不支持优化任务".to_string();

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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&scan).expect("scan 序列化失败"))
                .expect("scan JSON 非法");
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
        let src = include_str!("cli.rs");
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
    fn log_command_stays_human_readable() {
        // 日志命令刻意不做 JSON：它是给人排查用的，改成 JSON 反而更难读。
        // 直接钉签名 —— 一旦有人给 cmd_log 加了 json 参数，这里会红，
        // 提醒他要么真做 JSON 输出，要么回来改这条用例。
        let src = include_str!("cli.rs");
        assert!(
            src.contains("fn cmd_log(tail: Option<usize>, open: bool) -> u8 {"),
            "cmd_log 签名变了：要么真的实现 JSON 输出，要么回来改这条用例"
        );
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
        let src = include_str!("cli.rs");
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
        let src = include_str!("cli.rs");
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
