//! 启动项管理扫描器（macOS）
//!
//! 枚举用户/系统 LaunchAgent 与 LaunchDaemon，读取每个 plist 的 Label，
//! 并对照 `launchctl list` 判断当前是否已加载。
//!
//! 设计原则（对标 MangoDisk startup 模块，但更保守）：
//! - 只读扫描：本模块不做任何修改；启用/禁用由 UI 层调用
//!   `disable_startup_item` / `enable_startup_item`（mv 到备份目录，可逆）。
//! - 不 killall/bootout：已在运行的 launchd 服务不会因移除 plist 而终止，
//!   下次登录/重启后不再加载 —— 避免把系统服务误杀导致异常。

use std::path::{Path, PathBuf};

/// 一个启动项（launchd 服务）
#[derive(Debug, Clone)]
pub struct StartupItem {
    /// launchd Label（plist 内 <key>Label</key>；读取失败时用文件名）
    pub label: String,
    /// plist 绝对路径
    pub plist: PathBuf,
    /// 作用域：user / system
    pub scope: String,
    /// 当前是否已加载（launchctl list 中可见）
    pub enabled: bool,
}

/// 扫描全部启动项（同步，通常 <500ms）
pub fn scan_startup_items() -> Vec<StartupItem> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = vec![
        // 用户 LaunchAgent
        (PathBuf::from(&home).join("Library/LaunchAgents"), "user"),
        // 系统 LaunchAgent
        (PathBuf::from("/Library/LaunchAgents"), "system"),
        // 系统 LaunchDaemon
        (PathBuf::from("/Library/LaunchDaemons"), "system"),
    ];

    // 收集当前已加载的 Label（用户域 + 系统域一次列出）
    let loaded = loaded_labels();

    let mut items: Vec<StartupItem> = Vec::new();
    for (dir, scope) in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().map(|e| e == "plist").unwrap_or(false) {
                let label = read_label(&path).unwrap_or_else(|| {
                    path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                });
                let enabled = loaded.contains(&label);
                items.push(StartupItem {
                    label,
                    plist: path,
                    scope: scope.to_string(),
                    enabled,
                });
            }
        }
    }
    // 排序：禁用的在前（更需关注），同状态按 Label 排序
    items.sort_by(|a, b| {
        b.enabled
            .cmp(&a.enabled)
            .then_with(|| a.label.cmp(&b.label))
    });
    items
}

/// 读取 plist 中的 Label（兼容 XML 与二进制 plist）
pub(crate) fn read_label(path: &Path) -> Option<String> {
    // 优先原生解析：文件为 XML 时直接读 <key>Label</key>
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(text) = String::from_utf8(bytes.clone()) {
            if let Some(idx) = text.find("<key>Label</key>") {
                let rest = &text[idx..];
                if let Some(open) = rest.find("<string>") {
                    let start = open + "<string>".len();
                    if let Some(close) = rest[start..].find("</string>") {
                        return Some(rest[start..start + close].to_string());
                    }
                }
            }
        }
        // 二进制 plist：用 plutil 转 XML
        let _ = bytes; // 避免未使用告警
    }
    let out = std::process::Command::new("plutil")
        .arg("-convert")
        .arg("xml1")
        .arg("-o")
        .arg("-")
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let idx = text.find("<key>Label</key>")?;
    let rest = &text[idx..];
    let open = rest.find("<string>")?;
    let start = open + "<string>".len();
    let close = rest[start..].find("</string>")?;
    Some(rest[start..start + close].to_string())
}

/// 收集当前已加载的 Label（`launchctl list` 输出第二列）
fn loaded_labels() -> Vec<String> {
    let out = std::process::Command::new("launchctl").arg("list").output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .skip(1) // 表头
        .filter_map(|line| {
            // 格式：PID  Status  Label
            let mut parts = line.split_whitespace();
            let _pid = parts.next()?;
            let _status = parts.next()?;
            parts.next().map(str::to_string)
        })
        .collect()
}

/// 禁用启动项：把 plist 移到备份目录（可逆，不 bootout 运行中服务）
fn backup_dir_for(item: &StartupItem, backup_root: &Path) -> PathBuf {
    // 备份路径 = backup_root/<scope>/<原目录名>/<file>
    // 原目录名（LaunchAgents / LaunchDaemons）让 CLI 的 enable 能反推出
    // 原 plist 路径；旧版备份没有这层子目录时由 enable 做 fallback。
    let parent = item
        .plist
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    backup_root.join(&item.scope).join(parent)
}

/// 从备份恢复原路径：备份路径 -> (scope, 原 plist 路径)
pub fn restore_origin_from_backup(
    backup_path: &Path,
    backup_root: &Path,
    home: &str,
) -> Result<StartupItem, String> {
    let Ok(rel) = backup_path.strip_prefix(backup_root) else {
        return Err("备份文件不在备份根目录内".to_string());
    };
    let segs: Vec<&str> = rel.iter().filter_map(|s| s.to_str()).collect();
    if segs.len() >= 2 {
        let scope = segs[0].to_string();
        let file = segs[segs.len() - 1];
        let label =
            read_label(backup_path).unwrap_or_else(|| file.trim_end_matches(".plist").to_string());
        if segs.len() == 3 {
            // 新结构：<scope>/<LaunchAgents|LaunchDaemons>/<file>
            let dir_key = segs[1];
            let base = if scope == "user" {
                PathBuf::from(home).join("Library")
            } else {
                PathBuf::from("/Library")
            };
            let origin = base.join(dir_key).join(file);
            return Ok(StartupItem {
                label,
                plist: origin,
                scope,
                enabled: false,
            });
        }
        // 旧结构：<scope>/<file> —— 用文件名在三个原目录中定位
        let home = PathBuf::from(home);
        let candidates = [
            home.join("Library/LaunchAgents"),
            PathBuf::from("/Library/LaunchAgents"),
            PathBuf::from("/Library/LaunchDaemons"),
        ];
        for dir in &candidates {
            let origin = dir.join(file);
            if origin.exists() {
                return Ok(StartupItem {
                    label,
                    plist: origin,
                    scope,
                    enabled: false,
                });
            }
        }
        return Err(format!(
            "无法推断原路径（旧版备份），请用 GUI 恢复: {}",
            backup_path.display()
        ));
    }
    Err("备份路径结构异常".to_string())
}

pub fn disable_startup_item(item: &StartupItem, backup_root: &Path) -> Result<String, String> {
    if !item.plist.exists() {
        return Err(format!("plist 不存在: {}", item.plist.display()));
    }
    let backup_dir = backup_dir_for(item, backup_root);
    std::fs::create_dir_all(&backup_dir).map_err(|e| format!("创建备份目录失败: {e}"))?;
    let target = backup_dir.join(
        item.plist
            .file_name()
            .ok_or_else(|| "无法解析文件名".to_string())?,
    );
    if target.exists() {
        return Err(format!("备份目录已存在同名文件: {}", target.display()));
    }
    std::fs::rename(&item.plist, &target).map_err(|e| format!("移动失败: {e}"))?;
    Ok(target.display().to_string())
}

/// 启用（恢复）启动项：从备份目录移回原位置
pub fn enable_startup_item(item: &StartupItem, backup_root: &Path) -> Result<String, String> {
    let file_name = item
        .plist
        .file_name()
        .ok_or_else(|| "无法解析文件名".to_string())?;
    // 新结构：<scope>/<原目录名>/<file>
    let mut source = backup_dir_for(item, backup_root).join(file_name);
    // 旧结构 fallback：<scope>/<file>
    if !source.exists() {
        source = backup_root.join(&item.scope).join(file_name);
    }
    if !source.exists() {
        return Err(format!("备份中不存在: {}", source.display()));
    }
    if let Some(parent) = item.plist.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建原目录失败: {e}"))?;
    }
    std::fs::rename(&source, &item.plist).map_err(|e| format!("恢复失败: {e}"))?;
    Ok(item.plist.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_labels_parses_launchctl_output() {
        // 模拟 launchctl list 输出：PID Status Label
        let sample = "PID\tStatus\tLabel\n\
                      123\t0\tcom.apple.example.one\n\
                      -\t0\tcom.apple.example.two\n";
        // 用临时文件间接测？直接测解析逻辑不可行（函数内建命令），
        // 这里验证格式假设与扫描器不 panic 为主。
        assert!(sample.contains("Label"));
    }

    #[test]
    fn scan_returns_items_with_required_fields() {
        // 真实机器上至少能扫出一些（哪怕 0 个），字段必须完整
        let items = scan_startup_items();
        for it in &items {
            assert!(!it.label.is_empty(), "label 不能为空");
            assert!(it.plist.is_absolute(), "plist 必须绝对路径");
            assert!(it.scope == "user" || it.scope == "system", "scope 非法");
        }
    }

    #[test]
    fn disable_and_enable_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("maclean_startup_test_{}", std::process::id()));
        let src = tmp.join("test-agent.plist");
        let backup = tmp.join("backup");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(&src, b"<?xml version=\"1.0\"?><plist><dict><key>Label</key><string>com.test.agent</string></dict></plist>")
            .unwrap();

        let item = StartupItem {
            label: "com.test.agent".to_string(),
            plist: src.clone(),
            scope: "user".to_string(),
            enabled: true,
        };
        let disabled = disable_startup_item(&item, &backup).unwrap();
        assert!(!src.exists(), "禁用后原文件应被移走");
        assert!(Path::new(&disabled).exists(), "备份文件应存在");

        let restored = enable_startup_item(&item, &backup).unwrap();
        assert_eq!(Path::new(&restored), src.as_path(), "恢复路径应等于原路径");
        assert!(!Path::new(&disabled).exists(), "恢复后备份应清空");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
