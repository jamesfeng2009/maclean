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

/// 在访达中打开用户废纸篓（仅打开，绝不替用户执行清空）。
///
/// maclean 的删除统一「移入废纸篓」，同卷上清空前不释放空间。这里只负责把
/// 废纸篓在 Finder 中打开（`open` 走启动服务、不枚举目录内容，因而不需要
/// 「完全磁盘访问」）；清空是不可逆永久删除，交给 Finder 由用户本人确认。
/// 无参数、无注入面。仅 macOS。
#[tauri::command]
pub fn reveal_trash() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let dir = maclean_core::backup::trash_dir();
        if dir.is_empty() {
            return Err("无法定位废纸篓".to_string());
        }
        std::process::Command::new("open")
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("无法打开废纸篓: {e}"))?
            .success()
            .then_some(())
            .ok_or_else(|| "打开废纸篓失败".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("当前平台暂不支持打开废纸篓".to_string())
    }
}

/// 打开「系统设置 → 隐私与安全性 → 完全磁盘访问」，引导用户给 maclean 授权。
///
/// 一键卸载删除沙盒容器（`~/Library/Containers`、`HTTPStorages` 等）或
/// `/Applications` 下的 .app 时，可能被 TCC 以「完全磁盘访问 / App 管理」
/// 拒绝（os error 1 / EPERM），或因应用正在运行被占用。删除本身无法也不应
/// 绕过该授权；这里只把用户带到正确的设置面板，由用户本人开启开关后重试。
/// URL 为 macOS 系统设置固定 scheme、无外部输入与注入面。仅 macOS。
#[tauri::command]
pub fn open_full_disk_access_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // 完全磁盘访问面板（容器数据）。同页也可让用户确认「App 管理」（删除 .app）。
        let url = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
        std::process::Command::new("open")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("无法打开系统设置: {e}"))?
            .success()
            .then_some(())
            .ok_or_else(|| "打开系统设置失败".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("当前平台暂不支持打开该系统设置".to_string())
    }
}

/// 打开「系统设置 → 隐私与安全性 → App 管理（应用管理）」面板。
///
/// 删除 `/Applications` 下 root 安装（`root:wheel`）的 .app 本体时，即使有
/// 完全磁盘访问，仍受「App 管理」权限管控（或系统弹窗 Touch ID/密码确认）。
/// 此命令把用户带到对应面板，由用户本人开启 maclean 的开关后重试。
/// URL 为 macOS 系统设置固定 scheme、无外部输入与注入面。仅 macOS。
#[tauri::command]
pub fn open_app_management_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // App 管理面板（/Applications 下 .app 的删除授权）
        let url = "x-apple.systempreferences:com.apple.preference.security?Privacy_AppManagement";
        std::process::Command::new("open")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("无法打开系统设置: {e}"))?
            .success()
            .then_some(())
            .ok_or_else(|| "打开系统设置失败".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("当前平台暂不支持打开该系统设置".to_string())
    }
}

/* ============================== 扫描 ============================== */

/// 扫描后处理：与 egui 壳 `start_scan` 完全一致的可删除性二次判定。
///
/// 扫得出 ≠ 删得了：扫描阶段的判定在这里按当前文件系统实况复核一遍，
/// 扫描结果出口的统一校验（每个扫描器的结果都经过这里）：
///
/// 保证“会被『安全清理』默认勾选的项，执行删除时 100% 通过安全闸门”——
/// 用户点安全清理后不应再看到“已安全拦截 N 项”。
/// 1. 聚合项：逐个成员过闸，命中保护规则的成员在扫描期就剔除并回减大小/计数，
///    全部成员都受保护时整条移除（`scanner::retain_safe_batch_members`）；
/// 2. 会被默认勾选的单项（Safe/CacheOnly）：自身必须通过闸门，否则标不可删；
/// 3. 既有：SIP/属主等系统可删性检查，删不掉的项标 `deletable=false` 并带原因。
fn post_check(items: &mut Vec<ScanItem>) {
    let mut i = 0;
    while i < items.len() {
        if !items[i].batch_paths.is_empty() {
            // 聚合项：净化掉未过闸的成员；一个可删成员都不剩则整条丢弃
            if !scanner::retain_safe_batch_members(&mut items[i]) {
                items.remove(i);
                continue;
            }
        } else if matches!(
            items[i].recommend,
            Recommend::Safe | Recommend::CacheOnly
        ) {
            // 默认可选单项：必须过 safety 闸门，否则不进安全清理集合
            if let SafetyCheck::Danger(r) | SafetyCheck::Warning(r) =
                safety::check_path_safety_with_category(&items[i].path, &items[i].category)
            {
                items[i].deletable = false;
                if items[i].undeletable_reason.is_empty() {
                    items[i].undeletable_reason = r;
                }
            }
        }

        let (deletable, reason) = scanner::check_deletable(&items[i].path);
        items[i].deletable = deletable && items[i].deletable;
        if !items[i].deletable && !reason.is_empty() {
            items[i].undeletable_reason = reason;
        }
        i += 1;
    }
}

/// 按 scope 运行对应扫描器（只读）。
/// 各扫描器的墙钟预算（**上限保险**，不是目标耗时）。
///
/// 正常扫描应远低于此；仅当某扫描器内部仍存在漏网的「不可取消内核 IO 永久阻塞」
/// 时（例如一个我们没能提前识别的网络卷），到点放弃该扫描器本轮结果。这样
/// `run_full_scan` 的模块 `join` 不会被无限拖住，前端一定能到达「体检完成」。
#[cfg(target_os = "macos")]
fn scanner_budget(scope: &str) -> std::time::Duration {
    use std::time::Duration;
    match scope {
        // 重复文件要逐个做内容哈希，天然重计算，给最宽预算。
        "dup" => Duration::from_secs(420),
        // 全 home 大文件遍历（遍历层已剪掉网络卷 / TCC 容器）。
        "large" => Duration::from_secs(180),
        "dev_cache" => Duration::from_secs(150),
        "apps" => Duration::from_secs(120),
        "app_cache" => Duration::from_secs(90),
        "app_data" => Duration::from_secs(90),
        _ => Duration::from_secs(120),
    }
}

/// scope → 中文模块名（用于部分结果事件 / 提示）。
fn scope_label(scope: &str) -> &'static str {
    match scope {
        "dev_cache" => "开发者缓存",
        "app_cache" => "应用缓存",
        "app_data" => "应用数据与残留",
        "large" => "磁盘大文件",
        "dup" => "重复文件",
        "apps" => "已安装应用",
        "all" => "缓存与应用数据",
        _ => "部分扫描项",
    }
}

/// 运行一个扫描器，返回 `(项目, 是否为部分结果)`。
///
/// `partial == true` 表示该扫描器（或其内某些分段）因磁盘高负载 / 不可达目录在预算内
/// 没能完整跑完：此时 `项目` 是**已成功统计到的部分**（可能偏小），而不是像旧实现那样
/// 到点整体丢弃成空 Vec——繁忙环境下也先给用户可用结果，机器空闲后重新扫描即可补齐。
fn run_scanner(scope: &str) -> (Vec<ScanItem>, bool) {
    // app_cache 由 6 个相对独立的来源组成：改走「分段独立预算」，某一段超时只丢该段，
    // 其余段照常合并，避免最重的容器/Application Support 遍历拖垮整个 app_cache。
    #[cfg(target_os = "macos")]
    if scope == "app_cache" {
        return run_app_cache_segmented();
    }

    let key = scope.to_string();
    let job = move || -> Vec<ScanItem> {
        macro_rules! scan_of {
            ($scanner:expr) => {{
                let mut r = $scanner.scan();
                post_check(&mut r.items);
                r.items
            }};
        }

        match key.as_str() {
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
    };

    // 顶层硬超时兜底：整个扫描器在独立线程内运行，调用方到点即放弃等待。
    // 该线程即便仍阻塞在不可取消的内核 IO 上也只是被 detach（每扫描器至多一个、
    // 数量有界、进程退出即回收），绝不阻塞 run_full_scan 的模块 join。
    #[cfg(target_os = "macos")]
    let budget = scanner_budget(scope);
    #[cfg(not(target_os = "macos"))]
    let budget = std::time::Duration::from_secs(150);

    match scanner::scan_with_timeout(budget, job) {
        Some(items) => (items, false),
        None => {
            maclean_core::logger::warn(&format!(
                "[scan] 扫描器 {scope} 超过 {budget:?} 仍未完成，本轮返回已统计到的部分（可能偏小）；多见于磁盘繁忙、不可达网络卷或被系统拒绝访问的目录。"
            ));
            (Vec::new(), true)
        }
    }
}

/// 分段运行应用缓存扫描（macOS）：6 个来源各拿独立预算，超时只丢该段。
///
/// 返回 `(合并去重排序后的项, 是否有任一段未完成)`。总墙钟设上限兜底，单段再设上限，
/// 避免最重的一段在高负载下吃光全部预算、拖死其余本来很快的缓存来源。
#[cfg(target_os = "macos")]
fn run_app_cache_segmented() -> (Vec<ScanItem>, bool) {
    use std::time::{Duration, Instant};
    if !scanner::app_cache::home_available() {
        // home 不可得：正常的「空结果」，不算繁忙导致的部分结果。
        return (Vec::new(), false);
    }
    const TOTAL_BUDGET: Duration = Duration::from_secs(150);
    const PER_SEG_CAP: Duration = Duration::from_secs(45);

    let start = Instant::now();
    let mut items: Vec<ScanItem> = Vec::new();
    let mut partial = false;

    for (seg_name, seg_fn) in scanner::app_cache::scan_segments() {
        let elapsed = start.elapsed();
        if elapsed >= TOTAL_BUDGET {
            partial = true;
            maclean_core::logger::warn(&format!(
                "[scan] app_cache 分段总预算 {TOTAL_BUDGET:?} 已耗尽，剩余段（含 {seg_name}）本轮跳过；已完成分段照常返回。"
            ));
            break;
        }
        let budget = TOTAL_BUDGET.saturating_sub(elapsed).min(PER_SEG_CAP);
        match scanner::scan_with_timeout(budget, move || seg_fn()) {
            Some(part) => items.extend(part),
            None => {
                partial = true;
                maclean_core::logger::warn(&format!(
                    "[scan] app_cache 分段 {seg_name} 超过 {budget:?} 未完成，本轮只跳过该段，其它分段结果保留。"
                ));
            }
        }
    }

    // 与正常 scan 一致的去重 + 排序（分段缺失时去重仍安全：只少项、不重复计）。
    (scanner::app_cache::finish_items(items), partial)
}

#[cfg(target_os = "macos")]
fn run_all_scanners(app: &AppHandle) -> (Vec<ScanItem>, bool) {
    // 阶段顺序模拟交互稿的 9 阶段提示；每个扫描器独立 catch，单个挂了不拖垮全部。
    let stages = [
        ("dev_cache", "开发者缓存", 20u8),
        ("app_cache", "应用缓存", 55),
        ("app_data", "应用数据与残留", 85),
    ];
    let mut all = Vec::new();
    let mut partial = false;
    for (key, label, pct) in stages {
        let _ = app.emit(
            "scan-progress",
            serde_json::json!({ "stage": key, "label": label, "pct": pct }),
        );
        // 逐阶段累积：某阶段因高负载只拿到部分结果时，前序阶段的结果不丢。
        let (part, p) =
            std::panic::catch_unwind(AssertUnwindSafe(|| run_scanner(key))).unwrap_or_default();
        all.extend(part);
        partial |= p;
    }
    let _ = app.emit(
        "scan-progress",
        serde_json::json!({ "stage": "done", "label": "完成", "pct": 100 }),
    );
    (all, partial)
}

/// 全量体检各模块在整体进度中的权重（合计 100，反映相对耗时；large 最重）。
#[cfg(target_os = "macos")]
fn module_weight(key: &str) -> u8 {
    match key {
        "all" => 40,
        "large" => 25,
        "dup" => 20,
        "apps" => 15,
        _ => 0,
    }
}

#[cfg(target_os = "macos")]
fn module_bit(key: &str) -> u32 {
    match key {
        "all" => 1,
        "large" => 2,
        "dup" => 4,
        "apps" => 8,
        _ => 0,
    }
}

/// 在全量体检中运行单个模块：开始 / 完成各发一次 `scan-module` 状态事件，
/// 整体进度按已完成模块权重**单调累加**（模块并行完成顺序不定也不会回退）。
/// panic 按模块隔离，不拖垮其它线程。
#[cfg(target_os = "macos")]
fn run_module<F>(
    app: &AppHandle,
    partial_scopes: &std::sync::Mutex<Vec<String>>,
    done_bits: &std::sync::atomic::AtomicU32,
    key: &str,
    label: &str,
    f: F,
) -> Vec<ScanItem>
where
    F: FnOnce() -> (Vec<ScanItem>, bool),
{
    use std::sync::atomic::Ordering;
    // 按已完成模块位图计算整体进度（权重合计，最小 2%），模块并行完成顺序不定也不回退
    let pct_of = |bits: u32| -> u8 {
        ["all", "large", "dup", "apps"]
            .iter()
            .filter(|k| bits & module_bit(k) != 0)
            .map(|k| module_weight(k))
            .sum::<u8>()
            .max(2)
    };
    let _ = app.emit(
        "scan-module",
        serde_json::json!({ "scope": key, "status": "start", "items": [] }),
    );
    let _ = app.emit(
        "scan-progress",
        serde_json::json!({ "stage": "full", "label": format!("正在扫描：{label}"), "pct": pct_of(done_bits.load(Ordering::Relaxed)) }),
    );

    // f 返回 (本模块项目, 是否部分结果)；panic 兜底为（空、非部分）。
    let (items, partial) =
        std::panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| (Vec::new(), false));

    let _ = app.emit(
        "scan-module",
        serde_json::json!({ "scope": key, "status": "done", "items": items, "partial": partial }),
    );
    if partial {
        if let Ok(mut g) = partial_scopes.lock() {
            g.push(label.to_string());
        }
    }
    let bits = done_bits.fetch_or(module_bit(key), Ordering::AcqRel) | module_bit(key);
    let _ = app.emit(
        "scan-progress",
        serde_json::json!({ "stage": "full", "label": format!("已完成：{label}"), "pct": pct_of(bits) }),
    );
    items
}

/// 全量体检：**三个核心模块并行**扫描，扫描在后台进行、可自由切页。
///
/// 包含：缓存与应用数据（`all`）、磁盘大文件（`large`）、已安装应用（`apps`）。
/// **刻意不含重复文件（`dup`）**：重复文件要对候选文件逐个做内容哈希
/// （SHA256），是天然重计算，真机实测在重开发机上单模块就要数百秒；纳入
/// 全量会长时间拖住"体检完成"，并与缓存扫描争抢磁盘 IO。因此 dup 仅在
/// 「重复文件」页按需单独扫描（`run_scanner("dup")`，结果同样跨页复用）。
///
/// 真机实测补充：`all` 内部（dev/app 缓存与应用数据）若再做激进的顶层多线程
/// 并行，会在 IO bound 的海量小文件场景互相争抢，反而把本可秒级完成的 `apps`
/// 拖到数百秒；因此 `all` 内部维持原有串行（其目录大小统计 `dir_size_impl`
/// 自身已是分层 rayon 并行 BFS），只在**模块级**并行，让较快的 large/apps
/// 藏进 all 的长尾，墙钟 ≈ max(三者) 而非三段相加。
///
/// 每完成一个模块就通过 `scan-module` 事件把结果推给前端，前端按模块即时入库；
/// 全程只读，单模块 panic 被隔离，不影响其余模块。
#[cfg(target_os = "macos")]
fn run_full_scan(app: &AppHandle) -> Vec<ScanItem> {
    use std::sync::atomic::AtomicU32;
    let done_bits = AtomicU32::new(0);
    let _ = app.emit(
        "scan-progress",
        serde_json::json!({ "stage": "full", "label": "开始全盘体检", "pct": 2 }),
    );

    let mut out: Vec<ScanItem> = Vec::new();
    let partial_scopes = std::sync::Mutex::new(Vec::<String>::new());
    std::thread::scope(|scope| {
        // 模块 all = 开发者缓存 + 应用缓存 + 应用数据与残留。
        // 三段内部维持串行（见函数文档：激进并行在 IO bound 场景无收益）；逐段保留
        // 已统计部分，任一段繁忙超时都不拖累其它段。
        let h_all = scope.spawn(|| {
            run_module(app, &partial_scopes, &done_bits, "all", "缓存与应用数据", || {
                let mut all = Vec::new();
                let mut partial = false;
                for key in ["dev_cache", "app_cache", "app_data"] {
                    let (part, p) = run_scanner(key);
                    all.extend(part);
                    partial |= p;
                }
                (all, partial)
            })
        });
        // 模块 large = 磁盘大文件 / 大目录
        let h_large = scope.spawn(|| {
            run_module(app, &partial_scopes, &done_bits, "large", "磁盘大文件", || {
                run_scanner("large")
            })
        });
        // 模块 apps = 已安装应用
        let h_apps = scope.spawn(|| {
            run_module(app, &partial_scopes, &done_bits, "apps", "已安装应用", || {
                run_scanner("apps")
            })
        });

        for h in [h_all, h_large, h_apps] {
            if let Ok(part) = h.join() {
                out.extend(part);
            }
        }
    });

    // 任一模块只拿到部分结果时，统一向前端发一次「部分结果」事件（模块名去重保序），
    // 由前端提示用户「数字可能偏小、空闲后重扫可补齐」，而不是静默显示 0 或不说明。
    let partial_labels = partial_scopes.into_inner().unwrap_or_default();
    if !partial_labels.is_empty() {
        let mut seen = std::collections::HashSet::new();
        let labels: Vec<String> = partial_labels
            .into_iter()
            .filter(|name| seen.insert(name.clone()))
            .collect();
        let _ = app.emit(
            "scan-partial",
            serde_json::json!({ "scopes": labels }),
        );
    }

    let _ = app.emit(
        "scan-progress",
        serde_json::json!({ "stage": "done", "label": "完成", "pct": 100 }),
    );
    out
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

        // 各路径结果。full 路径在 run_full_scan 内自行发 scan-partial；
        // all / 单 scope 路径在各自分支内按需发 scan-partial，故外层的 partial 不直接使用。
        let (items, _partial): (Vec<ScanItem>, bool) = if scope == "full" {
            // 全量体检：逐模块经 scan-module 事件回传结果（后台渐进式）
            #[cfg(target_os = "macos")]
            {
                (run_full_scan(&app), false)
            }
            #[cfg(not(target_os = "macos"))]
            {
                // 全量体检目前仅在 macOS 提供多模块；其它平台退化为开发者缓存
                let v = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let mut r = scanner::dev_cache::DevCacheScanner::new().scan();
                    post_check(&mut r.items);
                    r.items
                }))
                .unwrap_or_default();
                (v, false)
            }
        } else if scope == "all" {
            #[cfg(target_os = "macos")]
            {
                let (v, p) = run_all_scanners(&app);
                if p {
                    let _ = app.emit(
                        "scan-partial",
                        serde_json::json!({ "scopes": ["缓存与应用数据"] }),
                    );
                }
                (v, p)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let v = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let mut r = scanner::dev_cache::DevCacheScanner::new().scan();
                    post_check(&mut r.items);
                    r.items
                }))
                .unwrap_or_default();
                (v, false)
            }
        } else {
            // 单扫描器（dup / large / apps / dev_cache…）：繁忙时返回部分结果。
            let (v, p) =
                std::panic::catch_unwind(AssertUnwindSafe(|| run_scanner(&scope))).unwrap_or_default();
            if p {
                let _ = app.emit(
                    "scan-partial",
                    serde_json::json!({ "scopes": [scope_label(&scope)] }),
                );
            }
            (v, p)
        };

        let _ = app.emit(
            "scan-progress",
            serde_json::json!({ "stage": "done", "label": "完成", "pct": 100 }),
        );
        // 扫描收尾：把本轮只读「大目录」占用分析写入持久缓存（无改动则空操作），
        // 使下次扫描未变化的大目录秒回；缓存仅作用于只读分析，不触碰删除判定。
        maclean_core::scanner::sizecache::flush();
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
    /// safe / cache_only / caution / advanced（实际序列化为 PascalCase 变体名）
    pub recommend: String,
    /// 前端按删除策略 + 本次覆盖给出的**意愿**：true=移入废纸篓，false=永久删除。
    /// 仅对安全/缓存项生效；注意/高级项在 [`clean_execute`] 内被强制改为 true。
    #[serde(default = "default_use_trash")]
    pub use_trash: bool,
}

/// 旧前端 / 缺省字段时按「移入废纸篓」处理（更安全的缺省）。
fn default_use_trash() -> bool {
    true
}

/// 后端权威裁决某项最终是否进废纸篓（前端不可信，纵深防御）。
///
/// 采用**白名单**而非黑名单：只有等级**明确**为 `Safe` / `CacheOnly` 的项
/// 才允许在请求永久删除时照办；注意 / 高级、以及任何无法识别 / 缺失 /
/// 大小写异常的等级一律落入废纸篓（默认保守）。这样即使前端被篡改传入
/// 伪造等级，风险项也不可能被永久删除。
///
/// - 注意/高级/未知：恒为 `true`（即使前端请求永久删除）；
/// - 安全/缓存：尊重前端按「删除策略 + 本次覆盖」给出的意愿。
fn resolve_use_trash(recommend: &str, requested: bool) -> bool {
    let explicitly_safe = matches!(recommend.trim(), "Safe" | "CacheOnly");
    requested || !explicitly_safe
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
            // 聚合项（.DS_Store / __pycache__ / Monorepo 等）：`path` 只是一段描述文字
            // （如 "~/ 下的 .DS_Store 文件 (11 个)"），并非真实文件，真正要删的是
            // `batch_paths` 里的每个成员。执行层 ops 对批量项也是“逐个成员过安全闸门，
            // 命中保护的成员单独跳过、其余照常删”。预览必须与执行同口径，不能拿那段并不
            // 存在的描述路径去过 check_deletable（必然返回不存在→整组被误拦，历史上导致
            // “仅选安全项却提示 1 项未通过安全检查”）。
            let (allowed, reason) = if !it.batch_paths.is_empty() {
                let (safe_n, blocked_n) =
                    it.batch_paths
                        .iter()
                        .fold((0usize, 0usize), |(s, b), bp| {
                            match safety::check_path_safety_with_category(bp, &it.category) {
                                SafetyCheck::Safe => (s + 1, b),
                                // Danger/Warning 成员在执行层都会被跳过，不计入可删成员
                                _ => (s, b + 1),
                            }
                        });
                if safe_n == 0 {
                    (false, "该组所有成员均未通过安全检查".to_string())
                } else if blocked_n > 0 {
                    (
                        true,
                        format!("{blocked_n} 个成员位于受保护目录，清理时将自动跳过"),
                    )
                } else {
                    (true, String::new())
                }
            } else {
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
                // 删除方式以后端裁决为准（前端不可信）：
                // - 注意/高级（含 IM 孤儿残留等）永远移入废纸篓，即使前端传 false；
                // - 安全/缓存项才尊重前端按「删除策略 + 本次覆盖」给出的意愿。
                let use_trash = resolve_use_trash(&i.recommend, i.use_trash);
                (
                    i.path.clone(),
                    i.category.clone(),
                    i.batch_paths.clone(),
                    use_trash,
                    i.size_bytes,
                )
            })
            .collect();

        let cancel = Arc::new(AtomicBool::new(false));
        let mut delete_rx: Option<std::sync::mpsc::Receiver<DeleteMessage>> = None;
        // 进度总数需在 to_delete 被 start_delete 取走所有权之前算好：
        // 每个顶层项 + 其 batch_paths 各算一个待处理路径。
        let total_paths: usize = to_delete
            .iter()
            .map(|t| 1usize + t.2.len())
            .sum::<usize>()
            .max(1);
        ops::start_delete(
            to_delete,
            lang_en,
            &mut delete_rx,
            false,
            true, // prefer_official_uninstaller
            cancel,
        );

        let mut report = CleanReport::default();
        let mut done_paths: usize = 0;
        if let Some(rx) = delete_rx {
            let emit_progress =
                |app: &AppHandle, done: usize, total: usize, rep: &CleanReport, path: &str, ok: bool| {
                    let _ = app.emit(
                        "clean-progress",
                        serde_json::json!({
                            "done": done,
                            "total": total,
                            "pct": ((done as f64 / total as f64) * 100.0).round().min(100.0),
                            "deleted": rep.deleted,
                            "intercepted": rep.intercepted,
                            "skipped": rep.skipped,
                            "path": path,
                            "ok": ok,
                        }),
                    );
                };
            while let Ok(msg) = rx.recv() {
                let mut last_path = String::new();
                let mut last_ok = true;
                let mut tick = false;
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
                        done_paths += 1;
                        last_path = path;
                        last_ok = ok;
                        tick = true;
                    }
                    DeleteMessage::Skip(..) | DeleteMessage::Info(_) => {
                        report.skipped += 1;
                        done_paths += 1;
                        tick = true;
                    }
                    DeleteMessage::NeedPassword(v) => {
                        report.need_password += v.len();
                        done_paths += v.len();
                        tick = true;
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
                if tick {
                    emit_progress(
                        &app,
                        done_paths.min(total_paths),
                        total_paths,
                        &report,
                        &last_path,
                        last_ok,
                    );
                }
            }
        }
        // 任何删除都可能改变大目录占用：丢弃只读体积缓存，使后续磁盘分析/概览立即
        // 基于当前文件系统重算，而不是复用删除前的旧数字（缓存不影响删除目标本身）。
        maclean_core::scanner::sizecache::invalidate_all();
        Ok(report)
    })
    .await
    .map_err(|e| format!("清理任务异常: {e}"))?
}

/* ============================== 应用一键卸载 ============================== */

/// 已安装应用**轻量清单**（应用卸载页首屏数据源）：元数据 + 真实图标，
/// **不含体积**，打开页面秒回。体积由 [`apps_sizes`] 在后台补算。
///
/// 历史上这里直接跑完整 `list_installed_apps()`，会对每个应用的本体及全部
/// 关联目录（微信 / Telegram 等可达数十 GB）做递归统计，冷跑约 80 秒并把
/// 磁盘 IO 打满，表现为「点击应用卸载卡死」。现拆成两阶段：本命令只做
/// plist 读取与图标提取（秒级），重 IO 的体积统计移到 [`apps_sizes`]。
#[tauri::command]
pub async fn apps_inventory() -> Result<Vec<scanner::uninstall::InstalledApp>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        {
            Ok(scanner::uninstall::list_installed_apps_light())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Vec::new())
        }
    })
    .await
    .map_err(|e| format!("应用清单加载异常: {e}"))?
}

/// 后台补算应用本体 / 数据 / 缓存体积，并**逐条流式推送**。
///
/// 重 IO。前端在轻量清单渲染后调用；后端并行统计，**每算完一个应用就 emit
/// 一条 `app-size` 事件**（payload 为单个 `AppInventorySize`），因此小应用
/// 先出体积、微信 / Telegram 这类数十 GB 的大目录最后才更新，不会因为最慢
/// 的一个而让整屏停留在「统计中…」。命令最终 resolve 全量结果作为完成信号。
#[tauri::command]
pub async fn apps_sizes(
    app: AppHandle,
    paths: Vec<String>,
) -> Result<Vec<scanner::uninstall::AppInventorySize>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        {
            use std::time::Duration;
            // IO 拥塞时过多线程只会互相拖累：并发按核数限制在 2..=6。
            let concurrency = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(2, 6);
            // 整批墙钟预算：磁盘繁忙时，算不完的应用在前端标注「繁忙跳过」，
            // 不让页面无限转圈；机器空闲时全部远早于预算完成、提前返回。
            let budget = Duration::from_secs(20);
            let completed = scanner::uninstall::app_inventory_sizes_bounded(
                paths,
                concurrency,
                budget,
                move |item| {
                    let _ = app.emit("app-size", &item);
                },
            );
            Ok(completed)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app, paths);
            Ok(Vec::new())
        }
    })
    .await
    .map_err(|e| format!("应用体积统计异常: {e}"))?
}

/// 应用一键卸载：应用本体 + 关联数据 + 关联缓存一次清理。
///
/// 安全边界全在 core 内：路径白名单（仅 /Applications 与 ~/Applications
/// 下的 .app）、保护级别拦截、官方卸载器优先（读配置开关）、删除逐成员过
/// safety 闸门并移入废纸篓（M-2 备份清单）。前端传入的路径不被信任，
/// 删除前在 Rust 侧重新校验。
#[tauri::command]
pub async fn app_uninstall(app_path: String, lang_en: bool) -> Result<ops::UninstallAppReport, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<ops::UninstallAppReport, String> {
        // 官方卸载器开关来自 core 配置（设置页同一把锁）
        let prefer_official_uninstaller =
            config::load_config().settings_prefer_official_uninstaller;
        Ok(ops::uninstall_app(&app_path, lang_en, prefer_official_uninstaller))
    })
    .await
    .map_err(|e| format!("卸载任务异常: {e}"))?
}

/* ============================== 日志 ============================== */

#[derive(Debug, Serialize)]
pub struct LogFileDto {
    pub name: String,
    pub size_bytes: u64,
}

/// 列出日志目录下的 maclean_*.log（新→旧）。
#[tauri::command]
pub fn logs_list() -> Vec<LogFileDto> {
    let dir = maclean_core::logger::log_dir();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("maclean_") && name.ends_with(".log") {
                let size_bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(LogFileDto { name, size_bytes });
            }
        }
    }
    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

/// 只接受日志目录内的合法日志文件名，拒绝路径穿越 / 任意文件读取。
fn resolve_log_file(name: Option<&str>) -> Option<PathBuf> {
    let dir = maclean_core::logger::log_dir();
    if let Some(name) = name {
        if name.contains('/')
            || name.contains('\\')
            || name.contains("..")
            || !name.starts_with("maclean_")
            || !name.ends_with(".log")
        {
            return None;
        }
        let p = dir.join(name);
        return if p.is_file() { Some(p) } else { None };
    }
    maclean_core::logger::latest_log_file()
}

/// 读取日志尾部（最多约 512KB），避免超大日志一次性灌入 WebView。
/// 截断时从下一个换行开始，避免切到半行。
#[tauri::command]
pub fn logs_read(name: Option<String>) -> Result<String, String> {
    let path = resolve_log_file(name.as_deref()).ok_or("没有可查看的日志文件")?;
    let bytes = std::fs::read(&path).map_err(|e| format!("读取日志失败: {e}"))?;
    const MAX: usize = 512 * 1024;
    let text = if bytes.len() > MAX {
        let mut start = bytes.len() - MAX;
        while start < bytes.len() && bytes[start] != b'\n' {
            start += 1;
        }
        if start < bytes.len() {
            start += 1;
        }
        String::from_utf8_lossy(&bytes[start..]).to_string()
    } else {
        String::from_utf8_lossy(&bytes).to_string()
    };
    Ok(text)
}

/// 在访达中定位最新日志文件（无日志则打开日志目录）。仅 macOS。
#[tauri::command]
pub fn logs_reveal() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let dir = maclean_core::logger::log_dir();
        let target = maclean_core::logger::latest_log_file().unwrap_or_else(|| dir.clone());
        let status = if target.is_file() {
            std::process::Command::new("open")
                .arg("-R")
                .arg(&target)
                .status()
        } else {
            std::process::Command::new("open")
                .arg(&dir)
                .status()
        };
        status
            .map_err(|e| format!("无法打开访达: {e}"))?
            .success()
            .then_some(())
            .ok_or_else(|| "打开访达失败".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("当前平台暂不支持图形定位日志目录".to_string())
    }
}

/* ======================= IM 保护：打开应用 / 只读占用分析 ======================= */

/// 按 bundle id 打开本机应用（IM 数据受保护，引导用户去 App 内清理时使用）。
///
/// 只读/启动型操作，不触碰任何数据。bundle id 做严格字符白名单校验，
/// 且通过 `open -b` 参数传递、不经 shell，杜绝注入。
#[tauri::command]
pub fn app_open(bundle_id: String) -> Result<(), String> {
    if bundle_id.is_empty()
        || bundle_id.len() > 255
        || !bundle_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err("非法的应用标识".to_string());
    }

    if cfg!(target_os = "macos") {
        let status = std::process::Command::new("open")
            .arg("-b")
            .arg(&bundle_id)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("无法启动打开操作: {e}"))?;
        if status.success() {
            maclean_core::log_scan_step(&format!("已请求打开应用: {}", bundle_id));
            Ok(())
        } else {
            Err(format!("应用未安装或无法打开（{bundle_id}）"))
        }
    } else {
        Err("当前系统暂不支持直接打开应用".to_string())
    }
}

/// 在访达（Finder）中定位并选中某个条目。
///
/// 仅用于引导用户**自己**处理受保护数据：当前严格白名单，只允许受支持
/// IM（微信 / QQ / 企微）的 `Data/Documents` 根，其它路径一律拒绝，避免
/// 成为"打开/触碰任意路径"的通用入口。
///
/// 用 `open -R` 交给有完全磁盘访问权限的 Finder 去显示/选中，maclean
/// 自身既不枚举也不读取该目录（规避无 FDA 时的 TCC 拒绝）；参数直传
/// argv、不经 shell，杜绝命令注入。
#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    let _im = maclean_core::im_data::im_app_for_docs_path(p).ok_or_else(|| {
        "仅支持在访达中打开受保护 IM（微信 / QQ / 企业微信）的 Documents 目录".to_string()
    })?;

    if !p.exists() {
        return Err("该目录不存在（数据可能已被移除）".to_string());
    }

    let status = std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("调用访达失败：{e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err("在访达中打开失败".to_string())
    }
}

/// 对 IM（微信/QQ/企业微信）的 Documents 根做**只读**占用分析。
///
/// 入参路径必须严格位于 `~/Library/Containers/<受支持IM>/Data/Documents`，
/// 否则拒绝；全程只统计目录大小，不删除、不修改任何内容。
#[tauri::command]
pub async fn im_breakdown(path: String) -> Result<maclean_core::im_data::ImBreakdown, String> {    // 微信 Documents 可达数十 GB、含几十万小文件，遍历是 CPU/IO 密集操作，
    // 必须放到阻塞线程池——若在主线程同步跑（旧实现），点击概览卡片后整个窗口
    // 会无响应直到统计结束，表现为“点击微信聊天数据卡顿”。
    tauri::async_runtime::spawn_blocking(move || -> Result<maclean_core::im_data::ImBreakdown, String> {
        let p = std::path::Path::new(&path);
        let app = maclean_core::im_data::im_app_for_docs_path(p)
            .ok_or_else(|| "该路径不是受支持 IM 的数据目录，已拒绝分析".to_string())?;
        maclean_core::log_scan_step(&format!(
            "IM 占用分析（只读）: {} — {}",
            app.name, path
        ));
        Ok(maclean_core::im_data::analyze(p))
    })
    .await
    .map_err(|e| format!("IM 占用分析任务执行失败: {e}"))?
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
    if let Some(v) = get("settings_delete_strategy") {
        if let Some(s) = v.as_str() {
            // 只接受两个合法值，其余一律忽略（避免写入脏配置）
            if s == "smart" || s == "trash" {
                cfg.settings_delete_strategy = s.to_string();
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn req(path: &str, batch_paths: Vec<&str>) -> CleanItemReq {
        CleanItemReq {
            path: path.to_string(),
            category: "DS_Store".to_string(),
            batch_paths: batch_paths.into_iter().map(|s| s.to_string()).collect(),
            size_bytes: 1,
            recommend: "safe".to_string(),
            use_trash: true,
        }
    }

    /// 在 HOME 下建一次性测试目录，返回其路径（测试结束自行清理）
    fn home_tmp(tag: &str) -> std::path::PathBuf {
        let p = std::path::PathBuf::from(std::env::var("HOME").unwrap())
            .join(format!(".maclean_preview_test_{}_{}", tag, std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn touch(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    /// 回归：聚合项的 path 是描述文字（文件不存在），不能因此把整组判为不可删。
    /// 只要 batch_paths 中存在可安全删除的成员，就应放行。
    #[test]
    fn preview_aggregate_uses_batch_paths_not_descriptor() {
        let root = home_tmp("ds");
        let safe1 = root.join("a/.DS_Store");
        let safe2 = root.join("b/.DS_Store");
        touch(&safe1);
        touch(&safe2);

        let out = clean_preview(vec![req(
            "~/ 下的 .DS_Store 文件 (2 个)",
            vec![
                safe1.to_str().unwrap(),
                safe2.to_str().unwrap(),
            ],
        )]);
        assert_eq!(out.len(), 1);
        assert!(out[0].allowed, "含可删成员的聚合项必须放行，旧逻辑会误拦整组");
        assert!(out[0].reason.is_empty(), "全部成员安全时不应有提示");

        // 混入一个受保护成员（node_modules 内）仍应放行，仅提示会跳过该成员
        let guarded = root.join("proj/node_modules/.DS_Store");
        touch(&guarded);
        let out2 = clean_preview(vec![req(
            "~/ 下的 .DS_Store 文件 (3 个)",
            vec![
                safe1.to_str().unwrap(),
                guarded.to_str().unwrap(),
            ],
        )]);
        assert!(out2[0].allowed, "部分成员受保护不应拖累整组，执行层会单独跳过");
        assert!(out2[0].reason.contains("跳过"), "应告知有成员被跳过: {}", out2[0].reason);

        // 所有成员都在受保护目录时才整组拒绝
        let out3 = clean_preview(vec![req(
            "~/ 下的 .DS_Store 文件 (1 个)",
            vec![guarded.to_str().unwrap()],
        )]);
        assert!(!out3[0].allowed, "没有可安全删除成员时必须拒绝");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 非聚合的普通项：SIP 系统保护路径仍必须判为不可清理（保留既有硬边界）。
    #[test]
    fn preview_plain_sip_path_blocked() {
        let item = CleanItemReq {
            path: "/System/Library/Frameworks".to_string(),
            category: "系统".to_string(),
            batch_paths: vec![],
            size_bytes: 1,
            recommend: "safe".to_string(),
            use_trash: true,
        };
        let out = clean_preview(vec![item]);
        assert!(!out[0].allowed, "SIP 系统保护路径必须拦截");
        assert!(!out[0].reason.is_empty(), "应给出拦截原因");
    }

    #[test]
    fn resolve_use_trash_forces_risky_and_unknown_into_trash() {
        // 注意/高级：即使前端请求永久删除(false)，后端也强制进废纸篓
        assert!(resolve_use_trash("Advanced", false));
        assert!(resolve_use_trash("Caution", false));
        assert!(resolve_use_trash(" Advanced ", false), "trim 后应识别为高级");
        // 未知 / 缺失 / 大小写异常等级：默认保守，一律进废纸篓
        assert!(resolve_use_trash("", false));
        assert!(resolve_use_trash("advanced", false));
        assert!(resolve_use_trash("whatever", false));
        // 明确安全/缓存：尊重前端意愿（false 才永久删除）
        assert!(!resolve_use_trash("Safe", false));
        assert!(resolve_use_trash("Safe", true));
        assert!(!resolve_use_trash("CacheOnly", false));
        assert!(resolve_use_trash("CacheOnly", true));
    }

    fn scan_item(path: &str, batch: Vec<&str>) -> ScanItem {
        ScanItem {
            path: path.to_string(),
            size_bytes: 1,
            category: "DS_Store".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            recommend: Recommend::Safe,
            description: String::new(),
            batch_paths: batch.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    /// post_check 是扫描结果出口：聚合项中未过闸的成员必须在扫描期剔除，
    /// 全部受保护时整条移除——保证安全清理执行阶段零拦截。
    #[test]
    fn post_check_purges_guarded_members_and_drops_empty() {
        let root = home_tmp("pc");
        let safe1 = root.join("a/.DS_Store");
        let bad = root.join("proj/node_modules/.DS_Store");
        touch(&safe1);
        touch(&bad);

        let mut items = vec![scan_item(
            "~/ 下的 .DS_Store 文件 (2 个)",
            vec![safe1.to_str().unwrap(), bad.to_str().unwrap()],
        )];
        post_check(&mut items);
        assert_eq!(items.len(), 1, "仍有安全成员时整条保留");
        assert_eq!(items[0].batch_paths.len(), 1, "受保护成员应被剔除");
        assert!(items[0].batch_paths[0].ends_with("a/.DS_Store"));
        assert!(items[0].deletable);

        let mut only_bad = vec![scan_item("x (1 个)", vec![bad.to_str().unwrap()])];
        post_check(&mut only_bad);
        assert!(only_bad.is_empty(), "成员全部受保护时应整条移除");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 默认可选单项若未过闸（SIP 系统路径），post_check 必须标为不可删。
    #[test]
    fn post_check_marks_unsafe_single_item_undeletable() {
        let mut items = vec![scan_item("/System/Library/Frameworks", vec![])];
        post_check(&mut items);
        assert!(!items[0].deletable, "SIP 单项必须不可删");
        assert!(!items[0].undeletable_reason.is_empty());
    }
}
