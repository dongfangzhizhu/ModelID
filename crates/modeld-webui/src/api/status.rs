//! GET /api/v1/status — store health and summary statistics.

use axum::{extract::State, response::Json};
use serde::Serialize;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Serialize)]
pub struct StatusResponse {
    /// Total number of unique models in the store.
    pub total_models: i64,
    /// Combined size of all indexed models in bytes.
    pub total_size_bytes: i64,
    /// Estimated bytes that could be reclaimed by deduplication.
    pub dedup_savings_bytes: u64,
    /// RFC-3339 timestamp of the last completed scan (if any).
    pub last_scan_at: Option<String>,
    /// Number of active/pending downloads.
    pub download_queue_size: usize,
    /// Number of files currently in quarantine.
    pub quarantine_count: usize,
    /// Server version.
    pub version: &'static str,
}

pub async fn get_status(State(state): State<AppState>) -> ApiResult<Json<StatusResponse>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    let total_models = db.count_models().unwrap_or(0);
    let total_size_bytes = db.total_size().unwrap_or(0);

    // Estimate dedup savings: sum of (size × extra_copy_count) for every model
    // with 2+ aliases.
    let models = db.list_models(None).unwrap_or_default();
    let mut dedup_savings_bytes = 0u64;
    for model in &models {
        let aliases = db.get_aliases_for_model(&model.blake3_hash).unwrap_or_default();
        if aliases.len() >= 2 {
            dedup_savings_bytes +=
                model.size_bytes as u64 * (aliases.len() as u64 - 1);
        }
    }

    // Active/pending download count.
    let download_queue_size = db.list_downloads(None).unwrap_or_default().len();

    // Quarantine count (best-effort; ignore errors if dir doesn't exist yet).
    let qm = modeld_core::QuarantineManager::new(&state.store_path);
    let quarantine_count = qm.list().map(|e| e.len()).unwrap_or(0);

    Ok(Json(StatusResponse {
        total_models,
        total_size_bytes,
        dedup_savings_bytes,
        last_scan_at: None,
        download_queue_size,
        quarantine_count,
        version: env!("CARGO_PKG_VERSION"),
    }))
}
