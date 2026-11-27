#!/usr/bin/env python3
"""Reproducible as-built checks for ROADMAP C6 (performance).

These checks do not claim to measure a 1 TiB model corpus; they assert that
the optimizations C6 depends on are present in the shipped code paths, so a
later refactor cannot silently remove them. Actual timings come from
`cargo bench` / `scripts/bench_gate.py` and `scripts/bench-proxy-streaming.ps1`.
"""

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]

CHECKS: list[tuple[str, str, tuple[str, ...]]] = [
    (
        "parallel hashing for large files",
        "crates/modeld-core/src/hash.rs",
        ("SMALL_FILE_THRESHOLD", "update_rayon", "Mmap::map"),
    ),
    (
        "parallel directory scanning",
        "crates/modeld-core/src/scanner.rs",
        ("use rayon::prelude::*", ".par_iter()"),
    ),
    (
        "batched transactional ingest",
        "crates/modeld-core/src/ingest.rs",
        ("insert_wal_transaction", "ingest_scan_results"),
    ),
    (
        "single-query dedup planning",
        "crates/modeld-core/src/dedup.rs",
        ("find_duplicate_groups()", "avoids N+1"),
    ),
    (
        "streaming proxy without whole-object buffering",
        "crates/modeld-proxy/src/server.rs",
        ("fn stream_full", "fn stream_range", "file.take("),
    ),
    (
        "cursor-backed incremental GC",
        "crates/modeld-core/src/gc.rs",
        ("pub struct GcCursor", "run_safe_incremental", "get_gc_candidate_counts_after"),
    ),
    (
        "paged GC candidate query",
        "crates/modeld-core/src/db.rs",
        ("get_gc_candidate_counts_after", "ORDER BY m.blake3_hash ASC"),
    ),
    (
        "incremental GC CLI surface",
        "crates/modeld-cli/src/cli.rs",
        ("incremental: bool", "batch_size: usize"),
    ),
]


def main() -> int:
    failures: list[str] = []
    for label, relative, needles in CHECKS:
        path = ROOT / relative
        if not path.exists():
            failures.append(f"{label}: missing {relative}")
            continue
        text = path.read_text(encoding="utf-8")
        missing = [needle for needle in needles if needle not in text]
        if missing:
            failures.append(f"{label}: missing {missing} in {relative}")

    if failures:
        print("C6 static gate: FAILED")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    print(f"C6 static gate: OK ({len(CHECKS)} as-built invariants)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
