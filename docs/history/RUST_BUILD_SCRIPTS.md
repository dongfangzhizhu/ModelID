# Rust 编译脚本系统

## 📌 概述

为 ModelID 项目创建了一套完整的、跨平台的 Rust crates 编译脚本系统。

### 支持的平台

| 平台 | 脚本 | 状态 |
|------|------|------|
| Windows | `build.ps1` (PowerShell) | ✅ |
| Windows | `build.py` (Python) | ✅ |
| Linux | `build.sh` (Bash) | ✅ |
| Linux | `build.py` (Python) | ✅ |
| macOS | `build.sh` (Bash) | ✅ |
| macOS | `build.py` (Python) | ✅ |

## 📂 创建的文件

### 编译脚本

```
scripts/
├── build.py          # Python 脚本 (推荐，跨平台)
├── build.sh          # Shell 脚本 (Linux/macOS)
├── build.ps1         # PowerShell 脚本 (Windows)
└── ...
```

### 文档

```
scripts/
├── README.md                        # 快速入门指南
├── BUILD_GUIDE.md                   # 详细使用指南
├── QUICK_BUILD_REFERENCE.md         # 命令速查表
└── ...
```

### 项目根目录

```
ModelID/
├── RUST_BUILD_SCRIPTS.md   # 本文件 - 功能总结
├── Makefile                # 已更新，包含 Rust 目标
└── scripts/                # 编译脚本目录
```

## 🎯 核心功能

### 1. 编译 (Build)

```bash
# 编译所有 crates (debug 模式)
python build.py

# 编译所有 crates (release 模式 - 优化)
python build.py --release

# 编译特定 crate
python build.py modeld-core
python build.py modeld-cli modeld-proxy
```

### 2. 检查 (Check)

```bash
# 检查代码语法 (不生成二进制)
python build.py --check

# 快速验证，比编译快
```

### 3. 测试 (Test)

```bash
# 运行所有 crates 的测试
python build.py --test

# 测试特定 crate
python build.py modeld-core --test
```

### 4. 代码格式化 (Format)

```bash
# 格式化所有 crates (使用 rustfmt)
python build.py --format

# Python 脚本独有功能
```

### 5. 代码检查/Lint (Lint)

```bash
# 运行 clippy 代码检查
python build.py --lint

# 详细输出
python build.py --lint --verbose
```

### 6. 详细输出 (Verbose)

```bash
# 显示详细的编译信息
python build.py --verbose
python build.py --release --verbose
```

## 📊 可用的 Crates

项目定义了 5 个 Rust crates（在 `Cargo.toml` 中）：

1. **modeld-core** - 核心库
   - Blake3 哈希算法实现
   - SQLite 数据库支持
   - 核心数据结构

2. **modeld-cli** - 命令行工具
   - CLI 应用程序
   - 命令行界面

3. **modeld-proxy** - 代理服务器
   - HTTP 代理功能
   - 小型 HTTP 服务器

4. **modeld-client** - 客户端库
   - 客户端实现
   - RESTful API 集成

5. **modeld-webui** - Web UI 服务器
   - Web 界面
   - Axum 框架
   - WebSocket 支持

## 🚀 快速使用

### 最简单的用法

```bash
cd ModelID
python scripts/build.py
```

### 通过 Makefile

```bash
make rust-build              # 编译所有
make rust-build-release      # release 编译
make rust-test               # 运行测试
make rust-lint               # Lint 检查
make rust-help               # 显示帮助
```

### Shell 命令 (Linux/macOS)

```bash
./scripts/build.sh
./scripts/build.sh --release
./scripts/build.sh --help
```

### PowerShell 命令 (Windows)

```powershell
.\scripts\build.ps1
.\scripts\build.ps1 -Release
.\scripts\build.ps1 -Help
```

## 📖 文档结构

### 📄 scripts/README.md
- 快速入门指南
- 文件说明
- 脚本选择建议

### 📄 scripts/BUILD_GUIDE.md
- 完整的使用指南
- 所有选项详解
- 常见场景示例
- 前置要求说明
- 故障排除
- CI/CD 集成示例

### 📄 scripts/QUICK_BUILD_REFERENCE.md
- 命令速查表
- Crates 列表
- 常见场景速查
- 对比三种方法
- 快速故障排除

### 📄 RUST_BUILD_SCRIPTS.md (本文件)
- 功能总结
- 脚本说明
- 核心特性
- 使用示例

## ⚙️ 脚本特性

### Python 脚本 (build.py) - 推荐

**优点：**
- ✅ 跨平台支持 (Windows/Linux/macOS)
- ✅ 功能最完整
  - 编译、检查、测试、格式化、lint
  - 多个 crates 支持
  - Release/Debug 模式
  - 详细和简洁输出选项
- ✅ 彩色输出（Windows 兼容处理）
- ✅ 友好的错误信息
- ✅ 编译后详细统计
- ✅ 平台检测和自动适配

**依赖：**
- Python 3.6+
- Rust toolchain

### Shell 脚本 (build.sh) - Linux/macOS

**优点：**
- ✅ 原生 shell，无额外依赖（除 Rust）
- ✅ 快速轻量级
- ✅ 完整功能（除格式化、lint）
- ✅ 彩色输出
- ✅ 详细的帮助文本

**依赖：**
- Bash shell
- Rust toolchain

### PowerShell 脚本 (build.ps1) - Windows

**优点：**
- ✅ 原生 PowerShell
- ✅ Windows 系统友好
- ✅ 完整功能（除格式化、lint）
- ✅ 彩色输出
- ✅ 参数式调用

**依赖：**
- PowerShell 5.0+
- Rust toolchain

## 💻 使用示例

### 示例 1：首次编译

```bash
python build.py
# 输出：
# [INFO] Starting build process...
# [INFO] Build type: debug
# [INFO] Crates to build: modeld-core, modeld-cli, modeld-proxy, modeld-client, modeld-webui
# 
# [INFO] Building modeld-core...
# [SUCCESS] Built modeld-core successfully
# ...
# [SUCCESS] All crates completed successfully!
```

### 示例 2：发布准备

```bash
# 1. 检查代码
python build.py --check

# 2. 格式化
python build.py --format

# 3. Lint
python build.py --lint

# 4. Release 编译
python build.py --release

# 5. 测试
python build.py --test
```

### 示例 3：特定 crate 开发

```bash
# 编译并测试 modeld-cli
python build.py modeld-cli --test --verbose

# 输出详细的编译和测试信息
```

### 示例 4：Makefile 使用

```bash
# 编译
make rust-build

# Release 模式
make rust-build-release

# 特定 crate
make rust-build-crate CRATE=modeld-core

# 测试
make rust-test

# Lint
make rust-lint

# 显示所有 Rust 命令
make rust-help
```

## 🔧 集成方式

### 1. 直接运行脚本

```bash
python scripts/build.py
```

### 2. 通过 Makefile

```bash
make rust-build
```

### 3. 在 CI/CD 中

GitHub Actions 示例：
```yaml
- name: Build Rust
  run: python scripts/build.py --release
```

### 4. 使用 cargo 直接

所有脚本最终都调用 `cargo` 命令，所以也可以：
```bash
cargo build --release
cargo test
cargo check
```

## 📋 前置要求

### 必需

1. **Rust 工具链**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
   
   验证：
   ```bash
   cargo --version
   rustc --version
   ```

### 可选

1. **Python 3.6+** (如果使用 Python 脚本)
   ```bash
   python --version
   ```

2. **rustfmt** (通常包含在 Rust 中)
   - 用于 `--format` 功能

3. **clippy** (通常包含在 Rust 中)
   - 用于 `--lint` 功能

## 🐛 常见问题

### Q: 选择哪个脚本？

**A:** 优先使用 Python 脚本 (`build.py`)，它跨平台且功能最完整。

| 场景 | 推荐 |
|------|------|
| 所有用户 | `python build.py` ⭐⭐⭐ |
| Linux/macOS 用户 | `./build.sh` ⭐⭐ |
| Windows 用户 | `.\build.ps1` ⭐⭐ |
| Makefile 用户 | `make rust-build` ⭐⭐⭐ |

### Q: cargo 找不到

**A:** 安装 Rust
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Q: 脚本权限被拒绝 (Linux/macOS)

**A:** 添加执行权限
```bash
chmod +x ./scripts/build.sh
```

### Q: 编译失败

**A:** 使用 --verbose 获取详细信息
```bash
python build.py --verbose
```

### Q: 特定 crate 编译失败

**A:** 单独编译该 crate
```bash
python build.py modeld-core --verbose
```

## 🎓 学习资源

- [Cargo 官方文档](https://doc.rust-lang.org/cargo/)
- [Rust 编程语言](https://www.rust-lang.org/)
- [项目 README](./README.md)
- [构建指南](./scripts/BUILD_GUIDE.md)
- [快速参考](./scripts/QUICK_BUILD_REFERENCE.md)

## 📝 更新日志

### 2026-06-24

创建完整的 Rust 编译脚本系统：

**创建的文件：**
- `scripts/build.py` - Python 编译脚本
- `scripts/build.sh` - Shell 编译脚本
- `scripts/build.ps1` - PowerShell 编译脚本
- `scripts/README.md` - 快速入门
- `scripts/BUILD_GUIDE.md` - 详细指南
- `scripts/QUICK_BUILD_REFERENCE.md` - 快速参考
- `RUST_BUILD_SCRIPTS.md` - 本文件

**功能：**
- ✅ 编译所有或特定 crates
- ✅ Debug 和 Release 模式
- ✅ 代码检查 (cargo check)
- ✅ 运行测试 (cargo test)
- ✅ 代码格式化 (cargo fmt)
- ✅ 代码检查 (cargo clippy)
- ✅ 详细统计和报告
- ✅ 跨平台支持

**更新的文件：**
- `Makefile` - 添加了 Rust 编译目标

## 🎯 下一步

1. **快速开始：** 运行 `python scripts/build.py`
2. **了解更多：** 查看 `scripts/README.md`
3. **完整指南：** 查看 `scripts/BUILD_GUIDE.md`
4. **命令速查：** 查看 `scripts/QUICK_BUILD_REFERENCE.md`

## 📞 支持

如有问题，请：
1. 查看文档中的"故障排除"部分
2. 运行 `python build.py --help` 获取帮助
3. 查看 Rust 官方文档

---

**文件位置：** `ModelID/scripts/`

**主要入口：** `scripts/build.py`

**文档入口：** `scripts/README.md`

**最后更新：** 2026-06-24
