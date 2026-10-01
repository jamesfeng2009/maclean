//! M-1 · macOS 官方卸载器识别与调用
//!
//! # 缺口
//!
//! 卸载应用时，本工具一直只做一件事：把 `/Applications/Foo.app` 整个目录
//! `remove_dir_all` 掉。很多应用**自带官方卸载器**（包内 Helpers 里的卸载
//! 工具，或 `/Applications/Uninstall Foo.app`），那些卸载器要干的活远不止
//! 删目录 —— 卸载 launchd 守护进程、注销 MachServices、清 pkgutil 收据、
//! 移除内核扩展、吊销辅助工具的系统扩展授权。
//!
//! 直接删目录的后果：卸载了"文件"，没卸载"软件"。残留的 launchd 任务会
//! 继续每分钟拉起一个已经不存在的二进制，控制台刷 `Could not find specified
//! service`，用户以为没卸干净又来清一次。
//!
//! # 边界：这里不做"猜测式自动执行"
//!
//! 找到的可执行文件**只在用户已经确认删除之后**才启动，而且优先用 `open -a`
//! 交给系统启动（走它的 GUI，用户看得见），而不是后台静默跑一个二进制。
//! 此外还有两条硬约束：
//! - 应用自己不能被当成自己的卸载器（"App Cleaner & Uninstaller" 这类名字
//!   里带 Uninstall 的应用极易误判，见 `normalize` 比对）
//! - 只在"明显是辅助工具"的目录里找，避免把应用主程序当卸载器
//!
//! # 平台无关
//!
//! 名字判定与命令构造都是纯函数、不加 cfg —— 本机（macOS）能测，
//! Windows 构建也能编译通过、不会漏测。

use std::path::{Path, PathBuf};

/// 卸载器形态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallerKind {
    /// 独立卸载器 .app（如 `Uninstall Foo.app`）：用 `open -a` 启动它的 GUI
    App,
    /// 命令行卸载工具（脚本或可执行文件）：直接执行
    Executable,
}

/// 找到的官方卸载器
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialUninstaller {
    pub path: String,
    pub kind: UninstallerKind,
}

/// 名字归一化：小写 + 去分隔符，用于"这是不是应用自己"的比对
///
/// 不做完整 Unicode 折叠，只处理 ASCII：应用名与卸载器名的比对不需要
/// 那么精确，需要处理的是大小写和 `Foo Bar` / `Foo-Bar` / `FooBar` 的差异。
fn normalize(name: &str) -> String {
    name.to_ascii_lowercase()
        .trim_end_matches(".app")
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// 文件名是否像卸载器
///
/// 只认明确的卸载语义词。刻意**不**收 "remove" / "clean" / "erase" ——
/// 这些词在辅助工具里太常见（比如日志轮转脚本），会把无关二进制拉进来。
pub fn looks_like_uninstaller(file_name: &str) -> bool {
    let n = file_name.to_ascii_lowercase();
    let stem = n.trim_end_matches(".app");
    stem.contains("uninstall") || stem.contains("卸载") || stem.contains("unins")
}

/// 文件是否可执行（任一执行位）
fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        matches!(
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some("exe") | Some("bat") | Some("cmd") | Some("ps1")
        )
    }
}

/// 在一个目录里找卸载器
///
/// - `self_name`：被卸载应用自己的归一化名字，用来排除"自己被当成自己的卸载器"
/// - `require_name_match`：包外搜索时要求卸载器名字里带应用名，
///   否则 `/Applications` 下任何一家厂商的卸载器都会被误配给这个应用
fn scan_dir(dir: &Path, self_name: &str, require_name_match: bool) -> Option<OfficialUninstaller> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut candidates: Vec<OfficialUninstaller> = Vec::new();

    for entry in rd.flatten() {
        let p = entry.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !looks_like_uninstaller(name) {
            continue;
        }
        let norm = normalize(name);
        // 排除应用自己：名字里带 Uninstall 的应用（清理工具类）极易自命中
        if !self_name.is_empty() && norm == self_name {
            continue;
        }
        if require_name_match && !self_name.is_empty() && !norm.contains(self_name) {
            continue;
        }

        if name.to_ascii_lowercase().ends_with(".app") {
            candidates.push(OfficialUninstaller {
                path: p.to_string_lossy().to_string(),
                kind: UninstallerKind::App,
            });
        } else if p.is_file() && is_executable(&p) {
            candidates.push(OfficialUninstaller {
                path: p.to_string_lossy().to_string(),
                kind: UninstallerKind::Executable,
            });
        }
    }

    // 多个候选时优先独立卸载器 App（有 GUI、用户看得见），其次可执行文件
    candidates.sort_by_key(|c| match c.kind {
        UninstallerKind::App => 0,
        UninstallerKind::Executable => 1,
    });
    candidates.into_iter().next()
}

/// 包内搜索目录：只在这些"明显是辅助工具"的位置找
///
/// 刻意**不**把 `Contents/MacOS` 的主程序当卸载器 —— 那里放的是应用本体，
/// 一旦名字里碰巧有 uninstall 字样就会把自己执行一遍。
const BUNDLE_SEARCH_DIRS: &[&str] = &[
    "Contents/Helpers",
    "Contents/Resources",
    "Contents/Library/LaunchServices",
    "Contents/SharedSupport",
];

/// 找应用的官方卸载器
///
/// `app_path` 是 .app 包路径，`app_name` 是展示用的应用名。
/// 找不到返回 None —— 调用方 fallback 到删目录，这是绝大多数应用的现状。
pub fn find_official_uninstaller(app_path: &str, app_name: &str) -> Option<OfficialUninstaller> {
    let app = Path::new(app_path);
    if !app.is_dir() {
        return None;
    }
    let self_name = normalize(app_name);

    // 1) 包内：只在辅助目录里找
    for sub in BUNDLE_SEARCH_DIRS {
        let dir = app.join(sub);
        if let Some(u) = scan_dir(&dir, &self_name, false) {
            return Some(u);
        }
    }

    // 2) 包外：同级目录下的独立卸载器（/Applications/Uninstall Foo.app）
    //    这里必须要求名字带应用名，否则会张冠李戴
    if let Some(parent) = app.parent() {
        if let Some(u) = scan_dir(parent, &self_name, true) {
            return Some(u);
        }
    }

    None
}

/// 构造启动命令（纯函数，不执行）
///
/// 拆出这一层是为了让"到底怎么启动"在开发机上就能断言 ——
/// 真正执行的那几行在 cfg(macos) 里，本机跑得到但 Windows 构建里也编译得过。
pub fn launch_command(u: &OfficialUninstaller) -> (String, Vec<String>) {
    match u.kind {
        // open -a 走系统 LaunchServices 启动，卸载器自己的 GUI 会弹出来，
        // 用户看得见在发生什么 —— 比后台静默跑一个二进制安全得多
        UninstallerKind::App => ("open".to_string(), vec!["-a".to_string(), u.path.clone()]),
        UninstallerKind::Executable => (u.path.clone(), Vec::new()),
    }
}

/// 启动官方卸载器
///
/// 只负责"叫起来"，不等它跑完：GUI 卸载器往往 fork 子进程后就退出，
/// `-W` 等待并不可靠，反而会把删除线程挂死。调用方应当把这一项标为
/// "已交由官方卸载器处理"，而不是"已删除"。
#[cfg(target_os = "macos")]
pub fn launch(u: &OfficialUninstaller) -> std::io::Result<()> {
    let (prog, args) = launch_command(u);
    std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// 从 .app 路径推导展示名（去掉 .app 后缀）
pub fn app_display_name(app_path: &str) -> String {
    PathBuf::from(app_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obvious_uninstaller_names_are_recognized() {
        assert!(looks_like_uninstaller("Uninstall Foo.app"));
        assert!(looks_like_uninstaller("uninstaller.sh"));
        assert!(looks_like_uninstaller("卸载工具"));
        assert!(looks_like_uninstaller("unins000.exe"));
    }

    #[test]
    fn generic_tool_names_are_not_mistaken_for_uninstallers() {
        // 反向验证：这些名字在辅助工具里太常见，认了就把无关二进制拉进来
        assert!(!looks_like_uninstaller("cleanup.sh"));
        assert!(!looks_like_uninstaller("remove_old_logs"));
        assert!(!looks_like_uninstaller("erase_helper"));
        assert!(!looks_like_uninstaller("MainMenu.nib"));
    }

    #[test]
    fn name_matching_is_case_and_separator_insensitive() {
        // "Foo Bar" / "foo-bar" / "FOOBAR" 必须归一化到同一个 key，
        // 否则大小写一变就漏判
        assert_eq!(normalize("Foo Bar"), normalize("foo-bar"));
        assert_eq!(normalize("FOOBAR"), normalize("Foo Bar"));
        assert_eq!(normalize("Foo.app"), normalize("Foo"));
    }

    #[test]
    fn launch_command_uses_open_for_app_bundles() {
        // 独立卸载器必须走 open -a，让用户看见它的 GUI，
        // 而不是后台静默执行一个二进制
        let u = OfficialUninstaller {
            path: "/Applications/Uninstall Foo.app".to_string(),
            kind: UninstallerKind::App,
        };
        let (prog, args) = launch_command(&u);
        assert_eq!(prog, "open");
        assert_eq!(args, vec!["-a", "/Applications/Uninstall Foo.app"]);
    }

    #[test]
    fn launch_command_executes_scripts_directly() {
        let u = OfficialUninstaller {
            path: "/Applications/Foo.app/Contents/Helpers/uninstall.sh".to_string(),
            kind: UninstallerKind::Executable,
        };
        let (prog, args) = launch_command(&u);
        assert_eq!(prog, u.path);
        assert!(args.is_empty());
    }

    #[test]
    fn app_display_name_strips_the_bundle_suffix() {
        assert_eq!(app_display_name("/Applications/Foo.app"), "Foo");
        assert_eq!(app_display_name("/Applications/Foo Bar.app"), "Foo Bar");
    }

    /// 建一棵真实的临时目录树
    ///
    /// 名字判定再怎么测，都证明不了"搜索路径写对了" —— 只有真造一棵树
    /// 才能验证 Contents/Helpers 这种相对路径没拼错。用 pid 做后缀，
    /// 避免并发测试互相踩。
    fn temp_tree(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("maclean-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        base
    }

    #[test]
    fn finds_an_uninstaller_inside_the_bundle() {
        let base = temp_tree("ou-in-bundle");
        let app = base.join("Foo.app");
        let helpers = app.join("Contents").join("Helpers");
        std::fs::create_dir_all(&helpers).unwrap();
        let exe = helpers.join("uninstall_foo.sh");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let found = find_official_uninstaller(&app.to_string_lossy(), "Foo");
        assert_eq!(
            found,
            Some(OfficialUninstaller {
                path: exe.to_string_lossy().to_string(),
                kind: UninstallerKind::Executable,
            })
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_app_is_never_its_own_uninstaller() {
        // 反向验证的核心用例：名字里带 Uninstall 的清理类工具，
        // 极易在包外搜索里自命中 —— 那等于让它"自杀"。
        let base = temp_tree("ou-self");
        let app = base.join("App Cleaner & Uninstaller.app");
        std::fs::create_dir_all(&app).unwrap();

        assert!(
            find_official_uninstaller(&app.to_string_lossy(), "App Cleaner & Uninstaller")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_missing_bundle_yields_no_uninstaller() {
        // 目录不存在时必须干净返回 None，不能 panic ——
        // 扫描结果可能已经过期（用户手动删过了）
        assert!(find_official_uninstaller("/nonexistent/Foo.app", "Foo").is_none());
    }
}
