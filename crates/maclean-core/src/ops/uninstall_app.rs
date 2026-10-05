//! 应用一键卸载（M-3）
//!
//! 应用卸载页的「一键卸载」入口：给定一个已安装应用的 .app 路径，
//! 一次完成「应用本体 + 关联数据 + 关联缓存」的卸载，全部走与扫描器
//! 完全一致的安全闸门：
//!
//! - **路径白名单**：只接受 `/Applications/` 与 `~/Applications/`（含
//!   `Chrome Apps.localized/` 等一层子目录）下的 `.app` 包，其它路径
//!   一律拒绝 —— 防止这个入口变成"删除任意目录"的通用通道；
//! - **保护级别**：Critical 直接拒绝；RequiresOfficialUninstaller
//!   （安全/MDM 应用）要求用官方卸载工具；DataProtected 放行但提示数据丢失；
//! - **官方卸载器优先**：设置开启时，应用自带卸载器则先启动卸载器
//!   （GUI 可见），不再自行删文件 —— 与 `start_delete` 的 M-1 行为一致；
//! - **执行复用 `start_delete`**：删除条目按扫描器同款 category 组装
//!   （`<名> (卸载)` / `<名> 数据` / `<名> 缓存`），逐成员过
//!   `safety::check_path_safety_with_category`，统一移入废纸篓（可恢复），
//!   并落 M-2 备份清单 + 待删快照。
//!
//! Chrome/Edge/Brave 及其它 Chromium 内核浏览器（Arc/Opera/豆包等）安装的 PWA：
//! 本体是 `~/Applications/<浏览器> Apps.localized/<Name>.app` 的轻量 shim
//! （可执行固定为 `app_mode_loader`）。一键卸载只移除该 shim（移入废纸篓、
//! 可还原），这与 Finder「拖进废纸篓」的官方卸载方式等价。
//!
//! 刻意**不**手动删除浏览器侧数据：其内部注册（Preferences / LevelDB 的
//! web_apps 注册表）与 `Web Applications/Manifest Resources/<资源id>` 用的是
//! 与 shim 的 shortcut id 不同的独立标识，无法安全映射，且浏览器运行中改写会
//! 损坏配置。浏览器下次启动自检到 shim 缺失，会自行清除失效注册与资源。
//! 因此 PWA 条目的关联数据按 0 处理，UI 仅展示 shim 本体体积（通常数 MB）。

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

use crate::app_protection::{self, ProtectionLevel};
use crate::i18n::{self, tf_lang};
use crate::scanner::uninstall::{
    find_associated_files, get_app_display_name, get_bundle_id, is_cache_like_path, path_size,
};
use crate::{logger, scanner};

use super::{start_delete, DeleteMessage};

/// 一键卸载结果
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct UninstallAppReport {
    /// blocked（拒绝执行）/ delegated（已交官方卸载器）/ done（已卸载）
    pub status: String,
    /// 给用户看的结果文案（zh/en 已按 lang_en 选好）
    pub message: String,
    /// 成功删除/移入废纸篓的顶层条目数
    pub deleted: usize,
    /// 被安全闸门拦截的条目数
    pub intercepted: usize,
    /// 路径已不存在等跳过数
    pub skipped: usize,
    /// 走废纸篓、可还原的条目数（M-2）
    pub restorable: usize,
    /// 清单记录总条目数（M-2）
    pub total: usize,
    /// M-2 恢复清单 id（可还原时）
    pub backup_id: Option<String>,
    /// 本次卸载涉及的字节数（本体 + 关联数据 + 关联缓存，删除前统计）
    pub freed_bytes: u64,
}

impl UninstallAppReport {
    fn blocked(message: String) -> Self {
        UninstallAppReport {
            status: "blocked".to_string(),
            message,
            ..Default::default()
        }
    }

    fn delegated(message: String) -> Self {
        UninstallAppReport {
            status: "delegated".to_string(),
            message,
            ..Default::default()
        }
    }
}

/// 一键卸载的路径白名单校验
///
/// 只允许 `.app` 包、且位于 `/Applications/` 或 `~/Applications/`（含一层
/// 子目录，PWA 快捷方式所在）之下。用规范化后的真实路径比对，杜绝符号
/// 链接穿越；显式 `..` 组件直接拒绝。
pub fn is_uninstallable_app_path(path: &str) -> bool {
    let p = Path::new(path);

    // 必须是 .app 包（目录）
    if p.extension().and_then(|e| e.to_str()) != Some("app") {
        return false;
    }
    // 显式 `..` 组件直接拒绝（无需 stat）
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        return false;
    }
    if !p.is_dir() {
        return false;
    }

    let home = scanner::home_dir();
    let raw_roots: [PathBuf; 2] = [PathBuf::from("/Applications"), home.join("Applications")];
    let roots: Vec<PathBuf> = raw_roots
        .iter()
        .map(|r| r.canonicalize().unwrap_or_else(|_| r.clone()))
        .collect();
    let target = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    roots.iter().any(|r| target.starts_with(r))
}

/// 应用一键卸载（阻塞执行；UI 应放后台线程调用）
///
/// `prefer_official_uninstaller`：应用自带官方卸载器时优先启动卸载器并
/// 返回 `delegated`，不自行删文件（卸载器通常连 launchd / pkgutil 收据
/// 一并处理，删文件会与它抢数据）。关闭时走完整删除。
pub fn uninstall_app(
    app_path: &str,
    lang_en: bool,
    prefer_official_uninstaller: bool,
) -> UninstallAppReport {
    // 1) 路径白名单（安全边界，前端传来的路径不被信任）
    if !is_uninstallable_app_path(app_path) {
        logger::warn(&format!("[uninstall] 拒绝非白名单路径: {}", app_path));
        return UninstallAppReport::blocked(
            i18n::t_lang(lang_en, "uninstall_invalid_path").to_string(),
        );
    }

    // 2) 读取 bundle id / 展示名
    let Some(bundle_id) = get_bundle_id(Path::new(app_path)) else {
        return UninstallAppReport::blocked(
            i18n::t_lang(lang_en, "uninstall_no_bundle").to_string(),
        );
    };
    let name = get_app_display_name(Path::new(app_path)).unwrap_or_else(|| {
        Path::new(app_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未知应用".to_string())
    });

    // 3) 保护级别
    let protection = app_protection::check_bundle_protection(&bundle_id);
    if matches!(protection, ProtectionLevel::Critical) {
        return UninstallAppReport::blocked(
            i18n::t_lang(lang_en, "uninstall_blocked_critical").to_string(),
        );
    }
    if matches!(protection, ProtectionLevel::RequiresOfficialUninstaller) {
        let vendor = app_protection::get_security_vendor(&bundle_id).unwrap_or("官方");
        return UninstallAppReport::blocked(tf_lang(
            lang_en,
            "uninstall_blocked_official",
            &[vendor],
        ));
    }

    // 4) 官方卸载器优先（设置开启时；与 start_delete 的 M-1 同款判定）
    if prefer_official_uninstaller {
        if let Some(u) =
            crate::scanner::official_uninstaller::find_official_uninstaller(app_path, &name)
        {
            #[cfg(target_os = "macos")]
            {
                match crate::scanner::official_uninstaller::launch(&u) {
                    Ok(_) => {
                        logger::info(&format!(
                            "[uninstall] 交由官方卸载器处理: {} -> {}",
                            app_path, u.path
                        ));
                        return UninstallAppReport::delegated(tf_lang(
                            lang_en,
                            "uninstall_delegated",
                            &[&u.path],
                        ));
                    }
                    Err(e) => {
                        logger::warn(&format!(
                            "[uninstall] 官方卸载器启动失败，回落到删除: {} ({})",
                            app_path, e
                        ));
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = &u;
            }
        }
    }

    // 5) 组装删除条目（category 与扫描器同款，安全闸门口径一致）
    let associated = find_associated_files(&bundle_id, &name);
    let (cache_paths, data_paths): (Vec<String>, Vec<String>) =
        associated.into_iter().partition(|p| is_cache_like_path(p));

    let mut entries: Vec<(String, String, Vec<String>, bool, u64)> = Vec::new();
    let mut freed_bytes: u64 = 0;

    let app_size = path_size(app_path);
    if app_size > 0 {
        freed_bytes += app_size;
        entries.push((
            app_path.to_string(),
            format!("{} (卸载)", name),
            vec![app_path.to_string()],
            true,
            app_size,
        ));
    }

    if !data_paths.is_empty() {
        let data_size: u64 = data_paths.iter().map(|p| path_size(p)).sum();
        freed_bytes += data_size;
        entries.push((
            data_paths[0].clone(),
            format!("{} 数据", name),
            data_paths,
            true,
            data_size,
        ));
    }

    if !cache_paths.is_empty() {
        let cache_size: u64 = cache_paths.iter().map(|p| path_size(p)).sum();
        freed_bytes += cache_size;
        entries.push((
            cache_paths[0].clone(),
            format!("{} 缓存", name),
            cache_paths,
            true,
            cache_size,
        ));
    }

    if entries.is_empty() {
        return UninstallAppReport::blocked(i18n::t_lang(lang_en, "uninstall_nothing").to_string());
    }

    // 6) 执行：同一套 start_delete（安全闸门 + 废纸篓 + M-2 备份清单 + 待删快照）。
    //    官方卸载器已在上一步处理过，这里传 false 避免二次触发。
    let cancel = Arc::new(AtomicBool::new(false));
    let mut delete_rx: Option<mpsc::Receiver<DeleteMessage>> = None;
    start_delete(entries, lang_en, &mut delete_rx, false, false, cancel);

    let mut report = UninstallAppReport {
        status: "done".to_string(),
        freed_bytes,
        ..Default::default()
    };

    if let Some(rx) = delete_rx {
        while let Ok(msg) = rx.recv() {
            match msg {
                DeleteMessage::Log(.., ok) => {
                    if ok {
                        report.deleted += 1;
                    } else {
                        report.intercepted += 1;
                    }
                }
                DeleteMessage::Skip(..) => report.skipped += 1,
                DeleteMessage::BackupRecorded {
                    id,
                    restorable,
                    total,
                } => {
                    report.backup_id = Some(id);
                    report.restorable = restorable;
                    report.total = total;
                }
                DeleteMessage::Done => break,
                _ => {}
            }
        }
    }

    // 卸载改变了大目录占用：丢弃只读体积缓存（与 clean_execute 一致）
    scanner::sizecache::invalidate_all();

    report.message = if report.intercepted == 0 {
        tf_lang(
            lang_en,
            "uninstall_done",
            &[&name, &report.deleted.to_string()],
        )
    } else {
        tf_lang(
            lang_en,
            "uninstall_partial",
            &[&name, &report.intercepted.to_string()],
        )
    };

    logger::info(&format!(
        "[uninstall] {} 一键卸载完成: {} 项, {} 项被拦截, 涉及 {} 字节",
        name, report.deleted, report.intercepted, freed_bytes
    ));

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_paths_are_rejected_without_touching_fs() {
        // 白名单边界：这些路径必须在触碰文件系统之前就被拒绝
        assert!(!is_uninstallable_app_path("/etc/passwd"));
        assert!(!is_uninstallable_app_path("/Users/Shared/Evil.app"));
        assert!(!is_uninstallable_app_path("/tmp/foo.app"));
        assert!(!is_uninstallable_app_path("/Applications/Foo"));
        assert!(!is_uninstallable_app_path(
            "/Applications/../../etc/Foo.app"
        ));
        assert!(!is_uninstallable_app_path(""));
        assert!(!is_uninstallable_app_path("~/Applications/Foo.app"));
    }

    #[test]
    fn uninstall_app_rejects_non_whitelist_path() {
        // 不存在的越界路径：必须返回 blocked，绝不能尝试删除
        let r = uninstall_app("/tmp/maclean-nonexistent-xyz.app", false, false);
        assert_eq!(r.status, "blocked");
        assert!(r.deleted == 0 && r.intercepted == 0);
        assert!(!r.message.is_empty());
    }

    #[test]
    fn system_applications_paths_are_not_uninstallable() {
        // /System 路径不在白名单内，即使看起来像 .app 也必须拒绝
        assert!(!is_uninstallable_app_path(
            "/System/Applications/Safari.app"
        ));
        assert!(!is_uninstallable_app_path(
            "/System/Applications/Utilities/Terminal.app"
        ));
    }
}
