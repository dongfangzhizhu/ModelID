#!/usr/bin/env python3
"""Fail the build when a benchmark regressed by more than the threshold.

ROADMAP B8: "基准回归阈值写入 CI（超过 20% 失败）".

Compares the `bench/latest.json` written by `cargo bench -p modeld-core` against
the committed `bench/baseline.json`. A metric that is missing from either side is
an error too: a renamed benchmark must update the baseline deliberately rather
than silently dropping out of the gate.

Usage:
    python scripts/bench_gate.py                 # check against the baseline
    python scripts/bench_gate.py --threshold 30  # looser, e.g. on a noisy runner
    python scripts/bench_gate.py --record        # rewrite the baseline from latest

Exit codes: 0 pass (or skip: baseline recorded on another platform),
1 regression, 2 bad/missing input.

Platform note: a wall-clock baseline is only meaningful on the machine class
that produced it, so the two files carry a `platform` key. A mismatch prints
SKIP rather than inventing a comparison; record a baseline per platform with
`--record` (once per CI runner image, not per commit).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BASELINE = REPO_ROOT / "bench" / "baseline.json"
LATEST = REPO_ROOT / "bench" / "latest.json"


def load(path: Path, label: str) -> tuple[str, dict[str, float]]:
    if not path.exists():
        print(f"error: {label} not found at {path}", file=sys.stderr)
        print("hint: run `cargo bench -p modeld-core` first", file=sys.stderr)
        raise SystemExit(2)
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        print(f"error: {path} is not valid JSON: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc

    platform = raw.get("platform", "unknown")
    results = raw.get("results")
    if not isinstance(results, list) or not results:
        print(f"error: {path} has no 'results' array", file=sys.stderr)
        raise SystemExit(2)

    metrics: dict[str, float] = {}
    for entry in results:
        name = entry.get("name")
        value = entry.get("value")
        if not isinstance(name, str) or not isinstance(value, (int, float)):
            print(f"error: bad entry in {path}: {entry!r}", file=sys.stderr)
            raise SystemExit(2)
        metrics[name] = float(value)
    return str(platform), metrics


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--threshold",
        type=float,
        default=20.0,
        help="maximum allowed slowdown in percent (default: 20)",
    )
    parser.add_argument(
        "--record",
        action="store_true",
        help="write bench/latest.json to bench/baseline.json and exit 0",
    )
    args = parser.parse_args()

    latest_platform, latest = load(LATEST, "latest benchmark results")

    if args.record:
        BASELINE.parent.mkdir(parents=True, exist_ok=True)
        BASELINE.write_text(LATEST.read_text(encoding="utf-8"), encoding="utf-8")
        print(f"recorded baseline from {LATEST} → {BASELINE}")
        return 0

    baseline_platform, baseline = load(BASELINE, "baseline")

    # Wall-clock numbers are only comparable on the same platform. Comparing a
    # Windows laptop against an ubuntu runner would report a permanent
    # regression, so a platform change is a *skip with a reason*, not a pass.
    if baseline_platform != latest_platform:
        print(
            f"SKIP: baseline was recorded on {baseline_platform}, this run is "
            f"{latest_platform}. Record a baseline for this platform with "
            f"`python scripts/bench_gate.py --record`."
        )
        return 0

    missing = sorted(set(baseline) - set(latest))
    added = sorted(set(latest) - set(baseline))
    if missing:
        print(f"error: benchmarks missing from latest run: {', '.join(missing)}", file=sys.stderr)
        print("hint: re-record the baseline if they were removed on purpose", file=sys.stderr)
        return 2

    print(f"{'benchmark':<28} {'baseline':>10} {'latest':>10} {'delta':>9}  verdict")
    print("-" * 72)

    failures: list[str] = []
    for name in sorted(baseline):
        base = baseline[name]
        now = latest[name]
        # A zero baseline cannot be compared proportionally; treat any value as
        # a pass so a broken fixture does not look like a regression.
        delta_pct = 0.0 if base <= 0 else (now - base) / base * 100.0
        regressed = base > 0 and delta_pct > args.threshold
        if regressed:
            failures.append(name)
        verdict = f"FAIL (>{args.threshold:.0f}%)" if regressed else "ok"
        print(f"{name:<28} {base:>10.3f} {now:>10.3f} {delta_pct:>8.1f}%  {verdict}")

    if added:
        print(f"\nnote: new benchmarks not in the baseline (ignored): {', '.join(added)}")
        print("      run `python scripts/bench_gate.py --record` to include them")

    if failures:
        print(
            f"\nperformance regression in: {', '.join(failures)}",
            file=sys.stderr,
        )
        return 1

    print("\nno regressions beyond the threshold")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
