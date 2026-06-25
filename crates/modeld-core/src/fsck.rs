//! Filesystem consistency checker (fsck / verify)
//!
//! Checks that the three layers of the store — DB metadata, CAS objects, and
//! alias paths — are mutually consistent.  Reports but does not automatically
//! fix problems; the `modeld verify --fix` flag can be added later.
//!
//! Checks performed:
//! 1. **missing_cas**      — model in DB but CAS file is gone
//! 2. **dangling_aliases** — alias path recorded in DB but file does not exist
//! 3. **size_mismatches**  — CAS file size differs from DB `size_bytes`
//! 4. **orphan_cas**       — CAS object on disk has no corresponding DB row

use crate::cas::CasStore;
use crate::db::Database;
use crate::hash::Blake3Hash;
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
}
