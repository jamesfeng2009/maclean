//! 标准化领域模型（Open Core 契约，strategy §19 / split `05_PUBLIC_CORE_CONTRACTS.md`）。
//!
//! 六个核心模型：
//! - [`StorageEntity`]：一个被识别的存储对象（文件/目录/快照/聚合项）；
//! - [`StorageObservation`]：一次扫描观测（某时刻的实体集合与总量）；
//! - [`DeveloperProject`]：开发者项目（工具链、产物、生命周期信息）；
//! - [`CleanupCandidate`]：清理候选（实体 + 安全决策 + 可释放量）；
//! - [`CleanupPolicy`]：清理策略（筛选条件 + 动作）；
//! - [`CleanupRun`]：一次清理执行记录。
//!
//! 这些模型是**契约**：公开 API 描述"引擎能表示什么"（what），
//! 私有层拥有"如何推导商业智能"（how）。契约保持稳定，实现自由演进。

use super::recommend::Recommend;
use super::scan::ScanItem;
use crate::safety::SafetyCheck;

/// 一个被识别的存储对象。
///
/// 与既有 [`ScanItem`] 语义对齐，并提供双向转换。它是扫描/观测/清理链路中
/// 的标准实体表示：既有 UI/CLI 输出继续使用 `ScanItem`，新契约层统一使用
/// `StorageEntity`。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StorageEntity {
    /// 完整路径（或快照名称/UUID）
    pub path: String,
    /// 大小（字节）
    pub size_bytes: u64,
    /// 分类名称，如 "Rust编译"、"Xcode编译"、"APFS快照"
    pub category: String,
    /// 推荐等级
    pub recommend: Recommend,
    /// 说明（告诉用户这是什么、删除后有什么影响）
    pub description: String,
    /// 是否可删除（部分系统级目录不可直接删除）
    pub deletable: bool,
    /// 不可删除的原因
    pub undeletable_reason: String,
    /// 批量删除的真实路径列表（聚合项）；空 = 单项，使用 path
    #[serde(default)]
    pub batch_paths: Vec<String>,
    /// 批量路径的修改时间（unix 秒，与 batch_paths 一一对应）
    #[serde(default)]
    pub batch_mtimes: Vec<i64>,
}

impl From<&ScanItem> for StorageEntity {
    fn from(item: &ScanItem) -> Self {
        Self {
            path: item.path.clone(),
            size_bytes: item.size_bytes,
            category: item.category.clone(),
            recommend: item.recommend,
            description: item.description.clone(),
            deletable: item.deletable,
            undeletable_reason: item.undeletable_reason.clone(),
            batch_paths: item.batch_paths.clone(),
            batch_mtimes: item.batch_mtimes.clone(),
        }
    }
}

impl From<StorageEntity> for ScanItem {
    fn from(e: StorageEntity) -> Self {
        Self {
            path: e.path,
            size_bytes: e.size_bytes,
            category: e.category,
            selected: false,
            deletable: e.deletable,
            undeletable_reason: e.undeletable_reason,
            recommend: e.recommend,
            description: e.description,
            batch_paths: e.batch_paths,
            batch_mtimes: e.batch_mtimes,
        }
    }
}

/// 一次扫描观测：某时刻对存储的一份快照。
///
/// 开源核心提供基础快照；私有层在此基础上做增长建模 / 预测 / 异常检测
/// （strategy §13.2：growth model / low-space prediction 属商业智能）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StorageObservation {
    /// 观测 ID（uuid）
    pub id: String,
    /// 观测时间（unix 秒）
    pub observed_at_unix: i64,
    /// 观测到的实体
    pub entities: Vec<StorageEntity>,
    /// 总占用（字节）
    pub total_bytes: u64,
    /// 观测来源（如 "full-scan" / "quick-scan" / "cli-scan"）
    pub source: String,
}

/// 开发者项目：工具链、产物与生命周期信息。
///
/// 基础项目发现（找到 Xcode / Cargo / npm / venv 等项目目录）属公开核心；
/// 进阶项目图谱（依赖关系、增长归因、风险排序）属商业智能（strategy §13）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeveloperProject {
    /// 项目根路径
    pub path: String,
    /// 项目名（目录名或清单名）
    pub name: String,
    /// 检测到的工具链（如 "xcode", "cargo", "npm", "venv"）
    pub toolchains: Vec<String>,
    /// 项目总占用（字节）
    pub size_bytes: u64,
    /// 上次使用时间（unix 秒，0 = 未知）
    #[serde(default)]
    pub last_used_at_unix: i64,
    /// 是否可重建（true = 产物可再生）
    #[serde(default)]
    pub regenerable: bool,
}

/// 清理候选：一个实体加上安全决策与可释放量。
///
/// 基础候选生成（扫描 → 可删除项）属公开核心；风险调整后的回收评分 /
/// 优先级排序属商业智能（strategy §13.1）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CleanupCandidate {
    /// 候选实体
    pub entity: StorageEntity,
    /// 安全闸门决策（删除前必须再次验证）
    pub safety: SafetyCheck,
    /// 预计可释放字节（可能小于 entity.size_bytes，如部分成员被保护）
    pub reclaimable_bytes: u64,
    /// 建议动作：delete（永久） / trash（废纸篓） / skip
    pub action: CleanupAction,
}

/// 候选动作
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CleanupAction {
    /// 永久删除（仅安全/缓存类）
    Delete,
    /// 移入废纸篓（可还原）
    Trash,
    /// 跳过
    Skip,
}

impl Default for CleanupAction {
    /// 默认不执行任何破坏性动作（策略未显式声明时保持安全）
    fn default() -> Self {
        Self::Skip
    }
}

/// 清理策略：筛选条件 + 动作（公开的本地策略契约）。
///
/// 策略验证 / 候选选择优化 / 冲突消解 / 自动推荐属商业智能
/// （strategy §13.3，私有 Pro 策略实现）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CleanupPolicy {
    /// 策略名
    pub name: String,
    /// 命中分类（空 = 全部）
    pub categories: Vec<String>,
    /// 最低推荐等级门槛（只处理 >= 该等级的候选）
    #[serde(default)]
    pub min_recommend: Recommend,
    /// 动作（Delete / Trash / Skip）
    #[serde(default)]
    pub action: CleanupAction,
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
}

/// 一次清理执行记录。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CleanupRun {
    /// 执行记录 ID（uuid）
    pub id: String,
    /// 开始时间（unix 秒）
    pub started_at_unix: i64,
    /// 结束时间（unix 秒）
    pub finished_at_unix: i64,
    /// 关联策略名
    pub policy: String,
    /// 候选清单（含各自安全决策）
    pub candidates: Vec<CleanupCandidate>,
    /// 实际释放字节
    pub freed_bytes: u64,
    /// 被安全闸门拦截的候选数
    pub blocked_count: u32,
    /// 是否全部成功
    pub ok: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_entity_from_scan_item_maps_all_fields() {
        let item = ScanItem {
            path: "/tmp/e".into(),
            size_bytes: 1024,
            category: "缓存".into(),
            selected: true,
            deletable: true,
            undeletable_reason: String::new(),
            recommend: Recommend::CacheOnly,
            description: "可重建".into(),
            batch_paths: vec!["/tmp/e/a".into()],
            batch_mtimes: vec![9],
        };
        let e = StorageEntity::from(&item);
        assert_eq!(e.path, "/tmp/e");
        assert_eq!(e.size_bytes, 1024);
        assert_eq!(e.recommend, Recommend::CacheOnly);
        assert_eq!(e.batch_paths, vec!["/tmp/e/a"]);
        assert_eq!(e.batch_mtimes, vec![9]);
        // 双向转换不丢信息（selected 在实体层无意义，转回时归 false）
        let back = ScanItem::from(e);
        assert_eq!(back.path, "/tmp/e");
        assert_eq!(back.recommend, Recommend::CacheOnly);
        assert_eq!(back.batch_paths, vec!["/tmp/e/a"]);
    }

    #[test]
    fn candidate_and_run_serialize() {
        let cand = CleanupCandidate {
            entity: StorageEntity {
                path: "/tmp/c".into(),
                size_bytes: 5,
                category: "c".into(),
                recommend: Recommend::Safe,
                description: String::new(),
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: vec![],
                batch_mtimes: vec![],
            },
            safety: SafetyCheck::Safe,
            reclaimable_bytes: 5,
            action: CleanupAction::Delete,
        };
        let run = CleanupRun {
            id: "run-1".into(),
            started_at_unix: 1,
            finished_at_unix: 2,
            policy: "default".into(),
            candidates: vec![cand],
            freed_bytes: 5,
            blocked_count: 0,
            ok: true,
        };
        let json = serde_json::to_string(&run).unwrap();
        let back: CleanupRun = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "run-1");
        assert_eq!(back.candidates.len(), 1);
        assert!(matches!(back.candidates[0].safety, SafetyCheck::Safe));
        assert_eq!(back.candidates[0].action, CleanupAction::Delete);
    }

    #[test]
    fn observation_and_policy_roundtrip() {
        let obs = StorageObservation {
            id: "obs-1".into(),
            observed_at_unix: 123,
            entities: vec![StorageEntity {
                path: "/x".into(),
                ..Default::default()
            }],
            total_bytes: 10,
            source: "cli-scan".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let back: StorageObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entities.len(), 1);
        assert_eq!(back.source, "cli-scan");

        let p = CleanupPolicy {
            name: "daily".into(),
            categories: vec!["缓存".into()],
            min_recommend: Recommend::Safe,
            action: CleanupAction::Trash,
            enabled: true,
        };
        let pj = serde_json::to_string(&p).unwrap();
        let pb: CleanupPolicy = serde_json::from_str(&pj).unwrap();
        assert_eq!(pb.name, "daily");
        assert_eq!(pb.action, CleanupAction::Trash);
    }

    #[test]
    fn project_model_roundtrip() {
        let proj = DeveloperProject {
            path: "/dev/app".into(),
            name: "app".into(),
            toolchains: vec!["cargo".into(), "xcode".into()],
            size_bytes: 99,
            last_used_at_unix: 0,
            regenerable: true,
        };
        let json = serde_json::to_string(&proj).unwrap();
        let back: DeveloperProject = serde_json::from_str(&json).unwrap();
        assert_eq!(back.toolchains.len(), 2);
        assert!(back.regenerable);
    }
}
