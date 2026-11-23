//! Environment-class tests (ROADMAP B8: "跨卷/挂载点/只读目录/文件锁 4 类环境测试").
//!
//! These are the four environments where a model store stops being a normal
//! directory and the assumptions in `links.rs` / `cas.rs` / `store_lock.rs` get
//! tested:
//!
//! | # | Class          | What must happen                                            |
//! |---|----------------|-------------------------------------------------------------|
//! | 1 | Cross-volume   | `--adopt` refuses with a clear error instead of copying (a silent copy is a 10 GiB surprise) |
//! | 2 | Mount point    | A junction loop must not send the walker into an infinite descent |
//! | 3 | Read-only      | An unwritable store layout fails loudly and leaves the library untouched |
//! | 4 | File lock      | A second mutator is refused with the holder's identity, and writes nothing |
//!
//! Classes that cannot be exercised on the current machine (no second volume, no
//! junction privilege) **skip with a printed reason** rather than passing
//! silently: a skipped environment test is information, a fake pass is not.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn modeld() -> Command {
    Command::new(env!("CARGO_BIN_EXE_modeld"))
}

/// Run the CLI with a deadline; kill it if it exceeds it.
///
/// Every test here could hang for a legitimate-looking reason (a walker stuck in
/// a junction loop, a lock that never releases). Letting the suite hang would
/// hide exactly the bug the test is looking for, so a timeout is a failure.
fn run_deadline(store: &Path, args: &[&str], deadline: Duration) -> (Option<i32>, String, String) {
    let mut child = modeld()
        .args(args)
        .arg("--store")
        .arg(store)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn modeld");

    let end = Instant::now() + deadline;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => {
                let out = child.wait_with_output().expect("collect output");
                return (
                    status.code(),
                    String::from_utf8_lossy(&out.stdout).into_owned(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                );
            }
            None => {
                if Instant::now() >= end {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "modeld {args:?} did not finish within {deadline:?} — \
                         it is stuck (infinite walk or an unreleased lock)"
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn write_model(path: &Path, marker: u8) {
    std::fs::write(path, vec![marker; 128 * 1024]).expect("write model file");
}

// ─────────────────────────────────────────────────────────────────────────────
// 1 — Cross-volume
// ─────────────────────────────────────────────────────────────────────────────

/// A directory that is writable and on a **different volume** from `other`.
fn other_volume_dir(other: &Path) -> Option<PathBuf> {
    for root in ["C:\\", "D:\\", "E:\\"] {
        let root = PathBuf::from(root);
        if !root.exists() {
            continue;
        }
        // One probe dir per volume, created through the normal tempfile path so
        // the test never invents a location the sandbox forbids.
        let candidate = root.join("modeld-volume-probe");
        if std::fs::create_dir_all(&candidate).is_err() {
            continue;
        }
        if !modeld_core::is_same_volume(&candidate, other) {
            return Some(candidate);
        }
        let _ = std::fs::remove_dir(&candidate);
    }
    None
}

#[test]
fn adopt_across_volumes_is_refused_instead_of_silently_copying() {
    let tmp = tempfile::TempDir::new().unwrap();
    let Some(foreign) = other_volume_dir(tmp.path()) else {
        eprintln!(
            "SKIP: this machine has a single volume, so the cross-volume contract \
             (adopt must refuse rather than copy) cannot be exercised here"
        );
        return;
    };

    let store = tmp.path().join("store");
    let lib = foreign.join("cross-volume-lib");
    std::fs::create_dir_all(&lib).unwrap();
    write_model(&lib.join("model.safetensors"), 0x11);

    // `init` lives on the store's volume, the library on the other one.
    let init_args = ["init", "--path", store.to_str().unwrap()];
    let (code, _out, _err) = run_deadline(&store, &init_args, Duration::from_secs(60));
    assert_eq!(code, Some(0), "init should succeed");

    let (code, out, err) =
        run_deadline(&store, &["scan", lib.to_str().unwrap(), "--adopt"], Duration::from_secs(120));

    assert_ne!(code, Some(0), "adopt across volumes must fail, not fall back to a copy: {err}");
    // The per-file reason is printed to stdout (the pretty report); only the
    // summary goes to stderr. Both must be considered to check the message.
    let combined = format!("{out}\n{err}").to_lowercase();
    assert!(
        combined.contains("cross-volume") || combined.contains("cross volume"),
        "the error should name the cross-volume condition so the user knows to pass --copy: \
         stdout={out} stderr={err}"
    );

    // And nothing may have been duplicated into the store.
    let cas = store.join("cas");
    let bytes = dir_size(&cas);
    assert_eq!(bytes, 0, "a refused adopt must not copy bytes into the store ({bytes} written)");

    // The original is untouched.
    assert_eq!(std::fs::read(lib.join("model.safetensors")).unwrap().len(), 128 * 1024);

    let _ = std::fs::remove_dir_all(&lib);
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.metadata() {
            Ok(meta) if meta.is_dir() => total += dir_size(&path),
            Ok(meta) => total += meta.len(),
            Err(_) => {}
        }
    }
    total
}

// ─────────────────────────────────────────────────────────────────────────────
// 2 — Mount point / junction loop
// ─────────────────────────────────────────────────────────────────────────────

/// Create a directory junction. No administrator rights needed, unlike symlinks.
#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) -> bool {
    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("run mklink");
    output.status.success()
}

#[cfg(unix)]
fn create_junction(link: &Path, target: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[test]
fn a_junction_loop_does_not_hang_the_walker() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    write_model(&lib.join("a.safetensors"), 0x22);
    write_model(&lib.join("b.safetensors"), 0x33);

    // `lib/loop -> lib`: any walker that follows it descends forever.
    if !create_junction(&lib.join("loop"), &lib) {
        eprintln!("SKIP: cannot create a junction here (privilege or filesystem limitation)");
        return;
    }

    let init_args = ["init", "--path", store.to_str().unwrap()];
    let (code, _out, _err) = run_deadline(&store, &init_args, Duration::from_secs(60));
    assert_eq!(code, Some(0));

    // Default traversal must not follow the loop, and must finish.
    let (code, _out, err) = run_deadline(
        &store,
        &["scan", lib.to_str().unwrap(), "--copy", "--full"],
        Duration::from_secs(120),
    );
    assert_eq!(code, Some(0), "scan over a junction loop failed: {err}");

    // Each real file exactly once — not "2 × depth", not zero.
    let stats = run_deadline(&store, &["stats", "--json"], Duration::from_secs(60));
    let value: serde_json::Value =
        serde_json::from_slice(stats.1.as_bytes()).expect("stats --json");
    assert_eq!(
        value["total_models"], 2,
        "the loop should yield the two real files exactly once: {value}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 3 — Read-only / unwritable store layout
// ─────────────────────────────────────────────────────────────────────────────

/// The store's `cas` slot is occupied by a *file*, so no CAS directory can be
/// created. This is the portable stand-in for a read-only store: on Windows a
/// read-only directory attribute does not stop file creation, and granting a
/// real ACL denial would need administrator rights the test must not require.
#[test]
fn an_unwritable_store_layout_fails_loudly_and_leaves_the_library_alone() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    write_model(&lib.join("precious.safetensors"), 0x44);

    let init_args = ["init", "--path", store.to_str().unwrap()];
    let (code, _out, _err) = run_deadline(&store, &init_args, Duration::from_secs(60));
    assert_eq!(code, Some(0), "init itself does not need `cas`, so it should pass");

    // Block the CAS directory. `init --path` creates it, so remove it first —
    // replacing a directory with a regular file needs an explicit removal.
    let cas_dir = store.join("cas");
    if cas_dir.exists() {
        std::fs::remove_dir_all(&cas_dir).unwrap();
    }
    std::fs::write(&cas_dir, b"not a directory").unwrap();

    let (code, _out, err) = run_deadline(
        &store,
        &["scan", lib.to_str().unwrap(), "--copy", "--full"],
        Duration::from_secs(120),
    );

    assert_ne!(code, Some(0), "scan must fail when the CAS cannot be created: {err}");
    assert!(!err.trim().is_empty(), "a failed scan must say why on stderr, not exit silently");

    // The library is the thing that must never be damaged.
    let bytes = std::fs::read(lib.join("precious.safetensors")).unwrap();
    assert_eq!(bytes.len(), 128 * 1024, "the source file must be untouched");
    assert!(bytes.iter().all(|b| *b == 0x44), "the source file content must be untouched");
}

// ─────────────────────────────────────────────────────────────────────────────
// 4 — File lock
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_second_mutator_is_refused_while_the_lock_is_held() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    write_model(&lib.join("model.safetensors"), 0x55);

    let init_args = ["init", "--path", store.to_str().unwrap()];
    let (code, _out, _err) = run_deadline(&store, &init_args, Duration::from_secs(60));
    assert_eq!(code, Some(0));

    // Hold the lock exactly as a running `scan`/`dedup` would.
    let held = modeld_core::StoreLock::acquire(&store, "test-holder", Duration::from_secs(5))
        .expect("acquire the store lock");

    let (code, _out, err) = run_deadline(
        &store,
        &["scan", lib.to_str().unwrap(), "--copy", "--full", "--wait", "0"],
        Duration::from_secs(120),
    );

    assert_eq!(code, Some(1), "a blocked scan must exit non-zero: {err}");
    let lower = err.to_lowercase();
    assert!(lower.contains("busy"), "the error should say the store is busy: {err}");
    assert!(
        lower.contains(&std::process::id().to_string()),
        "the error should name the holding pid ({}): {err}",
        std::process::id()
    );

    // Nothing may have been ingested while blocked.
    let stats = run_deadline(&store, &["stats", "--json"], Duration::from_secs(60));
    let value: serde_json::Value =
        serde_json::from_slice(stats.1.as_bytes()).expect("stats --json");
    assert_eq!(value["total_models"], 0, "a refused scan must not write anything: {value}");

    drop(held);
}
