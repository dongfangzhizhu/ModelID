//! Settings API — reads and writes `modeld.toml`.
//!
//! Only settings that something actually reads are exposed here (A7). A toggle
//! nothing honours is worse than no toggle: it makes the UI lie. The fields
//! below are wired as follows:
//!
//! | Field                  | Backing config            | Read by                    |
//! |------------------------|---------------------------|----------------------------|
//! | `store.root`           | — (read-only echo)        | the running process        |
//! | `store.incremental_scan` | `[scan] incremental`      | WebUI scan endpoint        |
//! | `gc.quarantine_days`   | `[gc] quarantine_ttl_days` | `GcEngine` (CLI + WebUI) |
//! | `ui.host` / `ui.port`  | `[serve] host/port`       | `modeld serve` on restart   |
//! | `ui.open_browser`      | `[serve] open_browser`    | `modeld serve` on restart   |
//!
//! Changes take effect on restart except `incremental_scan`, which is read per
//! scan.

use axum::{extract::State, response::Json};
use serde::{Deserialize, Serialize};
use std::path::Path;

use modeld_core::config::{load_config, save_config, ModeldConfig};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSettings {
    pub store: StoreSettings,
    pub gc: GcSettings,
    pub ui: UiSectionSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoreSettings {
    /// Absolute store path. Informational: changing it would require a restart
    /// with a different `--store`, so it is ignored on write.
    pub root: String,
    /// `[scan] incremental` — re-hash only changed files.
    pub incremental_scan: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GcSettings {
    /// `[gc] quarantine_ttl_days` — how long quarantined objects are kept.
    pub quarantine_days: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSectionSettings {
    pub port: u16,
    pub host: String,
    pub open_browser: bool,
}

impl UiSettings {
    /// Project the persisted config into the shape the settings page consumes.
    pub fn from_config(cfg: &ModeldConfig, store_path: &Path) -> Self {
        Self {
            store: StoreSettings {
                root: store_path.display().to_string(),
                incremental_scan: cfg.scan.incremental,
            },
            gc: GcSettings { quarantine_days: cfg.gc.quarantine_ttl_days },
            ui: UiSectionSettings {
                port: cfg.serve.port,
                host: cfg.serve.host.clone(),
                open_browser: cfg.serve.open_browser,
            },
        }
    }

    /// Validate and copy the editable fields into `cfg`.
    ///
    /// Returns a human-readable message on invalid input; nothing is written in
    /// that case.
    pub fn apply_to(&self, cfg: &mut ModeldConfig) -> Result<(), String> {
        if self.ui.port == 0 {
            return Err("port must be between 1 and 65535".to_string());
        }
        let host = self.ui.host.trim();
        if host.is_empty() {
            return Err("host must not be empty".to_string());
        }
        if !(1..=3650).contains(&self.gc.quarantine_days) {
            return Err("quarantine_days must be between 1 and 3650".to_string());
        }

        cfg.scan.incremental = self.store.incremental_scan;
        cfg.gc.quarantine_ttl_days = self.gc.quarantine_days;
        cfg.serve.host = host.to_string();
        cfg.serve.port = self.ui.port;
        cfg.serve.open_browser = self.ui.open_browser;
        // `store.root` is deliberately not applied: it mirrors the `--store` the
        // server was started with.
        Ok(())
    }
}

/// `GET /api/v1/settings` — current values, straight from `modeld.toml`.
pub async fn get_settings(State(state): State<AppState>) -> ApiResult<Json<UiSettings>> {
    let cfg = load_config(&state.store_path).unwrap_or_default();
    Ok(Json(UiSettings::from_config(&cfg, &state.store_path)))
}

/// `PUT /api/v1/settings` — validate, persist, return the stored values.
pub async fn put_settings(
    State(state): State<AppState>,
    Json(settings): Json<UiSettings>,
) -> ApiResult<Json<UiSettings>> {
    let mut cfg = load_config(&state.store_path)
        .map_err(|e| ApiError::internal(format!("could not read modeld.toml: {e}")))?;

    settings.apply_to(&mut cfg).map_err(ApiError::bad_request)?;

    save_config(&state.store_path, &cfg)
        .map_err(|e| ApiError::internal(format!("could not write modeld.toml: {e}")))?;

    modeld_core::audit::record(
        &state.store_path,
        modeld_core::AuditEntry {
            details: Some(format!(
                "Web UI: host={} port={} quarantine_days={} incremental={}",
                cfg.serve.host, cfg.serve.port, cfg.gc.quarantine_ttl_days, cfg.scan.incremental
            )),
            ..modeld_core::AuditEntry::success("config_change", "modeld.toml")
        },
    );

    Ok(Json(UiSettings::from_config(&cfg, &state.store_path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = dir.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        (dir, store)
    }

    /// `PUT` → `modeld.toml` → reload must return the same values, which is
    /// what "survives a restart" means.
    #[test]
    fn persist_roundtrip() {
        let (_dir, store) = temp_store();

        let mut cfg = ModeldConfig::default();
        let mut settings = UiSettings::from_config(&cfg, &store);
        assert_eq!(settings.gc.quarantine_days, 30);
        assert_eq!(settings.ui.port, 8234);
        assert!(settings.store.incremental_scan);

        settings.gc.quarantine_days = 7;
        settings.ui.port = 8500;
        settings.ui.host = "127.0.0.1".to_string();
        settings.ui.open_browser = true;
        settings.store.incremental_scan = false;
        settings.apply_to(&mut cfg).unwrap();
        save_config(&store, &cfg).unwrap();

        let reloaded = UiSettings::from_config(&load_config(&store).unwrap(), &store);
        assert_eq!(reloaded, settings);
    }

    /// `store.root` is an echo of the running server, not a writable setting.
    #[test]
    fn store_root_is_read_only() {
        let (_dir, store) = temp_store();
        let mut cfg = ModeldConfig::default();
        let mut settings = UiSettings::from_config(&cfg, &store);
        settings.store.root = "D:\\somewhere\\else".to_string();
        settings.apply_to(&mut cfg).unwrap();
        assert_eq!(UiSettings::from_config(&cfg, &store).store.root, store.display().to_string());
    }

    #[test]
    fn rejects_out_of_range_values() {
        let (_dir, store) = temp_store();
        let mut cfg = ModeldConfig::default();
        let mut settings = UiSettings::from_config(&cfg, &store);

        settings.ui.port = 0;
        assert!(settings.apply_to(&mut cfg).is_err());

        settings = UiSettings::from_config(&cfg, &store);
        settings.gc.quarantine_days = 0;
        assert!(settings.apply_to(&mut cfg).is_err());

        settings = UiSettings::from_config(&cfg, &store);
        settings.ui.host = "   ".to_string();
        assert!(settings.apply_to(&mut cfg).is_err());
    }
}
