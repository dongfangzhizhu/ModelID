//! Content-Addressable Storage (CAS) layer
//!
//! Implements the immutable storage backend based on RFC 0001:
//! - Prefix sharding (2-char hex: 256 shards)
//! - Immutability enforcement (read-only permissions)
//! - Path construction from BLAKE3 hash

use crate::hash::Blake3Hash;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// CAS storage manager
pub struct CasStore {
    /// Root directory of the CAS store
    root: PathBuf,
}

impl CasStore {
    /// Create a new CAS store instance
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self { root: root.as_ref().to_path_buf() }
    }

    /// Initialize the CAS store directory structure
    pub fn init(&self) -> Result<()> {
        // Create root CAS directory
        let cas_root = self.cas_root();
        fs::create_dir_all(&cas_root)
            .with_context(|| format!("Failed to create CAS root: {}", cas_root.display()))?;

        // Create all 256 prefix directories (00-ff)
        for prefix in 0..256 {
            let prefix_dir = cas_root.join(format!("{:02x}", prefix));
            fs::create_dir_all(&prefix_dir).with_context(|| {
                format!("Failed to create prefix directory: {}", prefix_dir.display())
            })?;
        }

        Ok(())
    }

    /// Get the CAS root directory path
    fn cas_root(&self) -> PathBuf {
        self.root.join("cas").join("blake3")
    }

    /// Construct the path for a given hash
    pub fn path_for_hash(&self, hash: &Blake3Hash) -> PathBuf {
        let prefix = hash.prefix();
        self.cas_root().join(prefix).join(hash.as_hex())
    }

    /// Check if a hash exists in the CAS
    pub fn contains(&self, hash: &Blake3Hash) -> bool {
        self.path_for_hash(hash).exists()
    }

    /// Store a file in the CAS
    ///
    /// Copies the file to the CAS with the given hash and makes it read-only.
    /// Returns the path where the file was stored.
    pub fn store(&self, source: &Path, hash: &Blake3Hash) -> Result<PathBuf> {
        let dest = self.path_for_hash(hash);

        // Skip if already exists
        if dest.exists() {
            return Ok(dest);
        }

        // Ensure parent directory exists
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory: {}", parent.display())
            })?;
        }

        // Copy file to CAS
        fs::copy(source, &dest).with_context(|| {
            format!("Failed to copy {} to {}", source.display(), dest.display())
        })?;

        // Make file read-only (immutability enforcement)
        Self::make_readonly(&dest)?;

        Ok(dest)
    }

    /// Make a file read-only
    #[cfg(unix)]
    fn make_readonly(path: &Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o444); // Read-only for all
        fs::set_permissions(path, perms)?;
        Ok(())
    }

    /// Make a file read-only (Windows)
    #[cfg(windows)]
    fn make_readonly(path: &Path) -> Result<()> {
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_readonly(true);
        fs::set_permissions(path, perms)?;
        Ok(())
    }

    /// Retrieve a file from the CAS (returns path if exists)
    pub fn get(&self, hash: &Blake3Hash) -> Option<PathBuf> {
        let path = self.path_for_hash(hash);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_file;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_cas_init() {
        let temp_dir = TempDir::new().unwrap();
        let store = CasStore::new(temp_dir.path());

        store.init().unwrap();

        // Verify CAS root exists
        assert!(store.cas_root().exists());

        // Verify all 256 prefix directories exist
        for prefix in 0..256 {
            let prefix_dir = store.cas_root().join(format!("{:02x}", prefix));
            assert!(prefix_dir.exists(), "Prefix {} should exist", prefix);
        }
    }

    #[test]
    fn test_path_construction() {
        let temp_dir = TempDir::new().unwrap();
        let store = CasStore::new(temp_dir.path());

        let hash = Blake3Hash::from_hex(
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        )
        .unwrap();

        let path = store.path_for_hash(&hash);
        let expected = temp_dir
            .path()
            .join("cas/blake3/ab/abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890");

        assert_eq!(path, expected);
    }

    #[test]
    fn test_store_and_retrieve() {
        let temp_dir = TempDir::new().unwrap();
        let store = CasStore::new(temp_dir.path());
        store.init().unwrap();

        // Create a test file
        let mut test_file = NamedTempFile::new().unwrap();
        test_file.write_all(b"test content").unwrap();
        test_file.flush().unwrap();

        // Hash the file
        let hash = hash_file(test_file.path()).unwrap();

        // Store in CAS
        let stored_path = store.store(test_file.path(), &hash).unwrap();

        // Verify file exists
        assert!(stored_path.exists());
        assert!(store.contains(&hash));

        // Verify can retrieve
        let retrieved = store.get(&hash).unwrap();
        assert_eq!(retrieved, stored_path);

        // Verify file is read-only
        let metadata = fs::metadata(&stored_path).unwrap();
        assert!(metadata.permissions().readonly());
    }

    #[test]
    fn test_store_duplicate() {
        let temp_dir = TempDir::new().unwrap();
        let store = CasStore::new(temp_dir.path());
        store.init().unwrap();

        // Create a test file
        let mut test_file = NamedTempFile::new().unwrap();
        test_file.write_all(b"duplicate test").unwrap();
        test_file.flush().unwrap();

        let hash = hash_file(test_file.path()).unwrap();

        // Store first time
        let path1 = store.store(test_file.path(), &hash).unwrap();

        // Store again (should not error, should return same path)
        let path2 = store.store(test_file.path(), &hash).unwrap();

        assert_eq!(path1, path2);
    }
}
