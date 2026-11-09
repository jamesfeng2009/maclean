# maclean CLI 契约（公开 · 稳定）

> 版本：contract v1 · 2026-10-13
> 适用范围：maclean 公开仓库 `apps/maclean-cli`（产物名 `maclean`）。
> 本契约对自动化脚本 / CI / 定时任务**承诺稳定**：字段名与语义不随
> 文案翻译、界面调整或版本迭代改变。破坏性变更必须 bump
> `contract_version` 并写入迁移说明。

## 1. 定位

maclean CLI 是**永久免费 · Apache-2.0 开源**的磁盘清理命令行工具：

- 无 GUI / 无许可证 / 无授权服务器依赖；
- 不因试用过期、服务器宕机、付款失败而失效；
- 删除前默认只读，`--dry-run` / 预览 / 确认三重保护。

## 2. 命令集

```text
maclean scan        扫描可清理项目（--tab / --deep / --format json|jsonl）
maclean clean       清理安全可删除项目（--dry-run / --yes / --safe-only）
maclean schedule    查看 / 注册 / 注销定时清理任务（--install / --remove / --days）
maclean check-disk  磁盘空间检查（--breakdown 分类占比）
maclean list        列出扫描类别
maclean apps        列出已安装应用
maclean uninstall   卸载应用（本体 + 关联数据，移入废纸篓可还原）
maclean system-data System Data 聚合视图（缓存/日志/快照/iOS 备份/App 支持拆分）
maclean apfs        APFS 快照 / 模拟器运行时（子命令 list / delete <target> --yes）
maclean log         日志路径 / 最近日志（--format json|jsonl 输出结构化信封）
maclean backups     历史删除清单
maclean restore     按清单还原
maclean startup     macOS 启动项管理（list / disable / enable / login-items）
maclean optimize    系统优化任务（list-tasks / run <task> --yes）
maclean dup-ignore  重复文件忽略规则（list / add / remove）
```

Pro 命令（`history / growth / forecast / policy / automation`）在公开
CLI 中为**契约占位**：可解析、返回错误信封与对应退出码，实现位于私有
maclean-pro 仓库。

## 3. JSON envelope

`--format json`（单对象）与 `--format jsonl`（流式，每行一个对象）输出：

```json
{
  "contract_version": 1,
  "request_id": "<uuid>",
  "command": "scan",
  "status": "ok",
  "data": { }
}
```

| 字段 | 类型 | 语义 |
|---|---|---|
| `contract_version` | int | 契约版本；变更字段语义时递增 |
| `request_id` | string | 一次请求一个 ID，便于日志关联 |
| `command` | string | 原始命令名 |
| `status` | string | `ok` / `error` / `warnings` / `cancelled` |
| `data` | object | 命令结果；字段按命令各自定义 |

机器格式（json / jsonl）恒无色、无进度条；进度信息只写 stderr。

- `json` 输出**一个**完整 envelope 对象；
- `jsonl` **每行都是一个完整 envelope**（含 `contract_version` /
  `request_id` / `command` / `status` / `data`），最后一行为收尾信封
  （`data` 省略，`status` 反映本次最终退出码）；warnings / cancelled /
  error 均有对应信封，不出现 `status:"ok"` 配失败退出码。

## 4. 退出码

| 值 | 常量 | 语义 |
|---|---|---|
| 0 | `EXIT_OK` | 成功 |
| 1 | `EXIT_FAILURE` | 通用失败 |
| 2 | — | 参数错误（clap 默认） |
| 3 | `EXIT_CONFIRM_REQUIRED` | 需要确认但未提供 `--yes` |
| 4 | `EXIT_CANCELLED` | 用户取消 |
| 5 | `EXIT_WARNINGS` | 部分成功（有失败项） |
| 6 | `EXIT_SERIALIZE` | 输出序列化失败 |
| 7 | — | Ctrl+C 中断 |

脚本判定成功/失败**只依据退出码**，不解析人类可读文本。

## 5. 安全默认值

- `scan` 默认只读，不删除任何文件；
- `clean` 默认预览（不删除），`--dry-run` 显式预览，`--yes` 才实际删除；
- 删除前对目标路径二次校验（符号链接 / 路径穿越 / 受保护目录 /
  挂载点 / 删除前后目录变化），见 `crates/maclean-core/src/safety.rs`
  与 `crates/maclean-core/src/ops/mod.rs`；
- 失败项**不会谎报成功**：部分失败返回 `EXIT_WARNINGS` 并逐项列出原因。

## 6. 兼容性规则

1. 新增字段允许；删除或重命名字段必须 bump `contract_version`。
2. 错误消息可改，`error_code` 语义不变。
3. 退出码语义只增不改。
4. 版本输出 `maclean --version` 为唯一版本来源（Cargo.toml）。

## 7. 示例

```bash
# 预览将清理的项目（不删除）
maclean clean --tab dev-cache --dry-run --format json

# CI 中执行安全清理：成功才算 0
maclean clean --safe-only --yes --format jsonl || echo "失败($?)"

# 定时任务注册
maclean schedule --install --days 7

# 脚本消费：只看退出码与 JSON
if maclean scan --deep --format json > /tmp/maclean-scan.json; then
  jq '.data.reclaimable' /tmp/maclean-scan.json
fi
```
