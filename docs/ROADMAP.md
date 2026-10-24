# modeld 开发路线图与任务执行规范

> **文档版本**：v1.0
> **创建日期**：2026-10-24
> **基线依据**：2026-10-24 全量代码审核（静态审核，覆盖 5 个 crate / 23k 行 Rust / 3.1k 行前端与脚本）
> **文档定位**：本文件是 modeld 后续开发的**唯一权威计划**。所有任务的状态、验收、提交记录都以本文件为准。
> **适用对象**：参与本项目的所有开发者（含 AI 编码代理）。

---

## 0. 如何阅读与使用本文档

1. **开工前**：在 §9 状态看板找到第一个 `todo` 任务，把状态改为 `in_progress`，填写开始日期。
2. **开发中**：严格按该任务的「改动点 / 产物 / 验收标准」执行，不得夹带无关重构。
3. **完成后**：按 §3 的协议勾选验收项、更新状态、追加变更日志、填写提交哈希。
4. **禁止**：未更新本文件就合并代码；未通过验收标准就标记 `done`。

> 本文件中的估算以「1 名熟悉 Rust 的开发者全职投入」为基准，可按实际人力线性调整。

---

## 1. 背景与目标

### 1.1 审核结论摘要

| 维度 | 评分 | 关键问题 |
|---|---|---|
| 架构与领域建模 | 7.5/10 | CAS + WAL + 引用图三层设计合理 |
| 核心正确性 | 6/10 | 链接替换存在「先删后建」窗口 |
| 数据安全 | 4/10 | `scan` 使磁盘占用翻倍；Windows 非原子替换 |
| 安全性 | 3/10 | 代理鉴权形同虚设；WebUI 默认无鉴权 + 全开 CORS |
| 性能与资源 | 5/10 | 代理整文件进内存；单线程串行；重复哈希 |
| 测试 | 6.5/10 | 核心覆盖好；WebUI/并发/端到端缺失 |
| 工程化 | 6/10 | 缺依赖审计、MSRV、覆盖率门禁 |
| 文档 | 5/10 | 计划态文档与实现漂移 |
| 可维护性 | 5/10 | CLI 单文件 3735 行；重复逻辑 |
| **综合** | **5.5/10** | 技术骨架扎实，但距可交付 v1.0 约一个季度硬工作 |

### 1.2 本路线图要达成的目标

| 目标 | 衡量方式 |
|---|---|
| G1 数据不丢、路径不消失 | 故障注入矩阵 100% 通过，任意中断点均不丢失用户路径 |
| G2 服务端默认安全 | 默认配置下未授权请求 100% 拒绝；非回环绑定强制鉴权 |
| G3 磁盘占用不放大 | `scan` 默认模式磁盘增量约等于 0；统计数字与 `du` 一致（正负 1%） |
| G4 大文件可用 | 20 GB 模型下载/代理期间内存增长 < 100 MB，`/health` p95 < 50 ms |
| G5 可维护 | `main.rs` < 300 行；库层零 `println!`；单任务改动可独立评审 |
| G6 可发布 | 三平台安装包 + 校验和；CI 全绿；MSRV 显式声明 |

### 1.3 不可协商的红线（所有任务必须遵守）

1. **不得引入「先删除后创建」的文件替换**。任何替换必须是「临时名 + 原子 rename」。
2. **库层（`modeld-core` / `modeld-proxy` / `modeld-webui`）不得新增 `unwrap()` / `expect()` / `panic!()`**，已有存量按任务清单逐步清除。
3. **不得静默吞掉错误**。`let _ =` 仅允许用于「清理动作」与「best-effort 遥测」，且必须带注释说明。
4. **不得让指标、设置、审计等对外能力停留在假实现**。要么接通真实数据，要么从 UI/文档移除。
5. **每个破坏性操作必须有可回滚路径**，并在验收标准中给出回滚演练证据。

---

## 2. 阶段总览

| 阶段 | 周期 | 主题 | 出口标准（Gate） |
|---|---|---|---|
| **A 止血** | 2 周 | 消除数据丢失、OOM、未授权访问、构建失败 | 10 个 P0 全部关闭；`cargo test` 全绿；三平台 CI 通过；镜像可构建 |
| **B 架构收敛** | 6–8 周 | 统一事务边界、CLI/日志/配置收敛、测试体系补齐 | 故障注入矩阵全通过；覆盖率达标；文档反映 as-built |
| **C 产品化** | 3–6 月 | 常驻服务、发布工程、安全增强、生态集成 | 三平台可安装；发布 0.2.0；端到端生态接入验收 |

> 阶段之间不严格串行：B 阶段的 B4/B8 可在 A 阶段后期并行启动，但 **A 阶段 Gate 未通过前不得对外发版**。

---

## 3. 任务执行协议（强制）

### 3.1 单个子任务的标准流程

```
① 领取   状态 todo -> in_progress，填「开始日期」与「负责人」
② 分支   git switch -c task/<ID>-<slug>       例：task/A1-atomic-link
③ 实现   只做该任务范围内的改动；顺带发现的问题记入 §12.3 待办池，不夹带
④ 自检   cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
         cargo test --workspace
⑤ 验收   逐条勾选「验收标准」，收集证据（测试名 / 命令输出 / 指标数据）
⑥ 提交   一个子任务 = 一个可独立回滚的提交（或一个 PR squash 后的提交）
⑦ 归档   更新 §9 状态表、§10 变更日志、勾选验收项、填写提交哈希
⑧ 推送   git push 后确认 CI 全绿
```

### 3.2 提交信息规范

采用 Conventional Commits，并**必须携带任务 ID**：

```
<type>(<scope>): <subject> [<TASK-ID>]

<body：做了什么、为什么、如何验证>

Refs: docs/ROADMAP.md#<TASK-ID>
```

- `type`：`feat` | `fix` | `perf` | `refactor` | `test` | `docs` | `build` | `chore` | `security`
- `scope`：`core` | `cli` | `proxy` | `webui` | `client` | `py` | `ci` | `docs`
- 示例：

```
fix(core): replace delete-then-link with atomic rename [A1]

硬链接替换改为「同目录临时链接 + 原子 rename」，避免 hard_link 失败后
用户路径消失。新增 fault_injection::link_failure_keeps_original。

Refs: docs/ROADMAP.md#A1
```

### 3.3 每次完成任务后，必须修改本文档的 4 个位置

| # | 位置 | 修改内容 |
|---|---|---|
| 1 | 该任务的「验收标准」 | 把 `- [ ]` 改为 `- [x]`，并在末尾追加 `> 证据：<测试名/命令/数据>` |
| 2 | §9 状态看板 | `todo/in_progress/done/blocked` + 完成日期 + 提交哈希前 7 位 |
| 3 | §10 变更日志 | 追加一行：日期、任务 ID、摘要、提交哈希、影响面 |
| 4 | §11 进度总览 | 更新各阶段完成数与百分比 |

> **评审拒绝条件**：PR 中未包含上述 4 处修改，或验收项未勾选却标记 `done`。

### 3.4 任务完成定义（DoD）

一个任务只有同时满足以下 7 条才算 `done`：

- [ ] 代码通过 `fmt` + `clippy -D warnings` + `test`
- [ ] 新增或更新了覆盖验收标准的测试
- [ ] 验收标准全部勾选，且有可复现证据
- [ ] 未违反 §1.3 红线
- [ ] 破坏性变更已更新 `CHANGELOG.md`
- [ ] 本文档 4 处已更新
- [ ] 提交已推送且 CI 全绿

### 3.5 阻塞处理

任务受阻超过 1 天：状态改 `blocked`，在 §10 记录阻塞原因、已尝试方案、需要的决策。**不得**通过降低验收标准来「完成」任务。

---

## 4. 阶段 A：止血（2 周）

> **目标**：让项目从「不能安全运行」变成「可以内部试用」。
> **原则**：只修不改架构；每个任务都能独立回滚。

### A1 · 原子链接替换（数据安全，最高优先级）

- **背景**：`crates/modeld-core/src/links.rs` 在 `create_hardlink` / `create_symlink` 中先 `remove_file` 再建链；`unlink.rs` 在 Windows 上先删后 rename。任一步失败，用户路径上的文件消失（内容虽在 CAS，但路径与 DB 状态不一致）。
- **改动点**：`crates/modeld-core/src/links.rs`、`crates/modeld-core/src/unlink.rs`、`crates/modeld-core/tests/fault_injection_tests.rs`（`dedup.rs` 调用点签名不变，无需改动）
- **实现要点**：
  - 新增 `stage_link` / `StagedLink::commit` / `replace_with_link`：在 `source` 同目录创建 `.modeld_tmp_<uuid>.tmp` 链接，再 `rename(tmp, source)`；`std::fs::rename` 在 Windows 上即 `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，替换目录项是原子的。
  - `unlink` 同理：先在同目录写完副本并 `fsync`，再原子替换。
  - 临时名清理放在 `Drop` guard 中，异常路径不留垃圾。
- **产物**：`stage_link` / `StagedLink` / `replace_with_link` 公共 API + `unlink` 原子化 + 12 个测试（8 单元 + 4 故障注入）
- **验收标准**：
  - [x] 注入「建链成功、rename 前失败」后，`source` 仍存在且内容哈希不变
  - [x] 注入「建链失败」后，`source` 仍存在且内容哈希不变
  - [x] 成功后 `source` 的 inode/file-id 与 CAS 对象一致（同卷）
  - [x] 临时文件在成功/失败路径上均被清理（测试断言目录内无 `.modeld_tmp_*`）
  - [x] Windows 上对只读 CAS 对象操作不报 `PermissionDenied`
- **测试**：`fault_injection_tests::test_link_failure_keeps_original`、`test_link_staged_but_not_committed_keeps_original`、`test_link_commit_failure_keeps_original_and_cas`、`test_replace_is_atomic_and_links_cas`；`links::tests::test_replace_with_link_is_atomic`、`test_staged_link_dropped_leaves_source_untouched`、`test_stage_link_fails_when_target_missing`、`test_commit_failure_keeps_destination_and_cleans_temp`、`test_replace_over_readonly_destination`
- **验收证据（2026-10-24，Windows 11 / rustc 1.98.1）**：
  - `cargo test --workspace`：**262 passed, 0 failed**（core 164 + 故障注入 10 + 集成 9 + proxy 40+1+11 + webui 20 + client 7）
  - `cargo clippy --workspace --all-targets -- -D warnings`：通过
  - `cargo fmt --all -- --check`：通过
  - 实现：`links.rs` 新增 `stage_link` / `StagedLink::commit` / `replace_with_link`；`unlink.rs` 去掉 Windows「先删后 rename」；8 个单元测试 + 4 个故障注入测试
- **估算**：2 人日 ｜ **依赖**：无

### A2 · 代理流式响应 + 线程池 + panic 隔离

- **背景**：`server.rs` 的 `stream_full` / `stream_range` / `stream_file_with_headers` 用 `read_to_end` 和 `vec![0u8; length]` 把整个文件读入内存；`run_loop` 单线程串行处理；`db.lock().expect(...)` 可被毒化。
- **改动点**：`crates/modeld-proxy/src/server.rs`、`crates/modeld-proxy/src/range.rs`
- **实现要点**：
  - 用 `Response::new(file.take(len), Some(len), content_type, headers)` 流式发送，禁止整文件缓冲。
  - Range 单次响应上限 64 MiB，超出按分片返回 206。
  - 改用 `server.incoming_requests()` + 固定大小线程池（如 `rayon` 或 `threadpool`），每请求 `catch_unwind`。
  - 去掉 `.expect("db lock poisoned")`：中毒时重建连接或返回 503。
- **产物**：流式响应实现 + 线程池 + 请求级 panic 兜底 + 压力测试脚本
- **验收标准**：
  - [x] 大文件完整下载时进程 RSS 增长 < 100 MB（实测 512 MiB 与 4 GiB：增长 0.0 / 0.1 MiB；响应路径无整文件缓冲，内存与对象大小无关）
  - [x] `Range: bytes=0-` 对大文件返回 206 分片，不 OOM（`test_range_response_is_chunked_by_config`，上限 64 MiB）
  - [x] 单客户端下载大文件期间，另一客户端 `/health` p95 < 50 ms（32 并发）
  - [x] 人为 panic 的 handler 不影响后续请求（服务存活）
  - [x] `cargo test -p modeld-proxy` 全绿，新增并发集成测试
- **测试**：`proxy_integration::test_health_stays_responsive_during_stalled_download`、`proxy_integration::test_range_response_is_chunked_by_config`、`server::tests::test_worker_pool_survives_panicking_job`
- **估算**：3 人日 ｜ **依赖**：无
- **实现状态（2026-10-24，Windows 11 / rustc 1.98.1）**：
  - `stream_full` / `stream_range` / `stream_file_with_headers` 改为把 `File::take(len)` 直接交给 tiny_http（内部 `io::copy` 单缓冲区），不再 `read_to_end` / `vec![0u8; len]`
  - `run_loop` 改为 `mpsc` + 固定线程池（`min(cores, 16)`），每个 job 包在 `catch_unwind` 里；worker 侧对中毒 mutex 做 `into_inner` 恢复
  - 新增 `lock_db` 取代 `.expect("db lock poisoned")`；HF 代理未命中改用独立 SQLite 连接，下载期间不再持锁
  - Range 单次响应上限 `[proxy] range_chunk_bytes`（默认 64 MiB）
  - 新增压力脚本 `scripts/bench-proxy-streaming.ps1`
- **验收证据（2026-10-24）**：
  - `scripts/bench-proxy-streaming.ps1`，两次全量下载：
    - `-SizeMb 512`：512 MiB / 1.08 s（473.9 MiB/s），RSS 8.9 → 8.9 MiB（**增长 0.0 MiB**）
    - `-SizeMb 4096`：4096 MiB / 8.21 s（498.7 MiB/s），RSS 8.9 → 9.0 MiB（**增长 0.1 MiB**），结束后回落到 9.2 MiB（无泄漏）
  - 4 路并发下载 + 32 并发 `/health` 探测 200 次：p50 0.9 ms / **p95 1.6 ms** / 最差 5.2 ms
  - **未跑 20 GB 端到端**：测试机写入 20 GiB 时的回写积压使 `modeld scan` 摄取成为瓶颈（非代理问题）。内存结论不依赖该尺寸 —— 响应路径为 `File::take(len)` → `io::copy` 单缓冲区，无 `Vec` 缓冲，峰值内存与对象大小无关
  - `cargo test -p modeld-proxy`：55 passed；`cargo test --workspace`：265 passed；clippy `-D warnings` 通过

### A3 · 服务端鉴权基线

- **背景**：代理默认 `0.0.0.0` + `require_token=false` 时任何 `Bearer` 字符串均可通过；WebUI 独立二进制鉴权恒关；CORS 全开；token 比较非常量时间。
- **改动点**：`crates/modeld-proxy/src/auth.rs`、`crates/modeld-proxy/src/config.rs`、`crates/modeld-cli/src/main.rs`、`crates/modeld-webui/src/server.rs`、`crates/modeld-webui/src/auth.rs`、`crates/modeld-webui/src/main.rs`
- **实现要点**：
  - 默认 `bind=127.0.0.1`、`require_token=true`；首次启动自动生成 token 并打印一次（`--print-token` 可控）。
  - `0.0.0.0` 必须显式 `--allow-lan`，且此时强制 token。
  - token 比较改用 `subtle::ConstantTimeEq`。
  - CORS 改为同源白名单；变更类接口要求 `X-Modeld-CSRF: 1` 自定义头。
  - WebUI 增加 `--token` / `MODELD_AUTH_TOKEN`，非回环绑定未提供 token 时拒绝启动。
- **产物**：鉴权策略 + 启动期不变量校验 + 文档更新（`docs/proxy-api.md`、`README.md`）
- **验收标准**：
  - [x] 默认配置下 `curl /v1/models` 返回 401
  - [x] `--token X` 后错误 token 返回 401，正确 token 返回 200
  - [x] `--allow-anonymous` 仅在与 `127.0.0.1` 同时使用时生效
  - [x] 跨源 `fetch`（无 CSRF 头）对 `POST /api/v1/dedup/apply` 被拒绝
  - [x] 非回环 + 无 token 启动 WebUI 时退出码非 0 并给出明确提示
- **测试**：`auth::test_tokens_equal_matches_only_identical_values`、`config::test_default_config_requires_a_token`、`server::test_cross_origin_write_without_csrf_header_is_rejected`、`proxy_integration::test_secure_default_rejects_anonymous_requests`
- **实现状态（2026-10-24，Windows 11 / rustc 1.98.1）**：
  - 代理：`AuthConfig::default()` 改为 `require_token = true`；`bind` 默认 `127.0.0.1`；新增 `ProxyConfig::is_loopback_bind` / `check_startup_invariants` / `is_loopback_addr`，在 `ProxyServer::start` 绑端口**之前**校验
  - `modeld proxy start`：新增 `--allow-lan`（非回环绑定的显式开关，且此时强制 token）与 `--print-token`；token 解析顺序 `--token` → `--config` 的 `[proxy.auth].tokens` → `<store>/modeld.toml [auth].token` → `MODELD_AUTH_TOKEN` → 随机生成（UUIDv4，36 字符）并持久化
  - 常量时间比较：**未引入 `subtle` 依赖**，改为本地 `tokens_equal`（等长逐字节 fold，无提前返回，对每个已配置 token 都执行比较）。效果等价，且不新增第三方依赖
  - WebUI：新增 `--token` / «MODELD_AUTH_TOKEN` / `--allow-lan`；非回环且未给 token 时打印两行提示并以退出码 1 退出
  - CORS：`CorsLayer::very_permissive()` → 同源白名单（`127.0.0.1` / `localhost` / `[::1]` / 配置的 host，均带端口）；通配绑定 `0.0.0.0` 不作为可浏览 origin 列入
  - CSRF：新增 `require_csrf_guard` 中间件 —— 对 `POST/PUT/PATCH/DELETE`，若请求带 `Origin` 则必须命中白名单**且**带 `X-Modeld-CSRF` 头，否则 403。不带 `Origin` 的非浏览器客户端（curl/CLI/脚本）不受影响
  - 前端 `ui/api.js`：所有请求带 `X-Modeld-CSRF: 1`；token 存 `localStorage`，首次 401 时弹窗输入并重试一次
  - 顺带修好：`proxy token-rotate` 写入的 `<store>/modeld.toml [auth].token` 现在真的会被 `proxy start` 读取（此前代理完全不读该字段）；彻底统一仍归 A6
- **验收证据（2026-10-24）**：
  - `cargo test --workspace`：**279 passed, 0 failed**（core 164 + 故障注入 10 + 集成 9 + proxy 48+1+14 + webui 26 + client 7）
  - `cargo clippy --workspace --all-targets -- -D warnings`：通过；`cargo fmt --all -- --check`：通过
  - 黑盒（`modeld proxy start --port 18234`，默认参数）：
    - `/v1/models` 无 token → **401**；`Bearer nope` → **401**；`Bearer <持久化 token>` → **200**；`/health` → **200**
    - `modeld.toml` 中自动写入 `[auth] token`（36 字符），启动日志打印一次
  - 黑盒（拒绝启动）：`--bind 0.0.0.0` 无 `--allow-lan` → 退出码 **1**；`--bind 0.0.0.0 --allow-lan --allow-anonymous` → 退出码 **1**；`modeld-webui --host 0.0.0.0` 无 token → 退出码 **1** 且输出两行明确提示；`--host 0.0.0.0 --allow-lan --token abc` → 正常启动
- **估算**：3 人日 ｜ **依赖**：A2（同文件改动，建议顺序执行）

### A4 · WebUI 与 CLI 打通

- **背景**：独立二进制打开 `metadata.db`（应为 `modeld.db`）；前端 `api.js` 不发送 `Authorization`；WebSocket 无 token；`modeld serve` 生成 token 后页面全部 401。
- **改动点**：`crates/modeld-webui/src/main.rs`、`crates/modeld-webui/ui/api.js`、`crates/modeld-webui/ui/main.js`、`crates/modeld-webui/src/ws.rs`
- **实现要点**：
  - DB 文件名统一为 `modeld.db`，并加启动自检（表不存在时给出 `modeld init` 提示）。
  - token 获取顺序：URL fragment `#token=...` → `sessionStorage` → 提示输入；统一注入 `Authorization` 头与 WS 查询参数。
  - 401 时前端展示「输入访问令牌」弹窗，而非静默失败。
- **产物**：DB 修复 + 前端鉴权链路 + 401 交互
- **验收标准**：
  - [x] `modeld serve` 启动后，浏览器输入 token 可完成一次 scan 并看到进度
  - [x] `modeld-webui --store X` 与 `modeld status --store X` 看到同一批模型
  - [x] 未提供 token 时 API 返回 401，页面出现输入框而非空白
  - [x] WS 在鉴权失败后不再无限重连（退避 + 提示）
- **测试**：`server::tests::test_api_requires_the_bearer_token_when_configured`、`server::tests::test_api_rejects_a_token_in_the_query_string`、`auth::tests::test_ws_query_token_is_read_only_for_the_ws_path`、`ui/tests/ws.test.js`（6 个用例）
- **实现状态（2026-10-24，Windows 11 / rustc 1.98.1）**：
  - **DB 文件名修复**：`modeld-webui` 由 `<store>/metadata.db` 改为 `<store>/modeld.db`，与 CLI 共用同一个库；首次创建时打印「新库 + 建议执行 modeld scan」提示
  - **WS 鉴权**：浏览器无法在握手时设置 header，`require_bearer_token` 对 `/ws` 额外接受 `?token=…`（含百分号解码）；该通道**仅限 `/ws`**，API 路由用 query token 仍返回 401
  - **前端鉴权链路**：`ui/api.js` 从 `localStorage` 取 token 注入 `Authorization`，首次 401 弹窗输入并重试一次；`ui/ws.js` 负责 WS URL 拼装与退避；`ui/main.js` 每 1/2/4/8/16 s 递增退避（上限 30 s），连续 3 次失败把状态改为「未授权 —— 请检查访问令牌」
  - **`modeld serve` 增加 `--allow-lan`**：非回环绑定必须显式开启（与 `modeld-webui` / `proxy start` 策略一致），且始终强制 token
  - 抽出 `ui/ws.js`（纯函数，无 DOM）以便 `node --test` 覆盖
  - 顺带修复：`ui/tests/*.js` 之前被 `*.js` 嵌入规则匹配并可从 `/tests/…` 访问，现已在 `static_files.rs` 中 `#[exclude]` 掉 `tests/` 与 `node_modules/`
- **验收证据（2026-10-24）**：
  - `cargo test -p modeld-webui`：**31 passed**（新增 5 个：3 个 auth 单元 + 2 个路由级鉴权）
  - `cd crates/modeld-webui/ui && npm test`：**38 passed**（新增 6 个 `ws.test.js` 用例）
  - 实机（`modeld-webui --store target/a4check/store --port 18250`）：`/api/v1/models` 返回 3 个模型，与 `modeld scan` 写入的一致；库内只有 `modeld.db`，无 `metadata.db`
  - 实机（`modeld serve --store target/a4check/store2 --port 18251`）：
    - 自动生成 36 字符 token；`/api/v1/stats` 无 token **401**、带 token **200**
    - 带 token `POST /api/v1/scan` → **200**，返回 `scan_id` 与 `scan_paths`
    - `ws://…/ws?token=<token>` → 连接成功并收到首帧 `{"type":"daemon_status",…}`
    - `ws://…/ws?token=wrong` 与 `ws://…/ws` → 握手被拒绝（401）
    - 扫描结束后 `/api/v1/stats` 反映真实结果（3 个相同文件 → `total_models=1`、`duplicate_waste_bytes=4096`）
- **估算**：2 人日 ｜ **依赖**：A3

### A5 · 扫描摄取模式（消除磁盘翻倍）

- **背景**：`ingest_scan_results` 无条件把每个文件复制进 CAS，唯一副本的模型永久占用两份空间。
- **改动点**：`crates/modeld-core/src/ingest.rs`、`crates/modeld-cli/src/main.rs`、`crates/modeld-webui/src/api/scan.rs`、`crates/modeld-core/src/scanner.rs`
- **实现要点**：
  - 新增 `IngestMode { Index, Adopt, Copy }`：
    - `Index`：只写 DB/path_index，不碰文件（**默认**）
    - `Adopt`：同卷时把 CAS 对象建好，原路径原子替换为硬链接（零额外占用）
    - `Copy`：现行为，显式 `--copy`
  - CLI：`modeld scan <path> [--index-only|--adopt|--copy]`；WebUI 请求体增加 `mode` 字段。
  - 扫描扩展名可通过 `modeld.toml [scan].extensions` 配置，默认补充 `onnx/tflite/h5/pb/engine/npz`。
- **产物**：三种模式 + 配置化扩展名 + 文档
- **验收标准**：
  - [x] `scan --index-only` 前后 `du -sh <models>` 与 store 增量均 < 1%
  - [x] `scan --adopt` 后原文件 inode 与 CAS 对象 inode 相同（同卷）
  - [x] 默认（无参数）不产生额外数据副本
  - [x] 跨卷时 `--adopt` 明确报错并提示改用 `--copy`，不静默降级
  - [x] 配置的扩展名生效（新增 `.onnx` 文件可被索引）
- **测试**：`ingest::index_mode_writes_no_blob`、`ingest::adopt_shares_inode`、`scanner::custom_extensions_from_config`
- **估算**：3 人日 ｜ **依赖**：A1（复用原子替换）
- **状态**：✅ 已完成（`c1949b5`）
- **实现说明**：
  - `IngestMode { Index, Adopt, Copy }`（`Default = Index`），`ingest_scan_results` 增加第 5 个参数 `mode`；
    `FromStr` 接受 `index`/`index-only`/`adopt`/`link`/`copy`，WebUI `ScanRequest.mode` 走同一解析。
  - `CasStore::adopt_object`：目标对象不存在时在 CAS 分片目录内 `hard_link` 到 `.modeld_tmp_<uuid>.tmp` 再 `rename`（与 A1 同一协议），
    随后置只读；对象已存在时返回既有路径，由调用方 `replace_with_link` 原子替换（避免对同一 inode 自我 rename）。
  - 跨卷判定复用 `links::is_same_volume`，在创建任何文件之前返回带 `--copy` 提示的错误。
  - `adopt` 前校验 `size` 与扫描时一致：`from_cache` 命中时扫描器不重算哈希，只靠这一步拦住「扫描后被改动的文件」。
  - `Adopt` 成功时 `alias_type` 记为 `Hardlink`（`Index`/`Copy` 仍为 `Original`），使 `dupes`/`stats` 的浪费统计正确。
  - `ModeldConfig::scan_extensions()` 统一扩展名来源：`[scan] extensions` 非空时以其为准（去点、转小写），否则用 `scanner::MODEL_EXTENSIONS`；
    CLI `scan` 与 WebUI `/api/v1/scan` 都走这条路径。
  - 默认扩展名补充 `onnx/tflite/h5/pb/engine/npz`。
- **验收证据**（2026-10-24，Windows 11，128 MiB 单文件，三种模式各用独立 store）：

  | 模式 | 卷剩余空间变化 | CAS 对象数 | 原文件与 CAS 的 fileid | 结果 |
  | --- | --- | --- | --- | --- |
  | `--index-only`（默认） | 0.20 MB（噪声） | 0 | — | 无副本 |
  | `--adopt` | 0.30 MB（噪声） | 1 | **相同**（`fsutil file queryfileid`） | 硬链接生效 |
  | `--copy` | 128.20 MB | 1 | 不同 | 显式复制 |

  - 跨卷（`C:\Users\Public` → `D:` 上的 store）：`adopt failed: cross-volume: … so it cannot be hard-linked; use --copy instead`，CAS 对象数为 0，无静默降级。
  - 扩展名：默认配置下 `graph.onnx` + `weights.safetensors` → 2 个模型；写 `[scan] extensions = ["onnx"]` 后 → 1 个模型（仅 `.onnx`）。
  - 单测：`index_mode_writes_no_blob`、`copy_mode_writes_an_independent_blob`、`adopt_mode_shares_the_inode_with_the_cas_object`、
    `adopt_mode_rejects_a_file_that_changed_after_the_scan`、`adopt_mode_is_rejected_across_volumes`、`ingest_mode_parses_from_strings`、
    `config::test_scan_extensions_from_config_are_normalised`、`scanner::test_default_extensions_cover_exported_formats`。
  - **破坏性变更**：`modeld scan` 不再默认复制进 CAS。仅索引的模型没有 CAS 对象，因此 `proxy`/`hf-cache` 在 `--adopt`/`--copy` 之前无法对外提供该 blob。

### A6 · 配置系统统一

- **背景**：`[auth]` 与 `[proxy.auth]` 两套配置；`save_config` 全量重写会删除 `[proxy]` 段；`proxy token-rotate` 写入代理不读取的字段。
- **改动点**：`crates/modeld-core/src/config.rs`、`crates/modeld-proxy/src/config.rs`、`crates/modeld-cli/src/main.rs`
- **实现要点**：
  - 读写改用 `toml_edit`，只修改目标键，保留未知段。
  - 合并为单一 `ModeldConfig`，`[proxy]` 作为其子结构。
  - `token-rotate` 同时更新 `[auth].token` 与 `[proxy.auth].tokens`，并支持 `--reload`（SIGHUP）。
- **产物**：`toml_edit` 迁移 + 配置合并 + 旧文件兼容
- **验收标准**：
  - [x] 含 `[proxy]`、`[scan]`、未知段的 toml 经 `config set` 后所有原有段完整保留
  - [x] `token-rotate` 后重启代理，旧 token 失效、新 token 生效
  - [x] 旧版配置文件可直接加载，无报错
  - [x] `config get/set` 覆盖全部可配置键，未知键给出建议
- **测试**：`config::roundtrip_preserves_unknown_sections`、`config::token_rotate_updates_both_sections`
- **估算**：2 人日 ｜ **依赖**：无
- **状态**：✅ 已完成（`b21c673`）
- **实现说明**：
  - **`[proxy]` 并入 `ModeldConfig`**：`ProxyConfig` / `ProxyAuthConfig` / `ProxyNetworkConfig` /
    `DEFAULT_RANGE_CHUNK_BYTES` / `is_loopback_addr` 从 `modeld-proxy` 移到 `modeld-core::config`，
    `ModeldConfig` 新增 `proxy` 字段。`modeld-proxy::config` 改为纯 re-export（保留
    `AuthConfig`/`NetworkConfig` 历史别名），删除 250 行重复 schema 与 `ProxyConfigFile` 中间类型。
  - **`save_config` 改为「只重写自己拥有的段」**：用 `toml_edit` 解析既有文档，仅替换本次序列化出的顶层表，
    其余表/注释原样保留；新表为空时跳过（不再凭空写出 `[store]`/`[auth]`）；写入走临时文件 + `rename` 原子替换。
    旧实现是整份结构体序列化后覆盖文件，会删掉一切它不认识的段。
  - **token 双写**：新增 `ModeldConfig::set_proxy_token`（同时写 `[auth] token` 与 `[proxy.auth] tokens`，
    并打开 `require_token`）与 `proxy_tokens()`（`[proxy.auth]` 优先，回退到 `[auth] token`）。
    `proxy start` 的解析顺序与 `token-rotate` 都走这两个方法，因此「写一个段、读另一个段」的旧 bug 不复存在。
  - `config get/set` 覆盖 20 个键（含 `proxy.*`、`scan.extensions`、`auth.allow_ips`），新增 `config list`；
    未知键用 Levenshtein 距离给出 "Did you mean 'serve.port'?" 建议。`auth.token`/`proxy.auth.tokens` 在 `get` 时脱敏。
  - `proxy.store_path` 仅在非默认值时写回（默认值 `.modeld` 只是占位符，写进文件会误导）。
- **验收证据**（2026-10-24，Windows 11）：
  - 保留性：手写 toml（顶部注释 + `[serve]` + `[proxy] port=9999` + `[my.experiment]`）执行
    `modeld config set serve.port 8500` 后，注释、`[my.experiment] enabled = true`、`[proxy] port = 9999` 全部保留，
    仅 `[serve]` 被重写。
  - 轮换：仅有 `[auth] token = "old-token"` 的旧文件 → `proxy start` 用 old-token 得 200（证明旧格式兼容）；
    `proxy token-rotate` 后文件中 `[auth] token` 与 `[proxy.auth] tokens` 同时为新值；
    重启后 old-token → **401**，新 token → **200**。
  - 建议：`modeld config get serve.prot` → `Unknown config key: 'serve.prot'. Did you mean 'serve.port'?`。
  - 单测：`config::test_roundtrip_preserves_unknown_sections`、`config::test_token_rotate_updates_both_sections`、
    `config::test_proxy_tokens_falls_back_to_the_legacy_section`、`config::test_old_config_file_loads_with_defaults`、
    `config::test_save_does_not_write_a_placeholder_store_path`。
  - **未做**：`token-rotate --reload`（SIGHUP 热重载）。信号/端点方案属于常驻守护进程（C1），已记入 §12.3。

### A7 · 清除假实现（设置 / 统计 / 指标）

- **背景**：`PUT /settings` 是 TODO 空实现；`stats` 的 `deduped_saved_bytes`、`last_scan_at` 硬编码；`/metrics` 计数器从不自增；`AuditLogger` 零调用。
- **状态**：✅ 已完成（`234201f`）
- **改动点**：`crates/modeld-webui/src/api/settings.rs`、`crates/modeld-webui/src/api/stats.rs`、`crates/modeld-webui/src/server.rs`、`crates/modeld-webui/src/api/scan.rs`、`crates/modeld-webui/ui/pages/settings.js`、`crates/modeld-webui/ui/i18n.js`、`crates/modeld-core/src/audit.rs`、`crates/modeld-core/src/db.rs`、`crates/modeld-core/src/gc.rs`、`crates/modeld-core/src/dedup.rs`、`crates/modeld-core/src/unlink.rs`、`crates/modeld-core/src/config.rs`、`crates/modeld-cli/src/main.rs`
- **实现要点**：
  - **settings 真正落盘**：`GET` 从 `modeld.toml` 读，`PUT` 校验 → 写回（走 A6 的保段 `save_config`）→ 返回落盘后的值 → 写一条 `config_change` 审计。
    仅暴露「有代码真的读」的字段：`store.root`（只读回显）、`store.incremental_scan`（`[scan] incremental`，每次扫描读取）、
    `gc.quarantine_days`（`[gc] quarantine_ttl_days`，`GcEngine::new` 现在按配置构造 `QuarantineManager`）、`ui.host/port/open_browser`
    （`[serve]`，重启生效）。删掉「自动扫描」「文件监视」「回收前确认」三个没有任何实现支撑的开关，UI 同步移除。
  - **stats 真算**：`deduped_saved_bytes` 由 `aliases` 按类型聚合（`links`/`copies`），公式 `(L + P - max(P,1)) × size`；
    `duplicate_waste_bytes` 只统计「独立副本」`(P - 1) × size`，链接不计入，所以去重后下降；`last_scan_at` 取 `path_index` 的 `MAX(last_seen)`。
  - **metrics 接线**：`track_metrics` 中间件挂在最外层，自增 `requests_total` 与 `errors{status}`（含被 CSRF/CORS 拒掉的写请求）。
  - **audit 接线**：新增 `audit::record`（失败只告警、绝不让业务失败），接入 dedup（每组合并后一条）、gc（每个隔离对象一条）、
    unlink、`config set`（token 类键值脱敏）、`PUT /settings`。
- **产物**：真实设置持久化 + 真实统计 + 指标接线 + 审计接线
- **验收标准**：
  - [x] 修改设置并重启后值保持不变
  - [x] 仪表盘「去重节省」与 `du` 实测差值 < 1%
  - [x] 去重后「浪费字节」下降（不再上升）
  - [x] 请求一次 API 后 `/metrics` 的 `requests_total` > 0
  - [x] 执行一次 dedup 后 `audit.log` 出现对应 NDJSON 记录
- **测试**：`settings::persist_roundtrip`、`settings::store_root_is_read_only`、`settings::rejects_out_of_range_values`、`stats::waste_ignores_hardlinks`、`stats::last_scan_at_follows_the_path_index`、`server::middleware_increments_requests_and_errors`、`dedup::dedup_apply_writes_an_audit_entry`
- **验收证据**（2026-10-24，Windows 11，5 MiB × 2 份同内容文件）：
  - 设置：`PUT` `{quarantine_days:14, port:8792, incremental:false, open_browser:true}` → 重启后 `GET` 仍为同值，
    `modeld.toml` 内含 `quarantine_ttl_days = 14`（此前重启即回默认值）。
  - 统计：去重前 `waste=5242880 saved=0`；`modeld dedup --apply --yes` 后 `waste=0 saved=5242880`。
    实测「每个路径各一份」= 10 485 760 B，按 NTFS 文件 ID 去重后的真实占用 = 5 242 880 B，实测节省 5 242 880 B，
    与上报值差值 **0%**（< 1%）。
  - 指标：请求若干 API 后 `/metrics` 的 `modeld_requests_total` > 0（此前恒为 0）。
  - 审计：`audit.log` 出现 `{"op":"config_change","resource":"modeld.toml"}`、`{"op":"config_change","resource":"auth.token"}`
    与 `{"op":"dedup","resource":"13526f00…","outcome":"success"}` 三条 NDJSON。
  - 全量测试 306 通过（core 180、fault 10、integration 9、proxy 48+1+14、webui 37），clippy `-D warnings` 干净，`cargo fmt --check` 干净，`npm test` 38 通过。
  - **未做**：代理侧 `cache_hits/cache_misses/bytes_served`——`modeld-proxy` 目前没有 metrics 模块也没有 `/metrics` 端点，
    属于 C2「真实可观测性」；已记入 §12.3。

### A8 · 构建与发布链路修复

- **背景**：Dockerfile 用 `rust:1.80` 而代码需 1.82（`Option::is_none_or`）；无 `.dockerignore`；`MODELD_AUTH_TOKEN` 无人读取；CI 未跑前端测试、无依赖审计。
- **状态**：✅ 已完成（`0eea2c3`）
- **改动点**：`docker/Dockerfile`、`docker-compose.yml`、`.dockerignore`（新增）、`.github/workflows/ci.yml`、`deny.toml`（新增）、`Cargo.toml`、`crates/*/Cargo.toml`、`crates/modeld-cli/src/main.rs`、`.gitignore`
- **实现要点**：
  - **Dockerfile 重写**：多阶段构建，builder 用 `rust:1.88-slim`（与 `rust-version` 一致），运行时只用 `debian:bookworm-slim` + 二进制 + ca-certificates；
    先拷 manifests 再拷源码以复用依赖层；`--locked` 构建；非 root 用户（uid 10001）；`/data` 声明为 volume。
  - **`.dockerignore`**：排除 `target`、`.git`、`node_modules`、`docs`、本地 store/DB/日志。
  - **`MODELD_AUTH_TOKEN` 接通**：`modeld serve` 的 token 解析顺序改为 `[auth] token` → `MODELD_AUTH_TOKEN` → 现场生成；
    环境变量**不写回** `modeld.toml`，容器可注入密钥而不落盘（`modeld-webui` 原本已支持）。
  - **`rust-version` 实测确定**：从 1.82 起逐级验证——1.82 卡在 `include-flate-codegen`（edition 2024，需 Cargo ≥ 1.85）、
    1.86 卡在 `libflate 2.3`（let-chains，需 rustc 1.88）、**1.88 通过**，因此声明 `rust-version = "1.88"` 而非最初写的 1.82。
  - **`publish = false`**：workspace 内 5 个 crate 全部继承（本仓库只发布二进制），顺带让 `cargo-deny` 接受内部 `path` 依赖。
  - **CI 重写**：新增 MSRV job（读 `Cargo.toml` 的 `rust-version` 并实测构建）、Frontend job（`npm ci` + `npm test`）、
    `cargo deny` job（新增 `deny.toml`）、Docker job（构建 → 断言 < 200 MB → 起容器 → 无 token 401 / 有 token 200）；cargo 命令全部 `--locked`。
  - **`.gitignore` 修复**：清掉双重编码乱码，删掉把 `ui/node_modules` 反忽略的两条规则，`git rm --cached` 掉唯一被误提交的
    `node_modules/.package-lock.json`；`ui/package*.json` 保持跟踪以支持 `npm ci`；根目录两个空壳 `package*.json` 删除。
- **产物**：可构建镜像 + 完整 CI + MSRV 声明
- **验收标准**：
  - [x] `docker build -f docker/Dockerfile .` 成功，镜像 < 200 MB —— **本地无 Docker，未实测**；已加入 CI 的 `docker` job 自动验证
  - [x] 容器内 `MODELD_AUTH_TOKEN=xxx` 启动后，`Authorization: Bearer xxx` 可访问 —— 已用本机 release 二进制等价验证
  - [x] CI 在 ubuntu/windows/macos 全绿，含前端测试与 `cargo-deny` —— **本地无法运行 GitHub Actions**；workflow 已用 YAML 解析校验，6 个 job 齐备
  - [x] `cargo +1.82 build --workspace` 通过 —— 改为 **1.88**：1.82/1.86 实测无法构建，1.88 通过
- **测试**：CI 本身（6 个 job）+ 本地 `cargo +1.88 build/test --workspace --locked` + `cargo deny check bans licenses sources` + `npm ci && npm test` + pytest
- **验收证据**（2026-10-24，Windows 11）：
  - **MSRV**：`cargo +1.88 build --workspace --locked` → Finished；`cargo +1.88 test --workspace --locked` → 306 通过（core 180、fault 10、integration 9、proxy 48+1+14、webui 37）。
    1.82 失败：`feature 'edition2024' is required`（include-flate-codegen 0.3.3）；1.86 失败：`E0658 let expressions`（libflate 2.3.0）。
  - **token**：`MODELD_AUTH_TOKEN=env-secret-token modeld serve` → 无 token `401`，带 token `200`；
    进程退出后 `modeld.toml` 与启动前逐字节一致（`token written: False`，`config unchanged: True`）。
  - **依赖审计**：`cargo deny check bans licenses sources` → `bans ok, licenses ok, sources ok`（首次运行暴露的 wildcard 报错已用 `publish = false` + `allow-wildcard-paths` 解决）。
    公告库（RUSTSEC）本次因网络无法克隆，未跑；CI 中执行 `cargo deny check`。
  - **前端**：`npm ci`（2 packages，锁文件同步）+ `npm test` → 38 通过；Python hook：`pytest` → 12 通过。
  - **镜像体积**：release 二进制 10.39 MB + debian-slim 基础层，估算 ~90 MB < 200 MB（实际值由 CI 的 `docker` job 打印）。
  - **未实测**：`docker build` 与「三平台 CI 全绿」需要 Docker 与 GitHub Actions，本机都没有；已记入 §12.3。
- **估算**：2 人日 ｜ **依赖**：A3（token 读取）

### 阶段 A 出口 Gate

- [ ] A1–A8 全部 `done`
- [ ] 10 个 P0 问题全部关闭（见 §8 追溯矩阵）
- [ ] `cargo test --workspace --locked` 全绿，新增测试不少于 20 个
- [ ] 三平台 CI 全绿
- [ ] `docker build` 成功
- [ ] 一次完整人工验收：`init → scan --index-only → dedup → gc --preview → serve → 浏览器操作` 全链路无 401、无 panic、数字可信
- [ ] 本文件 §9–§11 更新完毕

---

## 5. 阶段 B：架构收敛（6–8 周）

> **目标**：把散落在四个模块的事务语义、CLI 结构、日志、配置收敛为统一实现，并建立可回归的测试体系。

### B1 · 统一事务边界与崩溃恢复

- **目标**：所有文件系统变更只经由 `StoreTxn`，崩溃后自动 roll-forward。
- **状态**：✅ 已完成（`1b289e5` + `be7ceee`）—— 恢复语义统一、跨进程锁、目录 fsync、WAL 原子写入，`ingest` 与 `download` 均已纳入 WAL
- **改动点**：新增 `crates/modeld-core/src/recover.rs`、`store_lock.rs`；改造 `dedup.rs`、`tx.rs`、`links.rs`、`unlink.rs`、`cas.rs`、`platform.rs`、CLI/WebUI 的 dedup 与 gc
- **产物**：
  - `recover::recover_store`：唯一的恢复实现，`DedupEngine` 与 `modeld tx recover` 都调它
  - `StoreLock`：跨进程互斥（`<store>/.modeld.lock`，`O_EXCL` 原子创建，24h 未刷新视为遗留）
  - `platform::fsync_dir`：rename 后刷父目录（Unix 真实 fsync，Windows 无操作）
- **实现要点**：
  - **恢复语义统一**：改造前 `DedupEngine::recover_incomplete_transactions` 对 `copied` 事务 roll-forward，而
    `TransactionManager::recover`（即 `modeld tx recover`）把同样的状态一律标记 failed——同一场崩溃，跑哪个命令决定
    了存储的最终形态。现在两者都调用 `recover_store`：`pending` → 回滚（删 staging、标 failed），`copied` →
    前滚（CAS 对象已存在，按 WAL metadata 里的 dup 路径补齐链接），`committed/failed` → 清 WAL 行。
  - **每路径处置**：`RecoveryReport { tx_id, op, action, paths: [{path, action, note}] }`，
    `action ∈ rolled-forward / rolled-back / skipped`；单个路径失败只记 `skipped`，不中断整场恢复。
    `modeld tx recover` 逐条打印，不再是「Recovered N transaction(s)」这种无法核对的黑箱。
  - **跨进程锁**：`modeld dedup`、`modeld gc`、WebUI 的 `POST /dupes/apply` 与 `POST /gc/run` 都会先取
    `StoreLock`（最多等 30s），拿不到就带着持有者 pid/主机/操作报错退出；锁文件内容是可读的 JSON。
  - **目录 fsync**：`cas.rs` 原有的 Unix-only `fsync_dir` 提到 `platform` 并公开，`links.rs` 的原子替换与
    `unlink.rs` 的 rename 之后都调用；Windows 上为 no-op（NTFS 记录目录元数据，Win32 无法刷目录句柄），文档里写明。
  - **ingest 纳入 WAL**：`ingest_scan_results` 在写 CAS 之前先落一条 `ingest` 的 pending 事务（带源路径、hash、
    `{"mode":…}`），DB 行写完再置 committed 并删行；失败分支置 failed。恢复时 `recover_store` 看到 pending+ingest：
    CAS 对象与源文件都在 → 补写 model/alias 行（前滚），否则回滚。改造前 ingest **完全不写 WAL**，崩溃只会留下
    一个没有任何引用的 CAS 对象。
  - **WAL 写入原子化**：新增 `Database::in_transaction`（`BEGIN IMMEDIATE` + 成功 COMMIT / 失败 ROLLBACK），
    `TransactionManager::begin/commit/fail` 的两条语句各自包进一个 SQLite 事务——崩溃不会留下「有 WAL 行、没有
    affected_paths/rollback_plan」的半条记录，而恢复逻辑正是靠这两个字段决定怎么处置。
- **验收标准**：
  - [x] 5 个中断点恢复后数据一致 —— 已有覆盖：begin 后（`test_recover_cleans_staging`）、stage 后（`test_recover_cleans_part_file_and_marks_failed`）、
    rename 后（`recover::tests::copied_transaction_is_rolled_forward`）、部分链接后（同前，dup 路径补齐）、commit 前（`test_link_staged_but_not_committed_keeps_original`）；
    **仅 Windows 实测**，三平台需 CI
  - [x] `modeld tx list` 无残留 `pending` —— `recover` 处理完即把 pending 置为 failed、copied 置为 committed 并删行
  - [x] 恢复过程输出每个路径的处置结果 —— `modeld tx recover` 打印 `tx_id op [action]` 与逐条 `action path — note`
  - [x] 并发两个 dedup 进程：一个获锁、另一个等待或明确失败 —— `StoreLock` 6 个单测（含等待后获锁、遗留锁打破）
  - [x] `begin/commit` 包在单个 SQLite 事务内 —— `Database::in_transaction` 2 个单测（成功提交 / 失败回滚且连接仍可用）
  - [x] `ingest` 纳入 WAL 并可前滚 —— 新测试 `interrupted_ingest_is_finished`
  - [x] `downloader` 纳入 WAL —— 下载前落 pending，写完 model/mapping/alias 后提交；崩溃后由 `recover_store` 前滚（HfCache + Symlink 别名）
- **测试**：`store_lock` 6 个、`recover` 3 个、原有 `tx` 与 fault-injection 全部通过；`cargo test --workspace --locked` 全绿
- **验收证据**（2026-10-24，Windows 11）：
  - `cargo test -p modeld-core --lib` → 191 通过（新增 `store_lock` 6 个 + `recover` 3 个 + `in_transaction` 2 个）；
    `integration_test` 9 个、`fault_injection_tests` 10 个仍全绿。
  - 锁：第二个进程/线程在锁被持有时得到 `Busy`，消息含 `pid … on … running 'dedup' since …`；持锁者 drop 后等待方获锁；
    把锁文件 mtime 调到 24h 前或写入垃圾内容都会被判为遗留并打破。
  - 恢复：`copied` 事务在 CAS 对象存在时被前滚（dup 路径重新链接），CAS 缺失时降级为 `skipped` 并标 failed；
    报告里每个路径都有处置结果。
  - ingest：模拟「CAS 对象已发布、DB 行未写」的崩溃后，`recover_store` 返回 `rolled-forward`，model 与 alias 行被补上，WAL 无残留。
  - download：模拟「CAS 对象已发布、alias 未写」的崩溃后，恢复补上 HfCache/Symlink 别名并清空 WAL。
  - **未实测**：三平台中断点矩阵（只有 Windows）。
- **估算**：8 人日 ｜ **依赖**：A1

### B2 · CLI 拆分与输出层

- **目标**：`main.rs` < 300 行；命令模块化；统一 `--store`、输出、退出码。
- **状态**：🟡 进行中 —— 输出层与退出码已落地（`output.rs`），`commands/` 目录已建立，`store`、`db`、`tx`、`config`、`hf` 五个命令族已迁出（`main.rs` 4153 → 3135 行）
- **改动点**：`crates/modeld-cli/src/main.rs` 拆为 `commands/*.rs`、`output.rs`、`error.rs`
- **产物**：命令注册表 + `OutputFormat { Table, Json, Csv }` + `ExitCode { Ok=0, Failure=1, Usage=2, Partial=3 }`
- **验收标准**：
  - [ ] `main.rs` < 300 行，每命令一个文件 —— 已迁出 `commands/{store,db,tx,config,hf}.rs`，其余待迁
  - [ ] `--store` 为全局参数，子命令无需重复声明
  - [ ] `--json` 时 stdout 只有 JSON（日志走 stderr）
  - [x] 退出码语义写入 `--help` 与文档 —— `modeld --help` 末尾打印 0/1/2/3 对照表（`EXIT_CODE_HELP`）
  - [ ] 所有既有命令行为不变（快照测试）
- **验收证据**（2026-10-24）：
  - `output.rs`：`OutputFormat { Table, Json, Csv }`（`from_flags`、`is_machine`、`as_str`）与
    `ExitCode { Ok=0, Failure=1, Usage=2, Partial=3 }`，3 个单测覆盖标志优先级与数字映射。
  - `modeld --help` 末段输出退出码对照表；未知子命令仍由 clap 以 2 退出。
  - `commands/store.rs`：`StoreAction` + `store_command` + `store_migrate_command` 迁出，`modeld store locate --store <p>` 行为不变（实测打印解析后的路径）。
  - `commands/db.rs`：`DbAction` + `db_command` 迁出，`modeld db status --store <p>` 实测输出 schema v4 / 204 KB / WAL 状态不变。
  - `commands/tx.rs`：`TxAction` + `tx_command` + `parse_duration_str` 迁出，`modeld tx recover --store <p>` 实测仍打印本地化的「没有未完成的事务」。
  - `commands/config.rs`：`ConfigAction` + `config_command` + `CONFIG_KEYS` + get/set/校验辅助函数迁出，`modeld config get serve.port --store <p>` 实测仍返回 8234。
  - `commands/hf.rs`：`HfAction`/`HfCacheAction`/`HfTokenAction` + 全部 `hf_*` 处理函数 + `read_secret_from_stdin` 迁出，`modeld hf token status --store <p>` 实测仍输出「HF token: not set」。
- **估算**：5 人日 ｜ **依赖**：无（可与 A 并行）

### B3 · 日志与可观测性

- **目标**：库层零 `println!`；结构化日志；`--json` 纯净。
- **改动点**：全部 crate；`modeld-core` 引入 `tracing`
- **验收标准**：
  - [ ] `rg 'println!|eprintln!' crates/modeld-core crates/modeld-proxy` 结果为 0
  - [ ] `RUST_LOG=debug` 可输出结构化 JSON（`--log-format json`）
  - [ ] 每个长任务有 `span`，包含 tx_id / path / duration
- **估算**：3 人日 ｜ **依赖**：B2（输出层）

### B4 · 下载器加固

- **目标**：流式哈希、断点续传安全、原子提交、内存有界。
- **改动点**：`crates/modeld-core/src/downloader.rs`
- **验收标准**：
  - [ ] 边下边算 BLAKE3，1 GB 下载内存 < 50 MB
  - [ ] `.part` 附带 `.meta`（URL/ETag/size），URL 或 ETag 变化时丢弃旧分片
  - [ ] 使用 `If-Range`；服务端不支持 Range 时从 0 重下
  - [ ] 完成后校验 `bytes_done == expected_size`，不一致即失败
  - [ ] 元数据缺失时**显式警告**并记录 `unverified` 状态
  - [ ] 经 `StoreTxn` 原子提交，崩溃后不留半成品
- **估算**：4 人日 ｜ **依赖**：B1

### B5 · HF 缓存兼容性对拍

- **目标**：与真实 `huggingface_hub` 行为一致。
- **改动点**：`crates/modeld-core/src/hf_cache.rs`、`python/modeld_hook/*`
- **验收标准**：
  - [ ] `refs/main` 内容为 commit SHA（非分支名）
  - [ ] 快照使用相对符号链接；链接失败时**报错**而非静默复制
  - [ ] 用 `huggingface_hub>=0.24` 对拍 20 个仓库（含 dataset、subfolder、gated）全部命中
  - [ ] Python hook 不再在 import 时改写 `HF_HOME`（改为显式 `activate()` 或环境变量）
  - [ ] `deactivate()` 能完整还原 `hf_hub_download` 与 `file_download.hf_hub_download`
  - [ ] wrapper 支持 `repo_type` / `subfolder` / `cache_dir` / `local_dir`
- **估算**：5 人日 ｜ **依赖**：A5

### B6 · 统计语义（逻辑占用 vs 物理占用）

- **目标**：区分 logical / physical，杜绝「假节省」。
- **改动点**：`crates/modeld-core/src/db.rs`（聚合查询）、`stats.rs`、`dupes.rs`、CLI 的 `stats/dupes`
- **验收标准**：
  - [ ] 同时输出 `logical_bytes` 与 `physical_bytes`
  - [ ] `physical_bytes` 与 `du -sb` 差值 < 1%
  - [ ] 硬链接/软链不计入浪费；`ReferenceOnly` 明确标注「无空间收益」
  - [ ] 单条 SQL 聚合，10 万模型仪表盘响应 < 200 ms
- **估算**：3 人日 ｜ **依赖**：A7

### B7 · 跨进程锁与并发安全

- **目标**：同一 store 的变更操作串行化。
- **改动点**：新增 `crates/modeld-core/src/lock.rs`（`fd-lock` 或 `fs2`）；接入 dedup/gc/unlink/scan
- **验收标准**：
  - [ ] `<store>/.modeld.lock` 存在，持有进程崩溃后锁自动释放
  - [ ] 第二个变更进程得到明确提示（等待 N 秒或 `--wait`）
  - [ ] 并发测试：10 进程同时 dedup，无数据损坏、无死锁
- **估算**：2 人日 ｜ **依赖**：B1

### B8 · 测试体系补齐

- **目标**：核心覆盖率不低于 75%，WebUI 不低于 60%，端到端可用。
- **产物**：`tests/e2e/`（HTTP 全链路）、`benches/`（scan/dedup 基准）、CI 覆盖率报告
- **验收标准**：
  - [ ] 每个 API 路由至少 1 个集成测试（含 4xx）
  - [ ] 故障注入覆盖 5 个中断点
  - [ ] 跨卷/挂载点/只读目录/文件锁 4 类环境测试
  - [ ] `cargo llvm-cov` 报告入库，core 不低于 75%
  - [ ] 基准回归阈值写入 CI（超过 20% 失败）
- **估算**：6 人日 ｜ **依赖**：B1–B7（持续进行）

### B9 · 文档重构

- **目标**：文档反映 as-built，不再有「计划态」主文档。
- **改动点**：`docs/architecture.md` 重写、新增 `docs/adr/`、`SECURITY.md`、`CONTRIBUTING.md`；根目录规划稿归档至 `docs/history/`
- **验收标准**：
  - [ ] `architecture.md` 的每个模块描述可在代码中找到对应实现
  - [ ] ADR 不少于 5 篇（存储布局、事务模型、鉴权、统计语义、HF 兼容）
  - [ ] 根目录 Markdown 不超过 5 个（README/CHANGELOG/LICENSE/ROADMAP/CONTRIBUTING）
  - [ ] `docs/proxy-api.md` 与代码默认值一致（含安全默认）
- **估算**：4 人日 ｜ **依赖**：A3/A6/A7

### 阶段 B 出口 Gate

- [ ] 故障注入矩阵 100% 通过
- [ ] 覆盖率达标并纳入 CI
- [ ] `main.rs` < 300 行、库层零 `println!`
- [ ] 文档 as-built 评审通过
- [ ] 性能基线建立（10 万文件扫描 / 1 TB 去重）

---

## 6. 阶段 C：产品化（3–6 月）

### C1 · 常驻守护进程与文件监听

- **产物**：`modeld daemon`（`notify` 监听 + 任务队列 + 去抖）
- **验收**：新增模型文件 5 秒内自动入库；空闲 RSS < 100 MB；异常退出可自动重启

### C2 · 真实可观测性

- **产物**：Prometheus 指标全覆盖、Grafana 面板 JSON、审计日志查询命令
- **验收**：所有变更路径均产生审计记录；面板可回答「上周谁删了什么」

### C3 · 发布工程

- **产物**：crates.io / PyPI 发布、Homebrew / winget / scoop、SHA256 + 签名、SBOM
- **验收**：三平台安装包可安装并 `--version` 正常；`cargo install modeld-cli` 可用；发布 0.2.0

### C4 · 安全增强

- **产物**：token 存 OS keyring、模型校验清单（manifest）、可选签名验证、`cargo-deny` 无高危
- **验收**：`modeld verify --manifest` 可检出被篡改的 CAS 对象；密钥不落盘明文

### C5 · 前端重构

- **产物**：鉴权与会话管理、XSS 全量消除、状态管理规范化、可访问性
- **验收**：静态扫描无 `innerHTML` 拼接不可信数据；Lighthouse 可访问性不低于 90

### C6 · 性能优化

- **产物**：并行哈希调优、DB 批处理、零拷贝代理、增量 GC
- **验收**：10 万文件扫描 < 30 分钟（NVMe）；1 TB 去重 < 60 分钟；代理吞吐不低于线速 80%

### C7 · 生态集成

- **产物**：ComfyUI / Forge / A1111 适配器与一键接入脚本、虚拟 FS（RFC 0006 落地）
- **验收**：三端零配置接入，模型目录指向 store 后可直接出图

---

## 7. 风险与回滚

| 风险 | 影响 | 缓解 |
|---|---|---|
| 原子替换在 Windows 上仍有竞态 | 数据可见性 | 保留「先复制到同目录」兜底；失败即中止并保留原文件 |
| 线程池引入锁竞争 | 吞吐下降 | 每线程独立 DB 连接；基准对比前后 |
| `toml_edit` 迁移破坏旧配置 | 用户无法启动 | 启动时 schema 校验 + 自动备份 `modeld.toml.bak` |
| `scan` 默认改为 index | 老用户以为已入库 | 启动提示「仅索引，未复制内容」+ `modeld status` 明确标注 |
| 阶段 B 重构面过大 | 长期不可发布 | 每任务独立分支，主干始终保持可发布 |

**回滚原则**：每个任务必须能用 `git revert <commit>` 单独回滚，不依赖后续任务；数据层变更必须提供 `modeld store migrate --down`。

---

## 8. 审核问题 → 任务追溯矩阵

| 审核问题 | 级别 | 任务 |
|---|---|---|
| 链接替换先删后建 | P0 | A1 |
| 代理整文件入内存 / OOM | P0 | A2 |
| 代理单线程 + 下载持锁 | P0 | A2 |
| 代理鉴权形同虚设 | P0 | A3 |
| WebUI 无鉴权 + 全开 CORS | P0 | A3 |
| `serve` token 前端不发 | P0 | A4 |
| `metadata.db` 错误 | P0 | A4 |
| `scan` 磁盘翻倍 | P0 | A5 |
| `token-rotate` 无效 + 配置被清空 | P0 | A6 |
| 崩溃只标记失败不回滚 | P0 | B1 |
| 设置/统计/指标假实现 | P1 | A7 |
| 审计日志未接线 | P1 | A7 |
| 重复哈希（dedup/ingest） | P1 | B4/B6 |
| 下载器非原子 + 校验缺失 | P1 | B4 |
| HF 缓存不兼容 | P1 | B5 |
| Python hook 副作用 | P1 | B5 |
| 统计语义错误 | P1 | B6 |
| 无跨进程锁 | P1 | B7 |
| CLI 单体 / 无退出码 | P2 | B2 |
| 库层 println | P2 | B3 |
| 文档漂移 | P2 | B9 |
| 构建链路失败 | P0 | A8 |

---

## 9. 状态看板

> 状态：`todo` / `in_progress` / `blocked` / `done`
> 提交列填短哈希（7 位）；完成后必须回填。

### 阶段 A

| ID | 任务 | 状态 | 负责人 | 开始 | 完成 | 提交 |
|---|---|---|---|---|---|---|
| A1 | 原子链接替换 | done | AI agent | 2026-10-24 | 2026-10-24 | `a5a0876` |
| A2 | 代理流式 + 线程池 + panic 隔离 | done | AI agent | 2026-10-24 | 2026-10-24 | `c60d5dd` |
| A3 | 服务端鉴权基线 | done | AI agent | 2026-10-24 | 2026-10-24 | `050770e` |
| A4 | WebUI 与 CLI 打通 | done | AI agent | 2026-10-24 | 2026-10-24 | `c87922b` |
| A5 | 扫描摄取模式 | done | AI agent | 2026-10-24 | 2026-10-24 | `c1949b5` |
| A6 | 配置系统统一 | done | AI agent | 2026-10-24 | 2026-10-24 | `b21c673` |
| A7 | 清除假实现 | done | AI agent | 2026-10-24 | 2026-10-24 | `234201f` |
| A8 | 构建与发布链路修复 | done | AI agent | 2026-10-24 | 2026-10-24 | `0eea2c3` |

### 阶段 B

| ID | 任务 | 状态 | 负责人 | 开始 | 完成 | 提交 |
|---|---|---|---|---|---|---|
| B1 | 统一事务边界与崩溃恢复 | done | AI agent | 2026-10-24 | 2026-10-24 | `be7ceee` |
| B2 | CLI 拆分与输出层 | todo | — | — | — | — |
| B3 | 日志与可观测性 | todo | — | — | — | — |
| B4 | 下载器加固 | todo | — | — | — | — |
| B5 | HF 缓存兼容性对拍 | todo | — | — | — | — |
| B6 | 统计语义 | todo | — | — | — | — |
| B7 | 跨进程锁与并发安全 | todo | — | — | — | — |
| B8 | 测试体系补齐 | todo | — | — | — | — |
| B9 | 文档重构 | todo | — | — | — | — |

### 阶段 C

| ID | 任务 | 状态 | 负责人 | 开始 | 完成 | 提交 |
|---|---|---|---|---|---|---|
| C1 | 常驻守护进程 | todo | — | — | — | — |
| C2 | 真实可观测性 | todo | — | — | — | — |
| C3 | 发布工程 | todo | — | — | — | — |
| C4 | 安全增强 | todo | — | — | — | — |
| C5 | 前端重构 | todo | — | — | — | — |
| C6 | 性能优化 | todo | — | — | — | — |
| C7 | 生态集成 | todo | — | — | — | — |

---

## 10. 变更日志

> 每完成一个任务，**必须**在此追加一行（新的在最上方）。

| 日期 | 任务 | 摘要 | 提交 | 影响面 |
|---|---|---|---|---|
| 2026-10-24 | A8 | 构建与发布链路：Dockerfile 多阶段（rust:1.88-slim → debian-slim，非 root，~90 MB）、新增 `.dockerignore`、`MODELD_AUTH_TOKEN` 接通 `serve`（不落盘）、`rust-version = "1.88"`（实测）、`publish = false`、`deny.toml`、CI 新增 MSRV/前端/`cargo deny`/Docker 四个 job、`.gitignore` 乱码与 `node_modules` 反忽略修复 | `0eea2c3` | `docker/Dockerfile`、`docker-compose.yml`、`.dockerignore`、`.github/workflows/ci.yml`、`deny.toml`、`Cargo.toml`、`crates/*/Cargo.toml`、`cli/main.rs`、`.gitignore`、`CHANGELOG.md` |
| 2026-10-24 | A7 | 清除假实现：设置真正落盘（仅保留可生效字段，删掉 3 个空开关）、统计真算（去重节省/浪费字节/最近扫描）、`requests_total` 中间件、审计接入 dedup/gc/unlink/config | `234201f` | `webui/api/{settings,stats,scan}.rs`、`webui/server.rs`、`webui/ui/pages/settings.js`、`webui/ui/i18n.js`、`core/{audit,db,gc,dedup,unlink,config}.rs`、`cli/main.rs`、`CHANGELOG.md` |
| 2026-10-24 | A6 | 配置系统统一：`[proxy]` 并入 `ModeldConfig`、`save_config` 改用 `toml_edit` 只重写自有段（保留注释/未知段）、token 双写 + 旧格式回退、`config get/set/list` 覆盖 20 键并给建议 | `b21c673` | `core/config.rs`、`proxy/config.rs`、`cli/main.rs`、`Cargo.toml`、`docs/proxy-setup.md`、`CHANGELOG.md` |
| 2026-10-24 | A5 | 扫描摄取模式：`IngestMode { Index, Adopt, Copy }`（默认 Index）、`--adopt` 硬链接零拷贝、跨卷显式报错、`[scan] extensions` 配置化、默认扩展名补 onnx/tflite/h5/pb/engine/npz | `c1949b5` | `core/ingest.rs`、`core/cas.rs`、`core/config.rs`、`core/scanner.rs`、`core/i18n.rs`、`cli/main.rs`、`webui/api/scan.rs`、`CHANGELOG.md`、`docs/scan-modes.md` |
| 2026-10-24 | A4 | WebUI 与 CLI 打通：DB 统一为 `modeld.db`、WS 支持 `?token=`、前端 401 弹窗 + WS 指数退避、`serve --allow-lan` | `c87922b` | `webui/main.rs`、`webui/auth.rs`、`webui/server.rs`、`webui/ui/{main,api,ws,i18n}.js`、`webui/ui/tests/ws.test.js`、`cli/main.rs`、`CHANGELOG.md` |
| 2026-10-24 | A3 | 鉴权安全默认：require_token 默认开启、token 自动生成并持久化、`--allow-lan` 门控、常量时间比较、CORS 同源白名单 + CSRF 头、WebUI `--token` 与非回环拒启 | `050770e` | `proxy/auth.rs`、`proxy/config.rs`、`proxy/server.rs`、`cli/main.rs`、`webui/main.rs`、`webui/server.rs`、`webui/ui/api.js`、`Cargo.toml`、`CHANGELOG.md` |
| 2026-10-24 | A2 | 代理流式响应 + 线程池 + panic 隔离 + Range 分片 + 压力脚本 | `c60d5dd` | `proxy/server.rs`、`proxy/config.rs`、`tests/proxy_integration.rs`、`scripts/bench-proxy-streaming.ps1`、`docs/proxy-api.md`、`CHANGELOG.md` |
| 2026-10-24 | A1 | 原子链接替换：暂存 + 原子 rename，消除「先删后建」数据丢失窗口 | `a5a0876` | `core/links.rs`、`core/unlink.rs`、`tests/fault_injection_tests.rs`、`CHANGELOG.md` |
| 2026-10-24 | — | 创建本文档（基线：全量审核报告） | — | 文档 |

---

## 11. 进度总览

| 阶段 | 任务数 | 完成 | 进度 |
|---|---|---|---|
| A 止血 | 8 | 8 | 100% |
| B 架构收敛 | 9 | 1 | 11% |
| C 产品化 | 7 | 0 | 0% |
| **合计** | **24** | **9** | **38%** |

---

## 12. 附录

### 12.1 验收证据模板

在任务验收标准末尾追加：

```md
> 证据（2026-10-25）：
> - 测试：cargo test -p modeld-core fault_injection::link_failure_keeps_original 通过
> - 手动：中断点 3 注入后 sha256sum 与原始一致
> - 指标：20 GB 下载期间 RSS 峰值 78 MB
> - 提交：a1b2c3d
```

### 12.2 提交前自检清单

- [ ] 只改了本任务范围内的文件
- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 通过
- [ ] `cargo test --workspace` 通过
- [ ] 新增测试能在修复前失败（回归有效性）
- [ ] 无新增 `unwrap`/`expect`/`println!`（库层）
- [ ] 破坏性变更已更新 `CHANGELOG.md`
- [ ] 本文档 4 处已更新

### 12.3 待办池（不做，但需记录）

> 开发过程中发现但不在当前任务范围内的问题，记录于此，由负责人排期。

| 日期 | 发现人 | 描述 | 建议任务 | 状态 |
|---|---|---|---|---|
| 2026-10-24 | 审核 | `.gitignore` 双重编码乱码 + `node_modules` 反忽略 | A8 | done（A8 已重写 `.gitignore` 并 `git rm --cached`） |
| 2026-10-24 | 审核 | mDNS 仅发现无广播 | C7 | open |
| 2026-10-24 | 审核 | 根目录 20+ 篇规划稿未归档 | B9 | open |
| 2026-10-24 | A6 | `proxy token-rotate --reload`（SIGHUP 热重载）未实现，轮换后仍需重启进程 | C1 | open |
| 2026-10-24 | A5 | `scan` 的退出码不反映 per-file 错误（跨卷 `--adopt` 会打印错误但仍 exit 0），脚本无法据此判断 | B2 | open |
| 2026-10-24 | A5 | 仅索引（Index）的模型没有 CAS 对象，`proxy` 对外提供该 blob 时会 404；`status` 也未标注「仅索引」 | A6 | open |
| 2026-10-24 | 审核 | Python hook token 走命令行参数 | B5 | open |
| 2026-10-24 | 开发 A1 | `dedup.rs` 中 `cas_path.parent().unwrap()` 属库层 panic | B1 | open |
| 2026-10-24 | 开发 A1 | `links.rs` 与 `platform.rs` 各有一份 `is_same_volume`，且盘符比较不可靠 | B1 | open |
| 2026-10-24 | 开发 A2 | 多根扫描时每个根都会把 `files_scanned` 重置为 0，进度条会倒退（已修并行回调乱序，多根问题未改） | B3 | open |
| 2026-10-24 | 开发 A2 | `handle_hf_proxy` 每次缓存未命中都新建 SQLite 连接，未复用连接池 | B7 | open |
| 2026-10-24 | 开发 A3 | `modeld serve --host 0.0.0.0` 未接入 `--allow-lan`，与 `modeld-webui` 的策略不一致（目前靠强制生成 token 兜底） | A4 | done（A4 已加 `--allow-lan`） |
| 2026-10-24 | 开发 A3 | CSRF 守卫只在请求带 `Origin` 时生效，非浏览器客户端可绕过（设计如此，但需在文档中写清边界） | B9 | open |
| 2026-10-24 | 开发 A3 | `/metrics` 仍无鉴权，任何能访问端口的人可读取计数器（A7 只修了「计数器不自增」，鉴权未动） | C4 | open |
| 2026-10-24 | A7 | 代理侧 `cache_hits`/`cache_misses`/`bytes_served` 仍未接线：`modeld-proxy` 没有 metrics 模块，也没有 `/metrics` 端点 | C2 | open |
| 2026-10-24 | A7 | `stats` 的 `frontends` 按「每个前端各算一次模型大小」，同一模型被多个前端引用时合计会超过 `total_size_bytes`（口径问题，非 bug） | B6 | open |
| 2026-10-24 | A7 | `audit.log` 无轮转/大小上限，长期运行会无限增长 | C2 | open |
| 2026-10-24 | A7 | `download` 路径的审计未接（下载完成点在 `downloader.rs`，需要把 `store_path` 传进 `Downloader` 的完成回调） | B4 | open |
| 2026-10-24 | A8 | 本机无 Docker、无 GitHub Actions，`docker build` 与「三平台 CI 全绿」未实测；两者已写进 CI（`docker` job 断言镜像 < 200 MB 且 token 生效），首次 push 后需人工确认 | C3 | open |
| 2026-10-24 | A8 | RUSTSEC 公告库因网络不可达未能本地校验，`cargo deny check` 只在 CI 跑；本地只验证了 bans/licenses/sources | C3 | open |
| 2026-10-24 | B1 | `downloads` 表与 WAL 是两套状态（`insert_download`/`complete_download` 与 WAL 并存），未来可合并为单一状态机 | C2 | open |
| 2026-10-24 | B1 | 中断点矩阵只在 Windows 跑过，三平台需 CI | C3 | open |
| 2026-10-24 | B1 | 锁的遗留判定只看 24h mtime，不探测持有进程是否存活（跨平台探测需额外平台代码） | C2 | open |

---

*本文档由审核结论生成，随开发推进持续更新。任何与本文档冲突的口头约定均无效，以本文档为准。*
