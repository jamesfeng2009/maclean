//! 配置模块 - JSON 配置驱动架构（P2 系列）
//!
//! 借鉴 Win11Debloat (https://github.com/Raphire/Win11Debloat) 的配置驱动设计：
//! - P2-1: 配置外部化 —— apps.json（bloatware 列表）支持用户自定义覆盖
//! - P2-2: 设置持久化 —— config.json 保存用户设置与语言，导入/导出
//!
//! 配置目录: `app_data_dir()/config/`
//!   ├── apps.json      用户自定义 bloatware 列表（存在则完全替换内置 DB）
//!   └── config.json    用户设置持久化（settings_*、语言）

use std::path::PathBuf;

/// 配置目录
pub fn config_dir() -> PathBuf {
    crate::platform::app_data_dir().join("config")
}

/// apps.json 路径（用户自定义 bloatware 列表）
pub fn apps_json_path() -> PathBuf {
    config_dir().join("apps.json")
}

/// config.json 路径（用户设置持久化）
pub fn config_json_path() -> PathBuf {
    config_dir().join("config.json")
}

// =========================================================================
//  P2-2: 用户设置持久化
// =========================================================================

/// 用户设置（持久化到 config.json）
///
/// 所有字段带默认值，缺失字段自动回退默认（向后兼容）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 界面语言（true = English）
    pub lang_en: bool,
    /// 启动时显示菜单栏图标
    pub settings_menubar_icon: bool,
    /// 自动保持 sudo 会话
    pub settings_keep_sudo: bool,
    /// 扫描结果本地缓存
    pub settings_scan_cache: bool,
    /// 扫描全部磁盘（仅 Windows 生效，默认只扫 C 盘）
    pub settings_scan_all_disks: bool,
    /// 删除前二次确认（Advanced 项目）
    pub settings_confirm_advanced: bool,
    /// 合盖时禁止删除（macOS）
    pub settings_prevent_lid_close: bool,
    /// 操作前自动创建系统还原点（Windows）
    pub settings_auto_restore_point: bool,
    /// 深色模式（false = 浅色）
    ///
    /// 首次启动时若 config.json 不存在，会用系统偏好初始化；之后以这里为准。
    pub dark_mode: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            lang_en: false,
            settings_menubar_icon: true,
            settings_keep_sudo: true,
            settings_scan_cache: true,
            settings_scan_all_disks: false,
            settings_confirm_advanced: true,
            settings_prevent_lid_close: true,
            settings_auto_restore_point: true,
            dark_mode: false,
        }
    }
}

/// config.json 是否已存在（用于判断「首次启动」）
pub fn config_exists() -> bool {
    config_json_path().exists()
}

/// 从 config.json 加载用户设置
pub fn load_config() -> AppConfig {
    let path = config_json_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            crate::logger::error(&format!("config.json 解析失败（使用默认设置）: {}", e));
            AppConfig::default()
        }),
        Err(_) => AppConfig::default(),
    }
}

/// 保存用户设置到 config.json
pub fn save_config(cfg: &AppConfig) {
    let path = config_json_path();
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    match serde_json::to_string_pretty(cfg) {
        Ok(json) => {
            if std::fs::write(&path, json).is_ok() {
                crate::logger::info("用户设置已保存");
            }
        }
        Err(e) => crate::logger::error(&format!("序列化设置失败: {}", e)),
    }
}

// =========================================================================
//  P2-2: 配置导入/导出
// =========================================================================

/// 导出配置（config.json + 可选的 apps.json）到指定目录
///
/// 返回 (success, message)
pub fn export_config(dest_dir: &std::path::Path) -> (bool, String) {
    if std::fs::create_dir_all(dest_dir).is_err() {
        return (false, "cannot_create_dest_dir".to_string());
    }
    let mut exported = 0;
    let mut errors = Vec::new();

    // config.json 总是存在（内存态保存一份再导出，保证有内容）
    let src_config = config_json_path();
    if src_config.exists() {
        match std::fs::copy(&src_config, dest_dir.join("config.json")) {
            Ok(_) => exported += 1,
            Err(e) => errors.push(format!("config.json: {}", e)),
        }
    }

    // apps.json 仅用户自定义过才导出
    let src_apps = apps_json_path();
    if src_apps.exists() {
        match std::fs::copy(&src_apps, dest_dir.join("apps.json")) {
            Ok(_) => exported += 1,
            Err(e) => errors.push(format!("apps.json: {}", e)),
        }
    }

    if exported > 0 && errors.is_empty() {
        (true, format!("exported {} file(s)", exported))
    } else if exported > 0 {
        (false, format!("partial: {}", errors.join("; ")))
    } else if errors.is_empty() {
        (false, "nothing_to_export".to_string())
    } else {
        (false, errors.join("; "))
    }
}

/// 导出用的默认目录：~/Downloads/maclean-config/
pub fn default_export_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| crate::platform::home_dir().join("Downloads"))
        .join("maclean-config")
}

/// 在系统文件管理器中打开指定目录
///
/// macOS: open / Windows: explorer
pub fn open_in_file_manager(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(dir).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
}

/// 从指定目录导入配置（config.json / apps.json 存在哪个导哪个）
///
/// 返回 (success, message)。导入后需重启生效（apps.json 在启动时加载）。
pub fn import_config(src_dir: &std::path::Path) -> (bool, String) {
    let mut imported = 0;
    let mut errors = Vec::new();

    let dst = config_dir();
    if std::fs::create_dir_all(&dst).is_err() {
        return (false, "cannot_create_config_dir".to_string());
    }

    for name in ["config.json", "apps.json"] {
        let src = src_dir.join(name);
        if src.exists() {
            // 先校验 JSON 合法性，避免导入坏文件
            match std::fs::read_to_string(&src) {
                Ok(content) => {
                    if serde_json::from_str::<serde_json::Value>(&content).is_err() {
                        errors.push(format!("{}: invalid json", name));
                        continue;
                    }
                    match std::fs::write(dst.join(name), content) {
                        Ok(_) => imported += 1,
                        Err(e) => errors.push(format!("{}: {}", name, e)),
                    }
                }
                Err(e) => errors.push(format!("{}: {}", name, e)),
            }
        }
    }

    if imported > 0 && errors.is_empty() {
        (true, format!("imported {} file(s)", imported))
    } else if imported > 0 {
        (false, format!("partial: {}", errors.join("; ")))
    } else if errors.is_empty() {
        (false, "no_config_found".to_string())
    } else {
        (false, errors.join("; "))
    }
}
