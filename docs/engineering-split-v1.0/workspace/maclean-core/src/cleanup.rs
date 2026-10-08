use maclean_types::CleanupCandidate;

pub struct CleanupPlan {
    pub candidates: Vec<CleanupCandidate>,
}

pub trait CleanupExecutor {
    fn preview(&self, candidates: &[CleanupCandidate]) -> CleanupPlan;
}
