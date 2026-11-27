#!/usr/bin/env python3
"""阶段 B 出口 Gate 的可复现静态自检（B10）。

运行时证据（故障注入矩阵、全量测试）由 `cargo test` 单独产生并记录在路线图；
本脚本只检查可以从仓库文件确定性判定、且容易随后续改动漂移的条件：

1. CLI 主入口行数 < 300；
2. 库层没有 print/println/eprint/eprintln 宏（二进制入口单独排除）；
3. modeld-core 行覆盖率 >= 75% 且报告已入库；
4. CI 含三平台测试矩阵、coverage 与 bench 两个 job；
5. 路线图已勾选「文档 as-built 评审通过」；
6. 性能基线含 10 万文件扫描与 1 TiB 逻辑去重规划两项；
7. bench_gate.py 在当前基线上通过。

退出码：0 全部通过，1 至少一项失败，2 输入缺失。
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIBRARY_DIRS = [
    ROOT / "crates/modeld-core/src",
    ROOT / "crates/modeld-proxy/src",
    ROOT / "crates/modeld-client/src",
    ROOT / "crates/modeld-webui/src",
]
REQUIRED_BENCHMARKS = {
    "scan_64x1MiB_full_hash",
    "dedup_plan_100_groups",
    "scan_100k_files_full_hash",
    "dedup_plan_1tib_logical",
}


def check(name: str, ok: bool, detail: str) -> tuple[str, bool, str]:
    return name, ok, detail


def line_count(path: Path) -> int:
    return len(path.read_text(encoding="utf-8").splitlines())


def library_print_hits() -> list[str]:
    pattern = re.compile(r"\b(?:print|println|eprint|eprintln)!\s*\(")
    hits: list[str] = []
    for directory in LIBRARY_DIRS:
        for path in sorted(directory.rglob("*.rs")):
            # main.rs 是二进制入口，不属于库层。
            if path.name == "main.rs":
                continue
            for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                if pattern.search(line):
                    hits.append(f"{path.relative_to(ROOT).as_posix()}:{number}")
    return hits


def coverage_percent() -> float | None:
    report = ROOT / "coverage/core-summary.txt"
    if not report.exists():
        return None
    for line in report.read_text(encoding="utf-8").splitlines():
        if line.startswith("TOTAL"):
            match = re.search(r"([\d.]+)%", line)
            return float(match.group(1)) if match else None
    return None


def baseline_metrics() -> set[str]:
    import json

    path = ROOT / "bench/baseline.json"
    if not path.exists():
        return set()
    raw = json.loads(path.read_text(encoding="utf-8"))
    return {entry.get("name") for entry in raw.get("results", []) if isinstance(entry, dict)}


def run() -> list[tuple[str, bool, str]]:
    checks: list[tuple[str, bool, str]] = []

    main = ROOT / "crates/modeld-cli/src/main.rs"
    checks.append(
        check("CLI 主入口 < 300 行", main.exists() and line_count(main) < 300,
              f"main.rs = {line_count(main) if main.exists() else '缺失'} 行")
    )

    hits = library_print_hits()
    checks.append(check("库层零 print 宏", not hits, "命中：" + ", ".join(hits) if hits else "无命中"))

    coverage = coverage_percent()
    checks.append(
        check("core 覆盖率 >= 75%", coverage is not None and coverage >= 75.0,
              f"core-summary.txt TOTAL = {coverage if coverage is not None else '缺失'}")
    )

    ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    ci_ok = (
        "ubuntu-22.04" in ci
        and "windows-2022" in ci
        and "macos-15" in ci
        and re.search(r"^  coverage:\s*$", ci, re.M) is not None
        and re.search(r"^  bench:\s*$", ci, re.M) is not None
    )
    checks.append(check("CI 三平台 + coverage + bench", ci_ok, "检查 ci.yml 矩阵与 job 名"))

    roadmap = (ROOT / "docs/ROADMAP.md").read_text(encoding="utf-8")
    gate_ok = "- [x] 文档 as-built 评审通过" in roadmap
    checks.append(check("文档 as-built 评审已勾选", gate_ok, "检查 ROADMAP 阶段 B 出口 Gate"))

    metrics = baseline_metrics()
    checks.append(
        check("性能基线覆盖 100k/1TiB", REQUIRED_BENCHMARKS <= metrics,
              "缺少：" + ", ".join(sorted(REQUIRED_BENCHMARKS - metrics))
              if REQUIRED_BENCHMARKS - metrics else "四项基准全部存在")
    )

    gate = subprocess.run(
        [sys.executable, str(ROOT / "scripts/bench_gate.py"), "--threshold", "20"],
        cwd=ROOT,
        text=True,
        capture_output=True,
    )
    checks.append(
        check("bench_gate 20% 回归门禁", gate.returncode == 0,
              gate.stdout.strip().splitlines()[-1] if gate.stdout.strip() else gate.stderr.strip())
    )

    return checks


def main() -> int:
    checks = run()
    print(f"{'检查项':<32} {'结果':<6} 说明")
    print("-" * 78)
    for name, ok, detail in checks:
        print(f"{name:<32} {'PASS' if ok else 'FAIL':<6} {detail}")
    failed = [name for name, ok, _ in checks if not ok]
    if failed:
        print(f"\n未通过 {len(failed)} 项：{', '.join(failed)}", file=sys.stderr)
        return 1
    print("\n阶段 B 出口 Gate 静态检查全部通过")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
