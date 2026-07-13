# modeld 当前架构描述 (ARCHITECTURE_CURRENT.md)

> 生成日期：2025-07  
> 版本：代码审计基线（Phase 0）  
> 目的：记录现有模块边界，为后续阶段改动提供参考基准

---

## 1. 顶层结构

```
ModelID/                          # 仓库根（品牌名 modeld）
├── Cargo.toml                    # Workspace 配置，5 个 crate 成员
├── crates/
│   ├── modeld-core/              # 核心库（无网络依赖的核心逻辑）
│   ├── modeld-cli/               # 命令行入口二进制
│   ├── modeld-proxy/             # HTTP 代理服务器库 + 二进制
│   ├── modeld-client/            # 轻量级 HTTP 客户端库（连接 proxy）
│   └── modeld-webui/             # axum Web UI 服务库 + 二进制
├── python/                       # Python hook（modeld_hook）
├── docs/                         # 文档目录
└── scripts/                      # 构建/发布脚本
```

---

## 2. Crate 依赖关系图

```
┌─────────────────────────────────────────────┐
│                modeld-cli                   │
│   (src/main.rs — 单文件 ~1200 行)            │
└──────┬──────────┬──────────┬────────────────┘
       │          │          │
       ▼          ▼          ▼
┌──────────┐  ┌──────────┐  ┌──────────────┐
│modeld-   │  │modeld-   │  │modeld-webui  │
│core      │  │proxy     │  │              │
└──────────┘  └────┬─────┘  └──────┬───────┘
                   │               │
                   ▼               ▼
              ┌──────────┐    ┌──────────┐
              │modeld-   │    │modeld-   │
              │core      │    │core      │
              └──────────┘    └──────────┘

modeld-client → (独立，仅使用 ureq + serde_json)
```

**注意**：`modeld-client` 是 proxy 的消费者（用于 CLI `proxy status` 命令），不是 proxy 的依赖项。

---

## 3. modeld-core 内部模块边界

```
modeld-core/src/
├── lib.rs          ← 公共导出（re-export 所有公共类型）
│
├── hash.rs         ← BLAKE3 哈希计算
│   └── Blake3Hash  (newtype: [u8;32])
│       ├── hash_file(path) -> Result<Blake3Hash>
│       ├── from_hex(s)     -> Result<Blake3Hash>
│       ├── as_hex()        -> &str
│       └── prefix()        -> &str  (前两个 hex 字符)
│
├── cas.rs          ← CAS 存储层（只读对象存储）
│   └── CasStore { root: PathBuf }
│       ├── init()                          创建 256 个前缀目录
│       ├── store(src, hash) -> PathBuf     复制 + read-only
│       ├── get(hash) -> Option<PathBuf>    查找
│       ├── contains(hash) -> bool
│       └── path_for_hash(hash) -> PathBuf
│           格式: {root}/cas/blake3/{xx}/{64-char-hex}
│
├── db.rs           ← SQLite 元数据层
│   ├── Database { conn: Connection }
│   │   Schema v1 表：
│   │   ├── models       (blake3_hash, size_bytes, format, arch, category, quarantined_at, ...)
│   │   ├── aliases      (model_hash→FK, path UNIQUE, frontend, alias_type)
│   │   ├── wal_transactions (tx_id UNIQUE, operation IN('dedup','download','gc'), status, ...)
│   │   ├── downloads    (source_url, repo_id, filename, revision, status, bytes_*)
│   │   ├── hf_mappings  (sha256_hash UNIQUE, blake3_hash→FK, repo_id, filename)
│   │   ├── workflows    (path UNIQUE, file_hash, title, ref_count)
│   │   └── workflow_refs (workflow_id→FK, model_hash→FK, ref_type, model_name, resolved)
│   └── 枚举类型: AliasType, Frontend, TransactionStatus, DownloadStatus
│
├── scanner.rs      ← 文件系统扫描
│   └── Scanner { extensions, excluded_dirs, preindexed }
│       └── scan(root, progress_fn) -> Vec<ScannedFile>
│           Phase1: WalkDir 串行收集路径
│           Phase2: Rayon 并行 BLAKE3 哈希
│           增量优化: preindexed 缓存跳过重哈希
│
├── dedup.rs        ← 去重引擎
│   └── DedupEngine { db, store_path, link_capability }
│       ├── find_duplicates() -> Vec<DuplicateGroup>   (批量 DB 查询)
│       ├── select_canonical(files) -> CanonicalSelection
│       │   优先级: CAS内 > oldest mtime > shortest path > alphabetical
│       ├── execute_dedup_group(group, mode) -> DedupGroupResult
│       │   Phase A: WAL pending → copy → WAL copied
│       │   Phase B: rename → CAS, link dups, WAL committed
│       └── recover_incomplete_transactions()
│           pending → cleanup staging → WAL failed
│           copied  → complete Phase B → WAL committed
│
├── links.rs        ← 跨平台链接策略
│   ├── LinkCapability { has_symlink_privilege, primary_filesystem }
│   ├── is_same_volume(p1, p2) -> bool
│   └── create_link(source, target, capability) -> LinkResult
│       优先级: hardlink (同卷) > symlink (跨卷+有权限) > reference-only
│
├── gc.rs           ← 垃圾回收引擎
│   └── GcEngine<'a> { db: &'a mut Database, cas, quarantine }
│       ├── candidates() -> Vec<GcCandidate>   三级分类
│       │   hard_protected: workflow_ref_count > 0
│       │   soft_protected: alias_count > 0
│       │   orphan:         两者均为 0
│       ├── preview() -> GcPreview
│       └── run_safe() -> GcResult
│           quarantine orphans → DB 写回 quarantined_at → 清空 aliases
│
├── quarantine.rs   ← 隔离区管理
│   └── QuarantineManager { quarantine_dir, ttl_days=30 }
│       路径: {store}/quarantine/{hash}.{timestamp}
│       元数据: {store}/quarantine/{hash}.{timestamp}.meta (JSON)
│       ├── quarantine(file, hash, reason, refs) -> PathBuf
│       ├── restore(path) -> PathBuf
│       ├── list() -> Vec<QuarantineEntry>
│       ├── cleanup_expired() -> usize
│       └── delete_permanent(path)
│
├── fsck.rs         ← 一致性检查
│   └── run_fsck(db, store_path) -> FsckReport
│       检查项:
│       1. missing_cas: DB有记录但CAS文件消失
│       2. dangling_aliases: alias路径不存在
│       3. size_mismatches: CAS文件大小与DB不符
│       4. orphan_cas: CAS文件无DB记录
│
├── downloader.rs   ← HF 文件下载
│   └── Downloader { store_path, hf_token, hf_base_url }
│       ├── download_hf_file(db, repo_id, filename, revision, progress)
│       │   1. HEAD 请求获取 sha256/size
│       │   2. 检查 HF cache 和 SHA256→BLAKE3 映射
│       │   3. DB 记录下载意图
│       │   4. 流式下载到 tmp/downloads/{stable-part-name}.part
│       │   5. SHA256 验证（Range 续传支持 206 检测）
│       │   6. BLAKE3 哈希
│       │   7. CAS 存储 + DB 入库
│       │   8. HF cache entry 创建
│       │   9. HfCache alias 写入（防 GC 孤儿）
│       └── fetch_hf_metadata(repo_id, filename, revision)
│
├── hf_cache.rs     ← 伪 HF cache 目录管理
│   └── HfCache { root: {store}/hf_cache, cas }
│       目录结构: hub/models--{org}--{model}/blobs/{sha256}
│                                          /snapshots/{rev}/{filename}
│                                          /refs/{branch}
│       ├── create_cache_entry(repo, file, rev, sha256, blake3, branch)
│       └── check_cache(repo, rev, file) -> bool
│
├── workflow.rs     ← ComfyUI workflow 解析
│   ├── parse_workflow(path) -> ParsedWorkflow
│   │   支持 17 种 loader 节点类型
│   │   inputs 数组格式 + inputs 对象格式 + widgets_values 后备
│   ├── find_workflow_files(dir) -> Vec<PathBuf>
│   ├── index_workflow(db, path, lookup) -> (wf_id, resolved, unresolved)
│   └── build_model_lookup(db) -> HashMap<String, Blake3Hash>
│       以 alias 路径 basename 为键（正确解析 workflow 引用）
│
├── unlink.rs       ← 链接恢复（hardlink/symlink → 独立副本）
├── i18n.rs         ← 中/英文国际化（t/tf 函数）
└── (无 tx.rs / store_path.rs / config.rs / doctor.rs / governance.rs / platform.rs)
    ↑ 这些是后续阶段需要新增的模块
```

---

## 4. modeld-cli 结构

```
modeld-cli/src/main.rs  (~1200 行，单文件)
│
├── Commands 枚举
│   ├── Init, Scan, Status, Stats, Dupes, List, Info, Hash
│   ├── Dedup { dry_run, auto, report }   ← 无 --apply/--strategy
│   ├── Quarantine { action: QuarantineAction }
│   ├── HfCheck, HfDownload, HfSetup, HfStatus
│   ├── WorkflowScan, WorkflowDeps, RefsOrphans
│   ├── Gc { preview, cleanup_quarantine, cleanup_tmp }
│   ├── Verify { json }
│   ├── Unlink
│   └── Proxy { action: ProxyAction }
│       └── ProxyAction: Start | Discover | Status
│
├── 共享逻辑
│   ├── require_store_db(store_path) → 检查 modeld.db 是否存在
│   └── parse_size(input) → 解析 "100MB" 等格式
│
└── 注意事项
    ├── store 路径: init 默认 ~/.local/share/modeld，其他命令默认 .modeld
    ├── 无 MODELD_STORE 环境变量支持
    └── 无 modeld.toml 配置读取
```

---

## 5. modeld-proxy 结构

```
modeld-proxy/src/
├── lib.rs          ← 公共导出
├── server.rs       ← ProxyServer (tiny_http 同步服务器)
│   路由表:
│   GET /health                              → json_health() [无认证]
│   GET /v1/models                           → handle_list_models()
│   GET /v1/blobs/{hash}                     → handle_get_blob() [Range支持]
│   GET /v1/hf-proxy/{org}/{repo}/resolve/{rev}/{file} → handle_hf_proxy()
│   其他                                     → 404
│
├── auth.rs         ← Bearer token + IP 过滤中间件
│   check_auth(token, config) → bool
│   check_ip(peer, config)    → bool（CIDR 匹配）
│
├── config.rs       ← modeld.toml [proxy] 节 TOML 解析
│   ProxyConfig { port=8234, bind_address="0.0.0.0", auth, network }
│   ⚠️ 默认 bind_address="0.0.0.0"（应改为 "127.0.0.1"）
│
├── range.rs        ← HTTP Range header 解析
│   parse_range(header, total_len) → Option<(start, end)>
│
└── discovery.rs    ← mDNS 服务广播（_modeld._tcp）
    MdnsAnnouncer::start() / discover() [骨架实现]
```

---

## 6. modeld-webui 结构

```
modeld-webui/src/
├── lib.rs          ← pub mod 导出
├── main.rs         ← tokio async main
├── server.rs       ← axum Router + WebUiConfig
│   绑定: 127.0.0.1:8234
│   路由: /api/v1/* (REST) + /ws (WebSocket) + /* (静态文件)
│   ⚠️ 无 Bearer token 认证中间件
│   ⚠️ CORS 设置为 permissive（开发便利，生产需改）
│
├── state.rs        ← AppState { store_path, db_path, event_tx }
│   WsEvent 枚举 (ScanProgress/DedupProgress/DownloadProgress/GcProgress/...)
│
├── ws.rs           ← WebSocket upgrade handler (axum)
│   使用 tokio::sync::broadcast，容量未明确（需查 state.rs）
│
├── static_files.rs ← rust-embed 静态文件服务
│
├── error.rs        ← axum 错误类型
│
└── api/
    ├── mod.rs      ← 路由注册: /stats /models /dupes /downloads /scan /gc /settings
    ├── stats.rs    ← GET /api/v1/stats    → 骨架
    ├── models.rs   ← GET /api/v1/models   → 骨架
    ├── dupes.rs    ← GET/POST /api/v1/dupes → 骨架
    ├── downloads.rs← GET /api/v1/downloads → 骨架
    ├── scan.rs     ← POST /api/v1/scan/trigger → 骨架
    ├── gc.rs       ← POST /api/v1/gc/preview/run → 骨架
    └── settings.rs ← GET/PUT /api/v1/settings → 骨架
    
    ⚠️ 缺失路由: /api/v1/quarantine, /api/v1/tags, /api/v1/refs/:hash,
                 GET /metrics, /v1/hf-proxy/* (在proxy crate而非webui)
```

---

## 7. modeld-client 结构

```
modeld-client/src/
├── lib.rs          ← 公共导出
└── client.rs       ← ModeldClient { base_url, token }
    ├── health()          → GET /health
    ├── list_models()     → GET /v1/models
    ├── download_blob()   → GET /v1/blobs/{hash} (支持 Range)
    └── hf_proxy_url()    → 构造 /v1/hf-proxy/... URL
```

---

## 8. Python Hook 结构

```
python/
└── modeld_hook/    ← Python 包（monkey-patch HF 下载）
    ⚠️ 注意: Python hook 代码未在本次 Rust 审计中深入分析
    功能: 拦截 huggingface_hub 下载，重定向到 modeld proxy
    缺失: activate()/deactivate()/status() API
          MODELD_HOOK_DISABLE/MODELD_HOOK_LOG 环境变量支持
```

---

## 9. 数据流图

```
用户调用 modeld scan /path/to/models
         │
         ▼
    Scanner.scan()
    ├── WalkDir (串行遍历)
    └── Rayon 并行 BLAKE3 哈希
         │
         ▼
    CasStore.store(file, hash)
    ├── copy file → {store}/cas/blake3/{xx}/{hash}
    └── set_permissions(readonly)
         │
         ▼
    Database.insert_or_update_model(hash, size, ...)
    Database.insert_alias(hash, path, frontend, alias_type)
         │
         ▼
    [modeld dupes] → DedupEngine.find_duplicates() → DB 批量查询
         │
         ▼
    [modeld dedup --auto]
    ├── WAL: INSERT pending
    ├── copy canonical → staging
    ├── WAL: UPDATE copied
    ├── rename staging → CAS
    ├── create_link(dup → CAS)
    └── WAL: UPDATE committed → DELETE
         │
         ▼
    [modeld gc]
    ├── 分类: hard/soft/orphan
    ├── QuarantineManager.quarantine(cas_file)
    └── DB.quarantine_model() + DB.delete_aliases_for_model()

HF 下载流:
用户 → modeld hf-download org/model file
     → Downloader.download_hf_file()
       ├── HF HEAD (sha256, size)
       ├── 检查 hf_cache + sha256→blake3 映射
       ├── HTTP GET (Range resume)
       ├── SHA256 验证
       ├── BLAKE3 计算
       ├── CasStore.store()
       ├── HfCache.create_cache_entry()
       └── DB.insert_alias(HfCache, Symlink)

代理请求流:
HF客户端(diffusers/transformers)
→ MODELD_PROXY=http://localhost:8234
→ GET /v1/hf-proxy/org/model/resolve/main/file.safetensors
→ ProxyServer.handle_hf_proxy()
  ├── [cache hit] HfCache.check_cache() → stream from snapshot path
  └── [cache miss] Downloader.download_hf_file() → stream from CAS
```

---

## 10. 关键约束与不变量

| 约束 | 位置 | 说明 |
|---|---|---|
| CAS 对象只读 | `cas.rs::store()` | Unix: 0o444；Windows: readonly=true |
| BLAKE3 hash = 64 hex chars | `db.rs` CHECK | `CHECK (length(blake3_hash) = 64)` |
| alias.path UNIQUE | `db.rs` | 防止同路径重复注册 |
| operation IN(...) | `db.rs` CHECK | 限制为 'dedup'/'download'/'gc'（**当前版本限制**） |
| 去重默认 dry-run | `dedup.rs`、`cli::main.rs` | 无 `--auto` 则不修改文件 |
| WAL 恢复 | `dedup.rs::recover_incomplete_transactions()` | pending→fail+清 staging；copied→完成 Phase B |
| GC 不删除 hard-referenced 模型 | `gc.rs::run_safe()` | workflow_ref_count > 0 → skip |

---

## 11. 尚未创建的模块（计划中）

| 模块 | 对应任务 | 说明 |
|---|---|---|
| `modeld-core/src/store_path.rs` | Task 2.1 | 跨平台存储路径解析 |
| `modeld-core/src/config.rs` | Task 2.3 | modeld.toml 读写 |
| `modeld-core/src/tx.rs` | Task 3.1 | 独立 TransactionManager |
| `modeld-core/src/platform.rs` | Task 3.3 | 平台能力检测（Windows 专项） |
| `modeld-core/src/doctor.rs` | Task 6.1 | 环境诊断 |
| `modeld-core/src/audit.rs` | Task 9.1 | NDJSON 审计日志 |
| `modeld-core/src/governance.rs` | Task 13.1 | 标签/收藏/Pin/Provenance |
| `modeld-webui/src/metrics.rs` | Task 9.2 | Prometheus 指标 |

---

*本文档由 Phase 0 代码审计任务自动生成，反映代码库的当前状态，不包含任何预期或计划中的修改。*
