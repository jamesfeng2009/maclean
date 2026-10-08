use maclean_types::CleanupCandidate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyDecision {
    Allowed,
    Blocked,
    RequiresConfirmation,
}

pub trait SafetyGate {
    fn validate(&self, candidate: &CleanupCandidate) -> SafetyDecision;
}

pub struct DefaultSafetyGate;

impl SafetyGate for DefaultSafetyGate {
    fn validate(&self, candidate: &CleanupCandidate) -> SafetyDecision {
        if candidate.risk == maclean_types::RiskLevel::Destructive {
            SafetyDecision::RequiresConfirmation
        } else {
            SafetyDecision::Allowed
        }
    }
}
