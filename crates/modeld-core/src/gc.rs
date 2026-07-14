//! Safe Garbage Collection
//!
//! Protection hierarchy:
//!   1. Hard-protected  — `workflow_refs` exists → never GC
//!   2. Soft-protected  — aliases exist, no workflow refs → warn, skip
//!   3. Orphan          — no refs, no aliases → quarantine (30-day TTL)
//!   4. Final deletion  — quarantine TTL expired → permanent delete
//!
//! ## Fixes applied (audit)
//! - `GcEngine` now takes `&'a mut Database` so `run_safe` can write back.
//! - After a successful quarantine: sets `quarantined_at` on the model row
//!   and deletes all aliases so they no longer reference the missing file.

use crate::cas::CasStore;
use crate::db::{Database, Model};
use crate::hash::Blake3Hash;
use crate::quarantine::QuarantineManager;
use anyhow::Result;
use chrono::Utc;

// ─────────────────────────────────────────────────────────────────────────────
// RefStatus
// ─────────────────────────────────────────────────────────────────────────────

/// Classification of why a model has (or lacks) live references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefStatus {
    /// Referenced by at least one workflow / config file.
    HardReference,
    /// Has at least one alias but no workflow reference.
    SoftReference,
    /// User has explicitly pinned the model — never GC'd.
    Pinned,
    /// Last alias was accessed within the last 30 days.
    RecentlyUsed,
    /// Cannot determine status — conservative: do not GC.
    Unknown,
    /// No hard reference, no alias, not pinned — safe to quarantine.
    OrphanCandidate,
}

/// Classify the reference status of a single model.
///
/// Priority (highest wins):
/// 1. Pinned flag
/// 2. Workflow reference count > 0
/// 3. Alias count > 0
/// 4. Otherwise → `OrphanCandidate`
pub fn classify_refs(db: &Database, hash: &Blake3Hash) -> RefStatus {
    // Use the efficient batch query to get alias + workflow ref counts.
    // Fall back to Unknown when the model is not found.
    let rows = match db.get_gc_candidate_counts() {
        Ok(r) => r,
        Err(_) => return RefStatus::Unknown,
    };

    let row = rows.iter().find(|(m, _, _)| m.blake3_hash.as_hex() == hash.as_hex());
    let Some((model, alias_count, workflow_ref_count)) = row else {
        return RefStatus::Unknown;
    };

    if model.pinned {
        return RefStatus::Pinned;
    }
    if *workflow_ref_count > 0 {
        return RefStatus::HardReference;
    }
    if *alias_count > 0 {
        return RefStatus::SoftReference;
    }
    RefStatus::OrphanCandidate
}

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GcCandidate {
    pub model: Model,
    pub workflow_ref_count: usize,
    pub alias_count: usize,
    pub cas_file_exists: bool,
    pub savings_bytes: i64,
    /// Classification computed at scan time.
    pub ref_status: RefStatus,
}

impl GcCandidate {
    pub fn is_hard_protected(&self) -> bool {
        matches!(self.ref_status, RefStatus::HardReference)
    }
    pub fn is_soft_protected(&self) -> bool {
        matches!(self.ref_status, RefStatus::SoftReference)
    }
    pub fn is_orphan(&self) -> bool {
        matches!(self.ref_status, RefStatus::OrphanCandidate)
    }
    pub fn is_pinned(&self) -> bool {
        matches!(self.ref_status, RefStatus::Pinned)
    }
}

#[derive(Debug, Default)]
pub struct GcResult {
    pub quarantined: Vec<String>,
    pub skipped_protected: Vec<String>,
    pub skipped_soft: Vec<String>,
    pub bytes_recovered: i64,
    pub cleaned_quarantine: usize,
}

#[derive(Debug, Default)]
pub struct GcPreview {
    pub hard_protected: Vec<String>,
    pub soft_protected: Vec<GcPreviewItem>,
    pub would_quarantine: Vec<GcPreviewItem>,
    pub total_reclaimable_bytes: i64,
    pub expired_quarantine_count: usize,
    pub expired_quarantine_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct GcPreviewItem {
    pub hash_prefix: String,
    pub name: Option<String>,
    pub size_bytes: i64,
    pub alias_count: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// Engine
// ─────────────────────────────────────────────────────────────────────────────

/// GC engine.  Requires mutable DB access so that `run_safe` can write back
/// `quarantined_at` and clean up aliases.
pub struct GcEngine<'a> {
    db: &'a mut Database,
    cas: CasStore,
    quarantine: QuarantineManager,
}

impl<'a> GcEngine<'a> {
    pub fn new(db: &'a mut Database, store_path: &std::path::Path) -> Self {
        Self { db, cas: CasStore::new(store_path), quarantine: QuarantineManager::new(store_path) }
    }

    /// List all models with their protection status (single batch query).
    pub fn candidates(&self) -> Result<Vec<GcCandidate>> {
        let rows = self.db.get_gc_candidate_counts()?;
        let mut candidates = Vec::new();
        for (model, alias_count, workflow_ref_count) in rows {
            let cas_file_exists = self.cas.contains(&model.blake3_hash);

            // Compute ref_status inline to avoid a second DB round-trip.
            let ref_status = if model.pinned {
                RefStatus::Pinned
            } else if workflow_ref_count > 0 {
                RefStatus::HardReference
            } else if alias_count > 0 {
                RefStatus::SoftReference
            } else {
                RefStatus::OrphanCandidate
            };

            candidates.push(GcCandidate {
                savings_bytes: model.size_bytes,
                workflow_ref_count,
                alias_count,
                cas_file_exists,
                ref_status,
                model,
            });
        }
        Ok(candidates)
    }

    /// Preview what GC would do without making any changes.
    pub fn preview(&self) -> Result<GcPreview> {
        let candidates = self.candidates()?;
        let mut preview = GcPreview::default();

        for c in &candidates {
            let hp = &c.model.blake3_hash.as_hex()[..16];
            if c.is_hard_protected() {
                preview.hard_protected.push(hp.to_string());
            } else if c.is_soft_protected() {
                preview.soft_protected.push(GcPreviewItem {
                    hash_prefix: hp.to_string(),
                    name: c.model.format.clone(),
                    size_bytes: c.model.size_bytes,
                    alias_count: c.alias_count,
                });
            } else if c.is_orphan() {
                preview.would_quarantine.push(GcPreviewItem {
                    hash_prefix: hp.to_string(),
                    name: c.model.format.clone(),
                    size_bytes: c.model.size_bytes,
                    alias_count: 0,
                });
                preview.total_reclaimable_bytes += c.model.size_bytes;
            }
        }

        let entries = self.quarantine.list().unwrap_or_default();
        let expired: Vec<_> = entries.iter().filter(|e| e.days_remaining.is_none()).collect();
        preview.expired_quarantine_count = expired.len();
        preview.expired_quarantine_bytes = expired.iter().map(|e| e.meta.size_bytes as i64).sum();

        Ok(preview)
    }

    /// Quarantine all zero-ref models and write the result back to the DB.
    pub fn run_safe(&mut self) -> Result<GcResult> {
        let mut result = GcResult::default();
        let candidates = self.candidates()?;

        for c in &candidates {
            let hash_prefix = c.model.blake3_hash.as_hex()[..16].to_string();

            if c.is_hard_protected() {
                result.skipped_protected.push(hash_prefix);
                continue;
            }
            if c.is_soft_protected() {
                result.skipped_soft.push(hash_prefix);
                continue;
            }
            // Pinned or Unknown → skip conservatively (no GC)
            if c.is_pinned() || matches!(c.ref_status, RefStatus::Unknown | RefStatus::RecentlyUsed)
            {
                result.skipped_protected.push(hash_prefix);
                continue;
            }

            if c.is_orphan() && c.cas_file_exists {
                if let Some(cas_path) = self.cas.get(&c.model.blake3_hash) {
                    self.quarantine.init()?;
                    let hash_str = c.model.blake3_hash.as_hex().to_string();

                    match self.quarantine.quarantine(
                        &cas_path,
                        &hash_str,
                        "gc: no workflow refs, no aliases",
                        vec![],
                    ) {
                        Ok(_qpath) => {
                            result.quarantined.push(hash_prefix);
                            result.bytes_recovered += c.model.size_bytes;

                            // ── Write-back: mark quarantined in DB ──────────
                            if let Err(e) =
                                self.db.quarantine_model(&c.model.blake3_hash, Utc::now())
                            {
                                eprintln!(
                                    "{}",
                                    crate::i18n::tf(
                                        "warn.gc_db_quarantine_failed",
                                        &[
                                            ("hash", &&hash_str[..16]),
                                            ("error", &format!("{:#}", e)),
                                        ],
                                    )
                                );
                            }

                            // ── Delete dangling aliases ──────────────────────
                            if let Err(e) = self.db.delete_aliases_for_model(&c.model.blake3_hash) {
                                eprintln!(
                                    "{}",
                                    crate::i18n::tf(
                                        "warn.gc_alias_cleanup_failed",
                                        &[
                                            ("hash", &&hash_str[..16]),
                                            ("error", &format!("{:#}", e)),
                                        ],
                                    )
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "{}",
                                crate::i18n::tf(
                                    "warn.gc_quarantine_failed",
                                    &[("hash", &&hash_str[..16]), ("error", &format!("{:#}", e)),],
                                )
                            );
                        }
                    }
                }
            }
        }

        result.cleaned_quarantine = self.quarantine.cleanup_expired()?;
        Ok(result)
    }

    /// Clean expired quarantine entries.
    pub fn cleanup_quarantine(&self) -> Result<usize> {
        self.quarantine.cleanup_expired()
    }

    /// Remove stale temporary files left by interrupted downloads or dedup
    /// operations.
    ///
    /// - `{store}/tmp/downloads/*.part` older than `part_max_age_hours`
    /// - `{store}/tmp/cas_staging/*.tmp` older than `staging_max_age_hours`
    ///
    /// Returns the number of files removed.
    pub fn cleanup_tmp(
        store_path: &std::path::Path,
        part_max_age_hours: u64,
        staging_max_age_hours: u64,
    ) -> Result<usize> {
        let mut removed = 0usize;
        let now = std::time::SystemTime::now();

        let cleanup_dir = |dir: &std::path::Path,
                           ext: &str,
                           max_age_secs: u64|
         -> anyhow::Result<usize> {
            if !dir.exists() {
                return Ok(0);
            }
            let mut count = 0usize;
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let name =
                    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if !name.ends_with(ext) {
                    continue;
                }
                let age_secs = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|mtime| now.duration_since(mtime).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if age_secs >= max_age_secs && std::fs::remove_file(&path).is_ok() {
                    count += 1;
                }
            }
            Ok(count)
        };

        let downloads_dir = store_path.join("tmp").join("downloads");
        let staging_dir = store_path.join("tmp").join("cas_staging");

        removed += cleanup_dir(&downloads_dir, ".part", part_max_age_hours * 3600)?;
        removed += cleanup_dir(&staging_dir, ".tmp", staging_max_age_hours * 3600)?;

        Ok(removed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::hash::Blake3Hash;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_gc_candidate_flags() {
        let model = Model {
            id: 1,
            blake3_hash: Blake3Hash::from_hex(&"a".repeat(64)).unwrap(),
            size_bytes: 1000,
            format: None,
            arch: None,
            category: None,
            base_model: None,
            created_at: chrono::Utc::now(),
            last_seen: chrono::Utc::now(),
            quarantined_at: None,
            note: None,
            pinned: false,
            favorited: false,
            source_type: None,
            hf_repo_id: None,
            revision: None,
            download_url: None,
            license: None,
            downloaded_by: None,
            downloaded_at: None,
            original_filename: None,
            model_card_url: None,
        };

        let hard = GcCandidate {
            model: model.clone(),
            workflow_ref_count: 1,
            alias_count: 0,
            cas_file_exists: true,
            savings_bytes: 1000,
            ref_status: RefStatus::HardReference,
        };
        assert!(hard.is_hard_protected());
        assert!(!hard.is_soft_protected());
        assert!(!hard.is_orphan());

        let soft = GcCandidate {
            model: model.clone(),
            workflow_ref_count: 0,
            alias_count: 2,
            cas_file_exists: true,
            savings_bytes: 1000,
            ref_status: RefStatus::SoftReference,
        };
        assert!(soft.is_soft_protected());

        let orphan = GcCandidate {
            model: model.clone(),
            workflow_ref_count: 0,
            alias_count: 0,
            cas_file_exists: true,
            savings_bytes: 1000,
            ref_status: RefStatus::OrphanCandidate,
        };
        assert!(orphan.is_orphan());
    }

    #[test]
    fn test_gc_preview_empty_store() {
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();
        let gc = GcEngine::new(&mut db, tmp.path());
        let preview = gc.preview().unwrap();
        assert_eq!(preview.would_quarantine.len(), 0);
    }

    #[test]
    fn test_gc_sets_quarantined_at_and_clears_aliases() {
        use crate::db::{AliasType, Frontend};
        let tmp = TempDir::new().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();

        // Create a CAS file so the quarantine move succeeds
        let cas_dir = tmp.path().join("cas").join("blake3").join("aa");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let hash = Blake3Hash::from_hex(&"aa".repeat(32)).unwrap();
        let cas_file = cas_dir.join(hash.as_hex());
        std::fs::write(&cas_file, b"model data").unwrap();

        db.insert_or_update_model(&hash, 10, None, None, None, None).unwrap();
        // Alias exists initially
        db.insert_alias(&hash, "/some/path.safetensors", Frontend::User, AliasType::Original)
            .unwrap();

        // Remove alias so model becomes orphan
        db.delete_alias("/some/path.safetensors").unwrap();

        {
            let mut gc = GcEngine::new(&mut db, tmp.path());
            let result = gc.run_safe().unwrap();
            assert_eq!(result.quarantined.len(), 1);
        }

        // quarantined_at should now be set
        let model = db.get_model(&hash).unwrap().unwrap();
        assert!(model.quarantined_at.is_some(), "quarantined_at must be written back to DB");
    }

    // ─────────────────────────────────────────────────────────────────────
    // Property-based tests
    // ─────────────────────────────────────────────────────────────────────

    use crate::db::{AliasType, Frontend};
    use crate::refs::list_orphans;
    use proptest::prelude::*;

    /// One of a small number of reference states a synthetic model can be
    /// placed in for the purposes of this property test.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RefState {
        /// No aliases, no workflow refs, not pinned — a true orphan.
        Orphan,
        /// Has at least one alias, no workflow refs — soft-protected.
        Aliased,
        /// Pinned by the user — never GC'd regardless of refs.
        Pinned,
        /// Referenced by a workflow — hard-protected.
        WorkflowRef,
    }

    fn ref_state_strategy() -> impl Strategy<Value = RefState> {
        prop_oneof![
            Just(RefState::Orphan),
            Just(RefState::Aliased),
            Just(RefState::Pinned),
            Just(RefState::WorkflowRef),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]

        /// Property 5: GC preview quarantine set matches orphan detection.
        ///
        /// For any database state containing a mix of orphaned models (no
        /// aliases, no workflow refs, not pinned) and referenced models
        /// (aliased, pinned, or workflow-referenced), the `would_quarantine`
        /// list produced by `GcEngine::preview` must contain exactly the set
        /// of models that the independent orphan-detection code path
        /// (`refs::list_orphans`, which the WebUI/CLI `models`/`refs` views
        /// rely on for `is_orphan` classification) reports as orphans — and
        /// must exclude every referenced/pinned model. This guards against
        /// the two orphan-detection code paths (GC preview vs. models
        /// listing) silently diverging.
        ///
        /// **Validates: Requirements 2.5**
        #[test]
        fn prop_gc_preview_would_quarantine_matches_orphan_detection(
            states in prop::collection::vec(ref_state_strategy(), 0..12)
        ) {
            let tmp = TempDir::new().unwrap();
            let db_file = NamedTempFile::new().unwrap();
            let mut db = Database::open(db_file.path()).unwrap();

            let mut orphan_hashes: Vec<Blake3Hash> = Vec::new();

            for (i, state) in states.iter().enumerate() {
                // Distinct, valid 64-hex-char hash per model. The index is
                // encoded in the leading 16 hex chars (rather than the
                // trailing digits) so that `hash_prefix` (first 16 hex
                // chars, used by GcPreviewItem/preview grouping) is unique
                // per model — a trailing-digit encoding would collide on
                // the prefix for i in the same order of magnitude.
                let hash =
                    Blake3Hash::from_hex(&format!("{:016x}{:048x}", i + 1, 0u64)).unwrap();
                db.insert_or_update_model(&hash, 1000 + i as i64, None, None, None, None)
                    .unwrap();

                match state {
                    RefState::Orphan => {
                        orphan_hashes.push(hash.clone());
                    }
                    RefState::Aliased => {
                        db.insert_alias(
                            &hash,
                            &format!("/models/aliased_{i}.safetensors"),
                            Frontend::User,
                            AliasType::Original,
                        )
                        .unwrap();
                    }
                    RefState::Pinned => {
                        db.pin_model(&hash, true).unwrap();
                    }
                    RefState::WorkflowRef => {
                        let wf_id = db
                            .upsert_workflow(&format!("/workflows/wf_{i}.json"), None, None)
                            .unwrap();
                        db.insert_workflow_ref(
                            wf_id,
                            Some(hash.as_hex()),
                            "checkpoint",
                            &format!("model_{i}.safetensors"),
                            None,
                            true,
                        )
                        .unwrap();
                        db.update_workflow_ref_count(wf_id).unwrap();
                    }
                }
            }

            // Path A: GC preview's own orphan classification.
            let gc = GcEngine::new(&mut db, tmp.path());
            let preview = gc.preview().unwrap();
            let preview_prefixes: std::collections::HashSet<String> = preview
                .would_quarantine
                .iter()
                .map(|item| item.hash_prefix.clone())
                .collect();

            // Path B: the independent orphan-detection function used
            // elsewhere (e.g. by the `refs`/`models` views) for `is_orphan`.
            let orphans_from_refs = list_orphans(&db).unwrap();
            let refs_prefixes: std::collections::HashSet<String> = orphans_from_refs
                .iter()
                .map(|h| h.as_hex()[..16].to_string())
                .collect();

            // The two independent code paths must agree exactly.
            prop_assert_eq!(
                preview_prefixes.clone(),
                refs_prefixes.clone(),
                "GC preview would_quarantine set must match refs::list_orphans exactly"
            );

            // `would_quarantine` must contain exactly the synthetic orphan
            // models and no referenced/pinned model.
            let expected_prefixes: std::collections::HashSet<String> = orphan_hashes
                .iter()
                .map(|h| h.as_hex()[..16].to_string())
                .collect();
            prop_assert_eq!(preview_prefixes.len(), orphan_hashes.len());
            prop_assert_eq!(preview_prefixes, expected_prefixes);
        }
    }
}
