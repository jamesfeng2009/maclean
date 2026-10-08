# maclean 安全模型（SAFETY MODEL）

> 状态：Open Core 拆分版（2026-10-07，tasks.md P4-5）
> 实现位置：`crates/maclean-core/src/safety.rs` + `crates/maclean-platform/src/path.rs`（纯逻辑）
> 契约类型：`maclean-types::safety`（[`SafetyCheck`] / [`SafetyGate`]）

maclean 是一个"删除文件的工具"，因此**信任的核心是删除安全**。
本文件描述安全模型、威胁模型、清理不变量与权限模型。

---

## 1. 安全模型（五层防护）

现有实现（`safety.rs` 顶部注释）定义五层防护：

| 层 | 名称 | 内容 | 状态 |
|---|---|---|---|
| 1 | 路径白名单 | 只允许删除已知安全路径下的文件 | 已实现 |
| 2 | 路径黑名单 | 明确禁止删除的系统关键路径 | 已实现 |
| 3 | 路径模式校验 | 检查路径是否匹配预期的安全模式（分类绑定） | 已实现 |
| 4 | 删除前二次校验 | 删除前再次检查路径合法性（TOCTOU 对抗） | 已实现 |
| 5 | 删除日志 | 记录所有删除操作，可回溯（备份清单） | 已实现 |

**关键设计**：层 4 是"第 3 层 Danger 的精确补集" —— 扫描与删除两处共用
`check_path_safety_with_category`，杜绝"扫得出、删不掉"或"删得掉、扫不出"
的分叉。

## 2. 核心不变量（Cleanup Invariants）

任何清理操作（CLI / GUI / Tauri / 未来 Pro）必须满足：

1. **删除必经闸门**：任何删除执行前调用 `SafetyGate::validate` /
   `check_path_safety_with_category`；返回非 `Safe` 一律不得执行。
2. **系统关键路径永不删除**：黑名单覆盖系统目录、受保护区域
   （macOS `/System`、`/usr` 等 SIP 保护路径）。
3. **用户主目录本身永不删除**；黑名单还覆盖 `~/.ssh`、`~/Library/Mail`
   等敏感位置。
4. **真实 home 判定**：用 `getpwuid` 读 passwd 库（`real_home_dir`），
   不受 `$HOME` 环境变量偏移影响 —— 防止黑名单前缀整体偏移。
5. **挂载点防护**：路径等于或位于挂载点之下时按挂载语义处理
   （`maclean-platform::path::is_path_mounted`；跨卷删除/快照有专门语义）。
6. **二次校验**：删除前重新检查路径合法性（对抗 TOCTOU）。
7. **可回溯**：每次删除写入备份清单（`crate::backup::record`），
   UI 收到 `BackupRecorded` 消息；还原路径存在。
8. **闸门不可被 license 绕过**：绕过 license 最多解锁功能（商业损失），
   绝不能改变文件系统安全行为（strategy §66）。

## 3. 威胁模型

| 威胁 | 描述 | 缓解 |
|---|---|---|
| 路径注入 / 符号链接 | 恶意目录内 symlink 指向系统文件 | symlink 检测（`maclean-platform::path::is_symlink`）+ 删除前二次校验 |
| TOCTOU | 校验后到删除前路径被替换 | 删除前再次校验（层 4）+ 单线程删除出口 |
| `$HOME` 偏移 | 环境变量改变导致保护前缀偏移 | `getpwuid` 真实 home |
| 挂载点误判 | 外部卷被误当普通目录 | 挂载点判定 + 快照/卷专用语义 |
| 批量聚合绕过 | `batch_paths` 聚合项绕过单路径校验 | 批量路径逐项校验 |
| 提权滥用 | sudo / 提权会话执行未授权删除 | 提权仅用于明确的系统级操作；删除闸门在提权路径同样生效 |
| 撤销/还原破坏 | 还原时覆盖现有用户数据 | 还原走备份清单 + 目标存在性检查 |

## 4. 权限模型

- **普通删除**：用户权限，路径须可写。
- **系统级操作**（部分优化/提权场景）：macOS 使用
  `AuthorizationExecuteWithPrivileges` / Touch ID（`LAContext`），
  通过 Security.framework 链接（`build.rs` 对 macOS 目标
  `rustc-link-lib=framework=Security`）。
- **sudo 保活**：`sudo_keepalive` 仅用于明确的授权会话，不自动提权。
- **Windows**：删除路径与注册表项由 `reg_safety` 校验；资源嵌入
  （manifest UAC 级别）在构建期处理。

## 5. 公开/私有边界（安全相关）

- **公开**（Open Core）：白名单/黑名单/模式校验/删除前二次校验/删除日志 ——
  全部在 `maclean-core`，任何人可审计。
- **公开契约**：`SafetyCheck` / `SafetyGate` / `CleanupCandidate` 在
  `maclean-types`。
- **私有**（Pro）：商业智能（预测/推荐排序/风险评分）不触碰删除决策；
  即使 Pro 建议删除，执行仍走同一公开闸门。
