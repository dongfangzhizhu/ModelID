#!/bin/bash

# modeld-hook: Build and Publish to PyPI
# 功能: 一键打包、测试和发布 Python 包到 PyPI
# 用法: ./build_and_publish.sh [--publish] [--dry-run] [--version VERSION]

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# 函数：打印状态消息
write_status() {
    local message="$1"
    local type="${2:-info}"
    
    case "$type" in
        success)
            echo -e "${GREEN}[OK]${NC} $message"
            ;;
        error)
            echo -e "${RED}[FAIL]${NC} $message"
            ;;
        warn)
            echo -e "${YELLOW}[WARN]${NC} $message"
            ;;
        info)
            echo -e "${CYAN}[INFO]${NC} $message"
            ;;
    esac
}

# 函数：确认操作
confirm_action() {
    local message="$1"
    echo -ne "${YELLOW}$message (y/n): ${NC}"
    read -r response
    [[ "$response" == "y" || "$response" == "Y" ]]
}

# 解析命令行参数
PUBLISH=false
DRY_RUN=false
VERSION=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --publish)
            PUBLISH=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --version)
            VERSION="$2"
            shift 2
            ;;
        *)
            echo "未知参数: $1"
            exit 1
            ;;
    esac
done

# 获取项目路径
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
PYTHON_DIR="$PROJECT_ROOT/python"

write_status "项目根目录: $PROJECT_ROOT" "info"
write_status "Python 包目录: $PYTHON_DIR" "info"
echo ""

# 检查必要工具
write_status "检查必要工具..." "info"

if ! command -v python3 &> /dev/null; then
    write_status "Python3 未找到，请先安装 Python 3.9+" "error"
    exit 1
fi

PYTHON_VERSION=$(python3 --version)
write_status "Python: $PYTHON_VERSION" "success"

# 检查并安装依赖工具
for tool in build twine; do
    if ! python3 -c "import $tool" 2>/dev/null; then
        write_status "安装 $tool..." "warn"
        pip install "$tool" -q
    fi
done

echo ""

# 更新版本（如果提供）
if [ -n "$VERSION" ]; then
    write_status "更新版本号: $VERSION" "info"
    PYPROJECT_FILE="$PYTHON_DIR/pyproject.toml"
    sed -i "s/version = \"[^\"]*\"/version = \"$VERSION\"/" "$PYPROJECT_FILE"
    write_status "版本号已更新" "success"
    echo ""
fi

# 运行测试
write_status "运行 Python 测试..." "info"
cd "$PYTHON_DIR"

pip install -e ".[dev]" -q 2>/dev/null || true
python3 -m pytest tests/ -v

if [ $? -ne 0 ]; then
    write_status "测试失败" "error"
    exit 1
fi

write_status "测试通过" "success"
echo ""

# 清理旧的构建文件
write_status "清理旧的构建文件..." "info"

rm -rf build/ dist/ *.egg-info/ .pytest_cache/ __pycache__/ modeld_hook/__pycache__/ tests/__pycache__/

write_status "旧文件已清理" "success"
echo ""

# 构建 wheel 包
write_status "构建 wheel 包..." "info"

python3 -m build --wheel

if [ $? -ne 0 ]; then
    write_status "构建失败" "error"
    exit 1
fi

WHEEL_COUNT=$(ls -1 dist/*.whl 2>/dev/null | wc -l)
write_status "成功生成 $WHEEL_COUNT 个 wheel 包" "success"

echo ""
echo -e "${CYAN}生成的文件:${NC}"
ls -lh dist/ | awk 'NR>1 {printf "  - %s (%s)\n", $9, $5}'
echo ""

# 验证包
write_status "验证包完整性..." "info"

for wheel in dist/*.whl; do
    python3 -m twine check "$wheel"
    if [ $? -ne 0 ]; then
        write_status "包验证失败" "error"
        exit 1
    fi
    write_status "包 $(basename "$wheel") 验证成功" "success"
done

echo ""

# 发布到 PyPI
if [ "$PUBLISH" = true ]; then
    echo -e "${YELLOW}===========================================${NC}"
    echo -e "${YELLOW}准备发布到 PyPI${NC}"
    echo -e "${YELLOW}===========================================${NC}"
    echo ""
    
    # 检查 .pypirc 配置
    PYPIRC_PATH="$HOME/.pypirc"
    if [ ! -f "$PYPIRC_PATH" ]; then
        write_status "未找到 .pypirc 配置文件" "error"
        echo ""
        echo "请按照以下步骤配置 PyPI 凭证:"
        echo ""
        echo "1. 访问 https://pypi.org/account/register/"
        echo "2. 创建账户或登录"
        echo "3. 创建 API token: https://pypi.org/manage/account/tokens/"
        echo "4. 在 $PYPIRC_PATH 中配置:"
        echo ""
        cat << 'EOF'
[testpypi]
repository = https://test.pypi.org/legacy/
username = __token__
password = pypi-AgEIcHlwaS5vcmc...

[pypi]
repository = https://upload.pypi.org/legacy/
username = __token__
password = pypi-AgEIcHlwaS5vcmc...
EOF
        echo ""
        exit 1
    fi
    
    write_status ".pypirc 配置已找到" "success"
    echo ""
    
    # 显示待发布的包
    echo -e "${CYAN}待发布的包:${NC}"
    ls -1 dist/*.whl | xargs -I {} basename {} | sed 's/^/  - /'
    echo ""
    
    if [ "$DRY_RUN" = true ]; then
        write_status "执行试运行（不实际上传）..." "warn"
        echo ""
        python3 -m twine upload --dry-run --repository pypi dist/*
        write_status "试运行成功！实际发布时移除 --dry-run 参数" "success"
    else
        if ! confirm_action "确认发布到 PyPI？"; then
            write_status "取消发布" "warn"
            exit 0
        fi
        
        write_status "上传到 PyPI..." "info"
        echo ""
        
        python3 -m twine upload --repository pypi dist/*
        
        if [ $? -eq 0 ]; then
            write_status "成功发布到 PyPI！" "success"
            echo ""
            echo -e "${GREEN}安装命令:${NC}"
            echo "  pip install modeld-hook"
            echo ""
        else
            write_status "发布失败" "error"
            exit 1
        fi
    fi
else
    echo -e "${CYAN}===========================================${NC}"
    echo -e "${CYAN}构建完成！${NC}"
    echo -e "${CYAN}===========================================${NC}"
    echo ""
    echo "下一步:"
    echo "  1. 本地测试: pip install dist/modeld_hook-*.whl"
    echo "  2. 发布: ./build_and_publish.sh --publish"
    echo "  3. 试运行: ./build_and_publish.sh --publish --dry-run"
    echo ""
fi
