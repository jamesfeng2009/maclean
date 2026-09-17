//! 注册表备份的可信性校验
//!
//! 这个模块刻意**不做** `cfg(target_os)` 限定，尽管它只服务于 Windows 侧。
//!
//! 原因：`platform::windows_backup` 整体是 `#[cfg(target_os = "windows")]`，
//! 代码写进去在本机（macOS）既编译不到也测不到。而这里的判定是最后一道关
//! ——通过后紧接着就是 `reg import`（往注册表里写东西），写完就追不回来了。

#![allow(dead_code)]

use std::path::Path;

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

/// 备份键的根是否在预期范围内
///
/// 只接受机器/用户/类注册这几类 hive，防止有人把 `reg_key` 塞成 `HKCR\\*`
/// 或干脆是空串来把上面那条子树检查绕成恒真。
fn root_is_expected(reg_key: &str) -> bool {
    const HIVES: &[&str] = &[
        "hkey_local_machine",
        "hkey_current_user",
        "hkey_users",
        "hkey_classes_root",
    ];
    match key_components(reg_key).first() {
        Some(root) => HIVES.contains(&root.as_str()),
        None => false,
    }
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

    #[test]
    fn restore_entrypoint_actually_validates() {
        let src = include_str!("windows_backup.rs");
        let restore = src[src
            .find("pub fn restore_last_backup")
            .expect("restore_last_backup 不见了")..]
            .split("\npub fn ")
            .next()
            .unwrap();

        assert!(
            restore.contains("validate_backup_entry_content("),
            "restore_last_backup 没有接入内容校验，reg import 失去防护"
        );
        assert!(
            restore.contains("file_is_within_backup_dir("),
            "restore_last_backup 没有校验 .reg 文件是否位于备份目录内"
        );
    }
}
