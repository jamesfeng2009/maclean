//! maclean — 永久免费开源的 macOS 磁盘清理 CLI
//!
//! 本二进制是 maclean 公开仓库（github.com/jamesfeng2009/maclean）的
//! **唯一发行产物**。产品分层（DECISION BASELINE 2026-10-13）：
//!
//! - **maclean CLI**（本程序）：永久免费 · Apache-2.0 开源，适合脚本、
//!   CI/CD、定时任务和自动化工作流；扫描默认只读，删除前预览 + 确认。
//! - **maclean App**（GUI）：14 天完整试用，试用后付费解锁；位于私有
//!   maclean-pro 仓库，与 CLI 共享 `maclean-core` 引擎，但不共享本二进制。
//!
//! 独立性契约（MAC-CLI-001/004）：
//! - 不依赖 GUI 库、授权服务器、商业许可证或任何付费功能模块；
//! - 无许可证、无账号、无网络时均可安装、运行、升级；
//! - 试用过期 / 服务器宕机 / 付款失败均不影响本程序。
//!
//! 命令集合（`maclean --help` 查看完整列表）：
//! ```text
//! maclean scan / clean / schedule / check-disk / list / apps / uninstall
//! maclean log / backups / restore / startup / optimize / dup-ignore
//! ```
//! 结构化输出：`--format json|jsonl`，配合稳定退出码供脚本消费。

use maclean_core::logger;

/// 匿名首跑上报（MAC-TELE-001）
///
/// 目的：Homebrew 安装无公开下载计数，用 CLI 首次运行一次匿名打点作为
/// 「真实安装/使用」代理口径（私有看板 brew 板块数据源）。
///
/// 设计约束（延续 CLI 独立性契约，不破坏 MAC-CLI-001/004）：
/// - 仅首跑上报一次（`~/.maclean/telemetry.json` 标记）；
/// - 匿名：随机 UUID，不含路径/主机名/邮箱/命令参数；
/// - 完全可关：`MACLEAN_NO_TELEMETRY=1` 环境变量；
/// - 失败静默：网络不可用 / 超时 / 服务器宕机均不影响 CLI 主流程；
/// - 独立线程 + 3s 超时，绝不阻塞命令执行。
mod telemetry {
    use serde::{Deserialize, Serialize};
    use std::{env, fs, path::PathBuf};

    #[derive(Serialize, Deserialize)]
    struct TelemetryState {
        id: String,
    }

    const MARKER: &str = ".maclean/telemetry.json";
    const ENDPOINT: &str = "https://stats.getmaclean.com/t";

    pub fn report_once(version: &str) {
        if env::var_os("MACLEAN_NO_TELEMETRY").is_some() {
            return;
        }
        let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let marker = home.join(MARKER);

        // 已上报过 → 跳过（每台机器/用户只打一次点）
        if fs::read_to_string(&marker)
            .ok()
            .and_then(|s| serde_json::from_str::<TelemetryState>(&s).ok())
            .is_some()
        {
            return;
        }

        let id = uuid::Uuid::new_v4().to_string();
        let os = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "linux"
        };
        let body = serde_json::json!({ "id": id, "os": os, "version": version });
        let marker_for_thread = marker.clone();

        // 独立线程上报；无论成败都落标记，避免每次运行重试
        std::thread::spawn(move || {
            let _ = ureq::post(ENDPOINT)
                .timeout(std::time::Duration::from_secs(3))
                .send_json(body);
            if let Some(dir) = marker_for_thread.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(
                &marker_for_thread,
                serde_json::to_string(&TelemetryState { id }).unwrap_or_default(),
            );
        });
    }
}

fn main() {
    // 初始化日志系统（CLI 与后台任务共用）；横幅展示 CLI 自身版本
    logger::init_with_version(env!("CARGO_PKG_VERSION"));

    // 匿名首跑上报（brew 安装代理口径；静默、可关、单线程不阻塞）
    telemetry::report_once(env!("CARGO_PKG_VERSION"));

    // CLI 分发：所有子命令经 maclean-cli-contract crate 处理，返回语义退出码。
    // run_cli() 返回 Some(code) 表示已执行并应退出；None 表示未提供子命令。
    if let Some(code) = maclean_cli_contract::run_cli_with_version(env!("CARGO_PKG_VERSION")) {
        std::process::exit(code as i32);
    }

    // 未指定子命令：本构建是纯 CLI（无 GUI 可兜底），视为用法错误。
    eprintln!(
        "未指定子命令。可用命令见 `maclean --help`。\n\
         提示：maclean CLI 永久免费；GUI App 提供 14 天试用，见官网。"
    );
    std::process::exit(1);
}
