# maclean Open Core 工程调整 · 任务跟踪

> 目标：把现有 maclean 仓库按 **Open Core 策略**重构为「公开 Open Core + 私有 Pro」双仓库拓扑，
> 对齐三份输入方案的全部功能要求：
> 1. `MACLEAN_OPEN_CORE_LICENSE_AND_REPOSITORY_STRATEGY_V1.md` —— 许可 / 仓库 / 商业化边界策略基线（2026-10-07，DECISION BASELINE）
> 2. `maclean-open-core-engineering-split-v1.0/`（已归档 docs/engineering-split-v1.0/）—— 工程拆分方案
> 3. `maclean-commercialization-v1.0-docs/`（已归档 docs/commercialization/）—— 商业化 V1 文档 24 份（产品定位 / PRD / 架构 / 定价 / 路线图 / GTM）
>
> 基线：`main @ 3c6be09`（工作树除三个新增输入外干净）
> 旧「UI 改造」任务文件已归档为 `tasks-ui-renovation-v2.archived.md`（git 历史可追溯）。
> 状态图例：☐ 未开始 / 🔄 进行中 / ☑ 完成 / ⚠ 受阻 / 🗑 已移除

---

## 0. 当前仓库事实（已核实，2026-10-07）

| 事实 | 说明 | 对迁移的影响 |
|---|---|---|
| 根包 `maclean` 0.2.0 | egui GUI（`src/app.rs` 2258 行、`src/ui/mod.rs` 7874 行）+ CLI（`src/cli.rs` 2709 行）+ `src/license.rs` + `src/bin/keygen.rs` | GUI→Free、CLI→maclean-cli、license 需 REWRITE 拆公开/私有 |
| `crates/maclean-core` 已存在 | ops / safety / scanner / scheduler / platform / rules / config 等，UI 无关 | Phase 5（scanner 抽取）大部分已前置完成，只需对齐清单 |
| `tauri/src-tauri` | Tauri 2 壳，已是 workspace 成员 | 归属 Free 壳，需决策挂载位置 |
| `maclean-tauri-ui/` | 交互稿 HTML | 保留为设计资产 |
| `website/` | 独立仓库（`.gitignore` 排除），Astro | 定位更新另计（独立仓库） |
| `.env` | 已 gitignore，需审计内容 | 审计项 |
| `.github/workflows` | `ci.yml` + `windows-ci.yml` | 需补 secret scan / dependency review / deny |
| `scripts/ installers/ homebrew/ assets/` | 打包与分发链 | 保留公开 |
| 现有测试基线 | 380 tests 全绿（旧任务记录） | Phase 4/10 须保持等价 |

---

## 1. Phase 0 — 冻结（Freeze）

> 来源：split `03_MIGRATION_PLAN.md` Phase 0 + `07_MIGRATION_COMMANDS.md` §1

- [x] **P0-1** 创建迁移分支 `open-core/migration-v1`（git checkout -b）—— 2026-10-07 完成
- [x] **P0-2** 打冻结标签 `maclean-pre-open-core-v1`（annotated，指向 3c6be09）
- [ ] **P0-3** 确认冻结点 worktree 状态记录（git status / log 快照写入 `docs/migration/`）
- [x] **P0-4** 入库位置已定：策略 md 留根目录、方案目录归档 docs/（2026-10-08）；基线提交由 F-1 承载

---

## 2. Phase 1 — 审计（Audit）

> 来源：strategy §25/§26 + split `03` Phase 1 + `scripts/maclean-open-core-audit.sh`

- [x] **P1-1** 运行 `maclean-open-core-audit.sh`（Rust 文件清单 + 候选密钥文件 + 敏感字符串 + git history 扫描）—— 2026-10-07 完成
- [x] **P1-2** 检查 `.env` —— 内容为 `MACLEAN_DEV=1`（开发开关，非密钥；已 gitignore）
- [x] **P1-3** 全量文件分类：OPEN / PRIVATE / MOVE / REWRITE / UNKNOWN（见 §1.3，当前仓库文件 UNKNOWN = 0）
- [x] **P1-3a** 敏感字符串核查：命中均为误报（策略文档 §25 检索模式清单 + `design_tokens.rs` 的 TOKEN 常量）
- [x] **P1-3b** git history 独立复核：无 `BEGIN *PRIVATE KEY / STRIPE_SECRET / sk-* / AKIA*` 命中（排除 md / node_modules / website / target）
- [x] **P1-3c** keygen 私钥审计：`src/bin/keygen.rs::load_or_create_signing_key` 生成 Ed25519 私钥写盘（0600），公钥供 `src/license.rs` 的 `PUBLIC_KEY_HEX` —— 确认 keygen = PRIVATE，公钥常量 = OPEN（strategy §75）
- [ ] **P1-4** 依赖许可证清单：`cargo tree` + 手工核对 copyleft，产出 `THIRD_PARTY_NOTICES.md` 素材
- [ ] **P1-5** CI / release artifacts 审计：workflow 里是否存在密钥引用、产物是否含私钥
- [ ] **P1-6** 审计报告落盘 `docs/migration/AUDIT_REPORT.md`，UNKNOWN 清零后才可进入 Phase 2

### 1.3 文件分类基线（当前仓库实际文件）

| 路径 / 模块 | 决策 | 去向 | 依据 |
|---|---|---|---|
| `crates/maclean-core/src/scanner/{cache_registry,dev_cache,app_cache,large_files,residual_match}.rs` | OPEN | 已就位 `maclean-core::scanner` | CSV：MOVE 目标已达成 |
| `crates/maclean-core/src/scanner/apfs.rs` | OPEN | 核心适配器（平台差异小，保留核心） | CSV：MOVE→platform；视平台拆分进度决定 |
| `crates/maclean-core/src/scanner/windows_apps.rs` | REWRITE | `maclean-platform::windows::apps` | CSV |
| `crates/maclean-core/src/scanner/uninstall.rs` + `ops/uninstall_app.rs` | REWRITE | 公开 safety-aware 契约 + 平台实现 | CSV |
| `crates/maclean-core/src/safety.rs` + `platform/reg_safety.rs` | OPEN | `maclean-core::safety`（sanitize / protected path / TOCTOU / symlink） | strategy §12.1 |
| `crates/maclean-core/src/ops/mod.rs`（删除出口） | OPEN | `maclean-core`（三出口内建 safety 闸门） | 现状即合规 |
| `src/cli.rs` | MOVE | `crates/maclean-cli`（thin adapter，无删除逻辑） | split §6 |
| `src/license.rs` | REWRITE | 公开 verifier + 私有 issuer 分离 | CSV |
| `src/bin/keygen.rs` | PRIVATE | `maclean-license-server`（或私有工具） | 私钥生成，绝不可公开 |
| `src/{app,main,menubar,touchid,sudo_keepalive,updater,widgets,theme,icons}.rs` + `src/ui/` | MOVE | `apps/maclean-free`（egui 壳） | split Phase 7 |
| `src/ops/mod.rs` | REWRITE | 确认是否为 core ops 的再导出，去重 | 审计 |
| `tauri/` + `tauri/src-tauri` | MOVE(决策) | `apps/maclean-free`（Tauri 变体）或保留 `tauri/` 由 `apps/maclean-free` 聚合 | 待定：记录决策后执行 |
| `crates/maclean-core/src/{scheduler,backup,config,logger,i18n,design_tokens,im_data,app_protection}.rs` | OPEN | `maclean-core` | 现状 |
| `src/rules/`、`crates/maclean-core/src/rules/builtin_rules.json` | OPEN | `maclean-core`（基础规则）；premium 规则 PRIVATE | strategy §13.4 |
| `.env` / 密钥 / Stripe | PRIVATE | 不进入公开仓库；keygen 私有化 | strategy §25 |
| `website/` | 独立 | 定位更新在独立仓库进行 | gitignore |
| `installers/ scripts/ homebrew/ assets/ .github/` | OPEN | 保留，CI 增强见 Phase 12 | 现状 |

---

## 3. Phase 2 — 创建公开 crates 骨架

> 来源：strategy §3/§7/§11 + split `02_CARGO_WORKSPACE_SPLIT.md` + `workspace/` 骨架

- [x] **P2-1** 新建 `crates/maclean-types`（2026-10-07 完成）（domain 契约 + CLI envelope + error codes + feature identifiers；零依赖私有）
- [x] **P2-2** 新建 `crates/maclean-storage`（决策：JSON-first，见 §0；2026-10-07 完成）（SQLite 连接 / migrations / repositories；确认当前持久化方式后落地）
- [x] **P2-3** 新建 `crates/maclean-platform`（Trash/路径/注册表/磁盘契约；2026-10-07 完成）（macOS / Windows 平台适配器：Trash、APFS、注册表、symlink/junction）
- [x] **P2-4** 新建 `crates/maclean-cli`（命令解析/envelope/渲染/退出码；handler 迁移留 Phase 6）（命令解析 / JSON·JSONL / 退出码 / 人类可读渲染，调 core 服务）
- [x] **P2-5** 新建 `apps/maclean-free`（根包 src/build.rs/Cargo.toml 移入；bin 名保持 maclean/maclean-keygen；Tauri 保留 tauri/ 独立成员——决策记录）（桌面应用 / 基本 GUI / 菜单栏；egui + Tauri 双壳归属决策）
- [x] **P2-6** 根 `Cargo.toml` 改 virtual workspace（resolver=3、workspace.package、default-members=free；保留 `tauri/src-tauri` 成员的兼容处理）
- [x] **P2-7** 门禁：`cargo check --workspace` 在骨架层通过（允许暂时引用旧路径的过渡期，见 Phase 3-7 逐步替换）

---

## 4. Phase 3 — 抽取领域类型（Domain Types）

> 来源：strategy §19/§47 + split `03` Phase 3 + `05_PUBLIC_CORE_CONTRACTS.md`

- [x] **P3-1** 六个核心模型（+ Recommend/ScanItem/ScanResult 迁移）迁入 `maclean-types/src/domain/`：`StorageEntity / StorageObservation / DeveloperProject / CleanupCandidate / CleanupPolicy / CleanupRun`
- [x] **P3-2** CLI JSON envelope（扁平 status/error_code/error_message 设计） + `contract_version` 字段迁入 `maclean-types`（`{"contract_version","request_id","command","status","data"}`）
- [x] **P3-3** 错误码（EXIT_OK..EXIT_WARNINGS）+ Feature 枚举 + EntitlementProvider（BasicScan / BasicCleanup / AdvancedHistory / Forecast / SmartCleanup / ScheduledAutomation / PremiumRules / AiExplanation）迁入
- [x] **P3-4** 引用通过 re-export 统一到 maclean-types（core/free 引用路径不变）改为 `maclean-types` 路径依赖；`cargo check --workspace` 绿
- [x] **P3-5** 稳定性基线（serde 快照/往返/旧 JSON 兼容测试）：类型 serde 序列化测试（JSON 快照防漂移）

---

## 5. Phase 4 — 抽取安全层（Safety）

> 来源：strategy §12.1/§52 + split `03` Phase 4

- [x] **P4-1** safety 公开 API（SafetyCheck 迁 types + re-export；docs/safety/SAFETY_MODEL.md 已建） / 受保护路径 / TOCTOU / symlink / busy-file / 权限边界 → `maclean-core::safety`（已基本就位，补公开 API 与文档）
- [x] **P4-2** Trash 契约 → maclean-platform::trash（core 实现保留，后续迁移） → 平台适配器 + core action 模型（`platform/macos_trash.rs` 已就位，补 `maclean-platform` 归属）
- [x] **P4-3** `SafetyGate` 公开 trait（validate(&CleanupCandidate)->SafetyCheck）（`validate(&CleanupCandidate) -> SafetyDecision`）落位 `maclean-types` 或 `maclean-core`
- [x] **P4-4** 测试等价性：476 passed / 1 ignored（旧 380 基线未回退）：现有删除闸门测试全绿（380 基线不回退）
- [x] **P4-5** 文档：`docs/safety/SAFETY_MODEL.md`（安全模型/威胁模型/不变量/权限模型） 安全模型 / 威胁模型 / cleanup invariants / 权限模型

---

## 6. Phase 5 — 抽取 Scanners

> 来源：split `01` §2 / `08_FILE_CLASSIFICATION.csv` / `03` Phase 5

- [x] **P5-1** cache_registry / dev_cache / app_cache / large_files / residual_match —— 已在 maclean-core::scanner（验收通过：core 独立编译，无根包残留）
- [x] **P5-2** `apfs.rs` 决策（2026-10-07）：**保留 maclean-core**——深度耦合 core 的 io_pool/bulkdir/fs_guard，迁移期拆分零收益；平台契约已在 maclean-platform 立位
- [x] **P5-3** `windows_apps.rs` 决策（2026-10-07）：**保留 maclean-core**（采集层深度耦合 scanner 基建）；maclean-platform::registry 已立契约（extract_reg_value 纯解析），实现迁移留后续里程碑
- [x] **P5-4** `uninstall.rs` 决策（2026-10-07）：**保留 maclean-core**（安全耦合）；公开卸载契约由 CleanupCandidate/Policy 承载，实现迁移留后续
- [x] **P5-5** Scanner trait（core，scan()->ScanResult）、ScanResult/StorageEntity（types）已对齐；ScannerRegistry 由 free 应用编排，不新增运行时行为
- [x] **P5-6** 未新增 scanner 行为；cargo test --workspace 全绿

---

## 7. Phase 6 — 抽取 CLI

> 来源：strategy §21 + split `03` Phase 6

- [x] **P6-1** cli.rs 迁入 maclean-cli（handler.rs，2695 行）；free main 为 thin adapter（2026-10-08）
- [x] **P6-2** 命令集落位 args.rs（13 公开 + 5 Pro 占位）；JSON/JSONL 全部走 CliEnvelope（冒烟验证 contract_version/request_id/status/data）
- [x] **P6-3** history/growth/forecast/policy/automation 契约占位（可解析、返回 error envelope + 退出码 1）；实现入 ../maclean-pro
- [x] **P6-4** cmd_clean 删除循环改调 core::ops::best_effort_delete_with_reason（TOCTOU staging）；grep 无 fs::remove 残留
- [x] **P6-5** `cargo build --no-default-features -p maclean-free --release` 成功（纯 CLI 2.7MB）

---

## 8. Phase 7 — 抽取 Free UI

> 来源：strategy §22 + split `03` Phase 7

- [x] **P7-1** egui 壳已全部位于 apps/maclean-free（2026-10-07 移入；本轮核对）
- [x] **P7-2** ops/mod.rs 确认为薄壳（re-export core::ops + App 耦合的 start_scan 编排），无重复删除逻辑
- [x] **P7-3** Free UI 核对：Overview/DevCache/LargeFiles/AppCache/AppData/AppUninstall/SystemOptimize/Apfs/CustomRules/DuplicateFiles 全部属 Free 范围
- [x] **P7-4** 无商业 dashboard 依赖（disk_analyzer_history 是目录导航历史，非商业功能）
- [x] **P7-5** 决策：保留 tauri/ 独立 workspace 成员；`npm run tauri dev` 实测可跑（dev server + 编译 + 启动）
- [x] **P7-6** 双壳验证：egui GUI 二进制（--features gui，7.5MB arm64）+ Tauri（tauri build dmg 3.51MiB / tauri dev）均可用

---

## 9. Phase 8 — 私有 Pro Workspace

> 来源：strategy §4.2/§8/§11/§39 + split `02` §4-9 + `04_PRIVATE_PRO_BOUNDARY.md`

- [x] **P8-1** `../maclean-pro` sibling 骨架（workspace + .gitignore + README，2026-10-08）
- [x] **P8-2** 8 个 Pro crates 骨架（Cargo.toml + lib.rs 占位）
- [x] **P8-3** Pro workspace Cargo.toml（resolver=3 + 8 members）；依赖公开 crate 用 pinned 版本（注释约束）
- [x] **P8-4** 审计：公开代码无 Pro 智能/历史/预测/策略/自动化/AI 实现；license.rs 验签保留（架构合法，strategy §17），签发移出
- [x] **P8-5** 全部 Pro crate publish=false + license Proprietary；公开 crate 零 Pro 依赖（CI grep 门禁）
- [x] **P8-6** keygen.rs 移入 ../maclean-pro/crates/maclean-pro-entitlement/src/bin/；公开仓库已删（bin 目录移除）
- [x] **P8-7** `grep -r "maclean-pro" crates apps tauri Cargo.toml` 为空 ✓

---

## 10. Phase 9 — 许可与治理边界文件

> 来源：strategy §84/§32-§37/§40-§41 + split `03` Phase 9

- [x] **P9-1** LICENSE = Apache-2.0 全文（176 行标准文本）
- [x] **P9-2** NOTICE（maclean contributors 版权声明）
- [x] **P9-3** CONTRIBUTING.md（贡献漏斗 + 代码规范 + DCO 引用）
- [x] **P9-4** SECURITY.md（6 类高优先级攻击面 + 修复优先级 + 5 条安全不变量）
- [x] **P9-5** CODE_OF_CONDUCT.md（Contributor Covenant 2.1）
- [x] **P9-6** GOVERNANCE.md（owner 五域最终权威 + 仓库拓扑）
- [x] **P9-7** DCO.md（DCO 1.1 + git commit -s 说明）
- [x] **P9-8** TRADEMARK_POLICY.md（允许/禁止/授权例外）
- [x] **P9-9** THIRD_PARTY_NOTICES.md（Rust 直接依赖 + npm 依赖 + 工具链，license 核对）
- [x] **P9-10** deny.toml（advisories/bans/licenses 三段配置，copyleft deny）
- [x] **P9-11** docs/governance/LICENSE_BOUNDARY.md（公开/私有目录矩阵 + 5 条边界不变量）

---

## 11. Phase 10 — 构建门禁（Build Gates）

> 来源：strategy §7.1/§87/§89 + split `03` Phase 10 + `scripts/maclean-open-core-validate.sh`

- [x] **P10-1** `cargo check --workspace` 全绿（2026-10-07）
- [x] **P10-2** `cargo test --workspace` 全绿：476 passed / 1 ignored（types 21 / core 339 / free-bin 75 / storage 13 / cli 9 / platform 12 等）
- [x] **P10-3** `cargo clippy --workspace --all-targets -- -D warnings` 干净
- [x] **P10-4** `cargo fmt --all -- --check` 干净
- [x] **P10-5** 商业边界验证：grep 无命中（清理 4 处注释中的私有包名，2026-10-07）
- [x] **P10-6** 密钥扫描：无命中（validate.sh 内）+ Phase 1 history 复核
- [x] **P10-7** `maclean-open-core-validate.sh .` 通过（Public workspace validation passed）

---

## 12. Phase 11 — 平台验证与打包

> 来源：strategy §42/§43/§71 + split `03` Phase 11

- [x] **P11-1** macOS Apple Silicon 构建 + 冒烟（2026-10-07）：Tauri dmg 3.51 MiB + .app 结构/codesign 验证；CLI --help/check-disk/list/scan --tab dev-cache 通过
- [ ] **P11-2** macOS Intel（P1）构建
- [ ] **P11-3** Windows x64（P1）构建（交叉编译或 CI）
- [ ] **P11-4** 打包链：`scripts/build-package.sh` 出 7 产物（6 dmg/pkg + cli.zip），命名对齐 `maclean-free-*`（strategy §71）
- [ ] **P11-5** 产物签名 / notarization（macOS）/ Authenticode（Windows）策略文档化
- [ ] **P11-6** 发布产物命名避免暴露私有 crate 名

---

## 13. 商业化文档落地

> 来源：`docs/commercialization/` 24 份 + strategy §59/§60/§94

- [x] **C-1** 24 份商业化文档移入 `docs/commercialization/`（2026-10-08，原编号保留）
- [x] **C-2** 决策（2026-10-08）：策略基线保留根目录（tasks.md/CI 引用稳定）；副本路径记录于 docs/governance/ 索引
- [ ] **C-3** 拆分方案目录归档：`docs/engineering-split-v1.0/` → `docs/engineering-split-v1.0/`（含 CSV / 计划 / 脚本）
- [ ] **C-4** `README.md` 重写为 Open Core 定位（引用 strategy §60 措辞；明确 Free/Pro 边界；不伪装全开源）
- [ ] **C-5** `docs/` 结构落地：`architecture / safety / scanners / development / commercialization / governance / migration`
- [ ] **C-6** Free/Pro 产品边界矩阵文档化（strategy §14：Free=scan/inventory/classification/safety/basic cleanup；Pro=intelligence/history/forecast/policy/automation/premium rules）
- [ ] **C-7** 定价与命名对齐（strategy §61/§62：Open Core / Free / Pro / Team；$29.99 lifetime / $19.99 founding）→ 写入 README 或 docs/commercialization
- [ ] **C-8** website（独立仓库）定位更新：列为外部待办（Landing Page PRD `14_LANDING_PAGE_PRD.md` 为输入）

---

## 14. CI / GitHub 治理

> 来源：strategy §23/§86/§87/§90 + split `06_CODEOWNERS_AND_CI.md`

- [ ] **G-1** `.github/CODEOWNERS`（`* @jamesfeng2009` + core/types 路径）
- [ ] **G-2** `.github/dependabot.yml`（cargo + npm）
- [ ] **G-3** `.github/ISSUE_TEMPLATE/`（bug / feature / security）
- [ ] **G-4** `ci.yml` 增强：`cargo fmt --check` / `cargo clippy --all-targets --all-features -D warnings` / `cargo test --workspace` / secret scan（gitleaks 或 actions）/ dependency review
- [ ] **G-5** `windows-ci.yml` 对齐公开 workspace（去掉对私有内容的引用）
- [ ] **G-6** 防泄漏规则落 CI：grep 私有包名 / Stripe 模式 / license 私钥模式 / Team 服务 URL（strategy §90）
- [ ] **G-7** 分支保护与发布权限清单化（main 保护 / 必需 CI / review；GitHub 控制台配置为外部待办）
- [ ] **G-8** 私密仓库 GitHub 控制台配置（maclean-pro / maclean-license-server 创建）为外部待办

---

## 15. 收尾（提交分层与交付）

> 来源：split `07_MIGRATION_COMMANDS.md` §12

- [ ] **F-1** 按 10 个推荐 commit 分层提交（freeze / domain contracts / core / platform / cli / free app / private pro / commercial intelligence / license+governance / CI 硬化），不做巨型 commit
- [ ] **F-2** 每个阶段独立可回滚（git tag/commit 粒度），严禁 `rm -rf old-source` 自动化
- [ ] **F-3** 完成定义核对（split `03` §5）：公开构建通过 ∧ 私有 Pro 构建通过 ∧ 公开仓库无私有依赖 ∧ 无商业密钥 ∧ 安全测试等价 ∧ UNKNOWN 清零
- [ ] **F-4** 双端 push（github ssh / gitee）+ 三端 HEAD 核验
- [ ] **F-5** 本文件状态总表全量更新 + present_files 交付

---

## 16. 状态总表

| # | 任务 | 优先级 | 阶段 | 状态 | 备注 |
|---|---|---|---|---|---|
| 1 | P0 冻结分支 + tag | P0 | 0 | ☑ | open-core/migration-v1；maclean-pre-open-core-v1 @ 3c6be09 |
| 2 | P1 审计（脚本 + .env + 分类 + history 复核） | P0 | 1 | ☑ | 无真实密钥；keygen 确认 PRIVATE；UNKNOWN=0 |
| 3 | P2 公开 crates 骨架 | P0 | 2 | ☑ | types/storage/platform/cli 4 crate + apps/maclean-free + virtual workspace（resolver=3） |
| 4 | P3 领域类型抽取 | P0 | 3 | ☑ | 6 领域模型 + envelope + Feature + 引用统一；serde 快照测试 |
| 5 | P4 安全层公开化 | P0 | 4 | ☑ | SafetyGate trait + SafetyCheck 迁移 + docs/safety/SAFETY_MODEL.md |
| 6 | P5 scanners 对齐 | P0 | 5 | ☑ | 已就位；apfs/windows_apps/uninstall 决策=保留 core（2026-10-07 记录） |
| 7 | P6 CLI 抽取 | P0 | 6 | ☑ | handler 迁入 maclean-cli；envelope 契约 + Pro 占位 + 删除下沉 core（2026-10-08） |
| 8 | P7 Free UI 抽取 | P0 | 7 | ☑ | egui 壳核对/ops 薄壳/无商业 dashboard；Tauri 保留独立；双壳验证过 |
| 9 | P8 私有 Pro 骨架 | P0 | 8 | ☑ | ../maclean-pro 8 crates；keygen 移出；公开零引用 |
| 10 | P9 许可/治理文件 | P0 | 9 | ☑ | LICENSE/NOTICE/CONTRIBUTING/SECURITY/CoC/GOVERNANCE/DCO/TRADEMARK/THIRD_PARTY/deny/BOUNDARY |
| 11 | P10 构建门禁 | P0 | 10 | ☑ | check/test(476)/clippy/fmt/validate.sh 全绿（2026-10-07） |
| 12 | P11 平台验证+打包 | P1 | 11 | 🔄 | macOS AS 构建+冒烟+dmg 已过；Intel/Windows/签名/7 产物留待 |
| 13 | C 商业化文档落地 | P1 | 13 | 🔄 | C-1/C-2/C-3 已归档；README Open Core 版已更新；C-4~C-8 待做 |
| 14 | G CI/GitHub 治理 | P1 | 14 | ☐ | CODEOWNERS/dependabot/secret scan |
| 15 | F 分层提交+交付 | P0 | 15 | ☐ | 10 commits + 双端 push |

---

## 17. 参考

- 策略基线：`MACLEAN_OPEN_CORE_LICENSE_AND_REPOSITORY_STRATEGY_V1.md`（§73 迁移计划 / §96 实施清单 / §95 冻结决策）
- 工程拆分：`docs/engineering-split-v1.0/`（已从 maclean-open-core-engineering-split-v1.0/ 归档）
- 商业化：`docs/commercialization/00_DOC_INDEX.md`（已从 maclean-commercialization-v1.0-docs/ 归档）
- 现有代码：`src/`（egui 壳 + CLI + license）、`crates/maclean-core/`（核心）、`tauri/`（Tauri 壳）、`website/`（独立仓库）
- 历史任务：`tasks-ui-renovation-v2.archived.md`（UI 改造，已归档）
