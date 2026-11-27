//! Black-box acceptance for the C7 frontend adapters.

use modeld_core::{hash_file, CasStore, Database};
use std::path::Path;
use std::process::Command;

fn modeld() -> Command {
    Command::new(env!("CARGO_BIN_EXE_modeld"))
}

fn seed_one_model(store: &Path) {
    std::fs::create_dir_all(store).unwrap();
    let source = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(source.path(), b"synthetic model bytes").unwrap();
    let hash = hash_file(source.path()).unwrap();
    CasStore::new(store).store(source.path(), &hash).unwrap();

    let mut db = Database::open(&store.join("modeld.db")).unwrap();
    db.insert_or_update_model(&hash, 20, Some("safetensors"), None, None, None).unwrap();
    // The sync classifier reads original_filename from the models table.
    db.set_model_provenance(
        &hash,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("sd-base.safetensors"),
        None,
    )
    .unwrap();
}

#[test]
fn frontend_list_exposes_all_three_adapters() {
    let output = modeld().args(["frontend", "list"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in ["comfyui", "forge", "a1111"] {
        assert!(stdout.contains(name), "missing {name} in: {stdout}");
    }
}

#[test]
fn frontend_sync_builds_the_expected_virtual_directory() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = tmp.path().join("store");
    seed_one_model(&store);

    let output = modeld()
        .args(["--store", store.to_str().unwrap(), "frontend", "sync", "--frontend", "comfyui"])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("created=1"), "unexpected stdout: {stdout}");

    let link = store.join("virtual/comfyui/checkpoints/sd-base.safetensors");
    assert!(link.exists(), "missing virtual link {}", link.display());
    assert_eq!(std::fs::read(&link).unwrap(), b"synthetic model bytes");
}
