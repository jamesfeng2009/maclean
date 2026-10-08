//! schema_version 驱动的迁移框架。
//!
//! 每个存储 key 附带 `schema_version`；读取时若版本低于当前，按序执行迁移。
//! 迁移只做**结构化数据**的升级（字段改名/默认值），不触碰任何文件内容。

use serde::{Deserialize, Serialize};

/// 迁移错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    /// 存储中的版本高于当前代码（降级读取，拒绝）
    VersionAhead { found: u32, current: u32 },
    /// 迁移步骤失败
    StepFailed { from: u32, to: u32, reason: String },
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::VersionAhead { found, current } => write!(
                f,
                "存储版本 {found} 高于当前代码版本 {current}（请升级 maclean）"
            ),
            MigrationError::StepFailed { from, to, reason } => {
                write!(f, "迁移 {from} → {to} 失败: {reason}")
            }
        }
    }
}

/// 一个迁移步骤：把版本 `start` 的数据升级到 `start + 1`。
pub trait Migration {
    fn start_version(&self) -> u32;
    /// 对原始 JSON 值做升级（返回升级后的值）
    fn upgrade(&self, value: serde_json::Value) -> Result<serde_json::Value, String>;
}

/// 版本化文档：任何可持久化对象包一层版本号。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Versioned<T> {
    pub schema_version: u32,
    #[serde(flatten)]
    pub payload: T,
}

impl<T> Versioned<T> {
    pub fn new(payload: T) -> Self {
        Self {
            schema_version: 1,
            payload,
        }
    }
}

/// 按迁移表把 JSON 值升级到 `target_version`。
///
/// - `value` 中无 `schema_version`：视为版本 1；
/// - 版本 > target：返回 [`MigrationError::VersionAhead`]；
/// - 版本 < target：按 `migrations` 表逐级升级。
pub fn migrate_json(
    value: serde_json::Value,
    migrations: &[Box<dyn Migration>],
    target_version: u32,
) -> Result<serde_json::Value, MigrationError> {
    let mut v = value;
    let mut version = v
        .get("schema_version")
        .and_then(|x| x.as_u64())
        .unwrap_or(1) as u32;

    if version > target_version {
        return Err(MigrationError::VersionAhead {
            found: version,
            current: target_version,
        });
    }

    while version < target_version {
        let next = migrations
            .iter()
            .find(|m| m.start_version() == version)
            .ok_or_else(|| MigrationError::StepFailed {
                from: version,
                to: version + 1,
                reason: format!("缺少从版本 {version} 出发的迁移步骤"),
            })?;
        let upgraded = next
            .upgrade(v.clone())
            .map_err(|reason| MigrationError::StepFailed {
                from: version,
                to: version + 1,
                reason,
            })?;
        v = upgraded;
        version += 1;
    }

    // 升级后写回最新版本号
    if let Some(obj) = v.as_object_mut() {
        obj.insert("schema_version".into(), serde_json::json!(target_version));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 迁移步骤：v1 → v2 新增字段 `enabled`（默认 true）
    struct AddEnabled;
    impl Migration for AddEnabled {
        fn start_version(&self) -> u32 {
            1
        }
        fn upgrade(&self, value: serde_json::Value) -> Result<serde_json::Value, String> {
            let mut obj = value
                .as_object()
                .cloned()
                .ok_or_else(|| "不是对象".to_string())?;
            obj.entry("enabled").or_insert(serde_json::json!(true));
            Ok(serde_json::Value::Object(obj))
        }
    }

    fn migrations() -> Vec<Box<dyn Migration>> {
        vec![Box::new(AddEnabled)]
    }

    #[test]
    fn migrates_from_v1_to_target() {
        let value = serde_json::json!({ "schema_version": 1, "name": "x" });
        let out = migrate_json(value, &migrations(), 2).unwrap();
        assert_eq!(out["schema_version"], 2);
        assert_eq!(out["enabled"], true);
        assert_eq!(out["name"], "x");
    }

    #[test]
    fn missing_version_treated_as_v1() {
        let value = serde_json::json!({ "name": "x" });
        let out = migrate_json(value, &migrations(), 2).unwrap();
        assert_eq!(out["schema_version"], 2);
        assert_eq!(out["enabled"], true);
    }

    #[test]
    fn already_current_version_passes_through() {
        let value = serde_json::json!({ "schema_version": 2, "enabled": false });
        let out = migrate_json(value, &migrations(), 2).unwrap();
        assert_eq!(out["enabled"], false);
    }

    #[test]
    fn version_ahead_is_rejected() {
        let value = serde_json::json!({ "schema_version": 3 });
        let err = migrate_json(value, &migrations(), 2).unwrap_err();
        assert!(matches!(
            err,
            MigrationError::VersionAhead {
                found: 3,
                current: 2
            }
        ));
    }

    #[test]
    fn missing_migration_step_fails_loudly() {
        let value = serde_json::json!({ "schema_version": 1 });
        let err = migrate_json(value, &[], 2).unwrap_err();
        assert!(matches!(
            err,
            MigrationError::StepFailed { from: 1, to: 2, .. }
        ));
    }
}
