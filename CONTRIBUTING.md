# Contributing to maclean

感谢你考虑为 maclean 贡献。maclean 采用 **Open Core** 模式：本仓库是公开的
**Free / Open Core** 部分（Apache-2.0）；商业侧（Pro：历史 / 预测 / 策略 /
自动化 / AI / 授权）位于**私有仓库**，不接受公开贡献。

## 贡献漏斗（Contribution Funnel）

```text
Issue / Discussion → 澄清与范围确认 → Fork + 分支 → PR → Review → 合入
```

1. **先讨论再动手**：行为变更 / API 变更 / 安全相关改动，先开 Issue 或
   Discussion 说明动机与方案；小修（文档、错别字、测试）可直接 PR。
2. **范围确认**：维护者确认改动落在公开边界内（见 `docs/governance/LICENSE_BOUNDARY.md`）。
   涉及 Pro 商业能力的请求会转至私有渠道或关闭。
3. **开发**：`cargo fmt` / `cargo clippy --workspace --all-targets -- -D warnings` /
   `cargo test --workspace` 必须全绿（见 `tasks.md` Phase 10 门禁）。
4. **PR**：说明改动动机、测试结果、验证方式（含 `cargo check --no-default-features`
   纯 CLI 构建）。
5. **Review**：至少 1 名维护者 review；安全敏感改动（`safety.rs`、删除路径、
   提权、路径处理）需要安全专项 review（见 `SECURITY.md`）。

## 代码规范

- Rust 2021 edition；`cargo fmt` 风格；clippy `-D warnings` 零告警。
- 结构化输出（CLI JSON/JSONL）必须走 `maclean-types::cli::CliEnvelope` 契约；
  字段变更需 bump `contract_version`。
- **删除逻辑只允许在 `maclean-core`**：CLI / UI 层不得内联文件系统删除实现
  （Phase 6 规则）。
- 公开 crate（maclean-types / maclean-core / maclean-cli 等）不得依赖私有 crate。

## DCO

所有提交必须包含 `Signed-off-by`（见 `DCO.md`）。未签署的 commit 会被拒绝合并。

## 行为准则

参见 `CODE_OF_CONDUCT.md`。任何形式的骚扰或人身攻击将导致贡献权限被移除。
