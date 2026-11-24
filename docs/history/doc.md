# **ModelID/modeld 项目完善指导文档**

## **1. 项目目标**

将当前 `ModelID/modeld` 完善为一个安全、稳定、易安装、易使用、可扩展的 AI 模型资产管理工具。核心目标是帮助个人用户、AI 创作者、小型团队、实验室和企业私有化环境解决模型文件重复、下载重复、目录混乱、版本不可追溯、局域网共享困难和清理风险高的问题。

项目最终应具备以下产品定位：

> `modeld` 是面向 AI 模型文件的本地 CAS 存储、去重、下载缓存、局域网代理和可视化治理工具。它让用户下载一次模型，多工具复用，多设备共享，并且所有清理操作都可预览、可回滚、可审计。

## **2. 当前代码库需要优先统一的基础问题**

### **2.1 统一项目命名**

当前仓库名为 `ModelID`，README 主标题为 `modeld`。需要确定最终品牌策略。

建议方案：

- 仓库名改为 `modeld`。
- CLI 二进制名称保持 `modeld`。
- Web UI 二进制名称保持 `modeld-webui` 或改为 `modeld ui` 子命令。
- README 第一行明确：`modeld: AI model CAS, deduplication, cache proxy, and local registry`。
- 所有文档、包名、配置文件、环境变量统一使用 `modeld`。

需要修改的内容包括：

- `README.md`
- `Cargo.toml`
- Python package metadata
- Web UI title
- CLI help text
- docs 目录
- install scripts
- GitHub repository description
- release artifact names

验收标准：

- 用户在 README、CLI、Web UI、package metadata 中看到的项目名称一致。
- 搜索 `ModelID` 只应出现在历史兼容说明或迁移说明中。
- `modeld --version` 输出项目名、版本、commit hash、build target。

### **2.2 统一默认端口和服务模型**

当前 README 中 proxy 和 Web UI 都出现默认端口 `8234`，需要明确架构。

建议方案 A：

- `modeld daemon` 是统一后端服务，默认 `127.0.0.1:8234`。
- Web UI 是 daemon 的静态页面和 API。
- HF proxy 是 daemon 的 `/v1/hf-proxy` 路由。
- 用户只需要启动一个服务。

建议方案 B：

- proxy 默认端口 `8234`。
- Web UI 默认端口 `8235` 或 `9000`。
- README 明确二者关系。

优先建议采用方案 A，因为用户体验更简单。

验收标准：

- `modeld serve --store <path>` 能同时提供 REST API、Web UI、HF proxy 和 WebSocket。
- `modeld proxy start` 作为兼容命令，内部调用 `modeld serve --proxy-only`。
- `modeld-webui` 可以保留，但 README 推荐使用 `modeld serve --open`。

### **2.3 统一默认存储路径**

当前文档同时出现 `~/.local/share/modeld` 和 `.modeld/`。需要统一。

建议设计：

- Linux: `$XDG_DATA_HOME/modeld`，默认 `~/.local/share/modeld`
- macOS: `~/Library/Application Support/modeld`
- Windows: `%LOCALAPPDATA%\modeld`
- 项目级 store 可通过 `--store .modeld` 显式指定
- 环境变量：`MODELD_STORE`
- 配置文件：`modeld.toml`

新增命令：

```bash
modeld store locate
modeld store init
modeld store verify
modeld store migrate --from <old> --to <new>
modeld config get store.path
modeld config set store.path <path>
```

验收标准：

- 所有命令都能正确读取同一套 store path 解析逻辑。
- README、Web UI、CLI help 中默认路径一致。
- `modeld doctor` 能显示当前 store path、是否可写、磁盘剩余空间、文件系统类型。

## **3. P0：数据安全与事务系统**

### **3.1 所有破坏性操作必须事务化**

以下操作必须进入统一事务系统：

- dedup
- unlink
- gc
- quarantine cleanup
- alias rewrite
- CAS object promotion
- HF download commit
- store migration

每个事务需要记录：

- transaction id
- operation type
- start time
- end time
- status
- affected source paths
- target paths
- original hash
- new hash
- size
- mtime
- permissions
- platform-specific metadata
- rollback plan
- error message

新增命令：

```bash
modeld tx list
modeld tx show <tx_id>
modeld tx rollback <tx_id>
modeld tx recover
modeld tx cleanup --older-than 30d
```

验收标准：

- 任意 P0 操作中途 kill 进程，再执行 `modeld tx recover` 可以恢复到一致状态。
- 任意失败事务不会留下半替换文件、空文件、错误链接或孤儿 CAS staging 文件。
- 所有事务测试必须覆盖 Linux、macOS、Windows。

### **3.2 CAS 写入必须 crash-safe**

CAS 写入流程必须是：

1. 写入 `tmp/cas_staging/<tx_id>/<object_id>.part`
2. 流式计算 BLAKE3
3. fsync 文件
4. 校验 size 和 hash
5. 原子 rename 到 CAS 最终路径
6. fsync 父目录
7. 标记 DB committed
8. 清理 staging

注意事项：

- 不允许跨文件系统 rename。
- 如果 store 与源文件不在同一文件系统，必须 copy 后 verify。
- CAS 对象默认只读。
- 对 CAS 对象不允许原地修改。
- 如果目标 CAS 已存在，必须验证 hash 和 size 后复用。

验收标准：

- 模拟断电、进程崩溃、磁盘空间不足、权限不足时，store 不损坏。
- `modeld verify --deep` 能发现 CAS 缺失、大小不匹配、hash 不匹配、DB 孤儿记录。
- `modeld repair` 能修复 DB 孤儿、staging 残留、quarantine 元数据不一致。

## **4. P0：去重与回滚**

### **4.1 Dedup 默认必须安全保守**

默认行为：

```bash
modeld dedup
```

应等价于：

```bash
modeld dedup --dry-run
```

真正执行必须使用：

```bash
modeld dedup --apply
```

或：

```bash
modeld dedup --auto --yes
```

建议新增参数：

```bash
modeld dedup --strategy hardlink
modeld dedup --strategy symlink
modeld dedup --strategy copy-to-cas
modeld dedup --strategy virtual-alias
modeld dedup --min-size 500MB
modeld dedup --include <glob>
modeld dedup --exclude <glob>
modeld dedup --protect <path>
modeld dedup --pin <hash>
```

验收标准：

- 默认不修改用户文件。
- 执行前显示完整计划：将替换哪些文件、canonical 是哪个、预计释放多少空间、能否回滚。
- 所有被替换文件必须进入 quarantine 或可通过硬链接恢复。
- `modeld quarantine restore <id>` 能恢复到原路径。

### **4.2 Canonical path 选择规则需要可解释**

当前 README 中提到规则是 CAS > oldest mtime > shortest path。建议增强为：

1. 如果 CAS 中已存在对象，CAS 为 canonical。
2. 如果文件被用户 pin，pin 文件优先。
3. 如果路径属于受保护目录，优先不动。
4. 如果文件在近期被访问，优先保留真实文件。
5. 如果多个候选等价，选择最短路径。
6. 所有选择必须写入 dedup report。

新增命令：

```bash
modeld pin add <path-or-hash>
modeld pin remove <path-or-hash>
modeld pin list
```

验收标准：

- 用户可以解释为什么某个文件被保留、某个文件被替换。
- Web UI 中每个 duplicate group 都显示 canonical 选择原因。

## **5. P0：Windows 兼容专项**

### **5.1 新增 Windows 能力检测**

新增命令：

```bash
modeld doctor --windows
```

检查项：

- OS version
- filesystem type
- symlink privilege
- hardlink support
- junction support
- long path support
- antivirus lock risk
- current user permission
- store path writable
- source path writable
- cross-volume operation
- PowerShell execution policy
- path encoding

验收标准：

- Windows 普通用户不需要管理员权限也能完成 scan、hash、list、status。
- 若 symlink 不可用，自动降级到 hardlink/junction/copy 策略，并明确提示。
- 跨盘 dedup 不尝试硬链接。
- 长路径文件能正常扫描或给出明确修复建议。

### **5.2 文件锁处理**

Windows 上模型文件可能正在被 ComfyUI、A1111 或 Python 进程占用。

新增行为：

- 替换文件前检测 lock。
- 文件被占用时跳过，不中断整个任务。
- 生成 locked files report。
- Web UI 显示“稍后重试”。

新增命令：

```bash
modeld locks scan <path>
modeld dedup --skip-locked
modeld dedup --retry-locked
```

验收标准：

- 被占用文件不会导致事务失败。
- 不会强制删除或替换被占用文件。
- 用户能看到哪些文件被哪个进程占用，如果平台支持。

## **6. P0：HuggingFace 兼容**

### **6.1 支持完整 HF 下载语义**

需要覆盖：

- public repo
- private repo
- gated repo
- token auth
- revision
- branch
- commit hash
- filename
- subfolder
- LFS file
- Xet-backed file
- ETag
- Range request
- redirect
- retry
- offline cache
- snapshot layout

建议命令：

```bash
modeld hf download <repo_id> <filename>
modeld hf snapshot <repo_id>
modeld hf cache list
modeld hf cache verify
modeld hf token set
modeld hf token remove
modeld hf token status
```

注意：

- token 不得明文写入日志。
- token 存储应优先使用系统 keychain；不支持时使用权限受限文件。
- `--token` 参数应提示风险，推荐环境变量或 keychain。
- 下载成功必须校验 ETag/hash/size。
- 如果 Xet 兼容不足，必须明确提示并 fallback 到官方下载路径。

验收标准：

- 对 `hf_hub_download`、`snapshot_download`、`transformers.from_pretrained`、`diffusers.from_pretrained` 建立端到端测试。
- 支持 gated model 的授权失败提示。
- 私有 repo 不会泄漏 repo id、token、URL 到普通日志中。

### **6.2 Python hook 需要可观测和可禁用**

当前 README 说安装 `modeld-hook` 并 `import modeld_hook`。建议增强：

```python
import modeld_hook
modeld_hook.activate()
modeld_hook.deactivate()
modeld_hook.status()
```

环境变量：

```bash
MODELD_HOOK_ENABLE=1
MODELD_HOOK_DISABLE=1
MODELD_HOOK_LOG=debug
MODELD_HOOK_FALLBACK=1
```

验收标准：

- hook 不存在时不影响原始 HF 下载。
- hook 出错时默认 fallback。
- 用户能看到当前是否命中 modeld cache。
- 不 monkey patch 无关函数。
- 提供 pytest 覆盖不同版本的 `huggingface_hub`、`transformers`、`diffusers`。

## **7. P1：扫描器与索引系统**

### **7.1 增量扫描**

扫描大模型目录成本很高。必须避免每次全量 hash。

索引字段：

- path
- size
- mtime
- inode/file id
- device id
- quick fingerprint
- blake3 hash
- last seen time
- scan status
- error status

策略：

- 如果 size、mtime、file id 未变，跳过 hash。
- 可选 quick hash：文件头、中、尾采样。
- 对疑似变化文件再做 full hash。
- 支持 ignore pattern。
- 支持 symlink cycle detection。

新增命令：

```bash
modeld scan <path>
modeld scan --incremental
modeld scan --full
modeld scan --watch
modeld scan --exclude "*.tmp"
modeld scan --follow-symlinks=false
```

验收标准：

- 10 万文件目录扫描不会内存爆炸。
- symlink cycle 不会死循环。
- 网络盘出错不会中断整个扫描。
- 扫描进度可通过 WebSocket 推送到 Web UI。

### **7.2 模型类型识别**

识别以下文件：

- `.safetensors`
- `.ckpt`
- `.pt`
- `.pth`
- `.bin`
- `.gguf`
- `.onnx`
- `.engine`
- `.json`
- tokenizer files
- config files
- LoRA
- VAE
- ControlNet
- embedding
- diffusers snapshot

新增元数据：

- framework
- architecture
- quantization
- parameter count
- precision
- license
- source URL
- repo id
- revision
- tags
- user notes

验收标准：

- Library 页面可以按类型、来源、大小、hash、标签、最近使用过滤。
- 不识别的文件也能作为 generic blob 管理。

## **8. P1：引用图与安全 GC**

### **8.1 引用来源扩展**

支持解析：

- ComfyUI workflow JSON
- ComfyUI custom nodes config
- A1111 config
- Forge config
- Diffusers cache
- Transformers cache
- Ollama manifest
- Python scripts 中的模型路径
- 用户手动 pin
- 最近访问记录
- Web UI 收藏

引用等级：

- hard reference：workflow/config 明确引用
- soft reference：alias 或最近访问
- pinned：用户手动保护
- unknown：无法判断
- orphan candidate：可清理候选

GC 默认只处理 orphan candidate，且只进入 quarantine。

新增命令：

```bash
modeld refs scan <path>
modeld refs graph
modeld refs why <hash>
modeld refs orphans
modeld gc --preview
modeld gc --quarantine
modeld gc --cleanup --older-than 30d
```

验收标准：

- `modeld refs why <hash>` 能回答某模型为什么不能删除。
- Web UI 中每个模型显示引用来源。
- GC 不处理 pinned、recently used、unknown reference 文件。
- GC report 可导出 JSON。

## **9. P1：Proxy 与局域网共享**

### **9.1 安全默认值**

默认行为：

- 只监听 `127.0.0.1`
- 不允许匿名远程访问
- 绑定 `0.0.0.0` 时强提示
- token 自动生成
- 日志隐藏 token
- CORS 默认关闭或限制本机

命令：

```bash
modeld serve --host 127.0.0.1 --port 8234
modeld serve --host 0.0.0.0 --port 8234 --auth token
modeld proxy token rotate
modeld proxy status
```

验收标准：

- 局域网访问必须有明确授权。
- mDNS discovery 不泄漏敏感 store path。
- 支持 token rotation。
- Range request 测试覆盖断点续传和并发下载。

### **9.2 企业部署能力**

新增：

- TLS
- reverse proxy guide
- OIDC/LDAP 预留接口
- Prometheus metrics
- audit log
- rate limit
- IP allowlist
- storage quota
- read-only mode

验收标准：

- 可以用 Docker Compose 一键启动。
- 可以放在 Nginx/Caddy 后面。
- `/metrics` 可被 Prometheus 抓取。
- 所有下载、删除、GC、token 操作进入 audit log。

## **10. P1：Web UI 产品化**

### **10.1 首页改为任务导向**

Dashboard 应显示：

- 当前 store path
- 总模型数量
- 总占用空间
- 可节省空间
- 最近扫描时间
- 下载队列状态
- quarantine 项数
- 健康状态
- 一键扫描
- 一键查看重复
- 一键恢复
- 一键启动 proxy

首页操作区：

- “扫描模型文件夹”
- “查找重复模型”
- “安全释放空间”
- “下载 HuggingFace 模型”
- “配置 ComfyUI/A1111”
- “启动局域网共享”
- “恢复隔离文件”
- “运行健康检查”

验收标准：

- 新用户打开 Web UI 后 1 分钟内知道下一步做什么。
- 所有危险操作都有 preview。
- 所有危险操作都有 undo/restore 路径。

### **10.2 Duplicates 页面**

需要显示：

- duplicate group
- file count
- total size
- wasted size
- canonical candidate
- canonical reason
- affected frontends
- risk level
- action preview

操作：

- keep this file
- pin
- dedup group
- dedup selected
- exclude path
- open in file explorer
- copy hash

验收标准：

- 用户可以只处理一个 duplicate group。
- 用户可以排除某个目录。
- 用户可以保存 dedup plan。

### **10.3 Library 页面**

需要支持：

- search
- filter
- sort
- tags
- notes
- source repo
- license
- model type
- size
- hash
- aliases
- refs
- last used
- pin/unpin

验收标准：

- 10000 个模型记录下 UI 不卡顿。
- 搜索和分页在后端完成。
- 支持复制 hash、路径、HF repo id。

### **10.4 Downloads 页面**

需要支持：

- 添加 HF 下载任务
- token 状态提示
- 队列暂停/恢复
- 失败重试
- 下载速度
- ETA
- Range resume
- 完成后自动入库
- 下载日志

验收标准：

- 浏览器刷新后下载状态不丢失。
- 下载失败能看到明确原因。
- 私有模型鉴权失败不泄漏 token。

## **11. P1：安装、发布与分发**

### **11.1 Release 工程**

必须提供：

- GitHub Release
- Windows x64 installer
- Windows portable zip
- macOS universal binary
- Linux x64 tar.gz
- Linux arm64 tar.gz
- checksum
- signature
- changelog
- SBOM

包管理：

```bash
cargo install modeld
pip install modeld-hook
brew install modeld
choco install modeld
scoop install modeld
winget install modeld
```

如果短期无法全部支持，至少先提供：

- GitHub Release 二进制
- `cargo install`
- `pip install modeld-hook`
- Windows zip
- macOS/Linux tar.gz

验收标准：

- 用户不需要 Rust 环境也能安装。
- README Quick Start 从安装开始，而不是从 `modeld init` 开始。
- 每个 release artifact 有 checksum。

### **11.2 First-run onboarding**

新增：

```bash
modeld init --interactive
modeld doctor
modeld serve --open
```

交互式初始化询问：

- store path
- model directories
- frontend type
- enable proxy
- enable Web UI
- enable auto scan
- quarantine TTL
- symlink/hardlink strategy

验收标准：

- 新用户可以按提示完成配置。
- 所有选择写入 `modeld.toml`。
- 生成可复制的诊断报告。

## **12. P2：模型治理能力**

### **12.1 标签与收藏**

新增：

```bash
modeld tag add <hash> <tag>
modeld tag remove <hash> <tag>
modeld tag list
modeld note set <hash> "<text>"
modeld favorite add <hash>
```

用途：

- 标记项目
- 标记客户
- 标记模型类型
- 标记许可证
- 标记质量
- 标记是否可删除

验收标准：

- 标签可在 Web UI 搜索过滤。
- 标签导出导入不丢失。
- GC 尊重 protected tags。

### **12.2 License 与来源追踪**

每个模型应记录：

- source type
- HF repo id
- revision
- download URL
- license
- downloaded by
- downloaded at
- original filename
- original hash
- model card URL

验收标准：

- Library 页面显示模型来源。
- 企业用户可以导出 license report。
- 无来源模型显示为 local/imported。

## **13. 测试计划**

### **13.1 单元测试**

必须覆盖：

- path normalization
- BLAKE3 hashing
- CAS path mapping
- DB schema migration
- size parser
- glob matcher
- config loading
- quarantine metadata
- transaction state machine
- HF URL parsing

### **13.2 集成测试**

必须覆盖：

- init store
- scan directory
- detect duplicates
- dry-run dedup
- apply dedup
- rollback dedup
- quarantine restore
- gc preview
- gc quarantine
- hf download public file
- proxy range request
- Web UI API
- WebSocket progress

### **13.3 故障注入测试**

必须模拟：

- process kill
- disk full
- permission denied
- locked file
- corrupted DB
- corrupted CAS object
- network interruption
- HTTP 500
- token expired
- symlink failure
- cross-device rename failure

### **13.4 跨平台测试矩阵**

| 平台 | 必测项 | 说明 |
|---|---|---|
| Windows 11 x64 | scan/dedup/link/quarantine/Web UI | 最高优先级 |
| Ubuntu x64 | 全功能 | CI 主平台 |
| macOS Apple Silicon | scan/dedup/Web UI | 创作者常用平台 |
| Linux arm64 | proxy/serve | NAS 和小服务器场景 |

验收标准：

- CI 对每个 PR 运行核心测试。
- nightly 运行慢速大文件测试。
- release 前运行跨平台 E2E。

## **14. 数据库与迁移**

建议使用 SQLite WAL 模式，但必须处理并发。

要求：

- schema version
- migration table
- backup before migration
- busy timeout
- read/write connection pool
- transaction isolation
- vacuum strategy
- corruption detection

新增命令：

```bash
modeld db status
modeld db backup
modeld db restore <backup>
modeld db migrate
modeld db vacuum
```

验收标准：

- 从旧版本升级不丢数据。
- DB locked 时有清晰错误。
- scan、proxy、Web UI 并发不互相破坏。

## **15. CLI 设计规范**

所有命令遵循：

```bash
modeld <noun> <verb>
```

建议命令树：

```bash
modeld init
modeld doctor
modeld serve
modeld status
modeld scan
modeld hash
modeld list
modeld info
modeld dupes
modeld dedup
modeld gc
modeld quarantine
modeld refs
modeld hf
modeld proxy
modeld pin
modeld tag
modeld tx
modeld verify
modeld repair
modeld config
modeld store
modeld db
```

输出规范：

- 默认人类可读
- `--json` 机器可读
- `--quiet`
- `--verbose`
- `--no-color`
- `--yes`
- `--dry-run`
- 所有破坏性命令支持 `--dry-run`

验收标准：

- 每个命令有 help 示例。
- JSON 输出结构稳定，并有 schema 文档。
- 错误码稳定，便于脚本集成。

## **16. 文档重构**

README 需要重写为用户导向，而不是 Phase 导向。

建议结构：

1. What is modeld?
2. Why use it?
3. Install
4. Quick Start
5. Web UI
6. Safe deduplication
7. HuggingFace cache
8. LAN proxy
9. Recovery and quarantine
10. Supported platforms
11. Safety guarantees
12. Documentation links
13. Contributing
14. License

必须新增文档：

- `docs/getting-started.md`
- `docs/install.md`
- `docs/windows.md`
- `docs/dedup-safety.md`
- `docs/huggingface.md`
- `docs/proxy.md`
- `docs/web-ui.md`
- `docs/recovery.md`
- `docs/config.md`
- `docs/api.md`
- `docs/architecture.md`
- `docs/troubleshooting.md`

验收标准：

- README 中所有命令都在 CI 中通过 doc test 或 smoke test。
- 文档包含真实截图或 GIF。
- 危险操作文档明确说明恢复方式。

## **17. AI Coding 工具执行顺序**

### **阶段 0：代码审计与基线建立**

任务：

- 读取整个仓库结构。
- 生成模块依赖图。
- 找出 CLI、core、db、webui、python hook 的边界。
- 运行现有测试。
- 记录失败测试。
- 生成 `AUDIT.md`。

输出：

- `AUDIT.md`
- `ARCHITECTURE_CURRENT.md`
- 当前测试结果
- 当前命令可用性矩阵

禁止：

- 不要先大规模重构。
- 不要改动数据格式。
- 不要删除现有功能。

### **阶段 1：安全事务与恢复**

任务：

- 实现统一 transaction manager。
- 将 dedup、gc、quarantine 接入事务。
- 增加 rollback/recover。
- 增加 crash-safe CAS commit。
- 增加 verify/repair。

输出：

- `crates/modeld-core/src/tx.rs`
- `crates/modeld-core/src/repair.rs`
- CLI commands: `tx`, `verify`, `repair`
- 故障注入测试

验收：

- kill 进程后可恢复。
- dedup 失败不破坏原文件。
- verify 能发现损坏 CAS。

### **阶段 2：安装与 first-run**

任务：

- 统一 store path。
- 修复 README 默认路径。
- 增加 `doctor`。
- 增加 `init --interactive`。
- 增加 release scripts。

输出：

- 安装文档
- 平台检测
- release workflow
- checksums

验收：

- 无 Rust 环境用户可安装。
- Windows 普通用户可运行 `modeld doctor`。

### **阶段 3：HF 兼容**

任务：

- 补全 HF token、revision、private/gated、LFS/Xet 兼容。
- 增加下载校验。
- 增加 Python hook status。
- 增加 E2E 测试。

输出：

- `hf download`
- `hf snapshot`
- `hf token`
- Python hook tests

验收：

- public/private/gated 三类场景行为明确。
- token 不泄漏。
- 断点续传可靠。

### **阶段 4：Web UI 产品化**

任务：

- Dashboard 改任务导向。
- Duplicates 加 preview/restore。
- Library 加搜索过滤标签。
- Downloads 加队列管理。
- Settings 加 store/proxy/security 配置。

输出：

- Web UI 页面
- REST API
- WebSocket progress
- E2E UI tests

验收：

- 新用户可通过 Web UI 完成扫描、查看重复、执行安全 dedup、恢复 quarantine。
- 所有危险操作有 preview。

### **阶段 5：Proxy 和团队能力**

任务：

- 统一 `modeld serve`。
- 默认安全绑定。
- token auth。
- mDNS discovery。
- Prometheus metrics。
- audit log。

输出：

- `modeld serve`
- `/metrics`
- audit logs
- proxy docs

验收：

- 局域网共享可用。
- 默认不暴露无认证远程访问。
- Range request 通过测试。

### **阶段 6：模型治理**

任务：

- tags
- notes
- pin
- license metadata
- source tracking
- refs 扩展
- safe GC policy

输出：

- Library metadata
- license report
- refs graph
- pin-aware GC

验收：

- 用户可以知道每个模型来自哪里、被哪里引用、是否可删除。
- GC 不删除 pinned 和 unknown risk 模型。

## **18. AI Coding 工具实现提示词**

可以将下面提示词交给 AI coding 工具逐阶段执行：

```markdown
你正在维护一个 Rust + Web UI + Python hook 项目 modeld。目标是将其打造为安全可靠的 AI 模型文件 CAS、去重、HuggingFace 缓存代理和 Web UI 管理工具。

请先不要大规模重构。第一步读取仓库结构、Cargo workspace、CLI command、core storage、SQLite schema、Web UI API、Python hook。生成 AUDIT.md，列出当前功能、缺失功能、失败测试、风险点和建议修改顺序。

所有涉及用户文件修改的功能必须遵守以下规则：
1. 默认 dry-run。
2. 必须有事务日志。
3. 必须可 rollback。
4. 必须进入 quarantine，不直接删除。
5. 必须支持 verify 和 repair。
6. 必须跨平台，尤其是 Windows。
7. 必须有故障注入测试。

优先实现 P0：
- 统一 store path。
- 修复 proxy/Web UI 端口冲突。
- 增加 transaction manager。
- 增加 verify/repair。
- 加强 dedup rollback。
- 加强 Windows hardlink/symlink/junction fallback。
- 补全 HF token/revision/private/gated/download resume 校验。
- 增加 doctor 命令。

每个 PR 必须包含：
- 代码修改
- 单元测试
- 集成测试
- 文档更新
- 迁移说明
- 回滚方案

不要删除用户数据。不要让任何破坏性命令默认执行。不要在日志中输出 token。不要假设 symlink 在 Windows 可用。不要把无法确认无引用的模型作为可删除对象。
```

## **19. Definition of Done**

项目达到“大众可用”的最低标准如下：

- 提供 Windows/macOS/Linux 免编译安装包。
- `modeld init --interactive` 可完成首次配置。
- `modeld doctor` 能诊断环境问题。
- Web UI 可完成扫描、查看重复、预览 dedup、执行 dedup、恢复 quarantine。
- 所有破坏性操作默认可预览、可回滚、可审计。
- HF 下载支持 public/private/gated/revision/resume。
- Windows 普通用户可正常使用核心功能。
- README 中所有命令真实可运行。
- 有 release、checksum、changelog。
- 有端到端测试和故障注入测试。
- 有明确的数据安全承诺和恢复文档。

## **20. 建议的最小商业化版本**

第一个可商业化版本不需要做完整企业 MLOps，只需要做到：

- 本地模型扫描
- 重复模型检测
- 安全 dedup
- quarantine restore
- Web UI
- HF 下载缓存
- 局域网只读 proxy
- Windows 安装器
- 自动更新
- 基础标签和 pin
- 诊断报告

这个版本可以命名为 `modeld 1.0`。它的核心卖点应非常直接：

> 下载一次，处处可用；安全去重，随时恢复；本地优先，团队共享。