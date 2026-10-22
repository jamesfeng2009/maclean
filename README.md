# maclean CLI

**Free & Open Source · Storage Intelligence Engine for your terminal**

maclean 是面向开发者的磁盘清理与存储智能命令行工具：扫描结果会标注

**删了会怎样**（要重新下载 / 要重新编译 / 含用户数据需确认），删除前

对受保护应用单独拦一道，缓存永久删、大文件走废纸篓。Rust 编写，

macOS / Windows 双平台 CLI，适合脚本、CI/CD、定时任务与自动化工作流。

## 产品分层（2026-10-13 起）



| 产品                   | 授权模式                   | 仓库                                         | 能力                                                       |
| -------------------- | ---------------------- | ------------------------------------------ | -------------------------------------------------------- |
| **maclean CLI**（本仓库） | **永久免费**・Apache-2.0 开源 | `github.com/jamesfeng2009/maclean`         | 扫描 / 清单 / 分类 / 安全清理 / 还原 / 定时任务 / JSON 输出（独立编译，无需任何商业服务） |
| **maclean App**（GUI） | **14 天完整试用**，试用后付费解锁   | `www.getmaclean.com/download`（App 官网；仓库私有） | 可视化清理 / 空间历史 / 增长趋势 / 预测 / 智能策略 / 自动化 / 高级规则 / 本地 AI 解释  |



* 本仓库（公开）只构建、分发 **CLI**：无 GUI、无许可证服务器、无 Stripe、

  无试用状态机依赖。试用过期 / 服务器宕机 / 付款失败**均不影响 CLI**。

* CLI 与 App 共享 `maclean-core` 清理引擎，但 CLI 不共享商业授权门槛。

* Pro 命令（`history / growth / forecast / policy / automation`）在 CLI 中

  为**契约占位**（可解析、返回错误信封），实现在私有 Pro 仓库。

* 目录级许可矩阵：`docs/governance/LICENSE_BOUNDARY.md`。

## 快速开始



```
# 从源码安装
./scripts/install-cli.sh                 # 安装到 ~/.local/bin
./scripts/install-cli.sh --system        # 安装到 /usr/local/bin（需 sudo）

# 或 Homebrew（tap 已发布，无需先 brew tap）
brew install jamesfeng2009/maclean/maclean

# 预览将清理的项目（不删除任何文件）
maclean clean --tab dev-cache --dry-run

# 确认删除（脚本必须显式 --yes）
maclean clean --tab dev-cache --yes --format jsonl

# 磁盘占用分类占比
maclean check-disk --breakdown --json
```

## 构建与测试



```
cargo build --workspace       # 全量（5 个公开 crate + CLI）
cargo test --workspace        # 测试
cargo build --release         # 发布版 CLI
make package                  # CLI 发行产物（tar.gz + SHA256）
```

## 仓库结构



```
maclean/
├── crates/
│   ├── maclean-types        公开契约层（领域模型 / envelope / 错误码 / Feature）
│   ├── maclean-core         核心引擎（scanner / safety / ops / scheduler）
│   ├── maclean-storage      本地持久化（JSON-first）
│   ├── maclean-platform     平台适配契约（Trash / 路径 / 注册表 / 磁盘）
│   └── maclean-cli          CLI 契约（命令解析 / envelope 渲染 / 退出码）
├── apps/
│   └── maclean-cli          公开产品（纯 CLI 二进制，产物名 maclean）
├── schemas/  examples/  tests/  docs/  scripts/
├── Cargo.toml  LICENSE  NOTICE  README.md
```

## 命令总览



| 命令                                                                    | 能力                                 |
| --------------------------------------------------------------------- | ---------------------------------- |
| `scan [--tab <key>] [--deep]`                                         | 扫描可清理项（默认只读，可 Ctrl+C 取消）           |
| `clean [--tab <key>] [--safe-only] [--dry-run] [--yes] [--scheduled]` | 清理（默认预览；删除走与 App 同一套安全闸门）          |
| `check-disk [--breakdown]`                                            | 磁盘空间 / 分类占比总览                      |
| `list`                                                                | 列出可用扫描类别                           |
| `apps [--json]`                                                       | 列出已安装应用清单（含 PWA 标记与保护状态）           |
| `uninstall <路径\|名称> [--yes]`                                          | 卸载应用（本体 + 关联数据 + 缓存，移入废纸篓可还原；默认预览） |
| `startup list \| disable <label> \| enable <label>`                   | macOS 启动项管理（可逆禁用）                  |
| `optimize list-tasks \| run --task <id> [--yes]`                      | 系统优化 / 维护任务                        |
| `schedule [--install --days N] [--remove]`                            | 定时清理任务管理                           |
| `backups [--restorable-only]`                                         | 历史删除清单（废纸篓还原用）                     |
| `restore <id>`                                                        | 按清单还原可恢复项                          |
| `log [--tail N] [--open]`                                             | 查看日志                               |

## 平台现状



| 平台      | 状态                                                                         |
| ------- | -------------------------------------------------------------------------- |
| macOS   | 主力开发平台。构建、测试、打包（tar.gz + SHA256）在 CI 上跑通                                   |
| Windows | 可编译、可打包 exe（CI 出产物），但**未做真机冒烟** —— UAC 提权删除、还原点、废纸篓是否真进 Recycle Bin 等只能人工验 |

## 安全模型

这是会**真实删除用户文件**的程序，以下几条是硬约束，改动前先读：



* `app_protection.rs`：三级保护（Critical / RequiresOfficialUninstaller / DataProtected）。

  Critical 类应用（Apple 系、安全软件）不给删。

* `safety.rs` + `ops/mod.rs`：删除前安全预检（符号链接 / 路径穿越 / 受保护

  目录 / 挂载点 / TOCTOU 复检），与 App 完全同一层，不会因走 CLI 而绕过。

* 删除分档：缓存 → 永久删除；大文件 → 废纸篓（可恢复）。

* `clean` 默认**只预览不删除**：非交互终端未带 `--yes` 拒绝执行并返回退出码；

  失败项**不会谎报成功**（部分失败返回警告码并逐项列出原因）。

新增删除路径时，先问 "这条路径有没有可能命中用户数据"，而不是先问 "能不能删干净"。

## 脚本接口（稳定契约）



* 结构化输出：`--format human|json|jsonl`（机器格式恒无色、无进度条）。

* 稳定退出码：`0` 成功 / `1` 失败 / `2` 参数错误 / `3` 需确认 / `4` 用户取消 /

  `5` 部分成功 / `6` 序列化失败 / `7` Ctrl+C。

* 完整契约（字段、退出码、兼容规则）：`docs/cli-contract.md`。

## License

Apache-2.0。CLI **永久免费**；GUI App 提供 14 天试用，试用后付费解锁，

授权与购买流程在私有 maclean-pro 仓库实现，不在本仓库。App 下载入口：

`www.getmaclean.com/download`。

## 已知未完成



* **Windows 真机冒烟** —— UAC 提权删除、还原点创建 / 还原、废纸篓是否真进

  Recycle Bin。本机只能验编译，验不了这些。

* **Windows 侧应用保护** —— `app_protection.rs` 按 macOS bundle id 实现，

  Windows 目标上暂无等价分级保护。

## 质量门禁

CI 有三个 workflow：



* `ci.yml`（macOS）：`cargo fmt --check` → `cargo clippy --all-targets -D warnings`

  → `cargo test --workspace` → release 构建 → 版本号一致性

* `windows-ci.yml`（Windows）：`cargo check --all-targets` → release 构建出 exe + 冒烟

* `release-macos.yml`（发布）：universal binary → Developer ID 签名 → 公证 →

  tar.gz + SHA256

版本号唯一来源是 `Cargo.toml`（`apps/maclean-cli/Cargo.toml`）。