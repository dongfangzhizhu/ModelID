//! Unlink / undo-dedup: restore a hardlinked/symlinked alias to an
//! independent physical copy.
//!
//! When `dedup` replaces a duplicate file with a hard link pointing at the CAS
//! object, the user can later call `modeld unlink <path>` to get a standalone
//! copy of that file back (for example, before moving it to a different
//! volume).
//!
//! ## What this does
//! 1. Looks up the alias record for the given path.
//! 2. Copies the CAS object to a temporary file in the same directory.
//! 3. Atomically renames the temp file over the original path.
//! 4. Updates the alias `alias_type` to `Original` in the DB.

use crate::cas::CasStore;
use crate::db::{AliasType, Database, Frontend};
use crate::hash::Blake3Hash;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Result of a successful unlink operation.
#[derive(Debug)]
pub struct UnlinkResult {
    /// Path that was converted back to an independent file.
    pub path: PathBuf,
    /// BLAKE3 hash of the restored content.
    pub hash: Blake3Hash,
    /// Previous alias type that was replaced.
    pub previous_type: AliasType,
    /// Bytes written to disk.
    pub size_bytes: u64,
}

/// Unlink (undo dedup) for a single alias path.
///
/// The alias must already be recorded in the DB.  The CAS object it points to
/// must exist on disk.  After this call, `path` is an ordinary independent
/// file; the alias record is updated to `Original`.
///
/// This operation is safe even if it is interrupted mid-way: if the rename
/// step fails, the original hardlink/symlink is left untouched and the
/// temporary file is cleaned up.
#[allow(clippy::permissions_set_readonly_false)]
pub fn unlink_path(db: &mut Database, store_path: &Path, path: &Path) -> Result<UnlinkResult> {
    let path_str = path.to_string_lossy().to_string();

    // ── Validate ─────────────────────────────────────────────────────────────
    let alias = db
        .get_alias_by_path(&path_str)?
        .ok_or_else(|| anyhow!("No alias record found for path: {}", path.display()))?;

    if matches!(alias.alias_type, AliasType::Original) {
        return Err(anyhow!(
            "{} is already an independent file (alias_type = Original)",
            path.display()
        ));
    }

    let cas = CasStore::new(store_path);
    let cas_path = cas
        .get(&alias.model_hash)
        .ok_or_else(|| anyhow!("CAS object missing for hash {}", alias.model_hash.as_hex()))?;

    let cas_size = std::fs::metadata(&cas_path)
        .with_context(|| format!("Cannot stat CAS file {}", cas_path.display()))?
        .len();

    // ── Copy CAS → temp file in same directory ────────────────────────────
    let parent =
        path.parent().ok_or_else(|| anyhow!("Path has no parent directory: {}", path.display()))?;
    let tmp_path = parent.join(format!(".modeld_unlink_{}.tmp", &alias.model_hash.as_hex()[..16]));

    // On Windows, CAS files are read-only; fs::copy does not propagate that
    // attribute to the destination, so the copy is writable by default — good.
    std::fs::copy(&cas_path, &tmp_path).with_context(|| {
        format!("Failed to copy CAS {} to temp {}", cas_path.display(), tmp_path.display())
    })?;

    // Ensure the destination is writable (CAS copy might have inherited attrs)
    if let Ok(mut perms) = std::fs::metadata(&tmp_path).map(|m| m.permissions()) {
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(&tmp_path, perms);
        }
    }

    // ── Atomic rename over the original path ──────────────────────────────
    // On Windows we must remove the existing file first (hardlinks count as
    // regular files; Windows rename does not atomically replace).
    #[cfg(windows)]
    {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }

    if let Err(rename_err) = std::fs::rename(&tmp_path, path) {
        // Rename failed — clean up tmp and propagate
        let _ = std::fs::remove_file(&tmp_path);
        return Err(rename_err).with_context(|| {
            format!("Failed to rename {} → {}", tmp_path.display(), path.display())
        });
    }

    // ── Update DB ─────────────────────────────────────────────────────────
    let prev = alias.alias_type.clone();
    db.delete_alias(&path_str)?;
    db.insert_alias(&alias.model_hash, &path_str, Frontend::User, AliasType::Original)?;

    Ok(UnlinkResult {
        path: path.to_path_buf(),
        hash: alias.model_hash,
        previous_type: prev,
        size_bytes: cas_size,
    })
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
    fn test_unlink_converts_to_independent_copy() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();

        let cas = crate::cas::CasStore::new(tmp.path());
        cas.init().unwrap();

        // Create a model file and store in CAS
        let content = b"unlink test model content";
        let model_file = tmp.path().join("model.safetensors");
        std::fs::write(&model_file, content).unwrap();
        let hash = hash_file(&model_file).unwrap();
        let size = content.len() as i64;

        db.insert_or_update_model(&hash, size, None, None, None, None).unwrap();
        cas.store(&model_file, &hash).unwrap();

        // Register as a hardlink alias (simulates post-dedup state)
        db.insert_alias(&hash, &model_file.to_string_lossy(), Frontend::User, AliasType::Hardlink)
            .unwrap();

        // Run unlink
        let result = unlink_path(&mut db, tmp.path(), &model_file).unwrap();

        assert_eq!(result.previous_type, AliasType::Hardlink);
        assert_eq!(result.size_bytes, content.len() as u64);
        assert!(model_file.exists(), "file must exist after unlink");

        // Verify content is correct
        let restored = std::fs::read(&model_file).unwrap();
        assert_eq!(restored, content);

        // Verify DB alias is now Original
        let alias = db.get_alias_by_path(&model_file.to_string_lossy()).unwrap().unwrap();
        assert_eq!(alias.alias_type, AliasType::Original);
    }

    #[test]
    fn test_unlink_errors_on_unknown_path() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();

        let result = unlink_path(&mut db, tmp.path(), Path::new("/nonexistent/path.safetensors"));
        assert!(result.is_err());
    }

    #[test]
    fn test_unlink_errors_on_already_original() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();

        let cas = crate::cas::CasStore::new(tmp.path());
        cas.init().unwrap();

        let f = tmp.path().join("model.safetensors");
        std::fs::write(&f, b"data").unwrap();
        let hash = hash_file(&f).unwrap();
        db.insert_or_update_model(&hash, 4, None, None, None, None).unwrap();
        db.insert_alias(&hash, &f.to_string_lossy(), Frontend::User, AliasType::Original).unwrap();

        let result = unlink_path(&mut db, tmp.path(), &f);
        assert!(result.is_err(), "should error when alias_type is already Original");
    }
}
