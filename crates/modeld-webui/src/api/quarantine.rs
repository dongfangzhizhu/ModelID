//! Quarantine management REST API handlers.

use axum::{
    extract::{Path, State},
    response::Json,
};
use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Serialize)]
pub struct QuarantineItem {
    /// The quarantine file's unique id (blake3 hash).
    pub id: String,
    /// Original file path before quarantine.
    pub original_path: String,
    /// RFC-3339 timestamp when quarantined.
    pub quarantined_at: String,
    /// Reason for quarantine.
    pub reason: String,
    /// BLAKE3 hash of the file.
    pub blake3_hash: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Days remaining before TTL expiry (`null` if already expired).
    pub days_remaining: Option<i64>,
}

#[derive(Serialize)]
pub struct QuarantineListResponse {
    pub items: Vec<QuarantineItem>,
    pub total_size_bytes: u64,
}

/// `GET /api/v1/quarantine` — list all quarantined files.
pub async fn list_quarantine(
    State(state): State<AppState>,
) -> ApiResult<Json<QuarantineListResponse>> {
    let qm = modeld_core::QuarantineManager::new(&state.store_path);
    let entries = qm.list().unwrap_or_default();

    let total_size_bytes: u64 = entries.iter().map(|e| e.meta.size_bytes).sum();

    let items: Vec<QuarantineItem> = entries
        .iter()
        .map(|e| QuarantineItem {
            id: e.meta.blake3_hash.clone(),
            original_path: e.meta.original_path.clone(),
            quarantined_at: e.meta.quarantined_at.to_rfc3339(),
            reason: e.meta.reason.clone(),
            blake3_hash: e.meta.blake3_hash.clone(),
            size_bytes: e.meta.size_bytes,
            days_remaining: e.days_remaining,
        })
        .collect();

    Ok(Json(QuarantineListResponse { items, total_size_bytes }))
}

/// `POST /api/v1/quarantine/:id/restore` — restore a quarantined file by its
/// blake3 hash.  If multiple entries share the hash the most recently
/// quarantined one is restored.
pub async fn restore_quarantine(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let qm = modeld_core::QuarantineManager::new(&state.store_path);
    let entries = qm.list().map_err(|e| ApiError::internal(e.to_string()))?;

    // Find the entry matching the requested blake3 hash.
    // entries() is already sorted newest-first, so the first match is the
    // most recently quarantined one.
    let entry = entries
        .into_iter()
        .find(|e| e.meta.blake3_hash == id)
        .ok_or_else(|| ApiError::not_found(format!("No quarantine entry with id '{}'", id)))?;

    let restored_path =
        qm.restore(&entry.quarantine_path).map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "restored_path": restored_path.display().to_string(),
        "blake3_hash": id,
    })))
}
