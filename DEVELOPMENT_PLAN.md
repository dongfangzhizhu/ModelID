# modeld 开发计划总结

## 🎯 项目概述

**modeld** - AI 模型世界的 `containerd` + `git-lfs` + `nix store`

统一管理所有 AI 前端（ComfyUI / A1111 / Forge / InvokeAI）的模型存储，通过内容寻址消除重复，节省 TB 级磁盘空间。

**核心价值主张**：用户不需要修改任何现有配置，即可自动节省 TB 级磁盘空间。

## 📋 当前状态

✅ **Phase 0 设计文档已完成** - 2024年

已创建完整的架构设计和 RFC 文档，位于：
```
.kiro/specs/modeld-phase0-architecture/
├── README.md           # 文档导航和快速开始
├── spec.md             # 高层次概览
├── design.md           # 完整技术设计（包含 7 个 RFC）
├── requirements.md     # 详细功能和非功能需求
└── tasks.md            # 15 个实施任务清单
```

## 🗺️ 开发路线图

### Phase 0: 架构设计与 RFC ✅ **已完成**
- **工期**：2-3 周
- **状态**：设计文档已创建
- **交付物**：7 个 RFC + 完整架构设计
- **下一步**：审查设计，开始 Phase 1 实施

### Phase 1: Core Scanner MVP（下一阶段）
- **工期**：2-3 周
- **目标**：第一个可发布的工具
- **核心功能**：
  - 文件扫描器（支持 .safetensors/.gguf/.ckpt）
  - BLAKE3 哈希引擎（目标 ≥2GB/s）
  - SQLite 元数据注册表
  - 重复文件检测
  - safetensors 元数据解析

**CLI 命令**：
```bash
modeld scan <path>      # 扫描目录
modeld stats            # 统计信息
modeld dupes            # 重复文件报告
modeld list             # 列出所有模型
modeld info <hash>      # 单个文件详情
```

**成功标准**：
- 能扫描 500+ 模型的 TB 级目录不崩溃
- 正确识别重复文件
- Windows/Linux/macOS 三平台 CI 通过
- 展示"你有 XXX GB 重复文件"

### Phase 2: CAS Store & Dedup
- **工期**：6-8 周
- **目标**：实际节省磁盘空间
- **核心功能**：
  - CAS 存储实现
  - 虚拟模型目录（Virtual FS）
  - 去重引擎（transactional move）
  - 引用计数与 GC
  - 崩溃恢复

**成功标准**：
- 实际节省 TB 级空间
- ComfyUI/Forge 均可正常加载模型
- 崩溃恢复测试通过
- Windows 跨卷场景正确降级

### Phase 3: HF 拦截层
- **工期**：4-8 周
- **目标**：透明切入 Python AI 生态
- **核心功能**：
  - Fake HF cache layout
  - 下载去重
  - 断点续传
  - Python hook 安装

**成功标准**：
- `pip install modeld-hook` 后透明重定向
- 同一模型不重复下载
- 5 个主流框架集成测试通过

### Phase 4: Workflow & Reference Graph
- **工期**：4-6 周
- **目标**：智能模型管理
- **核心功能**：
  - ComfyUI workflow 解析
  - 依赖图构建
  - 安全 GC
  - 孤儿检测

### Phase 5: Local Registry & Proxy
- **工期**：6-8 周
- **目标**：局域网共享
- **核心功能**：
  - Local HF Proxy Server
  - 局域网自动发现
  - 分块传输
  - 访问控制

### Phase 6: Advanced Dedup（长期）
- **工期**：长期演进
- **目标**：平台级能力
- **核心功能**：
  - Chunk-level dedup
  - OCI-style model layers
  - 分布式存储
  - 远程引用

## 📊 累计工期估算

**Phase 0～5**：约 **7～9 个月**（含测试与缓冲）

| Phase | 工期 | 累计 |
|-------|------|------|
| Phase 0 | 2-3 周 | 3 周 |
| Phase 1 | 2-3 周 | 6 周 |
| Phase 2 | 6-8 周 | 14 周 |
| Phase 3 | 4-8 周 | 22 周 |
| Phase 4 | 4-6 周 | 28 周 |
| Phase 5 | 6-8 周 | 36 周 |

## 🛠️ 技术架构

### 系统组件

```
AI 前端层 (ComfyUI/Forge/A1111/InvokeAI)
    │
    ├─ HF Hook (Python) ──────┐
    └─ Virtual Model FS ───────┤
                               │
                    ┌──────────▼──────────┐
                    │       modeld        │
                    │  - Download Manager │
                    │  - CAS Storage      │
                    │  - Dedup Engine     │
                    │  - Metadata Index   │
                    │  - Ref Tracker      │
                    │  - Workflow Parser  │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │   Content Store     │
                    │   (BLAKE3-addressed)│
                    └─────────────────────┘
```

### 技术栈

| 组件 | 选型 | 理由 |
|------|------|------|
| Core daemon | **Rust** | 性能、内存安全、异步 IO |
| CLI | **Rust (clap)** | 与 daemon 共用 crate |
| Async runtime | **Tokio** | 成熟生态 |
| Hash | **BLAKE3** | 比 SHA256 快 3-5x |
| 元数据 DB | **SQLite + rusqlite** | 零依赖，WAL 模式 |
| HF Hook | **Python** | 必须与 Python AI 生态对接 |
| 配置 | **TOML** | Rust 生态标准 |
| 文件监听 | **notify** | 跨平台 |

### Repository 结构

```
modeld/
├── Cargo.toml                  (workspace)
├── crates/
│   ├── modeld-core/            (核心库)
│   ├── modeld-daemon/          (后台服务)
│   ├── modeld-cli/             (命令行工具)
│   ├── modeld-scanner/         (文件扫描)
│   ├── modeld-metadata/        (格式解析)
│   └── modeld-proxy/           (HF proxy)
├── python/
│   └── modeld_hook/            (HF 拦截层)
├── docs/
│   └── rfcs/                   (7 个 RFC 文档)
├── tests/integration/
└── README.md
```

## 🎯 关键设计决策

### 1. 内容寻址存储 (CAS)

**目录结构**：
```
$MODELD_STORE/
├── cas/blake3/ab/abcdef123...  (不可变对象)
├── virtual/comfyui/checkpoints/  (hardlinks → cas/)
├── tmp/downloads/  (下载中间态)
├── quarantine/  (待删除对象，30天宽限期)
└── modeld.db  (SQLite 元数据)
```

**特点**：
- BLAKE3 哈希（64 字符）
- 前缀分片（2 字符，256 个桶）
- 只读文件（chmod 444）
- 可扩展到百万级模型

### 2. Windows 兼容性策略（关键）

**挑战**：
- 符号链接需要开发者模式或管理员权限
- 跨卷 Hardlink 不支持
- Junction Point 仅支持目录

**解决方案**：多层降级策略

| 场景 | 技术方案 | 降级方案 |
|------|---------|---------|
| 同盘模型 dedup | NTFS Hardlink | 无需降级 |
| 跨盘模型 dedup | ❌ Hardlink 不可跨卷 | NTFS Junction / 引用计数 + 延迟 copy |
| 虚拟目录 | NTFS Symlink（需开发者模式）| NTFS Junction Point |
| 无权限环境 | - | 仅记录引用，不做物理 dedup |

### 3. 两阶段提交协议

**Phase A (Prepare)**：
1. 写 WAL 记录（status=pending）
2. 复制文件到 CAS tmp/
3. 验证 hash 一致性
4. 更新 WAL（status=copied）

**Phase B (Commit)**：
5. 原子 rename（tmp → final）
6. 在源路径创建 hardlink/symlink
7. 更新 WAL（status=committed）
8. 删除源文件（ref count 确认为 0 后）

**崩溃恢复**：启动时扫描 WAL，从中断点继续或回滚

### 4. HuggingFace 拦截

**两层策略**：

**Layer 1**: HF_HOME 环境变量（主策略）
```bash
export HF_HOME="$MODELD_STORE/hf_cache"
```
- 稳定，不受 HF 库更新影响
- 模拟完整 HF cache 目录结构

**Layer 2**: Python monkeypatch（补充）
```python
import modeld_hook  # 自动激活拦截
```
- 覆盖边缘情况
- 需要维护兼容性

## 📈 性能目标

| 场景 | 目标 | 备注 |
|------|------|------|
| 首次扫描 1TB（NVMe）| ≤ 15 分钟 | ≥2GB/s 吞吐量 |
| 增量扫描（无变化）| ≤ 30 秒 | 仅元数据检查 |
| `modeld dupes`（已扫描）| ≤ 1 秒 | 纯数据库查询 |
| 内存占用 | ≤ 200MB | 扫描期间 |

## ⚠️ 风险登记册

| 风险 | 概率 | 影响 | 缓解策略 |
|------|------|------|---------|
| Windows symlink 需要管理员权限 | 高 | 中 | 降级到 junction point；文档明确说明 |
| 跨卷 hardlink 不支持 | 确定 | 中 | Phase 0 明确 fallback 策略 |
| HF monkeypatch 因版本更新失效 | 中 | 高 | 优先 `HF_HOME` 方案；监控 HF 发布 |
| safetensors metadata 格式不规范 | 高 | 低 | 防御性解析；缺失视为 unknown |
| 单人维护生态碎片化压力 | 高 | 中 | 早期建立贡献者机制；模块化架构 |

## 🚀 下一步行动

### 立即行动（Phase 0 → Phase 1 过渡）

1. **审查 Phase 0 设计**
   - [ ] 阅读所有 RFC 文档
   - [ ] 验证 Windows 兼容性策略
   - [ ] 检查数据库 Schema 完整性
   - [ ] 识别潜在问题

2. **环境准备**
   - [ ] 安装 Rust 工具链（1.70+）
   - [ ] 设置 Cargo workspace
   - [ ] 配置开发 IDE（VS Code + rust-analyzer）
   - [ ] 准备测试环境（Windows/Linux）

3. **Phase 1 启动**
   - [ ] 创建 Git repository
   - [ ] 初始化 Cargo workspace
   - [ ] 设置 CI/CD（GitHub Actions）
   - [ ] 开始实现 Core Scanner MVP

### Phase 1 任务优先级

**高优先级**（MVP 核心）：
1. BLAKE3 哈希引擎
2. 文件扫描器
3. SQLite 数据库操作
4. 重复文件检测
5. CLI 命令实现

**中优先级**（增强功能）：
6. safetensors 元数据解析
7. 进度显示
8. 增量扫描
9. JSON 输出

**低优先级**（优化）：
10. 性能优化
11. 内存使用优化
12. 错误消息改进

## 📚 参考文档

### 设计文档
- **Phase 0 完整设计**：`.kiro/specs/modeld-phase0-architecture/design.md`
- **需求文档**：`.kiro/specs/modeld-phase0-architecture/requirements.md`
- **任务清单**：`.kiro/specs/modeld-phase0-architecture/tasks.md`
- **快速开始**：`.kiro/specs/modeld-phase0-architecture/README.md`

### 项目计划
- **总体计划**：`modeld-project-plan.md`

### 外部资源
- BLAKE3: https://github.com/BLAKE3-team/BLAKE3
- HuggingFace Hub: https://huggingface.co/docs/huggingface_hub
- SQLite WAL: https://www.sqlite.org/wal.html
- Rust Book: https://doc.rust-lang.org/book/

## 🎉 开源运营策略

### 渐进式采用路径

```
Step 1: modeld scan      ← 只读，零风险，1 分钟上手
         ↓ 用户看到"你有 320GB 重复"
Step 2: modeld dedup     ← 节省空间，可 dry-run
         ↓ 用户节省了实际空间
Step 3: pip install modeld-hook  ← 透明接入生态
         ↓ 新模型自动入库
Step 4: 完整生态集成     ← all-in
```

### 发布节奏

| 节点 | 版本 | 宣传重点 |
|------|------|---------|
| Phase 1 完成 | v0.1.0 | "你有 XXX GB 重复文件" |
| Phase 2 完成 | v0.2.0 | 实际节省空间数字 |
| Phase 3 完成 | v0.3.0 | "pip install，零配置" |
| Phase 4 完成 | v0.4.0 | "再也不会误删模型" |
| Phase 5 完成 | v1.0.0 | 局域网共享，正式 1.0 |

### 目标指标

| 阶段 | GitHub Stars | 活跃用户 |
|------|-------------|---------|
| Phase 1 发布后 1 个月 | 500+ | 100+ |
| Phase 2 发布后 | 2000+ | 1000+ |
| Phase 3 发布后 | 5000+ | 5000+ |

## 📄 许可证

待定（建议 MIT 或 Apache 2.0）

---

**文档版本**：1.0  
**当前 Phase**：Phase 0 设计完成 ✅  
**下一 Phase**：Phase 1 Core Scanner MVP  
**预计总工期**：7-9 个月（Phase 0-5）  
**更新日期**：2024
