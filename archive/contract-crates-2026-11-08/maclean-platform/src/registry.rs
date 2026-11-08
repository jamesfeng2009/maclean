//! Windows 注册表读取契约。
//!
//! 现有实现位于 maclean-core `platform/reg_safety.rs` 与
//! `scanner/windows_apps.rs`（注册表卸载项采集）。本模块定义契约与
//! 纯解析逻辑（`reg query` 输出解析），供未来迁移复用。

/// 注册表值类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    String(String),
    Dword(u32),
    MultiString(Vec<String>),
    ExpandString(String),
}

/// 注册表键路径（如 `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`）
pub type RegKeyPath = String;

/// 注册表读取器契约
pub trait RegistryReader {
    /// 读取某键下的值；键不存在返回 `Ok(None)`
    fn read_value(&self, key: &RegKeyPath, name: &str) -> Result<Option<RegValue>, String>;
}

/// 从 `reg query` 文本中取出某个值（纯函数，跨平台可测）。
///
/// 输出形如：`    USERPROFILE    REG_SZ    C:\Users\John Doe`
/// 两点注意：
/// - 路径可能含空格，不能按空白切完就去取最后一列；
/// - 名字要按整词匹配，否则查 `HOME` 会命中 `HOMEDRIVE`。
pub fn extract_reg_value(output: &str, name: &str) -> Option<String> {
    for raw in output.lines() {
        let line = raw.trim();
        // 名字后必须紧跟空白
        let Some(rest) = line
            .strip_prefix(name)
            .filter(|r| r.chars().next().map(|c| c.is_whitespace()).unwrap_or(false))
        else {
            continue;
        };
        // 第二列是注册表类型
        let Some((_ty, value)) = rest.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_value_with_spaces() {
        let out = "    USERPROFILE    REG_SZ    C:\\Users\\John Doe";
        assert_eq!(
            extract_reg_value(out, "USERPROFILE"),
            Some("C:\\Users\\John Doe".to_string())
        );
    }

    #[test]
    fn parse_name_must_match_whole_word() {
        let out = "    HOMEDRIVE    REG_SZ    C:";
        assert_eq!(extract_reg_value(out, "HOME"), None);
        assert_eq!(extract_reg_value(out, "HOMEDRIVE"), Some("C:".to_string()));
    }

    #[test]
    fn parse_multiline_output() {
        let out = "A\n    KEY    REG_SZ    value1\n    OTHER    REG_DWORD    0x1\n";
        assert_eq!(extract_reg_value(out, "KEY"), Some("value1".to_string()));
        assert_eq!(extract_reg_value(out, "OTHER"), Some("0x1".to_string()));
        assert_eq!(extract_reg_value(out, "NOPE"), None);
    }

    #[test]
    fn reg_value_enum_shape() {
        assert_eq!(RegValue::String("s".into()), RegValue::String("s".into()));
        assert_ne!(RegValue::Dword(1), RegValue::Dword(2));
    }
}
