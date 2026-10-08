//! 自动更新检查（GitHub Releases）
//!
//! 启动后在后台线程查询最新 Release，与当前版本（CARGO_PKG_VERSION）比较。
//! 使用系统自带 curl（macOS / Windows 10+ 均内置），不引入额外 HTTP/TLS 依赖。
//! 任何网络或解析失败都静默返回 None，不影响主流程。

use serde::Deserialize;

/// GitHub Releases API（latest 端点只返回正式版，不含 pre-release）
const RELEASES_API: &str = "https://api.github.com/repos/jamesfeng2009/maclean/releases/latest";

/// 检测到的新版本信息
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    /// 最新版本号（已去掉 v 前缀）
    pub version: String,
    /// Release 下载页
    pub url: String,
    /// 更新摘要（截取前 3 行）
    pub notes: String,
    /// 该 Release 的全部产物
    pub assets: Vec<UpdateAsset>,
    /// 为本机挑出的那一个产物（没有匹配项时 None）
    pub asset: Option<UpdateAsset>,
}

/// Release 里的一个产物
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<UpdateAsset>,
}

/// 为本机挑出该下的产物（纯函数）
///
/// # 为什么不写死文件名
///
/// 产物名现在有 `maclean-0.2.0-arm64.dmg`、`maclean-windows-x86_64-0.2.0.zip`
/// 两套命名，而且是手工上传的 Release。写死文件名等于把打包脚本的命名
/// 约定复制一份到这里 —— 哪天改了命名，更新功能就静默失效（找不到产物，
/// 用户只会看到"暂无可用更新"）。改成按后缀 + 架构关键字匹配，脆弱性低得多。
///
/// `target` 传 Rust 的 target triple（`std::env::consts::ARCH` + OS），
/// 这样测试可以直接构造三元组，不依赖本机到底是什么架构。
pub fn pick_asset<'a>(assets: &'a [UpdateAsset], target: &str) -> Option<&'a UpdateAsset> {
    let (os, arch) = split_target(target);
    let is_macos = os == "macos";
    let is_windows = os == "windows";

    // 架构关键字：同一架构在产物名里可能有多种写法，都要认
    let arch_tokens: &[&str] = match arch {
        "aarch64" => &["aarch64", "arm64"],
        "x86_64" => &["x86_64", "x64", "intel"],
        _ => &[arch],
    };

    let mut candidates: Vec<&UpdateAsset> = assets
        .iter()
        .filter(|a| {
            let n = a.name.to_ascii_lowercase();
            if is_macos {
                n.ends_with(".dmg") || n.ends_with(".zip")
            } else if is_windows {
                n.ends_with(".zip") || n.ends_with(".exe") || n.ends_with(".msi")
            } else {
                false
            }
        })
        .filter(|a| {
            // 跨平台关键字互斥：macOS 不能拿 windows 的包，反之亦然
            let n = a.name.to_ascii_lowercase();
            if is_macos {
                !n.contains("windows") && !n.contains("win64")
            } else if is_windows {
                n.contains("windows") || n.contains("win64") || !n.contains("macos")
            } else {
                false
            }
        })
        .collect();

    // 架构能匹配上的优先；匹配不上就退回第一个同平台的包
    if let Some(hit) = candidates.iter().find(|a| {
        arch_tokens
            .iter()
            .any(|t| a.name.to_ascii_lowercase().contains(t))
    }) {
        return Some(*hit);
    }
    if candidates.is_empty() {
        None
    } else {
        candidates.sort_by_key(|a| std::cmp::Reverse(a.size));
        Some(candidates[0])
    }
}

/// 从 target triple 拆出 (os, arch)
fn split_target(target: &str) -> (&str, &str) {
    // 形如 aarch64-apple-darwin / x86_64-pc-windows-msvc
    let mut parts = target.split('-');
    let arch = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    let os = if target.contains("darwin") || target.contains("apple") {
        "macos"
    } else if target.contains("windows") {
        "windows"
    } else {
        rest
    };
    (os, arch)
}

/// 找出某个产物对应的 .sha256 校验文件（纯函数）
pub fn pick_sha256_asset<'a>(
    assets: &'a [UpdateAsset],
    asset_name: &str,
) -> Option<&'a UpdateAsset> {
    let want = format!("{}.sha256", asset_name).to_ascii_lowercase();
    let loose = format!("{}.sha256", asset_name.trim_end_matches(".exe")).to_ascii_lowercase();
    assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase() == want)
        .or_else(|| assets.iter().find(|a| a.name.to_ascii_lowercase() == loose))
}

/// 当前机器的 target triple
fn current_target() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// 查询最新版本，有更新时返回 Some（阻塞式，必须放后台线程调用）
pub fn check_latest() -> Option<UpdateInfo> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: maclean-updater",
            RELEASES_API,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }

    let release: ReleaseJson = serde_json::from_slice(&out.stdout).ok()?;
    let latest = release.tag_name.trim_start_matches('v');
    let current = env!("CARGO_PKG_VERSION");

    if !version_newer(latest, current) {
        return None;
    }

    let target = current_target();
    let asset = pick_asset(&release.assets, &target).cloned();

    Some(UpdateInfo {
        version: latest.to_string(),
        url: release.html_url,
        notes: release.body.lines().take(3).collect::<Vec<_>>().join("\n"),
        assets: release.assets,
        asset,
    })
}

/// 解析 .sha256 文件的内容（纯函数）
///
/// `sha256sum` 的输出是 "hash  文件名"，`certutil` 的输出带标题行。
/// 这里只认第一段 64 位十六进制 —— 别的实现细节一概不猜。
pub fn parse_sha256_text(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|s| s.to_ascii_lowercase())
}

/// 计算文件的 sha256（hex）
pub fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    use sha2::Digest;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut f, &mut hasher).map_err(|e| e.to_string())?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect())
}

/// 校验下载产物的 sha256
///
/// 校验和拿不到时返回 Ok(false) 而不是 Err —— 那是"没得校验"，
/// 和"校验不通过"是两件事，调用方要能区分开。
pub fn verify_sha256(path: &std::path::Path, expected_text: &str) -> Result<bool, String> {
    let Some(expected) = parse_sha256_text(expected_text) else {
        return Ok(false);
    };
    let actual = sha256_file(path)?;
    Ok(actual == expected)
}

/// 下载产物到指定路径
///
/// 用 curl 而不是引入 HTTP 依赖：macOS 与 Windows 10+ 都内置。
/// `-f` 保证 404 / 5xx 不会写成一个 HTML 错误页再被当成安装包打开。
pub fn download(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let status = std::process::Command::new("curl")
        .args(["-fL", "--max-time", "600", "-o"])
        .arg(dest)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("无法启动 curl: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err("下载失败（网络不可达或产物不存在）".to_string())
    }
}

/// 下载 -> 校验 -> 交给系统打开
///
/// # 最后一步为什么不是"静默替换"
///
/// 自替换一个**正在运行**的可执行文件在 macOS 和 Windows 上都做不了稳妥：
/// Windows 上文件被占用根本写不进去；macOS 上 .app 要重签名、替换过程中
/// 用户可能正在用它。真要做到静默更新得另起一个 updater 进程，那是另一个
/// 量级的工程。所以这里把安装包交给系统打开，明确告诉用户还要做什么 ——
/// 比一个点了没反应的"一键更新"按钮诚实。
///
/// 返回的 String 是给 UI 直接显示的结果说明。
pub fn download_and_open(info: &UpdateInfo) -> Result<String, String> {
    let Some(asset) = &info.asset else {
        return Err("没有找到适用于当前系统的安装包".to_string());
    };

    let dir = crate::platform::app_data_dir().join("updates");
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建下载目录: {}", e))?;
    let dest = dir.join(&asset.name);

    download(&asset.browser_download_url, &dest)?;

    // 有校验和就验：验不过直接失败，绝不打开一个来源不明的二进制
    if let Some(s) = pick_sha256_asset(&info.assets, &asset.name) {
        let tmp = dir.join(format!("{}.sha256", asset.name));
        download(&s.browser_download_url, &tmp)?;
        let text = std::fs::read_to_string(&tmp).unwrap_or_default();
        match verify_sha256(&dest, &text) {
            Ok(true) => {}
            Ok(false) => {
                // 校验不通过就删掉：留着一个坏包，下次会拿到同一个文件
                let _ = std::fs::remove_file(&dest);
                return Err("校验和不匹配，已丢弃下载文件".to_string());
            }
            Err(e) => return Err(format!("校验失败: {}", e)),
        }
    }

    open_path(&dest)?;
    Ok(format!("已下载 {}，已打开安装包", asset.name))
}

/// 把下载好的文件交给系统（macOS: open；Windows: 在资源管理器中选中）
pub fn open_path(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("无法打开: {}", e))
    }
    #[cfg(target_os = "windows")]
    {
        // 正在运行的 exe 无法自我替换，只能把文件指给用户
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.to_string_lossy()))
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("无法打开: {}", e))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
        Err("当前平台不支持自动打开".to_string())
    }
}

/// 语义化版本比较：latest > current 返回 true
fn version_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| {
                // 允许 "0.2.0-beta" 形式，取数字前缀
                p.trim_start_matches('v')
                    .split(|c: char| !c.is_ascii_digit())
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let l = parse(latest);
    let c = parse(current);
    let len = l.len().max(c.len());
    for i in 0..len {
        let lv = l.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if lv != cv {
            return lv > cv;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str, size: u64) -> UpdateAsset {
        UpdateAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.com/{}", name),
            size,
        }
    }

    fn sample_assets() -> Vec<UpdateAsset> {
        vec![
            asset("maclean-0.2.0-arm64.dmg", 100),
            asset("maclean-0.2.0-x86_64.dmg", 120),
            asset("maclean-windows-x86_64-0.2.0.zip", 200),
            asset("maclean-windows-x86_64-0.2.0.zip.sha256", 1),
        ]
    }

    #[test]
    fn macos_picks_the_dmg_matching_its_architecture() {
        // 挑错架构的 dmg 在 Apple Silicon 上会装上 x86_64 版本走 Rosetta，
        // 用户察觉不到，但性能和兼容性问题会一直跟着他。
        let a = sample_assets();
        assert_eq!(
            pick_asset(&a, "aarch64-apple-darwin").map(|x| x.name.as_str()),
            Some("maclean-0.2.0-arm64.dmg")
        );
        assert_eq!(
            pick_asset(&a, "x86_64-apple-darwin").map(|x| x.name.as_str()),
            Some("maclean-0.2.0-x86_64.dmg")
        );
    }

    #[test]
    fn windows_never_receives_the_macos_package() {
        // 反向验证：跨平台的包一旦串台，用户会拿到一个打不开的文件
        let a = sample_assets();
        assert_eq!(
            pick_asset(&a, "x86_64-pc-windows-msvc").map(|x| x.name.as_str()),
            Some("maclean-windows-x86_64-0.2.0.zip")
        );
    }

    #[test]
    fn an_unknown_platform_gets_nothing() {
        // 宁可返回 None（UI 退回"打开下载页"），也不要随便给个包
        let a = sample_assets();
        assert!(pick_asset(&a, "x86_64-unknown-linux-gnu").is_none());
        assert!(pick_asset(&[], "aarch64-apple-darwin").is_none());
    }

    #[test]
    fn sha256_text_parsing_tolerates_both_tools() {
        let h = "a".repeat(64);
        // sha256sum：<hash>  文件名
        assert_eq!(
            parse_sha256_text(&format!("{}  maclean.exe", h)),
            Some(h.clone())
        );
        // certutil：带标题行，哈希单独一行
        let certutil = format!(
            "SHA256 hash of file:\n{}\nCertUtil: -hashfile command completed.",
            h
        );
        assert_eq!(parse_sha256_text(&certutil), Some(h.clone()));
        // 垃圾输入必须返回 None，不能拿一段空字符串去比对
        assert_eq!(parse_sha256_text("not a hash"), None);
        assert_eq!(parse_sha256_text(""), None);
    }

    #[test]
    fn sha256_sidecar_is_matched_by_name() {
        let a = sample_assets();
        assert!(pick_sha256_asset(&a, "maclean-windows-x86_64-0.2.0.zip").is_some());
        assert!(pick_sha256_asset(&a, "maclean-0.2.0-arm64.dmg").is_none());
    }

    #[test]
    fn test_version_newer() {
        assert!(version_newer("0.3.0", "0.2.0"));
        assert!(version_newer("1.0.0", "0.9.9"));
        assert!(version_newer("0.2.1", "0.2.0"));
        assert!(version_newer("v0.3.0", "0.2.0"));
        assert!(version_newer("0.2.0.1", "0.2.0"));
        assert!(!version_newer("0.2.0", "0.2.0"));
        assert!(!version_newer("0.1.9", "0.2.0"));
        assert!(!version_newer("0.2", "0.2.0"));
    }
}
