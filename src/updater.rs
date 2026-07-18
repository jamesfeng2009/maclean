//! 自动更新检查（GitHub Releases）
//!
//! 启动后在后台线程查询最新 Release，与当前版本（CARGO_PKG_VERSION）比较。
//! 使用系统自带 curl（macOS / Windows 10+ 均内置），不引入额外 HTTP/TLS 依赖。
//! 任何网络或解析失败都静默返回 None，不影响主流程。

use serde::Deserialize;

/// GitHub Releases API（latest 端点只返回正式版，不含 pre-release）
const RELEASES_API: &str =
    "https://api.github.com/repos/jamesfeng2009/maclean/releases/latest";

/// 检测到的新版本信息
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    /// 最新版本号（已去掉 v 前缀）
    pub version: String,
    /// Release 下载页
    pub url: String,
    /// 更新摘要（截取前 3 行）
    pub notes: String,
}

#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: String,
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

    Some(UpdateInfo {
        version: latest.to_string(),
        url: release.html_url,
        notes: release
            .body
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join("\n"),
    })
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
