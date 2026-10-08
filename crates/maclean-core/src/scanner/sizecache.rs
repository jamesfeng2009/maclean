//! 只读「大目录」占用分析的**持久化增量体积缓存**（P2，重扫提速）。
//！
//! ## 解决什么
//!
//! 磁盘分析会对家目录下几十个大目录（微信/QQ 容器、Docker/OrbStack、ComfyUI/MiniMax
//! 模型、`node_modules` 等）做**准确、无深度截断**的遍历；这些目录又大又深，即便有
//! 批量枚举与自适应并发，首次仍要几十秒到数分钟。但绝大多数情况下，两次扫描之间这些
//! 大目录**几乎不变**。本缓存让「未变化的大目录」重扫时只做一次 `lstat`（微秒级）就
//! 直接返回上次完整统计，变化的目录才重新遍历——「重扫只重算变化子树」。
//!
//! ## 正确性边界（重要）
//!
//! APFS 不提供「子树修改时间」，目录自身 mtime 只在其**直接**成员增删时变化，无法靠
//! 顶层一次 `lstat` 发现深层文件的原地内容改动。因此本缓存采取保守、可解释的策略：
//!
//! - **只服务只读「大目录占用」数字**（[`crate::scanner::dir_size_accurate`] 的调用方
//!   磁盘分析聚合）；**绝不用于任何可删除项的扫描**（缓存/开发缓存/重复/大文件清单都
//!   实时遍历），删除目标永远来自当前文件系统、再过安全闸门。
//! - 只缓存并返回**完整**统计结果（`incomplete==false`）；上次没扫全的目录每次重算，
//!   不会把下限当精确值复用。
//! - 命中键同时校验目录的 `(inode, mtime(sec+nsec), 目录逻辑大小)`：直接成员增删、
//!   改名、替换（常见于模型/容器文件更新）都会改 mtime/大小而失效。
//! - 有 TTL（[`CACHE_TTL_SECS`]）兜底；App 自己删除后会 [`invalidate_all`]（数字立即
//!   不再用旧缓存）；缓存文件损坏 / 版本不符一律忽略并自愈重建。
//!
//! ## 已知保守盲区
//!
//! 深层**已有文件被原地改写但各级父目录 mtime 未变**（APFS 上文件写会更新该文件自身
//! mtime，但不更新祖先目录 mtime）这一种情况，顶层 `(ino,mtime,size)` 可能不变而复用
//! 旧值，直到 TTL 或该路径上层发生增删。鉴于这只影响**只读**占用估计、且磁盘分析的
//! 大文件清单始终实时，此盲区可接受；用户点「重新扫描」在相关目录发生增删后即刷新。
//!
//! 全平台可编译：非 unix 平台 [`dir_size_accurate_cached`] 直接透传，不启用缓存。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 缓存文件格式版本；结构不兼容时 bump，旧文件会被忽略并重建。
const FORMAT_VERSION: u32 = 1;
/// 缓存有效期：7 天。到期后即使顶层签名不变也重新统计一次，兜住任何漏检的深层变更。
const CACHE_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// 磁盘上的缓存文件结构（版本化）。
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct CacheFile {
    version: u32,
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

/// 单个大目录的缓存记录。
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Entry {
    /// 目录 inode（目录被删除重建 / 换成同名其它卷对象时失配）。
    ino: u64,
    /// 目录自身 mtime（直接成员增删会变化）。
    mtime_sec: i64,
    mtime_nsec: i64,
    /// 目录自身逻辑大小（部分文件系统上随直接成员变化）。
    dir_size: u64,
    /// 上次完整统计到的子树物理占用（字节）。
    bytes: u64,
    /// 写入时的 Unix 秒（用于 TTL）。
    at: i64,
}

/// 命中校验所需的目录「当前」元数据快照（由一次 `lstat` 得到）。
#[derive(Clone, Copy)]
struct DirMeta {
    ino: u64,
    mtime_sec: i64,
    mtime_nsec: i64,
    dir_size: u64,
}

impl DirMeta {
    fn signature_eq(&self, e: &Entry) -> bool {
        self.ino == e.ino
            && self.mtime_sec == e.mtime_sec
            && self.mtime_nsec == e.mtime_nsec
            && self.dir_size == e.dir_size
    }
}

/// 可注入缓存文件路径与当前时间的存储（核心逻辑，便于确定性单测）。
struct Store {
    path: PathBuf,
    entries: HashMap<String, Entry>,
    dirty: bool,
}

impl Store {
    /// 读取（损坏 / 版本不符 / 不存在一律得到空表）。
    fn open(path: PathBuf) -> Self {
        let entries = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<CacheFile>(&text) {
                Ok(f) if f.version == FORMAT_VERSION => f.entries,
                Ok(f) => {
                    crate::logger::warn(&format!(
                        "[sizecache] 忽略版本不符的缓存（got {} want {FORMAT_VERSION}）: {}",
                        f.version,
                        path.display()
                    ));
                    HashMap::new()
                }
                Err(err) => {
                    crate::logger::warn(&format!(
                        "[sizecache] 缓存文件损坏，将重建: {} ({err})",
                        path.display()
                    ));
                    HashMap::new()
                }
            },
            Err(_) => HashMap::new(),
        };
        Self {
            path,
            entries,
            dirty: false,
        }
    }

    /// 命中：记录存在、签名一致、未过期。返回缓存的完整占用（incomplete 恒为 false，
    /// 因为不完整结果从不写入）。
    fn get(&self, key: &str, now: &DirMeta, now_secs: i64, ttl_secs: i64) -> Option<u64> {
        let e = self.entries.get(key)?;
        if now.signature_eq(e) && now_secs - e.at <= ttl_secs {
            Some(e.bytes)
        } else {
            None
        }
    }

    /// 写入一条**完整**统计；incomplete 不缓存。
    fn put(&mut self, key: String, meta: DirMeta, bytes: u64, incomplete: bool, now_secs: i64) {
        if incomplete {
            return;
        }
        self.entries.insert(
            key,
            Entry {
                ino: meta.ino,
                mtime_sec: meta.mtime_sec,
                mtime_nsec: meta.mtime_nsec,
                dir_size: meta.dir_size,
                bytes,
                at: now_secs,
            },
        );
        self.dirty = true;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 原子落盘（临时文件 + rename），避免写出半截 JSON。
    fn save(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let payload = CacheFile {
            version: FORMAT_VERSION,
            entries: std::mem::take(&mut self.entries),
        };
        let text = serde_json::to_string_pretty(&payload).map_err(std::io::Error::other)?;
        // save 会消费 entries（take），仅在真正写成功时才算提交；失败需回填。
        let write_result = (|| -> std::io::Result<()> {
            let tmp = self.path.with_extension(format!(
                "json.tmp.{}.{}",
                std::process::id(),
                // 纳秒避免同进程多次 flush 的临时名冲突。
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
            ));
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &self.path)?;
            Ok(())
        })();
        match write_result {
            Ok(()) => {
                // entries 已 take 进 payload 并成功写出：以写出内容重建内存表。
                self.entries = payload.entries;
                self.dirty = false;
                Ok(())
            }
            Err(e) => {
                // 写失败：回填，保留 dirty，下次可重试。
                self.entries = payload.entries;
                Err(e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
//  全局单例（运行时）
// ---------------------------------------------------------------------------

static GLOBAL: OnceLock<Mutex<Store>> = OnceLock::new();

fn global() -> &'static Mutex<Store> {
    GLOBAL.get_or_init(|| {
        let path = cache_file_path();
        Mutex::new(Store::open(path))
    })
}

fn cache_file_path() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".maclean").join("cache").join("dir_sizes.json")
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn meta_of(path: &Path) -> Option<DirMeta> {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::symlink_metadata(path).ok()?;
    if !md.is_dir() {
        return None;
    }
    Some(DirMeta {
        ino: md.ino(),
        mtime_sec: md.mtime(),
        mtime_nsec: md.mtime_nsec(),
        dir_size: md.len(),
    })
}

/// 计算一个大目录的占用，命中未过期缓存则秒回，否则实时准确统计并回填。
///
/// 仅供只读「大目录占用分析」使用；不要用于驱动删除的扫描。
#[cfg(unix)]
pub fn dir_size_accurate_cached(path: &Path) -> (u64, bool) {
    let Some(meta) = meta_of(path) else {
        // 顶层都 lstat 不到（已删除 / 非目录）：交给实时函数给出（通常为 0/incomplete）。
        return crate::scanner::dir_size_accurate(path);
    };
    let key = path.to_string_lossy().to_string();
    let now = now_secs();

    if let Some(bytes) =
        global()
            .lock()
            .expect("sizecache poisoned")
            .get(&key, &meta, now, CACHE_TTL_SECS)
    {
        return (bytes, false);
    }

    let (bytes, incomplete) = crate::scanner::dir_size_accurate(path);
    let mut g = global().lock().expect("sizecache poisoned");
    g.put(key, meta, bytes, incomplete, now);
    (bytes, incomplete)
}

/// 非 unix：缓存不启用，直接透传。
#[cfg(not(unix))]
pub fn dir_size_accurate_cached(path: &Path) -> (u64, bool) {
    crate::scanner::dir_size_accurate(path)
}

/// **只读**缓存：命中未过期条目时返回其字节数；未命中 / 已过期 / 非目录 /
/// 锁异常一律返回 `None`，**绝不**实时遍历统计。
///
/// 用于「删除前只想展示一个不阻塞的估算值」的场景（如一键卸载）：卸载不能
/// 为了算出"将释放多少字节"而先对几十 GB 的关联目录全递归——那会让用户点
/// 卸载后干等几分钟。删除本身不依赖体积，未命中就当作未知（0）处理。
#[cfg(unix)]
pub fn dir_size_peek_cached(path: &Path) -> Option<u64> {
    let meta = meta_of(path)?;
    let key = path.to_string_lossy().to_string();
    global()
        .lock()
        .ok()?
        .get(&key, &meta, now_secs(), CACHE_TTL_SECS)
}

/// 非 unix：无缓存，`None` 表示"未知、不要为此统计"。
#[cfg(not(unix))]
pub fn dir_size_peek_cached(_path: &Path) -> Option<u64> {
    None
}

/// 清空内存缓存并删除缓存文件（例如用户完成删除后，让占用数字立即不依赖旧缓存）。
pub fn invalidate_all() {
    let path = cache_file_path();
    if let Ok(mut g) = global().lock() {
        g.entries.clear();
        g.dirty = false;
    }
    let _ = std::fs::remove_file(&path);
}

/// 把累积的缓存落盘（扫描收尾时调用；无改动则是空操作）。
pub fn flush() {
    if let Ok(mut g) = global().lock() {
        if g.is_dirty() {
            if let Err(e) = g.save() {
                crate::logger::warn(&format!("[sizecache] 落盘失败（不影响本次结果）: {e}"));
            }
        }
    }
}

// =========================================================================
//  测试：直接构造 Store，注入路径与时间，验证命中 / 失效 / TTL / 版本 / 损坏
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "maclean_sizecache_{}_{}_{}.json",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn meta(ino: u64, sec: i64, nsec: i64, size: u64) -> DirMeta {
        DirMeta {
            ino,
            mtime_sec: sec,
            mtime_nsec: nsec,
            dir_size: size,
        }
    }

    #[test]
    fn hit_after_put_with_identical_signature_within_ttl() {
        let p = tmp_path("hit");
        let mut s = Store::open(p.clone());
        let m = meta(100, 1000, 5, 4096);
        s.put("/a".to_string(), m, 123_456, false, 9000);
        s.save().unwrap();

        let s2 = Store::open(p.clone());
        assert_eq!(s2.get("/a", &m, 9000, CACHE_TTL_SECS), Some(123_456));
        // TTL 边界内仍命中。
        assert_eq!(
            s2.get("/a", &m, 9000 + CACHE_TTL_SECS, CACHE_TTL_SECS),
            Some(123_456)
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn miss_when_ttl_expired_or_signature_changes() {
        let p = tmp_path("miss");
        let mut s = Store::open(p.clone());
        let m = meta(100, 1000, 5, 4096);
        s.put("/a".to_string(), m, 123_456, false, 9000);
        s.save().unwrap();

        let s2 = Store::open(p.clone());
        // 过期 1 秒：miss。
        assert_eq!(
            s2.get("/a", &m, 9000 + CACHE_TTL_SECS + 1, CACHE_TTL_SECS),
            None
        );
        // inode / mtime / 目录大小任一变化：miss。
        assert_eq!(
            s2.get("/a", &meta(101, 1000, 5, 4096), 9000, CACHE_TTL_SECS),
            None
        );
        assert_eq!(
            s2.get("/a", &meta(100, 1001, 5, 4096), 9000, CACHE_TTL_SECS),
            None
        );
        assert_eq!(
            s2.get("/a", &meta(100, 1000, 5, 8192), 9000, CACHE_TTL_SECS),
            None
        );
        // 未知 key：miss。
        assert_eq!(s2.get("/b", &m, 9000, CACHE_TTL_SECS), None);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn incomplete_result_is_not_cached() {
        let p = tmp_path("incomplete");
        let mut s = Store::open(p.clone());
        let m = meta(1, 1, 0, 0);
        s.put("/a".to_string(), m, 999, true, 100);
        assert!(!s.is_dirty());
        s.save().unwrap();
        let s2 = Store::open(p.clone());
        assert_eq!(s2.get("/a", &m, 100, CACHE_TTL_SECS), None);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_or_wrong_version_file_is_ignored() {
        let p = tmp_path("corrupt");
        std::fs::write(&p, b"{ this is not json").unwrap();
        let s = Store::open(p.clone());
        assert!(s.entries.is_empty());

        // 版本不符：忽略。
        std::fs::write(
            &p,
            r#"{"version":999,"entries":{"/a":{"ino":1,"mtime_sec":1,"mtime_nsec":0,"dir_size":0,"bytes":7,"at":1}}}"#,
        )
        .unwrap();
        let s = Store::open(p.clone());
        assert!(s.entries.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn round_trip_persists_entries() {
        let p = tmp_path("roundtrip");
        let mut s = Store::open(p.clone());
        s.put("/x".to_string(), meta(7, 50, 9, 1024), 4096, false, 1);
        s.put("/y".to_string(), meta(8, 51, 9, 2048), 8192, false, 1);
        s.save().unwrap();

        let s2 = Store::open(p.clone());
        assert_eq!(
            s2.get("/x", &meta(7, 50, 9, 1024), 1, CACHE_TTL_SECS),
            Some(4096)
        );
        assert_eq!(
            s2.get("/y", &meta(8, 51, 9, 2048), 1, CACHE_TTL_SECS),
            Some(8192)
        );
        let _ = std::fs::remove_file(&p);
    }
}
