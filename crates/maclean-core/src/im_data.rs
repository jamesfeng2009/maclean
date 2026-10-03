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

use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use rayon::prelude::*;

use crate::scanner::home_dir;

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

/// 分类在累加数组（也是 [`PART_ORDER`]）中的下标；其它分类为 `[0..=5]`，
/// 「其它」固定为 `OTHER_IDX`。
const OTHER_IDX: usize = 6;

/// 单个文件的真实磁盘占用（字节）：优先 `st_blocks×512`（与全项目其它扫描口径
/// 一致，APFS 稀疏文件不虚高）；拿不到块信息时退回逻辑长度。
#[inline]
fn file_disk_bytes(md: &std::fs::Metadata) -> u64 {
    let blocks = md.blocks();
    if blocks > 0 {
        blocks.saturating_mul(512)
    } else {
        md.len()
    }
}

/// 依据目录名（及父目录是否为 `msg`）判定整棵子树的归属分类下标。
/// 返回 `None` 表示未识别，继续向其子目录尝试分类；其中散落文件归入「其它」。
///
/// 下标语义必须与 [`PART_ORDER`] 的顺序一一对应（由单测
/// `part_order_indices_stable` 守护）。
#[inline]
fn classify_im_dir(name: &str, parent_is_msg: bool) -> Option<usize> {
    if name == "db_storage" {
        Some(0) // db
    } else if parent_is_msg && name == "video" {
        Some(1) // video
    } else if parent_is_msg && name == "attach" {
        Some(2) // image
    } else if parent_is_msg && name == "file" {
        Some(3) // file
    } else if BACKUP_DIR_NAMES.contains(&name) {
        Some(4) // backup
    } else if CACHE_DIR_NAMES.contains(&name) {
        Some(5) // cache
    } else {
        None
    }
}

/// 并行递归遍历一个目录子树，返回 `(总字节, 各分类字节[7])`。
///
/// - `forced` 为 `Some(k)` 时整棵子树都计入分类 k（已命中分类边界，不再细判）；
///   `None` 表示仍在未识别区域，子目录继续尝试分类、散落文件计入「其它」。
/// - 每个常规文件只 stat 一次；当前层收集完子目录后用 rayon 对各子树并行递归并
///   归约。不设深度上限（未命中的深目录照样走到文件）、不用看门狗，因此
///   「各分类之和 == 总量」严格成立，也不会因超时低估大附件目录。
fn walk_classify(dir: &Path, forced: Option<usize>) -> (u64, [u64; 7]) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return (0, [0; 7]), // TCC / 不存在等：静默跳过，绝不报错中断
    };
    let parent_is_msg = dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == "msg")
        .unwrap_or(false);

    let mut total = 0u64;
    let mut acc = [0u64; 7];
    let mut subs: Vec<(PathBuf, Option<usize>)> = Vec::new();

    for entry in entries.filter_map(|e| e.ok()) {
        // DirEntry::metadata 不跟随符号链接
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let path = entry.path();
        if md.is_dir() {
            let key = match forced {
                Some(k) => Some(k),
                None => classify_im_dir(&name_lower(&path), parent_is_msg),
            };
            subs.push((path, key));
        } else {
            let idx = forced.unwrap_or(OTHER_IDX);
            let size = file_disk_bytes(&md);
            total = total.saturating_add(size);
            acc[idx] = acc[idx].saturating_add(size);
        }
    }

    // 多核并行递归各子树并归约
    let (sub_total, sub_acc) = subs
        .par_iter()
        .map(|(p, k)| walk_classify(p, *k))
        .reduce(
            || (0u64, [0u64; 7]),
            |(t1, a1), (t2, a2)| {
                let mut a = [0u64; 7];
                for i in 0..7 {
                    a[i] = a1[i].saturating_add(a2[i]);
                }
                (t1.saturating_add(t2), a)
            },
        );

    let mut merged = [0u64; 7];
    for i in 0..7 {
        merged[i] = acc[i].saturating_add(sub_acc[i]);
    }
    (total.saturating_add(sub_total), merged)
}

/// 对 IM 的 Documents 根做只读分类统计（多核并行、单次遍历）。
///
/// 命中已知分类目录（db_storage / msg-video|attach|file / cache / backup）后，其下
/// 所有文件整体计入该分类；未命中区域的散落文件归入「其它数据」。每个常规文件只
/// stat 一次，「各分类之和 == 总量」严格成立，也避免了旧实现「整树一次 + 每个分类
/// 各一次」对几十万 IM 小文件的重复串行遍历。
///
/// 该函数为 CPU/IO 密集型，调用方（Tauri command）必须放到阻塞线程池，避免卡 UI。
pub fn analyze(docs_root: &Path) -> ImBreakdown {
    let app = im_app_for_docs_path(docs_root);
    let app_name = app.map(|a| a.name).unwrap_or("IM").to_string();
    let storage_hint = app
        .map(|a| a.storage_hint)
        .unwrap_or("应用的「设置 → 通用 → 存储空间」")
        .to_string();

    let (total_bytes, arr) = walk_classify(docs_root, None);

    let parts: Vec<ImPart> = PART_ORDER
        .iter()
        .enumerate()
        .filter_map(|(i, (key, label))| {
            let size_bytes = arr[i];
            (size_bytes > 0).then(|| ImPart {
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
        total_bytes,
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
    fn part_order_indices_stable() {
        // 守护 classify_im_dir 返回的下标与 PART_ORDER 不错位
        assert_eq!(PART_ORDER[0].0, "db");
        assert_eq!(PART_ORDER[1].0, "video");
        assert_eq!(PART_ORDER[2].0, "image");
        assert_eq!(PART_ORDER[3].0, "file");
        assert_eq!(PART_ORDER[4].0, "backup");
        assert_eq!(PART_ORDER[5].0, "cache");
        assert_eq!(PART_ORDER[OTHER_IDX].0, "other");
        assert_eq!(PART_ORDER.len(), 7);
    }

    #[test]
    fn classify_dir_indices() {
        assert_eq!(classify_im_dir("db_storage", false), Some(0));
        assert_eq!(classify_im_dir("video", true), Some(1));
        assert_eq!(classify_im_dir("video", false), None, "非 msg 父目录的 video 不算");
        assert_eq!(classify_im_dir("attach", true), Some(2));
        assert_eq!(classify_im_dir("file", true), Some(3));
        assert_eq!(classify_im_dir("backup", false), Some(4));
        assert_eq!(classify_im_dir("caches", false), Some(5));
        assert_eq!(classify_im_dir("random", false), None);
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
    fn forced_category_covers_entire_subtree() {
        // 命中分类目录后，其下任意深度、甚至同名“干扰”子目录都整体归该类
        let root = tmp_root("forced");
        mk_file(&root, "x/w/msg/video/a/b/c.mp4", 1000);
        mk_file(&root, "x/w/msg/video/db_storage/x.db", 1000);
        mk_file(&root, "x/w/msg/video/loose.bin", 1000);
        let b = analyze(&root);
        let get = |k: &str| b.parts.iter().find(|p| p.key == k).map(|p| p.size_bytes).unwrap_or(0);
        assert_eq!(get("video"), b.total_bytes, "video 子树应整体计入视频");
        assert_eq!(get("db"), 0, "已在 video 子树内的 db_storage 不应再被识别为数据库");
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
