//! 卸载残留的名称匹配规则
//!
//! 这个模块刻意**不做** `cfg(target_os)` 限定，尽管它目前只服务于 Windows 侧。
//!
//! 原因很实在：`scanner::windows_apps` 整体是 `#[cfg(target_os = "windows")]`，
//! 代码写在里面的话，开发机（macOS）上既编译不到也测不到 —— 而这里每一个
//! true/false 最终都落到 `remove_dir_all` 或 `reg delete /f`
//! （`/f` 跳过确认且递归删除整棵子树）。写完没人验证，等同于没保护。
//!
//! 代价是本模块的非测试代码在本机会触发 dead_code，故整体 `allow(dead_code)`。

#![allow(dead_code)]

// =========================================================================
//  安全关键：下面每个 true/false 都导向不可逆操作
// =========================================================================
//
//  - 目录匹配 → `delete_filesystem_residual` 走 `remove_dir_all`（无确认、不进回收站）
//  - 注册表匹配 → `delete_registry_residual` 走 `reg delete <key> /f`
//    （`/f` 跳过确认，**递归删除整个子树**）
//
//  原先的写法有两处致命缺陷：
//  1. 双向子串：`key.contains(name) || name.contains(key)`。反向那条让短键名
//    （如 `Microsoft`）能被长应用名（`Microsoft Edge`）"包含"从而命中。
//  2. 首词提取：`generate_search_names` 会把 "Microsoft Edge" 变成 "microsoft"，
//    即便只保留正向匹配，`Microsoft` 键依然被命中 —— 结果是删掉该用户
//    几乎所有 Microsoft 产品的设置。
//
//  更糟的是 `DisplayName` / `Uninstall` 键由安装方自己写入，属于攻击者可控输入：
//  把自己命名为 "Microsoft"，卸载时就能借本工具抹掉别人的注册表子树。
//
//  所以这里定了四条硬规则：
//  - **不做反向包含**（短键名永远不能被长名字含住）
//  - **不猜首词**（厂商键是共享的，删掉会连坐同厂商其它产品）
//  - **不做子串包含**（见下，A 类灾难的结构性根因）
//  - **受保护名单兜底**（即使前两条被绕过，共享厂商容器也删不掉）
//
//  [#4] 为什么连**正向**子串包含也要砍掉
//  ------------------------------------
//  `target.contains(app)` 的精度和爆炸半径是**反相关**的：目标名字越长、
//  越像共享厂商容器，就越容易含住某个短 token。这条规则会系统性挑中
//  「最不能删的那批目标」：
//
//    "GitHub".contains("Git")        → 卸载 Git 删掉 GitHub Desktop 用户数据
//    "TeamViewer".contains("Team")   → 卸载一个叫 Team 的东西删掉另一家产品
//    "JavaSoft".contains("Java")     → reg delete /f 注销机器上全部 JRE
//
//  于是失效被分成两类，之前被混为一谈：
//
//    A 类 · 词根拼接（上面三个）：目标名与 app 名**词根不同**，只是字符串相含。
//          可以用「折叠后精确相等」**结构性**消除，不损失任何正当召回
//          （`Docker Desktop` → 目录 `DockerDesktop` 由折叠相等天然覆盖）。
//    B 类 · 厂商名即容器名（`Microsoft` 键 vs `Microsoft Edge` 首词）：
//          字符串层面**无法**区分恶意与巧合（Uninstall 键整棵由安装方自写，
//          恶意包把自己叫 "Microsoft" 就能借本工具抹别人子树）。
//          只能靠「不猜首词」+ 保护名单拦截，二者缺一不可。
//
//  A 类已封死；B 类的召回改用版本化下钻（`version_scoped_subkey`）补回来 ——
//  不下模糊匹配的牌，只删推得出版本号的那一个节点。

/// 参与子串匹配所需的最短应用名长度
///
/// 低于这个长度的名字（"Go" / "R" / "V"）做子串匹配会命中大量无关目标：
/// `"google".contains("go") == true`，于是卸载 Go 会连 Chrome 用户数据一起
/// `remove_dir_all` 删掉。
pub(crate) const MIN_MATCH_LEN: usize = 4;

/// 归一化用于比对的名称：转小写，只保留字母数字
pub(crate) fn normalize_residual_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// AppData 下由多个产品共享的顶级容器目录，永不作为残留删除
///
/// 这些目录一旦被删，损失的不只是被卸载的那个应用 —— 比如 `Tencent`
/// 同时装着微信和 QQ，`Google` 装着 Chrome 全部 profile（含保存的密码）。
pub(crate) const PROTECTED_APPDATA_ROOTS: &[&str] = &[
    "microsoft",
    "microsoftcorporation",
    "microsoftwindows",
    "windows",
    "google",
    "googlellc",
    "appleinc",
    "applecomputerinc",
    "mozilla",
    "mozillacorporation",
    "oracle",
    "java",
    "javasoft",
    "tencent",
    "adobe",
    "nvidiacorporation",
    "intelcorporation",
    "operasoftware",
    "valve",
];

/// 共享厂商的注册表顶级键，永不作为残留删除
pub(crate) const PROTECTED_REG_ROOTS: &[&str] = &[
    "microsoft",
    "microsoftcorporation",
    "microsoftwindows",
    "microsoftoffice",
    "microsoftshared",
    "windows",
    "windowsnt",
    "wow6432node",
    "classes",
    "clients",
    "policies",
    "registeredapplications",
    "google",
    "googlellc",
    "mozilla",
    "mozillacorporation",
    "oracle",
    "java",
    "javasoft",
    "tencent",
    "odbc",
    "opengl",
    "debug",
];

/// 应用名变体：原样 + 去掉尾部的版本号/空格
///
/// "Python 3.11" → ["python311", "python"]。注意坑在这里：原实现会额外取
/// "第一个词"，那正是 "Microsoft Edge" 命中 `Microsoft` 键的来源。
/// "Go" 这类短名没有版本后缀可去，变体仍是 2 字符，会被长度门槛挡下。
pub(crate) fn search_name_variants(app_name: &str) -> Vec<String> {
    let normalized = normalize_residual_name(app_name);
    if normalized.is_empty() {
        return Vec::new();
    }

    let mut variants = vec![normalized.clone()];

    let without_version = app_name
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
        .trim();
    let trimmed = normalize_residual_name(without_version);
    if trimmed.len() >= MIN_MATCH_LEN && trimmed != normalized && !variants.contains(&trimmed) {
        variants.push(trimmed);
    }

    variants
}

/// 应用名变体（**保留**空格与标点，便于和环境变量里的路径原样比对）
///
/// 与 `search_name_variants` 的区别：后者把名字压成纯字母数字，适合比较
/// 目录名/键名；这里要拿去和 `C:\Program Files\...` 这种带分隔符的字符串比，
/// 压平反而匹配不上。同样不带首词提取，且一律过长度门槛。
pub(crate) fn display_name_variants(app_name: &str) -> Vec<String> {
    let full = app_name.to_lowercase();
    let mut variants = vec![full.clone()];

    let without_version = app_name
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
        .trim()
        .to_lowercase();
    if without_version != full && !without_version.is_empty() {
        variants.push(without_version);
    }

    variants
        .into_iter()
        .filter(|v| v.len() >= MIN_MATCH_LEN)
        .collect()
}

/// 目标名（目录名 / 注册表键名）是否命中任一应用名变体
///
/// **只有折叠后精确相等才算命中**，这里刻意不保留任何形式的子串包含。
///
/// 拼接式目录由折叠相等天然覆盖（`Docker Desktop` → `DockerDesktop`），
/// 而 `"GitHub".contains("Git") == true` 这类 A 类误杀被彻底排除 ——
/// 砍掉它的代价是少量漏报（例如 `Zoom Workplace` 匹配不到目录 `Zoom`），
/// 相对 `remove_dir_all` / `reg delete /f` 的不可逆性，这个取舍是明摆着的。
///
/// 长度门槛同样适用于相等判定：应用名短于 `MIN_MATCH_LEN` 时，连同名目录
/// 也不放行。`"Go"` 这种名字的用户自己建的目录可能也叫 Go，规则一开例外
/// 就守不住了。
fn matches_variant(target_name: &str, app_name: &str) -> bool {
    let target = normalize_residual_name(target_name);
    if target.is_empty() {
        return false;
    }

    search_name_variants(app_name)
        .iter()
        .any(|variant| variant.len() >= MIN_MATCH_LEN && target == *variant)
}

/// 目录是否可以作为该应用的残留被删除
///
/// 三条门槛全过才返回 true：非共享容器、至少一个变体达到长度要求、
/// 且**折叠后精确相等**（绝不部分匹配，也不反向包含）。
pub(crate) fn is_residual_dir(dir_name: &str, app_name: &str) -> bool {
    let dir_key = normalize_residual_name(dir_name);
    if dir_key.is_empty() || PROTECTED_APPDATA_ROOTS.contains(&dir_key.as_str()) {
        return false;
    }

    matches_variant(dir_name, app_name)
}

/// 注册表顶级键是否可以作为该应用的残留被删除
///
/// 与 `is_residual_dir` 同构，区别只是换成注册表保护名单。
pub(crate) fn is_residual_reg_key(key_name: &str, app_name: &str) -> bool {
    let key = normalize_residual_name(key_name);
    if key.is_empty() || PROTECTED_REG_ROOTS.contains(&key.as_str()) {
        return false;
    }

    matches_variant(key_name, app_name)
}

// =========================================================================
//  版本化下钻：用精确路径换回 B 类的召回
// =========================================================================
//
//  B 类目标（`JavaSoft`、`Microsoft`、`Oracle`）确实装着我们该清掉的残留，
//  但整棵容器不能碰。替代方案不是把模糊匹配加回来，而是把删除**粒度下移**：
//
//    HKLM\SOFTWARE\JavaSoft                                    ← 绝不删
//    HKLM\SOFTWARE\JavaSoft\Java Runtime Environment           ← 还是容器，不删
//    HKLM\SOFTWARE\JavaSoft\Java Runtime Environment\1.8.0_291 ← 只删这一个节点
//
//  最后一段版本号不用猜，从 Uninstall 键里安装方自己写的 `InstallLocation`
//  推：`C:\Program Files\Java\jre1.8.0_291` → `1.8.0_291`。证据来自安装方，
//  归属因此明确；同时 /f 递归的范围只剩这一个版本的子树。

/// `reg delete` 目标在 `SOFTWARE` 之下至少要有的层级数
///
/// 1 层就是厂商共享容器本身（`SOFTWARE\JavaSoft`），整棵砍掉是灾难；
/// 2 层（`JavaSoft\Java Runtime Environment`）一般还是产品线容器，也要挡。
/// 真正能安全删的是版本节点那一层，正好卡在这个值以上。
pub(crate) const MIN_REG_KEY_DEPTH: usize = 3;

/// 从安装目录叶名里提取版本串
///
/// `...\Java\jre1.8.0_291` → `Some("1.8.0_291")`，`...\Git` → `None`。
///
/// 两个过滤条件是精度的来源：
/// - 版本段必须含 `.` 或 `_`，否则 `Python311` 这种纯单词会被当成版本；
/// - 版本段之前至少两个字母，排除 `v2.1` 之类的噪声前缀。
pub(crate) fn install_dir_version_token(location: &str) -> Option<String> {
    let leaf = location
        .trim()
        .trim_matches('"')
        .split(|c| c == '/' || c == '\\')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .last()?;

    let digit_at = leaf.char_indices().find(|(_, c)| c.is_ascii_digit())?.0;
    if digit_at < 2 {
        return None;
    }

    let prefix = &leaf[..digit_at];
    let alpha_count = prefix.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if alpha_count < 2 {
        return None;
    }

    let version = leaf[digit_at..].trim_end_matches(|c| c == '/' || c == '\\');
    if version.is_empty() || (!version.contains('.') && !version.contains('_')) {
        return None;
    }

    Some(version.to_ascii_lowercase())
}

/// `reg delete <path> /f` 之前的结构性兜底（保护名单之外的第二道闸）
///
/// 把一条注册表键路径拆成 `SOFTWARE`（或 `Classes`）之后的段
///
/// 返回 `None` 表示这根本不像一条完整的键路径：没有 hive 前缀，或者找不到
/// `SOFTWARE`。中间的 `Wow6432Node` 只是 32 位包装层，不计入深度。
///
/// 抽出来是因为两档闸门要共用同一套拆解逻辑 —— 各写一份迟早会长歪。
fn reg_path_segments(full_key: &str) -> Option<Vec<String>> {
    let normalized = full_key.replace('\\', "/");
    let mut segs: Vec<String> = normalized
        .split('/')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    if segs.len() < 2 {
        return None;
    }

    // hive 段：没有就不认这条路径
    let first = segs[0].to_ascii_uppercase();
    let has_hive = first.starts_with("HKEY")
        || matches!(first.as_str(), "HKLM" | "HKCU" | "HKCR" | "HKU" | "HKCC");
    if !has_hive {
        return None;
    }
    segs.remove(0);

    let software_at = segs
        .iter()
        .position(|s| s.eq_ignore_ascii_case("software"))
        .or_else(|| segs.iter().position(|s| s.eq_ignore_ascii_case("classes")))?;
    segs = segs.split_off(software_at + 1);
    segs.retain(|s| !s.eq_ignore_ascii_case("wow6432node"));

    if segs.is_empty() {
        return None;
    }
    Some(segs)
}

/// **档位一 · 普通残留键**的最低门槛（给 `delete_registry_residual` 用）
///
/// 这一档只能挡"形状明显不对"的目标，**不能**照搬档位二的深度要求：
/// `scan_registry_residual` 现在产出的合法目标就是 `SOFTWARE` 的**一级**
/// 子键（`HKCU\SOFTWARE\Zoom`），要求深度 ≥3 会让整个注册表残留清理失效。
///
/// 挡的是：
/// - 没有 hive / 找不到 SOFTWARE ⇒ 不是完整键路径
/// - 末段是共享厂商容器（保护名单）
/// - 末段是空的（形如 `HKCU\SOFTWARE\`）
pub(crate) fn is_deletable_reg_key_basic(full_key: &str) -> bool {
    let segs = match reg_path_segments(full_key) {
        Some(s) => s,
        None => return false,
    };

    let leaf = match segs.last() {
        Some(leaf) => normalize_residual_name(leaf),
        None => return false,
    };
    if leaf.is_empty() || PROTECTED_REG_ROOTS.contains(&leaf.as_str()) {
        return false;
    }

    true
}

/// **档位二 · 版本化下钻目标**的严格门槛（`reg delete /f` 递归删整棵子树）
///
/// 即便调用方算错了归属、或者哪天有人手工填了条坏映射，这里也拦得住最致命
/// 的形状：
/// - SOFTWARE 之下层级不够（厂商容器、产品线容器）
/// - 末段落在保护名单里
/// - 末段不含数字：这类键大多带版本号，纯单词的叶子更可能是别的东西
pub(crate) fn is_deletable_reg_path(full_key: &str) -> bool {
    let segs = match reg_path_segments(full_key) {
        Some(s) => s,
        None => return false,
    };

    if segs.len() < MIN_REG_KEY_DEPTH {
        return false;
    }

    let leaf = match segs.last() {
        Some(leaf) => normalize_residual_name(leaf),
        None => return false,
    };
    if leaf.is_empty() || PROTECTED_REG_ROOTS.contains(&leaf.as_str()) {
        return false;
    }
    if !leaf.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }

    true
}

/// **文件系统残留目录**的最低门槛（给 `delete_filesystem_residual` 用）
///
/// 与注册表同构的思路：只挡形状明显不对的目标，避免把现有合法删除挡掉。
/// - 空路径、盘符根、只有一段的路径
/// - `AppData\<Roaming|Local|LocalLow>\<共享厂商容器>` 这一层
///   （`...\AppData\Roaming\Microsoft` 下装着同厂商所有产品）
pub(crate) fn is_deletable_fs_path(path: &str) -> bool {
    let p = path.trim().trim_matches('"');
    if p.is_empty() {
        return false;
    }

    let normalized = p.replace('\\', "/");
    let segs: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();

    // 只有一段 = 盘符根或单纯的根目录，删不得
    if segs.len() < 2 {
        return false;
    }

    let leaf = match segs.last() {
        Some(leaf) => normalize_residual_name(leaf),
        None => return false,
    };
    if leaf.is_empty() {
        return false;
    }

    // AppData\Roaming\<vendor> / AppData\Local\<vendor> 这一层是共享容器
    for (i, s) in segs.iter().enumerate() {
        if s.eq_ignore_ascii_case("appdata") {
            if let Some(vendor) = segs.get(i + 2) {
                let key = normalize_residual_name(vendor);
                if PROTECTED_APPDATA_ROOTS.contains(&key.as_str()) {
                    return false;
                }
            }
        }
    }

    true
}

/// 在共享容器的子树里挑出**只属于该版本**的那一个键
///
/// `subkeys` 由运行时的 `reg query /s` 枚举得到。返回 `None` 有两种情况，
/// 都必须保持拒绝而不是凑合：
/// - 0 个命中：没有可下的钻，宁可漏；
/// - 多个命中：归属判断不了，比不删更危险，直接放弃。
pub(crate) fn version_scoped_subkey(subkeys: &[String], version: &str) -> Option<String> {
    if version.is_empty() {
        return None;
    }
    let wanted = version.trim().to_ascii_lowercase();

    let mut hits: Vec<String> = subkeys
        .iter()
        .map(|k| k.trim().replace('\\', "/"))
        .filter(|k| {
            k.rsplit('/')
                .next()
                .map(|leaf| leaf.trim().to_ascii_lowercase())
                == Some(wanted.clone())
        })
        .collect();
    hits.dedup();

    if hits.len() != 1 {
        return None;
    }

    let candidate = hits.remove(0);
    if is_deletable_reg_path(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

/// **保护性**判定的名字近似度（给 `has_large_user_data` 这类启发式用）
///
/// 方向和 `is_residual_dir` 完全相反，刻意不共用同一套规则：那边命中 →
/// `remove_dir_all`，所以宁可漏报；这边命中 → **不允许卸载**，漏报意味着
/// 少了一层保护（fail-open）。照搬严格相等会把保护一起漏掉。
///
/// 于是这里取宽松语义：折叠后双向包含即可，也不设 4 字门槛。
/// 过度保护的后果只是「这个应用本工具不帮你卸」，可以接受；代价由调用方的
/// 50MB 体积阈值兜着 —— 名字再像，凑不出 50MB 也不会触发保护。
pub(crate) fn resembles_app_dir(dir_name: &str, app_name: &str) -> bool {
    let dir_key = normalize_residual_name(dir_name);
    let app_key = normalize_residual_name(app_name);
    if dir_key.is_empty() || app_key.is_empty() {
        return false;
    }
    if dir_key == app_key {
        return true;
    }

    let shorter = dir_key.len().min(app_key.len());
    if shorter < 2 {
        return false;
    }
    dir_key.contains(&app_key) || app_key.contains(&dir_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    // =====================================================================
    //  残留名称匹配回归（X-1 注册表 / X-2 文件系统）
    //
    //  这些用例在开发机（macOS）上就能跑，不需要真的有台 Windows —— 这也是
    //  本模块刻意不加 cfg 的唯一目的。代价是 `scan_*` 这类要调 reg.exe /
    //  读真实 AppData 的函数没法端到端跑起来，退而求其次做源码级检查，
    //  见最末两条。
    // =====================================================================

    #[test]
    fn uninstalling_short_named_app_does_not_target_unrelated_dirs() {
        // 卸载 Go 时，%LOCALAPPDATA%\Google 曾被判成残留并被 remove_dir_all，
        // 连 Chrome 保存的密码一起消失。
        assert!(!is_residual_dir("Google", "Go"));
        assert!(!is_residual_dir("google", "Go"));
        assert!(!is_residual_dir("Realtek", "R"));
        assert!(!is_residual_dir("Valve", "V"));
        // 3 字符以下连"看着就是自己"的目录也不放行，避免长度规则被开例外
        assert!(!is_residual_dir("Google", "Goo"));
    }

    #[test]
    fn shared_appdata_containers_are_never_residuals() {
        // 同厂商多个产品共用，删掉会连坐无关应用
        for dir in [
            "Microsoft",
            "Tencent",
            "Google",
            "Mozilla",
            "Adobe",
            "Oracle",
            "Java",
        ] {
            for app in [
                "Microsoft Edge",
                "WeChat",
                "Chrome",
                "Firefox 120",
                "Adobe Acrobat",
            ] {
                assert!(
                    !is_residual_dir(dir, app),
                    "共享目录 {} 被当成了 {} 的残留",
                    dir,
                    app
                );
            }
        }
    }

    #[test]
    fn dir_matching_is_never_reverse_containment() {
        assert!(!is_residual_dir("Microsoft", "Microsoft Edge"));
        assert!(!is_residual_dir("Java", "Java 8 Update 291"));
    }

    #[test]
    fn genuine_residual_dirs_are_still_detected() {
        // 光拦不住没用，正常残留必须还能扫出来，否则功能等于被废掉
        assert!(is_residual_dir("Zoom", "Zoom"));
        assert!(is_residual_dir("Discord", "Discord"));
        assert!(is_residual_dir("7-Zip", "7-Zip 22.01"));
        assert!(is_residual_dir("Spotify", "Spotify"));
        // 去版本号后应与目录名匹配
        assert!(is_residual_dir("Python", "Python 3.11"));
    }

    #[test]
    fn empty_or_junk_app_names_match_nothing() {
        assert!(!is_residual_dir("Anything", ""));
        assert!(!is_residual_dir("Anything", "   "));
        assert!(!is_residual_reg_key("Anything", ""));
        assert!(!is_residual_dir("", "Discord"));
    }

    #[test]
    fn registry_protected_roots_are_never_deleted() {
        // `reg delete <key> /f` 递归删整棵子树。DisplayName 由安装方写入
        // （攻击者可控），所以保护名单是最后一道防线，不能只靠名字长度。
        for key in [
            "Microsoft",
            "Windows",
            "Wow6432Node",
            "Classes",
            "Policies",
            "Clients",
            "RegisteredApplications",
            "Google",
            "Mozilla",
            "Oracle",
        ] {
            for app in [
                "Microsoft Edge",
                "Microsoft Visual Studio Code",
                "Windows Terminal",
                "Google Chrome",
                "Mozilla Firefox 120.0",
            ] {
                assert!(
                    !is_residual_reg_key(key, app),
                    "受保护注册表键 {} 被当成了 {} 的残留",
                    key,
                    app
                );
            }
        }
    }

    #[test]
    fn edge_uninstall_does_not_match_whole_microsoft_key() {
        // 原始缺陷：首词提取得到 "microsoft"，HKCU\SOFTWARE\Microsoft
        // 因此被整棵 reg delete /f 掉。
        assert!(!is_residual_reg_key("Microsoft", "Microsoft Edge"));
        assert!(!is_residual_reg_key("microsoft", "Microsoft Edge"));
        // 即便攻击者把 DisplayName 直接伪造成厂商名也删不掉
        assert!(!is_residual_reg_key("Microsoft", "Microsoft"));
    }

    #[test]
    fn short_app_names_do_not_match_registry_keys() {
        assert!(!is_residual_reg_key("Google", "Go"));
        assert!(!is_residual_reg_key("Realtek", "R"));
        assert!(!is_residual_reg_key("Valve", "V"));
    }

    #[test]
    fn genuine_registry_residuals_are_still_detected() {
        assert!(is_residual_reg_key("7-Zip", "7-Zip 22.01"));
        assert!(is_residual_reg_key("Notion", "Notion 2.0"));
        assert!(is_residual_reg_key("Discord", "Discord"));
        assert!(is_residual_reg_key("Zoom", "Zoom"));
        assert!(
            !is_residual_reg_key("Tencent", "Tencent"),
            "共用容器键仍受保护"
        );
    }

    #[test]
    fn documented_recall_loss_and_its_replacement() {
        // 刻意接受的能力回退："Java 8 Update 291" 再也匹配不到键 "JavaSoft"。
        // 想靠旧机制命中只能让 "java" ⊂ "javasoft"，而正向子串包含正是
        // "GitHub" ⊂ "Git"、"TeamViewer" ⊂ "Team" 那一整类误删的来源。
        //
        // 召回不是丢掉而是换了条路：X-5 的 `install_dir_version_token`
        // + `version_scoped_subkey` 从 InstallLocation 推出版本号，只删
        // `...\Java Runtime Environment\1.8.0_291` 这一个节点。
        // 证据取自安装方自填的路径，归属明确，/f 的递归范围也只剩单版本子树。
        assert!(!is_residual_reg_key("JavaSoft", "Java 8 Update 291"));
        // 名字里带了键名、且该键不在保护名单时仍然正常
        assert!(is_residual_reg_key("Notion", "Notion"));
    }

    #[test]
    fn display_name_variants_drop_short_names() {
        // 环境变量 PATH 条目清理用这张表：短名必须产生空列表，
        // 否则卸载 Go 会把 C:\Program Files\Google\... 从 PATH 里删掉。
        assert!(display_name_variants("Go").is_empty());
        assert!(display_name_variants("R").is_empty());
        assert!(display_name_variants("V").is_empty());
        // 长名保留原样 + 去版本号（保留空格，便于和路径比对）
        let v = display_name_variants("Google Chrome 119.0");
        assert!(v.contains(&"google chrome 119.0".to_string()));
        assert!(v.contains(&"google chrome".to_string()));
    }

    #[test]
    fn normalization_strips_separators_consistently() {
        assert_eq!(normalize_residual_name("Google Chrome"), "googlechrome");
        assert_eq!(normalize_residual_name("google_chrome-2"), "googlechrome2");
        assert_eq!(normalize_residual_name("7-Zip"), "7zip");
        assert_eq!(normalize_residual_name(""), "");
    }

    // ---------------------------------------------------------------------
    //  X-5：A 类（词根拼接）误杀
    //
    //  这类目标完全不属于被卸载的应用，只是名字恰好把它的 token 含在中间。
    //  原先靠 `target.contains(app)` 命中，现在连正向包含都不留，所以即便哪天
    //  有人把首词提取加回来，这几个跨厂商场景也不会再出事。
    // ---------------------------------------------------------------------

    #[test]
    fn partial_word_targets_are_never_residuals() {
        // "GitHub".contains("Git") —— 卸载 Git 会 remove_dir_all 掉 GitHub
        // Desktop 的用户数据；TeamViewer / Slack Technologies 同理。
        assert!(!is_residual_dir("GitHub", "Git"));
        assert!(!is_residual_dir("TeamViewer", "Team"));
        assert!(!is_residual_dir("SlackTechnologies", "Slack"));
        assert!(!is_residual_dir("PostmanRuntime", "Postman"));
        assert!(!is_residual_reg_key("TeamViewer", "Team"));
    }

    #[test]
    fn javasoft_is_unreachable_even_without_the_protection_list() {
        // 保护名单之外单独验证结构层：这条路走的不是名单，而是「折叠后不相等
        // 就不算命中」。名单可以被人改，规则不会。
        // app 名直接用 "Java" 是最坏情况 —— 旧实现下 "java" ⊂ "javasoft"
        // 正向命中，reg delete /f 会注销机器上全部 JRE。
        assert!(!matches_variant("JavaSoft", "Java"));
        assert!(!matches_variant("JavaSoft", "Java 8 Update 291"));
        // 拼接式目标（同应用、只是没有分隔符）仍然要能匹配，
        // 否则就是拿召回换安全、功能被整个废掉
        assert!(matches_variant("DockerDesktop", "Docker Desktop"));
        assert!(matches_variant("7-Zip", "7-Zip 22.01"));
    }

    #[test]
    fn genuine_recall_survives_the_stricter_rule() {
        // 折叠相等覆盖的常见形态：去掉版本号、去掉连字符/空格
        assert!(is_residual_dir("DockerDesktop", "Docker Desktop"));
        assert!(is_residual_dir("docker-desktop", "Docker Desktop"));
        assert!(is_residual_reg_key("GoogleChrome", "Google Chrome"));
    }

    #[test]
    fn install_dir_version_token_only_accepts_real_versions() {
        assert_eq!(
            install_dir_version_token(r"C:\Program Files\Java\jre1.8.0_291"),
            Some("1.8.0_291".to_string())
        );
        assert_eq!(
            install_dir_version_token("C:/Program Files/Java/jdk-17.0.1"),
            Some("17.0.1".to_string())
        );
        assert_eq!(
            install_dir_version_token(r"D:\apps\nodejs22.3.0"),
            Some("22.3.0".to_string())
        );
        // 没有版本后缀 → 无从下手，宁可漏
        assert_eq!(install_dir_version_token(r"C:\Program Files\Git"), None);
        assert_eq!(
            install_dir_version_token(r"C:\Program Files\Mozilla Firefox"),
            None
        );
        // "Python311" 无分隔符，是目录名不是版本号
        assert_eq!(
            install_dir_version_token(r"C:\Program Files\Python311"),
            None
        );
        // 单字母前缀噪声
        assert_eq!(install_dir_version_token(r"C:\opt\v2.1.0"), None);
        assert_eq!(install_dir_version_token(""), None);
    }

    #[test]
    fn version_scoped_subkey_requires_a_single_unambiguous_hit() {
        let full = "HKLM\\SOFTWARE\\JavaSoft\\Java Runtime Environment\\1.8.0_291".to_string();
        let other = "HKLM\\SOFTWARE\\JavaSoft\\Java Runtime Environment\\21.0.1".to_string();

        // 恰好一个 → 命中，且只能删那一个版本节点
        assert_eq!(
            version_scoped_subkey(&[full.clone(), other.clone()], "1.8.0_291"),
            Some("HKLM/SOFTWARE/JavaSoft/Java Runtime Environment/1.8.0_291".to_string())
        );
        // 0 个命中 → 漏报好过猜
        assert_eq!(version_scoped_subkey(&[other.clone()], "1.8.0_291"), None);
        // 版本号为空直接进入拒绝分支
        assert_eq!(version_scoped_subkey(&[full], ""), None);
    }

    #[test]
    fn version_scoped_subkey_refuses_ambiguous_hits() {
        // 同一版本号在多个 hive 下都出现，归属判断不了，比不删更危险
        let hklm = "HKLM\\SOFTWARE\\JavaSoft\\Java Runtime Environment\\1.8.0_291".to_string();
        let hkcu = "HKCU\\SOFTWARE\\JavaSoft\\Java Runtime Environment\\1.8.0_291".to_string();
        assert_eq!(version_scoped_subkey(&[hklm, hkcu], "1.8.0_291"), None);
        // 同一条被重复枚举不算歧义
        let a = "HKLM\\SOFTWARE\\JavaSoft\\Java Runtime Environment\\1.8.0_291".to_string();
        assert!(version_scoped_subkey(&[a.clone(), a], "1.8.0_291").is_some());
    }

    #[test]
    fn version_scoped_subkey_refuses_paths_that_are_too_shallow() {
        // 就算调用方算出了这种路径，结构性兜底也要拦下来
        let container_only = vec!["HKLM\\SOFTWARE\\JavaSoft\\1.8.0_291".to_string()];
        assert_eq!(version_scoped_subkey(&container_only, "1.8.0_291"), None);

        let no_hive = vec!["SOFTWARE\\JavaSoft\\Java Runtime Environment\\1.8.0_291".to_string()];
        assert_eq!(version_scoped_subkey(&no_hive, "1.8.0_291"), None);
    }

    #[test]
    fn registry_delete_targets_must_be_deep_and_versioned() {
        // 厂商容器 / 产品线容器：层级不够
        assert!(!is_deletable_reg_path("HKLM\\SOFTWARE\\JavaSoft"));
        assert!(!is_deletable_reg_path("HKCU\\SOFTWARE\\Microsoft"));
        assert!(!is_deletable_reg_path(
            "HKLM\\SOFTWARE\\JavaSoft\\Java Runtime Environment"
        ));
        // Wow6432Node 是包装层，不能当深度、更不能当末段
        assert!(!is_deletable_reg_path("HKLM\\SOFTWARE\\Wow6432Node"));
        assert!(!is_deletable_reg_path(
            "HKLM\\SOFTWARE\\Wow6432Node\\JavaSoft\\Java Runtime Environment"
        ));
        // 末段是纯单词容器（"Prefs" 挂着所有 Java 版本的偏好设置）
        assert!(!is_deletable_reg_path("HKLM\\SOFTWARE\\JavaSoft\\Prefs"));
        // 缺 hive → 不是完整键路径
        assert!(!is_deletable_reg_path(
            "SOFTWARE\\JavaSoft\\Java Runtime Environment\\1.8.0_291"
        ));

        // 唯一放行的形状：软件⁄产品线⁄版本 三层到位，末段带版本号
        assert!(is_deletable_reg_path(
            "HKLM\\SOFTWARE\\JavaSoft\\Java Runtime Environment\\1.8.0_291"
        ));
        assert!(is_deletable_reg_path(
            "HKLM\\SOFTWARE\\Wow6432Node\\JavaSoft\\Java Runtime Environment\\1.8.0_291"
        ));
    }

    // ---------------------------------------------------------------------
    //  调用点检查：上面全是纯函数用例，没人调用就是死代码。这正是上一轮在同
    //  类问题上栽过的跟头 —— 回退修复后纯函数用例照样全绿。
    //  windows_apps.rs 在本机编译不到（cfg(windows)），用 include_str! 做源码
    //  级验证，至少保证危险入口接的确实是这套严格匹配。
    // ---------------------------------------------------------------------

    #[test]
    fn protective_heuristic_stays_deliberately_looser_than_deletion() {
        // 方向相反 ⇒ 不能共用规则。这里的判定导向"不允许卸载"，
        // 漏判 = 少一层保护 = fail-open，所以比 is_residual_dir 宽：
        assert!(resembles_app_dir("Zoom", "Zoom Workplace"));
        assert!(resembles_app_dir("Google", "Go"));
        assert!(resembles_app_dir("TeamViewer", "Team"));
        // 宽松归宽松，短名单字仍要挡：否则 1~2 个字符的应用名会把 AppData
        // 下一大批目录都圈成"自己有数据"，保护就退化成禁止卸载一切。
        assert!(!resembles_app_dir("Discord", "D"));
        assert!(!resembles_app_dir("Discord", ""));
        assert!(!resembles_app_dir("", "Discord"));
        assert!(resembles_app_dir("Discord", "Discord"));
    }

    #[test]
    fn protective_entrypoint_does_not_reuse_the_strict_matcher() {
        // is_residual_dir 导向删除， resembles_app_dir 导向保护。两者若被合并，
        // 只要有人为了召回放宽规则，风险方向就会一侧爆掉。用源码钉住分工。
        let src = include_str!("windows_apps.rs");
        let guard = src[src
            .find("fn has_large_user_data")
            .expect("has_large_user_data 不见了")..]
            .split("\nfn ")
            .next()
            .unwrap();
        assert!(
            guard.contains("resembles_app_dir("),
            "保护性启发式改回了删除侧规则，会 fail-open"
        );
        assert!(
            !guard.contains("is_residual_dir("),
            "保护性启发式仍在复用删除侧的严格匹配"
        );
    }

    // -----------------------------------------------------------------
    //  #34 · 删除出口自带闸门
    //
    //  原先两个出口（remove_dir_all / reg delete /f）都是裸执行，保护全活
    //  在扫描函数里。这里验证闸门本身，以及闸门真的接到了出口上。
    // -----------------------------------------------------------------

    #[test]
    fn tier_one_gate_accepts_the_keys_the_scanner_actually_produces() {
        // 关键：当前 scan_registry_residual 产出的合法目标就是 SOFTWARE 的
        // **一级**子键。若把版本化下钻的深度要求（>=3）照搬到这一档，
        // 整个注册表残留清理会静默失效 —— 这条专门钉住这个回归。
        assert!(is_deletable_reg_key_basic("HKCU\\SOFTWARE\\Zoom"));
        assert!(is_deletable_reg_key_basic("HKCU\\SOFTWARE\\Discord"));
        assert!(is_deletable_reg_key_basic(
            "HKLM\\SOFTWARE\\Wow6432Node\\7-Zip"
        ));
        // 而严格档位对同一条是拒绝的（它只服务版本化下钻）
        assert!(!is_deletable_reg_path("HKCU\\SOFTWARE\\Zoom"));
    }

    #[test]
    fn tier_one_gate_still_blocks_broken_or_shared_keys() {
        // 共享厂商容器
        assert!(!is_deletable_reg_key_basic("HKCU\\SOFTWARE\\Microsoft"));
        assert!(!is_deletable_reg_key_basic("HKLM\\SOFTWARE\\JavaSoft"));
        assert!(!is_deletable_reg_key_basic(
            "HKLM\\SOFTWARE\\Wow6432Node\\Oracle"
        ));
        // 没有 hive / 找不到 SOFTWARE
        assert!(!is_deletable_reg_key_basic("SOFTWARE\\Zoom"));
        assert!(!is_deletable_reg_key_basic("Zoom"));
        assert!(!is_deletable_reg_key_basic(""));
        // 末段为空（形如 HKCU\\SOFTWARE\\）
        assert!(!is_deletable_reg_key_basic("HKCU\\SOFTWARE\\"));
    }

    #[test]
    fn filesystem_gate_blocks_roots_and_shared_appdata_containers() {
        assert!(is_deletable_fs_path(r"C:\Users\me\AppData\Roaming\Zoom"));
        assert!(is_deletable_fs_path(r"C:\Users\me\AppData\Local\Discord"));
        // AppData\<Roaming|Local>\<共享厂商容器> 这一层
        assert!(!is_deletable_fs_path(
            r"C:\Users\me\AppData\Roaming\Microsoft"
        ));
        assert!(!is_deletable_fs_path(r"C:\Users\me\AppData\Local\Google"));
        assert!(!is_deletable_fs_path(
            r"C:\Users\me\AppData\LocalLow\Oracle"
        ));
        // 盘符根、单段、空路径
        assert!(!is_deletable_fs_path(r"C:\"));
        assert!(!is_deletable_fs_path("/"));
        assert!(!is_deletable_fs_path(""));
        assert!(!is_deletable_fs_path("   "));
    }

    #[test]
    fn both_deletion_exits_have_their_own_gate() {
        // 纯函数的闸门不等于生效的闸门 —— 必须钉住出口真的调了它
        let src = include_str!("windows_apps.rs");

        let fs = src[src
            .find("pub fn delete_filesystem_residual")
            .expect("delete_filesystem_residual 不见了")..]
            .split("\npub fn ")
            .next()
            .unwrap();
        assert!(
            fs.contains("is_deletable_fs_path("),
            "remove_dir_all 出口没有闸门，扫描侧的判定不能代表删除时刻的状态"
        );

        let reg = src[src
            .find("pub fn delete_registry_residual")
            .expect("delete_registry_residual 不见了")..]
            .split("\npub fn ")
            .next()
            .unwrap();
        assert!(
            reg.contains("is_deletable_reg_key_basic("),
            "reg delete /f 出口没有闸门"
        );
        // 同时确认没有误用严格档位（会把一级子键全挡掉）
        assert!(
            !reg.contains("is_deletable_reg_path("),
            "删除出口用了版本化下钻的严格档位，会让一级子键全部删不掉"
        );
    }

    // -----------------------------------------------------------------
    //  #36 · 扩大 windows_apps.rs（4227 行、整体 cfg(windows)）的覆盖
    //
    //  这个函数式测不到的模块里还有一串不可逆操作：环境变量清理、卸载执行、
    //  批量残留清理。下面用源码级断言把它们各自的安全不变量钉住。
    // -----------------------------------------------------------------

    fn fn_body<'a>(src: &'a str, needle: &str) -> &'a str {
        src[src
            .find(needle)
            .unwrap_or_else(|| panic!("{} 不见了", needle))..]
            .split("\npub fn ")
            .next()
            .unwrap_or("")
    }

    #[test]
    fn clean_all_residuals_routes_through_the_gated_exits() {
        // 批量清理是 UI 上"一键清干净"的入口。它一旦绕过 delete_* 出口
        // 自己 remove_dir_all / reg delete，前面所有闸门就全废了。
        let src = include_str!("windows_apps.rs");
        let body = fn_body(src, "pub fn clean_all_residuals");
        assert!(
            body.contains("delete_registry_residual(")
                && body.contains("delete_filesystem_residual("),
            "批量清理没有走带闸门的删除出口"
        );
        assert!(
            !body.contains("remove_dir_all(") && !body.contains("\"delete\""),
            "批量清理里出现了裸删除调用，绕过了闸门"
        );
    }

    #[test]
    fn env_var_cleanup_respects_the_deletable_flag() {
        // 环境变量里 PATH 是重灾区：写错了整个系统的命令查找都会坏。
        // 扫描侧判定为不可删（如系统级变量需管理员权限）时必须直接退出。
        let src = include_str!("windows_apps.rs");
        let body = fn_body(src, "pub fn clean_env_var_residual");
        assert!(
            body.contains("!residual.deletable"),
            "环境变量清理没有检查 deletable，会去改不该改的变量"
        );
    }

    #[test]
    fn uninstall_command_is_not_shelled_out_through_cmd() {
        // UninstallString 由安装方自写，属于不可信输入。若用 `cmd /C <整串>`
        // 拼接执行，字符串里夹 `&` `|` 就能挂载任意命令。
        // 正确做法是解析出 exe + 参数，用 Command::new(exe).args(...)。
        let src = include_str!("windows_apps.rs");
        let body = fn_body(src, "fn execute_uninstall_command");
        assert!(
            body.contains("Command::new(&exe_path)"),
            "卸载命令没有走 可执行程序 + 参数数组 的形式"
        );
        assert!(
            !body.contains("\"/C\""),
            "卸载命令又被拼进 cmd /C 了，存在命令注入面"
        );
    }

    #[test]
    fn registry_residual_scan_marks_system_keys_as_not_deletable() {
        // 扫描侧对 HKLM 键必须标 deletable=false（要管理员权限），
        // 否则 UI 会把它当成可勾选项交给 reg delete。
        let src = include_str!("windows_apps.rs");
        let body = fn_body(src, "pub fn scan_registry_residual");
        assert!(body.contains("is_system"), "扫描注册表残留没有区分系统级键");
    }

    #[test]
    fn deletion_entrypoints_actually_use_the_strict_matcher() {
        let src = include_str!("windows_apps.rs");

        let fs_fn = src[src
            .find("pub fn scan_filesystem_residual")
            .expect("scan_filesystem_residual 不见了")..]
            .split("\npub fn ")
            .next()
            .unwrap();
        assert!(
            fs_fn.contains("is_residual_dir("),
            "scan_filesystem_residual 没走严格匹配，remove_dir_all 失去保护"
        );

        let reg_fn = src[src
            .find("pub fn scan_registry_residual")
            .expect("scan_registry_residual 不见了")..]
            .split("\npub fn ")
            .next()
            .unwrap();
        assert!(
            reg_fn.contains("is_residual_reg_key("),
            "scan_registry_residual 没走严格匹配，reg delete /f 失去保护"
        );
    }

    #[test]
    fn no_bidirectional_substring_match_remains() {
        // 反向包含曾让短键名被长应用名吞掉。一旦有人图方便加回来，
        // 前面所有用例都会被绕过，所以在这显式钉住。
        let src = include_str!("windows_apps.rs");
        assert!(
            !src.contains("sn.contains(&key_lower)"),
            "注册表反向子串匹配回来了"
        );
        assert!(
            !src.contains("search_name.contains(&dir_name)"),
            "目录反向子串匹配回来了"
        );
        assert!(
            !src.contains("fn generate_search_names"),
            "带首词提取的 generate_search_names 回来了"
        );
    }
}
