//! 语义退出码（供脚本/CI 消费，稳定契约）。
//!
//! 迁移自 maclean 根包 `src/cli.rs`；**数值以 `docs/cli-contract.md` §4
//! 为唯一事实来源**（E-01）。契约保留位：2 = 参数错误（clap 默认）、
//! 7 = Ctrl+C 硬中断，二者不由本模块常量表达。

/// 成功
pub const EXIT_OK: u8 = 0;
/// 通用失败
pub const EXIT_FAILURE: u8 = 1;
/// 需确认（非交互环境执行删除未带 --yes）
pub const EXIT_CONFIRM_REQUIRED: u8 = 3;
/// 用户取消（Ctrl+C 收敛到此码，见契约 §4）
pub const EXIT_CANCELLED: u8 = 4;
/// 带警告完成（clean 有失败/被拦截项）
pub const EXIT_WARNINGS: u8 = 5;
/// 输出序列化失败（E-01：不再占用 2，避免与 clap 参数错误撞车）
pub const EXIT_SERIALIZE: u8 = 6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_stable_contract() {
        // E-01：脚本/CI 依赖这些数值，改动即破坏契约（以 cli-contract.md §4 为准）
        assert_eq!(EXIT_OK, 0);
        assert_eq!(EXIT_FAILURE, 1);
        assert_eq!(EXIT_CONFIRM_REQUIRED, 3);
        assert_eq!(EXIT_CANCELLED, 4);
        assert_eq!(EXIT_WARNINGS, 5);
        assert_eq!(EXIT_SERIALIZE, 6);
        // 互不相同，且不得占用契约保留位 2（clap 参数错误）/ 7（Ctrl+C 硬中断）
        let mut seen = std::collections::HashSet::new();
        for c in [
            EXIT_OK,
            EXIT_FAILURE,
            EXIT_CONFIRM_REQUIRED,
            EXIT_CANCELLED,
            EXIT_WARNINGS,
            EXIT_SERIALIZE,
        ] {
            assert!(seen.insert(c), "退出码重复: {c}");
            assert_ne!(c, 2, "2 是 clap 参数错误保留位");
            assert_ne!(c, 7, "7 是 Ctrl+C 硬中断保留位");
        }
    }
}
