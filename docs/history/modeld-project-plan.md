# modeld — AI 模型层 CAS 基础设施项目计划

> **项目定位**：AI 模型世界里的 `containerd` + `git-lfs` + `nix store`
> **核心价值**：统一管理所有 AI 前端（ComfyUI / A1111 / Forge / InvokeAI）的模型存储，通过内容寻址消除重复，节省 TB 级磁盘空间。

---

## 目录

1. [项目总览](#1-项目总览)
2. [技术架构](#2-技术架构)
3. [技术栈决策](#3-技术栈决策)
4. [Phase 0 — 架构设计与 RFC](#4-phase-0--架构设计与-rfc)
5. [Phase 1 — Core Scanner MVP](#5-phase-1--core-scanner-mvp)
6. [Phase 2 — CAS Store & Dedup](#6-phase-2--cas-store--dedup)
7. [Phase 3 — HF 拦截层](#7-phase-3--hf-拦截层)
8. [Phase 4 — Workflow & Reference Graph](#8-phase-4--workflow--reference-graph)
9. [Phase 5 — Local Registry & Proxy](#9-phase-5--local-registry--proxy)
10. [Phase 6 — Advanced Dedup（长期）](#10-phase-6--advanced-dedup长期)
11. [里程碑总览](#11-里程碑总览)
12. [风险登记册](#12-风险登记册)
13. [开源运营策略](#13-开源运营策略)

---

## 1. 项目总览

### 1.1 解决的核心问题

| 问题 | 现状 | 目标状态 |
|---|---|---|
| 重复存储 | 同一模型在 ComfyUI / Forge / A1111 各存一份 | 唯一 CAS 对象，hardlink 共享 |
| 无统一索引 | LoRA / checkpoint 散落各目录 | 统一元数据数据库 |
| 重复下载 | 每个前端各自从 HF 下载 | 下载前 hash 检查，命中则跳过 |
| 误删风险 | 不知道模型被哪些 workflow 引用 | ref graph + safe GC |
| 局域网浪费 | 多台机器重复下载 | Local HF Proxy，一次下载全网共享 |

### 1.2 关键价值主张

```
用户不需要修改任何现有配置
即可自动节省 TB 级磁盘空间
```

### 1.3 项目边界（Scope）

**In Scope**
- 本地文件系统管理（Windows / Linux / macOS）
- safetensors / gguf / ckpt 模型格式
- HuggingFace 生态拦截
- ComfyUI workflow 依赖分析
- 局域网模型代理

**Out of Scope（当前阶段）**
- 云端存储同步
- 模型训练流程集成
- Web UI（Tauri 应用为独立子项目）

---

## 2. 技术架构

```
┌─────────────────────────────────────────────────────┐
│            AI 前端层                                  │
│   ComfyUI    Forge    A1111    InvokeAI    diffusers  │
└──────────────┬───────────────────────┬───────────────┘
               │                       │
     HF Hook (Python shim)      Virtual Model FS
     ~/.cache/huggingface        /virtual/comfy/
               │                       │
               └──────────┬────────────┘
                           │
               ┌───────────▼────────────┐
               │         modeld          │
               │  ┌─────────────────┐   │
               │  │ Download Manager│   │
               │  │ CAS Storage     │   │
               │  │ Dedup Engine    │   │
               │  │ Metadata Index  │   │
               │  │ Ref Tracker     │   │
               │  │ Workflow Parser │   │
               │  └─────────────────┘   │
               └───────────┬────────────┘
                           │
               ┌───────────▼────────────┐
               │     Content Store       │
               │  /store/blake3/ab/cd/   │
               │  (BLAKE3-addressed)     │
               └────────────────────────┘
```

### 2.1 CAS 存储布局

```
$MODELD_STORE/
├── cas/
│   └── blake3/
│       ├── ab/
│       │   └── abcdef1234...  (实际文件，不可变)
│       └── cd/
│           └── cdef5678...
├── virtual/
│   ├── comfyui/
│   │   ├── checkpoints/    → (hardlinks/symlinks → cas/)
│   │   └── loras/
│   ├── forge/
│   └── a1111/
├── tmp/
│   └── downloads/          (下载中间态)
└── modeld.db               (SQLite 元数据库)
```

### 2.2 元数据 Schema（核心）

```sql
-- 模型对象（CAS 中心表）
CREATE TABLE models (
    id          INTEGER PRIMARY KEY,
    blake3_hash TEXT UNIQUE NOT NULL,
    size_bytes  INTEGER NOT NULL,
    format      TEXT,           -- safetensors | gguf | ckpt | bin
    arch        TEXT,           -- sd1 | sdxl | flux | llm | ...
    base_model  TEXT,
    created_at  TEXT,
    last_seen   TEXT
);

-- 文件路径别名（多路径 → 同一 hash）
CREATE TABLE aliases (
    id          INTEGER PRIMARY KEY,
    model_hash  TEXT REFERENCES models(blake3_hash),
    path        TEXT NOT NULL,
    frontend    TEXT,           -- comfyui | forge | a1111 | hf_cache
    alias_type  TEXT            -- hardlink | symlink | copy | original
);

-- Workflow 引用（防误删核心）
CREATE TABLE refs (
    id           INTEGER PRIMARY KEY,
    model_hash   TEXT REFERENCES models(blake3_hash),
    ref_source   TEXT NOT NULL, -- workflow 文件路径
    ref_type     TEXT,          -- lora | checkpoint | vae | controlnet
    last_checked TEXT
);

-- 下载任务记录
CREATE TABLE downloads (
    id           INTEGER PRIMARY KEY,
    model_hash   TEXT,
    source_url   TEXT NOT NULL,
    status       TEXT,          -- pending | downloading | done | failed
    bytes_total  INTEGER,
    bytes_done   INTEGER,
    started_at   TEXT,
    finished_at  TEXT
);
```

---

## 3. 技术栈决策

| 模块 | 选型 | 决策理由 |
|---|---|---|
| Core daemon | **Rust** | mmap + async IO + BLAKE3，无 GC 停顿；GB 级文件操作必须系统级控制 |
| CLI | **Rust（clap）** | 与 daemon 共用 crate，零额外依赖 |
| Async runtime | **Tokio** | 成熟，生态完整，async-std 无明显优势 |
| Hash | **BLAKE3** | 比 SHA256 快 3-5x，支持并行 chunk，适合大文件 |
| 元数据 DB | **SQLite + rusqlite** | 无服务端依赖，部署极简，FTS5 支持全文搜索 |
| HF Hook | **Python** | 必须与 Python AI 生态对接，无法用 Rust 替代 |
| 配置文件 | **TOML** | Rust 生态标准，人类可读 |
| 文件监听 | **notify** | 跨平台，支持 Windows IOCP / Linux inotify |
| 桌面 UI（可选）| **Tauri** | 复用 Rust daemon，前端 Web 技术栈 |
| IPC | **Unix socket / Named pipe** | CLI 与 daemon 通信，跨平台 |

### 3.1 Repository 布局

```
modeld/
├── Cargo.toml                  (workspace)
├── crates/
│   ├── modeld-core/            (核心库：CAS、dedup、db)
│   ├── modeld-daemon/          (后台服务)
│   ├── modeld-cli/             (命令行工具)
│   ├── modeld-scanner/         (文件扫描、hash)
│   ├── modeld-metadata/        (safetensors/gguf parser)
│   └── modeld-proxy/           (HF proxy server)
├── python/
│   ├── modeld_hook/            (HF 拦截层)
│   └── tests/
├── docs/
│   ├── rfcs/
│   │   ├── 0001-storage-layout.md
│   │   ├── 0002-hash-strategy.md
│   │   ├── 0003-ref-model.md
│   │   ├── 0004-dedup-strategy.md
│   │   ├── 0005-windows-compat.md
│   │   ├── 0006-virtual-fs.md
│   │   └── 0007-hf-interception.md
│   └── architecture.md
├── tests/
│   └── integration/
└── README.md
```

---

## 4. Phase 0 — 架构设计与 RFC

> **工期**：2～3 周
> **原则**：不写核心业务代码，只做系统设计

### 4.1 目标

在任何人能完整描述系统的所有组件、边界、约束之前，不开始实现。

### 4.2 交付物清单

#### RFC 文档（docs/rfcs/）

| RFC 编号 | 主题 | 核心问题 |
|---|---|---|
| 0001 | Storage Layout | CAS 目录结构；chunk 边界预留；未来 OCI 兼容 |
| 0002 | Hash Strategy | BLAKE3 参数；增量 hash；chunk size |
| 0003 | Ref Model | workflow → model 引用图 schema；GC 触发条件 |
| 0004 | Dedup Strategy | identical file dedup；transactional move 两阶段提交 |
| 0005 | Windows Compatibility | symlink 权限问题；跨卷 hardlink 限制；junction point 降级策略 |
| 0006 | Virtual FS | hardlink / symlink 目录结构；各前端 mount 点 |
| 0007 | HF Interception | monkeypatch vs `HF_HOME` 环境变量方案对比；fake cache layout |

#### 关键设计决策记录（ADR）

每个重大选择写一个 ADR（Architecture Decision Record），格式：
- **背景**：为什么需要做这个决策
- **选项**：列出所有备选方案
- **决策**：最终选什么，为什么
- **后果**：接受了哪些权衡

### 4.3 Windows 兼容性专项研究

这是 Phase 0 最重要的输出之一，需要明确回答：

| 场景 | 技术方案 | 降级方案 |
|---|---|---|
| 同盘模型 dedup | NTFS Hardlink | 无需降级 |
| 跨盘模型 dedup | ❌ Hardlink 不可跨卷 | NTFS Junction / 引用计数 + 延迟 copy |
| 虚拟目录 | NTFS Symlink（需开发者模式）| NTFS Junction Point |
| 无权限环境 | - | 仅记录引用，不做物理 dedup |

### 4.4 成功标准

- [ ] 所有 7 份 RFC 完稿并 review
- [ ] Windows 兼容策略明确无遗漏
- [ ] SQLite schema 完成 v1 版本
- [ ] CAS layout spec 文档化
- [ ] 团队/自己能"完整描述系统"而不需查阅文档

---

## 5. Phase 1 — Core Scanner MVP

> **工期**：2～3 周
> **目标**：建立最核心的数据层；第一个可以公开发布的工具

### 5.1 功能范围

#### F1.1 文件扫描器

```bash
modeld scan <path>          # 扫描指定目录
modeld scan D:/models       # Windows 路径支持
modeld scan --watch         # 持续监听变化
```

支持格式：`.safetensors` / `.gguf` / `.ckpt` / `.pt` / `.bin`

扫描策略：
- 首次扫描：全量 hash
- 增量扫描：mtime + size 快速判断是否需要重新 hash
- 进度显示：文件数 / 已处理 / 预计剩余时间
- 断点续扫：意外中断后从断点继续

#### F1.2 BLAKE3 Hash 引擎

```
性能目标：NVMe SSD 上 ≥ 2GB/s（与存储带宽接近）
```

实现要点：
- mmap 大文件（避免用户态缓冲区拷贝）
- Rayon 并行 chunk hash
- 支持流式计算（for 超大文件 / 网络下载）
- 缓存：hash 结果 + (mtime, size) 绑定，文件未变不重新 hash

#### F1.3 SQLite Registry

记录每个已知文件的：
- `blake3_hash`（唯一标识）
- `size_bytes`
- `path`（可多条，same hash 不同路径）
- `format` / `arch`（从 metadata 解析）
- `scan_time`

WAL 模式开启，支持并发读。

#### F1.4 Duplicate Detection

```bash
modeld dupes                         # 列出所有重复组
modeld dupes --min-size 100MB        # 过滤小文件
modeld dupes --json                  # JSON 输出（供脚本使用）
```

输出格式示例：
```
🔴 Duplicate Group (×3, 12.4 GB wasted)
   blake3: abcdef1234...
   ├── D:/ComfyUI/models/checkpoints/v1-5-pruned.safetensors
   ├── D:/Forge/models/Stable-diffusion/v1-5-pruned.safetensors
   └── D:/A1111/models/Stable-diffusion/v1-5-pruned.safetensors
```

#### F1.5 safetensors Metadata Parser

从文件头（无需加载全部权重）解析：
- `modelspec.architecture`
- `modelspec.sai_model_spec`
- tensor 名称列表（推断 arch）
- 文件大小分布

**防御性原则**：metadata 缺失 / 格式异常 → 记录 `unknown`，不报错不中断扫描。

### 5.2 CLI 命令规范

```bash
# 基础操作
modeld scan [path]           # 扫描并建立 registry
modeld stats                 # 总体统计
modeld dupes                 # 重复文件报告
modeld list [--format sdxl]  # 列出所有已知模型
modeld info <hash|path>      # 单个文件详情

# 输出控制
modeld --json <cmd>          # JSON 输出
modeld --quiet <cmd>         # 仅输出结果，无进度
```

### 5.3 性能目标

| 场景 | 目标 |
|---|---|
| 首次扫描 1TB（NVMe）| ≤ 15 分钟 |
| 增量扫描（无变化）| ≤ 30 秒 |
| `modeld dupes`（已扫描）| ≤ 1 秒 |
| 内存占用 | ≤ 200MB（扫描中） |

### 5.4 成功标准

- [ ] 能扫描包含 500+ 模型的 TB 级目录不崩溃
- [ ] 正确识别重复文件（hash 一致性测试）
- [ ] Windows / Linux / macOS 三平台 CI 通过
- [ ] `modeld stats` 能展示"你有 XXX GB 重复文件"
- [ ] 用户公开发布，收集真实反馈

---

## 6. Phase 2 — CAS Store & Dedup

> **工期**：6～8 周（含充分测试）
> **目标**：第一个真正有"革命性价值"的阶段——实际节省磁盘空间

### 6.1 功能范围

#### F2.1 CAS Store

将扫描到的模型文件迁移进内容寻址存储：

```
/store/blake3/
└── ab/
    └── abcdef1234...     ← 以 hash 前两位分桶，避免单目录过大
```

文件属性：
- **只读**（chmod 444 / Windows read-only attribute）
- **不可变**：一旦存入，不修改、不移动
- 文件名 = blake3 hash 全串

#### F2.2 Virtual Model Directory

为每个前端创建虚拟目录：

```bash
modeld link comfyui --model-dir D:/ComfyUI/models
```

在 ComfyUI 的模型目录下创建 hardlink（同盘）或 symlink（跨盘），指向 CAS 中对应的文件。前端无需任何配置改动。

**链接策略优先级**：
1. 同卷 → NTFS Hardlink / Unix Hardlink（最佳：节省 inode，无权限要求）
2. 跨卷 + 有权限 → Symlink
3. 跨卷 + 无权限 → Junction Point（Windows）/ 引用记录（不做物理链接）

#### F2.3 Dedup Engine

```bash
modeld dedup                    # 交互式，逐组确认
modeld dedup --dry-run          # 预览，不修改
modeld dedup --auto             # 自动执行（无确认）
modeld dedup --report           # 仅输出报告
```

流程：
```
1. 扫描 → 找到重复组
2. 选定 "canonical" 路径（策略：优先 CAS 内已有，否则选最旧）
3. 迁移 canonical 到 CAS（若不在其中）
4. 原路径替换为 hardlink/symlink → CAS 中对象
5. 写 aliases 表
6. 事务提交
```

#### F2.4 Transactional Move（关键实现细节）

由于 OS 不提供跨卷原子 move，需要自实现两阶段提交：

```
Phase A：Prepare
  1. 写 WAL 记录（destination, source, status=pending）
  2. 复制文件到 CAS（tmp 目录）
  3. 验证 hash 一致性
  4. 更新 WAL（status=copied）

Phase B：Commit
  5. 原子 rename（tmp → final，同卷可用 rename(2)）
  6. 在源路径创建 hardlink/symlink
  7. 更新 WAL（status=committed）
  8. 删除源文件（ref count 确认为 0 后）

崩溃恢复：
  - 启动时扫描 WAL，找到 pending/copied 状态的记录
  - 从中断点继续或回滚
```

#### F2.5 Ref Counting & GC

```bash
modeld gc --dry-run             # 列出可回收对象
modeld gc                       # 执行垃圾回收
```

GC 规则：
- ref count = 0（无 alias，无 workflow ref）→ 可回收
- 新增"quarantine"机制：ref count 降为 0 后先进 quarantine 目录，30 天后才真正删除

### 6.2 成功标准

- [ ] `modeld dedup --dry-run` 正确预测节省空间
- [ ] dedup 后 ComfyUI / Forge 均可正常加载模型
- [ ] 崩溃恢复测试通过（模拟各阶段断电）
- [ ] Windows 跨卷场景正确降级
- [ ] 节省空间实测报告（"Saved 1.2TB"）

---

## 7. Phase 3 — HF 拦截层

> **工期**：4～8 周（兼容性测试量大）
> **目标**：透明切入 Python AI 生态，用户无需修改代码

### 7.1 接入方案对比

| 方案 | 优点 | 缺点 |
|---|---|---|
| monkeypatch `hf_hub_download` | 拦截精准 | huggingface_hub 频繁更新，维护成本高 |
| `HF_HOME` 环境变量 + fake cache | 稳定，无 patch | 需模拟完整 HF cache 目录结构 |
| 两者结合 | 覆盖最广 | 复杂度高 |

**推荐方案**：优先 `HF_HOME` + fake cache layout，monkeypatch 作为补充拦截层。

### 7.2 功能范围

#### F3.1 Fake HF Cache Layout

完整模拟 `~/.cache/huggingface/hub/` 目录结构，使所有读取该路径的框架无感知：

```
$MODELD_HF_CACHE/hub/
└── models--{org}--{model}/
    ├── blobs/
    │   └── {sha256}          → (symlink → CAS blake3 对象)
    └── snapshots/
        └── {revision}/
            └── model.safetensors → ../../../blobs/{sha256}
```

#### F3.2 Download Dedup

下载前检查：
1. 查 SQLite `downloads` 表，同 URL hash 已存在 → 直接返回路径
2. HF 文件 hash → BLAKE3 转换（HF 用 sha256，需要映射表）
3. 下载中：写到 `tmp/`，完成后 hash 验证，再 rename 进 CAS

#### F3.3 Resumable Downloads

- 支持 HTTP Range 断点续传
- 下载状态持久化到 SQLite
- 网络中断后自动重试（指数退避）

#### F3.4 安装与激活

```bash
pip install modeld-hook

# 方案 A：环境变量激活（推荐）
export HF_HOME="$(modeld hf-cache-path)"

# 方案 B：自动注入（写入 sitecustomize.py）
modeld hook install

# 方案 C：显式导入
python -c "import modeld_hook; modeld_hook.activate()"
```

### 7.3 兼容性测试矩阵

| 框架 | 测试版本 | 关键测试点 |
|---|---|---|
| diffusers | latest | `from_pretrained()` 下载 + 缓存 |
| transformers | latest | `AutoModel.from_pretrained()` |
| ComfyUI | latest | 内置下载器 |
| Forge | latest | 模型加载路径 |
| huggingface_hub | 0.20+ | `hf_hub_download()` / `snapshot_download()` |

### 7.4 成功标准

- [ ] `pip install modeld-hook` 后，diffusers / transformers 下载无感知重定向
- [ ] 同一模型不重复下载（缓存命中测试）
- [ ] 断点续传测试通过
- [ ] 5 个主流框架集成测试全部通过

---

## 8. Phase 4 — Workflow & Reference Graph

> **工期**：4～6 周
> **目标**：系统智能化——知道每个模型被谁用了

### 8.1 功能范围

#### F4.1 ComfyUI Workflow Parser

解析 `workflow.json`，提取模型依赖：

```bash
modeld workflow scan ~/.comfyui/workflows/
modeld workflow deps my_workflow.json
```

注意：ComfyUI workflow JSON 无正式规范，需处理：
- 不同版本的节点格式
- 自定义节点引入的模型引用
- 相对路径 vs 绝对路径 vs 模型名称（无路径）

解析策略：基于已知节点类型的 heuristic，未知节点记录为 `unresolved_ref`。

#### F4.2 Dependency Graph

```
workflow_A.json
  ├── checkpoint: v1-5-pruned.safetensors → [blake3: abcdef...]
  ├── lora: character_lora.safetensors    → [blake3: 123456...]
  └── vae: vae-ft-mse.safetensors         → [blake3: 789abc...]
```

图查询接口：
```bash
modeld refs list <model_hash>        # 哪些 workflow 引用了这个模型
modeld refs orphans                  # 没有任何引用的模型
modeld workflow using <model_path>   # 这个模型被哪些 workflow 使用
```

#### F4.3 Safe GC

```bash
modeld gc --safe                     # 只回收零引用且非 quarantine 的对象
modeld gc --preview                  # 预览可回收列表
modeld gc --force <hash>             # 强制删除（跳过 ref check，需二次确认）
```

GC 保护层级：
1. **硬保护**：任何 workflow ref → 不可 GC
2. **软保护**：alias 存在（前端虚拟目录链接）→ 警告并确认
3. **无保护**：ref count = 0，alias = 0 → 进 quarantine
4. **最终删除**：quarantine 超过 TTL（默认 30 天）

#### F4.4 Orphan Detection

```bash
modeld orphans                       # 未被任何 workflow 引用的模型
modeld orphans --since 30d           # 30 天内未被访问
modeld orphans --size-gt 1GB         # 大于 1GB 的孤儿
```

### 8.2 成功标准

- [ ] 正确解析 100 个真实 ComfyUI workflow 的依赖（测试集）
- [ ] `modeld gc --safe` 不删除任何被引用的模型
- [ ] orphan 检测准确率 ≥ 95%
- [ ] 误删恢复机制（quarantine）可验证

---

## 9. Phase 5 — Local Registry & Proxy

> **工期**：6～8 周
> **目标**：局域网一次下载，全网共享

### 9.1 功能范围

#### F5.1 Local HF Proxy Server

```bash
modeld proxy start --port 8234
# 使用方：
export HF_ENDPOINT=http://192.168.1.100:8234
```

代理行为：
- **Cache Hit**：从本地 CAS 直接返回，速度 = 内网带宽
- **Cache Miss**：从 HuggingFace 下载，写入本地 CAS，同时返回给请求方
- **兼容性**：完全兼容 HF Hub API，无需客户端修改

#### F5.2 局域网发现

```bash
modeld proxy discover                # 自动发现局域网内的 modeld proxy
```

使用 mDNS 广播，局域网内的 modeld 实例自动互相发现。

#### F5.3 Chunked Transfer

大文件传输优化：
- HTTP Range 支持
- 并发 chunk 传输
- 传输中断自动续传

#### F5.4 访问控制

```toml
# modeld.toml
[proxy]
port = 8234
allow_networks = ["192.168.1.0/24"]
require_auth = false        # 局域网内可关闭
token = ""                  # 可选 Bearer token
```

### 9.2 成功标准

- [ ] 局域网第二台机器通过 proxy 下载模型，速度达到内网带宽
- [ ] 中断续传测试通过
- [ ] `HF_ENDPOINT` 环境变量切换后，所有 HF 客户端透明使用本地 proxy

---

## 10. Phase 6 — Advanced Dedup（长期）

> **工期**：长期演进
> **目标**：进入 AI infra 平台级项目

### 10.1 功能方向

#### F6.1 Chunk-level Dedup

相同的权重 chunk 在不同模型间共享存储（类似 btrfs 的 block-level dedup）。

潜在收益：微调模型 vs base 模型之间，大量 chunk 相同，可节省 60-80% 空间。

技术挑战：safetensors 的 chunk 边界对齐；chunk 大小选择（16MB？64MB？）。

#### F6.2 OCI-style Model Layers

将模型存储为类似 OCI 镜像的 layer 结构，为未来容器化 AI 运行时做准备。

#### F6.3 Distributed Storage

跨机器 CAS 同步，支持：
- 主从复制（NAS 主，工作站从）
- P2P 分布（局域网多工作站互为副本）

#### F6.4 Remote Refs

支持远程 CAS 作为下载源（类似 OCI registry pull）。

---

## 11. 里程碑总览

| Phase | 交付物 | 用户价值 | 预计工期 |
|---|---|---|---|
| Phase 0 | RFC 文档集 + Schema | 无（内部） | 2-3 周 |
| Phase 1 | `modeld scan / dupes` | "你有 XXX GB 重复" | 2-3 周 |
| Phase 2 | CAS + dedup | 实际节省 TB 级空间 | 6-8 周 |
| Phase 3 | HF 拦截层 | 透明切入生态 | 4-8 周 |
| Phase 4 | Workflow ref graph | 安全 GC | 4-6 周 |
| Phase 5 | Local proxy | 局域网共享 | 6-8 周 |
| Phase 6 | Chunk dedup | 平台级能力 | 长期 |

**累计工期估算**：Phase 0～5 约 **7～9 个月**（含测试与缓冲）

---

## 12. 风险登记册

| 风险 | 概率 | 影响 | 缓解策略 |
|---|---|---|---|
| Windows symlink 需要管理员权限 | 高 | 中 | 降级到 junction point；文档明确说明 |
| 跨卷 hardlink 不支持 | 确定 | 中 | Phase 0 明确 fallback 策略 |
| HF monkeypatch 因版本更新失效 | 中 | 高 | 优先 `HF_HOME` 方案；监控 HF 发布 |
| safetensors metadata 格式不规范 | 高 | 低 | 防御性解析；缺失视为 unknown |
| ComfyUI workflow JSON 无正式规范 | 确定 | 中 | heuristic 解析 + 人工验证测试集 |
| transactional move 崩溃恢复复杂 | 中 | 高 | 充分测试 WAL；quarantine 兜底 |
| 单人维护生态碎片化压力 | 高 | 中 | 早期建立贡献者机制；模块化架构 |

---

## 13. 开源运营策略

### 13.1 渐进式采用路径（关键设计）

```
Step 1: modeld scan      ← 只读，零风险，1 分钟上手
         ↓ 用户看到"你有 320GB 重复"
Step 2: modeld dedup     ← 节省空间，可 dry-run
         ↓ 用户节省了实际空间
Step 3: pip install modeld-hook  ← 透明接入生态
         ↓ 新模型自动入库
Step 4: 完整生态集成     ← all-in
```

用户可以在任意阶段停下来，**不需要 all-in**。

### 13.2 发布节奏建议

| 节点 | 动作 | 宣传重点 |
|---|---|---|
| Phase 1 完成 | v0.1.0 发布 | "你有 XXX GB 重复文件" |
| Phase 2 完成 | v0.2.0 发布 | 实际节省空间数字 |
| Phase 3 完成 | v0.3.0 发布 | "pip install，零配置" |
| Phase 4 完成 | v0.4.0 发布 | "再也不会误删模型" |
| Phase 5 完成 | v1.0.0 发布 | 局域网共享，正式 1.0 |

### 13.3 社区建设

- README 第一屏：30 秒内看到演示结果（终端 GIF 动图）
- Discord / Matrix：用户反馈渠道，Phase 1 发布前建立
- r/StableDiffusion、r/comfyui：Phase 1 完成后发帖
- 贡献者指南：Phase 2 完成前发布，模块化架构降低贡献门槛

### 13.4 目标指标

| 阶段 | 目标 GitHub Stars | 目标活跃用户 |
|---|---|---|
| Phase 1 发布后 1 个月 | 500+ | 100+ |
| Phase 2 发布后 | 2000+ | 1000+ |
| Phase 3 发布后 | 5000+ | 5000+ |

---

*文档版本：v1.0*
*项目定位：AI 模型层的 Docker + Git + HuggingFace Cache*
