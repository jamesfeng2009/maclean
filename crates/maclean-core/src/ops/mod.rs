//! 后台操作层（maclean-core）
//!
//! 删除 / sudo / 系统优化执行与扫描消息契约。这一层不碰 egui / Web，
//! 只跟进程、文件系统和线程打交道，egui 壳与 Tauri 壳共用。
//!
//! 阶段 0（2026-10）：原 src/ops/mod.rs 拆为两半 —— 依赖 App 状态的
//! 扫描编排留在 bin（`src/ops/mod.rs`），数据契约与删除执行全部在本 crate。

use std::sync::mpsc;

// 模块路径本身也要引入：代码里大量写成 `scanner::Foo` / `logger::info(..)`
use crate::scanner::ScanItem;
use crate::{logger, platform, safety, scanner};

mod uninstall_app;

pub use uninstall_app::{is_uninstallable_app_path, uninstall_app, UninstallAppReport};

/// 后台扫描消息
pub enum ScanMessage {
    /// 扫描进度更新
    Progress(f32),
    /// 增量结果（扫描中部分项）— (items, tab_index)
    PartialItems(Vec<ScanItem>, u64),
    /// 当前扫描路径
    CurrentPath(String),
    /// 单个 Tab 扫描完成
    Done(Vec<ScanItem>, u64, u64), // (items, scan_time_ms, tab_index)
    /// 单个 Tab 扫描超时跳过（目录 IO 异常导致 readdir 永久阻塞）— (tab_index, reason)
    Skipped(u64, String),
    /// 全部扫描完成（用于批量扫描）
    AllDone,
}

/// 后台删除消息
pub enum DeleteMessage {
    /// 单项删除结果（日志, 路径, 类别, 是否成功）
    Log(String, String, String, bool),
    /// 单项跳过（日志, 路径, 类别）—— 路径已不存在等，不计入成功/失败统计
    Skip(String, String, String),
    /// 进度信息（不计入成功/失败统计）
    Info(String),
    /// 普通删除完成，部分项需要管理员权限
    NeedPassword(Vec<(String, String)>),
    /// 删除被用户中途取消（P0-3）：已删项照常记录清单，未处理项未动
    Cancelled,
    /// 全部删除完成
    Done,
    /// 本次删除已写入备份清单（M-2）
    ///
    /// 携带清单 id 与"可还原 / 总数"，让 UI 能如实告诉用户有多少项真的
    /// 有后悔药。永久删除的项不在这两个数里撒谎 —— restorable 只统计
    /// 走废纸篓的那部分。
    BackupRecorded {
        id: String,
        restorable: usize,
        total: usize,
    },
    /// Windows: 卸载后检测到残留，弹出残留清理弹窗
    #[cfg(target_os = "windows")]
    ResidualFound(scanner::windows_apps::UninstallResidual),
    /// Windows: 残留清理完成
    #[cfg(target_os = "windows")]
    ResidualCleaned(usize, usize, usize), // (注册表, 环境变量, 文件)
}

/// 将扫描结果按大小降序分批发送，实现增量显示
///
/// 排序后按每批最多 10 项分割，每批之间 sleep 50ms，
/// 让 UI 有机会渲染已发现的项。发送完毕后调用方再发送 Done
/// （Done 携带完整列表，会覆盖累积的部分项，保证最终结果一致）。
pub fn send_items_in_batches(tx: &mpsc::Sender<ScanMessage>, items: &[ScanItem], tab_idx: u64) {
    // App 卸载页不做增量：该页目录多、单目录又慢，增量追加会让列表在扫描
    // 尾部持续"长高/跳动"（用户已确认扫描完成后仍有抖动观感）。
    // 一次性出结果（调用方随后发 Done 携带完整列表），结果稳定不抖。
    // AppUninstall 在 Tab::all() 中的下标为 5（与 app::Tab::tab_index 一致）。
    if tab_idx == 5 {
        return;
    }

    // 按大小降序排序，让用户先看到最大的项
    let mut sorted: Vec<ScanItem> = items.to_vec();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));

    const BATCH_SIZE: usize = 10;
    for chunk in sorted.chunks(BATCH_SIZE) {
        // 报告当前扫描路径
        if let Some(last) = chunk.last() {
            let path = if last.path.is_empty() {
                last.category.clone()
            } else {
                last.path.clone()
            };
            let _ = tx.send(ScanMessage::CurrentPath(path));
        }
        let _ = tx.send(ScanMessage::PartialItems(chunk.to_vec(), tab_idx));
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 尽力删除：优先用系统命令（对 node_modules 等大目录更快），
/// 失败则尝试解除只读标志后再删，再失败就放弃
/// 删除失败原因归因（P1）
///
/// 让用户看到"为什么删不掉"，而不是一个裸的失败：
/// - 权限不足 → 提示可提权重试
/// - SIP 保护 → 即使 root 也删不掉，说明系统保护
/// - 被占用 → 提示关闭占用进程（Windows 常见）
/// - immutable → 提示文件被锁定
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteFailure {
    /// 权限不足（普通删除失败，可能需要提权）
    PermissionDenied,
    /// SIP/系统保护（即使 root 也删不掉）
    SipProtected,
    /// 被其他进程占用
    FileInUse,
    /// immutable/只读标志（已尝试 chflags 解除仍失败）
    Immutable,
    /// 其他原因
    Other,
}
impl DeleteFailure {
    /// 对应 i18n key（失败原因文案）
    pub fn label_key(self) -> &'static str {
        match self {
            Self::PermissionDenied => "fail_reason_perm",
            Self::SipProtected => "fail_reason_sip",
            Self::FileInUse => "fail_reason_inuse",
            Self::Immutable => "fail_reason_immutable",
            Self::Other => "fail_reason_other",
        }
    }
}

/// 把 path 原子改名到同目录下不可预测的 staging 名（TOCTOU 防护辅助）。
///
/// - macOS：`renameatx_np(RENAME_EXCL)` —— "目标不存在才 rename"，原子且绝不覆盖外部文件；
/// - Windows：`std::fs::rename` 对已存在目标直接失败，行为等价；
/// - 源路径已消失（NotFound）：返回原 path，调用方按"已消失"自然处理。
fn rename_to_staging(path: &std::path::Path) -> Result<std::path::PathBuf, DeleteFailure> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => return Err(DeleteFailure::Other),
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for attempt in 0..64u32 {
        let name = format!(
            ".maclean_stage_{}_{:x}_{}",
            std::process::id(),
            nanos,
            attempt
        );
        let staging = parent.join(&name);
        if staging.symlink_metadata().is_ok() {
            continue;
        }
        #[cfg(target_os = "macos")]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;
            let Ok(from) = CString::new(path.as_os_str().as_bytes()) else {
                return Err(DeleteFailure::Other);
            };
            let Ok(to) = CString::new(staging.as_os_str().as_bytes()) else {
                return Err(DeleteFailure::Other);
            };
            // RENAME_EXCL = 0x4：目标已存在则失败，原子且不覆盖外部文件
            let r = unsafe {
                libc::renameatx_np(
                    libc::AT_FDCWD,
                    from.as_ptr(),
                    libc::AT_FDCWD,
                    to.as_ptr(),
                    0x4,
                )
            };
            if r == 0 {
                return Ok(staging);
            }
            match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::PermissionDenied => {
                    return Err(DeleteFailure::PermissionDenied)
                }
                std::io::ErrorKind::NotFound => return Ok(path.to_path_buf()), // 源已消失
                std::io::ErrorKind::AlreadyExists => continue, // staging 被占，换名重试
                _ => return Err(DeleteFailure::Other),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            match std::fs::rename(path, &staging) {
                Ok(()) => return Ok(staging),
                Err(e) => match e.kind() {
                    std::io::ErrorKind::PermissionDenied => {
                        return Err(DeleteFailure::PermissionDenied)
                    }
                    std::io::ErrorKind::NotFound => return Ok(path.to_path_buf()),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::Other => continue,
                    _ => return Err(DeleteFailure::Other),
                },
            }
        }
    }
    Err(DeleteFailure::Other)
}

/// 尝试删除并归因失败原因（P1）
///
/// 删除逻辑与原 `best_effort_delete` 完全一致（rm -rf → chflags/chmod 重试），
/// 但失败时返回具体原因，供 UI 展示"为什么删不掉"。
/// `best_effort_delete` 保留为兼容包装（返回 bool）。
#[cfg(target_os = "macos")]
pub fn best_effort_delete_with_reason(path: &std::path::Path) -> Result<(), DeleteFailure> {
    use std::os::macos::fs::MetadataExt as MacMeta;
    use std::os::unix::fs::MetadataExt as UnixMeta;

    // ========== TOCTOU 防护（对标 MangoDisk 物理身份比对，流程更稳） ==========
    // 1. 捕获物理身份 (st_dev, st_ino) 并持有句柄 —— 防止验证后路径被替换、或
    //    unlink 后同一 inode 编号在删除窗口内被复用；
    // 2. 原子 rename 到同目录不可预测 staging 名（RENAME_EXCL，绝不覆盖外部文件）；
    // 3. 比对 staging 的物理身份与捕获值：不一致说明"验证→删除"之间原路径被换成
    //    新对象 → 恢复原路径并 fail-closed 拒绝删除；
    // 4. 后续 rm/chflags/chmod 只作用于 staging —— 删的始终是验证过的那一个对象。
    let deleted_path = {
        let Some(meta) = std::fs::symlink_metadata(path).ok() else {
            return Ok(()); // 目标已消失，视为删除成功
        };
        let identity = (meta.dev(), meta.ino());
        // 保持句柄打开直到本函数结束：防止 unlink 后同一物理身份被复用
        let _identity_handle = std::fs::File::open(path);

        let staging = rename_to_staging(path)?;
        let Some(staging_meta) = std::fs::symlink_metadata(&staging).ok() else {
            let _ = std::fs::rename(&staging, path); // 尽力恢复原路径
            return Err(DeleteFailure::Other);
        };
        if (staging_meta.dev(), staging_meta.ino()) != identity {
            // 验证后原路径被替换成新对象：恢复原位并拒绝删除（fail-closed）
            let _ = std::fs::rename(&staging, path);
            return Err(DeleteFailure::Other);
        }
        staging
    };

    let path_str = path.to_string_lossy().to_string(); // 归因用原路径（SIP 判断基于原路径）
    let deleted_str = deleted_path.to_string_lossy().to_string(); // 操作用 staging
    let gone = |p: &std::path::Path| !p.exists() && p.symlink_metadata().is_err();
    let rm = |p: &str| {
        std::process::Command::new("/bin/rm")
            .arg("-rf")
            .arg(p)
            .output()
    };

    // 第一遍：直接 rm staging
    if let Ok(o) = rm(&deleted_str) {
        if o.status.success() && gone(&deleted_path) {
            return Ok(());
        }
        if let Some(f) = classify_rm_failure(&o.stderr, &path_str) {
            return Err(f);
        }
    }

    // 第二遍：解除 immutable/只读标志后再删
    let _ = std::process::Command::new("/usr/bin/chflags")
        .arg("-R")
        .arg("nouchg")
        .arg(&deleted_str)
        .output();
    let _ = std::process::Command::new("/bin/chmod")
        .arg("-R")
        .arg("u+w")
        .arg(&deleted_str)
        .output();
    if let Ok(o) = rm(&deleted_str) {
        if o.status.success() && gone(&deleted_path) {
            return Ok(());
        }
        if let Some(f) = classify_rm_failure(&o.stderr, &path_str) {
            return Err(f);
        }
    }

    // 仍存在且无法归类：检查 immutable 标志（chflags 尝试可能没权限或没生效）
    {
        if let Ok(meta) = std::fs::symlink_metadata(&deleted_path) {
            let flags = meta.st_flags();
            // UF_IMMUTABLE=0x0001, SF_IMMUTABLE=0x8000
            if flags & 0x0001 != 0 || flags & 0x8000 != 0 {
                return Err(DeleteFailure::Immutable);
            }
        }
    }
    Err(DeleteFailure::Other)
}

/// 从 rm 的 stderr 归类失败原因（macOS）
#[cfg(target_os = "macos")]
fn classify_rm_failure(stderr: &[u8], path: &str) -> Option<DeleteFailure> {
    let text = String::from_utf8_lossy(stderr).to_lowercase();
    if text.contains("operation not permitted") || text.contains("not permitted") {
        return Some(if crate::safety::is_critical_system_path(path) {
            DeleteFailure::SipProtected
        } else {
            DeleteFailure::PermissionDenied
        });
    }
    if text.contains("resource busy") || text.contains("device busy") {
        return Some(DeleteFailure::FileInUse);
    }
    if text.contains("read-only file system") {
        return Some(DeleteFailure::SipProtected);
    }
    None
}

/// 尝试删除并归因失败原因（Windows）
#[cfg(target_os = "windows")]
pub fn best_effort_delete_with_reason(path: &std::path::Path) -> Result<(), DeleteFailure> {
    // ========== TOCTOU 防护（Windows 版） ==========
    // 先原子 rename 到同目录不可预测 staging（Windows 上 rename 目标存在即失败，
    // 天然不覆盖外部文件），随后所有删除只作用于 staging —— 删的始终是验证过
    // 的那一个对象，路径在验证后被替换也无法命中。
    // 限制：Windows 端未做物理身份(st_dev/st_ino)比对，依赖 staging 名不可预测
    //  + rename 原子性缩短竞态窗口；如需更强可引入 windows crate 取 FileIndex。
    let deleted_path = {
        if !path.exists() && path.symlink_metadata().is_err() {
            return Ok(());
        }
        rename_to_staging(path)?
    };

    let path_str = path.to_string_lossy().to_string();
    let deleted_str = deleted_path.to_string_lossy().to_string();
    let gone = |p: &std::path::Path| !p.exists() && p.symlink_metadata().is_err();

    let cmd_out = if deleted_path.is_dir() {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg("rd")
            .arg("/S")
            .arg("/Q")
            .arg(&deleted_str)
            .output()
    } else {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg("del")
            .arg("/F")
            .arg("/Q")
            .arg(&deleted_str)
            .output()
    };
    if let Ok(o) = &cmd_out {
        if o.status.success() && gone(&deleted_path) {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&o.stderr).to_lowercase();
        if text.contains("being used by another process") || text.contains("in use") {
            return Err(DeleteFailure::FileInUse);
        }
        if text.contains("access is denied") {
            return Err(DeleteFailure::PermissionDenied);
        }
    }

    // Rust API fallback
    let res = if deleted_path.is_dir() {
        std::fs::remove_dir_all(&deleted_path)
    } else {
        std::fs::remove_file(&deleted_path)
    };
    match res {
        Ok(()) if gone(&deleted_path) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(DeleteFailure::PermissionDenied)
        }
        Err(_) => Err(DeleteFailure::Other),
        _ => Err(DeleteFailure::Other),
    }
}

/// 尝试删除并归因失败原因（其他平台）
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn best_effort_delete_with_reason(path: &std::path::Path) -> Result<(), DeleteFailure> {
    let res = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match res {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(DeleteFailure::PermissionDenied)
        }
        Err(_) => Err(DeleteFailure::Other),
    }
}

/// 查找占用指定路径的进程名（Windows，P1）
///
/// 用 Windows 自带的 Restart Manager（rstrtmgr.dll）枚举占用进程，
/// 不依赖任何第三方工具（handle.exe / Process Explorer 都不需要）。
/// 找不到或调用失败返回空列表，由调用方决定如何提示。
#[cfg(target_os = "windows")]
pub fn find_locking_processes(path: &str) -> Vec<String> {
    // Restart Manager 的 P/Invoke 声明 + 查询流程，全部是 Windows 系统 API。
    // 占用的进程可能尚未写入 strAppName（如服务），此时过滤掉空名。
    let script = r#"
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class Rm {
    [StructLayout(LayoutKind.Sequential)] public struct RM_UNIQUE_PROCESS { public int dwProcessId; public System.Runtime.InteropServices.ComTypes.FILETIME ProcessStartTime; }
    public const int CCH_RM_MAX_APP_NAME = 255;
    [DllImport("rstrtmgr.dll", CharSet=CharSet.Unicode)] public static extern int RmStartSession(out uint pSessionHandle, int dwSessionFlags, string strSessionKey);
    [DllImport("rstrtmgr.dll", CharSet=CharSet.Unicode)] public static extern int RmRegisterResources(uint pSessionHandle, uint nFiles, string[] rgsFilenames, uint nApplications, RM_UNIQUE_PROCESS[] rgApplications, uint nServices, string[] rgsServiceNames);
    [DllImport("rstrtmgr.dll")] public static extern int RmGetList(uint dwSessionHandle, out uint pnProcInfoNeeded, ref uint pnProcInfo, [In, Out] RM_PROCESS_INFO[] rgAffectedApps, ref uint lpdwRebootReasons);
    [StructLayout(LayoutKind.Sequential)] public struct RM_PROCESS_INFO { public RM_UNIQUE_PROCESS Process; [MarshalAs(UnmanagedType.ByValTStr, SizeConst=CCH_RM_MAX_APP_NAME)] public string strAppName; public int ApplicationType; public uint AppStatus; public uint TSSessionId; [MarshalAs(UnmanagedType.Bool)] public bool bRestartable; }
    [DllImport("rstrtmgr.dll")] public static extern int RmEndSession(uint pSessionHandle);
}
"@
$key = [guid]::NewGuid().ToString()
$session = [uint32]0
[Rm]::RmStartSession([ref]$session, 0, $key) | Out-Null
$paths = @($args[0])
[Rm]::RmRegisterResources($session, [uint32]$paths.Count, $paths, 0, $null, 0, $null) | Out-Null
$needed = [uint32]0
$count = [uint32]0
$reasons = [uint32]0
[Rm]::RmGetList($session, [ref]$needed, [ref]$count, $null, [ref]$reasons) | Out-Null
if ($needed -gt 0) {
    $apps = New-Object Rm+RM_PROCESS_INFO[] $needed
    $count = $needed
    [Rm]::RmGetList($session, [ref]$needed, [ref]$count, $apps, [ref]$reasons) | Out-Null
    foreach ($a in $apps) { if ($a.strAppName) { $a.strAppName } }
}
[Rm]::RmEndSession($session) | Out-Null
"#;
    let out = std::process::Command::new("powershell")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        // 脚本与参数分开传：path 作为 $args[0]，Rust 负责按平台规则引号转义，
        // 避免手工拼接 format 字符串把引号/花括号搞坏
        .arg(script)
        .arg(path)
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// 获取系统所有挂载点 (macOS 专属)
///
/// 用 `getfsstat` 直接取内核的挂载表，而不是解析 `/sbin/mount` 的文本输出。
/// 文本解析有两个致命问题：
/// 1. 挂载点含空格（如 `/Volumes/My Disk`）时被 `split_whitespace` 截断，
///    导致路径匹配失败 → 误判"未挂载"而放行删除正在使用的卷；
/// 2. `mount` 输出是给人看的，含转义字符（`\040`），不是可靠的数据接口。
#[cfg(target_os = "macos")]
pub fn get_mount_points() -> Vec<String> {
    use std::ffi::CStr;

    unsafe {
        // 先取缓冲区大小，再分配
        let count = libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT);
        if count <= 0 {
            return Vec::new();
        }
        let cap = (count as usize).saturating_add(8);
        let mut buf: Vec<libc::statfs> = vec![std::mem::zeroed(); cap];
        let n = libc::getfsstat(
            buf.as_mut_ptr(),
            (cap * std::mem::size_of::<libc::statfs>()) as libc::c_int,
            libc::MNT_NOWAIT,
        );
        if n <= 0 {
            return Vec::new();
        }

        let mut mounts = Vec::new();
        for entry in buf.iter().take(n as usize) {
            let cstr = CStr::from_ptr(entry.f_mntonname.as_ptr());
            if let Ok(s) = cstr.to_str() {
                if !s.is_empty() {
                    mounts.push(s.to_string());
                }
            }
        }
        mounts
    }
}

/// 检查路径是否处于某个挂载点之内（macOS 专属）
///
/// 双向判断：
/// - 挂载点就是该路径，或位于该路径之下（原来的方向）；
/// - **该路径本身位于某个挂载点之内**（新增）—— 例如传入的目录位于一个
///   已挂载的 DMG 里，原来会被判为"未挂载"从而放行删除。
#[cfg(target_os = "macos")]
pub fn is_path_mounted(path: &str, mount_points: &[String]) -> bool {
    let normalized = path.trim_end_matches('/');
    for mp in mount_points {
        let m = mp.trim_end_matches('/');
        if m.is_empty() {
            continue;
        }
        // 挂载点 == 路径，或挂载点在路径之下
        if m == normalized || m.starts_with(&format!("{}/", normalized)) {
            return true;
        }
        // 路径本身在某个挂载点之内（且不是 "/" 这种根挂载）
        if m != "/" && normalized.starts_with(&format!("{}/", m)) {
            return true;
        }
    }
    false
}

/// 移动文件/目录到废纸篓（可恢复）
/// 用于 Caution/Advanced 级别的文件，给用户后悔的机会
/// 移动文件到废纸篓（跨平台，委托给 platform 模块）
pub fn move_to_trash(path: &str) -> bool {
    platform::move_to_trash(path)
}

/// `xcrun simctl runtime list -j` 中的单个 runtime 条目
#[cfg(target_os = "macos")]
#[derive(serde::Deserialize)]
pub struct SimRuntimeInfo {
    /// 挂载路径，如 /Library/Developer/CoreSimulator/Volumes/iOS_22F77
    #[serde(default, rename = "mountPath")]
    mount_path: Option<String>,
    /// Cryptex 镜像的父挂载路径
    #[serde(default, rename = "parentMountPath")]
    parent_mount_path: Option<String>,
    /// 系统是否标记该项可删除（正在使用/被依赖的会是 false）
    #[serde(default)]
    deletable: bool,
}

/// 解析出「归属于 path」且「系统允许删除」的模拟器 runtime UUID
///
/// 旧实现用正则从 `runtime list` 的整段文本里抓所有 UUID 再逐个删除，问题：
/// 1. 会连 `parentIdentifier` 这类无关 UUID 一起抓；
/// 2. 完全忽略 path —— 勾选一项却把机器上所有模拟器运行时连锅端；
/// 3. 不看系统给的 `deletable` 标记，正在使用的 runtime 也会被尝试删除。
///
/// 现在：JSON 解析 + 按 mountPath/parentMountPath 归属过滤 + 只取 deletable。
/// 解析失败时**拒绝删除**（fail-closed），不再退化为"全删"。
#[cfg(target_os = "macos")]
pub fn resolve_simulator_uuids(path: &str) -> Result<Vec<String>, String> {
    // fail-closed：空路径会让归属前缀退化成 "/"，把全部 runtime 判成"属于本项"
    // —— 与"只删勾选归属项"的设计矛盾，直接拒绝。
    if path.trim().is_empty() || path.trim() == "/" {
        return Err("resolve_simulator_uuids: 归属路径为空，拒绝解析".to_string());
    }
    let output = std::process::Command::new("xcrun")
        .args(["simctl", "runtime", "list", "-j"])
        .output()
        .map_err(|e| format!("xcrun simctl runtime list -j: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "xcrun simctl runtime list -j: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let map: std::collections::HashMap<String, SimRuntimeInfo> =
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("解析模拟器 runtime 列表失败: {}", e))?;

    let prefix = format!("{}/", path);
    let belongs = |candidate: Option<&String>| match candidate {
        Some(p) => *p == path || p.starts_with(&prefix),
        None => false,
    };

    let mut uuids: Vec<String> = map
        .iter()
        .filter(|(_, v)| v.deletable)
        .filter(|(_, v)| belongs(v.mount_path.as_ref()) || belongs(v.parent_mount_path.as_ref()))
        .map(|(k, _)| k.clone())
        .collect();
    uuids.sort();

    Ok(uuids)
}

/// 安全删除模拟器运行时镜像（/Library/Developer/CoreSimulator/Volumes）
///
/// 安全检查层：
/// 1. 进程检测：Xcode/Simulator/CoreSimulatorService 运行时拒绝删除
/// 2. 挂载点检测：正在挂载使用的运行时跳过，只删 UNUSED 的
/// 3. 使用 `xcrun simctl runtime delete <uuid>` 安全删除
/// 4. 如果 xcrun 失败，返回 Err 让调用方 fallback 到 sudo rm -rf（仅 UNUSED 项）
#[cfg(target_os = "macos")]
pub fn delete_simulator_volumes(path: &str, lang_en: bool) -> Result<String, String> {
    // 1. 进程检测：模拟器运行中时拒绝删除
    if safety::is_simulator_running() {
        return Err(crate::i18n::t_lang(lang_en, "log_skip_running")
            .replace("[{}]", "")
            .trim()
            .to_string());
    }

    // 2. 挂载点检测：如果路径被挂载使用，跳过
    let mount_points = get_mount_points();
    if mount_points.is_empty() {
        // mount 命令失败，无法确认安全，拒绝删除
        return Err(crate::i18n::t_lang(lang_en, "log_cannot_get_mount").to_string());
    }
    if is_path_mounted(path, &mount_points) {
        return Err(crate::i18n::t_lang(lang_en, "log_mount_in_use").to_string());
    }

    // 3. 只解析归属于该路径、且系统允许删除的 runtime
    let uuids: Vec<String> = resolve_simulator_uuids(path)?;

    if uuids.is_empty() {
        return Err(crate::i18n::t_lang(lang_en, "log_no_sim_runtimes").to_string());
    }

    // 4. 逐个删除（只删上面筛出来的这些，不再"全删"）
    let mut success_count = 0;
    let mut fail_msgs: Vec<String> = Vec::new();

    for uuid in &uuids {
        let del_output = std::process::Command::new("xcrun")
            .args(["simctl", "runtime", "delete", uuid])
            .output();

        match del_output {
            Ok(o) if o.status.success() => {
                success_count += 1;
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                // 如果是 "not found" 或 "already deleted" 类错误，算成功
                if stderr.contains("not found") || stderr.contains("No such") {
                    success_count += 1;
                } else {
                    fail_msgs.push(format!("{}: {}", uuid, stderr.trim()));
                }
            }
            Err(e) => {
                fail_msgs.push(format!("{}: {}", uuid, e));
            }
        }
    }

    if success_count > 0 {
        let suffix = if !fail_msgs.is_empty() {
            crate::i18n::tf_lang(
                lang_en,
                "log_sim_failed_suffix",
                &[&fail_msgs.len().to_string()],
            )
        } else {
            String::new()
        };
        let msg = crate::i18n::tf_lang(
            lang_en,
            "log_sim_deleted",
            &[&success_count.to_string(), &suffix],
        );
        Ok(msg)
    } else {
        Err(format!(
            "{}: {}",
            crate::i18n::t_lang(lang_en, "log_unknown"),
            fail_msgs.join("; ")
        ))
    }
}

/// 执行 Docker 系统清理
///
/// 运行 `docker system prune -a -f` 清理:
/// - 所有已停止的容器
/// - 所有未被容器使用的网络
/// - 所有未被容器引用的镜像（dangling + unused）
/// - 所有构建缓存
///
/// 注意：**不清理数据卷**。卷通常存放数据库、上传文件等不可重建的数据，
/// 且 `prune --volumes` 无确认、不可恢复。需要清卷请让用户在 Docker Desktop
/// 或 `docker volume prune`（交互式列出卷名）中自行操作。
pub fn run_docker_prune(lang_en: bool) -> Result<String, String> {
    // 先检查 Docker 是否运行
    let info_check = std::process::Command::new("docker")
        .arg("info")
        .output()
        .map_err(|e| format!("{}: {}", crate::i18n::t_lang(lang_en, "log_unknown"), e))?;

    if !info_check.status.success() {
        return Err(crate::i18n::t_lang(lang_en, "log_docker_not_running").to_string());
    }

    // 执行 prune（-f 跳过交互确认，-a 删除所有未使用镜像）
    //
    // P1-3：这里**不带 --volumes**。数据卷里常放数据库、上传文件等用户数据，
    // 而 `prune --volumes` 是静默全删且不可恢复；界面上也没有让用户确认卷清单的环节。
    // 需要清理卷时请用 Docker Desktop 或 `docker volume prune`（它会列出卷并交互式确认）。
    let output = std::process::Command::new("docker")
        .args(["system", "prune", "-a", "-f"])
        .output()
        .map_err(|e| format!("{}: {}", crate::i18n::t_lang(lang_en, "log_unknown"), e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(format!("docker prune: {}", stderr.trim()));
    }

    // 解析输出中的释放空间
    // 输出包含: "Total reclaimed space: 1.2GB"
    let reclaimed = stdout
        .lines()
        .find(|line| line.contains("Total reclaimed space"))
        .map(|line| line.split(':').nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_else(|| crate::i18n::t_lang(lang_en, "log_unknown").to_string());

    Ok(crate::i18n::tf_lang(
        lang_en,
        "log_docker_done",
        &[&reclaimed],
    ))
}

/// 执行 OrbStack 清理（orbctl 官方命令通道）。
///
/// 与 Docker 同理：OrbStack 的虚拟机/镜像文件是数据库结构，直接删文件
/// 会破坏镜像元数据（9-30 事故教训）。只走 orbctl 官方命令：
/// `orbctl delete <machine>` 逐个删除**已停止**的虚拟机（running 的一律
/// 跳过，避免中断服务）。orbctl **没有 prune 命令**（实测），reset 会
/// 删除全部 Linux/Docker 数据（太危险，绝不自动执行）。任何失败如实归因。
pub fn run_orbctl_clean(lang_en: bool) -> Result<String, String> {
    // 1. 先检查 orbctl 可用（orbctl 无 --version 标志，用 status 探测）
    let status_check = std::process::Command::new("orbctl")
        .arg("status")
        .output()
        .map_err(|e| format!("{}: {}", crate::i18n::t_lang(lang_en, "log_unknown"), e))?;
    if !status_check.status.success() {
        return Err("orbctl 不可用，请确认 OrbStack 已安装并启动".to_string());
    }

    let mut logs: Vec<String> = Vec::new();
    let mut deleted_any = false;

    // 2. 解析 orbctl list（空格分隔文本，非 JSON）：
    //    name  status  distro  version  arch  size  ip
    let list = std::process::Command::new("orbctl").arg("list").output();
    if let Ok(out) = list {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            for line in stdout.lines() {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() < 2 || cols[1] != "stopped" {
                    continue; // 只删 stopped；running 跳过
                }
                let name = cols[0];
                if name.is_empty() || name == "NAME" {
                    continue;
                }
                let rm = std::process::Command::new("orbctl")
                    .args(["delete", name])
                    .output();
                match rm {
                    Ok(r) if r.status.success() => {
                        deleted_any = true;
                        logs.push(format!("已删除虚拟机: {}", name));
                    }
                    Ok(r) => {
                        logs.push(format!(
                            "删除虚拟机失败 {}: {}",
                            name,
                            String::from_utf8_lossy(&r.stderr).trim()
                        ));
                    }
                    Err(e) => {
                        logs.push(format!("删除虚拟机失败 {}: {}", name, e));
                    }
                }
            }
        }
    }

    if logs.is_empty() {
        Ok("OrbStack 没有可清理的已停止虚拟机".to_string())
    } else if deleted_any {
        Ok(logs.join("；"))
    } else {
        Err(logs.join("；"))
    }
}

/// 启动后台删除线程（两阶段自动删除）
/// 阶段1: 普通删除（多线程并行 rm -rf）
/// 阶段2: 对失败项自动 sudo 批量删除（后台并发，只弹一次密码框）
pub fn start_delete(
    to_delete: Vec<(String, String, Vec<String>, bool, u64)>,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
    auto_restore: bool,
    // 卸载 .app 时优先交给厂商自带的官方卸载器（M-1，仅 macOS 有意义）
    prefer_official_uninstaller: bool,
    // P0-3：删除取消标志。UI 点「停止」置 true，worker 在子项边界检查，
    // 未处理的项原地保留，已删项照常落清单。
    delete_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        let failed_items: std::sync::Mutex<Vec<(String, String)>> =
            std::sync::Mutex::new(Vec::new());
        // M-2：本次删除实际删掉的项，结束后落一份清单。
        // 只记"真删掉了的"—— 不存在/被拒绝/失败的项不进清单，
        // 否则用户会以为还能还原一个从没被删过的东西。
        let backup_entries: std::sync::Mutex<Vec<crate::backup::BackupEntry>> =
            std::sync::Mutex::new(Vec::new());
        // P0-3：是否被用户中途取消（任一 worker 置位）
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        logger::info(&format!("删除任务开始: {} 项", to_delete.len()));

        // P0-2：删除开始前落"待删快照"（<id>.pending.json）。
        // 9-30 事故：删除中途 GUI 终止，正式清单（record）从未写入，
        // 事后完全无法还原"当时计划删什么"。快照先行 —— 正常结束由
        // finalize_pending 清除，中断/崩溃则留存供审计。
        let pending_id: Option<String> = {
            let mut pending: Vec<crate::backup::BackupEntry> = Vec::new();
            for (path, category, batch_paths, use_trash, size_bytes) in &to_delete {
                let restorable =
                    crate::backup::is_restorable_by_move(*use_trash, std::env::consts::OS);
                pending.push(crate::backup::BackupEntry {
                    path: path.clone(),
                    size_bytes: *size_bytes,
                    category: category.clone(),
                    restorable,
                });
                for bp in batch_paths {
                    pending.push(crate::backup::BackupEntry {
                        path: bp.clone(),
                        size_bytes: *size_bytes,
                        category: category.clone(),
                        restorable,
                    });
                }
            }
            crate::backup::record_pending(pending)
        };

        // P1-1: Windows 批次删除前自动创建系统还原点（20h 频率限制，开关控制）
        #[cfg(not(target_os = "windows"))]
        let _ = auto_restore;
        #[cfg(target_os = "windows")]
        if auto_restore {
            let (ok, msg) = platform::windows_backup::ensure_restore_point(false);
            let text = match (ok, msg.as_str()) {
                (true, "created") => {
                    crate::i18n::t_lang(lang_en, "restore_point_created").to_string()
                }
                (true, _) => crate::i18n::t_lang(lang_en, "restore_point_skipped").to_string(),
                (false, _) => crate::i18n::t_lang(lang_en, "restore_point_failed").to_string(),
            };
            let _ = tx.send(DeleteMessage::Info(text));
        }

        // ========== 阶段1: 普通删除（并行度调优，对标 MangoDisk benchmark 结论） ==========
        // 平台分层 worker 上限：macOS 2 / Windows 4 —— 更高并发在活跃索引/防病毒扫描
        // 场景下删除延迟不稳定；条目少时线程启动成本大于文件系统工作，直接串行。
        const PARALLEL_DELETE_ENTRY_THRESHOLD: usize = 16;
        #[cfg(target_os = "macos")]
        const MAX_PARALLEL_DELETE_WORKERS: usize = 2;
        #[cfg(target_os = "windows")]
        const MAX_PARALLEL_DELETE_WORKERS: usize = 4;
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        const MAX_PARALLEL_DELETE_WORKERS: usize = 4;

        let worker_count = if to_delete.len() >= PARALLEL_DELETE_ENTRY_THRESHOLD {
            std::cmp::min(MAX_PARALLEL_DELETE_WORKERS, to_delete.len())
        } else {
            1 // 小批串行，省掉线程调度开销
        };
        let idx = std::sync::atomic::AtomicUsize::new(0);

        std::thread::scope(|s| {
            for _ in 0..worker_count {
                s.spawn(|| {
                    loop {
                        let i = idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= to_delete.len() {
                            break;
                        }
                        // P0-3：用户中途取消 —— 未处理的项原地保留，不再删除。
                        // 其余 worker 下一轮也会在此退出；scope 等待全部退出。
                        if delete_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                            break;
                        }
                        let (path, category, batch_paths, use_trash, size_bytes) = &to_delete[i];
                        let path = path.to_string();
                        let category = category.to_string();
                        let batch_paths = batch_paths.clone();
                        let use_trash = *use_trash;
                        let size_bytes = *size_bytes;

                        // M-1：应用包优先交给官方卸载器。
                        // 删目录≠卸载软件：launchd 任务、pkgutil 收据、系统扩展
                        // 授权都清不掉，卸载完还会每分钟拉起一个已不存在的二进制。
                        // 启动失败不阻断 —— 回落到正常删除，用户至少还能卸掉文件。
                        #[cfg(target_os = "macos")]
                        if prefer_official_uninstaller && path.ends_with(".app") {
                            let name =
                                crate::scanner::official_uninstaller::app_display_name(&path);
                            if let Some(u) =
                                crate::scanner::official_uninstaller::find_official_uninstaller(
                                    &path, &name,
                                )
                            {
                                logger::info(&format!("交由官方卸载器处理: {} -> {}", path, u.path));
                                match crate::scanner::official_uninstaller::launch(&u) {
                                    Ok(_) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("↗ {} [{}] {}",
                                                crate::i18n::t_lang(lang_en, "log_official_uninstaller"),
                                                category, path),
                                            path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                        continue;
                                    }
                                    Err(e) => {
                                        logger::warn(&format!(
                                            "官方卸载器启动失败，回落到删除目录: {} ({})",
                                            path, e
                                        ));
                                    }
                                }
                            }
                        }

                        // Windows 应用卸载特殊处理（干净卸载：卸载程序 + 扫描残留，不自动清理）
                        #[cfg(target_os = "windows")]
                        if path.starts_with("uwp:") || path.starts_with("uninstall:") {
                            logger::info(&format!("开始卸载应用: {}", path));

                            // 查找应用信息以支持干净卸载
                            let key_name = if path.starts_with("uninstall:") {
                                &path[10..]
                            } else {
                                ""
                            };
                            let app_info = scanner::windows_apps::find_app_by_key(key_name);
                            let app_name = app_info.as_ref().map(|a| a.name.as_str()).unwrap_or("");
                            let install_path = app_info.as_ref().and_then(|a| a.install_location.as_deref());

                            // 执行干净卸载（只卸载 + 扫描残留，不自动清理）
                            // 用户选择权：残留信息返回给用户，由用户决定是否清理
                            let (success, msg, residual) = scanner::windows_apps::clean_uninstall(
                                &path,
                                app_name,
                                install_path,
                            );

                            if success {
                                logger::info(&format!("卸载成功: {}", msg));
                            } else {
                                logger::error(&format!("卸载失败: {}", msg));
                            }

                            // 记录残留扫描详情（不自动清理，等待用户确认）
                            if !residual.is_empty() {
                                let fs_size: u64 = residual.filesystem.iter().map(|f| f.size).sum();
                                logger::info(&format!(
                                    "残留扫描结果: 注册表 {} 项 (可删 {}), 环境变量 {} 项 (可删 {}), 文件 {} 项 (可删 {}, 共 {}) — 等待用户选择",
                                    residual.registry.len(),
                                    residual.registry.iter().filter(|r| r.deletable).count(),
                                    residual.env_vars.len(),
                                    residual.env_vars.iter().filter(|e| e.deletable).count(),
                                    residual.filesystem.len(),
                                    residual.filesystem.iter().filter(|f| f.deletable).count(),
                                    scanner::format_size(fs_size),
                                ));
                                // 发送残留信息到 UI，弹出残留清理弹窗让用户选择
                                let _ = tx.send(DeleteMessage::ResidualFound(residual));
                            }

                            let _ = tx.send(DeleteMessage::Log(
                                if success { format!("✓ {}", msg) } else { format!("✗ {}", msg) },
                                path.clone(), category.clone(), success));
                            safety::log_deletion(&path, &category, success, None);
                            if !success {
                                failed_items.lock().unwrap_or_else(|e| e.into_inner()).push((path.clone(), category.clone()));
                            }
                            continue;
                        }

                        // 批量删除模式（如 __pycache__）：逐个安全删除
                        if !batch_paths.is_empty() {
                            let mut success_count = 0;
                            let mut fail_count = 0;
                            for bp in &batch_paths {
                                match safety::check_path_safety_with_category(bp, &category) {
                                    safety::SafetyCheck::Danger(reason) => {
                                        fail_count += 1;
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("⛔ {}", crate::i18n::tf_lang(lang_en, "log_intercepted", &[bp, &reason])), bp.clone(), category.clone(), false));
                                        safety::log_deletion(bp, &category, false, Some(&reason));
                                        continue;
                                    }
                                    safety::SafetyCheck::Warning(reason) => {
                                        fail_count += 1;
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("⚠️ {}", crate::i18n::tf_lang(lang_en, "log_skipped", &[bp, &reason])), bp.clone(), category.clone(), false));
                                        safety::log_deletion(bp, &category, false, Some(&reason));
                                        continue;
                                    }
                                    safety::SafetyCheck::Safe => {}
                                }
                                let p = std::path::Path::new(bp.as_str());
                                // 对齐：已消失的路径记 SKIP —— 之前直接算成功，
                                // 用户什么都没做也让成功数虚高。
                                if !p.exists() && p.symlink_metadata().is_err() {
                                    let _ = tx.send(DeleteMessage::Skip(
                                        format!(
                                            "⏭️ {}",
                                            crate::i18n::tf_lang(
                                                lang_en,
                                                "log_skipped",
                                                &[bp, crate::i18n::t_lang(lang_en, "already_cleaned")]
                                            )
                                        ),
                                        bp.clone(),
                                        category.clone(),
                                    ));
                                    continue;
                                }
                                // 止血：批量分支尊重 use_trash —— 重复文件等批量项
                                // 同样走废纸篓，不再一律 best_effort 永久删除。
                                let deleted_ok = if use_trash {
                                    move_to_trash(bp)
                                } else {
                                    best_effort_delete_with_reason(p).is_ok()
                                };
                                if deleted_ok {
                                    success_count += 1;
                                    // M-2：批量项真删掉也记备份清单
                                    let restorable = crate::backup::is_restorable_by_move(
                                        use_trash,
                                        std::env::consts::OS,
                                    );
                                    if let Ok(mut guard) = backup_entries.lock() {
                                        guard.push(crate::backup::BackupEntry {
                                            path: bp.clone(),
                                            size_bytes,
                                            category: category.clone(),
                                            restorable,
                                        });
                                    }
                                } else {
                                    fail_count += 1;
                                    failed_items.lock().unwrap_or_else(|e| e.into_inner())
                                        .push((bp.clone(), category.clone()));
                                    // 失败归因 —— 用户能看到"为什么删不掉"
                                    let reason = if use_trash {
                                        crate::i18n::t_lang(lang_en, "log_trash_failed")
                                            .replace("{}", "")
                                            .trim()
                                            .to_string()
                                    } else {
                                        let failure =
                                            best_effort_delete_with_reason(p).err();
                                        #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
                                        let mut reason: String = failure
                                            .map(|f| crate::i18n::t_lang(lang_en, f.label_key()).to_string())
                                            .unwrap_or_else(|| {
                                                crate::i18n::t_lang(lang_en, "fail_reason_other").to_string()
                                            });
                                        // P1：Windows 上被占用时，尽力查出占用进程名
                                        #[cfg(target_os = "windows")]
                                        if let Some(DeleteFailure::FileInUse) = failure {
                                            let lockers = find_locking_processes(bp);
                                            if !lockers.is_empty() {
                                                reason = format!("{}: {}", reason, lockers.join(", "));
                                            }
                                        }
                                        reason
                                    };
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!(
                                            "⚠️ {} — {}",
                                            crate::i18n::t_lang(lang_en, "log_still_exists"),
                                            reason
                                        ),
                                        bp.clone(),
                                        category.clone(),
                                        false,
                                    ));
                                }
                            }
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ {}", crate::i18n::tf_lang(lang_en, "log_deleted", &[&category, &path, &success_count.to_string(), &fail_count.to_string()])),
                                path.clone(), category.clone(), fail_count == 0));
                            // 聚合伪路径（如 "Monorepo: xxx (N 个子包)"）不是真实文件，
                            // 不落 delete.log —— 否则每轮必记一条 FAIL 虚高失败数、
                            // 弹窗报"失败 N 项"误导用户。真实路径仍正常记录。
                            if std::path::Path::new(&path).exists() {
                                safety::log_deletion(&path, &category, fail_count == 0, None);
                            }
                            continue;
                        }

                        // 安全校验
                        match safety::check_path_safety_with_category(&path, &category) {
                            safety::SafetyCheck::Danger(reason) => {
                                let _ = tx.send(DeleteMessage::Log(
                                    format!("⛔ {}", crate::i18n::tf_lang(lang_en, "log_intercepted", &[&path, &reason])), path.clone(), category.clone(), false));
                                safety::log_deletion(&path, &category, false, Some(&reason));
                                continue;
                            }
                            safety::SafetyCheck::Warning(reason) => {
                                let _ = tx.send(DeleteMessage::Log(
                                    format!("⚠️ {}", crate::i18n::tf_lang(lang_en, "log_skipped", &[&path, &reason])), path.clone(), category.clone(), false));
                                safety::log_deletion(&path, &category, false, Some(&reason));
                                continue;
                            }
                            safety::SafetyCheck::Safe => {}
                        }

                        // APFS 快照特殊处理 (macOS 专属)
                        if cfg!(target_os = "macos") && category == "APFS快照" {
                            #[cfg(target_os = "macos")]
                            {
                                match scanner::apfs::delete_snapshot(&path) {
                                    Ok(_) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✓ {}", crate::i18n::tf_lang(lang_en, "log_snapshot_deleted", &[&path])), path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                    }
                                    Err(e) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✗ {}", crate::i18n::tf_lang(lang_en, "log_delete_failed", &[&path, &e])), path.clone(), category.clone(), false));
                                        safety::log_deletion(&path, &category, false, Some(&e));
                                    }
                                }
                            }
                            continue;
                        }

                        if cfg!(target_os = "macos") && category == "模拟器运行时" {
                            #[cfg(target_os = "macos")]
                            {
                                match scanner::apfs::delete_simulator_runtime(&path) {
                                    Ok(_) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✓ {}", crate::i18n::tf_lang(lang_en, "log_runtime_deleted", &[&path])), path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                    }
                                    Err(e) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✗ {}", crate::i18n::tf_lang(lang_en, "log_delete_failed", &[&path, &e])), path.clone(), category.clone(), false));
                                        safety::log_deletion(&path, &category, false, Some(&e));
                                    }
                                }
                            }
                            continue;
                        }

                        // 模拟器镜像/Cryptex — 通过 xcrun simctl runtime delete 安全删除 (macOS 专属)
                        if cfg!(target_os = "macos") && (category == "模拟器镜像" || category == "模拟器Cryptex") {
                            #[cfg(target_os = "macos")]
                            {
                                match delete_simulator_volumes(&path, lang_en) {
                                    Ok(msg) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✓ {}", msg), path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                    }
                                    Err(e) => {
                                        // xcrun 失败，加入 sudo 重试列表
                                        failed_items.lock().unwrap_or_else(|e| e.into_inner()).push((path.clone(), category.clone()));
                                        let _ = tx.send(DeleteMessage::Info(
                                            format!("🔄 {}", crate::i18n::tf_lang(lang_en, "log_xcrun_failed", &[&e])),
                                        ));
                                    }
                                }
                            }
                            continue;
                        }

                        // 模拟器缓存 — 进程检测后删除 (macOS 专属)
                        if cfg!(target_os = "macos") && category == "模拟器缓存" {
                            #[cfg(target_os = "macos")]
                            {
                                if safety::is_simulator_running() {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("⏭️ {}", crate::i18n::tf_lang(lang_en, "log_skip_running", &[&path])), path.clone(), category.clone(), false));
                                    safety::log_deletion(&path, &category, false, Some(crate::i18n::t_lang(lang_en, "log_skip_running").replace("[{}]", "").trim()));
                                    continue;
                                }
                            }
                            // 走普通删除流程（会自动 fallback 到 sudo）
                        }

                        // Docker 清理 — 通过 docker system prune 命令清理
                        if category == "Docker清理" {
                            match run_docker_prune(lang_en) {
                                Ok(msg) => {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("✓ {}", msg), path.clone(), category.clone(), true));
                                    safety::log_deletion(&path, &category, true, None);
                                }
                                Err(e) => {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("✗ {}", crate::i18n::tf_lang(lang_en, "log_docker_failed", &[&e])), path.clone(), category.clone(), false));
                                    safety::log_deletion(&path, &category, false, Some(&e));
                                }
                            }
                            continue;
                        }

                        // OrbStack 清理 — 通过 orbctl 命令清理（同 Docker 原则：
                        // 绝不直接删容器数据文件，走官方 CLI）
                        if category == "OrbStack清理" {
                            match run_orbctl_clean(lang_en) {
                                Ok(msg) => {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("✓ {}", msg), path.clone(), category.clone(), true));
                                    safety::log_deletion(&path, &category, true, None);
                                }
                                Err(e) => {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("✗ {}", crate::i18n::tf_lang(lang_en, "log_docker_failed", &[&e])), path.clone(), category.clone(), false));
                                    safety::log_deletion(&path, &category, false, Some(&e));
                                }
                            }
                            continue;
                        }

                        // 普通文件/目录删除 - 尽力删除模式
                        let p = std::path::Path::new(path.as_str());

                        if !p.exists() && p.symlink_metadata().is_err() {
                            // 对齐：路径已不存在 → 记 SKIP，不计成功也不计失败。
                            // 之前记"✓ (已清理)"会让成功数虚高 —— 用户没做任何事，
                            // 只是缓存过期或外部已删除。
                            let _ = tx.send(DeleteMessage::Skip(
                                format!(
                                    "⏭️ {}",
                                    crate::i18n::tf_lang(
                                        lang_en,
                                        "log_skipped",
                                        &[&path, crate::i18n::t_lang(lang_en, "already_cleaned")]
                                    )
                                ),
                                path.clone(),
                                category.clone(),
                            ));
                            continue;
                        }

                        // 拒绝删除符号链接
                        if let Ok(meta) = p.symlink_metadata() {
                            if meta.file_type().is_symlink() {
                                let _ = tx.send(DeleteMessage::Log(
                                    format!("⛔ {}", crate::i18n::tf_lang(lang_en, "log_symlink_rejected", &[&path])), path.clone(), category.clone(), false));
                                safety::log_deletion(&path, &category, false, Some(crate::i18n::t_lang(lang_en, "log_symlink_rejected").replace("{}", "").trim()));
                                continue;
                            }
                        }

                        // 尽力删除：废纸篓模式或永久删除
                        // P0-3：use_trash 项在废纸篓失败时**不得**降级为永久删除，
                        // 也不得进入 sudo 重试列表（sudo rm -rf 会让它彻底不可恢复）
                        let deleted_ok = if use_trash {
                            move_to_trash(&path)
                        } else {
                            best_effort_delete_with_reason(p).is_ok()
                        };

                        if deleted_ok {
                            let action = crate::i18n::t_lang(lang_en, if use_trash { "log_action_trashed" } else { "log_action_deleted" });
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ {} [{}] {}", action, category, path), path.clone(), category.clone(), true));
                            safety::log_deletion(&path, &category, true, None);

                            // M-2：真删掉一项，就往清单里记一笔。
                            // restorable 由平台判定：macOS 废纸篓可搬回，
                            // Windows 回收站没有稳定路径，一律 false（那边靠还原点）。
                            let restorable = crate::backup::is_restorable_by_move(
                                use_trash,
                                std::env::consts::OS,
                            );
                            if let Ok(mut guard) = backup_entries.lock() {
                                guard.push(crate::backup::BackupEntry {
                                    path: path.clone(),
                                    size_bytes,
                                    category: category.clone(),
                                    restorable,
                                });
                            }
                        } else if use_trash {
                            // 废纸篓失败：保留文件 + 明确告知，不进 sudo 重试列表
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✗ {}", crate::i18n::tf_lang(lang_en, "log_trash_failed", &[&path])), path.clone(), category.clone(), false));
                            safety::log_deletion(&path, &category, false, Some(crate::i18n::t_lang(lang_en, "log_trash_failed").replace("{}", "").trim()));
                        } else {
                            // 普通删除失败，加入待 sudo 列表；先归因失败原因展示给用户
                            let failure = best_effort_delete_with_reason(p).err();
                            #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
                            let mut reason: String = failure
                                .map(|f| crate::i18n::t_lang(lang_en, f.label_key()).to_string())
                                .unwrap_or_else(|| crate::i18n::t_lang(lang_en, "fail_reason_other").to_string());
                            // P1：Windows 上被占用时，尽力查出占用进程名，提示用户关闭
                            #[cfg(target_os = "windows")]
                            if let Some(DeleteFailure::FileInUse) = failure {
                                let lockers = find_locking_processes(&path);
                                if !lockers.is_empty() {
                                    reason = format!("{}: {}", reason, lockers.join(", "));
                                }
                            }
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✗ {} — {}",
                                    crate::i18n::t_lang(lang_en, "log_still_exists"),
                                    reason
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            ));
                            crate::logger::warn(&format!(
                                "[删除] 失败: {} — {} [{}]",
                                path, reason, category
                            ));
                            failed_items.lock().unwrap_or_else(|e| e.into_inner()).push((path, category));
                        }
                    }
                });
            }
        });

        // 毒化保护：若某个 worker panic，Mutex 会被 poison，这里必须仍能取出数据，
        // 否则整条删除链路连锁 panic，UI 永久卡在 Deleting 态
        let failed_items: Vec<(String, String)> =
            failed_items.into_inner().unwrap_or_else(|e| e.into_inner());
        let backup_entries: Vec<crate::backup::BackupEntry> = backup_entries
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());
        let cancelled = cancelled.load(std::sync::atomic::Ordering::Relaxed);

        logger::info(&format!(
            "阶段1删除完成, 失败 {} 项{}",
            failed_items.len(),
            if cancelled {
                ", 用户中途取消"
            } else {
                ""
            }
        ));

        // M-2：落清单。写盘失败只记日志、绝不影响删除结果 ——
        // 清单是事后追溯手段，不能因为它存不进去就把已删掉的东西说成没删。
        //
        // 边界说清楚：这份清单只覆盖本函数删掉的项。需要提权的那批走
        // 另一条链路（sudo 阶段），不进这份清单；它们本来就是永久删除，
        // 不可还原，进不进清单不改变"能不能后悔"这个结论。
        if let Some(id) = crate::backup::record(backup_entries.clone()) {
            let restorable = backup_entries.iter().filter(|e| e.restorable).count();
            let _ = tx.send(DeleteMessage::BackupRecorded {
                id,
                restorable,
                total: backup_entries.len(),
            });
        }
        // P0-2：正式清单已落盘，清除待删快照（正常路径）。
        if let Some(id) = pending_id {
            crate::backup::finalize_pending(&id);
        }

        // 普通删除完成后，若还有失败项，通知 GUI 弹出 egui 内置密码输入框
        if !failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(format!(
                "🔐 {}",
                crate::i18n::tf_lang(lang_en, "log_need_sudo", &[&failed_items.len().to_string()])
            )));
            let _ = tx.send(DeleteMessage::NeedPassword(failed_items));
            return;
        }

        // P0-3：用户中途取消 —— 与正常完成区分，UI 据此提示"已停止，未处理项未动"
        if cancelled {
            let _ = tx.send(DeleteMessage::Cancelled);
            return;
        }

        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 创建名称不可预测、仅属主可访问的临时文件，返回 (路径, 文件句柄)
///
/// 安全说明（P0-2）：旧实现把即将被 `sudo` 以 root 执行的脚本写在固定路径
/// （`/tmp/maclean_sudo_delete.sh` 等）。本地任意进程可以预先占位，或在写入后
/// 替换内容 → 以 root 执行任意代码，是标准的本地提权路径。
///
/// 对策：
/// - 文件名含纳秒时间戳 + pid + 尝试序号，不可预测
/// - `O_CREAT|O_EXCL`：已存在就换名重试，不覆盖已有文件
/// - `O_NOFOLLOW`：目标是符号链接则失败，防止被导向别处
/// - 权限 0600，属主外不可读写
#[cfg(unix)]
pub fn create_private_temp_file(
    prefix: &str,
    ext: &str,
) -> Option<(std::path::PathBuf, std::fs::File)> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    for attempt in 0..64u64 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(attempt as u128);
        let mixed = nanos ^ (((std::process::id() as u128) << 40) ^ ((attempt as u128) << 100));
        let name = format!("{}_{:032x}.{}", prefix, mixed, ext);
        let path = std::env::temp_dir().join(name);

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(mut file) => {
                if file.flush().is_ok() {
                    return Some((path, file));
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => continue,
        }
    }
    None
}

/// 写入一个受保护的临时文本文件，返回路径（见 create_private_temp_file 安全说明）
#[cfg(unix)]
pub fn write_private_temp_file(
    prefix: &str,
    ext: &str,
    content: &str,
) -> Option<std::path::PathBuf> {
    use std::io::Write;
    let (path, mut file) = create_private_temp_file(prefix, ext)?;
    file.write_all(content.as_bytes()).ok()?;
    file.flush().ok()?;
    Some(path)
}

/// 创建名称不可预测、独占打开的临时文件（Windows 实现）
///
/// Windows 没有 unix 的 `O_NOFOLLOW` / `mode(0o600)`，这里用等价手段达到同样的
/// 保护强度（设计目标与 unix 版一致）：
/// - `create_new(true)` → `CREATE_NEW`：目标已存在（**包括**那里已有一个符号链接
///   或 junction 占位）就直接失败，既不覆盖也不跟随，等价于 `O_EXCL | O_NOFOLLOW`
/// - `share_mode(0)`：独占打开，同机其它进程无法再读写该文件，等价于 0600 的作用
/// - 文件名混入纳秒时间戳 + pid + 尝试序号，不可用"猜下一个名字"的方式占位
///
/// 这条防线是为了让随后的 UAC 提权删除脚本不会被本地其它进程预先占位 / 替换，
/// 否则就是标准的本地提权路径（参见 unix 版注释里的 P0-2 说明）。
#[cfg(target_os = "windows")]
pub fn create_private_temp_file(
    prefix: &str,
    ext: &str,
) -> Option<(std::path::PathBuf, std::fs::File)> {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;

    const SHARE_NONE: u32 = 0;

    for attempt in 0..64u64 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(attempt as u128);
        let mixed = nanos ^ (((std::process::id() as u128) << 40) ^ ((attempt as u128) << 100));
        let name = format!("{}_{:032x}.{}", prefix, mixed, ext);
        let path = std::env::temp_dir().join(name);

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(SHARE_NONE)
            .open(&path)
        {
            Ok(mut file) => {
                if file.flush().is_ok() {
                    return Some((path, file));
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => continue,
        }
    }
    None
}

/// 写入一个受保护的临时文本文件，返回路径（见 create_private_temp_file 安全说明）
#[cfg(target_os = "windows")]
pub fn write_private_temp_file(
    prefix: &str,
    ext: &str,
    content: &str,
) -> Option<std::path::PathBuf> {
    use std::io::Write;
    let (path, mut file) = create_private_temp_file(prefix, ext)?;
    file.write_all(content.as_bytes()).ok()?;
    file.flush().ok()?;
    Some(path)
}

/// **不可逆删除之前的二次安全校验**（P0-1）
///
/// 名字里不再写 "sudo"：GUI 的提权删除和 CLI 的 `clean` 都要过这一层，
/// 语义是「执行删除的那一刻之前重做校验」，跟要不要提权无关。
///
/// 为什么必须复做：扫描时做过 `safety` 校验，但到真正删除之间有时间差，
/// 路径可能已被替换成别的东西（TOCTOU），也可能被换成了符号链接。
/// 这一层的删除一旦放行不可恢复，所以必须逐项重做。
/// 待删除项：(路径, 分类)
pub type DeleteItem = (String, String);
/// 被拦截项：(路径, 分类, 拦截原因)
pub type RejectedItem = (String, String, String);

/// 返回：`(允许放行的项, 被拦截的项及原因)`
pub fn sanitize_before_delete(
    items: Vec<DeleteItem>,
    lang_en: bool,
) -> (Vec<DeleteItem>, Vec<RejectedItem>) {
    let mut allowed: Vec<DeleteItem> = Vec::with_capacity(items.len());
    let mut rejected: Vec<RejectedItem> = Vec::new();

    for (path, category) in items {
        // 0. 受保护根本身（P0 兜底）：待删路径等于 /、home、系统根、卷根等 → 拒绝。
        //    与黑名单不同，这是 fail-closed 的最后一道闸 —— 规则根解析若退化
        //    到这里（如 $HOME 变量为空导致 "$HOME/Library" 变成 "/Library"），
        //    此处直接拦截，绝不把根目录交给删除流程。
        if safety::is_protected_root(&path) {
            rejected.push((
                path,
                category,
                crate::i18n::t_lang(lang_en, "log_protected_root_rejected")
                    .replace("{}", "")
                    .trim()
                    .to_string(),
            ));
            continue;
        }

        // 1. 重做安全校验（与阶段一相同规则）
        match safety::check_path_safety_with_category(&path, &category) {
            safety::SafetyCheck::Danger(reason) | safety::SafetyCheck::Warning(reason) => {
                rejected.push((path, category, reason));
                continue;
            }
            safety::SafetyCheck::Safe => {}
        }

        // 2. 符号链接复查：阶段一之后路径可能已被替换成软链，
        //    对软链执行 rm -rf 可能顺着链接删到链接目标之外的内容
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if meta.file_type().is_symlink() {
                rejected.push((
                    path,
                    category,
                    crate::i18n::t_lang(lang_en, "log_sudo_symlink_rejected")
                        .replace("{}", "")
                        .trim()
                        .to_string(),
                ));
                continue;
            }
        }

        // 3. 拒绝含危险字符的路径：sudo 脚本用单引号包裹参数，
        //    换行 / 反引号 / $( 都可能突破引号造成命令注入
        if path.contains('\n') || path.contains('\r') || path.contains('`') || path.contains("$(") {
            rejected.push((
                path,
                category,
                crate::i18n::t_lang(lang_en, "log_sudo_unsafe_path")
                    .replace("{}", "")
                    .trim()
                    .to_string(),
            ));
            continue;
        }

        allowed.push((path, category));
    }

    (allowed, rejected)
}

/// 启动后台 sudo 删除线程
/// 使用 egui 内置输入框收集到的密码，通过 sudo -S 的 stdin 传入，
/// 避免调用 System Events / osascript 触发钥匙串弹窗。
///
/// macOS 专属：`sudo -S` + `/bin/bash` 脚本 + `xcrun simctl` 三者共同构成这条链路，
/// 离开 macOS 都不成立。Windows 的实现走 UAC 提权，见下方同名函数。
#[cfg(target_os = "macos")]
pub fn start_sudo_delete(
    failed_items: Vec<(String, String)>,
    password: String,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
    delete_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    if failed_items.is_empty() {
        return;
    }

    // P0-1：sudo 以 root 执行 rm -rf，放行前必须重做安全校验
    let (failed_items, rejected) = sanitize_before_delete(failed_items, lang_en);

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        // P0-3：sudo 阶段同样尊重「停止」——已置取消标志则不再提权删除，
        // 失败项原地保留（后续可手动处理）。加上脚本内每项 20s 超时
        // （perl alarm），即使某个路径 IO 卡死也不会无限挂起。
        if delete_cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = tx.send(DeleteMessage::Info(
                crate::i18n::t_lang(lang_en, "log_delete_cancelled").to_string(),
            ));
            let _ = tx.send(DeleteMessage::Done);
            return;
        }
        // 先播报被安全校验拦截的项，避免用户以为软件没干活
        for (path, category, reason) in &rejected {
            let _ = tx.send(DeleteMessage::Log(
                format!(
                    "⛔ {}",
                    crate::i18n::tf_lang(lang_en, "log_sudo_rejected", &[path, reason])
                ),
                path.clone(),
                category.clone(),
                false,
            ));
            safety::log_deletion(path, category, false, Some(reason));
        }

        if failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(
                crate::i18n::t_lang(lang_en, "log_sudo_phase").to_string(),
            ));
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(format!(
            "🔐 {}",
            crate::i18n::t_lang(lang_en, "log_sudo_phase")
        )));

        // 日志文件也用受保护的随机名（避免被符号链接导向用户目录）
        let sudo_debug_log: Option<std::path::PathBuf> =
            write_private_temp_file("maclean_sudo_dbg", "log", "");
        let mut debug_entries: Vec<String> = Vec::new();
        debug_entries.push(format!(
            "[sudo phase] started, {} items",
            failed_items.len()
        ));
        for (p, c) in &failed_items {
            debug_entries.push(format!("  item: [{}] {}", c, p));
        }

        // ========== 预处理：模拟器镜像通过 sudo xcrun simctl runtime delete 删除 ==========
        let mut remaining_items: Vec<(String, String)> = Vec::new();
        for (path, category) in &failed_items {
            if category == "模拟器镜像" || category == "模拟器Cryptex" {
                // 用 sudo xcrun simctl runtime delete 删除本项归属的运行时
                // P1：只删归属于本项、且系统标记 deletable 的 runtime。
                // 旧脚本在 root 下遍历"所有" runtime 逐个删除，勾选一项会连锅端。
                let sim_uuids: Vec<String> = resolve_simulator_uuids(path)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|u| u.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
                    .collect();
                if sim_uuids.is_empty() {
                    // 没有可删目标：保留该项，交给后续 sudo rm 清理残留目录
                    remaining_items.push((path.clone(), category.clone()));
                    continue;
                }
                let uuid_list = sim_uuids
                    .iter()
                    .map(|u| format!("'{}'", u))
                    .collect::<Vec<_>>()
                    .join(" ");
                let script = format!(
                    r#"#!/bin/bash
set +e
count=0
fail=0
for uuid in {}; do
    xcrun simctl runtime delete "$uuid" 2>/dev/null
    rc=$?
    if [ $rc -eq 0 ]; then
        count=$((count + 1))
    else
        fail=$((fail + 1))
    fi
done
echo "xcrun_deleted:$count"
echo "xcrun_failed:$fail"
exit 0
"#,
                    uuid_list
                );
                // P0-2：固定路径脚本可被本地进程预先占位或替换 → sudo 以 root 执行任意内容。
                // 改用不可预测文件名 + O_EXCL + 0600；脚本交给 /bin/bash 执行，不需要 +x。
                let Some(xcrun_script) = write_private_temp_file("maclean_xcrun", "sh", &script)
                else {
                    crate::logger::error("无法安全创建临时脚本，跳过 xcrun 删除");
                    remaining_items.push((path.clone(), category.clone()));
                    continue;
                };

                let xcrun_result = std::process::Command::new("/usr/bin/sudo")
                    .arg("-S")
                    .arg("/bin/bash")
                    .arg(&xcrun_script)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        if let Some(mut stdin) = child.stdin.take() {
                            use std::io::Write;
                            let _ = writeln!(stdin, "{}", password);
                        }
                        child.wait_with_output()
                    });

                let mut xcrun_success = false;
                match &xcrun_result {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        debug_entries.push(format!("xcrun sudo stdout: {}", stdout));
                        debug_entries.push(format!("xcrun sudo stderr: {}", stderr));

                        // 检查是否密码错误
                        if stderr.contains("incorrect password") || stderr.contains("3 incorrect") {
                            let _ = tx.send(DeleteMessage::Info(format!(
                                "🔒 {}",
                                crate::i18n::t_lang(lang_en, "log_password_wrong")
                            )));
                            let _ = tx.send(DeleteMessage::NeedPassword(failed_items.clone()));
                            if let Some(p) = &sudo_debug_log {
                                let _ = std::fs::write(p, debug_entries.join("\n"));
                            }
                            let _ = std::fs::remove_file(&xcrun_script);
                            return;
                        }

                        // 检查 Volumes 目录是否已清空
                        let p = std::path::Path::new(path);
                        if !p.exists()
                            || std::fs::read_dir(p)
                                .map(|mut d| d.next().is_none())
                                .unwrap_or(true)
                        {
                            xcrun_success = true;
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✓ {}",
                                    crate::i18n::tf_lang(
                                        lang_en,
                                        "log_touchid_runtime_deleted_path",
                                        &[path]
                                    )
                                ),
                                path.clone(),
                                category.clone(),
                                true,
                            ));
                            safety::log_deletion(path, category, true, None);
                        }
                    }
                    Err(e) => {
                        debug_entries.push(format!("xcrun sudo error: {}", e));
                    }
                }

                let _ = std::fs::remove_file(&xcrun_script);

                if !xcrun_success {
                    // xcrun 删除后仍有残留，用 sudo rm -rf 清理
                    remaining_items.push((path.clone(), category.clone()));
                }
            } else {
                remaining_items.push((path.clone(), category.clone()));
            }
        }

        let failed_items = remaining_items;

        if failed_items.is_empty() {
            if let Some(p) = &sudo_debug_log {
                let _ = std::fs::write(p, debug_entries.join("\n"));
            }
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        // 写临时删除脚本：所有目录后台并行删除
        let current_user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "root".to_string());
        let mut script_content = String::from("#!/bin/bash\nset +e\n");
        script_content.push_str("workdir=$(/usr/bin/mktemp -d)\n");
        script_content.push_str("trap \"/bin/rm -rf \\\"$workdir\\\"\" EXIT\n\n");
        script_content.push_str("process_one() {\n");
        script_content.push_str("  local idx=\"$1\"\n");
        script_content.push_str("  local path=\"$2\"\n");
        script_content.push_str("  local out=\"$workdir/${idx}.out\"\n");
        script_content.push_str("  echo \">MACLEAN_BEGIN:$path\" > \"$out\"\n");
        script_content.push_str("  /usr/bin/perl -e \'alarm shift; exec @ARGV\' 10 /usr/bin/chflags -R nouchg \"$path\" 2>/dev/null\n");
        script_content.push_str("  /usr/sbin/chown -R '");
        script_content.push_str(&current_user.replace("'", "'\\''"));
        script_content.push_str(":staff' \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/chmod -R u+w \"$path\" 2>/dev/null\n");
        script_content.push_str("  /usr/bin/perl -e \'alarm shift; exec @ARGV\' 20 /bin/rm -rf \"$path\" >> \"$out\" 2>&1\n");
        script_content.push_str("  echo \">MACLEAN_EXIT:$path:$?\" >> \"$out\"\n");
        script_content.push_str("}\n\n");

        for (i, (path, _)) in failed_items.iter().enumerate() {
            let escaped = path.replace("'", "'\\''");
            script_content.push_str(&format!("process_one {} '{}' &\n", i, escaped));
        }
        script_content.push_str("\nwait\n");
        script_content
            .push_str("for f in \"$workdir\"/*.out; do [ -f \"$f\" ] && /bin/cat \"$f\"; done\n");
        script_content.push_str("exit 0\n");
        // P0-2：随机名 + O_EXCL + 0600，杜绝"本地进程替换脚本 → root 执行任意内容"
        let Some(tmp_script) = write_private_temp_file("maclean_sudo_del", "sh", &script_content)
        else {
            crate::logger::error("无法安全创建删除脚本，放弃 sudo 删除");
            for (path, category) in &failed_items {
                let _ = tx.send(DeleteMessage::Log(
                    format!(
                        "✗ {}",
                        crate::i18n::tf_lang(
                            lang_en,
                            "log_delete_failed",
                            &[path, "cannot create temp script securely"]
                        )
                    ),
                    path.clone(),
                    category.clone(),
                    false,
                ));
            }
            let _ = tx.send(DeleteMessage::Done);
            return;
        };

        debug_entries.push("--- delete script content ---".to_string());
        debug_entries.push(script_content.clone());

        // 使用 sudo -S，通过 stdin 传入密码，避免钥匙串弹窗
        let sudo_result = std::process::Command::new("/usr/bin/sudo")
            .arg("-S")
            .arg("/bin/bash")
            .arg(&tmp_script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = writeln!(stdin, "{}", password);
                }
                child.wait_with_output()
            });

        // 解析脚本输出，按路径收集错误信息
        let mut rm_stderr: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        match &sudo_result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                debug_entries.push(format!("sudo exit code: {:?}", output.status.code()));
                debug_entries.push(format!("sudo stdout:\n{}", stdout));
                debug_entries.push(format!("sudo stderr:\n{}", stderr));
                let mut current_path = String::new();
                for line in stdout.lines() {
                    if let Some(p) = line.strip_prefix(">MACLEAN_BEGIN:") {
                        current_path = p.to_string();
                    } else if let Some(_rest) = line.strip_prefix(">MACLEAN_EXIT:") {
                        current_path.clear();
                    } else if !line.is_empty() && !current_path.is_empty() {
                        rm_stderr
                            .entry(current_path.clone())
                            .or_default()
                            .push_str(line);
                        rm_stderr
                            .entry(current_path.clone())
                            .or_default()
                            .push('\n');
                    }
                }
            }
            Err(e) => {
                debug_entries.push(format!("sudo spawn error: {}", e));
            }
        }

        // 写入调试日志
        if let Some(p) = &sudo_debug_log {
            let _ = std::fs::write(p, debug_entries.join("\n"));
        }

        // 逐项验证删除结果
        match sudo_result {
            Ok(output) => {
                let stderr_all = String::from_utf8_lossy(&output.stderr);
                let sudo_failed = !output.status.success();
                let password_error = stderr_all.contains("sudo: 3 incorrect password attempts")
                    || stderr_all.contains("incorrect password");
                let user_cancelled = !password_error
                    && (stderr_all.contains("sudo: a password is required")
                        || stderr_all.contains("User canceled"));

                if password_error {
                    let _ = tx.send(DeleteMessage::Info(format!(
                        "🔒 {}",
                        crate::i18n::t_lang(lang_en, "log_password_wrong")
                    )));
                    // 密码错误时 sudo 不会执行任何删除，全部项都需要重试
                    let _ = tx.send(DeleteMessage::NeedPassword(failed_items));
                    let _ = std::fs::remove_file(&tmp_script);
                    return;
                }

                for (path, category) in &failed_items {
                    let p = std::path::Path::new(path.as_str());
                    if !p.exists() && p.symlink_metadata().is_err() {
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✓ {}",
                                crate::i18n::tf_lang(
                                    lang_en,
                                    "log_deleted_sudo",
                                    &[category, path]
                                )
                            ),
                            path.clone(),
                            category.clone(),
                            true,
                        ));
                        safety::log_deletion(path, category, true, None);
                        continue;
                    }

                    // 仍在：判断是 SIP 保护还是普通权限/占用问题
                    let err_text = rm_stderr
                        .get(path)
                        .map(|s| s.as_str())
                        .unwrap_or(&stderr_all);
                    let is_sip = err_text.contains("Operation not permitted")
                        || std::process::Command::new("/usr/bin/xattr")
                            .arg(path)
                            .output()
                            .map(|o| {
                                String::from_utf8_lossy(&o.stdout).contains("com.apple.provenance")
                            })
                            .unwrap_or(false)
                        || path.starts_with("/Library/Developer/CoreSimulator/Caches");

                    if is_sip {
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "🔒 {}",
                                crate::i18n::tf_lang(lang_en, "log_sip_protected", &[path])
                            ),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(
                            path,
                            category,
                            false,
                            Some(crate::i18n::t_lang(lang_en, "log_sip_reason")),
                        );
                    } else if user_cancelled {
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✗ {}",
                                crate::i18n::tf_lang(lang_en, "log_cancelled_auth", &[path])
                            ),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(
                            path,
                            category,
                            false,
                            Some(crate::i18n::t_lang(lang_en, "log_cancel_reason")),
                        );
                    } else if sudo_failed {
                        let detail = if stderr_all.is_empty() {
                            crate::i18n::tf_lang(
                                lang_en,
                                "log_exit_code",
                                &[&output.status.code().unwrap_or(-1).to_string()],
                            )
                        } else {
                            stderr_all.trim().to_string()
                        };
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✗ {}",
                                crate::i18n::tf_lang(
                                    lang_en,
                                    "log_delete_failed",
                                    &[path, &detail]
                                )
                            ),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(path, category, false, Some(&detail));
                    } else {
                        let detail = if err_text.is_empty() {
                            crate::i18n::t_lang(lang_en, "log_still_exists").to_string()
                        } else {
                            err_text.trim().to_string()
                        };
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✗ {}",
                                crate::i18n::tf_lang(
                                    lang_en,
                                    "log_delete_failed",
                                    &[path, &detail]
                                )
                            ),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(path, category, false, Some(&detail));
                    }
                }
            }
            Err(e) => {
                for (path, category) in &failed_items {
                    crate::logger::warn(&format!("[sudo删除] 无法启动 sudo: {} — {}", path, e));
                    let _ = tx.send(DeleteMessage::Log(
                        format!(
                            "✗ {}",
                            crate::i18n::tf_lang(
                                lang_en,
                                "log_cannot_start_sudo",
                                &[path, &e.to_string()]
                            )
                        ),
                        path.clone(),
                        category.clone(),
                        false,
                    ));
                    safety::log_deletion(path, category, false, Some(&e.to_string()));
                }
            }
        }

        // 清理临时脚本
        let _ = std::fs::remove_file(&tmp_script);

        // 如果全部失败项仍在且不是因为密码错误，正常结束
        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 使用 Touch ID 的 sudo 删除（不需要密码，sudo 自动触发 Touch ID）
/// 启动 Touch ID 提权的删除线程
///
/// 返回是否真的拉起了线程 —— 调用方据此决定要不要保留 `delete_rx`。
/// （空 `failed_items` 时直接返回 false，不碰 `delete_rx`。）
///
/// macOS 专属：依赖 pam_tid / sudo_local 这套 Touch ID 提权机制。
#[cfg(target_os = "macos")]
pub fn start_sudo_delete_touchid(
    failed_items: Vec<(String, String)>,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
    delete_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> bool {
    if failed_items.is_empty() {
        return false;
    }

    // P0-1：与密码路径同样，放行前必须重做安全校验
    let (failed_items, rejected) = sanitize_before_delete(failed_items, lang_en);

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);
    let started = true;

    std::thread::spawn(move || {
        if delete_cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = tx.send(DeleteMessage::Info(
                crate::i18n::t_lang(lang_en, "log_delete_cancelled").to_string(),
            ));
            let _ = tx.send(DeleteMessage::Done);
            return;
        }
        for (path, category, reason) in &rejected {
            let _ = tx.send(DeleteMessage::Log(
                format!(
                    "⛔ {}",
                    crate::i18n::tf_lang(lang_en, "log_sudo_rejected", &[path, reason])
                ),
                path.clone(),
                category.clone(),
                false,
            ));
            safety::log_deletion(path, category, false, Some(reason));
        }

        if failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(
            crate::i18n::t_lang(lang_en, "log_touchid_verifying").to_string(),
        ));

        let sudo_debug_log: Option<std::path::PathBuf> =
            write_private_temp_file("maclean_sudo_tid_dbg", "log", "");
        let mut debug_entries: Vec<String> = Vec::new();
        debug_entries.push(format!(
            "[touchid sudo] started, {} items",
            failed_items.len()
        ));

        // 预处理：模拟器镜像通过 xcrun simctl runtime delete
        let mut remaining_items: Vec<(String, String)> = Vec::new();
        for (path, category) in &failed_items {
            if category == "模拟器镜像" || category == "模拟器Cryptex" {
                let _ = tx.send(DeleteMessage::Info(format!(
                    "🔄 {}",
                    crate::i18n::tf_lang(lang_en, "log_touchid_prepare_xcrun", &[category])
                )));

                // P1（对齐密码分支 1993+）：只删归属于本勾选项、且系统标记
                // deletable 的 runtime。旧脚本在 root 下遍历"全部" runtime 逐个
                // 删除 —— 勾一个 Cryptex 会连锅端掉全机模拟器运行时。
                let sim_uuids: Vec<String> = resolve_simulator_uuids(path)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|u| u.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
                    .collect();
                if sim_uuids.is_empty() {
                    // 没有可删目标：保留该项，交给后续 sudo rm 清理残留目录
                    remaining_items.push((path.clone(), category.clone()));
                    continue;
                }
                let uuid_list = sim_uuids
                    .iter()
                    .map(|u| format!("'{}'", u))
                    .collect::<Vec<_>>()
                    .join(" ");
                let script = format!(
                    r#"#!/bin/bash
set +e
count=0
fail=0
for uuid in {}; do
    xcrun simctl runtime delete "$uuid" 2>/dev/null
    rc=$?
    if [ $rc -eq 0 ]; then
        count=$((count + 1))
    else
        fail=$((fail + 1))
    fi
done
echo "xcrun_deleted:$count"
echo "xcrun_failed:$fail"
exit 0
"#,
                    uuid_list
                );
                // P0-2：固定路径脚本可被本地进程预先占位或替换 → sudo 以 root 执行任意内容。
                // 改用不可预测文件名 + O_EXCL + 0600；脚本交给 /bin/bash 执行，不需要 +x。
                let Some(xcrun_script) = write_private_temp_file("maclean_xcrun", "sh", &script)
                else {
                    crate::logger::error("无法安全创建临时脚本，跳过 xcrun 删除");
                    remaining_items.push((path.clone(), category.clone()));
                    continue;
                };

                // 先清除 sudo 票据，确保能触发 Touch ID
                let _ = std::process::Command::new("/usr/bin/sudo")
                    .arg("-k")
                    .output();

                let _ = tx.send(DeleteMessage::Info(format!(
                    "⏳ {}",
                    crate::i18n::t_lang(lang_en, "log_wait_touchid")
                )));

                // P0-2：随机名 + O_EXCL + 0600，避免日志被符号链接劫持
                let (xcrun_log, xcrun_log_file) =
                    match create_private_temp_file("maclean_xcrun_tid", "log") {
                        Some(v) => v,
                        None => {
                            remaining_items.push((path.clone(), category.clone()));
                            continue;
                        }
                    };
                let xcrun_log_file = Some(xcrun_log_file);

                // 不用 -S，sudo 会自动弹出 Touch ID；stdout 重定向到文件避免 pipe 死锁
                let mut cmd = std::process::Command::new("/usr/bin/sudo");
                cmd.arg("/bin/bash").arg(&xcrun_script);
                if let Some(file) = xcrun_log_file {
                    cmd.stdout(file);
                }
                let xcrun_result = cmd.status();

                let mut xcrun_success = false;
                let xcrun_stdout = std::fs::read_to_string(&xcrun_log).unwrap_or_default();
                debug_entries.push(format!("xcrun touchid stdout:\n{}", xcrun_stdout));

                let _ = tx.send(DeleteMessage::Info(format!(
                    "✅ {}",
                    crate::i18n::tf_lang(
                        lang_en,
                        "log_xcrun_done",
                        &[&xcrun_stdout.trim().replace('\n', " ")]
                    )
                )));

                match &xcrun_result {
                    Ok(status) => {
                        if !status.success() {
                            // Touch ID 可能被取消
                            if xcrun_stdout.contains("canceled")
                                || xcrun_stdout.contains("cancelled")
                            {
                                let _ = tx.send(DeleteMessage::Info(format!(
                                    "🔒 {}",
                                    crate::i18n::t_lang(lang_en, "log_touchid_cancel")
                                )));
                                for (p, c) in &failed_items {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!(
                                            "✗ {}: {}",
                                            crate::i18n::t_lang(lang_en, "log_touchid_cancel"),
                                            p
                                        ),
                                        p.clone(),
                                        c.clone(),
                                        false,
                                    ));
                                    safety::log_deletion(
                                        p,
                                        c,
                                        false,
                                        Some(crate::i18n::t_lang(lang_en, "log_touchid_cancel")),
                                    );
                                }
                                if let Some(p) = &sudo_debug_log {
                                    let _ = std::fs::write(p, debug_entries.join("\n"));
                                }
                                let _ = std::fs::remove_file(&xcrun_script);
                                let _ = std::fs::remove_file(&xcrun_log);
                                let _ = tx.send(DeleteMessage::Done);
                                return;
                            }
                        }

                        // xcrun 删除成功即可认为该模拟器镜像项已清理
                        // 不必要求 Volumes 目录为空（mount point 会残留，且受 SIP 保护）
                        let deleted_count = xcrun_stdout
                            .lines()
                            .find(|l| l.starts_with("xcrun_deleted:"))
                            .and_then(|l| l.strip_prefix("xcrun_deleted:"))
                            .and_then(|n| n.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        let failed_count = xcrun_stdout
                            .lines()
                            .find(|l| l.starts_with("xcrun_failed:"))
                            .and_then(|l| l.strip_prefix("xcrun_failed:"))
                            .and_then(|n| n.trim().parse::<usize>().ok())
                            .unwrap_or(0);

                        if deleted_count > 0 && failed_count == 0 {
                            xcrun_success = true;
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✓ {}",
                                    crate::i18n::tf_lang(
                                        lang_en,
                                        "log_touchid_xcrun_deleted",
                                        &[&deleted_count.to_string()]
                                    )
                                ),
                                path.clone(),
                                category.clone(),
                                true,
                            ));
                            safety::log_deletion(path, category, true, None);
                        }
                    }
                    Err(e) => {
                        debug_entries.push(format!("xcrun touchid error: {}", e));
                    }
                }

                let _ = std::fs::remove_file(&xcrun_script);
                let _ = std::fs::remove_file(&xcrun_log);

                if !xcrun_success {
                    remaining_items.push((path.clone(), category.clone()));
                }
            } else {
                remaining_items.push((path.clone(), category.clone()));
            }
        }

        let failed_items = remaining_items;

        if failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(format!(
                "✅ {}",
                crate::i18n::t_lang(lang_en, "log_no_sudo_needed")
            )));
            if let Some(p) = &sudo_debug_log {
                let _ = std::fs::write(p, debug_entries.join("\n"));
            }
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(format!(
            "🔄 {}",
            crate::i18n::tf_lang(
                lang_en,
                "log_sudo_execute",
                &[&failed_items.len().to_string()]
            )
        )));

        // 写临时删除脚本：并行删除
        let current_user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "root".to_string());
        let mut script_content = String::from("#!/bin/bash\nset +e\n");
        script_content.push_str("workdir=$(/usr/bin/mktemp -d)\n");
        script_content.push_str("trap \"/bin/rm -rf \\\"$workdir\\\"\" EXIT\n\n");
        script_content.push_str("process_one() {\n");
        script_content.push_str("  local idx=\"$1\"\n");
        script_content.push_str("  local path=\"$2\"\n");
        script_content.push_str("  local out=\"$workdir/${idx}.out\"\n");
        script_content.push_str("  echo \">MACLEAN_BEGIN:$path\" > \"$out\"\n");
        script_content.push_str("  /usr/bin/perl -e \'alarm shift; exec @ARGV\' 10 /usr/bin/chflags -R nouchg \"$path\" 2>/dev/null\n");
        script_content.push_str("  /usr/sbin/chown -R '");
        script_content.push_str(&current_user.replace("'", "'\\''"));
        script_content.push_str(":staff' \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/chmod -R u+w \"$path\" 2>/dev/null\n");
        script_content.push_str("  /usr/bin/perl -e \'alarm shift; exec @ARGV\' 20 /bin/rm -rf \"$path\" >> \"$out\" 2>&1\n");
        script_content.push_str("  echo \">MACLEAN_EXIT:$path:$?\" >> \"$out\"\n");
        script_content.push_str("}\n\n");

        for (i, (path, _)) in failed_items.iter().enumerate() {
            let escaped = path.replace("'", "'\\''");
            script_content.push_str(&format!("process_one {} '{}' &\n", i, escaped));
        }
        script_content.push_str("\nwait\n");
        script_content
            .push_str("for f in \"$workdir\"/*.out; do [ -f \"$f\" ] && /bin/cat \"$f\"; done\n");
        script_content.push_str("exit 0\n");
        // P0-2：随机名 + O_EXCL + 0600
        let Some(tmp_script) = write_private_temp_file("maclean_sudo_tid", "sh", &script_content)
        else {
            crate::logger::error("无法安全创建删除脚本，放弃 Touch ID 删除");
            let _ = tx.send(DeleteMessage::Done);
            return;
        };
        debug_entries.push(format!("delete script: {}", tmp_script.display()));

        // 日志文件同样用受保护的随机名，避免被符号链接导向别处
        let Some((sudo_log, sudo_log_file)) =
            create_private_temp_file("maclean_sudo_tid_out", "log")
        else {
            crate::logger::error("无法安全创建日志文件，放弃 Touch ID 删除");
            let _ = tx.send(DeleteMessage::Done);
            return;
        };
        let sudo_log_file = Some(sudo_log_file);

        let _ = tx.send(DeleteMessage::Info(format!(
            "⏳ {}",
            crate::i18n::t_lang(lang_en, "log_wait_touchid")
        )));

        // 不用 -S，sudo 自动触发 Touch ID；stdout 重定向到文件避免 pipe 死锁
        let mut sudo_cmd = std::process::Command::new("/usr/bin/sudo");
        sudo_cmd.arg("/bin/bash").arg(&tmp_script);
        if let Some(file) = sudo_log_file {
            sudo_cmd.stdout(file);
        }
        let sudo_result = sudo_cmd.status();

        let _ = tx.send(DeleteMessage::Info(format!(
            "✅ {}",
            crate::i18n::t_lang(lang_en, "log_sudo_done2")
        )));

        // 解析输出
        let sudo_stdout = std::fs::read_to_string(&sudo_log).unwrap_or_default();
        debug_entries.push(format!(
            "sudo touchid exit code: {:?}",
            sudo_result.as_ref().ok().and_then(|s| s.code())
        ));
        debug_entries.push(format!("sudo touchid stdout:\n{}", sudo_stdout));

        let mut rm_stderr: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut current_path = String::new();
        for line in sudo_stdout.lines() {
            if let Some(p) = line.strip_prefix(">MACLEAN_BEGIN:") {
                current_path = p.to_string();
            } else if let Some(_rest) = line.strip_prefix(">MACLEAN_EXIT:") {
                current_path.clear();
            } else if !line.is_empty() && !current_path.is_empty() {
                rm_stderr
                    .entry(current_path.clone())
                    .or_default()
                    .push_str(line);
                rm_stderr
                    .entry(current_path.clone())
                    .or_default()
                    .push('\n');
            }
        }

        if let Some(p) = &sudo_debug_log {
            let _ = std::fs::write(p, debug_entries.join("\n"));
        }
        let _ = std::fs::remove_file(&sudo_log);

        // 逐项验证
        match sudo_result {
            Ok(_) => {
                let user_cancelled = sudo_stdout.contains("canceled")
                    || sudo_stdout.contains("cancelled")
                    || sudo_stdout.contains("User canceled");

                if user_cancelled {
                    let _ = tx.send(DeleteMessage::Info(format!(
                        "🔒 {}",
                        crate::i18n::t_lang(lang_en, "log_touchid_cancel")
                    )));
                    for (path, category) in &failed_items {
                        crate::logger::warn(&format!("[sudo删除] Touch ID 授权取消: {}", path));
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✗ {}: {}",
                                crate::i18n::t_lang(lang_en, "log_touchid_cancel"),
                                path
                            ),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(
                            path,
                            category,
                            false,
                            Some(crate::i18n::t_lang(lang_en, "log_touchid_cancel")),
                        );
                    }
                    let _ = std::fs::remove_file(&tmp_script);
                    let _ = tx.send(DeleteMessage::Done);
                    return;
                }

                for (path, category) in &failed_items {
                    let p = std::path::Path::new(path.as_str());
                    if !p.exists() && p.symlink_metadata().is_err() {
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✓ {}",
                                crate::i18n::tf_lang(
                                    lang_en,
                                    "log_deleted_touchid",
                                    &[category, path]
                                )
                            ),
                            path.clone(),
                            category.clone(),
                            true,
                        ));
                        safety::log_deletion(path, category, true, None);
                    } else {
                        let err_text = rm_stderr.get(path).map(|s| s.as_str()).unwrap_or("");
                        let err_permitted = err_text.contains("Operation not permitted");
                        let is_core_sim =
                            path.starts_with("/Library/Developer/CoreSimulator/Caches");
                        // ACL deny 规则保护的路径：任何权限（含 root/Touch ID）都无法删除，
                        // 单独归类，避免误判为 SIP 或误导用户反复授权。
                        let is_acl = err_permitted && crate::safety::path_is_acl_protected(path);
                        let is_sip = is_core_sim || (err_permitted && !is_acl);

                        if is_sip {
                            crate::logger::warn(&format!("[sudo删除] SIP 保护: {}", path));
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "🔒 {}",
                                    crate::i18n::tf_lang(lang_en, "log_sip_protected", &[path])
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            ));
                            safety::log_deletion(
                                path,
                                category,
                                false,
                                Some(crate::i18n::t_lang(lang_en, "log_sip_reason")),
                            );
                        } else if is_acl {
                            crate::logger::warn(&format!("[sudo删除] ACL 保护: {}", path));
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "🔒 {}",
                                    crate::i18n::tf_lang(lang_en, "log_acl_protected", &[path])
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            ));
                            safety::log_deletion(
                                path,
                                category,
                                false,
                                Some(crate::i18n::t_lang(lang_en, "log_acl_protected")),
                            );
                        } else {
                            let detail = if err_text.is_empty() {
                                crate::i18n::t_lang(lang_en, "log_still_exists").to_string()
                            } else {
                                err_text.trim().to_string()
                            };
                            crate::logger::warn(&format!("[sudo删除] 失败: {} — {}", path, detail));
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✗ {}",
                                    crate::i18n::tf_lang(
                                        lang_en,
                                        "log_delete_failed",
                                        &[path, &detail]
                                    )
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            ));
                            safety::log_deletion(path, category, false, Some(&detail));
                        }
                    }
                }
            }
            Err(e) => {
                for (path, category) in &failed_items {
                    crate::logger::warn(&format!("[sudo删除] 无法启动 sudo: {} — {}", path, e));
                    let _ = tx.send(DeleteMessage::Log(
                        format!(
                            "✗ {}",
                            crate::i18n::tf_lang(
                                lang_en,
                                "log_cannot_start_sudo",
                                &[path, &e.to_string()]
                            )
                        ),
                        path.clone(),
                        category.clone(),
                        false,
                    ));
                    safety::log_deletion(path, category, false, Some(&e.to_string()));
                }
            }
        }

        let _ = std::fs::remove_file(&tmp_script);
        let _ = tx.send(DeleteMessage::Done);
    });

    started
}

/// 跨平台在默认浏览器打开 URL
pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// 在后台线程执行系统优化任务，结果通过 channel 回传。
///
/// 原实现（`execute_optimize_task` 被 UI 主线程直接调用）会同步执行
/// `chmod -R ~/Library`（user_permissions_repair）、`diskutil verifyVolume /`
/// （startup_disk_verify）等耗时数分钟的命令，`.output()` 阻塞 egui 事件循环，
/// 界面完全冻结、无法点取消。改为 spawn 线程 + mpsc 回传结果，
/// GUI 每帧 poll 收结果写日志。
pub fn start_optimize_task(task_name: String, lang_en: bool) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = execute_optimize_task(&task_name, lang_en);
        let _ = tx.send(result);
    });
    rx
}

/// 执行单个优化任务
pub fn execute_optimize_task(task_name: &str, lang_en: bool) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Windows 平台：路由到 Windows 专属实现
    #[cfg(target_os = "windows")]
    let result = execute_windows_optimize_task(task_name, lang_en);

    // macOS 及其他平台：原有 macOS 实现
    #[cfg(not(target_os = "windows"))]
    let result = execute_macos_optimize_task(task_name, lang_en);

    format!("[{}] {}", timestamp, result)
}

/// macOS 优化任务执行
#[cfg(not(target_os = "windows"))]
pub fn execute_macos_optimize_task(task_name: &str, lang_en: bool) -> String {
    match task_name {
        "dns_cache_flush" => {
            // dscacheutil 和 killall 在现代 macOS 上需要 sudo
            let r1 = std::process::Command::new("dscacheutil")
                .arg("-flushcache")
                .output();
            let r2 = std::process::Command::new("killall")
                .arg("-HUP")
                .arg("mDNSResponder")
                .output();
            let success = r1.map(|o| o.status.success()).unwrap_or(false)
                && r2.map(|o| o.status.success()).unwrap_or(false);
            if success {
                crate::i18n::t_lang(lang_en, "opt_dns_success").to_string()
            } else {
                crate::i18n::t_lang(lang_en, "opt_dns_fail").to_string()
            }
        }
        "quicklook_rebuild" => {
            let r = std::process::Command::new("qlmanage")
                .arg("-r")
                .arg("cache")
                .output();
            match r {
                Ok(_) => crate::i18n::t_lang(lang_en, "opt_quicklook_success").to_string(),
                Err(_) => crate::i18n::t_lang(lang_en, "opt_quicklook_fail").to_string(),
            }
        }
        "launchservices_rebuild" => {
            // lsregister 路径在 macOS 10.0-15 上一致，但加 fallback 更稳健
            let lsregister_candidates = [
                "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister",
                "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
            ];
            let lsregister = lsregister_candidates
                .iter()
                .find(|p| std::path::Path::new(p).exists())
                .unwrap_or(&lsregister_candidates[0]);
            let r = std::process::Command::new(lsregister).arg("-gc").output();
            match r {
                Ok(_) => crate::i18n::t_lang(lang_en, "opt_launchservices_success").to_string(),
                Err(_) => crate::i18n::t_lang(lang_en, "opt_launchservices_fail").to_string(),
            }
        }
        "saved_state_cleanup" => {
            let home = std::env::var("HOME").unwrap_or_default();
            let state_dir = format!("{}/Library/Saved Application State", home);
            let mut count = 0;
            if let Ok(entries) = std::fs::read_dir(&state_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_dir() {
                        // 检查修改时间是否超过 30 天
                        if let Ok(meta) = path.metadata() {
                            if let Ok(mtime) = meta.modified() {
                                if let Ok(age) = mtime.elapsed() {
                                    if age.as_secs() > 30 * 86400 {
                                        let _ = std::fs::remove_dir_all(&path);
                                        count += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            crate::i18n::tf_lang(lang_en, "opt_saved_state_success", &[&count.to_string()])
        }
        "gatekeeper_cleanup" => {
            let home = std::env::var("HOME").unwrap_or_default();
            let db_path = format!(
                "{}/Library/Preferences/com.apple.LaunchServices.QuarantineEventsV2",
                home
            );
            if std::path::Path::new(&db_path).exists() {
                let r = std::process::Command::new("sqlite3")
                    .arg(&db_path)
                    .arg("DELETE FROM LSQuarantineEvent; VACUUM;")
                    .output();
                match r {
                    Ok(_) => crate::i18n::t_lang(lang_en, "opt_gatekeeper_success").to_string(),
                    Err(_) => crate::i18n::t_lang(lang_en, "opt_gatekeeper_fail").to_string(),
                }
            } else {
                crate::i18n::t_lang(lang_en, "opt_gatekeeper_empty").to_string()
            }
        }
        "memory_pressure_release" => {
            // purge 在所有 macOS 版本上都需要 sudo
            let r = std::process::Command::new("purge").output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_memory_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_memory_fail").to_string(),
            }
        }
        "spotlight_reindex" => {
            // mdutil -E / 重建根卷的 Spotlight 索引
            let r = std::process::Command::new("mdutil")
                .arg("-E")
                .arg("/")
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_spotlight_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_spotlight_fail").to_string(),
            }
        }
        "login_items_audit" => {
            // 打开系统设置 > 通用 > 登录项
            // macOS 13+ 使用 "x-apple.systempreferences:com.apple.LoginItems-Settings.extension"
            // macOS 12 及以下使用 "com.apple.preference.users"
            let url = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";
            let r = std::process::Command::new("open").arg(url).output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_login_items_opened").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_login_items_fail").to_string(),
            }
        }
        // ---- 系统维护（P1，对标 MangoDisk system_maintenance）----
        "icon_cache_rebuild" => {
            // 删除图标缓存存储，Finder 自动重建；killall 刷新 Dock/Finder
            let home = std::env::var("HOME").unwrap_or_default();
            let icon_store = format!("{}/Library/Caches/com.apple.iconservices.store", home);
            let mut ok = true;
            if std::path::Path::new(&icon_store).exists() {
                ok = std::fs::remove_dir_all(&icon_store).is_ok();
            }
            let r = std::process::Command::new("killall").arg("Finder").output();
            let kill_ok = r.map(|o| o.status.success()).unwrap_or(false);
            if ok && kill_ok {
                crate::i18n::t_lang(lang_en, "opt_icon_cache_success").to_string()
            } else if ok {
                crate::i18n::t_lang(lang_en, "opt_icon_cache_partial").to_string()
            } else {
                crate::i18n::t_lang(lang_en, "opt_icon_cache_fail").to_string()
            }
        }
        "finder_service_restart" => {
            // 重启 Finder：刷新文件关联、侧边栏与扩展
            let r = std::process::Command::new("killall").arg("Finder").output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_finder_restart_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_finder_restart_fail").to_string(),
            }
        }
        "audio_service_restart" => {
            // 重启 coreaudiod：修复无声/音频卡顿
            // coreaudiod 是系统守护进程，普通权限下 killall 可能被拒绝，
            // 失败时如实提示，不引导用户手动执行命令。
            let r = std::process::Command::new("killall")
                .arg("coreaudiod")
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_audio_restart_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_audio_restart_fail").to_string(),
            }
        }
        "legacy_overrides_clean" => {
            // 备份并删除 LaunchServices 打开方式覆盖，cfprefsd 刷新后系统重建默认关联
            let home = std::env::var("HOME").unwrap_or_default();
            let plist = format!(
                "{}/Library/Preferences/com.apple.LaunchServices.plist",
                home
            );
            if !std::path::Path::new(&plist).exists() {
                crate::i18n::t_lang(lang_en, "opt_legacy_overrides_empty").to_string()
            } else {
                let backup = format!("{}.maclean.bak", plist);
                let backed_up = std::fs::copy(&plist, &backup).is_ok();
                let removed = std::fs::remove_file(&plist).is_ok();
                let _ = std::process::Command::new("killall")
                    .arg("cfprefsd")
                    .output();
                if backed_up && removed {
                    crate::i18n::t_lang(lang_en, "opt_legacy_overrides_success").to_string()
                } else if removed {
                    crate::i18n::t_lang(lang_en, "opt_legacy_overrides_no_backup").to_string()
                } else {
                    crate::i18n::t_lang(lang_en, "opt_legacy_overrides_fail").to_string()
                }
            }
        }
        "user_permissions_repair" => {
            // 修复用户 Library 目录权限（不涉及系统目录，无需 sudo）
            let home = std::env::var("HOME").unwrap_or_default();
            let lib = format!("{}/Library", home);
            let r = std::process::Command::new("chmod")
                .arg("-R")
                .arg("u+rwX")
                .arg(&lib)
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_permissions_repair_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_permissions_repair_fail").to_string(),
            }
        }
        "startup_disk_verify" => {
            // 只读校验启动盘卷，不修改数据
            let r = std::process::Command::new("diskutil")
                .arg("verifyVolume")
                .arg("/")
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_startup_disk_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_startup_disk_fail").to_string(),
            }
        }
        _ => crate::i18n::tf_lang(lang_en, "opt_unknown", &[task_name]),
    }
}

/// Windows 优化任务执行
///
/// 参考 Win11Debloat (https://github.com/Raphire/Win11Debloat) 实现。
/// 注册表操作统一通过 `reg.exe` 命令执行，避免引入 winreg 等额外依赖。
#[cfg(target_os = "windows")]
pub fn execute_windows_optimize_task(task_name: &str, lang_en: bool) -> String {
    // P1-2: 注册表修改类任务执行前自动备份相关键（reg export 到备份目录）
    let reg_keys: &[&str] = match task_name {
        "win_disable_telemetry" => &[r"HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection"],
        "win_disable_copilot" => &[r"HKCU\Software\Policies\Microsoft\Windows\WindowsCopilot"],
        "win_disable_suggestions" => &[
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager",
        ],
        "win_disable_fast_startup" => {
            &[r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Power"]
        }
        _ => &[],
    };
    for key in reg_keys {
        platform::windows_backup::backup_registry_key(key, task_name);
    }

    match task_name {
        "win_dns_flush" => {
            let r = std::process::Command::new("ipconfig")
                .arg("/flushdns")
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_win_dns_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_win_dns_fail").to_string(),
            }
        }
        "win_temp_cleanup" => {
            // 清理临时目录、缩略图缓存、交付优化缓存
            let mut total_cleaned = 0u64;

            // 1. 用户临时文件
            if let Ok(temp) = std::env::var("TEMP") {
                total_cleaned += clean_dir_size(&temp);
            }
            if let Ok(tmp) = std::env::var("TMP") {
                total_cleaned += clean_dir_size(&tmp);
            }
            // 2. Windows 临时文件
            total_cleaned += clean_dir_size(r"C:\Windows\Temp");
            // 3. 缩略图缓存
            if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
                let thumb_cache = format!(r"{}\Microsoft\Windows\Explorer", localappdata);
                total_cleaned += clean_dir_size(&thumb_cache);
            }
            // 4. 交付优化缓存
            let _ = std::process::Command::new("cleanmgr")
                .args(["/sagerun:1", "/d", "C:"])
                .output();

            crate::i18n::tf_lang(
                lang_en,
                "opt_win_temp_success",
                &[&crate::scanner::format_size(total_cleaned)],
            )
        }
        "win_disable_telemetry" => {
            // HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection AllowTelemetry = 0
            let r = std::process::Command::new("reg")
                .args([
                    "add",
                    r"HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection",
                    "/v",
                    "AllowTelemetry",
                    "/t",
                    "REG_DWORD",
                    "/d",
                    "0",
                    "/f",
                ])
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_win_telemetry_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_win_telemetry_fail").to_string(),
            }
        }
        "win_disable_copilot" => {
            // HKCU\Software\Policies\Microsoft\Windows\WindowsCopilot TurnOffWindowsCopilot = 1
            let r = std::process::Command::new("reg")
                .args([
                    "add",
                    r"HKCU\Software\Policies\Microsoft\Windows\WindowsCopilot",
                    "/v",
                    "TurnOffWindowsCopilot",
                    "/t",
                    "REG_DWORD",
                    "/d",
                    "1",
                    "/f",
                ])
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_win_copilot_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_win_copilot_fail").to_string(),
            }
        }
        "win_disable_suggestions" => {
            // 关闭开始菜单建议、锁屏广告、设置建议
            let keys = [
                (
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
                    "Start_IrisRecommendations",
                ),
                (
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager",
                    "SubscribedContent-338388Enabled",
                ),
                (
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager",
                    "SubscribedContent-338389Enabled",
                ),
                (
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager",
                    "SystemPaneSuggestionsEnabled",
                ),
            ];
            let mut success = true;
            for (key, value) in keys {
                let r = std::process::Command::new("reg")
                    .args(["add", key, "/v", value, "/t", "REG_DWORD", "/d", "0", "/f"])
                    .output();
                if !r.map(|o| o.status.success()).unwrap_or(false) {
                    success = false;
                }
            }
            if success {
                crate::i18n::t_lang(lang_en, "opt_win_suggestions_success").to_string()
            } else {
                crate::i18n::t_lang(lang_en, "opt_win_suggestions_fail").to_string()
            }
        }
        "win_startup_audit" => {
            // 打开任务管理器启动页
            let r = std::process::Command::new("taskmgr").arg("/4").spawn();
            match r {
                Ok(_) => crate::i18n::t_lang(lang_en, "opt_win_startup_opened").to_string(),
                Err(_) => crate::i18n::t_lang(lang_en, "opt_win_startup_fail").to_string(),
            }
        }
        "win_disable_fast_startup" => {
            // HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Power HiberbootEnabled = 0
            let r = std::process::Command::new("reg")
                .args([
                    "add",
                    r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Power",
                    "/v",
                    "HiberbootEnabled",
                    "/t",
                    "REG_DWORD",
                    "/d",
                    "0",
                    "/f",
                ])
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_win_faststartup_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_win_faststartup_fail").to_string(),
            }
        }
        "win_restore_point" => {
            // 手动触发：强制创建还原点（跳过 20h 频率限制）
            let (ok, _msg) = platform::windows_backup::ensure_restore_point(true);
            if ok {
                crate::i18n::t_lang(lang_en, "opt_win_restore_success").to_string()
            } else {
                crate::i18n::t_lang(lang_en, "opt_win_restore_fail").to_string()
            }
        }
        "win_restart_explorer" => {
            // taskkill /f /im explorer.exe && start explorer.exe
            let r1 = std::process::Command::new("taskkill")
                .args(["/f", "/im", "explorer.exe"])
                .output();
            let r2 = std::process::Command::new("explorer.exe").spawn();
            let success = r1.map(|o| o.status.success()).unwrap_or(false) && r2.is_ok();
            if success {
                crate::i18n::t_lang(lang_en, "opt_win_explorer_success").to_string()
            } else {
                crate::i18n::t_lang(lang_en, "opt_win_explorer_fail").to_string()
            }
        }
        "win_trim_drives" => {
            // 对系统盘执行 TRIM / 碎片整理
            let r = std::process::Command::new("defrag")
                .args(["C:", "/O", "/H"])
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    crate::i18n::t_lang(lang_en, "opt_win_trim_success").to_string()
                }
                _ => crate::i18n::t_lang(lang_en, "opt_win_trim_fail").to_string(),
            }
        }
        _ => crate::i18n::tf_lang(lang_en, "opt_unknown", &[task_name]),
    }
}

/// 计算并清空目录，返回清理的字节数
#[cfg(target_os = "windows")]
pub fn clean_dir_size(dir: &str) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if let Ok(meta) = path.metadata() {
                total += meta.len();
            }
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    total
}

// =========================================================================
//  Windows UAC 提权删除
// =========================================================================
//
//  macOS 侧用 `sudo -S` 复用系统授权；Windows 没有 sudo，正确做法是 UAC 提权
//  （`Start-Process -Verb RunAs`），由系统弹窗完成授权。
//
//  下面的 `escape_ps_single_quoted` / `build_uac_delete_script` 故意不加
//  `#[cfg(target_os = "windows")]`：它们是纯字符串处理，跨平台可编译，
//  因此能在开发机（macOS）上跑真正的单元测试。命令构造是这个链路里唯一
//  可确定性测试的部分，也是最容易被注入突破的部分，必须有测试兜。

/// PowerShell 单引号字符串内的转义：`'` → `''`
///
/// 删除脚本里的路径一律用单引号字符串字面量承载（单引号串不做变量展开、
/// 不解释反引号，语义最接近"原样传参"）。唯一能突破它的是路径里自带的
/// 单引号，所以只需要把 `'` 翻倍。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn escape_ps_single_quoted(s: &str) -> String {
    s.replace('\'', "''")
}

/// 生成 UAC 提权删除脚本的内容
///
/// `items` 为 (路径, 类别)，`result_path` 为脚本写回逐项结果的位置。
/// 脚本对每项输出一行 `OK\t<path>` 或 `FAIL\t<path>\t<原因>`，主进程据此上报。
///
/// 注意 `Remove-Item` 必须走 `-LiteralPath`：默认的 `-Path` 会把 `[` `]`
/// 当通配符解释，含方括号的目录（如 `foo[1]`）会被匹配错或直接报错。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn build_uac_delete_script(items: &[(String, String)], result_path: &str) -> String {
    let mut s = String::new();
    s.push_str("$ErrorActionPreference = 'Stop'\n");
    s.push_str("$lines = New-Object System.Collections.Generic.List[string]\n");
    s.push_str("$paths = @(\n");
    for (path, _category) in items {
        // 路径中含 `;` 等字符在单引号字面量里是安全的，只需转义单引号
        s.push_str(&format!("'{}'\n", escape_ps_single_quoted(path)));
    }
    s.push_str(")\n");
    s.push_str(
        r#"foreach ($p in $paths) {
    if ([string]::IsNullOrWhiteSpace($p)) { continue }
    try {
        if (Test-Path -LiteralPath $p) {
            Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction Stop
        }
        if (Test-Path -LiteralPath $p) {
            [void]$lines.Add("FAIL`t$p`tdelete reported no error but target still exists")
        } else {
            [void]$lines.Add("OK`t$p")
        }
    } catch {
        [void]$lines.Add("FAIL`t$p`t$($_.Exception.Message)")
    }
}
"#,
    );
    s.push_str(&format!(
        "[System.IO.File]::WriteAllLines('{}', $lines, [System.Text.UTF8Encoding]::new($false))\n",
        escape_ps_single_quoted(result_path)
    ));
    s
}

/// 启动 UAC 提权删除线程（Windows 实现）
///
/// 与 macOS 版 `start_sudo_delete` 同签名（方便 UI 层不分平台调用）：
/// 普通删除失败的项目走到这里，用管理员权限重试一次。
///
/// 安全约束（与 macOS 侧对齐）：
/// - 放行前用 `sanitize_before_delete` 逐项重做 safety 校验，防阶段间的 TOCTOU
/// - 脚本与结果文件都写在不可预测、独占打开的临时文件里（见各自的
///   `create_private_temp_file` 说明），避免被本地进程占位后以管理员执行
/// - 路径经单引号转义后嵌入脚本
///
/// `password` 参数在 Windows 上不使用：授权由系统 UAC 弹窗完成，
/// 自绘输入框收集密码既不安全也无处可用。这里显式消费掉避免误用。
#[cfg(target_os = "windows")]
pub fn start_sudo_delete(
    failed_items: Vec<(String, String)>,
    password: String,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
    delete_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    // Windows 不做命令行密码，交给 UAC
    let _ = password;

    if failed_items.is_empty() {
        return;
    }

    // P0-1：以管理员权限删除前必须重做安全校验
    let (failed_items, rejected) = sanitize_before_delete(failed_items, lang_en);

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        // P0-3：sudo 阶段同样尊重「停止」——已置取消标志则不再提权删除，
        // 失败项原地保留（后续可手动处理）。加上脚本内每项 20s 超时
        // （perl alarm），即使某个路径 IO 卡死也不会无限挂起。
        if delete_cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = tx.send(DeleteMessage::Info(
                crate::i18n::t_lang(lang_en, "log_delete_cancelled").to_string(),
            ));
            let _ = tx.send(DeleteMessage::Done);
            return;
        }
        // 先播报被安全校验拦截的项，避免用户以为软件没干活
        for (path, category, reason) in rejected {
            let line = crate::i18n::tf_lang(lang_en, "log_sudo_rejected", &[&path, &reason]);
            let _ = tx.send(DeleteMessage::Log(line, path, category, false));
        }

        if failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(
            crate::i18n::t_lang(lang_en, "log_sudo_phase").to_string(),
        ));

        // path -> category，结果文件里只有路径，需要回填类别来发日志
        let category_of: std::collections::HashMap<String, String> = failed_items
            .iter()
            .map(|(p, c)| (p.clone(), c.clone()))
            .collect();

        // 结果文件：脚本以管理员身份写回逐项结果
        let Some(result_path) = write_private_temp_file("maclean_uac_res", "log", "") else {
            crate::logger::error("无法安全创建结果文件，放弃 UAC 提权删除");
            let _ = tx.send(DeleteMessage::Done);
            return;
        };
        let result_path_str = result_path.to_string_lossy().to_string();

        let Some(script_path) = write_private_temp_file(
            "maclean_uac_del",
            "ps1",
            &build_uac_delete_script(&failed_items, &result_path_str),
        ) else {
            crate::logger::error("无法安全创建删除脚本，放弃 UAC 提权删除");
            let _ = std::fs::remove_file(&result_path);
            let _ = tx.send(DeleteMessage::Done);
            return;
        };
        let script_path_str = script_path.to_string_lossy().to_string();

        // Start-Process -Verb RunAs 触发 UAC；-Wait 保证本次删除跑完才继续。
        // 用户在 UAC 弹窗点"否"会抛异常 → catch 分支返回非零，据此区分"被拒绝"
        // 与"删除失败"，不要用同一句报错糊过去。
        let launcher = format!(
            "try {{ $null = Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File','{}') -Verb RunAs -Wait -PassThru; exit 0 }} catch {{ exit 1 }}",
            escape_ps_single_quoted(&script_path_str)
        );
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &launcher])
            .status();

        let uac_failed = match status {
            Ok(s) => !s.success(),
            Err(e) => {
                crate::logger::error(&format!("无法启动 UAC 提权进程: {}", e));
                true
            }
        };

        let Ok(results) = std::fs::read_to_string(&result_path) else {
            crate::logger::error("未能读回 UAC 删除结果");
            let _ = std::fs::remove_file(&script_path);
            let _ = std::fs::remove_file(&result_path);
            let _ = tx.send(DeleteMessage::Done);
            return;
        };

        let mut reported = 0usize;
        for raw_line in results.lines() {
            let line = raw_line.trim_end_matches(['\r', '\n']);
            if line.trim().is_empty() {
                continue;
            }
            let mut parts = line.splitn(3, '\t');
            let status = parts.next().unwrap_or("");
            let path = parts.next().unwrap_or("").to_string();
            if path.is_empty() {
                continue;
            }
            let category = category_of.get(&path).cloned().unwrap_or_default();
            let message = parts.next().unwrap_or("").to_string();
            reported += 1;
            match status {
                "OK" => {
                    // 复用 macOS 侧同一句话：Deleted [<类别>] <路径> (admin privileges)
                    let _ = tx.send(DeleteMessage::Log(
                        crate::i18n::tf_lang(lang_en, "log_deleted_sudo", &[&category, &path]),
                        path,
                        category,
                        true,
                    ));
                }
                _ => {
                    // log_delete_failed 有两个占位符：路径 + 失败原因
                    let reason = if message.is_empty() {
                        crate::i18n::t_lang(lang_en, "log_still_exists").to_string()
                    } else {
                        message
                    };
                    let _ = tx.send(DeleteMessage::Log(
                        crate::i18n::tf_lang(lang_en, "log_delete_failed", &[&path, &reason]),
                        path,
                        category,
                        false,
                    ));
                }
            }
        }

        // UAC 被拒绝时脚本根本没执行，结果文件是空的，这里要明确告诉用户
        if uac_failed && reported == 0 {
            let _ = tx.send(DeleteMessage::Info(
                crate::i18n::t_lang(lang_en, "sudo_cancelled_log").to_string(),
            ));
        }

        let _ = std::fs::remove_file(&script_path);
        let _ = std::fs::remove_file(&result_path);
        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 其它平台（Linux 等）的提权删除
///
/// 既没有 macOS 的 `sudo -S`，也没有 Windows 的 UAC，所谓"提权重试"无从谈起。
/// 这里退化为再删一次并**如实上报结果**：不做 sudo 幻想，也不把失败粉饰成成功。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn start_sudo_delete(
    failed_items: Vec<(String, String)>,
    password: String,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
    delete_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let _ = password;
    if failed_items.is_empty() {
        return;
    }

    let (failed_items, rejected) = sanitize_before_delete(failed_items, lang_en);

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        for (path, category, reason) in rejected {
            let line = crate::i18n::tf_lang(lang_en, "log_sudo_rejected", &[&path, &reason]);
            let _ = tx.send(DeleteMessage::Log(line, path, category, false));
        }

        for (path, category) in failed_items {
            let ok = best_effort_delete_with_reason(std::path::Path::new(&path)).is_ok();
            let line = if ok {
                crate::i18n::tf_lang(lang_en, "log_deleted_sudo", &[&category, &path])
            } else {
                crate::i18n::tf_lang(
                    lang_en,
                    "log_delete_failed",
                    &[&path, crate::i18n::t_lang(lang_en, "log_still_exists")],
                )
            };
            let _ = tx.send(DeleteMessage::Log(line, path, category, ok));
        }

        let _ = tx.send(DeleteMessage::Done);
    });
}

// =========================================================================
//  CLI 专用 Touch ID 提权删除
//
//  与 GUI 的 start_sudo_delete_touchid 走同一安全通道（随机名临时脚本 +
//  O_EXCL + 0600 + sudo 触发 Touch ID），但提供同步接口、返回结构化结果，
//  供 cmd_clean 在普通删除遇权限失败后自动重试。仅 macOS。
// =========================================================================

/// 提权删除结果：(成功项, 失败项及其原因)
#[cfg(target_os = "macos")]
pub type CliSudoResult = (Vec<(String, String)>, Vec<(String, String)>);

#[cfg(target_os = "macos")]
pub fn cli_sudo_delete_touchid(items: Vec<(String, String)>) -> CliSudoResult {
    if items.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // 提权前必须复做安全校验（与 GUI 同层 TOCTOU 防护）
    let (allowed, rejected) = sanitize_before_delete(items, false);
    let mut failed: Vec<(String, String)> = rejected
        .into_iter()
        .map(|(p, _c, r)| (p, format!("安全拦截: {}", r)))
        .collect();

    if allowed.is_empty() {
        return (Vec::new(), failed);
    }

    let current_user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "root".to_string());

    let mut script_content = String::from("#!/bin/bash\nset +e\n");
    script_content.push_str("workdir=$(/usr/bin/mktemp -d)\n");
    script_content.push_str("trap \"/bin/rm -rf \\\"$workdir\\\"\" EXIT\n\n");
    script_content.push_str("process_one() {\n");
    script_content.push_str("  local idx=\"$1\"\n");
    script_content.push_str("  local path=\"$2\"\n");
    script_content.push_str("  local out=\"$workdir/${idx}.out\"\n");
    script_content.push_str("  echo \">MACLEAN_BEGIN:$path\" > \"$out\"\n");
    script_content.push_str("  /usr/bin/perl -e \'alarm shift; exec @ARGV\' 10 /usr/bin/chflags -R nouchg \"$path\" 2>/dev/null\n");
    script_content.push_str("  /usr/sbin/chown -R '");
    script_content.push_str(&current_user.replace("'", "'\\''"));
    script_content.push_str(":staff' \"$path\" 2>/dev/null\n");
    script_content.push_str("  /bin/chmod -R u+w \"$path\" 2>/dev/null\n");
    script_content.push_str("  /usr/bin/perl -e \'alarm shift; exec @ARGV\' 20 /bin/rm -rf \"$path\" >> \"$out\" 2>&1\n");
    script_content.push_str("  echo \">MACLEAN_EXIT:$path:$?\" >> \"$out\"\n");
    script_content.push_str("}\n\n");

    for (i, (path, _)) in allowed.iter().enumerate() {
        let escaped = path.replace("'", "'\\''");
        script_content.push_str(&format!("process_one {} '{}' &\n", i, escaped));
    }
    script_content.push_str("\nwait\n");
    script_content
        .push_str("for f in \"$workdir\"/*.out; do [ -f \"$f\" ] && /bin/cat \"$f\"; done\n");
    script_content.push_str("exit 0\n");

    // P0-2：随机名 + O_EXCL + 0600，避免脚本被本地进程预置
    let Some(tmp_script) = write_private_temp_file("maclean_sudo_tid", "sh", &script_content)
    else {
        crate::logger::error("无法安全创建提权删除脚本，跳过 Touch ID 删除");
        failed.extend(
            allowed
                .into_iter()
                .map(|(p, _c)| (p, "无法创建提权脚本".to_string())),
        );
        return (Vec::new(), failed);
    };

    let Some((sudo_log, sudo_log_file)) = create_private_temp_file("maclean_sudo_tid_out", "log")
    else {
        crate::logger::error("无法安全创建提权日志文件，跳过 Touch ID 删除");
        let _ = std::fs::remove_file(&tmp_script);
        failed.extend(
            allowed
                .into_iter()
                .map(|(p, _c)| (p, "无法创建提权日志文件".to_string())),
        );
        return (Vec::new(), failed);
    };

    // 清票据，确保触发 Touch ID
    let _ = std::process::Command::new("/usr/bin/sudo")
        .arg("-k")
        .output();

    let mut sudo_cmd = std::process::Command::new("/usr/bin/sudo");
    sudo_cmd.arg("/bin/bash").arg(&tmp_script);
    let mut child = sudo_cmd
        .stdout(std::process::Stdio::from(sudo_log_file))
        .spawn();
    let _ = sudo_log_file;

    // 等待 sudo（Touch ID 弹窗可能让用户等几秒）
    let sudo_result = child.as_mut().map(|c| c.wait());

    let sudo_stdout = std::fs::read_to_string(&sudo_log).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp_script);
    let _ = std::fs::remove_file(&sudo_log);

    let mut ok: Vec<(String, String)> = Vec::new();

    match sudo_result {
        Ok(Ok(_)) => {
            let user_cancelled = sudo_stdout.contains("canceled")
                || sudo_stdout.contains("cancelled")
                || sudo_stdout.contains("User canceled");
            if user_cancelled {
                failed.extend(
                    allowed
                        .into_iter()
                        .map(|(p, _c)| (p, "Touch ID 授权已取消".to_string())),
                );
                return (ok, failed);
            }
            for (path, category) in allowed {
                let p = std::path::Path::new(path.as_str());
                if !p.exists() && p.symlink_metadata().is_err() {
                    ok.push((path, category));
                } else {
                    failed.push((path, "提权删除后仍存在（可能受 SIP 保护）".to_string()));
                }
            }
        }
        _ => {
            failed.extend(
                allowed
                    .into_iter()
                    .map(|(p, _c)| (p, "提权删除失败（sudo 未执行或已取消）".to_string())),
            );
        }
    }

    (ok, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 抖动修复：App 卸载页一次性出结果 ----------

    fn mk_item(path: &str, size: u64) -> ScanItem {
        ScanItem {
            path: path.to_string(),
            size_bytes: size,
            category: "App残留".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: crate::scanner::Recommend::Advanced,
            description: String::new(),
        }
    }

    #[test]
    fn send_items_in_batches_skips_incremental_for_app_uninstall() {
        // App 卸载页一次性出结果：tab_idx=5（AppUninstall）不得发任何增量消息
        let (tx, rx) = std::sync::mpsc::channel::<ScanMessage>();
        let items = vec![
            mk_item("/tmp/a", 100),
            mk_item("/tmp/b", 200),
            mk_item("/tmp/c", 50),
        ];
        send_items_in_batches(&tx, &items, 5);
        drop(tx);
        assert!(
            rx.try_recv().is_err(),
            "AppUninstall 不应发送增量消息（否则列表扫描尾部持续跳动）"
        );
    }

    #[test]
    fn send_items_in_batches_still_sends_incremental_for_other_tabs() {
        // 其它 Tab 保持原有增量行为（分批 + CurrentPath）
        let (tx, rx) = std::sync::mpsc::channel::<ScanMessage>();
        let items = vec![
            mk_item("/tmp/a", 100),
            mk_item("/tmp/b", 200),
            mk_item("/tmp/c", 50),
        ];
        send_items_in_batches(&tx, &items, 1);
        drop(tx);
        let mut got_partial = false;
        while let Ok(msg) = rx.try_recv() {
            if matches!(msg, ScanMessage::PartialItems(_, _)) {
                got_partial = true;
            }
        }
        assert!(got_partial, "其它 Tab 应发送增量消息");
    }

    // ---------- P1: 删除失败归因 ----------

    #[cfg(target_os = "macos")]
    #[test]
    fn delete_failure_label_keys_map_to_i18n() {
        // 每个原因都有对应文案 key（缺失会在 t_lang fallback 成空串）
        for f in [
            DeleteFailure::PermissionDenied,
            DeleteFailure::SipProtected,
            DeleteFailure::FileInUse,
            DeleteFailure::Immutable,
            DeleteFailure::Other,
        ] {
            let key = f.label_key();
            assert!(
                !crate::i18n::t_lang(true, key).is_empty(),
                "EN key {key} 缺失"
            );
            assert!(
                !crate::i18n::t_lang(false, key).is_empty(),
                "ZH key {key} 缺失"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rm_failure_classification_attributes_permission_and_sip() {
        // 用户目录普通路径 + operation not permitted → 权限不足
        assert_eq!(
            classify_rm_failure(b"rm: /Users/me/x: Operation not permitted", "/Users/me/x"),
            Some(DeleteFailure::PermissionDenied)
        );
        // 系统保护路径 + not permitted → SIP
        assert_eq!(
            classify_rm_failure(
                b"rm: /System/Library/y: Operation not permitted",
                "/System/Library/y"
            ),
            Some(DeleteFailure::SipProtected)
        );
        // 忙 → 占用
        assert_eq!(
            classify_rm_failure(b"rm: /Users/me/z: Resource busy", "/Users/me/z"),
            Some(DeleteFailure::FileInUse)
        );
        // 只读文件系统 → SIP
        assert_eq!(
            classify_rm_failure(b"rm: /x: Read-only file system", "/x"),
            Some(DeleteFailure::SipProtected)
        );
        // 空 stderr → 无法归类
        assert_eq!(classify_rm_failure(b"", "/Users/me/a"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn delete_attempt_attributes_gone_path_as_success() {
        // 待删路径已不存在 → 视为成功（尽力删除语义）
        let tmp = std::env::temp_dir().join(format!("maclean_p1_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(best_effort_delete_with_reason(&tmp).is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn delete_attempt_attributes_real_directory_deletion() {
        // 真实目录删除成功
        let tmp = std::env::temp_dir().join(format!("maclean_p1_real_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("创建临时目录");
        std::fs::write(tmp.join("a.txt"), b"x").expect("写文件");
        let result = best_effort_delete_with_reason(&tmp);
        assert!(result.is_ok(), "普通目录应删除成功: {:?}", result);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rename_to_staging_moves_and_can_restore() {
        // TOCTOU staging 行为：改名后原路径消失、staging 指向原对象；恢复 rename 可还原
        let tmp = std::env::temp_dir().join(format!("maclean_toctou_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let target = tmp.join("victim");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("f.txt"), b"payload").unwrap();

        // 改名到 staging
        let staging = rename_to_staging(&target).expect("rename_to_staging 应成功");
        assert!(!target.exists(), "原路径应已被原子移走");
        assert!(staging.exists(), "staging 应存在");
        assert_eq!(
            std::fs::read_to_string(staging.join("f.txt")).unwrap(),
            "payload",
            "staging 应指向原物理对象"
        );
        // staging 必须在同一目录（同卷，保证原子性）
        assert_eq!(
            staging.parent().unwrap(),
            tmp,
            "staging 必须在原目录下（同卷原子 rename）"
        );

        // 恢复
        std::fs::rename(&staging, &target).unwrap();
        assert!(target.exists(), "恢复后原路径应存在");
        assert!(!staging.exists(), "staging 应清空");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ps_single_quote_is_doubled() {
        assert_eq!(escape_ps_single_quoted("plain"), "plain");
        assert_eq!(escape_ps_single_quoted("Bob's App"), "Bob''s App");
        assert_eq!(escape_ps_single_quoted("a'b'c"), "a''b''c");
    }

    #[test]
    fn ps_escape_neutralises_quote_breakout() {
        // 恶意路径试图提前闭合单引号再拼一条命令出去
        let evil = "C:\\x'; Remove-Item -LiteralPath C:\\Windows -Recurse -Force; echo '";
        let escaped = escape_ps_single_quoted(evil);
        // 转义后不应再出现"单个单引号跟着命令"的结构
        assert!(
            !escaped.contains("' Remove-Item"),
            "注入串没有被转义: {}",
            escaped
        );
        assert_eq!(
            escaped.matches('\'').count(),
            evil.matches('\'').count() * 2
        );
    }

    #[test]
    fn uac_script_uses_literalpath_and_escapes_paths() {
        let items = vec![
            (
                "C:\\Users\\Bob\\AppData\\Local\\Temp\\x".to_string(),
                "临时文件".to_string(),
            ),
            ("C:\\Users\\Bob's\\cache".to_string(), "Cache".to_string()),
        ];
        let script = build_uac_delete_script(&items, "C:\\tmp\\result.log");

        // -LiteralPath 而非 -Path：否则含 [] 的目录会被当通配符
        assert!(script.contains("Remove-Item -LiteralPath $p -Recurse -Force"));
        assert!(script.contains("Test-Path -LiteralPath $p"));
        // 两条路径都被写入，且带撇号的那条做了转义
        assert!(script.contains("'C:\\Users\\Bob\\AppData\\Local\\Temp\\x'"));
        assert!(script.contains("'C:\\Users\\Bob''s\\cache'"));
        // 结果文件落盘
        assert!(script.contains("[System.IO.File]::WriteAllLines('C:\\tmp\\result.log'"));
    }

    #[test]
    fn uac_script_result_line_protocol() {
        let items = vec![("C:\\a".to_string(), "A".to_string())];
        let script = build_uac_delete_script(&items, "C:\\r");
        assert!(script.contains("OK`t$p"), "缺少成功行协议");
        assert!(script.contains("FAIL`t$p"), "缺少失败行协议");
    }

    #[test]
    fn uac_script_empty_items_still_valid() {
        let script = build_uac_delete_script(&[], "C:\\r");
        assert!(script.contains("$paths = @(\n)"));
        assert!(script.contains("WriteAllLines"));
    }
}
