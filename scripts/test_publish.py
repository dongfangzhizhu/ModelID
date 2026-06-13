#!/usr/bin/env python3

"""
测试脚本：验证打包和发布的各个步骤

用法: python scripts/test_publish.py
"""

import sys
import subprocess
from pathlib import Path

def run_test(name, cmd, cwd=None):
    """运行测试"""
    print(f"\n{'='*60}")
    print(f"测试: {name}")
    print(f"{'='*60}")
    print(f"命令: {' '.join(cmd)}")
    print()
    
    try:
        result = subprocess.run(cmd, cwd=cwd, check=True)
        print(f"\n✓ {name} 通过")
        return True
    except subprocess.CalledProcessError as e:
        print(f"\n✗ {name} 失败 (返回码: {e.returncode})")
        return False

def main():
    """主测试函数"""
    project_root = Path(__file__).parent.parent
    python_dir = project_root / "python"
    
    tests = [
        # 环境检查
        ("检查 Python 版本", [sys.executable, "--version"], None),
        
        # 依赖检查
        ("检查 build 工具", [sys.executable, "-m", "pip", "show", "build"], None),
        ("检查 twine 工具", [sys.executable, "-m", "pip", "show", "twine"], None),
        
        # 项目结构检查
        ("检查 pyproject.toml", ["ls", "-la", str(python_dir / "pyproject.toml")], None),
        ("检查 Python 包", ["ls", "-la", str(python_dir / "modeld_hook")], None),
        
        # 测试
        ("运行单元测试", [sys.executable, "-m", "pytest", "tests/", "-v"], python_dir),
        
        # 构建
        ("构建 wheel", [sys.executable, "-m", "build", "--wheel"], python_dir),
        
        # 验证
        ("验证 wheel", [sys.executable, "-m", "twine", "check", "dist/*.whl"], python_dir),
    ]
    
    results = []
    for name, cmd, cwd in tests:
        success = run_test(name, cmd, cwd)
        results.append((name, success))
    
    # 总结
    print(f"\n{'='*60}")
    print("测试总结")
    print(f"{'='*60}")
    
    passed = sum(1 for _, success in results if success)
    total = len(results)
    
    for name, success in results:
        symbol = "✓" if success else "✗"
        print(f"{symbol} {name}")
    
    print()
    print(f"通过: {passed}/{total}")
    
    if passed == total:
        print("\n✓ 所有测试通过！可以发布到 PyPI")
        print("\n下一步:")
        print("  python scripts/build_and_publish.py --publish --dry-run")
        print("  python scripts/build_and_publish.py --publish")
        return 0
    else:
        print(f"\n✗ {total - passed} 个测试失败，请修复后重试")
        return 1

if __name__ == "__main__":
    sys.exit(main())
