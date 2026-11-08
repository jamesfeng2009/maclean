//! 卸载残留的名称匹配规则
//!
//! 这个模块刻意**不做** `cfg(target_os)` 限定，尽管它目前只服务于 Windows 侧。
//!
//! 原因很实在：`scanner::windows_apps` 整体是 `#[cfg(target_os = "windows")]`，
//! 代码写在里面的话，开发机（macOS）上既编译不到也测不到 —— 而这里每一个
//! true/false 最终都落到 `remove_dir_all` 或 `reg delete /f`
//! （`/f` 跳过确认且递归删除整棵子树）。写完没人验证，等同于没保护。
//!
//! 代价：macOS 上只有 win_protection 用到的少数几个函数有生产调用点，
//! 其余纯逻辑只在 `#[cfg(test)]` 里被消费，故仅对非 Windows 目标放宽
//! dead_code。**Windows 目标上刻意不放宽**：那里每个纯函数都必须被
//! windows_apps.rs 接上，clippy 的 dead_code 就是接线检查器，不许用 allow 掩盖。

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

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
//  D-02 · 该下钻已由 `windows_apps.rs::scan_registry_residual` 接线：
//  reg query /s 枚举 + InstallLocation 版本推导 + version_scoped_subkey，
//  不再是「只有定义和单测」的死代码。

/// 参与子串匹配所需的最短应用名长度
///
/// 低于这个长度的名字（"Go" / "R" / "V"）做子串匹配会命中大量无关目标：
/// `"google".contains("go") == true`，于是卸载 Go 会连 Chrome 用户数据一起
/// `remove_dir_all` 删掉。
pub const MIN_MATCH_LEN: usize = 4;

/// 归一化用于比对的名称：转小写，只保留字母数字
pub fn normalize_residual_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// AppData 下由多个产品共享的顶级容器目录，永不作为残留删除
///
/// 这些目录一旦被删，损失的不只是被卸载的那个应用 —— 比如 `Tencent`
/// 同时装着微信和 QQ，`Google` 装着 Chrome 全部 profile（含保存的密码）。
pub const PROTECTED_APPDATA_ROOTS: &[&str] = &[
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
pub const PROTECTED_REG_ROOTS: &[&str] = &[
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
pub fn search_name_variants(app_name: &str) -> Vec<String> {
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
pub fn display_name_variants(app_name: &str) -> Vec<String> {
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
pub fn is_residual_dir(dir_name: &str, app_name: &str) -> bool {
    let dir_key = normalize_residual_name(dir_name);
    if dir_key.is_empty() || PROTECTED_APPDATA_ROOTS.contains(&dir_key.as_str()) {
        return false;
    }

    matches_variant(dir_name, app_name)
}

/// 注册表顶级键是否可以作为该应用的残留被删除
///
/// 与 `is_residual_dir` 同构，区别只是换成注册表保护名单。
pub fn is_residual_reg_key(key_name: &str, app_name: &str) -> bool {
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
pub const MIN_REG_KEY_DEPTH: usize = 3;

/// 从安装目录叶名里提取版本串
///
/// `...\Java\jre1.8.0_291` → `Some("1.8.0_291")`，`...\Git` → `None`。
///
/// 两个过滤条件是精度的来源：
/// - 版本段必须含 `.` 或 `_`，否则 `Python311` 这种纯单词会被当成版本；
/// - 版本段之前至少两个字母，排除 `v2.1` 之类的噪声前缀。
pub fn install_dir_version_token(location: &str) -> Option<String> {
    let leaf = location
        .trim()
        .trim_matches('"')
        .split(['/', '\\'])
        .map(|s| s.trim())
        .rfind(|s| !s.is_empty())?;

    let digit_at = leaf.char_indices().find(|(_, c)| c.is_ascii_digit())?.0;
    if digit_at < 2 {
        return None;
    }

    let prefix = &leaf[..digit_at];
    let alpha_count = prefix.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if alpha_count < 2 {
        return None;
    }

    let version = leaf[digit_at..].trim_end_matches(['/', '\\']);
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
pub fn is_deletable_reg_key_basic(full_key: &str) -> bool {
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
pub fn is_deletable_reg_path(full_key: &str) -> bool {
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
pub fn is_deletable_fs_path(path: &str) -> bool {
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
pub fn version_scoped_subkey(subkeys: &[String], version: &str) -> Option<String> {
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
pub fn resembles_app_dir(dir_name: &str, app_name: &str) -> bool {
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

// =========================================================================
//  D-01 / F-01 / F-02 / F-03 / A-04 / A-11 / D-14 / D-15 / F-04
//  windows_apps.rs 的纯逻辑（刻意无 cfg，开发机可编译可测）
//
//  windows_apps 整体 `#[cfg(windows)]`，写在里面的东西本机 0 编译 0 测试。
//  这里把「能脱离 Windows API 独立判定」的部分全部抽出来：数据结构、命令
//  输出解析、名称推导、派发与计数。windows_apps.rs 只负责采集与真实删除，
//  判定与编排一律调这里，开发机上的行为测试因此能真跑生产函数。
// =========================================================================

/// D-01 · 注册表残留项
#[derive(Debug, Clone)]
pub struct RegistryResidual {
    pub key_path: String,
    pub is_system: bool,
    pub deletable: bool,
    pub reason: String,
}

/// D-01 · 环境变量残留项
#[derive(Debug, Clone)]
pub struct EnvVarResidual {
    pub var_name: String,
    pub current_value: String,
    pub entries_to_remove: Vec<String>,
    pub is_system: bool,
    pub deletable: bool,
    pub reason: String,
}

/// D-01 · 文件系统残留项
#[derive(Debug, Clone)]
pub struct FilesystemResidual {
    pub path: String,
    pub size: u64,
    pub deletable: bool,
    pub reason: String,
}

/// D-01 · 干净卸载残留扫描结果
#[derive(Debug, Clone)]
pub struct UninstallResidual {
    pub registry: Vec<RegistryResidual>,
    pub env_vars: Vec<EnvVarResidual>,
    pub filesystem: Vec<FilesystemResidual>,
}

impl UninstallResidual {
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty() && self.env_vars.is_empty() && self.filesystem.is_empty()
    }
    pub fn total_count(&self) -> usize {
        self.registry.len() + self.env_vars.len() + self.filesystem.len()
    }
    pub fn deletable_count(&self) -> usize {
        self.registry.iter().filter(|r| r.deletable).count()
            + self.env_vars.iter().filter(|e| e.deletable).count()
            + self.filesystem.iter().filter(|f| f.deletable).count()
    }
}

/// D-01 · 一次清理动作（注入执行器时按此派发）
#[derive(Debug, Clone)]
pub enum ResidualAction {
    Registry(String),
    EnvVar(EnvVarResidual),
    Filesystem(String),
}

/// D-01 · 「清理选中残留」的派发与计数核心（可注入执行器）。
///
/// `exec` 返回 true=成功 / false=失败；不可删项直接计入跳过、绝不派发。
/// 生产侧 `windows_apps::clean_selected_residuals` 把三个真实删除函数注入
/// 这里；测试侧注入记录动作的假执行器，从而在开发机上真跑整条派发链 ——
/// 不是 include_str! 源码文本断言。把本函数改成「不调用 exec」，
/// 下面 `d01_*` 行为用例必须变红。
pub fn clean_selected_residuals_with<F>(
    residuals: &[UninstallResidual],
    mut exec: F,
) -> (usize, usize, usize)
where
    F: FnMut(ResidualAction) -> bool,
{
    let (mut success, mut failed, mut skipped) = (0usize, 0usize, 0usize);
    for r in residuals {
        for reg in &r.registry {
            if !reg.deletable {
                skipped += 1;
                continue;
            }
            if exec(ResidualAction::Registry(reg.key_path.clone())) {
                success += 1;
            } else {
                failed += 1;
            }
        }
        for env in &r.env_vars {
            if !env.deletable {
                skipped += 1;
                continue;
            }
            if exec(ResidualAction::EnvVar(env.clone())) {
                success += 1;
            } else {
                failed += 1;
            }
        }
        for fs in &r.filesystem {
            if !fs.deletable {
                skipped += 1;
                continue;
            }
            if exec(ResidualAction::Filesystem(fs.path.clone())) {
                success += 1;
            } else {
                failed += 1;
            }
        }
    }
    (success, failed, skipped)
}

/// F-02 · 解析 reg query 输出行：**精确比较第一个 token** 的字段名。
///
/// 旧实现 `line.to_lowercase().contains(field.to_lowercase())` 会让
/// `"quietuninstallstring".contains("uninstallstring")` 恒真，
/// QuietUninstallString 行被 UninstallString 分支吞掉。DWORD 的 `0x…`
/// 统一转十进制，否则 `parse::<u64>()` 失败。
pub fn parse_reg_line(line: &str, field: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let sep = "    ";
    let (name, type_token, value): (String, String, String) = if trimmed.contains(sep) {
        let parts: Vec<&str> = trimmed.splitn(3, sep).collect();
        if parts.len() < 3 {
            return None;
        }
        (
            parts[0].trim().to_string(),
            parts[1].trim().to_string(),
            parts[2].trim().to_string(),
        )
    } else {
        let toks: Vec<&str> = trimmed.split_whitespace().collect();
        let type_idx = toks.iter().position(|p| p.starts_with("REG_"))?;
        if type_idx == 0 || type_idx + 1 > toks.len() {
            return None;
        }
        (
            toks[..type_idx].join(" "),
            toks[type_idx].to_string(),
            toks[type_idx + 1..].join(" "),
        )
    };
    if !name.eq_ignore_ascii_case(field) || value.is_empty() {
        return None;
    }
    if type_token.to_ascii_uppercase().contains("DWORD")
        && value.to_ascii_lowercase().starts_with("0x")
    {
        if let Ok(n) = u64::from_str_radix(&value[2..], 16) {
            return Some(n.to_string());
        }
    }
    Some(value)
}

/// F-03 · 解码外部命令输出：UTF-8 优先；非 UTF-8 时按 Latin-1 逐字节保留，
/// 不再用 `from_utf8_lossy` 把中文 OEM 字节全压成 U+FFFD。
pub fn decode_console_output(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// A-11 · bloatware 第 4 级模糊匹配：仅用于 UWP 包全名，且 app_id 须达到
/// MIN_MATCH_LEN。Win32 键名（`is_uwp=false`）一律不走模糊档。
pub fn bloatware_fuzzy_matches(app_id: &str, package_full_name: &str, is_uwp: bool) -> bool {
    if !is_uwp {
        return false;
    }
    let id = app_id.trim().to_lowercase();
    if id.len() < MIN_MATCH_LEN {
        return false;
    }
    package_full_name.to_lowercase().contains(&id)
}

/// F-04 · 按 app_id 去重，返回应保留的索引（忽略大小写，保留首次出现）。
pub fn dedup_indices_by_app_id(ids: &[String]) -> Vec<usize> {
    let mut seen = std::collections::HashSet::new();
    ids.iter()
        .enumerate()
        .filter(|(_, id)| seen.insert(id.to_ascii_lowercase()))
        .map(|(i, _)| i)
        .collect()
}

/// D-14 · 从卸载路径推导应用/包名。ops 侧对 UWP 传进来的 app_name 恒为空，
/// clean_uninstall 必须能自己从 `uwp:<PackageFullName>` 推出可用的名字。
pub fn derive_app_name_from_path(path: &str) -> Option<String> {
    let (is_uwp, rest) = if let Some(r) = path.strip_prefix("uwp:") {
        (true, r)
    } else if let Some(r) = path.strip_prefix("uninstall:") {
        (false, r)
    } else {
        let r = path.strip_prefix("dir:")?;
        (false, r)
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    if is_uwp {
        let name = rest.split('_').next().unwrap_or(rest);
        if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        }
    } else {
        Some(rest.to_string())
    }
}

/// D-15 · 从**当前** PATH 值里移除指定条目（大小写不敏感）。
/// 清理时必须重新 `reg query` 取当前值再调这里，不能用扫描时刻的
/// current_value 整条覆盖 —— 否则会抹掉用户确认期间其它程序新增的条目。
pub fn cleaned_path_value(current: &str, to_remove: &[String]) -> String {
    let rm: Vec<String> = to_remove.iter().map(|e| e.trim().to_lowercase()).collect();
    current
        .split(';')
        .filter(|e| !rm.contains(&e.trim().to_lowercase()))
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-01 · include_str! 源码断言必须先按 `\nmod tests` 切出生产段：
    /// 否则函数体会一直截到文件末尾（含测试模块），测试里出现同名
    /// 字符串即假绿/假红。
    fn production_src() -> &'static str {
        include_str!("windows_apps.rs")
            .split("\nmod tests")
            .next()
            .unwrap_or(include_str!("windows_apps.rs"))
    }

    fn win_protection_production_src() -> &'static str {
        include_str!("win_protection.rs")
            .split("\nmod tests")
            .next()
            .unwrap_or(include_str!("win_protection.rs"))
    }

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
        assert_eq!(
            version_scoped_subkey(std::slice::from_ref(&other), "1.8.0_291"),
            None
        );
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
        // A-05：has_large_user_data 现在把真实目录枚举后交给生产节点
        // user_data_dir_exceeds_threshold，后者才调 resembles_app_dir。
        let src = win_protection_production_src();
        let guard = fn_body(src, "fn user_data_dir_exceeds_threshold");
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
        let src = production_src();

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
        let src = production_src();
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
        let src = production_src();
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
        let src = production_src();
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
        let src = production_src();
        let body = fn_body(src, "pub fn scan_registry_residual");
        assert!(body.contains("is_system"), "扫描注册表残留没有区分系统级键");
    }

    #[test]
    fn deletion_entrypoints_actually_use_the_strict_matcher() {
        let src = production_src();

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
    fn h01_uninstall_exit_uses_the_production_protection_gate() {
        // H-01 · 生产出口必须调 allows_uninstall()/block_reason()，不许自己
        // match 重写一份。把调用改回硬编码的中文文案 match，下面断言必红。
        let src = production_src();
        let body = fn_body(src, "impl Scanner for WindowsUninstallScanner");
        assert!(
            body.contains("allows_uninstall()"),
            "卸载扫描出口没有调生产判定 allows_uninstall()"
        );
        assert!(
            body.contains("block_reason()"),
            "卸载扫描出口没有调生产判定 block_reason()"
        );
        assert!(
            !body.contains("\"系统关键应用，禁止卸载\""),
            "卸载扫描出口又在自己重写保护判定文案（假绿）"
        );
    }

    #[test]
    fn no_bidirectional_substring_match_remains() {
        // 反向包含曾让短键名被长应用名吞掉。一旦有人图方便加回来，
        // 前面所有用例都会被绕过，所以在这显式钉住。
        let src = production_src();
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

    // =================================================================
    //  D-01 · 行为测试：钉住「清理选中残留」入口真的调用了清理逻辑
    //
    //  不用 include_str! 源码文本断言。生产入口 windows_apps.rs::
    //  clean_selected_residuals 只是把三个真实删除函数注入
    //  clean_selected_residuals_with；派发与计数全在这个无 cfg 函数里，
    //  因此开发机能真跑。把 core 改成「不调用 exec」或「恒跳过」，
    //  下面两条用例必红。
    // =================================================================

    fn sample_residual() -> UninstallResidual {
        UninstallResidual {
            registry: vec![
                RegistryResidual {
                    key_path: r"HKCU\SOFTWARE\Zoom".to_string(),
                    is_system: false,
                    deletable: true,
                    reason: String::new(),
                },
                RegistryResidual {
                    key_path: r"HKLM\SOFTWARE\Microsoft".to_string(),
                    is_system: true,
                    deletable: false,
                    reason: "需要管理员权限".to_string(),
                },
            ],
            env_vars: vec![EnvVarResidual {
                var_name: "Path".to_string(),
                current_value: r"C:\Windows;C:\Zoom".to_string(),
                entries_to_remove: vec![r"C:\Zoom".to_string()],
                is_system: false,
                deletable: true,
                reason: String::new(),
            }],
            filesystem: vec![
                FilesystemResidual {
                    path: r"C:\Users\me\AppData\Roaming\Zoom".to_string(),
                    size: 1024,
                    deletable: true,
                    reason: String::new(),
                },
                FilesystemResidual {
                    path: r"C:\Users\me\AppData\Local\Microsoft".to_string(),
                    size: 4096,
                    deletable: false,
                    reason: "共享容器".to_string(),
                },
            ],
        }
    }

    #[test]
    fn d01_clean_selected_residuals_dispatches_every_deletable_action() {
        let r = sample_residual();
        let mut seen: Vec<ResidualAction> = Vec::new();
        let counts = clean_selected_residuals_with(std::slice::from_ref(&r), |a| {
            seen.push(a);
            true
        });
        // 3 个可删项全部成功，2 个不可删项被跳过
        assert_eq!(counts, (3, 0, 2));
        assert_eq!(seen.len(), 3, "每个可删残留都必须真正派发一次清理动作");
        assert!(seen
            .iter()
            .any(|a| matches!(a, ResidualAction::Registry(_))));
        assert!(seen.iter().any(|a| matches!(a, ResidualAction::EnvVar(_))));
        assert!(seen
            .iter()
            .any(|a| matches!(a, ResidualAction::Filesystem(_))));
    }

    #[test]
    fn d01_clean_selected_residuals_counts_failures_and_skips() {
        let r = sample_residual();
        let mut calls = 0;
        // 执行器恒失败：可删项全部计入失败，不可删项仍只计跳过、绝不派发
        let counts = clean_selected_residuals_with(std::slice::from_ref(&r), |_| {
            calls += 1;
            false
        });
        assert_eq!(counts, (0, 3, 2));
        assert_eq!(calls, 3, "不可删项不能被派发给删除函数");
        // 空清单：三个计数全 0
        assert_eq!(clean_selected_residuals_with(&[], |_| true), (0, 0, 0));
    }

    // --- F-02 · parse_reg_line ---

    #[test]
    fn f02_parse_reg_line_compares_the_first_token_exactly() {
        let quiet = "    QuietUninstallString    REG_SZ    msiexec /x {ABC} /quiet";
        assert_eq!(
            parse_reg_line(quiet, "QuietUninstallString").as_deref(),
            Some("msiexec /x {ABC} /quiet")
        );
        // 旧实现用 contains，这行会被当成 UninstallString 吞掉 —— 改回去必红
        assert_eq!(parse_reg_line(quiet, "UninstallString"), None);
        assert_eq!(
            parse_reg_line("    DisplayName    REG_SZ    WeChat", "DisplayName").as_deref(),
            Some("WeChat")
        );
        assert_eq!(
            parse_reg_line("    DisplayName    REG_SZ    ", "DisplayName"),
            None
        );
    }

    #[test]
    fn f02_parse_reg_line_converts_dword_hex_to_decimal() {
        assert_eq!(
            parse_reg_line("    EstimatedSize    REG_DWORD    0x1234", "EstimatedSize").as_deref(),
            Some("4660")
        );
        // 回退分支（split_whitespace）也要转
        assert_eq!(
            parse_reg_line("SystemComponent REG_DWORD 0x1", "SystemComponent").as_deref(),
            Some("1")
        );
    }

    // --- F-03 · decode_console_output ---

    #[test]
    fn f03_decode_console_output_preserves_utf8_and_does_not_collapse_oem_bytes() {
        assert_eq!(decode_console_output("元宝 😀".as_bytes()), "元宝 😀");
        let gbk_yuanbao = decode_console_output(&[0xD4, 0xAA, 0xB1, 0xA6]);
        let gbk_other = decode_console_output(&[0xD5, 0xC5, 0xC8, 0xFD]);
        assert!(!gbk_yuanbao.contains('\u{FFFD}'));
        assert_ne!(
            gbk_yuanbao, gbk_other,
            "不同 OEM 字节不得被压成同一个串（from_utf8_lossy 会全部变成 U+FFFD）"
        );
    }

    // --- A-11 · bloatware 模糊档 ---

    #[test]
    fn a11_fuzzy_bloatware_match_only_applies_to_uwp_full_name() {
        assert!(bloatware_fuzzy_matches(
            "Clipchamp.Clipchamp",
            "Clipchamp.Clipchamp_2.0_x64__8wekyb3d8bbwe",
            true
        ));
        // Win32 注册表键名永不走模糊档，否则短 app_id 会把普通应用误配成预装应用
        assert!(!bloatware_fuzzy_matches(
            "Clipchamp.Clipchamp",
            "SomeWin32Key",
            false
        ));
        // 短于 MIN_MATCH_LEN 的 app_id 直接拒绝
        assert!(!bloatware_fuzzy_matches("AB", "XXAB_1.0", true));
        assert!(!bloatware_fuzzy_matches("", "Anything_1.0", true));
    }

    // --- F-04 · app_id 去重 ---

    #[test]
    fn f04_dedup_indices_by_app_id_keeps_first_occurrence_case_insensitively() {
        let ids: Vec<String> = ["A", "B", "a", "C", "B"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(dedup_indices_by_app_id(&ids), vec![0, 1, 3]);
        assert_eq!(dedup_indices_by_app_id(&[]), Vec::<usize>::new());
    }

    // --- D-14 · 路径推导 app_name ---

    #[test]
    fn d14_derive_app_name_from_uninstall_path() {
        assert_eq!(
            derive_app_name_from_path("uwp:Microsoft.BingWeather_1.0.0_x64__8weky"),
            Some("Microsoft.BingWeather".to_string())
        );
        assert_eq!(
            derive_app_name_from_path("uninstall:7-Zip"),
            Some("7-Zip".to_string())
        );
        assert_eq!(
            derive_app_name_from_path("dir:MyApp"),
            Some("MyApp".to_string())
        );
        assert_eq!(derive_app_name_from_path("uwp:"), None);
        assert_eq!(derive_app_name_from_path(""), None);
        assert_eq!(derive_app_name_from_path("bogus"), None);
    }

    // --- D-15 · 用当前 PATH 值清理 ---

    #[test]
    fn d15_cleaned_path_value_keeps_entries_added_after_scan() {
        let current = r"C:\Windows;C:\Python39;C:\NewByOtherProgram";
        assert_eq!(
            cleaned_path_value(current, &[r"C:\Python39".to_string()]),
            r"C:\Windows;C:\NewByOtherProgram"
        );
        // 大小写不敏感
        assert_eq!(
            cleaned_path_value(r"C:\Windows;c:\python39", &[r"C:\Python39".to_string()]),
            r"C:\Windows"
        );
    }

    // --- A-04 · 生产接线（按 \nmod tests 切分生产段） ---

    #[test]
    fn a04_appdata_scan_uses_the_strict_residual_matcher() {
        let src = production_src();
        let body = fn_body(src, "fn scan_windows_app_data_and_cache");
        assert!(
            body.contains("is_residual_dir("),
            "卸载残留/关联数据的删除方向必须走严格匹配"
        );
        assert!(
            !body.contains("dir_lower.contains("),
            "删除方向仍在复用模糊子串匹配"
        );
    }
}
