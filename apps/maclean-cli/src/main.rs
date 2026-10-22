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

fn main() {
    // 初始化日志系统（CLI 与后台任务共用）
    logger::init();

    // CLI 分发：所有子命令经 maclean-cli-contract crate 处理，返回语义退出码。
    // run_cli() 返回 Some(code) 表示已执行并应退出；None 表示未提供子命令。
    if let Some(code) = maclean_cli_contract::run_cli() {
        std::process::exit(code as i32);
    }

    // 未指定子命令：本构建是纯 CLI（无 GUI 可兜底），视为用法错误。
    eprintln!(
        "未指定子命令。可用命令见 `maclean --help`。\n\
         提示：maclean CLI 永久免费；GUI App 提供 14 天试用，见官网。"
    );
    std::process::exit(1);
}
