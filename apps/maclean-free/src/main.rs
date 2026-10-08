//! maclean - macOS 磁盘清理 GUI 工具
//!
//! 使用 egui 构建，专注开发者缓存与深度清理。
//!
//! 纯 CLI 构建（--no-default-features）裁剪 GUI 壳，但 GUI 状态层
//! （app.rs 的 App 及 ops 的 GUI 删除流程）仍编译、仅不被引用。
//! 裁剪的目标是不链接 GUI 库，而非零 dead_code —— 此处集中放行，避免刷屏。

#![cfg_attr(not(feature = "gui"), allow(dead_code))]

// 已删除 mod aewp（2026-09-18）：AuthorizationExecuteWithPrivileges 的 FFI 封装，
// 全仓零引用。提权删除现已统一走 touchid.rs 的
// `osascript do shell script ... with administrator privileges`（macOS 14+ 官方推荐）。
// AEWP 自 macOS 10.7 起被 Apple 标记为 deprecated，恢复请从 git 历史取回。
// 阶段 0（2026-10）：核心逻辑（扫描 / 安全闸门 / 删除 / 备份 / 启动项 /
// 优化 / 调度 / 平台抽象 / i18n）已抽到独立 crate `maclean-core`。
// 这里把 core 的模块在 crate root 做同名再导入，bin 内历史路径
// `crate::logger` / `crate::scanner` / `crate::safety` 等全部继续可用。
// 注意 `ops` 不在此列：bin 有本地薄壳模块 `src/ops/mod.rs`（re-export
// core::ops 全部 + 保留 App 耦合的扫描编排）。
#[allow(unused_imports)]
use maclean_core::{
    app_protection, backup, config, i18n, logger, platform, rules, safety, scanner, scheduler,
};

mod app;
mod cli;
mod license;
mod ops;
mod touchid;
mod updater;

// GUI 壳（egui/eframe）：默认启用；`--no-default-features` 裁剪为纯 CLI。
#[cfg(feature = "gui")]
mod icons;
#[cfg(feature = "gui")]
mod menubar;
#[cfg(feature = "gui")]
mod theme;
#[cfg(feature = "gui")]
mod ui;
#[cfg(feature = "gui")]
mod widgets;

// macOS 专属：sudo 保活（core 不依赖，但 bin 删除流程会经 core ops 回调到它）。
#[cfg(target_os = "macos")]
mod sudo_keepalive;

#[cfg(feature = "gui")]
use crate::ui::Gui;

/// 写入扫描日志（用于追踪扫描进度，崩溃时定位问题）
///
/// bin 壳入口，实际记录由 `maclean_core::logger` 完成；core 内扫描器
/// 使用的是 `maclean_core::log_scan_step`，两条路径写同一个日志文件。
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

fn main() {
    // 优先加载 .env（开发模式 MACLEAN_DEV=1 等配置）
    load_dotenv();

    // 初始化日志系统（CLI 和 GUI 模式都需要）
    logger::init();

    // CLI 模式：有子命令时执行并退出（带语义退出码），无子命令时启动 GUI
    if let Some(code) = cli::run_cli() {
        logger::info("CLI 模式执行完毕，退出");
        std::process::exit(code as i32);
    }

    #[cfg(feature = "gui")]
    run_gui();

    // 纯 CLI 构建（--no-default-features）没有 GUI 可兜底：
    // 未带子命令视为用法错误，给出提示并返回通用失败码。
    #[cfg(not(feature = "gui"))]
    {
        eprintln!("未指定子命令。当前为纯 CLI 构建（GUI 已裁剪），可用命令见 `maclean --help`。");
        std::process::exit(1);
    }
}

/// GUI 入口（egui/eframe）。`--no-default-features` 裁剪后此函数不存在。
#[cfg(feature = "gui")]
fn run_gui() {
    logger::info("GUI 模式启动");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 680.0])
            .with_min_inner_size([760.0, 540.0])
            .with_title("Maclean"),
        ..Default::default()
    };

    if let Err(e) = eframe::run_native(
        "Maclean",
        options,
        Box::new(|_cc| Ok(Box::new(Gui::new()) as Box<dyn eframe::App>)),
    ) {
        logger::error(&format!("GUI 启动失败: {}", e));
        eprintln!("GUI 启动失败: {}", e);
    }
}

/// 获取磁盘信息（跨平台入口）
///
/// 阶段 0 后平台实现已下沉到 `maclean_core::platform`（df / PowerShell），
/// bin 内历史调用路径保留。
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
