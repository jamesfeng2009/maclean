# 冻结基线快照（P0-3，2026-10-07）

- 分支：`open-core/migration-v1`
- 冻结 tag：`maclean-pre-open-core-v1`（annotated，指向 `3c6be09`）
- 基线：`main @ 3c6be09` "fix: 删除大目录卡死错觉 —— 删除过程实时反馈 + 计时"
- 冻结时工作树：除三份输入（策略 md / 工程拆分方案 / 商业化文档）外干净
- 旧任务文件：`tasks-ui-renovation-v2.archived.md`（归档保留）

## 迁移起点约束

1. 公开仓库（maclean）与私有仓库（maclean-pro）分离；
2. 公开仓库可独立 `cargo build / test / run`，不依赖 Stripe / license-server / Pro / Team；
3. 商业依赖单向（Pro → 公开），公开 crate 永不反向依赖；
4. 私钥 / Stripe Secret / 数据库凭据永不入库。
