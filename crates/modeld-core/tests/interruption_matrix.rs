//! Interruption matrix (ROADMAP B8: "故障注入覆盖 5 个中断点").
//!
//! The dedup/ingest write protocol touches disk in five distinct places. A
//! crash at each one must leave a store that `verify` calls clean, a readable
//! library, and no transaction stuck in `pending` — that is the store-level
//! contract G1 ("任意中断点均不丢失用户路径") reduces to.
//!
//! | # | Interruption point   | Disk + WAL state at the crash                 | Recovery must      |
//! |---|----------------------|-----------------------------------------------|--------------------|
//! | 1 | after `begin`        | WAL `pending`, nothing touched                | roll back          |
//! | 2 | after staging copy   | WAL `pending`, `staging/<hash>.tmp` present   | drop the staging   |
//! | 3 | after CAS publish    | CAS object on disk, WAL still `pending`       | roll back, keep CAS |
//! | 4 | after a partial link | WAL `copied`, one of two paths re-linked      | roll forward       |
//! | 5 | before `commit`      | WAL `copied`, every path already linked       | commit, drop row   |
//!
//! Points 1–3 are the "abandon it" side of the protocol, 4–5 the "finish it"
//! side. Both sides are checked for the same invariants, because the failure
//! mode that matters is not "recovery picked the wrong branch" but "the store
//! is inconsistent afterwards".
//!
//! The per-point unit tests in `recover.rs` already cover the branches; this
//! file exists so the *whole* matrix is one auditable thing that runs against
//! the public API, and so a regression in any single point fails a test named
//! after that point.

use modeld_core::db::{AliasType, Frontend};
use modeld_core::links::LinkCapability;
use modeld_core::{
    hash_file, recover_store, run_verify, Blake3Hash, CasStore, Database, RecoveryReport,
    TransactionStatus, TxAction,
};
use std::path::PathBuf;
use tempfile::TempDir;

const PAYLOAD: &[u8] = &[0x5A; 256 * 1024];

// ─────────────────────────────────────────────────────────────────────────────
// Fixture — a store where a dedup of `b` onto the content of `a` is in flight
// ─────────────────────────────────────────────────────────────────────────────

struct Fixture {
    /// Kept alive for the whole test so `TempDir::drop` does not delete the
    /// store while a path outcome is still being inspected (hence no reads).
    #[allow(dead_code)]
    dir: TempDir,
    db: Database,
    store: PathBuf,
    hash: Blake3Hash,
    /// Canonical path recorded as the WAL `source_path`.
    a: PathBuf,
    /// The duplicate the dedup was about to replace with a link.
    b: PathBuf,
    caps: LinkCapability,
}

impl Fixture {
    /// Build the "one step before the crash" state that every point starts from:
    /// a CAS object for the content, a model row, `a` as the original alias, and
    /// `b` as an independent duplicate still holding its own bytes.
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let store = dir.path().join("store");
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&store).unwrap();

        let a = lib.join("a.safetensors");
        let b = lib.join("b.safetensors");
        std::fs::write(&a, PAYLOAD).unwrap();
        std::fs::write(&b, PAYLOAD).unwrap();

        let hash = hash_file(&a).unwrap();
        let mut db = Database::open(&store.join("modeld.db")).unwrap();

        // Publish the CAS object the dedup would link both paths to. The source
        // file is *not* replaced — that is the dedup's job, and it has not run.
        let cas = CasStore::new(&store);
        cas.init().unwrap();
        cas.store_crash_safe(&a, &hash, "fixture-setup").unwrap();

        db.insert_or_update_model(
            &hash,
            PAYLOAD.len() as i64,
            Some("safetensors"),
            None,
            None,
            None,
        )
        .unwrap();
        db.insert_alias(&hash, &a.to_string_lossy(), Frontend::User, AliasType::Original).unwrap();

        Self { dir, db, store, hash, a, b, caps: LinkCapability::detect() }
    }

    fn staging_dir(&self) -> PathBuf {
        self.store.join("tmp").join("cas_staging")
    }

    fn staging_file(&self) -> PathBuf {
        self.staging_dir().join(format!("{}.tmp", self.hash.as_hex()))
    }

    fn cas_path(&self) -> PathBuf {
        self.store.join("cas").join("blake3").join(self.hash.prefix()).join(self.hash.as_hex())
    }

    /// Plant the WAL row the dedup writes before touching any bytes.
    fn begin_wal(&mut self, tx_id: &str, status: TransactionStatus) {
        let dup_metadata =
            serde_json::to_string(&vec![self.b.to_string_lossy().to_string()]).unwrap();
        self.db
            .insert_wal_transaction(
                tx_id,
                "dedup",
                status,
                Some(&self.a.to_string_lossy()),
                Some(self.hash.as_hex()),
                Some(&dup_metadata),
            )
            .unwrap();
    }

    fn recover(&mut self) -> Vec<RecoveryReport> {
        recover_store(&mut self.db, &self.store, &self.caps).unwrap()
    }
}

/// Invariants that must hold for **every** interruption point.
fn assert_store_is_usable(f: &Fixture, point: &str) {
    // 1. The library is intact: both paths exist and read back byte-for-byte.
    for path in [&f.a, &f.b] {
        let bytes = std::fs::read(path).unwrap_or_else(|e| {
            panic!("[{point}] {} is unreadable after recovery: {e}", path.display())
        });
        assert_eq!(bytes.len(), PAYLOAD.len(), "[{point}] {} changed size", path.display());
        assert!(bytes.iter().all(|b| *b == 0x5A), "[{point}] {} content changed", path.display());
    }

    // 2. No transaction is left in flight.
    let incomplete = f.db.get_incomplete_wal_transactions().unwrap();
    assert!(
        incomplete.is_empty(),
        "[{point}] recovery left transactions in flight: {:?}",
        incomplete.iter().map(|t| (&t.tx_id, t.status.clone())).collect::<Vec<_>>()
    );

    // 3. `verify` agrees: no dangling alias, no missing object, no staging litter.
    let report = run_verify(&f.store, &f.db, false).unwrap();
    assert!(
        report.is_clean(),
        "[{point}] verify found {} issue(s) after recovery: {report:?}",
        report.total_issues(),
    );
}

fn only_report(reports: Vec<RecoveryReport>, point: &str) -> RecoveryReport {
    assert_eq!(
        reports.len(),
        1,
        "[{point}] expected exactly one recovered transaction: {reports:?}"
    );
    reports.into_iter().next().unwrap()
}

// ─────────────────────────────────────────────────────────────────────────────
// Point 1 — crash right after `begin`
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn interruption_1_after_begin() {
    let mut f = Fixture::new();
    f.begin_wal("tx-point-1", TransactionStatus::Pending);

    let report = only_report(f.recover(), "point 1");

    assert_eq!(report.action, TxAction::RolledBack, "nothing was written, so nothing to finish");
    // The duplicate is untouched: no space was reclaimed, and none was lost.
    assert!(!f.staging_file().exists(), "no staging file should exist");
    assert_store_is_usable(&f, "point 1");
}

// ─────────────────────────────────────────────────────────────────────────────
// Point 2 — crash after the staging copy, before the CAS rename
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn interruption_2_after_staging_copy() {
    let mut f = Fixture::new();
    f.begin_wal("tx-point-2", TransactionStatus::Pending);

    // Phase A copies the duplicate into staging. Crash before the rename.
    std::fs::create_dir_all(f.staging_dir()).unwrap();
    std::fs::write(f.staging_file(), PAYLOAD).unwrap();

    let report = only_report(f.recover(), "point 2");

    assert_eq!(report.action, TxAction::RolledBack);
    assert!(!f.staging_file().exists(), "the staging file must be removed, not left to rot");
    assert_store_is_usable(&f, "point 2");
}

// ─────────────────────────────────────────────────────────────────────────────
// Point 3 — crash after the CAS rename, before the WAL is updated
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn interruption_3_after_cas_publish() {
    let mut f = Fixture::new();
    // The CAS object is already on disk (fixture setup), the WAL still says
    // `pending`: this is the exact window between `rename` and the WAL update.
    f.begin_wal("tx-point-3", TransactionStatus::Pending);
    assert!(f.cas_path().exists(), "precondition: the object was published");

    let report = only_report(f.recover(), "point 3");

    // Recovery cannot know the operation was half-done, so it abandons it — but
    // it must not delete the published object, which the model row references.
    assert_eq!(report.action, TxAction::RolledBack);
    assert!(f.cas_path().exists(), "a published CAS object must never be deleted by rollback");
    assert_store_is_usable(&f, "point 3");

    // The rollback is real and consistent: the accounting still shows one alias
    // and nothing claimed as saved. `b` was never written to the alias table —
    // the dedup died before that — so it is an unindexed duplicate on disk and
    // `storage_stats` correctly does not count it as waste. It is still there,
    // at full size, for the next scan to pick up (checked above).
    let stats = f.db.storage_stats().unwrap();
    assert_eq!(stats.alias_count, 1, "rollback must not invent aliases: {stats:?}");
    assert_eq!(stats.deduped_saved_bytes, 0, "nothing was deduplicated: {stats:?}");
    assert_eq!(stats.logical_bytes, PAYLOAD.len() as u64, "{stats:?}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Point 4 — crash after one of two paths was re-linked
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn interruption_4_after_a_partial_link() {
    let mut f = Fixture::new();
    f.begin_wal("tx-point-4", TransactionStatus::Copied);

    // `a` made it to a link; `b` is still an independent copy. The WAL carries
    // both paths, so recovery replays the whole set rather than guessing.
    let linked = modeld_core::links::create_link(&f.a, &f.cas_path(), &f.caps);
    assert!(
        matches!(linked, modeld_core::links::LinkResult::Success(_)),
        "precondition: re-linking the canonical path should succeed, got a failure"
    );

    let report = only_report(f.recover(), "point 4");

    assert_eq!(report.action, TxAction::RolledForward, "half-applied work must be finished");
    assert!(report.paths.iter().any(|p| p.path == f.b), "recovery should report the duplicate");
    assert_store_is_usable(&f, "point 4");

    // The point of finishing it: the duplicate stopped being a duplicate.
    // Proven at the accounting level rather than by inode inspection —
    // `number_of_links()` is still unstable on Windows, and these figures are
    // what `modeld stats` shows the user anyway.
    let stats = f.db.storage_stats().unwrap();
    assert_eq!(stats.logical_bytes, 2 * PAYLOAD.len() as u64, "two paths: {stats:?}");
    assert_eq!(stats.physical_bytes, PAYLOAD.len() as u64, "one object: {stats:?}");
    assert_eq!(stats.duplicate_waste_bytes, 0, "nothing left to reclaim: {stats:?}");
    assert_eq!(stats.deduped_saved_bytes, PAYLOAD.len() as u64, "{stats:?}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Point 5 — crash right before `commit`
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn interruption_5_before_commit() {
    let mut f = Fixture::new();
    f.begin_wal("tx-point-5", TransactionStatus::Copied);

    // Everything already applied, only the WAL row was left behind.
    for path in [&f.a, &f.b] {
        let result = modeld_core::links::create_link(path, &f.cas_path(), &f.caps);
        assert!(matches!(result, modeld_core::links::LinkResult::Success(_)));
    }

    let report = only_report(f.recover(), "point 5");

    assert_eq!(report.action, TxAction::RolledForward);
    assert_store_is_usable(&f, "point 5");

    // Replaying a path that is already a link must not materialise a copy.
    let stats = f.db.storage_stats().unwrap();
    assert_eq!(stats.logical_bytes, 2 * PAYLOAD.len() as u64, "{stats:?}");
    assert_eq!(stats.physical_bytes, PAYLOAD.len() as u64, "{stats:?}");
    assert_eq!(stats.duplicate_waste_bytes, 0, "replay duplicated the data: {stats:?}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────
