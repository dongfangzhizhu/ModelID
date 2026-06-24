# 快速编译参考

## 快速命令速查表

### 编译所有 crates

```bash
# 推荐：Python (所有平台)
python build.py

# Linux/macOS: Shell
./scripts/build.sh

# Windows: PowerShell
.\scripts\build.ps1

# 通过 Makefile (所有平台)
make rust-build
```

### 编译特定 crate

```bash
# Python
python build.py modeld-core
python build.py modeld-cli modeld-proxy

# Shell (Linux/macOS)
./scripts/build.sh modeld-core

# PowerShell (Windows)
.\scripts\build.ps1 modeld-core

# Makefile
make rust-build-crate CRATE=modeld-core
```

### Release 模式编译

```bash
# Python
python build.py --release

# Shell
./scripts/build.sh --release

# PowerShell
.\scripts\build.ps1 -Release

# Makefile
make rust-build-release
```

### 检查代码

```bash
# Python
python build.py --check

# Shell
./scripts/build.sh --check

# PowerShell
.\scripts\build.ps1 -Check

# Makefile
make rust-check
```

### 运行测试

```bash
# Python
python build.py --test

# Shell
./scripts/build.sh --test

# PowerShell
.\scripts\build.ps1 -Test

# Makefile
make rust-test
```

### 代码格式化

```bash
# Python
python build.py --format

# Makefile
make rust-fmt
```

### 代码检查 (Clippy)

```bash
# Python
python build.py --lint

# Makefile
make rust-lint
```

### 详细输出

```bash
# Python
python build.py --verbose

# Shell
./scripts/build.sh --verbose

# PowerShell
.\scripts\build.ps1 -Verbose
```

---

## 可用的 Crates

1. **modeld-core** - 核心库，包含主要数据结构和算法
2. **modeld-cli** - 命令行工具
3. **modeld-proxy** - 代理服务器
4. **modeld-client** - 客户端库
5. **modeld-webui** - Web UI 服务器

---

## 常见场景

### 场景 1：快速开发构建
```bash
python build.py
```

### 场景 2：发布前检查
```bash
python build.py --release --lint
```

### 场景 3：修复特定 crate 后测试
```bash
python build.py modeld-cli --test
```

### 场景 4：格式化和 lint
```bash
python build.py --format
python build.py --lint
```

### 场景 5：清理并重新构建
```bash
cargo clean
python build.py
```

---

## 前置要求

- ✅ Rust 工具链 (cargo, rustc)
- ✅ Python 3.6+ (仅用于 Python 脚本)

验证：
```bash
cargo --version
python --version
```

---

## 获取帮助

### Python 脚本
```bash
python build.py --help
```

### Shell 脚本 (Linux/macOS)
```bash
./scripts/build.sh --help
```

### PowerShell 脚本 (Windows)
```bash
.\scripts\build.ps1 -Help
```

### Makefile
```bash
make rust-help
```

### 完整文档
查看 [BUILD_GUIDE.md](./BUILD_GUIDE.md)

---

## 故障排除

| 问题 | 解决方案 |
|------|--------|
| cargo 找不到 | 安装 Rust: https://rustup.rs/ |
| 权限被拒绝 | `chmod +x ./scripts/build.sh` |
| 某 crate 编译失败 | `python build.py <crate> --verbose` |
| 需要清理缓存 | `cargo clean` |
| Python 找不到 | 验证 `python --version` |

---

## 对比三种方法

| 特性 | Python | Shell | PowerShell |
|------|--------|-------|-----------|
| Windows | ✅ | ❌ | ✅ |
| Linux | ✅ | ✅ | ❌ |
| macOS | ✅ | ✅ | ❌ |
| 跨平台 | ✅ | ❌ | ❌ |
| 功能完整 | ✅ | ✅ | ✅ |
| **推荐** | ⭐⭐⭐ | ⭐⭐ | ⭐⭐ |

---

最后更新: 2026-06-24
