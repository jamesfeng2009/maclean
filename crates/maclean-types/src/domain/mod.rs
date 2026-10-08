//! 领域模型 —— 扫描项 / 观测 / 项目 / 清理候选（公开契约）

pub mod models;
pub mod recommend;
pub mod scan;

pub use models::{
    CleanupAction, CleanupCandidate, CleanupPolicy, CleanupRun, DeveloperProject, StorageEntity,
    StorageObservation,
};
pub use recommend::Recommend;
pub use scan::{ScanItem, ScanResult};
