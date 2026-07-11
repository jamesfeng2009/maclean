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
use std::time::Instant;
use walkdir::WalkDir;

use super::{dir_size, home_dir, Recommend, ScanItem, ScanResult, Scanner};

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

        // 依次扫描各类开发者缓存
        items.extend(scan_rust_caches());
        items.extend(scan_xcode_caches());
        items.extend(scan_node_caches());
        items.extend(scan_go_caches());
        items.extend(scan_homebrew_caches());
        items.extend(scan_pip_caches());
        items.extend(scan_jetbrains_caches());

        // Java/Gradle/Maven 和 Python 缓存
        scan_java_caches(&mut items);
        scan_python_caches(&mut items);

        // 更多语言缓存
        scan_more_dev_caches(&mut items);

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
            recommend: Recommend::Caution,
            description: "Cargo 包下载缓存，删除后编译时需重新下载".to_string(),
        });
    }

    items
}

// =========================================================================
//  Xcode 缓存扫描
// =========================================================================

/// 扫描 Xcode 相关缓存
fn scan_xcode_caches() -> Vec<ScanItem> {
    let mut items = Vec::new();
    let home = home_dir();
    let xcode_dir = home.join("Library/Developer/Xcode");

    // 1. DerivedData (Xcode 编译产物)
    let derived_data = xcode_dir.join("DerivedData");
    if derived_data.is_dir() {
        let size = dir_size(&derived_data);
        items.push(ScanItem {
            path: derived_data.to_string_lossy().to_string(),
            size_bytes: size,
            category: "Xcode编译".to_string(),
            selected: false,
            deletable: true,
            recommend: Recommend::Safe,
            description: "Xcode 编译缓存，重新构建会自动恢复".to_string(),
        });
    }

    // 2. iOS DeviceSupport (设备调试支持文件)
    let device_support = xcode_dir.join("iOS DeviceSupport");
    if device_support.is_dir() {
        let size = dir_size(&device_support);
        items.push(ScanItem {
            path: device_support.to_string_lossy().to_string(),
            size_bytes: size,
            category: "Xcode设备".to_string(),
            selected: false,
            deletable: true,
            recommend: Recommend::Caution,
            description: "iOS 设备调试符号，连接设备时会重新生成".to_string(),
        });
    }

    // 3. Archives (Xcode 归档文件)
    let archives = xcode_dir.join("Archives");
    if archives.is_dir() {
        let size = dir_size(&archives);
        items.push(ScanItem {
            path: archives.to_string_lossy().to_string(),
            size_bytes: size,
            category: "Xcode归档".to_string(),
            selected: false,
            deletable: true,
            recommend: Recommend::Advanced,
            description: "Xcode 归档文件，包含已发布 App 的归档".to_string(),
        });
    }

    // 4. 模拟器镜像 (系统级目录，不可直接删除，提示用 xcrun 删除)
    let sim_volumes = PathBuf::from("/Library/Developer/CoreSimulator/Volumes");
    if sim_volumes.is_dir() {
        let size = dir_size(&sim_volumes);
        items.push(ScanItem {
            path: sim_volumes.to_string_lossy().to_string(),
            size_bytes: size,
            category: "模拟器镜像".to_string(),
            selected: false,
            deletable: false, // 系统级，需用 xcrun simctl runtime delete 删除
            recommend: Recommend::Advanced,
            description: "iOS 模拟器运行时镜像，需用 xcrun simctl 删除".to_string(),
        });
    }

    // 5. 模拟器缓存
    let sim_caches = PathBuf::from("/Library/Developer/CoreSimulator/Caches");
    if sim_caches.is_dir() {
        let size = dir_size(&sim_caches);
        items.push(ScanItem {
            path: sim_caches.to_string_lossy().to_string(),
            size_bytes: size,
            category: "模拟器缓存".to_string(),
            selected: false,
            deletable: true,
            recommend: Recommend::Safe,
            description: "模拟器缓存，可安全删除".to_string(),
        });
    }

    items
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
                recommend: Recommend::Safe,
                description: "Node.js 依赖包，npm install 可恢复".to_string(),
            });
        }
    }

    // 2. ~/Library/pnpm/store (pnpm 全局存储)
    let pnpm_store = home.join("Library/pnpm/store");
    if pnpm_store.is_dir() {
        let size = dir_size(&pnpm_store);
        items.push(ScanItem {
            path: pnpm_store.to_string_lossy().to_string(),
            size_bytes: size,
            category: "pnpm缓存".to_string(),
            selected: false,
            deletable: true,
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
            recommend: Recommend::Safe,
            description: "Homebrew 下载缓存，可安全删除".to_string(),
        });
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

    // ~/Library/Caches/pip (pip 下载缓存)
    let pip_cache = home.join("Library/Caches/pip");
    if pip_cache.is_dir() {
        let size = dir_size(&pip_cache);
        items.push(ScanItem {
            path: pip_cache.to_string_lossy().to_string(),
            size_bytes: size,
            category: "pip缓存".to_string(),
            selected: false,
            deletable: true,
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
            recommend: Recommend::Safe,
            description: "JetBrains IDE 缓存，重启 IDE 会自动重建".to_string(),
        });
    }

    items
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
/// 从用户 home 目录下的多个常见工作区路径中搜索项目:
/// - ~/Downloads/myproject/workspace
/// - ~/Downloads/myStudy/project
/// - ~/FrontProject
/// - ~/Desktop
///
/// 仅返回实际存在的目录。
fn get_project_search_paths() -> Vec<PathBuf> {
    let home = home_dir();
    let candidates = [
        home.join("Downloads/myproject/workspace"),
        home.join("Downloads/myStudy/project"),
        home.join("FrontProject"),
        home.join("Desktop"),
    ];

    candidates
        .into_iter()
        .filter(|p| p.is_dir())
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
                if size > 10 * 1024 * 1024 { // > 10MB
                    items.push(ScanItem {
                        path: dir.to_string_lossy().to_string(),
                        size_bytes: size,
                        category: "Java编译".to_string(),
                        selected: false,
                        deletable: true,
                        recommend: Recommend::Safe,
                        description: "Java/Gradle 项目编译产物，gradle build 会自动重新生成".to_string(),
                    });
                }
            }
        }
    }
}

/// 扫描 Python 缓存
fn scan_python_caches(items: &mut Vec<ScanItem>) {
    let home = home_dir();

    // pip 缓存 (~/Library/Caches/pip)
    let pip_cache = home.join("Library/Caches/pip");
    if let Ok(size) = dir_size_checked(&pip_cache) {
        if size > 0 {
            items.push(ScanItem {
                path: pip_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "pip缓存".to_string(),
                selected: false,
                deletable: true,
                recommend: Recommend::Safe,
                description: "pip 下载缓存，可安全删除".to_string(),
            });
        }
    }

    // Conda 缓存 (~/.conda)
    let conda_pkgs = home.join(".conda/pkgs");
    if let Ok(size) = dir_size_checked(&conda_pkgs) {
        if size > 0 {
            items.push(ScanItem {
                path: conda_pkgs.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Conda缓存".to_string(),
                selected: false,
                deletable: true,
                recommend: Recommend::Caution,
                description: "Conda 包缓存，删除后安装时需重新下载".to_string(),
            });
        }
    }

    // Poetry 缓存 (~/Library/Caches/pypoetry)
    let poetry_cache = home.join("Library/Caches/pypoetry");
    if let Ok(size) = dir_size_checked(&poetry_cache) {
        if size > 0 {
            items.push(ScanItem {
                path: poetry_cache.to_string_lossy().to_string(),
                size_bytes: size,
                category: "Poetry缓存".to_string(),
                selected: false,
                deletable: true,
                recommend: Recommend::Safe,
                description: "Poetry 依赖缓存，可安全删除".to_string(),
            });
        }
    }

    // Python __pycache__ 目录
    for search_path in get_project_search_paths() {
        let pycache_dirs = search_dirs(&search_path, "__pycache__", 5);
        let mut total_py_size: u64 = 0;
        let mut py_paths: Vec<String> = Vec::new();
        for dir in &pycache_dirs {
            if let Ok(size) = dir_size_checked(dir) {
                total_py_size += size;
                py_paths.push(dir.to_string_lossy().to_string());
            }
        }
        if total_py_size > 10 * 1024 * 1024 { // > 10MB 才展示
            items.push(ScanItem {
                path: format!("{}个 __pycache__ 目录", py_paths.len()),
                size_bytes: total_py_size,
                category: "Python缓存".to_string(),
                selected: false,
                deletable: false, // 多个目录无法一键删除
                recommend: Recommend::Safe,
                description: "Python 字节码缓存，运行时自动重建，需手动清理".to_string(),
            });
        }
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

/// 扫描更多语言/工具的缓存
/// 覆盖 Ruby/PHP/Flutter/Swift/CocoaPods/CMake/Docker 等
fn scan_more_dev_caches(items: &mut Vec<ScanItem>) {
    let home = home_dir();

    // 定义缓存路径列表：(路径, 类别, 推荐等级, 描述)
    let cache_dirs: Vec<(&str, &str, Recommend, &str)> = vec![
        // Ruby
        ("~/.gem", "RubyGems", Recommend::Caution, "Ruby Gem 缓存，删除后安装时需重新下载"),
        ("~/.bundle/cache", "Bundler", Recommend::Safe, "Ruby Bundler 缓存，可安全删除"),
        ("~/.rbenv/versions", "rbenv", Recommend::Advanced, "rbenv 安装的 Ruby 版本，请确认后删除"),
        // PHP
        ("~/.composer/cache", "Composer", Recommend::Safe, "PHP Composer 下载缓存，可安全删除"),
        // Flutter/Dart
        ("~/.pub-cache", "Flutter/Dart", Recommend::Caution, "Dart/Flutter 包缓存，删除后需重新下载"),
        // Swift Package Manager
        ("~/.swiftpm", "SwiftPM", Recommend::Safe, "Swift Package Manager 缓存，可安全删除"),
        // CocoaPods
        ("~/Library/Caches/CocoaPods", "CocoaPods", Recommend::Safe, "CocoaPods 缓存，可安全删除"),
        // CMake
        ("~/.cmake", "CMake", Recommend::Safe, "CMake 缓存，可安全删除"),
        // Docker
        ("~/Library/Containers/com.docker.docker/Data/vms", "Docker", Recommend::Advanced, "Docker 虚拟机数据，请确认后删除"),
        // Android SDK
        ("~/Library/Android/sdk/system-images", "AndroidSDK", Recommend::Caution, "Android 模拟器系统镜像，删除后需重新下载"),
        // Yarn (非 nodejs 的独立缓存)
        ("~/.yarn/cache", "Yarn", Recommend::Safe, "Yarn 包缓存，可安全删除"),
        // Deno
        ("~/Library/Caches/deno", "Deno", Recommend::Safe, "Deno 缓存，可安全删除"),
        // Bun
        ("~/.bun/install/cache", "Bun", Recommend::Safe, "Bun 包缓存，可安全删除"),
    ];

    for (path_str, category, recommend, desc) in cache_dirs {
        let full_path = path_str.replace("~", &home.to_string_lossy());
        let path = Path::new(&full_path);
        if let Ok(size) = dir_size_checked(path) {
            if size > 0 {
                items.push(ScanItem {
                    path: full_path,
                    size_bytes: size,
                    category: category.to_string(),
                    selected: false,
                    deletable: true,
                    recommend,
                    description: desc.to_string(),
                });
            }
        }
    }

    // 通用构建产物目录扫描（dist、build、.next、.nuxt、.turbo、.svelte-kit）
    for search_path in get_project_search_paths() {
        let build_dir_names = vec![
            ("dist", "构建产物", Recommend::Safe, "前端构建产物，npm run build 会重新生成"),
            (".next", "Next.js", Recommend::Safe, "Next.js 构建缓存，可安全删除"),
            (".nuxt", "Nuxt.js", Recommend::Safe, "Nuxt.js 构建缓存，可安全删除"),
            (".turbo", "Turborepo", Recommend::Safe, "Turborepo 缓存，可安全删除"),
            (".svelte-kit", "SvelteKit", Recommend::Safe, "SvelteKit 构建缓存，可安全删除"),
            (".astro", "Astro", Recommend::Safe, "Astro 构建缓存，可安全删除"),
            (".remix", "Remix", Recommend::Safe, "Remix 构建缓存，可安全删除"),
            (".gradle", "Gradle项目", Recommend::Safe, "Gradle 项目本地缓存，可安全删除"),
        ];

        for (dir_name, category, recommend, desc) in &build_dir_names {
            let found_dirs = search_dirs(&search_path, dir_name, 4);
            for dir in found_dirs {
                // 排除 node_modules 内的目录
                if dir.to_string_lossy().contains("node_modules") {
                    continue;
                }
                if let Ok(size) = dir_size_checked(&dir) {
                    if size > 10 * 1024 * 1024 { // > 10MB
                        items.push(ScanItem {
                            path: dir.to_string_lossy().to_string(),
                            size_bytes: size,
                            category: category.to_string(),
                            selected: false,
                            deletable: true,
                            recommend: *recommend,
                            description: desc.to_string(),
                        });
                    }
                }
            }
        }
    }
}
