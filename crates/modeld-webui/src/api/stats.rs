use axum::{extract::State, response::Json};
use serde::Serialize;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Serialize)]
pub struct StatsResponse {
    pub total_models: i64,
    pub total_size_bytes: i64,
    pub duplicate_groups: usize,
    pub duplicate_waste_bytes: u64,
    pub deduped_saved_bytes: u64,
    pub frontends: Vec<FrontendStat>,
    pub last_scan_at: Option<String>,
    pub daemon_version: &'static str,
}

#[derive(Serialize)]
pub struct FrontendStat {
    pub name: String,
    pub model_count: usize,
    pub size_bytes: i64,
    pub duplicate_bytes: i64,
}

pub async fn get_stats(State(state): State<AppState>) -> ApiResult<Json<StatsResponse>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    let total_models = db.count_models().unwrap_or(0);
    let total_size_bytes = db.total_size().unwrap_or(0);

    // Compute duplicate stats: use DB to find models with 2+ aliases
    let models = db.list_models(None).unwrap_or_default();
    let mut duplicate_groups = 0usize;
    let mut duplicate_waste_bytes = 0u64;
    let mut frontend_map: std::collections::HashMap<String, FrontendStat> = Default::default();

    for model in &models {
        let aliases = db.get_aliases_for_model(&model.blake3_hash).unwrap_or_default();

        if aliases.len() >= 2 {
            duplicate_groups += 1;
            duplicate_waste_bytes += model.size_bytes as u64 * (aliases.len() as u64 - 1);
        }

        // Accumulate per-frontend stats
        let mut seen_frontends = std::collections::HashSet::new();
        for alias in &aliases {
            let name = alias.frontend.as_str().to_string();
            if seen_frontends.insert(name.clone()) {
                let entry = frontend_map.entry(name.clone()).or_insert(FrontendStat {
                    name,
                    model_count: 0,
                    size_bytes: 0,
                    duplicate_bytes: 0,
                });
                entry.model_count += 1;
                entry.size_bytes += model.size_bytes;
            }
        }
    }

    let frontends: Vec<FrontendStat> = frontend_map.into_values().collect();

    Ok(Json(StatsResponse {
        total_models,
        total_size_bytes,
        duplicate_groups,
        duplicate_waste_bytes,
        deduped_saved_bytes: 0,
        frontends,
        last_scan_at: None,
        daemon_version: env!("CARGO_PKG_VERSION"),
    }))
}
