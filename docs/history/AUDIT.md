# modeld 代码审计报告 (AUDIT.md)

> 生成日期：2025-07  
> 审计范围：Cargo workspace（modeld-core / modeld-cli / modeld-proxy / modeld-client / modeld-webui）  
> 对照规格：doc.md § 1–19，.kiro/specs/modeld-complete-implementation/requirements.md

---

## 1. 测试结果总览

`cargo test` 全部通过，零失败：

| 测试套件 | Pass | Fail | 说明 |
|---|---|---|---|
| modeld-cli (bin) | 0 | 0 | 无 #[test] |
| modeld-client | 7 | 0 | 客户端 URL 解析、JSON 反序列化 |
| modeld-core (unit) | 84 | 0 | CAS、DB、dedup、GC、fsck、hash、hf_cache、i18n、links、quarantine、scanner、unlink、workflow |
| modeld-core (integration) | 9 | 0 | 完整工作流、重复文件、崩溃恢复、去重 E2E、quarantine |
| modeld-proxy (unit) | 36 | 0 | auth、config、discovery、range、server URL 解析 |
| modeld-proxy/hf_proxy_test | 1 | 0 | HF proxy miss→hit |
| modeld-proxy (integration) | 11 | 0 | auth、blob 下载、Range 请求、HF proxy cache hit |
| modeld-webui | 0 | 0 | 无 #[test]（仅框架骨架） |
| **合计** | **148** | **0** | |

编译警告：`net2 v0.2.39` 含未来版本 Rust 将拒绝的代码（传递依赖，不在本项目控制范围内）。

---

## 2. 已实现功能

### 2.1 modeld-core

| 模块 | 功能 | 状态 |
|---|---|---|
| `cas.rs` | CAS 初始化（256 前缀目录）、store/get/contains、read-only 权限 | ✅ 完整 |
| `db.rs` | SQLite WAL 模式、schema v1（models/aliases/wal_transactions/downloads/hf_mappings/workflows/workflow_refs）、CRUD 全覆盖、批量 duplicate 查询（防 N+1）、`PRAGMA user_version` 版本控制 | ✅ 完整 |
| `scanner.rs` | 递归扫描、Rayon 并行哈希、增量缓存（size+path 匹配跳过重哈希）、隐藏目录过滤、排除列表、进度回调 | ✅ 完整 |
| `dedup.rs` | 两阶段提交（WAL pending → copied → committed）、canonical 选择（CAS > oldest > shortest > alphabetical）、hardlink/symlink/reference-only 降级、崩溃恢复（recover_incomplete_transactions）、dry-run/auto/report 模式 | ✅ 完整 |
| `gc.rs` | hard/soft/orphan 三级保护、GC preview、quarantine 写回 DB、过期清理、tmp 文件清理 | ✅ 完整 |
| `quarantine.rs` | quarantine/restore/list/cleanup_expired/delete_permanent、JSON 元数据、30 天 TTL、Windows read-only 兼容 | ✅ 完整 |
| `fsck.rs` | missing CAS、dangling aliases、size mismatch、orphan CAS 检测 | ✅ 基础完整（缺 `--deep` hash 校验、`--json` 输出、repair 命令） |
| `downloader.rs` | HF HEAD 元数据、SHA256 验证、Range resume（206 检测）、CAS 去重、HF cache alias 写入、错误路径 fail_download、稳定 .part 文件名 | ✅ 完整（缺 gated/private 友好提示、token keychain 存储） |
| `hf_cache.rs` | 伪 HF cache 目录结构（blobs/snapshots/refs）、create_cache_entry、Windows 硬链接/复制降级 | ✅ 完整 |
| `links.rs` | hardlink/symlink/reference-only、同卷检测（Unix dev_t / Windows drive letter）、Windows read-only 清除 | ✅ 完整 |
| `hash.rs` | BLAKE3 流式哈希、mmap 加速、前缀提取、hex 解析 | ✅ 完整 |
| `workflow.rs` | ComfyUI workflow JSON 解析（数组/对象 inputs 两种格式）、17 种 loader 类型、alias basename 构建 model lookup、DB 写入 workflow_refs | ✅ 完整 |
| `unlink.rs` | 硬链接/符号链接 → 独立物理副本恢复 | ✅ 完整 |
| `i18n.rs` | 中/英双语 key-value 表、tf 插值、sys-locale 自动检测 | ✅ 完整 |

### 2.2 modeld-cli

| 命令 | 实现状态 | 说明 |
|---|---|---|
| `init` | ✅ 功能 | 创建 CAS/DB/quarantine，路径默认 `~/.local/share/modeld` |
| `scan` | ✅ 功能 | 递归扫描、增量缓存、进度条、自动 init |
| `status` | ✅ 功能 | 统计+最近5条+quarantine |
| `stats` | ✅ 功能 | 模型数、别名数、索引大小、去重节省 |
| `dupes` | ✅ 功能 | 批量查重、`--min-size`、`--json` |
| `list` | ✅ 功能 | 列出模型、`--limit`、`--json` |
| `info` | ✅ 功能 | 详细信息+别名、`--json` |
| `hash` | ✅ 功能 | 计算单文件 BLAKE3 |
| `dedup` | ✅ 部分 | dry-run/auto/report 模式；缺 `--apply`、`--strategy`、`--protect`、`--pin` 等参数 |
| `quarantine list/cleanup/restore` | ✅ 功能 | |
| `hf-check` | ✅ 功能 | 检查缓存命中 |
| `hf-download` | ✅ 功能 | 下载并入库，支持 `--token`、`--revision` |
| `hf-setup` | ✅ 功能 | 打印/设置 HF_HOME |
| `hf-status` | ✅ 功能 | HF cache 统计 |
| `workflow-scan` | ✅ 功能 | 扫描并索引 ComfyUI workflow |
| `workflow-deps` | ✅ 功能 | 显示单文件依赖 |
| `refs-orphans` | ✅ 功能 | 列出无引用模型 |
| `gc` | ✅ 功能 | preview/run/cleanup-quarantine/cleanup-tmp |
| `verify` | ✅ 部分 | 基础 fsck，缺 `--deep` 哈希验证 |
| `unlink` | ✅ 功能 | 硬链接/符号链接还原 |
| `proxy start/discover/status` | ✅ 部分 | start 功能完整；discover 依赖 mDNS；status 调用远程 health |

### 2.3 modeld-proxy

| 功能 | 状态 |
|---|---|
| `/health` 端点（无需认证） | ✅ |
| `/v1/models` 列出所有 CAS 模型 | ✅ |
| `/v1/blobs/{hash}` 下载（支持 Range） | ✅ |
| `/v1/hf-proxy/…` HF 兼容代理（hit/miss pass-through） | ✅ |
| Bearer token 认证 | ✅ |
| IP 允许/拒绝列表 | ✅ |
| TOML 配置文件 | ✅ |
| mDNS 服务广播（`_modeld._tcp`） | ✅ 骨架实现（`discovery.rs`） |
| 路径遍历防护 | ✅ |

### 2.4 modeld-webui

| 功能 | 状态 |
|---|---|
| axum HTTP 服务器（127.0.0.1:8234） | ✅ 骨架 |
| REST API 路由挂载（`/api/v1/`） | ✅ 骨架 |
| WebSocket `/ws` 事件广播 | ✅ 骨架 |
| 静态文件嵌入（rust-embed） | ✅ 骨架 |
| `GET /api/v1/stats` | ✅ 骨架 |
| `GET /api/v1/models[/:hash]` | ✅ 骨架 |
| `GET /api/v1/dupes` / `POST /api/v1/dupes/dedup` | ✅ 骨架 |
| `GET /api/v1/downloads` | ✅ 骨架 |
| `POST /api/v1/scan/trigger` | ✅ 骨架 |
| `POST/GET /api/v1/gc/*` | ✅ 骨架 |
| `GET/PUT /api/v1/settings` | ✅ 骨架 |
| Bearer token 中间件 | ❌ 未实现 |
| Prometheus `/metrics` | ❌ 未实现 |
| 审计日志 | ❌ 未实现 |
| 实际前端 UI 页面 | ❌ 未实现（仅静态文件嵌入框架） |

### 2.5 modeld-client

| 功能 | 状态 |
|---|---|
| `health()` — GET /health | ✅ |
| `list_models()` — GET /v1/models | ✅ |
| `download_blob()` — GET /v1/blobs/{hash}（支持 Range） | ✅ |
| Bearer token | ✅ |

---

## 3. CLI 命令可用性矩阵

| 命令 | 可用性 | 说明 |
|---|---|---|
| `modeld init` | **functional** | |
| `modeld scan` | **functional** | |
| `modeld status` | **functional** | |
| `modeld stats` | **functional** | |
| `modeld dupes` | **functional** | |
| `modeld list` | **functional** | |
| `modeld info` | **functional** | |
| `modeld hash` | **functional** | |
| `modeld dedup` | **partial** | 缺 `--apply`/`--strategy`/`--protect`/`--pin` |
| `modeld quarantine list/cleanup/restore` | **functional** | |
| `modeld hf-check` | **functional** | |
| `modeld hf-download` | **functional** | |
| `modeld hf-setup` | **functional** | |
| `modeld hf-status` | **functional** | |
| `modeld workflow-scan` | **functional** | |
| `modeld workflow-deps` | **functional** | |
| `modeld refs-orphans` | **functional** | |
| `modeld gc` | **functional** | |
| `modeld verify` | **partial** | 缺 `--deep`/`--json`/`repair` |
| `modeld unlink` | **functional** | |
| `modeld proxy start` | **functional** | |
| `modeld proxy discover` | **partial** | mDNS 依赖系统环境 |
| `modeld proxy status` | **functional** | |
| `modeld doctor` | **stub** | 未实现 |
| `modeld store locate/init/verify/migrate` | **stub** | 未实现 |
| `modeld config get/set` | **stub** | 未实现 |
| `modeld tx list/show/rollback/recover/cleanup` | **stub** | 未实现 |
| `modeld db status/backup/restore/migrate/vacuum` | **stub** | 未实现 |
| `modeld tag add/remove/list` | **stub** | 未实现 |
| `modeld note set` | **stub** | 未实现 |
| `modeld pin add/remove/list` | **stub** | 未实现 |
| `modeld favorite add/remove` | **stub** | 未实现 |
| `modeld refs scan/why/orphans/graph` | **partial** | `refs-orphans` 已实现，其他未实现 |
| `modeld serve` | **stub** | 未实现（webui 有骨架，未统一入口） |
| `modeld hf snapshot` | **stub** | 未实现（仅单文件下载） |
| `modeld hf token set/remove/status` | **stub** | 未实现（无 keychain 存储） |
| `modeld hf cache list/verify` | **stub** | 仅通过 `hf-status` 间接查看 |
| `modeld repair` | **stub** | 未实现 |

---

## 4. 缺失功能（对照 doc.md）

### P0 优先级缺口

| 类别 | 缺失内容 |
|---|---|
| **事务管理器（§3.1）** | `modeld tx list/show/rollback/recover/cleanup` CLI 命令；`TransactionManager` 高级 API（rollback、recover 仅在 dedup 引擎内部，未暴露为独立命令） |
| **Crash-safe CAS（§3.2）** | fsync 调用（当前仅 copy + rename，无显式 fsync）；staging 子目录按 tx_id 隔离（当前共享目录）；跨文件系统检测（仅 dedup 有，downloader 无） |
| **Verify/Repair（§3.3）** | `--deep` hash 验证；`--json` 输出；`modeld repair` 命令（删 orphan DB 记录、清 staging、修 quarantine 元数据） |
| **Windows（§5）** | `modeld doctor --windows`；symlink 权限检测已有但未暴露为命令；文件锁检测（`ERROR_SHARING_VIOLATION`）未实现；跨盘自动降级提示 |
| **统一存储路径（§2.3）** | `MODELD_STORE` 环境变量未读取；`modeld store locate/init/verify/migrate`；`modeld config get/set`；`modeld.toml` 配置读取 |
| **Doctor 命令（§2.2）** | `modeld doctor` 完全未实现 |

### P1 优先级缺口

| 类别 | 缺失内容 |
|---|---|
| **Web UI（§10）** | Dashboard/Duplicates/Library/Downloads 实际页面及前端代码；Bearer auth 中间件；`modeld serve` 统一入口 |
| **HF 完整语义（§6）** | `modeld hf snapshot`（批量下载）；`modeld hf token set/remove/status`（keychain 存储）；gated repo 友好错误提示；token 不在 CLI `--token` 参数中泄漏（当前未警告） |
| **扫描增强（§7.1）** | `--incremental/--full/--watch`；inode/file-id 索引；symlink cycle 检测；`--follow-symlinks=false` |
| **引用图（§8）** | `modeld refs scan <path>`（A1111/Forge config）；`modeld refs why <hash>`；`modeld refs graph`；RefStatus 分类 |
| **安全 GC 策略（§8）** | pin 保护（pin/unpin 命令未实现）；unknown reference 跳过策略 |
| **Prometheus 指标（§9.3）** | `/metrics` 端点未实现 |
| **审计日志（§9.3）** | audit.log NDJSON 未实现 |
| **发布工程（§11.1）** | GitHub Release 多平台制品、checksums；`modeld --version` 缺 commit hash、build target |

### P2 优先级缺口

| 类别 | 缺失内容 |
|---|---|
| **标签/收藏（§12.1）** | `modeld tag/note/pin/favorite` 命令；DB v3 schema（`tags` 表、provenance 字段） |
| **License 追踪（§12.2）** | 下载时填写 provenance 字段；license 报告 CSV 导出 |
| **DB 管理命令（§14）** | `modeld db status/backup/restore/migrate/vacuum` |
| **Docker Compose（§9.4）** | 示例配置未提供 |

---

## 5. 风险点

| 风险 | 等级 | 说明 |
|---|---|---|
| **无 fsync 保证** | 高 | `cas.rs` 的 `store()` 仅调用 `fs::copy` + `set_permissions`，无 fsync/fdatasync。断电或 OS crash 可能导致 CAS 对象截断 |
| **WAL 事务粒度粗** | 高 | `wal_transactions` 表的 `operation` 字段只允许 `'dedup'/'download'/'gc'`（CHECK 约束），而 doc.md 要求覆盖 8 种操作类型。直接插入其他操作会报 DB 约束错误 |
| **dedup 默认行为** | 中 | 当前 `dedup` 命令默认是 dry-run，符合 doc.md 要求；但无 `--apply` 参数，用户需要 `--auto` 才能执行，与 doc.md 第 4.1 节的 `--apply` 规范不符 |
| **HF token 明文传递** | 中 | `--token` CLI 参数接受 token，但未对用户打印"建议用环境变量"警告（doc.md §6.1 要求） |
| **默认存储路径不一致** | 中 | `init` 命令默认 `~/.local/share/modeld`（仅 Linux 约定），未区分 macOS/Windows 平台路径；`scan/dedup/gc` 等命令默认 `.modeld`（当前目录），两套逻辑不统一 |
| **proxy 默认绑定 0.0.0.0** | 中 | `ProxyConfig::default()` 的 `bind_address` 是 `"0.0.0.0"`，doc.md §9.1 要求默认绑定 `127.0.0.1` |
| **WebUI 无认证中间件** | 中 | `modeld-webui` 的 `/api/v1/*` 路由无 Bearer token 验证，目前任何人可访问 |
| **net2 future-incompat** | 低 | 传递依赖警告，影响未来 Rust 版本编译 |
| **`modeld --version` 输出不完整** | 低 | 仅输出 CARGO_PKG_VERSION，缺 commit hash 和 build target（doc.md §2.4） |
| **mDNS discovery 未测试跨网络** | 低 | `discovery.rs` 骨架存在，但集成测试只验证本地空返回，不确认 mDNS 广播在真实网络环境中的行为 |

---

## 6. 建议修改顺序

按 doc.md 阶段划分，优先修复 P0 安全/正确性问题：

### Wave 0（立即）：一致性修复（不破坏现有 API）
1. **统一默认存储路径**：实现 `store_path.rs`，读取 `MODELD_STORE` 环境变量，区分平台默认值，所有命令使用同一逻辑（Task 2.1）
2. **proxy 默认绑定 127.0.0.1**：修改 `ProxyConfig::default()` 的 `bind_address`（小改动，高优先级）
3. **dedup 增加 `--apply` 参数**：补充 CLI 参数，与 doc.md 规范一致

### Wave 1：Config 和 Doctor（Task 2.3, 2.4, 6.1）
4. 实现 `modeld.toml` 读写（`config.rs`）
5. 实现 `modeld store locate/init/verify`
6. 实现 `modeld doctor`（包括 Windows 检查项）

### Wave 2：TransactionManager 完整化（Task 2.4, 3.1, 3.3）
7. 扩展 DB schema v2（`op_type`、`affected_paths`、`rollback_plan` 等字段；放宽 `operation` CHECK 约束）
8. 实现 `TransactionManager::rollback()`/`recover()` 独立逻辑
9. 实现 `modeld tx list/show/rollback/recover/cleanup` CLI 命令
10. 实现 `modeld verify --deep`、`--json`、`modeld repair`

### Wave 3：CAS crash-safe 加固（Task 4.1）
11. 在 `cas.rs::store()` 中增加 fsync（staging file + parent dir）
12. 使 staging 子目录按 `tx_id` 隔离（`tmp/cas_staging/<tx_id>/`）
13. 实现跨文件系统检测（统一 dedup 和 downloader）

### Wave 4：WebUI 认证 + Serve 统一（Task 10.1, 10.3）
14. 实现 Bearer token auth 中间件
15. 实现 `modeld serve`（统一 API + WebUI + HF proxy + WebSocket）

### Wave 5：HF 完整语义（Task 7.1, 7.3）
16. 实现 `modeld hf snapshot`
17. 实现 `modeld hf token set/remove/status`（keychain 支持）
18. 实现 gated repo 友好提示
19. 添加 `--token` CLI 参数安全警告

### Wave 6：模型治理（Task 13.x）
20. 实现 DB v3 schema（tags 表 + provenance 字段）
21. 实现 `modeld tag/note/pin/favorite/refs` 命令

### Wave 7：发布工程（Task 6.4）
22. 实现 `build.rs` 捕获 git hash 和 build target
23. 更新 GitHub Actions release workflow 多平台制品
24. 添加 SHA-256 checksums

---

## 7. 模块依赖概览

```
modeld-cli ──depends──> modeld-core
modeld-cli ──depends──> modeld-proxy
modeld-cli ──depends──> modeld-webui
modeld-webui ──depends──> modeld-core
modeld-proxy ──depends──> modeld-core
modeld-proxy ──depends──> modeld-client (间接，通过 ureq)
modeld-client ──depends──> (仅 serde_json / ureq)
modeld-core ──depends──> (blake3, rusqlite, rayon, walkdir, ureq, sha2, uuid, chrono, serde)
```

---

*本报告由 Phase 0 代码审计任务自动生成，禁止在本阶段修改任何现有源码。*
