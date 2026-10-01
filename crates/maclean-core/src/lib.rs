//! # maclean-core
//!
//! maclean 的 UI 无关核心：扫描器、安全闸门（safety）、删除/提权执行（ops）、
//! 备份清单、启动项、系统优化、调度与跨平台抽象。
//!
//! - egui 壳（`maclean` bin）与 Tauri 壳（`maclean-tauri`）都只通过本 crate
//!   触达文件系统；
//! - **任何删除都必须经过 [`safety`] 闸门**，前端拿不到裸文件句柄；
//! - 本 crate 不依赖 egui / Web 技术，可在 CI / 容器中独立编译与测试。
//!
//! 阶段 0（2026-10）从原单 crate 抽取，行为与抽取前逐行一致。

pub mod app_protection;
pub mod backup;
pub mod config;
pub mod design_tokens;
pub mod i18n;
pub mod logger;
pub mod ops;
pub mod platform;
pub mod rules;
pub mod safety;
pub mod scanner;
pub mod scheduler;

/// 写入扫描日志（用于追踪扫描进度，崩溃时定位问题）
///
/// 与 egui 壳 bin 中同名函数保持同一实现：core 内扫描器（如大文件扫描）
/// 通过本函数记录进度，Tauri 壳同样能在日志文件中看到完整轨迹。
pub fn log_scan_step(msg: &str) {
    logger::info(msg);
}
