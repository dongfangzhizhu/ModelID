//! Filesystem consistency checker (fsck / verify)
//!
//! Checks that the three layers of the store — DB metadata, CAS objects, and
//! alias paths — are mutually consistent.  Reports but does not automatically
//! fix problems; the `modeld verify --fix` flag can be added later.
//!
//! Two entry points are provided:
//!
//! 1. **`run_fsck`** — original fast check (backward-compatible).
//! 2. **`run_verify`** — extended check with optional deep hash verification.
//! 3. **`run_repair`** — non-destructive repair: removes orphan DB records
//!    and cleans up staging residue; never deletes user data.
//!
//! Check categories:
//! 1. **missing_cas**          — model in DB but CAS file is gone
//! 2. **dangling_aliases**     — alias path in DB but file does not exist
//! 3. **size_mismatches**      — CAS file size ≠ DB `size_bytes`
//! 4. **orphan_cas**           — CAS file on disk with no DB row
//! 5. **missing_cas_objects**  — (VerifyReport) same as missing_cas but typed
//! 6. **hash_mismatches**      — (deep) on-disk BLAKE3 hash ≠ recorded hash
//! 7. **orphan_db_records**    — alias pointing to a CAS object that is missing
//! 8. **staging_residue**      — leftover `.part` files in `tmp/cas_staging/`

use crate::cas::CasStore;
use crate::db::Database;
use crate::hash::{hash_file, Blake3Hash};
use anyhow::Result;
use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// Report types
// ─────────────────────────────────────────────────────────────────────────────

/// A single alias whose on-disk path no longer exists.
#[derive(Debug, Clone)]
pub struct DanglingAlias {
    pub path: String,
    pub model_hash: Blake3Hash,
}

/// A model whose CAS file size differs from what the DB recorded.
#[derive(Debug, Clone)]
pub struct SizeMismatch {
    pub hash: Blake3Hash,
    pub db_size: i64,
    pub disk_size: u64,
}

/// The complete result of a consistency check.
#[derive(Debug, Default)]
pub struct FsckReport {
    /// Models in DB whose CAS object is missing from disk.
    pub missing_cas: Vec<Blake3Hash>,
    /// Alias paths in DB that no longer exist on disk.
    pub dangling_aliases: Vec<DanglingAlias>,
    /// CAS objects whose on-disk size differs from the DB value.
    pub size_mismatches: Vec<SizeMismatch>,
    /// CAS files on disk that have no corresponding model row in the DB.
    pub orphan_cas: Vec<PathBuf>,
}

impl FsckReport {
    pub fn is_clean(&self) -> bool {
        self.missing_cas.is_empty()
            && self.dangling_aliases.is_empty()
            && self.size_mismatches.is_empty()
            && self.orphan_cas.is_empty()
    }

    pub fn total_issues(&self) -> usize {
        self.missing_cas.len()
            + self.dangling_aliases.len()
            + self.size_mismatches.len()
            + self.orphan_cas.len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Engine
// ─────────────────────────────────────────────────────────────────────────────

/// Run a full consistency check and return a report.
///
/// This is a read-only operation — nothing is modified on disk or in the DB.
pub fn run_fsck(db: &Database, store_path: &Path) -> Result<FsckReport> {
    let cas = CasStore::new(store_path);
    let mut report = FsckReport::default();

    // ── 1 & 3: Check every model in DB against its CAS object ───────────────
    let models = db.list_models(None)?;
    for model in &models {
        match cas.get(&model.blake3_hash) {
            None => {
                report.missing_cas.push(model.blake3_hash.clone());
            }
            Some(cas_path) => {
                if let Ok(meta) = std::fs::metadata(&cas_path) {
                    let disk_size = meta.len();
                    if disk_size != model.size_bytes as u64 {
                        report.size_mismatches.push(SizeMismatch {
                            hash: model.blake3_hash.clone(),
                            db_size: model.size_bytes,
                            disk_size,
                        });
                    }
                }
            }
        }
    }

    // ── 2: Check every alias path ────────────────────────────────────────────
    let aliases = db.list_all_aliases()?;
    for alias in &aliases {
        if !Path::new(&alias.path).exists() {
            report.dangling_aliases.push(DanglingAlias {
                path: alias.path.clone(),
                model_hash: alias.model_hash.clone(),
            });
        }
    }

    // ── 4: Walk CAS and look for objects not in DB ───────────────────────────
    let known_hashes: std::collections::HashSet<String> =
        models.iter().map(|m| m.blake3_hash.as_hex().to_string()).collect();

    let cas_root = store_path.join("cas").join("blake3");
    if cas_root.exists() {
        for prefix_entry in std::fs::read_dir(&cas_root)
            .into_iter()
            .flatten()
            .flatten()
        {
            let prefix_path = prefix_entry.path();
            if !prefix_path.is_dir() {
                continue;
            }
            for obj_entry in std::fs::read_dir(&prefix_path)
                .into_iter()
                .flatten()
                .flatten()
            {
                let obj_path = obj_entry.path();
                let hash_str = obj_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !known_hashes.contains(&hash_str) {
                    report.orphan_cas.push(obj_path);
                }
            }
        }
    }

    Ok(report)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Extended verify / repair (Task 4.3)
// ─────────────────────────────────────────────────────────────────────────────

/// A CAS object referenced by a DB model row that is missing from disk.
#[derive(Debug, Clone)]
pub struct MissingCasObject {
    pub hash: Blake3Hash,
    pub db_size: i64,
}

/// A CAS file whose BLAKE3 hash does not match the recorded hash.
/// Only populated when `deep = true` in `run_verify`.
#[derive(Debug, Clone)]
pub struct HashMismatch {
    pub hash: Blake3Hash,
    pub cas_path: PathBuf,
    pub expected_hex: String,
    pub computed_hex: String,
}

/// A DB alias record whose target CAS object is missing from disk.
#[derive(Debug, Clone)]
pub struct OrphanRecord {
    pub alias_path: String,
    pub model_hash: Blake3Hash,
}

/// Extended verification report.
#[derive(Debug, Default)]
pub struct VerifyReport {
    /// DB model rows whose CAS object is absent from disk.
    pub missing_cas_objects: Vec<MissingCasObject>,
    /// CAS files whose on-disk size differs from the DB value.
    pub size_mismatches: Vec<SizeMismatch>,
    /// CAS files whose BLAKE3 hash does not match (only with `deep = true`).
    pub hash_mismatches: Vec<HashMismatch>,
    /// Alias rows in DB pointing at a CAS object that does not exist on disk.
    pub orphan_db_records: Vec<OrphanRecord>,
    /// Leftover `.part` files in `{store}/tmp/cas_staging/`.
    pub staging_residue: Vec<PathBuf>,
}

impl VerifyReport {
    pub fn is_clean(&self) -> bool {
        self.missing_cas_objects.is_empty()
            && self.size_mismatches.is_empty()
            && self.hash_mismatches.is_empty()
            && self.orphan_db_records.is_empty()
            && self.staging_residue.is_empty()
    }

    pub fn total_issues(&self) -> usize {
        self.missing_cas_objects.len()
            + self.size_mismatches.len()
            + self.hash_mismatches.len()
            + self.orphan_db_records.len()
            + self.staging_residue.len()
    }
}

/// Summary of what `run_repair` cleaned up.
#[derive(Debug, Default)]
pub struct RepairResult {
    /// Number of orphan alias DB records removed.
    pub orphan_records_removed: usize,
    /// Number of staging `.part` files deleted.
    pub staging_files_removed: usize,
    /// Errors that occurred during repair (non-fatal).
    pub errors: Vec<String>,
}

/// Run an extended consistency check.
///
/// - `deep = false`: checks sizes only (fast, suitable for routine health checks).
/// - `deep = true`: also recomputes BLAKE3 for every CAS object (slow, use for
///   periodic integrity audits).
pub fn run_verify(store: &Path, db: &Database, deep: bool) -> Result<VerifyReport> {
    let cas = CasStore::new(store);
    let mut report = VerifyReport::default();

    // ── 1 & 3: models vs CAS ─────────────────────────────────────────────────
    let models = db.list_models(None)?;
    let mut known_hashes =
        std::collections::HashSet::<String>::with_capacity(models.len());

    for model in &models {
        known_hashes.insert(model.blake3_hash.as_hex().to_string());

        match cas.get(&model.blake3_hash) {
            None => {
                report.missing_cas_objects.push(MissingCasObject {
                    hash: model.blake3_hash.clone(),
                    db_size: model.size_bytes,
                });
            }
            Some(cas_path) => {
                if let Ok(meta) = std::fs::metadata(&cas_path) {
                    let disk_size = meta.len();
                    if disk_size != model.size_bytes as u64 {
                        report.size_mismatches.push(SizeMismatch {
                            hash: model.blake3_hash.clone(),
                            db_size: model.size_bytes,
                            disk_size,
                        });
                    }

                    // Deep: recompute hash and compare
                    if deep {
                        match hash_file(&cas_path) {
                            Ok(computed) => {
                                if computed.as_hex() != model.blake3_hash.as_hex() {
                                    report.hash_mismatches.push(HashMismatch {
                                        hash: model.blake3_hash.clone(),
                                        cas_path: cas_path.clone(),
                                        expected_hex: model
                                            .blake3_hash
                                            .as_hex()
                                            .to_string(),
                                        computed_hex: computed.as_hex().to_string(),
                                    });
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "verify: failed to hash {}: {:#}",
                                    cas_path.display(),
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // ── 2: alias → CAS orphan check ──────────────────────────────────────────
    let aliases = db.list_all_aliases()?;
    for alias in &aliases {
        if !known_hashes.contains(alias.model_hash.as_hex()) {
            // Model row missing entirely
            report.orphan_db_records.push(OrphanRecord {
                alias_path: alias.path.clone(),
                model_hash: alias.model_hash.clone(),
            });
        } else {
            // Model row exists but CAS object missing
            if !cas.contains(&alias.model_hash) {
                report.orphan_db_records.push(OrphanRecord {
                    alias_path: alias.path.clone(),
                    model_hash: alias.model_hash.clone(),
                });
            }
        }
    }

    // ── 4: staging residue ────────────────────────────────────────────────────
    let staging_root = store.join("tmp").join("cas_staging");
    if staging_root.exists() {
        collect_part_files(&staging_root, &mut report.staging_residue);
    }

    Ok(report)
}

/// Collect all `.part` files recursively under `dir`.
fn collect_part_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_part_files(&path, out);
        } else if path.extension().map_or(false, |e| e == "part") {
            out.push(path);
        }
    }
}

/// Non-destructive repair: remove orphan DB alias records and clean staging
/// residue.  **Never deletes user data (model files or CAS objects).**
pub fn run_repair(
    store: &Path,
    db: &mut Database,
    report: &VerifyReport,
) -> Result<RepairResult> {
    let mut result = RepairResult::default();

    // ── 1. Remove orphan alias DB records ─────────────────────────────────────
    for orphan in &report.orphan_db_records {
        match db.delete_alias(&orphan.alias_path) {
            Ok(_) => result.orphan_records_removed += 1,
            Err(e) => result.errors.push(format!(
                "delete alias {}: {:#}",
                orphan.alias_path, e
            )),
        }
    }

    // ── 2. Delete staging residue (.part files) ───────────────────────────────
    for part_path in &report.staging_residue {
        match std::fs::remove_file(part_path) {
            Ok(_) => result.staging_files_removed += 1,
            Err(e) => result.errors.push(format!(
                "remove staging {}: {:#}",
                part_path.display(),
                e
            )),
        }
    }

    // Clean up empty staging subdirectories (best-effort)
    let staging_root = store.join("tmp").join("cas_staging");
    if staging_root.exists() {
        remove_empty_dirs(&staging_root);
    }

    Ok(result)
}

/// Recursively remove empty directories under `dir` (best-effort, ignores errors).
fn remove_empty_dirs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_empty_dirs(&path);
            let _ = std::fs::remove_dir(&path); // only succeeds if empty
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AliasType, Database, Frontend};
    use crate::hash::hash_file;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_clean_store() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();

        let cas = CasStore::new(tmp.path());
        cas.init().unwrap();

        // Add a model file that actually exists in CAS
        let mf = NamedTempFile::new().unwrap();
        std::fs::write(mf.path(), b"model data").unwrap();
        let hash = hash_file(mf.path()).unwrap();
        let size = std::fs::metadata(mf.path()).unwrap().len() as i64;
        db.insert_or_update_model(&hash, size, None, None, None, None).unwrap();
        cas.store(mf.path(), &hash).unwrap();

        // Add an alias that points to an existing file
        db.insert_alias(&hash, &mf.path().to_string_lossy(), Frontend::User, AliasType::Original)
            .unwrap();

        let report = run_fsck(&db, tmp.path()).unwrap();
        assert!(report.is_clean(), "Report: {:?}", report);
    }

    #[test]
    fn test_detects_missing_cas() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();
        CasStore::new(tmp.path()).init().unwrap();

        let fake_hash = Blake3Hash::from_hex(&"b".repeat(64)).unwrap();
        db.insert_or_update_model(&fake_hash, 100, None, None, None, None).unwrap();

        let report = run_fsck(&db, tmp.path()).unwrap();
        assert_eq!(report.missing_cas.len(), 1);
        assert_eq!(report.missing_cas[0].as_hex(), fake_hash.as_hex());
    }

    #[test]
    fn test_detects_dangling_alias() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();
        let cas = CasStore::new(tmp.path());
        cas.init().unwrap();

        let mf = NamedTempFile::new().unwrap();
        std::fs::write(mf.path(), b"x").unwrap();
        let hash = hash_file(mf.path()).unwrap();
        let size = 1i64;
        db.insert_or_update_model(&hash, size, None, None, None, None).unwrap();
        cas.store(mf.path(), &hash).unwrap();

        let nonexistent = "/nonexistent/path/model.safetensors";
        db.insert_alias(&hash, nonexistent, Frontend::User, AliasType::Original).unwrap();

        let report = run_fsck(&db, tmp.path()).unwrap();
        assert_eq!(report.dangling_aliases.len(), 1);
        assert_eq!(report.dangling_aliases[0].path, nonexistent);
    }

    #[test]
    fn test_detects_orphan_cas() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let db = Database::open(db_file.path()).unwrap();

        // Create a CAS file with no DB entry
        let cas_dir = tmp.path().join("cas").join("blake3").join("cc");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let orphan_name = "c".repeat(64);
        std::fs::write(cas_dir.join(&orphan_name), b"orphan").unwrap();

        let report = run_fsck(&db, tmp.path()).unwrap();
        assert_eq!(report.orphan_cas.len(), 1);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // run_verify / run_repair tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_verify_clean_store() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();
        let cas = crate::cas::CasStore::new(tmp.path());
        cas.init().unwrap();

        let mf = NamedTempFile::new().unwrap();
        std::fs::write(mf.path(), b"hello verify").unwrap();
        let hash = hash_file(mf.path()).unwrap();
        let size = std::fs::metadata(mf.path()).unwrap().len() as i64;
        db.insert_or_update_model(&hash, size, None, None, None, None).unwrap();
        cas.store(mf.path(), &hash).unwrap();
        db.insert_alias(&hash, &mf.path().to_string_lossy(), Frontend::User, AliasType::Original)
            .unwrap();

        let report = run_verify(tmp.path(), &db, false).unwrap();
        assert!(report.is_clean(), "Expected clean: {:?}", report);
    }

    #[test]
    fn test_verify_detects_missing_cas_object() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();
        crate::cas::CasStore::new(tmp.path()).init().unwrap();

        let fake_hash = Blake3Hash::from_hex(&"e".repeat(64)).unwrap();
        db.insert_or_update_model(&fake_hash, 42, None, None, None, None).unwrap();

        let report = run_verify(tmp.path(), &db, false).unwrap();
        assert_eq!(report.missing_cas_objects.len(), 1);
        assert_eq!(report.missing_cas_objects[0].hash.as_hex(), fake_hash.as_hex());
    }

    #[test]
    fn test_verify_detects_orphan_db_record() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();
        let cas = crate::cas::CasStore::new(tmp.path());
        cas.init().unwrap();

        let mf = NamedTempFile::new().unwrap();
        std::fs::write(mf.path(), b"will be missing").unwrap();
        let hash = hash_file(mf.path()).unwrap();
        let size = std::fs::metadata(mf.path()).unwrap().len() as i64;
        db.insert_or_update_model(&hash, size, None, None, None, None).unwrap();
        // Do NOT store in CAS — alias points to a model whose CAS object is missing
        db.insert_alias(&hash, "/some/alias/path", Frontend::User, AliasType::Original)
            .unwrap();

        let report = run_verify(tmp.path(), &db, false).unwrap();
        assert_eq!(report.orphan_db_records.len(), 1);
        assert_eq!(report.orphan_db_records[0].alias_path, "/some/alias/path");
    }

    #[test]
    fn test_verify_detects_staging_residue() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let db = Database::open(db_file.path()).unwrap();

        // Create a leftover .part file
        let staging = tmp.path().join("tmp").join("cas_staging").join("dead-tx");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("abc.part"), b"partial").unwrap();

        let report = run_verify(tmp.path(), &db, false).unwrap();
        assert_eq!(report.staging_residue.len(), 1);
    }

    #[test]
    fn test_verify_deep_hash_check() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();
        let cas = crate::cas::CasStore::new(tmp.path());
        cas.init().unwrap();

        let mf = NamedTempFile::new().unwrap();
        std::fs::write(mf.path(), b"deep hash content").unwrap();
        let hash = hash_file(mf.path()).unwrap();
        let size = std::fs::metadata(mf.path()).unwrap().len() as i64;
        db.insert_or_update_model(&hash, size, None, None, None, None).unwrap();

        // Store in CAS (file is read-only after this)
        let cas_path = cas.store(mf.path(), &hash).unwrap();

        // Temporarily make it writable so we can corrupt it for the test
        let mut perms = std::fs::metadata(&cas_path).unwrap().permissions();
        perms.set_readonly(false);
        std::fs::set_permissions(&cas_path, perms).unwrap();
        // Corrupt the CAS object
        std::fs::write(&cas_path, b"CORRUPTED").unwrap();

        let report = run_verify(tmp.path(), &db, true).unwrap();
        assert_eq!(report.hash_mismatches.len(), 1);
        assert_eq!(report.hash_mismatches[0].expected_hex, hash.as_hex());
    }

    #[test]
    fn test_repair_removes_orphan_records_and_staging() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();
        let cas = crate::cas::CasStore::new(tmp.path());
        cas.init().unwrap();

        // Create an orphan alias (model in DB, no CAS file)
        let fake_hash = Blake3Hash::from_hex(&"f".repeat(64)).unwrap();
        db.insert_or_update_model(&fake_hash, 10, None, None, None, None).unwrap();
        db.insert_alias(&fake_hash, "/orphan/path", Frontend::User, AliasType::Original)
            .unwrap();

        // Create a staging residue
        let staging = tmp.path().join("tmp").join("cas_staging").join("tx-123");
        std::fs::create_dir_all(&staging).unwrap();
        let part_file = staging.join("file.part");
        std::fs::write(&part_file, b"leftover").unwrap();

        // Verify detects both
        let report = run_verify(tmp.path(), &db, false).unwrap();
        assert_eq!(report.orphan_db_records.len(), 1);
        assert_eq!(report.staging_residue.len(), 1);

        // Repair
        let repair = run_repair(tmp.path(), &mut db, &report).unwrap();
        assert_eq!(repair.orphan_records_removed, 1);
        assert_eq!(repair.staging_files_removed, 1);
        assert!(repair.errors.is_empty());

        // Staging file must be gone
        assert!(!part_file.exists());
    }
}
