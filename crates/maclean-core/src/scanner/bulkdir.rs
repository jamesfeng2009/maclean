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

// =========================================================================
//  C-05：硬链接 / APFS 克隆去重（同一 (dev,ino) 只计一次物理块）
//
//  Time Machine、pnpm store、模型库里大量文件是硬链接，逐文件无条件累加
//  `st_blocks×512` 会把同一物理块重复计 N 次。这里在一次目录枚举内维护
//  `(dev,ino)` 集合，只计首次出现的 inode。
//  注：跨目录出现的硬链接需在 scanner/mod.rs 的遍历层用全局集合去重 ——
//  该文件不在本轮修改边界内，本模块负责单目录枚举内的去重。
// =========================================================================

#[cfg(unix)]
#[derive(Default)]
struct InodeDedup {
    seen: std::collections::HashSet<(u64, u64)>,
}

#[cfg(unix)]
impl InodeDedup {
    /// 该 inode 是否首次出现（首次才应计入体积）。
    fn first(&mut self, meta: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;
        self.seen.insert((meta.dev(), meta.ino()))
    }
}

/// 目录枚举失败的归类。
///
/// 用来把「每个目录一条 WARN」压成「每次统计一行汇总」（2026-11-12 实测：
/// 一次全盘扫描 75 条，一天 1601 条，占主日志 32%，而其中 80% 是永远读不到的
/// TCC 目录 —— 逐条记录只会淹没真正的失败）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnumFailure {
    /// 权限不足（EPERM / EACCES）：TCC 保护的 App 缓存、其他用户的目录。
    /// 这类目录**每轮扫描都会失败**，属于预期，不该刷 WARN。
    Denied,
    /// 枚举期间路径消失（ENOENT / ENOTDIR）：并发删除或遍历期竞态。
    Vanished,
    /// 其他（EINTR / EIO / 数据异常等）：真正值得看一眼的那一类。
    Other,
}

/// 把 `io::Error` 归到三类之一。纯函数，便于单测钉住分类口径。
pub(crate) fn classify_enum_failure(err: &std::io::Error) -> EnumFailure {
    match err.kind() {
        std::io::ErrorKind::PermissionDenied => EnumFailure::Denied,
        std::io::ErrorKind::NotFound => EnumFailure::Vanished,
        // ENOTDIR 在稳定版 Rust 里没有对应 ErrorKind，但它在本场景同样意味着
        // 「路径声称是目录、实际不是」，按消失处理。
        #[cfg(unix)]
        _ if err.raw_os_error() == Some(libc::ENOTDIR) => EnumFailure::Vanished,
        _ => EnumFailure::Other,
    }
}

/// 一次扫描内累积的枚举失败计数。
///
/// 为什么是进程级全局而不是 `WalkTotals` 里的字段：`dir_size` 在清理链路里是
/// **每个候选目录调用一次**，计数挂在单次 walk 上，一次全盘扫描会输出 80+ 行
/// 「1 个目录未能枚举」—— 比原来的逐条 WARN 还多。改为全局累积，由扫描收尾
/// （[`crate::scanner::flush_enum_failures`]）统一输出一行，才是真的降噪。
///
/// 抽成独立结构是为了能单测：全局静态在并行测试里被多个用例同时读写，
/// 断言具体数字必然不稳。
#[derive(Default)]
pub(crate) struct FailureTally {
    denied: u64,
    vanished: u64,
    other: u64,
    /// 枚举超时（IO 卡死 / 忙）而跳过子树的目录数。与上面三类不同：
    /// 这是可以恢复的，下次扫描会无条件重试。
    timeouts: u64,
    /// 批量枚举失败、退回标准枚举的次数（性能信号，不影响正确性）。
    bulk_fallback: u64,
    /// 第一个不可枚举的路径，仅用于定位。
    sample: Option<String>,
}

impl FailureTally {
    fn record(&mut self, dir: &Path, kind: EnumFailure) {
        match kind {
            EnumFailure::Denied => self.denied += 1,
            EnumFailure::Vanished => self.vanished += 1,
            EnumFailure::Other => self.other += 1,
        }
        if self.sample.is_none() {
            self.sample = Some(dir.display().to_string());
        }
    }

    fn record_timeout(&mut self, dir: &Path) {
        self.timeouts += 1;
        if self.sample.is_none() {
            self.sample = Some(dir.display().to_string());
        }
    }

    /// 只有 macOS 有批量枚举接口，回退计数也只在那里产生；
    /// 测试（unix）直接调它验证汇总拼装，因此两者都要能编译到。
    #[cfg(any(all(test, unix), target_os = "macos"))]
    fn record_fallback(&mut self) {
        self.bulk_fallback += 1;
    }

    /// 取走并清零；没有失败也没有超时时返回 `None`。
    ///
    /// 只拼有内容的段落：「0 个已消失」这类零值不出现，日志行才好读。
    fn take(&mut self) -> Option<String> {
        let failed = self.denied + self.vanished + self.other;
        if failed == 0 && self.timeouts == 0 {
            self.bulk_fallback = 0;
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if failed > 0 {
            parts.push(format!(
                "{failed} 个目录未能枚举（权限 {} / 已消失 {} / 其他 {}）",
                self.denied, self.vanished, self.other
            ));
        }
        if self.timeouts > 0 {
            parts.push(format!(
                "{} 个目录枚举超时（IO 忙/卡死，下次扫描重试）",
                self.timeouts
            ));
        }
        let mut msg = parts.join("，");
        msg.push_str("，这部分体积未计入");
        if self.bulk_fallback > 0 {
            msg.push_str(&format!("；批量枚举回退 {} 次", self.bulk_fallback));
        }
        if let Some(sample) = self.sample.as_deref() {
            msg.push_str(&format!("；示例: {sample}"));
        }
        *self = FailureTally::default();
        Some(msg)
    }
}

static ENUM_FAILURES: std::sync::Mutex<FailureTally> = std::sync::Mutex::new(FailureTally {
    denied: 0,
    vanished: 0,
    other: 0,
    timeouts: 0,
    bulk_fallback: 0,
    sample: None,
});

/// 目录枚举失败时的记账（不写日志）。
pub(crate) fn record_enum_failure(dir: &Path, kind: EnumFailure) {
    if let Ok(mut acc) = ENUM_FAILURES.lock() {
        acc.record(dir, kind);
    }
}

/// 目录枚举超时（跳过其子树）时的记账。
pub(crate) fn record_enum_timeout(dir: &Path) {
    if let Ok(mut acc) = ENUM_FAILURES.lock() {
        acc.record_timeout(dir);
    }
}

/// 批量枚举回退计数（性能信号）。仅 macOS 有批量接口，故只在 macOS 编译。
#[cfg(target_os = "macos")]
pub(crate) fn record_bulk_fallback() {
    if let Ok(mut acc) = ENUM_FAILURES.lock() {
        acc.record_fallback();
    }
}

/// 取出并清零本轮计数，返回一行可直接写日志的汇总（无失败时返回 `None`）。
pub(crate) fn take_enum_failures() -> Option<String> {
    ENUM_FAILURES.lock().ok().and_then(|mut acc| acc.take())
}

/// 枚举一个目录。返回「打不开」级别的 IO 错误；单条目异常被忽略。
///
/// 批量枚举失败会退回标准枚举（结果一致，只慢一点），失败与回退都只记账、
/// 不写日志 —— 由扫描收尾统一汇总，避免一次全盘扫描刷 80 条 WARN。
pub(crate) fn list_dir(dir: &Path) -> std::io::Result<DirListing> {
    #[cfg(target_os = "macos")]
    {
        match list_dir_bulk(dir) {
            Ok(listing) => Ok(listing),
            Err(_) => {
                record_bulk_fallback();
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
    // C-05：单目录枚举内按 (dev,ino) 去重，硬链接只计一次物理块。
    #[cfg(unix)]
    let mut dedup = InodeDedup::default();
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
                    // C-05：同一 inode 重复出现（硬链接）时跳过，避免双计。
                    if !dedup.first(&meta) {
                        continue;
                    }
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
        // C-05：与 std 回退同口径，单目录枚举内按 (dev,ino) 去重。
        #[cfg(unix)]
        let mut dedup = super::InodeDedup::default();
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
                                // C-05：同一 inode 的硬链接只计一次。
                                if dedup.first(&meta) {
                                    use std::os::unix::fs::MetadataExt;
                                    out.files_bytes =
                                        out.files_bytes.saturating_add(meta.blocks() * 512);
                                }
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
//
//  `unix` 限定：用例用 `MetadataExt::blocks()` 与 `std::os::unix::fs::symlink`
//  做参照实现，这些 API 在 Windows 上不存在。Windows 侧走的是 std 回退路径
//  （`list_dir` 的非 macOS 实现），没有 st_blocks 语义可对照 —— 强行给个
//  跨平台版本只会测到标准库，测不到被测逻辑（这是本项目明确禁止的写法）。
//  2026-10-27 修复 windows-ci 的 `cargo check --all-targets` 红灯。
// =========================================================================

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    /// H-01：独立参照实现 —— 自己 read_dir 并按 (dev,ino) 去重累加物理块，
    /// **不调用** `list_dir` / `list_dir_std` / `list_dir_bulk` 任何被测逻辑。
    /// 旧参照与被测实现同样「逐文件累加 blocks 不去重」，硬链接双计两边
    /// 一起错（假绿）。生产丢掉去重时这里仍只计一次 → 差值让测试变红。
    fn reference(dir: &Path) -> (u64, usize) {
        let mut bytes = 0u64;
        let mut dirs = 0usize;
        let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
        for e in fs::read_dir(dir).unwrap().flatten() {
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                dirs += 1;
            } else if ft.is_file() {
                let md = e.metadata().unwrap();
                if seen.insert((md.dev(), md.ino())) {
                    bytes += md.blocks() * 512;
                }
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
        let tmp = std::env::temp_dir().join(format!("maclean_bulk_empty_{}", std::process::id()));
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
    fn bulk_unreadable_dir_returns_err_not_zero_byte_complete_listing() {
        // C-08：「打不开」级别的 IO 错误必须由 list_dir 以 Err 返回，供
        // visit_dir 分类计数后汇总上报；绝不能退化成「0 字节且完整」的 Ok 结果。
        // （2026-11-12：visit_dir 不再 `unwrap_or_default()` 静默吞掉，改为按
        // errno 分类累加，walk_tree 收尾输出一行汇总。）
        let tmp = std::env::temp_dir().join(format!("maclean_bulk_noacc_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let orig = fs::metadata(&tmp).unwrap().permissions().mode();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o000)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            match list_dir(&tmp) {
                Err(_) => {} // C-08：正确，错误可透传
                Ok(listing) => panic!(
                    "不可读目录不得返回 Ok(0 字节且完整): {} bytes, {} subdirs",
                    listing.files_bytes,
                    listing.subdirs.len()
                ),
            }
        }
        fs::set_permissions(&tmp, fs::Permissions::from_mode(orig)).ok();
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn failure_tally_accumulates_by_bucket_then_drains() {
        // 纯结构测试：不碰全局静态，避免与并行用例抢计数。
        let mut t = FailureTally::default();
        assert!(t.take().is_none(), "没有失败时不得输出汇总行");

        t.record(Path::new("/a/one"), EnumFailure::Denied);
        t.record(Path::new("/a/two"), EnumFailure::Denied);
        t.record(Path::new("/a/three"), EnumFailure::Denied);
        t.record(Path::new("/b/gone"), EnumFailure::Vanished);
        t.record(Path::new("/c/weird"), EnumFailure::Other);
        t.record_timeout(Path::new("/d/slow"));
        t.record_timeout(Path::new("/e/slow"));
        t.record_fallback();
        t.record_fallback();

        let line = t.take().expect("有失败时必须输出一行汇总");
        assert!(
            line.starts_with("5 个目录未能枚举（权限 3 / 已消失 1 / 其他 1）"),
            "实际: {line}"
        );
        assert!(line.contains("2 个目录枚举超时"), "超时计数错: {line}");
        assert!(line.contains("这部分体积未计入"), "应说明体积未计: {line}");
        assert!(line.contains("批量枚举回退 2 次"), "回退计数错: {line}");
        // 示例取第一条失败路径，用于定位
        assert!(line.contains("/a/one"), "示例路径应为首个失败: {line}");

        // 取走即清零：下一轮扫描不能带着上一轮的数字
        assert!(t.take().is_none(), "汇总后必须清零");
    }

    #[test]
    fn failure_tally_speaks_up_when_only_timeouts_happened() {
        // 只有超时（没有 EPERM 之类）时也必须输出一行，否则「子树被跳过」
        // 这件事就彻底不可见了。
        let mut t = FailureTally::default();
        t.record_timeout(Path::new("/a/slow"));
        let line = t.take().expect("只有超时也要汇总");
        assert!(line.starts_with("1 个目录枚举超时"), "实际: {line}");
        assert!(!line.contains("未能枚举"), "不应虚构权限类失败: {line}");
    }

    #[test]
    fn classify_maps_errno_to_three_buckets() {
        use std::io::Error;
        // TCC 保护的 App 缓存走这一类：每轮都失败，属预期，只计数不刷日志
        assert_eq!(
            classify_enum_failure(&Error::from_raw_os_error(libc::EACCES)),
            EnumFailure::Denied
        );
        assert_eq!(
            classify_enum_failure(&Error::from_raw_os_error(libc::EPERM)),
            EnumFailure::Denied,
            "EPERM 也是权限不足（TCC 保护的目录报的就是它），不能归到 Other"
        );
        // 遍历期被并发删除：正常竞态
        assert_eq!(
            classify_enum_failure(&Error::from_raw_os_error(libc::ENOENT)),
            EnumFailure::Vanished
        );
        assert_eq!(
            classify_enum_failure(&Error::from_raw_os_error(libc::ENOTDIR)),
            EnumFailure::Vanished
        );
        // 内核信号中断 / 数据异常：才是值得人看一眼的那一类
        assert_eq!(
            classify_enum_failure(&Error::from_raw_os_error(libc::EINTR)),
            EnumFailure::Other
        );
        assert_eq!(
            classify_enum_failure(&Error::new(
                std::io::ErrorKind::InvalidData,
                "getattrlistbulk 数据异常"
            )),
            EnumFailure::Other
        );
    }

    #[test]
    fn hardlinks_are_counted_once_by_inode() {
        // C-05 / H-01：同一 (dev,ino) 的两个硬链接只计一次物理块。
        // reference() 是独立参照（自己去重、不调被测逻辑）；生产若丢掉
        // 去重，files_bytes 会是参照的 2 倍 → 断言变红（反向验证）。
        let tmp =
            std::env::temp_dir().join(format!("maclean_bulk_hardlink_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let a = tmp.join("a.bin");
        let b = tmp.join("b.bin");
        fs::write(&a, vec![3u8; 4096]).unwrap();
        fs::hard_link(&a, &b).unwrap(); // b 是 a 的硬链接，同 (dev,ino)

        let one = fs::metadata(&a).unwrap().blocks() * 512;
        let got = list_dir(&tmp).expect("list_dir ok");
        let (ref_bytes, _) = reference(&tmp);
        assert_eq!(got.files_bytes, one, "硬链接只能计一次物理块");
        assert_eq!(ref_bytes, one, "独立参照也应只计一次");
        assert_eq!(got.files_bytes, ref_bytes, "生产与独立参照必须一致");
        fs::remove_dir_all(&tmp).ok();
    }
}
