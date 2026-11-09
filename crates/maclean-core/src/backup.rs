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
    /// 删除时记录在废纸篓里的**实际落点文件名**（B-04）。
    ///
    /// 空串 = 无法确认归属（Finder 重名改名、回收站无稳定路径等），
    /// 还原时必须判 failed，绝不退回"按 basename 猜一个名字搬过去"。
    /// `serde(default)` 兼容旧清单（缺少该字段的历史条目按不可确认处理）。
    #[serde(default)]
    pub trash_name: String,
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

/// 用 `create_new(true)` 原子占位一个清单文件（B-05）
///
/// 旧实现靠 `exists()` 判断再 `std::fs::write`（非 create_new）：两个并发删除
/// 任务（定时 + 手动）同一秒会拿到同一个 id、互相覆盖丢清单。这里直接以目标
/// 文件本身占位，冲突就换 id 重试；返回的 File 句柄交给调用方写入，全程只有
/// 一个写者。同一 id 的 `.json` 与 `.pending.json` 视为互相冲突，避免两种清单撞名。
fn claim_manifest_file(
    dir: &std::path::Path,
    now: u64,
    pending: bool,
) -> Option<(String, std::fs::File)> {
    let ext = if pending { "pending.json" } else { "json" };
    for i in 0..1000u32 {
        let id = if i == 0 {
            format!("{}", now)
        } else {
            format!("{}-{}", now, i)
        };
        let sibling = if pending {
            dir.join(format!("{}.json", id))
        } else {
            dir.join(format!("{}.pending.json", id))
        };
        if sibling.exists() {
            continue;
        }
        let path = dir.join(format!("{}.{}", id, ext));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => return Some((id, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// 把清单序列化并写入已占位的句柄（B-05）
fn write_claimed_manifest(file: &mut std::fs::File, manifest: &BackupManifest) -> Option<()> {
    use std::io::Write;
    let s = serde_json::to_string_pretty(manifest).ok()?;
    file.write_all(s.as_bytes()).ok()?;
    file.flush().ok()?;
    Some(())
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
    // B-05：原子占位，冲突换 id 重试。
    let Some((id, mut file)) = claim_manifest_file(&dir, now, false) else {
        crate::logger::warn("备份清单 id 分配失败，本次删除不记录清单");
        return None;
    };
    let path = dir.join(format!("{}.json", id));
    let manifest = BackupManifest {
        id: id.clone(),
        created_at: now,
        platform: std::env::consts::OS.to_string(),
        entries,
    };
    if write_claimed_manifest(&mut file, &manifest).is_some() {
        Some(id)
    } else {
        crate::logger::warn("备份清单写入失败，本次删除不记录");
        let _ = std::fs::remove_file(&path);
        None
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
    // B-05：pending 也走 create_new 原子占位，并同时检查 `.json` 兄弟文件。
    let Some((id, mut file)) = claim_manifest_file(&dir, now, true) else {
        crate::logger::warn("待删快照 id 分配失败，不记录快照");
        return None;
    };
    let path = dir.join(format!("{}.pending.json", id));
    let manifest = BackupManifest {
        id: id.clone(),
        created_at: now,
        platform: std::env::consts::OS.to_string(),
        entries,
    };
    if write_claimed_manifest(&mut file, &manifest).is_some() {
        Some(id)
    } else {
        crate::logger::warn("待删快照写入失败");
        let _ = std::fs::remove_file(&path);
        None
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
        // 用系统真实家目录而不是 $HOME：NSFileManager 回收时认的是前者，
        // 两边不一致会让 restore 找不到刚删进废纸篓的文件。
        crate::platform::real_home_dir()
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
        // B-04：只认「删除时写进清单的废纸篓实际落点」。没有记录就无法确认
        // 归属（重名时 Finder 会加后缀），一律进 failed —— 绝不退回按 basename
        // 猜名字，那会让两个不同目录的同名文件争抢同一个废纸篓条目。
        if e.trash_name.trim().is_empty() {
            report.failed.push(e.path.clone());
            continue;
        }
        report.restored.push(e.path.clone());
    }
    report
}

/// 还原顺序：按路径深度升序（祖先目录先回原位，子项再搬进去）
fn sort_restore_order(mut paths: Vec<String>) -> Vec<String> {
    paths.sort_by_key(|p| Path::new(p).components().count());
    paths
}

/// CORE-08：还原目标是否落在预期 home 内（解析符号链接之后仍在内）。
///
/// 清单是 `~/.maclean/backups/*.json`，无签名、无特权校验，任何以该用户
/// 身份运行的进程都能改它。`target` 直接来自清单：只做 `dst.exists()`
/// 的话，被篡改的清单能把废纸篓里的文件 rename 到任意目录；而且
/// `Path::exists()` 会跟随符号链接，悬空链接会被判「不存在」后直接覆盖。
///
/// `homes` 由调用方注入（生产是 `safety::protected_homes()` 的双源集合），
/// 这样测试能喂一个临时 home，不必依赖真实家目录。
fn restore_target_within_home(target: &str, homes: &[PathBuf]) -> bool {
    let dst = Path::new(target);
    if !dst.is_absolute() {
        return false;
    }
    // 词法形态里出现 . / .. 一律按篡改处理（清单记录的是删除前的绝对路径）
    if dst.components().any(|c| {
        matches!(
            c,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )
    }) {
        return false;
    }
    let Some(parent) = dst.parent() else {
        return false;
    };
    // canonicalize 解析整条父链上的符号链接；父目录不存在也直接失败
    let Ok(real_parent) = parent.canonicalize() else {
        return false;
    };
    homes.iter().any(|home| {
        home.canonicalize()
            .is_ok_and(|h| real_parent.starts_with(&h))
    })
}

/// B-04：按清单记录的 `trash_name` 从废纸篓搬回目标（不猜 basename）。
///
/// 目标父目录不存在、目标已存在、或落点名字含路径分隔符（防被篡改的清单
/// 穿越）都判失败 —— 覆盖一个当前正在用的文件比不还原更糟。
/// CORE-08 追加：目标父目录 canonical 后必须仍在预期 home 内，且用
/// `symlink_metadata` 判存在（悬空符号链接也算「已存在」，不许覆盖）。
fn restore_from_trash(trash_dir: &str, trash_name: &str, target: &str, homes: &[PathBuf]) -> bool {
    if trash_dir.is_empty() || trash_name.is_empty() {
        return false;
    }
    if trash_name.contains('/')
        || trash_name.contains('\\')
        || trash_name == "."
        || trash_name == ".."
    {
        return false;
    }
    let src = Path::new(trash_dir).join(trash_name);
    let dst = Path::new(target);
    if dst.symlink_metadata().is_ok() {
        return false;
    }
    if !restore_target_within_home(target, homes) {
        return false;
    }
    std::fs::rename(src, dst).is_ok()
}

/// 执行还原计划（CORE-08 抽出的可测入口）：与 `restore()` 同一段逻辑，只是
/// 把「从磁盘读清单」换成入参，让测试能直接喂一份被篡改的清单，而不必碰
/// 全局的 `set_test_backup_dir` 钩子（并行测试会互相覆盖那个钩子）。
fn execute_restore(manifest: &BackupManifest, trash_dir: &str, homes: &[PathBuf]) -> RestoreReport {
    let plan = restore_plan(manifest, trash_dir);
    let mut report = RestoreReport {
        not_restorable: plan.not_restorable,
        ..Default::default()
    };

    // 同一份清单里可能既有目录、又有目录内的文件。先还原子项会因父目录尚在
    // 废纸篓里而失败，所以按路径深度升序：祖先目录先回原位，子项再搬进去。
    let pending = sort_restore_order(plan.restored);

    for target in pending {
        // B-04：按记录取回，而不是用 target 的 basename 重新猜。
        let trash_name = manifest
            .entries
            .iter()
            .find(|e| e.path == target)
            .map(|e| e.trash_name.as_str())
            .unwrap_or("");
        if restore_from_trash(trash_dir, trash_name, &target, homes) {
            report.restored.push(target);
        } else {
            report.failed.push(target);
        }
    }
    report
}

/// 执行还原（尽力而为）
///
/// 只处理废纸篓里的项，且按清单记录的落点取回（B-04）：目标父目录不存在
/// 就跳过，目标已存在也跳过 —— 覆盖一个当前正在用的文件比不还原更糟。
pub fn restore(id: &str, trash_dir: &str) -> RestoreReport {
    let Some(manifest) = load(id) else {
        return RestoreReport::default();
    };
    execute_restore(&manifest, trash_dir, &crate::safety::protected_homes())
}

/// B-04：删除前声明废纸篓落点名字。名字已被占用（Finder 会重名改名）或路径
/// 无文件名时无法确认归属 → 返回空串，还原时判 failed。
pub fn claim_trash_name(trash_dir: &str, original: &str) -> String {
    if trash_dir.is_empty() {
        return String::new();
    }
    let Some(name) = Path::new(original).file_name() else {
        return String::new();
    };
    let name = name.to_string_lossy().to_string();
    if name.is_empty() || Path::new(trash_dir).join(&name).exists() {
        return String::new();
    }
    name
}

/// B-04：删除后复核声明的落点确实出现在废纸篓里；复核不过则无法确认归属。
pub fn confirm_trash_name(trash_dir: &str, claimed: &str) -> String {
    if trash_dir.is_empty() || claimed.is_empty() {
        return String::new();
    }
    if !Path::new(trash_dir).join(claimed).exists() {
        return String::new();
    }
    claimed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, restorable: bool) -> BackupEntry {
        // 默认按 basename 记录落点，模拟删除时成功确认归属的情形。
        let trash_name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        BackupEntry {
            path: path.to_string(),
            size_bytes: 100,
            category: "test".to_string(),
            restorable,
            trash_name,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn trash_dir_follows_system_home_not_env_home() {
        // 删除走 NSFileManager（系统真实家目录），还原必须去同一个废纸篓；
        // 若这里退回 $HOME，覆盖 HOME 运行时就会"删得进去、还原不出来"。
        let expected = crate::platform::real_home_dir().join(".Trash");
        assert_eq!(std::path::Path::new(&trash_dir()), expected);
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
    fn restore_puts_parent_dirs_before_children() {
        // 清单里同时有目录和目录内文件时，子项先还原会因父目录还在废纸篓而失败
        let order = sort_restore_order(vec![
            "/h/big/a.bin".to_string(),
            "/h/big".to_string(),
            "/h/big/b.bin".to_string(),
        ]);
        assert_eq!(order[0], "/h/big");
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

        // B-05：并发删除（定时 + 手动）不得互相覆盖清单。
        // 反向验证：把 claim_manifest_file 改回 exists() + std::fs::write，
        // 下面断言会因同一秒拿到重复 id 而变红。
        let handles: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    record(vec![entry(&format!("/Users/u/Caches/c{i}.bin"), true)])
                })
            })
            .collect();
        let ids: Vec<String> = handles
            .into_iter()
            .filter_map(|h| h.join().unwrap())
            .collect();
        assert_eq!(ids.len(), 8, "并发 record 有失败");
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 8, "并发删除拿到重复 id，清单互相覆盖");
        for id in &ids {
            assert!(load(id).is_some(), "清单 {id} 丢失");
        }

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

    // ---------- B-04：按记录的废纸篓落点还原 ----------

    #[test]
    fn b04_two_same_named_files_restore_to_their_own_directories() {
        // 反向验证：把 restore_from_trash 改回用 target 的 basename，或把
        // restore_plan 的 trash_name 校验去掉，下面断言必红。
        let tmp = std::env::temp_dir().join(format!("maclean_b04_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let trash = tmp.join("Trash");
        let d1 = tmp.join("dir1");
        let d2 = tmp.join("dir2");
        for d in [&trash, &d1, &d2] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(trash.join("f.txt"), b"from dir1").unwrap();
        std::fs::write(trash.join("f (1).txt"), b"from dir2").unwrap();

        let mk = |dir: &std::path::Path, name: &str| BackupEntry {
            path: dir.join("f.txt").to_string_lossy().to_string(),
            size_bytes: 9,
            category: "t".to_string(),
            restorable: true,
            trash_name: name.to_string(),
        };
        let m = BackupManifest {
            id: "x".to_string(),
            created_at: 0,
            platform: "macos".to_string(),
            entries: vec![mk(&d1, "f.txt"), mk(&d2, "f (1).txt")],
        };
        let trash_s = trash.to_string_lossy().to_string();
        let plan = restore_plan(&m, &trash_s);
        assert!(plan.failed.is_empty(), "有记录的两项不该进 failed");
        assert_eq!(plan.restored.len(), 2);
        for e in &m.entries {
            assert!(
                restore_from_trash(&trash_s, &e.trash_name, &e.path, std::slice::from_ref(&tmp)),
                "{} 按记录搬回失败",
                e.path
            );
        }
        assert_eq!(
            std::fs::read_to_string(d1.join("f.txt")).unwrap(),
            "from dir1"
        );
        assert_eq!(
            std::fs::read_to_string(d2.join("f.txt")).unwrap(),
            "from dir2"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn b04_missing_trash_name_is_failed_not_guessed() {
        let m = BackupManifest {
            id: "x".to_string(),
            created_at: 0,
            platform: "macos".to_string(),
            entries: vec![BackupEntry {
                path: "/a/b/f.txt".to_string(),
                size_bytes: 1,
                category: "t".to_string(),
                restorable: true,
                trash_name: String::new(),
            }],
        };
        let plan = restore_plan(&m, "/tmp/Trash");
        assert!(plan.restored.is_empty(), "无归属记录不得猜名字还原");
        assert_eq!(plan.failed, vec!["/a/b/f.txt".to_string()]);
        assert!(!restore_from_trash("/tmp/Trash", "", "/a/b/f.txt", &[]));
    }

    #[test]
    fn b04_claim_and_confirm_trash_name() {
        let tmp = std::env::temp_dir().join(format!("maclean_b04c_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let trash = tmp.join("Trash");
        std::fs::create_dir_all(&trash).unwrap();
        let s = trash.to_string_lossy().to_string();
        assert_eq!(claim_trash_name(&s, "/a/b/f.txt"), "f.txt");
        assert_eq!(confirm_trash_name(&s, "f.txt"), ""); // 删除前不存在 → 未落盘
        std::fs::write(trash.join("f.txt"), b"x").unwrap();
        // 名字已被占用 → 无法确认归属（Finder 会改名）
        assert_eq!(claim_trash_name(&s, "/a/b/f.txt"), "");
        assert_eq!(confirm_trash_name(&s, "f.txt"), "f.txt");
        assert_eq!(confirm_trash_name(&s, "missing.txt"), "");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ---------- CORE-08：被篡改的清单不得越界写回 ----------
    //
    // 威胁模型已核实：清单就是 `~/.maclean/backups/*.json`，`load()` 只读
    // JSON、无签名无权限校验，任何以该用户身份运行的进程都能改 `entries[].path`。

    fn c08_layout(tag: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let tmp = std::env::temp_dir().join(format!("maclean_c08_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("home");
        let outside = tmp.join("outside");
        let trash = tmp.join("Trash");
        for d in [&home, &outside, &trash] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(trash.join("payload.bin"), b"payload").unwrap();
        (tmp, home, outside, trash)
    }

    fn c08_manifest(target: &Path, trash_name: &str) -> BackupManifest {
        BackupManifest {
            id: "c08".to_string(),
            created_at: 0,
            platform: "macos".to_string(),
            entries: vec![BackupEntry {
                path: target.to_string_lossy().into_owned(),
                size_bytes: 7,
                category: "大文件".to_string(),
                restorable: true,
                trash_name: trash_name.to_string(),
            }],
        }
    }

    #[test]
    fn c08_manifest_target_outside_home_is_refused_without_writing() {
        let (tmp, home, outside, trash) = c08_layout("outside");
        let trash_s = trash.to_string_lossy().to_string();
        let target = outside.join("payload.bin");

        let report = execute_restore(
            &c08_manifest(&target, "payload.bin"),
            &trash_s,
            std::slice::from_ref(&home),
        );
        assert!(report.restored.is_empty());
        assert_eq!(report.failed, vec![target.to_string_lossy().into_owned()]);
        assert!(!target.exists(), "home 之外不该被写入");
        assert!(
            trash.join("payload.bin").exists(),
            "失败的还原不得消耗废纸篓条目"
        );

        // 反向验证：正常 home 内目标仍能还原，别把功能改死
        let inside = home.join("restored.bin");
        let report = execute_restore(
            &c08_manifest(&inside, "payload.bin"),
            &trash_s,
            std::slice::from_ref(&home),
        );
        assert_eq!(report.restored, vec![inside.to_string_lossy().into_owned()]);
        assert_eq!(std::fs::read(&inside).unwrap(), b"payload");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn c08_symlinked_manifest_targets_are_refused() {
        use std::os::unix::fs::symlink;

        let (tmp, home, outside, trash) = c08_layout("symlink");
        let trash_s = trash.to_string_lossy().to_string();

        // 形态一：清单里的父目录是符号链接，指向 home 外。
        // canonicalize 前它看起来在 home 内，旧实现会直接把文件写出去。
        let link = home.join("escape");
        symlink(&outside, &link).unwrap();
        let via_link = link.join("payload.bin");
        assert!(
            !restore_target_within_home(&via_link.to_string_lossy(), std::slice::from_ref(&home)),
            "canonical 后越出 home 的父目录必须被拒"
        );
        let report = execute_restore(
            &c08_manifest(&via_link, "payload.bin"),
            &trash_s,
            std::slice::from_ref(&home),
        );
        assert!(report.restored.is_empty());
        assert!(
            !outside.join("payload.bin").exists(),
            "不得穿越 symlink 越界写入"
        );

        // 形态二：target 自身是悬空 symlink。旧实现用 dst.exists() 跟随链接，
        // 会把它判成「不存在」然后 rename 覆盖。
        let dangling = home.join("dangling.bin");
        symlink(outside.join("nowhere.bin"), &dangling).unwrap();
        assert!(dangling.symlink_metadata().is_ok());
        assert!(
            !dangling.exists(),
            "exists() 会跟随链接 —— 这正是旧实现看到的假象"
        );
        let report = execute_restore(
            &c08_manifest(&dangling, "payload.bin"),
            &trash_s,
            std::slice::from_ref(&home),
        );
        assert_eq!(report.failed, vec![dangling.to_string_lossy().into_owned()]);
        assert!(
            !outside.join("nowhere.bin").exists(),
            "悬空 symlink 不得被覆盖成越界写入"
        );
        assert!(trash.join("payload.bin").exists(), "废纸篓条目不该被消耗");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
