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

// ─── Dedup operation ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DedupRequest {
    pub dry_run: bool,
    pub hashes: Option<Vec<String>>,
    pub strategy: Option<String>,
}

#[derive(Serialize)]
pub struct DedupOperation {
    pub hash: String,
    pub keep_path: String,
    pub replace_with_links: Vec<String>,
    pub would_save_bytes: i64,
}

#[derive(Serialize)]
pub struct DedupResponse {
    pub dry_run: bool,
    pub operations: Vec<DedupOperation>,
    pub total_would_save_bytes: i64,
}

pub async fn run_dedup(
    State(state): State<AppState>,
    Json(req): Json<DedupRequest>,
) -> ApiResult<Json<DedupResponse>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    let models = db.list_models(None).unwrap_or_default();
    let mut operations = Vec::new();
    let mut total_save = 0i64;

    for model in &models {
        // Filter by requested hashes
        if let Some(hashes) = &req.hashes {
            if !hashes.contains(&model.blake3_hash.as_hex().to_string()) {
                continue;
            }
        }

        let aliases = db.get_aliases_for_model(&model.blake3_hash).unwrap_or_default();
        if aliases.len() < 2 {
            continue;
        }

        let keep_path = aliases[0].path.clone();
        let replace: Vec<String> = aliases[1..].iter().map(|a| a.path.clone()).collect();
        let save = model.size_bytes * replace.len() as i64;
        total_save += save;

        operations.push(DedupOperation {
            hash: model.blake3_hash.as_hex().to_string(),
            keep_path,
            replace_with_links: replace,
            would_save_bytes: save,
        });
    }

    // TODO: when dry_run=false, call DedupEngine for actual execution

    Ok(Json(DedupResponse {
        dry_run: true, // always preview-only until DedupEngine is wired in
        operations,
        total_would_save_bytes: total_save,
    }))
}
