//! 扫描类别（Tab）—— `--tab` 参数的稳定契约。
//!
//! 2026-10-27 从 `crates/maclean-cli/src/handler.rs` 的 `ALL_TABS`
//! 字符串表迁移而来。迁移原因：`--tab` 原本是 `Option<String>`，
//! 拼错的值会一路穿过参数解析、缓存文件名，最后落到 `scan_tab` 的
//! `_ => Vec::new()` 分支 —— 表现为"扫过了但没东西"，没有任何报错。
//! 收成枚举后，非法值在 clap 解析阶段就被拒绝，并自动进入 `--help`。
//!
//! ## 兼容性
//!
//! [`ScanTab::key`] 返回的字符串是线上契约：它同时是 `--tab` 取值、
//! `~/.maclean/scan_cache/<key>.json` 的文件名、以及 JSON 输出的 `key`
//! 字段。改一个名字等于同时改三处契约，测试会拦住。

use clap::ValueEnum;

/// 扫描类别
///
/// clap 的默认取值就是变体名的 kebab-case（`DevCache` → `dev-cache`），
/// 与本模块的 [`ScanTab::key`] 一致；`tests` 里有一条用例把两者钉死，
/// 改任一方向都会红。
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ScanTab {
    /// 开发者缓存（Xcode / Docker / node_modules / 包管理器 / AI 模型）
    DevCache,
    /// 大文件
    LargeFiles,
    /// App 缓存
    AppCache,
    /// App 数据
    AppData,
    /// App 卸载残留
    AppUninstall,
    /// 系统优化任务
    Optimize,
    /// APFS 本地快照 / 模拟器运行时
    Apfs,
    /// 重复文件
    DupFiles,
    /// 用户自定义规则
    CustomRules,
    /// 展开为当前平台全部支持的类别（与 `--deep` 等价）
    All,
}

impl ScanTab {
    /// 全部具体类别（不含 [`ScanTab::All`]）。
    ///
    /// 顺序即 `maclean list` 的展示顺序与 `--deep` 的扫描顺序。
    pub const ALL_PUBLIC: &'static [ScanTab] = &[
        Self::DevCache,
        Self::LargeFiles,
        Self::AppCache,
        Self::AppData,
        Self::AppUninstall,
        Self::Optimize,
        Self::Apfs,
        Self::DupFiles,
        Self::CustomRules,
    ];

    /// 线上名称：`--tab` 取值 / 扫描缓存文件名 / JSON `key` 字段
    pub const fn key(self) -> &'static str {
        match self {
            Self::DevCache => "dev-cache",
            Self::LargeFiles => "large-files",
            Self::AppCache => "app-cache",
            Self::AppData => "app-data",
            Self::AppUninstall => "app-uninstall",
            Self::Optimize => "optimize",
            Self::Apfs => "apfs",
            Self::DupFiles => "dup-files",
            Self::CustomRules => "custom-rules",
            Self::All => "all",
        }
    }

    /// 人类可读标签（JSON `label` 字段 / 表格标题）
    pub const fn label(self) -> &'static str {
        match self {
            Self::DevCache => "开发者缓存",
            Self::LargeFiles => "大文件",
            Self::AppCache => "App缓存",
            Self::AppData => "App数据",
            Self::AppUninstall => "App卸载",
            Self::Optimize => "系统优化",
            Self::Apfs => "APFS快照",
            Self::DupFiles => "重复文件",
            Self::CustomRules => "自定义规则",
            Self::All => "全部",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// clap 的取值集合必须与 `key()` 完全一致 —— 否则 `--help` 教用户
    /// 输入一个 handler 不认识的字符串，等于把弱类型问题换了个位置。
    #[test]
    fn clap_possible_values_match_wire_keys() {
        for tab in ScanTab::ALL_PUBLIC
            .iter()
            .chain(std::iter::once(&ScanTab::All))
        {
            let possible = tab
                .to_possible_value()
                .unwrap_or_else(|| panic!("{} 没有 clap 取值", tab.key()));
            assert_eq!(
                possible.get_name(),
                tab.key(),
                "{} 的 clap 取值与 key() 不一致",
                tab.key()
            );
        }
    }

    /// `ALL_PUBLIC` 不能漏项：新增变体忘了加进列表，`--deep` 与 `list`
    /// 会静默少扫一类。
    #[test]
    fn all_public_covers_every_variant_except_all() {
        let variants: Vec<_> = <ScanTab as ValueEnum>::value_variants()
            .iter()
            .copied()
            .filter(|t| *t != ScanTab::All)
            .collect();
        assert_eq!(
            variants,
            ScanTab::ALL_PUBLIC,
            "ScanTab::ALL_PUBLIC 与枚举变体不同步"
        );
    }

    /// key 必须唯一且非空 —— 重复会导致缓存文件互相覆盖。
    #[test]
    fn keys_are_unique_and_non_empty() {
        let mut keys: Vec<&str> = Vec::new();
        for tab in ScanTab::ALL_PUBLIC
            .iter()
            .chain(std::iter::once(&ScanTab::All))
        {
            let k = tab.key();
            assert!(!k.is_empty(), "空 key");
            assert!(!keys.contains(&k), "重复 key: {k}");
            keys.push(k);
        }
    }

    /// 拼错的值必须被拒绝，而不是静默落到"扫了但没东西"。
    #[test]
    fn invalid_tab_value_is_rejected() {
        assert!(
            ScanTab::from_str("dev-cahce", true).is_err(),
            "拼错的 tab 被接受了，会静默扫出 0 项"
        );
        assert_eq!(ScanTab::from_str("dev-cache", true), Ok(ScanTab::DevCache));
    }

    #[test]
    fn help_lists_every_tab() {
        // 用真实的 clap 命令渲染一次 help，确认取值进得去
        #[derive(clap::Parser)]
        struct Probe {
            #[arg(long, value_enum)]
            tab: Option<ScanTab>,
        }
        let help = Probe::command().render_help().to_string();
        for tab in ScanTab::ALL_PUBLIC {
            assert!(
                help.contains(tab.key()),
                "help 里没有 {}，用户不知道有这个类别",
                tab.key()
            );
        }
    }
}
