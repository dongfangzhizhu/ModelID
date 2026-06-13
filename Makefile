.PHONY: help build test publish publish-test clean install-dev

# 默认目标
help:
	@echo "modeld 项目 Makefile 命令"
	@echo ""
	@echo "开发命令:"
	@echo "  make install-dev       - 安装开发依赖"
	@echo "  make test              - 运行测试"
	@echo "  make lint              - 运行 lint 检查"
	@echo ""
	@echo "构建命令:"
	@echo "  make build             - 构建 wheel 包"
	@echo "  make clean             - 清理构建文件"
	@echo ""
	@echo "发布命令:"
	@echo "  make publish-test      - 发布到 Test PyPI（推荐先做这个）"
	@echo "  make publish           - 发布到 PyPI"
	@echo "  make publish-dry-run   - 模拟发布（不实际上传）"
	@echo ""
	@echo "其他:"
	@echo "  make check             - 发布前检查清单"
	@echo "  make config-pypi       - 配置 PyPI 凭证"

# 安装开发依赖
install-dev:
	pip install -e "python[dev]"
	pip install build twine

# 运行测试
test:
	cd python && python -m pytest tests/ -v

# 运行 lint
lint:
	cd python && python -m flake8 modeld_hook/ tests/ --max-line-length=100 || true
	cd python && python -m black --check modeld_hook/ tests/ || true

# 构建 wheel
build:
	python scripts/build_and_publish.py

# 清理
clean:
	python scripts/build_and_publish.py --clean-only

# 发布前检查
check:
	@echo "发布前检查清单:"
	@echo ""
	@echo "检查清单:"
	@echo "  □ 所有测试通过 (make test)"
	@echo "  □ 版本号已更新 (python/pyproject.toml)"
	@echo "  □ README.md 内容正确"
	@echo "  □ 所有依赖已列出"
	@echo "  □ LICENSE 文件存在"
	@echo "  □ .pypirc 已配置 (make config-pypi)"
	@echo ""
	@echo "准备就绪？运行:"
	@echo "  make publish-dry-run   # 模拟发布"
	@echo "  make publish           # 实际发布到 PyPI"

# 配置 PyPI
config-pypi:
	@echo "配置 PyPI 凭证..."
	@echo ""
	@echo "1. 访问: https://pypi.org/manage/account/tokens/"
	@echo "2. 创建新 token 并复制"
	@echo "3. 编辑 ~/.pypirc 并粘贴 token"
	@echo ""
	@if [ -f ~/.pypirc ]; then \
		echo "✓ ~/.pypirc 已存在"; \
		ls -la ~/.pypirc; \
	else \
		echo "✗ ~/.pypirc 不存在，创建中..."; \
		cp scripts/.pypirc.template ~/.pypirc; \
		chmod 600 ~/.pypirc; \
		echo "✓ 已创建 ~/.pypirc，请编辑并填入 token"; \
		echo "位置: ~/.pypirc"; \
	fi

# 模拟发布
publish-dry-run:
	python scripts/build_and_publish.py --publish --dry-run

# 发布到 Test PyPI
publish-test:
	python scripts/build_and_publish.py --publish --test-pypi

# 发布到 PyPI
publish:
	python scripts/build_and_publish.py --publish

# Rust 相关命令
rust-test:
	cargo test --workspace

rust-build:
	cargo build --release

rust-fmt:
	cargo fmt --all
	cargo fmt --all -- --check

# 完整发布流程
release: check test build publish
	@echo ""
	@echo "✓ 发布完成！"
	@echo ""
	@echo "后续步骤:"
	@echo "  git tag -a v0.x.x -m 'Release v0.x.x'"
	@echo "  git push origin v0.x.x"

# 快速发布（跳过测试）
quick-publish:
	python scripts/build_and_publish.py --publish --skip-tests

.PHONY: all
all: install-dev test build
