//! 磁盘分析器
//!
//! 产出两类结果，供磁盘分析页「大文件 / 大目录」两个视图使用：
//! - **大文件**（[`scan_large_files`]）：递归遍历用户主目录，收集单个体积
//!   ≥ [`LARGE_FILE_THRESHOLD`] 的常规文件（视频 / 磁盘镜像 / 压缩包 / AI 模型 /
//!   安装包等），按类型打 category。这是概览「磁盘大文件」入口的口径。
//! - **大目录**（[`scan_directory`]）：列出主目录直接子项（文件 + 目录）并按
//!   占用排序，Mole / DaisyDisk 式的顶层占用排行。
//!
//! 安全 / 性能：
//! - 大文件递归会尝试进入 Downloads / Documents / Desktop / Movies / Pictures /
//!   Music（个人大文件集中区）；无 TCC 权限时 `read_dir` 直接报错，静默剪枝，
//!   不触发崩溃；`Library` 等系统/应用数据目录整体跳过（归属缓存/应用扫描）。
//! - 一律跳过隐藏目录、包管理/构建/缓存依赖（node_modules / target / .git /
//!   site-packages / go pkg/mod 等），这些属于「智能清理」的范畴，避免淹没列表。
//! - 跳过 .app / .photoslibrary 等 bundle 目录（应用与资源库应走卸载/专门入口）。
//! - 大目录大小走磁盘分析专用的 [`super::dir_size_accurate`]：分层并行 BFS
//!   （rayon）、st_blocks×512 真实占用（APFS 稀疏镜像不虚高）、**不做深度截断**、
//!   单目录 20s 宽松超时 + 黑名单 7 天 TTL 自愈，尽量给出准确体量；个别磁盘忙 /
//!   被占用而无法访问的子树会以 incomplete 如实标注「至少占用」，不再像早期
//!   10s 看门狗实现那样一次偶发卡顿就把整棵大目录判小或永久记 0。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;
use walkdir::WalkDir;

use super::{
    format_size, has_home, home_dir, sizecache::dir_size_accurate_cached, Recommend, ScanItem,
    ScanResult, Scanner,
};

/// 大文件阈值：单个文件 ≥ 100MB 才计入「大文件」视图
const LARGE_FILE_THRESHOLD: u64 = 100 * 1024 * 1024;

/// 大文件最多保留多少项（IPC 体积与渲染上限，按大小降序截断）
const LARGE_FILE_LIMIT: usize = 200;

/// 最小展示大小（大目录视图）：1MB（小于此值的子项不展示，避免列表过长）
const MIN_DISPLAY_SIZE: u64 = 1024 * 1024;

/// 包管理 / 构建 / 缓存类依赖目录名：大文件视图跳过（归属「智能清理」）
const DEP_CACHE_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    ".cache",
    ".gradle",
    ".m2",
    ".ivy",
    ".nuget",
    "__pycache__",
    "site-packages",
    "venv",
    ".venv",
    "Pods",
    ".yarn",
    ".pnpm-store",
    ".npm",
];

/// 主目录第一层下整体跳过的目录：系统 / 应用数据与 TCC 保护、非个人大文件区
fn skip_top_level_dir(name: &str) -> bool {
    matches!(
        name,
        "Library" | "Public" | "Applications" | "Sites"
    )
}

/// bundle / 资源库目录后缀：进入后会被拆散成内部文件，且应走卸载/专门入口
fn is_bundle_dir(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".photoslibrary")
        || lower.ends_with(".musiclibrary")
        || lower.ends_with(".tvlibrary")
        || lower.ends_with(".podcastlibrary")
        || lower.ends_with(".aplibrary")
        || lower.ends_with(".fcpbundle")
        || lower.ends_with(".logicx")
        || lower.ends_with(".band")
        || lower.ends_with(".app")
        || lower.ends_with(".xcworkspace")
        || lower.ends_with(".xcodeproj")
        || lower.ends_with(".bundle")
}

/// 大文件递归时，某个目录是否应被剪枝（不进入）。
///
/// - `depth` 为该目录相对扫描根的层级（根自身为 0，直接子项为 1）。
/// - 隐藏目录、依赖/缓存目录、bundle 目录一律跳过；
/// - 第一层额外跳过系统/应用数据目录；
/// - Go 模块缓存 `go/pkg/mod` 按路径跳过（`pkg` 单名太常见，不按名字误伤）。
fn skip_large_file_dir(path: &Path, name: &str, depth: usize, root: &Path) -> bool {
    if name.starts_with('.') {
        return true;
    }
    if depth == 1 && (skip_top_level_dir(name) || is_bundle_dir(name)) {
        return true;
    }
    if DEP_CACHE_DIRS.contains(&name) {
        return true;
    }
    if is_bundle_dir(name) {
        return true;
    }
    // Go 模块缓存：<root>/go/pkg/mod
    if let Ok(rel) = path.strip_prefix(root) {
        let parts: Vec<&str> = rel.iter().filter_map(|s| s.to_str()).collect();
        if parts.len() >= 3 && parts[0] == "go" && parts[1] == "pkg" && parts[2] == "mod" {
            return true;
        }
    }
    false
}

/// 按扩展名给大文件分类（决定环形图占比与 Badge 类别）
fn file_category(name: &str) -> &'static str {
    let ext = match name.rsplit_once('.') {
        Some((_, e)) => e.to_lowercase(),
        None => return "其它",
    };
    let cat = match ext.as_str() {
        // 视频
        "mp4" | "mov" | "mkv" | "avi" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg"
        | "3gp" | "ts" | "rmvb" | "rm" => "视频",
        // 音频
        "mp3" | "wav" | "flac" | "ape" | "aac" | "m4a" | "ogg" | "wma" => "音频",
        // 磁盘镜像 / 虚拟机
        "dmg" | "iso" | "img" | "qcow2" | "qcow" | "vmdk" | "vdi" | "vhd" | "vhdx"
        | "raw" | "sparseimage" | "sparsebundle" => "磁盘镜像",
        // 压缩包
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" | "lz4" => "压缩包",
        // AI / ML 模型权重
        "gguf" | "safetensors" | "onnx" | "pb" | "ckpt" | "pt" | "pth" | "h5" | "hdf5"
        | "tflite" | "ggml" => "AI模型",
        // 安装包
        "pkg" | "mpkg" => "安装包",
        // 数据集 / 数据库导出
        "sql" | "db" | "sqlite" | "dump" | "csv" | "parquet" | "arrow" => "数据集",
        _ => "其它",
    };
    cat
}

/// 递归收集 `root` 下单个文件 ≥ `threshold` 字节的常规文件（按大小降序，截断上限）。
///
/// 阈值参数化以便单测用小值快速构造；生产用 [`LARGE_FILE_THRESHOLD`]。
fn collect_large_files_under(root: &Path, threshold: u64) -> Vec<(PathBuf, u64)> {
    let mut out: Vec<(PathBuf, u64)> = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // 根自身放行；目录按剪枝规则过滤；文件一律放行（.dmg/.iso 等要保留）
            let depth = e.depth();
            if depth == 0 {
                return true;
            }
            if e.file_type().is_dir() {
                let p = e.path();
                // 网络/FUSE 挂载点（如 OrbStack 的 ~/OrbStack NFS）、TCC 容器：
                // 挂载点是真目录、follow_links(false) 挡不住，必须在此显式剪枝，
                // 否则 readdir/stat 全部走网络会把整个大文件扫描永久拖死。
                if crate::scanner::fs_guard::should_skip_traversal(p) {
                    return false;
                }
                let name = e.file_name().to_str().unwrap_or("");
                !skip_large_file_dir(p, name, depth, root)
            } else {
                true
            }
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| match e.metadata() {
            Ok(md) if md.len() >= threshold => Some((e.path().to_path_buf(), md.len())),
            _ => None,
        })
        .collect();
    out.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
    out.truncate(LARGE_FILE_LIMIT);
    out
}

/// 大文件视图：扫描用户主目录下的超大单文件
fn scan_large_files() -> ScanResult {
    let start = Instant::now();
    if !has_home() {
        return ScanResult { items: Vec::new(), total_size: 0, scan_time_ms: 0 };
    }
    let home = home_dir();
    let res = catch_unwind(AssertUnwindSafe(|| {
        collect_large_files_under(&home, LARGE_FILE_THRESHOLD)
    }))
    .unwrap_or_default();

    crate::log_scan_step(&format!("磁盘分析: 大文件扫描完成, {} 个 ≥100MB 文件", res.len()));

    let mut total_size = 0u64;
    let items = res
        .into_iter()
        .map(|(path, size)| {
            total_size += size;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let category = file_category(&name);
            ScanItem { batch_mtimes: vec![],
                path: path.to_string_lossy().to_string(),
                size_bytes: size,
                category: category.to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Advanced,
                description: format!("{category}文件 · {}", format_size(size)),
            }
        })
        .collect();

    ScanResult { items, total_size, scan_time_ms: start.elapsed().as_millis() as u64 }
}

/// 已知会导致崩溃、极慢或权限问题的目录/文件（大目录视图沿用的历史过滤）
fn is_problematic_path(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".photoslibrary")
        || lower.ends_with(".musiclibrary")
        || lower.ends_with(".tvlibrary")
        || lower.ends_with(".podcastlibrary")
        || lower.ends_with(".aplibrary")
        || lower.ends_with(".fcpbundle")
        || lower.ends_with(".logicx")
        || lower.ends_with(".band")
        || lower.ends_with(".app")
        || lower.ends_with(".xcworkspace")
        || lower.ends_with(".xcodeproj")
        || lower.ends_with(".bundle")
        || lower.ends_with(".pkg")
        || lower.ends_with(".dmg")
        || lower.ends_with(".iso")
}

/// 主目录下应跳过的目录名（TCC 保护，访问会触发权限弹窗）
fn should_skip_home_dir(name: &str) -> bool {
    matches!(
        name,
        "Library"
            | "Pictures"
            | "Music"
            | "Movies"
            | "Public"
            | "Applications"
            | "Sites"
            | "Documents"
            | "Desktop"
            | "Downloads"
    )
}

/// 磁盘分析器扫描器
#[derive(Debug, Default)]
pub struct LargeFileScanner;

impl LargeFileScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for LargeFileScanner {
    /// 扫描用户主目录：先递归收集超大单文件（大文件视图），再列出主目录直接
    /// 子项占用排行（大目录视图）。前端以 `category == "目录"` 区分两组。
    fn scan(&self) -> ScanResult {
        if !has_home() {
            return ScanResult {
                items: Vec::new(),
                total_size: 0,
                scan_time_ms: 0,
            };
        }
        let start = Instant::now();
        let home = home_dir();

        // 大文件（递归单文件，按类型分类；放在前，默认视图）
        let mut result = scan_large_files();
        // 大目录（主目录直接子项占用排行，category 恒为「目录」/「文件」）
        let dirs = scan_directory(&home);

        let file_total = result.total_size;
        result.items.extend(dirs.items);
        // 注意：大文件已包含在某些大目录内，两组 total 不可相加展示；前端各视图
        // 分别按自身列表求和。这里 total_size 仅给可能的调试/日志使用。
        result.total_size = file_total;
        result.scan_time_ms = start.elapsed().as_millis() as u64;
        result
    }
}

/// 扫描指定目录下的所有子项（文件 + 目录），按大小降序排列
///
/// 这是磁盘分析器的核心函数。对当前目录的每个子项：
/// - 文件：直接获取大小
/// - 目录：递归计算大小（带超时保护）
///
/// 结果按大小降序排列，便于用户快速定位占用空间的目录/文件。
pub fn scan_directory(path: &Path) -> ScanResult {
    catch_unwind(AssertUnwindSafe(|| scan_directory_impl(path))).unwrap_or_else(|_| ScanResult {
        items: Vec::new(),
        total_size: 0,
        scan_time_ms: 0,
    })
}

fn scan_directory_impl(path: &Path) -> ScanResult {
    let start = Instant::now();
    let mut items = Vec::new();
    crate::log_scan_step(&format!("磁盘分析: 扫描目录 {}", path.display()));

    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => {
            return ScanResult {
                items: Vec::new(),
                total_size: 0,
                scan_time_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    // 收集所有子项路径
    let paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            // 跳过隐藏文件/目录（以 . 开头）
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    return false;
                }
                // 如果是主目录的直接子项，跳过 TCC 保护目录
                if path == home_dir().as_path() && should_skip_home_dir(name) {
                    return false;
                }
                // 跳过问题 bundle
                if is_problematic_path(name) {
                    return false;
                }
            }
            true
        })
        .collect();

    crate::log_scan_step(&format!("磁盘分析: {} 个子项待计算", paths.len()));

    // 并行计算每个子项的大小。大目录走「尽力准确」统计（无深度截断 + 20s 宽松
    // 超时 + 黑名单兜底/自愈），返回 incomplete 标记部分未能访问的子树。
    // 元组：(路径, 大小, 是否目录, 是否统计不完整)
    let sized: Vec<(PathBuf, u64, bool, bool)> = paths
        .par_iter()
        .filter_map(|path| {
            let is_dir = path.is_dir();

            let (size, incomplete) = catch_unwind(AssertUnwindSafe(|| {
                if is_dir {
                    // 只读大目录占用：未变化目录命中持久化缓存秒回，变化子树才重算。
                    dir_size_accurate_cached(path)
                } else {
                    (path.symlink_metadata().map(|m| m.len()).unwrap_or(0), false)
                }
            }))
            // 统计过程 panic（极端 IO 异常）：当作不完整，避免给出误导性精确值
            .unwrap_or((0, true));

            Some((path.clone(), size, is_dir, incomplete))
        })
        .filter(|(_, size, _, _)| *size >= MIN_DISPLAY_SIZE)
        .collect();

    let total_size: u64 = sized.iter().map(|(_, s, _, _)| *s).sum();

    for (path, size, is_dir, incomplete) in sized {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let category = if is_dir { "目录" } else { "文件" };

        let description = if is_dir {
            if incomplete {
                format!(
                    "📁 {} — 至少 {}（部分内容磁盘忙或被占用、暂时无法访问，真实占用可能更大）",
                    name,
                    format_size(size)
                )
            } else {
                format!("📁 {} — {}（可进入查看详情）", name, format_size(size))
            }
        } else {
            format!("📄 {} — {}", name, format_size(size))
        };

        items.push(ScanItem { batch_mtimes: vec![],
            path: path.to_string_lossy().to_string(),
            size_bytes: size,
            category: category.to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Advanced,
            description,
        });
    }

    // 按大小降序排列
    items.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));

    crate::log_scan_step(&format!(
        "磁盘分析: 完成, {} 项, 总计 {}",
        items.len(),
        super::format_size(total_size)
    ));

    let scan_time_ms = start.elapsed().as_millis() as u64;

    ScanResult {
        items,
        total_size,
        scan_time_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// 临时目录 guard：drop 时递归清理，避免污染
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new() -> Self {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!(
                "maclean_large_test_{}_{n}",
                std::process::id()
            ));
            fs::create_dir_all(&p).unwrap();
            TmpDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        /// 相对 root 写入指定字节数的文件
        fn put(&self, rel: &str, bytes: usize) {
            let f = self.0.join(rel);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, vec![b'x'; bytes]).unwrap();
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_file_category() {
        assert_eq!(file_category("movie.MKV"), "视频");
        assert_eq!(file_category("a.mp4"), "视频");
        assert_eq!(file_category("disk.dmg"), "磁盘镜像");
        assert_eq!(file_category("vm.qcow2"), "磁盘镜像");
        assert_eq!(file_category("bundle.zip"), "压缩包");
        assert_eq!(file_category("model.gguf"), "AI模型");
        assert_eq!(file_category("weights.safetensors"), "AI模型");
        assert_eq!(file_category("setup.pkg"), "安装包");
        assert_eq!(file_category("dump.sql"), "数据集");
        assert_eq!(file_category("weird"), "其它");
        assert_eq!(file_category("noext"), "其它");
    }

    #[test]
    fn test_skip_dir_rules() {
        let root = PathBuf::from("/home/tester");
        // 第一层系统/应用数据目录
        assert!(skip_large_file_dir(&root.join("Library"), "Library", 1, &root));
        assert!(skip_large_file_dir(&root.join("Applications"), "Applications", 1, &root));
        // 非保护的第一层普通目录进入
        assert!(!skip_large_file_dir(&root.join("Projects"), "Projects", 1, &root));
        // 任意深度隐藏目录
        assert!(skip_large_file_dir(&root.join("a/.cache"), ".cache", 2, &root));
        // 依赖/缓存目录
        assert!(skip_large_file_dir(&root.join("p/node_modules"), "node_modules", 2, &root));
        assert!(skip_large_file_dir(&root.join("p/.git"), ".git", 3, &root));
        // bundle 目录
        assert!(skip_large_file_dir(&root.join("X.app"), "X.app", 1, &root));
        // Go 模块缓存按路径剪枝，普通名为 pkg 的目录不误伤
        assert!(skip_large_file_dir(
            &root.join("go/pkg/mod"),
            "mod",
            3,
            &root
        ));
        assert!(!skip_large_file_dir(&root.join("Projects/pkg"), "pkg", 2, &root));
    }

    #[test]
    fn test_collect_large_files_threshold_and_prune() {
        let tmp = TmpDir::new();
        let root = tmp.path();
        // 达标文件（threshold=5，下面用字节数精确控制）
        tmp.put("a/big.mkv", 10);
        tmp.put("sub/deep/model.gguf", 8);
        tmp.put("installer.dmg", 9); // 磁盘镜像文件必须保留
        // 低于阈值
        tmp.put("small.txt", 2);
        // 应被剪枝的目录内，即使有大文件也不出现
        tmp.put("node_modules/pkg/big.mkv", 10);
        tmp.put(".hidden/big.mkv", 10);
        tmp.put("Foo.app/Contents/MacOS/big.mkv", 10);
        tmp.put("go/pkg/mod/cache/x.zip", 10);
        tmp.put("Library/Caches/big.mkv", 10);

        let got = collect_large_files_under(root, 5);
        let names: Vec<String> = got
            .iter()
            .map(|(p, _)| p.strip_prefix(root).unwrap().to_string_lossy().to_string())
            .collect();

        assert!(names.contains(&"a/big.mkv".to_string()), "应包含深层视频: {names:?}");
        assert!(names.contains(&"sub/deep/model.gguf".to_string()));
        assert!(names.contains(&"installer.dmg".to_string()), ".dmg 是文件应保留: {names:?}");
        assert!(!names.iter().any(|n| n.contains("small.txt")));
        assert!(!names.iter().any(|n| n.contains("node_modules")), "依赖目录应剪枝: {names:?}");
        assert!(!names.iter().any(|n| n.starts_with(".hidden")), "隐藏目录应剪枝: {names:?}");
        assert!(!names.iter().any(|n| n.contains("Foo.app")), "bundle 应剪枝: {names:?}");
        assert!(!names.iter().any(|n| n.contains("go/pkg/mod")), "go mod 缓存应剪枝: {names:?}");
        assert!(!names.iter().any(|n| n.starts_with("Library")), "Library 应剪枝: {names:?}");

        // 按大小降序
        let sizes: Vec<u64> = got.iter().map(|(_, s)| *s).collect();
        let mut sorted = sizes.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(sizes, sorted, "应按大小降序: {sizes:?}");
        assert_eq!(got.len(), 3);
    }
}
