# Build Guide for ModelID Rust Projects

这个指南介绍如何使用跨平台编译脚本来编译 ModelID 项目中的 Rust crates。

## 概述

项目包含以下 5 个 crates：
- `modeld-core` - 核心库
- `modeld-cli` - 命令行工具
- `modeld-proxy` - 代理服务器
- `modeld-client` - 客户端库
- `modeld-webui` - Web UI 服务器

## 快速开始

### 方案 1：Python 脚本（推荐）

Python 脚本在 Windows 和 Linux/macOS 上都能使用。

```bash
# Linux/macOS
python build.py

# Windows (PowerShell)
python build.py

# Windows (CMD)
python build.py
```

### 方案 2：Shell 脚本（Linux/macOS）

```bash
chmod +x ./scripts/build.sh
./scripts/build.sh
```

### 方案 3：PowerShell 脚本（Windows）

```powershell
.\scripts\build.ps1
```

## 使用方法

### Python 脚本 (`build.py`)

#### 基础命令

```bash
# 编译所有 crates (debug 模式)
python build.py

# 编译所有 crates (release 模式)
python build.py --release

# 编译特定 crate
python build.py modeld-core
python build.py modeld-cli modeld-proxy

# 检查代码（不编译）
python build.py --check

# 运行测试
python build.py --test

# 格式化代码
python build.py --format

# 运行 clippy 代码检查
python build.py --lint
```

#### 高级选项

```bash
# 使用详细输出
python build.py --verbose

# Release 模式 + lint
python build.py --release --lint

# 特定 crate + 测试
python build.py modeld-core --test
```

#### 完整帮助

```bash
python build.py --help
```

### Shell 脚本 (`build.sh`) - Linux/macOS

#### 基础命令

```bash
# 编译所有 crates
./scripts/build.sh

# 编译所有 crates (release 模式)
./scripts/build.sh --release

# 编译特定 crate
./scripts/build.sh modeld-core
./scripts/build.sh modeld-core modeld-cli

# 检查代码
./scripts/build.sh --check

# 运行测试
./scripts/build.sh --test
```

#### 高级选项

```bash
# 使用详细输出
./scripts/build.sh --verbose

# Release 模式 + verbose
./scripts/build.sh --release --verbose

# 特定 crate + 测试
./scripts/build.sh --test modeld-core
```

#### 完整帮助

```bash
./scripts/build.sh --help
```

### PowerShell 脚本 (`build.ps1`) - Windows

#### 基础命令

```powershell
# 编译所有 crates
.\scripts\build.ps1

# 编译所有 crates (release 模式)
.\scripts\build.ps1 -Release

# 编译特定 crate
.\scripts\build.ps1 modeld-core
.\scripts\build.ps1 modeld-core modeld-cli

# 检查代码
.\scripts\build.ps1 -Check

# 运行测试
.\scripts\build.ps1 -Test
```

#### 高级选项

```powershell
# 使用详细输出
.\scripts\build.ps1 -Verbose

# Release 模式 + verbose
.\scripts\build.ps1 -Release -Verbose

# 特定 crate + 测试
.\scripts\build.ps1 -Test modeld-core
```

#### 完整帮助

```powershell
.\scripts\build.ps1 -Help
```

## 常见使用场景

### 场景 1：开发调试

编译所有 crates (debug 模式，最快)：

```bash
# Python
python build.py

# Shell (Linux/macOS)
./scripts/build.sh

# PowerShell (Windows)
.\scripts\build.ps1
```

### 场景 2：发布构建

编译所有 crates (release 模式，优化)：

```bash
# Python
python build.py --release

# Shell (Linux/macOS)
./scripts/build.sh --release

# PowerShell (Windows)
.\scripts\build.ps1 -Release
```

### 场景 3：开发特定功能

编译特定 crate：

```bash
# Python
python build.py modeld-cli

# Shell (Linux/macOS)
./scripts/build.sh modeld-cli

# PowerShell (Windows)
.\scripts\build.ps1 modeld-cli
```

### 场景 4：代码检查

编译前检查代码：

```bash
# Python
python build.py --check

# Shell (Linux/macOS)
./scripts/build.sh --check

# PowerShell (Windows)
.\scripts\build.ps1 -Check
```

### 场景 5：运行测试

```bash
# Python
python build.py --test

# Shell (Linux/macOS)
./scripts/build.sh --test

# PowerShell (Windows)
.\scripts\build.ps1 -Test
```

### 场景 6：代码质量检查

```bash
# 格式化
python build.py --format

# Lint 检查
python build.py --lint

# Lint + 详细输出
python build.py --lint --verbose
```

## 输出示例

编译成功时的输出：

```
[INFO] Starting build process...
[INFO] Build type: debug
[INFO] Crates to process: modeld-core, modeld-cli, modeld-proxy, modeld-client, modeld-webui

[INFO] Building modeld-core...
[SUCCESS] Built modeld-core successfully

[INFO] Building modeld-cli...
[SUCCESS] Built modeld-cli successfully

...

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
[INFO] Build Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
[SUCCESS] Successful: 5 crate(s)
  ✓ modeld-core
  ✓ modeld-cli
  ✓ modeld-proxy
  ✓ modeld-client
  ✓ modeld-webui

[SUCCESS] All crates completed successfully!
```

## 前置要求

### 必需

- **Rust 工具链** - 安装 [Rust](https://www.rust-lang.org/tools/install)
  
  验证安装：
  ```bash
  cargo --version
  rustc --version
  ```

### Python 脚本额外要求

- **Python 3.6+** - 用于运行 Python 脚本
  
  验证安装：
  ```bash
  python --version
  # 或
  python3 --version
  ```

### 可选

- **rustfmt** - 用于 `--format` 选项 (通常随 Rust 安装)
- **clippy** - 用于 `--lint` 选项 (通常随 Rust 安装)

## 故障排除

### 问题：找不到 cargo 命令

**解决方案：**
- 确保 Rust 已安装
- 检查 PATH 环境变量是否包含 Rust 的 bin 目录
- 重启终端或 IDE

### 问题：权限被拒绝 (Linux/macOS)

**解决方案：**
```bash
chmod +x ./scripts/build.sh
```

### 问题：Python 脚本找不到

**解决方案：**
- 确保 Python 已安装：`python --version`
- 在项目根目录运行脚本
- 在 Windows 上，使用 `python build.py` 而不是 `python3`

### 问题：某个 crate 编译失败

**解决方案：**
1. 使用 `--verbose` 查看详细错误信息
2. 检查该 crate 的依赖是否最新：`cargo update`
3. 清理构建缓存：`cargo clean`
4. 单独编译失败的 crate：`python build.py <crate-name>`

## 脚本对比

| 功能 | Python | Shell | PowerShell |
|------|--------|-------|-----------|
| 支持平台 | Windows, Linux, macOS | Linux, macOS | Windows |
| 易用性 | 高（跨平台） | 中 | 中 |
| 功能 | 最全面 | 标准 | 标准 |
| 依赖 | Python 3.6+ | Bash | PowerShell 5+ |
| 推荐 | ✅ | ✅ | ✅ |

## CI/CD 集成

### GitHub Actions 示例

```yaml
name: Build

on: [push, pull_request]

jobs:
  build:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest, macos-latest]
    
    steps:
      - uses: actions/checkout@v3
      
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      
      - name: Build (Linux/macOS)
        if: runner.os != 'Windows'
        run: ./scripts/build.sh --release
      
      - name: Build (Windows)
        if: runner.os == 'Windows'
        run: .\scripts\build.ps1 -Release
```

## 更多信息

- [Cargo 官方文档](https://doc.rust-lang.org/cargo/)
- [Rust 工程手册](https://www.rust-lang.org/what/wg-cargo/)
- [项目 README](../README.md)
