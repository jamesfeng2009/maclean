//! IPC 白名单命令层
//!
//! 前端（WebView）与本机能力之间**唯一**的桥梁。安全约束：
//!
//! - 前端拿不到任何裸文件句柄 / shell，只能调用这里登记的 command；
//! - 所有删除都走 [`maclean_core::ops::start_delete`]，其三个删除出口在
//!   core 内部逐个过 [`maclean_core::safety::check_path_safety_with_category`]
//!   闸门 —— 前端传来的路径不被信任，删除前在 Rust 侧重新判定；
//! - 启动项切换只移动 plist（备份/还原），不删除文件；
//! - 扫描是只读操作，panic 被 catch_unwind 吞掉并返回空结果，绝不拖垮壳。

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use maclean_core::config;
use maclean_core::config::AppConfig;
use maclean_core::ops::{self, DeleteMessage};
use maclean_core::safety::SafetyCheck;
use maclean_core::scanner;
use maclean_core::scanner::{Recommend, ScanItem, Scanner};
use maclean_core::{platform, safety};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/* ============================== 磁盘 ============================== */

#[derive(Debug, Serialize)]
pub struct DiskInfoDto {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub usage_pct: f32,
}

/// 启动磁盘容量（真实数据：`df -k /` / Windows PowerShell）。
#[tauri::command]
pub fn disk_info() -> DiskInfoDto {
    let (total, free) = platform::disk_info();
    let used = total.saturating_sub(free);
    let pct = if total > 0 {
        used as f32 / total as f32 * 100.0
    } else {
        0.0
    };
    DiskInfoDto {
        total_bytes: total,
        free_bytes: free,
        used_bytes: used,
        usage_pct: pct,
    }
}

/* ============================== 扫描 ============================== */

/// 扫描后处理：与 egui 壳 `start_scan` 完全一致的可删除性二次判定。
///
/// 扫得出 ≠ 删得了：扫描阶段的判定在这里按当前文件系统实况复核一遍，
/// 删不掉的项标 `deletable=false` 并带原因，前端只展示不可勾选态。
fn post_check(items: &mut [ScanItem]) {
    for item in items.iter_mut() {
        let (deletable, reason) = scanner::check_deletable(&item.path);
        item.deletable = deletable && item.deletable;
        if !item.deletable && !reason.is_empty() {
            item.undeletable_reason = reason;
        }
    }
}

/// 按 scope 运行对应扫描器（只读）。
fn run_scanner(scope: &str) -> Vec<ScanItem> {
    macro_rules! scan_of {
        ($scanner:expr) => {{
            let mut r = $scanner.scan();
            post_check(&mut r.items);
            r.items
        }};
    }

    match scope {
        "dev_cache" => scan_of!(scanner::dev_cache::DevCacheScanner::new()),
        "large" => scan_of!(scanner::large_files::LargeFileScanner::new()),
        "dup" => scan_of!(scanner::dup_files::DuplicateFileScanner::new()),
        #[cfg(target_os = "macos")]
        "app_cache" => scan_of!(scanner::app_cache::AppCacheScanner::new()),
        #[cfg(target_os = "macos")]
        "app_data" => scan_of!(scanner::app_data::AppDataScanner::new()),
        #[cfg(target_os = "macos")]
        "apps" => scan_of!(scanner::uninstall::UninstallScanner::new()),
        // 概览/智能清理页的跨扫描器聚合在 scan() 内用 run_all_scanners 处理，
        // run_scanner("all") 不会被调到；保留分支仅为穷尽匹配。
        "all" => Vec::new(),
        _ => Vec::new(),
    }
}

#[cfg(target_os = "macos")]
fn run_all_scanners(app: &AppHandle) -> Vec<ScanItem> {
    // 阶段顺序模拟交互稿的 9 阶段提示；每个扫描器独立 catch，单个挂了不拖垮全部。
    let stages = [
        ("dev_cache", "开发者缓存", 20u8),
        ("app_cache", "应用缓存", 55),
        ("app_data", "应用数据与残留", 85),
    ];
    let mut all = Vec::new();
    for (key, label, pct) in stages {
        let _ = app.emit(
            "scan-progress",
            serde_json::json!({ "stage": key, "label": label, "pct": pct }),
        );
        let part =
            std::panic::catch_unwind(AssertUnwindSafe(|| run_scanner(key))).unwrap_or_default();
        all.extend(part);
    }
    let _ = app.emit(
        "scan-progress",
        serde_json::json!({ "stage": "done", "label": "完成", "pct": 100 }),
    );
    all
}

/// 全量体检：顺序跑完 缓存/应用数据 → 大文件 → 重复文件 → 已安装应用。
///
/// 与 [`run_all_scanners`] 的差别：每完成一个**模块**就通过 `scan-module`
/// 事件把该模块结果推给前端，前端按模块即时入库。用户在概览点一次
/// 「开始扫描」即可让所有页面复用这一份结果，且无需等全部跑完——
/// 哪个模块先完成，对应页面立刻可看；扫描在后台进行，可自由切换页面。
/// 全程只读，单个扫描器 panic 被隔离，不拖垮其余模块。
#[cfg(target_os = "macos")]
fn run_full_scan(app: &AppHandle) -> Vec<ScanItem> {
    fn safe_run(key: &str) -> Vec<ScanItem> {
        std::panic::catch_unwind(AssertUnwindSafe(|| run_scanner(key))).unwrap_or_default()
    }
    let emit_pct = |pct: u8, label: &str| {
        let _ = app.emit(
            "scan-progress",
            serde_json::json!({ "stage": "full", "label": label, "pct": pct }),
        );
    };
    let emit_module = |key: &str, items: &[ScanItem]| {
        let _ = app.emit(
            "scan-module",
            serde_json::json!({ "scope": key, "items": items }),
        );
    };

    let mut all: Vec<ScanItem> = Vec::new();

    // 模块 all = 开发者缓存 + 应用缓存 + 应用数据与残留（概览 / 智能清理共用）
    emit_pct(6, "开发者缓存");
    let dev = safe_run("dev_cache");
    emit_pct(34, "应用缓存");
    let appc = safe_run("app_cache");
    emit_pct(58, "应用数据与残留");
    let appd = safe_run("app_data");
    let cache_items: Vec<ScanItem> = dev.into_iter().chain(appc).chain(appd).collect();
    emit_pct(66, "完成缓存扫描");
    emit_module("all", &cache_items);
    all.extend(cache_items);

    // 模块 large = 磁盘分析（大文件 / 大目录）
    emit_pct(72, "正在分析磁盘大文件");
    let large = safe_run("large");
    emit_pct(84, "完成大文件分析");
    emit_module("large", &large);
    all.extend(large);

    // 模块 dup = 重复文件
    emit_pct(88, "正在比对重复文件");
    let dup = safe_run("dup");
    emit_pct(96, "完成重复文件比对");
    emit_module("dup", &dup);
    all.extend(dup);

    // 模块 apps = 已安装应用
    emit_pct(98, "正在枚举已安装应用");
    let apps = safe_run("apps");
    emit_pct(100, "完成");
    emit_module("apps", &apps);
    all.extend(apps);

    all
}

/// 扫描命令（后台线程，进度经 `scan-progress` 事件推送）。
///
/// scope: dev_cache / app_cache / app_data / large / dup / apps / all
#[tauri::command]
pub async fn scan(scope: String, app: AppHandle) -> Result<Vec<ScanItem>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<ScanItem>, String> {
        let _ = app.emit(
            "scan-progress",
            serde_json::json!({ "stage": &scope, "label": "准备扫描", "pct": 2 }),
        );

        let items = if scope == "full" {
            // 全量体检：逐模块经 scan-module 事件回传结果（后台渐进式）
            #[cfg(target_os = "macos")]
            {
                run_full_scan(&app)
            }
            #[cfg(not(target_os = "macos"))]
            {
                // 全量体检目前仅在 macOS 提供五模块；其它平台退化为开发者缓存
                std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let mut r = scanner::dev_cache::DevCacheScanner::new().scan();
                    post_check(&mut r.items);
                    r.items
                }))
                .unwrap_or_default()
            }
        } else if scope == "all" {
            #[cfg(target_os = "macos")]
            {
                run_all_scanners(&app)
            }
            #[cfg(not(target_os = "macos"))]
            {
                std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let mut r = scanner::dev_cache::DevCacheScanner::new().scan();
                    post_check(&mut r.items);
                    r.items
                }))
                .unwrap_or_default()
            }
        } else {
            std::panic::catch_unwind(AssertUnwindSafe(|| run_scanner(&scope))).unwrap_or_default()
        };

        let _ = app.emit(
            "scan-progress",
            serde_json::json!({ "stage": "done", "label": "完成", "pct": 100 }),
        );
        Ok(items)
    })
    .await
    .map_err(|e| format!("扫描任务异常: {e}"))?
}

/* ============================== 启动项 ============================== */

#[derive(Debug, Serialize)]
pub struct StartupItemDto {
    pub label: String,
    pub plist: String,
    pub scope: String,
    /// 当前 launchctl 是否加载
    pub enabled: bool,
}

/// 启动项列表（只读扫描，<500ms）。
#[tauri::command]
pub fn startups_list() -> Vec<StartupItemDto> {
    scanner::startup::scan_startup_items()
        .into_iter()
        .map(|i| StartupItemDto {
            label: i.label,
            plist: i.plist.to_string_lossy().to_string(),
            scope: i.scope,
            enabled: i.enabled,
        })
        .collect()
}

fn startup_backup_root() -> PathBuf {
    platform::home_dir().join(".maclean/disabled_launchd")
}

/// 在备份目录中按 Label 递归查找被禁用的 plist（与 CLI 启用流程一致）。
fn find_backup_plist(root: &std::path::Path, label: &str) -> Option<PathBuf> {
    fn walk(dir: &std::path::Path, label: &str, out: &mut Option<PathBuf>) {
        if out.is_some() {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, label, out);
            } else if p.extension().is_some_and(|x| x == "plist")
                && scanner::startup::read_label(&p).as_deref() == Some(label)
            {
                *out = Some(p);
                return;
            }
        }
    }
    let mut out = None;
    walk(root, label, &mut out);
    out
}

/// 启用/禁用启动项。
///
/// **只改配置不删文件**：禁用把 plist 移到 `~/.maclean/disabled_launchd`
/// 备份目录，启用按 Label 找回备份并移回原位 —— 与 CLI / egui 壳调用
/// 同一组 core 函数。
#[tauri::command]
pub fn startup_set_enabled(
    label: String,
    plist: String,
    scope: String,
    enabled: bool,
) -> Result<String, String> {
    let backup_root = startup_backup_root();
    if enabled {
        // 前端给的 plist 是禁用前的原路径；启用时必须以备份目录为准反推原路径
        let backup_path = find_backup_plist(&backup_root, &label)
            .ok_or_else(|| format!("备份中未找到启动项: {label}"))?;
        let home = std::env::var("HOME").unwrap_or_default();
        let item = scanner::startup::restore_origin_from_backup(&backup_path, &backup_root, &home)?;
        scanner::startup::enable_startup_item(&item, &backup_root)
    } else {
        let item = scanner::startup::StartupItem {
            label,
            plist: PathBuf::from(&plist),
            scope,
            enabled: false,
        };
        scanner::startup::disable_startup_item(&item, &backup_root)
    }
}

/* ============================== 系统优化 ============================== */

/// 当前平台维护任务清单（低风险 safe / 高风险 advanced）。
#[tauri::command]
pub fn optimize_list() -> Vec<ScanItem> {
    scanner::optimize::current_platform_tasks()
}

/// 执行维护任务。高风险项由前端弹确认框后才会调用本命令。
#[tauri::command]
pub async fn optimize_run(path: String, lang_en: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        // execute_* 返回结果消息 String（不是 Result），这里统一包成 Ok。
        #[cfg(target_os = "macos")]
        {
            Ok(ops::execute_macos_optimize_task(&path, lang_en))
        }
        #[cfg(target_os = "windows")]
        {
            Ok(ops::execute_windows_optimize_task(&path, lang_en))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = lang_en;
            Err("当前平台不支持系统维护任务: ".to_string() + &path)
        }
    })
    .await
    .map_err(|e| format!("维护任务执行异常: {e}"))?
}

/* ============================== 清理（删除） ============================== */

/// 前端回传的待清理项；路径与分类来自此前真实扫描结果，删除前仍重新过闸门。
#[derive(Debug, Clone, Deserialize)]
pub struct CleanItemReq {
    pub path: String,
    pub category: String,
    #[serde(default)]
    pub batch_paths: Vec<String>,
    pub size_bytes: u64,
    /// safe / cache_only / caution / advanced
    pub recommend: String,
}

#[derive(Debug, Serialize)]
pub struct PreviewItemDto {
    pub path: String,
    pub category: String,
    pub size_bytes: u64,
    /// 通过 safety + 可删除性双重判定
    pub allowed: bool,
    /// 不允许时给用户看的原因
    pub reason: String,
    pub recommend: String,
}

/// Dry-run 预览：逐项过 safety 闸门（不做任何文件变更）。
#[tauri::command]
pub fn clean_preview(items: Vec<CleanItemReq>) -> Vec<PreviewItemDto> {
    items
        .into_iter()
        .map(|it| {
            let (allowed, reason) =
                match safety::check_path_safety_with_category(&it.path, &it.category) {
                    SafetyCheck::Safe => {
                        let (ok, why) = scanner::check_deletable(&it.path);
                        (ok, why)
                    }
                    SafetyCheck::Danger(r) => (false, r),
                    SafetyCheck::Warning(r) => {
                        let (ok, why) = scanner::check_deletable(&it.path);
                        // Warning 允许进入但保留提示（高风险走前端二次确认）
                        (ok, if why.is_empty() { r } else { why })
                    }
                };
            PreviewItemDto {
                path: it.path,
                category: it.category,
                size_bytes: it.size_bytes,
                allowed,
                reason,
                recommend: it.recommend,
            }
        })
        .collect()
}

#[derive(Debug, Default, Serialize)]
pub struct CleanReport {
    /// 成功删除/移入废纸篓的项数
    pub deleted: usize,
    /// 被闸门拦截（日志计为失败/拦截）的项
    pub intercepted: usize,
    /// 路径已不存在等跳过项
    pub skipped: usize,
    /// 需要管理员权限、本轮未处理的项（Tauri 壳暂不做提权，引导用桌面版）
    pub need_password: usize,
    /// 恢复清单 id
    pub backup_id: Option<String>,
    pub restorable: usize,
    pub total: usize,
    pub cancelled: bool,
}

/// 执行清理。复用 egui 壳同一条 `start_delete` 链路：
/// safety 闸门 + 废纸篓优先 + 官方卸载器 + M-2 备份清单。
#[tauri::command]
pub async fn clean_execute(
    items: Vec<CleanItemReq>,
    lang_en: bool,
    app: AppHandle,
) -> Result<CleanReport, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<CleanReport, String> {
        if items.is_empty() {
            return Ok(CleanReport::default());
        }
        // (path, category, batch_paths, use_trash, size_bytes)
        let to_delete: Vec<(String, String, Vec<String>, bool, u64)> = items
            .iter()
            .map(|i| {
                (
                    i.path.clone(),
                    i.category.clone(),
                    i.batch_paths.clone(),
                    true,
                    i.size_bytes,
                )
            })
            .collect();

        let cancel = Arc::new(AtomicBool::new(false));
        let mut delete_rx: Option<std::sync::mpsc::Receiver<DeleteMessage>> = None;
        ops::start_delete(
            to_delete,
            lang_en,
            &mut delete_rx,
            false,
            true, // prefer_official_uninstaller
            cancel,
        );

        let mut report = CleanReport::default();
        if let Some(rx) = delete_rx {
            while let Ok(msg) = rx.recv() {
                match msg {
                    DeleteMessage::Log(line, path, _, ok) => {
                        let _ = app.emit(
                            "clean-log",
                            serde_json::json!({ "line": line, "path": path, "ok": ok }),
                        );
                        if ok {
                            report.deleted += 1;
                        } else {
                            report.intercepted += 1;
                        }
                    }
                    DeleteMessage::Skip(..) | DeleteMessage::Info(_) => {
                        report.skipped += 1;
                    }
                    DeleteMessage::NeedPassword(v) => {
                        report.need_password += v.len();
                    }
                    DeleteMessage::BackupRecorded {
                        id,
                        restorable,
                        total,
                    } => {
                        report.backup_id = Some(id);
                        report.restorable = restorable;
                        report.total = total;
                    }
                    DeleteMessage::Cancelled => report.cancelled = true,
                    DeleteMessage::Done => break,
                }
            }
        }
        Ok(report)
    })
    .await
    .map_err(|e| format!("清理任务异常: {e}"))?
}

/* ============================== 设置 ============================== */

#[tauri::command]
pub fn settings_get() -> AppConfig {
    config::load_config()
}

/// 白名单字段更新；未列出的键一律忽略，避免前端写入任意配置。
#[tauri::command]
pub fn settings_set(patch: serde_json::Value) -> Result<(), String> {
    let mut cfg = config::load_config();
    let obj = patch.as_object().ok_or("settings patch 必须是对象")?;
    let get = |k: &str| obj.get(k).cloned();

    if let Some(v) = get("lang_en") {
        if let Some(x) = v.as_bool() {
            cfg.lang_en = x;
        }
    }
    if let Some(v) = get("settings_confirm_advanced") {
        if let Some(x) = v.as_bool() {
            cfg.settings_confirm_advanced = x;
        }
    }
    if let Some(v) = get("settings_prefer_official_uninstaller") {
        if let Some(x) = v.as_bool() {
            cfg.settings_prefer_official_uninstaller = x;
        }
    }
    if let Some(v) = get("settings_show_protected_items") {
        if let Some(x) = v.as_bool() {
            cfg.settings_show_protected_items = x;
        }
    }
    if let Some(v) = get("schedule_enabled") {
        if let Some(x) = v.as_bool() {
            cfg.schedule_enabled = x;
        }
    }
    if let Some(v) = get("schedule_interval_days") {
        if let Some(x) = v.as_u64() {
            cfg.schedule_interval_days = x as u32;
        }
    }
    config::save_config(&cfg);
    Ok(())
}

/* ============================== 设计 token ============================== */

/// 返回当前主题的完整色板（Rust 侧 design_tokens 是单一事实来源）。
#[tauri::command]
pub fn palette(dark: bool) -> maclean_core::design_tokens::Palette {
    if dark {
        maclean_core::design_tokens::Palette::DARK
    } else {
        maclean_core::design_tokens::Palette::LIGHT
    }
}

// 供前端展示用的等级 key 转换（骨架阶段预留）。
#[allow(dead_code)]
fn recommend_key(r: Recommend) -> &'static str {
    match r {
        Recommend::Safe => "safe",
        Recommend::CacheOnly => "cache_only",
        Recommend::Caution => "caution",
        Recommend::Advanced => "advanced",
    }
}
