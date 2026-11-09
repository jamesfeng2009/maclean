//! 删除路径的「来源准入」（A-07 / fail-closed 安全模型）。
//!
//! 安全闸门只允许删除**落在某个已声明扫描根下**的路径。扫描器在枚举根目录时
//! 调用 [`register_root`]；整个进程一个根都没注册时，任何文件删除都会被拒绝。
//!
//! 为什么是白名单而不是把黑名单再补厚：黑名单漏一条就是一次误删，准入根漏
//! 注册只是「少删一点」。这是 A-07 决策里「宁可少删不可误删」的落地方式。
//!
//! 与 `safety.rs` 的分工：
//! - `safety.rs` 的第 1~7 层负责**排除**（系统目录、home 黑名单、敏感文件名…）；
//! - 本模块负责**准入**（这个路径是否来自扫描器声明过的根）。
//!
//! 两者都通过才返回 `Safe`。

use std::path::{Component, Path, PathBuf};
use std::sync::{OnceLock, RwLock};

static ROOTS: OnceLock<RwLock<Vec<PathBuf>>> = OnceLock::new();

fn roots() -> &'static RwLock<Vec<PathBuf>> {
    ROOTS.get_or_init(|| RwLock::new(Vec::new()))
}

/// 词法规范化：消解 `.` 与可消解的 `..`，不访问文件系统、不解析符号链接。
/// 符号链接由 `safety.rs` 第 2 层单独处理，这里只保证组件比较不被 `..` 骗过。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // 只在能确认上一级是普通组件时回退；不能拿它越出根边界。
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn add_root(list: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidate.as_os_str().is_empty() && !list.contains(&candidate) {
        list.push(candidate);
    }
}

/// 声明一个扫描根。扫描器应在枚举之前调用；重复注册无害。
///
/// 同时登记词法形态与 `canonicalize()` 后的形态 —— macOS 上 `/var/folders/...`
/// 的规范形态是 `/private/var/folders/...`，只存一种会让真实扫描结果匹配不上。
pub fn register_root(root: &Path) {
    let lexical = normalize(root);
    let canonical = root.canonicalize().ok();
    let mut list = roots().write().unwrap_or_else(|e| e.into_inner());
    add_root(&mut list, lexical.clone());

    // macOS 上 /var、/tmp 分别是 /private/var、/private/tmp 的符号链接。
    // 若注册时目录尚不存在（测试里先注册再造目录是常态），canonicalize
    // 会失败，此时只登记词法形态，扫描结果 canonical 后的
    // `/private/var/folders/...` 就永远匹配不上 —— 表现为「明明注册了根
    // 却仍被闸门拒绝」。这里显式补一份 /private 变体，不依赖文件系统状态。
    for (link, real) in [("/var", "/private/var"), ("/tmp", "/private/tmp")] {
        if let Ok(rel) = lexical.strip_prefix(link) {
            add_root(&mut list, Path::new(real).join(rel));
        }
    }

    if let Some(c) = canonical {
        add_root(&mut list, c);
    }
}

/// 平台无关的路径组件键。
///
/// Windows 的 `canonicalize()` 返回 `\\?\C:\...` verbatim 前缀，而注册表 /
/// `dirs::home_dir()` 给的是 `C:\...`；两者的 `Component::Prefix` 不相等，
/// 直接按 `Path::components()` 比较会让「扫描根准入」「home 黑名单」这类
/// 组件级前缀判定在 Windows 上**整体失效**（曾表现为第 8 层恒 false）。
/// 这里统一：归一分隔符、剥掉 verbatim 前缀、Windows 下大小写不敏感。
///
/// `windows_style` 由入参传入而非 `cfg!`，这样 Windows 分支在开发机上也能真跑。
pub(crate) fn path_component_key_for(path: &Path, windows_style: bool) -> Vec<String> {
    let raw = path.to_string_lossy().replace('\\', "/");
    let stripped = raw.strip_prefix("//?/").unwrap_or(raw.as_str());
    stripped
        .split('/')
        .filter(|c| !c.is_empty())
        .map(|c| {
            if windows_style {
                c.to_ascii_lowercase()
            } else {
                c.to_string()
            }
        })
        .collect()
}

pub(crate) fn path_component_key(path: &Path) -> Vec<String> {
    path_component_key_for(path, cfg!(target_os = "windows"))
}

/// 平台无关的字符串键：归一分隔符并剥掉 Windows verbatim 前缀，
/// 保留前导 `/` 与盘符，供仍按字符串前缀匹配的准入表使用。
pub(crate) fn path_key_string(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    raw.strip_prefix("//?/").unwrap_or(&raw).to_string()
}

/// 把 `canonicalize()` 返回的 `\\?\C:\...` 还原成普通盘符路径。
///
/// 只剥盘符形态（第三个字符是盘符、第四个是冒号）；`\\?\UNC\...` 保持原样，
/// 避免造出一个根本不存在的路径。下游凡是按 `Path::starts_with` / 组件比较的
/// 判定，都要求 canonical 与 home 有同一形态的 Prefix。
pub(crate) fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    }
}

/// `path == root` 或 `path` 位于 `root` 之下（组件级，跨平台）。
/// 用组件而不是裸字符串 `starts_with`，避免 `CachesEvil` 命中 `Caches`。
pub(crate) fn path_is_under_key(path: &Path, root: &Path) -> bool {
    let p = path_component_key(path);
    let r = path_component_key(root);
    !r.is_empty() && p.len() >= r.len() && r.iter().zip(p.iter()).all(|(a, b)| a == b)
}

fn components_under(path: &Path, root: &Path) -> bool {
    path_is_under_key(path, root)
}

/// 路径是否落在任何已声明的扫描根下（fail-closed：无根注册时恒 false）。
pub fn is_vetted(path: &Path) -> bool {
    let lexical = normalize(path);
    let canonical = path.canonicalize().ok();
    let list = roots().read().unwrap_or_else(|e| e.into_inner());
    list.iter().any(|root| {
        components_under(&lexical, root)
            || canonical
                .as_deref()
                .is_some_and(|c| components_under(c, root))
    })
}

/// 标准可清理根（A-07b / CORE-10）：只登记**明确属于可清理范围**的根，
/// 即已知工具缓存 / 应用数据目录；绝不把整个 `$HOME` 注册进来 —— 那样等于
/// 把准入制退化成原来的纯黑名单，§6.1 的 fail-closed 收敛就白做了。
///
/// Downloads / Desktop / Documents / 项目目录属于**用户数据**而非可再生缓存，
/// 已从本表移出，改由「大文件」类目按需登记，见 [`LARGE_FILE_HOME_ROOTS`]。
/// 抽成常量是为了能直接断言口径，且不受进程内全局注册表的并行测试干扰。
const STANDARD_HOME_ROOTS: &[&str] = &[
    "Library/Caches",
    "Library/Application Support",
    "Library/Containers",
    "Library/Group Containers",
    "Library/Logs",
    "Library/Preferences",
    "Library/Saved Application State",
    "Library/HTTPStorages",
    "Library/Developer",
    ".cache",
    ".npm",
    ".yarn",
    ".cargo",
    ".gradle",
    ".m2",
    ".pub-cache",
    ".nuget",
    "go",
];

/// CORE-10：「大文件 / 大目录」类目专用的准入根。
///
/// 这些是用户数据目录，不是可再生缓存根。它们只在
/// `LargeFileScanner::scan()` 运行时登记（谁枚举谁声明），且删除仍要求
/// 用户在大文件视图里**主动勾选** —— 第 8 层只做来源准入，不代替用户选择。
/// 常驻登记会让其它类目也顺带获得这些根的删除许可，把准入面放大。
const LARGE_FILE_HOME_ROOTS: &[&str] = &[
    "Downloads",
    "Desktop",
    "Documents",
    "workspace",
    "projects",
    "dev",
    "Development",
];

/// 注册 CLI 扫描入口的一组**标准可清理根**（A-07b）。
///
/// 各扫描器若枚举了表外的根，应在自己的 `scan()` 里追加 [`register_root`]
/// 或 [`register_large_file_roots`]；漏注册只会少删，不会误删。
pub fn register_standard_scan_roots() {
    if let Some(home) = dirs::home_dir() {
        for rel in STANDARD_HOME_ROOTS {
            register_root(&home.join(rel));
        }
    }
    for p in [
        "/Library/Caches",
        "/Library/Developer",
        "/Applications",
        "/tmp",
        "/private/tmp",
        "/var/tmp",
    ] {
        register_root(Path::new(p));
    }
    register_root(&std::env::temp_dir());
}

/// CORE-10：由大文件扫描器**按需**声明用户数据根。
///
/// 不在 [`register_standard_scan_roots`] 里常驻登记：其它类目的扫描器没有
/// 枚举这些根，也不该获得它们的删除许可。`home` 由调用方传入，测试可用
/// 临时 HOME 验证而不依赖进程环境。
pub fn register_large_file_roots(home: &Path) {
    for rel in LARGE_FILE_HOME_ROOTS {
        register_root(&home.join(rel));
    }
}

/// 当前已注册的准入根快照（诊断/测试用）。
pub fn registered_roots() -> Vec<PathBuf> {
    roots().read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 注册表是进程内全局状态，测试并行执行。**不允许**在用例里 clear：
    // 那会随机抹掉别的用例刚注册的根，制造不稳定测试。每个用例用自己
    // 的唯一根，累加注册互不干扰。
    fn unique_root(tag: &str) -> PathBuf {
        Path::new("/__maclean_provenance_test__").join(tag)
    }

    #[test]
    fn unregistered_path_is_never_vetted() {
        assert!(!is_vetted(
            &unique_root("never_registered").join("sub/file")
        ));
    }

    #[test]
    fn component_prefix_rejects_sibling_with_same_string_prefix() {
        let root = unique_root("caches");
        register_root(&root);
        assert!(is_vetted(&root.join("foo/bar")));
        assert!(is_vetted(&root));

        let mut sibling = root.clone().into_os_string();
        sibling.push("Evil");
        assert!(!is_vetted(&PathBuf::from(sibling).join("foo")));
        assert!(!is_vetted(Path::new("/__maclean_provenance_test__")));
    }

    #[test]
    fn parent_dir_cannot_escape_root() {
        let root = unique_root("escape");
        register_root(&root);
        assert!(!is_vetted(Path::new(
            "/__maclean_provenance_test__/escape/../other/file"
        )));
        assert!(!is_vetted(Path::new(
            "/__maclean_provenance_test__/other/file"
        )));
    }

    #[test]
    fn multiple_roots_are_all_checked() {
        let a = unique_root("root-a");
        let b = unique_root("root-b");
        register_root(&a);
        register_root(&b);
        assert!(is_vetted(&a.join("one")));
        assert!(is_vetted(&b.join("two")));
        assert!(!is_vetted(&unique_root("root-c").join("three")));
    }

    #[test]
    fn macos_private_var_alias_covers_root_that_does_not_exist_yet() {
        // 回归：注册时目录不存在 → canonicalize 失败 → 只登记 /var 词法形态，
        // 扫描结果的 /private/var 规范形态匹配不上，闸门把正常项全拦下。
        let root = Path::new("/var/folders/zz/__maclean_missing_root_2026__");
        register_root(root);
        assert!(is_vetted(Path::new(
            "/private/var/folders/zz/__maclean_missing_root_2026__/sub/file"
        )));
    }

    #[test]
    fn standard_roots_registration_is_not_empty() {
        register_standard_scan_roots();
        assert!(!registered_roots().is_empty());
    }

    // ---------- CORE-10：准入根口径与文档一致 ----------

    #[test]
    fn c10_standard_roots_exclude_user_data_directories() {
        // 表本身的口径：标准根只含可再生缓存 / 应用数据根，两张表不重叠。
        for r in STANDARD_HOME_ROOTS {
            assert!(
                !LARGE_FILE_HOME_ROOTS.contains(r),
                "用户数据根混进了标准准入根表: {r}"
            );
        }
        // registered_roots() 快照与文档口径一致：表内每一项都确实登记了。
        // 只断言「在快照里」，不受其它用例并行追加注册的干扰。
        register_standard_scan_roots();
        let snap = registered_roots();
        let home = dirs::home_dir().unwrap_or_default();
        for rel in STANDARD_HOME_ROOTS {
            assert!(
                snap.iter().any(|p| p == &home.join(rel)),
                "标准根表里的 {rel} 没出现在 registered_roots() 快照中"
            );
        }
    }

    #[test]
    fn c10_large_file_roots_are_registered_on_demand() {
        // 用不存在的唯一假 home：真实 /tmp 已被标准表注册，is_vetted 会
        // 放行 /tmp 下的一切，用它会把断言掩盖掉。
        let home = Path::new("/__maclean_c10_probe__");
        let target = home.join("Downloads/big.dmg");
        assert!(!is_vetted(&target), "标准根不该覆盖用户数据目录");
        register_large_file_roots(home);
        assert!(is_vetted(&target), "大文件类目按需注册后应放行");
        assert!(is_vetted(&home.join("Documents/report.pdf")));
        assert!(
            !is_vetted(&home.join("Movies/never.declared")),
            "未声明的目录仍应 fail-closed"
        );
    }
}
