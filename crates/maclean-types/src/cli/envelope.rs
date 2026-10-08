//! CLI JSON envelope（strategy §47）。
//!
//! 结构化输出是稳定契约：字段名与语义不随文案翻译改变。
//! 破坏性变更 → `contract_version` 递增（见 [`crate::CONTRACT_VERSION`]）。

use serde::{Deserialize, Serialize};

use crate::cli::exit_code;

/// CLI 统一输出信封。
///
/// ```json
/// {
///   "contract_version": 1,
///   "request_id": "uuid",
///   "command": "inventory",
///   "status": "ok",
///   "data": {}
/// }
/// ```
///
/// 错误时 `status` 为 `"error"`，并携带 `error_code`（语义退出码）与
/// `error_message`（机器可读消息）：
///
/// ```json
/// {
///   "contract_version": 1,
///   "request_id": "uuid",
///   "command": "clean",
///   "status": "error",
///   "error_code": 4,
///   "error_message": "需要确认"
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliEnvelope<T> {
    /// 契约版本（[`crate::CONTRACT_VERSION`]）
    pub contract_version: u32,
    /// 请求 ID（调用方传入；未提供则服务端生成）
    pub request_id: String,
    /// 命令名（如 "inventory" / "scan" / "clean"）
    pub command: String,
    /// 状态：`"ok"` 或 `"error"`
    pub status: String,
    /// 错误时的语义退出码（成功时省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<u8>,
    /// 错误时的机器可读消息（成功时省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// 成功负载（错误时省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T> CliEnvelope<T> {
    /// 构造成功信封
    pub fn ok(request_id: impl Into<String>, command: impl Into<String>, data: T) -> Self {
        Self {
            contract_version: crate::CONTRACT_VERSION,
            request_id: request_id.into(),
            command: command.into(),
            status: "ok".to_string(),
            error_code: None,
            error_message: None,
            data: Some(data),
        }
    }

    /// 构造错误信封
    pub fn error(
        request_id: impl Into<String>,
        command: impl Into<String>,
        code: u8,
        message: impl Into<String>,
    ) -> Self {
        Self {
            contract_version: crate::CONTRACT_VERSION,
            request_id: request_id.into(),
            command: command.into(),
            status: "error".to_string(),
            error_code: Some(code),
            error_message: Some(message.into()),
            data: None,
        }
    }

    /// 是否成功
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }

    /// 语义退出码（成功 = 0；失败取 error_code，缺失时回退 1）
    pub fn exit_code(&self) -> u8 {
        if self.is_ok() {
            exit_code::EXIT_OK
        } else {
            self.error_code.unwrap_or(exit_code::EXIT_FAILURE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn envelope_ok_shape_matches_contract() {
        let e = CliEnvelope::ok("req-1", "inventory", serde_json::json!({"items": []}));
        let v: Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["contract_version"], 1);
        assert_eq!(v["request_id"], "req-1");
        assert_eq!(v["command"], "inventory");
        assert_eq!(v["status"], "ok");
        assert!(v["data"].is_object());
        assert!(v.get("error_code").is_none(), "成功信封不应带 error_code");
        assert!(e.is_ok());
        assert_eq!(e.exit_code(), 0);
    }

    #[test]
    fn envelope_error_shape_and_exit_code() {
        let e =
            CliEnvelope::<()>::error("req-2", "clean", exit_code::EXIT_CONFIRM_REQUIRED, "需确认");
        let v: Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["error_code"], 4);
        assert_eq!(v["error_message"], "需确认");
        assert!(v.get("data").is_none(), "错误信封不应携带 data");
        assert!(!e.is_ok());
        assert_eq!(e.exit_code(), 4);
    }

    #[test]
    fn envelope_roundtrip() {
        let e = CliEnvelope::ok("r", "scan", serde_json::json!({"total": 1}));
        let json = serde_json::to_string(&e).unwrap();
        let back: CliEnvelope<Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.request_id, "r");
        assert_eq!(back.command, "scan");
        assert!(back.is_ok());
    }

    #[test]
    fn error_without_code_falls_back_to_failure() {
        let e: CliEnvelope<()> = CliEnvelope {
            contract_version: 1,
            request_id: "r".into(),
            command: "clean".into(),
            status: "error".into(),
            error_code: None,
            error_message: Some("未知错误".into()),
            data: None,
        };
        assert_eq!(e.exit_code(), 1);
    }
}
