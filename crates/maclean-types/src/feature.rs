//! 产品功能门控（strategy §16）。
//!
//! 门属于应用/服务层：公开核心定义 [`Feature`] 与 [`EntitlementProvider`] 契约，
//! Free / Pro / Team 各自提供实现。**权威的 entitlement 决策不得只存在于 GUI 代码**。
//!
//! 安全不变量（strategy §66）：绕过 license 最多解锁功能（商业损失），
//! 绝不能改变文件系统安全行为 —— 删除链始终由 `SafetyCheck` 闸门独立把关。

use serde::{Deserialize, Serialize};

/// 产品功能标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Feature {
    /// 基础扫描（公开）
    BasicScan,
    /// 基础清理（公开）
    BasicCleanup,
    /// 进阶历史（商业）
    AdvancedHistory,
    /// 存储预测（商业）
    Forecast,
    /// 智能清理（商业）
    SmartCleanup,
    /// 定时自动化（商业）
    ScheduledAutomation,
    /// 高级规则包（商业）
    PremiumRules,
    /// AI 存储解释（商业）
    AiExplanation,
}

impl Feature {
    /// 该功能是否属于公开 Open Core（Free 可用）
    pub fn is_open_core(self) -> bool {
        matches!(self, Feature::BasicScan | Feature::BasicCleanup)
    }

    /// 稳定的字符串标识（license payload 与 CLI 共用）
    pub fn as_str(self) -> &'static str {
        match self {
            Feature::BasicScan => "basic_scan",
            Feature::BasicCleanup => "basic_cleanup",
            Feature::AdvancedHistory => "advanced_history",
            Feature::Forecast => "forecast",
            Feature::SmartCleanup => "smart_cleanup",
            Feature::ScheduledAutomation => "automation",
            Feature::PremiumRules => "premium_rules",
            Feature::AiExplanation => "ai_explanation",
        }
    }
}

/// 授权提供者：决定某个功能是否可用。
pub trait EntitlementProvider {
    /// 是否允许使用该功能
    fn allows(&self, feature: Feature) -> bool;
}

/// Free 授权：只开放 Open Core 功能。
#[derive(Debug, Clone, Copy, Default)]
pub struct FreeEntitlementProvider;

impl EntitlementProvider for FreeEntitlementProvider {
    fn allows(&self, feature: Feature) -> bool {
        feature.is_open_core()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_identifiers_are_stable() {
        assert_eq!(Feature::BasicScan.as_str(), "basic_scan");
        assert_eq!(Feature::AdvancedHistory.as_str(), "advanced_history");
        assert_eq!(Feature::ScheduledAutomation.as_str(), "automation");
        assert_eq!(Feature::AiExplanation.as_str(), "ai_explanation");
        // 与 strategy §18 license payload 的 features 字段对齐
        let payload_features = [
            "advanced_history",
            "forecast",
            "smart_cleanup",
            "automation",
        ];
        for f in payload_features {
            assert!(
                Feature::BasicScan.as_str() != f
                    && Feature::BasicCleanup.as_str() != f
                    && Feature::BasicScan.as_str() != f,
                "重复标识: {f}"
            );
        }
    }

    #[test]
    fn free_provider_only_opens_core() {
        let free = FreeEntitlementProvider;
        assert!(free.allows(Feature::BasicScan));
        assert!(free.allows(Feature::BasicCleanup));
        assert!(!free.allows(Feature::Forecast));
        assert!(!free.allows(Feature::SmartCleanup));
        assert!(!free.allows(Feature::AiExplanation));
        assert!(!free.allows(Feature::PremiumRules));
    }

    #[test]
    fn open_core_classification_matches_strategy() {
        // strategy §82 决策矩阵：scanner/safety/cleanup 公开；intelligence 私有
        assert!(Feature::BasicScan.is_open_core());
        assert!(Feature::BasicCleanup.is_open_core());
        assert!(!Feature::AdvancedHistory.is_open_core());
        assert!(!Feature::Forecast.is_open_core());
        assert!(!Feature::ScheduledAutomation.is_open_core());
    }

    #[test]
    fn feature_serializes_roundtrip() {
        let json = serde_json::to_string(&Feature::SmartCleanup).unwrap();
        let back: Feature = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Feature::SmartCleanup);
    }
}
