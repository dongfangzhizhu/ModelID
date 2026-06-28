pub mod downloads;
pub mod dupes;
pub mod gc;
pub mod models;
pub mod quarantine;
pub mod scan;
pub mod settings;
pub mod stats;
pub mod status;
pub mod tags;

use axum::{routing::get, routing::post, routing::put, Router};

use crate::state::AppState;

/// Mount all /api/v1/* routes.
/// NOTE: do NOT call .with_state() here — the outer Router in server.rs handles that.
pub fn routes() -> Router<AppState> {
    Router::new()
        // ── Status & Stats ────────────────────────────────────────────────
        .route("/status", get(status::get_status))
        .route("/stats", get(stats::get_stats))
        // ── Models ────────────────────────────────────────────────────────
        .route("/models", get(models::list_models))
        .route("/models/:hash", get(models::get_model))
        // ── Duplicates & Dedup ────────────────────────────────────────────
        .route("/dupes", get(dupes::list_dupes))
        // New canonical dedup routes
        .route("/dedup/preview", post(dupes::dedup_preview))
        .route("/dedup/apply", post(dupes::dedup_apply))
        // ── Downloads ─────────────────────────────────────────────────────
        .route("/downloads", get(downloads::list_downloads))
        // ── Scan ─────────────────────────────────────────────────────────
        .route("/scan", post(scan::trigger_scan))
        // Legacy trigger route kept for backward compat
        .route("/scan/trigger", post(scan::trigger_scan))
        // ── GC ────────────────────────────────────────────────────────────
        .route("/gc/preview", get(gc::gc_preview))
        .route("/gc/run", post(gc::gc_run))
        // ── Quarantine ────────────────────────────────────────────────────
        .route("/quarantine", get(quarantine::list_quarantine))
        .route("/quarantine/:id/restore", post(quarantine::restore_quarantine))
        // ── Settings ─────────────────────────────────────────────────────
        .route("/settings", get(settings::get_settings))
        .route("/settings", put(settings::put_settings))
        // ── Tags ──────────────────────────────────────────────────────────
        .route("/tags", get(tags::list_tags))
        .route("/models/:hash/tags", post(tags::add_model_tag))
}
