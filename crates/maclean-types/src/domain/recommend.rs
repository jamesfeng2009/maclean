//! 推荐等级 —— 帮助用户判断是否应该清理。
//!
//! 迁移自 maclean-core（原 `crates/maclean-core/src/scanner/mod.rs`），
//! 字段与语义逐字保留，保证序列化兼容。

/// 推荐等级 - 帮助用户判断是否应该清理
///
/// `Default` 派生自 `Safe`（clippy 1.99 的 `derivable_impls` 要求；
/// 原先手写的 `impl Default` 与 derive 完全等价）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum Recommend {
    /// 推荐清理 - 安全可删，重新构建/使用时会自动恢复
    #[default]
    Safe,
    /// 应用缓存 - 只含缓存/日志，删除后应用可正常运行并自动重建
    CacheOnly,
    /// 谨慎清理 - 删除后可能需要重新下载或配置
    Caution,
    /// 高级用户 - 需要了解风险后自行判断
    Advanced,
}

impl Recommend {
    /// 是否默认被"智能选择"勾选
    pub fn default_selected(self) -> bool {
        matches!(self, Recommend::Safe | Recommend::CacheOnly)
    }
}
