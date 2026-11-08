//! 原子写（临时文件 + fsync + rename）—— A-12 与 G-03/G-08 的共用工具。
//!
//! `std::fs::write` 会先把目标截断再写：进程崩溃或与读取并发时，目标会停在
//! 半截状态；下一次解析失败再"回退默认"就会把用户数据永久覆盖（A-12 数据
//! 丢失链路）。这里把「写同目录唯一临时文件 → fsync → rename → fsync 父目录」
//! 收敛成唯一实现，配置、备份清单、缓存落盘统一复用。

use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// 把 `content` 原子写入 `path`（A-12：取代各处的 `std::fs::write`）。
///
/// - 临时文件与目标同目录，保证 rename 同卷；名字含 pid + 纳秒 + 序号，
///   `create_new(true)` 独占创建，并发写不会交叉覆盖；
/// - 写完先 `sync_all()` 再 `rename()`，目标要么是旧内容、要么是新内容，
///   绝不会停在半截 JSON；
/// - rename 后 fsync 父目录，把目录项修改也持久化（Windows 打不开目录，忽略）。
pub fn atomic_write(path: &Path, content: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let pid = std::process::id();

    for attempt in 0..64u32 {
        let tmp = parent.join(format!(".{}.tmp.{}.{}.{}", name, pid, nanos, attempt));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        if let Err(e) = file.write_all(content).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        drop(file);
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "原子写：临时文件名冲突次数过多",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_content_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("maclean_atomic_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cfg.json");
        std::fs::write(&p, b"old").unwrap();

        atomic_write(&p, b"{\"ok\":true}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"ok\":true}");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "原子写不应残留临时文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_atomic_writes_never_leave_a_half_written_file() {
        // 反向验证要点：把生产实现改回「先截断再写」，并发下会出现半截 JSON，
        // 这条断言必须变红。
        let dir = std::env::temp_dir().join(format!("maclean_atomic_cc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cfg.json");
        std::thread::scope(|s| {
            for i in 0..8 {
                let p = p.clone();
                s.spawn(move || {
                    let body = format!("{{\"round\":{}}}", i);
                    atomic_write(&p, body.as_bytes()).unwrap();
                });
            }
        });
        let got = std::fs::read_to_string(&p).unwrap();
        assert!(
            got.starts_with("{\"round\":") && got.ends_with('}') && got.len() < 20,
            "出现半截或拼接内容: {got:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
