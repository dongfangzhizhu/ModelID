use axum::{extract::State, response::Json};
use serde::Serialize;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Serialize)]
pub struct DownloadItem {
    pub id: i64,
    pub name: String,
    pub source_url: String,
    pub status: String,
    pub bytes_total: i64,
    pub bytes_done: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub blake3_hash: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct DownloadsResponse {
    pub items: Vec<DownloadItem>,
}

pub async fn list_downloads(State(state): State<AppState>) -> ApiResult<Json<DownloadsResponse>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    // Pass None to list all downloads regardless of status
    let downloads = db.list_downloads(None).unwrap_or_default();

    let items = downloads
        .iter()
        .map(|d| {
            // Derive display name from filename or URL
            let name = d
                .filename
                .clone()
                .or_else(|| {
                    d.source_url.split('/').last().map(|s| s.to_string())
                })
                .unwrap_or_else(|| format!("download-{}", d.id));

            DownloadItem {
                id: d.id,
                name,
                source_url: d.source_url.clone(),
                status: d.status.as_str().to_string(),
                bytes_total: d.bytes_total,
                bytes_done: d.bytes_done,
                started_at: d.started_at.to_rfc3339(),
                finished_at: d.finished_at.map(|t| t.to_rfc3339()),
                blake3_hash: d.model_hash.clone(),
                error: d.error_message.clone(),
            }
        })
        .collect();

    Ok(Json(DownloadsResponse { items }))
}
