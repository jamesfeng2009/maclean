//! 平台抽象层
//!
//! 提供跨平台基础函数，屏蔽 macOS / Windows 差异。
//! - 路径相关：home_dir / app_data_dir / expand_path
//! - 删除到废纸篓：move_to_trash
//! - check_deletable 的跨平台实现

use std::path::{Path, PathBuf};

// Windows 操作前备份模块（系统还原点 + 注册表备份 + 还原入口）
#[cfg(target_os = "windows")]
pub mod windows_backup;
// macOS 进程内「移入废纸篓」（NSFileManager / NSWorkspace），替代 osascript 代删
#[cfg(target_os = "macos")]
mod macos_trash;
// 注册表备份的可信性校验。刻意不作平台限定 —— 这是 `reg import` 前最后一道关，
// 必须在开发机上就能编译和测试，详见模块顶部注释。
// H-02：公开模块，让校验函数与清单解析成为真实公开 API（去掉 allow(dead_code) 后
// 也不会因"本机编译不到 windows_backup"而报 dead_code）。
pub mod reg_safety;

/// 获取当前用户 home 目录
///
/// macOS: $HOME
/// Windows: %USERPROFILE%
pub fn home_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Default"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
    }
}

/// 系统真实家目录（macOS）
///
/// 与 [`NSFileManager`] 决定废纸篓位置时用的家目录一致：忽略 `$HOME` 覆盖，
/// 沙盒环境下返回容器家目录。删除走 NSFileManager、还原按 `$HOME` 找的时候，
/// 一旦 `$HOME` 被覆盖就会出现"删得进去、还原不出来"，两边必须用同一个家目录。
#[cfg(target_os = "macos")]
pub fn real_home_dir() -> PathBuf {
    use objc2_foundation::NSHomeDirectory;
    PathBuf::from(NSHomeDirectory().to_string())
}

/// maclean 应用数据目录（日志、缓存等）
///
/// macOS: ~/.maclean/
/// Windows: %APPDATA%\maclean\
pub fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home_dir().join(".maclean")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join("AppData").join("Roaming"))
            .join("maclean")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir().join(".maclean")
    }
}

// 2026-09-18 删除了 `platform::expand_path`：全仓零引用，且与
// `scanner::cache_registry::expand_path`（两参数、真正被 scan() 调用的那个）
// 重复。两份实现并存时，"改哪份才算生效"是个陷阱。

/// F-07：root 属主且当前进程非 root 时，返回「需提权」原因。
///
/// 抽成无 cfg 的纯函数，让这条判定在本机也能被行为测试覆盖。
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn root_owned_needs_privilege(uid: u32, euid: u32) -> Option<&'static str> {
    if uid == 0 && euid != 0 {
        Some("root 属主：需要管理员权限（提权删除）")
    } else {
        None
    }
}

/// 检测路径是否可删除（跨平台）
///
/// 返回 (deletable, reason)
pub fn check_deletable(path: &str) -> (bool, String) {
    let p = Path::new(path);

    // APFS 快照由专门逻辑处理
    if path.starts_with("snapshot:") {
        return (true, String::new());
    }

    #[cfg(target_os = "macos")]
    {
        // SIP/系统保护路径（/System、/usr、/Library/Developer/CoreSimulator 等）：
        // 任何权限（含管理员/Touch ID）都无法删除。扫描期直接标记不可删，
        // 默认不出现在清理列表（show_protected 开关打开时才可见、标不可删），
        // 避免"勾选 → 授权 → 删除失败(SIP) → 再弹窗"的循环。
        // 判定与删除入口 safety::is_critical_system_path 共用同一张表，
        // 保证"扫不出来"与"删不掉"永远一致。
        if crate::safety::is_critical_system_path(path) {
            return (
                false,
                "SIP保护: 此路径受 macOS 系统保护，任何权限均无法删除".to_string(),
            );
        }

        // F-07：旧实现对 uid==0 直接 return (true, "")，与函数末尾恒等，
        // 属主检查是死代码。真正接线：root 属主且当前用户非 root 时明确标注
        // 需提权，让上层走提权链路，而不是假装和普通文件一样可删。
        if let Ok(meta) = p.symlink_metadata() {
            use std::os::unix::fs::MetadataExt;
            let euid = unsafe { libc::geteuid() };
            if let Some(reason) = root_owned_needs_privilege(meta.uid(), euid) {
                return (true, reason.to_string());
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: 简化判断，Program Files 下的需要管理员权限但仍可尝试删除
        // 实际权限由删除时检查
        let _ = p; // 避免未使用警告
    }

    (true, String::new())
}

/// 目标是否位于只读卷（P1-2）
///
/// OrbStack 挂载卷、APFS 快照等只读卷上，Finder 无法把文件移入废纸篓，
/// 调用 osascript 只会弹"Some files can't be processed"对话框且**无法静音**。
/// 删除前预先探测：命中只读卷直接记失败跳过，不触发系统弹窗。
///
/// 探测失败（路径不可解析、statvfs 出错）一律返回 false —— 宁可让 Finder
/// 弹窗，也不因误判把可删文件跳过。
pub fn path_on_readonly_volume(path: &str) -> bool {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let _ = path;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            let Ok(c_path) = CString::new(path) else {
                return false;
            };
            let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(c_path.as_ptr(), &mut buf) } != 0 {
                return false;
            }
            (buf.f_flag & libc::ST_RDONLY) != 0
        }
        #[cfg(windows)]
        {
            false
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = path;
        false
    }
}

/// 端到端验证用的「隔离废纸篓」注入点，**仅 debug 构建存在**。
///
/// macOS 的 `NSFileManager` 用 `NSHomeDirectory()` 决定废纸篓位置，忽略
/// `$HOME`，所以 `HOME=$TMPHOME` 只能隔离 config/日志/备份清单，隔离不了
/// 真实 `~/.Trash`。真跑 `clean --tab large-files --yes` 必须换掉这个 sink。
///
/// release 构建里这个函数恒返回 `None`，整条分支被编译器消掉 —— 生产行为
/// 与加这段之前逐字一致，没有可被环境变量改变的用户面开关。
#[cfg(debug_assertions)]
fn test_trash_sink() -> Option<PathBuf> {
    let sink = std::env::var_os("MACLEAN_TEST_TRASH_DIR").map(PathBuf::from)?;
    // 误配置的防御：真实废纸篓（以及它的祖先/子孙）绝不能被当 sink，
    // 否则测试文件会写进用户数据区，正是我们要避开的地方。
    #[cfg(target_os = "macos")]
    let real_trash = real_home_dir().join(".Trash");
    #[cfg(not(target_os = "macos"))]
    let real_trash = home_dir().join(".Trash");
    if !sink.is_absolute() || sink.starts_with(&real_trash) || real_trash.starts_with(&sink) {
        return None;
    }
    Some(sink)
}

#[cfg(not(debug_assertions))]
fn test_trash_sink() -> Option<PathBuf> {
    None
}

/// 把文件改名进隔离 sink，**同名绝不覆盖**。
///
/// 只用 `symlink_metadata` 预判会有 TOCTOU 窗口（判空与 rename 之间名字
/// 可能被占用，而 POSIX `rename` 会直接覆盖目标）。这里改用 `create_new`
/// 独占创建占位：名字归本进程之后，rename 覆盖的是自己的占位文件，
/// 不可能碰到任何别人的数据。悬空 symlink 同样会让 `create_new` 失败。
///
/// 刻意只支持普通文件：目录无法用占位文件原子占名，猜错就是数据丢失。
/// 目录返回 false → 调用方按 `TrashFailed` 保留原文件，绝不降级永久删除。
fn rename_into_sink(path: &str, sink: &Path) -> bool {
    let src = Path::new(path);
    if !src.is_file() {
        return false;
    }
    if std::fs::create_dir_all(sink).is_err() {
        return false;
    }
    let Some(name) = src.file_name() else {
        return false;
    };
    let base = name.to_string_lossy();
    for n in 0..1000u32 {
        let candidate = if n == 0 {
            sink.join(name)
        } else {
            sink.join(format!("{base} [{n}]"))
        };
        let Ok(_guard) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        else {
            continue;
        };
        return match std::fs::rename(src, &candidate) {
            Ok(()) => true,
            Err(_) => {
                // rename 失败时把占位清掉，不在 sink 里留垃圾
                let _ = std::fs::remove_file(candidate);
                false
            }
        };
    }
    false
}

/// 移动文件/目录到废纸篓（跨平台）
///
/// macOS: 进程内 `NSFileManager`（静默回收）→ `NSWorkspace.recycleURLs`
///   （由本 App 弹正规 Touch ID / 密码授权），详见 [`macos_trash`]；
///   失败时**绝不**降级为永久删除。
/// Windows: 调用 SHFileOperation FO_DELETE + FOF_ALLOWUNDO
///
/// debug 构建可通过 `MACLEAN_TEST_TRASH_DIR` 把回收动作改写到隔离目录，
/// 供端到端「真删」验证；release 构建不含此通道。
pub fn move_to_trash(path: &str) -> bool {
    let p = Path::new(path);
    if !p.exists() && p.symlink_metadata().is_err() {
        return false;
    }
    if let Some(sink) = test_trash_sink() {
        return rename_into_sink(path, &sink);
    }

    #[cfg(target_os = "macos")]
    {
        macos_trash::move_to_trash_impl(path)
    }

    #[cfg(target_os = "windows")]
    {
        // F-10：路径不再拼进 `-Command` 字符串（撇号转义挡不住所有注入形态），
        // 改为独立参数传入、脚本里用 `$args[0]` 取；stderr 保留记日志，
        // 不再被 Stdio::null() 吞掉后假装成功。
        let ps_cmd = if p.is_dir() {
            "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($args[0],'OnlyErrorDialogs','SendToRecycleBin')"
        } else {
            "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($args[0],'OnlyErrorDialogs','SendToRecycleBin')"
        };
        let out = std::process::Command::new("powershell")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(ps_cmd)
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output();
        match out {
            Ok(o) if o.status.success() => true,
            Ok(o) => {
                crate::logger::warn(&format!(
                    "移入回收站失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ));
                false
            }
            Err(e) => {
                crate::logger::warn(&format!("移入回收站命令启动失败: {}", e));
                false
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // 本平台没有可用的废纸篓实现。绝不能伪装成"已移入废纸篓"直接
        // remove —— 调用方（use_trash 链路）承诺失败即保留文件。
        let _ = p;
        false
    }
}

/// 系统是否处于深色外观（用于首次启动时初始化主题）
///
/// - macOS: `defaults read -g AppleInterfaceStyle`，输出含 "Dark" 即深色
/// - Windows: `HKCU\...\Themes\Personalize` 的 `AppsUseLightTheme` 为 0 即深色
/// - 其它平台 / 命令失败：按浅色处理
///
/// 只在首次启动（config.json 不存在）时调用一次，失败不影响可用性。
pub fn system_prefers_dark() -> bool {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output();
        matches!(
            out.ok().map(|o| String::from_utf8_lossy(&o.stdout).to_string()),
            Some(s) if s.trim().eq_ignore_ascii_case("dark")
        )
    }

    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout).to_string();
                // 形如 `AppsUseLightTheme    REG_DWORD    0x0`
                s.rsplit_once("0x")
                    .map(|(_, v)| v.trim().starts_with('0'))
                    .unwrap_or(false)
            }
            Err(_) => false,
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// 获取磁盘信息 (macOS 实现) → (total, free)
///
/// 用 `df -k /` 读根卷容量 / 可用量：`parts[1]` 是总容量、`parts[3]` 是
/// Available（即可用/空闲），不是 used。F-08：统一注释与返回口径。
#[cfg(target_os = "macos")]
fn get_disk_info_macos() -> (u64, u64) {
    let output = std::process::Command::new("df").arg("-k").arg("/").output();

    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                if let (Ok(total_kb), Ok(free_kb)) =
                    (parts[1].parse::<u64>(), parts[3].parse::<u64>())
                {
                    return (total_kb * 1024, free_kb * 1024);
                }
            }
        }
    }

    (0, 0)
}

/// F-08：解析 `Get-PSDrive C | Select-Object Used,Free | ConvertTo-Json`
///
/// ConvertTo-Json 压缩成单行时，旧逐行 `starts_with("\"Used\"")` 全部不命中、
/// 静默返回 (0,0)。改用 serde_json 按字段取值；纯函数不加 cfg，便于本机测试。
/// 仅在 Windows 磁盘信息路径被调用，非 Windows 构建下按平台 gate 抑制 dead_code。
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_psdrive_json(text: &str) -> (u64, u64) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return (0, 0);
    };
    let used = v.get("Used").and_then(|x| x.as_u64()).unwrap_or(0);
    let free = v.get("Free").and_then(|x| x.as_u64()).unwrap_or(0);
    (used.saturating_add(free), free)
}

/// 获取磁盘信息 (Windows 实现) → (total, free)
#[cfg(target_os = "windows")]
fn get_disk_info_windows() -> (u64, u64) {
    let output = std::process::Command::new("powershell")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg("Get-PSDrive C | Select-Object Used,Free | ConvertTo-Json")
        .output();
    let Ok(output) = output else {
        return (0, 0);
    };
    if !output.status.success() {
        return (0, 0);
    }
    parse_psdrive_json(&String::from_utf8_lossy(&output.stdout))
}

/// 获取磁盘信息 (total, free)
///
/// macOS: `df -k /`；Windows: PowerShell `Get-PSDrive C`。
pub fn disk_info() -> (u64, u64) {
    #[cfg(target_os = "macos")]
    {
        get_disk_info_macos()
    }

    #[cfg(target_os = "windows")]
    {
        get_disk_info_windows()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn check_deletable_marks_sip_protected_paths_undeletable() {
        // SIP/系统保护路径（含 CoreSimulator）：任何权限都删不掉，
        // 扫描期必须标不可删 —— 否则用户勾选 → 授权 → 删除失败(SIP) → 弹窗循环。
        for p in [
            "/Library/Developer/CoreSimulator/Volumes/runtime",
            "/Library/Developer/CoreSimulator/Caches/iOS",
            "/System/Library/Frameworks",
            "/usr/bin",
            "/Library/LaunchDaemons/com.example.plist",
        ] {
            let (deletable, reason) = check_deletable(p);
            assert!(!deletable, "SIP 路径不应可删: {p}");
            assert!(!reason.is_empty(), "SIP 路径应有不可删原因: {p}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn check_deletable_keeps_user_paths_deletable() {
        // 用户目录/缓存不在系统保护表内，仍可删（走废纸篓或提权）
        let home = crate::scanner::home_dir();
        for p in [
            home.join("Library/Caches/com.example")
                .to_string_lossy()
                .to_string(),
            home.join("Downloads/tmp").to_string_lossy().to_string(),
            home.join(".npm/_cacache").to_string_lossy().to_string(),
        ] {
            let (deletable, _) = check_deletable(&p);
            assert!(deletable, "用户路径应可删: {p}");
        }
    }

    // ---------- F-07 ----------

    #[test]
    fn root_owned_needs_privilege_is_not_dead_code() {
        // 反向验证：把生产函数改回恒返回 None，第一条断言必红。
        assert!(root_owned_needs_privilege(0, 501).is_some());
        assert!(root_owned_needs_privilege(0, 0).is_none()); // 已是 root
        assert!(root_owned_needs_privilege(501, 501).is_none()); // 普通属主
    }

    // ---------- F-08 ----------

    #[test]
    fn psdrive_json_parses_compact_and_pretty_and_rejects_garbage() {
        // 压缩单行（旧逐行解析的盲区）
        assert_eq!(
            parse_psdrive_json("{\"Used\":123,\"Free\":456}"),
            (579, 456)
        );
        // 多行展开
        assert_eq!(
            parse_psdrive_json("{\n  \"Used\": 100,\n  \"Free\": 200\n}"),
            (300, 200)
        );
        // 无效 JSON 明确 (0,0)，不再静默误报容量
        assert_eq!(parse_psdrive_json("not json"), (0, 0));
    }

    // ---------- 隔离废纸篓 sink（端到端 `--yes` 真删验证用） ----------

    fn sink_fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maclean_sink_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sink_rename_never_overwrites_same_name() {
        let tmp = sink_fixture("same_name");
        let sink = tmp.join("Trash");
        let src_dir = tmp.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("big.bin");

        std::fs::write(&src, b"first").unwrap();
        assert!(rename_into_sink(&src.to_string_lossy(), &sink));
        std::fs::write(&src, b"second").unwrap();
        assert!(rename_into_sink(&src.to_string_lossy(), &sink));

        assert_eq!(
            std::fs::read(sink.join("big.bin")).unwrap(),
            b"first".to_vec()
        );
        assert_eq!(
            std::fs::read(sink.join("big.bin [1]")).unwrap(),
            b"second".to_vec()
        );
        assert!(!src.exists(), "源文件应已被搬走");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn sink_rename_treats_dangling_symlink_as_occupied() {
        use std::os::unix::fs::symlink;
        let tmp = sink_fixture("dangling");
        let sink = tmp.join("Trash");
        std::fs::create_dir_all(&sink).unwrap();
        let taken = sink.join("big.bin");
        symlink("/definitely/not/existing/target", &taken).unwrap();

        let src = tmp.join("big.bin");
        std::fs::write(&src, b"payload").unwrap();
        assert!(rename_into_sink(&src.to_string_lossy(), &sink));

        // 悬空链接原样保留（create_new 认的是名字被占用，不看目标是否存在）
        assert!(taken.symlink_metadata().is_ok());
        assert_eq!(
            std::fs::read_link(&taken).unwrap(),
            PathBuf::from("/definitely/not/existing/target")
        );
        assert_eq!(
            std::fs::read(sink.join("big.bin [1]")).unwrap(),
            b"payload".to_vec()
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sink_rename_is_fail_closed_for_dirs_and_missing_sources() {
        let tmp = sink_fixture("fail_closed");
        let sink = tmp.join("Trash");
        let dir = tmp.join("adir");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!rename_into_sink(&dir.to_string_lossy(), &sink));
        assert!(dir.exists(), "目录必须原样保留，绝不猜一个搬法");
        assert!(!rename_into_sink(
            &tmp.join("nope.bin").to_string_lossy(),
            &sink
        ));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sink_is_debug_only_and_wired_into_move_to_trash() {
        // 源码级接线断言：按 \nmod tests 切掉测试段，避免自命中。
        let src = include_str!("mod.rs");
        let prod = src.split("\nmod tests").next().unwrap_or(src);
        assert!(
            prod.contains("if let Some(sink) = test_trash_sink()"),
            "move_to_trash 没有接上隔离 sink"
        );
        assert!(
            prod.contains("return rename_into_sink(path, &sink);"),
            "sink 分支没有真正调用改名实现"
        );
        assert!(
            prod.contains("#[cfg(debug_assertions)]"),
            "env 读取必须被 debug_assertions 包住"
        );
        assert!(
            prod.contains("#[cfg(not(debug_assertions))]"),
            "release 构建必须有恒 None 的桩"
        );
        assert!(
            prod.contains(".create_new(true)"),
            "同名防覆盖必须靠独占创建，不能退化成先 exists 再 rename"
        );
        assert!(
            prod.contains("sink.starts_with(&real_trash)"),
            "真实废纸篓必须被排除在允许的 sink 之外"
        );
    }
}
