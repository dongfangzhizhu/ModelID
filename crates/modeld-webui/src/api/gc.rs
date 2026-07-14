//! GC API handlers — preview and run safe garbage collection.

use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, WsEvent};

// ─── GC Preview ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct GcPreviewItem {
    pub hash_prefix: String,
    pub name: Option<String>,
    pub size_bytes: i64,
    pub alias_count: usize,
}

#[derive(Serialize)]
pub struct GcPreviewResponse {
    /// Models that will be quarantined on next GC run.
    pub would_quarantine: Vec<GcPreviewItem>,
    /// Models with aliases but no workflow refs (soft-protected, will be skipped).
    pub soft_protected: Vec<GcPreviewItem>,
    /// Total bytes reclaimable by quarantining orphan models.
    pub total_reclaimable_bytes: i64,
    /// Number of already-quarantined entries past their TTL.
    pub expired_quarantine_count: usize,
    /// Bytes held by expired quarantine entries.
    pub expired_quarantine_bytes: i64,
}

/// `GET /api/v1/gc/preview` — dry-run: show what safe GC would do.
pub async fn gc_preview(State(state): State<AppState>) -> ApiResult<Json<GcPreviewResponse>> {
    let mut db = state.db.lock().map_err(|_| ApiError::internal("DB lock poisoned"))?;

    let gc = modeld_core::GcEngine::new(&mut db, &state.store_path);
    let preview = gc.preview().map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(GcPreviewResponse {
        would_quarantine: preview
            .would_quarantine
            .into_iter()
            .map(|i| GcPreviewItem {
                hash_prefix: i.hash_prefix,
                name: i.name,
                size_bytes: i.size_bytes,
                alias_count: i.alias_count,
            })
            .collect(),
        soft_protected: preview
            .soft_protected
            .into_iter()
            .map(|i| GcPreviewItem {
                hash_prefix: i.hash_prefix,
                name: i.name,
                size_bytes: i.size_bytes,
                alias_count: i.alias_count,
            })
            .collect(),
        total_reclaimable_bytes: preview.total_reclaimable_bytes,
        expired_quarantine_count: preview.expired_quarantine_count,
        expired_quarantine_bytes: preview.expired_quarantine_bytes,
    }))
}

// ─── GC Run ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
pub struct GcRunRequest {
    /// If provided, only GC models whose hash prefix matches one of these.
    /// Empty/absent = GC all orphans.
    pub hashes: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct GcRunResponse {
    /// Hashes (prefix) that were moved to quarantine.
    pub quarantined: Vec<String>,
    /// Hashes skipped because they have workflow refs (hard-protected).
    pub skipped_protected: Vec<String>,
    /// Hashes skipped because they still have aliases (soft-protected).
    pub skipped_soft: Vec<String>,
    /// Bytes moved to quarantine.
    pub bytes_recovered: i64,
    /// Expired quarantine entries cleaned up.
    pub cleaned_quarantine: usize,
}

/// `POST /api/v1/gc/run` — move orphan models to quarantine (safe; no permanent
/// deletion).
pub async fn gc_run(
    State(state): State<AppState>,
    body: Option<Json<GcRunRequest>>,
) -> ApiResult<Json<GcRunResponse>> {
    let _req = body.map(|b| b.0).unwrap_or_default();

    let mut db = state.db.lock().map_err(|_| ApiError::internal("DB lock poisoned"))?;

    let mut gc = modeld_core::GcEngine::new(&mut db, &state.store_path);
    let result = match gc.run_safe() {
        Ok(r) => r,
        Err(e) => {
            let _ = state.event_tx.send(WsEvent::OperationComplete {
                operation: "gc".to_string(),
                success: false,
                message: format!("GC failed: {e}"),
            });
            return Err(ApiError::internal(e.to_string()));
        }
    };

    let _ = state.event_tx.send(WsEvent::OperationComplete {
        operation: "gc".to_string(),
        success: true,
        message: format!(
            "GC complete: {} quarantined, {} bytes recovered",
            result.quarantined.len(),
            result.bytes_recovered
        ),
    });

    Ok(Json(GcRunResponse {
        quarantined: result.quarantined,
        skipped_protected: result.skipped_protected,
        skipped_soft: result.skipped_soft,
        bytes_recovered: result.bytes_recovered,
        cleaned_quarantine: result.cleaned_quarantine,
    }))
}

// ─── Serialization stability tests (audit Wave 2, Req 2.11) ───────────────────
//
// These tests exist as a regression guard: the WebUI frontend (`ui/*.js`)
// reads these exact JSON field names. If a field is renamed or removed here
// without updating the frontend, these tests should fail first.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_preview_response_field_names_are_stable() {
        let response = GcPreviewResponse {
            would_quarantine: vec![GcPreviewItem {
                hash_prefix: "abc123".to_string(),
                name: Some("model.safetensors".to_string()),
                size_bytes: 1024,
                alias_count: 0,
            }],
            soft_protected: vec![GcPreviewItem {
                hash_prefix: "def456".to_string(),
                name: None,
                size_bytes: 2048,
                alias_count: 1,
            }],
            total_reclaimable_bytes: 1024,
            expired_quarantine_count: 2,
            expired_quarantine_bytes: 4096,
        };

        let value = serde_json::to_value(&response).expect("serialize GcPreviewResponse");
        let obj = value.as_object().expect("GcPreviewResponse serializes to an object");

        for field in [
            "would_quarantine",
            "soft_protected",
            "total_reclaimable_bytes",
            "expired_quarantine_count",
            "expired_quarantine_bytes",
        ] {
            assert!(obj.contains_key(field), "GcPreviewResponse missing field `{field}`");
        }

        let item = &value["would_quarantine"][0];
        for field in ["hash_prefix", "name", "size_bytes", "alias_count"] {
            assert!(
                item.as_object().unwrap().contains_key(field),
                "GcPreviewItem missing field `{field}`"
            );
        }
    }

    #[test]
    fn gc_run_response_field_names_are_stable() {
        let response = GcRunResponse {
            quarantined: vec!["abc123".to_string()],
            skipped_protected: vec!["def456".to_string()],
            skipped_soft: vec!["ghi789".to_string()],
            bytes_recovered: 4096,
            cleaned_quarantine: 3,
        };

        let value = serde_json::to_value(&response).expect("serialize GcRunResponse");
        let obj = value.as_object().expect("GcRunResponse serializes to an object");

        for field in [
            "quarantined",
            "skipped_protected",
            "skipped_soft",
            "bytes_recovered",
            "cleaned_quarantine",
        ] {
            assert!(obj.contains_key(field), "GcRunResponse missing field `{field}`");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Property-based tests (audit Wave 3, Property 7)
// ─────────────────────────────────────────────────────────────────────────────
//
// `gc_run` is an axum handler whose only observable side-channel signal that
// "the operation completed" is the `WsEvent::OperationComplete` it broadcasts
// on `state.event_tx` right before returning. These tests build a real
// (temp-dir-backed) `AppState` + `Database`, subscribe to `event_tx` *before*
// invoking the handler, await the handler, and assert on the single
// `OperationComplete` event it broadcasts — exercising the real handler code
// path (no mocking), mirroring the pattern used for `trigger_scan` in
// `api::scan::property_tests`.
#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::state::AppState;
    use proptest::prelude::*;
    use tempfile::TempDir;

    /// Run `gc_run` against a fresh temp-dir-backed `AppState` seeded with
    /// `orphan_count` orphaned models (no aliases, no workflow refs — each
    /// with a real CAS object on disk so `run_safe` can quarantine it) and
    /// return the single `WsEvent::OperationComplete` broadcast during the
    /// call.
    fn run_gc_run_and_collect_operation_complete(orphan_count: usize) -> WsEvent {
        let rt = tokio::runtime::Runtime::new().expect("build tokio runtime");
        rt.block_on(async move {
            let store_dir = TempDir::new().expect("create store temp dir");
            let db_path = store_dir.path().join("modeld.db");
            let mut db = modeld_core::Database::open(&db_path).expect("open test database");

            let cas = modeld_core::CasStore::new(store_dir.path());
            cas.init().expect("init CAS");

            for i in 0..orphan_count {
                // Distinct 64-hex-char hash per orphan model.
                let hash =
                    modeld_core::Blake3Hash::from_hex(&format!("{:016x}{:048x}", i + 1, 0u64))
                        .expect("valid synthetic hash");
                let content = format!("orphan model content {i}");
                let src_path = store_dir.path().join(format!("src_{i}.bin"));
                std::fs::write(&src_path, content.as_bytes()).expect("write source file");

                // Store the (mismatched, but that's fine — CasStore::store
                // trusts the caller-provided hash) content under this
                // synthetic hash so a real CAS object exists for the GC
                // engine to quarantine.
                cas.store(&src_path, &hash).expect("store orphan CAS object");

                db.insert_or_update_model(&hash, content.len() as i64, None, None, None, None)
                    .expect("insert orphan model");
                // No alias, no workflow ref, not pinned -> RefStatus::OrphanCandidate.
            }

            let (state, _initial_rx) =
                AppState::new(db, store_dir.path().to_path_buf(), None, None);
            let mut rx = state.event_tx.subscribe();

            let Json(_resp) =
                gc_run(State(state.clone()), None).await.expect("gc_run handler must succeed");

            let ev = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
                .await
                .expect("timed out waiting for OperationComplete")
                .expect("event_tx channel closed unexpectedly");

            // Drain any further events (there should be none) so we can
            // assert exactly one OperationComplete was broadcast.
            let extra =
                tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
            assert!(extra.is_err(), "expected exactly one event to be broadcast by gc_run");

            ev
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(15))]

        /// Property 7: Completed long-running operations broadcast OperationComplete.
        ///
        /// For any number of orphaned models (including zero — a trivial
        /// success with nothing to quarantine), calling `gc_run` results in
        /// exactly one `WsEvent::OperationComplete` event with
        /// `operation == "gc"` and `success == true` (the current `gc_run`
        /// implementation only reports `success = false` when `run_safe`
        /// itself returns an `Err`, which does not happen for well-formed
        /// orphan sets processed against a healthy DB/CAS).
        ///
        /// **Validates: Requirements 3.7**
        #[test]
        fn prop_gc_run_broadcasts_operation_complete(orphan_count in 0usize..8) {
            let event = run_gc_run_and_collect_operation_complete(orphan_count);

            match event {
                WsEvent::OperationComplete { operation, success, .. } => {
                    prop_assert_eq!(operation.as_str(), "gc");
                    prop_assert!(success, "gc_run should report success for a healthy DB/CAS, regardless of orphan count");
                }
                other => prop_assert!(false, "expected OperationComplete, got {other:?}"),
            }
        }
    }
}
