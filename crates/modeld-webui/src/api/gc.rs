//! GC API handlers — preview and run safe garbage collection.

use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

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
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("DB lock poisoned"))?;

    let gc = modeld_core::GcEngine::new(&mut *db, &state.store_path);
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

    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("DB lock poisoned"))?;

    let mut gc = modeld_core::GcEngine::new(&mut *db, &state.store_path);
    let result = gc.run_safe().map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(GcRunResponse {
        quarantined: result.quarantined,
        skipped_protected: result.skipped_protected,
        skipped_soft: result.skipped_soft,
        bytes_recovered: result.bytes_recovered,
        cleaned_quarantine: result.cleaned_quarantine,
    }))
}
