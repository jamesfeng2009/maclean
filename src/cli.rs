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
}

/// 运行 CLI 命令，返回是否处理了 CLI（true=已处理，应退出；false=无命令，启动 GUI）
pub fn run_cli() -> bool {
    let cli = Cli::parse();

    match cli.command {
        None => false, // 无子命令，启动 GUI
        Some(cmd) => {
            run_command(cmd);
            true
        }
    }
}

fn run_command(cmd: Commands) {
    match cmd {
        Commands::Scan { tab, deep } => cmd_scan(tab, deep),
        Commands::Clean {
            tab,
            safe_only,
            dry_run,
        } => cmd_clean(tab, safe_only, dry_run),
        Commands::CheckDisk => cmd_check_disk(),
        Commands::List => cmd_list(),
        Commands::Log { tail, open } => cmd_log(tail, open),
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

fn cmd_scan(tab: Option<String>, deep: bool) {
    let tabs: Vec<(String, String)> = if deep {
        ALL_TABS
            .iter()
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

    for (tab_key, tab_label) in &tabs {
        println!("\n╔══════════════════════════════════════════╗");
        println!("║  {} — {} ({})", "扫描", tab_label, tab_key);
        println!("╚══════════════════════════════════════════╝");

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

        if items.is_empty() {
            println!("  （无可清理项目）\n");
            continue;
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

        total_size += tab_total;
        total_count += items.len();
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

fn cmd_clean(tab: Option<String>, safe_only: bool, dry_run: bool) {
    let tabs: Vec<String> = if let Some(t) = &tab {
        vec![t.clone()]
    } else {
        vec!["dev-cache".to_string()]
    };

    for tab_key in &tabs {
        let tab_label = ALL_TABS
            .iter()
            .find(|(k, _)| *k == tab_key.as_str())
            .map(|(_, v)| *v)
            .unwrap_or(tab_key);
        println!("\n🧹 清理 {} ({})", tab_label, tab_key);

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
            println!("  （无可清理项目）");
            continue;
        }

        let clean_size: u64 = to_clean.iter().map(|i| i.size_bytes).sum();
        println!(
            "  将清理 {} 项，释放 {}",
            to_clean.len(),
            format_size(clean_size)
        );

        if dry_run {
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

        let (allowed, rejected) = crate::ops::sanitize_before_delete(pairs, false);

        for (path, _category, reason) in &rejected {
            println!("  🛡️  已拦截 {} — {}", path, reason);
        }
        if skipped_snapshots > 0 {
            println!("  ⏭️  跳过 {} 个 APFS 快照（需特殊处理）", skipped_snapshots);
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
                    println!("  ✅ {}", path);
                    success += 1;
                }
                Err(e) => {
                    println!("  ❌ {} — {}", path, e);
                    failed += 1;
                }
            }
        }

        println!(
            "\n  完成：成功 {}，失败 {}，安全拦截 {}",
            success,
            failed,
            rejected.len()
        );
    }
}

fn cmd_check_disk() {
    let (total, free) = get_disk_info();
    if total == 0 {
        println!("❌ 无法获取磁盘信息");
        return;
    }

    let used = total - free;
    let used_pct = used as f64 / total as f64 * 100.0;
    let free_pct = free as f64 / total as f64 * 100.0;

    println!("╔══════════════════════════════════════════╗");
    println!("║           磁盘空间检查                    ║");
    println!("╠══════════════════════════════════════════╣");
    println!("  总容量:  {}", format_size(total));
    println!("  已使用:  {} ({:.1}%)", format_size(used), used_pct);
    println!("  可用:    {} ({:.1}%)", format_size(free), free_pct);
    println!();

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

fn cmd_list() {
    println!("\nmaclean 可用扫描类别：\n");
    for (key, label) in ALL_TABS {
        println!("  {:<16}  {}", format!("--tab {}", key), label);
    }
    println!("\n用法示例：");
    println!("  maclean scan --tab dev-cache     # 扫描开发者缓存");
    println!("  maclean scan --deep              # 深度扫描所有类别");
    println!("  maclean clean --tab dev-cache --safe-only --dry-run  # 预览安全清理");
    println!("  maclean check-disk               # 检查磁盘空间");
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
