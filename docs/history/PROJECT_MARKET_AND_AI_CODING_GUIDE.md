# modeld 项目市场与技术审计报告

生成日期：2026-06-30  
项目路径：`D:\AI\my\githubprojs\ModelID`  
项目定位：面向本地/局域网 AI 模型文件的内容寻址存储、去重、Hugging Face 缓存、代理、工作流引用图和 Web 管理工具。

---

## 1. 结论摘要

modeld 解决的是一个真实、正在变大的需求：本地 AI 创作者、开源模型玩家、研究团队和小型 AI 团队正在管理越来越多的大模型文件，重复下载、重复存储、跨工具缓存割裂、模型来源不可追踪、误删风险和局域网重复拉取都是真痛点。

这个项目的技术方向是对的，核心比普通“模型下载器”更有价值：它不是单纯管理一个目录，而是试图成为 AI 模型文件的本地基础设施层，类似“CAS + git-lfs + Hugging Face cache + ComfyUI/A1111 模型治理”。如果做稳，开源声誉空间不错；商业上适合走“免费开源核心 + Pro 桌面版/团队版/企业版”的路线。

但当前还不能按 README 里的“v1.0 / all complete / shipping”对外宣传。代码通过了 `cargo test`，底层模块进展明显，但仍存在会影响用户数据可信度和第一印象的 P0/P1 问题：

- WebUI 扫描只写数据库和 alias，看起来没有把模型对象写入 CAS。
- WebUI 前端 API 与后端路由不一致，部分按钮会直接 405/404 或拿不到预期字段。
- WebSocket 事件结构前后端不一致，扫描进度 UI 不能正确展示完成状态。
- `modeld proxy start` 现在变成 `modeld serve` 的兼容别名，但忽略 `--token`、`--allow-anonymous`、`--config`，且没有启动原来的 `modeld-proxy` HF 代理能力。
- CAS 存储存在新旧两套路径：安全的 `store_crash_safe()` 已实现，但 CLI scan、downloader、测试和 WebUI 路径仍大量调用旧 `store()`。
- README、AUDIT、ARCHITECTURE_CURRENT 与当前代码不同步，商业宣传风险较高。

建议：先不要继续堆新功能。下一阶段目标应是“把当前承诺做实”：修复数据正确性、WebUI 可用性、代理兼容性、文档真实性，再进入打包发布和商业化验证。

---

## 2. 外部市场判断

### 2.1 需求是否真实存在

需求真实存在，并且有三类用户会自然遇到：

1. 本地 AIGC 用户
   - ComfyUI、A1111、Forge、Fooocus、InvokeAI 等工具都会产生多套模型目录。
   - 常见模型包括 checkpoint、LoRA、VAE、ControlNet、Upscaler、GGUF、diffusers snapshot。
   - 一个用户很容易积累数百 GB 到数 TB 模型文件，重复 checkpoint/LoRA 复制非常常见。

2. 小团队/工作室
   - 多台机器共享同一批模型。
   - 每台机器从 Hugging Face/Civitai/内部网盘重复下载，浪费带宽、时间和硬盘。
   - 模型来源、版本、license、使用中的 workflow 依赖缺乏可追踪治理。

3. 企业/研究团队
   - 模型越来越大，合规要求越来越高。
   - 企业需要知道“这个模型来自哪里、被哪些项目用、是否可以删、是否含私有 token、是否能离线复现”。
   - 他们通常已有 MLflow/DVC/LakeFS/Artifact Registry，但这些工具并不专门解决“本地模型文件跨前端去重和 HF cache 兼容”。

公开信号：

- Hugging Face Models 页面在 2026-06-30 显示约 2,869,710 个模型，且覆盖 Transformers、Diffusers、GGUF、Safetensors、llama.cpp、Ollama、LM Studio 等生态。来源：[Hugging Face Models](https://huggingface.co/models)。
- ComfyUI GitHub 页面显示约 119k stars，并声明支持本地 Windows/Linux/macOS、云端、图像/视频/音频/3D 等多类模型。来源：[Comfy-Org/ComfyUI](https://github.com/Comfy-Org/ComfyUI)。
- AUTOMATIC1111 stable-diffusion-webui GitHub 页面显示约 164k stars。来源：[AUTOMATIC1111/stable-diffusion-webui](https://github.com/AUTOMATIC1111/stable-diffusion-webui)。
- Civitai 已经是面向生成式 AI 内容和模型的平台，公开资料显示其 2024 年已有千万级月访问，并承载大量可下载模型。来源：[Civitai 概览](https://en.wikipedia.org/wiki/Civitai)。
- MLOps 市场公开资料称 2024 年约 21.918 亿美元，2030 年预计约 166.134 亿美元。来源：[MLOps 概览](https://en.wikipedia.org/wiki/MLOps)。

### 2.2 市场规模估算

modeld 不应把 TAM 写成整个 MLOps 市场。更合理的是分层估算：

| 层级 | 用户范围 | 需求强度 | 可付费性 | 估算 |
|---|---:|---:|---:|---|
| 开源创作者/本地玩家 | 数百万级潜在用户，活跃付费意愿较弱 | 高 | 低到中 | 适合做免费开源入口 |
| 独立创作者/小工作室 | 数万到数十万级 | 很高 | 中 | 适合 Pro/Team 订阅或一次性授权 |
| AI 工具链团队/实验室 | 数千到数万级组织 | 高 | 中到高 | 适合团队版、私有 registry、支持服务 |
| 企业 AI 平台 | 数千级 | 中到高 | 高 | 需要合规、审计、SSO、权限、SLA |

保守可服务市场 SAM：

- 个人 Pro：假设全球 5-20 万高强度本地模型用户，1%-5% 付费，年费 29-99 美元，则年收入约 1.5 万到 99 万美元。
- 小团队版：假设 1,000-10,000 个创作/研究团队，2%-10% 转化，每团队每年 300-2,000 美元，则年收入约 0.6 万到 200 万美元。
- 企业/私有部署：假设 20-200 个高价值客户，每年 2,000-20,000 美元，则年收入约 4 万到 400 万美元。

现实判断：如果只是 CLI 去重工具，收益空间有限；如果做成稳定的“模型资产管理 + 局域网缓存 + WebUI + 合规元数据 + 私有部署”，有小而真实的商业空间。成功路径更像 Syncthing、DVC、Tailscale、Docker Desktop 的组合启发：开源核心建立信任，桌面体验和团队治理收钱。

### 2.3 竞品与替代方案

| 类型 | 代表 | 与 modeld 的关系 |
|---|---|---|
| 通用 artifact/model registry | MLflow Model Registry、DVC、Weights & Biases Artifacts、S3/MinIO | 更偏训练/实验/云端，不专注 ComfyUI/A1111/HF 本地目录兼容 |
| HF 原生 cache | Hugging Face hub cache | 生态标准，但跨前端、跨机器、去重治理、可视化、GC 保护不足 |
| 本地模型工具 | ComfyUI Manager、A1111 Extensions、Ollama、LM Studio | 管自己的生态，对跨工具模型文件治理不足 |
| 文件级去重 | hardlink/symlink 工具、rmlint、duperemove、btrfs/zfs dedup | 不懂模型来源、HF 语义、workflow refs、模型元数据 |
| 私有镜像/代理 | nginx cache、Artifactory、Nexus、HF mirror 脚本 | 偏网络缓存，不解决本地模型目录和 CAS 治理 |

modeld 的差异化必须明确：

- 不是“又一个模型下载器”，而是“本地模型资产层”。
- 不是只为 Hugging Face，而是兼容 ComfyUI/A1111/Forge/InvokeAI/Ollama/LM Studio 这类实际落地工具。
- 不是简单删除重复文件，而是有工作流引用图、隔离区、恢复、审计、token 安全、license/provenance。

---

## 3. 当前项目技术状态

### 3.1 已经做得不错的部分

从代码和测试看，项目已有真实工程量：

- Rust workspace 拆分合理：`modeld-core`、`modeld-cli`、`modeld-proxy`、`modeld-client`、`modeld-webui`。
- 核心 CAS、BLAKE3、SQLite、scanner、dedup、quarantine、GC、HF cache、workflow parser、doctor、config、tx、governance、refs 都已有实现。
- 测试数量较多，`cargo test` 当前通过：
  - `modeld-core` 单元测试 154 个通过。
  - fault injection 测试 6 个通过。
  - core integration 测试 9 个通过。
  - proxy 单元/集成测试通过。
  - webui 目前主要是 metrics 测试，缺少 API 和浏览器级测试。
- 安全方向已有意识：quarantine、token 不日志化、doctor、audit log、verify/repair、transaction manager、crash-safe CAS 函数都已经出现。

### 3.2 文档状态问题

当前文档不同步：

- `README.md` 宣称 `v1.0`、所有阶段 complete。
- `Cargo.toml` workspace 版本是 `0.1.0`。
- `AUDIT.md` 标记很多功能为 stub，但当前代码中这些命令和模块已经存在。
- `ARCHITECTURE_CURRENT.md` 也有陈旧描述，例如提到无 `tx.rs`/`store_path.rs`/`config.rs`/`doctor.rs`/`governance.rs`/`platform.rs`，但当前这些文件存在。

这会直接影响用户信任。建议在修复 P0 后重写 README 和审计文档，使用“Beta / Developer Preview”而不是 v1.0。

---

## 4. 关键 bug 与功能缺失

### P0：必须先修

#### 4.1 WebUI scan 没有写入 CAS

位置：`crates/modeld-webui/src/api/scan.rs`

问题：

- API scan 在后台任务中扫描文件后，只执行：
  - `db.insert_or_update_model`
  - `db.insert_alias`
  - `db.upsert_path_index`
- 没有执行 `CasStore::store()` 或 `CasStore::store_crash_safe()`。

影响：

- WebUI 扫描后 DB 认为模型存在，但 CAS 对象不存在。
- `/api/v1/models` 显示数据，`verify` 可能报 missing CAS。
- proxy blob 下载和 GC 可能基于错误状态运行。
- 用户可能以为已完成入库，实际没有去重/缓存保障。

修复要求：

- WebUI scan 必须与 CLI scan 共享同一套 scan ingestion 逻辑。
- 抽出核心函数，例如 `modeld_core::workflow::ingest_scan_results` 或 `modeld_core::scanner::ingest_files`。
- 对每个扫描结果调用 `CasStore::store_crash_safe(&file.path, &file.hash, scan_id)`。
- 写入 DB 前后保证 CAS 与 DB 一致。
- 增加集成测试：WebUI scan 后，`cas.path_for_hash(hash).exists()` 必须为 true。

#### 4.2 WebUI API 路由和前端请求不一致

位置：

- `crates/modeld-webui/src/api/mod.rs`
- `crates/modeld-webui/ui/api.js`
- `crates/modeld-webui/ui/pages/dashboard.js`

问题：

- 后端路由：`GET /api/v1/gc/preview`
- 前端调用：`POST /api/v1/gc/preview`
- 后端路由：`POST /api/v1/dedup/preview`、`POST /api/v1/dedup/apply`
- 前端仍暴露旧方法：`POST /api/v1/dupes/dedup`
- 前端 dashboard 期望 `preview.reclaimable`，后端返回 `would_quarantine`。

影响：

- GC 预览按钮可能失败。
- 去重按钮/页面可能使用旧接口失败。
- 用户会在第一轮体验中遇到无响应或错误 toast。

修复要求：

- 统一 API contract，优先保留新 canonical routes：
  - `POST /api/v1/dedup/preview`
  - `POST /api/v1/dedup/apply`
  - `GET /api/v1/gc/preview`
  - `POST /api/v1/gc/run`
- 更新 `ui/api.js`：
  - `gcPreview: () => request('GET', '/gc/preview')`
  - `dedupPreview: body => request('POST', '/dedup/preview', body || {})`
  - `dedupApply: body => request('POST', '/dedup/apply', body || {})`
- 更新 dashboard 使用 `preview.would_quarantine`。
- 保留旧 route 时也要加兼容测试，避免已有 UI 页面断掉。

#### 4.3 WebSocket 扫描事件前后端字段不一致

位置：

- `crates/modeld-webui/src/state.rs`
- `crates/modeld-webui/src/api/scan.rs`
- `crates/modeld-webui/ui/pages/dashboard.js`

问题：

- 后端 `WsEvent::ScanProgress` 字段是 `path/done/total`。
- 前端 `dashboard.js` 读取 `payload.phase/current_path/files_scanned/files_total`。
- 前端用 `p.phase === 'done'` 判断完成，但后端不会发送 `phase`。

影响：

- 进度条显示不准，按钮可能不会恢复，完成 toast 不触发。

修复要求：

- 将后端事件结构升级为：
  - `scan_id`
  - `phase`: `walking | hashing | indexing | done | error`
  - `current_path`
  - `files_scanned`
  - `files_total`
  - `bytes_scanned`
  - `bytes_total`
- 或者改前端使用现有 `path/done/total` 并监听 `operation_complete`。
- 推荐统一使用更丰富结构，并增加 JSON contract 测试。

#### 4.4 `modeld proxy start` 兼容行为被破坏

位置：`crates/modeld-cli/src/main.rs`

问题：

- `proxy_command` 中 `ProxyAction::Start` 直接调用 `serve_command`。
- `token`、`allow_anonymous`、`config` 参数被忽略，编译警告也提示 `unused variable: token`。
- 原先的 `proxy_start_command` 仍存在但不再使用。
- `serve_command` 启动的是 WebUI，不等价于 `modeld-proxy::ProxyServer` 的 HF proxy/blob proxy。

影响：

- README 中的 `modeld proxy start --token ... --no-allow-anonymous` 行为不可靠。
- 用户以为启动了 Hugging Face mirror，实际可能只启动 WebUI。
- 企业/局域网缓存这个核心卖点受损。

修复要求：

- 恢复 `modeld proxy start` 调用 `proxy_start_command`。
- 新增 `modeld serve` 作为统一入口，但必须显式说明：
  - 是否同时启动 WebUI API
  - 是否同时启动 HF proxy
  - 是否兼容 `/v1/hf-proxy`
- 如果要合并 server，需要把 `modeld-proxy` 路由迁移到 axum 或在统一入口中并行启动两个服务。
- 增加 CLI 集成测试：`proxy start --token abc` 后无 token 请求应 401，带 token 应通过。

#### 4.5 安全 CAS 写入函数没有被主路径使用

位置：

- `crates/modeld-core/src/cas.rs`
- `crates/modeld-cli/src/main.rs`
- `crates/modeld-core/src/downloader.rs`
- `crates/modeld-webui/src/api/scan.rs`

问题：

- `CasStore::store_crash_safe()` 已实现 fsync、hash 校验、staging、原子 rename/fallback。
- 但主路径仍大量调用旧 `CasStore::store()`：
  - CLI scan
  - downloader
  - 部分测试/工具路径
- downloader 先复制到 staging，再调用旧 `store()`，仍缺 crash-safe 事务整合。

影响：

- 断电/崩溃时仍可能留下半写 CAS 或 DB/CAS 不一致。
- 代码中“安全能力已实现”但用户实际路径未必安全。

修复要求：

- 让 `store()` 内部调用 `store_crash_safe()`，或将旧 `store()` 标记为测试/legacy only。
- CLI scan 使用 scan_id/tx_id 传入 `store_crash_safe()`。
- downloader 使用 `store_crash_safe(&tmp_file, &blake3, tx_id)`，不要手工复制一遍 staging。
- 修复后补 fault injection 测试覆盖 CLI scan 和 downloader 主路径。

### P1：影响可用性和增长

#### 4.6 WebUI 无浏览器级测试

当前 `modeld-webui` 只有 metrics 单元测试，没有覆盖：

- 页面能否加载。
- Dashboard 能否成功请求 stats。
- Scan/GC/Dedup 按钮是否命中正确路由。
- 鉴权 token 时前端是否能提供 token。
- WebSocket 事件是否能驱动 UI。

要求：

- 增加 Playwright 或轻量浏览器测试。
- 使用临时 store 启动 `modeld-webui`，访问页面，执行 scan dry path，检查 API 调用状态。
- 至少覆盖 desktop viewport 和窄屏 viewport。

#### 4.7 前端 token 支持缺失

后端 WebUI API 已支持 bearer token，但 `ui/api.js` 没有从 localStorage/sessionStorage/URL bootstrap 读取 token 并加 Authorization header。

影响：

- `modeld serve` 自动生成 token 后，浏览器页面如果直接打开，会访问 API 失败。
- 用户不知道在哪里输入 token。

要求：

- 首次访问若 API 401，显示 token 输入页/弹窗。
- token 存在 sessionStorage 或 OS keychain 桥接，不写入日志。
- 所有 fetch 和 WebSocket 都携带 token；WebSocket 可用 `?token=` 或 subprotocol，但要避免日志泄漏。

#### 4.8 下载器缺少完整 snapshot 语义

当前已有 HF 单文件下载，但真实用户常需要：

- 下载整个 repo snapshot。
- 选择 allow/ignore patterns。
- 处理 gated/private repo 错误提示。
- license/model card/provenance 自动写入。

要求：

- 实现 `modeld hf snapshot <repo_id> --revision --allow "*.safetensors" --ignore "*.bin"`。
- 对 401/403 gated repo 提供明确提示。
- token 优先级：CLI 参数 > env `HF_TOKEN` > modeld token store。
- provenance 写入 DB，并在 WebUI Library 展示。

#### 4.9 缺少 first-run 引导

新用户需要的是“我有 ComfyUI/A1111/Ollama/LM Studio，帮我找模型并节省空间”，不是先理解 CAS。

要求：

- `modeld doctor` 输出下一步动作。
- `modeld init --wizard` 检测常见路径：
  - ComfyUI models
  - A1111 models
  - Forge
  - InvokeAI
  - Ollama
  - LM Studio
  - Hugging Face cache
- WebUI 首屏提供路径选择和 dry-run report。

#### 4.10 发布包与安装体验不足

要求：

- GitHub Releases 提供 Windows/macOS/Linux 二进制。
- Windows 提供 `.msi` 或 zip，macOS 提供 notarized 包或 brew tap，Linux 提供 deb/rpm/tar.gz。
- `modeld --version` 输出 version、git hash、build target。
- README 第一屏给出 3 分钟可验证 demo。

### P2：商业化增强

#### 4.11 license/provenance 治理

要求：

- 下载时抓取 Hugging Face model card metadata。
- 记录 license、source URL、revision、sha256、blake3、downloaded_at。
- 导出 CSV/JSON license report。
- WebUI 支持按 license/source/filter。

#### 4.12 团队版功能

要求：

- 多用户只读/读写权限。
- SSO/OIDC。
- 审计日志下载。
- 配额、限速、缓存命中率统计。
- 私有 registry mirror。
- Docker Compose 一键部署。

#### 4.13 与生态深度集成

要求：

- ComfyUI custom node：展示缺失模型、自动下载、引用图、路径迁移。
- A1111/Forge extension。
- Ollama/llama.cpp/GGUF 目录兼容。
- VS Code/Cursor 插件用于团队模型资产查看。

---

## 5. 推荐产品定位

### 5.1 一句话定位

modeld 是面向本地和团队 AI 开发者的模型资产管理层：自动扫描、去重、缓存、追踪来源、保护工作流依赖，并在局域网中共享模型下载。

### 5.2 不建议的定位

不要定位成：

- “AI 模型网盘”
- “又一个 Hugging Face 下载器”
- “普通重复文件清理工具”
- “企业 MLOps 平台替代品”

这些定位要么太泛，要么和强竞品正面冲突。

### 5.3 最小可卖 MVP

必须具备：

- Windows/macOS/Linux 一键安装。
- WebUI 首屏扫描常见模型目录。
- Dry-run 显示可节省空间。
- 一键去重，默认安全隔离，可恢复。
- Hugging Face cache 兼容。
- ComfyUI/A1111/Forge 路径识别。
- verify/repair/doctor 可自救。
- 局域网只读代理，可 token 保护。

---

## 6. 收益模式建议

### 6.1 开源核心

免费 MIT 保留：

- CLI scan/dedup/verify/repair。
- CAS store。
- 单机 WebUI。
- HF cache 基础能力。
- 基础 workflow refs。

目的：获取信任、GitHub stars、社区贡献、生态插件。

### 6.2 Pro 个人版

收费 29-99 美元/年或一次性 49-149 美元：

- 桌面 GUI。
- 自动发现 ComfyUI/A1111/Ollama/LM Studio。
- 可视化迁移助手。
- 高级 license/provenance 报告。
- 自动监控模型目录。
- 更好的 Windows 硬链接/权限引导。

### 6.3 Team 版

收费 10-20 美元/seat/月或团队年费：

- 局域网共享模型 registry。
- 用户权限。
- 审计日志。
- 团队标签/收藏/备注。
- 私有模型 pin/retention policy。
- 缓存命中率与带宽节省统计。

### 6.4 Enterprise

年费 5k-50k 美元起：

- SSO/OIDC。
- 私有部署支持。
- SLA/支持。
- 合规报告。
- 代理/mirror 高可用。
- 与 MLflow/S3/Artifactory/LDAP 集成。

---

## 7. AI coding 工具执行指南

下面内容可以直接交给 AI coding 工具分批实施。

### 7.1 总体原则

1. 不新增大功能，先修复 P0 数据正确性。
2. 所有危险文件操作必须有 dry-run、quarantine、verify、repair。
3. CLI 与 WebUI 必须共享核心逻辑，避免两套 scan/dedup/GC 行为分叉。
4. 每个 API route 必须有前后端契约测试。
5. README 必须反映真实状态，不能提前宣布 v1.0。

### 7.2 Wave 1：修复 WebUI 数据正确性

目标：WebUI scan 与 CLI scan 产生同样一致的 CAS + DB 状态。

任务：

- 在 `modeld-core` 新增共享 ingestion 函数：
  - 输入：store_path、Database、Vec<ScannedFile>、scan_id。
  - 行为：init CAS/quarantine、store_crash_safe、insert model、insert alias、upsert path index。
  - 输出：processed count、bytes processed、errors。
- CLI `scan_command` 改用共享 ingestion。
- WebUI `trigger_scan` 改用共享 ingestion。
- 增加测试：
  - CLI scan 后 CAS 存在。
  - WebUI scan handler 后 CAS 存在。
  - 故意 wrong hash 时不写 DB。

验收：

```bash
cargo test -p modeld-core
cargo test -p modeld-webui
cargo test
```

### 7.3 Wave 2：修复 WebUI API contract

目标：所有前端按钮命中正确接口，返回字段一致。

任务：

- 更新 `crates/modeld-webui/ui/api.js`：
  - `gcPreview` 使用 GET。
  - 增加 `dedupPreview` 和 `dedupApply`。
  - 删除或保留兼容旧 `dedup`，但页面必须用新接口。
- 更新 `dashboard.js`：
  - 使用 `preview.would_quarantine`。
- 检查 `dupes.js`、`refs.js`、`settings.js`、`proxy.js` 是否还有旧字段。
- 为 API 返回结构写 Rust snapshot/serde 测试或 JS contract 测试。

验收：

```bash
cargo test -p modeld-webui
```

并启动 WebUI 手动点击：

- Dashboard stats
- Scan
- GC preview/run
- Duplicates preview/apply
- Library detail

### 7.4 Wave 3：统一 WebSocket 事件

目标：扫描进度、去重进度、下载进度在 UI 中可靠显示。

任务：

- 修改 `WsEvent::ScanProgress` 为具名完整结构。
- 后端 scan 发送 walking/hashing/indexing/done/error。
- 前端只解析一种结构。
- 操作完成时发送 `OperationComplete`，前端恢复按钮状态。
- 增加序列化测试，确保 JSON 字段名稳定。

验收：

- UI 扫描中进度条递增。
- 扫描完成按钮恢复。
- 断开 WebSocket 后能重连。

### 7.5 Wave 4：恢复 proxy start 语义

目标：`modeld proxy start` 必须仍是 HF/blob proxy，而不是悄悄变成 WebUI。

任务：

- `proxy_command(ProxyAction::Start)` 调回 `proxy_start_command`。
- 确认 `--token`、`--allow-anonymous`、`--config` 生效。
- `modeld serve` 保持 WebUI；如果要统一入口，另开任务迁移 proxy routes 到 axum。
- README 区分：
  - `modeld serve`: WebUI + API。
  - `modeld proxy start`: HF/blob LAN proxy。
- 增加 CLI/integration 测试。

验收：

```bash
cargo test -p modeld-proxy
cargo test --test proxy_integration
```

### 7.6 Wave 5：让 crash-safe CAS 成为默认路径

目标：所有用户主路径都使用 `store_crash_safe()`。

任务：

- 将 `CasStore::store()` 改为内部生成 tx_id 后调用 `store_crash_safe()`，或将旧函数改名 `store_legacy_for_tests`。
- downloader 移除手工 staging copy，直接使用 `store_crash_safe()`。
- CLI scan、WebUI scan 使用同一 ingestion 函数。
- fault injection 覆盖 downloader 和 scan。

验收：

```bash
cargo test -p modeld-core --test fault_injection_tests
cargo test
```

### 7.7 Wave 6：补齐认证体验

目标：开启 token 后 WebUI 对普通用户可用。

任务：

- 后端提供 `/api/v1/auth/status` 或静态 bootstrap config，告诉前端是否需要 token。
- 前端遇到 401 显示 token 输入 modal。
- token 保存在 sessionStorage。
- fetch 自动加 Authorization。
- WebSocket 鉴权方案明确实现。

验收：

- 无 token 访问 API 401。
- 输入 token 后页面所有 API 正常。
- token 不出现在日志和错误信息中。

### 7.8 Wave 7：文档和发布可信度

目标：让外部用户一眼知道项目真实状态、安装方式、风险边界。

任务：

- README 改为 Beta：
  - 当前稳定功能。
  - 已知限制。
  - 3 分钟 quickstart。
  - Windows 注意事项。
  - WebUI 认证说明。
- 重写 `AUDIT.md` 或改名 `AUDIT_2026_06_30.md`。
- 删除/更新陈旧的 `ARCHITECTURE_CURRENT.md`。
- 增加 release checklist。

验收：

- 新用户按 README 能完成 scan、dry-run dedup、verify。
- 所有命令示例与 `modeld --help` 一致。

---

## 8. 建议路线图

### 0-2 周：可信 Beta

- 修复 P0 bug。
- WebUI scan/dedup/GC 可用。
- README 降级为 Beta 并准确描述。
- 发布 Windows/Linux/macOS 二进制预览版。

### 3-6 周：个人用户增长

- 自动发现模型目录。
- WebUI first-run wizard。
- HF snapshot。
- Provenance/license 基础展示。
- ComfyUI/A1111 集成说明。

### 2-3 个月：团队版雏形

- LAN proxy 稳定。
- 只读团队 WebUI。
- token/用户管理。
- 审计日志下载。
- Docker Compose 部署。

### 3-6 个月：商业化验证

- Pro 桌面版。
- Team 私有部署。
- GitHub Sponsors/Patreon/License。
- 面向 AI 工作室和小研究团队做 20-50 个访谈。

---

## 9. 立刻可执行的 issue 列表

1. P0: WebUI scan must write files to CAS.
2. P0: Fix UI/backend route mismatch for GC preview and dedup.
3. P0: Align WebSocket scan event schema with frontend.
4. P0: Restore `modeld proxy start` to start the real proxy server.
5. P0: Make crash-safe CAS storage the default in scan and downloader.
6. P1: Add WebUI API contract tests.
7. P1: Add browser smoke tests for dashboard/library/dupes/settings.
8. P1: Add WebUI token entry and Authorization header support.
9. P1: Rewrite README as Beta and remove inaccurate v1.0 claim.
10. P1: Implement HF snapshot download with allow/ignore patterns.
11. P2: Add provenance/license reporting.
12. P2: Add first-run wizard for common model directories.

---

## 10. 最终判断

modeld 的方向值得继续做。它击中了开源 AI 模型生态一个朴素但高频的问题：模型文件太大、太多、太散、太容易重复，且跨工具缺少统一治理。

当前项目已经不是概念玩具，底层工程量足够支撑一个有影响力的开源项目。但它也还不是一个可以放心称为 v1.0 的大众工具。真正的分水岭不是再加多少命令，而是能否把“用户第一次扫描、第一次节省空间、第一次恢复误操作、第一次局域网共享”这四条路径做得可靠、清晰、可验证。

修完 P0 后，modeld 可以进入 Beta 公开推广；补齐安装、WebUI、HF snapshot 和生态集成后，才适合认真尝试 Pro/Team 商业化。
