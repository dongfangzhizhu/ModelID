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
	@echo "Rust 构建命令:"
	@echo "  make rust-build        - 编译所有 crates (debug)"
	@echo "  make rust-build-release - 编译所有 crates (release)"
	@echo "  make rust-test         - 测试所有 crates"
	@echo "  make rust-check        - 检查所有 crates"
	@echo "  make rust-fmt          - 格式化代码"
	@echo "  make rust-lint         - Lint 检查"
	@echo "  make rust-help         - 显示更多 Rust 命令"
	@echo ""
	@echo "Python 构建命令:"
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
# 使用编译脚本的推荐方式
rust-build:
	@echo "编译所有 Rust crates (debug 模式)..."
	@python scripts/build.py

rust-build-release:
	@echo "编译所有 Rust crates (release 模式)..."
	@python scripts/build.py --release

rust-build-crate:
	@echo "用法: make rust-build-crate CRATE=modeld-core"
	@echo "可用的 crates: modeld-core modeld-cli modeld-proxy modeld-client modeld-webui"
	@if [ -n "$(CRATE)" ]; then \
		python scripts/build.py $(CRATE); \
	fi

rust-check:
	@echo "检查所有 Rust crates..."
	@python scripts/build.py --check

rust-check-crate:
	@echo "用法: make rust-check-crate CRATE=modeld-core"
	@if [ -n "$(CRATE)" ]; then \
		python scripts/build.py --check $(CRATE); \
	fi

rust-test:
	@echo "测试所有 Rust crates..."
	@python scripts/build.py --test

rust-test-crate:
	@echo "用法: make rust-test-crate CRATE=modeld-core"
	@if [ -n "$(CRATE)" ]; then \
		python scripts/build.py --test $(CRATE); \
	fi

rust-fmt:
	@echo "格式化所有 Rust crates..."
	@python scripts/build.py --format

rust-lint:
	@echo "Lint 检查所有 Rust crates..."
	@python scripts/build.py --lint

rust-lint-verbose:
	@echo "Lint 检查所有 Rust crates (详细输出)..."
	@python scripts/build.py --lint --verbose

rust-clean:
	@echo "清理构建输出..."
	cargo clean

rust-help:
	@echo ""
	@echo "Rust 编译相关命令:"
	@echo ""
	@echo "  make rust-build              - 编译所有 crates (debug)"
	@echo "  make rust-build-release      - 编译所有 crates (release)"
	@echo "  make rust-build-crate        - 编译特定 crate (需指定 CRATE=...)"
	@echo "  make rust-check              - 检查代码"
	@echo "  make rust-check-crate        - 检查特定 crate"
	@echo "  make rust-test               - 运行所有测试"
	@echo "  make rust-test-crate         - 运行特定 crate 的测试"
	@echo "  make rust-fmt                - 格式化代码"
	@echo "  make rust-lint               - Lint 检查"
	@echo "  make rust-lint-verbose       - Lint 检查 (详细)"
	@echo "  make rust-clean              - 清理构建"
	@echo "  make rust-help               - 显示此帮助"
	@echo ""
	@echo "示例:"
	@echo "  make rust-build              # 编译所有 crates"
	@echo "  make rust-build-crate CRATE=modeld-cli"
	@echo "  make rust-test               # 测试所有"
	@echo "  make rust-lint-verbose       # 详细 lint"
	@echo ""

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
