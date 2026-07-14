//! BLAKE3 hashing module — standard, interoperable implementation
//!
//! Uses two strategies based on file size:
//! - Small files (<10 MB): sequential read via buffered I/O
//! - Large files (≥10 MB): memory-mapped + blake3's built-in rayon parallel hasher
//!
//! IMPORTANT: `Hasher::update_rayon()` (blake3 "rayon" feature) produces the
//! *standard* BLAKE3 tree hash — bit-for-bit identical to sequential hashing
//! and to `b3sum`.  The previous custom "chunk-hash-then-combine" scheme was
//! NOT standard BLAKE3 and produced hashes that differ from every other tool.

use anyhow::{Context, Result};
use blake3::Hasher;
use memmap2::Mmap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Threshold below which we use buffered-read instead of mmap (10 MB)
const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;

/// BLAKE3 hash result (64 hex characters = 32 bytes)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Blake3Hash(String);

impl Blake3Hash {
    /// Create from a 64-character lowercase hex string.
    pub fn from_hex(hex: &str) -> Result<Self> {
        anyhow::ensure!(
            hex.len() == 64,
            "BLAKE3 hash must be 64 hex characters, got {}",
            hex.len()
        );
        anyhow::ensure!(
            hex.chars().all(|c| c.is_ascii_hexdigit()),
            "Hash must contain only hex characters"
        );
        Ok(Self(hex.to_lowercase()))
    }

    /// The hash as a lowercase hex string.
    pub fn as_hex(&self) -> &str {
        &self.0
    }

    /// First 2 hex chars (used for CAS directory sharding).
    pub fn prefix(&self) -> &str {
        &self.0[..2]
    }
}

impl std::fmt::Display for Blake3Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Compute the standard BLAKE3 hash of a file.
///
/// Dispatches based on file size:
/// - Small (<10 MB): buffered sequential read
/// - Large (≥10 MB): mmap + `Hasher::update_rayon()` for parallel hashing
pub fn hash_file(path: &Path) -> Result<Blake3Hash> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read metadata for {}", path.display()))?;

    if metadata.len() < SMALL_FILE_THRESHOLD {
        hash_file_small(path)
    } else {
        hash_file_large(path)
    }
}

/// Hash a small file (<10 MB) using a 64 KB read buffer.
fn hash_file_small(path: &Path) -> Result<Blake3Hash> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;

    let mut hasher = Hasher::new();
    let mut buffer = vec![0u8; 65536]; // 64 KB

    loop {
        let n = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to read from {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(Blake3Hash(hasher.finalize().to_hex().to_string()))
}

/// Hash a large file (≥10 MB) using mmap + blake3's native parallel hasher.
///
/// `Hasher::update_rayon()` feeds the memory-mapped data into BLAKE3's
/// Rayon-based tree construction.  The result is **standard BLAKE3** —
/// identical to `b3sum`, `blake3::hash()`, and any other conforming
/// implementation, regardless of the number of threads used.
fn hash_file_large(path: &Path) -> Result<Blake3Hash> {
    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;

    // Safety: the file is opened read-only; we do not modify it during hashing.
    // On Linux a concurrent write could theoretically cause SIGBUS — acceptable
    // for the model-dedup use-case where files are not actively written.
    let mmap =
        unsafe { Mmap::map(&file).with_context(|| format!("Failed to mmap {}", path.display()))? };

    let mut hasher = Hasher::new();
    // update_rayon() uses Rayon's work-stealing pool to compute the BLAKE3
    // tree in parallel.  Requires `blake3` feature "rayon" (already set in
    // workspace Cargo.toml).
    hasher.update_rayon(&mmap);
    Ok(Blake3Hash(hasher.finalize().to_hex().to_string()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_hash_empty_file() {
        let file = NamedTempFile::new().unwrap();
        let hash = hash_file(file.path()).unwrap();
        // Standard BLAKE3 of empty input
        assert_eq!(
            hash.as_hex(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn test_hash_small_file() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"hello world").unwrap();
        file.flush().unwrap();
        let hash = hash_file(file.path()).unwrap();
        // Standard BLAKE3 of "hello world"
        assert_eq!(
            hash.as_hex(),
            "d74981efa70a0c880b8d8c1985d075dbcbf679b99a5f9914e5aaf96b831a9e24"
        );
    }

    #[test]
    fn test_hash_large_file_matches_small_path() {
        // Write a file that straddles the threshold, verify both code paths
        // produce the same hash for the same bytes.
        use std::io::Write;
        let mut file = NamedTempFile::new().unwrap();
        // 11 MB of repeated bytes
        let block = vec![0xABu8; 4096];
        for _ in 0..(11 * 1024 * 1024 / 4096) {
            file.write_all(&block).unwrap();
        }
        file.flush().unwrap();

        let hash_large = hash_file_large(file.path()).unwrap();
        let hash_small = hash_file_small(file.path()).unwrap();
        assert_eq!(
            hash_large.as_hex(),
            hash_small.as_hex(),
            "large and small paths must produce the same BLAKE3 hash"
        );
    }

    #[test]
    fn test_hash_prefix() {
        let hash = Blake3Hash::from_hex(
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        )
        .unwrap();
        assert_eq!(hash.prefix(), "ab");
    }

    #[test]
    fn test_hash_validation() {
        assert!(Blake3Hash::from_hex(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        )
        .is_ok());
        assert!(Blake3Hash::from_hex("abcdef").is_err()); // too short
        assert!(Blake3Hash::from_hex(
            "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg"
        )
        .is_err()); // invalid chars
    }
}
