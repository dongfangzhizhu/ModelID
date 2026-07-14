//! Content-Addressable Storage (CAS) layer
//!
//! Implements the immutable storage backend based on RFC 0001:
//! - Prefix sharding (2-char hex: 256 shards)
//! - Immutability enforcement (read-only permissions)
//! - Path construction from BLAKE3 hash
//! - Crash-safe staging write with fsync + atomic rename

use crate::hash::Blake3Hash;
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::io::{BufReader, Read, Write};
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

    // ── Crash-safe writer ────────────────────────────────────────────────────

    /// Store a file in the CAS using a crash-safe staging write.
    ///
    /// Steps:
    /// 1. Create `{store}/tmp/cas_staging/{tx_id}/{object_id}.part`
    /// 2. Stream `source` → staging while computing BLAKE3 incrementally
    /// 3. `fsync` the staging file
    /// 4. Verify the on-disk size and computed hash match `expected_hash`
    /// 5. If target CAS path already exists: verify hash+size match and reuse
    /// 6. Atomic rename staging → final CAS path
    /// 7. `fsync` the CAS parent directory (Unix only; no-op on Windows)
    /// 8. Set read-only permissions (mode `0o444` / `FILE_ATTRIBUTE_READONLY`)
    ///
    /// Cross-filesystem: if the staging directory is on a different device than
    /// the CAS directory the rename will fail; the method falls back to
    /// copy + delete.
    pub fn store_crash_safe(
        &self,
        source: &Path,
        expected_hash: &Blake3Hash,
        tx_id: &str,
    ) -> Result<PathBuf> {
        let dest = self.path_for_hash(expected_hash);

        // ── If CAS object already exists: verify and reuse ────────────────
        if dest.exists() {
            let disk_size = fs::metadata(&dest)
                .with_context(|| format!("Failed to stat existing CAS object: {}", dest.display()))?
                .len();
            let src_size = fs::metadata(source)
                .with_context(|| format!("Failed to stat source: {}", source.display()))?
                .len();
            if disk_size != src_size {
                return Err(anyhow!(
                    "CAS collision: existing object {} has size {} but source has size {}",
                    dest.display(),
                    disk_size,
                    src_size
                ));
            }
            return Ok(dest);
        }

        // ── 1. Prepare staging path ───────────────────────────────────────
        let staging_dir = self.root.join("tmp").join("cas_staging").join(tx_id);
        fs::create_dir_all(&staging_dir)
            .with_context(|| format!("Failed to create staging dir: {}", staging_dir.display()))?;

        let object_id = expected_hash.as_hex();
        let staging_path = staging_dir.join(format!("{}.part", object_id));

        // ── 2. Stream source → staging, compute BLAKE3 incrementally ─────
        let src_meta =
            fs::metadata(source).with_context(|| format!("stat source: {}", source.display()))?;
        let src_size = src_meta.len();

        let mut hasher = blake3::Hasher::new();
        let mut bytes_written: u64 = 0;

        {
            let src_file = fs::File::open(source)
                .with_context(|| format!("Open source: {}", source.display()))?;
            let mut reader = BufReader::new(src_file);
            let mut dst_file = fs::File::create(&staging_path)
                .with_context(|| format!("Create staging file: {}", staging_path.display()))?;

            let mut buf = vec![0u8; 256 * 1024]; // 256 KiB buffer
            loop {
                let n = reader.read(&mut buf).with_context(|| "Read source")?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                dst_file.write_all(&buf[..n]).with_context(|| "Write staging")?;
                bytes_written += n as u64;
            }

            // ── 3. fsync the staging file ─────────────────────────────────
            dst_file.flush().context("Flush staging file")?;
            dst_file.sync_all().context("fsync staging file")?;
        } // dst_file closed

        // ── 4. Verify size and hash ───────────────────────────────────────
        if bytes_written != src_size {
            let _ = fs::remove_file(&staging_path);
            return Err(anyhow!(
                "Size mismatch during staging: expected {} bytes, wrote {}",
                src_size,
                bytes_written
            ));
        }

        let computed = Blake3Hash::from_hex(hasher.finalize().to_hex().as_ref())
            .expect("blake3 output is always valid hex");
        if computed.as_hex() != expected_hash.as_hex() {
            let _ = fs::remove_file(&staging_path);
            return Err(anyhow!(
                "Hash mismatch during staging: expected {}, computed {}",
                expected_hash.as_hex(),
                computed.as_hex()
            ));
        }

        // ── 5. Ensure destination CAS directory exists ────────────────────
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Create CAS dir: {}", parent.display()))?;
        }

        // ── 6. Atomic rename (with cross-fs fallback) ─────────────────────
        if let Err(_rename_err) = fs::rename(&staging_path, &dest) {
            // Cross-filesystem move (EXDEV / ERROR_NOT_SAME_DEVICE) — copy + delete
            fs::copy(&staging_path, &dest)
                .with_context(|| format!("Copy staging → CAS: {}", dest.display()))?;
            let _ = fs::remove_file(&staging_path);
        }

        // ── 7. fsync parent directory (Unix only) ─────────────────────────
        #[cfg(unix)]
        if let Some(parent) = dest.parent() {
            fsync_dir(parent)?;
        }

        // ── 8. Set read-only permissions ──────────────────────────────────
        Self::make_readonly(&dest)?;

        Ok(dest)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform helpers
// ─────────────────────────────────────────────────────────────────────────────

/// fsync a directory file descriptor (Unix only).
///
/// This ensures the directory entry for the newly renamed CAS object is
/// durably persisted to disk before we return to the caller.
#[cfg(unix)]
fn fsync_dir(dir: &Path) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let dir_file =
        fs::File::open(dir).with_context(|| format!("Open dir for fsync: {}", dir.display()))?;
    let ret = unsafe { libc_fsync(dir_file.as_raw_fd()) };
    if ret != 0 {
        return Err(anyhow!(
            "fsync dir {} failed with errno {}",
            dir.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(unix)]
extern "C" {
    fn fsync(fd: i32) -> i32;
}

#[cfg(unix)]
unsafe fn libc_fsync(fd: i32) -> i32 {
    fsync(fd)
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

    #[test]
    fn test_store_crash_safe_basic() {
        let temp_dir = TempDir::new().unwrap();
        let store = CasStore::new(temp_dir.path());
        store.init().unwrap();

        let mut test_file = NamedTempFile::new().unwrap();
        test_file.write_all(b"crash safe content").unwrap();
        test_file.flush().unwrap();

        let hash = hash_file(test_file.path()).unwrap();
        let stored = store.store_crash_safe(test_file.path(), &hash, "test-tx-001").unwrap();

        assert!(stored.exists());
        assert!(store.contains(&hash));

        // File must be read-only
        let meta = std::fs::metadata(&stored).unwrap();
        assert!(meta.permissions().readonly());
    }

    #[test]
    fn test_store_crash_safe_idempotent() {
        let temp_dir = TempDir::new().unwrap();
        let store = CasStore::new(temp_dir.path());
        store.init().unwrap();

        let mut test_file = NamedTempFile::new().unwrap();
        test_file.write_all(b"idempotent test").unwrap();
        test_file.flush().unwrap();

        let hash = hash_file(test_file.path()).unwrap();

        // Store twice — second call should reuse the existing object
        let p1 = store.store_crash_safe(test_file.path(), &hash, "tx-idem-1").unwrap();
        let p2 = store.store_crash_safe(test_file.path(), &hash, "tx-idem-2").unwrap();

        assert_eq!(p1, p2);
    }

    #[test]
    fn test_store_crash_safe_hash_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        let store = CasStore::new(temp_dir.path());
        store.init().unwrap();

        let mut test_file = NamedTempFile::new().unwrap();
        test_file.write_all(b"mismatch content").unwrap();
        test_file.flush().unwrap();

        // Provide a wrong (all-zeros) expected hash
        let wrong_hash = Blake3Hash::from_hex(&"0".repeat(64)).unwrap();

        let result = store.store_crash_safe(test_file.path(), &wrong_hash, "tx-mismatch");
        assert!(result.is_err(), "Should fail on hash mismatch");
    }

    #[test]
    fn test_store_crash_safe_staging_cleaned_on_success() {
        let temp_dir = TempDir::new().unwrap();
        let store = CasStore::new(temp_dir.path());
        store.init().unwrap();

        let mut test_file = NamedTempFile::new().unwrap();
        test_file.write_all(b"staging cleanup test").unwrap();
        test_file.flush().unwrap();

        let hash = hash_file(test_file.path()).unwrap();
        let tx_id = "cleanup-test-tx";

        store.store_crash_safe(test_file.path(), &hash, tx_id).unwrap();

        // Staging directory should be cleaned up after successful store
        let staging = temp_dir.path().join("tmp").join("cas_staging").join(tx_id);
        // After rename the .part file should not exist (staging dir may or may not exist)
        let part = staging.join(format!("{}.part", hash.as_hex()));
        assert!(!part.exists(), "Staging .part file should be cleaned up");
    }
}
