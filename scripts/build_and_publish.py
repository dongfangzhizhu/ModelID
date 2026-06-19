#!/usr/bin/env python3

"""
modeld-hook: Build and Publish to PyPI

功能: 一键打包、测试和发布 Python 包到 PyPI
用法: python build_and_publish.py [OPTIONS]

选项:
  --publish             发布到 PyPI
  --dry-run             模拟发布（不实际上传）
  --version VERSION     更新版本号
  --test-pypi           发布到 Test PyPI
  --skip-tests          跳过测试
  --clean-only          仅清理，不构建
"""

import argparse
import os
import sys
import subprocess
import shutil
import json
from pathlib import Path
from typing import Optional, Tuple

# 颜色定义
class Colors:
    RED = '\033[0;31m'
    GREEN = '\033[0;32m'
    YELLOW = '\033[1;33m'
    CYAN = '\033[0;36m'
    RESET = '\033[0m'

def write_status(message: str, status_type: str = "info") -> None:
    """打印状态消息"""
    symbols = {
        "success": f"{Colors.GREEN}[OK]{Colors.RESET}",
        "error": f"{Colors.RED}[FAIL]{Colors.RESET}",
        "warn": f"{Colors.YELLOW}[WARN]{Colors.RESET}",
        "info": f"{Colors.CYAN}[INFO]{Colors.RESET}",
    }
    prefix = symbols.get(status_type, "[INFO]")
    print(f"{prefix} {message}")

def remove_tree(path: Path) -> None:
    """Best-effort recursive removal for build artifacts."""
    try:
        shutil.rmtree(path)
    except PermissionError as e:
        write_status(f"无法删除 {path}: {e}; 已跳过", "warn")

def run_command(cmd: list, cwd: Optional[Path] = None, check: bool = True) -> Tuple[int, str]:
    """运行命令并返回返回码和输出"""
    try:
        result = subprocess.run(
            cmd,
            cwd=cwd,
            capture_output=True,
            text=True,
            check=False
        )
        return result.returncode, result.stdout + result.stderr
    except Exception as e:
        if check:
            write_status(f"命令执行失败: {e}", "error")
            sys.exit(1)
        return 1, str(e)

def confirm_action(message: str) -> bool:
    """确认用户操作"""
    response = input(f"{Colors.YELLOW}{message} (y/n): {Colors.RESET}").strip().lower()
    return response in ['y', 'yes']

def check_python_version() -> None:
    """检查 Python 版本"""
    if sys.version_info < (3, 9):
        write_status("需要 Python 3.9 或更高版本", "error")
        sys.exit(1)
    write_status(f"Python: {sys.version.split()[0]}", "success")

def install_dependencies() -> None:
    """安装必要的依赖"""
    packages = ["build", "twine", "hatchling"]
    for package in packages:
        try:
            __import__(package.replace("-", "_"))
        except ImportError:
            write_status(f"安装 {package}...", "warn")
            run_command([sys.executable, "-m", "pip", "install", package, "-q"])

def get_project_paths() -> Tuple[Path, Path]:
    """获取项目路径"""
    script_dir = Path(__file__).parent
    project_root = script_dir.parent
    python_dir = project_root / "python"
    return project_root, python_dir

def update_version(python_dir: Path, version: str) -> None:
    """更新版本号"""
    write_status(f"更新版本号: {version}", "info")
    
    pyproject_file = python_dir / "pyproject.toml"
    content = pyproject_file.read_text()
    
    import re
    content = re.sub(r'version = "[^"]*"', f'version = "{version}"', content)
    
    pyproject_file.write_text(content)
    write_status("版本号已更新", "success")

def run_tests(python_dir: Path, skip_tests: bool) -> None:
    """运行测试"""
    if skip_tests:
        write_status("跳过测试", "warn")
        return
    
    write_status("运行 Python 测试...", "info")
    
    # 安装开发依赖
    run_command(
        [sys.executable, "-m", "pip", "install", "-e", ".[dev]", "-q"],
        cwd=python_dir,
        check=False
    )
    
    # 运行测试
    returncode, output = run_command(
        [sys.executable, "-m", "pytest", "tests/", "-v"],
        cwd=python_dir,
        check=False
    )
    
    print(output)
    
    if returncode != 0:
        write_status("测试失败", "error")
        sys.exit(1)
    
    write_status("测试通过", "success")

def clean_build_files(python_dir: Path) -> None:
    """清理旧的构建文件"""
    write_status("清理旧的构建文件...", "info")
    
    dirs_to_remove = ["build", "dist", ".pytest_cache"]
    patterns_to_remove = ["*.egg-info", "__pycache__"]
    
    for dir_name in dirs_to_remove:
        dir_path = python_dir / dir_name
        if dir_path.exists():
            remove_tree(dir_path)
    
    for pattern in patterns_to_remove:
        for item in python_dir.rglob(pattern):
            if item.is_dir():
                remove_tree(item)
    
    write_status("旧文件已清理", "success")

def build_wheel(python_dir: Path) -> None:
    """构建 wheel 包"""
    write_status("构建 wheel 包...", "info")
    
    returncode, output = run_command(
        [sys.executable, "-m", "build", "--wheel", "--no-isolation"],
        cwd=python_dir,
        check=False
    )
    
    if returncode != 0:
        write_status("构建失败", "error")
        print(output)
        sys.exit(1)
    
    dist_dir = python_dir / "dist"
    wheels = list(dist_dir.glob("*.whl"))
    write_status(f"成功生成 {len(wheels)} 个 wheel 包", "success")
    
    print()
    print(f"{Colors.CYAN}生成的文件:{Colors.RESET}")
    for wheel in wheels:
        size_mb = wheel.stat().st_size / (1024 * 1024)
        print(f"  - {wheel.name} ({size_mb:.2f} MB)")

def verify_packages(python_dir: Path) -> None:
    """验证包完整性"""
    write_status("验证包完整性...", "info")
    
    dist_dir = python_dir / "dist"
    for wheel in dist_dir.glob("*.whl"):
        returncode, output = run_command(
            [sys.executable, "-m", "twine", "check", str(wheel)],
            check=False
        )
        
        if returncode != 0:
            write_status("包验证失败", "error")
            print(output)
            sys.exit(1)
        
        write_status(f"包 {wheel.name} 验证成功", "success")

def publish_to_pypi(
    python_dir: Path,
    dry_run: bool,
    use_test_pypi: bool
) -> None:
    """发布到 PyPI"""
    print()
    print(f"{Colors.YELLOW}{'=' * 43}{Colors.RESET}")
    print(f"{Colors.YELLOW}准备发布到 {'Test ' if use_test_pypi else ''}PyPI{Colors.RESET}")
    print(f"{Colors.YELLOW}{'=' * 43}{Colors.RESET}")
    print()
    
    # 检查 .pypirc 配置
    pypirc_path = Path.home() / ".pypirc"
    if not pypirc_path.exists():
        write_status("未找到 .pypirc 配置文件", "error")
        print()
        print("请按照以下步骤配置 PyPI 凭证:")
        print()
        print("1. 访问 https://pypi.org/account/register/")
        print("2. 创建账户或登录")
        print("3. 创建 API token: https://pypi.org/manage/account/tokens/")
        print("4. 在 ~/.pypirc 中配置:")
        print()
        example = """[testpypi]
repository = https://test.pypi.org/legacy/
username = __token__
password = pypi-AgEIcHlwaS5vcmc...

[pypi]
repository = https://upload.pypi.org/legacy/
username = __token__
password = pypi-AgEIcHlwaS5vcmc..."""
        print(example)
        print()
        sys.exit(1)
    
    write_status(".pypirc 配置已找到", "success")
    print()
    
    # 显示待发布的包
    dist_dir = python_dir / "dist"
    wheels = list(dist_dir.glob("*.whl"))
    
    print(f"{Colors.CYAN}待发布的包:{Colors.RESET}")
    for wheel in wheels:
        print(f"  - {wheel.name}")
    print()
    
    # 选择仓库
    repository = "testpypi" if use_test_pypi else "pypi"
    
    if dry_run:
        write_status("执行试运行（不实际上传）...", "warn")
        print()
        
        cmd = [
            sys.executable, "-m", "twine", "upload",
            "--dry-run",
            f"--repository", repository,
            f"dist/*"
        ]
        returncode, output = run_command(cmd, cwd=python_dir, check=False)
        print(output)
        
        write_status("试运行成功！实际发布时移除 --dry-run 参数", "success")
    else:
        if not confirm_action(f"确认发布到 {'Test ' if use_test_pypi else ''}PyPI？"):
            write_status("取消发布", "warn")
            return
        
        write_status("上传到 PyPI...", "info")
        print()
        
        cmd = [
            sys.executable, "-m", "twine", "upload",
            f"--repository", repository,
            "dist/*"
        ]
        returncode, output = run_command(cmd, cwd=python_dir, check=False)
        print(output)
        
        if returncode == 0:
            write_status("成功发布到 PyPI！", "success")
            print()
            print(f"{Colors.GREEN}安装命令:{Colors.RESET}")
            print("  pip install modeld-hook")
            print()
        else:
            write_status("发布失败", "error")
            sys.exit(1)

def main() -> None:
    """主程序"""
    parser = argparse.ArgumentParser(description="modeld-hook: Build and Publish to PyPI")
    parser.add_argument("--publish", action="store_true", help="发布到 PyPI")
    parser.add_argument("--dry-run", action="store_true", help="模拟发布（不实际上传）")
    parser.add_argument("--version", help="更新版本号")
    parser.add_argument("--test-pypi", action="store_true", help="发布到 Test PyPI")
    parser.add_argument("--skip-tests", action="store_true", help="跳过测试")
    parser.add_argument("--clean-only", action="store_true", help="仅清理，不构建")
    
    args = parser.parse_args()
    
    # 检查环境
    check_python_version()
    install_dependencies()
    
    print()
    
    # 获取项目路径
    project_root, python_dir = get_project_paths()
    write_status(f"项目根目录: {project_root}", "info")
    write_status(f"Python 包目录: {python_dir}", "info")
    print()
    
    # 验证 Python 包存在
    if not python_dir.exists():
        write_status(f"Python 包目录不存在: {python_dir}", "error")
        sys.exit(1)
    
    # 更新版本
    if args.version:
        update_version(python_dir, args.version)
        print()
    
    # 清理旧文件
    clean_build_files(python_dir)
    print()
    
    # 仅清理模式
    if args.clean_only:
        write_status("清理完成", "success")
        return
    
    # 运行测试
    run_tests(python_dir, args.skip_tests)
    print()
    
    # 构建包
    build_wheel(python_dir)
    print()
    
    # 验证包
    verify_packages(python_dir)
    print()
    
    # 发布
    if args.publish:
        publish_to_pypi(python_dir, args.dry_run, args.test_pypi)
    else:
        print(f"{Colors.CYAN}{'=' * 43}{Colors.RESET}")
        print(f"{Colors.CYAN}构建完成！{Colors.RESET}")
        print(f"{Colors.CYAN}{'=' * 43}{Colors.RESET}")
        print()
        print("下一步:")
        print("  1. 本地测试: pip install dist/modeld_hook-*.whl")
        print("  2. 发布: python build_and_publish.py --publish")
        print("  3. 试运行: python build_and_publish.py --publish --dry-run")
        print("  4. Test PyPI: python build_and_publish.py --publish --test-pypi")
        print()

if __name__ == "__main__":
    main()
