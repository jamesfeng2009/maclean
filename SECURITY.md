# Security Policy

maclean 是一个**删除文件的磁盘工具**，其安全模型与一般应用不同：缺陷可能导致
**用户数据被误删**。请以最高优先级对待以下安全维度。

## 受支持的版本

| 版本 | 安全支持 |
|---|---|
| 0.3.x | ✅ 支持（活跃） |
| < 0.3 | ❌ 不支持 |

## 报告漏洞（Responsible Disclosure）

**不要**在公开 Issue / PR / 论坛中披露未修复的漏洞。

1. 私信报告至 **security@maclean.example**（发布前替换为真实邮箱；若 GitHub Security Advisory
   「Report a vulnerability」可用则优先使用）；
2. 邮件请包含：影响版本、复现步骤、影响范围、建议修复（可选）；
3. 我们承诺：24h 内确认收到，7 天内给出修复计划与时间表；
4. 修复发布前不会公开细节；修复后会在 release notes 致谢（如你同意署名）。

## 高优先级攻击面（按风险排序）

| 攻击面 | 风险 | 说明 |
|---|---|---|
| **Path Traversal / TOCTOU** | 误删任意路径 | `sanitize_before_delete` 与 `best_effort_delete_with_reason` 的物理身份比对（st_dev/st_ino）被绕过，或验证→删除窗口被替换 |
| **符号链接 / junction** | 删除链接目标 | symlink 指向外部目录时被 `remove_dir_all` 跟随；staging rename + 身份比对应阻止 |
| **提权边界** | 越权删除 | touchid / sudo 提权路径未二次校验；`--privileged` 在非交互环境被滥用 |
| **busy-file / 挂载点** | 破坏运行中服务 / 卸载卷 | 删除被占用文件或挂载中的卷（`is_path_mounted` 判定绕过） |
| **License 绕过** | 商业能力未授权使用 | 免费额度 / Feature 门控被绕过（`license.rs` 验签、`dev-mode` 泄漏） |
| **命令注入** | 任意命令执行 | sudo 脚本拼接（换行 / 反引号 / `$()`）绕过 `sanitize_before_delete`；临时脚本权限/名不可预测 |

## 修复优先级

- **P0（立即修复 + 私下披露）**：可导致任意路径删除 / 任意命令执行 / 提权的漏洞；
- **P1（尽快修复）**：导致误删受保护路径、损坏数据、License 绕过；
- **P2（常规）**：信息泄露、日志泄漏路径、非安全向 DoS。

## 安全不变量（对应 `docs/safety/SAFETY_MODEL.md`）

1. 公开的删除 API 必须经过 `SafetyGate` 校验；
2. 任何删除动作不得直接作用于"验证后可能被替换"的路径（staging + 物理身份比对）；
3. 受保护系统路径（SIP / `/System` / 挂载点）永远不可删除；
4. 提权（sudo/touchid）必须在用户可见确认后触发；
5. CLI/UI 层不得包含文件系统删除实现（删除只在 `maclean-core`）。
