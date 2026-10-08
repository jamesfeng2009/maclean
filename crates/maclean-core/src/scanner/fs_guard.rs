//! 文件系统遍历护栏。
//!
//! 目标：在「下钻进入某个目录」**之前**就识别并跳过会让 `readdir`/`stat`
//! 长期阻塞在内核、或属于越权访问的路径 —— 而不是等它卡死、再靠超时硬等。
//!
//! 两类目标：
//!
//! 1. **网络 / FUSE / 虚拟化共享文件系统**（NFS、SMB、WebDAV、SSHFS、
//!    macFUSE、virtiofs、vmhgfs、vboxsf …）。它们的挂载点是真实目录而不是
//!    符号链接，所以 `WalkDir::follow_links(false)` 根本挡不住；一旦走进，
//!    枚举和 `stat` 全部走网络，对端休眠 / 掉线时调用会在内核里无限期挂起。
//!    典型：OrbStack 以 NFS 把 `~/OrbStack` 挂进主目录。
//!
//! 2. **macOS TCC 沙盒容器**（`~/Library/Containers/<app>`、
//!    `~/Library/Group Containers/...`）。未获授权访问其它 App 的容器时，
//!    `readdir` 会被 TCC 拖到逐项超时；这类数据本就应在对应 App 内清理，
//!    maclean 默认不深入。
//!
//! 远程挂载点通过**一次性读取内核挂载表**（`getmntinfo(MNT_NOWAIT)`）得到，
//! 纯本地查询、不会触碰网络对端；之后只做路径前缀（组件级）匹配，从而做到
//! 「快跳、不硬等」。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 视为「网络 / FUSE / 虚拟化共享」的文件系统类型名（macOS `f_fstypename`）。
///
/// 本地外置盘（apfs / hfs / exfat / fat32 等）不在此列，仍正常扫描；
/// 通过 macFUSE 挂载的 NTFS/网盘（osxfuse / macfuse）会落入 FUSE 分支被跳过。
const REMOTE_OR_FUSE_TYPES: &[&str] = &[
    // 网络文件系统
    "nfs",
    "smbfs",
    "cifs",
    "webdav",
    "afpfs",
    "ftp",
    "ftpfs",
    "sshfs",
    "ceph",
    "cephfs",
    "9p",
    "9pfs",
    "virtiofs",
    "acfs",
    // FUSE / 用户态文件系统（网盘、容器挂载、NTFS-3G 等都可能无限期阻塞）
    "osxfuse",
    "macfuse",
    "fuse",
    "fusefs",
    "fuse_tx",
    "gvisor-fuse",
    // 虚拟机共享文件夹（宿主机<->客机桥接，行为等同网络盘）
    "vmhgfs",
    "vboxsf",
    "prl_fs",
    "prl_fsf",
    "drvfs",
];

/// 挂载表缓存有效期：短 TTL，既能避免每个目录都查一次 syscall，又能在
/// 扫描中途新插入 U 盘 / 网络卷后较快感知。
const MOUNT_CACHE_TTL: Duration = Duration::from_secs(10);

static MOUNT_CACHE: OnceLock<Mutex<(Instant, Vec<PathBuf>)>> = OnceLock::new();

#[cfg(target_os = "macos")]
mod imp {
    use super::REMOTE_OR_FUSE_TYPES;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    /// 把 `statfs` 里的 `[c_char; N]` C 字符串字段转成小写 `String`。
    fn c_char_array_to_string(buf: &[i8]) -> String {
        let bytes: Vec<u8> = buf
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).to_ascii_lowercase()
    }

    fn c_char_array_to_osstring(buf: &[i8]) -> OsString {
        let bytes: Vec<u8> = buf
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        OsString::from_vec(bytes)
    }

    /// 读取内核挂载表，返回所有「网络 / FUSE / 虚拟化共享」挂载点。
    ///
    /// 用 `MNT_NOWAIT(=2)`：立即返回当前已知挂载信息，不等待对在途 IO 的
    /// 文件系统做完整统计（`MNT_WAIT` 在 NFS 掉线时自身就可能阻塞）。
    pub(super) fn remote_mount_points() -> Vec<PathBuf> {
        let mut mntbuf: *mut libc::statfs = std::ptr::null_mut();
        // MNT_WAIT=1, MNT_NOWAIT=2（见 macOS <sys/mount.h>）。
        let count = unsafe { libc::getmntinfo(&mut mntbuf, 2) };
        if count <= 0 || mntbuf.is_null() {
            return Vec::new();
        }
        let entries = unsafe { std::slice::from_raw_parts(mntbuf, count as usize) };
        entries
            .iter()
            .filter_map(|s| {
                let fstype = c_char_array_to_string(&s.f_fstypename);
                if REMOTE_OR_FUSE_TYPES.contains(&fstype.as_str()) {
                    Some(PathBuf::from(c_char_array_to_osstring(&s.f_mntonname)))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::PathBuf;

    /// 非 macOS 平台暂不内置挂载表识别：本地外置盘照常扫描，极端慢路径由
    /// 有界 IO 池 + 扫描器级硬超时兜底，不会永久挂起。
    pub(super) fn remote_mount_points() -> Vec<PathBuf> {
        Vec::new()
    }
}

/// 取（带 TTL 缓存的）远程/FUSE 挂载点列表。
fn remote_mount_points_cached() -> Vec<PathBuf> {
    let slot =
        MOUNT_CACHE.get_or_init(|| Mutex::new((Instant::now() - MOUNT_CACHE_TTL, Vec::new())));
    let mut guard = match slot.lock() {
        Ok(g) => g,
        // 中毒只说明上次持锁线程 panic，直接重建即可。
        Err(p) => p.into_inner(),
    };
    let (ref mut at, ref mut mounts) = *guard;
    if at.elapsed() >= MOUNT_CACHE_TTL {
        *mounts = imp::remote_mount_points();
        *at = Instant::now();
    }
    mounts.clone()
}

/// 组件级前缀匹配：`path` 是否等于或位于某个挂载点之内。
///
/// 用 [`Path::starts_with`]（按 path component 比较），因此 `/Orb` 不会
/// 误匹配 `/OrbStack`，也无需自己拼尾部斜杠。
fn is_within_any(path: &Path, mounts: &[PathBuf]) -> bool {
    mounts
        .iter()
        .any(|m| path == m.as_path() || path.starts_with(m))
}

/// `path` 是否位于网络 / FUSE / 虚拟化共享挂载点之内（含挂载点本身）。
pub fn is_remote_path(path: &Path) -> bool {
    let mounts = remote_mount_points_cached();
    is_within_any(path, &mounts)
}

/// 判断相对主目录的路径是否落在 TCC 沙盒容器整棵子树内（纯路径规则，可跨平台单测）。
///
/// 命中：
/// - `~/Library/Containers/**`（含容器根，未授权访问其它 App 容器会被 TCC 拖死）
/// - `~/Library/Group Containers/**`（含组容器根）
fn is_tcc_container_relative(rel_components: &[String]) -> bool {
    if rel_components.len() < 2 {
        return false;
    }
    rel_components[0] == "Library"
        && (rel_components[1] == "Containers" || rel_components[1] == "Group Containers")
}

/// `path` 是否为 macOS TCC 沙盒容器（或其内部任意路径），不应被 maclean 深入遍历。
pub fn is_tcc_sandbox_container(path: &Path) -> bool {
    let home = super::home_dir();
    let rel = match path.strip_prefix(&home) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let comps: Vec<String> = rel
        .iter()
        .filter_map(|c| c.to_str())
        .map(|s| s.to_string())
        .collect();
    is_tcc_container_relative(&comps)
}

/// 是否应在遍历时**跳过下钻**该目录：远程/FUSE 挂载点，或 TCC 沙盒容器。
///
/// 这是所有递归遍历（dir_size 分层 BFS、各 WalkDir）统一调用的快跳入口。
pub fn should_skip_traversal(path: &Path) -> bool {
    is_remote_path(path) || is_tcc_sandbox_container(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comps(s: &str) -> Vec<String> {
        Path::new(s)
            .iter()
            .filter_map(|c| c.to_str())
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn tcc_container_roots_and_subtrees_are_protected() {
        assert!(is_tcc_container_relative(&comps("Library/Containers")));
        assert!(is_tcc_container_relative(&comps(
            "Library/Containers/com.tencent.xinWeChat"
        )));
        assert!(is_tcc_container_relative(&comps(
            "Library/Containers/com.tencent.xinWeChat/Data/Library/Caches"
        )));
        assert!(is_tcc_container_relative(&comps(
            "Library/Group Containers/59GAB.com.tencent.xinWeChat"
        )));
        assert!(is_tcc_container_relative(&comps(
            "Library/Group Containers/group.com.apple.notes/Library/Caches"
        )));
    }

    #[test]
    fn normal_library_dirs_are_not_tcc_containers() {
        assert!(!is_tcc_container_relative(&comps("Library")));
        assert!(!is_tcc_container_relative(&comps("Library/Caches")));
        assert!(!is_tcc_container_relative(&comps(
            "Library/Application Support/JetBrains"
        )));
        assert!(!is_tcc_container_relative(&comps("Library/Logs/JetBrains")));
    }

    #[test]
    fn mount_prefix_matches_by_component_not_string_prefix() {
        let mounts = vec![PathBuf::from("/Users/u/OrbStack")];
        assert!(is_within_any(Path::new("/Users/u/OrbStack"), &mounts));
        assert!(is_within_any(
            Path::new("/Users/u/OrbStack/data/img.qcow2"),
            &mounts
        ));
        // 字符串前缀相同但不是同一组件，不得误判
        assert!(!is_within_any(Path::new("/Users/u/OrbStackExtra"), &mounts));
        assert!(!is_within_any(Path::new("/Users/u/Projects"), &mounts));
    }

    #[test]
    fn multiple_remote_mounts_are_detected() {
        let mounts = vec![
            PathBuf::from("/Users/u/OrbStack"),
            PathBuf::from("/Volumes/team-smb"),
        ];
        assert!(is_within_any(Path::new("/Volumes/team-smb/a"), &mounts));
        assert!(!is_within_any(Path::new("/Volumes/local-usb"), &mounts));
    }
}
