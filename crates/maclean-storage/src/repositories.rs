//! 仓储层：结构化数据的读写接口。
//!
//! 当前实现基于 [`crate::json_store::JsonStore`]（JSON-first 决策，2026-10-07）。
//! 若未来引入 SQLite，本模块的接口保持不变、内部实现替换即可 —— 调用方
//! 不感知存储介质（strategy §20：核心 schema 公开）。

use crate::json_store::JsonStore;
use maclean_types::domain::ScanResult;
use std::path::PathBuf;

/// 扫描历史仓储：保存/读取扫描结果快照。
///
/// 只存结构化观测（[`ScanItem`]），不存原始文件内容。
pub struct ScanHistoryRepository {
    store: JsonStore,
}

impl ScanHistoryRepository {
    /// 以 `base_dir` 为存储根（内部使用 `scan_history` 子目录）
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            store: JsonStore::new(base_dir.join("scan_history")),
        }
    }

    /// 保存一次扫描结果（key = 快照 id，如时间戳）
    pub fn save_snapshot(&self, id: &str, result: &ScanResult) -> Result<(), String> {
        self.store.save(id, result)
    }

    /// 读取一次扫描结果
    pub fn load_snapshot(&self, id: &str) -> Result<Option<ScanResult>, String> {
        self.store.load(id)
    }

    /// 最近一次快照 id（按文件 mtime 排序；无快照返回 None）
    pub fn latest_snapshot_id(&self) -> Option<String> {
        let dir = self.store.dir();
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .collect();
        entries.sort_by_key(|e| e.metadata().map(|m| m.modified().ok()).ok().flatten());
        entries.last().map(|e| {
            e.path()
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
    }

    /// 全部快照 id（排序不稳定，仅用于列举）
    pub fn list_snapshot_ids(&self) -> Vec<String> {
        let dir = self.store.dir();
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .filter_map(|e| {
                        e.path()
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maclean_types::domain::{Recommend, ScanItem};

    fn tmp_base(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("maclean_repo_test_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn save_and_load_scan_snapshot() {
        let base = tmp_base("scan");
        let repo = ScanHistoryRepository::new(base.clone());
        let result = ScanResult {
            items: vec![ScanItem {
                path: "/tmp/cache".into(),
                size_bytes: 10,
                category: "缓存".into(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                recommend: Recommend::Safe,
                description: String::new(),
                batch_paths: vec![],
                batch_mtimes: vec![],
            }],
            total_size: 10,
            scan_time_ms: 3,
        };
        repo.save_snapshot("2026-10-07T00:00:00Z", &result).unwrap();
        let loaded = repo.load_snapshot("2026-10-07T00:00:00Z").unwrap().unwrap();
        assert_eq!(loaded.total_size, 10);
        assert_eq!(loaded.items[0].path, "/tmp/cache");
        assert_eq!(loaded.items[0].recommend, Recommend::Safe);
        assert!(repo
            .list_snapshot_ids()
            .contains(&"2026-10-07T00:00:00Z".to_string()));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn latest_snapshot_tracks_newest_write() {
        let base = tmp_base("latest");
        let repo = ScanHistoryRepository::new(base.clone());
        repo.save_snapshot(
            "old",
            &ScanResult {
                items: vec![],
                total_size: 1,
                scan_time_ms: 1,
            },
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        repo.save_snapshot(
            "new",
            &ScanResult {
                items: vec![],
                total_size: 2,
                scan_time_ms: 1,
            },
        )
        .unwrap();
        assert_eq!(repo.latest_snapshot_id().as_deref(), Some("new"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_snapshot_returns_none() {
        let base = tmp_base("missing");
        let repo = ScanHistoryRepository::new(base.clone());
        assert!(repo.load_snapshot("nope").unwrap().is_none());
        assert!(repo.latest_snapshot_id().is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
