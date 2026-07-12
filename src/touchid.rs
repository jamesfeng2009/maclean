//! Touch ID 支持
//!
//! 通过创建 /etc/pam.d/sudo_local 启用 Touch ID 认证（macOS 14+ 官方推荐方式）。
//! /etc/pam.d/sudo 中已包含 `auth include sudo_local`，只需创建 sudo_local 文件
//! 并取消注释 pam_tid.so 行即可。
//!
//! 注意：由于 SIP 保护，osascript 的 administrator 权限无法修改 /etc/pam.d/。
//! 必须通过 Terminal.app 中的 sudo 命令来创建/删除 sudo_local 文件。

use std::path::Path;
use std::process::Command;

/// sudo_local 文件路径
const SUDO_LOCAL_PATH: &str = "/etc/pam.d/sudo_local";

/// 检查系统是否支持 Touch ID（pam_tid.so 存在）
pub fn touch_id_supported() -> bool {
    // macOS 上 pam_tid.so 可能位于以下路径
    Path::new("/usr/lib/pam/pam_tid.so.2").exists()
        || Path::new("/usr/lib/pam/pam_tid.so").exists()
        || Path::new("/usr/libexec/pam_tid.so").exists()
}

/// 检查用户是否已录入指纹
pub fn touch_id_enrolled() -> bool {
    // bioutil -c 检查当前用户的指纹数量（不需要 sudo）
    let output = Command::new("/usr/bin/bioutil")
        .args(["-c"])
        .output();

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        // 输出格式: "User 501:       1 biometric template(s)"
        // 提取数字部分
        for line in stdout.lines() {
            if line.contains("biometric template") {
                // 提取 "N biometric template" 中的 N
                if let Some(num) = line
                    .split_whitespace()
                    .find(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty())
                {
                    if let Ok(n) = num.parse::<u32>() {
                        return n > 0;
                    }
                }
            }
        }
    }

    // Fallback: 检查 bioutil -r 输出
    let output = Command::new("/usr/bin/bioutil")
        .args(["-r"])
        .output();

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        // 输出包含 "Biometrics for unlock: 1" 表示已启用
        return stdout.contains("Biometrics for unlock: 1");
    }

    false
}

/// 检查是否同时有 Touch ID 硬件支持和已录入指纹
pub fn touch_id_available() -> bool {
    touch_id_supported() && touch_id_enrolled()
}

/// 检查 sudo 是否已启用 Touch ID 认证
///
/// 检查两个位置：
/// 1. /etc/pam.d/sudo_local（macOS 14+ 推荐方式，通过 include 引入）
/// 2. /etc/pam.d/sudo（旧方式，直接修改）
pub fn sudo_touch_id_enabled() -> bool {
    // 优先检查 sudo_local（推荐方式）
    if let Ok(content) = std::fs::read_to_string(SUDO_LOCAL_PATH) {
        // 确认 pam_tid.so 行未被注释
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            if trimmed.contains("pam_tid.so") {
                return true;
            }
        }
    }

    // 兼容旧方式：直接在 sudo 文件中
    if let Ok(content) = std::fs::read_to_string("/etc/pam.d/sudo") {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            if trimmed.contains("pam_tid.so") {
                return true;
            }
        }
    }

    false
}

/// 异步触发启用 Touch ID（非阻塞）
///
/// 打开 Terminal.app 执行 sudo cp 命令，立即返回。
/// 调用者应通过 `sudo_touch_id_enabled()` 轮询检测是否启用成功。
///
/// 返回 Ok 表示 Terminal 已成功打开，Err 表示无法打开 Terminal。
pub fn trigger_enable_touch_id() -> Result<(), String> {
    if sudo_touch_id_enabled() {
        return Ok(());
    }

    if !touch_id_supported() {
        return Err("系统不支持 Touch ID".to_string());
    }

    // 1. 准备 sudo_local 内容到临时文件
    let sudo_local_content = "# sudo_local: local config file which survives system update and is included for sudo\n\
# uncomment following line to enable Touch ID for sudo\n\
auth       sufficient     pam_tid.so\n";

    let tmp_path = "/tmp/maclean_sudo_local.tmp";
    std::fs::write(tmp_path, sudo_local_content)
        .map_err(|e| format!("无法写入临时文件: {}", e))?;

    // 2. 通过 Terminal.app 执行 sudo cp（非阻塞）
    let terminal_script = format!(
        "sudo cp {} {} && sudo chmod 444 {} && echo MACLEAN_TOUCHID_DONE && sleep 1 && exit",
        tmp_path, SUDO_LOCAL_PATH, SUDO_LOCAL_PATH
    );

    let apple_script = format!(
        r#"tell application "Terminal"
    activate
    do script "{}"
end tell"#,
        terminal_script.replace('"', "\\\"")
    );

    Command::new("/usr/bin/osascript")
        .args(["-e", &apple_script])
        .output()
        .map_err(|e| format!("无法打开 Terminal: {}", e))?;

    Ok(())
}

/// 异步触发禁用 Touch ID（非阻塞）
///
/// 打开 Terminal.app 执行 sudo rm 命令，立即返回。
/// 调用者应通过 `sudo_touch_id_enabled()` 轮询检测是否禁用成功。
pub fn trigger_disable_touch_id() -> Result<(), String> {
    if !sudo_touch_id_enabled() {
        return Ok(());
    }

    let terminal_script = format!(
        "sudo rm {} && echo MACLEAN_TOUCHID_DISABLED && sleep 1 && exit",
        SUDO_LOCAL_PATH
    );

    let apple_script = format!(
        r#"tell application "Terminal"
    activate
    do script "{}"
end tell"#,
        terminal_script.replace('"', "\\\"")
    );

    Command::new("/usr/bin/osascript")
        .args(["-e", &apple_script])
        .output()
        .map_err(|e| format!("无法打开 Terminal: {}", e))?;

    Ok(())
}
