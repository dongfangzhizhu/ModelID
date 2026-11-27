//! C4 验收：`verify --manifest` 能检出被篡改的 CAS 对象。
//!
//! 真实生成 CAS 对象 → 生成清单 → 篡改磁盘字节 → 校验必须报 mismatch；
//! 未篡改时必须 clean。这是「可检出被篡改」的直接黑盒证据。

use modeld_core::{
    db::{AliasType, Database, Frontend},
    generate_manifest, hash_file, load_manifest, save_manifest, verify_manifest, Blake3Hash,
    CasStore,
};
use std::fs;
use tempfile::TempDir;

/// CAS objects are immutable (read-only) on every platform: Windows sets the
/// read-only attribute, Unix uses mode 0444. The tamper tests must be able to
/// write to the object, so clear the bit portably first.
#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(path: &std::path::Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_readonly(false);
    fs::set_permissions(path, perms).unwrap();
}

fn fixture() -> (TempDir, std::path::PathBuf, Database, Blake3Hash) {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().to_path_buf();
    let cas = CasStore::new(&store);
    cas.init().unwrap();
    let mut db = Database::open(&store.join("modeld.db")).unwrap();

    let src = store.join("source.bin");
    fs::write(&src, b"model weights payload for manifest test").unwrap();
    let hash = hash_file(&src).unwrap();
    cas.store_crash_safe(&src, &hash, "manifest-test-tx").unwrap();
    db.insert_or_update_model(
        &hash,
        fs::metadata(&src).unwrap().len() as i64,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    db.insert_alias(&hash, &src.display().to_string(), Frontend::User, AliasType::Original)
        .unwrap();
    (tmp, store, db, hash)
}

#[test]
fn clean_store_passes_manifest() {
    let (_tmp, store, db, _) = fixture();
    let manifest = generate_manifest(&store, &db).unwrap();
    assert_eq!(manifest.objects.len(), 1);

    let path = store.join("manifest.json");
    save_manifest(&path, &manifest).unwrap();
    let loaded = load_manifest(&path).unwrap();
    assert_eq!(loaded.version, 1);

    let report = verify_manifest(&store, &loaded, true).unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.checked, 1);
}

#[test]
#[allow(clippy::permissions_set_readonly_false)]
fn tampered_cas_object_is_detected() {
    let (_tmp, store, db, hash) = fixture();
    let manifest = generate_manifest(&store, &db).unwrap();

    // 篡改 CAS 对象的一个字节（保持文件大小不变，确保 size 检查不会误报，
    // 真正触发重新哈希）。
    let cas = CasStore::new(&store);
    let obj = cas.get(&hash).unwrap();
    let mut bytes = fs::read(&obj).unwrap();
    bytes[0] ^= 0xFF;
    // CAS 对象是不可变的（只读），先清除只读位再写。
    make_writable(&obj);
    fs::write(&obj, &bytes).unwrap();

    let report = verify_manifest(&store, &manifest, true).unwrap();
    assert!(!report.is_clean(), "tampering must be detected: {report:?}");
    assert!(
        report.mismatches.iter().any(|m| m.contains("content hash changed")),
        "expected content-hash mismatch, got {report:?}"
    );
}

#[test]
#[allow(clippy::permissions_set_readonly_false)]
fn truncated_cas_object_is_detected() {
    let (_tmp, store, db, hash) = fixture();
    let manifest = generate_manifest(&store, &db).unwrap();
    let cas = CasStore::new(&store);
    let obj = cas.get(&hash).unwrap();
    make_writable(&obj);
    fs::write(&obj, b"short").unwrap();
    let report = verify_manifest(&store, &manifest, false).unwrap();
    assert!(!report.is_clean(), "truncation must be detected: {report:?}");
    assert!(report.mismatches.iter().any(|m| m.contains("size")), "{report:?}");
}

#[test]
#[allow(clippy::permissions_set_readonly_false)]
fn missing_cas_object_is_detected() {
    let (_tmp, store, db, hash) = fixture();
    let manifest = generate_manifest(&store, &db).unwrap();
    let cas = CasStore::new(&store);
    let obj = cas.get(&hash).unwrap();
    #[cfg(windows)]
    {
        let mut perms = fs::metadata(&obj).unwrap().permissions();
        perms.set_readonly(false);
        fs::set_permissions(&obj, perms).unwrap();
    }
    fs::remove_file(&obj).unwrap();
    let report = verify_manifest(&store, &manifest, false).unwrap();
    assert!(!report.is_clean());
    assert!(report.missing.iter().any(|h| h.as_str() == hash.as_hex()), "{report:?}");
}
