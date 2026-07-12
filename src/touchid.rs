//! Touch ID 支持
//!
//! 通过修改 /etc/pam.d/sudo 启用 Touch ID 认证，
//! 之后所有 sudo 操作自动使用 Touch ID，无需输入密码。

use std::path::Path;
use std::process::Command;

/// 检查系统是否支持 Touch ID（pam_tid.so 存在）
pub fn touch_id_supported() -> bool {
    Path::new("/usr/libexec/pam_tid.so").exists()
}

/// 检查用户是否已录入指纹
pub fn touch_id_enrolled() -> bool {
    // bioutil 在 Apple Silicon 上可能不存在，用 system_profiler 作为 fallback
    let output = Command::new("/usr/bin/bioutil")
        .args(["-c", "-s"])
        .output();

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        // 输出格式: "UserTouchIDCount: N" 或类似
        for line in stdout.lines() {
            let lower = line.to_lowercase();
            if lower.contains("touchid") || lower.contains("touch_id") {
                if let Some(num) = line.split(':').nth(1) {
                    if let Ok(n) = num.trim().parse::<u32>() {
                        return n > 0;
                    }
                }
            }
        }
    }

    // Fallback: 检查 Touch ID 配置数据库
    let home = std::env::var("HOME").unwrap_or_default();
    let touchid_db = format!("{}/Library/Preferences/com.apple.TouchID.plist", home);
    if Path::new(&touchid_db).exists() {
        // 如果配置文件存在，且包含 fingerprint 数据
        let output = Command::new("/usr/bin/defaults")
            .args(["read", &touchid_db])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            return stdout.contains("Fingerprint") || stdout.contains(" fingerprint");
        }
    }

    false
}

/// 检查是否同时有 Touch ID 硬件支持和已录入指纹
pub fn touch_id_available() -> bool {
    touch_id_supported() && touch_id_enrolled()
}

/// 检查 sudo 是否已启用 Touch ID 认证
pub fn sudo_touch_id_enabled() -> bool {
    std::fs::read_to_string("/etc/pam.d/sudo")
        .map(|c| c.contains("pam_tid.so"))
        .unwrap_or(false)
}

/// 启用 sudo Touch ID 认证
///
/// 在 /etc/pam.d/sudo 中 pam_smartcard.so 行后插入 pam_tid.so
/// 需要管理员权限（通过 osascript 提权）
pub fn enable_touch_id_sudo() -> Result<(), String> {
    if sudo_touch_id_enabled() {
        return Ok(());
    }

    if !touch_id_supported() {
        return Err("系统不支持 Touch ID".to_string());
    }

    // 构建安全的 sed 命令：在 pam_smartcard.so 行后插入 pam_tid.so
    // 如果没有 pam_smartcard.so，则在第一行 auth 后插入
    let shell_script = r#"
set +e
# 备份原文件
timestamp=$(date +%s)
cp /etc/pam.d/sudo /etc/pam.d/sudo.bak.$timestamp

# 检查是否已存在 pam_tid.so
if grep -q 'pam_tid\.so' /etc/pam.d/sudo; then
    echo "ALREADY_ENABLED"
    exit 0
fi

# 尝试在 pam_smartcard.so 行后插入
if grep -q 'pam_smartcard\.so' /etc/pam.d/sudo; then
    sed -i '' '/pam_smartcard\.so/a\
auth       sufficient     pam_tid.so
' /etc/pam.d/sudo
else
    # 在第一个 auth 行后插入
    sed -i '' '0,/^auth/s/^auth/auth\
auth       sufficient     pam_tid.so\
/' /etc/pam.d/sudo
fi

# 验证
if grep -q 'pam_tid\.so' /etc/pam.d/sudo; then
    echo "OK"
else
    # 恢复备份
    cp /etc/pam.d/sudo.bak.$timestamp /etc/pam.d/sudo
    echo "FAILED: verification failed, restored backup"
    exit 1
fi
"#;

    let apple_script = format!(
        r#"do shell script "{}" with administrator privileges"#,
        shell_script.replace('\\', "\\\\").replace('"', "\\\"")
    );

    let output = Command::new("/usr/bin/osascript")
        .args(["-e", &apple_script])
        .output()
        .map_err(|e| format!("无法启动 osascript: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        // 用户取消或密码错误
        if stderr.contains("User canceled") || stderr.contains("user canceled") {
            return Err("用户取消授权".to_string());
        }
        if stderr.contains("Authentication failed") || stderr.contains("incorrect") {
            return Err("管理员密码错误".to_string());
        }
        return Err(format!("启用失败: {}", stderr.trim()));
    }

    if stdout.contains("OK") || stdout.contains("ALREADY_ENABLED") {
        Ok(())
    } else {
        Err(format!("启用失败: {}", stdout.trim()))
    }
}

/// 禁用 sudo Touch ID 认证（恢复原状）
pub fn disable_touch_id_sudo() -> Result<(), String> {
    if !sudo_touch_id_enabled() {
        return Ok(());
    }

    let shell_script = r#"
set +e
timestamp=$(date +%s)
cp /etc/pam.d/sudo /etc/pam.d/sudo.bak.$timestamp

# 删除 pam_tid.so 行
sed -i '' '/pam_tid\.so/d' /etc/pam.d/sudo

# 验证
if grep -q 'pam_tid\.so' /etc/pam.d/sudo; then
    cp /etc/pam.d/sudo.bak.$timestamp /etc/pam.d/sudo
    echo "FAILED"
    exit 1
else
    echo "OK"
    exit 0
fi
"#;

    let apple_script = format!(
        r#"do shell script "{}" with administrator privileges"#,
        shell_script.replace('\\', "\\\\").replace('"', "\\\"")
    );

    let output = Command::new("/usr/bin/osascript")
        .args(["-e", &apple_script])
        .output()
        .map_err(|e| format!("无法启动 osascript: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if stderr.contains("User canceled") || stderr.contains("user canceled") {
            return Err("用户取消授权".to_string());
        }
        return Err(format!("禁用失败: {}", stderr.trim()));
    }

    if stdout.contains("OK") {
        Ok(())
    } else {
        Err(format!("禁用失败: {}", stdout.trim()))
    }
}
