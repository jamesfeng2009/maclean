# maclean × MangoDisk 竞品对比

> 评估基准：maclean（本项目，源码 + README）+ MangoDisk（公开官网 / GitHub / 第三方评测，2026-10-06 检索）。
> 定性评分（0–5）基于公开资料与源码的**定性评估**，非基准测试数据。

## 一句话结论

**maclean 的优势在「删除安全纵深 + 开发者场景深度 + 自动化/脚本化」；MangoDisk 的优势在「免费开源生态 + 平台成熟度 + UI/可视化 + 功能广度（隐私清理 / AI 解读 / 系统维护）」**。两者同为 Rust 内核，是同一赛道里「工程安全派」与「产品体验派」的典型分野。

---

## 一、功能矩阵

| 能力维度 | maclean | MangoDisk |
|---|---|---|
| 内核 | Rust，maclean-core 单一内核（egui + Tauri 双壳复用） | Rust + Tauri 2 |
| 深度清理类别 | 60+ 类（38 固定路径缓存注册表 + ~15 复杂扫描器 + 应用缓存 + APFS 快照） | 系统缓存 / 用户缓存 / 应用缓存 / 浏览器数据 / 开发工具 / AI 模型缓存 / 项目构建产物等 |
| 大文件清理 | ✅ 按类型（large_files.rs） | ✅ 按类型（视频/音频/图片/安装包等，可配置阈值 + Finder/Explorer 定位） |
| 重复文件清理 | ✅ 内容级哈希 | ✅ 内容级校验，每组保留一份 |
| 磁盘空间分析 | ✅ 分类占比 / 大文件视图 | ✅ treemap + sunburst 可视化 |
| 应用一键卸载 | ✅ 本体+关联数据+缓存，定点 stat + 有界预算 + TCC 授权引导 | ✅ 关联缓存/设置/残留一并清理 |
| 启动项管理 | ✅ 可逆禁用 | ✅ 可逆禁用 |
| 系统优化 | ✅ optimize 任务（DNS 缓存刷新等） | ✅ 系统优化 + 系统维护（修复搜索/图标/声音/网络异常） |
| 隐私清理 | ❌ 无专项 | ✅ 浏览器/应用/系统活动痕迹 |
| AI 解读 | ❌ 无 | ✅ AI 解释缓存用途与清理影响（不自动执行） |
| 安全闸门 | ✅ 三级保护（Critical 拒删 / RequiresOfficialUninstaller / DataProtected）+ TOCTOU 复查 + 出口白名单 | ✅ 扫描只读 + 删除前逐项确认 + 每条规则 3 道检查 |
| 删除可恢复 | ✅ 大文件/卸载走废纸篓 + M-2 备份清单 restore | 部分（确认制，未见清单还原能力） |
| 提权 | ✅ Touch ID / 密码（sudo_local pam_tid） | 未公开 |
| CLI / 脚本化 | ✅ 同一二进制：scan/clean/check-disk/uninstall/startup/optimize/schedule/restore，json/jsonl + 语义退出码 | ❌ 无 |
| 定时调度 | ✅ launchd / schtasks 无人值守 | ❌ 无 |
| 平台 | macOS 主力 + Windows（可编译、未真机冒烟） | macOS + Windows（已发布），社区提及 Linux |
| 商业模式 | 闭源商业授权（Ed25519 双密钥，lifetime/yearly） | 免费开源（GitHub ~3.6k star，Homebrew 可装） |
| 社区与生态 | 本地项目，CI + README + 打包脚本齐备 | 官网 + Blog + V2EX/DEV 第三方评测 + Homebrew |
| 语言 | 中/英双语 i18n | 中/英（另有韩/土/葡等多语言） |

---

## 二、maclean 的优点（相对 MangoDisk）

1. **删除安全纵深明显更深**——这是最核心的差异：
   - 三级保护：Apple 系 / 安全软件（Critical）直接拒删，MDM 类要求官方卸载器，含用户数据需确认；
   - 删除前 TOCTOU 复查（符号链接穿越拦截）+ 出口白名单，CLI 与 GUI 同一套闸门，无法绕过；
   - M-2 备份清单 + 废纸篓还原（`restore <id>`），删错能按清单找回——MangoDisk 是「确认制」，没有清单还原能力。
2. **开发者场景深度**：38 个固定路径缓存注册表 + 15 个复杂扫描器（Xcode / Docker / AI 模型缓存 / 模拟器运行时）+ IM 数据专项，覆盖面比 MangoDisk 的「开发工具」分类更细、更工程化。
3. **自动化 / 脚本化是 MangoDisk 完全没有的维度**：同一二进制的 CLI（json/jsonl 结构化输出 + 语义退出码 + `--yes` 门禁 + `--privileged` 自动提权）+ launchd/schtasks 定时调度，可直接进 CI / 服务器 / 容器做无人值守清理。
4. **提权链路完整**：Touch ID / 密码提权删除，覆盖需要 sudo 的系统级残留（LaunchDaemons 等），MangoDisk 未公开等价能力。
5. **架构可维护性**：maclean-core 单一内核被 egui / Tauri / CLI 三个壳复用，安全逻辑只有一份实现；刚完成的卸载修复（定点 stat + 有界预算 + TCC 引导）体现了对真实磁盘高负载的工程化处理。
6. **刚落地的卸载可靠性**：Group Containers 定点 stat、3 秒迭代级预算、体积只读缓存、TCC 失败后引导授权——同类工具少有针对「卸载卡死」根因的系统性修复。

## 三、maclean 的缺点（相对 MangoDisk）

1. **生态与获客处于绝对劣势**：MangoDisk 免费开源（GitHub ~3.6k star、262 forks、Homebrew 一条命令安装、官网 + Blog + 多篇第三方评测），maclean 为闭源商业授权，无公开社区、无官网推广、无包管理器分发。
2. **Windows 侧未完成真机验证**：UAC 提权删除、系统还原点、Recycle Bin、深色模式都只在编译层验证；MangoDisk 的 Windows 已是正式发布形态。
3. **功能广度少两块**：隐私清理（浏览器/应用/系统痕迹）与 AI 解读（解释「这是什么缓存、清了会怎样」）是 MangoDisk 的差异化功能，maclean 缺失。
4. **UI / 可视化成熟度**：MangoDisk 的 Tauri 界面（treemap + sunburst + 交互演示）已到产品级；maclean 的新 Tauri 壳仍是骨架版（当前主 UI 是 egui），磁盘空间分析缺 treemap/sunburst 这类一眼看懂的可视化。
5. **商业化门槛**：License 系统（Ed25519 + 机器指纹 + 销售台账）意味着 maclean 面向付费用户，信任门槛高——在 MangoDisk「免费开源」的对比下，功能没有拉开代差前很难说服用户付费。
6. **品牌与信任背书**：MangoDisk 有「规则过官方文档 + 真实系统测试 + 每条规则 3 道检查」的公开承诺，且有 V2EX / DEV 社区背书；maclean 的安全能力写在代码里，但缺乏面向用户的信任表达。
7. **多语言**：maclean 仅中/英，MangoDisk 已覆盖韩/土/葡等多语言市场。

---

## 四、维度雷达（0–5 定性评分）

```echarts
{
  title: { text: 'maclean vs MangoDisk 能力维度（0-5 定性评分）', left: 'center', top: 8, textStyle: { fontSize: 15 } },
  tooltip: { trigger: 'item', triggerOn: 'mousemove|click', renderMode: 'richText', confine: true },
  legend: { bottom: 2, data: ['maclean', 'MangoDisk'], textStyle: { fontSize: 12 } },
  radar: {
    center: ['50%', '53%'],
    radius: '62%',
    indicator: [
      { name: '删除安全与可恢复', max: 5 },
      { name: '开发者场景深度', max: 5 },
      { name: '自动化与脚本化', max: 5 },
      { name: '平台覆盖成熟度', max: 5 },
      { name: '社区与生态', max: 5 },
      { name: 'UI 与可视化', max: 5 },
      { name: '功能广度', max: 5 }
    ],
    axisName: { fontSize: 11 },
    splitLine: { lineStyle: { width: 1 } }
  },
  series: [
    {
      type: 'radar',
      name: 'maclean',
      symbolSize: 5,
      lineStyle: { width: 2 },
      areaStyle: { opacity: 0.14 },
      data: [{ value: [5, 5, 5, 2, 1, 3, 4], name: 'maclean' }]
    },
    {
      type: 'radar',
      name: 'MangoDisk',
      symbolSize: 5,
      lineStyle: { width: 2 },
      areaStyle: { opacity: 0.14 },
      data: [{ value: [4, 4, 1, 4, 4, 4, 5], name: 'MangoDisk' }]
    }
  ]
}
```

## 五、建议（把缺点变机会）

1. **补上「信任表达」**：把已有的安全能力（三级保护 / TOCTOU / M-2 还原 / 卸载预算修复）做成官网或 README 的安全模型页——MangoDisk 用「3 道检查」一句话获得信任，maclean 值得同等甚至更强的公开表达。
2. **优先完成 Windows 真机冒烟**：UAC 提权、还原点、Recycle Bin 验证通过后，Windows 才真正成为卖点（MangoDisk 已占位，越晚越难打）。
3. **差异化主打自动化**：MangoDisk 没有 CLI / 调度，maclean 的 `schedule` + 语义退出码是唯一有「无人值守清理」能力的工具——面向 CI / 服务器 / 开发者工作站是 MangoDisk 打不进来的护城河，应作为营销主轴。
4. **低成本补功能**：隐私清理（浏览器 Cookie/历史痕迹扫描）与 AI 解读（对已有 60+ 类别注册表加 LLM 解释层）可在现有架构上增量实现，补上功能广度短板。
5. **UI 追上**：Tauri 壳从骨架版推进到正式版，磁盘分析补 treemap/sunburst，与 MangoDisk 的颜值差距是用户第一印象，值得优先投入。
6. **分发渠道**：考虑 Homebrew cask 安装，降低试用门槛；商业授权与免费版并存（如免费版限额度）比纯付费更容易冷启动。
