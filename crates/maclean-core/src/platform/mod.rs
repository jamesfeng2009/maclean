//! 平台抽象层
//!
//! 提供跨平台基础函数，屏蔽 macOS / Windows 差异。
//! - 路径相关：home_dir / app_data_dir / expand_path
//! - 删除到废纸篓：move_to_trash
//! - check_deletable 的跨平台实现

use std::path::{Path, PathBuf};

// Windows 操作前备份模块（系统还原点 + 注册表备份 + 还原入口）
#[cfg(target_os = "windows")]
pub mod windows_backup;
// macOS 进程内「移入废纸篓」（NSFileManager / NSWorkspace），替代 osascript 代删
#[cfg(target_os = "macos")]
mod macos_trash;
// 注册表备份的可信性校验。刻意不作平台限定 —— 这是 `reg import` 前最后一道关，
// 必须在开发机上就能编译和测试，详见模块顶部注释。
mod reg_safety;

/// 获取当前用户 home 目录
///
/// macOS: $HOME
/// Windows: %USERPROFILE%
pub fn home_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Default"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
    }
}

/// maclean 应用数据目录（日志、缓存等）
///
/// macOS: ~/.maclean/
/// Windows: %APPDATA%\maclean\
pub fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home_dir().join(".maclean")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join("AppData").join("Roaming"))
            .join("maclean")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir().join(".maclean")
    }
}

// 2026-09-18 删除了 `platform::expand_path`：全仓零引用，且与
// `scanner::cache_registry::expand_path`（两参数、真正被 scan() 调用的那个）
// 重复。两份实现并存时，"改哪份才算生效"是个陷阱。

/// 检测路径是否可删除（跨平台）
///
/// 返回 (deletable, reason)
pub fn check_deletable(path: &str) -> (bool, String) {
    let p = Path::new(path);

    // APFS 快照由专门逻辑处理
    if path.starts_with("snapshot:") {
        return (true, String::new());
    }

    #[cfg(target_os = "macos")]
    {
        // SIP/系统保护路径（/System、/usr、/Library/Developer/CoreSimulator 等）：
        // 任何权限（含管理员/Touch ID）都无法删除。扫描期直接标记不可删，
        // 默认不出现在清理列表（show_protected 开关打开时才可见、标不可删），
        // 避免"勾选 → 授权 → 删除失败(SIP) → 再弹窗"的循环。
        // 判定与删除入口 safety::is_critical_system_path 共用同一张表，
        // 保证"扫不出来"与"删不掉"永远一致。
        if crate::safety::is_critical_system_path(path) {
            return (
                false,
                "SIP保护: 此路径受 macOS 系统保护，任何权限均无法删除".to_string(),
            );
        }

        // 检查属主（unix 专属）
        if let Ok(meta) = p.symlink_metadata() {
            use std::os::unix::fs::MetadataExt;
            let uid = meta.uid();
            if uid == 0 {
                return (true, String::new());
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: 简化判断，Program Files 下的需要管理员权限但仍可尝试删除
        // 实际权限由删除时检查
        let _ = p; // 避免未使用警告
    }

    (true, String::new())
}

/// 目标是否位于只读卷（P1-2）
///
/// OrbStack 挂载卷、APFS 快照等只读卷上，Finder 无法把文件移入废纸篓，
/// 调用 osascript 只会弹"Some files can't be processed"对话框且**无法静音**。
/// 删除前预先探测：命中只读卷直接记失败跳过，不触发系统弹窗。
///
/// 探测失败（路径不可解析、statvfs 出错）一律返回 false —— 宁可让 Finder
/// 弹窗，也不因误判把可删文件跳过。
pub fn path_on_readonly_volume(path: &str) -> bool {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let _ = path;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            let Ok(c_path) = CString::new(path) else {
                return false;
            };
            let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(c_path.as_ptr(), &mut buf) } != 0 {
                return false;
            }
            (buf.f_flag & libc::ST_RDONLY) != 0
        }
        #[cfg(windows)]
        {
            false
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = path;
        false
    }
}

/// 移动文件/目录到废纸篓（跨平台）
///
/// macOS: 进程内 `NSFileManager`（静默回收）→ `NSWorkspace.recycleURLs`
///   （由本 App 弹正规 Touch ID / 密码授权），详见 [`macos_trash`]；
///   失败时**绝不**降级为永久删除。
/// Windows: 调用 SHFileOperation FO_DELETE + FOF_ALLOWUNDO
pub fn move_to_trash(path: &str) -> bool {
    let p = Path::new(path);
    if !p.exists() && p.symlink_metadata().is_err() {
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        macos_trash::move_to_trash_impl(path)
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: 用 PowerShell 调用 VisualBasic.FileSystem 模块的 FileIO.FileSystem.DeleteDirectory
        // 带 UIOption::OnlyErrorDialogs + RecycleOption::SendToRecycleBin
        let ps_cmd = if p.is_dir() {
            format!(
                "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory('{}','OnlyErrorDialogs','SendToRecycleBin')",
                path.replace('\'', "''")
            )
        } else {
            format!(
                "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile('{}','OnlyErrorDialogs','SendToRecycleBin')",
                path.replace('\'', "''")
            )
        };
        let status = std::process::Command::new("powershell")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(&ps_cmd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        status.map(|s| s.success()).unwrap_or(false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // 本平台没有可用的废纸篓实现。绝不能伪装成"已移入废纸篓"直接
        // remove —— 调用方（use_trash 链路）承诺失败即保留文件。
        let _ = p;
        false
    }
}

/// 系统是否处于深色外观（用于首次启动时初始化主题）
///
/// - macOS: `defaults read -g AppleInterfaceStyle`，输出含 "Dark" 即深色
/// - Windows: `HKCU\...\Themes\Personalize` 的 `AppsUseLightTheme` 为 0 即深色
/// - 其它平台 / 命令失败：按浅色处理
///
/// 只在首次启动（config.json 不存在）时调用一次，失败不影响可用性。
pub fn system_prefers_dark() -> bool {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output();
        matches!(
            out.ok().map(|o| String::from_utf8_lossy(&o.stdout).to_string()),
            Some(s) if s.trim().eq_ignore_ascii_case("dark")
        )
    }

    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout).to_string();
                // 形如 `AppsUseLightTheme    REG_DWORD    0x0`
                s.rsplit_once("0x")
                    .map(|(_, v)| v.trim().starts_with('0'))
                    .unwrap_or(false)
            }
            Err(_) => false,
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// 获取磁盘信息 (macOS 实现) → (total, used)
///
/// 用 `df -k /` 读根卷容量 / 可用量（阶段 0 从 bin 的 main.rs 原样下沉，
/// 供 egui 壳与 Tauri 壳共用同一份实现）。
#[cfg(target_os = "macos")]
fn get_disk_info_macos() -> (u64, u64) {
    let output = std::process::Command::new("df").arg("-k").arg("/").output();

    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                if let (Ok(total_kb), Ok(free_kb)) =
                    (parts[1].parse::<u64>(), parts[3].parse::<u64>())
                {
                    return (total_kb * 1024, free_kb * 1024);
                }
            }
        }
    }

    (0, 0)
}

/// 获取磁盘信息 (Windows 实现) → (total, free)
///
/// 用 PowerShell `Get-PSDrive C` 读系统盘（阶段 0 从 main.rs 原样下沉）。
#[cfg(target_os = "windows")]
fn get_disk_info_windows() -> (u64, u64) {
    // Windows: 用 fsutil 或 wmic 获取磁盘信息
    // 这里用 PowerShell 调用 Get-PSDrive
    let output = std::process::Command::new("powershell")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg("Get-PSDrive C | Select-Object Used,Free | ConvertTo-Json")
        .output();

    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // 解析 JSON: {"Used":123,"Free":456}
        let mut used: u64 = 0;
        let mut free: u64 = 0;
        for line in stdout.lines() {
            let line = line.trim();
            if line.starts_with("\"Used\"") {
                if let Some(val) = line.split(':').nth(1) {
                    let val = val.trim().trim_end_matches(',').trim();
                    used = val.parse::<u64>().unwrap_or(0);
                }
            } else if line.starts_with("\"Free\"") {
                if let Some(val) = line.split(':').nth(1) {
                    let val = val.trim().trim_end_matches(',').trim();
                    free = val.parse::<u64>().unwrap_or(0);
                }
            }
        }
        return (used + free, free);
    }

    (0, 0)
}

/// 获取磁盘信息 (total, free)
///
/// macOS: `df -k /`；Windows: PowerShell `Get-PSDrive C`。
pub fn disk_info() -> (u64, u64) {
    #[cfg(target_os = "macos")]
    {
        get_disk_info_macos()
    }

    #[cfg(target_os = "windows")]
    {
        get_disk_info_windows()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::check_deletable;

    #[cfg(target_os = "macos")]
    #[test]
    fn check_deletable_marks_sip_protected_paths_undeletable() {
        // SIP/系统保护路径（含 CoreSimulator）：任何权限都删不掉，
        // 扫描期必须标不可删 —— 否则用户勾选 → 授权 → 删除失败(SIP) → 弹窗循环。
        for p in [
            "/Library/Developer/CoreSimulator/Volumes/runtime",
            "/Library/Developer/CoreSimulator/Caches/iOS",
            "/System/Library/Frameworks",
            "/usr/bin",
            "/Library/LaunchDaemons/com.example.plist",
        ] {
            let (deletable, reason) = check_deletable(p);
            assert!(!deletable, "SIP 路径不应可删: {p}");
            assert!(!reason.is_empty(), "SIP 路径应有不可删原因: {p}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn check_deletable_keeps_user_paths_deletable() {
        // 用户目录/缓存不在系统保护表内，仍可删（走废纸篓或提权）
        let home = crate::scanner::home_dir();
        for p in [
            home.join("Library/Caches/com.example")
                .to_string_lossy()
                .to_string(),
            home.join("Downloads/tmp").to_string_lossy().to_string(),
            home.join(".npm/_cacache").to_string_lossy().to_string(),
        ] {
            let (deletable, _) = check_deletable(&p);
            assert!(deletable, "用户路径应可删: {p}");
        }
    }
}
