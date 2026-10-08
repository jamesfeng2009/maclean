//! 输出格式与颜色控制（CLI 契约）。
//!
//! 迁移自 maclean 根包 `src/cli.rs`（含 clap ValueEnum 派生），
//! 供根包 CLI 与未来 `maclean-cli` crate 共用。

use clap::ValueEnum;

/// 输出格式（机器可读输出永不包含 ANSI 颜色）
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum OutputFormat {
    Human,
    Json,
    Jsonl,
}

/// 人类可读输出的颜色控制
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl OutputFormat {
    /// 是否机器可读（JSON / JSONL）
    pub fn is_machine_readable(self) -> bool {
        !matches!(self, OutputFormat::Human)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_color_parse_from_clap_values() {
        assert_eq!(OutputFormat::from_str("json", true), Ok(OutputFormat::Json));
        assert_eq!(
            OutputFormat::from_str("jsonl", true),
            Ok(OutputFormat::Jsonl)
        );
        assert_eq!(
            OutputFormat::from_str("human", true),
            Ok(OutputFormat::Human)
        );
        assert_eq!(ColorMode::from_str("never", true), Ok(ColorMode::Never));
        assert!(OutputFormat::Json.is_machine_readable());
        assert!(OutputFormat::Jsonl.is_machine_readable());
        assert!(!OutputFormat::Human.is_machine_readable());
    }
}
