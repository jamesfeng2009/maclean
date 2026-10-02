//! IM（即时通讯）应用数据的**只读**占用分析
//!
//! 背景：微信 / QQ 等 IM 把聊天记录、收发的图片视频与文件放在沙盒
//! `~/Library/Containers/<bundle>/Data/Documents` 下，体积可达数十 GB。
//! 这些是高价值用户数据，maclean **绝不直接删除**：
//! - 消息数据库（db_storage）与附件原件（msg/video|attach|file）都不提供删除；
//! - 这里只把"空间分别被什么占用"讲清楚，并引导用户到 App 内清理。
//!
//! 分类规则按目录名匹配、对版本/多账号结构保持宽容：微信 4.0 为
//! `xwechat_files/<wxid>/…`，老版本与 QQ 布局不同，未命中的部分一律落入
//! "其它数据"，绝不因识别不出而误判。

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::scanner::{dir_size, home_dir};

/// 受支持的 IM 应用：(bundle_id, 中文显示名, 应用内清理入口提示)
pub struct ImApp {
    pub bundle: &'static str,
    pub name: &'static str,
    /// 应用内存储管理路径，用于引导用户（仅文案）
    pub storage_hint: &'static str,
}

/// IM bundle 白名单。
///
/// 精确匹配容器 bundle id（`com.tencent.qq.share` 等扩展不会误命中，
/// 它们的 Documents 也很小、到不了展示阈值）。
/// QQ / 企业微信的 bundle id 以各版本实测为准；仅微信做了细分类，
/// 其它 IM 的分析会退化为"总量 + 打开应用"，仍然安全。
pub const IM_APPS: &[ImApp] = &[
    ImApp {
        bundle: "com.tencent.xinWeChat",
        name: "微信",
        storage_hint: "微信「设置 → 通用 → 存储空间」",
    },
    ImApp {
        bundle: "com.tencent.qq",
        name: "QQ",
        storage_hint: "QQ「设置 → 通用 → 存储空间」",
    },
    ImApp {
        bundle: "com.tencent.WeWorkMac",
        name: "企业微信",
        storage_hint: "企业微信「设置 → 通用 → 存储空间」",
    },
    ImApp {
        bundle: "com.tencent.workbuddy.mac",
        name: "企业微信",
        storage_hint: "企业微信「设置 → 通用 → 存储空间」",
    },
];

/// 按 bundle id 查 IM 应用
pub fn im_app(bundle_id: &str) -> Option<&'static ImApp> {
    IM_APPS.iter().find(|a| a.bundle == bundle_id)
}

/// 判断某路径是否为受支持 IM 的 `Data/Documents` 根，是则返回该 IM。
///
/// 严格解析 `~/Library/Containers/<bundle>/Data/Documents` 三段结构，
/// 作为 `im_breakdown` 的入口校验，避免被诱导去统计任意目录。
pub fn im_app_for_docs_path(path: &Path) -> Option<&'static ImApp> {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let home = std::fs::canonicalize(home_dir()).ok()?;
    let containers = home.join("Library/Containers");
    let rel = canon.strip_prefix(containers).ok()?;

    let mut comps = rel.components();
    let bundle = comps.next()?.as_os_str().to_str()?;
    let seg_data = comps.next()?;
    let seg_docs = comps.next()?;
    // 只能恰好是 Data/Documents，不能是其上层或更深路径
    if comps.next().is_some() {
        return None;
    }
    let is_data = matches!(seg_data, Component::Normal(s) if s == "Data");
    let is_docs = matches!(seg_docs, Component::Normal(s) if s == "Documents");
    if !(is_data && is_docs) {
        return None;
    }
    im_app(bundle)
}

/// 一个占用分类（稳定 key + 中文标签 + 字节数）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImPart {
    pub key: &'static str,
    pub label: &'static str,
    pub size_bytes: u64,
}

/// IM Documents 的只读占用构成
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImBreakdown {
    pub app_name: String,
    pub storage_hint: String,
    pub root: String,
    pub total_bytes: u64,
    pub parts: Vec<ImPart>,
}

/// 分类的固定展示顺序与标签
const PART_ORDER: &[(&str, &str)] = &[
    ("db", "消息数据库（聊天记录索引）"),
    ("video", "视频"),
    ("image", "图片与表情"),
    ("file", "收到的文件"),
    ("backup", "聊天备份"),
    ("cache", "缓存与日志"),
    ("other", "其它数据（设置、登录态等）"),
];

/// 命中即整体归类、不再深入的缓存/日志/临时目录名（小写精确匹配）
const CACHE_DIR_NAMES: &[&str] = &[
    "cache",
    "caches",
    "cachedir",
    "httpcache",
    "imagecache",
    "videocache",
    "gpucache",
    "webkit",
    "thumbnails",
    "temp",
    "tmp",
    "tpreportplugincache",
    "log",
    "logs",
];

/// 备份目录名（小写精确匹配）
const BACKUP_DIR_NAMES: &[&str] = &["backup", "backups"];

fn name_lower(dir: &Path) -> String {
    dir.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// 对 IM 的 Documents 根做只读分类统计。
///
/// 用受限深度的手动 BFS：命中已知分类目录即整体 `dir_size` 计一次并入队剪枝，
/// 避免重复累计；未识别的目录继续向下。`消息数据库/视频/图片/文件/缓存/备份`
/// 之和之外的全部余量归入"其它数据"，保证各分类之和等于总量（受看门狗
/// 近似统计影响可能有微小误差，用 saturating 兜底）。
pub fn analyze(docs_root: &Path) -> ImBreakdown {
    let app = im_app_for_docs_path(docs_root);
    let app_name = app.map(|a| a.name).unwrap_or("IM").to_string();
    let storage_hint = app
        .map(|a| a.storage_hint)
        .unwrap_or("应用的「设置 → 通用 → 存储空间」")
        .to_string();

    let total = dir_size(docs_root);
    let mut acc: BTreeMap<&'static str, u64> = BTreeMap::new();
    let add = |acc: &mut BTreeMap<&'static str, u64>, key: &'static str, size: u64| {
        *acc.entry(key).or_insert(0) += size;
    };

    // 手动 BFS（不跟随符号链接），命中分类即剪枝
    let mut queue: Vec<(PathBuf, usize)> = vec![(docs_root.to_path_buf(), 0)];
    const MAX_DEPTH: usize = 8;
    while let Some((dir, depth)) = queue.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue; // 散落文件由 other 兜底，不逐个统计
            }
            if depth >= MAX_DEPTH {
                continue;
            }
            let name = name_lower(&path);
            let parent_is_msg = dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == "msg")
                .unwrap_or(false);

            let key: Option<&'static str> = if name == "db_storage" {
                Some("db")
            } else if parent_is_msg && name == "video" {
                Some("video")
            } else if parent_is_msg && name == "attach" {
                Some("image")
            } else if parent_is_msg && name == "file" {
                Some("file")
            } else if BACKUP_DIR_NAMES.contains(&name.as_str()) {
                Some("backup")
            } else if CACHE_DIR_NAMES.contains(&name.as_str()) {
                Some("cache")
            } else {
                None
            };

            if let Some(k) = key {
                add(&mut acc, k, dir_size(&path));
            } else {
                queue.push((path, depth + 1));
            }
        }
    }

    let classified: u64 = acc.values().sum();
    let other = total.saturating_sub(classified);
    if other > 0 {
        acc.insert("other", other);
    }

    let parts: Vec<ImPart> = PART_ORDER
        .iter()
        .filter_map(|(key, label)| {
            acc.get(*key).map(|&size_bytes| ImPart {
                key,
                label,
                size_bytes,
            })
        })
        .collect();

    ImBreakdown {
        app_name,
        storage_hint,
        root: docs_root.to_string_lossy().to_string(),
        total_bytes: total,
        parts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("maclean_im_test_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn mk_file(root: &Path, rel: &str, bytes: usize) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![7u8; bytes]).unwrap();
    }

    #[test]
    fn classify_wechat4_layout() {
        let root = tmp_root("wechat4");
        mk_file(&root, "xwechat_files/wxid_x/db_storage/msg0.db", 1024);
        mk_file(&root, "xwechat_files/wxid_x/msg/video/v.mp4", 4096);
        mk_file(&root, "xwechat_files/wxid_x/msg/attach/a.jpg", 2048);
        mk_file(&root, "xwechat_files/wxid_x/msg/file/f.pdf", 3072);
        mk_file(&root, "Caches/c1.tmp", 256);
        mk_file(&root, "Backup/bak.dat", 512);
        mk_file(&root, "app_data/conf.json", 128);

        let b = analyze(&root);
        let get = |k: &str| b.parts.iter().find(|p| p.key == k).map(|p| p.size_bytes);

        assert!(get("db").is_some(), "应识别消息数据库");
        assert!(get("video").is_some(), "应识别视频");
        assert!(get("image").is_some(), "应识别图片");
        assert!(get("file").is_some(), "应识别文件");
        assert!(get("cache").is_some(), "应识别缓存");
        assert!(get("backup").is_some(), "应识别备份");
        assert!(get("other").is_some(), "未识别部分应归入其它");

        // 各分类都被统计到、且不超过总量；分类之和等于总量
        for p in &b.parts {
            assert!(p.size_bytes > 0 && p.size_bytes <= b.total_bytes, "{} 越界", p.key);
        }
        let sum: u64 = b.parts.iter().map(|p| p.size_bytes).sum();
        assert_eq!(sum, b.total_bytes, "各分类之和应等于总量");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn im_whitelist_exact_match() {
        assert!(im_app("com.tencent.xinWeChat").is_some());
        assert!(im_app("com.tencent.qq").is_some());
        // 扩展 bundle 不应被当作主 IM
        assert!(im_app("com.tencent.qq.share").is_none());
        assert!(im_app("com.apple.Safari").is_none());
    }

    #[test]
    fn non_wechat_dirs_go_to_other() {
        let root = tmp_root("plain");
        mk_file(&root, "random/a/b/c.dat", 1000);
        mk_file(&root, "Documents2/x", 1000);
        let b = analyze(&root);
        // 没有命中任何已知分类时，全部进 other
        assert_eq!(b.parts.len(), 1);
        assert_eq!(b.parts[0].key, "other");
        let _ = fs::remove_dir_all(&root);
    }
}
