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
            // 保留最短路径的那个（更可能是"原始"而非深层副本），其余为待删副本
            group.sort_by(|a, b| {
                let al = a.to_string_lossy().len();
                let bl = b.to_string_lossy().len();
                al.cmp(&bl).then(a.cmp(b))
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
            items.push(ScanItem {
                path: keep.to_string_lossy().into_owned(),
                size_bytes: total_dup,
                category: "重复文件".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                recommend: Recommend::Safe,
                description: format!(
                    "发现 {} 个相同文件，将保留 {}，其余 {} 个副本移入废纸篓（可恢复）",
                    batch.len() + 1,
                    keep.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    batch.len()
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
