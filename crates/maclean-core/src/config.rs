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
    // A-12：测试钩子，单测把配置目录指向临时位置，绝不碰真实 config.json。
    #[cfg(test)]
    {
        if let Ok(guard) = TEST_CONFIG_DIR.lock() {
            if let Some(d) = guard.as_ref() {
                if !d.as_os_str().is_empty() {
                    return d.clone();
                }
            }
        }
    }
    crate::platform::app_data_dir().join("config")
}

#[cfg(test)]
static TEST_CONFIG_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn set_test_config_dir(d: PathBuf) {
    if let Ok(mut guard) = TEST_CONFIG_DIR.lock() {
        *guard = Some(d);
    }
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
    /// 卸载应用时优先调用其官方卸载器（仅 macOS 生效，M-1）
    ///
    /// 默认开：删目录不等于卸载软件，launchd 任务 / pkgutil 收据 /
    /// 系统扩展授权都要靠官方卸载器才清得掉。关掉则退回只删 .app 目录。
    pub settings_prefer_official_uninstaller: bool,
    /// 显示受保护/系统项（默认关）
    ///
    /// 系统保护路径（如 FamilyCircle，受 macOS 系统级访问控制保护、
    /// 任何权限都删不掉）在扫描时已直接排除。若仍有漏网项被探针识别为
    /// 不可删除，默认在列表中隐藏，避免"勾选 → 授权 → 失败"循环；
    /// 开启后置灰显示，便于排查。
    pub settings_show_protected_items: bool,
    /// 定时清理（C-3）：是否开启
    pub schedule_enabled: bool,
    /// 定时清理间隔（天），只取 1 / 7 / 30
    pub schedule_interval_days: u32,
    /// 上次定时清理执行的 Unix 秒，0 = 从未执行
    pub schedule_last_run: u64,
    /// 删除前二次确认（Advanced 项目）
    pub settings_confirm_advanced: bool,
    /// 删除方式策略：
    /// - `"smart"`（默认）：安全/缓存项直接永久删除（立即释放空间），
    ///   注意/高级项移入废纸篓（可恢复）；
    /// - `"trash"`：所有项一律移入废纸篓，最稳妥、可从废纸篓恢复。
    ///
    /// 无论哪种策略，注意/高级项在后端都被**强制**移入废纸篓，前端无法覆盖。
    pub settings_delete_strategy: String,
    /// 合盖时禁止删除（macOS）
    pub settings_prevent_lid_close: bool,
    /// 操作前自动创建系统还原点（Windows）
    pub settings_auto_restore_point: bool,
    /// 深色模式（false = 浅色）
    ///
    /// 首次启动时若 config.json 不存在，会用系统偏好初始化；之后以这里为准。
    pub dark_mode: bool,
    /// 重复文件扫描的用户忽略名单（子串匹配路径，命中即跳过）
    ///
    /// 与内置硬排除（site-packages 等清单管理目录）互补：这里给用户
    /// 自己声明"这些目录/文件绝不参与重复清理"，CLI 用
    /// `maclean dup-ignore add/remove/list` 维护。
    pub dup_ignore_patterns: Vec<String>,
    /// 大文件阈值（字节）：默认 100MB
    ///
    /// CLI 用 `scan --large-min-mb N` / `clean --large-min-mb N` 覆盖，
    /// 也可直接改 config.json。
    pub large_file_min_bytes: u64,
    /// 重复文件最小大小（字节）：默认 200KB
    ///
    /// CLI 用 `--dup-min-kb N` 覆盖。
    pub dup_file_min_bytes: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            lang_en: false,
            settings_menubar_icon: true,
            settings_keep_sudo: true,
            settings_scan_cache: true,
            settings_scan_all_disks: false,
            settings_prefer_official_uninstaller: true,
            settings_show_protected_items: false,
            schedule_enabled: false,
            schedule_interval_days: 7,
            schedule_last_run: 0,
            settings_confirm_advanced: true,
            settings_delete_strategy: "smart".to_string(),
            settings_prevent_lid_close: true,
            settings_auto_restore_point: true,
            dark_mode: false,
            dup_ignore_patterns: Vec::new(),
            large_file_min_bytes: 100 * 1024 * 1024,
            dup_file_min_bytes: 200 * 1024,
        }
    }
}

/// config.json 是否已存在（用于判断「首次启动」）
pub fn config_exists() -> bool {
    config_json_path().exists()
}

/// 带错误通道地加载 config.json（A-12）
///
/// 解析失败返回 Err，由调用方显式处理；绝不把损坏文件"降级"成默认配置后
/// 又悄悄写回 —— 那正是用户设置不可恢复地丢失的链路。
pub fn load_config_checked() -> Result<AppConfig, String> {
    let path = config_json_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        // 文件不存在 = 首次启动，用内存默认值但不落盘。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AppConfig::default()),
        Err(e) => return Err(format!("读取 config.json 失败: {}", e)),
    };
    serde_json::from_str(&content).map_err(|e| format!("config.json 解析失败: {}", e))
}

/// 从 config.json 加载用户设置（A-12）
///
/// 解析失败时必须**显式报错并保留原文件**：这里不写回任何东西，也不假装
/// "使用默认设置"就万事大吉；`save_config` 会在写盘前复核，损坏文件一律
/// 拒绝覆盖，用户数据不会被默认值冲掉。是否重置由用户决定（见 `reset_config`）。
pub fn load_config() -> AppConfig {
    let cfg = match load_config_checked() {
        Ok(cfg) => cfg,
        Err(e) => {
            crate::logger::error(&format!(
                "config.json 解析失败：保留原文件、拒绝写回默认（请手动修复或重置）: {}",
                e
            ));
            AppConfig::default()
        }
    };
    // CORE-U03：配置「加载/应用处」把 settings_scan_all_disks 灌进运行期开关。
    apply_runtime_config(&cfg);
    cfg
}

// =========================================================================
//  CORE-U03 · settings_scan_all_disks 接线
//
//  背景：windows_apps::should_scan_all_disks() = AtomicBool || 环境变量，是 OR
//  关系；但此前 settings_scan_all_disks 字段零读取、set_scan_all_disks 零调用，
//  config=true 时 AtomicBool 恒 false，只能靠 MACLEAN_SCAN_ALL_DISKS 生效。
//  这里在 load_config() 里把 config 值灌进 set_scan_all_disks，且**不破坏** OR：
//  只按有效值置位 true、绝不复位 false（复位会清掉别人已置位的 true，且环境
//  变量由 should_scan_all_disks 自己的 OR 分支独立承担，无需在此固化）。
// =========================================================================

/// CORE-U03 · 「扫描全部磁盘」有效判定（纯逻辑、跨平台、入参注入）。
/// 与 windows_apps::should_scan_all_disks 一致：程序内开关（config 置位）与
/// 环境变量 OR，任一为真即扫全部磁盘（环境变量优先级不被接线覆盖）。
pub fn scan_all_disks_enabled(runtime_flag: bool, env_flag: bool) -> bool {
    runtime_flag || env_flag
}

/// CORE-U03 · 环境变量真值解析（纯逻辑，入参注入；测试不碰真实进程环境）。
pub fn parse_scan_all_disks_env(raw: Option<&str>) -> bool {
    raw.map(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
    .unwrap_or(false)
}

/// CORE-U03 · 把 config 灌进运行期开关（真实生产调用点，setter 注入）。
///
/// `cfg.settings_scan_all_disks` 在此被真实读取；`env_flag` 由调用方注入
/// （生产读环境变量、测试注入定值，避免机器相关夹具）。只按 OR 有效值置位
/// true、绝不复位 false。setter 注入：生产在 Windows 注入真实的
/// `windows_apps::set_scan_all_disks`，非 Windows 注入 no-op。
pub fn apply_scan_all_disks<F: FnOnce(bool)>(cfg: &AppConfig, env_flag: bool, set: F) {
    if scan_all_disks_enabled(cfg.settings_scan_all_disks, env_flag) {
        set(true);
    }
}

/// CORE-U03 · load_config() 的应用钩子：平台分支只在 Windows 下发真实开关。
fn apply_runtime_config(cfg: &AppConfig) {
    let env_flag =
        parse_scan_all_disks_env(std::env::var("MACLEAN_SCAN_ALL_DISKS").ok().as_deref());
    #[cfg(target_os = "windows")]
    {
        apply_scan_all_disks(cfg, env_flag, |v| {
            crate::scanner::windows_apps::set_scan_all_disks(v);
        });
    }
    #[cfg(not(target_os = "windows"))]
    {
        // 非 Windows 无该运行期开关：仍跑同一套纯逻辑判定，setter 为 no-op。
        apply_scan_all_disks(cfg, env_flag, |_v| {});
    }
}

/// 保存用户设置到 config.json（A-12）
///
/// 写盘前复核现有文件：损坏的 config.json 必须原样保留，绝不因一次"内存里
/// 是默认值"的保存就把用户设置永久覆盖。合法内容走共用原子写。
pub fn save_config(cfg: &AppConfig) {
    let path = config_json_path();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if let Err(e) = serde_json::from_str::<AppConfig>(&existing) {
            crate::logger::error(&format!(
                "config.json 已损坏，拒绝覆盖以保护原文件（如需重置请调用 reset_config）: {}",
                e
            ));
            return;
        }
    }
    let json = match serde_json::to_string_pretty(cfg) {
        Ok(j) => j,
        Err(e) => {
            crate::logger::error(&format!("序列化设置失败: {}", e));
            return;
        }
    };
    match crate::atomic_write::atomic_write(&path, json.as_bytes()) {
        Ok(()) => crate::logger::info("用户设置已保存"),
        Err(e) => crate::logger::error(&format!("原子写用户设置失败: {}", e)),
    }
}

/// 用户显式决定重置配置时调用（A-12）：唯一允许覆盖损坏文件的入口。
pub fn reset_config() {
    let path = config_json_path();
    match serde_json::to_string_pretty(&AppConfig::default()) {
        Ok(json) => {
            if let Err(e) = crate::atomic_write::atomic_write(&path, json.as_bytes()) {
                crate::logger::error(&format!("重置配置失败: {}", e));
            }
        }
        Err(e) => crate::logger::error(&format!("序列化默认设置失败: {}", e)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_u03_config_flag_enables_all_disks_without_env() {
        // 跨平台纯逻辑、入参注入：不碰真实进程环境、不硬编码开发机绝对路径。
        // 反向验证：把 `||` 改成 `&&`、或把 apply_scan_all_disks 改成无条件
        // set(cfg.settings_scan_all_disks)（复位 false），下面断言立刻红。
        assert!(
            scan_all_disks_enabled(true, false),
            "config=true 不应依赖环境变量即可扫全部磁盘"
        );
        assert!(
            scan_all_disks_enabled(false, true),
            "环境变量优先级必须保留（OR 关系）"
        );
        assert!(!scan_all_disks_enabled(false, false));
        assert!(parse_scan_all_disks_env(Some("1")));
        assert!(parse_scan_all_disks_env(Some(" YES ")));
        assert!(parse_scan_all_disks_env(Some("on")));
        assert!(!parse_scan_all_disks_env(Some("0")));
        assert!(!parse_scan_all_disks_env(None));

        let cfg_true = AppConfig {
            settings_scan_all_disks: true,
            ..Default::default()
        };
        let cfg_false = AppConfig {
            settings_scan_all_disks: false,
            ..Default::default()
        };
        let mut calls: Vec<bool> = Vec::new();
        // config=true, env=false → 下发 true（config 独立生效）
        apply_scan_all_disks(&cfg_true, false, |v| calls.push(v));
        assert_eq!(calls, vec![true], "config=true 必须下发 true");
        // config=false, env=true → 仍下发 true（环境变量优先级不被覆盖）
        calls.clear();
        apply_scan_all_disks(&cfg_false, true, |v| calls.push(v));
        assert_eq!(calls, vec![true], "env=true 仍应开启扫描");
        // config=false, env=false → 不下发（绝不复位已置位的 true）
        calls.clear();
        apply_scan_all_disks(&cfg_false, false, |v| calls.push(v));
        assert!(calls.is_empty(), "不得下发 false 复位 OR 开关");
    }

    #[test]
    fn a12_broken_config_is_preserved_and_never_overwritten_by_a_default_save() {
        // A-12 验收用例：半截 JSON → 不覆盖 → 重启仍保留。
        // 合并为单测，避免全局测试目录钩子在并行用例间互相覆盖。
        // 反向验证：把 save_config 改回 std::fs::write（或去掉写前复核），
        // 下面第二条断言会立刻变红——原文件被默认值冲掉。
        let tmp = std::env::temp_dir().join(format!("maclean_cfg_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        set_test_config_dir(tmp.clone());
        let path = config_json_path();
        let broken = "{\"lang_en\": true,";
        std::fs::write(&path, broken).unwrap();

        // ① 解析失败必须显式报错，不能被当成正常默认。
        assert!(load_config_checked().is_err(), "半截 JSON 必须报错");
        // ② 任意一次保存都不得覆盖原文件（否则用户设置永久丢失）。
        save_config(&AppConfig::default());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            broken,
            "损坏的 config.json 被默认值覆盖了"
        );
        // ③ 「重启」（再次加载）仍是同一份损坏文件，行为一致。
        assert!(load_config_checked().is_err());
        // ④ 只有显式重置才允许覆盖，且落盘内容可正常解析。
        reset_config();
        let cfg = load_config_checked().expect("重置后应可解析");
        assert!(!cfg.lang_en);

        // ⑤ 正常配置往返：保存 → 重新加载拿到真实值。
        let custom = AppConfig {
            lang_en: true,
            schedule_interval_days: 30,
            ..Default::default()
        };
        save_config(&custom);
        let reloaded = load_config_checked().unwrap();
        assert!(reloaded.lang_en);
        assert_eq!(reloaded.schedule_interval_days, 30);

        set_test_config_dir(PathBuf::new());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
