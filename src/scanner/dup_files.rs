//! 重复文件扫描器（P3）
//!
//! 在用户主目录下查找内容相同的文件（大小 >= 1MB），每组保留一个，
//! 其余作为可清理副本进入废纸篓 —— 可恢复，与 maclean 的 soft-delete
//! 理念一致（MangoDisk 是永久删除，这是我们的差异优势）。
//!
//! 算法（借鉴 MangoDisk 但更省 IO）：
//! 1. walkdir 收集主目录下大小 >= 阈值的普通文件（跳过系统/应用目录与自身缓存）
//! 2. 按文件大小分组
//! 3. 组内 >1 个时，先哈希前 64KB 预筛（避免对大文件重复全量哈希）
//! 4. 预筛同组内再全量 SHA256 确认（杜绝哈希碰撞）
//!
//! 安全边界：
//! - 只扫描用户主目录，绝不扫描 /、/System、/Library、/Applications 等系统根
//! - 排除 symlink（防指向系统文件的链接被误算/误删）
//! - 排除应用包与已知缓存根，避免把"应用本体"当重复文件
//! - 每组保留一份：被删的都是副本，保留文件不动

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;
use sha2::{Digest, Sha256};

use super::{has_home, home_dir, Recommend, ScanItem, ScanResult, Scanner};
// 清单管理目录判定与删除阶段共用 safety 第 4.6 层，保证「扫不进/删不掉」一致
use crate::safety::is_manifest_managed_path;

/// 参与重复检测的最小文件大小（1MB）
const MIN_SIZE: u64 = 1024 * 1024;
/// 预筛哈希只读前 64KB
const PREFIX_LEN: u64 = 64 * 1024;
/// 最多保留的重复组数（防内存/IO 失控）
const MAX_GROUPS: usize = 200;

/// 跳过的主目录子树（相对 home）
const SKIP_REL: &[&str] = &[
    "Library/Containers",
    "Library/Application Support",
    "Library/Developer/CoreSimulator",
    "Library/Developer/Xcode/DerivedData",
    "Library/Caches",
    "Library/WebKit",
    "node_modules",
    ".Trash",
    ".git",
    "go/pkg/mod",
    ".rustup",
    ".cargo/registry",
];

/// 跳过的主目录直接子项名（如应用包）
const SKIP_DIR_NAMES: &[&str] = &[
    "Applications",
    "Desktop",
    "Movies",
    "Pictures",
    "Music",
    "Library",
    ".Trash",
    ".cache",
    ".local",
    ".npm",
    ".cargo",
    ".gradle",
    ".rustup",
];

/// 重复文件扫描器
#[derive(Debug, Default)]
pub struct DuplicateFileScanner;

impl DuplicateFileScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for DuplicateFileScanner {
    fn scan(&self) -> ScanResult {
        let start = Instant::now();
        if !has_home() {
            return ScanResult {
                items: Vec::new(),
                total_size: 0,
                scan_time_ms: 0,
            };
        }
        let home = home_dir();
        // 提质：用户配置的重复文件忽略名单（config.json -> dup_ignore_patterns）
        let ignore_patterns: Vec<String> = crate::config::load_config().dup_ignore_patterns;

        // 1. 收集候选文件（大小分组）
        let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
        let walker = walkdir::WalkDir::new(&home)
            .min_depth(1)
            .max_depth(14)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !should_skip_dir(e, &home));
        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();
            if entry.file_type().is_symlink() {
                continue;
            }
            if entry.file_type().is_dir() {
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            // 止血：清单管理的包目录（site-packages / dist-packages /
            // node_modules / .venv* / .terraform / go/pkg/mod / *.app/Contents）
            // 任意层级硬排除 —— 这些目录由 RECORD / package-lock.json /
            // go.sum 等清单管理，删任一份副本都会破坏完整性校验。
            if is_manifest_managed_path(path) {
                continue;
            }
            // 提质：用户可配置忽略名单（子串匹配路径，命中即跳过）
            if !ignore_patterns.is_empty()
                && ignore_patterns
                    .iter()
                    .any(|pat| path.to_string_lossy().contains(pat.as_str()))
            {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let size = meta.len();
            if size >= MIN_SIZE {
                by_size.entry(size).or_default().push(path.to_path_buf());
            }
        }

        // 2. 大小组内预筛（前 64KB 哈希）→ 3. 全量哈希确认
        let candidates: Vec<_> = by_size
            .into_par_iter()
            .filter(|(_, paths)| paths.len() > 1)
            .map(|(_, paths)| find_duplicate_groups(paths))
            .collect();

        // 4. 组装 ScanItem（每组一个聚合项）
        let mut items: Vec<ScanItem> = Vec::new();
        for mut group in candidates.into_iter().take(MAX_GROUPS) {
            if group.len() < 2 {
                continue;
            }
            // 提质：按包根分桶 —— 同一包根下出现 ≥2 份副本说明是包内镜像/
            // 硬链接场景，宁可不省空间，整组不呈现（删任一份都可能破坏包
            // 一致性）。
            let mut buckets: std::collections::HashMap<PathBuf, usize> =
                std::collections::HashMap::new();
            for p in &group {
                if let Some(b) = package_bucket(p) {
                    *buckets.entry(b).or_insert(0) += 1;
                }
            }
            if buckets.values().any(|&c| c >= 2) {
                continue;
            }
            // 跨包根（或普通副本）：保留 mtime 最新的一份 —— 用户最近触碰的
            // 更可能是"正在用"的原始文件；平局按路径较短者优先（更可能是根）。
            group.sort_by(|a, b| {
                let mt_a = a
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let mt_b = b
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                mt_b.cmp(&mt_a).then_with(|| {
                    a.to_string_lossy()
                        .len()
                        .cmp(&b.to_string_lossy().len())
                        .then(a.cmp(b))
                })
            });
            let keep = group.remove(0);
            let total_dup: u64 = group
                .iter()
                .map(|p| p.metadata().map(|m| m.len()).unwrap_or(0))
                .sum();
            let batch: Vec<String> = group
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            let keep_name = keep
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            items.push(ScanItem {
                path: keep.to_string_lossy().into_owned(),
                size_bytes: total_dup,
                category: "重复文件".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                recommend: Recommend::Safe,
                // 对齐 + 提质：文案与行为一致（强制走废纸篓，可恢复），
                // 并完整写"保留了谁、删了谁"供用户核对。
                description: format!(
                    "发现 {} 个相同文件，保留 {}（mtime 最新），其余 {} 个副本移入废纸篓（可恢复）：{}",
                    batch.len() + 1,
                    keep_name,
                    batch.len(),
                    batch.join(", ")
                ),
                batch_paths: batch,
            });
        }

        items.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        ScanResult {
            items,
            total_size,
            scan_time_ms: start.elapsed().as_millis() as u64,
        }
    }
}

/// 是否应跳过该目录子树（安全 + 效率）
fn should_skip_dir(entry: &walkdir::DirEntry, home: &Path) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    let path = entry.path();
    if path == home {
        return false;
    }
    // 止血：清单管理目录任意层级剪枝（提前整棵跳过，避免逐文件过滤）
    if is_manifest_managed_path(path) {
        return true;
    }
    let rel = path.strip_prefix(home).unwrap_or(path);
    let rel_str = rel.to_string_lossy();
    if SKIP_REL.iter().any(|s| rel_str.starts_with(s)) {
        return true;
    }
    if rel.components().count() == 1
        && SKIP_DIR_NAMES
            .iter()
            .any(|d| rel_str == *d || rel_str.starts_with(&format!("{}/", d)))
    {
        return true;
    }
    false
}

/// 组内找重复：先预筛（前 64KB），再全量哈希
fn find_duplicate_groups(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    // 预筛：前 64KB 哈希 → 只有预筛相同才全量哈希
    let prefix_hashed: Vec<(PathBuf, String)> = paths
        .into_par_iter()
        .filter_map(|p| partial_hash(&p).map(|h| (p, h)))
        .collect();

    let mut prefix_groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (p, h) in prefix_hashed {
        prefix_groups.entry(h).or_default().push(p);
    }

    let mut result = Vec::new();
    for (_, group) in prefix_groups {
        if group.len() < 2 {
            continue;
        }
        // 全量哈希
        let mut full: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for p in group {
            if let Some(h) = full_hash(&p) {
                full.entry(h).or_default().push(p);
            }
        }
        for (_, dup) in full {
            if dup.len() >= 2 {
                result.extend(dup);
            }
        }
    }
    result
}

/// 清单管理目录判定统一走 safety::is_manifest_managed_path（第 4.6 层）。
/// 命中即该路径（或该子树）不参与重复检测，也与删除阶段共用同一判定。
/// 提质：推断副本所属的"包根"（管理目录标记的完整前缀）
///
/// 与 is_manifest_managed_path 同源标记；此处用于组内分桶 ——
/// 同包根 ≥2 份副本时整组跳过，跨包根才进入保留者选择。
fn package_bucket(path: &Path) -> Option<PathBuf> {
    let comps: Vec<PathBuf> = path
        .components()
        .map(|c| PathBuf::from(c.as_os_str()))
        .collect();
    for (i, c) in comps.iter().enumerate() {
        let name = c.to_string_lossy();
        if name == "site-packages"
            || name == "dist-packages"
            || name == "node_modules"
            || name == ".terraform"
        {
            return Some(comps[..=i].iter().collect());
        }
        if name.starts_with(".venv")
            && (name.len() == 5 || name[5..].chars().all(|ch| ch.is_ascii_digit()))
        {
            return Some(comps[..=i].iter().collect());
        }
        if name == "go"
            && comps
                .get(i + 1)
                .map(|s| s.to_string_lossy().as_ref() == "pkg")
                .unwrap_or(false)
            && comps
                .get(i + 2)
                .map(|s| s.to_string_lossy().as_ref() == "mod")
                .unwrap_or(false)
        {
            return Some(comps[..=i + 2].iter().collect());
        }
        if name.ends_with(".app")
            && comps
                .get(i + 1)
                .map(|s| s.to_string_lossy().as_ref() == "Contents")
                .unwrap_or(false)
        {
            return Some(comps[..=i + 1].iter().collect());
        }
    }
    None
}

/// 前 N 字节哈希（预筛）
fn partial_hash(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; PREFIX_LEN as usize];
    let mut hasher = Sha256::new();
    let mut read = 0usize;
    loop {
        let n = f.read(&mut buf[..]).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        read += n;
        if read as u64 >= PREFIX_LEN {
            break;
        }
    }
    Some(format!("{:x}", hasher.finalize()))
}

/// 全量哈希
fn full_hash(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_and_full_hash_agree_on_identical_files() {
        let tmp = std::env::temp_dir().join(format!("maclean_dup_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let a = tmp.join("a.bin");
        let b = tmp.join("b.bin");
        let data = vec![0xABu8; 2 * 1024 * 1024];
        std::fs::write(&a, &data).unwrap();
        std::fs::write(&b, &data).unwrap();
        assert_eq!(partial_hash(&a), partial_hash(&b));
        assert_eq!(full_hash(&a), full_hash(&b));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn different_content_does_not_collide() {
        let tmp = std::env::temp_dir().join(format!("maclean_dup_test2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let a = tmp.join("a.bin");
        let b = tmp.join("b.bin");
        std::fs::write(&a, vec![0xABu8; 2 * 1024 * 1024]).unwrap();
        std::fs::write(&b, vec![0xCDu8; 2 * 1024 * 1024]).unwrap();
        assert_ne!(full_hash(&a), full_hash(&b));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn manifest_managed_paths_are_excluded_any_depth() {
        // 止血：site-packages / node_modules / .venv* / .terraform /
        // go/pkg/mod / *.app/Contents 任意层级命中即排除，不依赖 home 前缀。
        for p in [
            "/Users/u/.venv/lib/python3.13/site-packages/pkg/a.bin",
            "/Users/u/proj/node_modules/pkg/b.bin",
            "/Users/u/a/b/c/dist-packages/pkg/c.bin",
            "/Users/u/tools/.venv2/bin/d.bin",
            "/Users/u/ops/.terraform/e.bin",
            "/Users/u/gopath/go/pkg/mod/f.bin",
            "/Users/u/App.app/Contents/Resources/g.bin",
        ] {
            assert!(is_manifest_managed_path(Path::new(p)), "应排除: {}", p);
        }
        for p in [
            "/Users/u/Downloads/a.bin",
            "/Users/u/proj/src/h.bin",
            "/Users/u/.venvista/i.bin", // 前缀 .venv 严格匹配：.venvista 不误伤
        ] {
            assert!(!is_manifest_managed_path(Path::new(p)), "不应误排除: {}", p);
        }
    }

    #[test]
    fn package_bucket_groups_same_package_root() {
        // 提质：同一包根应映射到同一桶；不同包根桶不同；普通文件无桶。
        let a = package_bucket(Path::new(
            "/Users/u/p1/.venv/lib/python3.13/site-packages/x/y.bin",
        ));
        let b = package_bucket(Path::new(
            "/Users/u/p1/.venv/lib/python3.13/site-packages/z/w.bin",
        ));
        assert_eq!(a, b, "同一 .venv 下两份副本应同桶");
        let c = package_bucket(Path::new(
            "/Users/u/p2/venv2/lib/python3.13/site-packages/q.bin",
        ));
        assert_ne!(a, c, "不同 .venv 应不同桶");
        assert!(a.is_some());
        assert_eq!(
            package_bucket(Path::new("/Users/u/Downloads/a.bin")),
            None,
            "普通文件无包根"
        );
    }

    #[test]
    fn find_duplicate_groups_returns_only_duplicates() {
        let tmp = std::env::temp_dir().join(format!("maclean_dup_test3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let a = tmp.join("a.bin");
        let b = tmp.join("b.bin");
        let c = tmp.join("c.bin");
        std::fs::write(&a, vec![1u8; 2 * 1024 * 1024]).unwrap();
        std::fs::write(&b, vec![1u8; 2 * 1024 * 1024]).unwrap();
        std::fs::write(&c, vec![2u8; 2 * 1024 * 1024]).unwrap();
        let result = find_duplicate_groups(vec![a.clone(), b.clone(), c.clone()]);
        // a/b 重复进入结果；c 不重复
        assert!(result.contains(&a));
        assert!(result.contains(&b));
        assert!(!result.contains(&c));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
