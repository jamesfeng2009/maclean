//! 公开 CLI 命令参数（clap 契约）。
//!
//! 完整命令集（strategy §21 / split 03 Phase 6）：
//! 公开命令 scan / clean / schedule / check-disk / list / apps / uninstall /
//! log / backups / restore / startup / optimize / dup-ignore；
//! Pro 命令（history / growth / forecast / policy / automation）仅契约占位，
//! 实现留在私有 Pro 仓库。

use clap::{Parser, Subcommand};
use maclean_types::cli::{ColorMode, OutputFormat};

/// maclean — macOS 磁盘清理工具
#[derive(Parser, Debug)]
#[command(name = "maclean", version, about = "macOS 磁盘清理工具", long_about = None)]
pub struct Cli {
    /// 子命令（无子命令时启动 GUI）
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// 以 JSON 输出（供脚本/监控系统消费；等价 --format json）
    ///
    /// 结构化输出是稳定契约：字段名与语义不随文案翻译改变。
    #[arg(long, global = true)]
    pub json: bool,

    /// 输出格式：human / json / jsonl（jsonl 流式，每行一个对象）
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,

    /// 人类可读输出的颜色：auto / always / never（机器格式恒无色）
    #[arg(long, global = true, value_enum, default_value_t = ColorMode::Auto)]
    pub color: ColorMode,

    /// 关闭 stderr 进度条（脚本/日志场景）
    #[arg(long, global = true)]
    pub no_progress: bool,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum Commands {
    /// 扫描可清理项目
    Scan {
        /// 指定扫描的 Tab（dev-cache, large-files, app-cache, app-data, app-uninstall, optimize, apfs, dup-files, custom-rules）
        #[arg(long)]
        tab: Option<String>,

        /// 深度扫描（扫描所有 Tab）
        #[arg(long)]
        deep: bool,

        /// 大文件阈值（MB），写入 config.json 并立即生效（默认 100）
        #[arg(long)]
        large_min_mb: Option<u64>,

        /// 重复文件最小大小（KB），写入 config.json 并立即生效（默认 200）
        #[arg(long)]
        dup_min_kb: Option<u64>,
    },

    /// 清理安全可删除的项目
    ///
    /// 默认只预览不删除；加 --yes 才实际执行。定时任务（--scheduled）
    /// 视为用户已确认（无人值守自动清理，行为不变）。
    Clean {
        /// 指定清理的 Tab
        #[arg(long)]
        tab: Option<String>,

        /// 仅清理推荐安全项（Safe）
        #[arg(long)]
        safe_only: bool,

        /// 只预览将清理的项目，不实际删除
        #[arg(long, conflicts_with = "yes")]
        dry_run: bool,

        /// 确认执行删除（非交互环境必须显式携带，否则拒绝执行）
        #[arg(long, conflicts_with = "dry_run")]
        yes: bool,

        /// 由系统定时任务调用（C-3）：执行后把 schedule_last_run 记为当前时间
        ///
        /// 定时任务（launchd / 任务计划）跑的就是这条命令。带上它才知道
        /// "上次定时清理是什么时候"，否则设置页永远显示"尚未执行过"。
        #[arg(long)]
        scheduled: bool,

        /// 定时任务的登录补偿：未到期直接跳过（配合 --scheduled 使用）
        ///
        /// launchd 的 RunAtLoad / 任务计划的登录触发会在每次登录时调用一次；
        /// 带上它，命令先检查 schedule_last_run + 间隔，未到期就安静退出，
        /// 到期了才真正清理 —— 既补上错过的执行，又不会每次登录都清一遍。
        #[arg(long)]
        only_if_due: bool,

        /// 权限失败项自动提权删除（macOS 弹 Touch ID / 密码授权；非交互环境不可用）
        #[arg(long)]
        privileged: bool,

        /// 大文件阈值（MB），写入 config.json 并立即生效（默认 100）
        #[arg(long)]
        large_min_mb: Option<u64>,

        /// 重复文件最小大小（KB），写入 config.json 并立即生效（默认 200）
        #[arg(long)]
        dup_min_kb: Option<u64>,
    },

    /// 查看 / 注册 / 注销定时清理任务（C-3）
    Schedule {
        /// 注册系统定时任务
        #[arg(long)]
        install: bool,

        /// 注销系统定时任务
        #[arg(long)]
        remove: bool,

        /// 设置间隔（天）：1 / 7 / 30，其它值会被夹到最近的档位
        #[arg(long)]
        days: Option<u32>,
    },

    /// System Data 聚合视图（缓存 / 日志 / 本地快照 / iOS 备份 / App 支持）
    ///
    /// 对应 Apple 储存空间里的 “System Data” 分类：它不是某一个目录，
    /// 而是若干类数据的总和。这里给出可解释的拆分明细。
    SystemData,

    /// APFS 本地快照 / 模拟器运行时管理
    ///
    /// `clean --tab apfs` 只会跳过快照（快照不是文件，不能直接删除）。
    /// 这个命令是 CLI 里唯一能真正删除快照的入口，删除时按需弹系统授权。
    Apfs {
        #[command(subcommand)]
        action: ApfsAction,
    },

    /// 检查磁盘空间使用情况；--breakdown 输出分类占比
    CheckDisk {
        /// 扫描各缓存类别并按分类输出磁盘占用占比（P2 口径）
        #[arg(long)]
        breakdown: bool,
    },

    /// 列出所有可用的扫描类别
    List,

    /// 列出已安装应用清单（含 Chrome/Safari 安装的 PWA，一键卸载用）
    Apps,

    /// 一键卸载应用（本体 + 关联数据 + 缓存，移入废纸篓可还原）
    ///
    /// 默认只预览不卸载；加 --yes 才实际执行。接受完整 .app 路径，
    /// 也接受应用名（自动在 /Applications 与 ~/Applications 下解析）。
    Uninstall {
        /// 应用路径（/Applications/Foo.app）或应用名
        app: String,

        /// 确认执行卸载（非交互环境必须显式携带，否则拒绝执行）
        #[arg(long)]
        yes: bool,
    },

    /// 查看日志文件路径或输出最近日志
    Log {
        /// 输出最近 N 行日志（默认显示路径）
        #[arg(long)]
        tail: Option<usize>,

        /// 在资源管理器/Finder 中打开日志目录
        #[arg(long)]
        open: bool,
    },

    /// 列出历史删除清单（M-2）
    ///
    /// 每次删除都会落一份清单，记录删了哪些路径、多大、其中多少项还救得回来。
    Backups {
        /// 只看还有可还原项的清单
        #[arg(long)]
        restorable_only: bool,
    },

    /// 按清单还原（M-2）
    ///
    /// 只还原当时走废纸篓的项。永久删除的缓存字节已经不在了，
    /// 命令会明确列出它们为"不可还原"，不会假装成功。
    Restore {
        /// 清单 id（`maclean backups` 输出的第一列）
        id: String,
    },

    /// macOS 启动项管理（LaunchAgents / LaunchDaemons）
    Startup {
        #[command(subcommand)]
        action: StartupAction,
    },

    /// 系统优化/维护任务
    Optimize {
        #[command(subcommand)]
        action: OptimizeAction,
    },

    /// 管理重复文件扫描的用户忽略名单（config.json -> dup_ignore_patterns）
    ///
    /// 命中模式的路径（子串匹配）不参与重复文件清理，与内置硬排除互补。
    DupIgnore {
        #[command(subcommand)]
        action: DupIgnoreAction,
    },

    // ---- Pro 命令（strategy §21 / split 03 Phase 6）----
    // 仅私有 Pro 构建可用；公开版返回错误信封占位（status:"error",
    // error_code:1）。契约形状保持稳定，脚本按退出码判断。
    /// Pro：历史记录（仅私有 Pro 版可用）
    History,
    /// Pro：增长分析（仅私有 Pro 版可用）
    Growth,
    /// Pro：预测（仅私有 Pro 版可用）
    Forecast,
    /// Pro：策略（仅私有 Pro 版可用）
    Policy,
    /// Pro：自动化（仅私有 Pro 版可用）
    Automation,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum ApfsAction {
    /// 列出本地快照与模拟器运行时
    List,
    /// 删除指定快照/运行时（target 来自 `apfs list` 的 path）
    Delete {
        /// 快照名（com.apple.TimeMachine.<timestamp>.local）或运行时 UUID
        target: String,
        /// 确认删除（CLI 必须显式携带，避免误删）
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum StartupAction {
    /// 列出启动项（含已加载状态）
    List,
    /// 禁用启动项（plist 移入备份目录，可逆）
    Disable {
        /// launchd Label（`maclean startup list` 第一列）
        label: String,
    },
    /// 启用（从备份目录恢复 plist）
    Enable {
        /// launchd Label
        label: String,
    },
    /// 列出 macOS 登录项（Login Items）
    ///
    /// LaunchAgents/Daemons 之外，系统设置里的"登录项"由
    /// backgroundtaskmanagement 管理，只能通过 System Events 读取。
    /// 禁用/删除需在系统设置中操作，这里提供一键跳转。
    LoginItems {
        /// 直接打开系统设置的登录项页面
        #[arg(long)]
        open: bool,
    },
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum DupIgnoreAction {
    /// 列出当前忽略名单
    List,
    /// 添加忽略模式（子串匹配路径；命中即跳过，如 "site-packages"、"/backup/old"）
    Add {
        /// 路径子串模式
        pattern: String,
    },
    /// 移除一个忽略模式（需与 add 时的字符串完全一致）
    Remove {
        /// 要移除的模式
        pattern: String,
    },
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum OptimizeAction {
    /// 列出全部优化/维护任务（含风险等级）
    ListTasks,
    /// 执行指定任务（非 Safe 任务需 --yes 确认）
    Run {
        /// 任务 id（`maclean optimize list-tasks` 第一列）
        task: String,
        /// 确认执行非 Safe 任务
        #[arg(long)]
        yes: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn parses_scan_with_json_flag() {
        let args = Cli::try_parse_from(["maclean", "scan", "--json"]).unwrap();
        assert!(matches!(
            args.command,
            Some(Commands::Scan { deep: false, .. })
        ));
        assert!(args.json);
    }

    #[test]
    fn parses_check_disk_breakdown() {
        let args = Cli::try_parse_from(["maclean", "check-disk", "--breakdown"]).unwrap();
        assert!(matches!(
            args.command,
            Some(Commands::CheckDisk { breakdown: true })
        ));
    }

    #[test]
    fn no_subcommand_means_gui() {
        let args = Cli::try_parse_from(["maclean"]).unwrap();
        assert!(args.command.is_none());
    }

    #[test]
    fn value_enum_roundtrip_for_output_format() {
        assert_eq!(
            OutputFormat::from_str("jsonl", true),
            Ok(OutputFormat::Jsonl)
        );
        assert_eq!(ColorMode::from_str("never", true), Ok(ColorMode::Never));
    }

    #[test]
    fn pro_commands_parse_as_placeholders() {
        // P6-3：Pro 命令在公开 CLI 契约层可解析（占位），handler 返回不可用
        for (name, cmd) in [
            ("history", Commands::History),
            ("growth", Commands::Growth),
            ("forecast", Commands::Forecast),
            ("policy", Commands::Policy),
            ("automation", Commands::Automation),
        ] {
            let args = Cli::try_parse_from(["maclean", name]).unwrap();
            assert_eq!(args.command, Some(cmd), "{name} 占位解析失败");
        }
    }

    #[test]
    fn full_command_set_has_all_public_and_pro_commands() {
        // 带必需子命令/参数的命令单独验证；其余命令可裸解析
        for cmd in [
            "scan",
            "clean",
            "schedule",
            "check-disk",
            "list",
            "apps",
            "log",
            "backups",
            "history",
            "growth",
            "forecast",
            "policy",
            "automation",
        ] {
            assert!(
                Cli::try_parse_from(["maclean", cmd]).is_ok(),
                "命令 {cmd} 应可解析"
            );
        }
        assert!(
            Cli::try_parse_from(["maclean", "uninstall", "Example.app"]).is_ok(),
            "uninstall 带 app 参数应可解析"
        );
        assert!(
            Cli::try_parse_from(["maclean", "restore", "2026-01-01T00:00:00Z"]).is_ok(),
            "restore 带清单 id 应可解析"
        );
        assert!(
            Cli::try_parse_from(["maclean", "startup", "list"]).is_ok(),
            "startup 带子命令应可解析"
        );
        assert!(
            Cli::try_parse_from(["maclean", "optimize", "list-tasks"]).is_ok(),
            "optimize 带子命令应可解析"
        );
        assert!(
            Cli::try_parse_from(["maclean", "dup-ignore", "list"]).is_ok(),
            "dup-ignore 带子命令应可解析"
        );
        assert!(
            Cli::try_parse_from(["maclean", "apfs", "list"]).is_ok(),
            "apfs list 应可解析"
        );
        assert!(
            Cli::try_parse_from([
                "maclean",
                "apfs",
                "delete",
                "com.apple.TimeMachine.2024-01-15-100000.local",
                "--yes"
            ])
            .is_ok(),
            "apfs delete 应可解析"
        );
    }

    #[test]
    fn clean_parses_scheduled_and_only_if_due() {
        // 定时任务 plist 里写的就是这两个参数组合；解析失败会让定时清理
        // 在启动时直接报参数错误，永远不执行。
        let args = Cli::try_parse_from([
            "maclean",
            "clean",
            "--safe-only",
            "--scheduled",
            "--only-if-due",
        ])
        .unwrap();
        match args.command {
            Some(Commands::Clean {
                scheduled,
                only_if_due,
                safe_only,
                ..
            }) => {
                assert!(scheduled);
                assert!(only_if_due);
                assert!(safe_only);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }
}
