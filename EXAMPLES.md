# PyPI 发布工作流 - 使用示例

本文件提供实际的使用示例。

---

## 🎯 示例 1：首次发布（最简单）

### 目标
第一次发布 modeld-hook v0.1.0 到 PyPI

### 步骤

#### 第 1 次运行：配置凭证（一次性）

```bash
# 配置 PyPI 凭证
make config-pypi
```

输出：
```
→ 配置 PyPI 凭证...

✓ .pypirc 已存在
  -rw------- 1 user group 256 Jun 13 17:00 ~/.pypirc
```

#### 第 2 次运行：发布

```bash
# 完整发布（包含测试、构建、验证）
make publish
```

完整输出流程：
```
→ 项目根目录: /home/user/modeld
→ Python 包目录: /home/user/modeld/python

→ 检查必要工具...
✓ Python: Python 3.10.0
✓ build 工具已找到
✓ twine 工具已找到

→ 运行 Python 测试...
================================================== test session starts ==================================================
platform linux -- Python 3.10.0
collected 1 test

tests/test_modeld_hook.py::test_import PASSED [100%]

================================================== 1 passed in 0.03s ==================================================

✓ 测试通过

→ 清理旧的构建文件...
✓ 旧文件已清理

→ 构建 wheel 包...
✓ 成功生成 1 个 wheel 包

生成的文件:
  • modeld_hook-0.1.0-py3-none-any.whl (4.23 KB)

→ 验证包完整性...
✓ 包 modeld_hook-0.1.0-py3-none-any.whl 验证成功

═══════════════════════════════════════════════════════════════════════════════
准备发布到 PyPI
═══════════════════════════════════════════════════════════════════════════════

✓ .pypirc 配置已找到

待发布的包:
  • modeld_hook-0.1.0-py3-none-any.whl

确认发布到 PyPI？ (y/n): y

→ 上传到 PyPI...

Uploading modeld_hook-0.1.0-py3-none-any.whl
100%|████████████████████████████| 4.23K/4.23K [00:02<00:00, 1.80KB/s]

✓ 成功发布到 PyPI！

✓ 安装命令:
  pip install modeld-hook
```

### 验证

发布后可以：

```bash
# 访问 PyPI
https://pypi.org/project/modeld-hook/

# 安装验证
pip install modeld-hook
pip show modeld-hook

# 测试功能
python -c "import modeld_hook; print('Success!')"
```

---

## 🎯 示例 2：发布新版本

### 目标
从 v0.1.0 升级到 v0.2.0，包含新功能

### 步骤

#### 1. 修改代码和测试

```bash
# 修改代码
# ...

# 运行测试验证
make test
```

#### 2. 更新版本号并发布

```bash
# 一行命令：更新版本 + 发布
python scripts/build_and_publish.py --version 0.2.0 --publish
```

输出：
```
→ 项目根目录: /home/user/modeld
→ Python 包目录: /home/user/modeld/python

→ 更新版本号: 0.2.0
✓ 版本号已更新

→ 运行 Python 测试...
tests/test_modeld_hook.py::test_import PASSED
✓ 测试通过

→ 清理旧的构建文件...
✓ 旧文件已清理

→ 构建 wheel 包...
✓ 成功生成 1 个 wheel 包

生成的文件:
  • modeld_hook-0.2.0-py3-none-any.whl (5.12 KB)

→ 验证包完整性...
✓ 包 modeld_hook-0.2.0-py3-none-any.whl 验证成功

═══════════════════════════════════════════════════════════════════════════════
准备发布到 PyPI
═══════════════════════════════════════════════════════════════════════════════

✓ .pypirc 配置已找到

待发布的包:
  • modeld_hook-0.2.0-py3-none-any.whl

确认发布到 PyPI？ (y/n): y

→ 上传到 PyPI...
✓ 成功发布到 PyPI！

✓ 安装命令:
  pip install modeld-hook
```

#### 3. 创建 Git 标签

```bash
git tag -a v0.2.0 -m "Release v0.2.0: Add new features"
git push origin v0.2.0
```

---

## 🎯 示例 3：安全发布（干运行）

### 目标
在正式发布前模拟整个过程

### 步骤

```bash
# 模拟发布（所有步骤都一样，除了不实际上传）
make publish-dry-run
```

输出：
```
[省略前面的步骤...]

执行试运行（不实际上传）...

Uploading modeld_hook-0.1.0-py3-none-any.whl
100%|████████████████████████████| 4.23K/4.23K [00:00<00:00, 100.0KB/s]

✓ 试运行成功！实际发布时移除 --dry-run 参数
```

这表示所有验证都通过了，可以安心实际发布。

---

## 🎯 示例 4：在 Test PyPI 上测试

### 目标
在生产发布前在 Test PyPI 测试

### 步骤

```bash
# 发布到 Test PyPI（完全隔离的环境）
make publish-test
```

输出类似 PyPI 发布，但上传到 `https://test.pypi.org/`

### 验证

```bash
# 从 Test PyPI 安装
pip install -i https://test.pypi.org/simple/ modeld-hook

# 检查版本
pip show modeld-hook
```

如果一切正常，再发布到正式 PyPI：

```bash
make publish
```

---

## 🎯 示例 5：跳过测试快速发布

### 目标
对于小的 bug 修复，快速发布（仍然验证包完整性）

### 步骤

```bash
# 跳过单元测试但仍进行包验证
python scripts/build_and_publish.py --publish --skip-tests
```

**注意**: 仅在充分确信代码无误时使用！

---

## 🎯 示例 6：使用不同的脚本

### Bash 脚本（Linux/macOS）

```bash
# 使用 Bash 脚本
./scripts/build_and_publish.sh --publish

# 带选项
./scripts/build_and_publish.sh --version 0.3.0 --publish
./scripts/build_and_publish.sh --publish --dry-run
```

### PowerShell 脚本（Windows）

```powershell
# 使用 PowerShell 脚本
.\scripts\build_and_publish.ps1 -publish

# 带选项
.\scripts\build_and_publish.ps1 -version "0.3.0" -publish
.\scripts\build_and_publish.ps1 -publish -dry_run
```

### Python 脚本（全平台）

```bash
# 全平台一致的方式
python scripts/build_and_publish.py --publish
python scripts/build_and_publish.py --version 0.3.0 --publish
python scripts/build_and_publish.py --publish --dry-run
```

---

## 🐛 示例 7：故障恢复

### 问题 1：发布失败但本地已清理

```bash
# 重新构建和发布
make build
make publish
```

### 问题 2：版本号已存在

```bash
# 错误信息：Package already exists

# 解决：使用新版本号
python scripts/build_and_publish.py --version 0.1.1 --publish
```

### 问题 3：Token 过期

```bash
# 错误信息：401 Unauthorized

# 解决步骤：
# 1. 访问 https://pypi.org/manage/account/tokens/
# 2. 生成新 token
# 3. 更新 ~/.pypirc 中的 password 字段
# 4. 重试发布

make publish
```

### 问题 4：网络中断

```bash
# 重新尝试（Twine 会检测重新上传）
make publish
```

---

## 📊 示例 8：发布流程查看

### 查看即将发生的操作

```bash
# 不仅仅是模拟，还显示详细步骤
python scripts/test_publish.py
```

输出：
```
============================================================
测试: 检查 Python 版本
============================================================
命令: python --version

Python 3.10.0

✓ 检查 Python 版本 通过

[... 更多测试 ...]

============================================================
测试总结
============================================================
✓ 检查 Python 版本
✓ 检查 build 工具
✓ 检查 twine 工具
✓ 运行单元测试
✓ 构建 wheel
✓ 验证 wheel

通过: 6/6

✓ 所有测试通过！可以发布到 PyPI

下一步:
  python scripts/build_and_publish.py --publish --dry-run
  python scripts/build_and_publish.py --publish
```

---

## 🎯 示例 9：完整的 CI/CD 流程

### 在自动化系统中发布

```bash
# GitHub Actions 或 CI/CD 中的用法
python scripts/build_and_publish.py \
    --version 0.4.0 \
    --publish \
    --skip-tests  # 因为 CI 已运行测试
```

### 环境变量方式

```bash
# 从环境变量读取 token（更安全的做法）
export TWINE_PASSWORD=${{ secrets.PYPI_TOKEN }}
export TWINE_USERNAME=${{ secrets.PYPI_USERNAME }}

python scripts/build_and_publish.py --publish
```

---

## 🎯 示例 10：项目团队工作流

### 开发者工作流

```bash
# 开发者只需运行
make publish
```

### 维护者工作流

```bash
# 维护者进行版本管理和 git 标签
git checkout -b release/v0.3.0
python scripts/build_and_publish.py --version 0.3.0 --publish
git tag -a v0.3.0 -m "Release v0.3.0"
git push origin v0.3.0
```

### 版本发布工作流

```bash
# 自动化版本发布脚本
#!/bin/bash
VERSION="0.3.0"
python scripts/build_and_publish.py --version $VERSION --publish
git tag -a v$VERSION -m "Release v$VERSION"
git push origin v$VERSION
echo "✓ Released v$VERSION"
```

---

## 💡 最佳实践

### ✅ 推荐做法

```bash
# 1. 总是先运行干运行
make publish-dry-run

# 2. 在 Test PyPI 上测试
make publish-test

# 3. 最后才正式发布
make publish

# 4. 创建 git 标签
git tag -a v0.2.0 -m "Release v0.2.0"
```

### ❌ 不推荐做法

```bash
# ✗ 直接发布，不验证
make publish

# ✗ 跳过所有测试
python scripts/build_and_publish.py --publish --skip-tests

# ✗ 连续发布相同版本号（会失败）
```

---

## 🎉 总结

使用这套工作流系统，发布变得简单且安全：

- 最简单：`make publish`（3 秒）
- 最安全：`make publish-dry-run` + `make publish`（1 分钟）
- 最灵活：`python scripts/build_and_publish.py --version X.Y.Z --publish`

立即开始发布吧！ 🚀
