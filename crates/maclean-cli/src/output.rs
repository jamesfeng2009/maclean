//! 输出渲染：JSON / JSONL / 人类可读。
//!
//! 机器可读输出（JSON/JSONL）永不包含 ANSI 颜色；人类可读输出由
//! 调用方通过 `ColorMode` 决定是否着色。

use maclean_types::cli::{CliEnvelope, ColorMode, OutputFormat};

/// 人类可读的渲染结果
pub struct HumanOutput {
    pub lines: Vec<String>,
}

impl HumanOutput {
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    pub fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    /// 是否启用 ANSI 颜色（Always / Auto 在 TTY 时启用）
    pub fn color_enabled(color: ColorMode) -> bool {
        match color {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => {
                use std::io::IsTerminal;
                std::io::stdout().is_terminal()
            }
        }
    }

    /// 输出为单个字符串（自动换行）
    pub fn render(self) -> String {
        self.lines.join("\n")
    }
}

impl Default for HumanOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// 渲染一层输出：按 `format` 决定 JSON / JSONL / human。
///
/// - `command`：命令名（进 envelope）；
/// - `request_id`：请求 ID；
/// - `payload`：成功负载（json / jsonl 的 data）；
/// - `human`：人类可读行（`None` 时输出空行）。
pub fn render_output<T: serde::Serialize>(
    format: OutputFormat,
    command: &str,
    request_id: &str,
    payload: T,
    human: HumanOutput,
) -> String {
    match format {
        OutputFormat::Json => {
            let env = CliEnvelope::ok(request_id, command, payload);
            serde_json::to_string_pretty(&env)
                .unwrap_or_else(|e| format!("{{\"error\":\"序列化失败: {e}\"}}"))
        }
        OutputFormat::Jsonl => {
            // JSONL：data 行 + 结束信封行
            let data = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
            let data_line = serde_json::to_string(&data).unwrap_or_else(|_| "null".into());
            let env =
                CliEnvelope::<serde_json::Value>::ok(request_id, command, serde_json::Value::Null);
            let env_line = serde_json::to_string(&env).unwrap_or_else(|_| "{}".into());
            format!("{data_line}\n{env_line}")
        }
        OutputFormat::Human => human.render(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn json_output_is_envelope_with_data() {
        let out = render_output(
            OutputFormat::Json,
            "inventory",
            "req-1",
            serde_json::json!({"total": 3}),
            HumanOutput::new(),
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["contract_version"], 1);
        assert_eq!(v["command"], "inventory");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["data"]["total"], 3);
    }

    #[test]
    fn jsonl_output_is_two_lines() {
        let out = render_output(
            OutputFormat::Jsonl,
            "scan",
            "req-2",
            serde_json::json!({"items": [1, 2]}),
            HumanOutput::new(),
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        let data: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(data["items"].as_array().unwrap().len(), 2);
        let env: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(env["command"], "scan");
    }

    #[test]
    fn human_output_renders_lines() {
        let mut h = HumanOutput::new();
        h.push("磁盘使用率: 75%");
        h.push("可清理: 1.2 GB");
        let out = render_output(OutputFormat::Human, "check-disk", "req", (), h);
        assert_eq!(out, "磁盘使用率: 75%\n可清理: 1.2 GB");
    }

    #[test]
    fn color_enabled_matches_mode() {
        // Auto 依赖 TTY（CI 通常非 TTY）；Always/Never 是确定的
        assert!(HumanOutput::color_enabled(ColorMode::Always));
        assert!(!HumanOutput::color_enabled(ColorMode::Never));
    }
}
