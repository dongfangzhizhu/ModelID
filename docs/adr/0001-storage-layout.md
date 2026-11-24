# ADR-0001：CAS 存储布局与不可变对象

- **状态**：Accepted（as-built，2026-11-24；源码基线 `f6663ce`）
- **日期**：2026-11-24
- **相关任务**：A1、A5、B5、B9
- **相关文档**：[as-built 架构](../architecture.md)、[统计语义](0004-statistics-semantics.md)

## 背景

modeld 需要在多个 AI 前端之间共享同一份模型文件，同时保留用户原来的路径。早期规划稿设想过 256 分片、子分片、只读对象、虚拟前端目录、64 MiB 分块哈希等不同层次的设计；其中一部分已实现，一部分仍是规划。本文只记录当前代码中实际生效的布局。

## 决策

1. **store 路径解析顺序固定**：CLI `--store` > `MODELD_STORE` > `<platform-default>/modeld.toml [store].path` > 平台默认目录。
   - 源码：`crates/modeld-core/src/store_path.rs::resolve_store_path` / `default_store_path`。

2. **CAS 对象按 BLAKE3 前 2 个十六进制字符分片**：
   ```text
   <store>/cas/blake3/<2-hex-prefix>/<64-hex-blake3>
   ```
   `init()` 一次创建 `00..ff` 256 个目录；不实现子分片。
   - 源码：`crates/modeld-core/src/cas.rs::CasStore::{init,path_for_hash}`、`crates/modeld-core/src/hash.rs::Blake3Hash::prefix`。

3. **哈希是标准 BLAKE3**，与 `b3sum` 逐位一致；小文件（< 10 MiB）顺序读，大文件用 `memmap2::Mmap` + `Hasher::update_rayon()`。旧的自定义「分块再组合」哈希已废弃。
   - 源码：`crates/modeld-core/src/hash.rs::{hash_file,hash_file_small,hash_file_large}`。

4. **入库必须原子且可恢复**：`store_crash_safe` 先写 `<store>/tmp/cas_staging/<tx_id>/<hash>.part`，流式计算并校验 size + BLAKE3，`sync_all()`，再原子 rename 到 CAS 目标；跨卷 rename 失败时退回 copy + delete；最后设置只读权限（`0o444` / Windows read-only）。成功和失败路径都只做 `remove_dir` 删除**空** staging 目录，不做 `remove_dir_all`。
   - 源码：`crates/modeld-core/src/cas.rs::store_crash_safe` / `discard_staging_dir`。
   - 理由：一次 `scan` 的多个文件共用同一个 `tx_id`，递归删除会误删其他仍在写的 `.part`，重新引入本决策要避免的损坏。

5. **已有文件的零拷贝采纳限定同卷**：`adopt_object` 只对同卷文件建硬链接；跨卷明确报错并要求 `--copy`，不静默复制。
   - 源码：`crates/modeld-core/src/cas.rs::adopt_object`、`crates/modeld-core/src/ingest.rs::IngestMode`。

6. **HF 假缓存复用 CAS 对象**，`blobs/<sha256>` 与 `snapshots/<revision>/<file>` 都是相对符号链接，`refs/<branch>` 文件内容是 40-hex commit SHA 而不是分支名。相对目标按主机切换分隔符（Windows 用反斜杠，Unix 用正斜杠），否则 Windows 上会生成悬空链接。
   - 源码：`crates/modeld-core/src/hf_cache.rs::{relative_target_for_host,create_relative_symlink,create_cache_entry_with_type,check_cache}`。

## 后果

- 优点：O(1) 定位、256 目录足以支撑当前规模；内容只读化后不会被前端误改；崩溃不会留下半对象；同卷扫描零额外磁盘占用。
- 代价：CAS 根目录与 store 必须在同一卷（暂存 → CAS 的 rename 语义）；Windows 上只读对象和符号链接受权限/开发者模式影响，失败必须显式上报而不是静默复制。
- 明确不做：`virtual/{comfyui,forge,a1111}` 虚拟目录、类别检测、CAS 子分片、内存 LRU 哈希缓存。它们只存在于历史规划稿，当前代码无实现。

## 验证

- `crates/modeld-core/src/cas.rs` 单元测试：staging 成功/失败均清理、跨卷拒绝、哈希校验失败不留对象。
- `crates/modeld-core/tests/integration_test.rs`、`tests/fault_injection_tests.rs`、`tests/interruption_matrix.rs`。
- `crates/modeld-cli/tests/environment.rs`：跨卷 `--adopt` 非零退出且 CAS 字节为 0；junction 自环不挂死。
