# Phase 3: HuggingFace 拦截层

**状态**: ✅ **完成**
**完成日期**: 2026-06-13

## 目标

实现透明的 HuggingFace 生态拦截，让所有主流 AI 框架的 HF 下载自动路由到 modeld CAS，跨框架去重。

---

## 实现内容

### 1. 数据库扩展（`crates/modeld-core/src/db.rs`）

新增两张表：

#### `downloads` 表
| 字段 | 说明 |
|------|------|
| `model_hash` | BLAKE3 hash（外键 → models） |
| `source_url` | 完整下载 URL |
| `sha256_hash` | HF 提供的 SHA256 |
| `repo_id` | HF repo ID |
| `filename` | 文件名 |
| `revision` | Git 分支/commit |
| `status` | pending/downloading/done/failed |
| `bytes_done/total` | 进度 |

#### `hf_mappings` 表（SHA256 ↔ BLAKE3 双向映射）
| 字段 | 说明 |
|------|------|
| `sha256_hash` | HuggingFace 使用的 SHA256 |
| `blake3_hash` | modeld CAS 使用的 BLAKE3 |
| `repo_id` | 来源 repo |
| `filename` | 文件名 |

---

### 2. HF 假缓存结构（`crates/modeld-core/src/hf_cache.rs`）

完整模拟官方 `~/.cache/huggingface/hub/` 目录结构：

```
$MODELD_STORE/hf_cache/hub/
└── models--{org}--{model}/
    ├── blobs/{sha256}          → symlink → CAS 对象
    ├── refs/main               → 文本文件（包含 revision hash）
    └── snapshots/{revision}/
        └── {filename}          → symlink → ../../blobs/{sha256}
```

**跨平台 symlink 策略**：
- Linux/macOS: `std::os::unix::fs::symlink`
- Windows: 优先 symlink_file（需 Developer Mode）→ hard_link → copy

---

### 3. 下载管理器（`crates/modeld-core/src/downloader.rs`）

完整下载流程：
1. `HEAD` 请求获取元数据（SHA256 from `X-Linked-Etag` 响应头）
2. 检查 HF 假缓存目录（命中直接返回）
3. 检查 SHA256→BLAKE3 数据库映射（命中跳过下载）
4. 下载到 `tmp/downloads/{id}.part`（支持 HTTP Range 断点续传）
5. 计算 BLAKE3 hash，检查 CAS 中是否已有（内容去重）
6. 移入 CAS，记录 `downloads` 和 `hf_mappings`
7. 创建假 HF 缓存目录结构

---

### 4. CLI HF 命令（`crates/modeld-cli/src/main.rs`）

| 命令 | 功能 |
|------|------|
| `modeld hf-check <repo> <file>` | 查询本地缓存；`--json` 输出供 Python hook 调用 |
| `modeld hf-download <repo> <file>` | 通过 modeld CAS 下载，自动去重，显示进度条 |
| `modeld hf-setup` | 打印 HF_HOME 配置说明（PowerShell/bash/CMD） |
| `modeld hf-status` | 显示 HF 缓存统计（repos/blobs/下载记录） |

---

### 5. Python modeld_hook 包（`python/modeld_hook/`）

**双层拦截策略**：

| 层 | 机制 | 覆盖 |
|----|------|------|
| Layer 1 | `HF_HOME` 环境变量 | ~95%（官方 API，稳定） |
| Layer 2 | Python monkeypatch | ~5%（边缘情况 fallback） |

**文件结构**：
```
python/
├── modeld_hook/
│   ├── __init__.py    # 自动激活逻辑（import即启用）
│   └── intercept.py   # hf_hub_download/snapshot_download wrapper
├── tests/
│   └── test_modeld_hook.py  # mock测试套件
└── pyproject.toml     # Python包配置（Python ≥ 3.9）
```

**使用方法**：
```python
# Method 1: 显式导入
import modeld_hook  # 自动激活

# Method 2: 仅使用 HF_HOME 策略
export HF_HOME="$(modeld hf-setup --print-path)"
```

---

## 测试统计

| 测试类型 | 数量 |
|----------|------|
| hf_cache 单元测试 | 5 |
| downloader 单元测试 | 3 |
| db (downloads/hf_mappings) 单元测试 | 包含在原有 60 个中 |
| 集成测试 | 9 |
| **总计** | **69** ✅ |

---

## 下一步：Phase 4

按照 [modeld-project-plan.md](./modeld-project-plan.md)，Phase 4 将实现：
- **虚拟文件系统层（VFS）**：FUSE/WinFsp 挂载点
- **AI 前端集成**：ComfyUI/Forge/A1111 链接自动刷新
- **模型分类**：safetensors/gguf/diffusers 自动识别
- **守护进程**：后台监控 + REST API
