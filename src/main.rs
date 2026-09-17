//! maclean - macOS 磁盘清理 GUI 工具
//!
//! 使用 egui 构建，专注开发者缓存与深度清理。

#[cfg(target_os = "macos")]
mod aewp;
mod app;
mod app_protection;
mod cli;
mod config;
mod i18n;
mod icons;
mod license;
mod logger;
mod menubar;
mod ops;
mod platform;
mod safety;
mod scanner;
#[cfg(target_os = "macos")]
mod sudo_keepalive;
mod theme;
#[cfg(target_os = "macos")]
mod touchid;
mod ui;
mod updater;
mod widgets;

use crate::ui::Gui;

/// 写入扫描日志（用于追踪扫描进度，崩溃时定位问题）
pub fn log_scan_step(msg: &str) {
    logger::info(msg);
}

/// 从项目根目录 `.env` 加载环境变量（仅开发用，无第三方依赖）
///
/// 解析 KEY=VALUE 格式，忽略空行和 # 注释。
/// 若变量已存在于系统环境，则保留系统值（方便 launch.json / shell 覆盖）。
fn load_dotenv() {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(_) => return,
    };
    let path = cwd.join(".env");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            // 已存在的环境变量优先，不覆盖
            if std::env::var(key).is_err() {
                std::env::set_var(key, value);
            }
        }
    }
}

fn main() -> eframe::Result {
    // 优先加载 .env（开发模式 MACLEAN_DEV=1 等配置）
    load_dotenv();

    // 初始化日志系统（CLI 和 GUI 模式都需要）
    logger::init();

    // CLI 模式：有子命令时执行并退出，无子命令时启动 GUI
    if cli::run_cli() {
        logger::info("CLI 模式执行完毕，退出");
        return Ok(());
    }

    // GUI 模式
    logger::info("GUI 模式启动");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 680.0])
            .with_min_inner_size([760.0, 540.0])
            .with_title("Maclean"),
        ..Default::default()
    };

    eframe::run_native(
        "Maclean",
        options,
        Box::new(|_cc| Ok(Box::new(Gui::new()) as Box<dyn eframe::App>)),
    )
}

/// 获取磁盘信息 (macOS 实现)
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

/// 获取磁盘信息 (Windows 实现)
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

/// 获取磁盘信息（跨平台入口）
fn get_disk_info() -> (u64, u64) {
    platform::disk_info()
}

#[cfg(test)]
mod tests {
    // 上轮补 P0 回归测试时加的：sanitize_before_delete / write_private_temp_file 等
    // 都从这里来。别改成逐个具名导入再删这条 —— 编译能过但测试会集体失踪。
    use crate::ops::*;

    // ---------- P0-1: sudo 阶段二次校验 ----------

    #[test]
    fn sanitize_rejects_path_with_newline() {
        // 换行会让 sudo 脚本的单引号包裹失效 → 命令注入
        let (allowed, rejected) = sanitize_before_delete(
            vec![("/tmp/foo\n/bin/rm -rf /".to_string(), "缓存".to_string())],
            false,
        );
        assert!(allowed.is_empty());
        assert_eq!(rejected.len(), 1);
    }

    #[test]
    fn sanitize_rejects_path_with_command_substitution() {
        let (allowed, rejected) = sanitize_before_delete(
            vec![("/tmp/$(whoami)".to_string(), "缓存".to_string())],
            false,
        );
        assert!(allowed.is_empty());
        assert_eq!(rejected.len(), 1);
    }

    #[test]
    fn sanitize_rejects_path_with_backtick() {
        let (allowed, rejected) =
            sanitize_before_delete(vec![("/tmp/a`id`b".to_string(), "缓存".to_string())], false);
        assert!(allowed.is_empty());
        assert_eq!(rejected.len(), 1);
    }

    #[test]
    fn sanitize_rejects_system_protected_path() {
        // 系统关键目录即使在阶段一漏过，阶段二也必须拦下
        let (allowed, rejected) = sanitize_before_delete(
            vec![("/System/Library/Foo".to_string(), "缓存".to_string())],
            false,
        );
        assert!(allowed.is_empty());
        assert_eq!(rejected.len(), 1);
    }

    #[test]
    fn sanitize_rejects_empty_path() {
        let (allowed, rejected) =
            sanitize_before_delete(vec![(String::new(), "缓存".to_string())], false);
        assert!(allowed.is_empty());
        assert_eq!(rejected.len(), 1);
    }

    // ---------- P1: 挂载点判定 ----------

    #[cfg(target_os = "macos")]
    #[test]
    fn mount_detection_matches_exact_and_nested() {
        let mounts = vec!["/Volumes/My Disk".to_string(), "/".to_string()];
        // 完全相等
        assert!(is_path_mounted("/Volumes/My Disk", &mounts));
        // 挂载点在路径之下（原逻辑）
        assert!(is_path_mounted("/Volumes", &mounts));
        // 路径位于挂载点之内（新增：原来会漏判并放行删除挂载中的卷）
        assert!(is_path_mounted("/Volumes/My Disk/sub", &mounts));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mount_detection_is_not_fooled_by_spaces() {
        // 旧实现用 split_whitespace 解析 mount 输出，含空格的挂载点会被截断
        let mounts = vec!["/Volumes/My Disk".to_string()];
        assert!(is_path_mounted("/Volumes/My Disk", &mounts));
        assert!(is_path_mounted("/Volumes/My Disk/inner", &mounts));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mount_detection_ignores_root_mount() {
        // "/" 是根挂载，不能让所有路径都判为"已挂载"
        let mounts = vec!["/".to_string()];
        assert!(!is_path_mounted("/Users/foo/Library/Caches", &mounts));
    }

    // ---------- P0-2: 受保护的临时文件 ----------

    #[cfg(unix)]
    #[test]
    fn temp_script_is_private_and_unpredictable() {
        use std::os::unix::fs::PermissionsExt;

        let a = write_private_temp_file("maclean_test", "sh", "#!/bin/bash\ntrue\n")
            .expect("应能创建临时脚本");
        let b = write_private_temp_file("maclean_test", "sh", "#!/bin/bash\ntrue\n")
            .expect("应能创建临时脚本");

        // 文件名不可预测：连续两次创建不能撞名
        assert_ne!(a, b);

        // 权限必须是 0600，其他用户不可读写（否则可被替换成恶意脚本）
        let mode = std::fs::metadata(&a).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "临时脚本权限应为 0600，实际 {:o}", mode);

        // 内容正确落盘
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "#!/bin/bash\ntrue\n");

        let _ = std::fs::remove_file(a);
        let _ = std::fs::remove_file(b);
    }

    #[cfg(unix)]
    #[test]
    fn temp_file_never_overwrites_existing() {
        // O_EXCL 语义：目录下已有同名文件时应换名重试，而不是覆盖。
        // 这里验证连续创建 32 个文件彼此不冲突（模拟攻击者占位场景）。
        let mut paths = Vec::new();
        for i in 0..32 {
            let p = write_private_temp_file("maclean_race", "sh", &format!("true #{}", i))
                .expect("应能创建");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("true #{}", i));
            paths.push(p);
        }
        let unique: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(unique.len(), paths.len(), "临时文件名发生碰撞");
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    }
}
