# modeld 架构（as-built）

> **文档状态**：as-built，基于 B8 完成时的源码（提交 `f6663ce`）核对，最后更新 2026-11-24。
> **本文不再描述计划态**。旧 Phase 0 规划稿已归档至 [`docs/history/ARCHITECTURE_PHASE0.md`](history/ARCHITECTURE_PHASE0.md)；本文中的每个模块都给出当前代码中的落点。
> **任务计划与验收记录**：[`docs/ROADMAP.md`](ROADMAP.md) 是唯一权威计划文件；API 默认值以 [`docs/proxy-api.md`](proxy-api.md) 为准。

---

## 1. 系统边界与进程形态

modeld 是一个本地优先的 AI 模型内容寻址存储（CAS）系统。它不要求前端改配置：文件仍出现在用户原来的路径上，但内容可以只存一份，路径通过硬链接 / 符号链接 / 仅数据库记录指向同一 CAS 对象。

当前交付形态是一个 Rust workspace（5 个 crate）和一个可选的 Python 包：

| 组件 | 产物 | 入口 | 源码 |
|---|---|---|---|
| `modeld-core` | lib | 领域库 | `crates/modeld-core/src/lib.rs` |
| `modeld-cli` | `modeld` 二进制 | 命令行 | `crates/modeld-cli/src/main.rs`、`src/cli.rs` |
| `modeld-webui` | lib + `modeld-webui` 二进制 | 浏览器管理端与 HTTP/WebSocket API | `crates/modeld-webui/src/lib.rs`、`src/main.rs`、`src/server.rs` |
| `modeld-proxy` | lib | 局域网 CAS 注册表 + HF 兼容代理 | `crates/modeld-proxy/src/lib.rs`、`src/server.rs` |
| `modeld-client` | lib | 同步阻塞 Rust SDK | `crates/modeld-client/src/lib.rs`、`src/client.rs` |
| `modeld_hook` | Python 包 | HF 下载拦截（可选） | `python/modeld_hook/__init__.py`、`intercept.py` |

workspace 定义在根目录 `Cargo.toml`；`rust-version = 1.88`。二进制只有两个：`modeld` 与 `modeld-webui`；proxy 与 client 是被 CLI / 其他程序调用的库。

典型数据流：

```text
scan <path> ──> Scanner ──> BLAKE3 ──> IngestMode
                               │          ├─ Index   只写 SQLite/path_index（默认）
                               │          ├─ Adopt   同卷硬链入 CAS
                               │          └─ Copy    跨卷复制入 CAS
                               │
                               └──> Database(models / aliases / path_index)

dedup ──> TxManager::begin ──> staging(.part) + fsync + hash ──> CAS publish
      ──> links.rs 暂存链接 + rename ──> alias/WAL commit ──> audit.log
      └── 崩溃 / 中断 ──> recover::recover_store：pending 回滚，copied 前滚

hf-download / HF_ENDPOINT ──> Downloader(流式 SHA256+BLAKE3, .part.meta)
      ──> CAS + hf_mappings ──> hf_cache(hub/blobs/snapshots/refs) ──> HF 框架

modeld-webui / modeld proxy ──> 同一个 <store>/modeld.db
```

---

## 2. 存储模型（对应 ADR-0001 / ADR-0002 / ADR-0004）

### 2.1 store 目录

默认 store 路径由 `crates/modeld-core/src/store_path.rs` 决定：

1. CLI `--store`
2. 环境变量 `MODELD_STORE`
3. `<平台默认目录>/modeld.toml` 中的 `[store].path`
4. 平台默认：Windows `%LOCALAPPDATA%\modeld`；macOS `~/Library/Application Support/modeld`；Linux `$XDG_DATA_HOME/modeld` 或 `~/.local/share/modeld`

实际目录：

```text
<store>/
├── cas/blake3/<2 位前缀>/<64 位 BLAKE3>   不可变对象
├── hf_cache/hub/models--<org>--<repo>/     HF 兼容假缓存
│   ├── blobs/<sha256>                     → CAS 对象的相对符号链接
│   ├── refs/<branch>                       内容为 40-hex commit SHA
│   └── snapshots/<revision>/<file>        → ../../blobs/<sha256>
├── tmp/downloads/*.part{,.meta}           可续传下载分片
├── tmp/cas_staging/<tx_id>/<hash>.part    两阶段提交暂存
├── quarantine/<...>.meta                  30 天软删除
├── audit.log                              追加式 NDJSON
├── backups/                               迁移前 VACUUM INTO 备份
├── modeld.db                              SQLite（WAL 模式）
└── modeld.toml                            配置与 token
```

**注意**：`virtual/` 前端虚拟目录、子分片 `cas/blake3/<c1>/<c2>/...` 与 GC 的 `refs` 表只存在于旧规划稿；当前代码没有实现。

### 2.2 CAS 与哈希

| 能力 | 实现 | 源码 |
|---|---|---|
| BLAKE3 前缀分片（2 hex，256 目录） | `CasStore::path_for_hash` / `init` | `crates/modeld-core/src/cas.rs` |
| 原子入库：staging → fsync → 校验 → rename → 只读 | `CasStore::store_crash_safe` | `crates/modeld-core/src/cas.rs` |
| 同卷零拷贝采纳 | `CasStore::adopt_object` | `crates/modeld-core/src/cas.rs` |
| 标准 BLAKE3（与 `b3sum` 一致） | `hash_file` | `crates/modeld-core/src/hash.rs` |
| 小文件顺序读 / 大文件 mmap + rayon | `SMALL_FILE_THRESHOLD = 10 MiB`、`hash_file_small` / `hash_file_large` | `crates/modeld-core/src/hash.rs` |

当前实现**没有** 64 MiB 哈希分块、自定义分块组合哈希、100K 条内存 LRU 缓存。增量扫描的缓存是持久化的 `path_index` 表，匹配键为 path + size + mtime + inode + device id。

### 2.3 SQLite 元数据

数据库实现集中在 `crates/modeld-core/src/db.rs`：

- `Database::open` 设置 `journal_mode=WAL`、`synchronous=NORMAL`、`foreign_keys=ON`、`busy_timeout=5000ms`。
- `SCHEMA_VERSION = 4`，`init_schema` 通过 `PRAGMA user_version` 做 v1→v4 迁移；迁移前 `backup_before_migration` 使用 `VACUUM INTO`。
- 当前共 9 张表：`models`、`aliases`、`wal_transactions`、`downloads`、`hf_mappings`、`workflows`、`workflow_refs`、`tags`、`path_index`。
- 旧稿中的 `refs` 表不存在；显式引用落在 `workflow_refs` + `aliases` + `models.pinned`。
- 统计入口是 `Database::storage_stats()` / `frontend_stats()`，没有独立的 `stats.rs`。

`AuditLogger`（`crates/modeld-core/src/audit.rs`）把变更写为 `<store>/audit.log` 的 NDJSON；`i18n.rs` 按 `MODELD_LANG > LC_ALL/LC_MESSAGES/LANG > sys-locale > en` 选择语言。

---

## 3. modeld-core：领域模块逐项对照

`crates/modeld-core/src/lib.rs` 是模块清单与对外 re-export 的唯一入口。下表列出当前真实存在的模块；描述均可按“源码”列定位。

| 模块 | 职责（as-built） | 关键类型 / 函数 | 源码 |
|---|---|---|---|
| `config` | 读取/原子保存 `<store>/modeld.toml`；只重写自有节并保留注释与未知表；token 双写 | `ModeldConfig`、`load_config`、`save_config`、`set_proxy_token`、`DEFAULT_RANGE_CHUNK_BYTES` | `src/config.rs` |
| `store_path` | 解析 store 路径优先级与平台默认值 | `resolve_store_path`、`default_store_path` | `src/store_path.rs` |
| `hash` | 标准 BLAKE3、10 MiB 阈值、mmap+rayon | `Blake3Hash`、`hash_file` | `src/hash.rs` |
| `cas` | 不可变对象布局、原子入库、只读化、同卷 adopt | `CasStore::{init,store,store_crash_safe,adopt_object}` | `src/cas.rs` |
| `db` | schema v4 / 9 表、WAL 连接、模型/别名/下载/HF 映射/路径索引、统计聚合 | `Database`、`StorageStats`、`storage_stats`、`in_transaction` | `src/db.rs` |
| `tx` | 统一事务边界：`begin/commit/fail/rollback/recover/cleanup`，8 种 `OpType` | `TransactionManager`、`TxPlan`、`RollbackPlan` | `src/tx.rs` |
| `recover` | 崩溃恢复：`pending` 回滚、`copied` 前滚、`committed` 清理 | `recover_store`、`RecoveryReport` | `src/recover.rs` |
| `scanner` | WalkDir 收集 + Rayon 并行哈希；glob 排除、符号链接环检测、增量索引 | `Scanner`、`ScanOptions`、`scan` | `src/scanner.rs` |
| `ingest` | scan 与 WebUI 共用的唯一摄取路径：Index / Adopt / Copy | `IngestMode`、`ingest_scan_results` | `src/ingest.rs` |
| `dedup` | 重复分组、canonical 选择、两阶段执行、风险分级、Windows 锁文件跳过 | `DedupEngine::plan_dedup/execute_dedup_group/run_dedup`、`DedupStrategy` | `src/dedup.rs` |
| `links` | 链接能力探测、同卷判断、暂存链接 + 原子 rename、reference-only 回退 | `LinkCapability::detect`、`StagedLink`、`replace_with_link` | `src/links.rs` |
| `unlink` | 把链接还原成独立文件并改别名状态 | `unlink_path` | `src/unlink.rs` |
| `store_lock` | 跨进程独占锁：`flock(LOCK_EX)` / `LockFileEx`，锁文件不删除 | `StoreLock::{try_acquire,acquire,current_holder}` | `src/store_lock.rs` |
| `platform` | 平台能力、同卷判断、文件占用检测、目录 fsync | `detect_capabilities`、`is_same_volume` | `src/platform.rs` |
| `downloader` | HF 下载：流式双哈希、`.part.meta` 续传、原子 DB 提交、token 管理 | `Downloader::download_hf_file`、`token_set/get/status` | `src/downloader.rs` |
| `hf_cache` | HF Hub 兼容假缓存：blob / snapshot / refs、repo type 前缀、统计 | `HfCache::create_cache_entry_with_type`、`check_cache` | `src/hf_cache.rs` |
| `quarantine` | 30 天软删除、`.meta` 元数据、restore / cleanup / stats | `QuarantineManager` | `src/quarantine.rs` |
| `gc` | 引用分类与候选生成、safe 执行、隔离区/临时目录清理 | `GcEngine::{candidates,preview,run_safe}` | `src/gc.rs` |
| `refs` | 基于 workflow_refs / aliases / pinned 的引用解释与孤儿列表 | `scan_workflow_refs`、`explain_refs`、`list_orphans` | `src/refs.rs` |
| `workflow` | ComfyUI workflow JSON 解析与模型依赖索引 | `parse_workflow`、`index_workflow` | `src/workflow.rs` |
| `governance` | tag / note / pin / favorite / provenance 与 JSON 导入导出 | `ProvenanceInfo` 等 | `src/governance.rs` |
| `fsck` | 快检 `run_fsck`、深检 `run_verify`、保守修复 `run_repair` | `FsckReport`、`VerifyReport` | `src/fsck.rs` |
| `doctor` | store 可写性、磁盘空间、SQLite WAL、版本、平台能力体检 | `run_doctor` | `src/doctor.rs` |
| `audit` | 追加式审计日志 | `AuditLogger::log/recent` | `src/audit.rs` |
| `i18n` | 中英文文案与语言探测 | `Lang::detect`、`t`、`tf` | `src/i18n.rs` |

**统计口径**见 ADR-0004；**事务模型**见 ADR-0002；**HF 兼容**见 ADR-0005。

### 3.1 去重与崩溃恢复的关键不变量

1. 任何路径替换都经过 `StagedLink`：先在目标同目录建临时链接，再原子 rename；失败时原路径仍有原始字节。
2. CAS 发布前必须完成 `.part` 复制、`sync_all`、size + BLAKE3 校验；跨卷 rename 失败退回 copy + delete，绝不静默改变内容。
3. WAL 状态决定恢复动作：`pending` 删除暂存并标记 `rolled_back`；`copied` 前滚链接与别名；`committed` 清理记录。
4. staging 目录只做 `remove_dir`，不做 `remove_dir_all`：同一次 `scan` 共用 `tx_id`，删除非空目录会误伤其他在飞 `.part`。
5. `store_lock` 是整库互斥；只读命令（stats/verify/list）刻意不加锁，所有 layout-mutating 命令都必须加锁。

`crates/modeld-core/tests/interruption_matrix.rs` 在 5 个中断点验证上述不变量：begin 后、staging 复制后、CAS 发布后、部分链接后、commit 前。

---

## 4. modeld-cli：命令与输出契约

命令树定义在 `crates/modeld-cli/src/cli.rs`，分发在 `src/main.rs`，各命令实现位于 `src/commands/*.rs`。

| 领域 | 命令 | 实现 |
|---|---|---|
| 初始化/扫描 | `init`、`scan`、`status`、`stats`、`dupes`、`list`、`info`、`hash` | `src/util.rs`、`commands/scan.rs`、`commands/query.rs` |
| 去重/修复 | `dedup`、`quarantine`、`gc`、`unlink`、`verify`、`refs`、`tx`、`db`、`store` | `commands/{dedup,repair,gc,tx,db,store}.rs` |
| HF | `hf-check`、`hf-download`、`hf-setup`、`hf-status`、`hf` | `commands/hf.rs`、`commands/hfstore.rs`、`src/util.rs` |
| 服务 | `serve`、`proxy`、`config`、`doctor` | `commands/{serve,proxy,config}.rs`、`src/util.rs` |
| 治理 | `tag`、`note`、`pin`、`favorite`、`workflow-scan`、`workflow-deps`、`refs-orphans` | `commands/meta.rs`、`src/util.rs` |

契约要点：

- 日志写 stderr（`init_logging`，`src/util.rs`），`--json` 命令的 stdout 供脚本解析；集中序列化入口是 `src/output.rs::write_json`。少数命令仍直接 `println!` JSON，属于已知技术债。
- 变更命令 `scan` / `dedup` / `gc` / `unlink` / `quarantine cleanup|restore` 均支持 `--wait <秒>`（默认 30，`0` 表示立即失败），等待提示写 stderr。锁由 `src/util.rs::lock_store` 调 `StoreLock::acquire` 实现。
- 退出码：`0` 成功、`1` 操作失败、`2` clap 用法错误。`output.rs` 中定义了 `3 = partial failure` 的帮助文案，但当前 `main` 分发路径尚未实际返回 3；**不要把「已文档化」当作已接线行为**。
- `verify` 当前调用 `run_fsck`（快检）；核心库中的 `run_verify`（深检重算哈希）与 `run_repair` 尚未在 CLI 暴露。

命令行快照测试：`crates/modeld-cli/tests/cli_snapshot.rs`；10 进程并发测试：`tests/concurrency.rs`；环境类测试：`tests/environment.rs`。

---

## 5. modeld-proxy：局域网服务与 HF 镜像

`crates/modeld-proxy/src` 模块：

| 模块 | 职责 | 源码 |
|---|---|---|
| `config` | `[proxy]` 配置、loopback 判定、启动不变量 | `src/config.rs`（注意：当前 workspace 实际以 `modeld-core::config::ProxyConfig` 为准，`src/config.rs` 为兼容层，默认值与 core 同步） |
| `auth` | IP allow/deny（deny 优先）、Bearer token 校验、常量时间比较 | `src/auth.rs` |
| `server` | 路由、流式文件响应、Range 分片、HF 回源与缓存头、worker pool、panic 隔离 | `src/server.rs` |
| `range` | 单段 Range 解析；多段只取第一段 | `src/range.rs` |
| `discovery` | mDNS 服务发现（只发现不广播） | `src/discovery.rs` |

路由：

| 方法 | 路径 | 鉴权 | 说明 |
|---|---|---|---|
| GET | `/health` | 无 | 返回 `status/version/uptime_seconds/model_count/total_bytes` |
| GET | `/v1/models` | Bearer + IP | 返回 `models/total/total_size_bytes`；`format` / `category` 可为 `null` |
| GET | `/v1/blobs/{hash}` | Bearer + IP | 200 全量或 206 分片；单窗口上限 64 MiB |
| GET | `/v1/hf-proxy/{org}/{repo}/resolve/{revision}/{file}` | Bearer + IP | 命中返回 `X-Modeld-Cache: hit`；未命中回源后写 CAS 与假缓存，返回 `miss` + `X-Modeld-Blake3` |

安全默认、token 优先级、错误码与配置字段的完整说明见 [`proxy-api.md`](proxy-api.md) 与 ADR-0003。要点：

- CLI 默认绑定 `127.0.0.1:8234`，首次启动生成 UUIDv4 token 并双写 `[auth] token` / `[proxy.auth] tokens`。
- 非 loopback 绑定必须显式 `--allow-lan`，且必须带 token；`--allow-anonymous` 仅允许 loopback。
- IP 规则先于 token 校验；deny 命中返回 403。仅 IPv4 CIDR 是精确语义，IPv6 / hostname 退化为前缀子串匹配。
- proxy 不提供 `/metrics`；WebUI 的 `GET /metrics` 才是免鉴权的计数器端点。
- `modeld-client` 是同步 SDK，支持续传，但**不校验下载内容的 BLAKE3**；需要端到端校验的调用方应自行重算。

测试：`crates/modeld-proxy/tests/proxy_integration.rs`、`tests/hf_proxy_test.rs`；流式压测脚本 `scripts/bench-proxy-streaming.ps1`。

---

## 6. modeld-webui：嵌入式管理端

`crates/modeld-webui/src`：

| 层 | 职责 | 源码 |
|---|---|---|
| 启动 | `--store/--port/--host/--token/--allow-lan/--open`；非 loopback 必须有 token | `src/main.rs` |
| 路由器 | `/api/v1` nest、`/ws`、`/metrics`、SPA fallback；中间件顺序 metrics → compression → cors → csrf → route auth | `src/server.rs` |
| 鉴权 | Bearer token；`/ws` 额外允许 `?token=`；token 不写日志 | `src/auth.rs` |
| CSRF / CORS | 对带 `Origin` 的写请求要求白名单 origin + `X-Modeld-CSRF`；CORS 只允许白名单 | `src/server.rs` |
| 指标 | `modeld_*` 计数器与 Prometheus 渲染；`/metrics` 故意免鉴权 | `src/metrics.rs`、`src/server.rs` |
| 状态 | `Arc<Mutex<Database>>`、store path、`broadcast::Sender<WsEvent>`、metrics、token、`rate_limit_per_min` | `src/state.rs` |
| WebSocket | 7 类 `WsEvent` 广播 | `src/ws.rs` |
| 静态资源 | `rust-embed` 编译期嵌入 `ui/`；未命中返回 `index.html` | `src/static_files.rs` |
| 错误处理 | `ApiError { status, message, code }` | `src/error.rs` |

`/api/v1` 路由共 18 条声明（`/settings` 含 GET 与 PUT）：status、stats、models、models/:hash、dupes、dedup/preview、dedup/apply、downloads、scan、scan/trigger、gc/preview、gc/run、quarantine、quarantine/:id/restore、settings、tags、models/:hash/tags。`crates/modeld-webui/tests/api_e2e.rs` 编译期读取 `src/api/mod.rs` 的路由表，新增路由若没有 e2e 覆盖会让测试失败。

前端位于 `crates/modeld-webui/ui/`：`main.js`（Hash Router / EventBus / WS）、`api.js`（统一 fetch + token + CSRF 头 + 401 重输）、`ws.js`（URL/退避纯函数）、`i18n.js`、`pages/*.js`（dashboard、dupes、library、downloads、refs、proxy、settings）。`ui/tests/*.test.js` 用 node:test + fast-check 覆盖纯函数。

已知边界：`AppState.rate_limit_per_min` 字段存在但尚未挂限流中间件；`/metrics` 免鉴权需在网络层限制。

---

## 7. Python hook：HF 拦截的真实边界

`python/modeld_hook` 0.1.0，运行时零依赖，Python ≥ 3.9。

- **推荐路径**：`export HF_HOME="$(modeld hf-setup --print-path)"`，让 `huggingface_hub` 直接使用 modeld 假缓存。
- **显式路径**：`import modeld_hook; modeld_hook.activate()`。
- **自动激活**：仅 `MODELD_AUTO_HOOK=1 python ...`；import 本身无副作用。
- `activate()` 先探测 `modeld --version`，设置 `HF_HOME`，再 monkeypatch `huggingface_hub.hf_hub_download` 与 `huggingface_hub.file_download.hf_hub_download`；`deactivate()` 还原。
- wrapper 先 `modeld hf-check --json`，未命中再 `modeld hf-download --json`，失败回退原 HF。
- **限制**：依赖 `modeld` 在 PATH；`cache_dir` / `local_dir` / `force_download` 直接透传原库；`snapshot_download` 包装函数存在但未启用；`modeld hook install` 与 `MODELD_NO_AUTO_HOOK` 只在旧文档中出现，代码未实现/未读取。

---

## 8. 测试、覆盖率与性能基线

| 层级 | 证据 |
|---|---|
| 核心单元 / 集成 | `crates/modeld-core/src/*.rs` 内联测试；`tests/integration_test.rs`、`tests/fault_injection_tests.rs` |
| 中断矩阵 | `crates/modeld-core/tests/interruption_matrix.rs`（5 点） |
| 属性测试 | `crates/modeld-core/proptest-regressions/`、`crates/modeld-webui/proptest-regressions/` |
| CLI | `crates/modeld-cli/tests/{cli_snapshot,concurrency,environment}.rs` |
| WebUI | `crates/modeld-webui/tests/api_e2e.rs`（28 个 HTTP 级测试）、`ui/tests/*.test.js` |
| Proxy / Client | `crates/modeld-proxy/tests/*.rs`、`crates/modeld-client/src/client.rs` 内联测试 |
| Python | `python/tests/test_modeld_hook.py` |
| 覆盖率 | `scripts/coverage.py`，core 行覆盖率 79.43%（报告在 `coverage/`），CI 门禁 75% |
| 性能 | `crates/modeld-core/benches/scan_dedup.rs`、`bench/baseline.json`、`scripts/bench_gate.py`（20% 回归阈值，按平台隔离） |
| CI | `.github/workflows/ci.yml`：三平台 fmt/clippy/test、前端、Python、docker、deny、bench、coverage |

B8 全量回归：`cargo test --workspace --locked` → 394 passed / 0 failed。

---

## 9. 明确不做 / 尚未实现

为避免旧规划稿继续误导，以下概念**不在当前代码中**（相应概念只在 `docs/history/ARCHITECTURE_PHASE0.md` 或 `docs/rfcs/` 中作为历史设计出现）：

- `virtual/{comfyui,forge,a1111}` 虚拟目录、类别检测、`sync-virtual`、`categorize`、`frontends list`。
- 64 MiB 哈希分块与自定义组合哈希；100K 条内存 LRU 哈希缓存；CAS 子分片。
- `refs` 表；junction 链接创建（仅能力探测与 DB 枚举存在）。
- `modeld init --enable-python-hook`、`modeld hook install`、`verify --fix`、`proxy token-rotate --reload`。
- `[hashing]`、`[cache]`、`[frontends.*]` 以及旧稿中的部分 `[gc]` 配置项。
- 退出码 3 的实际返回；WebUI 限流；proxy `/metrics`；客户端 BLAKE3 校验。

这些缺口已按任务编号记录在 [`docs/ROADMAP.md`](ROADMAP.md) §12.3；本文只陈述现状，不把它们写成既有能力。

---

## 10. 决策记录索引

- [ADR-0001 存储布局](adr/0001-storage-layout.md)
- [ADR-0002 事务与恢复模型](adr/0002-transaction-and-recovery.md)
- [ADR-0003 鉴权与安全默认](adr/0003-authentication-defaults.md)
- [ADR-0004 统计语义](adr/0004-statistics-semantics.md)
- [ADR-0005 HuggingFace 兼容性](adr/0005-huggingface-compatibility.md)
