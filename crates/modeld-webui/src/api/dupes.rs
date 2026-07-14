use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::state::{AppState, WsEvent};

#[derive(Serialize)]
pub struct DupePath {
    pub path: String,
    pub frontend: String,
}

#[derive(Serialize)]
pub struct DupeGroup {
    pub blake3_hash: String,
    pub name: String,
    pub arch: Option<String>,
    pub size_bytes: i64,
    pub copy_count: usize,
    pub waste_bytes: i64,
    pub paths: Vec<DupePath>,
}

#[derive(Serialize)]
pub struct DupesResponse {
    pub items: Vec<DupeGroup>,
    pub total_waste_bytes: i64,
}

pub async fn list_dupes(State(state): State<AppState>) -> ApiResult<Json<DupesResponse>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    let models = db.list_models(None).unwrap_or_default();
    let mut total_waste_bytes = 0i64;
    let mut items = Vec::new();

    for model in &models {
        let aliases = db.get_aliases_for_model(&model.blake3_hash).unwrap_or_default();

        if aliases.len() < 2 {
            continue;
        }

        let paths: Vec<DupePath> = aliases
            .iter()
            .map(|a| DupePath { path: a.path.clone(), frontend: a.frontend.as_str().to_string() })
            .collect();

        let waste = model.size_bytes * (aliases.len() as i64 - 1);
        total_waste_bytes += waste;

        // Derive a readable name from first alias path
        let name = aliases
            .first()
            .and_then(|a| {
                std::path::Path::new(&a.path).file_name().map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| model.blake3_hash.as_hex()[..8].to_string());

        items.push(DupeGroup {
            blake3_hash: model.blake3_hash.as_hex().to_string(),
            name,
            arch: model.arch.clone(),
            size_bytes: model.size_bytes,
            copy_count: aliases.len(),
            waste_bytes: waste,
            paths,
        });
    }

    // Sort by waste (largest first)
    items.sort_by_key(|b| std::cmp::Reverse(b.waste_bytes));

    Ok(Json(DupesResponse { items, total_waste_bytes }))
}

// ─── Dedup Preview ───────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct DedupPreviewOperation {
    pub hash: String,
    pub keep_path: String,
    pub replace_with_links: Vec<String>,
    pub would_save_bytes: i64,
    pub strategy: String,
}

#[derive(Serialize)]
pub struct DedupPreviewResponse {
    pub operations: Vec<DedupPreviewOperation>,
    pub total_would_save_bytes: i64,
}

/// `POST /api/v1/dedup/preview` — dry-run dedup; return what would be done.
pub async fn dedup_preview(State(state): State<AppState>) -> ApiResult<Json<DedupPreviewResponse>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    let models = db.list_models(None).unwrap_or_default();
    let mut operations = Vec::new();
    let mut total_save = 0i64;

    for model in &models {
        let aliases = db.get_aliases_for_model(&model.blake3_hash).unwrap_or_default();
        if aliases.len() < 2 {
            continue;
        }

        let keep_path = aliases[0].path.clone();
        let replace: Vec<String> = aliases[1..].iter().map(|a| a.path.clone()).collect();
        let save = model.size_bytes * replace.len() as i64;
        total_save += save;

        operations.push(DedupPreviewOperation {
            hash: model.blake3_hash.as_hex().to_string(),
            keep_path,
            replace_with_links: replace,
            would_save_bytes: save,
            strategy: "hardlink".to_string(),
        });
    }

    Ok(Json(DedupPreviewResponse { operations, total_would_save_bytes: total_save }))
}

// ─── Dedup Apply ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DedupApplyRequest {
    /// Optional subset of hashes to dedup; absent = all duplicates.
    pub hashes: Option<Vec<String>>,
    /// Strategy override: "hardlink" | "symlink" | "copy_to_cas"
    pub strategy: Option<String>,
}

#[derive(Serialize)]
pub struct DedupApplyResponse {
    pub groups_processed: usize,
    pub groups_succeeded: usize,
    pub groups_failed: usize,
    pub space_saved: u64,
    pub files_deduplicated: usize,
}

/// `POST /api/v1/dedup/apply` — execute dedup (moves duplicates to quarantine /
/// replaces with hardlinks; does **not** permanently delete anything).
pub async fn dedup_apply(
    State(state): State<AppState>,
    body: Option<Json<DedupApplyRequest>>,
) -> ApiResult<Json<DedupApplyResponse>> {
    let req = body.map(|b| b.0);

    let store_path = state.store_path.clone();
    let event_tx = state.event_tx.clone();

    // Run the blocking dedup in a spawn_blocking task.
    let dedup_result =
        tokio::task::spawn_blocking(move || -> anyhow::Result<modeld_core::DedupStats> {
            let filter_hashes: Option<std::collections::HashSet<String>> =
                req.as_ref().and_then(|r| r.hashes.as_ref()).map(|hs| hs.iter().cloned().collect());

            // Open a dedicated DB connection for the DedupEngine (it needs ownership).
            let db_path = store_path.join("modeld.db");
            let db = modeld_core::Database::open(&db_path)?;

            let mut engine = modeld_core::DedupEngine::new(db, store_path.clone());

            let groups = engine.find_duplicates()?;

            let filtered_groups: Vec<_> = groups
                .into_iter()
                .filter(|g| {
                    filter_hashes
                        .as_ref()
                        .is_none_or(|hs| hs.contains(&g.hash.as_hex().to_string()))
                })
                .collect();

            let mut stats = modeld_core::DedupStats::default();
            for group in &filtered_groups {
                stats.groups_processed += 1;
                match engine.execute_dedup_group(group, modeld_core::DedupMode::Auto) {
                    Ok(r) => {
                        stats.groups_succeeded += 1;
                        stats.space_saved += r.space_saved;
                        stats.files_deduplicated += r.links_created.len();
                    }
                    Err(_) => {
                        stats.groups_failed += 1;
                    }
                }
            }

            Ok(stats)
        })
        .await;

    let result = match dedup_result {
        Ok(Ok(stats)) => stats,
        Ok(Err(e)) => {
            let _ = event_tx.send(WsEvent::OperationComplete {
                operation: "dedup".to_string(),
                success: false,
                message: format!("Dedup failed: {e}"),
            });
            return Err(crate::error::ApiError::internal(e.to_string()));
        }
        Err(join_err) => {
            let _ = event_tx.send(WsEvent::OperationComplete {
                operation: "dedup".to_string(),
                success: false,
                message: format!("Dedup failed: {join_err}"),
            });
            return Err(crate::error::ApiError::internal(join_err.to_string()));
        }
    };

    let _ = event_tx.send(WsEvent::OperationComplete {
        operation: "dedup".to_string(),
        success: result.groups_failed == 0,
        message: format!(
            "Dedup complete: {} of {} groups succeeded, {} files deduplicated, {} bytes saved",
            result.groups_succeeded,
            result.groups_processed,
            result.files_deduplicated,
            result.space_saved
        ),
    });

    Ok(Json(DedupApplyResponse {
        groups_processed: result.groups_processed,
        groups_succeeded: result.groups_succeeded,
        groups_failed: result.groups_failed,
        space_saved: result.space_saved,
        files_deduplicated: result.files_deduplicated,
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
    fn dedup_preview_response_field_names_are_stable() {
        let response = DedupPreviewResponse {
            operations: vec![DedupPreviewOperation {
                hash: "abc123".to_string(),
                keep_path: "/models/a.safetensors".to_string(),
                replace_with_links: vec!["/models/b.safetensors".to_string()],
                would_save_bytes: 1024,
                strategy: "hardlink".to_string(),
            }],
            total_would_save_bytes: 1024,
        };

        let value = serde_json::to_value(&response).expect("serialize DedupPreviewResponse");
        let obj = value.as_object().expect("DedupPreviewResponse serializes to an object");

        for field in ["operations", "total_would_save_bytes"] {
            assert!(obj.contains_key(field), "DedupPreviewResponse missing field `{field}`");
        }

        let op = &value["operations"][0];
        for field in ["hash", "keep_path", "replace_with_links", "would_save_bytes", "strategy"] {
            assert!(
                op.as_object().unwrap().contains_key(field),
                "DedupPreviewOperation missing field `{field}`"
            );
        }
    }

    #[test]
    fn dedup_apply_response_field_names_are_stable() {
        let response = DedupApplyResponse {
            groups_processed: 5,
            groups_succeeded: 4,
            groups_failed: 1,
            space_saved: 2048,
            files_deduplicated: 3,
        };

        let value = serde_json::to_value(&response).expect("serialize DedupApplyResponse");
        let obj = value.as_object().expect("DedupApplyResponse serializes to an object");

        for field in [
            "groups_processed",
            "groups_succeeded",
            "groups_failed",
            "space_saved",
            "files_deduplicated",
        ] {
            assert!(obj.contains_key(field), "DedupApplyResponse missing field `{field}`");
        }
    }

    #[test]
    fn dupes_response_field_names_are_stable() {
        let response = DupesResponse {
            items: vec![DupeGroup {
                blake3_hash: "abc123".to_string(),
                name: "model.safetensors".to_string(),
                arch: None,
                size_bytes: 1024,
                copy_count: 2,
                waste_bytes: 1024,
                paths: vec![DupePath {
                    path: "/models/a.safetensors".to_string(),
                    frontend: "user".to_string(),
                }],
            }],
            total_waste_bytes: 1024,
        };

        let value = serde_json::to_value(&response).expect("serialize DupesResponse");
        let obj = value.as_object().expect("DupesResponse serializes to an object");

        for field in ["items", "total_waste_bytes"] {
            assert!(obj.contains_key(field), "DupesResponse missing field `{field}`");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Property-based tests (audit Wave 3, Property 7)
// ─────────────────────────────────────────────────────────────────────────────
//
// `dedup_apply` is an axum handler whose only observable side-channel signal
// that "the operation completed" is the `WsEvent::OperationComplete` it
// broadcasts on `state.event_tx` right before returning. These tests build a
// real (temp-dir-backed) `AppState` + `Database`, subscribe to `event_tx`
// *before* invoking the handler, await the handler, and assert on the single
// `OperationComplete` event it broadcasts — exercising the real handler code
// path (no mocking), mirroring the pattern used for `trigger_scan` in
// `api::scan::property_tests`.
#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::state::AppState;
    use modeld_core::db::{AliasType, Frontend};
    use proptest::prelude::*;
    use tempfile::TempDir;

    /// Run `dedup_apply` against a fresh temp-dir-backed `AppState` seeded
    /// with a single duplicate group (two aliases sharing `declared_hash`,
    /// both pointing at real files containing `actual_content`), and return
    /// the single `WsEvent::OperationComplete` broadcast during the call.
    ///
    /// When `actual_content`'s real BLAKE3 hash matches `declared_hash`,
    /// `execute_dedup_group` succeeds (this simulates the well-formed input
    /// case). When it does not match, the CAS staging hash-verification
    /// inside `execute_dedup_group` fails for that group, causing
    /// `groups_failed > 0` — this simulates a genuine failure path without
    /// needing to break the filesystem or DB out-of-band.
    fn run_dedup_apply_and_collect_operation_complete(
        declared_hash: &modeld_core::Blake3Hash,
        actual_content: &[u8],
    ) -> WsEvent {
        let rt = tokio::runtime::Runtime::new().expect("build tokio runtime");
        rt.block_on(async move {
            let store_dir = TempDir::new().expect("create store temp dir");
            let db_path = store_dir.path().join("modeld.db");
            let mut db = modeld_core::Database::open(&db_path).expect("open test database");

            let models_dir = store_dir.path().join("models");
            std::fs::create_dir_all(&models_dir).expect("create models dir");
            let path_a = models_dir.join("a.safetensors");
            let path_b = models_dir.join("b.safetensors");
            std::fs::write(&path_a, actual_content).expect("write file a");
            std::fs::write(&path_b, actual_content).expect("write file b");

            db.insert_or_update_model(
                declared_hash,
                actual_content.len() as i64,
                None,
                None,
                None,
                None,
            )
            .expect("insert model");
            db.insert_alias(
                declared_hash,
                &path_a.display().to_string(),
                Frontend::User,
                AliasType::Original,
            )
            .expect("insert alias a");
            db.insert_alias(
                declared_hash,
                &path_b.display().to_string(),
                Frontend::User,
                AliasType::Original,
            )
            .expect("insert alias b");

            let (state, _initial_rx) =
                AppState::new(db, store_dir.path().to_path_buf(), None, None);
            let mut rx = state.event_tx.subscribe();

            let Json(_resp) = dedup_apply(State(state.clone()), None)
                .await
                .expect("dedup_apply handler must succeed");

            let ev = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
                .await
                .expect("timed out waiting for OperationComplete")
                .expect("event_tx channel closed unexpectedly");

            // Drain any further events (there should be none) so we can
            // assert exactly one OperationComplete was broadcast.
            let extra =
                tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
            assert!(extra.is_err(), "expected exactly one event to be broadcast by dedup_apply");

            ev
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(15))]

        /// Property 7: Completed long-running operations broadcast OperationComplete.
        ///
        /// For any duplicate group whose two aliased files' real content
        /// matches the model's declared hash (a well-formed duplicate),
        /// calling `dedup_apply` results in exactly one
        /// `WsEvent::OperationComplete` event with `operation == "dedup"`
        /// and `success == true`.
        ///
        /// **Validates: Requirements 3.7**
        #[test]
        fn prop_dedup_apply_success_broadcasts_operation_complete(
            content in prop::collection::vec(1u8..=255, 1..256)
        ) {
            // Write the content to a scratch file first so we can compute
            // its real BLAKE3 hash via the same code path production uses.
            let scratch = TempDir::new().unwrap();
            let scratch_path = scratch.path().join("scratch.bin");
            std::fs::write(&scratch_path, &content).unwrap();
            let real_hash = modeld_core::hash_file(&scratch_path).unwrap();

            let event = run_dedup_apply_and_collect_operation_complete(&real_hash, &content);

            match event {
                WsEvent::OperationComplete { operation, success, .. } => {
                    prop_assert_eq!(operation.as_str(), "dedup");
                    prop_assert!(success, "dedup_apply should report success when all groups dedup cleanly");
                }
                other => prop_assert!(false, "expected OperationComplete, got {other:?}"),
            }
        }

        /// Property 7 (failure path): For any duplicate group whose declared
        /// hash does not match the aliased files' real content, calling
        /// `dedup_apply` still results in exactly one
        /// `WsEvent::OperationComplete` event with `operation == "dedup"`,
        /// but with `success == false` (the CAS staging hash-verification
        /// inside `execute_dedup_group` rejects the mismatched group,
        /// incrementing `groups_failed`).
        ///
        /// **Validates: Requirements 3.7**
        #[test]
        fn prop_dedup_apply_hash_mismatch_broadcasts_failed_operation_complete(
            declared_seed in 1u8..=255,
            actual_content in prop::collection::vec(1u8..=255, 1..256)
        ) {
            // Build a declared hash from content that is guaranteed to
            // differ from `actual_content` (different length via the
            // leading marker byte + a distinct fill byte), so the staged
            // copy's real hash never matches the declared one.
            let mut declared_content = actual_content.clone();
            declared_content.push(declared_seed);
            declared_content.push(declared_seed.wrapping_add(1));
            prop_assume!(declared_content != actual_content);

            let scratch = TempDir::new().unwrap();
            let scratch_path = scratch.path().join("scratch.bin");
            std::fs::write(&scratch_path, &declared_content).unwrap();
            let declared_hash = modeld_core::hash_file(&scratch_path).unwrap();

            let event = run_dedup_apply_and_collect_operation_complete(&declared_hash, &actual_content);

            match event {
                WsEvent::OperationComplete { operation, success, .. } => {
                    prop_assert_eq!(operation.as_str(), "dedup");
                    prop_assert!(!success, "dedup_apply should report failure when a group's staged content hash-mismatches");
                }
                other => prop_assert!(false, "expected OperationComplete, got {other:?}"),
            }
        }
    }
}
