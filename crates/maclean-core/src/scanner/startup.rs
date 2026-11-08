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

    // H-09 · 用户域与系统域分别枚举，系统服务不再被误标 enabled=false
    let (user_loaded, system_loaded) = loaded_labels();

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
                // 用户级项看用户域；系统级项看系统域（并兼容其在用户域可见）
                let enabled = if scope == "user" {
                    user_loaded.contains(&label)
                } else {
                    system_loaded.contains(&label) || user_loaded.contains(&label)
                };
                items.push(StartupItem {
                    label,
                    plist: path,
                    scope: scope.to_string(),
                    enabled,
                });
            }
        }
    }
    finalize_startup_items(items)
}

/// H-09 · 排序收尾（抽成纯函数，开发机可做行为测试；不再让 scan 在目录
/// 为空时「循环零次即通过」）。同状态按 Label 升序。
fn finalize_startup_items(mut items: Vec<StartupItem>) -> Vec<StartupItem> {
    items.sort_by(|a, b| {
        b.enabled
            .cmp(&a.enabled)
            .then_with(|| a.label.cmp(&b.label))
    });
    items
}

/// 读取 plist 中的 Label（兼容 XML 与二进制 plist）
pub fn read_label(path: &Path) -> Option<String> {
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
    // C-12 · 带超时 + 退出码检查（不再裸跑 output()，失败与「没有结果」可区分）
    let text = command_stdout("plutil", &["-convert", "xml1", "-o", "-", path.to_str()?])?;
    let idx = text.find("<key>Label</key>")?;
    let rest = &text[idx..];
    let open = rest.find("<string>")?;
    let start = open + "<string>".len();
    let close = rest[start..].find("</string>")?;
    Some(rest[start..start + close].to_string())
}

/// H-09 · 解析 `launchctl list` 输出（PID Status Label，跳过表头）。
/// 抽成纯函数，开发机上可做表驱动单测（旧测试只对自己写的常量断言 contains）。
fn parse_loaded_labels(text: &str) -> std::collections::HashSet<String> {
    text.lines()
        .skip(1) // 表头
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _pid = parts.next()?;
            let _status = parts.next()?;
            parts.next().map(str::to_string)
        })
        .collect()
}

/// H-09 · 解析 `launchctl print system` 输出里的服务 label。
/// 只收「含点号、形如 bundle id」的 token，避免把状态词/十六进制地址当服务名。
fn parse_system_service_labels(text: &str) -> std::collections::HashSet<String> {
    text.lines()
        .flat_map(|line| line.split_whitespace())
        .filter(|tok| {
            tok.contains('.')
                && !tok.starts_with("0x")
                && tok
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        })
        .map(str::to_string)
        .collect()
}

/// C-12 · 带超时 + 退出码检查地读命令 stdout。
/// 失败 / 超时 / 未启动都记日志并返回 None；「确实没有结果」是
/// Success(空输出) → Some("")，二者在 CommandOutcome 层已可区分。
fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    match super::io_pool::command_output_checked(program, args, std::time::Duration::from_secs(5)) {
        super::io_pool::CommandOutcome::Success(out) => {
            Some(String::from_utf8_lossy(&out.stdout).to_string())
        }
        super::io_pool::CommandOutcome::Failed(_) => {
            crate::logger::warn(&format!("{} 退出码非 0，按未知处理", program));
            None
        }
        super::io_pool::CommandOutcome::Timeout => {
            crate::logger::warn(&format!("{} 执行超时，按未知处理", program));
            None
        }
        super::io_pool::CommandOutcome::SpawnError(e) => {
            crate::logger::warn(&format!("{} 无法启动: {}", program, e));
            None
        }
    }
}

/// 收集当前已加载的 Label：用户域（launchctl list）+ 系统域（launchctl print system）。
///
/// H-09 · 旧实现只用 `launchctl list`（当前用户域），而目录清单里含
/// /Library/LaunchAgents 与 LaunchDaemons，系统服务必被误标 enabled=false。
/// 现在系统服务单独用 `launchctl print system` 枚举。
fn loaded_labels() -> (
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
) {
    let user = command_stdout("launchctl", &["list"])
        .map(|s| parse_loaded_labels(&s))
        .unwrap_or_default();
    let system = command_stdout("launchctl", &["print", "system"])
        .map(|s| parse_system_service_labels(&s))
        .unwrap_or_default();
    (user, system)
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
/// 调用点在 cli 的 cmd_startup（macOS 专属），其它平台编译但不使用。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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
        // H-09 · 不再对自己写的常量断言 contains("Label")，直接表驱动测纯函数
        let sample = "PID\tStatus\tLabel\n\
                      123\t0\tcom.apple.example.one\n\
                      -\t0\tcom.apple.example.two\n";
        let set = parse_loaded_labels(sample);
        assert!(set.contains("com.apple.example.one"));
        assert!(set.contains("com.apple.example.two"));
        assert!(!set.contains("Label"), "表头不能被当成服务名");
        assert_eq!(set.len(), 2);
        // 反向验证：去掉 skip(1)（表头），Label 会被算进集合，断言必红
    }

    #[test]
    fn loaded_labels_skips_malformed_lines() {
        let set = parse_loaded_labels("PID Status Label\nshort\n\n\t123\t0\tcom.apple.x\n");
        assert!(set.contains("com.apple.x"));
        assert!(!set.contains("short"), "字段不足的行不能产出服务名");
    }

    #[test]
    fn system_service_labels_are_parsed_from_launchctl_print() {
        let sample =
            "state = active\n  0x1f  com.apple.sysmond  M  A\n  12345  com.example.daemon  D\n";
        let set = parse_system_service_labels(sample);
        assert!(set.contains("com.apple.sysmond"));
        assert!(set.contains("com.example.daemon"));
        assert!(!set.contains("0x1f"), "十六进制地址不能被当成服务名");
        assert!(!set.contains("state"));
    }

    #[test]
    fn startup_items_are_sorted_with_same_state_by_label() {
        // H-09 · scan 在目录为空时循环零次即通过；这里直接测生产排序函数
        let mk = |label: &str, enabled: bool| StartupItem {
            label: label.to_string(),
            plist: PathBuf::from(format!("/Library/LaunchAgents/{}.plist", label)),
            scope: "system".to_string(),
            enabled,
        };
        let items = vec![mk("b", true), mk("a", true), mk("d", false), mk("c", false)];
        let sorted = finalize_startup_items(items);
        let on: Vec<&str> = sorted
            .iter()
            .filter(|i| i.enabled)
            .map(|i| i.label.as_str())
            .collect();
        let off: Vec<&str> = sorted
            .iter()
            .filter(|i| !i.enabled)
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(on, vec!["a", "b"]);
        assert_eq!(off, vec!["c", "d"]);
        // 反向验证：把 finalize_startup_items 改成不排序，上面两条必红
    }

    #[test]
    fn scan_returns_items_with_required_fields() {
        // 真实机器上至少能扫出一些（哪怕 0 个），字段必须完整。
        // 排序/解析逻辑由上面几个行为测试覆盖（不依赖目录里恰好有启动项）。
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
