//! Black-box acceptance for bounded incremental GC [C6].

use modeld_core::{Database, GcCursor};
use std::path::Path;
use std::process::Command;

fn seed_store(store: &Path) {
    std::fs::create_dir_all(store).unwrap();
    let mut db = Database::open(&store.join("modeld.db")).unwrap();
    for byte in [0xaa, 0xbb, 0xcc] {
        let hash = modeld_core::Blake3Hash::from_hex(&format!("{:02x}", byte).repeat(32)).unwrap();
        db.insert_or_update_model(&hash, 10, None, None, None, None).unwrap();
    }
}

#[test]
fn incremental_gc_processes_one_batch_and_persists_the_cursor() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = tmp.path().join("store");
    seed_store(&store);

    let output = Command::new(env!("CARGO_BIN_EXE_modeld"))
        .args(["--store", store.to_str().unwrap(), "gc", "--incremental", "--batch-size", "1"])
        .output()
        .expect("run modeld gc --incremental");

    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("scanned 1") || stdout.contains("扫描 1"),
        "unexpected stdout: {stdout}"
    );

    let expected = "aa".repeat(32);
    let cursor = GcCursor::load(&store).unwrap();
    assert_eq!(cursor.last_hash.as_deref(), Some(expected.as_str()));
}
