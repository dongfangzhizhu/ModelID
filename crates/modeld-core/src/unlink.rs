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

    // ── Copy CAS → temp file in the same directory ────────────────────────
    let parent =
        path.parent().ok_or_else(|| anyhow!("Path has no parent directory: {}", path.display()))?;
    // Unique name: a leftover from a crashed run must never be reused, and two
    // concurrent unlinks must not clobber each other's temporary copy.
    let tmp_path = parent.join(format!(".modeld_unlink_{}.tmp", uuid::Uuid::new_v4().simple()));

    std::fs::copy(&cas_path, &tmp_path).with_context(|| {
        format!("Failed to copy CAS {} to temp {}", cas_path.display(), tmp_path.display())
    })?;

    // `fs::copy` propagates the source permission bits, so the copy inherits
    // the read-only mode of the CAS object; an unlinked file must be writable.
    if let Err(e) = make_writable(&tmp_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e).with_context(|| format!("Failed to make {} writable", tmp_path.display()));
    }

    // ── Atomic rename over the original path ──────────────────────────────
    // `std::fs::rename` is `rename(2)` on Unix and
    // `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` on Windows: both replace the
    // directory entry atomically, so `path` is never missing — not even when
    // the call fails. Windows refuses to replace a read-only destination and
    // the alias is usually a read-only hardlink to CAS, so clear that flag
    // first and restore it afterwards.
    #[cfg(windows)]
    let cas_was_readonly = {
        let was_readonly =
            std::fs::metadata(&cas_path).map(|m| m.permissions().readonly()).unwrap_or(false);
        if was_readonly {
            clear_readonly(path);
        }
        was_readonly
    };

    if let Err(rename_err) = std::fs::rename(&tmp_path, path) {
        // Rename failed — clean up tmp and propagate; `path` is untouched.
        let _ = std::fs::remove_file(&tmp_path);
        #[cfg(windows)]
        {
            if cas_was_readonly {
                set_readonly(&cas_path);
            }
        }
        return Err(rename_err).with_context(|| {
            format!("Failed to rename {} → {}", tmp_path.display(), path.display())
        });
    }

    #[cfg(windows)]
    {
        if cas_was_readonly {
            // Clearing the flag above touched the shared file record, so the
            // CAS object is writable right now; restore its immutability.
            set_readonly(&cas_path);
        }
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

/// Restore ordinary writable-file permissions on a freshly copied file.
///
/// `fs::copy` propagates the source mode on Unix (CAS objects are `0o444`) and
/// the read-only attribute on Windows, so the copy must be made writable before
/// it becomes the user's own file again.
#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        // 0o644, not `set_readonly(false)`, which would grant world-write (0o666).
        perms.set_mode(0o644);
        std::fs::set_permissions(path, perms)
    }
    #[cfg(windows)]
    {
        let mut perms = std::fs::metadata(path)?.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            std::fs::set_permissions(path, perms)?;
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(())
    }
}

/// Clear the read-only attribute of a path that is about to be replaced.
#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
fn clear_readonly(path: &Path) {
    // `symlink_metadata`: the entry itself, not the CAS object behind a symlink.
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.permissions().readonly() {
            let mut perms = meta.permissions();
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

/// Set the read-only attribute (Windows only).
#[cfg(windows)]
fn set_readonly(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        if !perms.readonly() {
            perms.set_readonly(true);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
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
