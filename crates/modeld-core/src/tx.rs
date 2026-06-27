//! Transaction Manager for atomic multi-file operations
//!
//! Provides a high-level WAL-backed transaction lifecycle:
//! - `begin()` → PENDING
//! - `commit()` → COMMITTED
//! - `fail()` → FAILED
//! - `rollback()` → ROLLED_BACK (idempotent, second call is no-op)
//! - `recover()` → scans PENDING transactions left by crashed processes,
//!                marks them FAILED and cleans up staging files
//! - `list()` → query transaction history with optional filter
//! - `cleanup_older_than()` → purge old transaction records

use crate::db::{Database, TransactionStatus};
use crate::hash::Blake3Hash;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// Type of operation being managed by a transaction
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpType {
    Dedup,
    Unlink,
    Gc,
    QuarantineCleanup,
    AliasRewrite,
    CasPromotion,
    HfDownloadCommit,
    StoreMigration,
}

impl OpType {
    pub fn as_str(&self) -> &'static str {
        match self {
            OpType::Dedup => "dedup",
            OpType::Unlink => "unlink",
            OpType::Gc => "gc",
            OpType::QuarantineCleanup => "quarantine_cleanup",
            OpType::AliasRewrite => "alias_rewrite",
            OpType::CasPromotion => "cas_promotion",
            OpType::HfDownloadCommit => "hf_download_commit",
            OpType::StoreMigration => "store_migration",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "dedup" => Some(OpType::Dedup),
            "unlink" => Some(OpType::Unlink),
            "gc" => Some(OpType::Gc),
            "quarantine_cleanup" => Some(OpType::QuarantineCleanup),
            "alias_rewrite" => Some(OpType::AliasRewrite),
            "cas_promotion" => Some(OpType::CasPromotion),
            "hf_download_commit" => Some(OpType::HfDownloadCommit),
            "store_migration" => Some(OpType::StoreMigration),
            _ => None,
        }
    }
}

/// High-level transaction status (simpler than the internal DB `TransactionStatus`
/// which also tracks the intermediate `Copied` phase used by the dedup engine).
#[derive(Debug, Clone, PartialEq)]
pub enum TxStatus {
    Pending,
    Committed,
    Failed,
    RolledBack,
}

impl TxStatus {
    fn from_db(status: &TransactionStatus) -> Self {
        match status {
            TransactionStatus::Pending | TransactionStatus::Copied => TxStatus::Pending,
            TransactionStatus::Committed => TxStatus::Committed,
            TransactionStatus::Failed => TxStatus::Failed,
            TransactionStatus::RolledBack => TxStatus::RolledBack,
        }
    }

    #[allow(dead_code)]
    fn to_db(&self) -> TransactionStatus {
        match self {
            TxStatus::Pending => TransactionStatus::Pending,
            TxStatus::Committed => TransactionStatus::Committed,
            TxStatus::Failed => TransactionStatus::Failed,
            TxStatus::RolledBack => TransactionStatus::RolledBack,
        }
    }
}

/// Describes a single filesystem path affected by a transaction.
///
/// Used both in `affected_paths` (what changed) and `rollback_plan.entries`
/// (what to undo).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathEntry {
    /// The path that was modified (source of the operation).
    pub source: PathBuf,
    /// Where the source was linked/moved to (e.g. the CAS path), if any.
    pub target: Option<PathBuf>,
    /// Hash of the file at `source` before the operation.
    pub original_hash: Option<Blake3Hash>,
    /// Hash of the file at `source` after the operation (may equal original for links).
    pub new_hash: Option<Blake3Hash>,
    /// Size of the file in bytes.
    pub size: u64,
}

/// Contains all information needed to restore the pre-transaction state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RollbackPlan {
    pub entries: Vec<PathEntry>,
}

/// Input provided to `TransactionManager::begin()`.
pub struct TxPlan {
    pub op_type: OpType,
    pub affected_paths: Vec<PathEntry>,
    pub rollback_plan: RollbackPlan,
}

/// Opaque handle returned by `begin()`.  Pass to `commit()` or `fail()`.
pub struct TxHandle {
    pub tx_id: String,
}

/// A complete transaction record as read from the database.
#[derive(Debug, Clone)]
pub struct TxRecord {
    pub tx_id: String,
    pub op_type: Option<OpType>,
    pub status: TxStatus,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub affected_paths: Vec<PathEntry>,
    pub rollback_plan: RollbackPlan,
    pub error_message: Option<String>,
}

/// Filter criteria for `TransactionManager::list()`.
#[derive(Debug, Default)]
pub struct TxFilter {
    pub status: Option<TxStatus>,
    pub op_type: Option<OpType>,
}

/// Result of recovering a single crashed transaction.
#[derive(Debug)]
pub struct TxRecoveryResult {
    pub tx_id: String,
    pub action: RecoveryAction,
}

/// What was done during recovery.
#[derive(Debug)]
pub enum RecoveryAction {
    /// Transaction was in PENDING state and has been marked FAILED.
    MarkedFailed,
    /// Staging directory for this transaction was cleaned up.
    StagingCleaned,
}

// ─────────────────────────────────────────────────────────────────────────────
// TransactionManager
// ─────────────────────────────────────────────────────────────────────────────

/// High-level WAL-backed transaction manager.
///
/// Wraps the lower-level `Database` WAL methods with lifecycle logic, JSON
/// serialisation of rollback plans, and crash-recovery.
pub struct TransactionManager<'a> {
    pub db: &'a mut Database,
    pub store_path: PathBuf,
}

impl<'a> TransactionManager<'a> {
    /// Create a new `TransactionManager` over the given database and store root.
    pub fn new(db: &'a mut Database, store_path: impl AsRef<Path>) -> Self {
        Self { db, store_path: store_path.as_ref().to_path_buf() }
    }

    // ── Lifecycle ────────────────────────────────────────────────────────────

    /// Begin a new transaction.  Inserts a PENDING record in the WAL.
    pub fn begin(&mut self, _op: OpType, plan: TxPlan) -> Result<TxHandle> {
        let tx_id = Uuid::new_v4().to_string();
        let op_str = plan.op_type.as_str();

        let affected_json = serde_json::to_string(&plan.affected_paths)
            .context("Failed to serialize affected_paths")?;
        let rollback_json = serde_json::to_string(&plan.rollback_plan)
            .context("Failed to serialize rollback_plan")?;

        self.db.insert_wal_transaction(
            &tx_id,
            op_str,
            TransactionStatus::Pending,
            None,
            None,
            None,
        )?;

        self.db.update_wal_extended(
            &tx_id,
            Some(op_str),
            Some(&affected_json),
            Some(&rollback_json),
            None,
            None,
        )?;

        Ok(TxHandle { tx_id })
    }

    /// Commit a transaction.  Sets status to COMMITTED and records end time.
    pub fn commit(&mut self, handle: TxHandle) -> Result<()> {
        let end_time = Utc::now().to_rfc3339();
        self.db.update_wal_status(&handle.tx_id, TransactionStatus::Committed)?;
        self.db.update_wal_extended(&handle.tx_id, None, None, None, Some(&end_time), None)?;
        Ok(())
    }

    /// Mark a transaction as failed.  Records the error message and end time.
    pub fn fail(&mut self, handle: TxHandle, err: &str) -> Result<()> {
        let end_time = Utc::now().to_rfc3339();
        self.db.update_wal_status(&handle.tx_id, TransactionStatus::Failed)?;
        self.db.update_wal_extended(
            &handle.tx_id,
            None,
            None,
            None,
            Some(&end_time),
            Some(err),
        )?;
        Ok(())
    }

    /// Roll back a transaction.
    ///
    /// - Reads the stored `rollback_plan` and restores all affected files.
    /// - Updates the status to ROLLED_BACK.
    /// - Second call on an already-rolled-back transaction is a **no-op**.
    pub fn rollback(&mut self, tx_id: &str) -> Result<()> {
        let tx = self
            .db
            .get_wal_transaction(tx_id)?
            .ok_or_else(|| anyhow!("Transaction not found: {}", tx_id))?;

        // Idempotency: second call is a no-op
        if tx.status == TransactionStatus::RolledBack {
            return Ok(());
        }

        // Execute rollback plan
        if let Some(rollback_json) = &tx.rollback_plan {
            match serde_json::from_str::<RollbackPlan>(rollback_json) {
                Ok(plan) => {
                    for entry in &plan.entries {
                        if let Err(e) = self.restore_entry(entry) {
                            eprintln!(
                                "tx: rollback entry failed for {}: {:#}",
                                entry.source.display(),
                                e
                            );
                            // Continue rolling back remaining entries
                        }
                    }
                }
                Err(e) => {
                    eprintln!("tx: failed to parse rollback_plan for {}: {}", tx_id, e);
                }
            }
        }

        let end_time = Utc::now().to_rfc3339();
        self.db.update_wal_status(tx_id, TransactionStatus::RolledBack)?;
        self.db.update_wal_extended(tx_id, None, None, None, Some(&end_time), None)?;
        Ok(())
    }

    // ── Recovery ─────────────────────────────────────────────────────────────

    /// Recover from a crash: scan for PENDING transactions, mark them FAILED,
    /// and clean up staging files left behind in `tmp/cas_staging/`.
    pub fn recover(&mut self) -> Result<Vec<TxRecoveryResult>> {
        let incomplete = self.db.get_incomplete_wal_transactions()?;
        let mut results = Vec::new();

        for tx in incomplete {
            // Clean up per-transaction staging directory
            let staging_dir =
                self.store_path.join("tmp").join("cas_staging").join(&tx.tx_id);
            if staging_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&staging_dir) {
                    eprintln!(
                        "tx: recover — failed to remove staging dir {}: {}",
                        staging_dir.display(),
                        e
                    );
                } else {
                    results.push(TxRecoveryResult {
                        tx_id: tx.tx_id.clone(),
                        action: RecoveryAction::StagingCleaned,
                    });
                }
            }

            // Also clean up legacy flat staging files (dedup engine format)
            if let Some(ref hash_str) = tx.target_hash {
                let legacy_path = self
                    .store_path
                    .join("tmp")
                    .join("cas_staging")
                    .join(format!("{}.tmp", hash_str));
                if legacy_path.exists() {
                    let _ = std::fs::remove_file(&legacy_path);
                }
            }

            let end_time = Utc::now().to_rfc3339();
            self.db.update_wal_status(&tx.tx_id, TransactionStatus::Failed)?;
            self.db.update_wal_extended(
                &tx.tx_id,
                None,
                None,
                None,
                Some(&end_time),
                Some("Recovered from crash: process was interrupted"),
            )?;

            results.push(TxRecoveryResult {
                tx_id: tx.tx_id,
                action: RecoveryAction::MarkedFailed,
            });
        }

        Ok(results)
    }

    // ── Query ────────────────────────────────────────────────────────────────

    /// List transactions with optional filtering by status and/or op_type.
    pub fn list(&self, filter: TxFilter) -> Result<Vec<TxRecord>> {
        let txs = self.db.list_wal_transactions()?;

        let records: Vec<TxRecord> = txs
            .into_iter()
            .filter_map(|tx| {
                let tx_status = TxStatus::from_db(&tx.status);

                // Status filter
                if let Some(ref s) = filter.status {
                    if &tx_status != s {
                        return None;
                    }
                }

                let op_type = tx.op_type.as_deref().and_then(OpType::from_str);

                // Op-type filter
                if let Some(ref ot) = filter.op_type {
                    if op_type.as_ref() != Some(ot) {
                        return None;
                    }
                }

                let affected_paths: Vec<PathEntry> = tx
                    .affected_paths
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();

                let rollback_plan: RollbackPlan = tx
                    .rollback_plan
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();

                let end_time = tx
                    .end_time
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc));

                Some(TxRecord {
                    tx_id: tx.tx_id,
                    op_type,
                    status: tx_status,
                    start_time: tx.created_at,
                    end_time,
                    affected_paths,
                    rollback_plan,
                    error_message: tx.error_message,
                })
            })
            .collect();

        Ok(records)
    }

    /// Delete transaction records older than `duration`.
    ///
    /// Returns the number of records removed.
    pub fn cleanup_older_than(&mut self, duration: Duration) -> Result<usize> {
        let threshold = Utc::now()
            - chrono::Duration::from_std(duration)
                .map_err(|e| anyhow!("Invalid duration: {}", e))?;

        let all = self.db.list_wal_transactions()?;
        let mut count = 0;

        for tx in all {
            if tx.created_at < threshold {
                self.db.delete_wal_transaction(&tx.tx_id)?;
                count += 1;
            }
        }

        Ok(count)
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    /// Restore a single file from its rollback plan entry.
    ///
    /// For dedup rollback: copies the CAS object (`target`) back to the
    /// original path (`source`), converting the link back to an independent copy.
    fn restore_entry(&self, entry: &PathEntry) -> Result<()> {
        // Nothing to do if source directory no longer exists
        if let Some(parent) = entry.source.parent() {
            if !parent.exists() {
                return Ok(());
            }
        }

        // Use `target` (CAS path) as the restore source
        let restore_from = match &entry.target {
            Some(t) if t.exists() => t.clone(),
            _ => return Ok(()), // no restore source available
        };

        // On Windows: make `source` writable before overwriting
        #[cfg(windows)]
        if entry.source.exists() {
            if let Ok(mut perms) = std::fs::metadata(&entry.source).map(|m| m.permissions()) {
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(&entry.source, perms);
            }
        }

        // Write to a temp file, then atomically rename
        let tmp_path = entry.source.with_extension("rollback_tmp");
        std::fs::copy(&restore_from, &tmp_path).with_context(|| {
            format!(
                "rollback: copy {} → {}",
                restore_from.display(),
                tmp_path.display()
            )
        })?;
        std::fs::rename(&tmp_path, &entry.source).with_context(|| {
            format!("rollback: rename {} → {}", tmp_path.display(), entry.source.display())
        })?;

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Serde for Blake3Hash (needed so PathEntry can be serialized)
// ─────────────────────────────────────────────────────────────────────────────

// Blake3Hash already derives nothing by default — we implement Serialize/Deserialize
// manually via the hex string representation.
impl Serialize for Blake3Hash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_hex())
    }
}

impl<'de> Deserialize<'de> for Blake3Hash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Blake3Hash::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    fn make_manager(tmp: &TempDir) -> (Database, PathBuf) {
        let db_file = tmp.path().join("test.db");
        let db = Database::open(&db_file).unwrap();
        (db, tmp.path().to_path_buf())
    }

    fn empty_plan(op: OpType) -> TxPlan {
        TxPlan { op_type: op, affected_paths: vec![], rollback_plan: RollbackPlan::default() }
    }

    #[test]
    fn test_begin_commit() {
        let tmp = TempDir::new().unwrap();
        let (mut db, store) = make_manager(&tmp);
        let mut tm = TransactionManager::new(&mut db, &store);

        let handle = tm.begin(OpType::Gc, empty_plan(OpType::Gc)).unwrap();
        let tx_id = handle.tx_id.clone();
        tm.commit(handle).unwrap();

        let records = tm.list(TxFilter::default()).unwrap();
        let rec = records.iter().find(|r| r.tx_id == tx_id).unwrap();
        assert_eq!(rec.status, TxStatus::Committed);
        assert!(rec.end_time.is_some());
    }

    #[test]
    fn test_begin_fail() {
        let tmp = TempDir::new().unwrap();
        let (mut db, store) = make_manager(&tmp);
        let mut tm = TransactionManager::new(&mut db, &store);

        let handle = tm.begin(OpType::Dedup, empty_plan(OpType::Dedup)).unwrap();
        let tx_id = handle.tx_id.clone();
        tm.fail(handle, "disk full").unwrap();

        let records = tm.list(TxFilter::default()).unwrap();
        let rec = records.iter().find(|r| r.tx_id == tx_id).unwrap();
        assert_eq!(rec.status, TxStatus::Failed);
        assert_eq!(rec.error_message.as_deref(), Some("disk full"));
    }

    #[test]
    fn test_rollback_idempotent() {
        let tmp = TempDir::new().unwrap();
        let (mut db, store) = make_manager(&tmp);
        let mut tm = TransactionManager::new(&mut db, &store);

        // Create a real file to roll back
        let file_path = tmp.path().join("model.bin");
        std::fs::write(&file_path, b"original content").unwrap();

        // Create a fake CAS target
        let cas_dir = tmp.path().join("cas").join("blake3").join("ab");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let cas_path = cas_dir.join("a".repeat(64));
        std::fs::write(&cas_path, b"original content").unwrap();

        let plan = TxPlan {
            op_type: OpType::Dedup,
            affected_paths: vec![],
            rollback_plan: RollbackPlan {
                entries: vec![PathEntry {
                    source: file_path.clone(),
                    target: Some(cas_path.clone()),
                    original_hash: None,
                    new_hash: None,
                    size: 16,
                }],
            },
        };

        let handle = tm.begin(OpType::Dedup, plan).unwrap();
        let tx_id = handle.tx_id.clone();
        tm.commit(handle).unwrap();

        // First rollback
        tm.rollback(&tx_id).unwrap();
        // Second rollback — must be a no-op (no error)
        tm.rollback(&tx_id).unwrap();

        let records = tm.list(TxFilter::default()).unwrap();
        let rec = records.iter().find(|r| r.tx_id == tx_id).unwrap();
        assert_eq!(rec.status, TxStatus::RolledBack);
    }

    #[test]
    fn test_recover_cleans_staging() {
        let tmp = TempDir::new().unwrap();
        let (mut db, store) = make_manager(&tmp);
        let mut tm = TransactionManager::new(&mut db, &store);

        let handle = tm.begin(OpType::Dedup, empty_plan(OpType::Dedup)).unwrap();
        let tx_id = handle.tx_id.clone();

        // Simulate staging dir left by a crash
        let staging = tmp
            .path()
            .join("tmp")
            .join("cas_staging")
            .join(&tx_id);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("object.part"), b"partial").unwrap();

        // Don't call commit() — simulate a crash, then recover
        drop(tm);

        let mut tm2 = TransactionManager::new(&mut db, &store);
        let results = tm2.recover().unwrap();

        // Staging should be gone
        assert!(!staging.exists());
        assert!(!results.is_empty());
    }

    #[test]
    fn test_cleanup_older_than() {
        let tmp = TempDir::new().unwrap();
        let (mut db, store) = make_manager(&tmp);
        let mut tm = TransactionManager::new(&mut db, &store);

        let handle = tm.begin(OpType::Gc, empty_plan(OpType::Gc)).unwrap();
        tm.commit(handle).unwrap();

        // 0-duration threshold keeps everything
        let removed = tm.cleanup_older_than(Duration::from_secs(0)).unwrap();
        assert!(removed >= 1);
    }

    #[test]
    fn test_list_filter_by_op_type() {
        let tmp = TempDir::new().unwrap();
        let (mut db, store) = make_manager(&tmp);
        let mut tm = TransactionManager::new(&mut db, &store);

        let h1 = tm.begin(OpType::Gc, empty_plan(OpType::Gc)).unwrap();
        let h2 = tm.begin(OpType::Dedup, empty_plan(OpType::Dedup)).unwrap();
        tm.commit(h1).unwrap();
        tm.commit(h2).unwrap();

        let gc_only =
            tm.list(TxFilter { op_type: Some(OpType::Gc), status: None }).unwrap();
        assert!(gc_only.iter().all(|r| r.op_type == Some(OpType::Gc)));

        let dedup_only =
            tm.list(TxFilter { op_type: Some(OpType::Dedup), status: None }).unwrap();
        assert!(dedup_only.iter().all(|r| r.op_type == Some(OpType::Dedup)));
    }
}
