//! 开发者缓存扫描器
//!
//! 扫描各类开发者工具产生的缓存和构建产物，包括:
//! - Rust Cargo (target 目录、registry 缓存)
//! - Xcode (DerivedData、设备支持、归档、模拟器)
//! - Node.js (node_modules、pnpm/npm 缓存)
//! - Go (模块缓存)
//! - Homebrew (下载缓存)
//! - pip (包缓存)
//! - JetBrains IDE (旧版本配置、缓存)
//! - Java (Gradle 缓存/版本、Maven 仓库、build 目录)
//! - Python (pip/Conda/Poetry 缓存、__pycache__)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use walkdir::WalkDir;

use super::{dir_size, home_dir, Recommend, ScanItem, ScanResult, Scanner};
use crate::safety;

/// 开发者缓存扫描器
#[derive(Debug, Default)]
pub struct DevCacheScanner;

impl DevCacheScanner {
    /// 创建新的开发者缓存扫描器实例
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for DevCacheScanner {
    fn scan(&self) -> ScanResult {
        let start = Instant::now();
        let mut items = Vec::new();

        // 依次扫描各类开发者缓存（跨平台）
        items.extend(scan_rust_caches());
        // Xcode 缓存仅 macOS
        #[cfg(target_os = "macos")]
        items.extend(scan_xcode_caches());
        items.extend(scan_node_caches());
        items.extend(scan_go_caches());
        // Homebrew 仅 macOS
        #[cfg(target_os = "macos")]
        items.extend(scan_homebrew_caches());
        items.extend(scan_pip_caches());
        // JetBrains 跨平台（macOS 路径，Windows 上会返回空）
        #[cfg(target_os = "macos")]
        items.extend(scan_jetbrains_caches());

        // Java/Gradle/Maven 和 Python 缓存（跨平台）
        scan_java_caches(&mut items);
        scan_python_caches(&mut items);

        // 更多语言缓存（注册表驱动：37 个固定路径缓存，跨平台）
        items.extend(super::cache_registry::RegistryScanner::new().scan().items);

        // 构建产物递归扫描（dist/.next/.nuxt 等，跨平台）
        scan_build_artifacts(&mut items);

        // K8s（跨平台） / Docker（macOS 专属 Docker Desktop 路径）
        items.extend(scan_k8s_caches());
        #[cfg(target_os = "macos")]
        items.extend(scan_docker_caches());
        items.extend(scan_ai_model_caches());
        items.extend(scan_monorepo_caches());

        // 安装包清理（跨平台，按平台扩展名过滤）
        scan_installer_files(&mut items);

        // 孤儿 LaunchAgent/LaunchDaemon 检测（仅 macOS）
        #[cfg(target_os = "macos")]
        items.extend(scan_orphaned_launchd());

        // .DS_Store 文件清理（仅 macOS）
        #[cfg(target_os = "macos")]
        items.extend(scan_ds_store_files());

        // Windows 专属扫描器
        #[cfg(target_os = "windows")]
        {
            items.extend(scan_wsl2_vhdx());
            items.extend(scan_windows_temp());
            items.extend(scan_docker_windows());
        }

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
//  Rust Cargo 缓存扫描
// =========================================================================

/// 扫描 Rust/Cargo 相关缓存
fn scan_rust_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // 1. 搜索工作区中的 target 目录（Rust 编译产物）
    for base in get_project_search_paths() {
        for target_dir in search_dirs(&base, "target", 3) {
            let size = dir_size(&target_dir);
            items.push(ScanItem {
                path: target_dir.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Rust编译".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Rust 编译产物，cargo build 会自动重新生成".to_string(),
            });
        }
    }

    // 2. ~/.cargo/registry (Cargo 包下载缓存)
    let cargo_registry = home.join(".cargo/registry");
    if cargo_registry.is_dir() {
        let size = dir_size(&cargo_registry);
        items.push(ScanItem {
            path: cargo_registry.to_string_lossy().to_string(),
            size_bytes: size,
            category: "Rust编译".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Caution,
            description: "Cargo 包下载缓存，删除后编译时需重新下载".to_string(),
        });
    }

    items
}

// =========================================================================
//  Xcode 缓存扫描
// =========================================================================

/// 扫描 Xcode 相关缓存（参考 DevCleaner 细粒度拆分）
fn scan_xcode_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();
    let xcode_dir = home.join("Library/Developer/Xcode");

    // 1. DerivedData - 按项目拆分
    let derived_data = xcode_dir.join("DerivedData");
    if derived_data.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&derived_data) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let size = dir_size(&path);
                if size > 1024 * 1024 {
                    // > 1MB 才展示
                    // 提取项目名（DerivedData 目录名格式：ProjectName-xxxxxxxx）
                    let project_name = name.split('-').next().unwrap_or(name);
                    items.push(ScanItem {
                        path: path.to_string_lossy().to_string(),
                        size_bytes: size,
                        category: format!("Xcode编译-{}", project_name),
                        selected: false,
                        deletable: true,
                        undeletable_reason: String::new(),
                        batch_paths: Vec::new(),
                        recommend: Recommend::Safe,
                        description: format!(
                            "项目 {} 的编译缓存，重新构建会自动恢复",
                            project_name
                        ),
                    });
                }
            }
        }
    }

    // 2. iOS DeviceSupport - 按版本拆分，保留最新版
    let device_support = xcode_dir.join("iOS DeviceSupport");
    if device_support.is_dir() {
        let mut versions: Vec<(String, PathBuf, u64)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&device_support) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.is_empty() || path.starts_with(".") {
                    continue;
                }
                let size = dir_size(&path);
                versions.push((name.to_string(), path, size));
            }
        }
        // 按 iOS 版本号排序，找出最新版
        versions.sort_by(|a, b| version_compare(&a.0, &b.0));

        for (i, (version, path, size)) in versions.iter().enumerate() {
            let is_latest = i == versions.len().saturating_sub(1);
            items.push(ScanItem {
                path: path.to_string_lossy().to_string(),
                size_bytes: *size,
                category: format!("iOS设备-{}", version),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: if is_latest {
                    Recommend::Advanced
                } else {
                    Recommend::Safe
                },
                description: if is_latest {
                    format!("iOS {} 设备调试符号（最新版，建议保留）", version)
                } else {
                    format!("iOS {} 旧版调试符号，可安全删除", version)
                },
            });
        }
    }

    // 3. Archives - 按日期/项目拆分
    let archives = xcode_dir.join("Archives");
    if archives.is_dir() {
        // Archives 目录结构：Archives/YYYY-MM-DD/ProjectName.xcarchive
        if let Ok(date_dirs) = std::fs::read_dir(&archives) {
            for date_dir in date_dirs.filter_map(|e| e.ok()) {
                let date_path = date_dir.path();
                if !date_path.is_dir() {
                    continue;
                }
                let date_name = date_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if let Ok(archive_entries) = std::fs::read_dir(&date_path) {
                    for archive_entry in archive_entries.filter_map(|e| e.ok()) {
                        let archive_path = archive_entry.path();
                        let archive_name = archive_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        let size = dir_size(&archive_path);
                        if size > 1024 * 1024 {
                            // > 1MB
                            items.push(ScanItem {
                                path: archive_path.to_string_lossy().to_string(),
                                size_bytes: size,
                                category: format!("Xcode归档-{}", date_name),
                                selected: false,
                                deletable: true,
                                undeletable_reason: String::new(),
                                batch_paths: Vec::new(),
                                recommend: Recommend::Advanced,
                                description: format!(
                                    "归档 {} ({})，包含构建和调试信息",
                                    archive_name, date_name
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    // 4. 模拟器镜像 (系统级目录，通过 xcrun simctl runtime delete 安全删除)
    // 如果模拟器相关服务（Xcode/Simulator/CoreSimulatorService/simdiskimaged）
    // 在运行，runtime 镜像会被锁定，直接不展示；如果大小为 0，也没有清理价值，不展示。
    let sim_volumes = PathBuf::from("/Library/Developer/CoreSimulator/Volumes");
    if sim_volumes.is_dir() && !safety::is_simulator_running() {
        let size = get_simulator_runtime_size().unwrap_or(0);
        if size > 0 {
            items.push(ScanItem {
                path: sim_volumes.to_string_lossy().to_string(),
                size_bytes: size,
                category: "模拟器镜像".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                recommend: Recommend::Caution,
                description: "iOS 模拟器运行时镜像，将通过 xcrun simctl runtime delete 安全删除"
                    .to_string(),
                batch_paths: Vec::new(),
            });
        }
    }

    // 5. 模拟器缓存（root 属主，sudo rm -rf 可删，内容为可重建的缓存）
    let sim_caches = PathBuf::from("/Library/Developer/CoreSimulator/Caches");
    if sim_caches.is_dir() && !safety::is_simulator_running() {
        let size = dir_size(&sim_caches);
        if size > 0 {
            items.push(ScanItem {
                path: sim_caches.to_string_lossy().to_string(),
                size_bytes: size,
                category: "模拟器缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                recommend: Recommend::Caution,
                description: "模拟器系统缓存，删除后自动重建，需管理员权限".to_string(),
                batch_paths: Vec::new(),
            });
        }
    }

    // 6. 模拟器 Cryptex（系统级运行时扩展，与 Volumes 同级）
    let sim_cryptex = PathBuf::from("/Library/Developer/CoreSimulator/Cryptex");
    if sim_cryptex.is_dir() && !safety::is_simulator_running() {
        // 优先使用 /usr/bin/du -sk 获取真实大小。普通 dir_size() 也能得到近似
        // 值，但部分子目录会触发 Permission denied，du 处理 mount point 更准确。
        let size = get_simulator_cryptex_size().unwrap_or(0);
        if size > 0 {
            items.push(ScanItem {
                path: sim_cryptex.to_string_lossy().to_string(),
                size_bytes: size,
                category: "模拟器Cryptex".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                recommend: Recommend::Caution,
                description: "模拟器运行时 Cryptex 扩展，通过 xcrun simctl runtime delete 安全删除"
                    .to_string(),
                batch_paths: Vec::new(),
            });
        }
    }

    // 6. 文档缓存 (DevCleaner 特有)
    let doc_cache = xcode_dir.join("Documentation Cache");
    if doc_cache.is_dir() {
        let size = dir_size(&doc_cache);
        if size > 0 {
            items.push(ScanItem {
                path: doc_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Xcode文档缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Xcode 在线文档缓存，可安全删除".to_string(),
            });
        }
    }

    // 7. 设备日志 (DevCleaner 特有)
    let device_logs = xcode_dir.join("iOS Device Logs");
    if device_logs.is_dir() {
        let size = dir_size(&device_logs);
        if size > 0 {
            items.push(ScanItem {
                path: device_logs.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Xcode设备日志".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "设备日志和崩溃报告，可安全删除".to_string(),
            });
        }
    }

    // 8. Xcode 旧版离线文档
    let offline_docs = home.join("Library/Developer/Shared/Documentation/DocSets");
    if offline_docs.is_dir() {
        let size = dir_size(&offline_docs);
        if size > 0 {
            items.push(ScanItem {
                path: offline_docs.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Xcode离线文档".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Advanced,
                description: "Xcode 旧版离线文档，可能不再需要".to_string(),
            });
        }
    }

    // 9. watchOS DeviceSupport
    let watch_support = xcode_dir.join("watchOS DeviceSupport");
    if watch_support.is_dir() {
        let size = dir_size(&watch_support);
        if size > 0 {
            items.push(ScanItem {
                path: watch_support.to_string_lossy().to_string(),
                size_bytes: size,
                category: "watchOS设备".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: "watchOS 设备调试符号，连接手表时会重新生成".to_string(),
            });
        }
    }

    // 10. AppleConnect 设备支持
    let connector_support = xcode_dir.join("AppleConnectLogs");
    if connector_support.is_dir() {
        let size = dir_size(&connector_support);
        if size > 0 {
            items.push(ScanItem {
                path: connector_support.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Xcode连接日志".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Apple Connect 日志，可安全删除".to_string(),
            });
        }
    }

    items
}

/// 简单的版本号比较（如 "17.4" vs "17.5"）
fn version_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let parse_ver = |s: &str| -> Vec<u32> {
        s.split(|c: char| !c.is_ascii_digit() && c != '.')
            .filter(|p| !p.is_empty())
            .flat_map(|p| p.split('.'))
            .filter_map(|n| n.parse::<u32>().ok())
            .collect()
    };
    let va = parse_ver(a);
    let vb = parse_ver(b);
    va.cmp(&vb)
}

// =========================================================================
//  Node.js 缓存扫描
// =========================================================================

/// 扫描 Node.js 相关缓存
fn scan_node_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // 1. 搜索工作区中的 node_modules 目录
    for base in get_project_search_paths() {
        for nm_dir in search_dirs(&base, "node_modules", 3) {
            let size = dir_size(&nm_dir);
            items.push(ScanItem {
                path: nm_dir.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Node依赖".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Node.js 依赖包，npm install 可恢复".to_string(),
            });
        }
    }

    // 2. pnpm 全局存储 (跨平台)
    // macOS: ~/Library/pnpm/store
    // Windows: %LOCALAPPDATA%\pnpm\store
    let pnpm_store = {
        #[cfg(target_os = "macos")]
        {
            home.join("Library/pnpm/store")
        }
        #[cfg(target_os = "windows")]
        {
            std::env::var("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join("AppData/Local"))
                .join("pnpm/store")
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            home.join(".local/share/pnpm/store")
        }
    };
    if pnpm_store.is_dir() {
        let size = dir_size(&pnpm_store);
        items.push(ScanItem {
            path: pnpm_store.to_string_lossy().to_string(),
            size_bytes: size,
            category: "pnpm缓存".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Caution,
            description: "pnpm 全局存储，删除后需重新安装依赖".to_string(),
        });
    }

    // 3. ~/.npm (npm 全局缓存)
    let npm_cache = home.join(".npm");
    if npm_cache.is_dir() {
        let size = dir_size(&npm_cache);
        items.push(ScanItem {
            path: npm_cache.to_string_lossy().to_string(),
            size_bytes: size,
            category: "npm缓存".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Safe,
            description: "npm 下载缓存，可安全删除".to_string(),
        });
    }

    items
}

// =========================================================================
//  Go 缓存扫描
// =========================================================================

/// 扫描 Go 模块缓存
fn scan_go_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // ~/go/pkg/mod (Go 模块下载缓存)
    let go_mod = home.join("go/pkg/mod");
    if go_mod.is_dir() {
        let size = dir_size(&go_mod);
        items.push(ScanItem {
            path: go_mod.to_string_lossy().to_string(),
            size_bytes: size,
            category: "Go模块".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Caution,
            description: "Go 模块缓存，编译时需重新下载".to_string(),
        });
    }

    items
}

// =========================================================================
//  Homebrew 缓存扫描
// =========================================================================

/// 扫描 Homebrew 下载缓存
fn scan_homebrew_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // ~/Library/Caches/Homebrew (Homebrew 下载的包缓存)
    let brew_cache = home.join("Library/Caches/Homebrew");
    if brew_cache.is_dir() {
        let size = dir_size(&brew_cache);
        items.push(ScanItem {
            path: brew_cache.to_string_lossy().to_string(),
            size_bytes: size,
            category: "Homebrew缓存".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Safe,
            description: "Homebrew 下载缓存，可安全删除".to_string(),
        });
    }

    // ~/Library/Caches/Homebrew/downloads (具体下载文件)
    let brew_downloads = home.join("Library/Caches/Homebrew/downloads");
    if brew_downloads.is_dir() {
        let size = dir_size(&brew_downloads);
        if size > 10 * 1024 * 1024 {
            items.push(ScanItem {
                path: brew_downloads.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Homebrew下载".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Homebrew 已下载的安装包，可安全删除".to_string(),
            });
        }
    }

    // Homebrew Caskroom 旧版本缓存（/opt/homebrew/Caskroom 或 /usr/local/Caskroom）
    for caskroom in ["/opt/homebrew/Caskroom", "/usr/local/Caskroom"] {
        let cask_path = PathBuf::from(caskroom);
        if cask_path.is_dir() {
            // 检查每个 cask 是否有多个版本
            if let Ok(entries) = std::fs::read_dir(&cask_path) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let cask_dir = entry.path();
                    let cask_name = entry.file_name().to_string_lossy().to_string();
                    if let Ok(version_entries) = std::fs::read_dir(&cask_dir) {
                        let versions: Vec<_> = version_entries.filter_map(|e| e.ok()).collect();
                        if versions.len() > 1 {
                            // 有多个版本，旧版本可清理
                            let mut total_old_size: u64 = 0;
                            let mut old_paths: Vec<String> = Vec::new();
                            for (i, v) in versions.iter().enumerate() {
                                if i < versions.len() - 1 {
                                    let size = dir_size(&v.path());
                                    total_old_size += size;
                                    old_paths.push(v.path().to_string_lossy().to_string());
                                }
                            }
                            if total_old_size > 10 * 1024 * 1024 {
                                items.push(ScanItem {
                                    path: format!("{} ({}个旧版本)", cask_name, old_paths.len()),
                                    size_bytes: total_old_size,
                                    category: "Homebrew旧版".to_string(),
                                    selected: false,
                                    deletable: true,
                                    undeletable_reason: String::new(),
                                    batch_paths: old_paths,
                                    recommend: Recommend::Safe,
                                    description: format!("{} 的旧版本，最新版已保留", cask_name),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    items
}

// =========================================================================
//  pip 缓存扫描
// =========================================================================

/// 扫描 pip 包管理器缓存
fn scan_pip_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // pip 下载缓存 (跨平台)
    // macOS: ~/Library/Caches/pip
    // Windows: %LOCALAPPDATA%\pip\Cache
    let pip_cache = {
        #[cfg(target_os = "macos")]
        {
            home.join("Library/Caches/pip")
        }
        #[cfg(target_os = "windows")]
        {
            std::env::var("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join("AppData/Local"))
                .join("pip/Cache")
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            home.join(".cache/pip")
        }
    };
    if pip_cache.is_dir() {
        let size = dir_size(&pip_cache);
        items.push(ScanItem {
            path: pip_cache.to_string_lossy().to_string(),
            size_bytes: size,
            category: "pip缓存".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Safe,
            description: "pip 下载缓存，可安全删除".to_string(),
        });
    }

    items
}

// =========================================================================
//  JetBrains IDE 缓存扫描
// =========================================================================

/// 扫描 JetBrains IDE 相关缓存和旧版本配置
fn scan_jetbrains_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // 1. ~/Library/Application Support/JetBrains/ - 找出非最新版本的配置目录
    let jb_support = home.join("Library/Application Support/JetBrains");
    if jb_support.is_dir() {
        // 收集所有版本目录，按产品名分组
        // key: 产品名 (如 "IntelliJIdea"), value: (目录全名, 版本号字符串)
        let mut products: HashMap<String, Vec<(String, String)>> = HashMap::new();

        if let Ok(entries) = std::fs::read_dir(&jb_support) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                // 解析 JetBrains 目录名，提取产品名和版本号
                if let Some((product, version)) = parse_jetbrains_dir(&name) {
                    products.entry(product).or_default().push((name, version));
                }
            }
        }

        // 对每个产品，按版本降序排序，保留最新版本，其余标记为旧版
        for (_product, mut versions) in products {
            // 版本号格式如 "2024.2"，字符串比较即可正确排序
            versions.sort_by(|a, b| b.1.cmp(&a.1));

            // 跳过最新版本（第一个），其余作为旧版
            for (dir_name, _) in versions.iter().skip(1) {
                let dir_path = jb_support.join(dir_name);
                let size = dir_size(&dir_path);
                items.push(ScanItem {
                    path: dir_path.to_string_lossy().to_string(),
                    size_bytes: size,
                    category: "IDE旧版".to_string(),
                    selected: false,
                    deletable: true,
                    undeletable_reason: String::new(),
                    batch_paths: Vec::new(),
                    recommend: Recommend::Safe,
                    description: "JetBrains IDE 旧版本配置，已保留最新版".to_string(),
                });
            }
        }
    }

    // 2. ~/Library/Caches/JetBrains - IDE 缓存目录
    let jb_caches = home.join("Library/Caches/JetBrains");
    if jb_caches.is_dir() {
        let size = dir_size(&jb_caches);
        items.push(ScanItem {
            path: jb_caches.to_string_lossy().to_string(),
            size_bytes: size,
            category: "IDE缓存".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Safe,
            description: "JetBrains IDE 缓存，重启 IDE 会自动重建".to_string(),
        });
    }

    items
}

/// 通过 xcrun simctl runtime list 获取 iOS 模拟器运行时镜像总大小
///
/// 输出示例：
///   Total Disk Images: 3 (24.2G)
/// 解析最后一行的总大小（例如 24.2G）。
///
/// 注：普通 dir_size() 也能遍历该目录，但会跳过无权限子目录，结果可能
/// 偏大（包含 overlay 文件）或不准确。simctl 提供 Apple 官方的 runtime
/// 镜像大小统计，更适合展示给用户。
fn get_simulator_runtime_size() -> Option<u64> {
    let output = std::process::Command::new("/usr/bin/xcrun")
        .args(["simctl", "runtime", "list"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    // 查找 Total Disk Images 行，例如 "Total Disk Images: 3 (24.2G)"
    for line in stdout.lines() {
        if let Some(start) = line.find("(") {
            if let Some(end) = line.find(")") {
                let size_str = &line[start + 1..end];
                let bytes = parse_size_str(size_str.trim());
                if bytes > 0 {
                    return Some(bytes);
                }
            }
        }
    }
    None
}

/// 获取模拟器 Cryptex 目录大小
///
/// 优先使用 /usr/bin/du -sk 获取 Cryptex 真实大小。普通 dir_size() 递归
/// 遍历该目录也能得到近似值，但部分子目录会触发 Permission denied，
/// du 在处理 mount point 时更准确且性能更好。失败则返回 None。
fn get_simulator_cryptex_size() -> Option<u64> {
    let path = PathBuf::from("/Library/Developer/CoreSimulator/Cryptex");
    let output = std::process::Command::new("/usr/bin/du")
        .args(["-sk", &path.to_string_lossy()])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.split_whitespace().collect();
    if let Some(first) = parts.first() {
        if let Ok(kb) = first.parse::<u64>() {
            return Some(kb * 1024);
        }
    }
    None
}

/// 解析 JetBrains 目录名，提取产品名和版本号
///
/// JetBrains 目录名格式如:
/// - "IntelliJIdea2024.2" -> ("IntelliJIdea", "2024.2")
/// - "RustRover2024.1"    -> ("RustRover", "2024.1")
/// - "PyCharmProfessional2023.3" -> ("PyCharmProfessional", "2023.3")
fn parse_jetbrains_dir(name: &str) -> Option<(String, String)> {
    // 找到第一个数字的位置（版本号开始处）
    let pos = name.find(|c: char| c.is_ascii_digit())?;
    let product = name[..pos].to_string();
    let version = name[pos..].to_string();

    if product.is_empty() || version.is_empty() {
        return None;
    }

    Some((product, version))
}

// =========================================================================
//  辅助函数
// =========================================================================

/// 获取项目搜索路径列表
///
/// 搜索用户 home 目录下的项目工作区:
/// - ~/Downloads/myproject/workspace
/// - ~/Downloads/myStudy/project
/// - ~/FrontProject
///
/// 注意: Downloads 受 TCC 保护，在没有完全磁盘访问权限时遍历会触发弹窗。
/// 这里仍然保留，因为开发者缓存扫描是核心功能。
/// 如果触发弹窗，用户需在系统设置中授予完全磁盘访问权限。
fn get_project_search_paths() -> Vec<PathBuf> {
    let home = home_dir();
    let candidates = [
        home.join("Downloads/myproject/workspace"),
        home.join("Downloads/myStudy/project"),
        home.join("FrontProject"),
    ];

    candidates
        .into_iter()
        .filter(|p| p.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false))
        .collect()
}

/// 在指定目录下搜索特定名称的子目录
///
/// 使用 walkdir 递归搜索，限制最大深度，跳过 .git 等版本控制目录。
/// 搜索完成后过滤掉嵌套的匹配项（例如 node_modules 内部的 node_modules），
/// 避免重复计算大小。
///
/// # 参数
/// - `base`: 搜索起始目录
/// - `name`: 要搜索的目录名（如 "target"、"node_modules"）
/// - `max_depth`: 最大搜索深度
fn search_dirs(base: &Path, name: &str, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !base.is_dir() {
        return found;
    }

    // 使用 walkdir 遍历，跳过 .git/.svn/.hg 目录以提高性能
    for entry in WalkDir::new(base)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // 跳过版本控制目录
            if e.depth() > 0 && e.file_type().is_dir() {
                let dir_name = e.file_name().to_string_lossy();
                if dir_name == ".git" || dir_name == ".svn" || dir_name == ".hg" {
                    return false;
                }
            }
            true
        })
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_dir() && entry.file_name().to_string_lossy() == name {
            found.push(entry.path().to_path_buf());
        }
    }

    // 过滤掉嵌套的匹配目录
    // 例如: 如果同时找到了 a/node_modules 和 a/node_modules/pkg/node_modules，
    // 后者是前者的子目录，应移除以避免重复计算大小
    // 先克隆一份用于比较，避免 retain 的可变借用与闭包中的不可变借用冲突
    let all_found = found.clone();
    found.retain(|path| {
        !all_found
            .iter()
            .any(|other| other != path && path.starts_with(other))
    });

    found
}

/// 扫描 Java/Gradle/Maven 缓存
fn scan_java_caches(items: &mut Vec<ScanItem>) {
    let home = home_dir();

    // Gradle 缓存 (~/.gradle/caches)
    let gradle_caches = home.join(".gradle/caches");
    if let Ok(size) = dir_size_checked(&gradle_caches) {
        if size > 0 {
            items.push(ScanItem {
                path: gradle_caches.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Gradle缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: "Gradle 构建缓存，删除后编译时需重新下载依赖".to_string(),
            });
        }
    }

    // Gradle wrapper 发行版 (~/.gradle/wrapper/dists)
    let gradle_dists = home.join(".gradle/wrapper/dists");
    if let Ok(size) = dir_size_checked(&gradle_dists) {
        if size > 0 {
            items.push(ScanItem {
                path: gradle_dists.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Gradle版本".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Gradle Wrapper 下载的版本，可安全删除会自动重新下载".to_string(),
            });
        }
    }

    // Maven 本地仓库 (~/.m2/repository)
    let m2_repo = home.join(".m2/repository");
    if let Ok(size) = dir_size_checked(&m2_repo) {
        if size > 0 {
            items.push(ScanItem {
                path: m2_repo.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Maven仓库".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: "Maven 本地依赖仓库，删除后编译时需重新下载".to_string(),
            });
        }
    }

    // Java 项目 build 目录
    for search_path in get_project_search_paths() {
        let build_dirs = search_dirs(&search_path, "build", 4);
        for dir in build_dirs {
            // 排除 node_modules 内的 build
            if dir.to_string_lossy().contains("node_modules") {
                continue;
            }
            if let Ok(size) = dir_size_checked(&dir) {
                if size > 10 * 1024 * 1024 {
                    // > 10MB
                    items.push(ScanItem {
                        path: dir.to_string_lossy().to_string(),
                        size_bytes: size,
                        category: "Java编译".to_string(),
                        selected: false,
                        deletable: true,
                        undeletable_reason: String::new(),
                        batch_paths: Vec::new(),
                        recommend: Recommend::Safe,
                        description: "Java/Gradle 项目编译产物，gradle build 会自动重新生成"
                            .to_string(),
                    });
                }
            }
        }
    }
}

/// 扫描 Python 缓存
fn scan_python_caches(items: &mut Vec<ScanItem>) {
    let home = home_dir();

    // pip 缓存 (跨平台)
    // macOS: ~/Library/Caches/pip
    // Windows: %LOCALAPPDATA%\pip\Cache
    let pip_cache = {
        #[cfg(target_os = "macos")]
        {
            home.join("Library/Caches/pip")
        }
        #[cfg(target_os = "windows")]
        {
            std::env::var("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join("AppData/Local"))
                .join("pip/Cache")
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            home.join(".cache/pip")
        }
    };
    if let Ok(size) = dir_size_checked(&pip_cache) {
        if size > 0 {
            items.push(ScanItem {
                path: pip_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "pip缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "pip 下载缓存，可安全删除".to_string(),
            });
        }
    }

    // Conda 缓存 (~/.conda) - 跨平台
    let conda_pkgs = home.join(".conda/pkgs");
    if let Ok(size) = dir_size_checked(&conda_pkgs) {
        if size > 0 {
            items.push(ScanItem {
                path: conda_pkgs.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Conda缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: "Conda 包缓存，删除后安装时需重新下载".to_string(),
            });
        }
    }

    // Poetry 缓存 (跨平台)
    // macOS: ~/Library/Caches/pypoetry
    // Windows: %APPDATA%\pypoetry\Cache
    let poetry_cache = {
        #[cfg(target_os = "macos")]
        {
            home.join("Library/Caches/pypoetry")
        }
        #[cfg(target_os = "windows")]
        {
            std::env::var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join("AppData/Roaming"))
                .join("pypoetry/Cache")
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            home.join(".cache/pypoetry")
        }
    };
    if let Ok(size) = dir_size_checked(&poetry_cache) {
        if size > 0 {
            items.push(ScanItem {
                path: poetry_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Poetry缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Poetry 依赖缓存，可安全删除".to_string(),
            });
        }
    }

    // Python __pycache__ 目录 — 聚合为单个项，batch_paths 存储真实路径
    let mut all_pycache_paths: Vec<String> = Vec::new();
    let mut all_pycache_size: u64 = 0;
    for search_path in get_project_search_paths() {
        let pycache_dirs = search_dirs(&search_path, "__pycache__", 5);
        for dir in &pycache_dirs {
            if let Ok(size) = dir_size_checked(dir) {
                all_pycache_size += size;
                all_pycache_paths.push(dir.to_string_lossy().to_string());
            }
        }
    }
    if all_pycache_size > 10 * 1024 * 1024 {
        // > 10MB 才展示
        items.push(ScanItem {
            path: format!("{}个 __pycache__ 目录", all_pycache_paths.len()),
            size_bytes: all_pycache_size,
            category: "Python缓存".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            recommend: Recommend::Safe,
            description: "Python 字节码缓存，运行时自动重建".to_string(),
            batch_paths: all_pycache_paths,
        });
    }
}

/// 安全的目录大小计算（带错误处理）
fn dir_size_checked(path: &Path) -> Result<u64, String> {
    if !path.is_dir() {
        return Ok(0);
    }
    let mut total: u64 = 0;
    for entry in WalkDir::new(path)
        .follow_links(false)
        .max_depth(50)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() > 0 && e.file_type().is_dir() {
                std::fs::metadata(e.path()).is_ok()
            } else {
                true
            }
        })
    {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_file() {
                    if let Ok(metadata) = entry.metadata() {
                        total += metadata.len();
                    }
                }
            }
            Err(_) => continue,
        }
    }
    Ok(total)
}

/// 扫描构建产物目录（dist/.next/.nuxt/.turbo/.svelte-kit/.astro/.remix/.gradle）
/// 固定路径缓存已迁移至 cache_registry 模块统一管理
fn scan_build_artifacts(items: &mut Vec<ScanItem>) {
    // 通用构建产物目录扫描（dist、build、.next、.nuxt、.turbo、.svelte-kit）
    for search_path in get_project_search_paths() {
        let build_dir_names = vec![
            (
                "dist",
                "构建产物",
                Recommend::Safe,
                "前端构建产物，npm run build 会重新生成",
            ),
            (
                ".next",
                "Next.js",
                Recommend::Safe,
                "Next.js 构建缓存，可安全删除",
            ),
            (
                ".nuxt",
                "Nuxt.js",
                Recommend::Safe,
                "Nuxt.js 构建缓存，可安全删除",
            ),
            (
                ".turbo",
                "Turborepo",
                Recommend::Safe,
                "Turborepo 缓存，可安全删除",
            ),
            (
                ".svelte-kit",
                "SvelteKit",
                Recommend::Safe,
                "SvelteKit 构建缓存，可安全删除",
            ),
            (
                ".astro",
                "Astro",
                Recommend::Safe,
                "Astro 构建缓存，可安全删除",
            ),
            (
                ".remix",
                "Remix",
                Recommend::Safe,
                "Remix 构建缓存，可安全删除",
            ),
            (
                ".gradle",
                "Gradle项目",
                Recommend::Safe,
                "Gradle 项目本地缓存，可安全删除",
            ),
        ];

        for (dir_name, category, recommend, desc) in &build_dir_names {
            let found_dirs = search_dirs(&search_path, dir_name, 4);
            for dir in found_dirs {
                // 排除 node_modules 内的目录
                if dir.to_string_lossy().contains("node_modules") {
                    continue;
                }
                if let Ok(size) = dir_size_checked(&dir) {
                    if size > 10 * 1024 * 1024 {
                        // > 10MB
                        items.push(ScanItem {
                            path: dir.to_string_lossy().to_string(),
                            size_bytes: size,
                            category: category.to_string(),
                            selected: false,
                            deletable: true,
                            undeletable_reason: String::new(),
                            batch_paths: Vec::new(),
                            recommend: *recommend,
                            description: desc.to_string(),
                        });
                    }
                }
            }
        }
    }
}

// =========================================================================
//  安装包清理（Downloads / Desktop 下的 dmg/pkg/exe/msi 等）
// =========================================================================

/// 扫描安装包文件（Downloads、Desktop 等位置）
///
/// 智能检测安装包对应的应用是否已安装：
/// - 已安装 → 标记 Safe（安装包可安全删除）
/// - 未安装 → 标记 Caution（用户可能还需要安装）
fn scan_installer_files(items: &mut Vec<ScanItem>) {
    let home = home_dir();

    // 扫描多个常见位置
    let scan_dirs = [home.join("Downloads"), home.join("Desktop")];

    // 安装包扩展名（按平台区分）
    #[cfg(target_os = "macos")]
    let installer_exts = [".dmg", ".pkg", ".iso", ".zip", ".tar.gz", ".tgz", ".7z"];
    #[cfg(target_os = "windows")]
    let installer_exts = [".exe", ".msi", ".iso", ".zip", ".7z", ".msix"];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let installer_exts = [".iso", ".zip", ".tar.gz", ".tgz", ".7z"];

    let mut installers: Vec<(String, u64, String, bool)> = Vec::new();
    // (path, size, filename, is_installed)

    for dir in &scan_dirs {
        if !dir.is_dir() {
            continue;
        }

        for entry in walkdir::WalkDir::new(dir)
            .max_depth(2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let filename_lower = entry.file_name().to_string_lossy().to_lowercase();
            let path_str = path.to_string_lossy().to_string();

            // 检查是否是安装包扩展名
            let is_installer = installer_exts.iter().any(|ext| filename_lower.ends_with(ext));
            if !is_installer {
                continue;
            }

            // .zip 特殊处理：需要包含安装包特征，避免误删用户压缩包
            if filename_lower.ends_with(".zip") && !is_installer_zip(&filename_lower) {
                continue;
            }

            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if size < 50 * 1024 * 1024 {
                // 小于 50MB 的不展示
                continue;
            }

            let filename = entry.file_name().to_string_lossy().to_string();
            let installed = check_app_installed(&filename);

            installers.push((path_str, size, filename, installed));
        }
    }

    if installers.is_empty() {
        return;
    }

    // 按大小降序排序
    installers.sort_by(|a, b| b.1.cmp(&a.1));

    for (path, size, filename, installed) in installers {
        let (recommend, description) = if installed {
            (
                Recommend::Safe,
                format!("{} 已安装，安装包可安全删除", filename),
            )
        } else {
            (
                Recommend::Caution,
                format!("{} 的安装包，删除后需重新下载安装", filename),
            )
        };

        items.push(ScanItem {
            path,
            size_bytes: size,
            category: "安装包".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend,
            description,
        });
    }
}

/// 检查 .zip 文件名是否像安装包（避免误删用户普通压缩包）
fn is_installer_zip(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    lower.contains("setup")
        || lower.contains("install")
        || lower.contains("installer")
        || lower.contains("pkg")
        || lower.contains("download")
}

/// 检查安装包对应的应用是否已安装（跨平台入口）
fn check_app_installed(installer_filename: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        check_app_installed_macos(installer_filename)
    }
    #[cfg(target_os = "windows")]
    {
        check_app_installed_windows(installer_filename)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// macOS: 从安装包文件名提取应用名，检查 /Applications 和 ~/Applications
#[cfg(target_os = "macos")]
fn check_app_installed_macos(installer_filename: &str) -> bool {
    // 从文件名提取应用名（去掉扩展名）
    let stem = std::path::Path::new(installer_filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    if stem.is_empty() {
        return false;
    }

    // 移除常见平台/架构后缀，得到核心应用名
    let app_name = stem
        .replace("-darwin-universal", "")
        .replace("-darwin-arm64", "")
        .replace("-darwin-x64", "")
        .replace("-macos", "")
        .replace("-mac", "")
        .replace("-osx", "")
        .trim()
        .to_string();

    if app_name.is_empty() {
        return false;
    }

    // 精确匹配：/Applications/<AppName>.app
    let exact_path = format!("/Applications/{}.app", app_name);
    if std::path::Path::new(&exact_path).exists() {
        return true;
    }

    // 精确匹配：~/Applications/<AppName>.app
    let user_app = home_dir()
        .join("Applications")
        .join(format!("{}.app", app_name));
    if user_app.exists() {
        return true;
    }

    // 模糊匹配：/Applications 下是否有包含关键词的 .app
    let app_lower = app_name.to_lowercase();
    for search_dir in &["/Applications", &home_dir().join("Applications").to_string_lossy()] {
        if let Ok(entries) = std::fs::read_dir(search_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".app") {
                        let name_lower = name.to_lowercase().replace(".app", "");
                        // 双向包含匹配
                        if name_lower.contains(&app_lower) || app_lower.contains(&name_lower) {
                            return true;
                        }
                    }
                }
            }
        }
    }

    false
}

/// Windows: 从安装包文件名提取应用名，查注册表 Uninstall 键
#[cfg(target_os = "windows")]
fn check_app_installed_windows(installer_filename: &str) -> bool {
    // 从文件名提取应用名（去掉扩展名）
    let stem = std::path::Path::new(installer_filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    if stem.is_empty() {
        return false;
    }

    // 移除常见安装包后缀，得到核心应用名
    let app_name = stem
        .replace("Setup", "")
        .replace("setup", "")
        .replace("Installer", "")
        .replace("installer", "")
        .replace("Install", "")
        .replace("install", "")
        .replace("x64", "")
        .replace("x86", "")
        .replace("win64", "")
        .replace("win32", "")
        .replace("amd64", "")
        .replace("-", " ")
        .replace("_", " ")
        .trim()
        .to_string();

    if app_name.len() < 2 {
        return false;
    }

    // 通过注册表 Uninstall 键查找（64位 + 32位 + 用户级）
    let uninstall_keys = [
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    ];

    for key_path in &uninstall_keys {
        if let Ok(output) = std::process::Command::new("reg")
            .args([
                "query",
                &format!("HKLM\\{}", key_path),
                "/s",
                "/f",
                &app_name,
            ])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("DisplayName") {
                return true;
            }
        }
    }

    // 也查用户级注册表
    if let Ok(output) = std::process::Command::new("reg")
        .args([
            "query",
            "HKCU\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            "/s",
            "/f",
            &app_name,
        ])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("DisplayName") {
            return true;
        }
    }

    false
}

/// 格式化文件大小（本地辅助函数）
fn format_size_local(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

// =========================================================================
//  K8s / Helm 配置缓存扫描
// =========================================================================

/// 扫描 Kubernetes / Helm 相关缓存
///
/// 只扫描缓存目录，绝不扫描 ~/.kube/config（重要配置文件）
fn scan_k8s_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // ~/.kube/cache (kubectl discovery/mapping 缓存)
    let kube_cache = home.join(".kube/cache");
    if let Ok(size) = dir_size_checked(&kube_cache) {
        if size > 0 {
            items.push(ScanItem {
                path: kube_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "K8s缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description:
                    "kubectl 缓存（discovery、mapping 等），删除后下次 kubectl 命令自动重建"
                        .to_string(),
            });
        }
    }

    // ~/.kube/http-cache (HTTP 缓存)
    let kube_http = home.join(".kube/http-cache");
    if let Ok(size) = dir_size_checked(&kube_http) {
        if size > 0 {
            items.push(ScanItem {
                path: kube_http.to_string_lossy().to_string(),
                size_bytes: size,
                category: "K8sHTTP缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "kubectl HTTP 缓存，删除后自动重建".to_string(),
            });
        }
    }

    // ~/.cache/helm/repository (Helm 仓库索引缓存)
    let helm_repo = home.join(".cache/helm/repository");
    if let Ok(size) = dir_size_checked(&helm_repo) {
        if size > 0 {
            items.push(ScanItem {
                path: helm_repo.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Helm缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: "Helm 仓库索引缓存，删除后执行 helm repo update 恢复".to_string(),
            });
        }
    }

    // ~/.cache/helm/plugins (Helm 插件缓存)
    let helm_plugins = home.join(".cache/helm/plugins");
    if let Ok(size) = dir_size_checked(&helm_plugins) {
        if size > 0 {
            items.push(ScanItem {
                path: helm_plugins.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Helm插件缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Helm 插件缓存，可安全删除".to_string(),
            });
        }
    }

    items
}

// =========================================================================
//  Docker 镜像/层缓存扫描
// =========================================================================

/// 检查 Docker 是否安装且 daemon 正在运行
fn is_docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 解析 `docker system df` 输出，获取可回收空间（字节）
///
/// 输出格式示例:
/// ```text
/// TYPE            TOTAL   ACTIVE  SIZE      RECLAIMABLE
/// Images          5       2       1.2GB     800MB (66%)
/// Containers      3       1       50MB      30MB (60%)
/// Local Volumes   2       1       500MB     200MB (40%)
/// Build Cache     10      0       300MB     300MB
/// ```
fn get_docker_reclaimable_size() -> u64 {
    let output = Command::new("docker")
        .args([
            "system",
            "df",
            "--format",
            "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}",
        ])
        .output();

    let Ok(out) = output else { return 0 };
    if !out.status.success() {
        return 0;
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut total: u64 = 0;
    for line in stdout.lines() {
        // 每行: "Images\t1.2GB\t800MB (66%)"
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            // Reclaimable 列: "800MB (66%)" 或 "0B"
            let reclaim_str = parts[2].split_whitespace().next().unwrap_or("0B");
            total += parse_size_str(reclaim_str);
        }
    }
    total
}

/// 解析 Docker/人类可读的大小字符串为字节数
///
/// 支持: 1.2GB, 800MB, 50KB, 1024B, 0B, 24.2G, 100M, 10K
fn parse_size_str(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() || s == "0B" {
        return 0;
    }

    // 分离数字和单位
    let (num_part, unit) = s
        .find(|c: char| c.is_alphabetic())
        .map(|idx| (&s[..idx], &s[idx..]))
        .unwrap_or((s, "B"));

    let num: f64 = num_part.parse().unwrap_or(0.0);
    let multiplier: f64 = match unit.to_uppercase().as_str() {
        "GB" | "G" => 1024.0 * 1024.0 * 1024.0,
        "MB" | "M" => 1024.0 * 1024.0,
        "KB" | "K" => 1024.0,
        "B" => 1.0,
        _ => 1.0,
    };

    (num * multiplier) as u64
}

/// 扫描 Docker 相关缓存
///
/// - 调用 `docker system df` 获取可回收空间（需要 Docker 运行）
/// - 扫描 ~/.docker/buildx/cache (构建缓存)
/// - 扫描 ~/Library/Containers/com.docker.docker/Data (Docker Desktop 数据)
fn scan_docker_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // 1. Docker daemon 可回收空间（通过 docker system prune 清理）
    if is_docker_available() {
        let reclaimable = get_docker_reclaimable_size();
        if reclaimable > 0 {
            items.push(ScanItem {
                // 特殊路径标记，删除时走 docker prune 分支
                path: "docker:system-prune".to_string(),
                size_bytes: reclaimable,
                category: "Docker清理".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Advanced,
                description: format!(
                    "执行 docker system prune -a --volumes 清理未使用镜像/容器/卷/网络\n\
                     可回收约 {} 空间，清理后需重新拉取镜像",
                    crate::scanner::format_size(reclaimable)
                ),
            });
        }
    }

    // 2. ~/.docker/buildx/cache (BuildKit 构建缓存)
    let buildx_cache = home.join(".docker/buildx/cache");
    if let Ok(size) = dir_size_checked(&buildx_cache) {
        if size > 0 {
            items.push(ScanItem {
                path: buildx_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Docker构建缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Docker BuildKit 构建缓存，删除后自动重建".to_string(),
            });
        }
    }

    // 3. ~/Library/Containers/com.docker.docker/Data/vms (Docker Desktop 虚拟机数据)
    let docker_vms = home.join("Library/Containers/com.docker.docker/Data/vms");
    if let Ok(size) = dir_size_checked(&docker_vms) {
        if size > 0 {
            items.push(ScanItem {
                path: docker_vms.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Docker虚拟机".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Advanced,
                description: "Docker Desktop 虚拟机数据，删除前请先退出 Docker Desktop".to_string(),
            });
        }
    }

    // 4. ~/Library/Containers/com.docker.docker/Data/cache (Docker Desktop 缓存)
    let docker_cache = home.join("Library/Containers/com.docker.docker/Data/cache");
    if let Ok(size) = dir_size_checked(&docker_cache) {
        if size > 0 {
            items.push(ScanItem {
                path: docker_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Docker缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: "Docker Desktop 缓存数据".to_string(),
            });
        }
    }

    items
}

// =========================================================================
//  AI 模型缓存扫描
// =========================================================================

/// 扫描 AI/ML 模型相关缓存
///
/// 分类展示:
/// - HuggingFace 模型（按 repo 拆分，Caution 级别）
/// - HuggingFace 临时缓存（Safe 级别）
/// - Ollama 模型（Advanced 级别）
/// - PyTorch 缓存（Caution 级别）
/// - llama.cpp 缓存（Safe 级别）
fn scan_ai_model_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // 1. HuggingFace 模型缓存 (~/.cache/huggingface/hub)
    // 目录结构: hub/models--<org>--<name>/snapshots/<hash>/
    let hf_hub = home.join(".cache/huggingface/hub");
    if hf_hub.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&hf_hub) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();

                // 只处理 models-- 前缀的目录
                if !name.starts_with("models--") {
                    continue;
                }

                // 解析 org/name: models--<org>--<name>
                let model_id = name.trim_start_matches("models--").replace("--", "/");
                let size = dir_size(&path);
                if size > 10 * 1024 * 1024 {
                    // > 10MB 才展示
                    items.push(ScanItem {
                        path: path.to_string_lossy().to_string(),
                        size_bytes: size,
                        category: "AI模型-HF".to_string(),
                        selected: false,
                        deletable: true,
                        undeletable_reason: String::new(),
                        batch_paths: Vec::new(),
                        recommend: Recommend::Caution,
                        description: format!("HuggingFace 模型 {}，删除后需重新下载", model_id),
                    });
                }
            }
        }
    }

    // 2. HuggingFace datasets 缓存 (~/.cache/huggingface/datasets)
    let hf_datasets = home.join(".cache/huggingface/datasets");
    if let Ok(size) = dir_size_checked(&hf_datasets) {
        if size > 0 {
            items.push(ScanItem {
                path: hf_datasets.to_string_lossy().to_string(),
                size_bytes: size,
                category: "AI缓存-HF".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "HuggingFace datasets 临时缓存，可安全删除".to_string(),
            });
        }
    }

    // 3. HuggingFace transformers 缓存 (~/.cache/huggingface/transformers)
    let hf_transformers = home.join(".cache/huggingface/transformers");
    if let Ok(size) = dir_size_checked(&hf_transformers) {
        if size > 0 {
            items.push(ScanItem {
                path: hf_transformers.to_string_lossy().to_string(),
                size_bytes: size,
                category: "AI缓存-HF".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "HuggingFace transformers 临时缓存，可安全删除".to_string(),
            });
        }
    }

    // 4. Ollama 模型 (~/.ollama/models)
    // 聚合展示整个 models 目录，包含 blobs 和 manifests
    let ollama_models = home.join(".ollama/models");
    if let Ok(size) = dir_size_checked(&ollama_models) {
        if size > 10 * 1024 * 1024 {
            // > 10MB 才展示
            // 尝试统计模型数量（通过 manifests 目录）
            let model_count = count_ollama_models(&home);
            let count_desc = if model_count > 0 {
                format!(
                    "Ollama 本地模型（{} 个），删除后需 ollama pull 重新下载",
                    model_count
                )
            } else {
                "Ollama 本地模型，删除后需 ollama pull 重新下载".to_string()
            };

            items.push(ScanItem {
                path: ollama_models.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Ollama模型".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Advanced,
                description: count_desc,
            });
        }
    }

    // 5. PyTorch 缓存 (~/.cache/torch)
    let torch_cache = home.join(".cache/torch");
    if let Ok(size) = dir_size_checked(&torch_cache) {
        if size > 0 {
            items.push(ScanItem {
                path: torch_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "PyTorch缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: "PyTorch 缓存（含预训练权重），删除后需重新下载".to_string(),
            });
        }
    }

    // 6. llama.cpp 缓存 (跨平台)
    // macOS: ~/Library/Caches/llama
    // Windows: %USERPROFILE%\.cache\llama
    let llama_cache = {
        #[cfg(target_os = "macos")]
        {
            home.join("Library/Caches/llama")
        }
        #[cfg(target_os = "windows")]
        {
            home.join(".cache/llama")
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            home.join(".cache/llama")
        }
    };
    if let Ok(size) = dir_size_checked(&llama_cache) {
        if size > 0 {
            items.push(ScanItem {
                path: llama_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "llama缓存".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "llama.cpp 缓存，可安全删除".to_string(),
            });
        }
    }

    items
}

/// 统计 Ollama 已安装的模型数量
///
/// 通过解析 ~/.ollama/models/manifests 目录结构获取
fn count_ollama_models(home: &Path) -> usize {
    let manifests = home.join(".ollama/models/manifests");
    if !manifests.is_dir() {
        return 0;
    }

    let mut count = 0;
    // manifests/<registry>/<library>/<model>:<tag>
    for entry in WalkDir::new(&manifests)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            count += 1;
        }
    }
    count
}

// =========================================================================
//  Monorepo 感知扫描
// =========================================================================

/// Monorepo 标记文件列表
const MONOREPO_MARKERS: &[&str] = &[
    "pnpm-workspace.yaml", // pnpm workspace
    "lerna.json",          // Lerna
    "nx.json",             // Nx
    "turbo.json",          // Turborepo
    "rush.json",           // Rush
];

/// 检测目录是否为 monorepo root
fn is_monorepo_root(dir: &Path) -> bool {
    MONOREPO_MARKERS
        .iter()
        .any(|marker| dir.join(marker).exists())
}

/// 识别 monorepo 类型
fn detect_monorepo_type(dir: &Path) -> &'static str {
    if dir.join("pnpm-workspace.yaml").exists() {
        "pnpm"
    } else if dir.join("lerna.json").exists() {
        "lerna"
    } else if dir.join("nx.json").exists() {
        "nx"
    } else if dir.join("turbo.json").exists() {
        "turbo"
    } else if dir.join("rush.json").exists() {
        "rush"
    } else {
        "unknown"
    }
}

/// 统计 monorepo 中包含 package.json 的子包数量
fn count_monorepo_packages(root: &Path) -> usize {
    WalkDir::new(root)
        .max_depth(3)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() > 0 && e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy();
                // 跳过 node_modules / .git / dist / build
                if name == "node_modules" || name == ".git" || name == "dist" || name == "build" {
                    return false;
                }
            }
            true
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir() && e.path().join("package.json").exists())
        .count()
}

/// 扫描 Monorepo 的所有子包 node_modules
///
/// 在项目搜索路径下查找 monorepo root，聚合展示所有子包的 node_modules
fn scan_monorepo_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();

    for base in get_project_search_paths() {
        // 在 base 下 3 层深度查找 monorepo root
        let mut monorepo_roots: Vec<PathBuf> = Vec::new();

        for entry in WalkDir::new(&base)
            .max_depth(3)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() > 0 && e.file_type().is_dir() {
                    let name = e.file_name().to_string_lossy();
                    if name == "node_modules" || name == ".git" || name == ".svn" || name == ".hg" {
                        return false;
                    }
                }
                true
            })
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_dir() && is_monorepo_root(entry.path()) {
                monorepo_roots.push(entry.path().to_path_buf());
            }
        }

        // 去重：移除嵌套的 monorepo root
        monorepo_roots.sort();
        monorepo_roots.dedup();
        // 先克隆一份用于比较，避免 retain 的可变借用与闭包内的不可变借用冲突
        let all_roots = monorepo_roots.clone();
        monorepo_roots.retain(|root| {
            !all_roots
                .iter()
                .any(|other| other != root && root.starts_with(other))
        });

        // 对每个 monorepo root，扫描所有子包的 node_modules
        for monorepo_root in monorepo_roots {
            let nm_dirs = search_dirs(&monorepo_root, "node_modules", 5);
            if nm_dirs.is_empty() {
                continue;
            }

            let mut total_size: u64 = 0;
            let mut all_paths: Vec<String> = Vec::new();

            for nm in &nm_dirs {
                let size = dir_size(nm);
                if size > 0 {
                    total_size += size;
                    all_paths.push(nm.to_string_lossy().to_string());
                }
            }

            // 只展示 > 50MB 的 monorepo
            if total_size < 50 * 1024 * 1024 {
                continue;
            }

            let mono_type = detect_monorepo_type(&monorepo_root);
            let pkg_count = count_monorepo_packages(&monorepo_root);
            let root_name = monorepo_root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| monorepo_root.to_string_lossy().to_string());

            items.push(ScanItem {
                path: format!("Monorepo: {} ({} 个子包)", root_name, pkg_count),
                size_bytes: total_size,
                category: "Monorepo依赖".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: all_paths,
                recommend: Recommend::Safe,
                description: format!(
                    "{} monorepo，包含 {} 个子包的 node_modules\n\
                     删除后需在 root 目录执行 {} install 恢复",
                    mono_type,
                    pkg_count,
                    match mono_type {
                        "pnpm" => "pnpm",
                        "lerna" | "nx" | "turbo" | "rush" => "npm",
                        _ => "npm/pnpm",
                    }
                ),
            });
        }
    }

    items
}

// =========================================================================
//  孤儿 LaunchAgent/LaunchDaemon 检测
// =========================================================================

/// 扫描指向已卸载应用的 LaunchAgent/LaunchDaemon plist 文件
///
/// 扫描以下目录中的 .plist 文件：
/// - ~/Library/LaunchAgents（用户级，可删除）
/// - /Library/LaunchAgents（系统级，仅展示不可删）
/// - /Library/LaunchDaemons（系统级，仅展示不可删）
///
/// 使用 `plutil -p` 解析 plist，提取 Program 或 ProgramArguments 中的可执行路径，
/// 若路径指向的程序不存在，则判定为孤儿项。
fn scan_orphaned_launchd() -> Vec<ScanItem> {
    let home = home_dir();
    let scan_dirs: Vec<(PathBuf, bool)> = vec![
        // (目录, 是否用户级可删除)
        (home.join("Library/LaunchAgents"), true),
        (PathBuf::from("/Library/LaunchAgents"), false),
        (PathBuf::from("/Library/LaunchDaemons"), false),
    ];

    let mut items = Vec::new();

    for (dir, is_user_level) in scan_dirs {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.is_empty() || !name.ends_with(".plist") {
                continue;
            }

            // 使用 plutil -p 解析 plist
            let program_path = match extract_program_from_plist(&path) {
                Some(p) => p,
                None => continue,
            };

            // 检查程序是否存在
            if std::path::Path::new(&program_path).exists() {
                continue;
            }

            // 程序不存在 → 孤儿项
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

            let level_label = if is_user_level {
                "用户级"
            } else {
                "系统级"
            };
            let category = format!("孤儿服务-{}", level_label);

            let (deletable, reason, recommend) = if is_user_level {
                (true, String::new(), Recommend::Safe)
            } else {
                (
                    false,
                    "系统级 LaunchDaemon/Agent，需在终端用 sudo 手动删除".to_string(),
                    Recommend::Advanced,
                )
            };

            items.push(ScanItem {
                path: path.to_string_lossy().to_string(),
                size_bytes: size,
                category,
                selected: false,
                deletable,
                undeletable_reason: reason,
                batch_paths: Vec::new(),
                recommend,
                description: format!(
                    "指向 {} 的服务已失效（程序已被卸载），{}",
                    program_path,
                    if is_user_level {
                        "可安全删除此 plist".to_string()
                    } else {
                        format!("需手动清理：sudo rm \"{}\"", path.display())
                    }
                ),
            });
        }
    }

    items
}

/// 从 plist 文件中提取程序路径
///
/// 使用 `plutil -p` 输出人类可读格式，然后查找 Program 或 ProgramArguments 键。
/// plutil -p 输出格式示例：
///   "Program" => "/usr/bin/foo"
///   "ProgramArguments" => [
///     0 => "/usr/bin/foo"
///     1 => "-bar"
///   ]
fn extract_program_from_plist(plist_path: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/plutil")
        .arg("-p")
        .arg(plist_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut in_program_args = false;

    for line in stdout.lines() {
        let trimmed = line.trim();

        // 优先匹配 "Program" => "/path"（单值键）
        if trimmed.contains("\"Program\"") && trimmed.contains("=>") {
            if let Some(p) = extract_quoted_value(trimmed) {
                return Some(p);
            }
        }

        // 跟踪 ProgramArguments 数组
        if trimmed.contains("\"ProgramArguments\"") {
            in_program_args = true;
            continue;
        }

        if in_program_args {
            // 数组结束
            if trimmed.starts_with(']') {
                in_program_args = false;
                continue;
            }
            // 数组第一个元素（索引 0）通常是可执行路径
            if trimmed.starts_with("0") || trimmed.contains("\"/") {
                if let Some(p) = extract_quoted_value(trimmed) {
                    // 只接受绝对路径
                    if p.starts_with('/') {
                        return Some(p);
                    }
                }
            }
        }
    }

    None
}

/// 从 plutil 输出行中提取引号包裹的路径值
fn extract_quoted_value(line: &str) -> Option<String> {
    // 查找 => 后面的引号字符串
    let after_arrow = line.split("=>").nth(1)?;
    let after_arrow = after_arrow.trim();

    // 匹配 "..." 格式
    if after_arrow.starts_with('"') {
        let rest = &after_arrow[1..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }

    None
}

// =========================================================================
//  .DS_Store 文件清理
// =========================================================================

/// 扫描用户主目录下的 .DS_Store 文件
///
/// .DS_Store 是 Finder 自动生成的目录元数据文件（保存图标位置、排序方式等），
/// 可安全删除，Finder 会在下次访问目录时自动重建。
///
/// 递归扫描主目录（最大深度 5），跳过系统保护目录和大型缓存目录，
/// 将所有 .DS_Store 路径聚合为单个 ScanItem，通过 batch_paths 批量删除。
fn scan_ds_store_files() -> Vec<ScanItem> {
    let home = home_dir();
    let mut ds_store_paths: Vec<String> = Vec::new();
    let mut total_size: u64 = 0;

    // 跳过这些子目录（避免深入无意义的缓存/系统目录）
    let skip_dirs: &[&str] = &[
        ".Trash",
        ".git",
        "Library/Caches",
        "Library/Developer",
        "Library/Application Support",
        "Library/Containers",
        "Library/Group Containers",
        "Library/Mail",
        "Library/Messages",
        "Library/Metadata",
        "Library/Mobile Documents",
        "Library/Photos",
        "Library/Suggestions",
        "Library/VoiceTrigger",
        "node_modules",
        "target",
        ".npm",
        ".cargo",
        ".gradle",
        ".m2",
        "Movies",
        "Pictures",
        "Music",
    ];

    for entry in WalkDir::new(&home)
        .max_depth(5)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() > 0 && e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                // 跳过黑名单目录
                for skip in skip_dirs {
                    if name == *skip {
                        return false;
                    }
                }
                // 跳过隐藏目录（.开头），但允许 .DS_Store 所在的当前层
                if name.starts_with('.') && e.depth() > 0 {
                    return false;
                }
            }
            true
        })
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy();
            if name == ".DS_Store" {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                total_size += size;
                ds_store_paths.push(entry.path().to_string_lossy().to_string());
            }
        }
    }

    if ds_store_paths.is_empty() {
        return Vec::new();
    }

    let count = ds_store_paths.len();

    vec![ScanItem {
        path: format!("~/ 下的 .DS_Store 文件 ({} 个)", count),
        size_bytes: total_size,
        category: "DS_Store".to_string(),
        selected: false,
        deletable: true,
        undeletable_reason: String::new(),
        batch_paths: ds_store_paths,
        recommend: Recommend::Safe,
        description: format!(
            "Finder 自动生成的目录元数据文件，共 {} 个。\n删除后 Finder 会在访问目录时自动重建，无任何风险。",
            count
        ),
    }]
}

// =========================================================================
//  Windows 专属扫描器
// =========================================================================

/// 扫描 WSL2 vhdx 虚拟磁盘文件（Windows 独家卖点）
///
/// WSL2 发行版使用 vhdx 虚拟磁盘文件，删除文件后空间不会自动回收，
/// 需要 `wsl --shutdown` + `diskpart compact` 压缩。这是 maclean Windows 版的差异化功能。
#[cfg(target_os = "windows")]
fn scan_wsl2_vhdx() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let local_appdata = match std::env::var("LOCALAPPDATA") {
        Ok(v) => PathBuf::from(v),
        Err(_) => return items,
    };

    // WSL2 发行版 vhdx 路径: %LOCALAPPDATA%\Packages\{distro}\LocalState\
    let packages_dir = local_appdata.join("Packages");
    if !packages_dir.is_dir() {
        return items;
    }

    // 扫描所有 WSL 相关的包目录
    let wsl_patterns = ["CanonicalGroupLimited", "Microsoft.WSL"];
    for entry in std::fs::read_dir(&packages_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if !wsl_patterns.iter().any(|p| dir_name.contains(p)) {
            continue;
        }

        let local_state = entry.path().join("LocalState");
        if !local_state.is_dir() {
            continue;
        }

        // 查找 vhdx 文件
        for vhdx_entry in std::fs::read_dir(&local_state)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = vhdx_entry.path();
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !filename.ends_with(".vhdx") {
                continue;
            }

            if let Ok(meta) = vhdx_entry.metadata() {
                let size = meta.len();
                if size > 100 * 1024 * 1024 {
                    // > 100MB
                    items.push(ScanItem {
                        path: path.to_string_lossy().to_string(),
                        size_bytes: size,
                        category: "WSL2虚拟磁盘".to_string(),
                        selected: false,
                        deletable: false, // 不可直接删除，需要压缩
                        undeletable_reason: "需要先 wsl --shutdown，再用 diskpart compact 压缩".to_string(),
                        batch_paths: Vec::new(),
                        recommend: Recommend::Advanced,
                        description: format!(
                            "WSL2 发行版虚拟磁盘（{}）。\n删除文件后空间不会自动回收，需要压缩 vhdx 才能释放空间。",
                            filename
                        ),
                    });
                }
            }
        }
    }

    items
}

/// 扫描 Windows 临时文件
#[cfg(target_os = "windows")]
fn scan_windows_temp() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();

    // %TEMP% 目录
    let temp_dir = std::env::var("TEMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Local/Temp"));
    if temp_dir.is_dir() {
        let size = dir_size(&temp_dir);
        if size > 0 {
            items.push(ScanItem {
                path: temp_dir.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Windows临时文件".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Safe,
                description: "Windows 临时文件目录，可安全删除".to_string(),
            });
        }
    }

    // Windows 更新缓存 (%WINDIR%\SoftwareDistribution\Download)
    if let Ok(windir) = std::env::var("WINDIR") {
        let win_update = PathBuf::from(windir).join("SoftwareDistribution/Download");
        if win_update.is_dir() {
            let size = dir_size(&win_update);
            if size > 0 {
                items.push(ScanItem {
                    path: win_update.to_string_lossy().to_string(),
                    size_bytes: size,
                    category: "Windows更新缓存".to_string(),
                    selected: false,
                    deletable: false, // 需要先停 Windows Update 服务
                    undeletable_reason: "需要先停止 Windows Update 服务才能删除".to_string(),
                    batch_paths: Vec::new(),
                    recommend: Recommend::Advanced,
                    description: "Windows Update 下载缓存，需停止 wuauserv 服务后清理".to_string(),
                });
            }
        }
    }

    // 缩略图缓存 (%LOCALAPPDATA%\Microsoft\Windows\Explorer)
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let thumb_cache = PathBuf::from(local_appdata).join("Microsoft/Windows/Explorer");
        if thumb_cache.is_dir() {
            let mut thumb_size: u64 = 0;
            let mut thumb_paths: Vec<String> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&thumb_cache) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let filename = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if filename.starts_with("thumbcache_") || filename.starts_with("iconcache_") {
                        if let Ok(meta) = entry.metadata() {
                            thumb_size += meta.len();
                            thumb_paths.push(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
            if thumb_size > 0 {
                items.push(ScanItem {
                    path: thumb_cache.to_string_lossy().to_string(),
                    size_bytes: thumb_size,
                    category: "Windows缩略图".to_string(),
                    selected: false,
                    deletable: true,
                    undeletable_reason: String::new(),
                    batch_paths: thumb_paths,
                    recommend: Recommend::Safe,
                    description: "Windows 资源管理器缩略图缓存，删除后自动重建".to_string(),
                });
            }
        }
    }

    items
}

/// 扫描 Docker Desktop for Windows 数据
#[cfg(target_os = "windows")]
fn scan_docker_windows() -> Vec<ScanItem> {
    let mut items = Vec::new();

    // Docker Desktop 数据: %APPDATA%\Docker\wsl\data\ext4.vhdx
    if let Ok(appdata) = std::env::var("APPDATA") {
        let docker_data = PathBuf::from(appdata).join("Docker/wsl/data");
        if docker_data.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&docker_data) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let filename = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if filename.ends_with(".vhdx") {
                        if let Ok(meta) = entry.metadata() {
                            let size = meta.len();
                            if size > 0 {
                                items.push(ScanItem {
                                    path: path.to_string_lossy().to_string(),
                                    size_bytes: size,
                                    category: "Docker虚拟机".to_string(),
                                    selected: false,
                                    deletable: false,
                                    undeletable_reason: "需要先退出 Docker Desktop 并运行 docker system prune -a".to_string(),
                                    batch_paths: Vec::new(),
                                    recommend: Recommend::Advanced,
                                    description: format!(
                                        "Docker Desktop WSL2 虚拟磁盘（{}）。\n建议: docker system prune -a 清理后压缩 vhdx",
                                        filename
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    items
}
