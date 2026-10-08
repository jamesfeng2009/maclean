//! # maclean-platform
//!
//! 平台适配器契约层（Open Core 公开部分）。
//!
//! maclean 需要跨平台（macOS / Windows）能力：废纸篓、路径安全、
//! 注册表读取、磁盘信息。本 crate 定义这些能力的 **trait 契约**与
//! 可独立测试的**纯逻辑**，OS 差异实现由各平台适配器提供。
//!
//! ## 2026-10-07 落地决策
//!
//! - `apfs.rs` / `windows_apps.rs`（core 内扫描器）**暂保留在 maclean-core**：
//!   深度耦合 core 的 IO 基建与安全闸门，迁移期拆分无收益；本 crate 先立契约，
//!   后续按需迁移实现（tasks.md P5-2/P5-3）。
//! - 本 crate 不触碰扫描 / 删除业务逻辑。
//!
//! ## 模块
//!
//! - [`trash`]：废纸篓契约（move / restore）；
//! - [`path`]：路径安全纯逻辑（symlink / junction 检测、挂载点判定）；
//! - [`registry`]：Windows 注册表读取契约；
//! - [`disk`]：磁盘信息契约。
//!
//! ## 许可
//!
//! Apache-2.0。

pub mod disk;
pub mod path;
pub mod registry;
pub mod trash;
