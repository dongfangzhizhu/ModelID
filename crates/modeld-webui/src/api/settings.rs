use axum::{extract::State, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Serialize, Deserialize, Clone)]
pub struct UiSettings {
    pub store: StoreSettings,
    pub gc: GcSettings,
    pub ui: UiSectionSettings,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StoreSettings {
    pub root: String,
    pub auto_scan_on_start: bool,
    pub watch_enabled: bool,
    pub incremental_scan: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GcSettings {
    pub quarantine_days: u32,
    pub confirm_before_gc: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct UiSectionSettings {
    pub port: u16,
    pub host: String,
    pub open_browser: bool,
}

pub async fn get_settings(State(state): State<AppState>) -> ApiResult<Json<UiSettings>> {
    Ok(Json(UiSettings {
        store: StoreSettings {
            root: state.store_path.display().to_string(),
            auto_scan_on_start: false,
            watch_enabled: false,
            incremental_scan: true,
        },
        gc: GcSettings { quarantine_days: 30, confirm_before_gc: true },
        ui: UiSectionSettings { port: 8234, host: "127.0.0.1".to_string(), open_browser: false },
    }))
}

pub async fn put_settings(
    State(_state): State<AppState>,
    Json(_settings): Json<UiSettings>,
) -> Result<StatusCode, StatusCode> {
    // TODO: persist settings to modeld.toml
    Ok(StatusCode::NO_CONTENT)
}
