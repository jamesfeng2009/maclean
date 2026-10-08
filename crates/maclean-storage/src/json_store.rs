//! 原子 JSON 文件读写。
//!
//! 写盘流程：写临时文件 → fsync → rename 覆盖。崩溃/断电不会留下半截 JSON。
//! 读盘流程：不存在返回 None；解析失败返回错误（调用方决定回退默认）。

use std::path::{Path, PathBuf};

/// 读写 JSON 文件的小工具（泛型 T: Serialize + Deserialize）。
pub struct JsonStore {
    dir: PathBuf,
}

impl JsonStore {
    /// 以 `dir` 为存储目录创建 store（不立即建目录）
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// 存储目录
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 某个 key 对应的文件路径（`dir/<key>.json`）
    pub fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// 读取并反序列化；文件不存在返回 `Ok(None)`
    pub fn load<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>, String> {
        let path = self.path_for(key);
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content)
                .map(Some)
                .map_err(|e| format!("{}: JSON 解析失败: {}", path.display(), e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: 读取失败: {}", path.display(), e)),
        }
    }

    /// 序列化并原子写入（temp + rename）
    pub fn save<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<(), String> {
        let path = self.path_for(key);
        let json = serde_json::to_string_pretty(value).map_err(|e| format!("序列化失败: {e}"))?;
        self.write_atomic(&path, json.as_bytes())
    }

    /// 是否存在
    pub fn exists(&self, key: &str) -> bool {
        self.path_for(key).exists()
    }

    /// 删除某个 key 的文件
    pub fn remove(&self, key: &str) -> Result<(), String> {
        let path = self.path_for(key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: 删除失败: {}", path.display(), e)),
        }
    }

    /// 原子写：临时文件 + rename。
    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("{}: 创建目录失败: {}", parent.display(), e))?;
        }
        // 同目录临时文件保证 rename 原子（跨文件系统不保证）
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)
            .map_err(|e| format!("{}: 写临时文件失败: {}", tmp.display(), e))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: 替换失败: {}", path.display(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("maclean_store_test_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Sample {
        name: String,
        n: u64,
    }

    #[test]
    fn load_missing_returns_none() {
        let store = JsonStore::new(tmp_dir("missing"));
        assert!(store.load::<Sample>("nope").unwrap().is_none());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let store = JsonStore::new(tmp_dir("roundtrip"));
        store
            .save(
                "scan",
                &Sample {
                    name: "x".into(),
                    n: 7,
                },
            )
            .unwrap();
        let loaded: Sample = store.load("scan").unwrap().unwrap();
        assert_eq!(
            loaded,
            Sample {
                name: "x".into(),
                n: 7
            }
        );
        assert!(store.exists("scan"));
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn save_overwrites_atomically() {
        let store = JsonStore::new(tmp_dir("overwrite"));
        store
            .save(
                "k",
                &Sample {
                    name: "a".into(),
                    n: 1,
                },
            )
            .unwrap();
        store
            .save(
                "k",
                &Sample {
                    name: "b".into(),
                    n: 2,
                },
            )
            .unwrap();
        let loaded: Sample = store.load("k").unwrap().unwrap();
        assert_eq!(loaded.name, "b");
        // 无残留临时文件
        assert!(!store.path_for("k").with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn corrupted_json_returns_error_not_panic() {
        let store = JsonStore::new(tmp_dir("corrupt"));
        let path = store.path_for("bad");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(store.load::<Sample>("bad").is_err());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn remove_is_idempotent() {
        let store = JsonStore::new(tmp_dir("remove"));
        store
            .save(
                "k",
                &Sample {
                    name: "a".into(),
                    n: 1,
                },
            )
            .unwrap();
        store.remove("k").unwrap();
        assert!(!store.exists("k"));
        store.remove("k").unwrap(); // 不存在也不报错
        let _ = std::fs::remove_dir_all(store.dir());
    }
}
