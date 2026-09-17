# maclean 项目 Review

> 审查范围：`/Users/fengyu/Downloads/myproject/workspace/maclean`
> 代码量：28 个 Rust 文件，41,701 行（其中 `src/main.rs` 8,508 行、`scanner/windows_apps.rs` 3,641 行、`scanner/dev_cache.rs` 2,614 行）
> 技术栈：egui 0.29 / eframe 0.29 + clap CLI + ed25519-dalek 离线授权，声明双端（macOS / Windows）

## 0. 一句话结论

**功能面铺得很开（9 个清理分类 + 双端 + CLI + 授权 + 菜单栏 HUD），工程底座没跟上**：一个 8508 行的 main.rs、7 个 `static mut` 全局、删除链路零测试、sudo 阶段绕过安全校验。
按危险程度排序，**P0 的四个问题都属于"会真的弄坏用户机器"或被提权，不是风格问题**。这些必须先修，UI 重做才有意义。

---

## ✅ 修复进度（2026-09-13）

已修复并通过 `cargo check` + `cargo test`（75 个用例全绿）：

| 编号 | 问题 | 修复方式 | 位置 |
|---|---|---|---|
| P0-1 | sudo 阶段绕过安全校验 + TOCTOU | 新增 `sanitize_for_sudo()`：逐项重跑 safety 校验 + 符号链接复查 + 危险字符拦截（换行/反引号/`$(`），两个 sudo 入口均在放行前调用 | `main.rs` |
| P0-2 | `/tmp` 可预测脚本提权 | `touchid.rs` 改为**内容内联进 `sh -c`，完全不落盘**；4 处 sudo 脚本改用 `write_private_temp_file()`（随机名 + `O_EXCL\|O_NOFOLLOW` + 0600） | `touchid.rs`、`main.rs` |
| P0-3 | 废纸篓失败静默降级 | `platform::move_to_trash` 失败不再 fallback `remove_dir_all`；`use_trash` 项失败后**不进 sudo 重试列表**（否则 `rm -rf` 会让它彻底不可恢复），改为保留文件 + 明确提示 | `platform/mod.rs`、`main.rs` |
| P0-4 | mutex 毒化 + 弹窗卡死 | 4 处 `.lock().unwrap()` → `unwrap_or_else(\|e\| e.into_inner())`；删除通道 `Disconnected` 时强制 `finish_delete()` 复位 `confirm` | `main.rs` |
| P1-1 | 模拟器镜像删光全部 runtime | 改用 `simctl runtime list -j` 解析，按 `mountPath`/`parentMountPath` 归属过滤 + 只取 `deletable=true`；解析失败 **fail-closed**（不再退化为全删） | `main.rs` |
| P1-2 | 挂载点 fail-open | `get_mount_points()` 改用 `getfsstat(2)` 而非解析 `mount` 文本；`is_path_mounted` 补上"路径位于挂载点之内"的反向判断 | `main.rs` |
| P1-3 | docker prune 无确认删全部卷 | 移除 `--volumes`（只 prune 镜像/容器/网络/构建缓存），并在扫描项描述中说明；需要清卷引导用户用 `docker volume prune` | `main.rs`、`dev_cache.rs` |
| P2 | Homebrew 删掉最新版 | 新增 `compare_versions()`（自然版本序 + semver 预发布规则），按版本排序后保留最大版本；跳过 `latest` 与隐藏目录 | `dev_cache.rs` |
| P2 | Time Machine 快照误标 Safe | `Recommend::Safe` → `Caution`，不再被"智能选择"默认勾选；描述补上"删除后不可恢复" | `apfs.rs` |
| P2 | 版本号三处不一致 | Makefile 与 `build-package.sh` 均改为从 `Cargo.toml` 读取；新增 CI job 校验不得再出现硬编码 | `Makefile`、`scripts/build-package.sh`、`.github/workflows/ci.yml` |
| P2 | Homebrew sha256 全 0 | 新增 `scripts/update-homebrew.sh`：从 GitHub Release 下载真实产物计算 sha256 并回填 formula；formula 改用 `on_arm`/`on_intel` 分架构 URL（原 URL 用的 `-universal.dmg` 与实际产物名不符） | `homebrew/maclean.rb`、新脚本 |
| P2 | 只做 ad-hoc 签名未公证 | `build-package.sh` 新增 `notarize_app()`：有 `MACLEAN_SIGN_IDENTITY` 时走 Developer ID 签名 + `--options runtime` + `notarytool` 公证 + `stapler`；无凭据时明确告警而非静默产出不可分发的包 | `scripts/build-package.sh` |
| P2 | 删除链路零测试 | 新增 13 个用例：sudo 二次校验（注入字符/符号链接/系统目录）、挂载点判定（含空格/根挂载）、临时文件权限与不覆盖、版本比较；全仓 `cargo test` = **75 passed / 0 failed** | `main.rs`、`scanner/mod.rs`、`scanner/dev_cache.rs` |
| P2 | CI 只有 Windows cargo check | 新增 `.github/workflows/ci.yml`：macOS 上跑 `fmt --check` + `clippy` + `cargo test` + release 构建，外加版本号一致性 job；代码已全量 `cargo fmt` | `.github/workflows/ci.yml` |

### 仍待处理（不在本次清单内，需要你拍板）

1. **`.env` 运行时后门**：`main.rs:104-127` 读 CWD 的 `.env`，`license.rs:33` 见 `MACLEAN_DEV=1` 直接返回已激活。分发包误带 `.env` 即全量绕过。
   建议改成 `cfg(debug_assertions)` 编译期关门 —— **但这会改变 `make dev-release` 的行为**，属于产品决策，没擅自改。
2. **7 个 `static mut` 全局**（`main.rs:154-161`）产生 ~76 处告警，Rust 2024 edition 下是硬错误。CI 里 clippy 因此暂时 `continue-on-error`。
   彻底修需要把状态迁入 `Mutex` / `impl eframe::App`，属结构性重构，建议单独立项。
3. **License 无 `exp` / 无吊销**：一次签发永久有效，需要服务端定期复验，属后端改造。
4. **缺 README / LICENSE / 隐私政策**。

---

## 1. P0 · 必修（会造成实际损害）

### 1.1 sudo 阶段完全绕过安全校验，且存在 TOCTOU
`src/main.rs:3918 / 3951` 只在阶段一调用 `safety::check_path_safety_with_category`；
`start_sudo_delete`（4132–4330）与 `start_sudo_delete_touchid`（5370–5772）直接拿 `failed_items` 拼 `rm -rf`，**既不重做校验，也不复查符号链接**（4074 的 symlink 检查在阶段二缺失）。
阶段一判定到阶段二执行之间路径可被替换。SIP 判定也是事后做的（4411）。
→ sudo 前逐项重跑 safety 校验 + `symlink_metadata` 复查。

### 1.2 /tmp 可预测脚本 → 本地提权
`src/touchid.rs:162` 写固定路径 `/tmp/maclean_sudo_local.tmp`，172–177 再 `sudo cp` 到 `/etc/pam.d/sudo_local`。
write 与 cp 之间可被替换 → 以 root 写入任意 PAM 配置。`main.rs:4276 / 5576` 同为 write→chmod→sudo 窗口。
→ `mktemp` 随机名 + `O_EXCL|O_NOFOLLOW`，或直接管道喂给 sudo。

### 1.3 废纸篓失败静默降级为永久删除
`src/platform/mod.rs:164-172` osascript 失败后 fallback `remove_dir_all`；`main.rs:4086-4091` 再 fallback `best_effort_delete`（`rm -rf`）。
osascript 调 Finder 需要 TCC 授权，**未授权时必然静默走永久删除**。且 `app.rs:1072` 把 Caution 定为永久删除，只有 Advanced 走废纸篓。
→ 废纸篓失败即中止并明确提示，禁止静默 rm；Caution 也应默认可恢复。

### 1.4 删除线程 mutex 毒化 → 弹窗卡死
`main.rs:3908 / 3940 / 4104 / 4111` 全用 `failed_items.lock().unwrap()`，任一 worker panic 即毒化 mutex 并连锁 panic；
tx 被 drop 后 UI 在 `main.rs:510` 的 `Err(_) => break` 直接退出轮询，**没有把 `confirm` 复位为 `None`**，弹窗永久卡在 Deleting 态。
→ `unwrap_or_else(|e| e.into_inner())`；`Disconnected` 时强制 `finish_delete()`。

---

## 2. P1 · 高危 / 明显 bug

| # | 问题 | 证据 | 建议 |
|---|---|---|---|
| 2.1 | 模拟器镜像删除范围失控：勾选一项会删光所有运行时 | `main.rs:3689` 只校验入参，3707–3719 提取 `xcrun simctl runtime list` 的**全部** UUID 逐个删；sudo 脚本 4169 同样 | 按 path 反查对应 UUID，只删目标 |
| 2.2 | 挂载点检测 fail-open | `main.rs:3604-3606` 按空白切分取 `parts[2]`，挂载点含空格即被截断 → 误判未挂载放行 | 改用 `statfs`/`getfsstat`，勿解析文本 |
| 2.3 | Docker `prune -a --volumes` 无确认即删全部未用卷 | `main.rs:3786` | 执行前列卷 + 独立确认 |
| 2.4 | Homebrew「旧版本」判定用 `read_dir` 顺序，未排序 → 可能删掉最新版 | `scanner/dev_cache.rs:620-631` | 按版本号排序后保留最新 |
| 2.5 | Time Machine 快照标为 `Safe` 且 `deletable=true`，会被"智能选择"默认勾选，size 显示 0 用户无法判断 | `scanner/apfs.rs:90` + `app.rs:597` + `scanner/mod.rs:68` | 降为 `Caution` |
| 2.6 | Application Support 下 4 层内命中 `T/Temp/sentry/logs` 等通用名即判 Safe | `scanner/app_cache.rs:249-334, 411` | 加 App 白名单或降 Caution |
| 2.7 | 安装包"双向包含"模糊匹配，Chrome-1.zip 可能误判已安装而被自动删 | `scanner/dev_cache.rs:1426, 1322-1330` | 改 Caution |
| 2.8 | 卸载残留前缀模糊匹配，一旦判为未安装就 `deletable=true` | `scanner/uninstall.rs:1486-1495, 1258` | 提高阈值 + 二次确认 |
| 2.9 | 回滚能力基本不存在：safety.rs 只写日志，macOS 侧零备份；Windows 还原点也不覆盖文件删除 | `safety.rs:769-796`、`main.rs:3831-3839` | 至少 Advanced/Caution 走废纸篓或硬链接快照 |
| 2.10 | sudo 密码长期以 `String` 存于内存（`app.rs:195`、线程 4134），无 zeroize；未拒绝含换行的密码 | `main.rs:4202`、`touchid.rs:131`、`sudo_keepalive.rs:53` | zeroize + 校验换行 |
| 2.11 | keepalive 每 25s `sudo -n -v`，无总时长上限；`static mut`（17 行裸 unsafe）；`end_sudo_session` 全库未见调用 | `sudo_keepalive.rs:72-95` | 加会话上限 + 退出时吊销票据 |

---

## 3. P2 · 工程质量与架构

**架构**
- `main.rs` 8508 行同时装了 UI 渲染（`render_gui` 2103）、删除业务（`start_delete` 3812 / `start_sudo_delete` 4132，各 300+ 行）、主题常量（36–61）。→ 拆 `ui/` + `ops/delete.rs` + `theme.rs`。
- 7 个 `static mut` 全局（`main.rs:154-161`），全文件 10 处 `unsafe`。→ 改 `impl eframe::App`，状态进结构体。
- `App` 结构体 **64 个 pub 字段**（`app.rs:148-282`），混杂扫描态 / UI 态 / sudo / TouchID / License / 设置态。→ 拆子结构体。
- 设置字段双份存：`App.settings_*`（`app.rs:242-248`）与 `config::AppConfig`（`config.rs:40-51`）靠 `app.rs:1245-1248` 手工同步。→ 单一来源。
- `dir_size` 三份实现、跳过规则互相不一致（`scanner/mod.rs:149` / `large_files.rs:211` / `dev_cache.rs:1133`），其中一份**不跳过 .photoslibrary**。→ 统一 + rayon。
- `format_size` 逐字重复三遍（`mod.rs:128` / `dev_cache.rs:1518` / `uninstall.rs:1195`）。

**性能**
- `dev_cache.rs` 完全没用 rayon（全文件无 `par_iter`）。
- `scan_build_artifacts` 对 8 个目录名各做一次全树遍历（`dev_cache.rs:1169-1245`），加 target/node_modules/build/__pycache__ 约 12 次重复遍历同一工作区。→ 一次 walk 收集 + `par_iter` 算大小。
- 扫描确实在后台线程（GUI 未阻塞，这点是对的）。

**测试与 CI**
- 零 `tests/` 目录；`main.rs`、`app.rs`、`config.rs`、`cli.rs`、`menubar.rs` **完全无测试** —— 恰恰是最危险的删除链路与状态机没覆盖。
- `.github/workflows/windows-ci.yml` 只跑 `cargo check` + `build`，**无 clippy / fmt / test / macOS job**。
- Makefile 声明 `.PHONY: test` 但**没有 test 目标**。

**版本与分发**
- 版本号三处不一致：`Cargo.toml:3` = 0.2.0、`Makefile:5` = 0.1.0、`scripts/build-package.sh:12` = 0.2.0；且 `build-package.sh:210` 硬编码 `maclean-0.2.0.pkg` 没用 `$VERSION`。
- `homebrew/maclean.rb:3-4` 的 sha256 是**全 0 占位**，安装必失败。
- `build-package.sh:174` 只做 `codesign --sign -`（ad-hoc），**未公证**，Gatekeeper 必拦截。
- `updater.rs:34` 用 curl 查 GitHub API，只提示跳转、不下载不验签。
- 缺 README / LICENSE / 隐私政策。

**"双端"的真实程度**
Windows 侧非空壳（`windows_apps.rs` 3641 行，真实注册表 + UWP + 卸载执行），但**不对称**：
`optimize.rs:75` 的 `windows_optimize_tasks()` 因 `mod.rs` 的 `cfg(target_os="macos")` 成了死代码，Windows 上「系统优化」Tab 返回空；
macOS 6 个专属扫描器 vs Windows 3 个；`cache_registry.rs:378` 用字符串 `"N/A"` 标记平台不适用，脆弱。
→ 要么补齐，要么在 Windows 上隐藏该 Tab，**不要留空页面**。

**授权与合规**
- 好消息：`.env` 未入库（`.gitignore:19` 已忽略），无硬编码私钥 / Token。
- 但 `.env` 是运行时后门：`main.rs:104-127` 读 CWD 的 `.env`，`license.rs:33` 见 `MACLEAN_DEV=1` 直接返回已激活。分发包误带 `.env` 即全量绕过 → 改用 `cfg(debug_assertions)` 或 cargo feature 编译期关门。
- License 纯本地 ed25519，payload 的 `iat`/`plan` 未参与过期判断，**一次签发永久有效**，无吊销机制 → 加 `exp` + 定期复验。

---

## 4. 做得对的地方

- 扫描全部在后台线程 + mpsc 分批回传（`send_items_in_batches`），GUI 不阻塞——这是最容易做错的一点，做对了。
- `Recommend` 四档分级 + `default_selected()` 的模型设计是对的，把"风险"变成了数据而不是弹窗文案。
- 删除前有 `deletable` / `undeletable_reason` 预检（`app.rs:precheck_deletability`），并区分 SIP / 权限 / 占用三种失败原因。
- `safety.rs` 有删除日志，`platform/windows_backup.rs` 有还原点，Windows 侧不是敷衍的。
- i18n 中英双语 + CLI 与 GUI 共用扫描器（`cli.rs` 复用同一批 `Scanner`），分层是清楚的。

---

## 5. 建议的修复顺序

1. **先修 P0 四项**（1.4 弹窗卡死 → 1.1 sudo 校验 → 1.2 /tmp 提权 → 1.3 静默 rm），这四项加起来不到一天，但直接决定"这个工具会不会砸用户机器"。
2. **给删除链路补测试**：`start_delete` / `quota_gate_for` / safety 校验各写 3–5 个 case，CI 上跑 `cargo test` + `clippy -D warnings` + `fmt --check`。
3. **拆 main.rs**（`ui/` + `ops/` + `theme.rs`），顺手消掉 `static mut`、统一控件尺寸、把 emoji 导航换成线性图标。
4. **再动 UI**：按 `maclean-ui-design-v2.html` 实施，先做 theme.rs 与列表页复用，再做弹窗与状态页。
5. **发布链路**：单一版本源、Homebrew 补 sha256、公证、补 README/LICENSE。

## 6. 配套产出

- `maclean-ui-design-v2.html` — UI 设计稿 v2（12 屏 + 8 类弹窗 + HUD + 设计系统 + egui 落地清单），已按 1:1 逻辑像素绘制，可直接对着改代码。
