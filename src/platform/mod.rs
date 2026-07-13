//! 平台抽象层
//!
//! 提供跨平台基础函数，屏蔽 macOS / Windows 差异。
//! - 路径相关：home_dir / app_data_dir / expand_path
//! - 删除到废纸篓：move_to_trash
//! - check_deletable 的跨平台实现

use std::path::{Path, PathBuf};

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
        while let (Some(start), Some(end)) = (result.find('%'), result[start + 1..].find('%')) {
            let var_name = &result[start + 1..start + 1 + end];
            if let Ok(val) = std::env::var(var_name) {
                result = format!("{}{}{}", &result[..start], val, &result[start + 2 + end..]);
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
        // fallback: 直接删除
        if p.is_dir() {
            std::fs::remove_dir_all(p).is_ok()
        } else {
            std::fs::remove_file(p).is_ok()
        }
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
