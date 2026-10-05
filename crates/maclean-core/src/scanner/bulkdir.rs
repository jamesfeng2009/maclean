//! 目录批量枚举：用 `getattrlistbulk(2)` 一次系统调用取回一批条目的名字与类型。
//!
//! ## 背景（P1-e 性能项）
//!
//! 旧实现对每个目录 `read_dir()` 后，还要靠 per-entry 的类型判断 / `metadata()` 取
//! `st_blocks`。磁盘繁忙时这类逐条元数据请求排队在内核，容易把目录拖过看门狗。
//! `getattrlistbulk` 一次调用返回整批条目，是 Finder / `du` 等使用的目录枚举接口。
//!
//! ## 物理占用口径（必须与旧实现逐字节一致）
//!
//! 实测在当前 Darwin 内核（24/25）上，`getattrlistbulk` **不能稳定回带数据叉的
//! allocated size**（`ATTR_FORK_ALLOCSIZE`）：与 `ATTR_CMN_OBJTYPE` 同请求会 ERANGE，
//! 不带 OBJTYPE 又被内核静默丢弃（`returned.forkattr=0`）。因此本模块用 bulk 批量取
//! **名字 + 类型**（批量枚举替代逐条 readdir，且对 d_type 未知的文件系统更稳），再只
//! 对**常规文件**取一次物理占用（`st_blocks×512`，稀疏文件不虚高）。目录自身块不计、
//! 符号链接及其它类型忽略、全程**不跟随符号链接**（`O_NOFOLLOW` + 内核类型）。
//!
//! 现代内核要求 commonattr 必须带 `ATTR_CMN_RETURNED_ATTRS`，否则整次调用 EINVAL；返回
//! 记录按头部的 `attribute_set_t` **动态游标解析**（不硬编码偏移）。
//!
//! 非 macOS（以及 macOS 批量调用失败）走 [`list_dir_std`]，语义与旧的
//! `read_dir + per-file metadata` 完全相同，保证跨平台与兜底正确。

use std::path::{Path, PathBuf};

/// 单个目录的枚举结果：直接常规文件物理占用之和，与其直接子目录。
#[derive(Default)]
pub(crate) struct DirListing {
    /// 直接常规文件数据叉 allocated size 之和（字节，`st_blocks×512`，非逻辑长度）。
    pub files_bytes: u64,
    /// 直接子目录（不含符号链接），供上层下钻。
    pub subdirs: Vec<PathBuf>,
}

/// 枚举一个目录。返回「打不开」级别的 IO 错误；单条目异常被忽略。
pub(crate) fn list_dir(dir: &Path) -> std::io::Result<DirListing> {
    #[cfg(target_os = "macos")]
    {
        match list_dir_bulk(dir) {
            Ok(listing) => Ok(listing),
            Err(e) => {
                crate::logger::warn(&format!(
                    "[bulkdir] getattrlistbulk 失败，退回标准枚举 {}: {e}",
                    dir.display()
                ));
                list_dir_std(dir)
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        list_dir_std(dir)
    }
}

// =========================================================================
//  非 macOS 兜底：与历史实现逐行等价（read_dir + 每文件 metadata）
// =========================================================================

fn list_dir_std(dir: &Path) -> std::io::Result<DirListing> {
    let mut out = DirListing::default();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            out.subdirs.push(entry.path());
        } else if ft.is_file() {
            if let Ok(meta) = entry.metadata() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    out.files_bytes += meta.blocks() * 512;
                }
                #[cfg(not(unix))]
                {
                    out.files_bytes += meta.len();
                }
            }
        }
    }
    Ok(out)
}

// =========================================================================
//  macOS：getattrlistbulk 批量取名字 + 类型
// =========================================================================

#[cfg(target_os = "macos")]
mod imp_bulk {
    use super::{DirListing, Path, PathBuf};
    use std::ffi::CString;
    use std::mem;
    use std::os::unix::ffi::OsStrExt;

    /// vnode 类型（`<sys/vnode.h>` enum vtype，libc 未导出，Abi 稳定）。
    const VREG: i32 = 1; // 常规文件
    const VDIR: i32 = 2; // 目录（VLNK=5，其余值一律忽略）

    /// commonattr 位（部分 libc 未导出，按稳定 ABI 声明）。
    const ATTR_CMN_ERROR_LOCAL: libc::attrgroup_t = 0x2000_0000;
    const ATTR_CMN_RETURNED_ATTRS_LOCAL: libc::attrgroup_t = 0x8000_0000;

    const ATTR_BIT_MAP_COUNT: u16 = 5;
    /// 每次批量调用缓冲区（1 MiB），装不下循环续取。
    const BULK_BUF_SIZE: usize = 1 << 20;
    /// 定长头：length(4) + returned attribute_set_t(20)。
    const MIN_HEADER: usize = 4 + 20;

    struct RawEntry {
        name: Vec<u8>,
        objtype: i32,
    }

    pub(super) fn list_dir_bulk(dir: &Path) -> std::io::Result<DirListing> {
        let common = ATTR_CMN_RETURNED_ATTRS_LOCAL
            | ATTR_CMN_ERROR_LOCAL
            | libc::ATTR_CMN_NAME
            | libc::ATTR_CMN_OBJTYPE;
        let entries = collect(dir, common)?;

        let mut out = DirListing::default();
        for e in entries {
            match e.objtype {
                VDIR => {
                    let child: PathBuf = dir.join(std::ffi::OsStr::from_bytes(&e.name));
                    out.subdirs.push(child);
                }
                VREG => {
                    // bulk 在本内核不回 allocated size：只对常规文件取一次物理占用。
                    let path = dir.join(std::ffi::OsStr::from_bytes(&e.name));
                    if let Ok(meta) = std::fs::symlink_metadata(&path) {
                        if meta.is_file() {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::MetadataExt;
                                out.files_bytes =
                                    out.files_bytes.saturating_add(meta.blocks() * 512);
                            }
                            #[cfg(not(unix))]
                            {
                                out.files_bytes = out.files_bytes.saturating_add(meta.len());
                            }
                        }
                    }
                }
                _ => {
                    // 符号链接（VLNK=5）及其它类型：忽略，不跟随、不计体积。
                }
            }
        }
        Ok(out)
    }

    fn collect(dir: &Path, common: u32) -> std::io::Result<Vec<RawEntry>> {
        let path_c = CString::new(dir.as_os_str().as_encoded_bytes())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "路径含 NUL"))?;
        let fd = unsafe {
            libc::open(
                path_c.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        struct FdGuard(i32);
        impl Drop for FdGuard {
            fn drop(&mut self) {
                unsafe {
                    libc::close(self.0);
                }
            }
        }
        let _fd = FdGuard(fd);

        let mut attrlist: libc::attrlist = unsafe { mem::zeroed() };
        attrlist.bitmapcount = ATTR_BIT_MAP_COUNT;
        attrlist.commonattr = common;

        let mut buf: Vec<u8> = vec![0u8; BULK_BUF_SIZE];
        let mut entries: Vec<RawEntry> = Vec::new();

        loop {
            let count = unsafe {
                libc::getattrlistbulk(
                    fd,
                    &mut attrlist as *mut libc::attrlist as *mut libc::c_void,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                    0,
                )
            };
            if count < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if count == 0 {
                break;
            }

            let mut cursor = 0usize;
            for _ in 0..count {
                if cursor + MIN_HEADER > buf.len() {
                    return Err(malformed("条目头越界"));
                }
                let entry_len = read_u32(&buf, cursor) as usize;
                if entry_len < MIN_HEADER || cursor + entry_len > buf.len() {
                    return Err(malformed("条目长度非法"));
                }
                let end = cursor + entry_len;

                let ret_common = read_u32(&buf, cursor + 4);

                let mut p = cursor + MIN_HEADER;

                let mut entry_error = 0u32;
                if ret_common & ATTR_CMN_ERROR_LOCAL != 0 {
                    if p + 4 > end {
                        return Err(malformed("error 字段越界"));
                    }
                    entry_error = read_u32(&buf, p);
                    p += 4;
                }

                if ret_common & libc::ATTR_CMN_NAME == 0 {
                    cursor = end;
                    continue;
                }
                if p + 8 > end {
                    return Err(malformed("name attrreference 越界"));
                }
                let name_ref_base = p;
                let name_dataoff = read_i32(&buf, p).max(0) as usize;
                let name_len = read_u32(&buf, p + 4) as usize;
                p += 8;
                let name_base = name_ref_base + name_dataoff;
                if name_len == 0 || name_base + name_len > end {
                    return Err(malformed("名字偏移/长度非法"));
                }
                let mut name = buf[name_base..name_base + name_len].to_vec();
                if name.last() == Some(&0) {
                    name.pop();
                }

                let mut objtype = 0i32;
                if ret_common & libc::ATTR_CMN_OBJTYPE != 0 {
                    if p + 4 > end {
                        return Err(malformed("objtype 越界"));
                    }
                    objtype = read_i32(&buf, p);
                }

                if entry_error != 0 {
                    cursor = end;
                    continue;
                }

                entries.push(RawEntry { name, objtype });
                cursor = end;
            }
        }

        Ok(entries)
    }

    fn malformed(msg: &str) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("getattrlistbulk 数据异常: {msg}"),
        )
    }

    #[inline]
    fn read_u32(buf: &[u8], at: usize) -> u32 {
        u32::from_ne_bytes(buf[at..at + 4].try_into().unwrap())
    }
    #[inline]
    fn read_i32(buf: &[u8], at: usize) -> i32 {
        i32::from_ne_bytes(buf[at..at + 4].try_into().unwrap())
    }
}

#[cfg(target_os = "macos")]
use imp_bulk::list_dir_bulk;

// =========================================================================
//  测试：枚举口径必须与标准 read_dir+metadata（st_blocks）逐字节一致
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn reference(dir: &Path) -> (u64, usize) {
        let mut bytes = 0u64;
        let mut dirs = 0usize;
        for e in fs::read_dir(dir).unwrap().flatten() {
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                dirs += 1;
            } else if ft.is_file() {
                bytes += e.metadata().unwrap().blocks() * 512;
            }
        }
        (bytes, dirs)
    }

    #[test]
    fn bulk_matches_std_on_mixed_tree() {
        let tmp = std::env::temp_dir().join(format!("maclean_bulk_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub_a/deep")).unwrap();
        fs::create_dir_all(tmp.join("sub_b")).unwrap();
        fs::write(tmp.join("f1.txt"), b"hello world").unwrap();
        fs::write(tmp.join("sub_a/f2.bin"), vec![7u8; 4096]).unwrap();
        fs::write(tmp.join("sub_a/deep/f3.log"), b"x".repeat(9000)).unwrap();
        std::os::unix::fs::symlink(tmp.join("f1.txt"), tmp.join("link_to_f1")).unwrap();
        fs::write(tmp.join("sub_b/我的 文件.dat"), vec![1u8; 5000]).unwrap();
        fs::write(tmp.join(".hidden"), b"abc").unwrap();
        fs::write(tmp.join("empty"), b"").unwrap();
        for i in 0..2000u32 {
            let _ = fs::write(tmp.join("sub_b").join(format!("many_{i:05}.tmp")), b"q");
        }

        // macOS：批量路径必须真正成功（而非悄悄回退 std 才恰好相等）。
        #[cfg(target_os = "macos")]
        {
            let direct = list_dir_bulk(&tmp).expect("macOS 批量枚举必须成功");
            let (rb, rd) = reference(&tmp);
            assert_eq!(direct.files_bytes, rb);
            assert_eq!(direct.subdirs.len(), rd);
        }

        let got = list_dir(&tmp).expect("list_dir ok");
        let (ref_bytes, ref_dirs) = reference(&tmp);
        assert_eq!(got.files_bytes, ref_bytes);
        assert_eq!(got.subdirs.len(), ref_dirs);
        let mut names: Vec<String> = got
            .subdirs
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["sub_a".to_string(), "sub_b".to_string()]);
        assert!(!got
            .subdirs
            .iter()
            .any(|p| p.to_string_lossy().contains("link_to_f1")));

        let many = list_dir(&tmp.join("sub_b")).expect("list_dir sub_b ok");
        let (m_bytes, m_dirs) = reference(&tmp.join("sub_b"));
        assert_eq!(many.files_bytes, m_bytes, "sub_b（含跨批次）块占用一致");
        assert_eq!(many.subdirs.len(), m_dirs);

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn bulk_empty_and_missing_dir() {
        let tmp =
            std::env::temp_dir().join(format!("maclean_bulk_empty_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let got = list_dir(&tmp).unwrap();
        assert_eq!(got.files_bytes, 0);
        assert!(got.subdirs.is_empty());

        let missing = tmp.join("does_not_exist");
        assert!(list_dir(&missing).is_err());
        fs::remove_dir_all(&tmp).ok();
    }

    #[cfg(unix)]
    #[test]
    fn bulk_unreadable_dir_does_not_panic() {
        let tmp =
            std::env::temp_dir().join(format!("maclean_bulk_noacc_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let orig = fs::metadata(&tmp).unwrap().permissions().mode();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o000)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert!(list_dir(&tmp).is_err());
        }
        fs::set_permissions(&tmp, fs::Permissions::from_mode(orig)).ok();
        fs::remove_dir_all(&tmp).ok();
    }
}
