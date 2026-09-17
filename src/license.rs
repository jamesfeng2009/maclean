//! License 授权模块
//!
//! 离线 Ed25519 签名验证方案：
//! - 开发者持有私钥（keygen 工具），为每个购买者签发 License Key
//! - App 内置公钥，本地验证签名，无需联网激活
//! - Key 绑定邮箱 + 机器指纹，防随意传播
//!
//! License Key 格式:
//!   MACL-<base64url(payload_json)>-<base64url(signature)>
//! payload: { "email": "...", "plan": "lifetime", "iat": 1735689600, "mid": "<机器指纹哈希,可选>" }
//!
//! 免费额度策略（Freemium）：
//! - 扫描功能永久免费
//! - 免费版累计清理 500MB 额度，用完后需激活
//! - 激活后无限制

use std::path::PathBuf;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::logger;

/// 免费版累计清理额度（500MB）
pub const FREE_CLEAN_QUOTA_BYTES: u64 = 500 * 1024 * 1024;

/// 开发者白名单模式：仅在编译期 feature `dev-mode` 开启时跳过所有 License 限制
///
/// 用途：开发/测试阶段无阻碍体验全部功能，无需真实 License。
/// 本地开发请用 `make dev`（等价于 `cargo run --features dev-mode`）。
///
/// 安全：原实现读运行时环境变量 `MACLEAN_DEV`，且没有任何 cfg/feature 门控 ——
/// 这意味着**正式发布的二进制**里只要 `MACLEAN_DEV=1 open -a Maclean` 就能
/// 绕过全部付费校验（`quota_gate_for` / `load_status` / `check_quota_allow`）。
/// 改为编译期 feature 后，不带该 feature 的构建里这个分支会被整体编译掉，
/// 二进制中不存在任何运行时绕过入口（改环境变量、注入 .env 均无效）。
pub fn is_dev_mode() -> bool {
    cfg!(feature = "dev-mode")
}

/// License payload（被签名的数据）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicensePayload {
    /// 购买者邮箱
    pub email: String,
    /// 授权计划：lifetime / yearly
    pub plan: String,
    /// 签发时间（Unix 秒）
    pub iat: u64,
    /// 绑定的机器指纹哈希（可选，空表示不绑定）
    #[serde(default)]
    pub mid: String,
}

/// License 验证结果
#[derive(Debug, Clone)]
pub enum LicenseStatus {
    /// 已激活（合法 License）
    Activated { email: String, plan: String },
    /// 免费版（未激活或 License 无效）
    Free,
}

/// 本地持久化的激活状态
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActivationRecord {
    /// License Key 原文
    key: String,
    /// 激活时的机器指纹哈希
    machine_hash: String,
    /// 激活时间
    activated_at: u64,
}

/// 免费额度使用记录
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct QuotaRecord {
    /// 免费版已累计清理的字节数
    used_bytes: u64,
}

// =========================================================================
//  公钥（由 maclean-keygen 生成，私钥由开发者离线保管，绝不入库）
// =========================================================================

/// 内置 Ed25519 公钥（hex 编码）
/// 首次构建前运行 `cargo run --bin maclean-keygen -- init` 生成并替换此值
const PUBLIC_KEY_HEX: &str = "76016327957f69e12fafca74e154b0654718b11763c31094295b9dfc2a09edcb";

// =========================================================================
//  License 验证
// =========================================================================

/// 验证 License Key 是否合法
///
/// 校验流程：
/// 1. 格式拆分（MACL-<payload>-<sig>）
/// 2. base64url 解码 payload 和签名
/// 3. Ed25519 公钥验证签名
/// 4. 若 payload 绑定了机器指纹，校验本机是否匹配
pub fn verify_license(key: &str) -> Result<LicensePayload, String> {
    let key = key.trim();
    if !key.starts_with("MACL-") {
        return Err("无效的 License 格式（应以 MACL- 开头）".to_string());
    }

    let body = &key[5..];
    // 用 `.` 分隔：base64url（A-Z a-z 0-9 - _）不含 `.`，rfind 唯一命中分隔符。
    // 不能用 `-`，它是 base64url 合法字符，签名中可能出现导致分割错位。
    let sep = body
        .rfind('.')
        .ok_or_else(|| "无效的 License 格式（缺少签名段）".to_string())?;
    let payload_b64 = &body[..sep];
    let sig_b64 = &body[sep + 1..];

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| "License payload 解码失败".to_string())?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| "License 签名解码失败".to_string())?;

    let pubkey_bytes =
        hex_decode(PUBLIC_KEY_HEX).map_err(|_| "内置公钥未配置，请联系作者".to_string())?;
    let pubkey_array: [u8; 32] = pubkey_bytes
        .try_into()
        .map_err(|_| "内置公钥格式错误".to_string())?;
    let verifying_key =
        VerifyingKey::from_bytes(&pubkey_array).map_err(|_| "内置公钥解析失败".to_string())?;

    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "License 签名长度错误".to_string())?;
    let signature = Signature::from_bytes(&sig_array);

    verifying_key
        .verify(&payload_bytes, &signature)
        .map_err(|_| "License 签名验证失败，请确认输入完整且来自官方渠道".to_string())?;

    let payload: LicensePayload =
        serde_json::from_slice(&payload_bytes).map_err(|_| "License 内容解析失败".to_string())?;

    // 机器绑定校验
    if !payload.mid.is_empty() {
        let local = machine_hash();
        if payload.mid != local {
            return Err("此 License 已绑定其他设备，请在原设备上解绑或联系支持".to_string());
        }
    }

    Ok(payload)
}

// =========================================================================
//  激活状态持久化
// =========================================================================

/// 激活记录存储路径
fn activation_path() -> PathBuf {
    config_dir().join("activation.json")
}

/// 免费额度记录存储路径
fn quota_path() -> PathBuf {
    config_dir().join("quota.json")
}

/// 跨平台配置目录
fn config_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        crate::scanner::home_dir().join("Library/Application Support/maclean")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| crate::scanner::home_dir().join("AppData/Roaming"))
            .join("maclean")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        crate::scanner::home_dir().join(".config/maclean")
    }
}

/// 当前机器的指纹哈希（SHA-256 of 平台唯一标识）
///
/// macOS: IOPlatformUUID
/// Windows: MachineGuid（注册表）
pub fn machine_hash() -> String {
    let raw = raw_machine_id();
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hasher.update(b"maclean-license-v1");
    hex_encode(&hasher.finalize())
}

/// 读取平台原始机器标识
fn raw_machine_id() -> String {
    #[cfg(target_os = "macos")]
    {
        // IOPlatformUUID 是硬件级 UUID，重装系统不变
        if let Ok(out) = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice", "-a"])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            // plist 中提取 IOPlatformUUID
            if let Some(start) = text.find("<key>IOPlatformUUID</key>") {
                if let Some(s) = text[start..].find("<string>") {
                    let rest = &text[start + s + 8..];
                    if let Some(e) = rest.find("</string>") {
                        return rest[..e].trim().to_string();
                    }
                }
            }
        }
        // 回退：hostname + username
        format!(
            "{}-{}",
            std::env::var("HOSTNAME").unwrap_or_default(),
            std::env::var("USER").unwrap_or_default()
        )
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(out) = std::process::Command::new("reg")
            .args([
                "query",
                "HKLM\\SOFTWARE\\Microsoft\\Cryptography",
                "/v",
                "MachineGuid",
            ])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if line.contains("MachineGuid") {
                    if let Some(guid) = line.split_whitespace().last() {
                        return guid.to_string();
                    }
                }
            }
        }
        format!(
            "{}-{}",
            std::env::var("COMPUTERNAME").unwrap_or_default(),
            std::env::var("USERNAME").unwrap_or_default()
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::fs::read_to_string("/etc/machine-id")
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }
}

/// 加载当前激活状态
pub fn load_status() -> LicenseStatus {
    if is_dev_mode() {
        return LicenseStatus::Activated {
            email: "dev@maclean.app".to_string(),
            plan: "dev".to_string(),
        };
    }

    let path = activation_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return LicenseStatus::Free;
    };
    let Ok(record) = serde_json::from_str::<ActivationRecord>(&text) else {
        return LicenseStatus::Free;
    };

    // 机器指纹变化（换机/重装）→ License 失效，需重新激活
    if record.machine_hash != machine_hash() {
        logger::info("机器指纹变化，License 需重新激活");
        return LicenseStatus::Free;
    }

    match verify_license(&record.key) {
        Ok(payload) => LicenseStatus::Activated {
            email: payload.email,
            plan: payload.plan,
        },
        Err(e) => {
            logger::info(&format!("已存储的 License 验证失败: {}", e));
            LicenseStatus::Free
        }
    }
}

/// 激活 License（验证并持久化）
pub fn activate(key: &str) -> Result<LicensePayload, String> {
    let payload = verify_license(key)?;

    let record = ActivationRecord {
        key: key.trim().to_string(),
        machine_hash: machine_hash(),
        activated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };

    let dir = config_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(format!("创建配置目录失败: {}", e));
    }
    let text =
        serde_json::to_string_pretty(&record).map_err(|e| format!("序列化激活记录失败: {}", e))?;
    std::fs::write(activation_path(), text).map_err(|e| format!("写入激活记录失败: {}", e))?;

    logger::info(&format!("License 激活成功: {}", payload.email));
    Ok(payload)
}

/// 取消激活（删除本地记录）
pub fn deactivate() {
    let _ = std::fs::remove_file(activation_path());
    logger::info("License 已取消激活");
}

// =========================================================================
//  免费额度
// =========================================================================

/// 读取免费额度已用量（字节）
pub fn quota_used() -> u64 {
    std::fs::read_to_string(quota_path())
        .ok()
        .and_then(|t| serde_json::from_str::<QuotaRecord>(&t).ok())
        .map(|r| r.used_bytes)
        .unwrap_or(0)
}

/// 记录本次清理用量（仅免费版需要调用）
pub fn quota_add(bytes: u64) {
    let mut record = std::fs::read_to_string(quota_path())
        .ok()
        .and_then(|t| serde_json::from_str::<QuotaRecord>(&t).ok())
        .unwrap_or_default();
    record.used_bytes = record.used_bytes.saturating_add(bytes);

    let dir = config_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Ok(text) = serde_json::to_string_pretty(&record) {
            let _ = std::fs::write(quota_path(), text);
        }
    }
}

/// 免费额度剩余（字节）
pub fn quota_remaining() -> u64 {
    FREE_CLEAN_QUOTA_BYTES.saturating_sub(quota_used())
}

/// 检查本次清理是否在免费额度内
/// 返回 None 表示允许（已激活或额度足够），Some(剩余字节) 表示超额需拦截
pub fn check_quota_allow(plan_bytes: u64) -> Option<u64> {
    if is_dev_mode() {
        return None; // 开发者模式，无限制
    }
    if matches!(load_status(), LicenseStatus::Activated { .. }) {
        return None; // 已激活，无限制
    }
    let remaining = quota_remaining();
    if plan_bytes <= remaining {
        None
    } else {
        Some(remaining)
    }
}

// =========================================================================
//  工具函数
// =========================================================================

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("hex 长度为奇数".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

// =========================================================================
//  测试
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- P0-6: 开发者后门不得存在于发布构建 ----------

    #[cfg(not(feature = "dev-mode"))]
    #[test]
    fn dev_mode_is_off_without_feature() {
        // 未启用 dev-mode feature 时，is_dev_mode() 必须恒为 false，
        // 且不能存在任何运行时绕过入口（环境变量 / .env 注入均无效）。
        assert!(!is_dev_mode());

        // 即便有人设置了历史上那个环境变量，也不得生效
        std::env::set_var("MACLEAN_DEV", "1");
        assert!(
            !is_dev_mode(),
            "MACLEAN_DEV 环境变量仍能开启开发模式 —— 发布包可被绕过"
        );
        std::env::remove_var("MACLEAN_DEV");
    }

    #[test]
    fn test_quota_default() {
        // 未激活时额度检查逻辑
        let remaining = quota_remaining();
        assert!(remaining <= FREE_CLEAN_QUOTA_BYTES);
    }

    #[test]
    fn test_invalid_key_format() {
        assert!(verify_license("bad-key").is_err());
        assert!(verify_license("MACL-onlyone").is_err());
        assert!(verify_license("").is_err());
    }

    #[test]
    fn test_machine_hash_stable() {
        // 同一台机器两次计算应一致
        assert_eq!(machine_hash(), machine_hash());
        assert_eq!(machine_hash().len(), 64); // SHA-256 hex
    }

    /// 端到端：用 vendor 私钥签发的真实 key 必须能通过 app 端公钥验证。
    /// 此 key 由 `maclean-keygen sign --email test@example.com --plan pro` 生成，
    /// mid 为空（不绑定机器），任何设备都应验证通过。
    #[test]
    fn test_verify_signed_key_end_to_end() {
        let key = "MACL-eyJlbWFpbCI6InRlc3RAZXhhbXBsZS5jb20iLCJwbGFuIjoicHJvIiwiaWF0IjoxNzg0Mjk0NTg3LCJtaWQiOiIifQ.NIEtcHorde6_OABdCKLEnaMD2M_QWOWYoxgUuVXw-i8QyzaqT3ozvFImXgQwLgxvn3NJb9Zs2oE1dZRv0dVTBg";
        let payload = verify_license(key).expect("真实签发的 key 应验证通过");
        assert_eq!(payload.email, "test@example.com");
        assert_eq!(payload.plan, "pro");
        assert!(payload.mid.is_empty());
    }

    #[test]
    fn test_tampered_key_rejected() {
        // 篡改 payload（换成 hacker 邮箱）但沿用原签名，签名校验必须失败
        let key = "MACL-eyJlbWFpbCI6ImhhY2tlckBldmlsLmNvbSIsInBsYW4iOiJwcm8iLCJpYXQiOjE3ODQyOTQ1ODcsIm1pZCI6IiJ9.NIEtcHorde6_OABdCKLEnaMD2M_QWOWYoxgUuVXw-i8QyzaqT3ozvFImXgQwLgxvn3NJb9Zs2oE1dZRv0dVTBg";
        assert!(verify_license(key).is_err());
    }
}
