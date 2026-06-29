//! Scan API handler — POST /api/v1/scan

use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::error::ApiResult;
use crate::state::{AppState, WsEvent};

/// Request body for `POST /api/v1/scan`.
///
/// Both the legacy single-path format (`path`) and the new multi-path format
/// (`paths`) are accepted for backward compatibility.
#[derive(Deserialize, Default)]
pub struct ScanRequest {
    /// A single directory to scan (legacy field).
    pub path: Option<String>,
    /// One or more directories to scan (new canonical field).
    pub paths: Option<Vec<String>>,
    pub force_rehash: Option<bool>,
    /// Follow symbolic links when traversing (default: false)
    pub follow_symlinks: Option<bool>,
    /// Glob patterns to exclude
    pub exclude_globs: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct ScanResponse {
    pub scan_id: String,
    pub message: String,
    /// Paths that will be scanned (resolved from request).
    pub scan_paths: Vec<String>,
}

pub async fn trigger_scan(
    State(state): State<AppState>,
    body: Option<Json<ScanRequest>>,
) -> ApiResult<Json<ScanResponse>> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let scan_id = uuid::Uuid::new_v4().to_string();

    // Resolve scan paths: prefer `paths`, fall back to `path`, then store root.
    let mut scan_paths: Vec<std::path::PathBuf> = req
        .paths
        .as_deref()
        .map(|ps| ps.iter().map(std::path::PathBuf::from).collect())
        .or_else(|| req.path.as_deref().map(|p| vec![std::path::PathBuf::from(p)]))
        .unwrap_or_else(|| vec![state.store_path.clone()]);

    // De-duplicate
    scan_paths.dedup();

    let scan_paths_display: Vec<String> =
        scan_paths.iter().map(|p| p.display().to_string()).collect();

    let db_arc = state.db.clone();
    let store_path = state.store_path.clone();
    let event_tx = state.event_tx.clone();
    let sid = scan_id.clone();
    let force_rehash = req.force_rehash.unwrap_or(false);
    let follow_symlinks = req.follow_symlinks.unwrap_or(false);
    let exclude_globs = req.exclude_globs.unwrap_or_default();

    tokio::spawn(async move {
        for scan_root in scan_paths {
            let _ = event_tx.send(WsEvent::ScanProgress {
                path: scan_root.display().to_string(),
                done: 0,
                total: 0,
            });

            let db_arc2 = db_arc.clone();
            let store_path2 = store_path.clone();
            let event_tx2 = event_tx.clone();
            let event_tx3 = event_tx.clone(); // kept for the post-scan progress send
            let scan_root2 = scan_root.clone();
            let excl2 = exclude_globs.clone();

            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
                // Load preindexed cache for incremental scan
                let preindexed = {
                    let db = db_arc2.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
                    db.get_all_indexed_paths().unwrap_or_default()
                };

                let store_canonical =
                    std::fs::canonicalize(&store_path2).unwrap_or(store_path2.clone());

                let scan_opts = modeld_core::ScanOptions {
                    incremental: !force_rehash,
                    full: force_rehash,
                    follow_symlinks,
                    exclude_globs: excl2,
                };

                let scanner = modeld_core::Scanner::new()
                    .with_excluded_dirs(vec![store_canonical])
                    .with_preindexed(preindexed)
                    .with_scan_options(scan_opts);

                // Count files first for accurate total in progress events
                let (total_files, _) = scanner.count_files(&scan_root2).unwrap_or((0, 0));
                let total = total_files as u64;
                let done_counter = Arc::new(AtomicU64::new(0));
                let done_arc = done_counter.clone();
                let etx = event_tx2.clone();
                let root_str = scan_root2.display().to_string();

                let scanned = scanner.scan(&scan_root2, move |path, _size| {
                    let done = done_arc.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = etx.send(WsEvent::ScanProgress {
                        path: path.display().to_string(),
                        done,
                        total,
                    });
                    let _ = root_str; // keep capture alive
                })?;

                let cas_path = store_path2.join("cas");
                std::fs::create_dir_all(&cas_path).ok();

                let mut db = db_arc2.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
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
                    db.insert_alias(
                        &file.hash,
                        &file.path.to_string_lossy(),
                        modeld_core::db::Frontend::User,
                        modeld_core::db::AliasType::Original,
                    )
                    .ok();
                    // Update incremental scan index
                    db.upsert_path_index(
                        &file.path.to_string_lossy(),
                        &file.hash,
                        file.size as i64,
                        file.mtime,
                        file.inode,
                        file.device_id,
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

            let _ = event_tx3.send(WsEvent::ScanProgress {
                path: scan_root.display().to_string(),
                done: files_count,
                total: files_count,
            });
        }

        // Signal all-paths complete
        let _ = event_tx.send(WsEvent::OperationComplete {
            operation: format!("scan:{}", sid),
            success: true,
            message: "Scan complete".to_string(),
        });
    });

    Ok(Json(ScanResponse {
        scan_id,
        message: "Scan started".to_string(),
        scan_paths: scan_paths_display,
    }))
}
