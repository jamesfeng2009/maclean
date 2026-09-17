//! 扫描结果磁盘缓存
//!
//! 将扫描结果序列化到磁盘，避免每次全量扫描。
//! 缓存有效期为 7 天（TTL），过期后自动失效重新扫描。
//!
//! 缓存文件位置：~/.maclean/scan_cache/<tab_name>.json
//! 文件格式：JSON，包含时间戳和序列化的 ScanResult

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::ScanResult;

/// 缓存有效期：1 天（秒）
///
/// 用户通常当天清理一次后不会再次清理，1 天 TTL 确保下次打开时重新扫描。
/// 配合删除后自动 invalidate_cache，保证清理后立即看到最新数据。
const CACHE_TTL_SECS: u64 = 24 * 60 * 60;

/// 缓存版本号
///
/// 变更扫描结果结构或过滤逻辑时递增此版本号，使旧缓存自动失效。
const CACHE_VERSION: u32 = 8;

/// 缓存文件的 JSON 包装结构
#[derive(serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    /// 缓存写入时间（UNIX 时间戳，秒）
    timestamp: u64,
    /// 扫描耗时（毫秒）
    scan_time_ms: u64,
    /// 扫描结果项
    items: Vec<super::ScanItem>,
    /// 总大小
    total_size: u64,
    /// 缓存版本号，用于强制旧缓存失效
    version: u32,
}

/// 获取缓存目录路径
///
/// 生产环境：~/.maclean/scan_cache/
/// 测试环境：系统临时目录下的 maclean_test_scan_cache/
fn cache_dir() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join("maclean_test_scan_cache")
    }
    #[cfg(not(test))]
    {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        home.join(".maclean").join("scan_cache")
    }
}

/// 获取指定 Tab 的缓存文件路径
fn cache_file_path(tab_name: &str) -> PathBuf {
    cache_dir().join(format!("{}.json", tab_name))
}

/// 获取当前 UNIX 时间戳（秒）
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 判断是否为真实文件系统路径
///
/// 有些条目是**聚合项**：`path` 只是描述文字（如 "12个 __pycache__ 目录"），
/// 真实路径放在 `batch_paths` 里；Windows 卸载项则用 `uwp:` / `uninstall:`
/// 前缀，APFS 快照项用 `com.apple.TimeMachine.*` 名字。这些都不能 stat。
fn is_fs_path(p: &str) -> bool {
    if cfg!(windows) {
        // C:\... 或 UNC \\server\share
        let b = p.as_bytes();
        (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/'))
            || p.starts_with("\\\\")
    } else {
        p.starts_with('/')
    }
}

/// 单个路径的 TOCTOU 校验：存在且**不是**符号链接
///
/// 用 `symlink_metadata` 而非 `metadata`：前者不跟随链接，能发现
/// "文件已被替换成指向别处的软链" 这种情况。跟随软链删除，
/// 有可能删到 `~/Library/Mail` 这类完全无关的地方。
fn path_is_deletable(p: &str) -> bool {
    match std::fs::symlink_metadata(p) {
        Ok(meta) => !meta.file_type().is_symlink(),
        Err(_) => false,
    }
}

/// 缓存条目重新校验（TOCTOU 防线）
///
/// 缓存里的路径是上次扫描那一刻的快照，写入到加载之间最长 24h。
/// 这期间文件可能已经：
///   - 被用户/别的应用删掉 —— 不重新检查的话仍然显示为"可回收"，
///     删除时静默失败，列表里的可用空间长期虚高；
///   - 被替换成指向别处的符号链接 —— 删除时跟随链接删到无关目录。
fn revalidate_item(mut item: super::ScanItem) -> Option<super::ScanItem> {
    if is_fs_path(&item.path) {
        // 普通条目：path 自身必须还存在且不是软链
        if !path_is_deletable(&item.path) {
            return None;
        }
        // 顺带刷新大小：文件可能在这段时间里被改写过。
        // 目录不能用 metadata().len()（那只是目录项大小，不是递归大小），
        // 重新递归统计会让缓存失去意义，所以目录保留缓存值。
        if let Ok(meta) = std::fs::symlink_metadata(&item.path) {
            if !meta.is_dir() {
                item.size_bytes = meta.len();
            }
        }
    } else if !item.batch_paths.is_empty() {
        // 聚合条目：真实路径在 batch_paths 里，逐个筛掉失效的
        item.batch_paths
            .retain(|bp| !is_fs_path(bp) || path_is_deletable(bp));
        if item.batch_paths.is_empty() {
            return None;
        }
    }
    Some(item)
}

/// 尝试从磁盘加载缓存的扫描结果
///
/// 如果缓存文件存在且未过期（7 天内），返回反序列化的 ScanResult。
/// 否则返回 None。
pub fn load_cache(tab_name: &str) -> Option<ScanResult> {
    let path = cache_file_path(tab_name);

    let content = std::fs::read_to_string(&path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&content).ok()?;

    // 检查 TTL
    let now = now_secs();
    if now > entry.timestamp && now - entry.timestamp > CACHE_TTL_SECS {
        // 缓存已过期
        return None;
    }

    // 检查缓存版本，旧版本缓存强制失效
    if entry.version != CACHE_VERSION {
        return None;
    }

    // 重新校验路径：缓存写入到现在最长 24h，路径可能已失效或被换成软链。
    // 必须在**这里**筛掉，否则用户看到的"可释放空间"是虚高的，
    // 且删除时可能跟随软链删到无关目录。
    let items: Vec<super::ScanItem> = entry
        .items
        .into_iter()
        .filter_map(revalidate_item)
        .collect();

    // 重新校验后大小可能被刷新，必须重算而不是沿用缓存里的 total_size
    let total_size = items.iter().map(|i| i.size_bytes).sum();

    Some(ScanResult {
        items,
        total_size,
        scan_time_ms: entry.scan_time_ms,
    })
}

/// 将扫描结果保存到磁盘缓存
///
/// 即使保存失败也不会影响主流程（静默失败）。
pub fn save_cache(tab_name: &str, result: &ScanResult) {
    let dir = cache_dir();

    // 确保目录存在
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }

    let entry = CacheEntry {
        timestamp: now_secs(),
        scan_time_ms: result.scan_time_ms,
        items: result.items.clone(),
        total_size: result.total_size,
        version: CACHE_VERSION,
    };

    let path = cache_file_path(tab_name);
    if let Ok(json) = serde_json::to_string(&entry) {
        let _ = std::fs::write(&path, json);
    }
}

/// 清除指定 Tab 的缓存
///
/// 用于用户手动刷新或缓存失效场景。
pub fn invalidate_cache(tab_name: &str) {
    let path = cache_file_path(tab_name);
    let _ = std::fs::remove_file(&path);
}

/// 清除所有 Tab 的缓存
pub fn invalidate_all_caches() {
    let dir = cache_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
}

#[cfg(test)]
mod tests {
    use super::super::{Recommend, ScanItem, ScanResult};
    use super::*;

    #[test]
    fn test_cache_save_and_load() {
        let tab_name = "test_tab_save_load";
        // 先清除旧缓存
        invalidate_cache(tab_name);

        // 缓存加载时会重校验路径，所以这里必须用**真实存在**的文件，
        // 不能再用 "/tmp/test_cache" 这种假路径（否则会被判定为已失效而丢弃）。
        let dir = std::env::temp_dir().join("maclean_cache_it");
        let _ = std::fs::create_dir_all(&dir);
        let real = dir.join("real_file.bin");
        std::fs::write(&real, vec![0u8; 1024]).expect("写入临时文件失败");

        let result = ScanResult {
            items: vec![ScanItem {
                path: real.to_string_lossy().to_string(),
                size_bytes: 1024,
                category: "test".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                recommend: Recommend::Safe,
                description: "test item".to_string(),
                batch_paths: Vec::new(),
            }],
            total_size: 1024,
            scan_time_ms: 42,
        };

        // 保存
        save_cache(tab_name, &result);

        // 加载
        let loaded = load_cache(tab_name).expect("缓存应存在");
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].path, real.to_string_lossy());
        assert_eq!(loaded.items[0].size_bytes, 1024);
        assert_eq!(loaded.scan_time_ms, 42);
        assert_eq!(loaded.total_size, 1024);

        let _ = std::fs::remove_file(&real);
        invalidate_cache(tab_name);
    }

    // ---------- P1-13: 缓存加载必须重新校验路径（TOCTOU） ----------

    fn mk_item(path: &str, size: u64, batch: Vec<String>) -> ScanItem {
        ScanItem {
            path: path.to_string(),
            size_bytes: size,
            category: "test".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            recommend: Recommend::Safe,
            description: String::new(),
            batch_paths: batch,
        }
    }

    #[test]
    fn cache_drops_paths_that_vanished_since_scan() {
        // 缓存写入到现在最长 24h。路径若已被删除，仍然显示为"可回收"
        // 会让可释放空间长期虚高，且删除时静默失败。
        let tab = "test_tab_toctou_vanish";
        invalidate_cache(tab);

        let dir = std::env::temp_dir().join("maclean_cache_it");
        let _ = std::fs::create_dir_all(&dir);
        let alive = dir.join("alive.bin");
        let dead = dir.join("dead.bin");
        std::fs::write(&alive, vec![0u8; 100]).unwrap();
        std::fs::write(&dead, vec![0u8; 200]).unwrap();

        let result = ScanResult {
            items: vec![
                mk_item(&alive.to_string_lossy(), 100, Vec::new()),
                mk_item(&dead.to_string_lossy(), 200, Vec::new()),
            ],
            total_size: 300,
            scan_time_ms: 1,
        };
        save_cache(tab, &result);

        // 缓存保存之后，其中一项被删除
        std::fs::remove_file(&dead).unwrap();

        let loaded = load_cache(tab).expect("未过期缓存应存在");
        assert_eq!(loaded.items.len(), 1, "已消失的路径必须从缓存结果里剔除");
        assert_eq!(loaded.items[0].path, alive.to_string_lossy());
        assert_eq!(
            loaded.total_size, 100,
            "total_size 必须按剩余项重算，不能沿用缓存里的 300"
        );

        let _ = std::fs::remove_file(&alive);
        invalidate_cache(tab);
    }

    #[test]
    fn cache_drops_paths_replaced_by_symlink() {
        // 路径若被替换成指向别处的符号链接，删除时会跟随链接删到无关目录。
        let tab = "test_tab_toctou_symlink";
        invalidate_cache(tab);

        let dir = std::env::temp_dir().join("maclean_cache_it");
        let _ = std::fs::create_dir_all(&dir);
        let victim = dir.join("victim_target.bin");
        std::fs::write(&victim, vec![0u8; 500]).unwrap();

        let link = dir.join("now_a_symlink");
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&victim, &link).unwrap();

        // 链接是**存在**的 —— 用 metadata（跟随）会误判为正常文件
        assert!(std::fs::metadata(&link).is_ok());

        let result = ScanResult {
            items: vec![mk_item(&link.to_string_lossy(), 500, Vec::new())],
            total_size: 500,
            scan_time_ms: 1,
        };
        save_cache(tab, &result);

        let loaded = load_cache(tab).expect("未过期缓存应存在");
        assert!(
            loaded.items.is_empty(),
            "被替换成符号链接的路径必须剔除，否则会删到链接目标"
        );

        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&victim);
        invalidate_cache(tab);
    }

    #[test]
    fn cache_filters_invalid_batch_paths_on_aggregate_items() {
        // 聚合项（__pycache__ / .DS_Store）的 path 是描述文字，
        // 真实路径在 batch_paths 里 —— 那里也必须逐个筛。
        let tab = "test_tab_toctou_batch";
        invalidate_cache(tab);

        let dir = std::env::temp_dir().join("maclean_cache_it");
        let _ = std::fs::create_dir_all(&dir);
        let real = dir.join("aggregate_member.bin");
        std::fs::write(&real, vec![0u8; 10]).unwrap();
        let gone = dir.join("aggregate_gone.bin");

        let result = ScanResult {
            items: vec![mk_item(
                "3个 __pycache__ 目录",
                999,
                vec![
                    "descriptor-not-a-path".to_string(),
                    real.to_string_lossy().to_string(),
                    gone.to_string_lossy().to_string(),
                ],
            )],
            total_size: 999,
            scan_time_ms: 1,
        };
        save_cache(tab, &result);

        let loaded = load_cache(tab).expect("未过期缓存应存在");
        assert_eq!(loaded.items.len(), 1, "至少还有一个有效成员就不应整条丢弃");
        assert_eq!(
            loaded.items[0].batch_paths,
            vec![
                "descriptor-not-a-path".to_string(),
                real.to_string_lossy().to_string()
            ],
            "失效成员必须剔除，伪路径成员应保留"
        );

        let _ = std::fs::remove_file(&real);
        invalidate_cache(tab);
    }

    #[test]
    fn cache_drops_aggregate_item_when_all_members_gone() {
        let tab = "test_tab_toctou_batch_empty";
        invalidate_cache(tab);

        let dir = std::env::temp_dir().join("maclean_cache_it");
        let _ = std::fs::create_dir_all(&dir);
        let gone = dir.join("gone_aggregate_member.bin");

        let result = ScanResult {
            items: vec![mk_item(
                "2个 .DS_Store",
                777,
                vec![gone.to_string_lossy().to_string()],
            )],
            total_size: 777,
            scan_time_ms: 1,
        };
        save_cache(tab, &result);

        let loaded = load_cache(tab).expect("未过期缓存应存在");
        assert!(
            loaded.items.is_empty(),
            "成员全部失效的聚合项不应再显示（删了必然 0 成功）"
        );

        invalidate_cache(tab);
    }

    #[test]
    fn cache_refreshes_stale_file_size() {
        // 文件大小在这段时间里可能变了，缓存里的旧值会误导用户。
        let tab = "test_tab_toctou_size";
        invalidate_cache(tab);

        let dir = std::env::temp_dir().join("maclean_cache_it");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("grown.bin");
        std::fs::write(&f, vec![0u8; 100]).unwrap();

        let result = ScanResult {
            items: vec![mk_item(&f.to_string_lossy(), 100, Vec::new())],
            total_size: 100,
            scan_time_ms: 1,
        };
        save_cache(tab, &result);

        std::fs::write(&f, vec![0u8; 4096]).unwrap();

        let loaded = load_cache(tab).expect("未过期缓存应存在");
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].size_bytes, 4096, "文件大小应被刷新");
        assert_eq!(loaded.total_size, 4096, "total_size 应随之重算");

        let _ = std::fs::remove_file(&f);
        invalidate_cache(tab);
    }

    #[test]
    fn test_cache_miss_returns_none() {
        let tab_name = "test_tab_nonexistent";
        invalidate_cache(tab_name);
        assert!(load_cache(tab_name).is_none());
    }

    #[test]
    fn test_cache_invalidate() {
        let tab_name = "test_tab_invalidate";
        let result = ScanResult {
            items: Vec::new(),
            total_size: 0,
            scan_time_ms: 0,
        };
        save_cache(tab_name, &result);
        assert!(load_cache(tab_name).is_some());
        invalidate_cache(tab_name);
        assert!(load_cache(tab_name).is_none());
    }

    #[test]
    fn test_cache_version_invalidates_old_cache() {
        let tab_name = "test_tab_version";
        invalidate_cache(tab_name);

        // 手动写入一个旧版本缓存（version 为 1）
        let path = cache_file_path(tab_name);
        let old_entry = CacheEntry {
            timestamp: now_secs(),
            scan_time_ms: 0,
            items: Vec::new(),
            total_size: 0,
            version: 1,
        };
        let json = serde_json::to_string(&old_entry).expect("序列化应成功");
        let _ = std::fs::create_dir_all(cache_dir());
        let _ = std::fs::write(&path, json);

        // 当前版本号为 CACHE_VERSION，旧缓存应被判定为失效
        assert!(load_cache(tab_name).is_none());

        // 清理
        invalidate_cache(tab_name);
    }
}
