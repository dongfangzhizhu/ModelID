# modeld 构建与发布指南

本指南提供完整的项目打包和发布流程。

---

## 📦 项目结构

```
modeld/
├── python/                          # Python 包（modeld-hook）
│   ├── modeld_hook/                 # 源代码
│   ├── tests/                       # 测试
│   ├── pyproject.toml               # 包配置
│   └── README.md                    # 包说明
├── scripts/                         # 构建脚本
│   ├── build_and_publish.py         # Python 脚本（推荐）
│   ├── build_and_publish.sh         # Bash 脚本
│   ├── build_and_publish.ps1        # PowerShell 脚本
│   ├── test_publish.py              # 测试脚本
│   └── .pypirc.template             # PyPI 配置模板
├── crates/                          # Rust 核心库
│   ├── modeld-core/
│   └── modeld-cli/
├── Makefile                         # 快速命令
├── PUBLISH_GUIDE.md                 # 完整发布指南
├── QUICK_PUBLISH.md                 # 快速参考
└── BUILD_RELEASE_GUIDE.md           # 本文件
```

---

## 🚀 快速开始

### 第一次发布

```bash
# 1. 配置 PyPI 凭证
make config-pypi
# 编辑 ~/.pypirc，填入 API token

# 2. 安装依赖
make install-dev

# 3. 运行测试
make test

# 4. 构建包
make build

# 5. 模拟发布（推荐）
make publish-dry-run

# 6. 正式发布
make publish
```

### 更新版本发布

```bash
# 更新版本号
python scripts/build_and_publish.py --version 0.2.0 --publish
```

---

## 📋 三种发布方式

### 方式 1：使用 Makefile（最简单）

```bash
make publish                # 发布到 PyPI
make publish-test          # 发布到 Test PyPI
make publish-dry-run       # 模拟发布
make check                 # 发布前检查
```

### 方式 2：使用 Python 脚本（最灵活）

```bash
# 完整流程
python scripts/build_and_publish.py --publish

# 选项：
#   --version VERSION      更新版本号
#   --dry-run             模拟发布
#   --test-pypi           发布到 Test PyPI
#   --skip-tests          跳过测试
#   --clean-only          仅清理
```

### 方式 3：使用 Shell 脚本

```bash
# Bash/Linux/macOS
./scripts/build_and_publish.sh --publish
./scripts/build_and_publish.sh --publish --dry-run

# PowerShell/Windows
.\scripts\build_and_publish.ps1 -publish
.\scripts\build_and_publish.ps1 -publish -dry_run
```

---

## 📊 发布流程详解

### 1. 准备阶段

```bash
# 检查环境
python --version              # Python 3.9+
pip install build twine       # 安装工具

# 检查项目结构
ls -la python/pyproject.toml
ls -la python/modeld_hook/
```

### 2. 测试阶段

```bash
cd python

# 安装开发依赖
pip install -e ".[dev]"

# 运行测试
python -m pytest tests/ -v

# 检查代码质量
flake8 modeld_hook/
black --check modeld_hook/
```

### 3. 构建阶段

```bash
cd python

# 清理旧文件
rm -rf build/ dist/ *.egg-info/

# 构建 wheel
python -m build --wheel

# 验证包
python -m twine check dist/*.whl
```

### 4. 发布阶段

```bash
# 模拟发布（强烈推荐！）
python -m twine upload --dry-run --repository pypi dist/*

# 实际发布
python -m twine upload --repository pypi dist/*
```

### 5. 验证阶段

```bash
# 等待 PyPI 处理（通常 < 1 分钟）
# 然后访问：https://pypi.org/project/modeld-hook/

# 本地验证
pip install modeld-hook --upgrade
pip show modeld-hook
python -c "import modeld_hook; print(modeld_hook.__version__)"
```

---

## 🔐 安全最佳实践

### PyPI 凭证管理

```bash
# 1. 生成 API Token（一次性显示）
#    访问: https://pypi.org/manage/account/tokens/

# 2. 保存到 ~/.pypirc
cat > ~/.pypirc << 'EOF'
[pypi]
username = __token__
password = pypi-YOUR_TOKEN
EOF

# 3. 设置权限（重要！）
chmod 600 ~/.pypirc

# 4. 从版本控制中排除
echo ".pypirc" >> ~/.gitignore_global
```

### 最佳实践

- ✅ 使用 API Token（比用户名密码更安全）
- ✅ 为每个 package 创建独立 token
- ✅ 定期轮换 token
- ✅ 先在 Test PyPI 上测试
- ✅ 使用 `--dry-run` 验证
- ✅ 验证包签名和完整性

---

## ✅ 发布检查清单

发布前必须检查：

```bash
# 1. 版本号规范检查
grep "version = " python/pyproject.toml
# 应该是: version = "X.Y.Z"

# 2. 依赖检查
grep -A 20 "dependencies" python/pyproject.toml

# 3. 元数据检查
grep -E "name|description|author|license" python/pyproject.toml

# 4. 文件检查
ls -la python/README.md python/LICENSE

# 5. 测试检查
cd python && python -m pytest tests/ -v

# 6. 构建检查
cd python && python -m build --wheel

# 7. 包验证
cd python && python -m twine check dist/*.whl

# 8. 凭证检查
test -f ~/.pypirc && echo "✓ 凭证已配置" || echo "✗ 需要配置凭证"
```

---

## 🐛 常见问题

### Q: 如何更新版本号？

```bash
# 自动更新
python scripts/build_and_publish.py --version 0.2.0

# 手动更新
# 编辑 python/pyproject.toml 中的 version 字段
```

### Q: 如何在发布前测试？

```bash
# 1. 在 Test PyPI 上测试
python scripts/build_and_publish.py --publish --test-pypi

# 2. 验证可安装
pip install -i https://test.pypi.org/simple/ modeld-hook

# 3. 确认无误后发布到真实 PyPI
python scripts/build_and_publish.py --publish
```

### Q: 如何撤销已发布的版本？

```bash
# PyPI 不允许删除已发布版本，但可以：
# 1. 发布新版本修复问题
# 2. 在 PyPI 管理界面标记为 "Yanked"（弃用）
#    访问: https://pypi.org/project/modeld-hook/
```

### Q: Token 过期了怎么办？

```bash
# 1. 访问 https://pypi.org/manage/account/tokens/
# 2. 生成新 token
# 3. 更新 ~/.pypirc
# 4. 重新发布
```

### Q: 发布到错误的仓库怎么办？

```bash
# 可以删除本地文件并重新发布到正确仓库
cd python
rm -rf dist/
python -m build --wheel
python -m twine upload --repository pypi dist/*  # 确保用对仓库
```

---

## 📈 完整发布流程示例

### 发布 v0.2.0

```bash
# 1. 更新版本号
python scripts/build_and_publish.py --version 0.2.0

# 2. 运行完整测试
make test

# 3. 本地验证包
make build

# 4. 模拟发布到 PyPI
make publish-dry-run

# 5. 如果一切正常，发布
make publish

# 6. 验证发布成功
# 访问 https://pypi.org/project/modeld-hook/0.2.0/

# 7. Git 打标签
git tag -a v0.2.0 -m "Release v0.2.0"
git push origin v0.2.0

# 8. 创建 GitHub Release
# 访问 https://github.com/YOUR_USER/modeld/releases
```

---

## 🎯 推荐工作流

### 开发流程

```bash
# 1. 创建特性分支
git checkout -b feature/xxx

# 2. 开发并测试
make test
make lint

# 3. 提交 PR
git push origin feature/xxx

# 4. Review 和 merge
```

### 发布流程

```bash
# 1. 更新版本（进行 semantic versioning）
# 主版本.次版本.补丁版本 (e.g., 0.1.0 → 0.2.0)

# 2. 完整测试
make check
make test

# 3. 发布前验证
make publish-dry-run

# 4. 发布
make publish

# 5. 标记版本
git tag -a v0.2.0 -m "Release v0.2.0"
git push origin v0.2.0

# 6. 创建 Release Notes
# 在 GitHub 上创建 Release，总结主要变化
```

---

## 📚 相关资源

- [PyPI 官方指南](https://packaging.python.org/)
- [Twine 文档](https://twine.readthedocs.io/)
- [PEP 440 版本规范](https://peps.python.org/pep-0440/)
- [Python 打包最佳实践](https://packaging.python.org/guides/)

---

## 🆘 获取帮助

- 查看 `PUBLISH_GUIDE.md` 获取详细说明
- 查看 `QUICK_PUBLISH.md` 获取快速参考
- 运行 `make help` 查看所有可用命令
- 运行 `python scripts/test_publish.py` 进行发布前检查

---

**祝发布顺利！** 🚀
