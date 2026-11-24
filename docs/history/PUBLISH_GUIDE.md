# modeld-hook PyPI 发布指南

## 📋 前置准备

### 1. 创建 PyPI 账户

1. 访问 [PyPI 注册页面](https://pypi.org/account/register/)
2. 填写账户信息（邮箱必须有效）
3. 验证邮箱
4. 登录账户

### 2. 创建 API Token

1. 登录 PyPI 后，访问 [API Token 管理页面](https://pypi.org/manage/account/tokens/)
2. 点击 "Create new API token"
3. 为 token 命名（例如：`modeld-hook-publish`）
4. **作用域选择**: "Entire account" 或 "Only for modeld-hook"
5. **复制 token**（格式：`pypi-AgEIcHl...`）

> ⚠️ **重要**: Token 只会显示一次，请妥善保存！

### 3. 配置 ~/.pypirc

创建或编辑 `~/.pypirc` 文件（Windows 上是 `%APPDATA%\.pypirc`）：

```ini
[distutils]
index-servers =
    pypi
    testpypi

[pypi]
repository = https://upload.pypi.org/legacy/
username = __token__
password = pypi-YOUR_TOKEN_HERE

[testpypi]
repository = https://test.pypi.org/legacy/
username = __token__
password = pypi-YOUR_TEST_TOKEN_HERE
```

**设置文件权限**（重要！）：

```bash
# Linux/macOS
chmod 600 ~/.pypirc

# Windows PowerShell
icacls "$env:APPDATA\.pypirc" /inheritance:r /grant:r "$env:USERNAME`:F"
```

---

## 🚀 发布步骤

### 方式 1：使用 Python 脚本（推荐）

#### 第一次发布前的测试

```bash
# 1. 清理并构建
python scripts/build_and_publish.py --clean-only

# 2. 构建 wheel 包
python scripts/build_and_publish.py

# 3. 在 Test PyPI 上测试发布（推荐先做这个）
python scripts/build_and_publish.py --publish --test-pypi --dry-run

# 4. 实际发布到 Test PyPI
python scripts/build_and_publish.py --publish --test-pypi
```

#### 正式发布到 PyPI

```bash
# 1. 更新版本号
python scripts/build_and_publish.py --version 0.2.0

# 2. 模拟发布（验证没问题）
python scripts/build_and_publish.py --publish --dry-run

# 3. 实际发布到 PyPI
python scripts/build_and_publish.py --publish
```

### 方式 2：使用 PowerShell 脚本（Windows）

```powershell
# 构建
.\scripts\build_and_publish.ps1

# 模拟发布
.\scripts\build_and_publish.ps1 -publish -dry_run

# 实际发布
.\scripts\build_and_publish.ps1 -publish

# 更新版本并发布
.\scripts\build_and_publish.ps1 -version "0.2.0" -publish
```

### 方式 3：使用 Bash 脚本（Linux/macOS）

```bash
# 构建
./scripts/build_and_publish.sh

# 模拟发布
./scripts/build_and_publish.sh --publish --dry-run

# 实际发布
./scripts/build_and_publish.sh --publish

# 更新版本并发布
./scripts/build_and_publish.sh --version "0.2.0" --publish
```

### 方式 4：手动命令

```bash
cd python

# 1. 安装依赖
pip install build twine

# 2. 清理旧文件
rm -rf build dist *.egg-info

# 3. 构建包
python -m build --wheel

# 4. 验证包
python -m twine check dist/*.whl

# 5. 模拟上传（推荐先做这个）
python -m twine upload --dry-run --repository pypi dist/*

# 6. 实际上传
python -m twine upload --repository pypi dist/*
```

---

## ✅ 验证发布

### 本地验证

```bash
# 从本地 wheel 文件安装
pip install dist/modeld_hook-*.whl

# 测试导入
python -c "import modeld_hook; print(modeld_hook.__version__)"
```

### PyPI 验证

发布后，访问以下 URL 验证：

- **PyPI**: https://pypi.org/project/modeld-hook/
- **Test PyPI**: https://test.pypi.org/project/modeld-hook/

### 安装验证

```bash
# 用户可以这样安装
pip install modeld-hook

# 查看版本
pip show modeld-hook

# 测试
python -c "import modeld_hook"
```

---

## 🔄 更新发布

### 发布新版本

1. **更新版本号** 在 `python/pyproject.toml`:

```toml
[project]
version = "0.2.0"  # 更新这里
```

2. **运行完整流程**:

```bash
python scripts/build_and_publish.py --publish
```

### 版本号规范

遵循 [PEP 440](https://peps.python.org/pep-0440/) 版本规范：

- **正式版**: `0.1.0`, `0.2.0`, `1.0.0`
- **测试版**: `0.1.0a1`, `0.1.0b1`, `0.1.0rc1`
- **开发版**: `0.1.0.dev0`
- **后续版**: `0.1.0.post1`

> ⚠️ **重要**: PyPI 不允许覆盖已发布的版本号！必须发布新的版本号。

---

## 🐛 常见问题

### Q1: "Invalid distribution" 错误

**原因**: wheel 包格式不正确

**解决**:
```bash
python -m twine check dist/*.whl
```

### Q2: "401 Unauthorized" 错误

**原因**: PyPI 凭证错误或过期

**解决**:
1. 检查 `~/.pypirc` 中的 token 是否正确
2. 重新生成 token（已生成的 token 无法查看）
3. 确保文件权限正确（`chmod 600 ~/.pypirc`）

### Q3: 版本号已存在

**原因**: PyPI 不允许覆盖已发布的版本

**解决**: 使用新的版本号
```bash
python scripts/build_and_publish.py --version "0.2.1"
```

### Q4: 试运行成功但实际上传失败

**原因**: 可能是网络问题或 token 过期

**解决**:
```bash
# 重新尝试
python -m twine upload dist/*.whl

# 或者完整流程
python scripts/build_and_publish.py --publish
```

### Q5: "Package already exists" 错误

**原因**: 这个版本号已在 PyPI 上发布过

**解决**: 必须使用新版本号：
```bash
# 增加补丁版本
python scripts/build_and_publish.py --version "0.1.1"
```

---

## 📊 发布检查清单

在正式发布前，请确保：

- [ ] 所有测试通过 (`pytest tests/`)
- [ ] 没有 lint 警告
- [ ] 版本号已更新（`python/pyproject.toml`）
- [ ] CHANGELOG.md 已更新
- [ ] README.md 内容正确
- [ ] 所有依赖已列出（`dependencies`）
- [ ] LICENSE 文件存在
- [ ] 在 Test PyPI 上测试过 (`--test-pypi`)
- [ ] `.pypirc` 权限正确 (`chmod 600`)

---

## 📝 发布后步骤

1. **标记 git tag**:

```bash
git tag -a v0.2.0 -m "Release version 0.2.0"
git push origin v0.2.0
```

2. **创建 GitHub Release**:
   - 访问 https://github.com/YOUR_USER/modeld/releases
   - 填写 Release Notes
   - 附加 wheel 文件

3. **公告**:
   - 在 README.md 中标记新版本
   - 发送社区通知

---

## 🔐 安全提示

⚠️ **重要安全建议**:

1. **不要共享 token**: Token 相当于密码，严格保密
2. **使用作用域 token**: 为每个 package 创建独立 token
3. **定期轮换 token**: PyPI 允许创建多个 token，旧的可删除
4. **不要提交凭证**: 确保 `.pypirc` 在 `.gitignore` 中
5. **设置文件权限**: `chmod 600 ~/.pypirc`

---

## 🆘 获取帮助

- [PyPI 官方文档](https://packaging.python.org/)
- [Twine 使用指南](https://twine.readthedocs.io/)
- [项目 Issues](https://github.com/YOUR_USER/modeld/issues)

---

**祝发布顺利！** 🚀
