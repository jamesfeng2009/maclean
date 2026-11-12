//! 声明式清理规则子系统（P2）
//!
//! 借鉴 MangoDisk 的 declarative schema，但叠加 maclean 的安全层：
//! 每一条规则的根目录在加载时都必须通过 `safety::validate_rule_root`
//! （绝对路径、非受保护根、组件数 >= 3、无目录遍历/控制字符），
//! 扫描出的每个候选还会再过 `check_path_safety_with_category`，fail-closed。
//!
//! 规则来源：
//! 1. 内置规则（`builtin_rules.json`，随二进制嵌入，零运行时文件依赖）
//! 2. 用户自定义规则（`~/.config/maclean/rules/*.json`，可选覆盖/扩充）
//!
//! 一条规则 = 一组根目录 + 一组子目录名模式 + 最小体积阈值 + 风险等级。
//! patterns 必须非空，否则规则直接判无效（fail-closed：宁可漏扫，不可误删）。

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use serde::Deserialize;

use crate::safety;
use crate::scanner::{
    dir_size, finalize_scan_result, has_home, home_dir, Recommend, ScanItem, ScanResult, Scanner,
};

const BUILTIN_RULES_JSON: &str = include_str!("builtin_rules.json");

/// 用户自定义规则目录（相对 $HOME）
const USER_RULES_REL: &str = ".config/maclean/rules";

/// 单条声明式规则
#[derive(Debug, Clone, Deserialize)]
pub struct CleanRule {
    pub id: String,
    /// 规则显示名 i18n key（当前 UI 经 description 展示，保留供后续使用）
    #[allow(dead_code)]
    pub name_key: String,
    pub desc_key: String,
    pub category: String,
    /// "macos" | "windows" | "all"
    #[serde(default = "default_platform")]
    pub platform: String,
    /// 规则根模板，支持 $HOME / %USERPROFILE% 变量
    pub roots: Vec<String>,
    /// 子目录名/文件模式（精确名、* 通配），必须非空
    pub patterns: Vec<String>,
    /// 候选最小体积（字节），小于该值的目录不列入
    #[serde(default = "default_min_size")]
    pub min_size: u64,
    /// "Safe" | "Advanced"
    #[serde(default = "default_risk")]
    pub risk: String,
    /// 删除前必须已退出的进程名列表（对标 MangoDisk required_stopped_processes）。
    /// 任一进程仍在运行 → 该项锁定为不可删除（fail-closed），提示用户先退出。
    #[serde(default)]
    pub required_stopped_processes: Vec<String>,
}

fn default_platform() -> String {
    "all".to_string()
}
fn default_min_size() -> u64 {
    50 * 1024 * 1024
}
fn default_risk() -> String {
    "Safe".to_string()
}

impl CleanRule {
    /// 该规则是否适用于给定 OS（H-01：纯逻辑，os 由入参传入，
    /// macos / windows / 其它 三个分支都能在开发机上真跑、真测）。
    fn matches_platform_for(&self, os: &str) -> bool {
        // "all" 是「跨平台」，不能塞进某个具体 OS 的守卫里：旧写法把 36 条
        // platform="all" 的内置规则在 Linux 上全部拒掉（加载数为 0）。
        self.platform == "all" || self.platform.as_str() == os
    }

    /// 该规则是否适用于当前平台（生产入口，委托给纯逻辑）。
    fn matches_platform(&self) -> bool {
        self.matches_platform_for(std::env::consts::OS)
    }

    /// 解析并校验规则根，返回已解析的绝对路径（P0 校验失败 → None，规则整体跳过）
    fn resolved_roots(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .filter_map(|root| safety::validate_rule_root(root).ok())
            .map(std::path::PathBuf::from)
            .collect()
    }

    /// 规则整体校验：平台匹配 + 至少一个可用根 + patterns 非空
    fn validate(&self) -> Result<(), String> {
        if !self.matches_platform() {
            return Err(format!("规则 {} 平台不匹配: {}", self.id, self.platform));
        }
        if self.resolved_roots().is_empty() {
            return Err(format!("规则 {} 无可用根目录", self.id));
        }
        if self.patterns.is_empty() {
            return Err(format!(
                "规则 {} patterns 为空（fail-closed 拒绝）",
                self.id
            ));
        }
        if !matches!(self.risk.as_str(), "Safe" | "Advanced") {
            return Err(format!("规则 {} risk 非法: {}", self.id, self.risk));
        }
        Ok(())
    }

    /// 目录名/文件名是否匹配任一模式
    fn pattern_matches(&self, name: &str) -> bool {
        self.patterns.iter().any(|pat| {
            if let Some(suffix) = pat.strip_prefix('*') {
                // "*suffix"：以 suffix 结尾
                name.ends_with(suffix)
            } else {
                // 精确匹配（含嵌套路径如 "Service Worker/CacheStorage"）
                name == pat || name.starts_with(&format!("{}/", pat))
            }
        })
    }
}

/// 声明式规则扫描器
#[derive(Debug, Default)]
pub struct RuleScanner {
    rules: Vec<CleanRule>,
}

impl RuleScanner {
    pub fn new() -> Self {
        let mut rules = Self::load_builtin();
        rules.extend(Self::load_user_rules());
        Self { rules }
    }

    /// 内置规则（嵌入二进制）
    fn load_builtin() -> Vec<CleanRule> {
        Self::parse_rules(BUILTIN_RULES_JSON, "builtin")
    }

    /// 用户自定义规则：~/.config/maclean/rules/*.json
    fn load_user_rules() -> Vec<CleanRule> {
        if !has_home() {
            return Vec::new();
        }
        let dir = home_dir().join(USER_RULES_REL);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut rules = Vec::new();
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(text) = std::fs::read_to_string(&p) {
                    rules.extend(Self::parse_rules(&text, &p.to_string_lossy()));
                }
            }
        }
        rules
    }

    fn parse_rules(json: &str, source: &str) -> Vec<CleanRule> {
        let parsed: Result<Vec<CleanRule>, _> = serde_json::from_str(json);
        let parsed = match parsed {
            Ok(v) => v,
            Err(e) => {
                crate::logger::info(&format!("规则文件解析失败 {}: {}", source, e));
                return Vec::new();
            }
        };
        let mut ok = Vec::new();
        for rule in parsed {
            match rule.validate() {
                Ok(()) => ok.push(rule),
                // 被拒绝的规则逐条记录：数量少、且是用户需要知道的可行动信息
                // （自定义规则写错了要能定位到具体原因）。
                Err(reason) => {
                    crate::logger::info(&format!("规则被拒绝: {} ({})", reason, source));
                }
            }
        }
        // 加载成功只汇总一行：原来每条规则一行，内置 4 条 × 每次扫描 = 主日志
        // 第二大噪声源（2026-11-12 实测一天 1452 行）。
        if !ok.is_empty() {
            crate::logger::info(&format!("规则已加载 {} 条 ({})", ok.len(), source));
        }
        ok
    }
}

impl Scanner for RuleScanner {
    fn scan(&self) -> ScanResult {
        let start = std::time::Instant::now();
        let mut items = Vec::new();

        // 占用进程（删前要求停进程）：全扫描只取一次系统进程快照。
        // A-06：None = 快照未知（命令失败/非零退出/平台不支持），由
        // required_processes_decision 对声明了 required_stopped_processes 的项 fail-closed 锁定。
        let running = running_process_names();
        for rule in &self.rules {
            for root in rule.resolved_roots() {
                items.extend(scan_rule_root(&root, rule, running.as_deref()));
            }
        }

        // C-17：去重 / 排序 / 汇总收敛到共享 helper（各 Scanner 同一口径）。
        finalize_scan_result(items, start.elapsed().as_millis() as u64)
    }
}

/// 扫描单个规则根：walkdir 收集匹配子目录，过滤体积阈值与安全校验
fn scan_rule_root(root: &Path, rule: &CleanRule, snapshot: Option<&[String]>) -> Vec<ScanItem> {
    if !root.is_dir() {
        return Vec::new();
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    let walker = walkdir::WalkDir::new(root)
        .min_depth(1)
        .max_depth(3)
        .follow_links(false);
    for entry in walker.into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // 模式匹配整个相对路径（支持 "Service Worker/CacheStorage" 这类嵌套名）
        let rel = entry
            .path()
            .strip_prefix(root)
            .map(|r| r.to_string_lossy().to_string())
            .unwrap_or_default();
        if rule.pattern_matches(&name) || rule.pattern_matches(&rel) {
            candidates.push(entry.path().to_path_buf());
        }
    }

    candidates
        .par_iter()
        .filter_map(|p| {
            let p_str = p.to_string_lossy().to_string();
            // 安全层兜底：候选目录本身必须通过既有安全检查
            if !safety::gate_allows(&p_str, &rule.category) {
                return None;
            }
            let size = dir_size(p);
            if size < rule.min_size {
                return None;
            }
            // A-06：删前要求停进程走纯函数；快照未知（None）一律 fail-closed 锁定，
            // 绝不再把「空列表」当成「没有进程在跑」。
            let (deletable, undeletable_reason) =
                required_processes_decision(&rule.required_stopped_processes, snapshot);
            Some(ScanItem {
                batch_mtimes: vec![],
                path: p_str,
                size_bytes: size,
                category: rule.category.clone(),
                selected: false,
                deletable,
                undeletable_reason,
                recommend: if rule.risk == "Safe" {
                    Recommend::Safe
                } else {
                    Recommend::Advanced
                },
                description: rule.desc_key.clone(),
                batch_paths: Vec::new(),
            })
        })
        .collect()
}

/// 当前运行进程名快照（小写）。
///
/// - macOS：`ps -axo comm=`；Windows：`tasklist /FO CSV /NH`。
/// - 命令成功且退出码 0 → `Some(names)`（哪怕为空也表示「确实没在跑」）；
/// - 命令启动失败 / 非零退出 / 平台不支持 → `None`（A-06「未知」态，
///   调用方对声明了 `required_stopped_processes` 的项 fail-closed 锁定，
///   绝不当成「没有进程在跑」）。
fn running_process_names() -> Option<Vec<String>> {
    let (text, mac_format) = capture_process_snapshot()?;
    Some(parse_process_snapshot(&text, mac_format))
}

/// A-06：平台相关的进程快照采集。成功返回 (原始输出, 是否 mac 的 ps 格式)；
/// 命令启动失败 / 非零退出 / 非 mac/win 平台一律 `None`（未知）。
/// 真正的命令执行是平台分支，解析逻辑抽到无 cfg 的纯函数，见 parse_process_snapshot。
#[cfg(target_os = "macos")]
fn capture_process_snapshot() -> Option<(String, bool)> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-axo", "comm="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some((String::from_utf8_lossy(&out.stdout).into_owned(), true))
}

#[cfg(target_os = "windows")]
fn capture_process_snapshot() -> Option<(String, bool)> {
    let out = std::process::Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some((String::from_utf8_lossy(&out.stdout).into_owned(), false))
}

/// 非 mac/win 平台没有可用的进程快照命令 → 未知（fail-closed 由调用方兜底）。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn capture_process_snapshot() -> Option<(String, bool)> {
    None
}

/// A-06：解析进程快照文本为小写进程名列表（无 cfg 纯逻辑）。
/// 平台格式由 `mac_format` 入参区分：两个分支都能在开发机上真跑、真测。
/// - `true`：`ps -axo comm=`（路径取 basename）；
/// - `false`：`tasklist /FO CSV /NH`（取逗号首列、去引号）。
fn parse_process_snapshot(text: &str, mac_format: bool) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let name = if mac_format {
            line.trim()
                .rsplit('/')
                .next()
                .unwrap_or(line.trim())
                .to_lowercase()
        } else {
            line.split(',')
                .next()
                .unwrap_or("")
                .trim_matches('"')
                .to_lowercase()
        };
        if !name.is_empty() {
            names.push(name);
        }
    }
    names
}

/// A-06：删前停进程判定（纯函数，生产 scan_rule_root 与测试共用，禁止另写一套）。
/// - required 为空 → 无此约束，正常可删；
/// - snapshot=None（快照未知）→ 无法确认要求的进程已退出 → fail-closed 强制锁定并给原因；
/// - snapshot=Some 且有要求进程仍在运行 → 锁定；快照成功且要求的进程已退出 → 可删。
fn required_processes_decision(required: &[String], snapshot: Option<&[String]>) -> (bool, String) {
    if required.is_empty() {
        return (true, String::new());
    }
    let Some(running) = snapshot else {
        return (
            false,
            "required_stopped_processes:进程快照未知（fail-closed 锁定）".to_string(),
        );
    };
    let still_running: Vec<String> = required
        .iter()
        .filter(|proc_name| {
            let wanted = proc_name.to_lowercase();
            running.iter().any(|n| n == &wanted)
        })
        .cloned()
        .collect();
    if still_running.is_empty() {
        (true, String::new())
    } else {
        (
            false,
            format!("required_stopped_processes:{}", still_running.join(", ")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rules_load_and_validate() {
        let rules = RuleScanner::load_builtin();
        assert!(!rules.is_empty(), "内置规则不能为空");
        // 全部通过校验才进列表
        assert!(
            rules.len() >= 8,
            "内置规则应至少有 8 条，实际 {}",
            rules.len()
        );
        // 每个规则必须有可解析的根（P0 校验）
        for r in &rules {
            assert!(!r.resolved_roots().is_empty(), "规则 {} 无可用根", r.id);
        }
    }

    #[test]
    fn builtin_rules_full_coverage_2026_09() {
        // P0：开发者缓存/AI/容器全覆盖 —— 这是「内置规则文件」的属性，与当前
        // OS 无关。load_builtin() 会按平台过滤（Linux 天然少 8 条 macos 规则），
        // 拿它数总数会把「平台过滤正常」误判成「文件缺规则」。
        let rules: Vec<CleanRule> =
            serde_json::from_str(BUILTIN_RULES_JSON).expect("内置规则 JSON 必须可解析");
        let ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        assert!(rules.len() >= 40, "规则总数应 >= 40，实际 {}", rules.len());

        // 语言/工具链全覆盖：Rust/Node/Go/Python/Mobile/IDE 各至少 1 条
        for lang in [
            "pnpm-store",
            "yarn-cache",
            "bun-cache",
            "deno-cache",     // Node 生态
            "go-build-cache", // Go
            "uv-cache",
            "poetry-cache",
            "conda-pkgs",    // Python
            "sccache-cache", // Rust 工具链
            "cocoapods-cache",
            "swiftpm-cache",
            "flutter-cache", // Mobile
            "vscode-cache",
            "jetbrains-cache", // IDE
        ] {
            assert!(ids.contains(&lang), "缺少规则: {}", lang);
        }

        // AI 模型缓存 + 容器细分
        for ai in ["huggingface-cache", "ollama-models", "lm-studio-models"] {
            assert!(ids.contains(&ai), "缺少 AI 规则: {}", ai);
        }
        for ct in [
            "podman-data",
            "orbstack-data",
            "colima-data",
            "minikube-cache",
        ] {
            assert!(ids.contains(&ct), "缺少容器规则: {}", ct);
        }

        // 进程锁定规则必须带 required_stopped_processes
        let ollama = rules.iter().find(|r| r.id == "ollama-models").unwrap();
        assert!(
            !ollama.required_stopped_processes.is_empty(),
            "ollama 应锁定进程"
        );
        let vscode = rules.iter().find(|r| r.id == "vscode-cache").unwrap();
        assert!(
            !vscode.required_stopped_processes.is_empty(),
            "vscode 应锁定进程"
        );

        // 生产入口仍要按平台过滤：Linux 加载 36 条 all、macOS 加载全部，
        // 但绝不能因平台分支写错而一条都不加载（CI 首跑的真实回归）。
        let loaded = RuleScanner::load_builtin();
        assert!(
            loaded.len() >= 8,
            "当前平台加载的内置规则过少: {}",
            loaded.len()
        );
        assert!(
            loaded.iter().all(|r| r.matches_platform()),
            "加载了不属于当前平台的规则"
        );
    }

    #[test]
    fn invalid_rules_are_rejected_fail_closed() {
        // patterns 为空 → 拒绝
        let bad = CleanRule {
            id: "bad-empty-pattern".into(),
            name_key: "x".into(),
            desc_key: "x".into(),
            category: "x".into(),
            platform: "all".into(),
            roots: vec!["$HOME/Library/Caches/Foo".to_string()],
            patterns: vec![],
            min_size: 0,
            risk: "Safe".into(),
            required_stopped_processes: vec![],
        };
        assert!(bad.validate().is_err(), "patterns 为空应拒绝");

        // 根是受保护根 → 无可用根 → 拒绝
        let bad_root = CleanRule {
            id: "bad-root".into(),
            name_key: "x".into(),
            desc_key: "x".into(),
            category: "x".into(),
            platform: "all".into(),
            roots: vec!["/System/Library".to_string()],
            patterns: vec!["Cache".into()],
            min_size: 0,
            risk: "Safe".into(),
            required_stopped_processes: vec![],
        };
        assert!(bad_root.validate().is_err(), "受保护根应被拒绝");

        // risk 非法 → 拒绝
        let bad_risk = CleanRule {
            id: "bad-risk".into(),
            name_key: "x".into(),
            desc_key: "x".into(),
            category: "x".into(),
            platform: "all".into(),
            roots: vec!["$HOME/Library/Caches/Foo".to_string()],
            patterns: vec!["Cache".into()],
            min_size: 0,
            risk: "Whatever".into(),
            required_stopped_processes: vec![],
        };
        assert!(bad_risk.validate().is_err(), "risk 非法应拒绝");
    }

    #[test]
    fn pattern_matching_supports_exact_and_wildcard() {
        let rule = CleanRule {
            id: "t".into(),
            name_key: "x".into(),
            desc_key: "x".into(),
            category: "x".into(),
            platform: "all".into(),
            roots: vec![],
            patterns: vec!["Cache".into(), "*.log".into()],
            min_size: 0,
            risk: "Safe".into(),
            required_stopped_processes: vec![],
        };
        assert!(rule.pattern_matches("Cache"));
        assert!(rule.pattern_matches("abc.log"));
        assert!(!rule.pattern_matches("Caches"));
        assert!(!rule.pattern_matches("abc.txt"));
    }

    #[test]
    fn platform_filter_is_table_driven() {
        // H-01：平台过滤改表驱动具体输入，不再写
        // `matches_platform() == (OS == "windows")` 这种「改坏实现仍绿」的恒等式。
        // 反向验证：把 matches_platform_for 的 os 守卫去掉 / 改错，下面用例必红。
        // "all" 的语义是跨平台；旧表把 ("all","linux") 写成 false，等于把
        // 「Linux 加载不到内置规则」这个 bug 固化成期望 —— 已订正。
        let mk = |platform: &str| CleanRule {
            id: "t".into(),
            name_key: "x".into(),
            desc_key: "x".into(),
            category: "x".into(),
            platform: platform.into(),
            roots: vec![],
            patterns: vec!["Cache".into()],
            min_size: 0,
            risk: "Safe".into(),
            required_stopped_processes: vec![],
        };
        // (platform, os, expected)
        let cases = [
            ("all", "macos", true),
            ("all", "windows", true),
            ("all", "linux", true),
            ("all", "freebsd", true),
            ("macos", "macos", true),
            ("macos", "windows", false),
            ("macos", "linux", false),
            ("windows", "macos", false),
            ("windows", "windows", true),
            ("windows", "linux", false),
        ];
        for (platform, os, expected) in cases {
            assert_eq!(
                mk(platform).matches_platform_for(os),
                expected,
                "platform={platform}, os={os}"
            );
        }
    }

    #[test]
    fn scan_rule_root_respects_min_size_and_safety() {
        // 临时目录造一个 1KB 缓存子目录 + 一个 2MB 缓存子目录
        let tmp = std::env::temp_dir().join(format!("maclean_rules_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        crate::provenance::register_root(&tmp); // A-07b：规则根需显式声明准入
        let small = tmp.join("tiny");
        let big = tmp.join("BigCache");
        std::fs::create_dir_all(&small).unwrap();
        std::fs::create_dir_all(&big).unwrap();
        std::fs::write(small.join("a"), vec![0u8; 1024]).unwrap();
        std::fs::write(big.join("b"), vec![0u8; 2 * 1024 * 1024]).unwrap();

        let rule = CleanRule {
            id: "t".into(),
            name_key: "x".into(),
            desc_key: "x".into(),
            category: "测试缓存".into(),
            platform: "all".into(),
            roots: vec![tmp.to_string_lossy().into_owned()],
            patterns: vec!["BigCache".into(), "tiny".into()],
            min_size: 1024 * 1024,
            risk: "Safe".into(),
            required_stopped_processes: vec![],
        };
        let no_running: Vec<String> = Vec::new();
        let items = scan_rule_root(&tmp, &rule, Some(no_running.as_slice()));
        // 只应命中 BigCache（>=1MB），tiny 被体积阈值过滤
        assert_eq!(items.len(), 1, "应只有大目录命中: {:?}", items);
        assert_eq!(items[0].path, big.to_string_lossy());
        assert!(items[0].size_bytes >= 2 * 1024 * 1024);
        assert_eq!(items[0].recommend, Recommend::Safe);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn required_stopped_processes_locks_items_fail_closed() {
        // 规则声明需停止的进程：
        // - 快照含该进程 → 锁定不可删；
        // - 快照成功且为空（已确认进程退出）→ 可删；
        // - 快照未知（None）→ fail-closed 强制锁定（A-06：旧实现把空列表当
        //   「没进程在跑」是 fail-open，本用例不再把它写成期望）。
        let tmp = std::env::temp_dir().join(format!("maclean_proc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        crate::provenance::register_root(&tmp); // A-07b：规则根需显式声明准入
        let cache = tmp.join("ChromeCache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("a"), vec![0u8; 2 * 1024 * 1024]).unwrap();

        let rule = CleanRule {
            id: "t".into(),
            name_key: "x".into(),
            desc_key: "x".into(),
            category: "浏览器缓存".into(),
            platform: "all".into(),
            roots: vec![tmp.to_string_lossy().into_owned()],
            patterns: vec!["ChromeCache".into()],
            min_size: 1024 * 1024,
            risk: "Safe".into(),
            required_stopped_processes: vec!["Google Chrome".into()],
        };
        let running_chrome = vec!["google chrome".to_string()];
        let no_running: Vec<String> = Vec::new();

        // 进程仍在运行 → deletable=false + 原因
        let locked = scan_rule_root(&tmp, &rule, Some(running_chrome.as_slice()));
        assert_eq!(locked.len(), 1);
        assert!(!locked[0].deletable, "进程运行时必须锁定不可删");
        assert!(
            locked[0]
                .undeletable_reason
                .starts_with("required_stopped_processes:"),
            "原因应标记需退出进程: {}",
            locked[0].undeletable_reason
        );

        // 进程已退出（快照成功、空列表）→ 正常可删
        let free = scan_rule_root(&tmp, &rule, Some(no_running.as_slice()));
        assert_eq!(free.len(), 1);
        assert!(free[0].deletable, "进程退出后应可删");
        assert!(free[0].undeletable_reason.is_empty());

        // A-06：快照未知 → fail-closed 锁定（不可把「未知」当成「没有进程在跑」）
        let unknown = scan_rule_root(&tmp, &rule, None);
        assert_eq!(unknown.len(), 1);
        assert!(!unknown[0].deletable, "进程快照未知必须 fail-closed 锁定");
        assert!(unknown[0]
            .undeletable_reason
            .starts_with("required_stopped_processes:"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn required_processes_decision_pure_function_is_fail_closed() {
        // A-06：删前停进程的纯逻辑（生产 scan_rule_root 与本用例共用，禁止另写一套）。
        // 反向验证：把 None 分支改成返回 (true, "")（退回 fail-open），下面断言必红。
        let req = vec!["Google Chrome".to_string()];
        let running = vec!["google chrome".to_string()];
        let none: Vec<String> = Vec::new();
        let no_req: Vec<String> = Vec::new();

        // 无 required 约束：快照已知/未知都不锁定
        assert_eq!(
            required_processes_decision(&no_req, None),
            (true, String::new())
        );
        assert_eq!(
            required_processes_decision(&no_req, Some(none.as_slice())),
            (true, String::new())
        );

        // 快照未知 → fail-closed 锁定并给原因
        let (d, r) = required_processes_decision(&req, None);
        assert!(!d, "进程快照未知必须锁定");
        assert!(r.starts_with("required_stopped_processes:"));

        // 快照成功但要求的进程仍在 → 锁定
        let (d, r) = required_processes_decision(&req, Some(running.as_slice()));
        assert!(!d);
        assert!(r.contains("Google Chrome"));

        // 快照成功且要求的进程已退出 → 可删
        assert_eq!(
            required_processes_decision(&req, Some(none.as_slice())),
            (true, String::new())
        );
    }

    #[test]
    fn parse_process_snapshot_runs_both_platform_formats() {
        // A-06 / H-01：解析逻辑无 cfg，两个平台格式由入参区分，
        // macOS 开发机上也能真跑 Windows 的 tasklist 分支。
        let mac = "/usr/bin/Google Chrome\n/sbin/Finder\n\n";
        assert_eq!(
            parse_process_snapshot(mac, true),
            vec!["google chrome".to_string(), "finder".to_string()]
        );
        let win = "\"chrome.exe\",\"1234\",\"Console\"\r\n\"Finder.EXE\",\"9\",\"Console\"\r\n";
        assert_eq!(
            parse_process_snapshot(win, false),
            vec!["chrome.exe".to_string(), "finder.exe".to_string()]
        );
    }
}
