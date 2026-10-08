# maclean

**Open Core · Free Edition · Storage Intelligence Engine**

maclean 是面向开发者的磁盘清理与存储智能引擎：扫描结果会标注**删了会怎样**
（要重新下载 / 要重新编译 / 含用户数据需确认），删除前对受保护应用单独拦一道，
缓存永久删、大文件走废纸篓。Rust + egui/eframe，macOS / Windows 双平台 GUI + CLI。

## 开源与商业边界（Open Core）

| 版本 | 仓库 | 许可 | 能力 |
|---|---|---|---|
| **maclean Free**（本仓库） | `github.com/jamesfeng2009/maclean` | Apache-2.0 | 扫描 / 清单 / 分类 / 安全 / 基础清理 / 还原（独立编译，无需任何商业服务） |
| **maclean Pro**（私有） | `github.com/jamesfeng2009/maclean-pro` | Proprietary | 存储智能 / 完整历史 / 预测 / 智能策略 / 定时自动化 / 高级规则 / 本地 AI 解释 / Pro Dashboard |

- 本仓库（公开）可独立构建：`cargo build` / `cargo test` / `cargo run`；
  不依赖 Stripe、license-server、maclean-pro、Team Server。
- Pro 命令（`history / growth / forecast / policy / automation`）在 CLI 中
  为契约占位，实现在私有仓库。
- 目录级许可矩阵：`docs/governance/LICENSE_BOUNDARY.md`。

## 构建

```bash
cargo build --workspace                    # 全量
cargo test --workspace                     # 测试
cargo build --release -p maclean-free      # egui GUI
cargo build --release --no-default-features -p maclean-free   # 纯 CLI
cd tauri && npm install && npm run tauri build                # Tauri dmg
```

## 仓库结构

```text
maclean/
├── crates/   types · core(scanner/inventory/classification/cleanup/safety/restore) · storage · platform · cli
├── apps/     maclean-free（egui 壳 + CLI 入口）
├── tauri/    Tauri 2 壳（独立 workspace 成员）
├── schemas/  JSON / migrations
├── examples/ tests/  docs/  scripts/
├── Cargo.toml  LICENSE  NOTICE  README.md
```

---

# maclean

面向开发者的磁盘清理工具。Rust + egui/eframe，macOS / Windows 双平台 GUI + CLI。

不是"扫一遍报个数字"的玩具：扫描结果会标注**删了会怎样**（要重新下载 / 要重新编译 /
含用户数据需确认），删除前对受保护应用单独拦一道，缓存永久删、大文件走废纸篓。

---

## 平台现状

| 平台 | 状态 |
|---|---|
| macOS | 主力开发平台。构建、测试、打包（.pkg / .dmg）都在 CI 上跑通 |
| Windows | 可编译、可打包 exe（CI 出产物），但**未做真机冒烟** —— UAC 提权删除、还原点、废纸篓是否真进 Recycle Bin 等只能人工验 |

Windows 上还有一条运行期前置条件：manifest 里声明了 `longPathAware`，但 Windows 默认
`MAX_PATH = 260` 还受注册表 `LongPathsEnabled` 控制。扫 `node_modules` 这类深层目录前，
建议让用户在 Windows 10 1607+ 上开启：

```powershell
New-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem" `
  -Name "LongPathsEnabled" -Value 1 -PropertyType DWORD -Force
```

不开不会崩，但超长路径的目录可能扫不出来——症状是"这个缓存明明很大却没被统计到"。

---

## 功能

**扫描** —— 60+ 类别：

- `cache_registry.rs`：38 个固定路径缓存（Ruby / PHP / Dart / Swift / .NET / Elixir /
  Haskell / Nix / Julia / Unity / Unreal / Terraform / Electron …）
- `dev_cache.rs`：~15 个复杂扫描器（Xcode / Docker / AI 模型缓存等）
- `app_cache.rs`：应用缓存、应用组缓存、日志
- `apfs.rs`：Time Machine 本地快照、iOS 模拟器运行时
- `large_files.rs` / `uninstall.rs` / `residual_match.rs`：大文件、卸载残留
- `windows_apps.rs`：Windows 侧注册表卸载项

**清理** —— 按推荐等级分档，缓存永久删除、大文件移入废纸篓。

**提权** —— macOS 上支持密码提权与 Touch ID 提权（写 sudo_local 启用
`pam_tid.so`）。Touch ID 的启用路径是：提示 → 输入密码 → 写 sudo_local →
Touch ID 删除。早期还有一套"开 Terminal 让用户自己授权、应用轮询等待"的设计，
2026-09-19 已删除：它从未被接线，而密码路径已经能完成同样的事。

**菜单栏常驻** —— 托盘显示磁盘占用，60 秒周期刷新。

**界面国际化** —— 中 / 英切换。缓存类目的英文文案唯一来源是
`CacheDef::desc_en`（`i18n::registry_description_en` 按中文反查取用）；
`i18n.rs` 只保留注册表之外的文案。**不要**把注册表条目的英文再抄一份进 `i18n.rs`
—— 之前抄过，两份漂出了措辞差异，现在有测试拦着。

---

---

## 命令行（CLI）

GUI 与 CLI 是**同一个二进制**：不带参数启动 GUI，带子命令走 CLI。
脚本化场景（CI / 定时任务 / 监控）请使用结构化输出（`--json` / `--format jsonl`）
与语义退出码。

### 安装

```bash
./scripts/install-cli.sh          # cargo build --release 后安装到 ~/.local/bin
./scripts/install-cli.sh --system # 安装到 /usr/local/bin（需 sudo）
./scripts/install-cli.sh --cli-only # 纯 CLI 二进制（--no-default-features，不链接 GUI 库）
```

纯 CLI 构建场景：服务器 / CI / 容器只需要 `maclean clean --yes` 这类命令时，
用 `--cli-only` 安装，二进制不携带任何窗口系统依赖（体积更小、供应链更干净）。
裁剪只移除 GUI 壳，扫描 / 删除闸门 / 备份 / 启动项 / 优化 / 调度 / 提权能力全部保留。

### 命令总览

| 命令 | 能力 | 对应 Tab |
|---|---|---|
| `scan [--tab <key>] [--deep]` | 扫描可清理项（含进度、可 Ctrl+C 取消） | 全部 |
| `clean [--tab <key>] [--safe-only] [--dry-run] [--yes] [--scheduled]` | 删除（默认预览；删除走与 GUI 同一套安全闸门） | 全部 |
| `check-disk [--breakdown]` | 磁盘空间 / 分类占比总览 | 磁盘分析 |
| `list` | 列出可用扫描类别 | - |
| `apps [--json]` | 列出已安装应用清单（含 Chrome/Safari 安装的 PWA 标记与保护状态） | 应用卸载 |
| `uninstall <路径\|名称> [--yes]` | 一键卸载应用（本体+关联数据+缓存，移入废纸篓可还原；默认预览） | 应用卸载 |
| `startup list \| disable <label> \| enable <label>` | macOS 启动项管理（可逆禁用） | 启动项 |
| `optimize list-tasks \| run --task <id> [--yes]` | 系统优化/维护任务 | 系统优化 |
| `schedule [--install --days N] [--remove]` | 定时清理任务管理 | 设置 |
| `backups [--restorable-only]` | 列出历史删除清单（废纸篓还原用） | 设置 |
| `restore <id>` | 按清单还原可恢复项 | 设置 |
| `log [--tail N] [--open]` | 查看日志（刻意保持人类可读，不做 JSON） | - |

### 安全确认（P0）

`clean` 默认**只预览不删除**：

- 交互终端：直接运行 `clean` 输出将删清单，不带 `--yes` 不执行。
- 非交互终端（管道 / 脚本）：未带 `--yes` 时拒绝执行并返回退出码 `4`。
- 定时任务 `--scheduled` 视为用户在 GUI 配置时已确认，无人值守执行。
- 删除前会复做 `sanitize_before_delete`（TOCTOU 防护：符号链接复查、受保护
  路径拦截、占用处理），与 GUI 完全同一层，不会因走 CLI 而绕过。
- 权限不足的项：默认报告失败（退出码 8）并提示；加 `--privileged` 自动提权
  ——交互终端下会**先逐项列出待提权路径，再弹出系统授权（Touch ID / 密码）**，
  用户取消则计入失败；非交互环境不弹窗，直接如实报告（保证脚本确定性）。

### 退出码契约（脚本消费）

| 码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | 通用失败（找不到项、磁盘信息不可读等） |
| 2 | JSON 序列化失败 |
| 4 | 需要确认（非 TTY 下 `clean` / `uninstall` 未带 `--yes`；`optimize run` 非低风险任务未带 `--yes`） |
| 7 | Ctrl+C 取消 |
| 8 | `clean` / `restore` 有失败项或被安全拦截项（带警告完成） |

### 输出格式

- 全局参数 `--json`（兼容旧脚本，等价于 `--format json`）、`--format human|json|jsonl`、
  `--color auto|always|never`、`--no-progress`。
- 机器可读格式下自动禁用颜色（同时尊重 `NO_COLOR`）。
- `jsonl`：`scan` 每 Tab 一行、`clean` 每项一个 `deleted`/`failed` 事件，适合流式处理。

### 示例

```bash
# 预览要删什么（不删任何东西）
maclean clean --tab dev-cache

# 确认删除（脚本里必须显式 --yes）
maclean clean --tab dev-cache --yes --format jsonl

# 权限失败自动提权（交互终端：先列出待提权项，再弹 Touch ID）
maclean clean --tab dev-cache --yes --privileged

# 磁盘占用分类占比
maclean check-disk --breakdown --json

# 启动项可逆禁用/恢复
maclean startup list
maclean startup disable com.example.agent
maclean startup enable  com.example.agent

# 系统维护任务
maclean optimize list-tasks
maclean optimize run --task dns_cache_flush --yes
```

---

## 安全模型

这是会**真实删除用户文件**的程序，以下几条是硬约束，改动前先读：

- `app_protection.rs`：三级保护（Critical / RequiresOfficialUninstaller / DataProtected）。
  Critical 类应用（Apple 系、安全软件）不给删。
- `safety.rs`：删除前的安全预检。
- 删除分档：缓存 → 永久删除；大文件 → 废纸篓（可恢复）。
- CLI 删除前强制校验；Windows 侧删除出口有独立闸门。

新增删除路径时，先问"这条路径有没有可能命中用户数据"，而不是先问"能不能删干净"。

---

## 构建与开发

```bash
make build      # debug
make release    # release
make run        # 运行
make test       # 单元测试
make lint       # clippy -D warnings（CI 阻塞门禁）
make fmt        # 格式检查
make package    # 打包 .pkg / .dmg
```

交叉编译验证（macOS 上验 Windows 代码路径，CI 也这么做）：

```bash
cargo check --target x86_64-pc-windows-msvc --all-targets
```

`--all-targets` 不能省 —— 少了它，`#[cfg(test)]` 里的 Windows 分支根本不编译，
测试代码烂掉也发现不了（历史上因此藏了 13 个编译错误）。

### 关于 Windows 资源嵌入

`build.rs` 会把 `assets/icon/maclean.ico` 与 `assets/windows.manifest`
（图标 / DPI 感知 / 长路径 / UAC 级别 / 版本信息）编进 exe。

- Windows 原生构建：资源编译失败会直接 panic，CI 拦得住。
- 从 macOS 交叉编译：没有资源编译器时**不会**嵌入，只在构建日志留一条
  `cargo:warning`。交叉编译产物可以验编译，**不能用来发布**。
- 想在本机也真的嵌入，装 `mingw-w64` 后指定编译器即可：

```bash
RC_x86_64_pc_windows_msvc=/opt/homebrew/bin/x86_64-w64-mingw32-windres \
  cargo build --target x86_64-pc-windows-msvc
```

---

## License

双 Ed25519 密钥：私钥只在签发端，公钥内置在 `src/license.rs`。

```bash
cargo run --bin maclean-keygen -- init    # 生成密钥对，公钥粘进 license.rs
cargo run --bin maclean-keygen -- sign --email a@b.com --plan lifetime
cargo run --bin maclean-keygen -- sign --email a@b.com --plan yearly   # 自动 iat + 365d
cargo run --bin maclean-keygen -- sign --email a@b.com --days 30
```

Payload 结构：`email` / `plan` / `iat` / `mid`（机器指纹）/ `v`（schema 版本）/
`exp`（0 = 永不过期）/ `jti`（key id）。老版本 key 没有后三个字段，靠
`#[serde(default)]` 兼容。

**每把 key 的 `jti` 必须记进销售台账** —— 将来要吊销（退款、漏发）时，
吊销表只能按 `jti` 匹配，没有别的东西能定位一把 key。

过期判定走单调时钟（`max(系统时间, 上次观测)`）：用户把系统日期改回过去
不会让 `exp` 变远，堵掉"改日期续命"。

> `dev-mode` feature 会跳过全部 License / 额度限制。**发布构建（CI、打包脚本、
> Homebrew）一律不得启用**，否则打出的包人人免费用。

---

## 已知未完成

按优先级排在前面的：

- **Windows 真机冒烟** —— UAC 提权删除、还原点创建/还原、废纸篓是否真进
  Recycle Bin、深色模式跟随。本机只能验编译，验不了这些。
- **Windows 侧应用保护** —— `app_protection.rs` 是按 macOS bundle id 做的，
  在 Windows 目标上整块是死代码。Windows 目前没有等价的分级保护。

---

## 质量门禁

CI 有两个 workflow：

- `ci.yml`（macOS）：`cargo fmt --check` → `cargo clippy --all-targets -D warnings`
  → `cargo test` → release 构建 → 版本号一致性（Makefile / 打包脚本不得硬编码版本）
- `windows-ci.yml`（Windows）：`cargo check --all-targets` → release 构建出 exe

版本号唯一来源是 `Cargo.toml`。`build.rs` 里写进 exe 的 VERSIONINFO 也从
`CARGO_PKG_VERSION` 取，不手写第二份。
