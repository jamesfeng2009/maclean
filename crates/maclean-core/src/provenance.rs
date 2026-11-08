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

/// 组件级前缀判定：`path == root` 或 `path` 是 `root` 的后代。
/// 用 `Component` 而不是字符串 `starts_with`，避免 `CachesEvil` 命中 `Caches`。
fn components_under(path: &Path, root: &Path) -> bool {
    let mut p = path.components();
    let mut r = root.components();
    loop {
        match r.next() {
            Some(rc) => match p.next() {
                Some(pc) if pc == rc => continue,
                _ => return false,
            },
            None => return true,
        }
    }
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

/// 注册 CLI 扫描入口的一组**标准可清理根**（A-07b）。
///
/// 只登记「明确属于可清理范围」的根，绝不把整个 `$HOME` 注册进来 ——
/// 那样等于把准入制退化成原来的纯黑名单。各扫描器若枚举了表外的根，
/// 应在自己的 `scan()` 里追加 [`register_root`]；漏注册只会少删，不会误删。
pub fn register_standard_scan_roots() {
    if let Some(home) = dirs::home_dir() {
        const HOME_ROOTS: &[&str] = &[
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
            "Downloads",
            "Desktop",
            "Documents",
            "workspace",
            "projects",
            "dev",
            "Development",
        ];
        for rel in HOME_ROOTS {
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
}
