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

**菜单栏常驻** —— 托盘显示磁盘占用，60 秒周期刷新。

**界面国际化** —— 中 / 英切换。缓存类目的英文文案唯一来源是
`CacheDef::desc_en`（`i18n::registry_description_en` 按中文反查取用）；
`i18n.rs` 只保留注册表之外的文案。**不要**把注册表条目的英文再抄一份进 `i18n.rs`
—— 之前抄过，两份漂出了措辞差异，现在有测试拦着。

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
- **Touch ID 提权流程** —— `ConfirmState::OfferTouchIdSetup` 目前从未被置位，
  Touch ID 启用这条路走不通（clippy 清死代码时发现的）。
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
