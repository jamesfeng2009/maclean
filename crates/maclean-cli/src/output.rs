//! 统一输出层（E-02 / E-08 / E-10 / D-09）。
//!
//! D-09 修复：本模块原先的 `render_output` / `HumanOutput` 从未被 handler
//! 调用（零接线）。现在 handler 的 `emit_json` / `emit_error` / 结束信封
//! 全部下沉到本模块，模块成为唯一的结构化输出出口。
//!
//! 契约口径（`docs/cli-contract.md` §3）：
//! - `json`  → 单个完整 envelope 对象；
//! - `jsonl` → **每行一个完整 envelope**（E-10：不再是裸 data 行），
//!   最后一行为收尾信封（data 省略）；
//! - `human` → 由各命令自行 println，本模块不产出机器文本。
//!
//! 所有信封的语义退出码统一由 [`CliEnvelope::exit_code`] 派生（E-08）。

use std::cell::RefCell;

use maclean_types::cli::exit_code::{EXIT_SERIALIZE, EXIT_WARNINGS};
use maclean_types::cli::{CliEnvelope, OutputFormat};
use serde::Serialize;

/// 契约 §3 的 status 四值
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warnings,
    Cancelled,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warnings => "warnings",
            Status::Cancelled => "cancelled",
            Status::Error => "error",
        }
    }
}

/// 退出码 → 收尾信封的 status（E-08：warnings/cancelled/error 都要有信封）
pub fn status_for_exit_code(code: u8) -> Status {
    use maclean_types::cli::exit_code::{EXIT_CANCELLED, EXIT_CONFIRM_REQUIRED, EXIT_OK};
    match code {
        EXIT_OK => Status::Ok,
        EXIT_WARNINGS => Status::Warnings,
        EXIT_CANCELLED => Status::Cancelled,
        // 需确认 / 通用失败均属错误信封
        EXIT_CONFIRM_REQUIRED => Status::Error,
        _ => Status::Error,
    }
}

thread_local! {
    /// 当前命令名（envelope.command）
    static CURRENT_COMMAND: RefCell<&'static str> =
        const { RefCell::new("") };
    /// 本次请求 ID（一次请求一个，run_cli 生成）
    static REQUEST_ID: RefCell<String> =
        const { RefCell::new(String::new()) };
}

pub fn set_command(cmd: &'static str) {
    CURRENT_COMMAND.with(|c| *c.borrow_mut() = cmd);
}

pub fn set_request_id(id: String) {
    REQUEST_ID.with(|r| *r.borrow_mut() = id);
}

fn command() -> &'static str {
    CURRENT_COMMAND.with(|c| *c.borrow())
}

fn request_id() -> String {
    REQUEST_ID.with(|r| r.borrow().clone())
}

/// 纯渲染：构造字符串 + 由 `env.exit_code()` 派生退出码。
/// 序列化失败返回 `Err`，绝不静默降级成 `null`/`ok`（D-09 / E-02）。
fn render<T: Serialize>(
    env: CliEnvelope<T>,
    format: OutputFormat,
    pretty: bool,
) -> Result<(String, u8), serde_json::Error> {
    let code = env.exit_code();
    let s = match format {
        OutputFormat::Human => String::new(),
        OutputFormat::Json => {
            if pretty {
                serde_json::to_string_pretty(&env)?
            } else {
                serde_json::to_string(&env)?
            }
        }
        // E-10：每行一个完整 envelope
        OutputFormat::Jsonl => serde_json::to_string(&env)?,
    };
    Ok((s, code))
}

/// 落地渲染结果：机器格式写 stdout，序列化失败输出结构化错误并以
/// `EXIT_SERIALIZE`（非 0）退出。
fn finish(result: Result<(String, u8), serde_json::Error>, format: OutputFormat) -> u8 {
    match result {
        Ok((s, code)) => {
            if format != OutputFormat::Human && !s.is_empty() {
                println!("{s}");
            }
            code
        }
        Err(e) => {
            // E-02：错误详情经 serde_json 转义，不再手写插值 JSON
            eprintln!(
                "{}",
                serde_json::json!({
                    "error": "json serialization failed",
                    "detail": e.to_string(),
                })
            );
            std::process::exit(EXIT_SERIALIZE as i32);
        }
    }
}

/// 输出一层成功数据（status:"ok"）；返回由 exit_code() 派生的退出码。
/// 保持旧签名，handler 里 ~30 处调用无需改动。
pub fn emit_json<T: Serialize>(value: &T, format: OutputFormat, pretty: bool) -> u8 {
    emit_json_status(value, format, pretty, Status::Ok)
}

/// E-08：支持 status 参数（warnings / ok）。退出码由 envelope.exit_code()
/// 统一派生，调用方可用返回值，不再硬编码。
pub fn emit_json_status<T: Serialize>(
    value: &T,
    format: OutputFormat,
    pretty: bool,
    status: Status,
) -> u8 {
    let env = CliEnvelope {
        contract_version: maclean_types::CONTRACT_VERSION,
        request_id: request_id(),
        command: command().to_string(),
        status: status.as_str().to_string(),
        error_code: None,
        error_message: None,
        data: Some(value),
    };
    finish(render(env, format, pretty), format)
}

/// E-02：统一错误发射。机器格式输出完整 `CliEnvelope::error`（status:"error"）
/// 到 stdout；绝不会出现 `status:"ok"` 配失败退出码。human 写 stderr。
pub fn emit_error(format: OutputFormat, code: u8, msg: impl Into<String>) -> u8 {
    let msg = msg.into();
    if format == OutputFormat::Human {
        eprintln!("{msg}");
        return code;
    }
    let env = CliEnvelope::<()>::error(request_id(), command(), code, msg);
    finish(render(env, format, false), format)
}

/// 收尾信封行（jsonl 每行一个对象；json 仅 cancelled 时补单对象）。
pub fn emit_close_envelope(format: OutputFormat, status: Status) {
    let env: CliEnvelope<()> = CliEnvelope {
        contract_version: maclean_types::CONTRACT_VERSION,
        request_id: request_id(),
        command: command().to_string(),
        status: status.as_str().to_string(),
        error_code: None,
        error_message: None,
        data: None,
    };
    let _ = finish(render(env, format, false), format);
}

#[cfg(test)]
mod tests {
    use super::*;
    use maclean_types::cli::exit_code::{
        EXIT_CANCELLED, EXIT_CONFIRM_REQUIRED, EXIT_FAILURE, EXIT_OK,
    };
    use serde_json::Value;

    fn env_ok<T: Serialize>(data: T) -> CliEnvelope<T> {
        CliEnvelope::ok("req-1", "scan", data)
    }

    #[test]
    fn json_render_is_a_full_envelope() {
        let (s, code) = render(
            env_ok(serde_json::json!({"total": 3})),
            OutputFormat::Json,
            false,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["contract_version"], 1);
        assert_eq!(v["request_id"], "req-1");
        assert_eq!(v["command"], "scan");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["data"]["total"], 3);
        assert_eq!(code, EXIT_OK); // 由 exit_code() 派生
    }

    #[test]
    fn jsonl_render_is_one_full_envelope_per_line() {
        // E-10：Jsonl 也是完整 envelope，不是裸 data
        let (s, _) = render(
            env_ok(serde_json::json!({"items": [1, 2]})),
            OutputFormat::Jsonl,
            false,
        )
        .unwrap();
        assert!(!s.contains('\n'), "Jsonl 单行就是一个完整 envelope");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["contract_version"], 1);
        assert_eq!(v["command"], "scan");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["data"]["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn human_render_emits_no_machine_text() {
        let (s, code) = render(env_ok(()), OutputFormat::Human, false).unwrap();
        assert!(s.is_empty(), "human 不得产出机器文本");
        assert_eq!(code, EXIT_OK);
    }

    #[test]
    fn error_envelope_has_status_error_and_no_data() {
        // E-02：错误路径形状断言，status 绝不可能是 ok
        let env = CliEnvelope::<()>::error("req", "restore", EXIT_FAILURE, "manifest not found");
        let (s, code) = render(env, OutputFormat::Json, false).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["error_code"], EXIT_FAILURE as u64);
        assert_eq!(v["error_message"], "manifest not found");
        assert!(v.get("data").is_none());
        assert_eq!(code, EXIT_FAILURE);
    }

    #[test]
    fn warnings_and_cancelled_status_derive_nonzero_codes() {
        // E-08：warnings/cancelled 信封的退出码由 exit_code() 派生，非 0
        let mk = |status: &str| CliEnvelope::<()> {
            contract_version: 1,
            request_id: "r".into(),
            command: "clean".into(),
            status: status.into(),
            error_code: None,
            error_message: None,
            data: None,
        };
        let (_, w) = render(mk("warnings"), OutputFormat::Json, false).unwrap();
        assert_eq!(w, EXIT_WARNINGS);
        let (_, c) = render(mk("cancelled"), OutputFormat::Json, false).unwrap();
        assert_eq!(c, EXIT_CANCELLED);
    }

    #[test]
    fn status_for_exit_code_covers_full_spectrum() {
        assert_eq!(status_for_exit_code(EXIT_OK), Status::Ok);
        assert_eq!(status_for_exit_code(EXIT_WARNINGS), Status::Warnings);
        assert_eq!(status_for_exit_code(EXIT_CANCELLED), Status::Cancelled);
        assert_eq!(status_for_exit_code(EXIT_CONFIRM_REQUIRED), Status::Error);
        assert_eq!(status_for_exit_code(EXIT_FAILURE), Status::Error);
    }

    /// 故意返回 Err 的序列化器：反向验证「改坏生产代码测试必须变红」。
    struct Fail;
    impl Serialize for Fail {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("boom"))
        }
    }

    #[test]
    fn serialization_failure_is_not_silently_swallowed() {
        // D-09/E-02：序列化失败必须冒泡成 Err（上层转 EXIT_SERIALIZE 非 0 退出），
        // 不能像旧 render_output 那样 unwrap_or 降级后返回 exit 0。
        let r = render(env_ok(Fail), OutputFormat::Json, false);
        assert!(r.is_err(), "序列化失败被静默降级会吞错并返回 exit 0");
        assert_ne!(EXIT_SERIALIZE, EXIT_OK, "序列化失败退出码必须非 0");
    }
}
