use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Deserialize, Default)]
pub struct ScanRequest {
    pub path: Option<String>,
    pub force_rehash: Option<bool>,
}

#[derive(Serialize)]
pub struct ScanResponse {
    pub scan_id: String,
    pub message: String,
}

pub async fn trigger_scan(
    State(state): State<AppState>,
    body: Option<Json<ScanRequest>>,
) -> ApiResult<Json<ScanResponse>> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let scan_id = uuid::Uuid::new_v4().to_string();

    let db_arc = state.db.clone();
    let store_path = state.store_path.clone();
    let event_tx = state.event_tx.clone();

    tokio::spawn(async move {
        let scan_root = req
            .path
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| store_path.clone());

        let _ = event_tx.send(crate::state::WsEvent::ScanProgress(
            crate::state::ScanProgressPayload {
                files_scanned: 0,
                files_total: 0,
                new_models_found: 0,
                duplicates_found: 0,
                phase: "walking".to_string(),
                current_path: scan_root.display().to_string(),
            },
        ));

        // Run blocking scan in a dedicated thread
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
            let scanner = modeld_core::Scanner::new();
            let scanned = scanner.scan(&scan_root, |_path, _size| {})?;

            // Store each scanned file into DB
            let cas_path = store_path.join("cas");
            std::fs::create_dir_all(&cas_path).ok();

            let mut db = db_arc.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
            let mut count = 0usize;

            for file in &scanned {
                db.insert_or_update_model(
                    &file.hash,
                    file.size as i64,
                    None,
                    None,
                    None,
                    None,
                )?;
                // Try inserting alias; ignore duplicate errors
                db.insert_alias(
                    &file.hash,
                    &file.path.to_string_lossy(),
                    modeld_core::db::Frontend::User,
                    modeld_core::db::AliasType::Original,
                )
                .ok();
                count += 1;
            }
            Ok(count)
        })
        .await;

        let files_count = result
            .as_ref()
            .ok()
            .and_then(|r| r.as_ref().ok())
            .copied()
            .unwrap_or(0) as u64;

        let _ = event_tx.send(crate::state::WsEvent::ScanProgress(
            crate::state::ScanProgressPayload {
                files_scanned: files_count,
                files_total: files_count,
                new_models_found: files_count,
                duplicates_found: 0,
                phase: "done".to_string(),
                current_path: String::new(),
            },
        ));
    });

    Ok(Json(ScanResponse { scan_id, message: "Scan started".to_string() }))
}
