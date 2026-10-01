//! 删除前的备份清单（M-2）
//!
//! # 为什么要有这个模块
//!
//! Windows 侧删除前有系统级回滚手段（`ensure_restore_point` / `backup_registry_key` /
//! `restore_last_backup`），macOS 侧之前**什么都没有** —— 点下删除就只有废纸篓
//! 兜底，而走永久删除的缓存项连废纸篓都没有。
//!
//! 但这里有个必须说清楚的边界：**本模块不制造回滚能力，只记录事实**。
//! 缓存是 `remove_dir_all` 删掉的，字节已经没了，任何"还原"按钮都变不出数据。
//! 所以清单里每一项都带 `restorable`，永久删除的项一律是 false，UI 必须照这个
//! 显示，不能给用户"都能还原"的错觉 —— 那比没有回滚更危险。
//!
//! 真正可还原的只有两类：
//! - 走废纸篓/回收站删除的项（数据在，只是换了位置）
//! - Windows 上删前备份过的注册表键
//!
//! # 平台无关
//! 判定逻辑不碰系统 API，`restore_plan` 是纯函数，开发机可测。
//! 只有落盘路径和"从废纸篓搬回去"依赖具体平台。

use std::path::{Path, PathBuf};

/// 清单保留天数
///
/// 超过这个时间的清单自动清理：磁盘路径会复用，一个月前的"还原"很可能
/// 把旧文件搬到一个早就被新内容占掉的路径上，反而制造混乱。
const RETAIN_DAYS: u64 = 30;

/// 单个被删项的记录
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackupEntry {
    /// 删除前的原始路径
    pub path: String,
    pub size_bytes: u64,
    pub category: String,
    /// 这一项是否真的能还原
    ///
    /// 只有走废纸篓/回收站、或删前备份过的数据才是 true。
    /// 永久删除的缓存一律 false —— 不撒谎是本模块的底线。
    pub restorable: bool,
}

/// 一次删除操作的完整清单
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackupManifest {
    /// 清单 id，同时是文件名（不含扩展名）
    pub id: String,
    /// Unix 秒
    pub created_at: u64,
    pub platform: String,
    pub entries: Vec<BackupEntry>,
}

impl BackupManifest {
    pub fn restorable_count(&self) -> usize {
        self.entries.iter().filter(|e| e.restorable).count()
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.size_bytes).sum()
    }
}

/// 清单目录：~/.maclean/backups
///
/// 测试钩子：单测通过 `set_test_backup_dir` 把目录指向临时位置，
/// 绝不碰真实 backups（那里存着历史删除的还原清单）。
fn backup_dir() -> PathBuf {
    #[cfg(test)]
    {
        if let Ok(guard) = TEST_BACKUP_DIR.lock() {
            if let Some(d) = guard.as_ref() {
                return d.clone();
            }
        }
    }
    crate::platform::app_data_dir().join("backups")
}

#[cfg(test)]
static TEST_BACKUP_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn set_test_backup_dir(d: PathBuf) {
    if let Ok(mut guard) = TEST_BACKUP_DIR.lock() {
        *guard = Some(d);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 生成清单 id
///
/// 用时间戳 + 序号：同一秒内连续删两次不会互相覆盖（覆盖 = 丢清单）。
fn make_id(now: u64) -> String {
    let base = format!("{}", now);
    let dir = backup_dir();
    if !dir.join(format!("{}.json", base)).exists() {
        return base;
    }
    for i in 1..1000 {
        let candidate = format!("{}-{}", now, i);
        if !dir.join(format!("{}.json", candidate)).exists() {
            return candidate;
        }
    }
    // 极端情况兜底：用 1000 次都撞上几乎不可能，落到这里也不该丢清单
    format!("{}-{}", now, now_secs())
}

/// 记录一次删除
///
/// 返回清单 id；写盘失败返回 None —— **不阻断删除**。清单是事后追溯手段，
/// 不能因为它写不进去就让用户删不掉东西（那会把可用性问题变成阻塞）。
pub fn record(entries: Vec<BackupEntry>) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let dir = backup_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        crate::logger::warn("备份清单目录创建失败，本次删除不记录清单");
        return None;
    }
    let now = now_secs();
    let id = make_id(now);
    let manifest = BackupManifest {
        id: id.clone(),
        created_at: now,
        platform: std::env::consts::OS.to_string(),
        entries,
    };
    let path = dir.join(format!("{}.json", id));
    match serde_json::to_string_pretty(&manifest) {
        Ok(s) => match std::fs::write(&path, s) {
            Ok(_) => Some(id),
            Err(e) => {
                crate::logger::warn(&format!("备份清单写入失败，本次删除不记录: {}", e));
                None
            }
        },
        Err(e) => {
            crate::logger::warn(&format!("备份清单序列化失败: {}", e));
            None
        }
    }
}

/// 记录**待删快照**（P0-2）：删除开始前把计划清单落盘。
///
/// 历史事故（9-30）：删除中途 GUI 终止，`record()` 从未执行，事后
/// backups/ 里没有任何记录，只能靠 delete.log 拼凑。本函数在 worker
/// 开始前就把"计划删什么"写进 `<id>.pending.json`；删除正常完成后再
/// 由 `finalize_pending` 清除 —— 一旦删除中断/崩溃，快照留存，
/// 审计能完整还原"当时打算删什么"。
pub fn record_pending(entries: Vec<BackupEntry>) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let dir = backup_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        crate::logger::warn("备份清单目录创建失败，待删快照不记录");
        return None;
    }
    let now = now_secs();
    let id = make_id(now);
    let manifest = BackupManifest {
        id: id.clone(),
        created_at: now,
        platform: std::env::consts::OS.to_string(),
        entries,
    };
    let path = dir.join(format!("{}.pending.json", id));
    match serde_json::to_string_pretty(&manifest) {
        Ok(s) => match std::fs::write(&path, s) {
            Ok(_) => Some(id),
            Err(e) => {
                crate::logger::warn(&format!("待删快照写入失败: {}", e));
                None
            }
        },
        Err(e) => {
            crate::logger::warn(&format!("待删快照序列化失败: {}", e));
            None
        }
    }
}

/// 删除正常完成后清除待删快照（P0-2）。
///
/// 实际清单已由 `record()` 落盘为 `<id>.json`；快照只服务"中断兜底"，
/// 正常结束就删掉，避免 list() 里出现重复条目。删除失败也不 panic。
pub fn finalize_pending(id: &str) {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return;
    }
    let p = backup_dir().join(format!("{}.pending.json", id));
    if std::fs::remove_file(p).is_err() {
        crate::logger::warn(&format!("待删快照清除失败（可手动删除）: {}", id));
    }
}

/// 列出所有清单（按时间倒序，最近的在前）
pub fn list() -> Vec<BackupManifest> {
    let dir = backup_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<BackupManifest> = rd
        .flatten()
        // 只认正式清单 `<id>.json`；`<id>.pending.json` 是删除中断时的
        // 待删快照，不混进"可还原"列表（它代表计划而非事实）。
        .filter(|e| {
            let fname = e.file_name();
            let name = fname.to_string_lossy();
            name.ends_with(".json") && !name.ends_with(".pending.json")
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str::<BackupManifest>(&s).ok())
        .collect();
    sort_newest_first(&mut out);
    out
}

/// 按创建时间倒序（最近的在前）
///
/// 抽成纯函数是为了让排序规则可被直接测试：`list()` 依赖真实磁盘，
/// 在测试里造不出确定的清单集合。
fn sort_newest_first(out: &mut [BackupManifest]) {
    out.sort_by_key(|a| std::cmp::Reverse(a.created_at));
}

pub fn load(id: &str) -> Option<BackupManifest> {
    // id 直接拼进路径：必须拒绝任何含路径分隔符的输入，否则可以读任意 json
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return None;
    }
    let p = backup_dir().join(format!("{}.json", id));
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str::<BackupManifest>(&s).ok())
}

/// 判定一项是否真的能靠"搬回去"还原
///
/// # 为什么平台走参数而不是 cfg!
///
/// 这条判定是"能不能还原"的唯一真相来源，判定错了 UI 就会给用户一个点了
/// 没反应、或者凭空造出空壳文件的按钮。所以它必须在本机（macOS）上就能把
/// **两个平台的分支都测一遍**。读 `cfg!(target_os)` 的话 Windows 分支在
/// 开发机上根本不编译，等于那半边逻辑永远零测试。
///
/// # 各平台的真实情况
///
/// - macOS：废纸篓就是 `~/.Trash` 这个真实目录，改名进去、改名出来，可逆。
/// - Windows：回收站是 `$Recycle.Bin` 下的元数据 + `$I`/`$R` 重命名条目，
///   没有稳定路径可供 rename 回去；那边的回滚走系统还原点
///   （`platform::windows_backup`），不走这条路。
/// - 其余平台：不认识，一律按不可还原处理。宁可少给承诺，不能给假承诺。
pub fn is_restorable_by_move(via_trash: bool, platform: &str) -> bool {
    match platform {
        "macos" => via_trash,
        _ => false,
    }
}

/// 废纸篓目录（仅 macOS 有可用语义）
///
/// Windows 返回空串：`restore` 收到空串会直接把该项判为 failed，
/// 不会去碰任何路径。这里不 panic、不 unwrap —— 备份链路上的任何意外
/// 都只能是"少还原一项"，不能变成崩掉整个删除流程。
pub fn trash_dir() -> String {
    #[cfg(target_os = "macos")]
    {
        crate::platform::home_dir()
            .join(".Trash")
            .to_string_lossy()
            .to_string()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ();
        String::new()
    }
}

/// 清理过期清单
pub fn prune_old() -> usize {
    let cutoff = now_secs().saturating_sub(RETAIN_DAYS * 86400);
    let mut removed = 0;
    for m in list() {
        if m.created_at < cutoff {
            let p = backup_dir().join(format!("{}.json", m.id));
            if std::fs::remove_file(p).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// 还原结果
#[derive(Debug, Default)]
pub struct RestoreReport {
    pub restored: Vec<String>,
    /// 清单里本来就标了不可还原（永久删除）—— 不是失败，是事实
    pub not_restorable: Vec<String>,
    /// 尝试了但没成功（废纸篓里找不到 / 目标路径被占）
    pub failed: Vec<String>,
}

/// 计算还原计划（纯函数，不碰文件系统）
///
/// 只给 restorable=true 的项制定动作：不可还原的项直接进 not_restorable，
/// 绝不"试着还原一下看看" —— 那会在目标路径上凭空造出空目录或残缺文件。
pub fn restore_plan(manifest: &BackupManifest, trash_dir: &str) -> RestoreReport {
    let mut report = RestoreReport::default();
    // 没有废纸篓目录（Windows 等非 macOS 平台）时，标了 restorable 也无从搬起。
    // 一律进 failed，绝不退化成"按相对路径找同名文件"—— 那会在当前工作目录
    // 里抓到同名但无关的文件，然后把它搬进用户目录。
    if trash_dir.is_empty() {
        for e in &manifest.entries {
            if e.restorable {
                report.failed.push(e.path.clone());
            } else {
                report.not_restorable.push(e.path.clone());
            }
        }
        return report;
    }
    for e in &manifest.entries {
        if !e.restorable {
            report.not_restorable.push(e.path.clone());
            continue;
        }
        // 废纸篓里的名字通常是原文件名；重名时 Finder 会加后缀，这里只认
        // 最朴素的那一种，认不出就报 failed —— 宁可报"没找到"也不能猜。
        let name = Path::new(&e.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            report.failed.push(e.path.clone());
            continue;
        }
        report.restored.push(e.path.clone());
        let _ = (trash_dir, &name);
    }
    report
}

/// 执行还原（尽力而为）
///
/// 只处理废纸篓里的项：目标父目录不存在就跳过（用户可能已经重装系统或
/// 移动了目录），目标已存在也跳过 —— 覆盖一个当前正在用的文件比不还原更糟。
pub fn restore(id: &str, trash_dir: &str) -> RestoreReport {
    let Some(manifest) = load(id) else {
        return RestoreReport::default();
    };
    let plan = restore_plan(&manifest, trash_dir);
    let mut report = RestoreReport {
        not_restorable: plan.not_restorable,
        ..Default::default()
    };

    for target in plan.restored {
        let name = Path::new(&target)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let src = Path::new(trash_dir).join(&name);
        let dst = Path::new(&target);
        if dst.exists() {
            report.failed.push(target);
            continue;
        }
        if let Some(parent) = dst.parent() {
            if !parent.exists() {
                report.failed.push(target);
                continue;
            }
        }
        match std::fs::rename(&src, dst) {
            Ok(_) => report.restored.push(target),
            Err(_) => report.failed.push(target),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, restorable: bool) -> BackupEntry {
        BackupEntry {
            path: path.to_string(),
            size_bytes: 100,
            category: "test".to_string(),
            restorable,
        }
    }

    #[test]
    fn permanent_deletes_are_never_marked_restorable() {
        // 底线：清单不得把永久删除的项说成可还原。
        // 这条如果被绕过，UI 就会出现一个点了没反应、或者更糟——
        // 在目标路径造出一个空壳的"还原"按钮。
        let m = BackupManifest {
            id: "x".to_string(),
            created_at: 0,
            platform: "macos".to_string(),
            entries: vec![
                entry("/Users/a/Caches/x", false),
                entry("/Users/a/Docs/y", true),
            ],
        };
        assert_eq!(m.restorable_count(), 1);
        let plan = restore_plan(&m, "/Users/a/.Trash");
        assert_eq!(plan.not_restorable, vec!["/Users/a/Caches/x".to_string()]);
        assert_eq!(plan.restored, vec!["/Users/a/Docs/y".to_string()]);
    }

    #[test]
    fn entries_without_a_filename_cannot_be_planned() {
        let m = BackupManifest {
            id: "x".to_string(),
            created_at: 0,
            platform: "macos".to_string(),
            entries: vec![entry("/", true)],
        };
        let plan = restore_plan(&m, "/Users/a/.Trash");
        assert_eq!(plan.failed, vec!["/".to_string()]);
        assert!(plan.restored.is_empty());
    }

    #[test]
    fn manifest_id_rejects_path_traversal() {
        // id 会直接拼进文件路径，含分隔符的输入必须拒绝
        assert!(load("../../etc/passwd").is_none());
        assert!(load("a/b").is_none());
        assert!(load("..").is_none());
    }

    #[test]
    fn empty_delete_does_not_create_a_manifest() {
        // 删了 0 项不该留下一个空清单污染列表
        assert!(record(Vec::new()).is_none());
    }

    #[test]
    fn pending_snapshot_lifecycle() {
        // P0-2：待删快照完整生命周期 —— 写入 → list() 排除 → 空 entries 不写
        // → finalize 清除 → 正式清单照常列出 → 路径穿越防护。目录注入到临时
        // 位置，不碰真实 backups；合并为单测，避免全局测试钩子并行互相覆盖。
        let tmp =
            std::env::temp_dir().join(format!("maclean_backup_pending_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        set_test_backup_dir(tmp.clone());

        // 写入待删快照
        let id =
            record_pending(vec![entry("/Users/u/Caches/x.bin", true)]).expect("待删快照应写入成功");
        assert!(
            tmp.join(format!("{}.pending.json", id)).exists(),
            "待删快照文件应存在"
        );
        // list() 必须排除 pending 快照（正式清单还没有 → 列表为空）
        assert!(list().is_empty(), "pending 快照不得混入清单列表");
        // 空 entries 不写快照
        assert!(record_pending(Vec::new()).is_none());
        // finalize 后清除
        finalize_pending(&id);
        assert!(
            !tmp.join(format!("{}.pending.json", id)).exists(),
            "finalize 后 pending 快照应被清除"
        );
        // 正式清单落盘后正常出现在列表
        let real_id = record(vec![entry("/Users/u/Caches/x.bin", true)]).unwrap();
        let listed = list();
        assert_eq!(listed.len(), 1, "正式清单应出现在列表");
        assert_eq!(listed[0].id, real_id);
        // 路径穿越防护：含分隔符的 id 直接忽略
        finalize_pending("../../etc/passwd");

        set_test_backup_dir(std::path::PathBuf::new());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn total_bytes_sums_every_entry() {
        let m = BackupManifest {
            id: "x".to_string(),
            created_at: 0,
            platform: "macos".to_string(),
            entries: vec![entry("/a", true), entry("/b", false)],
        };
        assert_eq!(m.total_bytes(), 200);
    }

    #[test]
    fn restorability_depends_on_platform_not_on_wishful_thinking() {
        // 这条同时覆盖 macOS 与 Windows 两个分支 —— 平台作为入参传进来，
        // 所以在 macOS 开发机上 Windows 分支也是真跑过的，不是"应该没问题"。
        assert!(is_restorable_by_move(true, "macos"));
        assert!(!is_restorable_by_move(false, "macos"));
        // 回收站没有可供 rename 的稳定路径，走系统还原点，不冒充可还原
        assert!(!is_restorable_by_move(true, "windows"));
        assert!(!is_restorable_by_move(true, "linux"));
    }

    #[test]
    fn empty_trash_dir_never_restores_anything() {
        // 反向验证的重点：空废纸篓目录必须让所有项进 failed，
        // 而不是退化成相对路径查找（那会把 cwd 下的同名文件搬进用户目录）
        let m = BackupManifest {
            id: "x".to_string(),
            created_at: 0,
            platform: "windows".to_string(),
            entries: vec![entry("C:\\Users\\a\\Docs\\y", true)],
        };
        let plan = restore_plan(&m, "");
        assert!(plan.restored.is_empty());
        assert_eq!(plan.failed, vec!["C:\\Users\\a\\Docs\\y".to_string()]);
    }

    #[test]
    fn listing_sorts_newest_first() {
        // 走生产代码里的那个排序函数，而不是在测试里重写一遍降序 ——
        // 后者只是在验证标准库，list() 的排序规则改坏了它也照样通过。
        let mut v = [
            BackupManifest {
                id: "old".to_string(),
                created_at: 1,
                platform: "macos".to_string(),
                entries: Vec::new(),
            },
            BackupManifest {
                id: "new".to_string(),
                created_at: 9,
                platform: "macos".to_string(),
                entries: Vec::new(),
            },
        ];
        sort_newest_first(&mut v);
        assert_eq!(v[0].id, "new");
        assert_eq!(v[1].id, "old");
    }

    #[test]
    fn listing_routes_through_the_shared_sorter() {
        // 排序规则只有一处实现：list() 若自己再写一遍 sort_by，
        // 上面的测试就测了个寂寞
        // 只取测试模块之前的部分：否则本测试自己的字符串字面量会被自己数进去
        let src = include_str!("backup.rs");
        let prod = src.split("\nmod tests").next().unwrap_or(src);
        assert_eq!(
            prod.matches("std::cmp::Reverse").count(),
            1,
            "排序规则出现了第二处实现，sort_newest_first 没被共用"
        );
        assert!(
            src.contains("    sort_newest_first(&mut out);"),
            "list() 不再经过 sort_newest_first"
        );
    }
}
