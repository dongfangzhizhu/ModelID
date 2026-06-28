use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::state::AppState;

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
            .map(|a| DupePath {
                path: a.path.clone(),
                frontend: a.frontend.as_str().to_string(),
            })
            .collect();

        let waste = model.size_bytes * (aliases.len() as i64 - 1);
        total_waste_bytes += waste;

        // Derive a readable name from first alias path
        let name = aliases
            .first()
            .and_then(|a| {
                std::path::Path::new(&a.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
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
    items.sort_by(|a, b| b.waste_bytes.cmp(&a.waste_bytes));

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
pub async fn dedup_preview(
    State(state): State<AppState>,
) -> ApiResult<Json<DedupPreviewResponse>> {
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

    // Run the blocking dedup in a spawn_blocking task.
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<modeld_core::DedupStats> {
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
                filter_hashes.as_ref().map_or(true, |hs| {
                    hs.contains(&g.hash.as_hex().to_string())
                })
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
    .await
    .map_err(|e| crate::error::ApiError::internal(e.to_string()))?
    .map_err(|e| crate::error::ApiError::internal(e.to_string()))?;

    Ok(Json(DedupApplyResponse {
        groups_processed: result.groups_processed,
        groups_succeeded: result.groups_succeeded,
        groups_failed: result.groups_failed,
        space_saved: result.space_saved,
        files_deduplicated: result.files_deduplicated,
    }))
}
