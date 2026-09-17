# maclean 代码逻辑 Review（2026-09-15）

范围：`src/` 全量 29,497 行（不含 `target/`）。
方法：核心链路（main / ops / app / ui 状态机 / safety / license）逐行读；scanner 与平台层并行深挖；**所有标为「已验证」的条目均由我亲自读码或本机实测确认**，未验证的单独标注。

---

## ✅ 修复状态（2026-09-15 当轮已全部修复）

P0-1 ~ P0-6 全部修完，86 个测试全绿（新增 7 个回归测试），`cargo check --all-targets` 0 error。

| 项 | 修复位置 | 回归测试 | 反向验证 |
|---|---|---|---|
| P0-1 一键清理空操作 | `ui/mod.rs` 改调 `prepare_delete_cross_tab` | —（UI 行为，需手测） | — |
| P0-2 Touch ID 卡死 | `ui/mod.rs` `clear_delete_rx = !started_new_delete` | —（UI 行为，需手测） | — |
| P0-3 确认框 0 项 0 B | `app.rs` 新增 `pending_items/count/total_size`；`ui/mod.rs` 改用之 | —（UI 行为，需手测） | — |
| P0-4 bundle ID 穿越 | `scanner/uninstall.rs` 入口字符校验 + 出口边界兜底 | `safe_path_segment_rejects_traversal` / `allowed_associated_path_rejects_outside_home` / `associated_files_reject_path_traversal_bundle_id` | ✅ 回退后测试失败并复现 `/Users/fengyu/Library/Containers/../../../..` |
| P0-5 「 \| 」早退 | `safety.rs` `\|\|` → `&&` | `pipe_separator_no_longer_bypasses_blacklist` / `time_machine_snapshot_still_allowed` | ✅ 回退后测试失败并复现 `/System/ \| x -> Safe` |
| P0-6 license 后门 | `license.rs` 改编译期 feature + `Cargo.toml` + `Makefile` | `dev_mode_is_off_without_feature` | ✅ `strings target/release/maclean \| grep MACLEAN_DEV` = 0 |

**反向验证**：P0-4 / P0-5 的测试都做过"回退修复 → 测试必须失败"的验证，确认不是"永远通过"的假测试。P0-4 回退后实测复现了越界路径 `/Users/fengyu/Library/Containers/../../../..`（解析后为 `/Users`），证明该漏洞真实可利用。

**P0-6 说明**：改用编译期 feature 而非 `debug_assertions`，因为后者可在 profile 里被改、且发布与本地 release 是同一条命令，不够显式。
- 本地开发：`make dev`（= `cargo run --features dev-mode`）
- 发布：`cargo build --release`（不带 feature，后门代码整体编译掉）
- `scripts/build-package.sh` 与 `.github/workflows/ci.yml` 均用 `cargo build --release`，已确认不带该 feature。

**遗留**：CI 的 `cargo fmt --all -- --check` 与 `cargo clippy -- -D warnings` 两道门禁**在本次改动前就已经是红的**（全项目 99 个 clippy warning、fmt 涉及 icons/widgets/ui/ops/main 五个文件）。本轮只清理了自己新增代码的格式与 clippy 问题，未做全量格式化以免 diff 失控。需要单独安排一次 `cargo fmt --all` + clippy 清理。

---

## 结论先行

最严重的问题不是架构，是**三个用户一上手就会撞上的功能死穴**：

1. 概览页「一键清理」按钮点了**完全没反应**；
2. 开了 Touch ID 走 sudo 删除，**弹窗必然永久卡死**；
3. 跨 Tab 删除时确认框显示「**0 项 / 0 B**」，但点下去真删 N 个文件。

数据安全侧有 2 个真实可绕过的口子（bundle ID 拼路径、`" | "` 早退），以及 1 个进了 release 二进制的授权后门。

---

# P0 — 必修（功能死穴 / 数据安全）

## P0-1 概览「一键清理」是空操作 —— 已验证

`src/ui/mod.rs:5073-5083`

```rust
if clean_btn.clicked() {
    for &(tab_idx, item_idx, _) in &recommendation_items {
        if let Some(item) = app.results[tab_idx].get_mut(item_idx) {
            item.selected = true;          // 跨 Tab 勾上了
        }
    }
    app.prepare_delete();                   // ← 问题在这
}
```

`prepare_delete()`（`app.rs:943`）用 `let idx = self.tab_index()`，此时 `tab == Overview` → `idx = 0`。而 `results[0]` **恒为空**（`start_scan` 对 Overview 返回空，`start_scan_all` 从索引 1 开始），于是 `selected.is_empty()` → 静默 `return`，`confirm` 保持 `None`，确认框永不弹出。

**反证**：`app.rs:965` 的 `prepare_delete_cross_tab()` 完全适配这个场景，但**全仓没有调用点**——这是明显的漏接。

**副作用**：跨 Tab 的 `item.selected = true` 已经写进去了却没被消费，用户切到别的 Tab 会看到莫名其妙勾了一堆项。

修复：改为 `app.prepare_delete_cross_tab(recommendation_items.iter().map(|(t, i, _)| (*t, *i)).collect());`

---

## P0-2 Touch ID 删除路径必然永久卡死 —— 已验证

`src/ui/mod.rs:462-489`（`Ok(DeleteMessage::NeedPassword)` 分支）

```rust
if app.touch_id_enabled && !clamshell_closed {
    app.confirm = ConfirmState::SudoWithTouchId;
    start_sudo_delete_touchid(items, app.lang_en, &mut self.delete_rx);  // 刚写入新 receiver
}
...
clear_delete_rx = true;   // ← 无条件执行
break;
```

分支外（`ui/mod.rs:563`）紧接着 `if clear_delete_rx { self.delete_rx = None; }`。

**结果**：刚创建的 receiver 在**同一帧**被丢弃，删除线程后续所有消息无人接收，`app.confirm` 永远停在 `SudoWithTouchId`，`delete_done/delete_total` 停在 `0/N` —— 用户看到"Touch ID 验证中"转圈，进度不动，取消键也出不来，只能杀进程。

注意同一文件 `Disconnected` 分支（`:551`）注释里明确写着"否则弹窗会永久卡在 Deleting（历史 bug）"——说明这条链路以前就踩过一次坑，重构时又踩回来了。

修复：启动了新的删除线程时不要置 `clear_delete_rx`。建议把 `clear_delete_rx` 的初值改为 `false`，只在 `Done` / `Disconnected` 分支显式置 `true`。

---

## P0-3 跨 Tab 删除的确认框显示「0 项 / 0 B」—— 已验证

`src/ui/mod.rs:2940-2977`

```rust
let count = app.selected_count();
let size  = app.selected_total_size();
let idx   = app.tab_index();          // = 0（Overview）
for item in &app.results[idx] { ... } // results[0] 恒空
```

统计源是「当前 Tab」，而 `pending_delete` 是跨 Tab 的 `(tab_idx, item_idx)` 列表，两者不同源。

**后果**：
- 标题显示「确认删除 **0 项**？」、可释放 **0 B**、分级统计（安全/谨慎/高级）全 0、预览列表为空；
- 但点「确认删除」会**真实删除 N 个跨 Tab 文件**。用户在零信息下确认高危操作；
- `needs_admin`（:2970-2976）同样按当前 Tab 计算 → 漏判提权需求 → 该 sudo 的不 sudo，删除静默失败。

修复：确认框一律按 `app.pending_delete` 汇总，不依赖 `tab_index()`。

---

## P0-4 uninstall 关联文件：bundle ID 未校验直接拼路径 —— 已验证

`src/scanner/uninstall.rs:855`

```rust
let p = home.join(format!("Library/Containers/{}", bid));
if p.exists() { paths.push(...); }
```

`bid` 来自 `get_bundle_id()`（`:683`）对 `defaults read <plist> CFBundleIdentifier` 的原始 stdout **trim 后直接使用，零校验**。

**触发**：某个 `.app` 的 `CFBundleIdentifier` 写作 `../../../../Users/xxx/Documents`，拼出的路径 `~/Library/Containers/../../../../Users/xxx/Documents` 经 `p.exists()` 判定为真 → 整个 **Documents 目录被当作"应用关联数据"推入删除列表且 `deletable`**。

**兜底拦不住**：`safety.rs` 第 4a 层是精确匹配（只保护目录本身，明确允许删除子项，注释写在 `:140`），而 `Documents` 不在 `is_critical_system_path` 的名单里 → 一路放行。

修复（两道都要）：
1. `bid` 做字符白名单 `[A-Za-z0-9._\-]`，非法直接跳过该变体；
2. 候选路径 `canonicalize()` 后必须 `Path::starts_with(home)`（组件匹配，不是字符串前缀）才入列。

同类问题：`Group Containers`（`:866+`）、`Caches`、`Application Support` 等后续拼接处需一并加固。

---

## P0-5 `safety.rs:74`「` | `」早退，绕过全部 7 层防护 —— 已验证

```rust
// 非文件路径（APFS 快照等），跳过文件路径检查
if path.starts_with("com.apple.TimeMachine.") || path.contains(" | ") {
    return SafetyCheck::Safe;
}
```

这个判断位于第 1 层之后、**第 3/4/5/7 层黑名单之前**。任何包含「空格+竖线+空格」的路径被无条件放行：

- `/System/Library | x` → `Safe`
- `/Users/u/Library/Mail | x` → `Safe`

本意是放行 APFS 快照（`com.apple.TimeMachine.xxx | yyy`）这类非文件路径，但 `contains(" | ")` 条件过宽，且 `||` 让第二个条件独立生效，把前缀约束整个废掉了。

修复：收紧为 `path.starts_with("com.apple.TimeMachine.") && path.contains(" | ")`，并把该判断移到所有黑名单检查**之后**（作为兜底放行，而非前置放行）。

---

## P0-6 license 开发者后门进了 release 二进制 —— 已验证

`src/license.rs:33-37`

```rust
pub fn is_dev_mode() -> bool {
    std::env::var("MACLEAN_DEV").map(|v| v == "1" || ...).unwrap_or(false)
}
```

无 `cfg(test)` 也无 feature 门控。发布版只要 `MACLEAN_DEV=1 open -a Maclean` 就返回 `Activated { plan: "dev" }`，且 `quota_gate_for()`（`:992`）直接放行、无额度限制。

修复：改为编译期 feature（`#[cfg(feature = "dev")]`），发布 profile 不启用。

---

# P1 — 应该修（正确性 / 体验 / 性能）

| # | 位置 | 问题 | 影响 |
|---|---|---|---|
| 7 | `ops/mod.rs:266-269` | `start_scan_all` **不清空** `results`（对比 `start_scan` 在 `:54` 有 `clear()`） | 二次「扫描全部」时 `PartialItems` 走 `extend`，`Done` 前列表是旧+新叠加，概览统计翻倍；某 Tab panic 未发 Done 则重复项固化 |
| 8 | `ui/mod.rs:2340 / 2002 / 2446` | 扫描态判定不统一：这三处用 `current_scan_state()`（只看当前 Tab），而 `ui/mod.rs:151` 已用 `any(Scanning)`。`start_scan_all` 不置 `scan_states[0]` | 概览页「扫描」按钮扫描中永不置灰 → 可并发拉起多轮全量扫描，I/O 打满；底部删除栏扫描中仍可点 → 扫描中触发删除 |
| 9 | `scanner/dev_cache.rs:2418-2426` | `skip_dirs` 含 `"Library/Caches"`、`"Library/Application Support"` 等**带斜杠**的项，但比较是 `name == *skip`，`name` 只是单个文件名 | 这两条（及其余带斜杠项）**永不生效**，遍历仍深入 Caches / Application Support |
| 10 | `scanner/dev_cache.rs:203` | `if name.is_empty() \|\| path.starts_with(".")` 应为 `name.starts_with('.')`。对完整路径调 `Path::starts_with` 是组件匹配，恒为 false | 隐藏目录过滤失效 |
| 11 | `app.rs:462 `next_tab`/`move_up`/`toggle_select`/`quit`，`should_quit` `:1240` | 重构后 `Gui::update` 里**无任何全局按键处理**，这些方法全仓无调用点；界面仍提示「按 R 扫描」（`ui/mod.rs:2304/2319`） | 键盘导航整体失效，且除托盘外无退出路径 |
| 12 | `ui/mod.rs:777-784` | `render_app_uninstall_panel` 开头**无条件** `logger::info(...)` | 60fps 下每秒数十次日志 I/O，纯浪费 |
| 13 | `scanner/cache.rs:73-95` | 缓存（TTL 24h）反序列化后直接进 UI 并参与删除，加载时不重新 stat、不校验路径类型/归属 | TOCTOU：路径已被删/已被替换成软链仍照删；删除后仅靠调用方 `invalidate_cache`，其他 Tab 旧缓存仍持有失效路径 |
| 14 | `scanner/app_cache.rs:341 / 541` | `scan_app_support_caches` 与 `scan_browser_caches` 对 `.../Default/GPUCache`、`.../Default/Cache` 用同一份 `CACHE_DIR_NAMES` 各扫一次 | 同 Tab 内重复项、`total_size` 翻倍 |
| 15 | `safety.rs:136` | `dirs::home_dir()` 在 unix 读 `$HOME` 环境变量 | 组合场景（`HOME` 被改 + 从磁盘缓存加载绝对路径）下第 4 层保护前缀整体偏移，真实 `~/Library/Mail`、`~/.ssh` 不在 `is_critical_system_path` 名单内 → 放行。建议改用 `getpwuid` 取真实 home |
| 16 | `scanner/mod.rs:193` | `home_dir()` 失败回退 `"/"` | 失败时会去扫 `/Library/Application Support`、`/`，且结果标 `deletable:true`。失败应返回空结果 |
| 17 | `ui/mod.rs:4922/4960/4992`、`widgets.rs:516-518` | 深色模式下残留硬编码色（`232,255,243`、`#1B4E8C` 等） | 深色下变成暗底暗字 / 突兀亮块 |
| 18 | `ui/mod.rs:2460 / 2678` | 渲染期 `app.results[tab_idx].clone()` 整表深拷贝；`build_uninstall_groups` 在 header/panel/filter_pills/footer 重复构建 4 次 | 列表上万项时明显掉帧 |
| 19 | `ops/mod.rs:69` 进度线程 + `ui/mod.rs:364` | `Done` 把 `scan_progress` 置 1.0，但 200ms 估算线程仍继续投递 `Progress` | 多 Tab 扫描时进度条在 1.0 ↔ 0.x 间反复回退 |

---

# P1 修复状态（2026-09-15 完成）

**全部 19 条已修复。测试 114 全绿，0 编译错误，新增代码 clippy 干净。**
关键几条做过**反向验证**：临时把修复回退 → 对应测试必须失败，否则视为假覆盖、重写。

| # | 修法 | 反向验证 |
|---|---|---|
| 7 | 抽出 `App::reset_for_full_scan()`，`start_scan_all` 改为调用它第 2 次全量扫描不再叠加旧结果 | ✅ 去掉 `clear()` → `reset_for_full_scan_clears_previous_results` 失败 |
| 8 | `App` 上收敛为 `any_scanning()` / `tab_scanning(idx)`，UI 不再直接摸 `scan_states` | ✅ |
| 9 | `dev_cache.rs` 抽出 `should_skip_dir_entry()`：带斜杠项按路径后缀匹配（要求前导分隔符） | ✅ 回退 → `skip_dirs_with_slash_actually_match` 失败 |
| 10 | 隐藏目录判断改为 `name.starts_with('.')` | ✅ 见 `hidden_dir_detection_uses_file_name` |
| 11 | `Gui::handle_shortcuts()`：Cmd+Q / Esc / `/` / Tab / ↑↓ / Space / R；焦点在输入框时不抢键 | 手工验证（需真实键盘事件） |
| 12 | 删除卸载面板每帧的 `logger::info` | 无测试（删除类改动） |
| 13 | `cache.rs` 加载时逐个 `symlink_metadata` 重校验：丢弃已消失 / 被换成软链的路径，刷新文件大小，重算 `total_size`；聚合项筛 `batch_paths` | ✅ 回退 → 4 条测试失败 |
| 14 | `app_cache.rs` 抽出 `dedup_by_path()` | ✅ 回退 → `dedup_removes_repeated_paths` 失败 |
| 15 | 新增 `real_home_dir()`（getpwuid）与 `protected_homes()`；第 4 层抽出 `check_home_paths(canonical, canonical_str, homes)`，**homes 可注入** | ✅ 回退锁定单 home → `home_protection_covers_every_injected_home_not_just_the_first` 失败 |
| 16 | `home_dir()` 失败返回空路径；新增 `has_home()`，三个扫描器入口加守卫 | ✅ |
| 17 | 新增 `theme::danger_100/danger_600` token，替换 `widgets.rs` 与两处低对比硬编码色 | 视觉项，未反向验证 |
| 18 | `build_uninstall_groups` 每帧 3 次 → 1 次（顶层算好往下传）；胶囊统计抽出 `uninstall_pill_stats()` | ✅ 回退 CacheOnly 合并 → `pill_stats_merges_cache_only_into_safe` 失败 |
| 19 | `poll_scan` 用局部 `scan_finished` 拦掉 Done 之后的 Progress | ✅ 回退 → `scan_progress_does_not_regress_after_done` 失败（left 0.7 / right 1.0） |

## 过程中的两个教训

1. **直接调内部纯函数的测试可能是假覆盖。** #15 一开始只测了 `homes_from()`，绕过了真正的组合点 —— 回退修复后测试照样通过。改成从第 4 层可注入入口验证才抓得住。
2. **脚本插辅函数会切碎文档归属。** 在 `safety.rs` 里插 helper 时把 `check_path_safety_with_category` 的文档注释拦腰截断（clippy `doc_lazy_continuation` 才暴露），已搬运修好。

## 仍未处理

- **渲染期 `app.results[tab_idx].clone()` 深拷贝（#18 剩余部分）**：三处 clone 是借用检查所必需（闭包要同时持有 `&mut App` 和列表），改成借用编译不过。要彻底去掉得重写三处闭包为索引访问 + 逐字段取值，改动面很大。**留到能跑 GUI 实测时再做** —— 没有 UI 冒烟测试的情况下动 6000 行渲染路径，风险远大于收益。
- **CI 的 fmt 门禁仍是红的**：`icons.rs` / `widgets.rs` / `ui/mod.rs` / `ops/mod.rs` / `main.rs` 有上次重构遗留的格式问题。我只格式化了自己新增的部分，没做全量 `cargo fmt`（diff 会失控到无法 review）。
- **Windows 子串匹配误删（`windows_apps.rs`）** 仍未复现验证。

---

# P2 — 轻微

- `scanner/apfs.rs:74-80` 快照名取 `line[idx..]` 到行尾；`delete_snapshot` 的 `strip_suffix(".local")` 失败时 `unwrap_or(name)` 会把整行（含 `com.apple.TimeMachine.` 前缀）传给 `tmutil` → 删除必失败且不可诊断。
- `platform/mod.rs:154` 废纸篓用 AppleScript，`replace('"', "\\\"")` **未先转义反斜杠** → 文件名含 `\"` 时可提前闭合字符串（潜在 RCE）。建议改 `trash` crate。
- `sudo_keepalive.rs:32/136` `Mutex::lock().unwrap()`，锁中毒即 panic。
- `license.rs` payload 无 `exp` 字段、代码也从不与当前时间比较 → `yearly` 等同永久；`mid` 为空时跳过绑机（`:142`），测试 key 即空 mid → 一把 key 全网通用。
- `license.rs:202` `Command::new("ioreg")` 走 PATH，失败回退到 `HOSTNAME`/`USER` 环境变量 → 指纹可伪造。
- `safety.rs:68` `..` 检测按 `/` 切分，Windows 路径 `C:\a\..\..\b` 不触发；且该函数全无 `cfg` 门控，Windows 上黑名单整体失配。
- 深色模式目前只能手动切换，eframe 0.29 无 `follow_system_theme`，首次启动取一次系统偏好。

---

# 已推翻的误报（别浪费时间改）

**「大小写绕过 `/system` → `realpath` 不纠正大小写」—— 不成立。**
本机实测（APFS，大小写不敏感卷）：

```
realpath(/system)         = /System
realpath(/SYSTEM)         = /System
realpath(/system/library) = /System/Library
realpath(//System/Library)= /System/Library
```

`canonicalize()` 会归一化大小写，所以 `safety.rs` 第 3 层的 `starts_with("/System/")` 仍然命中。这条不需要改。

（衍生出的真实但低危的点：`canonicalize` **失败**时回退原始字符串做比较，此时不做大小写/斜杠归一。但路径不存在时删除本身也会失败，风险可忽略。）

---

# 未实测项

以下来自静态阅读、我未在本机或目标平台验证，修复前建议先复现：

- **Windows 注册表/目录子串匹配误删**（`scanner/windows_apps.rs:3316/3501/3247/3269`）：`key_lower.contains(sn) || sn.contains(&key_lower)` 双向子串 + `generate_search_names` 会生成首词。卸载 "Microsoft Edge" → 生成 "microsoft" → 可能匹配 `HKCU\SOFTWARE\Microsoft` 并执行 `reg delete /f`（连子键抹掉）；应用名为 "Go"/"R"/"V" 这类短名时，`dir_name.contains("go")` 会命中 `%APPDATA%\Google` → 删掉 Chrome 用户数据。**这条如果成立是 Windows 上的灾难级 bug，建议优先复现。**
- `platform/windows_backup.rs:261-289`：manifest.json 位于用户可写 `%APPDATA%`，改 `reg_file` 指向任意 `.reg` 即被 `reg import`（持久化）。
- `scanner/dev_cache.rs:1754` docker `parse_size_str` 把 SI 单位（`1.2GB`=10⁹）按 1024³ 换算，高估 7–20%；且把 "Local Volumes" 的 RECLAIMABLE 计入，但 UI 描述写"不会清理数据卷"。

---

# 修复顺序建议

1. **P0-1 / P0-2 / P0-3**（半天）：三个都是 UI 层的单点改动，风险低、收益立竿见影。P0-2 建议顺手把 `clear_delete_rx` 的默认语义改成"只在真正结束时清"，避免同类问题再犯。
2. **P0-4 / P0-5**（半天）：数据安全，改完要补测试（构造恶意 `CFBundleIdentifier`、构造含 ` | ` 的路径）。
3. **P0-6**（10 分钟）：license 后门，发布前必须关。
4. **Windows 子串匹配复现**（优先级取决于 Windows 支持是不是真的要上）。
5. P1 按需排期，其中 #7 #8 #12 #19 建议连着一起做（都是扫描状态机的一致性）。

---

# 结构层面的两点观察

- **`ui/mod.rs` 仍是 6,047 行的单体**，且混了「纯渲染」与「状态决策」。P0-2/P0-3 都是"状态决策埋在渲染函数里、无法单测"的直接后果。建议下一步拆成 `ui/panels/` + `ui/dialogs/` + `ui/state.rs`（状态迁移单独可测）。
- **扫描态判定在 `ui/` 里有两套语义**（`any(Scanning)` vs `current_scan_state()`），这是 P1-8 的根因。建议在 `App` 上收敛成 `any_scanning()` / `tab_scanning(idx)` 两个明确方法，禁止 UI 直接摸 `scan_states` 数组。
