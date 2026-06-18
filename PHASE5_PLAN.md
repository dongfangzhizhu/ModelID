# Phase 5: Local Registry & Proxy

**状态**: ✅ **完成**  
**开始日期**: 2026-06-13  
**完成日期**: 2026-06-19  
**目标**: 实现局域网模型共享和 HuggingFace 代理服务器

> **实现说明**：Phase 5 按本计划完成，但有两处与原计划文档的偏差，
> 已根据决策固化：
> 1. **默认端口为 8234**（按 `modeld-project-plan.md §9`，非本文件原写的 8080）。
> 2. **配置并入统一 `modeld.toml [proxy]` 段**（非独立的 `proxy.toml`）。
> 3. **mDNS 发布依赖系统守护进程**：所用 `mdns` crate 仅支持发现（scan），
>    不支持发布。`modeld proxy discover` 能扫到任何注册为
>    `_modeld._tcp.local.` 的服务；要让本机被发现，需用 avahi-publish /
>    dns-sd / Bonjour 发布。详见 `docs/proxy-setup.md`。


---

## 🎯 目标

让多台机器共享同一个 modeld CAS 存储，避免每台机器重复下载相同的模型。实现本地 HuggingFace 代理服务器，支持局域网内的模型分发。

---

## 📋 任务分解

### 1. HTTP 代理服务器基础 ⏳
**预估**: 1周

**任务**:
- [ ] 实现基础 HTTP server（tiny_http）
- [ ] 路由系统
  - `/health` - 健康检查
  - `/v1/models` - 模型列表 API
  - `/v1/blobs/{hash}` - CAS 对象下载
  - `/v1/hf-proxy/{org}/{repo}/{file}` - HF 代理
- [ ] Range requests 支持（HTTP 206 Partial Content）
- [ ] CORS 支持
- [ ] 优雅关闭（Ctrl+C 处理）

**产出**: `crates/modeld-proxy/src/server.rs`

---

### 2. HuggingFace 代理层 ⏳
**预估**: 1.5周

**任务**:
- [ ] HF API 转发
  - HEAD 请求（获取文件元数据）
  - GET 请求（下载文件）
- [ ] 缓存策略
  - 检查本地 CAS 是否已有
  - Cache hit → 直接返回本地文件
  - Cache miss → 从 HF 下载 + 存入 CAS + 转发
- [ ] SHA256 ↔ BLAKE3 映射
- [ ] 并发下载控制
- [ ] 进度回调

**产出**: `crates/modeld-proxy/src/hf_proxy.rs`

---

### 3. Range Requests 支持 ⏳
**预估**: 3天

**任务**:
- [ ] 实现 HTTP Range 头解析
- [ ] 支持单 Range（`bytes=0-1023`）
- [ ] 支持多 Range（`bytes=0-1023, 2048-3071`）
- [ ] 返回 206 Partial Content 响应
- [ ] Content-Range 头生成
- [ ] multipart/byteranges 响应（多 Range）
- [ ] 单元测试

**产出**: `crates/modeld-proxy/src/range.rs`

---

### 4. 访问控制 🔲
**预估**: 1周

**任务**:
- [ ] 配置文件（`proxy.toml`）
  - `bind_address` / `port`
  - `allow_anonymous`
  - `require_token`
  - `allowed_ips` / `denied_ips`
- [ ] Bearer Token 认证
  - `Authorization: Bearer <token>` 头验证
  - Token 存储（配置文件 / 环境变量）
- [ ] IP 白名单/黑名单
- [ ] 速率限制（基础）
- [ ] 审计日志

**产出**: `crates/modeld-proxy/src/auth.rs`

---

### 5. 局域网自动发现 🔲
**预估**: 1.5周

**任务**:
- [ ] mDNS/Bonjour 服务注册
  - 服务类型: `_modeld._tcp.local.`
  - TXT 记录: 版本、CAS 统计
- [ ] 服务发现客户端
  - 扫描局域网内的 modeld 服务器
  - `modeld proxy discover` 命令
- [ ] 跨平台支持
  - Windows: DNS-SD API
  - Linux/macOS: Avahi/Bonjour
- [ ] 单元测试（mock DNS-SD）

**产出**: `crates/modeld-proxy/src/discovery.rs`

**依赖**: `mdns` crate 或 `zeroconf`

---

### 6. CLI 代理命令 ⏳
**预估**: 3天

**任务**:
- [ ] `modeld proxy start` - 启动代理服务器
  - `--bind <addr>` - 绑定地址（默认 `0.0.0.0:8080`）
  - `--token <token>` - 访问 token
  - `--allow-anonymous` - 允许匿名访问
  - `--config <path>` - 配置文件路径
- [ ] `modeld proxy discover` - 发现局域网内的 modeld 服务器
- [ ] `modeld proxy status` - 显示代理服务器状态
- [ ] 守护进程模式（后台运行）

**产出**: `crates/modeld-cli/src/main.rs` (proxy 子命令)

---

### 7. 客户端 SDK 🔲
**预估**: 1周

**任务**:
- [ ] Rust 客户端库
  - `ModeldClient::connect(url)`
  - `.list_models()` - 列出远程模型
  - `.download_blob(hash)` - 下载 CAS 对象
  - `.download_hf_file(repo, file)` - 通过代理下载 HF 文件
- [ ] 连接池
- [ ] 自动重试
- [ ] 进度回调
- [ ] 单元测试 + 集成测试

**产出**: `crates/modeld-client/src/lib.rs`

---

### 8. 集成测试 🔲
**预估**: 1周

**任务**:
- [ ] 端到端代理测试
  - 启动 proxy server
  - 客户端下载文件
  - 验证 Range requests
  - 验证认证
- [ ] HF 代理测试
  - Mock HuggingFace API
  - 测试缓存命中/未命中
- [ ] 并发测试
  - 多客户端同时下载
  - 验证无数据竞争
- [ ] 性能基准测试
  - 吞吐量测试（目标 >100 MB/s）
  - 延迟测试

**产出**: `crates/modeld-proxy/tests/integration_test.rs`

---

## 📐 架构设计

### 系统组件

```
┌─────────────────────────────────────────────────────────┐
│                  Client Machines                         │
│  ComfyUI / diffusers / transformers                      │
│          ↓ HF_HOME or Python hook                        │
└──────────────────┬──────────────────────────────────────┘
                   │ HTTP
                   ↓
┌─────────────────────────────────────────────────────────┐
│              modeld Proxy Server                         │
│  ┌───────────────────────────────────────────────────┐  │
│  │  HTTP Router                                       │  │
│  │  ├─ /health                                        │  │
│  │  ├─ /v1/models                                     │  │
│  │  ├─ /v1/blobs/{hash}                               │  │
│  │  └─ /v1/hf-proxy/{org}/{repo}/{file}              │  │
│  └───────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────┐  │
│  │  Auth Middleware                                   │  │
│  │  ├─ Bearer Token                                   │  │
│  │  ├─ IP Whitelist                                   │  │
│  │  └─ Rate Limiting                                  │  │
│  └───────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────┐  │
│  │  HF Proxy Layer                                    │  │
│  │  ├─ Cache Check (local CAS)                       │  │
│  │  ├─ Download (HF API)                             │  │
│  │  └─ Store to CAS                                   │  │
│  └───────────────────────────────────────────────────┘  │
└──────────────────┬──────────────────────────────────────┘
                   │
                   ↓
┌─────────────────────────────────────────────────────────┐
│                modeld CAS Store                          │
│  cas/blake3/{prefix}/{hash}                             │
│  modeld.db (SQLite)                                      │
└─────────────────────────────────────────────────────────┘
```

### API 规范

#### `GET /health`
健康检查

**响应**:
```json
{
  "status": "ok",
  "version": "0.5.0",
  "uptime_seconds": 3600
}
```

#### `GET /v1/models`
列出所有模型

**响应**:
```json
{
  "models": [
    {
      "hash": "abcd1234...",
      "size_bytes": 4470000000,
      "format": "safetensors",
      "category": "checkpoint"
    }
  ],
  "total": 1234,
  "total_size_bytes": 5000000000000
}
```

#### `GET /v1/blobs/{hash}`
下载 CAS 对象

**请求头**:
- `Range: bytes=0-1023` (可选)
- `Authorization: Bearer <token>` (如果需要认证)

**响应**:
- `200 OK` (完整文件) 或 `206 Partial Content` (Range)
- `Content-Type: application/octet-stream`
- `Content-Length: <size>`
- `Content-Range: bytes 0-1023/4470000000` (如果是 Range)

#### `GET /v1/hf-proxy/{org}/{repo}/resolve/{revision}/{file}`
HuggingFace 代理下载

**示例**:
```
GET /v1/hf-proxy/stabilityai/stable-diffusion-xl-base-1.0/resolve/main/sd_xl_base_1.0.safetensors
```

**流程**:
1. 检查本地 CAS 是否有该文件（通过 SHA256 映射）
2. Cache hit → 返回本地文件（`X-Modeld-Cache: hit`）
3. Cache miss → 从 HF 下载 → 存入 CAS → 转发给客户端（`X-Modeld-Cache: miss`）

**响应头**:
- `X-Modeld-Cache: hit | miss`
- `X-Modeld-Blake3: <hash>`
- `Content-Type: application/octet-stream`

---

## 🛠️ 技术栈

### HTTP Server
```toml
tiny_http = "0.12"  # 轻量级 HTTP server
```

### 配置文件
```toml
toml = "0.8"        # TOML 配置解析
serde.workspace = true
```

### 服务发现（可选）
```toml
mdns = "3.0"        # mDNS 服务发现
# 或
zeroconf = "0.11"   # 跨平台 Bonjour/Avahi
```

### 信号处理
```toml
ctrlc = "3.4"       # Ctrl+C 优雅关闭
```

---

## 📝 配置文件格式

**`proxy.toml` 示例**:
```toml
[server]
bind_address = "0.0.0.0"
port = 8080
store_path = "/var/lib/modeld"

[auth]
require_token = true
tokens = [
    "secret-token-123",
    "another-token-456"
]

[network]
allow_anonymous = false
allowed_ips = ["192.168.1.0/24", "10.0.0.0/8"]
denied_ips = []

[limits]
max_concurrent_downloads = 10
rate_limit_requests_per_minute = 100

[discovery]
enable_mdns = true
service_name = "modeld-proxy"
```

---

## 🧪 测试策略

### 单元测试
- Range 解析器（各种边界情况）
- 认证中间件（token 验证）
- IP 白名单/黑名单
- 配置文件解析

### 集成测试
- HTTP server 启动/关闭
- 完整请求响应流程
- Range requests 多场景
- 并发下载

### 性能测试
- 吞吐量基准（目标 >100 MB/s）
- 并发连接数（目标 >100 并发）
- 延迟测试（局域网 <10ms）

---

## 🎯 成功标准

### 功能性
- [ ] HTTP server 可启动并响应请求
- [ ] Range requests 正确实现（单/多 Range）
- [ ] HF 代理缓存命中率 >90%
- [ ] Bearer Token 认证工作正常
- [ ] IP 白名单/黑名单正确执行
- [ ] 优雅关闭（Ctrl+C 无数据丢失）
- [ ] mDNS 服务发现可在局域网内找到服务器

### 性能
- [ ] 吞吐量 >100 MB/s（局域网千兆网络）
- [ ] 延迟 <10ms（局域网）
- [ ] 并发支持 >100 连接
- [ ] 内存占用 <500MB（空闲）

### 质量
- [ ] 所有单元测试通过
- [ ] 所有集成测试通过
- [ ] 无编译警告
- [ ] 代码有完整文档注释

---

## ⚠️ 风险与缓解

### 风险1: 网络安全
**影响**: 未授权访问可导致数据泄露
**缓解**:
- 默认需要 token 认证
- 提供 IP 白名单
- 审计日志记录所有访问
- 文档强调安全设置

### 风险2: 并发下载冲突
**影响**: 多客户端同时下载同一文件可能导致 CAS 损坏
**缓解**:
- 使用 Phase 2 的两阶段提交
- 文件锁机制
- WAL 恢复

### 风险3: 跨平台网络发现
**影响**: mDNS 在某些网络环境下可能不工作
**缓解**:
- 提供手动配置选项（IP:Port）
- 文档说明网络要求
- 提供诊断命令

---

## 📚 文档计划

Phase 5 完成后需要新增的文档：
- [ ] `docs/proxy-setup.md` - 代理服务器设置指南
- [ ] `docs/proxy-api.md` - HTTP API 完整规范
- [ ] `docs/lan-sharing.md` - 局域网共享配置指南
- [ ] `docs/security.md` - 安全最佳实践
- [ ] `README.md` - 添加代理服务器示例

---

## 🚀 实施顺序

**推荐实施顺序**（依赖关系优化）:

**Week 1**: HTTP Server 基础
- Day 1-2: 基础 HTTP server + 路由
- Day 3-4: Range requests 实现
- Day 5: 健康检查 + 模型列表 API

**Week 2**: HF 代理层
- Day 1-3: HF 代理核心逻辑
- Day 4-5: 缓存策略 + SHA256↔BLAKE3 映射

**Week 3**: CLI + 认证
- Day 1-2: CLI proxy 命令
- Day 3-5: Bearer Token 认证 + IP 白名单

**Week 4**: 服务发现
- Day 1-3: mDNS 服务注册
- Day 4-5: 服务发现客户端

**Week 5-6**: 客户端 SDK + 集成测试
- Week 5: Rust 客户端库
- Week 6: 集成测试 + 性能测试

---

## 📈 Phase 5 之后

Phase 5 完成后，modeld 将具备完整的局域网共享能力。Phase 6 将聚焦于高级特性：

### Phase 6 预览: 高级去重
- **Chunk-level dedup**: 文件级 → 块级去重
- **OCI 镜像层**: 类似 Docker 层的模型版本管理
- **分布式存储**: 多节点 CAS 同步
- **远程引用**: 引用远程 CAS 而非本地拷贝

---

**文档版本**: 1.0  
**当前 Phase**: Phase 5 进行中 🚧  
**上一 Phase**: Phase 4 完成 ✅  
**预计完成**: 2026-07-30  
**更新日期**: 2026-06-13
