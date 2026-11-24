# ADR-0002：事务边界、链接原子性与崩溃恢复

- **状态**：Accepted（as-built，2026-11-24；源码基线 `f6663ce`）
- **日期**：2026-11-24
- **相关任务**：A1、B1、B7、B8
- **相关文档**：[存储布局](0001-storage-layout.md)、[as-built 架构](../architecture.md)

## 背景

去重涉及三类副作用：复制/发布 CAS 对象、替换用户路径上的文件、写 SQLite 元数据。任意一步中断都不能让用户路径消失，也不能让数据库声称拥有实际不存在的链接。早期实现使用「先删除后建链」和基于 `O_EXCL` 标记文件的跨进程锁，二者分别有路径消失窗口和「崩溃后靠 mtime 猜持有者」的问题。

## 决策

1. **路径替换必须经过暂存链接**：`StagedLink` 在目标同目录创建 `.modeld_tmp_<uuid>.tmp`，成功后 `rename(tmp, target)`；Drop 时清理临时名。绝不允许先删目标再建链。
   - 源码：`crates/modeld-core/src/links.rs::{stage_link,StagedLink::commit,replace_with_link}`、`src/unlink.rs::unlink_path`。

2. **去重采用两阶段提交**：
   - Phase A：`TransactionManager::begin` → WAL `pending` → 复制 `.part` 并流式校验 → `copied`。
   - Phase B：CAS publish / 链接替换 → 写 alias → `TransactionManager::commit` → 审计记录。
   - 源码：`crates/modeld-core/src/dedup.rs::execute_dedup_group`、`src/tx.rs`。

3. **WAL 状态决定恢复动作**：`pending` 回滚（删暂存、标记 `rolled_back`），`copied` 前滚（重放 alias 链接），`committed` 清理。`TransactionManager::recover` 幂等，`rollback` 也幂等。
   - 源码：`crates/modeld-core/src/recover.rs::{recover_store,RecoveryReport}`、`src/tx.rs::{TransactionManager,OpType,TxStatus}`。
   - 当前 `OpType` 有 8 种：Dedup、Unlink、Gc、QuarantineCleanup、AliasRewrite、CasPromotion、HfDownloadCommit、StoreMigration。

4. **跨进程互斥由内核持有**：`StoreLock` 使用 `flock(LOCK_EX)`（Unix）/ `LockFileEx`（Windows），进程退出即释放；锁文件刻意不删除，避免 unlink 竞态；文件内容只是 pid/op/host/started_at 的诊断记录，不是锁的真值。
   - 源码：`crates/modeld-core/src/store_lock.rs`、CLI 入口 `crates/modeld-cli/src/util.rs::lock_store`。
   - 粒度是**整个 store**：`scan` / `dedup` / `gc` / `unlink` / `quarantine cleanup|restore` 互斥；只读命令（`stats` / `verify` / `list`）不加锁。CLI `--wait <秒>` 默认 30，`0` 表示立即失败，等待提示写 stderr 以保持 `--json` 输出纯净。

5. **SQLite 本身也是 WAL**：`journal_mode=WAL`、`synchronous=NORMAL`、`foreign_keys=ON`、`busy_timeout=5000ms`；下载完成的 model / mapping / download / alias / WAL commit 包在一次 `Database::in_transaction` 里。
   - 源码：`crates/modeld-core/src/db.rs::{Database::open,in_transaction}`。

## 后果

- 优点：任意中断点后用户路径仍有原始字节；崩溃/强杀不会留下伪活锁；`--wait` 让多进程脚本可预测；恢复动作可重复执行。
- 代价：整库互斥会降低并发写吞吐，10 万模型级大库扫描会阻塞其他写操作直到 `--wait` 超时；锁持有者记录是 best-effort，记录写失败时 busy 提示可能显示上一轮信息。
- 明确不做：按 CAS 分片或读写锁拆分粒度（待 C6）；NFS/SMB 上内核锁语义未实测；`proxy token-rotate --reload` 与此无关但同样未实现。

## 验证

- `crates/modeld-core/tests/fault_injection_tests.rs`：链接失败、rename 失败、只读目标等场景保持原文件。
- `crates/modeld-core/tests/interruption_matrix.rs`：begin / staging copy / CAS publish / partial link / before commit 五个中断点，统一断言 `verify` clean、无 in-flight WAL、两条库路径内容不变。
- `crates/modeld-cli/tests/concurrency.rs`：10 个真实进程并发 `dedup --auto --wait 120`，`verify` 无损坏、无死锁、无 staging 残留。
- `crates/modeld-cli/tests/environment.rs`：另一进程持锁时 `--wait 0` 被拒并打印持有者 pid。
