pub mod downloads;
pub mod dupes;
pub mod gc;
pub mod models;
pub mod scan;
pub mod settings;
pub mod stats;

use axum::{routing::get, routing::post, routing::put, Router};

use crate::state::AppState;

/// Mount all /api/v1/* routes.
/// NOTE: do NOT call .with_state() here — the outer Router in server.rs handles that.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Stats
        .route("/stats", get(stats::get_stats))
        // Models
        .route("/models", get(models::list_models))
        .route("/models/:hash", get(models::get_model))
        // Duplicates
        .route("/dupes", get(dupes::list_dupes))
        .route("/dupes/dedup", post(dupes::run_dedup))
        // Downloads
        .route("/downloads", get(downloads::list_downloads))
        // Scan
        .route("/scan/trigger", post(scan::trigger_scan))
        // GC
        .route("/gc/preview", post(gc::gc_preview))
        .route("/gc/run", post(gc::gc_run))
        // Settings
        .route("/settings", get(settings::get_settings))
        .route("/settings", put(settings::put_settings))
}
