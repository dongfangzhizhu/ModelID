use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Serialize)]
pub struct GcCandidate {
    pub blake3_hash: String,
    pub name: String,
    pub size_bytes: u64,
    pub quarantine_since: String,
    pub reason: String,
}

#[derive(Serialize)]
pub struct GcPreviewResponse {
    pub reclaimable: Vec<GcCandidate>,
    pub total_reclaimable_bytes: u64,
}

pub async fn gc_preview(State(state): State<AppState>) -> ApiResult<Json<GcPreviewResponse>> {
    let qm = modeld_core::QuarantineManager::new(&state.store_path);
    let entries = qm.list().unwrap_or_default();

    let mut total = 0u64;
    let reclaimable: Vec<GcCandidate> = entries
        .iter()
        .map(|e| {
            total += e.meta.size_bytes;
            GcCandidate {
                blake3_hash: e.meta.blake3_hash.clone(),
                name: std::path::Path::new(&e.meta.original_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| e.meta.blake3_hash[..8].to_string()),
                size_bytes: e.meta.size_bytes,
                quarantine_since: e.meta.quarantined_at.to_rfc3339(),
                reason: e.meta.reason.clone(),
            }
        })
        .collect();

    Ok(Json(GcPreviewResponse { reclaimable, total_reclaimable_bytes: total }))
}

#[derive(Deserialize, Default)]
pub struct GcRunRequest {
    pub hashes: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct GcRunResponse {
    pub deleted: usize,
    pub bytes_freed: u64,
}

pub async fn gc_run(
    State(state): State<AppState>,
    body: Option<Json<GcRunRequest>>,
) -> ApiResult<Json<GcRunResponse>> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let qm = modeld_core::QuarantineManager::new(&state.store_path);
    let entries = qm.list().unwrap_or_default();

    let to_delete: Vec<_> = entries
        .iter()
        .filter(|e| {
            if let Some(hashes) = &req.hashes {
                hashes.contains(&e.meta.blake3_hash)
            } else {
                true
            }
        })
        .collect();

    let deleted = to_delete.len();
    let bytes_freed: u64 = to_delete.iter().map(|e| e.meta.size_bytes).sum();

    for entry in &to_delete {
        qm.delete_permanent(&entry.quarantine_path).ok();
    }

    Ok(Json(GcRunResponse { deleted, bytes_freed }))
}
