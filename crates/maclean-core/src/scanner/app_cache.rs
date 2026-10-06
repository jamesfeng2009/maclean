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
/// 语义化命名清理项的最小展示阈值（微信缓存 / 飞书渲染缓存 / Go构建缓存等）
pub(crate) const NAMED_CACHE_MIN: u64 = 10 * 1024 * 1024;
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

        // 逐段扫描（与 scan_segments 同源）；commands 层的分段部分结果模式也复用这些段。
        for (_label, seg) in scan_segments() {
            items.extend(seg());
        }
        items = finish_items(items);

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        let scan_time_ms = start.elapsed().as_millis() as u64;

        ScanResult {
            items,
            total_size,
            scan_time_ms,
        }
    }
}

/// 扫描段类型：`(段标识, 扫描函数)`
type Segment = (&'static str, fn() -> Vec<ScanItem>);

/// 应用缓存的各独立扫描段：`(段标识, 扫描函数)`。
///
/// 每段彼此独立、可在各自的墙钟预算下运行；高磁盘负载时某一段即便超时也只丢该段，
/// 其余段结果照常合并（部分结果），不再因整扫描器到点而全部归零。段函数都是无捕获
/// 的 `fn`，可安全跨线程移交。分段结果合并后务必调用 [`finish_items`] 做与
/// [`AppCacheScanner::scan`] 一致的去重 / 排序。
pub fn scan_segments() -> [Segment; 7] {
    [
        // 语义化命名清理项放最前：与通用扫描（system_caches 等）命中同一路径时，
        // dedup 按「保留首次出现项」规则保住带应用语义名 + 风险等级的条目。
        ("named_app_caches", scan_named_app_caches),
        ("im_containers", scan_containers),
        ("group_containers", scan_group_containers),
        ("app_support_caches", scan_app_support_caches),
        ("system_caches", scan_system_caches),
        ("logs", scan_logs),
        ("browser_caches", scan_browser_caches),
    ]
}

/// home 目录是否可用：逐段扫描前先判空，home 不可得时不硬扫（避免退化为扫系统目录）。
pub fn home_available() -> bool {
    has_home()
}

/// 对分段（或整体）扫描得到的原始项做**与正常 scan 一致**的收尾：按路径去重
/// （保留首次出现项）后按大小降序排列。
///
/// 必须去重：`scan_app_support_caches` 与 `scan_browser_caches` 都用缓存目录名匹配，
/// Chrome 的 `Default/Cache`、`Default/GPUCache` 会被两段各扫一次，不去重则
/// `total_size` 翻倍、可释放空间虚高。分段超时导致其中一段缺失时去重仍安全（只少项、
/// 不重复计）。
pub fn finish_items(mut items: Vec<ScanItem>) -> Vec<ScanItem> {
    items = dedup_by_path(items);
    items.sort_by_key(|i| std::cmp::Reverse(i.size_bytes));
    items
}

// =========================================================================
//  语义化命名清理项（对标 MangoDisk 的 WeChat/Lark/Go build 粒度）
// =========================================================================

/// 语义化命名清理项段：对已知应用的关键缓存做**定点**扫描（不扫全盘），
/// 每个应用一个语义命名项 + 风险等级 + 说明文案。
///
/// 覆盖（与本机是否安装无关，路径存在即展示）：
/// - 微信缓存 / 微信日志：`~/Library/Containers/com.tencent.xinWeChat/Data` 下
///   的 `Cache`、`Cache_Data`、`GPUCache`、`DawnCache` 与 `Documents/app_data/log`。
///   容器受 TCC 保护，**不走 walkdir**（fs_guard 会拦截 Containers），改用手动
///   定深 read_dir + 有界 `dir_size`；无授权时读不到只丢项、不卡段。
/// - 飞书 / Lark 渲染缓存：`~/Library/Application Support/LarkShell/aha/users/*`
///   下的 `DawnCache` / `GPUCache` / `Code Cache` / `Cache` 等（Electron 渲染缓存）。
///   LarkShell 是普通目录，可安全 walkdir（深度有界）。
/// - Go 构建缓存：`~/Library/Caches/go-build`（此前仅覆盖 `~/go/pkg/mod` 模块缓存）。
/// - uv 缓存：`~/Library/Caches/uv`；Yarn 下载缓存：`~/Library/Caches/Yarn`、
///   `~/Library/Caches/Yarn v6`。
pub fn scan_named_app_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    // 1) 微信缓存 + 日志（容器定点，见下方 scan_wechat_caches）
    scan_wechat_caches(&mut items);
    // 2) 飞书 / Lark 渲染缓存（Application Support 定点 walkdir）
    scan_lark_caches(&mut items);
    // 3) 工具链注册表：Go build / uv / Yarn / Bun / Deno / Playwright / CocoaPods /
    //    Swift SPM / Composer / Ruby gem / Bundler（dev_cache 未覆盖的缺口）
    items.extend(super::named_catalog::scan_toolchain_rules());
    // 4) 已安装应用通用驱动：/Applications 下每个应用的 Caches / Logs / 容器缓存
    items.extend(super::named_catalog::scan_installed_app_caches());
    items
}

/// 微信缓存 + 日志：容器内定点定深枚举（Containers 被 fs_guard 拦截，不走 walkdir）。
fn scan_wechat_caches(items: &mut Vec<ScanItem>) {
    let home = home_dir();
    let data = home.join("Library/Containers/com.tencent.xinWeChat/Data");
    // 无授权 / 未安装：stat 失败直接返回（不 panic、不卡段）
    if !data.is_dir() {
        return;
    }

    // 日志（新老版本都在 Documents/app_data/log；新版另有 Documents/log）
    let log_dir = data.join("Documents/app_data/log");
    push_named_cache_item(
        items,
        &log_dir,
        "微信日志",
        "微信诊断日志与临时显示数据，删除后自动重建；聊天记录、账号与用户文件不受影响。清理前请退出微信。",
        Recommend::CacheOnly,
    );
    let log_dir2 = data.join("Documents/log");
    push_named_cache_item(
        items,
        &log_dir2,
        "微信日志",
        "微信诊断日志与临时显示数据，删除后自动重建；聊天记录、账号与用户文件不受影响。清理前请退出微信。",
        Recommend::CacheOnly,
    );

    // Documents 根下的结构（新版微信为平铺，实测）：
    // - 缓存目录直接平铺在根下：cacheDir / Caches / Cache / TPReportPluginCache；
    // - mmkv（会话配置）、xwechat_files（聊天文件库）是用户数据，**绝不**下探；
    // - 内置浏览器（radium）webview 缓存在
    //   Documents/app_data/radium/web/profiles/<profile>/Cache（深度 7 层，实测路径）；
    // - 其它目录按旧版 <uuid>/Cache/Cache_Data 结构再下一层匹配缓存名。
    let docs_root = data.join("Documents");
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    // 1) 新版平铺缓存目录
    if let Ok(rd) = std::fs::read_dir(&docs_root) {
        for entry in rd.filter_map(|e| e.ok()) {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(
                name.as_str(),
                "cacheDir" | "Caches" | "Cache" | "TPReportPluginCache"
            ) {
                let size = dir_size(&p);
                if size >= NAMED_CACHE_MIN {
                    items.push(ScanItem {
                        path: p.to_string_lossy().to_string(),
                        size_bytes: size,
                        category: "微信缓存".to_string(),
                        selected: false,
                        deletable: true,
                        undeletable_reason: String::new(),
                        batch_paths: Vec::new(),
                        recommend: Recommend::CacheOnly,
                        description: format!(
                            "微信的网页 / 图片 / 视频 / 代码等临时缓存（{}），删除后可自动重建；聊天记录与收发文件不受影响。清理前请退出微信。",
                            name
                        ),
                    });
                }
                continue;
            }
            if name.contains("xwechat")
                || name.contains("mmkv")
                || name.contains("Files")
                || name.contains("confsdk")
                || name.contains("app_data")
            {
                continue;
            }
            // 3) 旧版 <uuid> 用户根：下一层匹配缓存目录名
            if let Ok(rd2) = std::fs::read_dir(&p) {
                for e in rd2.filter_map(|e| e.ok()) {
                    let cp = e.path();
                    if !cp.is_dir() {
                        continue;
                    }
                    let cname = e.file_name().to_string_lossy().to_string();
                    if !is_cache_dir_name(&cname) {
                        continue;
                    }
                    if !seen.insert(cp.clone()) {
                        continue;
                    }
                    let size = dir_size(&cp);
                    if size >= NAMED_CACHE_MIN {
                        items.push(ScanItem {
                            path: cp.to_string_lossy().to_string(),
                            size_bytes: size,
                            category: "微信缓存".to_string(),
                            selected: false,
                            deletable: true,
                            undeletable_reason: String::new(),
                            batch_paths: Vec::new(),
                            recommend: Recommend::CacheOnly,
                            description: format!(
                                "微信的网页 / 图片 / 视频 / 代码等临时缓存（{}），删除后可自动重建；聊天记录与收发文件不受影响。清理前请退出微信。",
                                cname
                            ),
                        });
                    }
                }
            }
        }
    }

    // 2) 内置浏览器（radium）webview 缓存：
    //    Documents/app_data/radium/web/profiles/<profile>/Cache|Cache_Data|GPUCache|…
    //    只下探两层（profiles → profile → 缓存目录），数量有界，绝不下探 xwechat_files。
    let radium_profiles = data.join("Documents/app_data/radium/web/profiles");
    if let Ok(rd) = std::fs::read_dir(&radium_profiles) {
        for profile in rd.filter_map(|e| e.ok()) {
            let pp = profile.path();
            if !pp.is_dir() {
                continue;
            }
            if let Ok(rd2) = std::fs::read_dir(&pp) {
                for e in rd2.filter_map(|e| e.ok()) {
                    let cp = e.path();
                    if !cp.is_dir() {
                        continue;
                    }
                    let cname = e.file_name().to_string_lossy().to_string();
                    if !is_cache_dir_name(&cname) {
                        continue;
                    }
                    if !seen.insert(cp.clone()) {
                        continue;
                    }
                    let size = dir_size(&cp);
                    if size >= NAMED_CACHE_MIN {
                        items.push(ScanItem {
                            path: cp.to_string_lossy().to_string(),
                            size_bytes: size,
                            category: "微信缓存".to_string(),
                            selected: false,
                            deletable: true,
                            undeletable_reason: String::new(),
                            batch_paths: Vec::new(),
                            recommend: Recommend::CacheOnly,
                            description: format!(
                                "微信内置浏览器的网页 / 代码渲染缓存（{}），删除后可自动重建；聊天记录与收发文件不受影响。清理前请退出微信。",
                                cname
                            ),
                        });
                    }
                }
            }
        }
    }
}

/// 飞书 / Lark 渲染缓存：`~/Library/Application Support/LarkShell` 下按缓存目录名
/// 匹配（Electron 的 DawnCache / GPUCache / Code Cache / Cache 等），深度有界。
fn scan_lark_caches(items: &mut Vec<ScanItem>) {
    let home = home_dir();
    let lark_shell = home.join("Library/Application Support/LarkShell");
    if !lark_shell.is_dir() {
        return;
    }

    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for entry in walkdir::WalkDir::new(&lark_shell)
        .max_depth(5)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() > 0 && e.file_type().is_dir() {
                if crate::scanner::fs_guard::should_skip_traversal(e.path()) {
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
        if !entry.file_type().is_dir() || entry.depth() == 0 {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if !is_cache_dir_name(&dir_name) {
            continue;
        }
        let path = entry.path().to_path_buf();
        if crate::safety::is_manifest_managed_path(&path) {
            continue;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        let size = dir_size(&path);
        if size >= NAMED_CACHE_MIN {
            items.push(ScanItem {
                path: path.to_string_lossy().to_string(),
                size_bytes: size,
                category: "飞书渲染缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::CacheOnly,
                description: format!(
                    "飞书（Lark）的网页 / 代码 / 图形渲染缓存（{}），删除后可自动重建；消息、文件、工作区与登录态不受影响。清理前请退出飞书。",
                    dir_name
                ),
            });
        }
    }
}

/// 定点推送一个命名缓存项：目录存在且体积达标才进列表。
pub(crate) fn push_named_cache_item(
    items: &mut Vec<ScanItem>,
    dir: &std::path::Path,
    category: &str,
    description: &str,
    recommend: Recommend,
) {
    if !dir.is_dir() {
        return;
    }
    let size = dir_size(dir);
    if size < NAMED_CACHE_MIN {
        return;
    }
    items.push(ScanItem {
        path: dir.to_string_lossy().to_string(),
        size_bytes: size,
        category: category.to_string(),
        selected: false,
        deletable: true,
        undeletable_reason: String::new(),
        batch_paths: Vec::new(),
        recommend,
        description: description.to_string(),
    });
}

// =========================================================================
//  ~/Library/Containers
// =========================================================================

/// 扫描 IM 应用容器的 Documents 占用（只读、受保护）。
///
/// 只处理 [`im_data::IM_APPS`] 白名单内的微信 / QQ / 企业微信：
/// - 路径精确为 `~/Library/Containers/<bundle>/Data/Documents`；
/// - **App 仍安装**：`deletable=false`、标"聊天数据（受保护）"，引导到 App
///   内存储空间管理或在访达中打开，由用户自行处理；
/// - **App 已卸载**：容器数据成为孤儿，转为可清理项（`deletable=true`、
///   `Advanced`、默认不勾选、删除时强制进废纸篓）；
/// - 是否在位通过枚举 `.app` 的 bundle id 判断（不依赖 Spotlight）；枚举
///   结果为空（异常）时保守地一律按"仍安装"保护；
/// - 点击后的细分占用由 `im_data::analyze`（`im_breakdown` 命令）按需完成。
///
/// **刻意不再无差别广扫 `~/Library/Containers`**：旧实现列出全部容器后对每个并行
/// `dir_size`，在未获全盘访问授权时会对大量其它 App 沙盒目录逐个撞 TCC（readdir/
/// stat 在内核卡到超时），上百个容器足以耗尽模块预算、把整个"应用缓存"拖到超时。
/// 其它沙盒 App 容器无授权、删除高危，不扫；Docker/OrbStack 等由 dev_cache 专门
/// 通道只读展示。
/// 判定某 IM 是否仍安装。
///
/// - `installed` 为空（一个应用都没枚举到，属枚举异常）→ `None`：无法判定，
///   调用方必须按「仍安装」保守保护，避免把受保护数据误判成孤儿；
/// - 命中该 bundle → `Some(true)`；未命中 → `Some(false)`（数据已成孤儿）。
fn im_install_state(
    im_bundle: &str,
    installed: &std::collections::HashSet<String>,
) -> Option<bool> {
    if installed.is_empty() {
        None
    } else {
        Some(installed.contains(im_bundle))
    }
}

pub fn scan_containers() -> Vec<ScanItem> {
    let home = home_dir();
    // 已安装 App 的 bundle id 集合：用于区分 IM「仍安装（受保护）」与
    // 「已卸载（孤儿数据可清理）」。空集合表示枚举异常，调用处保守保护。
    let installed = super::uninstall::installed_bundle_id_set();
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
            let path = docs_dir.to_string_lossy().to_string();
            // 空集合（一个应用都没枚举到）视为无法判定，保守按"仍安装"处理。
            let still_installed = im_install_state(im.bundle, &installed).unwrap_or(true);

            if still_installed {
                // 微信/QQ/企业微信仍安装：聊天记录库 + 收发的图片/视频/文件，
                // 物理上可删但代价极高（可能永久丢失聊天记录），标记为不可删除，
                // 只做只读展示 + 引导（App 内清理 / 访达中打开 Documents）。
                Some(ScanItem {
                    path,
                    size_bytes: docs_size,
                    category: format!("{} 聊天数据（受保护）", im.name),
                    selected: false,
                    deletable: false,
                    undeletable_reason: format!(
                        "{}仍在使用：聊天记录与收发文件受保护，建议在{}内清理或在访达中自行处理",
                        im.name, im.storage_hint
                    ),
                    batch_paths: Vec::new(),
                    recommend: Recommend::Advanced,
                    description: format!(
                        "{}的聊天记录、图片/视频与收到的文件保存在此。建议在{}中按会话管理；maclean 只做只读分析，不直接删除这些数据。",
                        im.name, im.storage_hint
                    ),
                })
            } else {
                // App 已卸载：沙盒容器里残留的聊天数据已无主，转为可清理。
                // 仍属高价值数据 → Advanced、默认不勾；删除出口在后端被强制
                // 移入废纸篓（可在清空前恢复），不做永久删除。
                Some(ScanItem {
                    path,
                    size_bytes: docs_size,
                    category: format!("{} 残留数据（App 已卸载）", im.name),
                    selected: false,
                    deletable: true,
                    undeletable_reason: String::new(),
                    batch_paths: Vec::new(),
                    recommend: Recommend::Advanced,
                    description: format!(
                        "{}已不在本机，但其聊天记录、图片/视频与收到的文件仍残留在沙盒容器中。确认不再需要后可清理，将移入废纸篓，清空前可恢复。",
                        im.name
                    ),
                })
            }
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
pub fn scan_group_containers() -> Vec<ScanItem> {
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
pub fn scan_app_support_caches() -> Vec<ScanItem> {
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
pub(crate) fn is_cache_dir_name(name: &str) -> bool {
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
pub fn scan_system_caches() -> Vec<ScanItem> {
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
pub fn scan_logs() -> Vec<ScanItem> {
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
pub fn scan_browser_caches() -> Vec<ScanItem> {
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

    #[test]
    fn im_install_state_detects_present_absent_and_unknown() {
        use std::collections::HashSet;
        let wx = "com.tencent.xinWeChat";

        // 空集合：无法判定 → None（调用方据此保守保护）
        assert_eq!(im_install_state(wx, &HashSet::new()), None);

        // 命中：仍安装
        let present: HashSet<String> =
            [wx, "com.tencent.qq", "com.something.else"].iter().map(|s| s.to_string()).collect();
        assert_eq!(im_install_state(wx, &present), Some(true));

        // 未命中：App 已卸载 → 孤儿
        let absent: HashSet<String> =
            ["com.finder.other", "com.another.app"].iter().map(|s| s.to_string()).collect();
        assert_eq!(im_install_state(wx, &absent), Some(false));
    }

    #[test]
    fn named_cache_dir_name_matches_chromium_and_im_caches() {
        // 语义化命名段复用的缓存目录名匹配，必须覆盖 Electron / 微信的典型缓存目录
        assert!(is_cache_dir_name("Cache_Data"));
        assert!(is_cache_dir_name("DawnCache"));
        assert!(is_cache_dir_name("GPUCache"));
        assert!(is_cache_dir_name("Code Cache"));
        assert!(is_cache_dir_name("cache"));
        assert!(is_cache_dir_name("Logs"));
        // 用户数据目录不得误命中
        assert!(!is_cache_dir_name("Documents"));
        assert!(!is_cache_dir_name("Messages"));
        assert!(!is_cache_dir_name("Files"));
        assert!(!is_cache_dir_name("profile_main"));
    }

    #[test]
    fn push_named_cache_item_ignores_missing_or_tiny_dirs() {
        // 定点推送：不存在的目录 / 体积不足的目录都不产生项（不触碰文件系统）
        let mut items = Vec::new();
        let missing = std::env::temp_dir().join("maclean_named_missing_xyz");
        push_named_cache_item(
            &mut items,
            &missing,
            "Go构建缓存",
            "test",
            Recommend::CacheOnly,
        );
        assert!(items.is_empty());
    }
}
