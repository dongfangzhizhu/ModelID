//! Cross-platform link strategy
//!
//! Priority:
//!   1. Hard link  — same volume, zero extra disk space
//!   2. Symlink    — cross-volume, requires SeCreateSymbolicLinkPrivilege on Windows
//!   3. Reference-only — DB record only; file stays in place, no disk savings
//!
//! ## Hardlink / symlink creation protocol
//!
//! Replacing a duplicate path with a link must never leave that path missing,
//! even if the link call fails or the process dies half-way.  `hard_link` and
//! the symlink APIs refuse to overwrite an existing path, so the naive
//! "remove, then link" sequence has a window in which the user's model file
//! does not exist on disk.
//!
//! Instead the link is staged under a temporary name **in the same directory**
//! as its destination and then renamed over it:
//!
//! ```text
//! stage:  link(cas_object, dir/.modeld_tmp_<uuid>.tmp)    // destination untouched
//! commit: rename(dir/.modeld_tmp_<uuid>.tmp, destination)  // atomic replace
//! ```
//!
//! `rename(2)` on Unix and `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` (what
//! `std::fs::rename` uses on Windows) replace the directory entry atomically,
//! so a reader observes either the old file or the new link — never a gap.
//! If staging fails, or the caller drops the [`StagedLink`] without committing,
//! the temporary name is removed and the destination is left untouched.
//!
//! On Windows, CAS objects are read-only.  A read-only destination cannot be
//! replaced, so `commit` clears the flag first and restores it afterwards
//! (the flag lives on the shared file record, i.e. on the CAS object itself).

use crate::db::AliasType;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// Outcome of a link-creation attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkResult {
    Success(AliasType),
    Failed(String),
}

/// Detected system link capabilities.
#[derive(Debug, Clone)]
pub struct LinkCapability {
    pub has_symlink_privilege: bool,
    pub primary_filesystem: String,
}

impl LinkCapability {
    /// Auto-detect capabilities from the current process environment.
    pub fn detect() -> Self {
        Self {
            has_symlink_privilege: detect_symlink_privilege(),
            primary_filesystem: detect_primary_filesystem(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Capability detection
// ─────────────────────────────────────────────────────────────────────────────

fn detect_symlink_privilege() -> bool {
    #[cfg(windows)]
    {
        use std::fs;
        use std::os::windows::fs::symlink_file;
        let tmp = std::env::temp_dir();
        let target = tmp.join("modeld_symtest_target.txt");
        let link = tmp.join("modeld_symtest_link.txt");
        if fs::write(&target, "t").is_err() {
            return false;
        }
        let ok = symlink_file(&target, &link).is_ok();
        let _ = fs::remove_file(&link);
        let _ = fs::remove_file(&target);
        ok
    }
    #[cfg(not(windows))]
    {
        true // Unix: symlinks allowed by default
    }
}

fn detect_primary_filesystem() -> String {
    #[cfg(windows)]
    {
        "NTFS".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        "ext4".to_string()
    }
    #[cfg(target_os = "macos")]
    {
        "APFS".to_string()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        "unknown".to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Volume check
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` when both paths reside on the same filesystem volume.
/// Hard links require same-volume; symlinks work across volumes.
pub fn is_same_volume(path1: &Path, path2: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Component;
        let drive = |p: &Path| -> Option<String> {
            p.components().next().and_then(|c| {
                if let Component::Prefix(px) = c {
                    Some(px.as_os_str().to_string_lossy().to_uppercase())
                } else {
                    None
                }
            })
        };
        match (drive(path1), drive(path2)) {
            (Some(d1), Some(d2)) => d1 == d2,
            _ => false,
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::MetadataExt;
        match (path1.metadata(), path2.metadata()) {
            (Ok(m1), Ok(m2)) => m1.dev() == m2.dev(),
            _ => false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Create the best available link from `dup_path` (source) to `cas_path` (target).
///
/// `source` is the **duplicate** file path that will be replaced by the link.
/// `target` is the **CAS object** — the single authoritative copy of the content.
pub fn create_link(source: &Path, target: &Path, capability: &LinkCapability) -> LinkResult {
    if is_same_volume(source, target) {
        return replace_with_link(source, target, AliasType::Hardlink);
    }
    if capability.has_symlink_privilege {
        return replace_with_link(source, target, AliasType::Symlink);
    }
    create_reference_only(source, target)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create one link of `kind` at `link` pointing at `target`.
///
/// Only the two filesystem-backed alias types are accepted; `ReferenceOnly`
/// and `Original` describe records, not directory entries.
fn create_link_at(kind: &AliasType, target: &Path, link: &Path) -> io::Result<()> {
    match kind {
        AliasType::Hardlink => fs::hard_link(target, link),
        AliasType::Symlink => {
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(target, link)
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, link)
            }
            #[cfg(not(any(windows, unix)))]
            {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "symbolic links are not supported on this platform",
                ))
            }
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{other:?} is not a filesystem link kind"),
        )),
    }
}

/// A link that exists on disk under a temporary name but is not yet visible at
/// its destination.
///
/// Created by [`stage_link`].  Call [`StagedLink::commit`] to swap it into
/// place atomically; drop it to abandon the replacement, which removes the
/// temporary entry and leaves the destination exactly as it was.
#[derive(Debug)]
#[must_use = "dropping a StagedLink discards it; call commit() to replace the destination"]
pub struct StagedLink {
    tmp_path: PathBuf,
    dest_path: PathBuf,
    target_path: PathBuf,
    alias_type: AliasType,
    committed: bool,
}

impl StagedLink {
    /// Destination path this link replaces on [`commit`](StagedLink::commit).
    pub fn dest(&self) -> &Path {
        &self.dest_path
    }

    /// Alias type recorded when this link is committed.
    pub fn alias_type(&self) -> &AliasType {
        &self.alias_type
    }

    /// Atomically replace the destination with the staged link.
    ///
    /// On failure the destination is left untouched and the staged link is
    /// removed; the caller may safely retry or fall back to `ReferenceOnly`.
    pub fn commit(mut self) -> LinkResult {
        // A read-only destination cannot be replaced on Windows.  Remember the
        // flag so it can be restored whichever way the rename goes: clearing it
        // affects the shared file record, i.e. the CAS object itself.
        let dest_was_readonly = prepare_replace(&self.dest_path);

        match fs::rename(&self.tmp_path, &self.dest_path) {
            Ok(()) => {
                self.committed = true;
                restore_readonly(&self.dest_path, dest_was_readonly);
                // The rename is durable only once the directory entry is
                // flushed; a crash right after this point must not lose the
                // alias (the caller records it in the WAL right after).
                if let Some(parent) = self.dest_path.parent() {
                    let _ = crate::platform::fsync_dir(parent);
                }
                LinkResult::Success(self.alias_type.clone())
            }
            Err(e) => {
                restore_readonly(&self.dest_path, dest_was_readonly);
                LinkResult::Failed(format!(
                    "Failed to replace {} with a link to {}: {}",
                    self.dest_path.display(),
                    self.target_path.display(),
                    e
                ))
            }
        }
    }
}

impl Drop for StagedLink {
    fn drop(&mut self) {
        if !self.committed {
            // Best effort: the destination was never touched, so failing to
            // clean up only leaks a single temporary directory entry.
            discard_staged_link(&self.tmp_path, &self.target_path);
        }
    }
}

/// Remove a staged link without disturbing the object it points at.
///
/// On Windows the staged entry may be a read-only hardlink, and clearing that
/// flag affects the *shared* file record — i.e. the CAS object.  Removing the
/// entry therefore has to restore the target's flag afterwards.
fn discard_staged_link(tmp: &Path, target: &Path) {
    #[cfg(windows)]
    {
        let was_readonly = prepare_replace(tmp);
        let _ = fs::remove_file(tmp);
        restore_readonly(target, was_readonly);
    }
    #[cfg(not(windows))]
    {
        let _ = fs::remove_file(tmp);
        let _ = target;
    }
}

/// Create a link to `target` beside `source` **without touching `source`**.
///
/// The link is created in `source`'s own directory, which guarantees the later
/// rename stays on one volume, and under a UUID-suffixed name, so concurrent
/// operations and leftovers from a crashed run cannot collide.
pub fn stage_link(source: &Path, target: &Path, kind: AliasType) -> io::Result<StagedLink> {
    let parent = source.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", source.display()),
        )
    })?;
    fs::create_dir_all(parent)?;

    let tmp_path = parent.join(format!(".modeld_tmp_{}.tmp", Uuid::new_v4().simple()));

    if let Err(e) = create_link_at(&kind, target, &tmp_path) {
        // Never leave a half-created link behind.
        discard_staged_link(&tmp_path, target);
        return Err(e);
    }

    Ok(StagedLink {
        tmp_path,
        dest_path: source.to_path_buf(),
        target_path: target.to_path_buf(),
        alias_type: kind,
        committed: false,
    })
}

/// Atomically replace `source` with a link to `target`.
///
/// Equivalent to [`stage_link`] followed by [`StagedLink::commit`], with errors
/// folded into [`LinkResult::Failed`] for callers that match on that type.
/// Either the old file or the new link is visible at `source` at all times.
pub fn replace_with_link(source: &Path, target: &Path, kind: AliasType) -> LinkResult {
    match stage_link(source, target, kind) {
        Ok(staged) => staged.commit(),
        Err(e) => LinkResult::Failed(format!(
            "Failed to stage link {} → {}: {}",
            source.display(),
            target.display(),
            e
        )),
    }
}

/// Clear the read-only attribute ahead of a replacement.
///
/// Returns `true` when the flag was actually cleared.  No-op on Unix, where
/// replacing a directory entry depends on directory permissions only.
#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
fn prepare_replace(dest: &Path) -> bool {
    // `symlink_metadata`: inspect the directory entry itself, not the CAS object
    // a symlink points at — replacing a symlink never requires touching CAS.
    if let Ok(meta) = fs::symlink_metadata(dest) {
        if meta.permissions().readonly() {
            let mut perms = meta.permissions();
            perms.set_readonly(false);
            if fs::set_permissions(dest, perms).is_ok() {
                return true;
            }
        }
    }
    false
}

/// See [`prepare_replace`]: nothing to do off Windows.
#[cfg(not(windows))]
fn prepare_replace(_dest: &Path) -> bool {
    false
}

/// Re-apply a read-only flag that [`prepare_replace`] cleared.
#[cfg(windows)]
fn restore_readonly(dest: &Path, was_readonly: bool) {
    if !was_readonly {
        return;
    }
    if let Ok(meta) = fs::metadata(dest) {
        let mut perms = meta.permissions();
        if !perms.readonly() {
            perms.set_readonly(true);
            let _ = fs::set_permissions(dest, perms);
        }
    }
}

/// See [`restore_readonly`]: nothing to do off Windows.
#[cfg(not(windows))]
fn restore_readonly(_dest: &Path, _was_readonly: bool) {}

/// Record-only fallback: file stays at original location, nothing deleted.
/// No disk space is reclaimed; the caller must NOT count this as space saved.
fn create_reference_only(_source: &Path, _target: &Path) -> LinkResult {
    LinkResult::Success(AliasType::ReferenceOnly)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_detect_symlink_privilege() {
        let _ = detect_symlink_privilege(); // must not panic
    }

    #[test]
    fn test_link_capability_detect() {
        let cap = LinkCapability::detect();
        assert!(!cap.primary_filesystem.is_empty());
    }

    #[test]
    fn test_is_same_volume_same_dir() {
        let tmp = tempdir().unwrap();
        let f1 = tmp.path().join("a.txt");
        let f2 = tmp.path().join("b.txt");
        fs::write(&f1, "x").unwrap();
        fs::write(&f2, "y").unwrap();
        assert!(is_same_volume(&f1, &f2));
    }

    // ── Atomic replacement ───────────────────────────────────────────────────

    /// No staging entry may survive any code path, success or failure.
    fn assert_no_temp_files(dir: &Path) {
        let leftovers: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".modeld_tmp_"))
            .collect();
        assert!(leftovers.is_empty(), "temporary links left behind: {leftovers:?}");
    }

    #[test]
    fn test_create_link_replaces_source() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("target.bin");
        let source = tmp.path().join("source.bin"); // the "duplicate"

        fs::write(&target, b"content").unwrap();
        fs::write(&source, b"content").unwrap(); // duplicate already exists

        let cap = LinkCapability::detect();
        let result = create_link(&source, &target, &cap);

        match result {
            LinkResult::Success(AliasType::Hardlink) => {
                // source now exists as a hardlink
                assert!(source.exists());
                assert_eq!(fs::read(&source).unwrap(), b"content");
            }
            LinkResult::Success(AliasType::ReferenceOnly) => {
                // cross-volume env (e.g. CI); reference-only is acceptable
            }
            other => panic!("Unexpected result: {:?}", other),
        }
        assert_no_temp_files(tmp.path());
    }

    #[test]
    fn test_replace_with_link_is_atomic() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("cas.bin");
        let source = tmp.path().join("dup.bin");
        fs::write(&target, b"canonical").unwrap();
        fs::write(&source, b"duplicate").unwrap();

        let result = replace_with_link(&source, &target, AliasType::Hardlink);
        assert!(matches!(result, LinkResult::Success(AliasType::Hardlink)), "{result:?}");

        // The destination never disappears: it is either the old file or the link.
        assert!(source.exists(), "destination must never go missing");
        assert_eq!(fs::read(&source).unwrap(), b"canonical");
        assert_eq!(fs::read(&target).unwrap(), b"canonical");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                fs::metadata(&source).unwrap().ino(),
                fs::metadata(&target).unwrap().ino(),
                "source must share the CAS object's inode"
            );
        }
        assert_no_temp_files(tmp.path());
    }

    #[test]
    fn test_staged_link_dropped_leaves_source_untouched() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("cas.bin");
        let source = tmp.path().join("dup.bin");
        fs::write(&target, b"canonical").unwrap();
        fs::write(&source, b"duplicate").unwrap();

        let staged = stage_link(&source, &target, AliasType::Hardlink).unwrap();
        assert_eq!(staged.dest(), source.as_path());
        assert_eq!(staged.alias_type(), &AliasType::Hardlink);
        // Staging must not disturb the destination.
        assert_eq!(fs::read(&source).unwrap(), b"duplicate");

        drop(staged); // simulate "link created, replacement never happened"

        assert_eq!(fs::read(&source).unwrap(), b"duplicate");
        assert_no_temp_files(tmp.path());
    }

    #[test]
    fn test_stage_link_fails_when_target_missing() {
        let tmp = tempdir().unwrap();
        let source = tmp.path().join("dup.bin");
        fs::write(&source, b"duplicate").unwrap();

        let missing = tmp.path().join("missing.bin");
        let err = stage_link(&source, &missing, AliasType::Hardlink).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(fs::read(&source).unwrap(), b"duplicate");
        assert_no_temp_files(tmp.path());
    }

    #[test]
    fn test_commit_failure_keeps_destination_and_cleans_temp() {
        // A directory cannot be replaced by a file link, so the rename fails
        // *after* the link was staged successfully — the failure path that the
        // old delete-then-link code turned into data loss.
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("cas.bin");
        fs::write(&target, b"canonical").unwrap();
        let occupied = tmp.path().join("occupied");
        fs::create_dir(&occupied).unwrap();

        let result = replace_with_link(&occupied, &target, AliasType::Hardlink);
        assert!(matches!(result, LinkResult::Failed(_)), "{result:?}");
        assert!(occupied.is_dir(), "destination must be untouched");
        assert_no_temp_files(tmp.path());
    }

    #[test]
    fn test_replace_handles_existing_destination() {
        // Regression guard: raw hard_link refuses to overwrite, so the staged
        // protocol must not depend on deleting the destination first.
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("t.bin");
        let source = tmp.path().join("s.bin");
        fs::write(&target, b"data").unwrap();
        fs::write(&source, b"data").unwrap();
        assert!(
            fs::hard_link(&target, &source).is_err(),
            "hard_link to an existing path must fail"
        );

        assert!(matches!(
            replace_with_link(&source, &target, AliasType::Hardlink),
            LinkResult::Success(_)
        ));
        assert!(source.exists());
        assert_no_temp_files(tmp.path());
    }

    #[test]
    fn test_replace_over_readonly_destination() {
        // Deduplicating a path that is already a read-only hardlink to CAS:
        // Windows refuses to replace a read-only destination, so the flag has
        // to be cleared and restored around the rename.
        let tmp = tempdir().unwrap();
        let cas = tmp.path().join("cas.bin");
        let first = tmp.path().join("a.bin");
        fs::write(&cas, b"canonical").unwrap();
        fs::write(&first, b"canonical").unwrap();

        // Simulate a CAS object that has already been made immutable.
        let mut perms = fs::metadata(&cas).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&cas, perms).unwrap();

        assert!(matches!(
            replace_with_link(&first, &cas, AliasType::Hardlink),
            LinkResult::Success(_)
        ));
        // `first` now shares the read-only CAS object ...
        assert!(fs::metadata(&first).unwrap().permissions().readonly());

        // ... and replacing that read-only path again must still succeed.
        assert!(matches!(
            replace_with_link(&first, &cas, AliasType::Hardlink),
            LinkResult::Success(_)
        ));
        assert_eq!(fs::read(&first).unwrap(), b"canonical");
        // CAS immutability must be restored, not silently dropped.
        assert!(fs::metadata(&cas).unwrap().permissions().readonly());
        assert_no_temp_files(tmp.path());
    }

    #[test]
    fn test_reference_only_does_not_touch_source() {
        let tmp = tempdir().unwrap();
        let source = tmp.path().join("dup.bin");
        let target = tmp.path().join("cas.bin");
        fs::write(&source, b"dup").unwrap();
        fs::write(&target, b"cas").unwrap();

        let result = create_reference_only(&source, &target);
        assert_eq!(result, LinkResult::Success(AliasType::ReferenceOnly));
        // source file must still exist unchanged
        assert!(source.exists());
        assert_eq!(fs::read(&source).unwrap(), b"dup");
    }
}
