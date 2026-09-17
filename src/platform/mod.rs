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

/// 展开路径中的 ~ 和环境变量
///
/// macOS: ~/Library/Caches → /Users/xxx/Library/Caches
/// Windows: %LOCALAPPDATA%\Foo → C:\Users\xxx\AppData\Local\Foo
pub fn expand_path(path: &str) -> PathBuf {
    let mut result = path.to_string();

    // 展开 ~ (macOS/Linux)
    if result.starts_with('~') {
        let home = home_dir();
        result = format!("{}{}", home.display(), &result[1..]);
    }

    // 展开 Windows 环境变量 %VAR%
    #[cfg(target_os = "windows")]
    {
        loop {
            let start = match result.find('%') {
                Some(s) => s,
                None => break,
            };
            let end = match result[start + 1..].find('%') {
                Some(e) => start + 1 + e,
                None => break,
            };
            let var_name = &result[start + 1..end];
            if let Ok(val) = std::env::var(var_name) {
                result = format!("{}{}{}", &result[..start], val, &result[end + 1..]);
            } else {
                // 找不到变量，跳过避免死循环
                break;
            }
        }
    }

    PathBuf::from(result)
}

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
        // CoreSimulator 运行时镜像
        if path.starts_with("/Library/Developer/CoreSimulator/Volumes") {
            return (true, String::new());
        }
        // CoreSimulator/Caches
        if path.starts_with("/Library/Developer/CoreSimulator/Caches") {
            return (true, String::new());
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

/// 移动文件/目录到废纸篓（跨平台）
///
/// macOS: 调用 NSWorkspace.recycleURLs 或 fallback 到 rm
/// Windows: 调用 SHFileOperation FO_DELETE + FOF_ALLOWUNDO
pub fn move_to_trash(path: &str) -> bool {
    let p = Path::new(path);
    if !p.exists() && !p.symlink_metadata().is_ok() {
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: 尝试 osascript 调用 Finder 移到废纸篓
        let script = format!(
            "tell application \"Finder\" to delete (POSIX file \"{}\" as alias)",
            path.replace('"', "\\\"")
        );
        let status = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if status.map(|s| s.success()).unwrap_or(false) {
            return true;
        }

        // 安全策略（P0-3）：废纸篓失败时**绝不**静默降级为永久删除。
        // osascript 调用 Finder 需要 TCC「自动化」授权，未授权时这里必然失败；
        // 旧实现会 fallback 到 remove_dir_all，用户以为"可恢复"的文件被永久删除。
        // 正确做法：返回 false，由调用方明确告知用户并保留文件。
        crate::logger::warn(&format!(
            "move_to_trash 失败，已保留文件（未降级为永久删除）: {}",
            path
        ));
        false
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
        if p.is_dir() {
            std::fs::remove_dir_all(p).is_ok()
        } else {
            std::fs::remove_file(p).is_ok()
        }
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

/// 获取磁盘信息 (total, used)
///
/// macOS: 调用 statvfs
/// Windows: 调用 GetDiskFreeSpaceEx
pub fn disk_info() -> (u64, u64) {
    #[cfg(target_os = "macos")]
    {
        crate::get_disk_info_macos()
    }

    #[cfg(target_os = "windows")]
    {
        crate::get_disk_info_windows()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        (0, 0)
    }
}
