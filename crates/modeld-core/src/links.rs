//! Cross-platform link strategy implementation
//!
//! Implements RFC 0005 Windows compatibility and link creation:
//! - Hardlinks (same volume, zero overhead)
//! - Symlinks (cross volume, requires privileges on Windows)
//! - Junctions (Windows directories, no privileges needed)
//! - Reference-only (fallback when no linking available)

use crate::db::AliasType;
use std::path::Path;

/// Link creation result
#[derive(Debug, Clone, PartialEq)]
pub enum LinkResult {
    Success(AliasType),
    Failed(String),
}

/// System link capabilities
#[derive(Debug, Clone)]
pub struct LinkCapability {
    pub has_symlink_privilege: bool,
    pub primary_filesystem: String,
}

impl LinkCapability {
    /// Detect system link capabilities
    pub fn detect() -> Self {
        let has_symlink_privilege = detect_symlink_privilege();
        let primary_filesystem = detect_primary_filesystem();

        Self { has_symlink_privilege, primary_filesystem }
    }
}

/// Detect if current user has symlink creation privilege
fn detect_symlink_privilege() -> bool {
    #[cfg(windows)]
    {
        use std::fs;
        use std::os::windows::fs::symlink_file;

        let temp_dir = std::env::temp_dir();
        let test_target = temp_dir.join("modeld_test_target.txt");
        let test_link = temp_dir.join("modeld_test_link.txt");

        // Create target file
        if fs::write(&test_target, "test").is_err() {
            return false;
        }

        // Try to create symlink
        let result = symlink_file(&test_target, &test_link);

        // Cleanup
        let _ = fs::remove_file(&test_link);
        let _ = fs::remove_file(&test_target);

        result.is_ok()
    }

    #[cfg(not(windows))]
    {
        // Unix systems allow symlinks by default
        true
    }
}

/// Detect primary filesystem type
fn detect_primary_filesystem() -> String {
    #[cfg(windows)]
    {
        // On Windows, assume NTFS for C: drive
        // TODO: Could use GetVolumeInformation API for precise detection
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

/// Check if two paths are on the same volume
pub fn is_same_volume(path1: &Path, path2: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Component;

        // Extract drive letters (e.g., "C:" from "C:\path")
        let get_drive = |path: &Path| -> Option<String> {
            path.components().next().and_then(|c| {
                if let Component::Prefix(prefix) = c {
                    Some(prefix.as_os_str().to_string_lossy().to_string())
                } else {
                    None
                }
            })
        };

        let drive1 = get_drive(path1);
        let drive2 = get_drive(path2);

        match (drive1, drive2) {
            (Some(d1), Some(d2)) => d1 == d2,
            _ => false, // Conservative: assume different volumes if unclear
        }
    }

    #[cfg(not(windows))]
    {
        // Unix: Use device ID from stat
        use std::os::unix::fs::MetadataExt;

        match (path1.metadata(), path2.metadata()) {
            (Ok(m1), Ok(m2)) => m1.dev() == m2.dev(),
            _ => false, // Conservative: assume different if metadata unavailable
        }
    }
}

/// Create link using appropriate strategy
pub fn create_link(source: &Path, target: &Path, capability: &LinkCapability) -> LinkResult {
    // Step 1: Check filesystem type
    // For now, we assume NTFS/ext4/APFS (full support)
    // Future: Detect exFAT/FAT32 and return reference-only

    // Step 2: Check if same volume
    if is_same_volume(source, target) {
        // Same volume - try hardlink
        return create_hardlink(source, target);
    }

    // Step 3: Cross-volume - check privilege
    if capability.has_symlink_privilege {
        // Have privilege - try symlink
        return create_symlink(source, target);
    }

    // Step 4: No privilege - reference-only mode
    create_reference_only(source, target)
}

/// Create hardlink
fn create_hardlink(source: &Path, target: &Path) -> LinkResult {
    match std::fs::hard_link(target, source) {
        Ok(_) => LinkResult::Success(AliasType::Hardlink),
        Err(e) => LinkResult::Failed(format!("Hardlink error: {}", e)),
    }
}

/// Create symlink
fn create_symlink(source: &Path, target: &Path) -> LinkResult {
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
}

/// Create reference-only entry (no physical link)
fn create_reference_only(_source: &Path, _target: &Path) -> LinkResult {
    // File remains at original location
    // Record in aliases table with type='reference_only'
    LinkResult::Success(AliasType::ReferenceOnly)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_detect_symlink_privilege() {
        let has_priv = detect_symlink_privilege();
        println!("Symlink privilege: {}", has_priv);
        // Just ensure it doesn't panic, result depends on system
    }

    #[test]
    fn test_detect_primary_filesystem() {
        let fs_type = detect_primary_filesystem();
        println!("Primary filesystem: {}", fs_type);
        assert!(!fs_type.is_empty());
    }

    #[test]
    fn test_link_capability_detect() {
        let cap = LinkCapability::detect();
        println!("Capability: {:?}", cap);
        assert!(!cap.primary_filesystem.is_empty());
    }

    #[test]
    fn test_is_same_volume() {
        let temp_dir = tempdir().unwrap();
        let file1 = temp_dir.path().join("file1.txt");
        let file2 = temp_dir.path().join("file2.txt");

        fs::write(&file1, "test1").unwrap();
        fs::write(&file2, "test2").unwrap();

        // Files in same temp directory should be on same volume
        assert!(is_same_volume(&file1, &file2));
    }

    #[test]
    fn test_create_hardlink_same_volume() {
        let temp_dir = tempdir().unwrap();
        let target = temp_dir.path().join("target.txt");
        let link = temp_dir.path().join("link.txt");

        fs::write(&target, "test content").unwrap();

        let cap = LinkCapability::detect();
        let result = create_link(&link, &target, &cap);

        match result {
            LinkResult::Success(AliasType::Hardlink) => {
                assert!(link.exists());
                let content = fs::read_to_string(&link).unwrap();
                assert_eq!(content, "test content");
            }
            _ => panic!("Expected hardlink success"),
        }
    }

    #[test]
    fn test_hardlink_metadata() {
        let temp_dir = tempdir().unwrap();
        let target = temp_dir.path().join("target.txt");
        let link = temp_dir.path().join("link.txt");

        fs::write(&target, "test").unwrap();

        let result = create_hardlink(&link, &target);

        if let LinkResult::Success(AliasType::Hardlink) = result {
            // Verify both files have same inode/content
            let target_meta = fs::metadata(&target).unwrap();
            let link_meta = fs::metadata(&link).unwrap();

            // Both should have same size
            assert_eq!(target_meta.len(), link_meta.len());

            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                // On Unix, hardlinks share the same inode
                assert_eq!(target_meta.ino(), link_meta.ino());
            }
        } else {
            panic!("Hardlink creation failed");
        }
    }

    #[test]
    fn test_reference_only() {
        let temp_dir = tempdir().unwrap();
        let source = temp_dir.path().join("source.txt");
        let target = temp_dir.path().join("target.txt");

        fs::write(&target, "test").unwrap();

        let result = create_reference_only(&source, &target);

        assert_eq!(result, LinkResult::Success(AliasType::ReferenceOnly));
        // Source file should NOT be created in reference-only mode
        assert!(!source.exists());
    }
}
