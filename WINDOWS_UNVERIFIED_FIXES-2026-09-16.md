# Windows 未实测项：验证结论与修复报告

日期：2026-09-16　　范围：Jeffery 提出的 3 条未实测项（拆为 4 个 task）
结果：**177 passed / 0 failed**（入轮 149），Windows target 0 error，每项均做过反向验证

---

## 一、复现结论

### X-1 注册表子串误删 — **成立，且比预估更严重**

`scanner/windows_apps.rs::scan_registry_residual`

```rust
// 修复前
let matched = search_names.iter()
    .any(|sn| key_lower.contains(sn) || sn.contains(&key_lower));
```

链路上有两处各自独立地致命：

1. **双向子串**：反向那条 `sn.contains(&key_lower)` 让短键名被长应用名"含住"。
2. **首词提取**：`generate_search_names("Microsoft Edge")` 产出 `"microsoft"`。

只修其中一条没用 —— 保留首词提取、只删反向包含时，`Microsoft` 键仍会被
`Microsoft Edge` 正向命中。

终点是 `delete_registry_residual` 的 `reg delete <key> /f`：**`/f` 跳过确认且递归删除
整棵子树**。抹掉 `HKCU\SOFTWARE\Microsoft` 等于清掉该用户几乎所有 Microsoft 产品设置。

补充一个你没提但同级的面：`DisplayName` / `Uninstall` 键由**安装方自己写入**。
恶意安装包把自己命名为 `"Microsoft"`，用户点卸载就能借本工具删别人的注册表子树。
所以长度门槛不够，保护名单是必须的兜底层。

### X-2 目录子串误删 — **成立**

`scan_filesystem_residual` 用 `dir_name.contains(&name_lower)`，无长度门槛。
卸载 `"Go"` → `"google".contains("go")` → `%LOCALAPPDATA%\Google` 被列为
`deletable: true` 残留 → `delete_filesystem_residual` 走 `remove_dir_all`
（无确认框、不进回收站）→ Chrome 用户数据连保存的密码一起消失。

**对你判断的一处修正**：`has_large_user_data`（原 1389 行附近）虽然也用了双向子串，
但它是**保护性启发式** —— 命中导致"不允许卸载"，属于**过度保护**（体验问题），
不是数据丢失。真正导向删除的是 3247 / 3269 那一对。

### X-3 manifest 未校验 — **成立**

`manifest.json` 位于用户可写的 `%APPDATA%\maclean\backup\`，`load_manifest()`
反序列化后不做任何校验，`restore_last_backup()` 直接 `reg import entry.reg_file`。
改写 manifest 或替换 .reg 文件，就能借"还原"往任意注册表位置写入。
若进程以管理员运行则构成提权面。

### X-4 Docker 尺寸 — **代码上一轮就改对了，但零测试**

`get_docker_reclaimable_size` 已经传 `si=true`、已经跳过 Local Volumes。
问题不是没修，是**没人证明**。这种状态随时会被误改回去。

---

## 二、修复方案

### X-1 / X-2：三条硬规则 + 共享实现

抽出 `src/scanner/residual_match.rs`，删除目录与注册表匹配两处统一走：

- **不做反向包含** —— 短键名永远不能被长名字含住
- **不猜首词** —— 厂商键是共享的，删掉连坐同厂商其它产品
- **受保护名单兜底** —— 即使前两条被绕过，共享厂商容器也删不掉
  （注册表名单独含 Microsoft / Windows / Wow6432Node / Classes / Policies / …；
  AppData 名单含 Tencent / Google / Microsoft / Mozilla …）

`generate_search_names`（首词提取的根因）已删除。它那条既有单测
`test_generate_search_names` 断言的正是这个危险行为 —— 属于"把 bug 钉住"的测试，一并移除。

#### 刻意接受的能力回退

去掉首词提取后，`"Java 8 Update 291"` 再也匹配不到键 `"JavaSoft"`。
想命中只能靠 `"java" ⊂ "javasoft"`，而那正是 Microsoft 灾难的来源。
`reg delete /f` 递归整棵子树，**漏报一个残留键的代价远低于误删**，
这个回退不改，并有用例 `documented_recall_loss_from_dropping_first_word` 记录理由。

正常残留仍然扫得出来：Zoom、Discord、Spotify、7-Zip、Python("Python 3.11")、
Notion、以及对 vs 应用名自成一体的键。

### X-3：三重校验

新增 `src/platform/reg_safety.rs`，`restore_last_backup` 导入前必须过：

1. `.reg` 文件 canonicalize 后落在备份目录内（挡 `..` 与软链）
2. 文件内容声明的键**全部**落在该项记录的 `reg_key` 子树内
   （按完整组件比较，防止 `"EvilExtra"` 被当成 `"Ev"` 的子键；`[-KEY]` 删除形式也算进来）
3. `reg_key` 的 hive 必须在预期集合内 —— 否则空 `reg_key` 会让子树检查退化成恒真

### X-4：抽出纯函数 + 补测试

抽出 `sum_docker_reclaimable(stdout)` 后补 5 条用例，把 SI 换算、
Local Volumes 排除、输出格式错位、脏数据不 panic、调用点接线全部钉住。

---

## 三、本轮最大的坑：`#[cfg(target_os)]` 模块的假覆盖

我把 X-1/X-2 的纯函数和 test 写进了 `windows_apps.rs`。该文件是
`#[cfg(target_os = "windows")]`，于是本机：

```
cargo test windows_apps
→ 0 passed; 0 failed; 149 filtered out
```

**一个都没编译。** 而 `cargo check --all-targets` 照样全绿 —— 看着通过了，实则零验证。
上一轮同类问题上栽过（`is_windows_critical_path` 回退后纯函数用例照样全绿），这轮又踩了一次。

**解决办法不是"在 cfg 模块里想办法"，而是把待验证逻辑挪出 cfg 模块**：

| 新模块（不限平台） | 服务对象的 cfg 模块 |
|---|---|
| `src/scanner/residual_match.rs` | `scanner::windows_apps` |
| `src/platform/reg_safety.rs` | `platform::windows_backup` |

两者加 `#![allow(dead_code)]`，因为本机非 Windows 构建下没有调用方。

**判定法则**：Windows-only 文件里的任何修正，必须跑
`cargo check --target x86_64-pc-windows-msvc --all-targets` 才算验证过。
本机 check 通过不代表活干完了。

**信号**：`cargo test <名>` 出现 `X filtered out` 且 0 passed —— 那就是 cfg 挡住了，
别当成用例写错了。

---

## 四、测试与反向验证

| Task | 新增用例 | 反向验证（回退修复） |
|---|---|---|
| X-1/X-2 残留匹配 | 14 | **8 条转红**，含调用点级 `deletion_entrypoints_actually_use_the_strict_matcher` |
| X-3 备份校验 | 9 | **3 条转红**，含调用点级 `restore_entrypoint_actually_validates` |
| X-4 Docker | 5 | **5 条转红** |

反向验证里特意同时回退了"调用点"：把 `is_residual_dir(...)` 改回裸
`dir_name.contains(...)` 后，**14 条里有 8 条才红**。若只有纯函数会红、调用点不红，
说明保护再好也是死代码 —— 这条必须验证，不能只看纯函数。

---

## 五、顺带修掉的

- `ui/mod.rs` 的 `use crate::platform` 缺 cfg 限定，本机报 unused import（上轮我引入的）
- `main.rs` tests 里 `use crate::ops::*;` 缩进不规范（保留该导入，测试依赖它）

## 六、遗留 / 待你拍板

1. **40 个文件仍未 commit** —— 不提交，windows-ci 永远不会真正拦住任何东西。没替你提交。
2. **W-11 打包 manifest**（DPI 感知 / .ico）—— 交叉编译调不了 `rc.exe`，需真机验证。
3. **W-13 Windows 真机冒烟** —— 尤其 X-3 的三重校验在真实 UAC 提升场景下的表现，
   以及"Content-Mismatch 时用户看到什么"。
4. **新增的能力回退**（JavaSoft 一类键不再被自动识别）需要你确认是否接受。
   若需要更高召回，正确做法是维护一张"应用名 → 已知残留键"的显式映射表，
   而不是把首词模糊匹配加回来。
5. **顺带发现、未动**：`get_simulator_runtime_size()` 里 `parse_size_str(..., false)`
   假设 `xcrun simctl` 用 1024 进制。Apple 通常按 1000 进制，若属实则 macOS 侧模拟器
   大小同样虚高约 7%。属既有行为且需要真机核对，本轮不动。
