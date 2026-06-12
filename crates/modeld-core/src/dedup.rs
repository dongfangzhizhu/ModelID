//! Deduplication engine with two-phase commit protocol
//!
//! Implements RFC 0004 deduplication strategy:
//! - Canonical path selection algorithm
//! - Two-phase commit protocol for atomic operations
//! - Crash recovery via WAL
//! - Multiple dedup modes (interactive, dry-run, auto, report)

use crate::db::{AliasType, Database, Frontend, TransactionStatus};
use crate::hash::{hash_file, Blake3Hash};
use crate::links::{create_link, LinkCapability, LinkResult};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

/// Duplicate group - files with same hash
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub hash: Blake3Hash,
    pub files: Vec<FileInfo>,
    pub total_size: u64,
}

/// File information for deduplication
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: SystemTime,
}

impl FileInfo {
    pub fn from_path(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("Failed to get metadata for {}", path.display()))?;

        let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        Ok(FileInfo {
            path: path.to_path_buf(),
            size: metadata.len(),
            mtime,
        })
    }
}

/// Canonical path selection result
#[derive(Debug)]
pub struct CanonicalSelection {
    pub canonical: PathBuf,
    pub duplicates: Vec<PathBuf>,
    pub reason: String,
}

/// Deduplication mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupMode {
    Interactive,
    DryRun,
    Auto,
    Report,
}

/// Result of a dedup operation on a single group
#[derive(Debug)]
pub struct DedupGroupResult {
    pub hash: Blake3Hash,
    pub canonical: PathBuf,
    pub links_created: Vec<(PathBuf, AliasType)>,
    pub space_saved: u64,
}

/// Deduplication statistics
#[derive(Debug, Default)]
pub struct DedupStats {
    pub groups_processed: usize,
    pub groups_succeeded: usize,
    pub groups_failed: usize,
    pub space_saved: u64,
    pub files_deduplicated: usize,
}

/// Deduplication engine
pub struct DedupEngine {
    db: Database,
    store_path: PathBuf,
    link_capability: LinkCapability,
}

impl DedupEngine {
    pub fn new(db: Database, store_path: PathBuf) -> Self {
        let link_capability = LinkCapability::detect();
        Self {
            db,
            store_path,
            link_capability,
        }
    }

    pub fn with_link_capability(
        db: Database,
        store_path: PathBuf,
        link_capability: LinkCapability,
    ) -> Self {
        Self {
            db,
            store_path,
            link_capability,
        }
    }

    /// Find all duplicate groups in the database
    pub fn find_duplicates(&self) -> Result<Vec<DuplicateGroup>> {
        let models = self.db.list_models(None)?;
        let mut groups = Vec::new();

        for model in models {
            // Get all aliases for this model
            let aliases = self.db.get_aliases_for_model(&model.blake3_hash)?;

            // Only include if there are 2+ files (duplicates exist)
            if aliases.len() >= 2 {
                let mut files = Vec::new();

                for alias in aliases {
                    let path = PathBuf::from(&alias.path);
                    if path.exists() {
                        if let Ok(info) = FileInfo::from_path(&path) {
                            files.push(info);
                        }
                    }
                }

                if files.len() >= 2 {
                    groups.push(DuplicateGroup {
                        hash: model.blake3_hash.clone(),
                        files,
                        total_size: model.size_bytes as u64,
                    });
                }
            }
        }

        Ok(groups)
    }

    /// Select canonical file from duplicate group
    ///
    /// Priority order (RFC 0004):
    /// 1. Already in CAS → Use existing CAS object
    /// 2. Oldest mtime → Likely the original file
    /// 3. Shortest path → Simpler to reference
    /// 4. First alphabetically → Deterministic tiebreaker
    pub fn select_canonical(&self, files: &[FileInfo]) -> CanonicalSelection {
        if files.is_empty() {
            panic!("Cannot select canonical from empty file list");
        }

        let cas_prefix = self.store_path.join("cas");

        // Priority 1: Check if any file already in CAS
        for file in files {
            if file.path.starts_with(&cas_prefix) {
                let duplicates = files
                    .iter()
                    .filter(|f| f.path != file.path)
                    .map(|f| f.path.clone())
                    .collect();

                return CanonicalSelection {
                    canonical: file.path.clone(),
                    duplicates,
                    reason: "Already in CAS (immutable, verified)".to_string(),
                };
            }
        }

        // Priority 2-4: Find oldest, then shortest, then alphabetical
        let canonical = files
            .iter()
            .min_by_key(|file| {
                (
                    file.mtime,                               // Oldest first
                    file.path.as_os_str().len(),              // Shortest path
                    file.path.to_string_lossy().to_string(),  // Alphabetical
                )
            })
            .unwrap();

        let duplicates = files
            .iter()
            .filter(|f| f.path != canonical.path)
            .map(|f| f.path.clone())
            .collect();

        let reason = if files.iter().filter(|f| f.mtime == canonical.mtime).count() > 1 {
            if files
                .iter()
                .filter(|f| f.path.as_os_str().len() == canonical.path.as_os_str().len())
                .count()
                > 1
            {
                "Alphabetically first (deterministic)".to_string()
            } else {
                "Shortest path (simpler reference)".to_string()
            }
        } else {
            "Oldest file (likely original)".to_string()
        };

        CanonicalSelection {
            canonical: canonical.path.clone(),
            duplicates,
            reason,
        }
    }

    /// Calculate potential space savings from deduplication
    pub fn calculate_savings(&self, groups: &[DuplicateGroup]) -> u64 {
        groups
            .iter()
            .map(|group| {
                // Save space for all duplicates (keep canonical)
                group.total_size * (group.files.len() as u64 - 1)
            })
            .sum()
    }

    /// Execute two-phase commit deduplication for a single group
    ///
    /// Phase A (Prepare):
    /// 1. Generate transaction ID (UUID)
    /// 2. Write WAL record (status='pending')
    /// 3. Copy canonical to staging (tmp/cas_staging/{hash}.tmp)
    /// 4. Verify hash of staged file
    /// 5. Update WAL (status='copied')
    /// 6. fsync equivalent (database commit)
    ///
    /// Phase B (Commit):
    /// 7. Atomic rename to CAS
    /// 8. Create links for each duplicate
    /// 9. Record aliases in database
    /// 10. Update WAL (status='committed')
    pub fn execute_dedup_group(
        &mut self,
        group: &DuplicateGroup,
        mode: DedupMode,
    ) -> Result<DedupGroupResult> {
        let selection = self.select_canonical(&group.files);

        // Dry-run: just report, don't execute
        if mode == DedupMode::DryRun || mode == DedupMode::Report {
            return Ok(DedupGroupResult {
                hash: group.hash.clone(),
                canonical: selection.canonical,
                links_created: Vec::new(),
                space_saved: group.total_size * (selection.duplicates.len() as u64),
            });
        }

        // === PHASE A: PREPARE ===
        let tx_id = Uuid::new_v4().to_string();
        let staging_dir = self.store_path.join("tmp").join("cas_staging");
        std::fs::create_dir_all(&staging_dir)
            .with_context(|| format!("Failed to create staging dir: {}", staging_dir.display()))?;

        let staging_path = staging_dir.join(format!("{}.tmp", group.hash.as_hex()));
        let cas_path = self.cas_path_for_hash(&group.hash);

        // Write WAL record - status=pending
        self.db.insert_wal_transaction(
            &tx_id,
            "dedup",
            TransactionStatus::Pending,
            Some(selection.canonical.to_string_lossy().as_ref()),
            Some(group.hash.as_hex()),
            None,
        )?;

        // Check if already in CAS (skip staging if so)
        let already_in_cas = cas_path.exists();

        if !already_in_cas {
            // Copy canonical to staging
            std::fs::copy(&selection.canonical, &staging_path).with_context(|| {
                format!(
                    "Failed to copy {} to staging {}",
                    selection.canonical.display(),
                    staging_path.display()
                )
            })?;

            // Verify hash of staged file
            let staged_hash = hash_file(&staging_path)
                .with_context(|| "Failed to hash staged file during verification")?;

            if staged_hash.as_hex() != group.hash.as_hex() {
                // Hash mismatch - clean up and abort
                let _ = std::fs::remove_file(&staging_path);
                self.db
                    .update_wal_status(&tx_id, TransactionStatus::Failed)?;
                return Err(anyhow!(
                    "Hash mismatch during staging: expected {}, got {}",
                    group.hash.as_hex(),
                    staged_hash.as_hex()
                ));
            }
        }

        // Update WAL - status=copied
        self.db
            .update_wal_status(&tx_id, TransactionStatus::Copied)?;

        // === PHASE B: COMMIT ===

        // Atomic rename to CAS (if not already there)
        if !already_in_cas {
            let cas_dir = cas_path.parent().unwrap();
            std::fs::create_dir_all(cas_dir)?;

            // On Windows, rename can fail across volumes - use copy+delete as fallback
            if let Err(_) = std::fs::rename(&staging_path, &cas_path) {
                // Fallback: copy then remove staging
                std::fs::copy(&staging_path, &cas_path)?;
                let _ = std::fs::remove_file(&staging_path);
            }

            // Make CAS object read-only (immutable)
            let mut perms = std::fs::metadata(&cas_path)?.permissions();
            perms.set_readonly(true);
            let _ = std::fs::set_permissions(&cas_path, perms);
        }

        // Create links for each duplicate
        let mut links_created = Vec::new();
        let mut space_saved = 0u64;

        // First, ensure canonical is tracked in aliases
        let canonical_path_str = selection.canonical.to_string_lossy().to_string();
        if self.db.get_alias_by_path(&canonical_path_str)?.is_none() {
            self.db.insert_alias(
                &group.hash,
                &canonical_path_str,
                Frontend::User,
                AliasType::Original,
            )?;
        }

        // Create links for duplicates
        for dup_path in &selection.duplicates {
            let link_result = create_link(dup_path, &cas_path, &self.link_capability);

            let alias_type = match &link_result {
                LinkResult::Success(atype) => atype.clone(),
                LinkResult::Failed(err) => {
                    eprintln!(
                        "Warning: failed to create link for {}: {}",
                        dup_path.display(),
                        err
                    );
                    // Fall back to reference-only
                    AliasType::ReferenceOnly
                }
            };

            // Record alias in database
            let dup_path_str = dup_path.to_string_lossy().to_string();
            if self.db.get_alias_by_path(&dup_path_str)?.is_none() {
                self.db
                    .insert_alias(&group.hash, &dup_path_str, Frontend::User, alias_type.clone())?;
            } else {
                // Update existing alias type if changed
                self.db.delete_alias(&dup_path_str)?;
                self.db
                    .insert_alias(&group.hash, &dup_path_str, Frontend::User, alias_type.clone())?;
            }

            links_created.push((dup_path.clone(), alias_type));
            space_saved += group.total_size;
        }

        // Update WAL - status=committed
        self.db
            .update_wal_status(&tx_id, TransactionStatus::Committed)?;

        // Clean up committed WAL entry
        self.db.delete_wal_transaction(&tx_id)?;

        Ok(DedupGroupResult {
            hash: group.hash.clone(),
            canonical: selection.canonical,
            links_created,
            space_saved,
        })
    }

    /// Run deduplication on all duplicate groups
    pub fn run_dedup(
        &mut self,
        mode: DedupMode,
        progress_fn: impl Fn(usize, usize, &str),
    ) -> Result<DedupStats> {
        // First, recover any incomplete transactions from previous crashes
        self.recover_incomplete_transactions()?;

        let groups = self.find_duplicates()?;
        let total = groups.len();
        let mut stats = DedupStats::default();

        for (i, group) in groups.iter().enumerate() {
            let hash_prefix = &group.hash.as_hex()[..8];
            progress_fn(i + 1, total, hash_prefix);

            stats.groups_processed += 1;

            match self.execute_dedup_group(group, mode) {
                Ok(result) => {
                    stats.groups_succeeded += 1;
                    stats.space_saved += result.space_saved;
                    stats.files_deduplicated += result.links_created.len();
                }
                Err(e) => {
                    stats.groups_failed += 1;
                    eprintln!("Warning: dedup failed for group {}: {}", hash_prefix, e);
                }
            }
        }

        Ok(stats)
    }

    /// Recover incomplete WAL transactions after crash
    ///
    /// Recovery strategy:
    /// - pending: staging file may exist, clean up and mark failed
    /// - copied: staging is valid, continue Phase B
    /// - committed: already done, just clean up WAL entry
    pub fn recover_incomplete_transactions(&mut self) -> Result<usize> {
        let incomplete = self.db.get_incomplete_wal_transactions()?;
        let count = incomplete.len();

        for tx in incomplete {
            let staging_dir = self.store_path.join("tmp").join("cas_staging");

            match tx.status {
                TransactionStatus::Pending => {
                    // Phase A didn't complete - clean up staging file if it exists
                    if let Some(ref hash) = tx.target_hash {
                        let staging_path = staging_dir.join(format!("{}.tmp", hash));
                        if staging_path.exists() {
                            let _ = std::fs::remove_file(&staging_path);
                        }
                    }
                    // Mark as failed
                    self.db.update_wal_status(&tx.tx_id, TransactionStatus::Failed)?;
                    eprintln!(
                        "Recovery: rolled back pending transaction {}",
                        &tx.tx_id[..8]
                    );
                }

                TransactionStatus::Copied => {
                    // Phase A completed but Phase B didn't start
                    // Try to continue Phase B
                    if let (Some(ref hash_str), Some(ref source_path)) =
                        (&tx.target_hash, &tx.source_path)
                    {
                        let staging_path = staging_dir.join(format!("{}.tmp", hash_str));

                        if let Ok(hash) = Blake3Hash::from_hex(hash_str) {
                            let cas_path = self.cas_path_for_hash(&hash);

                            if staging_path.exists() && !cas_path.exists() {
                                // Continue Phase B: move staging to CAS
                                if let Some(cas_dir) = cas_path.parent() {
                                    let _ = std::fs::create_dir_all(cas_dir);
                                }

                                if let Err(e) = std::fs::rename(&staging_path, &cas_path) {
                                    eprintln!(
                                        "Recovery: failed to move staging to CAS for tx {}: {}",
                                        &tx.tx_id[..8],
                                        e
                                    );
                                    // Try copy+delete
                                    if let Ok(_) = std::fs::copy(&staging_path, &cas_path) {
                                        let _ = std::fs::remove_file(&staging_path);
                                    }
                                }

                                // Make read-only
                                if cas_path.exists() {
                                    if let Ok(meta) = std::fs::metadata(&cas_path) {
                                        let mut perms = meta.permissions();
                                        perms.set_readonly(true);
                                        let _ = std::fs::set_permissions(&cas_path, perms);
                                    }

                                    // Re-create links for the source path
                                    let src = PathBuf::from(source_path);
                                    if src.exists() {
                                        let link_result =
                                            create_link(&src, &cas_path, &self.link_capability);
                                        let alias_type = match link_result {
                                            LinkResult::Success(t) => t,
                                            LinkResult::Failed(_) => AliasType::ReferenceOnly,
                                        };
                                        let _ = self.db.insert_alias(
                                            &hash,
                                            source_path,
                                            Frontend::User,
                                            alias_type,
                                        );
                                    }

                                    eprintln!(
                                        "Recovery: completed Phase B for tx {}",
                                        &tx.tx_id[..8]
                                    );
                                }
                            } else if cas_path.exists() {
                                // CAS already has the file, just clean up staging
                                if staging_path.exists() {
                                    let _ = std::fs::remove_file(&staging_path);
                                }
                            }
                        }
                    }

                    // Mark committed
                    self.db
                        .update_wal_status(&tx.tx_id, TransactionStatus::Committed)?;
                    self.db.delete_wal_transaction(&tx.tx_id)?;
                }

                TransactionStatus::Committed => {
                    // Already committed, just clean WAL entry
                    self.db.delete_wal_transaction(&tx.tx_id)?;
                }

                TransactionStatus::Failed => {
                    // Already marked failed, clean up
                    self.db.delete_wal_transaction(&tx.tx_id)?;
                }
            }
        }

        if count > 0 {
            eprintln!("Recovery: processed {} incomplete transactions", count);
        }

        Ok(count)
    }

    /// Get CAS path for a given hash
    pub fn cas_path_for_hash(&self, hash: &Blake3Hash) -> PathBuf {
        let hex = hash.as_hex();
        let prefix = hash.prefix();
        self.store_path
            .join("cas")
            .join("blake3")
            .join(prefix)
            .join(&hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Blake3Hash;
    use std::fs;
    use tempfile::{tempdir, NamedTempFile};

    fn make_engine(temp_dir: &tempfile::TempDir) -> (DedupEngine, NamedTempFile) {
        let temp_db = NamedTempFile::new().unwrap();
        let db = Database::open(temp_db.path()).unwrap();
        let engine = DedupEngine::new(db, temp_dir.path().to_path_buf());
        (engine, temp_db)
    }

    #[test]
    fn test_select_canonical_oldest() {
        let temp_dir = tempdir().unwrap();
        let (engine, _db) = make_engine(&temp_dir);

        // Create files with different mtimes
        let file1 = temp_dir.path().join("file1.txt");
        let file2 = temp_dir.path().join("file2.txt");
        let file3 = temp_dir.path().join("file3.txt");

        fs::write(&file1, "test").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        fs::write(&file2, "test").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        fs::write(&file3, "test").unwrap();

        let files = vec![
            FileInfo::from_path(&file1).unwrap(),
            FileInfo::from_path(&file2).unwrap(),
            FileInfo::from_path(&file3).unwrap(),
        ];

        let selection = engine.select_canonical(&files);

        // file1 should be canonical (oldest or shortest/alphabetical if timing doesn't work)
        assert_eq!(selection.canonical, file1);
        assert_eq!(selection.duplicates.len(), 2);
        println!("Reason: {}", selection.reason);
    }

    #[test]
    fn test_select_canonical_cas_priority() {
        let temp_dir = tempdir().unwrap();
        let (engine, _db) = make_engine(&temp_dir);

        // Create CAS structure
        let cas_dir = temp_dir.path().join("cas").join("blake3").join("ab");
        fs::create_dir_all(&cas_dir).unwrap();

        let cas_file = cas_dir.join("abcdef123");
        let user_file = temp_dir.path().join("user_file.txt");

        fs::write(&cas_file, "test").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(&user_file, "test").unwrap();

        let files = vec![
            FileInfo::from_path(&user_file).unwrap(),
            FileInfo::from_path(&cas_file).unwrap(),
        ];

        let selection = engine.select_canonical(&files);

        // CAS file should be canonical even though user_file is older
        assert_eq!(selection.canonical, cas_file);
        assert!(selection.reason.contains("CAS"));
    }

    #[test]
    fn test_select_canonical_shortest_path() {
        let temp_dir = tempdir().unwrap();
        let (engine, _db) = make_engine(&temp_dir);

        let short_path = temp_dir.path().join("a.txt");
        let long_path = temp_dir.path().join("very_long_filename_here.txt");

        fs::write(&short_path, "test").unwrap();
        fs::write(&long_path, "test").unwrap();

        let files = vec![
            FileInfo::from_path(&short_path).unwrap(),
            FileInfo::from_path(&long_path).unwrap(),
        ];

        let selection = engine.select_canonical(&files);

        // Shortest path should be selected
        assert_eq!(selection.canonical, short_path);
    }

    #[test]
    fn test_calculate_savings() {
        let temp_dir = tempdir().unwrap();
        let (engine, _db) = make_engine(&temp_dir);

        let groups = vec![
            DuplicateGroup {
                hash: Blake3Hash::from_hex(
                    "1111111111111111111111111111111111111111111111111111111111111111",
                )
                .unwrap(),
                files: vec![
                    FileInfo {
                        path: PathBuf::from("file1"),
                        size: 1000,
                        mtime: SystemTime::now(),
                    };
                    3
                ],
                total_size: 1000,
            },
            DuplicateGroup {
                hash: Blake3Hash::from_hex(
                    "2222222222222222222222222222222222222222222222222222222222222222",
                )
                .unwrap(),
                files: vec![
                    FileInfo {
                        path: PathBuf::from("file2"),
                        size: 2000,
                        mtime: SystemTime::now(),
                    };
                    2
                ],
                total_size: 2000,
            },
        ];

        let savings = engine.calculate_savings(&groups);

        // Group 1: 1000 * (3-1) = 2000
        // Group 2: 2000 * (2-1) = 2000
        // Total: 4000
        assert_eq!(savings, 4000);
    }

    #[test]
    fn test_find_duplicates_empty() {
        let temp_dir = tempdir().unwrap();
        let (engine, _db) = make_engine(&temp_dir);

        let groups = engine.find_duplicates().unwrap();
        assert_eq!(groups.len(), 0);
    }

    #[test]
    fn test_recover_no_transactions() {
        let temp_dir = tempdir().unwrap();
        let (mut engine, _db) = make_engine(&temp_dir);

        // Should succeed with no incomplete transactions
        let count = engine.recover_incomplete_transactions().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_recover_pending_transaction() {
        let temp_dir = tempdir().unwrap();
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        // Insert a pending WAL transaction
        let tx_id = "test-tx-pending-001";
        let hash_str = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        db.insert_wal_transaction(
            tx_id,
            "dedup",
            TransactionStatus::Pending,
            Some("/original/file.safetensors"),
            Some(hash_str),
            None,
        )
        .unwrap();

        // Create a fake staging file
        let staging_dir = temp_dir.path().join("tmp").join("cas_staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_file = staging_dir.join(format!("{}.tmp", hash_str));
        fs::write(&staging_file, "fake content").unwrap();

        let mut engine = DedupEngine::new(db, temp_dir.path().to_path_buf());
        let count = engine.recover_incomplete_transactions().unwrap();

        // Should have processed 1 transaction
        assert_eq!(count, 1);
        // Staging file should be removed
        assert!(!staging_file.exists());
    }

    #[test]
    fn test_two_phase_commit_dry_run() {
        let temp_dir = tempdir().unwrap();
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        // Create test file with known hash
        let test_file1 = temp_dir.path().join("model1.safetensors");
        let test_file2 = temp_dir.path().join("model2.safetensors");
        let content = b"fake model content for testing";
        fs::write(&test_file1, content).unwrap();
        fs::write(&test_file2, content).unwrap();

        // Hash the files
        let hash = crate::hash::hash_file(&test_file1).unwrap();

        // Register in DB
        db.insert_or_update_model(&hash, content.len() as i64, None, None, None, None)
            .unwrap();
        db.insert_alias(
            &hash,
            &test_file1.to_string_lossy(),
            crate::db::Frontend::User,
            AliasType::Original,
        )
        .unwrap();
        db.insert_alias(
            &hash,
            &test_file2.to_string_lossy(),
            crate::db::Frontend::User,
            AliasType::Original,
        )
        .unwrap();

        let mut engine = DedupEngine::new(db, temp_dir.path().to_path_buf());
        let groups = engine.find_duplicates().unwrap();
        assert_eq!(groups.len(), 1);

        // Dry run should not create any links
        let result = engine
            .execute_dedup_group(&groups[0], DedupMode::DryRun)
            .unwrap();
        assert_eq!(result.links_created.len(), 0);
        // Space saved should be non-zero
        assert!(result.space_saved > 0);
    }

    #[test]
    fn test_two_phase_commit_auto() {
        let temp_dir = tempdir().unwrap();
        let temp_db = NamedTempFile::new().unwrap();
        let mut db = Database::open(temp_db.path()).unwrap();

        // Create duplicate files
        let test_file1 = temp_dir.path().join("model1.safetensors");
        let test_file2 = temp_dir.path().join("model2.safetensors");
        let content = b"fake model content for dedup testing 12345";
        fs::write(&test_file1, content).unwrap();
        fs::write(&test_file2, content).unwrap();

        let hash = crate::hash::hash_file(&test_file1).unwrap();

        db.insert_or_update_model(&hash, content.len() as i64, None, None, None, None)
            .unwrap();
        db.insert_alias(
            &hash,
            &test_file1.to_string_lossy(),
            crate::db::Frontend::User,
            AliasType::Original,
        )
        .unwrap();
        db.insert_alias(
            &hash,
            &test_file2.to_string_lossy(),
            crate::db::Frontend::User,
            AliasType::Original,
        )
        .unwrap();

        let mut engine = DedupEngine::new(db, temp_dir.path().to_path_buf());
        let groups = engine.find_duplicates().unwrap();
        assert_eq!(groups.len(), 1);

        // Auto mode should actually execute
        let result = engine
            .execute_dedup_group(&groups[0], DedupMode::Auto)
            .unwrap();

        // CAS file should exist
        let cas_path = engine.cas_path_for_hash(&groups[0].hash);
        assert!(cas_path.exists(), "CAS file should exist after dedup");

        // Link(s) should have been created
        assert!(
            !result.links_created.is_empty(),
            "Should have created links"
        );
        println!(
            "Links created: {:?}",
            result.links_created.iter().map(|(p, t)| (p.display().to_string(), t.as_str())).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_cas_path_for_hash() {
        let temp_dir = tempdir().unwrap();
        let (engine, _db) = make_engine(&temp_dir);

        let hash = Blake3Hash::from_hex(
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        )
        .unwrap();

        let path = engine.cas_path_for_hash(&hash);
        assert!(path.to_string_lossy().contains("cas"));
        assert!(path.to_string_lossy().contains("blake3"));
        assert!(path.to_string_lossy().contains("ab")); // prefix
        assert!(path
            .to_string_lossy()
            .contains("abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"));
    }
}
