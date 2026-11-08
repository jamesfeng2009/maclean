//! 注册表备份的可信性校验
//!
//! 这个模块刻意**不做** `cfg(target_os)` 限定，尽管它只服务于 Windows 侧。
//!
//! 原因：`platform::windows_backup` 整体是 `#[cfg(target_os = "windows")]`，
//! 代码写进去在本机（macOS）既编译不到也测不到。而这里的判定是最后一道关
//! ——通过后紧接着就是 `reg import`（往注册表里写东西），写完就追不回来了。

// H-02：不再用 `#![allow(dead_code)]` 掩盖"校验函数彻底失去调用点"。本模块
// 已由 platform/mod.rs 声明为 `pub mod`，公开 API 在 macOS 开发机上也有
// 消费者（windows_backup.rs 的源码断言）与本机行为测试。

use std::path::Path;

// =========================================================================
//  注册表备份清单（F-09 / B-06）：定义放在无 cfg 模块，便于本机行为测试
// =========================================================================

/// 一次注册表备份的记录
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RegBackupEntry {
    /// 时间戳字符串（yyyy-MM-dd HH:mm:ss，用于显示与分批）
    pub timestamp: String,
    /// 备份来源（如 win_disable_telemetry）
    pub source: String,
    /// .reg 备份文件完整路径
    pub reg_file: String,
    /// 原始注册表键路径（如 HKLM\SOFTWARE\...）
    pub reg_key: String,
}

/// 备份清单
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RegBackupManifest {
    pub entries: Vec<RegBackupEntry>,
}

/// F-09：解析备份清单。损坏 JSON 必须返回 Err，绝不 `unwrap_or_default`
/// 静默清空 —— 那会让所有备份条目凭空消失，还原入口失效。
pub fn parse_reg_manifest(content: &str) -> Result<RegBackupManifest, String> {
    serde_json::from_str::<RegBackupManifest>(content)
        .map_err(|e| format!("备份清单解析失败，拒绝按空清单继续: {}", e))
}

/// B-06：按 `(timestamp, source)` 分组，返回**最后一批**的索引。
///
/// 旧实现取 `entries.last().source` 后把历史上所有同 source 的 `.reg` 全
/// import；同 source 连续备份两次会把旧值一起写回。这里锁定最后一次备份
/// 那一批（时间戳相同的一批）。
pub fn last_backup_batch(keys: &[(String, String)]) -> Vec<usize> {
    let Some((last_t, last_s)) = keys.last() else {
        return Vec::new();
    };
    keys.iter()
        .enumerate()
        .filter(|(_, (t, s))| t == last_t && s == last_s)
        .map(|(i, _)| i)
        .collect()
}

/// F-09：powershell 取时间失败时的回退时间戳。
///
/// 用 SystemTime 秒 + pid + 进程内递增序号，永不为空、彼此唯一，
/// 避免退化为 `_<source>_<key>.reg` 后被 `reg export /y` 覆盖旧备份。
pub fn fallback_file_stamp() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}_{}", secs, std::process::id(), seq)
}

/// 把注册表键路径拆成组件（大小写不敏感比较用）
///
/// 同时认 `\` 和 `/`：`reg import` 两种分隔符都吃得下，
/// 只用其中一种切分就等于留了个能绕过子树判断的后门。
fn key_components(key: &str) -> Vec<String> {
    key.split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_lowercase())
        .collect()
}

/// `child` 是否位于 `parent` 子树内（含自身）
///
/// 必须按**完整组件**比较。`"HKLM\\Software\\ensed"` 是 `"HKLM\\Software\\e"`
/// 的字符串前缀，但并不是它的子键 —— 只比前缀会把别人的子树认进来。
pub fn key_is_within(child: &str, parent: &str) -> bool {
    let c = key_components(child);
    let p = key_components(parent);
    c.len() >= p.len() && c.iter().zip(p.iter()).all(|(a, b)| a == b)
}

/// 从 .reg 文件内容里取出它要写入的注册表键列表
///
/// 格式形如：
/// ```text
/// Windows Registry Editor Version 5.00
///
/// [HKEY_LOCAL_MACHINE\SOFTWARE\Foo\Bar]
/// "Name"="value"
/// [-HKEY_LOCAL_MACHINE\SOFTWARE\Foo\Gone]   ; 删除键
/// ```
///
/// 注意 `[-...]` 是**删除**键的形式，同样要算进来 —— 它比加值更危险。
pub fn reg_file_keys(content: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        // 兼容两种写法：`-HKEY...` 与 `-HKEY...`（不同导出版本）
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some(end) = rest.rfind(']') else {
            continue;
        };
        let inner = rest[..end].trim();
        let key = match inner.strip_prefix('-') {
            Some(k) => k.trim(),
            None => inner,
        };
        if key.is_empty() {
            continue;
        }
        keys.push(key.to_string());
    }
    keys
}

/// 备份文件是否落在我们自己的备份目录内
///
/// `canonical_candidate` 必须由调用方先做 `canonicalize()`：原始路径可能是软链
/// 或含 `..`，先 canonicalize 再比较才不会被指向别处的文件骗过。
pub fn file_is_within_backup_dir(canonical_candidate: &Path, canonical_root: &Path) -> bool {
    canonical_candidate == canonical_root || canonical_candidate.starts_with(canonical_root)
}

/// 校验一条备份项是否可以安全地被执行 `reg import`
///
/// 三重检查，任一不过都拒绝：
/// 1. 文件内容非空且确实声明了要写的键
/// 2. 声明的键全部落在 `entry.reg_key` 子树内 —— 防止 manifest 或 .reg 文件被
///    换掉后偷渡无关注册表键
/// 3. `entry.reg_key` 本身是我们认知内的根（HKLM/HKCU/HKU/HKCR，且带-SOFTWARE
///    或 Windows\\CurrentVersion 这类业务前缀），不是 `HKCR\\*` 之类的通配面
///
/// 返回 Ok(()) 表示放行，Err 里是给用户看的拒绝原因。
pub fn validate_backup_entry_content(reg_key: &str, content: &str) -> Result<(), String> {
    let keys = reg_file_keys(content);
    if keys.is_empty() {
        return Err("备份文件里没有任何注册表键，拒绝导入".to_string());
    }

    if !root_is_expected(reg_key) {
        return Err(format!("备份键 {} 不在受支持的根下，拒绝导入", reg_key));
    }

    for k in &keys {
        if !key_is_within(k, reg_key) {
            return Err(format!(
                "备份文件试图写入范围外的键 {}（备份源为 {}），拒绝导入",
                k, reg_key
            ));
        }
    }

    Ok(())
}

/// 备份键的根是否在预期范围内（B-09）
///
/// 只接受机器/用户/类注册这几类 hive 且**至少再带一个业务前缀组件**：
/// 单独一个 `HKEY_LOCAL_MACHINE` 会让 `key_is_within` 退化成恒真——manifest
/// 里把 `reg_key` 改成裸 hive，任意 HKLM 键（含 `CurrentVersion\Run`）都能
/// 通过子树检查。另外拒绝 `*` 通配面。
fn root_is_expected(reg_key: &str) -> bool {
    const HIVES: &[&str] = &[
        "hkey_local_machine",
        "hkey_current_user",
        "hkey_users",
        "hkey_classes_root",
    ];
    if reg_key.contains('*') {
        return false;
    }
    let comps = key_components(reg_key);
    if comps.len() < 2 {
        return false;
    }
    HIVES.contains(&comps[0].as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- key_is_within ----------

    #[test]
    fn within_checks_whole_components_not_prefix() {
        // 只比字符串前缀会把 "ensed" 当成 "e" 的子键
        assert!(key_is_within(
            r"HKLM\SOFTWARE\Foo\Bar",
            r"HKLM\SOFTWARE\Foo"
        ));
        assert!(!key_is_within(
            r"HKLM\SOFTWARE\EvilExtra",
            r"HKLM\SOFTWARE\Ev"
        ));
        assert!(key_is_within(r"HKLM\SOFTWARE\Foo", r"HKLM\SOFTWARE\Foo"));
        // 换 hive 不算
        assert!(!key_is_within(r"HKCU\SOFTWARE\Foo", r"HKLM\SOFTWARE\Foo"));
    }

    #[test]
    fn within_is_case_insensitive_and_slash_tolerant() {
        assert!(key_is_within(
            r"hklm\software\foo\bar",
            r"HKLM\SOFTWARE\Foo"
        ));
        // 正斜杠也要能拆：reg import 认它，我们不认就会漏判
        assert!(key_is_within("HKLM/SOFTWARE/Foo/Bar", r"HKLM\SOFTWARE\Foo"));
        assert!(!key_is_within("HKLM/SOFTWARE/Foo/Bar", r"HKLM\SOFTWARE\Ba"));
    }

    // ---------- reg_file_keys ----------

    #[test]
    fn parses_keys_including_deletion_form() {
        let content = concat!(
            "Windows Registry Editor Version 5.00\r\n",
            "\r\n",
            r"[HKEY_LOCAL_MACHINE\SOFTWARE\Foo\Bar]",
            "\r\n",
            "\"Name\"=dword:00000001\r\n",
            "\r\n",
            r"[-HKEY_LOCAL_MACHINE\SOFTWARE\Foo\Gone]",
            "\r\n",
        );
        let keys = reg_file_keys(content);
        assert_eq!(
            keys,
            vec![
                r"HKEY_LOCAL_MACHINE\SOFTWARE\Foo\Bar".to_string(),
                r"HKEY_LOCAL_MACHINE\SOFTWARE\Foo\Gone".to_string(),
            ]
        );
    }

    #[test]
    fn junk_content_yields_no_keys() {
        assert!(reg_file_keys("").is_empty());
        assert!(reg_file_keys("not a reg file at all").is_empty());
        assert!(reg_file_keys("[unterminated").is_empty());
    }

    // ---------- validate_backup_entry_content ----------

    #[test]
    fn legit_backup_passes() {
        let content = concat!(
            "Windows Registry Editor Version 5.00\r\n",
            r"[HKEY_CURRENT_USER\SOFTWARE\maclean\test]",
            "\r\n",
            "\"a\"=\"b\"\r\n",
        );
        assert!(
            validate_backup_entry_content(r"HKEY_CURRENT_USER\SOFTWARE\maclean\test", content)
                .is_ok()
        );
    }

    #[test]
    fn tampered_reg_file_smuggling_other_keys_is_rejected() {
        // 这是本模块要挡的核心场景：manifest.json 位于用户可写的 %APPDATA%，
        // 换个 reg_file 或改个文件内容，就能借"还原"往任意位置写注册表。
        let evil = concat!(
            "Windows Registry Editor Version 5.00\r\n",
            "[HKEY_LOCAL_MACHINE\\SOFTWARE\\maclean\\ok]\r\n",
            "\"a\"=\"b\"\r\n",
            // 偷渡：往 Run 键写持久化
            "[HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run]\r\n",
            "\"Evil\"=\"C:\\\\Windows\\\\Temp\\\\evil.exe\"\r\n",
        );
        let err = validate_backup_entry_content(r"HKEY_LOCAL_MACHINE\SOFTWARE\maclean\ok", evil)
            .unwrap_err();
        assert!(
            err.contains("范围外"),
            "错误信息应当说明是越界写入，实际: {}",
            err
        );
    }

    #[test]
    fn empty_and_hive_less_backup_keys_are_rejected() {
        let content = r#"[HKEY_CURRENT_USER\SOFTWARE\x]"#;
        // 空 reg_key：子树检查会退化成恒真，必须先卡住 root
        assert!(validate_backup_entry_content("", content).is_err());
        // 未知 hive
        assert!(validate_backup_entry_content(r"HKEY_ELSEWHERE\x", content).is_err());
        // 空内容
        assert!(validate_backup_entry_content(r"HKEY_CURRENT_USER\SOFTWARE\x", "").is_err());
    }

    // ---------- file_is_within_backup_dir ----------

    #[test]
    fn backup_file_must_live_under_backup_root() {
        let root = Path::new("/Users/bob/.maclean/backup/registry");
        assert!(file_is_within_backup_dir(
            Path::new("/Users/bob/.maclean/backup/registry/a.reg"),
            root
        ));
        // 逃逸到别处（调用方已 canonicalize，所以 ../ 已展开）
        assert!(!file_is_within_backup_dir(
            Path::new("/Users/bob/evil.reg"),
            root
        ));
        // 父目录不是子目录
        assert!(!file_is_within_backup_dir(
            Path::new("/Users/bob/.maclean/backup"),
            root
        ));
    }

    // ---------- 调用点检查 ----------
    //
    // 老规矩：纯函数再对，没人调用就是死代码。windows_backup.rs 在本机编译不到
    // （cfg(windows)），只能用源码级检查保证"还原入口确实接了这道校验"。

    /// H-02：断言**调用表达式**而非裸字符串包含 —— 把调用注释掉但保留
    /// 字符串的老坑必须挡住。
    fn has_live_call(body: &str, needle: &str) -> bool {
        body.lines().any(|line| {
            let t = line.trim();
            !t.starts_with("//") && !t.starts_with('*') && t.contains(needle)
        })
    }

    #[test]
    fn restore_entrypoint_actually_validates() {
        // H-02：按 `\nmod tests` 切出生产段，避免把测试模块自己的字符串数进去。
        let src = include_str!("windows_backup.rs");
        let prod = src.split("\nmod tests").next().unwrap_or(src);
        let restore = prod[prod
            .find("pub fn restore_last_backup")
            .expect("restore_last_backup 不见了")..]
            .split("\npub fn ")
            .next()
            .unwrap();

        assert!(
            has_live_call(restore, "validate_backup_entry_content("),
            "restore_last_backup 没有接入内容校验，reg import 失去防护"
        );
        assert!(
            has_live_call(restore, "file_is_within_backup_dir("),
            "restore_last_backup 没有校验 .reg 文件是否位于备份目录内"
        );
        // B-08：reg import 必须用 canonicalize 后的同一路径。
        assert!(
            has_live_call(restore, "canonical_file"),
            "reg import 未使用 canonicalize 后的路径"
        );
        assert!(
            !restore.contains("[\"import\", &entry.reg_file]"),
            "reg import 又退回使用 manifest 里的原始字符串路径"
        );
    }

    // ---------- B-09 ----------

    #[test]
    fn b09_bare_hive_and_wildcard_keys_are_rejected() {
        let content = concat!(
            "Windows Registry Editor Version 5.00\r\n",
            "[HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run]\r\n",
            "\"Evil\"=\"C:\\\\evil.exe\"\r\n",
        );
        // 裸 hive：子树检查恒真，必须拒绝
        assert!(validate_backup_entry_content("HKEY_LOCAL_MACHINE", content).is_err());
        assert!(validate_backup_entry_content(r"HKEY_LOCAL_MACHINE\*", content).is_err());
        // 反向验证：把 root_is_expected 改回只校验 hive，第一条断言必红。
        // 合法业务前缀仍放行（别把正常备份误杀）
        assert!(validate_backup_entry_content(
            r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
            content
        )
        .is_ok());
    }

    // ---------- B-06 ----------

    #[test]
    fn b06_only_the_last_batch_is_selected_for_same_source() {
        let k = |t: &str| (t.to_string(), "win_disable_telemetry".to_string());
        let keys = vec![
            k("2026-01-01 10:00:00"),
            k("2026-01-01 10:00:00"),
            k("2026-01-01 10:00:01"),
            k("2026-01-01 10:00:01"),
        ];
        // 同 source 连续备份两次 → 只还原第二批（索引 2、3）
        assert_eq!(last_backup_batch(&keys), vec![2, 3]);
        assert!(last_backup_batch(&[]).is_empty());
    }

    // ---------- F-09 ----------

    #[test]
    fn f09_corrupt_manifest_is_rejected_not_silently_emptied() {
        assert!(parse_reg_manifest("{\"entries\": [").is_err());
        assert!(parse_reg_manifest("").is_err());
        assert_eq!(
            parse_reg_manifest("{\"entries\":[]}")
                .unwrap()
                .entries
                .len(),
            0
        );
        // 反向验证：改回 unwrap_or_default，第一条断言必红。
    }

    #[test]
    fn f09_fallback_stamp_is_nonempty_and_unique() {
        let a = fallback_file_stamp();
        let b = fallback_file_stamp();
        assert!(!a.is_empty());
        assert_ne!(a, b, "回退时间戳必须唯一，避免覆盖旧 .reg 备份");
    }
}
