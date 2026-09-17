//! maclean License 签发工具（仅开发者使用，私钥绝不分发）
//!
//! 用法:
//!   maclean-keygen init                          生成密钥对（一次性，打印公钥用于内置 App）
//!   maclean-keygen sign --email a@b.com          签发 License（不绑定机器）
//!   maclean-keygen sign --email a@b.com --mid <hash>  签发并绑定机器指纹
//!
//! 私钥存储在 ~/.maclean-vendor/secret.key（权限 0600），请勿提交到仓库。

use std::path::PathBuf;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use clap::{Parser, Subcommand};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "maclean-keygen", about = "maclean License 签发工具")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 生成密钥对（私钥存本地，公钥打印出来内置到 App 源码）
    Init,
    /// 为购买者签发 License Key
    Sign {
        /// 购买者邮箱
        #[arg(long)]
        email: String,
        /// 授权计划（lifetime / yearly）
        #[arg(long, default_value = "lifetime")]
        plan: String,
        /// 绑定机器指纹哈希（可选，购买者可提供 App 内显示的机器码）
        #[arg(long)]
        mid: Option<String>,
    },
}

#[derive(Serialize)]
struct LicensePayload<'a> {
    email: &'a str,
    plan: &'a str,
    iat: u64,
    mid: &'a str,
}

fn vendor_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".maclean-vendor")
}

fn secret_path() -> PathBuf {
    vendor_dir().join("secret.key")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn load_or_create_signing_key(create: bool) -> SigningKey {
    let path = secret_path();
    if let Ok(hex) = std::fs::read_to_string(&path) {
        let bytes = hex_decode(hex.trim()).expect("私钥文件损坏");
        let array: [u8; 32] = bytes.try_into().expect("私钥长度错误");
        return SigningKey::from_bytes(&array);
    }
    if !create {
        eprintln!("错误: 私钥不存在，请先运行 `maclean-keygen init`");
        std::process::exit(1);
    }

    // 生成新密钥对
    let mut rng = rand_core::OsRng;
    let signing_key = SigningKey::generate(&mut rng);

    std::fs::create_dir_all(vendor_dir()).expect("创建 vendor 目录失败");
    std::fs::write(&path, hex_encode(&signing_key.to_bytes())).expect("写入私钥失败");

    // Unix 权限 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    let verifying_key: VerifyingKey = signing_key.verifying_key();
    println!("=== 密钥对已生成 ===");
    println!("私钥: {:?}（已保存，权限 0600，请勿泄露/提交）", path);
    println!();
    println!("公钥（请粘贴到 src/license.rs 的 PUBLIC_KEY_HEX 常量）:");
    println!("{}", hex_encode(&verifying_key.to_bytes()));
    println!();
    println!("替换后重新编译 App 即可签发有效 License。");

    signing_key
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => {
            if secret_path().exists() {
                eprintln!(
                    "错误: 私钥已存在于 {:?}，如需重新生成请先手动删除",
                    secret_path()
                );
                std::process::exit(1);
            }
            load_or_create_signing_key(true);
        }
        Commands::Sign { email, plan, mid } => {
            let signing_key = load_or_create_signing_key(false);

            let payload = LicensePayload {
                email: &email,
                plan: &plan,
                iat: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                mid: mid.as_deref().unwrap_or(""),
            };

            let payload_json = serde_json::to_vec(&payload).expect("序列化失败");
            let signature = signing_key.sign(&payload_json);

            // 用 `.` 分隔：base64url 字符集不含 `.`，分割永远唯一（`-` 会冲突）
            let key = format!(
                "MACL-{}.{}",
                URL_SAFE_NO_PAD.encode(&payload_json),
                URL_SAFE_NO_PAD.encode(signature.to_bytes())
            );

            println!("=== License 签发成功 ===");
            println!("邮箱: {}", email);
            println!("计划: {}", plan);
            if let Some(m) = &mid {
                println!("绑定机器: {}", m);
            }
            println!();
            println!("License Key（发送给购买者）:");
            println!("{}", key);
        }
    }
}
