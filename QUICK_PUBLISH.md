# 快速发布参考

## ⚡ 一键命令

### 首次发布前：配置 PyPI 凭证

```bash
# 1. 获取 token
#   访问: https://pypi.org/manage/account/tokens/

# 2. 配置 ~/.pypirc
cp scripts/.pypirc.template ~/.pypirc
# 编辑 ~/.pypirc 并填入 token

# 3. 设置权限（重要！）
chmod 600 ~/.pypirc
```

---

## 🚀 完整发布流程

### 第一次发布

```bash
# Python 脚本方式（推荐）
python scripts/build_and_publish.py --publish

# 或者 Bash
./scripts/build_and_publish.sh --publish

# 或者 PowerShell
.\scripts\build_and_publish.ps1 -publish
```

### 更新版本发布

```bash
# 更新版本号 + 发布
python scripts/build_and_publish.py --version 0.2.0 --publish

# 仅模拟（确认无误）
python scripts/build_and_publish.py --version 0.2.0 --publish --dry-run
```

---

## 🧪 发布前测试

```bash
# 在 Test PyPI 上测试
python scripts/build_and_publish.py --publish --test-pypi

# 验证 Test PyPI 上可安装
pip install -i https://test.pypi.org/simple/ modeld-hook
```

---

## ✅ 验证发布成功

```bash
# 在 PyPI 上查看
https://pypi.org/project/modeld-hook/

# 本地验证可安装
pip install modeld-hook --upgrade

# 检查版本
pip show modeld-hook
```

---

## 📊 脚本对比

| 脚本 | 平台 | 优点 |
|------|------|------|
| `build_and_publish.py` | 全平台 | 功能最完整，推荐 |
| `build_and_publish.sh` | Linux/macOS | 简洁，原生 bash |
| `build_and_publish.ps1` | Windows | 原生 PowerShell 支持 |
| 手动命令 | 全平台 | 完全控制，学习用 |

---

## 🔧 常用选项

```bash
# Python 脚本选项
python scripts/build_and_publish.py \
    --publish           # 发布到 PyPI
    --dry-run           # 模拟发布（不实际上传）
    --test-pypi         # 发布到 Test PyPI
    --version 0.2.0     # 更新版本号
    --skip-tests        # 跳过测试
    --clean-only        # 仅清理旧文件

# 示例：更新版本并发布到 Test PyPI 测试
python scripts/build_and_publish.py --version 0.2.0 --publish --test-pypi

# 示例：跳过测试快速发布
python scripts/build_and_publish.py --publish --skip-tests
```

---

## 🐛 快速故障排除

| 问题 | 解决 |
|------|------|
| 401 Unauthorized | 检查 `~/.pypirc` token，重新生成 |
| Package already exists | 增加版本号重试 |
| Tests failed | 解决测试问题后重试 |
| Network error | 检查网络，稍后重试 |
| wheel validation failed | 运行 `twine check dist/*.whl` |

---

## 📖 更多信息

详见 `PUBLISH_GUIDE.md` 获取完整文档
