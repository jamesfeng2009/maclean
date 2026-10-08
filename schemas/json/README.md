# schemas/json

CLI JSON / JSONL envelope 与领域模型的 JSON Schema。

- envelope 契约：`{"contract_version","request_id","command","status","data"}`
  （`maclean-types::cli::CliEnvelope`，contract_version = 1）
- schema 文件在后续里程碑随 `maclean-types` 发布同步生成。
