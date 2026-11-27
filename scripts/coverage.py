#!/usr/bin/env python3
"""Line coverage for `modeld-core`, with a 75% gate (ROADMAP B8).

Uses the LLVM tools shipped by `rustup component add llvm-tools-preview`
directly. `cargo-llvm-cov` would be nicer but needs crates.io, which is not
reachable in this environment; the raw tools do the same job:

  1. build the core unit + integration tests with `-C instrument-coverage`
  2. run them with `LLVM_PROFILE_FILE` pointing at `coverage/`
  3. `llvm-profdata merge` the `.profraw` files
  4. `llvm-cov report` and `llvm-cov export` for the summary

The report is committed under `coverage/`; the `.profraw` / `.profdata`
artifacts are generated and ignored.

Usage:
    python scripts/coverage.py            # measure and enforce the 75% gate
    python scripts/coverage.py --no-gate  # just report, never fail
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
COVERAGE_DIR = REPO_ROOT / "coverage"
PROFRAW_DIR = REPO_ROOT / "target" / "coverage"
MIN_LINES = 75.0


def run(cmd: list[str], *, env: dict[str, str] | None = None, check: bool = True) -> subprocess.CompletedProcess:
    print("+", " ".join(cmd), flush=True)
    return subprocess.run(cmd, cwd=REPO_ROOT, env=env, check=check, text=True)


def toolchain_bin() -> Path:
    sysroot = subprocess.run(
        ["rustc", "--print", "sysroot"], check=True, capture_output=True, text=True
    ).stdout.strip()
    suffix = ".exe" if sys.platform == "win32" else ""
    bin_dir = Path(sysroot) / "lib" / "rustlib" / machine_triple() / "bin"
    llvm_cov = bin_dir / f"llvm-cov{suffix}"
    llvm_profdata = bin_dir / f"llvm-profdata{suffix}"
    for tool in (llvm_cov, llvm_profdata):
        if not tool.exists():
            print(
                f"error: {tool} not found. Install the tools with:\n"
                "    rustup component add llvm-tools-preview",
                file=sys.stderr,
            )
            raise SystemExit(2)
    return bin_dir


def machine_triple() -> str:
    out = subprocess.run(
        ["rustc", "-vV"], check=True, capture_output=True, text=True
    ).stdout
    match = re.search(r"host:\s+(\S+)", out)
    if not match:
        raise SystemExit("cannot determine rustc host triple from `rustc -vV`")
    return match.group(1)


def find_test_binaries(env: dict[str, str]) -> list[Path]:
    """Locate every instrumented `modeld-core` test executable.

    Cargo 在 `RUSTFLAGS=-C instrument-coverage` 下会把 lib 单测可执行文件
    放到 `target/debug/build/<pkg>-<hash>/out/`，而不是 `target/debug/deps/`；
    旧脚本只扫 deps/，在 CI 上就会误判为“找不到测试二进制”。先问 cargo 要
    JSON 产物清单，拿到本次插桩构建的真实路径（lib 单测 + 各集成测试）；
    失败时再递归扫描兜底。
    """
    probe = subprocess.run(
        [
            "cargo", "test", "-p", "modeld-core", "--locked",
            "--no-run", "--message-format=json",
        ],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    found: list[Path] = []
    if probe.returncode == 0:
        for line in probe.stdout.splitlines():
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if msg.get("reason") != "compiler-artifact":
                continue
            if "modeld-core" not in (msg.get("package_id") or ""):
                continue
            profile = msg.get("profile") or {}
            executable = msg.get("executable")
            if profile.get("test") is True and executable:
                candidate = Path(executable)
                if not candidate.is_absolute():
                    candidate = (REPO_ROOT / candidate).resolve()
                if candidate.is_file() and candidate not in found:
                    found.append(candidate)
    if found:
        return found

    # 兜底：兼容旧版 cargo 只把 lib 单测二进制写在 deps/ 的行为。
    debug = REPO_ROOT / "target" / "debug"
    candidates = sorted(
        p for p in debug.rglob("modeld_core-*")
        if p.is_file() and p.suffix in {".exe", ""} and os.access(p, os.X_OK)
    )
    if not candidates:
        raise SystemExit(f"no modeld_core test binary found under {debug}")
    return [max(candidates, key=lambda p: p.stat().st_mtime_ns)]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-gate", action="store_true", help="report but do not fail on low coverage")
    args = parser.parse_args()

    bin_dir = toolchain_bin()
    suffix = ".exe" if sys.platform == "win32" else ""
    llvm_cov = bin_dir / f"llvm-cov{suffix}"
    llvm_profdata = bin_dir / f"llvm-profdata{suffix}"

    COVERAGE_DIR.mkdir(exist_ok=True)
    # One directory per run instead of deleting the previous run's profraw
    # files: bulk deletion is both unnecessary here and blocked by a number of
    # sandboxes. `target/` is ignored by git, so old runs cost nothing.
    run_dir = PROFRAW_DIR / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%f")
    run_dir.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " -C instrument-coverage").strip()
    env["LLVM_PROFILE_FILE"] = str(run_dir / "modeld_core-%p-%m.profraw")

    # Build and run in one command so cargo picks up the instrumented binary.
    # A non-zero exit here is *not* fatal for this script: coverage data was
    # still written, and the normal `rust` CI job is the test gate. Failing
    # both jobs for one slow instrumented test would hide the coverage result.
    tests = run(["cargo", "test", "-p", "modeld-core", "--locked"], env=env, check=False)
    if tests.returncode != 0:
        print(
            "WARNING: the instrumented test run exited non-zero "
            f"({tests.returncode}); producing the coverage report anyway. "
            "Use the regular test job for pass/fail.",
            file=sys.stderr,
        )

    profraws = list(run_dir.glob("*.profraw"))
    if not profraws:
        print("error: no .profraw files were produced — instrumentation did not run", file=sys.stderr)
        return 2

    profdata = run_dir / "core.profdata"
    run([
        str(llvm_profdata), "merge", "-sparse",
        *(str(p) for p in profraws),
        "-o", str(profdata),
    ])

    binaries = find_test_binaries(env)
    print("using test binaries:")
    for binary in binaries:
        print(f"  {binary}")

    # The raw LLVM export contains every instrumented crate (std, the whole
    # dependency tree) — ~35 MB on this machine. Filter it down to
    # `crates/modeld-core/src` ourselves. `llvm-cov -ignore-filename-regex`
    # cannot do that portably: on Windows the paths use backslashes, so the
    # usual `registry/src` pattern silently misses everything and the reported
    # "TOTAL" is a meaningless ~15%.
    export = subprocess.run(
        [
            str(llvm_cov), "export", *(str(binary) for binary in binaries),
            f"-instr-profile={profdata}",
            "-format=text",
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    (run_dir / "export.json").write_text(export, encoding="utf-8")

    payload = json.loads(export)
    core_prefix = (REPO_ROOT / "crates" / "modeld-core" / "src")
    def norm(path: str) -> str:
        return path.replace("\\", "/").rstrip("/").lower()
    prefix = norm(str(core_prefix))

    rows: list[tuple[str, int, int, float]] = []
    for entry in payload["data"][0]["files"]:
        filename = entry["filename"]
        if not norm(filename).startswith(prefix + "/"):
            continue
        lines = entry["summary"]["lines"]
        count, covered = int(lines["count"]), int(lines["covered"])
        pct = 100.0 * covered / count if count else 0.0
        rows.append((filename, count, covered, pct))

    if not rows:
        print("error: no modeld-core source files found in the coverage export", file=sys.stderr)
        return 2

    total_count = sum(r[1] for r in rows)
    total_covered = sum(r[2] for r in rows)
    lines_pct = 100.0 * total_covered / total_count if total_count else 0.0

    # Small, readable report committed as the acceptance evidence.
    report_lines = [
        "modeld-core line coverage (unit + integration tests, `crates/modeld-core/src` only)",
        "",
        f"{'file':<28} {'lines':>7} {'covered':>8} {'%':>7}",
        "-" * 54,
    ]
    short = str(core_prefix).replace("\\", "/").lower() + "/"
    for filename, count, covered, pct in sorted(rows, key=lambda r: r[0]):
        report_lines.append(
            f"{filename.replace(chr(92), '/').lower().removeprefix(short):<28} "
            f"{count:>7} {covered:>8} {pct:>6.1f}%"
        )
    report_lines += [
        "-" * 54,
        f"{'TOTAL (modeld-core only)':<28} {total_count:>7} {total_covered:>8} {lines_pct:>6.1f}%",
        "",
        "Dependencies and std are excluded: including them made the same data",
        "look like 15% coverage on Windows because llvm-cov's -ignore regex",
        "did not match backslash paths.",
    ]
    (COVERAGE_DIR / "core-summary.txt").write_text("\n".join(report_lines) + "\n", encoding="utf-8")
    (COVERAGE_DIR / "core-summary.json").write_text(
        json.dumps(
            {
                "scope": "crates/modeld-core/src",
                "tool": "rustc -C instrument-coverage + llvm-profdata/llvm-cov",
                "line_count": total_count,
                "line_covered": total_covered,
                "line_percent": round(lines_pct, 2),
                "files": [
                    {"file": r[0], "count": r[1], "covered": r[2], "percent": round(r[3], 2)}
                    for r in rows
                ],
            },
            indent=2,
            ensure_ascii=False,
        ) + "\n",
        encoding="utf-8",
    )
    (COVERAGE_DIR / "core-line-coverage.txt").write_text(f"{lines_pct:.2f}\n", encoding="utf-8")
    print("\n".join(report_lines[-8:]))
    print(f"modeld-core line coverage: {lines_pct:.2f}% (gate: {MIN_LINES:.0f}%)")

    if lines_pct < MIN_LINES and not args.no_gate:
        print(f"FAIL: line coverage {lines_pct:.2f}% is below {MIN_LINES:.0f}%", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
