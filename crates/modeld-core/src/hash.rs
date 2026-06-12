//! BLAKE3 hashing module with performance optimizations
//!
//! Implements two strategies based on RFC 0002:
//! - Small files (<10MB): Direct read strategy
//! - Large files (≥10MB): Memory-mapped parallel chunking

use anyhow::{Context, Result};
use blake3::Hasher;
use memmap2::Mmap;
use rayon::prelude::*;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Small file threshold: 10MB
const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024;

/// Chunk size for parallel hashing: 64MB
const CHUNK_SIZE: usize = 64 * 1024 * 1024;

/// BLAKE3 hash result (64 hex characters, 32 bytes)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Blake3Hash(String);

impl Blake3Hash {
    /// Create from 64-character hex string
    pub fn from_hex(hex: &str) -> Result<Self> {
        anyhow::ensure!(hex.len() == 64, "BLAKE3 hash must be 64 hex characters");
        anyhow::ensure!(
            hex.chars().all(|c| c.is_ascii_hexdigit()),
            "Hash must contain only hex characters"
        );
        Ok(Self(hex.to_lowercase()))
    }

    /// Get hash as hex string
    pub fn as_hex(&self) -> &str {
        &self.0
    }

    /// Get first 2 characters (prefix for sharding)
    pub fn prefix(&self) -> &str {
        &self.0[..2]
    }
}

impl std::fmt::Display for Blake3Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Compute BLAKE3 hash of a file
///
/// Automatically selects strategy based on file size:
/// - Small files (<10MB): Direct read
/// - Large files (≥10MB): Memory-mapped parallel hashing
pub fn hash_file(path: &Path) -> Result<Blake3Hash> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read metadata for {}", path.display()))?;

    let size = metadata.len();

    if size < SMALL_FILE_THRESHOLD {
        hash_file_small(path)
    } else {
        hash_file_large(path)
    }
}

/// Hash small file (<10MB) using direct read strategy
fn hash_file_small(path: &Path) -> Result<Blake3Hash> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;

    let mut hasher = Hasher::new();
    let mut buffer = vec![0u8; 65536]; // 64KB buffer

    loop {
        let n = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to read from {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let hash = hasher.finalize();
    Ok(Blake3Hash(hash.to_hex().to_string()))
}

/// Hash large file (≥10MB) using memory-mapped parallel strategy
fn hash_file_large(path: &Path) -> Result<Blake3Hash> {
    let file =
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;

    let mmap = unsafe {
        Mmap::map(&file).with_context(|| format!("Failed to mmap {}", path.display()))?
    };

    // For very large files, use parallel chunking
    if mmap.len() > CHUNK_SIZE * 2 {
        hash_parallel(&mmap)
    } else {
        // For medium files, single-threaded is faster
        let mut hasher = Hasher::new();
        hasher.update(&mmap);
        let hash = hasher.finalize();
        Ok(Blake3Hash(hash.to_hex().to_string()))
    }
}

/// Hash using parallel chunking (for very large files)
fn hash_parallel(data: &[u8]) -> Result<Blake3Hash> {
    let chunks: Vec<_> = data.chunks(CHUNK_SIZE).collect();

    // Hash each chunk in parallel
    let chunk_hashes: Vec<_> = chunks
        .par_iter()
        .map(|chunk| {
            let mut hasher = Hasher::new();
            hasher.update(chunk);
            hasher.finalize()
        })
        .collect();

    // Combine chunk hashes
    let mut final_hasher = Hasher::new();
    for hash in chunk_hashes {
        final_hasher.update(hash.as_bytes());
    }

    let hash = final_hasher.finalize();
    Ok(Blake3Hash(hash.to_hex().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_hash_empty_file() {
        let file = NamedTempFile::new().unwrap();
        let hash = hash_file(file.path()).unwrap();
        // BLAKE3 of empty string
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
        // BLAKE3 of "hello world"
        assert_eq!(
            hash.as_hex(),
            "d74981efa70a0c880b8d8c1985d075dbcbf679b99a5f9914e5aaf96b831a9e24"
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
        // Valid hash
        assert!(Blake3Hash::from_hex(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        )
        .is_ok());

        // Too short
        assert!(Blake3Hash::from_hex("abcdef").is_err());

        // Invalid characters
        assert!(Blake3Hash::from_hex(
            "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg"
        )
        .is_err());
    }
}
