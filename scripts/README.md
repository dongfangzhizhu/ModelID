# ModelID 编译脚本

这个目录包含用于编译 ModelID Rust 项目的跨平台脚本。

## 📋 文件说明

### 编译脚本（选择其中一个）

| 文件 | 平台 | 语言 | 说明 |
|------|------|------|------|
| **build.py** | ✅ Windows ✅ Linux ✅ macOS | Python | 推荐使用，功能最完整，跨平台 |
| **build.sh** | ❌ Windows ✅ Linux ✅ macOS | Bash/Shell | Linux/macOS 用户可用 |
| **build.ps1** | ✅ Windows ❌ Linux ❌ macOS | PowerShell | Windows 用户可用 |

### 文档

| 文件 | 说明 |
|------|------|
| **BUILD_GUIDE.md** | 详细的使用指南，包含所有功能说明和示例 |
| **QUICK_BUILD_REFERENCE.md** | 快速参考卡片，命令速查表 |
| **README.md** | 本文件 |

## 🚀 快速开始

### 第 1 步：确保已安装 Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# 或访问 https://www.rust-lang.org/tools/install
```

验证安装：
```bash
cargo --version
rustc --version
```

### 第 2 步：运行编译脚本

#### 方式 A：Python（推荐，所有平台）

```bash
# 编译所有 crates (调试模式)
python build.py

# 或更详细的用法
python build.py --help
```

#### 方式 B：Shell（Linux/macOS）

```bash
chmod +x ./build.sh
./build.sh --help
```

#### 方式 C：PowerShell（Windows）

```powershell
.\build.ps1 -Help
```

#### 方式 D：Makefile（所有平台）

```bash
make rust-build      # 编译
make rust-test       # 测试
make rust-lint       # lint 检查
make rust-help       # 显示所有命令
```

## 📦 Crates 列表

项目包含以下 5 个 Rust crates：

1. **modeld-core** - 核心库
   - 包含主要的数据结构和算法
   - 其他 crates 的依赖

2. **modeld-cli** - 命令行工具
   - CLI 应用程序

3. **modeld-proxy** - 代理服务器
   - HTTP/代理功能

4. **modeld-client** - 客户端库
   - 客户端实现

5. **modeld-webui** - Web UI 服务器
   - Web 界面和 API 服务

## 💡 常见用法示例

### 开发调试

```bash
# 编译所有 crates (最快的调试构建)
python build.py

# 只检查代码，不编译
python build.py --check

# 运行所有测试
python build.py --test
```

### 发布构建

```bash
# 编译所有 crates (优化的发布构建)
python build.py --release

# 发布前进行 lint 检查
python build.py --lint --release
```

### 针对特定 crate

```bash
# 只编译某个 crate
python build.py modeld-core

# 编译并测试某个 crate
python build.py modeld-cli --test

# 编译多个 crates
python build.py modeld-core modeld-cli modeld-proxy
```

### 代码质量

```bash
# 格式化代码
python build.py --format

# 运行 clippy linter
python build.py --lint

# 详细的 lint 输出
python build.py --lint --verbose
```

## 🔍 详细文档

- **[BUILD_GUIDE.md](./BUILD_GUIDE.md)** - 完整的使用指南
  - 所有选项说明
  - 详细的使用场景
  - 故障排除
  - CI/CD 集成示例

- **[QUICK_BUILD_REFERENCE.md](./QUICK_BUILD_REFERENCE.md)** - 快速参考
  - 命令速查表
  - 常见场景
  - 快速故障排除

## ⚙️ 脚本功能对比

| 功能 | Python | Shell | PowerShell |
|------|--------|-------|-----------|
| 编译 (debug) | ✅ | ✅ | ✅ |
| 编译 (release) | ✅ | ✅ | ✅ |
| 编译特定 crate | ✅ | ✅ | ✅ |
| 检查代码 | ✅ | ✅ | ✅ |
| 运行测试 | ✅ | ✅ | ✅ |
| 格式化代码 | ✅ | ❌ | ❌ |
| Lint 检查 | ✅ | ❌ | ❌ |
| 跨平台 | ✅ | ❌ | ❌ |
| 颜色输出 | ✅ | ✅ | ✅ |
| 详细统计 | ✅ | ✅ | ✅ |

## 🛠️ 选择哪个脚本？

### 推荐：Python 脚本 (`build.py`)

**优点：**
- ✅ 跨平台 (Windows, Linux, macOS)
- ✅ 功能最完整（格式化、lint 等）
- ✅ 易于使用
- ✅ 详细的帮助信息

**要求：**
- Python 3.6+

### Linux/macOS 用户：Shell 脚本 (`build.sh`)

**优点：**
- ✅ 原生 shell，无额外依赖
- ✅ 功能完整

**要求：**
- Bash shell
- 需要 chmod +x

### Windows 用户：PowerShell 脚本 (`build.ps1`)

**优点：**
- ✅ 原生 PowerShell
- ✅ Windows 友好

**要求：**
- PowerShell 5.0+

## 📝 使用示例

### 示例 1：第一次编译

```bash
python build.py
# 或
make rust-build
```

### 示例 2：开发工作流

```bash
# 1. 检查代码
python build.py --check

# 2. 格式化
python build.py --format

# 3. Lint 检查
python build.py --lint

# 4. 编译
python build.py

# 5. 测试
python build.py --test
```

### 示例 3：发布流程

```bash
# 1. 更新版本（在 Cargo.toml 中）
# 2. 编译 release 版本
python build.py --release

# 3. Lint 检查
python build.py --lint

# 4. 测试
python build.py --test

# 5. 格式化
python build.py --format
```

### 示例 4：特定 crate 开发

```bash
# 只编译和测试 modeld-cli
python build.py modeld-cli --test --verbose
```

## ⚡ Makefile 快捷方式

如果你熟悉 Makefile，可以使用这些命令：

```bash
make rust-build              # 编译所有 (debug)
make rust-build-release      # 编译所有 (release)
make rust-test               # 测试所有
make rust-check              # 检查所有
make rust-fmt                # 格式化
make rust-lint               # Lint 检查
make rust-build-crate CRATE=modeld-core  # 编译特定 crate
```

## 🐛 常见问题

### Q: cargo 命令找不到

**A:** 确保已安装 Rust
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Q: 脚本执行被拒绝 (Linux/macOS)

**A:** 需要添加执行权限
```bash
chmod +x ./build.sh
```

### Q: Python 脚本找不到

**A:** 确保 Python 已安装
```bash
python --version
# 或
python3 --version
```

### Q: 某个 crate 编译失败

**A:** 使用 --verbose 获取详细错误信息
```bash
python build.py <crate-name> --verbose
```

或查看 [BUILD_GUIDE.md](./BUILD_GUIDE.md) 中的故障排除部分。

## 📚 更多信息

- [Cargo 官方文档](https://doc.rust-lang.org/cargo/)
- [Rust 工程手册](https://www.rust-lang.org/what/wg-cargo/)
- [项目 README](../README.md)

## 🎯 下一步

1. 查看 [QUICK_BUILD_REFERENCE.md](./QUICK_BUILD_REFERENCE.md) 了解常用命令
2. 阅读 [BUILD_GUIDE.md](./BUILD_GUIDE.md) 获取完整说明
3. 运行 `python build.py --help` 查看所有选项
4. 开始开发！

---

**最后更新**: 2026-06-24

**作者**: ModelID 开发团队

**许可证**: MIT
