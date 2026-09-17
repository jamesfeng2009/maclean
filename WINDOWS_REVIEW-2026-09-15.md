# maclean Windows 端完整性评估与修复报告

日期：2026-09-15
范围：Windows 端功能实现完整度排查 → 缺陷修复 → 回归测试
验证环境：开发机 macOS（Windows 侧通过交叉编译 + 跨平台单测覆盖）

---

## 一、结论先行

**Windows 端此前处于"代码写了大半，但从未被编译验证过"的状态。**

最硬的证据：`cargo check --target x86_64-pc-windows-msvc --all-targets` 一开始报
**13 个编译错误**。也就是说，Windows 目标**根本构建不出来**——不是"功能少几个"，
而是产物都不存在。

### 为什么这么久没人发现

排查出两个原因，缺一不可：

1. `.github/workflows/windows-ci.yml` **确实存在**（9b7ad92 引入），但其中的
   `cargo check` **没有带 `--all-targets`**，且 Linux/macOS 宿主永远编译不到
   `#[cfg(target_os = "windows")]` 里的代码，等于没有真正的验证。
2. **工作区 40 个文件至今没有 commit/push**——包括 `src/ops/mod.rs`（该文件
   **在 HEAD 里根本不存在**）。CI 从未见过这批代码，门禁自然无从生效。

> 一句话：门禁的门是关着的，但代码根本没走到门口。

---

## 二、功能完整度评估矩阵

| 能力面 | 状态 | 说明 |
|---|---|---|
| 平台抽象层（home / app_data / 路径展开 / 回收站 / 深色跟随 / 磁盘信息） | ✅ 已实现 | Windows 分支完整，含 registry 查询 AppsUseLightTheme |
| 开发者缓存 Tab | ✅ 可用 | 38 条缓存登记表仅 1 条 N/A（Nix，Windows 本就无）；含 WSL2 vhdx / Temp / Docker Desktop |
| 大文件 Tab | ✅ 可用 | 平台无关 |
| App 缓存 / 数据 / 卸载 Tab | ✅ 可用 | `windows_apps.rs` 4266 行：UWP / winget / MSI / 注册表卸载 / 残留扫描清理 |
| 卸载后残留清理弹窗 | ✅ 可用 | 注册表 + 环境变量 + 文件三类 |
| 系统还原点 / 注册表备份 / 还原入口 | ✅ 可用 | `windows_backup.rs` |
| 提权删除 | ⚠️ **原为 macOS sudo 语义，Windows 100% 失败** | 见 W-7，已重写 |
| **系统优化 Tab** | ⚠️ **代码写好了但被 cfg 挡住，永远空屏** | 见 W-8，已接线 |
| JetBrains 缓存 | ⚠️ **原硬编码排除 Windows** | 见 W-9，已补 |
| License 机器指纹 | ✅ 已实现 | MachineGuid + COMPUTERNAME 回退 |
| CLI | ✅ 基本可用 | optimize 子命令此前在 Windows 失效，已修 |
| **删除前安全校验** | 🔴 **Windows 上形同虚设** | 见 W-3/4/5/10，已补 |
| 打包 manifest（DPI / 图标 / UAC） | ❌ 缺失 | 见 W-11，未实施 |

---

## 三、严重缺陷：安全校验在 Windows 上等于没开

这是本次最需要警觉的部分。一个会**真实删除用户文件**的工具，其"删除前最终屏障"
在 Windows 上几乎是空的：

| 层 | 原实现 | Windows 上的后果 |
|---|---|---|
| 第 3 层 系统黑名单 | 全是 `/System` `/usr` `/bin` 等 POSIX 路径 | `C:\Windows`、`System32`、Program Files、ProgramData、盘符根**全部判定为 Safe** |
| 第 1 层 路径遍历 | `path.split('/')` | `C:\Users\x\..\..\Windows` 拆不出 `..` 组件，**直接绕过** |
| 第 4 层 home 黑名单 | 全是 `Library/xxx` 后缀，且用 `format!("{}/")` 拼接 | Windows 反斜杠路径永远匹配不上，用户凭据/邮件数据**无保护** |
| 第 6 层 敏感文件名 | `.cargo/credentials` 写死正斜杠 | Windows 路径不命中 |

这些并非"理论上存在"的风险：注册表里读出的卸载路径可由安装方决定，
一旦 path 被构造到 `C:\Windows\...`，原代码会**照删不误**。

---

## 四、修复清单与反向验证

用户要求的验收方式是"需要通过测试"。这里采取的做法是：
**每修一处，就把修复回退一次，确认对应测试确实会失败**——否则无法证明这条用例有效。

| Task | 修复内容 | 反向验证结果 |
|---|---|---|
| W-1 / W-2 | Windows target 编译通过（给 macOS sudo 链路加 cfg；补 platform 导入） | 13 error → **0 error** |
| W-3 | 新增 `is_windows_critical_path`：系统路径 / UNC / 盘符根 / `\\?\` 设备前缀 / 大小写不敏感 | 入口用例 **FAILED** ✓ |
| W-4 | 遍历检测同时按 `/` 与 `\` 切分 | **FAILED** ✓ |
| W-5 | home 黑名单拆成 macOS / Windows 两套；改用 `Path::starts_with`（按组件比较） | 3 条 **FAILED** ✓ |
| W-5 | Windows 第二重真实 home（注册表 `Volatile Environment`，防 `%USERPROFILE%` 篡改） | 2 条用例 |
| W-7 | Windows UAC 提权删除替代 macOS sudo 密码链路 | 脚本构造 5 条用例 |
| W-8 | 系统优化 Tab 放开到 Windows | 改掉一个执行器分支名即 **FAILED** ✓ |
| W-9 | JetBrains 支持 Windows 路径 + 版本号自然序排序 | 5 条用例 |
| W-10 | 敏感文件名支持 Windows 分隔符 | `Some("credential")` ≠ `Some(".cargo/credentials")` **FAILED** ✓ |
| W-12 | `windows-ci.yml` 补 `--all-targets`，并注明 release 不得开 dev-mode | — |

**测试结果：149 passed / 0 failed**（修复前 114，本轮新增 35 条）
**Windows target：`cargo check --target x86_64-pc-windows-msvc --all-targets` 0 error**

### 反向验证暴露的一个陷阱

回退 W-3 修复后，**直接调用 `is_windows_critical_path` 的那 6 条用例依然全绿**，
只有走入口 `is_critical_system_path` 的那条失败了。

原因很直白：纯函数写得再正确，只要没人调用它就是死代码。这个项目和上一次 P0/#15
遇到的是同一个坑——**只测纯函数是不够的，必须有一条打在真正被调用的入口上的用例**。

---

## 五、顺手修掉的连带缺陷

JetBrains 旧版本清理按**字符串**比版本号：

```
"2024.10" < "2024.2"   // 字符串比较：'1' < '2'
```

于是 `2024.10` 会被判为"旧版"删掉，而真正的旧版 `2024.2` 反而留下。
JetBrains 一年发三四个版本，跨到两位数小版本是必然的。

既然本轮要把这条删除逻辑推广到 Windows，就必须先把这个数据丢失风险修掉。
已改为复用 `dev_cache.rs:443` 已有的 `version_compare`（它还能处理预发布后缀）。
> 注：我第一版自行写了个 `version_key`，属于重复造轮子，已删除。

---

## 六、未完成项

| Task | 内容 | 未做的原因 |
|---|---|---|
| **W-11** | Windows 打包 manifest（DPI 感知 / .ico 图标 / UAC 执行级别） | 交叉编译环境无法调用 `rc.exe`，引入 `embed-resource` 需新增依赖；效果必须在真机肉眼验证。**等你决定是否要做。** |
| **W-13** | Windows 真机冒烟 | UAC 提权删除、卸载残留弹窗、优化任务实际效果、还原点创建/还原、深色模式跟随、回收站是否真进 Recycle Bin——这些在本机无法覆盖，只能人工在 Windows 上跑。 |

另外两项需要你知晓：

- **工作区 40 个文件仍未 commit**。`src/ops/mod.rs` 甚至不在 HEAD 里。
  不提交的话，Windows CI 永远不会真正开始起作用。我没有替你提交。
- **fmt 门禁仍红**：`icons.rs` / `main.rs` / `ops/mod.rs` / `safety.rs` 等存在格式差异
  （部分是上次重构遗留，部分是本轮新增）。跑 `cargo fmt --all` 会造出无法 review 的
  巨型 diff，建议单独排期。

---

## 七、设计取舍说明

**为什么 `AppData\Local` 没有整棵列为禁区？**

因为 Windows 的清理目标恰恰大量位于其下（npm / pip / Chrome 缓存都在
`AppData\Local`）。一刀切会让 Windows 端几乎无事可做。

采取的策略与 macOS 侧对 `Library/Caches` 的处理保持一致：**只禁根目录自身，
放行其子项**。真正危险的东西改用前缀禁掉——凭据管理器、DPAPI 主密钥、Outlook
数据、用户证书存储、`.ssh`、`.gnupg`。这条边界有一条专门用例守着
（`windows_home_roots_blocked_but_cache_targets_allowed`），
既防"保护不足"，也防"保护过度把功能废掉"。
