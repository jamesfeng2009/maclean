//! CLI 命令行接口
//!
//! 支持 `maclean scan`、`maclean clean`、`maclean check-disk` 等子命令。
//! 无参数时返回 None，由 main 启动 GUI。

use clap::{Parser, Subcommand};

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
use crate::scanner::uninstall::UninstallScanner;
use crate::scanner::{format_size, Scanner};
use crate::scanner::{Recommend, ScanItem};

/// maclean — macOS 磁盘清理工具
#[derive(Parser, Debug)]
#[command(name = "maclean", version, about = "macOS 磁盘清理工具", long_about = None)]
pub struct Cli {
    /// 子命令（无子命令时启动 GUI）
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// 以 JSON 输出（供脚本/监控系统消费）
    ///
    /// 结构化输出是稳定契约：字段名与语义不随文案翻译改变。人类可读的表格
    /// 输出不受影响（有测试锁住）。
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 扫描可清理项目
    Scan {
        /// 指定扫描的 Tab（dev-cache, large-files, app-cache, app-data, app-uninstall, optimize, apfs）
        #[arg(long)]
        tab: Option<String>,

        /// 深度扫描（扫描所有 Tab）
        #[arg(long)]
        deep: bool,
    },

    /// 清理安全可删除的项目
    Clean {
        /// 指定清理的 Tab
        #[arg(long)]
        tab: Option<String>,

        /// 仅清理推荐安全项（Safe）
        #[arg(long)]
        safe_only: bool,

        /// 试运行，只显示会清理什么，不实际删除
        #[arg(long)]
        dry_run: bool,
    },

    /// 检查磁盘空间使用情况
    CheckDisk,

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
}

/// 运行 CLI 命令，返回是否处理了 CLI（true=已处理，应退出；false=无命令，启动 GUI）
pub fn run_cli() -> bool {
    let cli = Cli::parse();

    match cli.command {
        None => false, // 无子命令，启动 GUI
        Some(cmd) => {
            run_command(cmd, cli.json);
            true
        }
    }
}

fn run_command(cmd: Commands, json: bool) {
    match cmd {
        Commands::Scan { tab, deep } => cmd_scan(tab, deep, json),
        Commands::Clean {
            tab,
            safe_only,
            dry_run,
        } => cmd_clean(tab, safe_only, dry_run, json),
        Commands::CheckDisk => cmd_check_disk(json),
        Commands::List => cmd_list(json),
        // 日志是给人看的，不做 JSON
        Commands::Log { tail, open } => cmd_log(tail, open),
        Commands::Backups { restorable_only } => cmd_backups(restorable_only, json),
        // 还原结果涉及逐个路径的成功/失败，JSON 更有用（脚本可据此重试）
        Commands::Restore { id } => cmd_restore(&id, json),
    }
}

// =========================================================================
//  结构化输出（--json）
//
//  所有命令共用这一层：先算出数据，再决定渲染成表格还是 JSON。
//  绝不能"表格和 JSON 各扫一遍" —— 那两份结果可能对不上。
// =========================================================================

fn print_json<T: serde::Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{}", s),
        Err(e) => {
            // 序列化失败不能静默：调用方拿不到任何输出会以为"没有可清理项"
            eprintln!(
                "{{\"error\": \"json serialization failed\", \"detail\": \"{}\"}}",
                e
            );
            std::process::exit(2);
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
];

/// 该扫描类别在当前平台是否有对应扫描器
///
/// `scan_tab` 里 "apfs" 只有 `#[cfg(target_os = "macos")]` 分支，其它平台
/// 落到 `_ => Vec::new()`。`--deep` 和 `list` 若不过滤，Windows 上会打印一个
/// 恒为 0 项的「APFS快照」—— 用户以为扫过了没东西，其实是这个平台没有。
fn tab_supported(name: &str) -> bool {
    match name {
        "apfs" => cfg!(target_os = "macos"),
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

#[derive(serde::Serialize)]
struct JsonItem {
    path: String,
    size_bytes: u64,
    category: String,
    description: String,
    deletable: bool,
    undeletable_reason: String,
    recommend: Recommend,
}

#[derive(serde::Serialize)]
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

fn cmd_scan(tab: Option<String>, deep: bool, json: bool) {
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

    for (tab_key, tab_label) in &tabs {
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

        if json {
            continue;
        }

        println!("\n╔══════════════════════════════════════════╗");
        println!("║  扫描 — {} ({})", tab_label, tab_key);
        println!("╚══════════════════════════════════════════╝");

        if items.is_empty() {
            println!("  （无可清理项目）\n");
            continue;
        }

        println!("  发现 {} 项，总计 {}", items.len(), format_size(tab_total));
        println!(
            "  其中安全可清理：{} 项，{}",
            safe_count,
            format_size(safe_size)
        );
        println!("  耗时 {:.2}s\n", elapsed.as_secs_f64());

        // 列出前 20 项
        let display_count = items.len().min(20);
        for (i, item) in items.iter().take(display_count).enumerate() {
            let recommend_icon = match item.recommend {
                Recommend::Safe => "✅",
                Recommend::CacheOnly => "🧹",
                Recommend::Caution => "⚠️",
                Recommend::Advanced => "🔴",
            };
            let deletable = if item.deletable { "" } else { " 🔒" };
            println!(
                "  {:>3}. {} [{}] {}{}",
                i + 1,
                recommend_icon,
                format_size(item.size_bytes),
                item.path,
                deletable
            );
            if !item.description.is_empty() {
                println!("       └─ {}", item.description);
            }
        }

        if items.len() > display_count {
            println!("  ... 还有 {} 项未显示", items.len() - display_count);
        }
    }

    if json {
        print_json(&JsonScan {
            command: "scan",
            tabs: out_tabs,
            total_count,
            total_size,
        });
        return;
    }

    if tabs.len() > 1 {
        println!("\n═══════════════════════════════════════════");
        println!(
            "  合计：{} 项，总计 {}",
            total_count,
            format_size(total_size)
        );
        println!("═══════════════════════════════════════════");
    }
}

#[derive(serde::Serialize)]
struct JsonPath {
    path: String,
    size_bytes: u64,
}

#[derive(serde::Serialize)]
struct JsonFailure {
    path: String,
    reason: String,
}

#[derive(serde::Serialize)]
struct JsonCleanTab {
    key: String,
    label: String,
    planned: Vec<JsonPath>,
    deleted: Vec<String>,
    failed: Vec<JsonFailure>,
    rejected: Vec<JsonFailure>,
    skipped_snapshots: usize,
}

#[derive(serde::Serialize)]
struct JsonClean {
    command: &'static str,
    dry_run: bool,
    tabs: Vec<JsonCleanTab>,
    success: usize,
    failed: usize,
    rejected: usize,
}

fn cmd_clean(tab: Option<String>, safe_only: bool, dry_run: bool, json: bool) {
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
            if !json {
                println!("  （无可清理项目）");
            }
            continue;
        }

        let clean_size: u64 = to_clean.iter().map(|i| i.size_bytes).sum();
        let mut rec = JsonCleanTab {
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
        };

        if !json {
            println!("\n🧹 清理 {} ({})", tab_label, tab_key);
            println!(
                "  将清理 {} 项，释放 {}",
                to_clean.len(),
                format_size(clean_size)
            );
        }

        if dry_run {
            if json {
                out.push(rec);
                continue;
            }
            println!("  [dry-run] 未实际删除，以下为将被清理的项目：");
            for item in &to_clean {
                println!("    - [{}] {}", format_size(item.size_bytes), item.path);
            }
            continue;
        }

        // 删除前复做安全校验（与 GUI 同一层）
        //
        // 扫描时判过 `deletable`，但从扫描到删除有时间差：路径可能已被替换
        // （TOCTOU），也可能被换成符号链接 —— 对软链执行 remove_dir_all 会
        // 顺着链接删到目标之外。GUI 侧 ops::sanitize_before_delete 一直在做
        // 这件事，CLI 原先完全跳过，等于绕过整层防护直接删。
        let pairs: Vec<(String, String)> = to_clean
            .iter()
            .filter(|i| !i.path.starts_with("snapshot:"))
            .map(|i| (i.path.clone(), i.category.clone()))
            .collect();
        let skipped_snapshots = to_clean.len() - pairs.len();
        rec.skipped_snapshots = skipped_snapshots;

        let (allowed, rejected) = crate::ops::sanitize_before_delete(pairs, false);
        rec.rejected = rejected
            .iter()
            .map(|(p, _c, r)| JsonFailure {
                path: p.clone(),
                reason: r.clone(),
            })
            .collect();
        g_rejected += rejected.len();

        if !json {
            for (path, _category, reason) in &rejected {
                println!("  🛡️  已拦截 {} — {}", path, reason);
            }
            if skipped_snapshots > 0 {
                println!(
                    "  ⏭️  跳过 {} 个 APFS 快照（需特殊处理）",
                    skipped_snapshots
                );
            }
        }

        // 实际删除
        let mut success = 0usize;
        let mut failed = 0usize;
        for (path, _category) in &allowed {
            let result = if std::path::Path::new(path).is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };

            match result {
                Ok(_) => {
                    rec.deleted.push(path.clone());
                    success += 1;
                    if !json {
                        println!("  ✅ {}", path);
                    }
                }
                Err(e) => {
                    rec.failed.push(JsonFailure {
                        path: path.clone(),
                        reason: e.to_string(),
                    });
                    failed += 1;
                    if !json {
                        println!("  ❌ {} — {}", path, e);
                    }
                }
            }
        }
        g_success += success;
        g_failed += failed;

        if !json {
            println!(
                "\n  完成：成功 {}，失败 {}，安全拦截 {}",
                success,
                failed,
                rejected.len()
            );
        }
        out.push(rec);
    }

    if json {
        print_json(&JsonClean {
            command: "clean",
            dry_run,
            tabs: out,
            success: g_success,
            failed: g_failed,
            rejected: g_rejected,
        });
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

fn cmd_check_disk(json: bool) {
    let (total, free) = get_disk_info();
    if total == 0 {
        if json {
            // 拿不到磁盘信息时必须是**结构化的错误**，不能静默成功：
            // 监控脚本看到 exit 0 且字段全 0 会以为"磁盘空了"
            eprintln!("{{\"command\": \"check-disk\", \"error\": \"unable to read disk info\"}}");
            std::process::exit(1);
        }
        println!("❌ 无法获取磁盘信息");
        return;
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

    if json {
        print_json(&JsonDisk {
            command: "check-disk",
            total_bytes: total,
            used_bytes: used,
            free_bytes: free,
            used_percent: used_pct,
            free_percent: free_pct,
            alert_level: level,
        });
        return;
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

fn cmd_list(json: bool) {
    let supported: Vec<(&str, &str)> = ALL_TABS
        .iter()
        .filter(|(k, _)| tab_supported(k))
        .map(|(k, v)| (*k, *v))
        .collect();

    if json {
        print_json(&JsonList {
            command: "list",
            tabs: supported
                .iter()
                .map(|(k, v)| JsonListEntry { key: k, label: v })
                .collect(),
        });
        return;
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
fn cmd_backups(restorable_only: bool, json: bool) {
    let pruned = crate::backup::prune_old();
    let mut manifests = crate::backup::list();
    if restorable_only {
        manifests.retain(|m| m.restorable_count() > 0);
    }

    if json {
        print_json(&JsonBackups {
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
        });
        return;
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
            let ts = chrono_like(m.created_at);
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
}

/// 把 Unix 秒渲染成固定宽度的时间串
///
/// 刻意不引入 chrono：这里只需要一个人类可读的时间戳，为此拖进一个
/// 日期库（连带时区表）不值得。格式固定为 UTC，避免不同机器显示不一致。
fn chrono_like(secs: u64) -> String {
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
fn cmd_restore(id: &str, json: bool) {
    let Some(manifest) = crate::backup::load(id) else {
        if json {
            print_json(&serde_json::json!({
                "command": "restore",
                "manifest_id": id,
                "error": "manifest not found",
            }));
        } else {
            eprintln!("找不到清单 {}，用 `maclean backups` 查看可用清单", id);
        }
        std::process::exit(1);
    };

    let report = crate::backup::restore(id, &crate::backup::trash_dir());

    if json {
        print_json(&JsonRestore {
            command: "restore",
            manifest_id: id,
            restored: &report.restored,
            not_restorable: &report.not_restorable,
            failed: &report.failed,
        });
        return;
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

fn cmd_log(tail: Option<usize>, open: bool) {
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
        return;
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
            dry_run: true,
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
        let check = src[src
            .find("fn cmd_check_disk(json: bool) {")
            .expect("cmd_check_disk")..]
            .split("\nfn cmd_list(")
            .next()
            .unwrap();
        let json_at = check
            .find("if json {")
            .expect("cmd_check_disk 没有 json 分支");
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
        assert_eq!(chrono_like(0), "1970-01-01 00:00:00");
        // 闰年 2 月 29 日：手搓日历最容易错的就是这里
        assert_eq!(chrono_like(1709164800), "2024-02-29 00:00:00");
        assert_eq!(chrono_like(1789831800), "2026-09-19 15:30:00");
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
        assert_eq!(chrono_like(secs), "2100-01-01 00:00:00");
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
    fn log_command_stays_human_readable() {
        // 日志命令刻意不做 JSON：它是给人排查用的，改成 JSON 反而更难读。
        // 直接钉签名 —— 一旦有人给 cmd_log 加了 json 参数，这里会红，
        // 提醒他要么真做 JSON 输出，要么回来改这条用例。
        let src = include_str!("cli.rs");
        assert!(
            src.contains("fn cmd_log(tail: Option<usize>, open: bool) {"),
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
