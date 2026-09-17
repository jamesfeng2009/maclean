//! Touch ID 支持
//!
//! 通过创建 /etc/pam.d/sudo_local 启用 Touch ID 认证（macOS 14+ 官方推荐方式）。
//! /etc/pam.d/sudo 中已包含 `auth include sudo_local`，只需创建 sudo_local 文件
//! 并取消注释 pam_tid.so 行即可。
//!
//! 启用流程说明：
//! - 创建 /etc/pam.d/sudo_local 需要管理员权限，首次启用必须输入一次密码
//!   （因为此时 Touch ID for sudo 尚未启用，无法绕过密码框）。
//! - 启用成功后，后续所有 sudo 命令都会由系统弹出 Touch ID 提示。
//!
//! 特权操作使用 osascript `do shell script ... with administrator privileges`，
//! 比 AEWP (AuthorizationExecuteWithPrivileges) 在新版 macOS 上更稳定可靠。
//!
//! 兼容 macOS 12+ (Monterey 到 Tahoe)，Intel 和 Apple Silicon 均可。

use std::io::Write;
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
    let output = Command::new("/usr/bin/bioutil").args(["-c"]).output();

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
    let output = Command::new("/usr/bin/bioutil").args(["-r"]).output();

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

/// 生成安装 sudo_local 的 shell 命令（内容内联，不产生临时文件）
///
/// 安全说明（P0-2）：旧实现把内容写死在 `/tmp/maclean_sudo_local.tmp`，
/// 再用 `sudo cp` 复制到 /etc/pam.d/sudo_local。写入与复制之间存在时间窗：
/// 本地任意进程都可以抢先创建或在写完后替换该文件，从而以 root 写入任意
/// PAM 配置 —— 这是标准的本地提权路径。
///
/// 现在内容直接内联进 `sh -c`，中间不落任何可被替换的文件。
fn install_sudo_local_script() -> String {
    let content = prepare_sudo_local_content();
    let mut parts: Vec<String> = Vec::new();
    for line in content.lines() {
        // 内容固定且不含单引号；去掉单引号只是防御性处理
        parts.push(format!("printf '%s\\n' '{}'", line.replace('\'', "")));
    }
    format!(
        "{{ {} ; }} > {} && /usr/sbin/chown root:wheel {} && /bin/chmod 444 {}",
        parts.join(" ; "),
        SUDO_LOCAL_PATH,
        SUDO_LOCAL_PATH,
        SUDO_LOCAL_PATH
    )
}

/// 准备 sudo_local 临时文件内容
fn prepare_sudo_local_content() -> String {
    "# sudo_local: local config file which survives system update and is included for sudo\n\
# uncomment following line to enable Touch ID for sudo\n\
auth       sufficient     pam_tid.so\n"
        .to_string()
}

/// 使用 sudo -S 验证密码是否有效
///
/// 通过 `sudo -k && echo password | sudo -S -p "" -v` 验证。
/// 返回 true 表示密码正确且 sudo 票据已缓存。
fn verify_sudo_password(password: &str) -> bool {
    let _ = Command::new("/usr/bin/sudo").arg("-k").output();
    let mut child = match std::process::Command::new("/usr/bin/sudo")
        .args(["-S", "-p", "", "-v"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(format!("{}\n", password).as_bytes());
    }

    match child.wait() {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

/// 使用密码通过 sudo -S 创建 /etc/pam.d/sudo_local
///
/// 因为 osascript `with administrator privileges` 在 macOS 15 上
/// 无法写入 /etc/pam.d/，所以改用 sudo -S。
///
/// 返回 Ok(true) 表示成功创建，Ok(false) 表示已存在或无需创建，
/// Err 表示密码错误或其他错误。
pub fn enable_touch_id_with_password(password: &str) -> Result<bool, String> {
    if sudo_touch_id_enabled() {
        return Ok(false);
    }

    if !touch_id_supported() {
        return Err("系统不支持 Touch ID".to_string());
    }

    // 1. 先验证密码是否正确
    if !verify_sudo_password(password) {
        return Err("密码错误".to_string());
    }

    // 2. 生成安装脚本（内容内联，不写临时文件，见 install_sudo_local_script 注释）
    let script = install_sudo_local_script();

    // 3. 使用 sudo -S 写入 /etc/pam.d/sudo_local（不存在可被替换的中间文件）
    let mut child = Command::new("/usr/bin/sudo")
        .args(["-S", "-p", "", "/bin/sh", "-c", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 sudo: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(format!("{}\n", password).as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("sudo 执行失败: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("创建 sudo_local 失败: {}", stderr));
    }

    // 4. 验证是否真的启用了
    if !sudo_touch_id_enabled() {
        return Err("sudo_local 创建后未能检测到 Touch ID 配置".to_string());
    }

    // 5. 清除 sudo 票据，确保下一次 sudo 能触发 Touch ID
    // 否则 sudo -S -v 获取的票据会让后续 sudo 跳过认证
    let _ = Command::new("/usr/bin/sudo").arg("-k").output();

    Ok(true)
}

/// 触发启用 Touch ID 流程（旧版 osascript 方式，已废弃）
///
/// 保留此函数以便兼容旧调用点，但实际逻辑改为返回提示信息，
/// 调用者应改用 `enable_touch_id_with_password`。
pub fn trigger_enable_touch_id() -> Result<(), String> {
    Err("请使用 enable_touch_id_with_password 并提供管理员密码".to_string())
}

/// 使用密码通过 sudo -S 禁用 Touch ID
#[allow(dead_code)]
pub fn disable_touch_id_with_password(password: &str) -> Result<bool, String> {
    if !sudo_touch_id_enabled() {
        return Ok(false);
    }

    if !verify_sudo_password(password) {
        return Err("密码错误".to_string());
    }

    let script = format!("rm -f \"{}\"", SUDO_LOCAL_PATH);
    let mut child = Command::new("/usr/bin/sudo")
        .args(["-S", "-p", "", "/bin/sh", "-c", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 sudo: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(format!("{}\n", password).as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("sudo 执行失败: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("删除 sudo_local 失败: {}", stderr));
    }

    Ok(true)
}

/// 异步触发禁用 Touch ID（非阻塞）
#[allow(dead_code)]
pub fn trigger_disable_touch_id() -> Result<(), String> {
    Err("请使用 disable_touch_id_with_password 并提供管理员密码".to_string())
}
