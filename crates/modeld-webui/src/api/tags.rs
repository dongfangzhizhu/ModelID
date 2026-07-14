//! Tags REST API handlers.
//!
//! `GET  /api/v1/tags`                   — list all tags with usage counts
//! `POST /api/v1/models/:hash/tags`      — add a tag to a specific model

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ─── List all tags ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct TagSummary {
    pub tag: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct TagsResponse {
    pub tags: Vec<TagSummary>,
    pub total: usize,
}

/// `GET /api/v1/tags` — return all tags with their usage counts, sorted by
/// usage (most-used first).
pub async fn list_tags(State(state): State<AppState>) -> ApiResult<Json<TagsResponse>> {
    let db = state.db.lock().map_err(|_| ApiError::internal("DB lock poisoned"))?;

    let pairs = db.list_all_tags().map_err(|e| ApiError::internal(e.to_string()))?;

    let total = pairs.len();
    let tags = pairs.into_iter().map(|(tag, count)| TagSummary { tag, count }).collect();

    Ok(Json(TagsResponse { tags, total }))
}

// ─── Add tag to model ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddTagRequest {
    pub tag: String,
}

#[derive(Serialize)]
pub struct AddTagResponse {
    pub model_hash: String,
    pub tag: String,
    pub tags: Vec<String>,
}

/// `POST /api/v1/models/:hash/tags` — attach a tag to the given model.
pub async fn add_model_tag(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    Json(body): Json<AddTagRequest>,
) -> ApiResult<(StatusCode, Json<AddTagResponse>)> {
    // Validate the tag is non-empty
    let tag = body.tag.trim().to_string();
    if tag.is_empty() {
        return Err(ApiError::bad_request("tag must not be empty"));
    }

    // Validate hash length (BLAKE3 = 64 hex chars)
    if hash.len() != 64 {
        return Err(ApiError::bad_request("Invalid hash format: must be 64 hex characters"));
    }

    let mut db = state.db.lock().map_err(|_| ApiError::internal("DB lock poisoned"))?;

    // Ensure the model exists and parse the hash
    let hash_obj = modeld_core::Blake3Hash::from_hex(&hash)
        .map_err(|_| ApiError::bad_request("Invalid hash format"))?;
    let _model = db
        .get_model(&hash_obj)
        .unwrap_or(None)
        .ok_or_else(|| ApiError::not_found("Model not found"))?;

    // Insert the tag (idempotent — INSERT OR IGNORE)
    db.add_tag(&hash_obj, &tag).map_err(|e| ApiError::internal(e.to_string()))?;

    // Return the updated tag list for this model
    let tags = db.get_tags(&hash_obj).map_err(|e| ApiError::internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(AddTagResponse { model_hash: hash, tag, tags })))
}
