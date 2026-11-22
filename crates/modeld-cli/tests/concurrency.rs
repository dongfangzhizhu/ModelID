//! Cross-process concurrency tests (ROADMAP B7).
//!
//! Acceptance: "并发测试：10 进程同时 dedup，无数据损坏、无死锁".
//!
//! This drives the real binary, not library functions: the whole point is that
//! *separate processes* serialise on `<store>/.modeld.lock`. Threads inside one
//! process would not prove that.
//!
//! Every process must exit 0 (they queue on `--wait`), the store must still
//! verify clean afterwards, and the savings must be the same as a single run —
//! a loser that half-applied its plan would show up as a broken alias or a
//! duplicate object.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Path to the binary under test (provided by cargo for integration tests).
fn modeld() -> Command {
    Command::new(env!("CARGO_BIN_EXE_modeld"))
}

fn run(store: &Path, args: &[&str]) -> std::process::Output {
    let out = modeld()
        .args(args)
        .arg("--store")
        .arg(store)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("failed to run modeld {args:?}: {e}"));
    assert!(
        out.status.success(),
        "modeld {args:?} failed: status={:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Build a store with three identical 1 MiB files plus one distinct file.
fn prepare_store(root: &Path) -> (PathBuf, PathBuf) {
    let store = root.join("store");
    let lib = root.join("lib");
    std::fs::create_dir_all(&lib).unwrap();

    let payload = vec![0xA5u8; 1024 * 1024];
    for name in ["a.safetensors", "b.safetensors", "c.safetensors"] {
        std::fs::write(lib.join(name), &payload).unwrap();
    }
    std::fs::write(lib.join("unique.safetensors"), vec![0x11u8; 512 * 1024]).unwrap();

    run(&store, &["init"]);
    run(&store, &["scan", lib.to_str().unwrap(), "--copy", "--full"]);
    (store, lib)
}

fn stats_json(store: &Path) -> serde_json::Value {
    let out = run(store, &["stats", "--json"]);
    serde_json::from_slice(&out.stdout).expect("stats --json is valid JSON")
}

/// Wait for every child, killing the set if the deadline passes.
///
/// A deadlock must fail the test, not hang CI: this is the "无死锁" half of the
/// acceptance criterion.
fn wait_all(
    children: Vec<std::process::Child>,
    deadline: Duration,
) -> Vec<std::process::ExitStatus> {
    let mut statuses = Vec::new();
    let end = Instant::now() + deadline;
    for mut child in children {
        loop {
            match child.try_wait().expect("try_wait") {
                Some(status) => {
                    statuses.push(status);
                    break;
                }
                None => {
                    if Instant::now() >= end {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!(
                            "a dedup process did not finish within {deadline:?} — \
                             deadlock or unbounded lock wait"
                        );
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
    statuses
}

/// Ten processes dedup the same store at once: all must succeed, and the store
/// must be consistent afterwards.
#[test]
fn ten_concurrent_dedup_processes_do_not_corrupt_the_store() {
    let root = tempfile::tempdir().unwrap();
    let (store, lib) = prepare_store(root.path());

    let before = stats_json(&store);
    assert_eq!(before["duplicate_waste_bytes"], 2 * 1024 * 1024, "two redundant copies");

    // All ten queue on the same lock. `--wait` is generous so the assertion is
    // about correctness, not about scheduling speed.
    let mut children = Vec::new();
    for _ in 0..10 {
        let child = modeld()
            .args(["dedup", "--auto", "--wait", "120", "--store"])
            .arg(&store)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn dedup");
        children.push(child);
    }

    let statuses = wait_all(children, Duration::from_secs(180));
    for (i, status) in statuses.iter().enumerate() {
        assert_eq!(status.code(), Some(0), "dedup process {i} exited with {status:?}");
    }

    // 1. The store verifies: CAS objects, aliases and DB agree.
    let verify = run(&store, &["verify", "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&verify.stdout).expect("verify JSON");
    assert_eq!(
        report["clean"], true,
        "store failed consistency verification after concurrent dedup: {report}"
    );
    assert_eq!(report["total_issues"], 0, "store reported issues after concurrent dedup: {report}");

    // 2. Savings are exactly what one run would produce — no double counting,
    //    no lost links.
    let after = stats_json(&store);
    assert_eq!(after["duplicate_waste_bytes"], 0, "all copies deduplicated");
    assert_eq!(after["deduped_saved_bytes"], 2 * 1024 * 1024);
    assert_eq!(after["physical_bytes"], 1536 * 1024, "one 1 MiB + one 512 KiB object");

    // 3. The files are still readable and share storage (hard links on one volume).
    for name in ["a.safetensors", "b.safetensors", "c.safetensors"] {
        let bytes = std::fs::read(lib.join(name)).expect("file still readable");
        assert_eq!(bytes.len(), 1024 * 1024);
        assert!(bytes.iter().all(|b| *b == 0xA5), "{name} content changed");
    }

    // 4. No lock file left locked, and no staging leftovers.
    assert!(
        modeld_core::StoreLock::try_acquire(&store, "after").is_ok(),
        "the store lock is still held after every process exited"
    );
    let staging = store.join("tmp").join("cas_staging");
    if staging.exists() {
        let leftovers: Vec<_> = std::fs::read_dir(&staging).unwrap().flatten().collect();
        assert!(leftovers.is_empty(), "staging leftovers: {leftovers:?}");
    }
}
