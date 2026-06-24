#!/usr/bin/env python3

"""
Build script for ModelID Rust projects
Supports both Windows and Linux/macOS environments
Usage:
    python build.py                     # Build all crates
    python build.py modeld-core         # Build specific crate
    python build.py --release           # Build all in release mode
    python build.py --help              # Show help
"""

import sys
import subprocess
import platform
import argparse
from pathlib import Path
from typing import List, Tuple
import shutil

# Available crates
CRATES = [
    "modeld-core",
    "modeld-cli",
    "modeld-proxy",
    "modeld-client",
    "modeld-webui",
]

# Color codes for terminal output
class Colors:
    RED = '\033[91m'
    GREEN = '\033[92m'
    YELLOW = '\033[93m'
    BLUE = '\033[94m'
    RESET = '\033[0m'
    
    # Windows compatibility
    if platform.system() == 'Windows':
        RED = GREEN = YELLOW = BLUE = RESET = ''


def print_info(message: str):
    """Print info message"""
    print(f"{Colors.BLUE}[INFO]{Colors.RESET} {message}")


def print_success(message: str):
    """Print success message"""
    print(f"{Colors.GREEN}[SUCCESS]{Colors.RESET} {message}")


def print_error(message: str):
    """Print error message"""
    print(f"{Colors.RED}[ERROR]{Colors.RESET} {message}")


def print_warn(message: str):
    """Print warning message"""
    print(f"{Colors.YELLOW}[WARN]{Colors.RESET} {message}")


def get_project_root() -> Path:
    """Get the root directory of the project"""
    current_file = Path(__file__).resolve()
    return current_file.parent.parent


def check_cargo_installed() -> bool:
    """Check if cargo is installed"""
    return shutil.which('cargo') is not None


def build_crate(crate_name: str, release: bool = False, verbose: bool = False) -> bool:
    """Build a single crate"""
    cargo_cmd = ['cargo', 'build', '-p', crate_name]
    
    if verbose:
        cargo_cmd.append('--verbose')
    
    if release:
        cargo_cmd.append('--release')
    
    print_info(f"Building {crate_name}...")
    try:
        result = subprocess.run(cargo_cmd, cwd=get_project_root(), check=False)
        if result.returncode == 0:
            print_success(f"Built {crate_name} successfully")
            return True
        else:
            print_error(f"Failed to build {crate_name}")
            return False
    except Exception as e:
        print_error(f"Error building {crate_name}: {e}")
        return False


def check_crate(crate_name: str, release: bool = False, verbose: bool = False) -> bool:
    """Check a single crate"""
    cargo_cmd = ['cargo', 'check', '-p', crate_name]
    
    if verbose:
        cargo_cmd.append('--verbose')
    
    if release:
        cargo_cmd.append('--release')
    
    print_info(f"Checking {crate_name}...")
    try:
        result = subprocess.run(cargo_cmd, cwd=get_project_root(), check=False)
        if result.returncode == 0:
            print_success(f"Checked {crate_name} successfully")
            return True
        else:
            print_error(f"Check failed for {crate_name}")
            return False
    except Exception as e:
        print_error(f"Error checking {crate_name}: {e}")
        return False


def test_crate(crate_name: str, verbose: bool = False) -> bool:
    """Test a single crate"""
    cargo_cmd = ['cargo', 'test', '-p', crate_name]
    
    if verbose:
        cargo_cmd.append('--verbose')
    
    print_info(f"Testing {crate_name}...")
    try:
        result = subprocess.run(cargo_cmd, cwd=get_project_root(), check=False)
        if result.returncode == 0:
            print_success(f"Tested {crate_name} successfully")
            return True
        else:
            print_error(f"Tests failed for {crate_name}")
            return False
    except Exception as e:
        print_error(f"Error testing {crate_name}: {e}")
        return False


def format_crate(crate_name: str) -> bool:
    """Format a single crate"""
    cargo_cmd = ['cargo', 'fmt', '-p', crate_name]
    
    print_info(f"Formatting {crate_name}...")
    try:
        result = subprocess.run(cargo_cmd, cwd=get_project_root(), check=False)
        if result.returncode == 0:
            print_success(f"Formatted {crate_name} successfully")
            return True
        else:
            print_error(f"Failed to format {crate_name}")
            return False
    except Exception as e:
        print_error(f"Error formatting {crate_name}: {e}")
        return False


def lint_crate(crate_name: str, verbose: bool = False) -> bool:
    """Lint a single crate"""
    cargo_cmd = ['cargo', 'clippy', '-p', crate_name, '--', '-D', 'warnings']
    
    if verbose:
        cargo_cmd.insert(2, '--verbose')
    
    print_info(f"Linting {crate_name}...")
    try:
        result = subprocess.run(cargo_cmd, cwd=get_project_root(), check=False)
        if result.returncode == 0:
            print_success(f"Linted {crate_name} successfully")
            return True
        else:
            print_warn(f"Lint warnings found in {crate_name}")
            return False
    except Exception as e:
        print_error(f"Error linting {crate_name}: {e}")
        return False


def build_all(release: bool = False, verbose: bool = False) -> Tuple[List[str], List[str]]:
    """Build all crates"""
    successful = []
    failed = []
    
    for crate in CRATES:
        if build_crate(crate, release, verbose):
            successful.append(crate)
        else:
            failed.append(crate)
        print()
    
    return successful, failed


def check_all(release: bool = False, verbose: bool = False) -> Tuple[List[str], List[str]]:
    """Check all crates"""
    successful = []
    failed = []
    
    for crate in CRATES:
        if check_crate(crate, release, verbose):
            successful.append(crate)
        else:
            failed.append(crate)
        print()
    
    return successful, failed


def test_all(verbose: bool = False) -> Tuple[List[str], List[str]]:
    """Test all crates"""
    successful = []
    failed = []
    
    for crate in CRATES:
        if test_crate(crate, verbose):
            successful.append(crate)
        else:
            failed.append(crate)
        print()
    
    return successful, failed


def format_all() -> Tuple[List[str], List[str]]:
    """Format all crates"""
    successful = []
    failed = []
    
    for crate in CRATES:
        if format_crate(crate):
            successful.append(crate)
        else:
            failed.append(crate)
        print()
    
    return successful, failed


def lint_all(verbose: bool = False) -> Tuple[List[str], List[str]]:
    """Lint all crates"""
    successful = []
    failed = []
    
    for crate in CRATES:
        if lint_crate(crate, verbose):
            successful.append(crate)
        else:
            failed.append(crate)
        print()
    
    return successful, failed


def print_summary(successful: List[str], failed: List[str]):
    """Print build summary"""
    print("━" * 64)
    print_info("Build Summary")
    print("━" * 64)
    
    if successful:
        print_success(f"Successful: {len(successful)} crate(s)")
        for crate in successful:
            print(f"  ✓ {crate}")
    
    if failed:
        print_error(f"Failed: {len(failed)} crate(s)")
        for crate in failed:
            print(f"  ✗ {crate}")
    
    print()


def main():
    """Main function"""
    parser = argparse.ArgumentParser(
        description='Build script for ModelID Rust projects',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=f"""
Examples:
  python build.py                    # Build all crates in debug mode
  python build.py modeld-core        # Build specific crate
  python build.py --release          # Build all crates in release mode
  python build.py --check            # Check all crates
  python build.py --test             # Test all crates
  python build.py --format           # Format all crates
  python build.py --lint             # Lint all crates
  python build.py --release --lint   # Lint in release mode

Available crates:
  {', '.join(CRATES)}
        """
    )
    
    parser.add_argument(
        'crates',
        nargs='*',
        help='Specific crates to build (if empty, builds all)'
    )
    parser.add_argument(
        '-r', '--release',
        action='store_true',
        help='Build in release mode (optimized)'
    )
    parser.add_argument(
        '-v', '--verbose',
        action='store_true',
        help='Show detailed build output'
    )
    parser.add_argument(
        '-c', '--check',
        action='store_true',
        help='Only check code without building'
    )
    parser.add_argument(
        '-t', '--test',
        action='store_true',
        help='Build and run tests'
    )
    parser.add_argument(
        '--format',
        action='store_true',
        help='Format code (requires rustfmt)'
    )
    parser.add_argument(
        '--lint',
        action='store_true',
        help='Run clippy linter'
    )
    
    args = parser.parse_args()
    
    # Check if cargo is installed
    if not check_cargo_installed():
        print_error("cargo not found. Please install Rust and cargo.")
        sys.exit(1)
    
    # Determine which crates to process
    target_crates = args.crates if args.crates else CRATES
    
    # Validate crate names
    for crate in target_crates:
        if crate not in CRATES:
            print_error(f"Unknown crate: {crate}")
            print_info(f"Available crates: {', '.join(CRATES)}")
            sys.exit(1)
    
    print_info(f"Platform: {platform.system()} {platform.release()}")
    print_info(f"Python: {sys.version.split()[0]}")
    print_info("Starting build process...")
    print_info(f"Build type: {'release' if args.release else 'debug'}")
    print_info(f"Crates to process: {', '.join(target_crates)}")
    print()
    
    successful = []
    failed = []
    
    # Determine action and execute
    if args.format:
        print_info("Running format...")
        print()
        for crate in target_crates:
            if format_crate(crate):
                successful.append(crate)
            else:
                failed.append(crate)
            print()
    elif args.lint:
        print_info("Running lint...")
        print()
        for crate in target_crates:
            if lint_crate(crate, args.verbose):
                successful.append(crate)
            else:
                failed.append(crate)
            print()
    elif args.check:
        print_info("Running checks...")
        print()
        for crate in target_crates:
            if check_crate(crate, args.release, args.verbose):
                successful.append(crate)
            else:
                failed.append(crate)
            print()
    elif args.test:
        print_info("Running tests...")
        print()
        for crate in target_crates:
            if test_crate(crate, args.verbose):
                successful.append(crate)
            else:
                failed.append(crate)
            print()
    else:
        print_info("Running build...")
        print()
        for crate in target_crates:
            if build_crate(crate, args.release, args.verbose):
                successful.append(crate)
            else:
                failed.append(crate)
            print()
    
    # Print summary
    print_summary(successful, failed)
    
    # Exit with appropriate code
    if failed:
        sys.exit(1)
    else:
        print_success("All crates completed successfully!")
        sys.exit(0)


if __name__ == '__main__':
    main()
