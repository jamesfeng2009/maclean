//! sudo 会话保活模块
//!
//! 通过定期执行 `sudo -n -v` 刷新 sudo 票据，避免每次 sudo 操作都弹出密码框。
//! 默认 macOS sudo 票据有效期为 5 分钟（300 秒），这里每 25 秒刷新一次，
//! 留足裕量防止票据过期。

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 保活线程的停止标志和密码（密码仅在 start 时使用一次，不长期保存）
struct KeepaliveState {
    stop_flag: Arc<AtomicBool>,
}

/// 全局保活状态（线程安全）
///
/// 2026-09 之前这里是 `static mut`，读写要 `unsafe`；现在直接放进 `Mutex`，
/// 与下面的 `KEEPALIVE_INIT` 合并成一个锁保护的 `Option`，借用检查器兜底。
static KEEPALIVE_STATE: Mutex<Option<KeepaliveState>> = Mutex::new(None);
static KEEPALIVE_INIT: Mutex<()> = Mutex::new(());

/// 启动 sudo 会话
///
/// 1. 用 `sudo -S -v` 验证密码并获取 sudo 票据
/// 2. 启动后台线程，每 25 秒执行 `sudo -n -v` 刷新票据
///
/// 返回 Ok(true) 表示成功启动，Ok(false) 表示已有活动会话，
/// Err 表示密码错误或无法启动。
pub fn start_sudo_session(password: &str) -> Result<bool, String> {
    // 加锁防止并发初始化
    let _guard = KEEPALIVE_INIT.lock().unwrap();

    // 如果已有活动会话，直接返回
    if is_sudo_active() {
        // 停止旧的保活线程（如果有）
        stop_keepalive_thread();
        // 继续用新密码重新验证
    }

    // 1. 验证密码并获取 sudo 票据
    // 先清除旧票据
    let _ = std::process::Command::new("/usr/bin/sudo")
        .arg("-k")
        .output();

    let mut child = std::process::Command::new("/usr/bin/sudo")
        .args(["-S", "-p", "", "-v"])
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
        if stderr.contains("incorrect password") || stderr.contains("3 incorrect") {
            return Err("密码错误".to_string());
        }
        return Err(format!("sudo 验证失败: {}", stderr.trim()));
    }

    // 2. 启动保活线程
    let stop_flag = Arc::new(AtomicBool::new(false));
    let thread_stop = stop_flag.clone();

    std::thread::spawn(move || {
        // 每 25 秒刷新一次 sudo 票据
        while !thread_stop.load(Ordering::Relaxed) {
            // 分段 sleep 以便快速响应停止请求
            for _ in 0..25 {
                if thread_stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            // 执行 sudo -n -v 刷新票据（-n 表示非交互，不提示输入密码）
            let result = std::process::Command::new("/usr/bin/sudo")
                .args(["-n", "-v"])
                .output();
            // 如果 sudo -n -v 失败（票据已过期且无法非交互刷新），退出保活线程
            match result {
                Ok(o) if o.status.success() => { /* 刷新成功，继续 */ }
                _ => {
                    // 票据过期，停止保活
                    return;
                }
            }
        }
    });

    // 保存全局状态
    KEEPALIVE_STATE
        .lock()
        .unwrap()
        .replace(KeepaliveState { stop_flag });

    Ok(true)
}

/// 检查 sudo 会话是否仍然活跃
///
/// 通过 `sudo -n -v` 检测，成功返回 true。
pub fn is_sudo_active() -> bool {
    let output = std::process::Command::new("/usr/bin/sudo")
        .args(["-n", "-v"])
        .output();
    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

/// 结束 sudo 会话
///
/// 停止保活线程并清除 sudo 票据。
pub fn end_sudo_session() {
    stop_keepalive_thread();
    // 清除 sudo 票据
    let _ = std::process::Command::new("/usr/bin/sudo")
        .arg("-k")
        .output();
}

/// 停止保活线程（内部函数）
fn stop_keepalive_thread() {
    let _guard = KEEPALIVE_INIT.lock().unwrap();
    if let Some(state) = KEEPALIVE_STATE.lock().unwrap().take() {
        state.stop_flag.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_sudo_active_without_sudo() {
        // 在没有 sudo 票据的情况下，应该返回 false（CI 环境下）
        // 这个测试只是确保函数能正常调用，不依赖具体结果
        let _ = is_sudo_active();
    }
}
