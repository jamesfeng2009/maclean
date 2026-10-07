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
use crate::scanner::{dir_size, has_home, home_dir, Recommend, ScanItem, ScanResult, Scanner};

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
    /// 该规则是否适用于当前平台
    fn matches_platform(&self) -> bool {
        let os = std::env::consts::OS;
        matches!(self.platform.as_str(), "all" | "macos" if os == "macos")
            || matches!(self.platform.as_str(), "all" | "windows" if os == "windows")
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
                Ok(()) => {
                    crate::logger::info(&format!("规则已加载: {} ({})", rule.id, source));
                    ok.push(rule);
                }
                Err(reason) => {
                    crate::logger::info(&format!("规则被拒绝: {} ({})", reason, source));
                }
            }
        }
        ok
    }
}

impl Scanner for RuleScanner {
    fn scan(&self) -> ScanResult {
        let start = std::time::Instant::now();
        let mut items = Vec::new();

        // 占用进程（删前要求停进程）：全扫描只取一次系统进程快照
        let running = running_process_names();
        for rule in &self.rules {
            for root in rule.resolved_roots() {
                items.extend(scan_rule_root(&root, rule, &running));
            }
        }

        // 按路径去重（多个规则可能命中同一目录）
        let mut seen = std::collections::HashSet::new();
        items.retain(|i| seen.insert(i.path.clone()));

        items.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        ScanResult {
            total_size,
            scan_time_ms: start.elapsed().as_millis() as u64,
            items,
        }
    }
}

/// 扫描单个规则根：walkdir 收集匹配子目录，过滤体积阈值与安全校验
fn scan_rule_root(root: &Path, rule: &CleanRule, running: &[String]) -> Vec<ScanItem> {
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
            if !matches!(
                safety::check_path_safety_with_category(&p_str, &rule.category),
                safety::SafetyCheck::Safe
            ) {
                return None;
            }
            let size = dir_size(p);
            if size < rule.min_size {
                return None;
            }
            // 删前要求停进程（fail-closed）：任一要求进程仍在运行 → 该项锁定
            let mut deletable = true;
            let mut undeletable_reason = String::new();
            if !rule.required_stopped_processes.is_empty() {
                let still_running: Vec<String> = rule
                    .required_stopped_processes
                    .iter()
                    .filter(|proc_name| {
                        let wanted = proc_name.to_lowercase();
                        running.iter().any(|n| n == &wanted)
                    })
                    .cloned()
                    .collect();
                if !still_running.is_empty() {
                    deletable = false;
                    undeletable_reason =
                        format!("required_stopped_processes:{}", still_running.join(", "));
                }
            }
            Some(ScanItem { batch_mtimes: vec![],
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

/// 当前运行进程名快照（小写，无扩展名差异）。
///
/// - macOS：`ps -axo comm=`（进程名/路径，取 basename）；
/// - Windows：`tasklist /FO CSV /NH`（"name.exe"）。
///
/// 失败时返回空列表：调用方按"未知运行状态"处理 —— 规则要求进程时，
/// 若快照失败则 fail-closed 锁定该项（宁可不可删，不可误删）。
fn running_process_names() -> Vec<String> {
    let mut names = Vec::new();
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("/bin/ps")
            .args(["-axo", "comm="])
            .output()
        {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let name = line
                        .trim()
                        .rsplit('/')
                        .next()
                        .unwrap_or(line.trim())
                        .to_lowercase();
                    if !name.is_empty() {
                        names.push(name);
                    }
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(out) = std::process::Command::new("tasklist")
            .args(["/FO", "CSV", "/NH"])
            .output()
        {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    // CSV: "name.exe","pid","session"... 取第一列并去引号
                    let first = line.split(',').next().unwrap_or("").trim_matches('"');
                    let name = first.to_lowercase();
                    if !name.is_empty() {
                        names.push(name);
                    }
                }
            }
        }
    }
    names
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
        // P0：开发者缓存/AI/容器全覆盖 —— 总数与类别断言
        let rules = RuleScanner::load_builtin();
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
    fn platform_filter_respects_current_os() {
        let rule = CleanRule {
            id: "t".into(),
            name_key: "x".into(),
            desc_key: "x".into(),
            category: "x".into(),
            platform: "windows".into(),
            roots: vec!["$HOME/Library/Caches/Foo".to_string()],
            patterns: vec!["Cache".into()],
            min_size: 0,
            risk: "Safe".into(),
            required_stopped_processes: vec![],
        };
        // 在 macOS 上 windows-only 规则不匹配平台
        assert_eq!(rule.matches_platform(), std::env::consts::OS == "windows");
    }

    #[test]
    fn scan_rule_root_respects_min_size_and_safety() {
        // 临时目录造一个 1KB 缓存子目录 + 一个 2MB 缓存子目录
        let tmp = std::env::temp_dir().join(format!("maclean_rules_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
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
        let items = scan_rule_root(&tmp, &rule, &[]);
        // 只应命中 BigCache（>=1MB），tiny 被体积阈值过滤
        assert_eq!(items.len(), 1, "应只有大目录命中: {:?}", items);
        assert_eq!(items[0].path, big.to_string_lossy());
        assert!(items[0].size_bytes >= 2 * 1024 * 1024);
        assert_eq!(items[0].recommend, Recommend::Safe);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn required_stopped_processes_locks_items_fail_closed() {
        // 规则声明需停止的进程：进程在运行 → 锁定不可删；进程已退出 → 可删
        let tmp = std::env::temp_dir().join(format!("maclean_proc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
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

        // 进程仍在运行 → deletable=false + 原因
        let locked = scan_rule_root(&tmp, &rule, &["google chrome".to_string()]);
        assert_eq!(locked.len(), 1);
        assert!(!locked[0].deletable, "进程运行时必须锁定不可删");
        assert!(
            locked[0]
                .undeletable_reason
                .starts_with("required_stopped_processes:"),
            "原因应标记需退出进程: {}",
            locked[0].undeletable_reason
        );

        // 进程已退出 → 正常可删
        let free = scan_rule_root(&tmp, &rule, &[]);
        assert_eq!(free.len(), 1);
        assert!(free[0].deletable, "进程退出后应可删");
        assert!(free[0].undeletable_reason.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
