//! 语义退出码（供脚本/CI 消费，稳定契约）。
//!
//! 迁移自 maclean 根包 `src/cli.rs`，值保持不变。

/// 成功
pub const EXIT_OK: u8 = 0;
/// 通用失败
pub const EXIT_FAILURE: u8 = 1;
/// JSON 序列化失败（历史保留）
pub const EXIT_SERIALIZE: u8 = 2;
/// 需确认（非交互环境执行删除未带 --yes）
pub const EXIT_CONFIRM_REQUIRED: u8 = 4;
/// 用户取消（Ctrl+C）
pub const EXIT_CANCELLED: u8 = 7;
/// 带警告完成（clean 有失败/被拦截项）
pub const EXIT_WARNINGS: u8 = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_stable_contract() {
        // 脚本/CI 依赖这些数值，改动即破坏契约
        assert_eq!(EXIT_OK, 0);
        assert_eq!(EXIT_FAILURE, 1);
        assert_eq!(EXIT_SERIALIZE, 2);
        assert_eq!(EXIT_CONFIRM_REQUIRED, 4);
        assert_eq!(EXIT_CANCELLED, 7);
        assert_eq!(EXIT_WARNINGS, 8);
        // 互不相同
        let mut seen = std::collections::HashSet::new();
        for c in [
            EXIT_OK,
            EXIT_FAILURE,
            EXIT_SERIALIZE,
            EXIT_CONFIRM_REQUIRED,
            EXIT_CANCELLED,
            EXIT_WARNINGS,
        ] {
            assert!(seen.insert(c), "退出码重复: {c}");
        }
    }
}
