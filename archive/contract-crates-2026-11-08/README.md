# 契约骨架 crate 归档（2026-11-08）

## 为什么在这里

`maclean-storage` 与 `maclean-platform` 于 2026-10-08 作为 Open Core 拆分的
「先立契约」骨架加入 workspace（`278959e` / `7bdb69c`），但直到 v0.4.3 仍然：

- `cargo metadata` / `cargo tree -i` 反向依赖为 **0**，全仓无 `use`、无 `[dependencies]` 引用；
- 四个私有工程包（production / open-core-split / commercialization / license-server）
  共 111 个条目里**没有任何依赖声明**，只有设计文档提过名字；
- `tasks.md` P5-2/P5-3 承诺的「后续按需迁移」从未发生，core 内已有重复实现且开始漂移
  （`safety.rs::extract_reg_value` 与 `registry.rs` 逐行重复；
  `ops::is_path_mounted` 与 `path.rs` 签名/分支不一致）；
- `46cdbe2` 已自行标注 `publish = false` 并写明「外部也不会有消费者」。

按 2026-11-08 的 D-08 决策（方案 C），二者移出 workspace，代码保留在此归档，
由 git 历史可完整恢复。**它们不参与 `cargo build/test/clippy --workspace`，
也不是本仓库的公开 API 面。**

## 当前能力在 core 的对应实现

| 归档 crate | core 内实际实现 |
|---|---|
| `storage::json_store` | `config.rs::load_config/save_config`、`scanner/sizecache.rs`、`scanner/cache.rs` |
| `storage::repositories` | `backup.rs::record/list/load/restore_plan/restore` |
| `storage::migrations` | 无（core 当前不做 schema 迁移） |
| `platform::trash` | `platform/macos_trash.rs`、`platform/mod.rs::move_to_trash`、`backup.rs` |
| `platform::path` | `ops/mod.rs::is_path_mounted` |
| `platform::registry` | `safety.rs::extract_reg_value` |
| `platform::disk` | `platform/mod.rs::disk_info` |

## 重新接入前必须修完的已知缺陷

归档代码**不可独立构建**（`version.workspace = true` / `maclean-types.workspace = true`
依赖根 workspace 上下文）。重新把它接回 workspace 之前，逐项解决：

1. `storage/src/json_store.rs:64-74` — `write_atomic` 名不副实：固定临时文件名
   `*.json.tmp` 导致并发 save 交叉写/交叉 rename，且全程无 `fsync`。
2. `storage/src/migrations.rs:47-52,66-69,99-101` — 版本号硬编码 v1、
   `as u32` 截断、非对象 JSON 静默跳过；且框架零真实迁移步骤、未接线到仓储。
3. `storage/src/json_store.rs:25-27` + `repositories.rs:25-27` — `path_for(key)`
   未校验 `..` / `/`，id 可穿越出存储目录。
4. `platform/src/path.rs:9-31` — `is_path_mounted` 只做字符串前缀比较，不
   canonicalize、不展开 `~`/`..`；`is_symlink` 对权限错误 fail-open。
5. `storage/src/repositories.rs` / `migrations.rs` — 缺「旧版本 → 新版本 → 再启动」
   与损坏文件的端到端测试（v1 迁移幂等、`VersionAhead` 回滚、半截 JSON 不 panic）。
6. 语义等价性：与 core 现有实现逐函数对比（尤其 `is_path_mounted` 两版分支差异），
   确认替换后行为不变，再删除 core 里的重复实现。

缺陷 1–5 对应审查清单 `G-03/G-04/G-05/G-06`、`D-07`、`H-11`；这些条目随本次
归档退出当前构建范围，在此登记为重新接入的准入门槛。

## 恢复方式

```bash
git mv archive/contract-crates-2026-11-08/maclean-storage crates/maclean-storage
git mv archive/contract-crates-2026-11-08/maclean-platform crates/maclean-platform
# 再把两行加回根 Cargo.toml 的 [workspace].members，并从 exclude 移除 "archive"
```
