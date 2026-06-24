//! Cross-platform link strategy
//!
//! Priority:
//!   1. Hard link  — same volume, zero extra disk space
//!   2. Symlink    — cross-volume, requires SeCreateSymbolicLinkPrivilege on Windows
//!   3. Reference-only — DB record only; file stays in place, no disk savings
//!
//! ## Hardlink / symlink creation protocol
//!
//! When dedup replaces a *duplicate* path with a link, the duplicate file
//! already exists on disk.  Both `hard_link` and the symlink APIs return
//! `AlreadyExists` if the destination path is occupied, so we **must**
//! remove the duplicate before creating the link.  The removal is safe because:
//!   - The canonical content has already been copied to CAS.
//!   - CAS objects are immutable and will survive any subsequent failure.
//!
//! On Windows, CAS objects are set read-only (`make_readonly`).  `remove_file`
//! on a read-only file returns `PermissionDenied`, so we clear the flag first.

use crate::db::AliasType;
use std::path::Path;

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
    { "NTFS".to_string() }
    #[cfg(target_os = "linux")]
    { "ext4".to_string() }
    #[cfg(target_os = "macos")]
    { "APFS".to_string() }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    { "unknown".to_string() }
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
        return create_hardlink(source, target);
    }
    if capability.has_symlink_privilege {
        return create_symlink(source, target);
    }
    create_reference_only(source, target)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Remove a file, clearing the read-only attribute first on Windows.
///
/// CAS objects are made immutable (`set_readonly(true)`).  On Windows,
/// `remove_file` on a read-only file returns `PermissionDenied`; we must
/// clear the flag before deletion.
fn remove_file_force(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            if perms.readonly() {
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
    std::fs::remove_file(path)
}

/// Replace the duplicate file at `source` with a hard link to `target`.
///
/// `std::fs::hard_link(original, link)` creates a new directory entry `link`
/// pointing at `original`.  If `link` already exists the call fails with
/// `AlreadyExists`, so we remove `source` first.  The content is already safe
/// in CAS (`target`).
fn create_hardlink(source: &Path, target: &Path) -> LinkResult {
    if let Err(e) = remove_file_force(source) {
        return LinkResult::Failed(format!(
            "Failed to remove duplicate before hardlink ({}): {}",
            source.display(), e
        ));
    }
    match std::fs::hard_link(target, source) {
        Ok(_) => LinkResult::Success(AliasType::Hardlink),
        Err(e) => LinkResult::Failed(format!(
            "Hardlink {} → {} failed after removing duplicate: {}",
            source.display(), target.display(), e
        )),
    }
}

/// Replace the duplicate file at `source` with a symlink pointing to `target`.
fn create_symlink(source: &Path, target: &Path) -> LinkResult {
    if let Err(e) = remove_file_force(source) {
        return LinkResult::Failed(format!(
            "Failed to remove duplicate before symlink ({}): {}",
            source.display(), e
        ));
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        match symlink_file(target, source) {
            Ok(_) => LinkResult::Success(AliasType::Symlink),
            Err(e) => LinkResult::Failed(format!("Symlink error: {}", e)),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        match symlink(target, source) {
            Ok(_) => LinkResult::Success(AliasType::Symlink),
            Err(e) => LinkResult::Failed(format!("Symlink error: {}", e)),
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        LinkResult::Failed("Symlinks not supported on this platform".to_string())
    }
}

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

    #[test]
    fn test_create_hardlink_replaces_source() {
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
    }

    #[test]
    fn test_hardlink_source_must_not_exist_before() {
        // Verify the old bug: without remove_file_force, hard_link returns AlreadyExists.
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("t.bin");
        let source = tmp.path().join("s.bin");
        fs::write(&target, b"data").unwrap();
        fs::write(&source, b"data").unwrap(); // source exists

        // Old (wrong) call order — should fail:
        let err = std::fs::hard_link(&target, &source);
        assert!(err.is_err(), "hard_link to existing path must fail");

        // New (correct) call order — should succeed:
        fs::remove_file(&source).unwrap();
        std::fs::hard_link(&target, &source).unwrap();
        assert!(source.exists());
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
