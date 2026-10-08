# maclean UI 改造 · 任务跟踪

> 目标：把当前 egui 界面改造成 `maclean-Tauri-UI交互稿`（2026-10-01 评审版）的视觉效果与交互反馈。
> 约束：**核心 ops / safety / scanner 逻辑一律不动**，只做表现层；每个阶段过门禁（fmt + clippy -D warnings + 380 tests）并打包交付。
> 路线未定前先做「阶段 0」，两条路线共享。

---

## 路线决策（先勾选，二选一）

| 路线 | 说明 | 投入 | 风险 | 能达成效果 |
|---|---|---|---|---|
| **A. egui 精修** | 在现有单二进制上重构表现层 | 中（2-3 周） | 低，不动删除逻辑 | 交互稿 ~85%（无毛玻璃 / 无 spring 动效） |
| **B. 迁移 Tauri** | Rust 核心抽 crate + Web 前端重写 | 大（6-10 周） | 中，UI 全重写 | 交互稿 ~100%（Web 全能力） |

- [ ] 路线 A：egui 精修（**推荐起步**——9 轮事故沉淀的安全逻辑零风险复用，单二进制分发不变；效果不足再叠加路线 B 只换壳）
- [x] 路线 B：迁移 Tauri（核心逻辑先完成「阶段 0」的 crate 抽取，换壳成本才可控）——**本轮已选，交付阶段 0 + Tauri 骨架**

---

## 阶段 0：前置（两条路线共享）

- [x] **P0** 创建备份基线：`git tag ui-baseline-252314c`，确认 worktree 干净（annotated tag 指向 6edac80）
- [x] **P0** 核心逻辑抽独立 crate：`maclean-core`（ops / safety / scanner / scheduler），UI 层只依赖它（workspace 化，commit 2bb20ea，380 tests 全绿）
  - 验收：`cargo test` 全绿；`maclean` bin 引用 `maclean-core` 后行为不变
- [ ] **P0** 全量截图基线：当前 8 个 Tab 各截一张 `_shots/baseline/*.png`，改造后逐一对比
- [x] **P1** 交互稿设计 token 落地为共享常量（色板 / 圆角 / 间距 / 阴影），egui 与未来 Web 侧同源（`maclean-core::design_tokens`，含钉值测试；前端 tokens.css 同源）

---

## 路线 A：egui 精修任务

### A-P0 视觉 token 系统（对应交互稿 CSS 变量）
- [ ] `src/theme.rs` 浅色 Palette 对齐交互稿：
  - 背景 `#F6F7F9`、卡片 `#FFFFFF`、面板二 `#FBFCFE`（当前 LIGHT 是 `#FAFAFA/#FFFFFF/#F5F5F7`）
  - 品牌 `#4F46E5`（当前 `#4B3FE3`）、品牌浅底 `#EEF2FF`/`#E0E7FF`
  - 语义色对齐：safe `#10B981`、cache `#3B82F6`、caution `#F59E0B`、danger `#EF4444`（含 50/100/600 档）
  - 文本四级：`#111827 / #4B5563 / #9CA3AF / 占位`
  - 描边：`#E8EAEE / #DDE1E7`
- [ ] 圆角 token 三级：16 / 12 / 8px（当前 R 常量对齐）
- [ ] 阴影 token：`--shadow` 与 `--shadow-lg` 两档（egui 用 `Frame`/`Shadow` 实现）
- [ ] 深色 Palette 同步对齐交互稿 `[data-theme="dark"]`（`#0E1013 / #171A20 / #1D2128`）
- [ ] 验收：`cargo test` 通过；截图对比基线无明显回退

### A-P1 布局骨架
- [ ] 左侧图标导航 rail：64px 宽、纯图标 + hover tooltip、选中态品牌浅底 + 左侧 3px 指示条、底部「检查更新」
  - egui 实现：`SidePanel::left` 固定宽 + `egui::Widget` 自绘
- [ ] 顶栏：面包屑（maclean / 当前页）+ 右侧「搜索 / 主题切换 / 语言」按钮组
- [ ] 内容区统一：24px 内边距、滚动条样式、卡片背景 `#FFFFFF` + 1px 描边
- [ ] 验收：8 个 Tab 全部在新骨架下正常渲染，无布局回归

### A-P2 各页面改造
- [ ] **概览**：磁盘 hero 卡（环形图 65% + 总量大字 + 已用/可用 + 开始扫描按钮）、可释放空间 2×3 卡片（名称/大小/项数/风险徽章）、右侧保护状态 4 行 + 最近清理 3 行
  - egui 环形图：`Painter` 画圆弧（`stroke` 分段），或引入 `egui_plot` 饼图；动画用 `ctx.request_repaint` + 插值
- [ ] **磁盘分析**：分类占比环形图 + 图例行（色块/名称/大小/百分比）、大目录 Top 树（可展开，chevron 旋转）
- [ ] **智能清理**：分类卡片列表（勾选框/图标/描述/项数/风险徽章/大小）、底部 sticky 汇总条（已选/预计释放/全选/清理按钮，Danger 态）
- [ ] **重复文件**：分组折叠头 + 文件行（保留/删除徽章、路径、大小）、顶部保护策略提示条
- [ ] **应用卸载**：卡片网格（图标/名称/版本/大小/残留）、选中红色描边、汇总条
- [ ] **启动项**：开关行（图标/名称/路径/类型/switch），禁用只改配置不删文件
- [ ] **系统优化**：维护项列表（图标/说明/收益/风险徽章/运行按钮），低风险直接跑、高风险走确认弹窗
- [ ] **设置**：分组卡片（外观/安全策略/关于），安全级别三态分段控件
- [ ] 验收：每个页面截图与交互稿比对，色值/间距/状态一一对应

### A-P3 交互反馈与动效
- [ ] 扫描进度遮罩：进度条 + 百分比 + 当前目录 + 取消按钮（复用现有 start_scan，接入新视觉）
- [ ] 删除确认弹窗：危险态（2px 红描边 + 警示横幅「不可逆操作」+ 路径列表 + 取消/确认）
  - 已有点：`widgets::modal_danger_frame()` / `danger_banner()`（252314c）→ 按交互稿重排
- [ ] Toast 通知（右上角滑入，成功/信息/警告三态）——egui 自绘浮动层
- [ ] 主题切换即时生效 + 持久化（已有 `toggle_dark_mode`，补「跟随系统」三态可选）
- [ ] 微动效：卡片 hover 上浮、按钮按压、开关位移、进度条 transition
  - 遵守 `reduced-motion`：`ctx.options().animation_time` 可控
- [ ] 验收：关键路径手测一遍（扫描→勾选→确认→清理→toast），无僵尸按钮

---

## 路线 B：Tauri 迁移任务（如选）

### B-P0 工程
- [x] 新建 `tauri/` 目录：`tauri@2` + Rust 后端复用 `maclean-core`（`tauri/src-tauri`，path 依赖 maclean-core）
- [x] IPC 契约设计：`disk_info / scan / clean_preview / clean_execute / startups_list / startup_set_enabled / optimize_list / optimize_run / settings_get / settings_set / palette`，全部走白名单 command（事件：scan-progress / clean-log）
- [x] 安全边界：**所有删除必须过 `maclean-core::safety` 再经 IPC 暴露**，前端拿不到裸文件句柄（clean_execute 复用 core `ops::start_delete`，其三个删除出口内建 safety 闸门；启动项只移 plist 不删文件）
- [ ] 打包链：保留 dmg/pkg；引入 Tauri bundler（体积目标 ≤ 15MB）——骨架阶段未做，`cargo build -p maclean-tauri` 与 `npm run tauri dev` 已验证可运行

### B-P1 前端
- [x] 用交互稿 `maclean-tauri-ui/maclean-Tauri-UI交互稿.html` 作为骨架直接工程化（React 18 + Vite 5 + TS strict，CSS/图标/SVG 1:1 移植）
- [x] 逐页落地：概览 / 磁盘分析 / 智能清理 / 重复文件 / 应用卸载 / 启动项 / 系统优化 / 设置（概览/清理/启动项/优化/设置已运行时接真数据验证；分析/重复/卸载复用同一套 scan IPC）
- [x] 深色模式：CSS 变量 + 首次启动 `prefers-color-scheme` 跟随，手动切换即时生效并持久化
- [x] 动效：扫描进度、确认弹窗、toast 全部真实化（scan-progress 事件 / 危险确认 modal / 右上 toast，接真数据）

### B-P2 双轨并行
- [ ] 迁移期保留 egui 版本可回退：`cargo build --features legacy-egui`
- [ ] 双版本并行发布两轮，行为一致后下线 egui

---

## 每阶段统一验收闭环（无论路线）

- [ ] `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --bin maclean`（380 passed 基线）
- [ ] 截图对比：`_shots/` 与交互稿逐页比对
- [ ] 打包 7 产物（6 dmg/pkg + cli.zip）+ 安装替换 /Applications + ad-hoc 重签
- [ ] 双端 push（github ssh / gitee）+ 三端 HEAD 核验
- [ ] present_files 一次性交付

---

## 状态总表

| # | 任务 | 优先级 | 路线 | 状态 | 备注 |
|---|------|--------|------|------|------|
| 1 | 备份基线 + crate 抽取 | P0 | 共享 | ☑ | tag ui-baseline-252314c；commit 2bb20ea |
| 2 | 截图基线 | P0 | 共享 | ☐ | 本轮骨架范围未做 |
| 3 | 视觉 token 落地 | P0 | A | ☐ | theme.rs |
| 4 | 布局骨架（rail/顶栏） | P1 | A | ☐ | ui/mod.rs |
| 5 | 概览页 | P2 | A | ☐ | |
| 6 | 磁盘分析页 | P2 | A | ☐ | |
| 7 | 智能清理页 | P2 | A | ☐ | |
| 8 | 重复文件页 | P2 | A | ☐ | |
| 9 | 应用卸载页 | P2 | A | ☐ | |
| 10 | 启动项页 | P2 | A | ☐ | |
| 11 | 系统优化页 | P2 | A | ☐ | |
| 12 | 设置页 | P2 | A | ☐ | |
| 13 | 扫描遮罩 + 确认弹窗 | P3 | A | ☐ | |
| 14 | Toast + 主题切换 + 动效 | P3 | A | ☐ | |
| 15 | Tauri 工程 + IPC（如选 B） | P0 | B | ☑ | 11 个白名单 command + 2 事件；cargo clippy -D warnings 干净 |
| 16 | 前端页面工程化（如选 B） | P1 | B | ☑ | React+Vite+TS strict，8 页，tsc+vite build 通过，5 页运行时真数据验证 |

---

## 参考

- 交互稿：`maclean-tauri-ui/maclean-Tauri-UI交互稿.html`（评审版，浏览器可直接打开）
- 现有主题：`src/theme.rs`（Palette LIGHT/DARK、sync_visuals/apply_visuals）
- 现有 UI：`src/ui/mod.rs`、`src/widgets.rs`（modal_danger_frame/danger_banner 已有）
- 配置持久化：`src/config.rs`（dark_mode 默认 false）
