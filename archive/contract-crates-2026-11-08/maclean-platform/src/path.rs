//! 路径安全纯逻辑：symlink / junction 检测、挂载点判定。
//!
//! 这些判定不依赖平台 API，可跨平台编译与测试；OS 差异只发生在
//! "如何拿到挂载表 / 如何判定 junction" 的采集端（由适配器提供）。

use std::path::Path;

/// 检测路径是否为符号链接（跟随 path 本身，不解析目标）。
pub fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// 判定路径是否位于任一挂载点之下（或等于挂载点）。
///
/// 语义与 maclean-core `is_path_mounted` 一致：
/// - 完全相等（路径就是挂载点）→ true；
/// - 路径位于挂载点之下（挂载点是路径前缀）→ true；
/// - 根挂载点 `"/"` 不参与判定（否则所有路径都算"已挂载"）。
pub fn is_path_mounted(path: &Path, mounts: &[String]) -> bool {
    let p = path.to_string_lossy().replace('\\', "/");
    let p = p.trim_end_matches('/');
    mounts.iter().any(|m| {
        let m = m.replace('\\', "/");
        let m = m.trim_end_matches('/');
        if m.is_empty() || m == "/" {
            return false;
        }
        p == m || p.starts_with(&format!("{m}/"))
    })
}

/// Windows junction / macOS 符号链接的统一判定（别名）。
pub fn is_link_or_junction(path: &Path) -> bool {
    is_symlink(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_detection_exact_and_nested() {
        let mounts = vec!["/Volumes/My Disk".to_string(), "/data".to_string()];
        assert!(is_path_mounted(Path::new("/Volumes/My Disk"), &mounts));
        assert!(is_path_mounted(Path::new("/Volumes/My Disk/sub"), &mounts));
        assert!(is_path_mounted(Path::new("/data/x/y"), &mounts));
        assert!(!is_path_mounted(Path::new("/Volumes/Other"), &mounts));
        assert!(!is_path_mounted(Path::new("/Volumes"), &mounts));
    }

    #[test]
    fn mount_detection_ignores_root() {
        let mounts = vec!["/".to_string()];
        assert!(!is_path_mounted(Path::new("/Users/foo"), &mounts));
        assert!(!is_path_mounted(Path::new("/"), &mounts));
    }

    #[test]
    fn mount_detection_normalizes_separators() {
        let mounts = vec![r"C:\Users".to_string()];
        assert!(is_path_mounted(Path::new(r"C:\Users\me"), &mounts));
        assert!(is_path_mounted(Path::new("C:/Users/me"), &mounts));
    }

    #[test]
    fn symlink_detection_works() {
        let dir = std::env::temp_dir().join(format!("mcl_plat_link_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real");
        std::fs::write(&real, "x").unwrap();
        let link = dir.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(is_symlink(&link));
        assert!(!is_symlink(&real));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
