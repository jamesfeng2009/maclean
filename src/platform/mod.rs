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
/// AppleScript 双引号字符串字面量转义。必须先转义 `\` 再转义 `"`，
/// 否则引号转义产生的反斜杠会被二次处理。
#[cfg(any(target_os = "macos", test))]
fn applescript_string_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn move_to_trash(path: &str) -> bool {
    let p = Path::new(path);
    if !p.exists() && p.symlink_metadata().is_err() {
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: 尝试 osascript 调用 Finder 移到废纸篓
        // AppleScript 字符串字面量里 \ 和 " 都有特殊含义，必须都转义。
        // 只转义 " 时，文件名含 \" （macOS 合法）就能闭合字面量并注入后续
        // AppleScript 语句 —— Finder 常具完全磁盘访问，注入即越权删除。
        let script = format!(
            "tell application \"Finder\" to delete (POSIX file \"{}\" as alias)",
            applescript_string_literal(path)
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

#[cfg(test)]
mod tests {
    use super::applescript_string_literal;

    #[test]
    fn applescript_literal_cannot_be_closed_by_filename() {
        // 历史漏洞：只转义 " 不转义 \。文件名 `a\" & (do shell script "rm -rf …") & "`
        // 里的 \" 会吃掉引号转义、闭合字面量并注入语句。
        assert_eq!(applescript_string_literal(r#"plain"d"#), r#"plain\"d"#);
        assert_eq!(
            applescript_string_literal(r#"back\slash"#),
            r#"back\\slash"#
        );
        // 关键回归：注入样本转义后，字符串里不存在未配对的可闭合引号
        let evil = r#"x\" & (do shell script "rm -rf /") & ""#;
        let out = applescript_string_literal(evil);
        // 每个字面 " 前必是转义它的 \（即 \\ 或 \" 形式），首尾无裸引号
        assert!(out.starts_with("x\\\\"), "got {}", out);
        let bytes: Vec<char> = out.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == '"' {
                assert!(
                    i > 0 && bytes[i - 1] == '\\',
                    "裸引号可闭合字面量: pos {}",
                    i
                );
                i += 1;
            } else {
                i += 1;
            }
        }
    }
}
