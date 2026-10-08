//! # maclean-types
//!
//! maclean **公开契约层**（Open Core 公共边界）。
//!
//! 这是公开仓库中最底层的 crate：所有公开/私有代码都依赖它，但它不依赖任何
//! maclean 内部 crate，也不触碰文件系统 / 平台 API / 网络。
//!
//! ## 内容
//!
//! - [`domain`]：领域模型 —— 扫描项、观测、项目、清理候选/策略/执行记录；
//! - [`cli`]：CLI 契约 —— JSON envelope、语义退出码、输出格式；
//! - [`safety`]：安全闸门决策类型（[`SafetyCheck`]）；
//! - [`feature`]：产品功能门控（[`Feature`] + [`EntitlementProvider`]）。
//!
//! ## 兼容性承诺
//!
//! 本 crate 中的类型是稳定公开契约（strategy §19/§46）：一旦发布，
//! 破坏性变更必须升级 `contract_version` 并提供迁移路径。新增字段应带
//! `#[serde(default)]` 保持向后兼容。
//!
//! ## 许可
//!
//! Apache-2.0（Open Core 公开部分）。

pub mod cli;
pub mod domain;
pub mod feature;
pub mod safety;

/// 当前 CLI JSON 契约版本（strategy §47）。
///
/// 每次破坏性变更（字段改名 / 语义变化）必须递增；向后兼容的新增字段
/// 不要求递增。
pub const CONTRACT_VERSION: u32 = 1;
