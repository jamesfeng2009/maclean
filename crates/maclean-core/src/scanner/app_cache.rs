//! App 容器缓存扫描器
//!
//! 扫描 ~/Library/Containers、~/Library/Group Containers、
//! ~/Library/Application Support、~/Library/Caches、~/Library/Logs
//! 下的应用缓存数据。

use std::path::PathBuf;
use std::time::Instant;

use rayon::prelude::*;

use super::{dir_size, has_home, home_dir, Recommend, ScanItem, ScanResult, Scanner};
use crate::im_data;

/// 50MB 阈值
const CONTAINER_MIN: u64 = 50 * 1024 * 1024;
/// 100MB 阈值
const CACHE_MIN: u64 = 100 * 1024 * 1024;
// 2026-09-18 删除了 APP_SUPPORT_MIN（500MB）：Application Support 的筛选
// 实际走 app_data.rs，这里没有引用。

/// App 缓存扫描器
#[derive(Debug, Default)]
pub struct AppCacheScanner;

impl AppCacheScanner {
    pub fn new() -> Self {
        Self
    }
}

/// 按路径去重，保留首次出现的那一项
///
/// `scan_app_support_caches` 与 `scan_browser_caches` 都用同一份
/// `CACHE_DIR_NAMES` 做匹配，Chrome 的 `Default/Cache`、`Default/GPUCache`
/// 这类目录会被两个函数各扫一次 —— 同一路径进列表两次，
/// `total_size` 直接翻倍，用户看到的"可释放空间"是虚高的。
fn dedup_by_path(items: Vec<ScanItem>) -> Vec<ScanItem> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.path.clone()))
        .collect()
}

impl Scanner for AppCacheScanner {
    fn scan(&self) -> ScanResult {
        let start = Instant::now();
        let mut items = Vec::new();

        // home 获取不到时不要硬扫（否则会退化为扫描系统目录）
        if !has_home() {
            return ScanResult {
                items: Vec::new(),
                total_size: 0,
                scan_time_ms: 0,
            };
        }

        items.extend(scan_containers());
        items.extend(scan_group_containers());
        items.extend(scan_app_support_caches());
        items.extend(scan_system_caches());
        items.extend(scan_logs());
        items.extend(scan_browser_caches());

        // 按路径去重（保留首次出现的那一项）
        //
        // scan_app_support_caches 与 scan_browser_caches 都用 CACHE_DIR_NAMES
        // 做匹配，Chrome 的 Default/Cache、Default/GPUCache 这类目录会被两个
        // 函数各扫一次 —— 同一路径进列表两次，total_size 直接翻倍，
        // 用户看到的"可释放空间"是虚高的。
        items = dedup_by_path(items);

        // 按大小降序排列
        items.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));

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
//  ~/Library/Containers
// =========================================================================

/// 扫描 IM 应用容器的 Documents 占用（只读、受保护）。
///
/// 只处理 [`im_data::IM_APPS`] 白名单内的微信 / QQ / 企业微信：
/// - 路径精确为 `~/Library/Containers/<bundle>/Data/Documents`；
/// - 一律 `deletable=false`、标"聊天数据（受保护）"，引导到 App 内存储空间管理；
/// - 点击后的细分占用由 `im_data::analyze`（`im_breakdown` 命令）按需完成。
///
/// **刻意不再无差别广扫 `~/Library/Containers`**：旧实现列出全部容器后对每个并行
/// `dir_size`，在未获全盘访问授权时会对大量其它 App 沙盒目录逐个撞 TCC（readdir/
/// stat 在内核卡到超时），上百个容器足以耗尽模块预算、把整个"应用缓存"拖到超时。
/// 其它沙盒 App 容器无授权、删除高危，不扫；Docker/OrbStack 等由 dev_cache 专门
/// 通道只读展示。
fn scan_containers() -> Vec<ScanItem> {
    let home = home_dir();
    im_data::IM_APPS
        .par_iter()
        .filter_map(|im| {
            let docs_dir = home
                .join("Library/Containers")
                .join(im.bundle)
                .join("Data")
                .join("Documents");
            // 交给带超时的 dir_size：不存在 / 无授权 / IO 卡都在有界时间内返回 0 或
            // 部分值，故此处不再先做会裸卡在内核的 is_dir 探测。
            let docs_size = dir_size(&docs_dir);
            if docs_size < CONTAINER_MIN {
                return None;
            }
            Some(ScanItem {
                path: docs_dir.to_string_lossy().to_string(),
                size_bytes: docs_size,
                category: format!("{} 聊天数据（受保护）", im.name),
                selected: false,
                deletable: false,
                undeletable_reason: format!(
                    "{}聊天数据受保护，直接删除会丢失聊天记录与收发文件且难以恢复，请到{}内清理",
                    im.name, im.name
                ),
                batch_paths: Vec::new(),
                recommend: Recommend::Advanced,
                description: format!(
                    "{}的聊天记录、图片/视频与收到的文件保存在此。建议在{}中按会话管理；maclean 只做只读分析，不直接删除这些数据。",
                    im.name, im.storage_hint
                ),
            })
        })
        .collect()
}

// =========================================================================
//  ~/Library/Group Containers
// =========================================================================

/// 组容器通用缓存扫描**已停用**（返回空）。
///
/// `~/Library/Group Containers` 是各 App 的共享容器，未授权时同样逐个撞 TCC、很难
/// 读到；其内容又常含登录态 / 共享配置，通用"Caches 可删"风险高。OrbStack 等组容器
/// 占用由 dev_cache 专门通道只读展示，IM 共享数据体现在其 Documents 占用与
/// `im_data::analyze` 的细分里。保留空函数以维持 `scan()` 调用点稳定。
fn scan_group_containers() -> Vec<ScanItem> {
    Vec::new()
}

// =========================================================================
//  ~/Library/Application Support — 递归扫描缓存子目录
// =========================================================================

/// 已知的缓存/临时目录名（不区分大小写匹配）
///
/// 这些是各类 App 在 Application Support 下创建的缓存子目录，
/// 删除后 App 会自动重建，不影响用户数据。
/// 参考 Mole (https://github.com/tw93/Mole) 的 app_caches.sh、dev.sh、
/// user.sh 中数百条 safe_clean 路径归纳而来，覆盖：
/// - Electron / Chromium 应用（VS Code, Discord, Slack, Teams 等）
/// - 浏览器（Chrome, Brave, Arc, Vivaldi, Firefox, Yandex 等）
/// - 游戏（Steam, Battle.net, Minecraft, RPCS3 等）
/// - 设计/媒体（Adobe, Sketch, Figma, Final Cut Pro 等）
/// - 通信（微信, QQ, DingTalk, Telegram 等）
/// - 开发工具（Claude, Antigravity, Qoder 等）
const CACHE_DIR_NAMES: &[&str] = &[
    // === 通用缓存 ===
    "Cache",
    "Caches",
    "cache",
    "caches",
    "CachedData",
    "CachedExtensions",
    "CachedExtensionVSIXs",
    "Cache_Data",
    "CacheData",
    "CacheStorage",
    "cacheStorage",
    "Application Cache",
    "application_cache",
    // === Electron / Chromium 缓存 ===
    "Code Cache",
    "CodeCache",
    "code_cache",
    "GPUCache",
    "gpu_cache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "DawnCache",
    "GrShaderCache",
    "GraphiteDawnCache",
    "ShaderCache",
    "shader_cache",
    "shadercache",
    // 注意：Service Worker / ServiceWorker / WebStorage 不在此列表
    // 这些目录包含用户数据（PWA 注册信息、Web Storage），不应被清理工具触碰
    // 浏览器缓存清理由 scan_browser_caches() 专门处理 Service Worker/CacheStorage 子目录
    "Crashpad", // Chromium 崩溃报告（Crashpad/completed）
    // === 浏览器特有缓存 ===
    "cache2", // Firefox profile cache
    "component_crx_cache",
    "extensions_crx_cache",
    "crx_cache",
    "OptGuideOnDeviceModel",
    "OptGuideOnDeviceClassifierModel",
    // === Web 缓存 ===
    "webcache",
    "webcache2",
    "browser_cache",
    "BrowserCache",
    "NetworkCache",
    "network_cache",
    // === 日志 ===
    "logs",
    "Logs",
    "log",
    "holmeslogs", // DingTalk
    // === 临时文件 ===
    "tmp",
    "Temp",
    "temp",
    "T",
    // === 崩溃报告 ===
    "crash-reports",
    "CrashReporter",
    "CrashReports",
    "SentryCrash",
    "sentry-crash",
    "sentry",
    // === 媒体缓存 ===
    "thumbnails",
    "thumbnail-cache",
    "Media Cache Files",
    "MediaCache",
    "videoCache",
    "VideoCache",
    "CacheClip", // DaVinci Resolve
    // === 游戏 / 工具缓存 ===
    "htmlcache",     // Steam web cache
    "appcache",      // Steam app cache
    "depotcache",    // Steam depot cache
    "stremio-cache", // Stremio
    "DictUpdate",    // WeType 输入法
    // === 通信应用缓存（QQ Music 等）===
    "iRRCache",
    "iCache",
    "iTemp",
    "iLog",
    // === 其他 ===
    "Photos.cache", // Address Book
];

/// 递归扫描 Application Support 下的缓存子目录
///
/// 策略：用 walkdir 迭代遍历（不会栈溢出），最大深度 4 层，
/// 遇到目录名匹配已知缓存名时，计算大小并加入清理列表，
/// 不再继续递归该目录（剪枝，避免重复扫描缓存内部文件）。
fn scan_app_support_caches() -> Vec<ScanItem> {
    let home = home_dir();
    let app_support = home.join("Library/Application Support");
    let mut items = Vec::new();

    if !app_support.is_dir() {
        return items;
    }

    // 收集所有匹配的缓存目录
    let mut cache_dirs: Vec<(PathBuf, u64)> = Vec::new();

    for entry in walkdir::WalkDir::new(&app_support)
        .max_depth(4)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // 跳过隐藏目录（如 .vscode 内部不再深入）
            if e.depth() > 0 {
                // 网络/FUSE 挂载点、TCC 沙盒容器不深入（避免 readdir/stat 卡在内核）
                if e.file_type().is_dir()
                    && crate::scanner::fs_guard::should_skip_traversal(e.path())
                {
                    return false;
                }
                if let Some(name) = e.file_name().to_str() {
                    if name.starts_with('.') && e.depth() > 1 {
                        return false;
                    }
                }
            }
            true
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_dir() {
            continue;
        }
        // 跳过根目录本身
        if entry.depth() == 0 {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();

        // 检查是否匹配已知缓存目录名
        if !is_cache_dir_name(&dir_name) {
            continue;
        }

        let path = entry.path().to_path_buf();
        // 清单管理目录（venv/site-packages/node_modules 等）永不进候选，
        // 与 safety 第 4.6 层同判定 —— 缓存名匹配（log/tmp/T）可能撞上
        // App Support 下存放的项目环境，删掉即毁环境。
        if crate::safety::is_manifest_managed_path(&path) {
            continue;
        }
        let size = dir_size(&path);

        // 只展示 >10MB 的缓存目录
        if size >= 10 * 1024 * 1024 {
            cache_dirs.push((path, size));
        }
    }

    // 按大小降序排序
    cache_dirs.sort_by_key(|a| std::cmp::Reverse(a.1));
    cache_dirs.dedup_by(|a, b| a.0 == b.0);

    for (path, size) in cache_dirs {
        // 从路径中提取 App 名称（Application Support/<AppName>/.../<CacheDir>）
        let app_name = extract_app_name_from_path(&path, &app_support);
        let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("缓存");

        items.push(ScanItem {
            path: path.to_string_lossy().to_string(),
            size_bytes: size,
            category: format!("{} 缓存", app_name),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Safe,
            description: format!("{} 的 {} 目录，删除后自动重建", app_name, dir_name),
        });
    }

    items
}

/// 检查目录名是否匹配已知缓存目录名（不区分大小写）
fn is_cache_dir_name(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    CACHE_DIR_NAMES
        .iter()
        .any(|&cache_name| name_lower == cache_name.to_lowercase())
}

/// 从路径中提取 App 名称
///
/// 例如：~/Library/Application Support/Code/Cache → "Code"
///      ~/Library/Application Support/Steam/htmlcache → "Steam"
fn extract_app_name_from_path(path: &std::path::Path, app_support: &std::path::Path) -> String {
    if let Ok(rel) = path.strip_prefix(app_support) {
        if let Some(first) = rel.components().next() {
            return first.as_os_str().to_string_lossy().to_string();
        }
    }
    "未知App".to_string()
}

// =========================================================================
//  ~/Library/Caches
// =========================================================================

/// 扫描系统缓存目录
fn scan_system_caches() -> Vec<ScanItem> {
    let home = home_dir();
    let caches_dir = home.join("Library/Caches");
    let mut items = Vec::new();

    let entries = match std::fs::read_dir(&caches_dir) {
        Ok(e) => e,
        Err(_) => return items,
    };

    let paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    let sized: Vec<(PathBuf, u64)> = paths
        .par_iter()
        .filter_map(|path| {
            let size = dir_size(path);
            if size >= CACHE_MIN {
                Some((path.clone(), size))
            } else {
                None
            }
        })
        .collect();

    for (path, size) in sized {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("缓存")
            .to_string();
        items.push(ScanItem {
            path: path.to_string_lossy().to_string(),
            size_bytes: size,
            category: format!("系统缓存-{}", name),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Safe,
            description: "系统缓存目录，可安全删除".to_string(),
        });
    }

    items
}

// =========================================================================
//  ~/Library/Logs
// =========================================================================

/// 扫描系统日志
fn scan_logs() -> Vec<ScanItem> {
    let home = home_dir();
    let logs_dir = home.join("Library/Logs");
    let mut items = Vec::new();

    if logs_dir.is_dir() {
        let size = dir_size(&logs_dir);
        if size >= CACHE_MIN {
            items.push(ScanItem {
                path: logs_dir.to_string_lossy().to_string(),
                size_bytes: size,
                category: "系统日志".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "系统日志文件，可安全删除".to_string(),
            });
        }
    }

    items
}

// =========================================================================
//  浏览器缓存（可重建的缓存子目录，不触碰用户数据）
// =========================================================================

/// 扫描浏览器可重建缓存
///
/// 只清理缓存类子目录（AI 模型、GPU 缓存、Code Cache、Crashpad 等），
/// 不触碰用户数据（书签、密码、扩展、登录状态、IndexedDB 等）。
///
/// 支持浏览器：
/// - Google Chrome
/// - Microsoft Edge
/// - Brave
/// - Arc
/// - Firefox
/// - Safari
fn scan_browser_caches() -> Vec<ScanItem> {
    let home = home_dir();
    let app_support = home.join("Library/Application Support");
    let caches = home.join("Library/Caches");
    let mut items: Vec<ScanItem> = Vec::new();

    // Chromium 系浏览器共享相同的缓存子目录结构
    // 这些子目录都是可重建的缓存，删除后浏览器会自动重新生成
    let chromium_cache_subdirs: &[&str] = &[
        "OptGuideOnDeviceModel",           // Chrome 本地 AI 模型（可重新下载）
        "OptGuideOnDeviceClassifierModel", // AI 分类模型
        "optimization_guide_model_store",  // 模型存储
        "component_crx_cache",             // 组件 CRX 缓存
        "GPUCache",                        // GPU 着色器缓存
        "GraphiteDawnCache",               // Graphite GPU 缓存
        "Crashpad",                        // 崩溃报告
        "Safe Browsing",                   // 安全浏览数据库（可重建）
        "OnDeviceHeadSuggestModel",        // 搜索建议模型
        "ZxcvbnData",                      // 密码强度评估数据
        "CertificateRevocation",           // 证书吊销列表
        "segmentation_platform",           // 分段平台数据
        "ActorSafetyLists",                // 安全列表
        "WasmTtsEngine",                   // WebAssembly TTS 引擎
    ];

    // 各浏览器在 Application Support 下的根目录
    let chromium_browsers: &[(&str, &str)] = &[
        ("Google/Chrome", "Google Chrome"),
        ("Microsoft Edge", "Microsoft Edge"),
        ("BraveSoftware/Brave-Browser", "Brave"),
        ("Arc/User Data", "Arc"),
        ("Vivaldi", "Vivaldi"),
        ("Chromium", "Chromium"),
    ];

    for (rel_path, browser_name) in chromium_browsers {
        let browser_root = app_support.join(rel_path);
        if !browser_root.is_dir() {
            continue;
        }
        items.extend(scan_chromium_cache_subdirs(
            &browser_root,
            browser_name,
            chromium_cache_subdirs,
        ));
    }

    // Firefox 缓存（不同结构）
    let firefox_profiles = app_support.join("Firefox/Profiles");
    if firefox_profiles.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&firefox_profiles) {
            for entry in entries.filter_map(|e| e.ok()) {
                let profile_path = entry.path();
                if !profile_path.is_dir() {
                    continue;
                }
                // Firefox 缓存目录
                for cache_subdir in &["cache2", "startupCache", "shader-cache", "thumbnails"] {
                    let cache_path = profile_path.join(cache_subdir);
                    if cache_path.is_dir() {
                        let size = dir_size(&cache_path);
                        if size >= 10 * 1024 * 1024 {
                            // 10MB 阈值
                            let profile_name = profile_path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();
                            items.push(ScanItem {
                                path: cache_path.to_string_lossy().to_string(),
                                size_bytes: size,
                                category: "浏览器缓存".to_string(),
                                selected: false,
                                deletable: true,
                                undeletable_reason: String::new(),
                                batch_paths: Vec::new(),
                                recommend: Recommend::Safe,
                                description: format!(
                                    "Firefox ({}) 的 {} 缓存，可安全清理",
                                    profile_name, cache_subdir
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    // Safari 缓存
    let safari_cache = caches.join("com.apple.Safari");
    if safari_cache.is_dir() {
        let size = dir_size(&safari_cache);
        if size >= 10 * 1024 * 1024 {
            items.push(ScanItem {
                path: safari_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "浏览器缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Safari 缓存文件，可安全清理".to_string(),
            });
        }
    }

    // Safari WebKit 网络缓存
    let webkit_cache = caches.join("WebKit");
    if webkit_cache.is_dir() {
        let size = dir_size(&webkit_cache);
        if size >= 10 * 1024 * 1024 {
            items.push(ScanItem {
                path: webkit_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "浏览器缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution, // WebKit 缓存被多个 App 共享
                description: "WebKit 网络缓存（被 Safari 等 App 共享），可安全清理".to_string(),
            });
        }
    }

    items
}

/// 扫描 Chromium 系浏览器的缓存子目录
fn scan_chromium_cache_subdirs(
    browser_root: &std::path::Path,
    browser_name: &str,
    cache_subdirs: &[&str],
) -> Vec<ScanItem> {
    let mut items = Vec::new();

    // Chromium 系浏览器有多个 Profile：Default、Profile 1、Profile 2 等
    // 每个 Profile 下也有 GPUCache、Code Cache 等缓存
    let mut profile_dirs: Vec<PathBuf> = vec![browser_root.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(browser_root) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "Default" || name.starts_with("Profile ") {
                profile_dirs.push(entry.path());
            }
        }
    }

    for profile_dir in &profile_dirs {
        let profile_label = if profile_dir == browser_root {
            "根目录".to_string()
        } else {
            profile_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        };

        // 扫描该 Profile 下的缓存子目录
        for subdir in cache_subdirs {
            let cache_path = profile_dir.join(subdir);
            if !cache_path.is_dir() {
                continue;
            }
            let size = dir_size(&cache_path);
            if size < 10 * 1024 * 1024 {
                continue; // 10MB 阈值
            }
            let is_user_safe = matches!(
                *subdir,
                "OptGuideOnDeviceModel"
                    | "OptGuideOnDeviceClassifierModel"
                    | "optimization_guide_model_store"
                    | "component_crx_cache"
                    | "GPUCache"
                    | "GraphiteDawnCache"
                    | "Crashpad"
                    | "OnDeviceHeadSuggestModel"
                    | "ZxcvbnData"
                    | "WasmTtsEngine"
            );
            items.push(ScanItem {
                path: cache_path.to_string_lossy().to_string(),
                size_bytes: size,
                category: "浏览器缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: if is_user_safe {
                    Recommend::Safe
                } else {
                    Recommend::Caution
                },
                description: format!(
                    "{} ({}) 的 {} 缓存，删除后浏览器会自动重建",
                    browser_name, profile_label, subdir
                ),
            });
        }

        // Profile 级别的额外缓存（Default/Cache、Default/Code Cache 等）
        for extra in &["Cache", "Code Cache", "Service Worker/CacheStorage"] {
            let cache_path = profile_dir.join(extra);
            if cache_path.is_dir() {
                let size = dir_size(&cache_path);
                if size >= 10 * 1024 * 1024 {
                    items.push(ScanItem {
                        path: cache_path.to_string_lossy().to_string(),
                        size_bytes: size,
                        category: "浏览器缓存".to_string(),
                        selected: false,
                        deletable: true,
                        undeletable_reason: String::new(),
                        batch_paths: Vec::new(),
                        recommend: Recommend::Safe,
                        description: format!(
                            "{} ({}) 的 {} 缓存，删除后浏览器会自动重建",
                            browser_name, profile_label, extra
                        ),
                    });
                }
            }
        }
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::Recommend;

    fn mk(path: &str, size: u64) -> ScanItem {
        ScanItem {
            path: path.to_string(),
            size_bytes: size,
            category: "test".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            recommend: Recommend::Safe,
            description: String::new(),
            batch_paths: Vec::new(),
        }
    }

    /// P1-14：同一路径被两个子扫描器各扫一次时必须只保留一个
    ///
    /// 不去重的话 Chrome 的 Default/Cache、Default/GPUCache 会各出现两次，
    /// 界面上看着是两个不同的 1GB 目录，实际是同一个 —— 加起来就翻倍了。
    #[test]
    fn dedup_removes_repeated_paths() {
        let items = vec![
            mk("/Users/j/Library/Caches/Google/Chrome/Default/Cache", 1000),
            mk(
                "/Users/j/Library/Caches/Google/Chrome/Default/GPUCache",
                500,
            ),
            mk("/Users/j/Library/Caches/Google/Chrome/Default/Cache", 1000),
        ];
        let out = dedup_by_path(items);
        assert_eq!(out.len(), 2, "重复路径必须被去掉");
        assert_eq!(
            out[0].path,
            "/Users/j/Library/Caches/Google/Chrome/Default/Cache"
        );
        assert_eq!(
            out[1].path,
            "/Users/j/Library/Caches/Google/Chrome/Default/GPUCache"
        );
        assert_eq!(
            out.iter().map(|i| i.size_bytes).sum::<u64>(),
            1500,
            "总大小不能翻倍"
        );
    }

    #[test]
    fn dedup_keeps_distinct_paths_in_order() {
        let items = vec![mk("/a", 1), mk("/b", 2), mk("/a", 3)];
        let out = dedup_by_path(items);
        assert_eq!(
            out.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(),
            vec!["/a", "/b"]
        );
        // 保留的是首次出现那一项（大小 1，不是后来的 3）
        assert_eq!(out[0].size_bytes, 1);
    }

    #[test]
    fn dedup_handles_empty() {
        assert!(dedup_by_path(Vec::new()).is_empty());
    }
}
