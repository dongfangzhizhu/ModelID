# modeld Web UI 实现规范

> **目标**：为 modeld daemon 添加一个内嵌的本地 Web UI，用户通过 `http://localhost:8234/ui` 访问，无需额外安装任何依赖。
>
> **原则**：UI 只是展示层，所有业务逻辑保留在 daemon；前端极简，优先可靠性而非炫技。

---

## 目录

1. [技术架构决策](#1-技术架构决策)
2. [后端：内嵌 HTTP 服务](#2-后端内嵌-http-服务)
3. [REST API 接口规范](#3-rest-api-接口规范)
4. [WebSocket 实时推送](#4-websocket-实时推送)
5. [前端结构规范](#5-前端结构规范)
6. [页面功能详细规范](#6-页面功能详细规范)
   - 6.1 [仪表板 Dashboard](#61-仪表板-dashboard)
   - 6.2 [重复文件 Duplicates](#62-重复文件-duplicates)
   - 6.3 [模型库 Library](#63-模型库-library)
   - 6.4 [下载器 Downloads](#64-下载器-downloads)
   - 6.5 [引用图 References](#65-引用图-references)
   - 6.6 [局域网代理 Proxy](#66-局域网代理-proxy)
   - 6.7 [设置 Settings](#67-设置-settings)
7. [Crate 结构与集成方式](#7-crate-结构与集成方式)
8. [错误处理规范](#8-错误处理规范)
9. [安全与访问控制](#9-安全与访问控制)
10. [实现优先级](#10-实现优先级)

---

## 1. 技术架构决策

### 1.1 整体架构

```
modeld-daemon (单进程)
├── 核心业务逻辑 (CAS / Dedup / Scanner / ...)
├── IPC 服务 (Unix socket / Named pipe)  ← CLI 使用
└── HTTP 服务 (axum)                     ← Web UI 使用
    ├── GET  /ui/*          静态文件（内嵌于二进制）
    ├── /api/v1/*           REST API
    └── /ws                 WebSocket 实时推送
```

**关键原则**：HTTP 服务与 daemon 在同一进程，共享同一个 AppState，不需要跨进程通信。

### 1.2 技术选型

| 组件 | 选型 | 理由 |
|---|---|---|
| HTTP 框架 | `axum` | tokio 生态，与 daemon async runtime 无缝共存 |
| 静态文件内嵌 | `rust-embed` | 编译时将前端文件打包进二进制，零部署依赖 |
| 前端框架 | 原生 HTML + JS（无框架） | bundle size 极小，无构建步骤，离线可用 |
| 实时通信 | `axum` WebSocket | 推送扫描进度、下载进度，避免轮询 |
| 序列化 | `serde_json` | 已有依赖，无需新增 |

> **不使用 React / Vue / Svelte**：UI 复杂度不需要组件框架，引入构建工具链会显著增加维护成本。用原生 JS 的 `fetch` + DOM 操作完全足够。

### 1.3 启动方式

```toml
# modeld.toml
[ui]
enabled = true
port = 8234
host = "127.0.0.1"   # 默认只监听本机，不暴露到局域网
open_browser = false  # 启动后是否自动打开浏览器
```

```bash
# CLI 控制
modeld daemon start          # 根据配置决定是否启动 UI
modeld daemon start --ui     # 强制启用 UI
modeld daemon start --no-ui  # 强制禁用 UI
```

---

## 2. 后端：内嵌 HTTP 服务

### 2.1 Cargo 依赖

在 `crates/modeld-daemon/Cargo.toml` 中添加：

```toml
[dependencies]
axum = { version = "0.7", features = ["ws"] }
axum-extra = { version = "0.9", features = ["typed-header"] }
rust-embed = { version = "8", features = ["compression"] }
tower = { version = "0.4", features = ["util"] }
tower-http = { version = "0.5", features = ["cors", "compression-gzip"] }
tokio-tungstenite = "0.21"
```

### 2.2 模块结构

在 `crates/modeld-daemon/src/` 下新建：

```
ui/
├── mod.rs          路由注册与服务启动
├── server.rs       axum 服务器初始化
├── static_files.rs rust-embed 静态文件服务
├── ws.rs           WebSocket 连接管理与事件广播
└── api/
    ├── mod.rs      API 路由汇总
    ├── stats.rs    GET /api/v1/stats
    ├── models.rs   GET /api/v1/models, GET /api/v1/models/:hash
    ├── dupes.rs    GET /api/v1/dupes, POST /api/v1/dupes/dedup
    ├── downloads.rs GET/POST/DELETE /api/v1/downloads
    ├── refs.rs     GET /api/v1/refs, GET /api/v1/refs/:hash
    ├── proxy.rs    GET /api/v1/proxy/status, GET /api/v1/proxy/peers
    ├── scan.rs     POST /api/v1/scan/trigger
    ├── gc.rs       POST /api/v1/gc/preview, POST /api/v1/gc/run
    └── settings.rs GET/PUT /api/v1/settings
```

### 2.3 AppState 共享

```rust
// ui/mod.rs
#[derive(Clone)]
pub struct UiState {
    pub db: Arc<Database>,           // 复用 daemon 已有的 DB 连接
    pub scanner: Arc<Scanner>,       // 复用 Scanner
    pub download_manager: Arc<DownloadManager>,
    pub event_tx: broadcast::Sender<UiEvent>,  // WebSocket 广播通道
}

pub async fn start_ui_server(state: UiState, config: UiConfig) -> Result<()> {
    let app = Router::new()
        .nest("/api/v1", api::routes(state.clone()))
        .route("/ws", get(ws::handler))
        .fallback(static_files::handler)   // 所有其他路径返回前端 index.html
        .with_state(state)
        .layer(
            ServiceBuilder::new()
                .layer(CorsLayer::permissive())  // 仅本地访问，CORS 不是安全边界
                .layer(CompressionLayer::new()),
        );

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

### 2.4 静态文件内嵌

```rust
// ui/static_files.rs
#[derive(RustEmbed)]
#[folder = "../../ui/dist/"]   // 前端文件目录，相对于 crate 根
#[prefix = ""]
struct Assets;

pub async fn handler(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref())],
                content.data,
            ).into_response()
        }
        // SPA 路由：找不到文件则返回 index.html
        None => {
            let index = Assets::get("index.html").unwrap();
            (
                [(header::CONTENT_TYPE, "text/html")],
                index.data,
            ).into_response()
        }
    }
}
```

---

## 3. REST API 接口规范

所有接口：
- 前缀：`/api/v1/`
- 请求/响应格式：`application/json`
- 错误格式：`{ "error": "描述", "code": "ERROR_CODE" }`

### 3.1 统计概览

```
GET /api/v1/stats
```

响应：

```json
{
  "total_models": 247,
  "total_size_bytes": 3298534883328,
  "duplicate_groups": 38,
  "duplicate_waste_bytes": 1288490188800,
  "deduped_saved_bytes": 365072220160,
  "frontends": [
    {
      "name": "comfyui",
      "path": "D:/ComfyUI/models",
      "model_count": 134,
      "size_bytes": 1932735283200,
      "duplicate_bytes": 730144440320
    }
  ],
  "last_scan_at": "2025-06-20T14:32:00Z",
  "daemon_version": "0.2.0",
  "daemon_uptime_secs": 3600
}
```

### 3.2 模型列表

```
GET /api/v1/models
```

查询参数：

| 参数 | 类型 | 说明 |
|---|---|---|
| `q` | string | 搜索（名称/hash/arch） |
| `type` | string | `checkpoint \| lora \| vae \| controlnet \| gguf` |
| `arch` | string | `sdxl \| sd15 \| flux \| llm` |
| `frontend` | string | `comfyui \| forge \| a1111` |
| `orphan` | bool | 仅显示无引用模型 |
| `page` | int | 默认 1 |
| `per_page` | int | 默认 50，最大 200 |
| `sort` | string | `size \| name \| last_seen \| ref_count` |
| `order` | string | `asc \| desc` |

响应：

```json
{
  "items": [
    {
      "blake3_hash": "abcdef1234...",
      "name": "sd_xl_base_1.0.safetensors",
      "format": "safetensors",
      "arch": "sdxl",
      "type": "checkpoint",
      "size_bytes": 6938045952,
      "ref_count": 5,
      "is_orphan": false,
      "frontends": ["comfyui", "forge"],
      "paths": [
        "D:/ComfyUI/models/checkpoints/sd_xl_base_1.0.safetensors"
      ],
      "created_at": "2024-11-01T10:00:00Z",
      "last_seen": "2025-06-20T12:00:00Z"
    }
  ],
  "total": 247,
  "page": 1,
  "per_page": 50
}
```

```
GET /api/v1/models/:hash
```

响应：同上单条，额外包含 `aliases`（所有路径别名）和 `metadata`（safetensors 解析出的原始元数据）。

### 3.3 重复文件

```
GET /api/v1/dupes
```

查询参数：`min_waste_bytes`（过滤小组）、`arch`、`sort=waste|count`

响应：

```json
{
  "items": [
    {
      "blake3_hash": "abcdef...",
      "name": "sd_xl_base_1.0.safetensors",
      "arch": "sdxl",
      "size_bytes": 6938045952,
      "copy_count": 3,
      "waste_bytes": 13876091904,
      "paths": [
        { "path": "D:/ComfyUI/models/checkpoints/...", "frontend": "comfyui" },
        { "path": "D:/Forge/models/...",               "frontend": "forge" },
        { "path": "E:/A1111/models/...",               "frontend": "a1111" }
      ]
    }
  ],
  "total_waste_bytes": 1288490188800
}
```

```
POST /api/v1/dupes/dedup
```

请求体：

```json
{
  "dry_run": true,
  "hashes": ["abcdef...", "123456..."],   // 留空表示全量
  "strategy": "hardlink"                  // "hardlink" | "symlink"
}
```

响应：

```json
{
  "dry_run": true,
  "operations": [
    {
      "hash": "abcdef...",
      "keep_path": "D:/ComfyUI/models/checkpoints/sd_xl_base_1.0.safetensors",
      "replace_with_links": [
        "D:/Forge/models/Stable-diffusion/sd_xl_base_1.0.safetensors"
      ],
      "would_save_bytes": 6938045952
    }
  ],
  "total_would_save_bytes": 13876091904
}
```

> `dry_run: true` 时只返回预览，`dry_run: false` 时执行操作。执行时通过 WebSocket 推送进度。

### 3.4 下载器

```
GET  /api/v1/downloads           # 下载任务列表（含历史）
POST /api/v1/downloads           # 添加新下载任务
DELETE /api/v1/downloads/:id     # 取消/删除任务
POST /api/v1/downloads/:id/pause
POST /api/v1/downloads/:id/resume
```

POST 请求体：

```json
{
  "url": "https://huggingface.co/black-forest-labs/FLUX.1-dev/resolve/main/flux1-dev.safetensors",
  "target_frontend": "comfyui",
  "target_type": "checkpoint"
}
```

也支持 HF 模型 ID 格式：

```json
{
  "hf_repo": "black-forest-labs/FLUX.1-dev",
  "hf_filename": "flux1-dev.safetensors",
  "hf_revision": "main"
}
```

GET 响应中每条任务：

```json
{
  "id": 42,
  "name": "flux1-dev.safetensors",
  "source_url": "https://huggingface.co/...",
  "status": "downloading",
  "bytes_total": 23804854272,
  "bytes_done": 15949021389,
  "speed_bps": 47185920,
  "eta_secs": 168,
  "started_at": "2025-06-20T14:00:00Z",
  "finished_at": null,
  "blake3_hash": null,
  "error": null
}
```

`status` 枚举：`pending | downloading | paused | done | failed | cancelled`

### 3.5 引用图

```
GET /api/v1/refs
```

返回所有有引用关系的模型及其被哪些 workflow 引用。

```
GET /api/v1/refs/:hash
```

返回单个模型的引用详情：

```json
{
  "blake3_hash": "abcdef...",
  "name": "sd_xl_base_1.0.safetensors",
  "ref_count": 5,
  "refs": [
    {
      "workflow_path": "/home/user/.comfyui/workflows/portrait.json",
      "ref_type": "checkpoint",
      "last_checked": "2025-06-20T10:00:00Z"
    }
  ],
  "is_orphan": false,
  "gc_safe": false
}
```

```
GET /api/v1/refs/orphans
```

返回所有孤儿模型列表（`ref_count = 0`）。

### 3.6 扫描控制

```
POST /api/v1/scan/trigger
```

请求体：

```json
{
  "path": "D:/ComfyUI/models",   // 留空则扫描所有注册目录
  "force_rehash": false
}
```

响应：`{ "scan_id": "uuid-xxx" }` — 通过 WebSocket 跟踪进度。

### 3.7 垃圾回收

```
POST /api/v1/gc/preview
```

返回可回收对象列表（不执行删除）：

```json
{
  "reclaimable": [
    {
      "blake3_hash": "abc...",
      "name": "old_model.safetensors",
      "size_bytes": 2147483648,
      "quarantine_since": "2025-05-01T00:00:00Z",
      "reason": "ref_count_zero"
    }
  ],
  "total_reclaimable_bytes": 10737418240
}
```

```
POST /api/v1/gc/run
```

请求体：`{ "hashes": [...] }` — 留空则回收所有可回收对象。

### 3.8 代理状态

```
GET /api/v1/proxy/status
```

```json
{
  "enabled": true,
  "port": 8234,
  "endpoint": "http://192.168.1.100:8234",
  "stats": {
    "requests_today": 1247,
    "cache_hit_rate": 0.84,
    "bytes_served": 136365211648
  }
}
```

```
GET /api/v1/proxy/peers
```

返回 mDNS 发现的局域网内其他 modeld 实例列表。

### 3.9 设置

```
GET /api/v1/settings
PUT /api/v1/settings
```

GET 响应/PUT 请求体结构与 `modeld.toml` 字段对应：

```json
{
  "store": {
    "root": "D:/modeld-store",
    "auto_scan_on_start": true,
    "watch_enabled": true,
    "incremental_scan": true
  },
  "gc": {
    "quarantine_days": 30,
    "confirm_before_gc": true
  },
  "proxy": {
    "port": 8234,
    "host": "0.0.0.0",
    "allow_networks": ["192.168.1.0/24"],
    "require_auth": false,
    "token": ""
  },
  "ui": {
    "port": 8234,
    "host": "127.0.0.1",
    "open_browser": false
  }
}
```

PUT 后 daemon 热重载配置（不需要重启）。

---

## 4. WebSocket 实时推送

### 4.1 连接

```
ws://localhost:8234/ws
```

前端在页面加载时建立连接，断线后 5 秒自动重连（指数退避，最大 30 秒）。

### 4.2 事件类型

所有事件格式：

```json
{ "type": "EVENT_TYPE", "payload": { ... }, "ts": "2025-06-20T14:00:00Z" }
```

#### 扫描进度

```json
{
  "type": "scan_progress",
  "payload": {
    "scan_id": "uuid-xxx",
    "current_path": "D:/ComfyUI/models/loras/character.safetensors",
    "files_scanned": 1024,
    "files_total": 1580,
    "bytes_hashed": 549755813888,
    "new_models_found": 12,
    "duplicates_found": 3,
    "phase": "hashing"   // "walking" | "hashing" | "indexing" | "done"
  }
}
```

#### 下载进度

```json
{
  "type": "download_progress",
  "payload": {
    "id": 42,
    "bytes_done": 15949021389,
    "bytes_total": 23804854272,
    "speed_bps": 47185920,
    "eta_secs": 168,
    "status": "downloading"
  }
}
```

#### Dedup 进度

```json
{
  "type": "dedup_progress",
  "payload": {
    "completed": 12,
    "total": 38,
    "current_file": "sd_xl_base_1.0.safetensors",
    "bytes_saved_so_far": 83316244480
  }
}
```

#### 文件变化通知

```json
{
  "type": "file_changed",
  "payload": {
    "event": "created",   // "created" | "deleted" | "modified"
    "path": "D:/ComfyUI/models/loras/new_lora.safetensors",
    "frontend": "comfyui"
  }
}
```

#### 守护进程状态

```json
{
  "type": "daemon_status",
  "payload": {
    "scanning": false,
    "deduping": false,
    "proxy_running": true
  }
}
```

### 4.3 Rust 端广播实现

```rust
// ui/ws.rs
use tokio::sync::broadcast;

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    ScanProgress(ScanProgressPayload),
    DownloadProgress(DownloadProgressPayload),
    DedupProgress(DedupProgressPayload),
    FileChanged(FileChangedPayload),
    DaemonStatus(DaemonStatusPayload),
}

// 在各业务模块中，持有 event_tx: broadcast::Sender<UiEvent>
// 有状态变化时直接发送，不关心是否有订阅者
scanner.on_progress(|p| {
    let _ = event_tx.send(UiEvent::ScanProgress(p));
});
```

---

## 5. 前端结构规范

### 5.1 文件结构

```
ui/
├── index.html       入口，内联关键 CSS，引用 main.js
├── main.js          路由、状态管理、WebSocket 客户端
├── style.css        全局样式
├── api.js           所有 fetch 调用的封装（单一文件）
└── pages/
    ├── dashboard.js
    ├── dupes.js
    ├── library.js
    ├── downloads.js
    ├── refs.js
    ├── proxy.js
    └── settings.js
```

> 不需要 bundler，直接用 ES modules（`type="module"`），现代浏览器原生支持。

### 5.2 路由约定

使用 hash 路由（`#/dashboard`、`#/dupes` 等），避免需要服务端路由配置。

```javascript
// main.js 路由核心
const routes = {
  '/dashboard': () => import('./pages/dashboard.js'),
  '/dupes':     () => import('./pages/dupes.js'),
  '/library':   () => import('./pages/library.js'),
  '/downloads': () => import('./pages/downloads.js'),
  '/refs':      () => import('./pages/refs.js'),
  '/proxy':     () => import('./pages/proxy.js'),
  '/settings':  () => import('./pages/settings.js'),
};

window.addEventListener('hashchange', navigate);

async function navigate() {
  const path = location.hash.replace('#', '') || '/dashboard';
  const loader = routes[path];
  if (!loader) { location.hash = '#/dashboard'; return; }
  const mod = await loader();
  document.getElementById('page-content').innerHTML = '';
  mod.render(document.getElementById('page-content'));
}
```

### 5.3 API 客户端封装

```javascript
// api.js — 所有接口调用都通过这个模块，方便后续统一处理认证/错误

const BASE = '/api/v1';

async function request(method, path, body) {
  const res = await fetch(BASE + path, {
    method,
    headers: body ? { 'Content-Type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new ApiError(err.error, err.code, res.status);
  }
  return res.status === 204 ? null : res.json();
}

export const api = {
  stats:     ()           => request('GET',  '/stats'),
  models:    (params)     => request('GET',  '/models?' + new URLSearchParams(params)),
  model:     (hash)       => request('GET',  `/models/${hash}`),
  dupes:     (params)     => request('GET',  '/dupes?' + new URLSearchParams(params)),
  dedup:     (body)       => request('POST', '/dupes/dedup', body),
  downloads: ()           => request('GET',  '/downloads'),
  addDownload:(body)      => request('POST', '/downloads', body),
  cancelDownload: (id)    => request('DELETE',`/downloads/${id}`),
  refs:      ()           => request('GET',  '/refs'),
  refDetail: (hash)       => request('GET',  `/refs/${hash}`),
  orphans:   ()           => request('GET',  '/refs/orphans'),
  scan:      (body)       => request('POST', '/scan/trigger', body),
  gcPreview: ()           => request('POST', '/gc/preview'),
  gcRun:     (body)       => request('POST', '/gc/run', body),
  proxyStatus:()          => request('GET',  '/proxy/status'),
  proxyPeers: ()          => request('GET',  '/proxy/peers'),
  settings:  ()           => request('GET',  '/settings'),
  saveSettings:(body)     => request('PUT',  '/settings', body),
};
```

### 5.4 WebSocket 客户端

```javascript
// main.js — WebSocket 管理，事件分发到各页面

class EventBus {
  constructor() { this._listeners = {}; }
  on(type, fn) {
    (this._listeners[type] ??= []).push(fn);
    return () => this.off(type, fn);
  }
  off(type, fn) {
    this._listeners[type] = (this._listeners[type] ?? []).filter(f => f !== fn);
  }
  emit(type, payload) {
    (this._listeners[type] ?? []).forEach(fn => fn(payload));
  }
}

export const events = new EventBus();

function connectWs() {
  const ws = new WebSocket(`ws://${location.host}/ws`);
  ws.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    events.emit(msg.type, msg.payload);
  };
  ws.onclose = () => setTimeout(connectWs, 5000);
  return ws;
}

connectWs();
```

页面订阅事件示例：

```javascript
// pages/downloads.js
import { events } from '../main.js';

export function render(container) {
  // 渲染初始内容...

  // 订阅下载进度
  const unsub = events.on('download_progress', (payload) => {
    updateProgressBar(payload.id, payload.bytes_done, payload.bytes_total, payload.speed_bps);
  });

  // 页面卸载时取消订阅（路由切换时调用）
  container._cleanup = unsub;
}
```

### 5.5 UI 组件约定

用纯函数生成 HTML 字符串，复杂动态部分用 DOM API：

```javascript
// 格式化工具函数（所有页面共用）
function fmtBytes(bytes) {
  const units = ['B','KB','MB','GB','TB'];
  let i = 0, v = bytes;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

function fmtRelTime(isoStr) {
  const diff = Date.now() - new Date(isoStr);
  if (diff < 60000)  return '刚刚';
  if (diff < 3600000) return `${Math.floor(diff/60000)} 分钟前`;
  if (diff < 86400000) return `${Math.floor(diff/3600000)} 小时前`;
  return `${Math.floor(diff/86400000)} 天前`;
}

function fmtSpeed(bps) {
  return fmtBytes(bps) + '/s';
}
```

---

## 6. 页面功能详细规范

### 6.1 仪表板 Dashboard

**路由**：`#/dashboard`

**布局**：顶部 4 个指标卡 → 存储分布表 → 底部扫描状态栏 + 快捷操作按钮

**指标卡数据**（来自 `GET /api/v1/stats`）：

| 卡片 | 数据字段 | 颜色 |
|---|---|---|
| 已索引模型 | `total_models` | 默认 |
| 可节省空间 | `duplicate_waste_bytes` | 危险色（红/橙）|
| 已节省空间 | `deduped_saved_bytes` | 成功色（绿）|
| 上次扫描 | `last_scan_at`（相对时间）| 默认 |

**存储分布表**列：前端名称、路径、模型数、占用空间、重复浪费

**扫描状态栏**：

- 通过 WebSocket `scan_progress` 事件实时更新
- 无扫描时显示"监听中"+ 最后扫描时间
- 扫描进行中显示进度条 + 当前文件路径 + 预计剩余时间

**快捷按钮**：

- `立即扫描` → POST `/api/v1/scan/trigger`，扫描进度实时展示
- `执行 dedup` → 跳转到 `#/dupes` 页面
- `垃圾回收` → 先调用 `/api/v1/gc/preview`，弹出确认对话框显示可回收列表，确认后调用 `/api/v1/gc/run`

**自动刷新**：页面可见时每 30 秒刷新一次统计数字（不需要实时）。

---

### 6.2 重复文件 Duplicates

**路由**：`#/dupes`

**布局**：顶部工具栏（搜索 + 筛选 + 一键 dedup 按钮）→ 重复组列表（可展开）

**重复组卡片**（展开后显示）：

- 头部：`×N 副本` 标签 + 文件名 + 架构标签 + 浪费空间
- 展开体：
  - 路径列表，第一条标为"主文件（保留）"，其余标为"将被替换为 hardlink"
  - 允许用户手动切换哪条路径作为主文件
- 操作行：`保留第一个，其余 hardlink` / `dry-run 预览` / `忽略此组`

**一键 dedup 流程**：

1. 调用 `POST /api/v1/dupes/dedup { dry_run: true }` 获取预览
2. 弹出确认对话框，显示"将节省 X GB，替换 N 个文件"
3. 用户确认后调用 `dry_run: false`
4. WebSocket 实时更新每组的处理进度
5. 完成后刷新列表

**筛选项**：架构（全部/SDXL/SD1.5/Flux/GGUF）、最小浪费空间（100MB/1GB/10GB）、排序（浪费空间/副本数/文件大小）

**空状态**：显示"未发现重复文件，当前存储使用率最优"。

---

### 6.3 模型库 Library

**路由**：`#/library`

**布局**：顶部工具栏（搜索 + 类型筛选 + 架构筛选）→ 模型列表（虚拟滚动，模型数量可能很大）

**列表行**：

- 左侧：类型图标（checkpoint/lora/vae/controlnet 各用不同图标）
- 主信息：文件名 + 元数据行（类型 · 引用数 · 前端列表）
- 右侧：架构标签 + 文件大小 + 右箭头

**孤儿标记**：`ref_count = 0` 的模型在元数据行显示橙色"孤儿"标签

**点击行展开详情面板**（或跳转到详情页）：

- 完整 blake3 hash（可复制）
- 所有路径别名（frontend 类型 + 路径）
- safetensors 元数据（arch、tensor 数量等，来自解析结果）
- 引用的 workflow 列表
- 操作：`复制 hash` / `在文件管理器中打开` / `强制 GC`

**虚拟滚动**：列表超过 100 条时使用虚拟滚动，只渲染可视区域内的行（避免大量 DOM 节点）：

```javascript
// 简单虚拟滚动实现
const ROW_HEIGHT = 56;  // px

function renderVirtualList(container, items, renderItem) {
  const total = items.length * ROW_HEIGHT;
  const spacer = document.createElement('div');
  spacer.style.height = total + 'px';
  container.appendChild(spacer);

  function update() {
    const scrollTop = container.scrollTop;
    const viewHeight = container.clientHeight;
    const startIdx = Math.floor(scrollTop / ROW_HEIGHT);
    const endIdx = Math.min(items.length, startIdx + Math.ceil(viewHeight / ROW_HEIGHT) + 3);
    // 只渲染 [startIdx, endIdx) 范围内的行，用 absolute 定位
  }

  container.addEventListener('scroll', update);
  update();
}
```

---

### 6.4 下载器 Downloads

**路由**：`#/downloads`

**布局**：顶部输入栏（URL 输入 + 添加按钮）→ 任务列表

**添加下载**：

- 支持粘贴完整 HuggingFace URL（`https://huggingface.co/...`）
- 支持 `repo_id/filename` 简写格式（如 `black-forest-labs/FLUX.1-dev/flux1-dev.safetensors`）
- 添加后立即出现在列表中，状态为 `pending`

**任务卡片**显示：

- 文件名 + 来源（HF repo 路径，单色等宽字体）
- 下载中：进度条 + 速度 + ETA
- 完成：绿色"已存入 CAS，hash 验证通过" + blake3 hash 前 8 位
- 失败：红色错误信息 + 重试按钮
- 操作按钮：暂停/恢复（下载中/暂停中）、取消（非完成态）

**实时更新**：通过 WebSocket `download_progress` 事件更新进度条，避免轮询。

**历史记录**：完成/失败的任务保留显示，可按状态筛选（进行中/全部历史）。

---

### 6.5 引用图 References

**路由**：`#/refs`

**布局**：顶部工具栏（搜索 + "仅显示孤儿"开关）→ 模型卡片列表

**模型卡片**：

- 头部：文件名 + 架构标签 + 引用数（或"孤儿"警告）
- 展开：workflow 文件列表，每条显示路径 + 引用类型（checkpoint/lora/vae）+ 最后检查时间

**孤儿模型的操作**：

- 显示"未被任何 workflow 引用，可安全回收"提示
- 提供"加入 GC 队列"按钮，点击后模型进入隔离区

**批量操作**：勾选多个孤儿后，可批量加入 GC 队列。

**Workflow 扫描触发**：页面顶部显示"上次 workflow 扫描时间"，提供"重新扫描 workflows"按钮（调用 `POST /api/v1/scan/trigger`，类型指定为 workflow）。

---

### 6.6 局域网代理 Proxy

**路由**：`#/proxy`

**布局**：本机代理状态卡 + 统计数字 → 局域网设备列表

**本机代理卡**：

- 代理地址（可一键复制）
- `export HF_ENDPOINT=...` 命令（代码样式，可一键复制）
- 启动/停止按钮
- 状态指示（运行中/已停止）

**统计数字**：今日请求数、缓存命中率、节省流量（仅显示，不需要图表，Phase 1 阶段）

**局域网设备列表**（来自 mDNS 发现）：

- 每个设备：主机名 + IP + 状态（在线/离线）+ 今日请求数
- 在线设备可以点击"设为默认代理"（修改本机 `HF_ENDPOINT` 配置）

---

### 6.7 设置 Settings

**路由**：`#/settings`

**布局**：分组设置项列表 → 底部保存/重置按钮

**分组**：

1. **扫描与存储**
   - CAS 根目录（路径输入框 + 选择按钮）
   - 启动时自动扫描（开关）
   - 文件变化监听（开关）
   - 增量扫描策略（开关）

2. **垃圾回收**
   - 隔离区保留天数（数字输入）
   - GC 前需要确认（开关）

3. **局域网代理**
   - 代理端口（数字输入）
   - 允许的网段（文本输入，逗号分隔）
   - 需要 token 认证（开关）
   - token 值（文本输入，仅在认证开启时显示）

4. **Web UI**
   - 监听端口（数字输入）
   - 仅本机访问（开关，控制 host 是 127.0.0.1 还是 0.0.0.0）
   - 启动时打开浏览器（开关）

**危险区**（分组底部分隔）：

- `卸载 modeld`：展示说明文字"将还原所有 hardlink 为原始文件副本，然后删除 CAS 存储"，点击后弹出强确认对话框（需要输入"UNINSTALL"文字确认）

**保存行为**：调用 `PUT /api/v1/settings`，成功后显示"配置已保存，即时生效"提示（daemon 热重载，不需要重启）。

---

## 7. Crate 结构与集成方式

### 7.1 在 `modeld-daemon` 中集成

```rust
// crates/modeld-daemon/src/main.rs

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load()?;
    let db = Database::open(&config.store.root)?;
    let scanner = Scanner::new(db.clone());
    let download_manager = DownloadManager::new(db.clone());

    let (event_tx, _) = broadcast::channel(256);

    // 启动各业务服务
    let scanner_handle = tokio::spawn(scanner.run(event_tx.clone()));
    let proxy_handle   = tokio::spawn(proxy::run(config.proxy.clone(), db.clone()));

    // 启动 Web UI（如果配置了）
    if config.ui.enabled {
        let ui_state = ui::UiState {
            db: db.clone(),
            scanner: scanner.clone(),
            download_manager: download_manager.clone(),
            event_tx: event_tx.clone(),
        };
        tokio::spawn(ui::start_ui_server(ui_state, config.ui.clone()));
        tracing::info!("Web UI: http://{}:{}/ui", config.ui.host, config.ui.port);
    }

    // 启动 IPC 服务（CLI 用）
    let ipc_handle = tokio::spawn(ipc::serve(db.clone(), config.ipc.clone()));

    tokio::signal::ctrl_c().await?;
    Ok(())
}
```

### 7.2 前端文件的构建与嵌入

前端文件放在 `ui/` 目录（与 `crates/` 同级），无需 npm/node，直接写静态文件。

```
modeld/
├── Cargo.toml
├── crates/
│   └── modeld-daemon/
│       └── build.rs    # 如需要构建步骤（当前不需要）
├── ui/
│   ├── index.html
│   ├── main.js
│   ├── style.css
│   ├── api.js
│   └── pages/
└── ...
```

`rust-embed` 配置：

```rust
// crates/modeld-daemon/src/ui/static_files.rs
#[derive(RustEmbed)]
#[folder = "../../../ui/"]   // 相对于 crate 目录
#[include = "*.html"]
#[include = "*.js"]
#[include = "*.css"]
struct Assets;
```

### 7.3 开发模式

开发时不想每次改前端都重新编译 Rust，可以添加开发模式：

```rust
// 开发模式：从文件系统读取，不从嵌入资源读取
#[cfg(debug_assertions)]
pub async fn handler(uri: Uri) -> impl IntoResponse {
    let path = format!("./ui/{}", uri.path().trim_start_matches('/'));
    match tokio::fs::read(&path).await {
        Ok(bytes) => { /* 返回文件内容 */ }
        Err(_) => { /* 返回 index.html */ }
    }
}

#[cfg(not(debug_assertions))]
pub async fn handler(uri: Uri) -> impl IntoResponse {
    // 从 rust-embed 读取
}
```

---

## 8. 错误处理规范

### 8.1 API 错误格式

```json
{
  "error": "用户可读的错误描述",
  "code": "MACHINE_READABLE_CODE",
  "details": {}   // 可选，调试信息
}
```

常见错误码：

| Code | HTTP 状态 | 说明 |
|---|---|---|
| `NOT_FOUND` | 404 | 模型 hash 不存在 |
| `DEDUP_IN_PROGRESS` | 409 | 已有 dedup 正在运行 |
| `SCAN_IN_PROGRESS` | 409 | 扫描进行中，不能同时触发 |
| `INVALID_URL` | 400 | 下载 URL 格式不合法 |
| `CROSS_VOLUME_HARDLINK` | 422 | 跨卷 hardlink 不支持，需降级策略 |
| `PERMISSION_DENIED` | 403 | 文件权限不足 |
| `INTERNAL_ERROR` | 500 | 内部错误 |

### 8.2 前端错误处理

```javascript
// api.js
class ApiError extends Error {
  constructor(message, code, status) {
    super(message);
    this.code = code;
    this.status = status;
  }
}

// 各页面统一的错误展示
function showError(container, err) {
  const div = document.createElement('div');
  div.className = 'error-banner';
  div.textContent = err instanceof ApiError ? err.message : '操作失败，请重试';
  container.prepend(div);
  setTimeout(() => div.remove(), 5000);
}
```

### 8.3 操作确认对话框

对于破坏性操作（dedup、GC、卸载），必须显示确认对话框，包含：

- 操作说明（"将替换 N 个文件为 hardlink，节省 X GB"）
- 操作是否可逆的说明
- 确认和取消按钮

对话框用原生 `<dialog>` 元素实现，避免引入任何 modal 库。

---

## 9. 安全与访问控制

### 9.1 默认只监听本机

```toml
[ui]
host = "127.0.0.1"   # 默认，只允许本机访问
```

如果用户改为 `0.0.0.0`，daemon 启动时打印警告：

```
⚠️  Web UI 监听在所有网络接口，确保防火墙已正确配置
```

### 9.2 可选 token 认证

```toml
[ui]
require_auth = false
token = ""
```

启用后，所有 `/api/v1/*` 请求必须携带 `Authorization: Bearer <token>` header。WebSocket 连接在握手时通过 query 参数传递 token：`ws://localhost:8234/ws?token=xxx`

### 9.3 CSRF 防护

由于 UI 只在 `localhost` 访问，且 API 使用 `Content-Type: application/json`，浏览器的同源策略已经提供了足够的保护。不需要额外的 CSRF token。

---

## 10. 实现优先级

按 Phase 顺序，Web UI 功能随主体功能一起交付：

### Phase 1 配套（与 `modeld scan / dupes` 同步）

- [ ] 基础 HTTP 服务（axum 框架搭建）
- [ ] 静态文件内嵌（rust-embed）
- [ ] 前端路由框架（hash router）
- [ ] WebSocket 连接与重连
- [ ] `GET /api/v1/stats` 接口
- [ ] **仪表板页面**（统计数字 + 存储分布表）
- [ ] `GET /api/v1/dupes` 接口
- [ ] **重复文件页面**（仅展示，不含操作）

### Phase 2 配套（与 CAS + dedup 同步）

- [ ] `POST /api/v1/dupes/dedup` 接口（含 dry-run）
- [ ] `GET/PUT /api/v1/settings` 接口
- [ ] **重复文件页面**（加入操作功能：dedup、dry-run）
- [ ] **设置页面**
- [ ] Dedup 进度 WebSocket 事件
- [ ] `POST /api/v1/gc/preview` 和 `/gc/run` 接口
- [ ] 仪表板"垃圾回收"快捷操作

### Phase 3 配套（与 HF 拦截层同步）

- [ ] `GET/POST/DELETE /api/v1/downloads` 接口
- [ ] 下载进度 WebSocket 事件
- [ ] **下载器页面**（完整功能）
- [ ] `GET /api/v1/models` 接口
- [ ] **模型库页面**（基础列表）

### Phase 4 配套（与 Workflow ref graph 同步）

- [ ] `GET /api/v1/refs` 和 `/refs/:hash` 接口
- [ ] **引用图页面**
- [ ] 模型库页面加入孤儿标记和引用详情
- [ ] 仪表板加入孤儿模型快捷入口

### Phase 5 配套（与 Local Proxy 同步）

- [ ] `GET /api/v1/proxy/status` 和 `/proxy/peers` 接口
- [ ] **局域网代理页面**
- [ ] mDNS 设备发现展示

---

*文档版本：v1.0*
*对应项目计划：modeld-project-plan.md v1.0*