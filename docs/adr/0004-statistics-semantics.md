# ADR-0004：存储统计的口径与计算方式

- **状态**：Accepted（as-built，2026-11-24；源码基线 `f6663ce`）
- **日期**：2026-11-24
- **相关任务**：A7、B6、B9
- **相关文档**：[as-built 架构](../architecture.md)

## 背景

早期 `stats` 会构造 `DedupEngine` 逐个 stat 别名路径，在大库上既慢又容易把「同一内容有多个路径」误当成「浪费了多份磁盘」。同时 dashboard 对每个模型查一次别名，形成 N+1；`duplicate_bytes` 字段甚至从未被填充，恒为 0。本 ADR 固定统计口径和实现方式。

## 决策

1. **四个量各司其职**（字段定义在 `crates/modeld-core/src/db.rs::StorageStats`）：
   - `logical_bytes`：每个 alias 路径都按完整文件大小计数，回答「用户看到的模型总大小」。
   - `physical_bytes`：`Σ models.size_bytes`，每个唯一内容只算一次，回答「store 实际拥有多少字节」。与 `du -sb <store>/cas` 对拍应使用此字段。
   - `deduped_saved_bytes`：已由 hardlink / symlink / junction 节省的字节；若同组还有 `original` 副本，则只计算链接本身带来的节省，避免重复计账。
   - `duplicate_waste_bytes`：仍为独立 `original` 副本造成的浪费，即 `size * (copies - 1)`。
   - 辅助字段：`duplicate_groups`（`copies > 1` 的组数）、`multi_path_groups`（`aliases >= 2` 的组数）、`alias_count`、`model_count`。

2. **统计不遍历文件系统**：`Database::storage_stats()` 是一条 SQL，通过子查询按 `model_hash` 聚合 aliases，排除 `quarantined_at IS NOT NULL` 的模型。聚合失败返回错误，dashboard 显示 500，而不是渲染成「0 bytes」。
   - 源码：`crates/modeld-core/src/db.rs::storage_stats`；CLI 输出 `crates/modeld-cli/src/commands/query.rs`。

3. **被排除的模型不计数**：隔离区模型不进入统计；`LinkType::ReferenceOnly` 不按副本重复计账（当前生产写入路径会先把内容放进 CAS，因此 `Σ size_bytes` 是实际占用的可靠近似；该假设记录在 `db.rs` 注释与 ROADMAP §12.3）。

4. **前端分布单独聚合**：`Database::frontend_stats()` 用一条 `GROUP BY frontend` 查询取代 N+1。同一模型在同一 frontend 内只算一次；`duplicate_bytes` 报告该 frontend 内仍可被链接替换的副本，不跨 frontend 累加。
   - 源码：`crates/modeld-core/src/db.rs::frontend_stats`、`crates/modeld-webui/src/api/stats.rs`。

5. **`dupes` 与 `stats` 分工**：`stats` 只做数据库聚合，回答「有多少/多少字节」；`dupes` 才检查文件现状，回答「哪些路径可以处理」。`modeld stats --json` 输出 `logical_bytes` / `physical_bytes` / `deduped_saved_bytes` / `duplicate_waste_bytes`，供脚本与 `du` 对拍。

## 后果

- 优点：大库统计从 O(N+M) 次查询降为 1 次；脚本能拿到稳定字段；重复节省不再被夸大。
- 代价：口径依赖 DB 与磁盘一致；`reference_only` 语义若未来扩大使用，需要重新审视 `physical_bytes` 的假设；`logical_bytes` 与 `physical_bytes` 的差额包含链接节省和真实浪费两部分，不能单独作为「可回收空间」。
- 未验证：10 万模型下 `storage_stats()` 是否 < 200 ms；≥10 GB 真实库与 `du -sb` 的长期偏差（ROADMAP §12.3）。

## 验证

- `crates/modeld-core/tests/interruption_matrix.rs`：中断点第 4/5 断言 `logical_bytes=512 KiB`、`physical_bytes=256 KiB`、浪费 0，证明共享而非仅仅可读。
- `crates/modeld-cli/tests/cli_snapshot.rs`：`stats --json` 输出纯 JSON 且字段稳定。
- B6 对拍：19.5 MB 合成库上 `physical_bytes` 与 `cas/` 实际字节差值为 0。
