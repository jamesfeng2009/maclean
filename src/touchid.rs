//! Touch ID 支持
//!
//! 通过创建 /etc/pam.d/sudo_local 启用 Touch ID 认证（macOS 14+ 官方推荐方式）。
//! /etc/pam.d/sudo 中已包含 `auth include sudo_local`，只需创建 sudo_local 文件
//! 并取消注释 pam_tid.so 行即可。
//!
//! 特权操作使用三种方案（按优先级）：
//! 1. AEWP (AuthorizationExecuteWithPrivileges) — 系统原生密码弹窗，不打开终端
//! 2. Terminal.app + sudo — 回退方案，打开终端窗口
//! 3. osascript administrator — 最终回退（可能被 TCC 拦截）
//!
//! 兼容 macOS 12+ (Monterey 到 Tahoe)，Intel 和 Apple Silicon 均可。

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
        for line in stdout.lines() {
            if line.contains("biometric template") {
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

/// 准备 sudo_local 临时文件内容
fn prepare_sudo_local_content() -> String {
    "# sudo_local: local config file which survives system update and is included for sudo\n\
# uncomment following line to enable Touch ID for sudo\n\
auth       sufficient     pam_tid.so\n"
        .to_string()
}

/// 异步触发启用 Touch ID（非阻塞）
///
/// 优先使用 AEWP（系统原生密码弹窗），不可用时回退到 Terminal.app。
/// 调用者应通过 `sudo_touch_id_enabled()` 轮询检测是否启用成功。
pub fn trigger_enable_touch_id() -> Result<(), String> {
    if sudo_touch_id_enabled() {
        return Ok(());
    }

    if !touch_id_supported() {
        return Err("系统不支持 Touch ID".to_string());
    }

    // 1. 准备 sudo_local 内容到临时文件
    let tmp_path = "/tmp/maclean_sudo_local.tmp";
    let content = prepare_sudo_local_content();
    std::fs::write(tmp_path, &content)
        .map_err(|e| format!("无法写入临时文件: {}", e))?;

    // 2. 优先尝试 AEWP（在后台线程中执行，不阻塞 GUI）
    if crate::aewp::aewp_available() {
        let tmp_path_owned = tmp_path.to_string();
        std::thread::spawn(move || {
            let script = format!(
                "cp {} {} && chmod 444 {} && rm -f {}",
                tmp_path_owned, SUDO_LOCAL_PATH, SUDO_LOCAL_PATH, tmp_path_owned
            );
            let _ = crate::aewp::execute_with_privileges("/bin/sh", &["-c", &script]);
        });
        return Ok(());
    }

    // 3. 回退：通过 Terminal.app 执行 sudo cp（非阻塞）
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
/// 优先使用 AEWP，不可用时回退到 Terminal.app。
pub fn trigger_disable_touch_id() -> Result<(), String> {
    if !sudo_touch_id_enabled() {
        return Ok(());
    }

    // 1. 优先尝试 AEWP（后台线程）
    if crate::aewp::aewp_available() {
        std::thread::spawn(move || {
            let _ = crate::aewp::execute_with_privileges(
                "/bin/rm",
                &["-f", SUDO_LOCAL_PATH],
            );
        });
        return Ok(());
    }

    // 2. 回退：Terminal.app
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
