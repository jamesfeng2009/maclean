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

/// 缓存有效期：7 天（秒）
const CACHE_TTL_SECS: u64 = 7 * 24 * 60 * 60;

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
}

/// 获取缓存目录路径
///
/// ~/.maclean/scan_cache/
fn cache_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".maclean").join("scan_cache")
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

    Some(ScanResult {
        items: entry.items,
        total_size: entry.total_size,
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
    use super::*;
    use super::super::{Recommend, ScanItem, ScanResult};

    #[test]
    fn test_cache_save_and_load() {
        let tab_name = "test_tab_save_load";
        // 先清除旧缓存
        invalidate_cache(tab_name);

        let result = ScanResult {
            items: vec![ScanItem {
                path: "/tmp/test_cache".to_string(),
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
        assert_eq!(loaded.items[0].path, "/tmp/test_cache");
        assert_eq!(loaded.items[0].size_bytes, 1024);
        assert_eq!(loaded.scan_time_ms, 42);
        assert_eq!(loaded.total_size, 1024);

        // 清理
        invalidate_cache(tab_name);
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
}
