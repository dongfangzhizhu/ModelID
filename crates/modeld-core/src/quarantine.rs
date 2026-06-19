//! Quarantine mechanism for safe file management
//!
//! Implements RFC 0004 quarantine strategy:
//! - Move files to quarantine directory with metadata
//! - 30-day TTL for automatic cleanup
//! - Restore functionality

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Quarantine metadata stored alongside quarantined files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineMeta {
    /// Original file path before quarantine
    pub original_path: String,
    /// Timestamp when file was quarantined
    pub quarantined_at: DateTime<Utc>,
    /// Reason for quarantine
    pub reason: String,
    /// BLAKE3 hash of the file
    pub blake3_hash: String,
    /// File size in bytes
    pub size_bytes: u64,
    /// Paths that referenced this file (alias paths)
    pub references: Vec<String>,
}

/// Information about a quarantined file
#[derive(Debug, Clone)]
pub struct QuarantineEntry {
    /// Path to the quarantined file
    pub quarantine_path: PathBuf,
    /// Metadata
    pub meta: QuarantineMeta,
    /// Days remaining before TTL expiry (None if already expired)
    pub days_remaining: Option<i64>,
}

/// Quarantine manager
pub struct QuarantineManager {
    /// Root quarantine directory (e.g., store/quarantine/)
    quarantine_dir: PathBuf,
    /// TTL in days (default: 30)
    ttl_days: i64,
}

impl QuarantineManager {
    /// Create a new QuarantineManager
    pub fn new(store_path: &Path) -> Self {
        Self { quarantine_dir: store_path.join("quarantine"), ttl_days: 30 }
    }

    pub fn with_ttl(store_path: &Path, ttl_days: i64) -> Self {
        Self { quarantine_dir: store_path.join("quarantine"), ttl_days }
    }

    /// Initialize the quarantine directory
    pub fn init(&self) -> Result<()> {
        std::fs::create_dir_all(&self.quarantine_dir).with_context(|| {
            format!("Failed to create quarantine directory: {}", self.quarantine_dir.display())
        })?;
        Ok(())
    }

    /// Move a file to quarantine
    ///
    /// Creates:
    /// - quarantine/{hash}.{timestamp}        (the file itself)
    /// - quarantine/{hash}.{timestamp}.meta   (JSON metadata)
    pub fn quarantine(
        &self,
        file_path: &Path,
        hash: &str,
        reason: &str,
        references: Vec<String>,
    ) -> Result<PathBuf> {
        self.init()?;

        let now = Utc::now();
        let timestamp = now.timestamp();
        let filename = format!("{}.{}", hash, timestamp);
        let quarantine_path = self.quarantine_dir.join(&filename);
        let meta_path = self.quarantine_dir.join(format!("{}.meta", filename));

        // Get file metadata before moving
        let file_meta = std::fs::metadata(file_path)
            .with_context(|| format!("Failed to get metadata for {}", file_path.display()))?;

        // Create metadata
        let meta = QuarantineMeta {
            original_path: file_path.to_string_lossy().to_string(),
            quarantined_at: now,
            reason: reason.to_string(),
            blake3_hash: hash.to_string(),
            size_bytes: file_meta.len(),
            references,
        };

        // Write metadata file first
        let meta_json = serde_json::to_string_pretty(&meta)
            .with_context(|| "Failed to serialize quarantine metadata")?;
        std::fs::write(&meta_path, meta_json)
            .with_context(|| format!("Failed to write metadata to {}", meta_path.display()))?;

        // Move file to quarantine
        // Try rename first (atomic), fall back to copy+delete
        if std::fs::rename(file_path, &quarantine_path).is_err() {
            std::fs::copy(file_path, &quarantine_path).with_context(|| {
                format!(
                    "Failed to copy {} to quarantine {}",
                    file_path.display(),
                    quarantine_path.display()
                )
            })?;
            // Only remove original if copy succeeded
            std::fs::remove_file(file_path).with_context(|| {
                format!("Failed to remove original file {}", file_path.display())
            })?;
        }

        Ok(quarantine_path)
    }

    /// List all quarantined files
    pub fn list(&self) -> Result<Vec<QuarantineEntry>> {
        if !self.quarantine_dir.exists() {
            return Ok(Vec::new());
        }

        let now = Utc::now();
        let mut entries = Vec::new();

        let read_dir = std::fs::read_dir(&self.quarantine_dir)
            .with_context(|| "Failed to read quarantine directory")?;

        for entry in read_dir {
            let entry = entry?;
            let path = entry.path();

            // Skip metadata files
            if path.extension().map(|e| e == "meta").unwrap_or(false) {
                continue;
            }

            // Read corresponding metadata
            let meta_path = path
                .with_extension("")
                .with_extension("")
                .with_file_name(format!("{}.meta", path.file_name().unwrap().to_string_lossy()));

            if meta_path.exists() {
                if let Ok(meta_content) = std::fs::read_to_string(&meta_path) {
                    if let Ok(meta) = serde_json::from_str::<QuarantineMeta>(&meta_content) {
                        let expiry = meta.quarantined_at + Duration::days(self.ttl_days);
                        let days_remaining =
                            if now < expiry { Some((expiry - now).num_days()) } else { None };

                        entries.push(QuarantineEntry {
                            quarantine_path: path,
                            meta,
                            days_remaining,
                        });
                    }
                }
            }
        }

        // Sort by quarantine date (newest first)
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.meta.quarantined_at));

        Ok(entries)
    }

    /// Restore a quarantined file to its original location
    pub fn restore(&self, quarantine_path: &Path) -> Result<PathBuf> {
        let meta_path = self.meta_path_for(quarantine_path);

        if !meta_path.exists() {
            return Err(anyhow!(
                "No metadata found for quarantined file: {}",
                quarantine_path.display()
            ));
        }

        let meta_content = std::fs::read_to_string(&meta_path)?;
        let meta: QuarantineMeta = serde_json::from_str(&meta_content)
            .with_context(|| "Failed to parse quarantine metadata")?;

        let original_path = PathBuf::from(&meta.original_path);

        // Ensure parent directory exists
        if let Some(parent) = original_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        // Check if original location is already occupied
        if original_path.exists() {
            return Err(anyhow!(
                "Cannot restore: file already exists at {}",
                original_path.display()
            ));
        }

        // Move file back to original location
        if std::fs::rename(quarantine_path, &original_path).is_err() {
            std::fs::copy(quarantine_path, &original_path)?;
            std::fs::remove_file(quarantine_path)?;
        }

        // Remove metadata
        let _ = std::fs::remove_file(&meta_path);

        Ok(original_path)
    }

    /// Delete a quarantined file permanently (no restore)
    pub fn delete_permanent(&self, quarantine_path: &Path) -> Result<()> {
        let meta_path = self.meta_path_for(quarantine_path);

        std::fs::remove_file(quarantine_path).with_context(|| {
            format!("Failed to remove quarantined file: {}", quarantine_path.display())
        })?;

        if meta_path.exists() {
            let _ = std::fs::remove_file(&meta_path);
        }

        Ok(())
    }

    /// Clean up expired quarantine entries (past TTL)
    ///
    /// Returns number of entries cleaned up
    pub fn cleanup_expired(&self) -> Result<usize> {
        let entries = self.list()?;
        let mut cleaned = 0;

        for entry in entries {
            if entry.days_remaining.is_none() {
                // Expired - delete permanently
                if let Err(e) = self.delete_permanent(&entry.quarantine_path) {
                    let p = entry.quarantine_path.display().to_string();
                    let es = format!("{:#}", e);
                    eprintln!(
                        "{}",
                        crate::i18n::tf(
                            "warn.quarantine_cleanup_failed",
                            &[("path", &p), ("error", &es)],
                        )
                    );
                } else {
                    cleaned += 1;
                }
            }
        }

        Ok(cleaned)
    }

    /// Get the metadata path for a quarantined file
    fn meta_path_for(&self, quarantine_path: &Path) -> PathBuf {
        let filename = quarantine_path.file_name().unwrap().to_string_lossy().to_string();
        self.quarantine_dir.join(format!("{}.meta", filename))
    }

    /// Get stats about quarantine
    pub fn stats(&self) -> Result<QuarantineStats> {
        let entries = self.list()?;
        let total_size: u64 = entries.iter().map(|e| e.meta.size_bytes).sum();
        let expired_count = entries.iter().filter(|e| e.days_remaining.is_none()).count();

        Ok(QuarantineStats { total_files: entries.len(), total_size, expired_files: expired_count })
    }
}

/// Quarantine statistics
#[derive(Debug)]
pub struct QuarantineStats {
    pub total_files: usize,
    pub total_size: u64,
    pub expired_files: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_quarantine_init() {
        let temp_dir = tempdir().unwrap();
        let qm = QuarantineManager::new(temp_dir.path());
        qm.init().unwrap();

        let quarantine_dir = temp_dir.path().join("quarantine");
        assert!(quarantine_dir.exists());
    }

    #[test]
    fn test_quarantine_file() {
        let temp_dir = tempdir().unwrap();
        let qm = QuarantineManager::new(temp_dir.path());

        // Create a test file
        let test_file = temp_dir.path().join("model.safetensors");
        fs::write(&test_file, "fake model content").unwrap();

        let fake_hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let quarantine_path =
            qm.quarantine(&test_file, fake_hash, "duplicate detected", vec![]).unwrap();

        // Original should be gone
        assert!(!test_file.exists(), "Original file should be removed");
        // Quarantine should exist
        assert!(quarantine_path.exists(), "Quarantine file should exist");

        // Metadata should exist
        let meta_filename = quarantine_path.file_name().unwrap().to_string_lossy().to_string();
        let meta_path = temp_dir.path().join("quarantine").join(format!("{}.meta", meta_filename));
        assert!(meta_path.exists(), "Metadata file should exist");

        // Read and verify metadata
        let meta_content = fs::read_to_string(&meta_path).unwrap();
        let meta: QuarantineMeta = serde_json::from_str(&meta_content).unwrap();
        assert_eq!(meta.original_path, test_file.to_string_lossy());
        assert_eq!(meta.reason, "duplicate detected");
        assert_eq!(meta.blake3_hash, fake_hash);
    }

    #[test]
    fn test_list_quarantine() {
        let temp_dir = tempdir().unwrap();
        let qm = QuarantineManager::with_ttl(temp_dir.path(), 30);

        // Create and quarantine two files
        for i in 0..2 {
            let test_file = temp_dir.path().join(format!("model{}.safetensors", i));
            fs::write(&test_file, format!("fake model content {}", i)).unwrap();
            let hash = format!("{:064x}", i);
            qm.quarantine(&test_file, &hash, "test", vec![]).unwrap();
        }

        let entries = qm.list().unwrap();
        assert_eq!(entries.len(), 2);

        // All should have days_remaining (not expired)
        for entry in &entries {
            assert!(entry.days_remaining.is_some());
            assert!(entry.days_remaining.unwrap() >= 29); // ~30 days
        }
    }

    #[test]
    fn test_restore_quarantined_file() {
        let temp_dir = tempdir().unwrap();
        let qm = QuarantineManager::new(temp_dir.path());

        // Create test file in a subdirectory
        let subdir = temp_dir.path().join("models");
        fs::create_dir_all(&subdir).unwrap();
        let test_file = subdir.join("model.safetensors");
        let content = b"restore test content";
        fs::write(&test_file, content).unwrap();

        let fake_hash = "1111111111111111111111111111111111111111111111111111111111111111";
        let quarantine_path = qm.quarantine(&test_file, fake_hash, "test", vec![]).unwrap();

        // Restore
        let restored_path = qm.restore(&quarantine_path).unwrap();
        assert_eq!(restored_path, test_file);
        assert!(restored_path.exists(), "Restored file should exist");

        // Content should match
        let restored_content = fs::read(&restored_path).unwrap();
        assert_eq!(restored_content, content);

        // Quarantine file should be gone
        assert!(!quarantine_path.exists());
    }

    #[test]
    fn test_cleanup_expired() {
        let temp_dir = tempdir().unwrap();
        // Use 0-day TTL so everything is immediately expired
        let qm = QuarantineManager::with_ttl(temp_dir.path(), 0);

        // Create and quarantine files
        for i in 0..3 {
            let test_file = temp_dir.path().join(format!("model{}.safetensors", i));
            fs::write(&test_file, format!("content {}", i)).unwrap();
            let hash = format!("{:064x}", i);
            qm.quarantine(&test_file, &hash, "test", vec![]).unwrap();
        }

        // All should be "expired" with 0-day TTL
        let cleaned = qm.cleanup_expired().unwrap();
        assert_eq!(cleaned, 3, "Should have cleaned up 3 expired entries");

        // Quarantine should be empty now
        let entries = qm.list().unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_quarantine_stats() {
        let temp_dir = tempdir().unwrap();
        let qm = QuarantineManager::new(temp_dir.path());

        let test_file = temp_dir.path().join("model.safetensors");
        fs::write(&test_file, "stat test content 12345").unwrap();

        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        qm.quarantine(&test_file, hash, "test", vec![]).unwrap();

        let stats = qm.stats().unwrap();
        assert_eq!(stats.total_files, 1);
        assert!(stats.total_size > 0);
        assert_eq!(stats.expired_files, 0);
    }

    #[test]
    fn test_quarantine_with_references() {
        let temp_dir = tempdir().unwrap();
        let qm = QuarantineManager::new(temp_dir.path());

        let test_file = temp_dir.path().join("model.safetensors");
        fs::write(&test_file, "ref test").unwrap();

        let references = vec![
            "/comfyui/models/model.safetensors".to_string(),
            "/forge/models/model.safetensors".to_string(),
        ];

        let hash = "2222222222222222222222222222222222222222222222222222222222222222";
        let quarantine_path = qm.quarantine(&test_file, hash, "dedup", references.clone()).unwrap();

        // Read back metadata
        let entries = qm.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].meta.references.len(), 2);
        assert_eq!(entries[0].meta.references[0], references[0]);

        // Cleanup
        let _ = qm.delete_permanent(&quarantine_path);
    }
}
