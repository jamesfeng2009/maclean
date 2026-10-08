# 目录级许可矩阵（License Boundary）

> 依据 strategy §6（目录级许可矩阵）+ split `04_PRIVATE_PRO_BOUNDARY.md`。

## 公开仓库（Apache-2.0）

| 路径 | 许可 | 商业角色 | 说明 |
|---|---|---|---|
| `apps/maclean-free/` | Apache-2.0 | Free 桌面应用（egui 壳 + CLI 入口） | 扫描 / 清单一 / 分类 / 安全 / 基础清理 / 还原 |
| `crates/maclean-types/` | Apache-2.0 | 公开契约层 | 领域模型 / CLI envelope / 错误码 / Feature 标识 |
| `crates/maclean-core/` | Apache-2.0 | 核心引擎 | scanner / safety / cleanup / restore / backup / scheduler |
| `crates/maclean-storage/` | Apache-2.0 | 本地持久化 | JSON-first 存储 / migrations / repositories |
| `crates/maclean-platform/` | Apache-2.0 | 平台适配契约 | Trash / 路径 / 注册表 / 磁盘 |
| `crates/maclean-cli/` | Apache-2.0 | CLI 层 | 命令解析 / envelope 渲染 / 退出码（Pro 命令为契约占位） |
| `tauri/` | Apache-2.0 | Tauri 壳 | 前端 + src-tauri（调 maclean-core） |
| `docs/` `scripts/` `assets/` `.github/` | Apache-2.0（文档/工具链） | 工程资产 | — |
| `LICENSE` `NOTICE` 等治理文件 | — | 治理 | 见 `GOVERNANCE.md` |

## 私有仓库（Proprietary，位于公开仓库之外）

| 路径（私有 maclean-pro 仓库） | 许可 | 商业角色 |
|---|---|---|
| `crates/maclean-pro-intelligence/` | Proprietary | Pro 智能分析 |
| `crates/maclean-pro-history/` | Proprietary | Pro 历史 |
| `crates/maclean-pro-forecast/` | Proprietary | Pro 预测 |
| `crates/maclean-pro-policy/` | Proprietary | Pro 策略 / premium rules |
| `crates/maclean-pro-automation/` | Proprietary | Pro 自动化（ScheduledAutomation） |
| `crates/maclean-pro-ai/` | Proprietary | Pro AI（AiExplanation） |
| `crates/maclean-pro-entitlement/` | Proprietary | License 签发 / 权威授权决策 |
| `crates/maclean-pro-ui/` | Proprietary | Pro 高级商业面板 |

## 边界不变量

1. 公开目录只含 Apache-2.0 可分发内容；私有目录（Pro）永不出现在公开仓库。
2. 商业依赖**单向**：Pro → 公开；公开 crate 不得依赖私有 crate（CI grep 门禁）。
3. License 私钥 / 签发材料（keygen、secret.key）绝不在公开仓库（strategy §17/§25）。
4. Pro 命令（history / growth / forecast / policy / automation）在公开 CLI 中
   仅契约占位，实现位于私有 Pro 仓库。
5. 边界调整由 owner 决策并记录（`GOVERNANCE.md`）；新增公开 crate 需同步更新本矩阵。
