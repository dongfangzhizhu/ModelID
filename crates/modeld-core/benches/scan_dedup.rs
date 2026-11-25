//! Benchmarks for the two hot paths: hashing a library and planning a dedup.
//!
//! Run with `cargo bench -p modeld-core` (release profile). Results are written
//! to `bench/latest.json` *and* printed, so CI can diff them against
//! `bench/baseline.json` with `scripts/bench_gate.py`, which fails the build if a
//! metric regressed by more than 20%.
//!
//! Deliberately dependency-free (`harness = false`): no `criterion`, because the
//! gate only needs a coarse, stable number — "did this get 20% slower" — and a
//! new dev-dependency for that would be a poor trade. Each benchmark runs a few
//! warm-up iterations first and reports the *median* of the measured ones, which
//! is enough to ignore the noise that matters here (cold page cache, antivirus).
//!
//! What is measured is the engine, not the CLI shell: `Scanner::scan` over a
//! synthetic library, and `DedupEngine::plan` over a store with one duplicated
//! group per size class. Two additional baselines close the stage B exit gate:
//! a 100,000-file full scan and a 1 TiB *logical* dedup plan. The latter is a
//! metadata-scale benchmark (SQLite rows plus tiny placeholder paths); it does
//! not read or write 1 TiB of model bytes, and the result must be read as the
//! planning/aggregation cost at that logical size, not whole-store IO.
//!
//! Fixtures are written to a unique `target/bench` subdirectory per run and
//! deliberately not bulk-deleted: a recursive delete of 100k files can be
//! blocked by the sandbox, and the directory is ignored by git.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use modeld_core::{Blake3Hash, CasStore, Database, DedupEngine, ScanOptions, Scanner};

/// A benchmark result: stable name, milliseconds, and what it measured.
struct Measurement {
    name: &'static str,
    unit: &'static str,
    value: f64,
}

fn main() {
    let dir = workspace_temp_dir("bench-scan-dedup");

    let results = vec![
        measure_scan(&dir),
        measure_dedup_plan(&dir),
        measure_scan_100k(&dir),
        measure_dedup_plan_1tib(&dir),
    ];

    // Print a human-readable table…
    println!("\n{:<24} {:>12}  unit", "benchmark", "median");
    println!("{}", "-".repeat(52));
    for m in &results {
        println!("{:<24} {:>12.3}  {}", m.name, m.value, m.unit);
    }

    // …and a machine-readable file for the gate.
    //
    // The platform is recorded because a wall-clock baseline only means anything
    // on the machine class that produced it: comparing a Windows laptop's
    // numbers against an ubuntu CI runner would report a permanent "regression".
    // `scripts/bench_gate.py` refuses to compare across platforms.
    let json = format!(
        "{{\n  \"generated_by\": \"cargo bench -p modeld-core\",\n  \"platform\": \"{}\",\n  \"results\": [\n{}\n  ]\n}}\n",
        std::env::consts::OS.to_string() + "-" + std::env::consts::ARCH,
        results
            .iter()
            .map(|m| format!(
                "    {{ \"name\": \"{}\", \"value\": {:.6}, \"unit\": \"{}\" }}",
                m.name, m.value, m.unit
            ))
            .collect::<Vec<_>>()
            .join(",\n")
    );
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench");
    std::fs::create_dir_all(&out_dir).expect("create bench/ dir");
    let out = out_dir.join("latest.json");
    std::fs::write(&out, json).expect("write bench results");
    println!("\nwrote {}", out.display());
}

/// Median wall-clock time of `iterations` runs, after `warmup` untimed ones.
fn median_ms(warmup: usize, iterations: usize, mut run: impl FnMut()) -> f64 {
    for _ in 0..warmup {
        run();
    }
    let mut samples: Vec<Duration> = (0..iterations)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed()
        })
        .collect();
    samples.sort();
    let median = samples[samples.len() / 2];
    median.as_secs_f64() * 1000.0
}

/// Hashing dominates every ingest: 64 files × 1 MiB, cold-ish each iteration.
fn measure_scan(root: &Path) -> Measurement {
    let lib = root.join("scan-lib");
    let _ = std::fs::remove_dir_all(&lib);
    std::fs::create_dir_all(&lib).expect("create lib");

    // Distinct content per file, so nothing can be short-circuited by a hash
    // cache: 64 MiB total.
    let chunk = vec![0x3Cu8; 1024 * 1024];
    for i in 0..64u8 {
        let mut bytes = chunk.clone();
        bytes[0] = i;
        bytes[1] = i.wrapping_mul(7);
        std::fs::write(lib.join(format!("model-{i:02}.safetensors")), &bytes).expect("write model");
    }

    let value = median_ms(1, 3, || {
        let scanner = Scanner::new().with_scan_options(ScanOptions {
            incremental: false,
            full: true,
            exclude_globs: vec![],
            follow_symlinks: false,
        });
        let files = scanner.scan(&lib, |_, _| {}).expect("scan");
        assert_eq!(files.len(), 64, "benchmark fixture should yield 64 files");
    });

    Measurement { name: "scan_64x1MiB_full_hash", unit: "ms", value }
}

/// Dedup planning walks every alias and groups by hash: 200 paths over 100
/// duplicated groups.
fn measure_dedup_plan(root: &Path) -> Measurement {
    let store = root.join("dedup-store");
    let _ = std::fs::remove_dir_all(&store);
    std::fs::create_dir_all(&store).expect("create store");

    let db = Database::open(&store.join("modeld.db")).expect("open db");
    let cas = CasStore::new(&store);
    cas.init().expect("init cas");

    let lib = root.join("dedup-lib");
    let _ = std::fs::remove_dir_all(&lib);
    std::fs::create_dir_all(&lib).expect("create lib");

    // 100 contents, each present twice → 100 groups, 100 reclaimable copies.
    let mut db = db;
    for i in 0..100u16 {
        let bytes = vec![i as u8; 4096];
        let a = lib.join(format!("g{i}-a.safetensors"));
        let b = lib.join(format!("g{i}-b.safetensors"));
        std::fs::write(&a, &bytes).expect("write a");
        std::fs::write(&b, &bytes).expect("write b");

        let hash = modeld_core::hash_file(&a).expect("hash");
        cas.store_crash_safe(&a, &hash, "bench-setup").expect("store object");
        db.insert_or_update_model(&hash, 4096, None, None, None, None).expect("model row");
        db.insert_alias(
            &hash,
            &a.to_string_lossy(),
            modeld_core::db::Frontend::User,
            modeld_core::db::AliasType::Original,
        )
        .expect("alias a");
        db.insert_alias(
            &hash,
            &b.to_string_lossy(),
            modeld_core::db::Frontend::User,
            modeld_core::db::AliasType::Original,
        )
        .expect("alias b");
    }

    let value = median_ms(1, 3, || {
        // `plan` is read-only, so running it repeatedly over one store is fair.
        let engine =
            DedupEngine::new(Database::open(&store.join("modeld.db")).expect("db"), store.clone());
        let plan = engine.plan_dedup().expect("plan");
        assert_eq!(plan.groups.len(), 100, "benchmark fixture should produce 100 duplicate groups");
    });

    Measurement { name: "dedup_plan_100_groups", unit: "ms", value }
}

/// Stage B exit gate: full scan over 100,000 model-shaped files.
///
/// Files are tiny (one byte) on purpose — the gate is about path enumeration
/// and per-file hash dispatch at 100k scale, not about disk throughput. The
/// fixture is created outside the timed section.
fn measure_scan_100k(root: &Path) -> Measurement {
    const FILES: u32 = 100_000;
    let lib = root.join("scan-100k");
    std::fs::create_dir_all(&lib).expect("create 100k lib");

    for i in 0..FILES {
        let path = lib.join(format!("m-{i:06}.safetensors"));
        std::fs::write(&path, [i as u8]).expect("write 100k fixture file");
    }

    // Creating 100k entries is already slow on Windows; one measured run with
    // no warm-up is enough for a regression gate, and keeps CI time bounded.
    let value = median_ms(0, 1, || {
        let scanner = Scanner::new().with_scan_options(ScanOptions {
            incremental: false,
            full: true,
            exclude_globs: vec![],
            follow_symlinks: false,
        });
        let files = scanner.scan(&lib, |_, _| {}).expect("scan 100k");
        assert_eq!(files.len(), FILES as usize, "100k benchmark fixture changed");
    });

    Measurement { name: "scan_100k_files_full_hash", unit: "ms", value }
}

/// Stage B exit gate: dedup planning at 1 TiB of logical size.
///
/// 1,000 model rows × 1 GiB = 1 TiB of unique content, each with two original
/// paths, so the plan reports exactly 1 TiB of reclaimable bytes. Only the
/// metadata and tiny placeholder files are materialised; this measures the
/// planning/aggregation path at that logical size, not 1 TiB of IO.
fn measure_dedup_plan_1tib(root: &Path) -> Measurement {
    const MODELS: u32 = 1_000;
    const GIB: u64 = 1 << 30;

    let store = root.join("dedup-1tib-store");
    std::fs::create_dir_all(store.join("shadow")).expect("create 1TiB store");
    let mut db = Database::open(&store.join("modeld.db")).expect("open 1TiB db");

    for i in 0..MODELS {
        let hash = Blake3Hash::from_hex(&format!("{:064x}", i + 1)).expect("valid hash");
        db.insert_or_update_model(&hash, GIB as i64, None, None, None, None)
            .expect("insert 1GiB model");
        for side in ["a", "b"] {
            let path = store.join("shadow").join(format!("{i:04}-{side}.safetensors"));
            std::fs::write(&path, b"x").expect("write placeholder path");
            db.insert_alias(
                &hash,
                &path.to_string_lossy(),
                modeld_core::db::Frontend::User,
                modeld_core::db::AliasType::Original,
            )
            .expect("insert alias");
        }
    }

    let value = median_ms(1, 3, || {
        let engine =
            DedupEngine::new(Database::open(&store.join("modeld.db")).expect("db"), store.clone());
        let plan = engine.plan_dedup().expect("plan 1TiB");
        assert_eq!(plan.groups.len(), MODELS as usize, "1TiB fixture changed");
        assert_eq!(plan.total_savings_bytes, GIB * MODELS as u64, "1TiB savings changed");
    });

    Measurement { name: "dedup_plan_1tib_logical", unit: "ms", value }
}

/// A unique temp dir next to the target dir, so the benchmark never measures
/// the sandbox's `%TEMP%` (which may be on another, slower, volume). Each run
/// gets its own directory: bulk-deleting a previous 100k-file fixture can be
/// blocked by the sandbox, and `target/` is ignored by git.
fn workspace_temp_dir(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bench");
    let unique = format!(
        "{}-{}",
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let dir = base.join(unique);
    std::fs::create_dir_all(&dir).expect("create bench temp dir");
    dir
}
