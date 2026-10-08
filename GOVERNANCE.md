# maclean Project Governance

## Owner 与最终权威

maclean 是一个**单人主导 + 社区贡献**项目。仓库 owner（`@jamesfeng2009`）
对以下事项拥有**最终权威**（final authority）：

| 领域 | Owner 决策 | 说明 |
|---|---|---|
| **License** | 采用 / 变更开源许可 | 当前 Apache-2.0（策略基线 `MACLEAN_OPEN_CORE_LICENSE_AND_REPOSITORY_STRATEGY_V1.md`）；变更需发布治理公告 |
| **Trademark** | 品牌使用授权 | 见 `TRADEMARK_POLICY.md`；任何授权例外由 owner 书面批准 |
| **Release** | 版本号 / 发布节奏 / 产物签名 | 语义化版本；发布产物（dmg/pkg/cli）由 owner 或 CI 签名；`main` 分支受保护 |
| **Security** | 漏洞披露 / 修复优先级 / 拒绝披露 | 见 `SECURITY.md`；owner 是漏洞披露流程的最终裁决人 |
| **Commercial Boundary** | Open Core 边界划分 | 什么留在公开仓库、什么进入私有 Pro，见 `docs/governance/LICENSE_BOUNDARY.md` 与 `strategy §14`；边界调整由 owner 决定并记录 |

## 决策流程

1. **常规改动**：PR + review（见 `CONTRIBUTING.md`）；owner 合入。
2. **争议**：owner 裁决，裁决记录在 PR / Issue 中。
3. **治理级改动**（License / 边界 / 品牌）：先开 Discussion 征求社区意见，
   owner 发布决策说明并更新本文档。

## 仓库拓扑

```text
公开仓库（本仓库）        私有仓库（不公开）
maclean (Open Core)        maclean-pro (Pro 商业侧)
├── apps/maclean-free      ├── maclean-pro-intelligence
├── crates/maclean-types   ├── maclean-pro-history
├── crates/maclean-core    ├── maclean-pro-forecast
├── crates/maclean-cli     ├── maclean-pro-policy
├── crates/maclean-storage ├── maclean-pro-automation
├── crates/maclean-platform├── maclean-pro-ai
└── tauri                  ├── maclean-pro-entitlement
                           └── maclean-pro-ui
```

- 商业依赖**单向**：Pro → 公开 crate；公开 crate **永不反向依赖** Pro。
- 私有逻辑（私钥签发、授权决策、Pro 智能）绝不进入公开仓库。

## 贡献者

通过 DCO（见 `DCO.md`）与 `CODE_OF_CONDUCT.md` 参与；贡献者名单在
release notes 中致谢。
