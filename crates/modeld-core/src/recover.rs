//! Crash recovery for the write-ahead log.
//!
//! One implementation, used by both `DedupEngine` (which calls it at the start
//! of a run) and `modeld tx recover`.  Before this module existed the two had
//! drifted: the engine rolled `copied` transactions *forward*, while
//! `TransactionManager::recover` marked them *failed* — the same crash produced
//! two different stores depending on which command you ran afterwards.
//!
//! What each state means:
//!
//! | WAL state   | Disk state after the crash                    | Recovery |
//! |-------------|-----------------------------------------------|----------|
//! | `pending`   | staging file may exist, CAS untouched         | roll back: delete staging, mark failed |
//! | `copied`    | CAS object committed, alias links may be partial | roll forward: re-link from WAL metadata |
//! | `committed` | done                                          | drop the WAL row |
//!
//! Every path the recovery touched is reported, so `modeld tx recover` can print
//! what happened to each one instead of a bare count.

use crate::db::{AliasType, Database, Frontend, TransactionStatus};
use crate::hash::Blake3Hash;
use crate::links::{create_link, LinkCapability, LinkResult};
use anyhow::Result;
use chrono::Utc;
use std::path::{Path, PathBuf};

/// What recovery did to a single path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathAction {
    /// The path was re-pointed at the CAS object (the transaction was
    /// half-done and the remaining links were replayed).
    RolledForward,
    /// The path was left as it was; the transaction was abandoned.
    RolledBack,
    /// The path no longer exists, or the transaction could not be completed.
    Skipped,
}

impl PathAction {
    /// Stable identifier used by the CLI and by `--json`.
    pub fn as_str(&self) -> &'static str {
        match self {
            PathAction::RolledForward => "rolled-forward",
            PathAction::RolledBack => "rolled-back",
            PathAction::Skipped => "skipped",
        }
    }
}

/// Disposition of one path touched by recovery.
#[derive(Debug, Clone)]
pub struct PathOutcome {
    /// The path in question.
    pub path: PathBuf,
    /// What happened to it.
    pub action: PathAction,
    /// Human-readable detail (why it was skipped, what it became, …).
    pub note: String,
}

/// Overall disposition of one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxAction {
    /// The transaction was completed.
    RolledForward,
    /// The transaction was abandoned and cleaned up.
    RolledBack,
    /// Nothing could be done; the record was dropped.
    Skipped,
}

impl TxAction {
    /// Stable identifier used by the CLI and by `--json`.
    pub fn as_str(&self) -> &'static str {
        match self {
            TxAction::RolledForward => "rolled-forward",
            TxAction::RolledBack => "rolled-back",
            TxAction::Skipped => "skipped",
        }
    }
}

/// Recovery report for one transaction.
#[derive(Debug, Clone)]
pub struct RecoveryReport {
    /// Transaction id from the WAL.
    pub tx_id: String,
    /// Operation the transaction was performing (`dedup`, …).
    pub op: String,
    /// What recovery decided.
    pub action: TxAction,
    /// Per-path dispositions.
    pub paths: Vec<PathOutcome>,
}

impl RecoveryReport {
    fn new(tx_id: &str, op: &str, action: TxAction) -> Self {
        Self { tx_id: tx_id.to_string(), op: op.to_string(), action, paths: Vec::new() }
    }

    fn record(&mut self, path: impl Into<PathBuf>, action: PathAction, note: impl Into<String>) {
        self.paths.push(PathOutcome { path: path.into(), action, note: note.into() });
    }
}

/// CAS path for a hash, without constructing a `CasStore`.
pub fn cas_path_for(store_path: &Path, hash: &Blake3Hash) -> PathBuf {
    store_path.join("cas").join("blake3").join(hash.prefix()).join(hash.as_hex())
}

/// Replay every incomplete transaction in the store.
///
/// Returns one report per transaction, oldest first.  Errors from individual
/// paths are recorded as `Skipped` rather than aborting the whole recovery —
/// one unreadable file must not strand the rest of the store.
pub fn recover_store(
    db: &mut Database,
    store_path: &Path,
    caps: &LinkCapability,
) -> Result<Vec<RecoveryReport>> {
    let incomplete = db.get_incomplete_wal_transactions()?;
    let staging_dir = store_path.join("tmp").join("cas_staging");
    let mut reports = Vec::with_capacity(incomplete.len());

    for tx in incomplete {
        let report = match tx.status {
            TransactionStatus::Pending => {
                // An interrupted ingest is worth finishing: the CAS object may
                // already be published, and without the DB rows nothing would
                // ever reference it again.
                if tx.operation == "ingest" || tx.operation == "download" {
                    if let Some(report) = roll_forward_file_import(db, store_path, &tx) {
                        reports.push(report);
                        continue;
                    }
                }

                let mut report =
                    RecoveryReport::new(&tx.tx_id, &tx.operation, TxAction::RolledBack);

                // Per-transaction staging directory (the `TransactionManager` layout).
                let tx_staging = staging_dir.join(&tx.tx_id);
                if tx_staging.exists() {
                    match std::fs::remove_dir_all(&tx_staging) {
                        Ok(()) => report.record(
                            &tx_staging,
                            PathAction::RolledBack,
                            "staging directory removed; the CAS was never written",
                        ),
                        Err(e) => report.record(
                            &tx_staging,
                            PathAction::Skipped,
                            format!("could not remove staging directory: {e}"),
                        ),
                    }
                }

                if let Some(ref hash) = tx.target_hash {
                    let staging = staging_dir.join(format!("{hash}.tmp"));
                    if staging.exists() {
                        match std::fs::remove_file(&staging) {
                            Ok(()) => report.record(
                                &staging,
                                PathAction::RolledBack,
                                "staging file removed; the CAS was never written",
                            ),
                            Err(e) => report.record(
                                &staging,
                                PathAction::Skipped,
                                format!("could not remove staging file: {e}"),
                            ),
                        }
                    }
                }
                db.update_wal_status(&tx.tx_id, TransactionStatus::Failed)?;
                db.update_wal_extended(
                    &tx.tx_id,
                    None,
                    None,
                    None,
                    Some(&Utc::now().to_rfc3339()),
                    Some("Recovered from crash: process was interrupted"),
                )?;
                report
            }

            TransactionStatus::Copied => {
                let mut report =
                    RecoveryReport::new(&tx.tx_id, &tx.operation, TxAction::RolledForward);
                let Some(hash) =
                    tx.target_hash.as_deref().and_then(|h| Blake3Hash::from_hex(h).ok())
                else {
                    report.action = TxAction::Skipped;
                    report.record(
                        store_path,
                        PathAction::Skipped,
                        "transaction has no usable target hash",
                    );
                    db.update_wal_status(&tx.tx_id, TransactionStatus::Failed)?;
                    db.update_wal_extended(
                        &tx.tx_id,
                        None,
                        None,
                        None,
                        Some(&Utc::now().to_rfc3339()),
                        Some("Recovered from crash: process was interrupted"),
                    )?;
                    reports.push(report);
                    continue;
                };

                let cas_path = cas_path_for(store_path, &hash);
                let staging = staging_dir.join(format!("{}.tmp", hash.as_hex()));

                // Phase A may have been interrupted between the rename and the
                // WAL update: finish it if the staging file is still there.
                if staging.exists() && !cas_path.exists() {
                    if let Some(parent) = cas_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if std::fs::rename(&staging, &cas_path).is_err() {
                        let _ = std::fs::copy(&staging, &cas_path);
                        let _ = std::fs::remove_file(&staging);
                    }
                    if let Ok(meta) = std::fs::metadata(&cas_path) {
                        let mut perms = meta.permissions();
                        perms.set_readonly(true);
                        let _ = std::fs::set_permissions(&cas_path, perms);
                    }
                    report.record(&cas_path, PathAction::RolledForward, "CAS object published");
                } else if staging.exists() {
                    let _ = std::fs::remove_file(&staging);
                    report.record(
                        &staging,
                        PathAction::RolledBack,
                        "duplicate staging file removed",
                    );
                }

                if !cas_path.exists() {
                    report.action = TxAction::Skipped;
                    report.record(
                        &cas_path,
                        PathAction::Skipped,
                        "CAS object is missing; cannot complete the transaction",
                    );
                    db.update_wal_status(&tx.tx_id, TransactionStatus::Failed)?;
                    db.update_wal_extended(
                        &tx.tx_id,
                        None,
                        None,
                        None,
                        Some(&Utc::now().to_rfc3339()),
                        Some("Recovered from crash: process was interrupted"),
                    )?;
                    reports.push(report);
                    continue;
                }

                // The canonical path: link it to the CAS object if it is still
                // a separate copy.
                if let Some(ref source) = tx.source_path {
                    let src = PathBuf::from(source);
                    if src.exists() && src != cas_path {
                        let atype = match create_link(&src, &cas_path, caps) {
                            LinkResult::Success(t) => {
                                report.record(
                                    &src,
                                    PathAction::RolledForward,
                                    "canonical path linked",
                                );
                                t
                            }
                            LinkResult::Failed(e) => {
                                report.record(
                                    &src,
                                    PathAction::Skipped,
                                    format!("canonical path left untouched: {e}"),
                                );
                                AliasType::Original
                            }
                        };
                        let _ = db.delete_alias(source);
                        let _ = db.insert_alias(&hash, source, Frontend::User, atype);
                    } else if src.exists() {
                        let _ = db.insert_alias(&hash, source, Frontend::User, AliasType::Original);
                    }
                }

                // Every duplicate recorded in the WAL metadata.
                let dup_paths: Vec<PathBuf> = tx
                    .metadata
                    .as_deref()
                    .and_then(|m| serde_json::from_str::<Vec<String>>(m).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();

                for dup_path in dup_paths {
                    if !dup_path.exists() {
                        report.record(&dup_path, PathAction::Skipped, "path no longer exists");
                        continue;
                    }
                    let dup_str = dup_path.to_string_lossy().to_string();
                    let atype = match create_link(&dup_path, &cas_path, caps) {
                        LinkResult::Success(t) => {
                            report.record(&dup_path, PathAction::RolledForward, "duplicate linked");
                            t
                        }
                        LinkResult::Failed(e) => {
                            report.record(
                                &dup_path,
                                PathAction::Skipped,
                                format!("left as a separate copy: {e}"),
                            );
                            AliasType::ReferenceOnly
                        }
                    };
                    let _ = db.delete_alias(&dup_str);
                    let _ = db.insert_alias(&hash, &dup_str, Frontend::User, atype);
                }

                db.update_wal_status(&tx.tx_id, TransactionStatus::Committed)?;
                db.delete_wal_transaction(&tx.tx_id)?;
                report
            }

            TransactionStatus::Committed
            | TransactionStatus::Failed
            | TransactionStatus::RolledBack => {
                let mut report = RecoveryReport::new(&tx.tx_id, &tx.operation, TxAction::Skipped);
                report.record(store_path, PathAction::Skipped, "stale WAL record removed");
                db.delete_wal_transaction(&tx.tx_id)?;
                report
            }
        };

        reports.push(report);
    }

    Ok(reports)
}

/// Finish an interrupted `ingest` if the CAS object made it to disk.
///
/// Returns `None` when the transaction cannot be completed (no usable hash, no
/// object, or the source vanished); the caller then rolls it back.
fn roll_forward_file_import(
    db: &mut Database,
    store_path: &Path,
    tx: &crate::db::WalTransaction,
) -> Option<RecoveryReport> {
    let hash = tx.target_hash.as_deref().and_then(|h| Blake3Hash::from_hex(h).ok())?;
    let source = tx.source_path.as_deref()?;
    let cas_path = cas_path_for(store_path, &hash);
    if !cas_path.exists() || !Path::new(source).exists() {
        return None;
    }

    let size = std::fs::metadata(&cas_path).map(|m| m.len() as i64).unwrap_or(0);
    db.insert_or_update_model(&hash, size, None, None, None, None).ok()?;
    // A downloaded file is referenced through the HF cache (symlink); an
    // ingested one through its own path.
    let (frontend, alias_type) = if tx.operation == "download" {
        (Frontend::HfCache, AliasType::Symlink)
    } else {
        let alias_type = tx
            .metadata
            .as_deref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
            .and_then(|v| v.get("mode").and_then(|m| m.as_str()).map(str::to_string))
            .map(|mode| if mode == "adopt" { AliasType::Hardlink } else { AliasType::Original })
            .unwrap_or(AliasType::Original);
        (Frontend::User, alias_type)
    };
    if db.get_alias_by_path(source).ok().flatten().is_none() {
        let _ = db.insert_alias(&hash, source, frontend, alias_type);
    }

    let mut report = RecoveryReport::new(&tx.tx_id, &tx.operation, TxAction::RolledForward);
    report.record(&cas_path, PathAction::RolledForward, "CAS object was already published");
    report.record(source, PathAction::RolledForward, "model and alias rows restored");
    let _ = db.update_wal_status(&tx.tx_id, TransactionStatus::Committed);
    let _ = db.delete_wal_transaction(&tx.tx_id);
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TransactionStatus;

    fn hash_of(bytes: &[u8]) -> Blake3Hash {
        Blake3Hash::from_hex(blake3::hash(bytes).to_hex().as_str()).unwrap()
    }

    fn store(tmp: &tempfile::TempDir) -> (Database, PathBuf) {
        let db = Database::open(&tmp.path().join("modeld.db")).unwrap();
        (db, tmp.path().to_path_buf())
    }

    #[test]
    fn pending_transaction_is_rolled_back_and_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, store_path) = store(&tmp);
        let staging = store_path.join("tmp").join("cas_staging");
        std::fs::create_dir_all(&staging).unwrap();
        let hash = hash_of(b"recover-pending");
        std::fs::write(staging.join(format!("{}.tmp", hash.as_hex())), b"partial").unwrap();
        db.insert_wal_transaction(
            "tx-pending",
            "dedup",
            TransactionStatus::Pending,
            None,
            Some(hash.as_hex()),
            None,
        )
        .unwrap();

        let reports = recover_store(&mut db, &store_path, &LinkCapability::detect()).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].action, TxAction::RolledBack);
        assert_eq!(reports[0].op, "dedup");
        assert!(reports[0].paths.iter().any(|p| p.action == PathAction::RolledBack));
        assert!(!staging.join(format!("{}.tmp", hash.as_hex())).exists());
        assert!(db.get_incomplete_wal_transactions().unwrap().is_empty());
    }

    #[test]
    fn copied_transaction_is_rolled_forward() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, store_path) = store(&tmp);
        // The CAS object exists (Phase A finished) and one duplicate is still
        // a separate copy (Phase B was interrupted).
        let content = vec![7u8; 4096];
        let hash = hash_of(&content);
        let cas_path = cas_path_for(&store_path, &hash);
        std::fs::create_dir_all(cas_path.parent().unwrap()).unwrap();
        std::fs::write(&cas_path, &content).unwrap();

        let dup = store_path.join("dup.bin");
        std::fs::write(&dup, &content).unwrap();
        db.insert_wal_transaction(
            "tx-copied",
            "dedup",
            TransactionStatus::Copied,
            Some(dup.to_string_lossy().as_ref()),
            Some(hash.as_hex()),
            Some(&serde_json::to_string(&vec![dup.to_string_lossy().to_string()]).unwrap()),
        )
        .unwrap();

        let reports = recover_store(&mut db, &store_path, &LinkCapability::detect()).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].action, TxAction::RolledForward);
        assert!(reports[0].paths.iter().any(|p| p.action == PathAction::RolledForward));
        assert!(db.get_incomplete_wal_transactions().unwrap().is_empty());
    }

    #[test]
    fn interrupted_ingest_is_finished() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, store_path) = store(&tmp);
        let content = vec![9u8; 2048];
        let hash = hash_of(&content);
        let cas_path = cas_path_for(&store_path, &hash);
        std::fs::create_dir_all(cas_path.parent().unwrap()).unwrap();
        std::fs::write(&cas_path, &content).unwrap();
        let source = store_path.join("model.safetensors");
        std::fs::write(&source, &content).unwrap();

        db.insert_wal_transaction(
            "tx-ingest",
            "ingest",
            TransactionStatus::Pending,
            Some(source.to_string_lossy().as_ref()),
            Some(hash.as_hex()),
            Some("{\"mode\":\"copy\"}"),
        )
        .unwrap();

        let reports = recover_store(&mut db, &store_path, &LinkCapability::detect()).unwrap();
        assert_eq!(reports[0].action, TxAction::RolledForward);
        assert!(reports[0].paths.iter().any(|p| p.action == PathAction::RolledForward));
        // The DB rows the crashed ingest never wrote are now there.
        assert!(db.get_alias_by_path(source.to_string_lossy().as_ref()).unwrap().is_some());
        assert!(db.get_incomplete_wal_transactions().unwrap().is_empty());
    }

    #[test]
    fn interrupted_download_is_finished() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, store_path) = store(&tmp);
        let content = vec![3u8; 1024];
        let hash = hash_of(&content);
        let cas_path = cas_path_for(&store_path, &hash);
        std::fs::create_dir_all(cas_path.parent().unwrap()).unwrap();
        std::fs::write(&cas_path, &content).unwrap();
        // The HF snapshot path the download would have linked.
        let snapshot = store_path.join("hf").join("models--org--repo").join("blob");
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        std::fs::write(&snapshot, &content).unwrap();

        db.insert_wal_transaction(
            "tx-dl",
            "download",
            TransactionStatus::Pending,
            Some(snapshot.to_string_lossy().as_ref()),
            Some(hash.as_hex()),
            Some("{\"repo\":\"org/repo\",\"file\":\"m.bin\"}"),
        )
        .unwrap();

        let reports = recover_store(&mut db, &store_path, &LinkCapability::detect()).unwrap();
        assert_eq!(reports[0].action, TxAction::RolledForward);
        assert!(db.get_alias_by_path(snapshot.to_string_lossy().as_ref()).unwrap().is_some());
        assert!(db.get_incomplete_wal_transactions().unwrap().is_empty());
    }

    #[test]
    fn copied_transaction_without_a_cas_object_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut db, store_path) = store(&tmp);
        let hash = hash_of(b"never-written");
        db.insert_wal_transaction(
            "tx-orphan",
            "dedup",
            TransactionStatus::Copied,
            None,
            Some(hash.as_hex()),
            None,
        )
        .unwrap();

        let reports = recover_store(&mut db, &store_path, &LinkCapability::detect()).unwrap();
        assert_eq!(reports[0].action, TxAction::Skipped);
        assert!(reports[0].paths.iter().any(|p| p.action == PathAction::Skipped));
        assert!(db.get_incomplete_wal_transactions().unwrap().is_empty());
    }
}
