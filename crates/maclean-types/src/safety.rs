//! 安全闸门决策类型。
//!
//! 迁移自 maclean-core `src/safety.rs`（值语义逐字保留）。
//!
//! 安全是公开信任层（strategy §12.1/§52）：用户应当能检查 maclean
//! 如何防止破坏性行为。删除链必须经过安全闸门：

//! ```text
//! Scanner → Candidate → SafetyGate → Executor
//! ```

/// 安全检查结果
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SafetyCheck {
    /// 安全，可以删除
    Safe,
    /// 危险，拒绝删除，附带原因
    Danger(String),
    /// 警告，需要额外确认
    Warning(String),
}

impl SafetyCheck {
    /// 是否允许执行（Safe 才放行；Warning/Danger 一律不放行）
    pub fn is_safe(&self) -> bool {
        matches!(self, SafetyCheck::Safe)
    }

    /// 是否需要用户额外确认（Warning）
    pub fn needs_confirmation(&self) -> bool {
        matches!(self, SafetyCheck::Warning(_))
    }

    /// 简要原因（Danger/Warning 时存在）
    pub fn reason(&self) -> Option<&str> {
        match self {
            SafetyCheck::Safe => None,
            SafetyCheck::Danger(r) | SafetyCheck::Warning(r) => Some(r),
        }
    }
}

/// 安全闸门：任何删除/破坏性操作在**执行前**必须通过 `validate`。
///
/// 安全不变量（strategy §52）：
/// - 闸门是执行链路上的独立检查，**不**作为 GUI/CLI 的可选开关存在；
/// - `validate` 返回非 `Safe` 时，调用方不得执行删除；
/// - 绕过 license 最多解锁功能，绝不能绕过安全闸门（strategy §66）。
pub trait SafetyGate {
    /// 对清理候选做删除前校验，返回决策
    fn validate(&self, candidate: &crate::domain::CleanupCandidate) -> SafetyCheck;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CleanupAction, CleanupCandidate, StorageEntity};

    #[test]
    fn safety_check_semantics() {
        assert!(SafetyCheck::Safe.is_safe());
        assert!(!SafetyCheck::Danger("受保护路径".into()).is_safe());
        assert!(!SafetyCheck::Warning("需要确认".into()).is_safe());
        assert!(SafetyCheck::Warning("x".into()).needs_confirmation());
        assert_eq!(SafetyCheck::Danger("原因".into()).reason(), Some("原因"));
        assert_eq!(SafetyCheck::Safe.reason(), None);
    }

    #[test]
    fn safety_check_serializes_roundtrip() {
        let json = serde_json::to_string(&SafetyCheck::Danger("blocked".into())).unwrap();
        let back: SafetyCheck = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SafetyCheck::Danger("blocked".into()));
        // 与既有枚举形状一致（externally tagged）
        assert!(json.contains("Danger"));
    }

    /// 全拒闸门：验证 trait 能被实现并用于门禁
    struct RejectAllGate;
    impl SafetyGate for RejectAllGate {
        fn validate(&self, _candidate: &CleanupCandidate) -> SafetyCheck {
            SafetyCheck::Danger("测试闸门：一律拒绝".into())
        }
    }

    fn candidate() -> CleanupCandidate {
        CleanupCandidate {
            entity: StorageEntity {
                path: "/tmp/x".into(),
                ..Default::default()
            },
            safety: SafetyCheck::Safe,
            reclaimable_bytes: 1,
            action: CleanupAction::Delete,
        }
    }

    #[test]
    fn safety_gate_trait_is_implementable_and_enforced() {
        let gate = RejectAllGate;
        let c = candidate();
        let decision = gate.validate(&c);
        assert!(!decision.is_safe(), "闸门拒绝时必须拦截");
        assert!(matches!(decision, SafetyCheck::Danger(_)));
    }
}
