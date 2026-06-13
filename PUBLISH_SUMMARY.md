# PyPI 发布工作流 - 完成总结

已为 modeld-hook Python 包创建了完整的一键发布系统。

---

## 📦 文件清单

### 脚本文件

| 文件 | 说明 | 平台 |
|------|------|------|
| `scripts/build_and_publish.py` | 主要发布脚本（推荐）| 全平台 ✅ |
| `scripts/build_and_publish.sh` | Bash 脚本 | Linux/macOS ✅ |
| `scripts/build_and_publish.ps1` | PowerShell 脚本 | Windows ✅ |
| `scripts/test_publish.py` | 发布前测试脚本 | 全平台 ✅ |
| `scripts/.pypirc.template` | PyPI 配置模板 | - |

### 文档文件

| 文件 | 说明 |
|------|------|
| `PUBLISH_GUIDE.md` | 完整发布指南（200+ 行） |
| `QUICK_PUBLISH.md` | 快速参考卡片 |
| `BUILD_RELEASE_GUIDE.md` | 构建发布完整说明 |
| `PUBLISH_SUMMARY.md` | 本文件 |

### 配置文件

| 文件 | 说明 |
|------|------|
| `Makefile` | 快速命令（开发和发布）|
| `.gitignore.publish` | 发布相关的 git 排除规则 |

---

## 🎯 核心功能

### Python 脚本特性

```
✓ 完整的构建流程（清理→测试→构建→验证→发布）
✓ 灵活的选项系统
  - --publish          发布到 PyPI
  - --dry-run          模拟发布（推荐先做）
  - --version VERSION  自动更新版本号
  - --test-pypi        发布到 Test PyPI（测试）
  - --skip-tests       跳过测试（快速发布）
  - --clean-only       仅清理旧文件

✓ 自动依赖安装（build, twine）
✓ 自动测试运行
✓ 包完整性验证
✓ 清晰的进度显示和错误提示
✓ 安全的发布确认
✓ 凭证自动检测
```

---

## ⚡ 使用方式

### 快速发布（推荐）

#### 方式 1：Makefile（最简单）

```bash
# 配置 PyPI 凭证
make config-pypi

# 发布
make publish

# 或者试运行
make publish-dry-run
```

#### 方式 2：Python 脚本

```bash
# 完整流程（推荐）
python scripts/build_and_publish.py --publish

# 模拟发布（先做这个验证）
python scripts/build_and_publish.py --publish --dry-run

# 更新版本并发布
python scripts/build_and_publish.py --version 0.2.0 --publish

# 发布到 Test PyPI（测试）
python scripts/build_and_publish.py --publish --test-pypi
```

#### 方式 3：Bash 脚本（Linux/macOS）

```bash
./scripts/build_and_publish.sh --publish
./scripts/build_and_publish.sh --publish --dry-run
```

#### 方式 4：PowerShell 脚本（Windows）

```powershell
.\scripts\build_and_publish.ps1 -publish
.\scripts\build_and_publish.ps1 -publish -dry_run
```

---

## 🔧 Makefile 命令大全

```bash
make help              # 显示帮助
make install-dev       # 安装开发依赖
make test              # 运行测试
make lint              # 代码检查
make build             # 构建 wheel
make clean             # 清理文件
make publish           # 发布到 PyPI
make publish-test      # 发布到 Test PyPI
make publish-dry-run   # 模拟发布
make check             # 发布前检查清单
make config-pypi       # 配置 PyPI 凭证
```

---

## 📋 首次使用步骤

### 1. 配置 PyPI 凭证（一次性）

```bash
# 方式 1：使用 Makefile（推荐）
make config-pypi

# 方式 2：手动配置
# a) 访问 https://pypi.org/manage/account/tokens/
# b) 创建新 API token
# c) 复制 token
# d) 编辑 ~/.pypirc，填入 token
# e) 设置权限：chmod 600 ~/.pypirc
```

### 2. 安装依赖

```bash
make install-dev
# 或
python -m pip install build twine
```

### 3. 运行测试

```bash
make test
```

### 4. 构建包

```bash
make build
```

### 5. 模拟发布（强烈推荐！）

```bash
make publish-dry-run
```

### 6. 实际发布

```bash
make publish
```

---

## 🚀 发布流程详解

### 完整流程（Python 脚本）

```
1. 环境检查
   └─ Python 版本 ✓
   └─ build 工具 ✓
   └─ twine 工具 ✓

2. 版本更新（可选）
   └─ 更新 pyproject.toml

3. 测试阶段
   └─ pip install -e ".[dev]"
   └─ pytest tests/

4. 清理阶段
   └─ 删除 build/
   └─ 删除 dist/
   └─ 删除 *.egg-info/

5. 构建阶段
   └─ python -m build --wheel

6. 验证阶段
   └─ twine check dist/*.whl

7. 发布阶段（可选）
   └─ 模拟：twine upload --dry-run
   └─ 实际：twine upload --repository pypi
```

---

## ✅ 发布检查清单

```bash
# 1. 环境检查
python --version                        # Python 3.9+
pip show build twine                   # 工具已安装

# 2. 版本检查
grep "version = " python/pyproject.toml # 版本号规范 X.Y.Z

# 3. 文件检查
ls python/README.md                    # 说明文档
ls python/LICENSE                      # 许可证

# 4. 测试检查
cd python && python -m pytest tests/ -v # 所有测试通过

# 5. 包检查
cd python && python -m build --wheel    # 构建成功
cd python && python -m twine check dist/*.whl  # 包有效

# 6. 凭证检查
test -f ~/.pypirc                       # 凭证已配置
cat ~/.pypirc | grep password           # token 已填入

# 7. 发布验证
python scripts/build_and_publish.py --publish --dry-run  # 模拟成功
```

---

## 📊 脚本对比

| 特性 | Python | Bash | PowerShell | 手动 |
|------|--------|------|-----------|------|
| 跨平台 | ✅ | ❌ | ❌ | ✅ |
| 易用性 | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐ | ⭐ |
| 功能完整性 | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ✅ |
| 自动化程度 | 最高 | 高 | 高 | 低 |
| 推荐指数 | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐ | 学习 |

**结论**: 推荐使用 Python 脚本或 Makefile

---

## 🎓 工作流示例

### 场景 1：首次发布 v0.1.0

```bash
# 1. 检查一切准备就绪
make check

# 2. 配置 PyPI
make config-pypi
# 填入 token

# 3. 发布
make publish
```

### 场景 2：发布新版本 v0.2.0

```bash
# 1. 更新版本
python scripts/build_and_publish.py --version 0.2.0

# 2. 验证
make test
make publish-dry-run

# 3. 发布
make publish

# 4. 打 git 标签
git tag -a v0.2.0 -m "Release v0.2.0"
git push origin v0.2.0
```

### 场景 3：在 Test PyPI 上测试

```bash
# 1. 在 Test PyPI 上发布
make publish-test

# 2. 验证可安装
pip install -i https://test.pypi.org/simple/ modeld-hook

# 3. 检查版本
pip show modeld-hook
```

### 场景 4：快速发布（跳过测试）

```bash
# 用于已验证的小更新
python scripts/build_and_publish.py --publish --skip-tests
```

---

## 🐛 常见问题快速解决

| 问题 | 原因 | 解决 |
|------|------|------|
| "401 Unauthorized" | token 错误 | 重新生成 token，更新 ~/.pypirc |
| "Package already exists" | 版本号重复 | 增加版本号重试 |
| "Tests failed" | 测试未通过 | 修复测试代码 |
| "Module not found" | 依赖缺失 | 运行 `make install-dev` |
| "Permission denied" | 权限问题 | `chmod 600 ~/.pypirc` |

详见 `PUBLISH_GUIDE.md` 获取完整故障排除

---

## 📚 相关文档

| 文档 | 内容 |
|------|------|
| `QUICK_PUBLISH.md` | 快速参考卡片（复制粘贴命令） |
| `PUBLISH_GUIDE.md` | 完整指南（详细说明和原理） |
| `BUILD_RELEASE_GUIDE.md` | 构建发布完整流程 |

---

## ✨ 特色功能

### 1. 智能依赖管理
```bash
# 自动安装缺失的工具
build, twine 不存在时自动安装
```

### 2. 灵活的版本控制
```bash
# 一行命令更新版本并发布
python scripts/build_and_publish.py --version 0.2.0 --publish
```

### 3. 安全的发布流程
```bash
# 强制运行测试
pytest 必须通过才能继续

# 包验证
twine 自动检查包完整性

# 模拟发布
--dry-run 让你在实际上传前验证
```

### 4. 清晰的反馈
```bash
# 彩色输出，清晰的状态提示
✓ 成功  ✗ 失败  ⚠ 警告  → 信息
```

### 5. 完整的错误处理
```bash
# 任何失败都会:
1. 显示清晰的错误消息
2. 提供解决建议
3. 安全退出（不泄露数据）
```

---

## 🎉 总结

现在你有了一套完整的 PyPI 发布系统：

✅ **多语言支持**: Python、Bash、PowerShell  
✅ **多种方式**: 脚本、Makefile、手动命令  
✅ **完整文档**: 快速参考、详细指南、最佳实践  
✅ **自动化**: 测试、构建、验证、发布全流程  
✅ **安全性**: token 管理、权限检查、确认提示  
✅ **易用性**: 一键发布、智能选项、清晰反馈  

---

## 🚀 立即开始

```bash
# 第一次使用
make config-pypi
make publish

# 就这么简单！
```

---

**Happy Publishing! 🎉**

相关链接：
- [PyPI 官网](https://pypi.org/)
- [Python 打包指南](https://packaging.python.org/)
- [Twine 文档](https://twine.readthedocs.io/)
