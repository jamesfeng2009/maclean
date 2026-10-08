//! 扫描项 / 扫描结果 —— 迁移自 maclean-core `scanner/mod.rs`。
//!
//! 字段与语义逐字保留（含 serde 默认值），保证与既有 JSON 输出兼容。

use super::recommend::Recommend;

/// 扫描项 - 表示一个可清理的文件或目录
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanItem {
    /// 完整路径（或快照名称/UUID）
    pub path: String,
    /// 大小（字节）
    pub size_bytes: u64,
    /// 分类名称，如 "Rust编译"、"Xcode编译"、"APFS快照" 等
    pub category: String,
    /// 是否被用户选中（用于 UI 交互）
    pub selected: bool,
    /// 是否可删除（部分系统级目录不可直接删除）
    pub deletable: bool,
    /// 不可删除的原因（deletable=false 时显示给用户）
    pub undeletable_reason: String,
    /// 推荐等级
    pub recommend: Recommend,
    /// 该项的说明（告诉用户这是什么，删除后有什么影响）
    pub description: String,
    /// 批量删除的真实路径列表（用于 __pycache__ 等聚合项）
    /// 为空表示单项删除，使用 path 字段
    pub batch_paths: Vec<String>,
    /// 批量路径的修改时间（unix 秒，与 `batch_paths` 一一对应，重复文件等场景展示用）
    #[serde(default)]
    pub batch_mtimes: Vec<i64>,
}

/// 扫描结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanResult {
    /// 所有扫描到的项目
    pub items: Vec<ScanItem>,
    /// 总大小（字节）
    pub total_size: u64,
    /// 扫描耗时（毫秒）
    pub scan_time_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 序列化兼容性基线：ScanItem 的 JSON 字段名与默认值不得漂移。
    /// 既有 CLI/UI 依赖这份形状（maclean scan --format json）。
    #[test]
    fn scan_item_json_shape_is_stable() {
        let item = ScanItem {
            path: "/tmp/x".into(),
            size_bytes: 42,
            category: "测试".into(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            recommend: Recommend::Safe,
            description: "d".into(),
            batch_paths: vec![],
            batch_mtimes: vec![],
        };
        let json = serde_json::to_value(&item).unwrap();
        let obj = json.as_object().unwrap();
        // 字段全集稳定（新增字段必须带 serde(default)，否则这里会红）
        assert_eq!(
            obj.keys().len(),
            10,
            "ScanItem 字段数漂移：{}",
            obj.keys().map(|k| k.as_str()).collect::<Vec<_>>().join(",")
        );
        for key in [
            "path",
            "size_bytes",
            "category",
            "selected",
            "deletable",
            "undeletable_reason",
            "recommend",
            "description",
            "batch_paths",
            "batch_mtimes",
        ] {
            assert!(obj.contains_key(key), "缺少字段 {key}");
        }
    }

    #[test]
    fn scan_item_roundtrip_preserves_fields() {
        let item = ScanItem {
            path: "/tmp/rt".into(),
            size_bytes: 7,
            category: "c".into(),
            selected: true,
            deletable: true,
            undeletable_reason: String::new(),
            recommend: Recommend::Caution,
            description: "desc".into(),
            batch_paths: vec!["/tmp/rt/a".into(), "/tmp/rt/b".into()],
            batch_mtimes: vec![1, 2],
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: ScanItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.path, item.path);
        assert_eq!(back.size_bytes, item.size_bytes);
        assert_eq!(back.recommend, item.recommend);
        assert_eq!(back.batch_paths, item.batch_paths);
        assert_eq!(back.batch_mtimes, item.batch_mtimes);
    }

    #[test]
    fn scan_item_missing_batch_mtimes_defaults_empty() {
        // 旧版本 JSON 可能没有 batch_mtimes 字段：serde(default) 保证向后兼容
        let json = r#"{"path":"/x","size_bytes":1,"category":"c","selected":false,
                      "deletable":true,"undeletable_reason":"","recommend":"Safe",
                      "description":"","batch_paths":[]}"#;
        let item: ScanItem = serde_json::from_str(json).unwrap();
        assert!(item.batch_mtimes.is_empty());
    }

    #[test]
    fn scan_result_roundtrip() {
        let r = ScanResult {
            items: vec![ScanItem::default()],
            total_size: 100,
            scan_time_ms: 5,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ScanResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_size, 100);
        assert_eq!(back.items.len(), 1);
    }
}
