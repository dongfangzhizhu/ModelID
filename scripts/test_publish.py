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
        subprocess.run(cmd, cwd=cwd, check=True)
        print(f"\n[OK] {name} 通过")
        return True
    except FileNotFoundError as e:
        print(f"\n[FAIL] {name} 失败 (找不到命令: {e.filename})")
        return False
    except subprocess.CalledProcessError as e:
        print(f"\n[FAIL] {name} 失败 (返回码: {e.returncode})")
        return False

def run_path_test(name, path):
    """检查路径是否存在"""
    print(f"\n{'='*60}")
    print(f"测试: {name}")
    print(f"{'='*60}")
    print(f"路径: {path}")
    if path.exists():
        print(f"\n[OK] {name} 通过")
        return True
    print(f"\n[FAIL] {name} 失败 (路径不存在)")
    return False

def main():
    """主测试函数"""
    project_root = Path(__file__).parent.parent
    python_dir = project_root / "python"
    
    tests = [
        # 环境检查
        ("cmd", "检查 Python 版本", [sys.executable, "--version"], None),
        
        # 依赖检查
        ("cmd", "检查 build 工具", [sys.executable, "-m", "pip", "show", "build"], None),
        ("cmd", "检查 twine 工具", [sys.executable, "-m", "pip", "show", "twine"], None),
        ("cmd", "检查 hatchling 工具", [sys.executable, "-m", "pip", "show", "hatchling"], None),
        
        # 项目结构检查
        ("path", "检查 pyproject.toml", python_dir / "pyproject.toml", None),
        ("path", "检查 Python 包", python_dir / "modeld_hook", None),
        
        # 测试
        ("cmd", "运行单元测试", [sys.executable, "-m", "pytest", "tests/", "-v"], python_dir),
        
        # 构建
        ("cmd", "构建 wheel", [sys.executable, "-m", "build", "--wheel", "--no-isolation"], python_dir),
    ]
    
    results = []
    for kind, name, payload, cwd in tests:
        if kind == "path":
            success = run_path_test(name, payload)
        else:
            success = run_test(name, payload, cwd)
        results.append((name, success))

    wheels = sorted((python_dir / "dist").glob("*.whl"))
    if wheels:
        success = run_test(
            "验证 wheel",
            [sys.executable, "-m", "twine", "check", *[str(wheel) for wheel in wheels]],
            python_dir,
        )
    else:
        success = run_path_test("验证 wheel", python_dir / "dist" / "*.whl")
    results.append(("验证 wheel", success))
    
    # 总结
    print(f"\n{'='*60}")
    print("测试总结")
    print(f"{'='*60}")
    
    passed = sum(1 for _, success in results if success)
    total = len(results)
    
    for name, success in results:
        symbol = "[OK]" if success else "[FAIL]"
        print(f"{symbol} {name}")
    
    print()
    print(f"通过: {passed}/{total}")
    
    if passed == total:
        print("\n[OK] 所有测试通过！可以发布到 PyPI")
        print("\n下一步:")
        print("  python scripts/build_and_publish.py --publish --dry-run")
        print("  python scripts/build_and_publish.py --publish")
        return 0
    else:
        print(f"\n[FAIL] {total - passed} 个测试失败，请修复后重试")
        return 1

if __name__ == "__main__":
    sys.exit(main())
