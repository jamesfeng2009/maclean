//! # maclean-cli
//!
//! CLI 层（Open Core 公开部分）：命令解析 + 分发 + 输出渲染。
//!
//! 分层（split 03 Phase 6）：CLI → service(core) → platform。
//! 本 crate 的 handler 只做参数解析与渲染编排，**不含任何文件系统删除
//! 实现**；删除/扫描/安全闸门全部在 [`maclean_core`]。
//!
//! 公开命令集（strategy §21）：
//!
//! ```text
//! maclean scan / clean / schedule / check-disk / list / apps / uninstall
//! maclean log / backups / restore / startup / optimize / dup-ignore
//! maclean scan --format json / jsonl   （envelope 结构化输出，稳定契约）
//! ```
//!
//! Pro 命令（history / growth / forecast / policy / automation）只保留
//! 契约占位（可解析、返回错误信封），实现留在私有 Pro 仓库。

pub mod args;
pub mod handler;
pub mod output;

pub use handler::{format_timestamp, install_cancel_handler, run_cli};
pub use maclean_types::cli::{
    ColorMode, OutputFormat, EXIT_CANCELLED, EXIT_CONFIRM_REQUIRED, EXIT_FAILURE, EXIT_OK,
    EXIT_SERIALIZE, EXIT_WARNINGS,
};
