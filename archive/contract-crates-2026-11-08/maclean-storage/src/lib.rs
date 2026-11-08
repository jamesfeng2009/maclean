//! # maclean-storage
//!
//! 本地持久化层（Open Core 公开部分）。
//!
//! 2026-10-07 落地决策：当前 maclean 的持久化是 **JSON 文件**（`config.json` /
//! `apps.json` / 扫描结果缓存），无 SQLite。本 crate 提供：
//!
//! - [`json_store`]：原子 JSON 文件读写（temp + rename，崩溃不写坏文件）；
//! - [`migrations`]：`schema_version` 驱动的迁移框架；
//! - [`repositories`]：仓储层（扫描历史 / 配置等结构化读写）。
//!
//! 规则（strategy §20）：隐藏私有语义，不隐藏普通数据库工程。核心 schema
//! 公开，未来若引入 SQLite，`repositories` 接口保持不变、实现替换即可。
//!
//! ## 许可
//!
//! Apache-2.0。

pub mod json_store;
pub mod migrations;
pub mod repositories;
