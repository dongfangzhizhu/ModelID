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

/// `GET /api/v1/stats` — library figures computed from the database.
pub async fn get_stats(State(state): State<AppState>) -> ApiResult<Json<StatsResponse>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    let total_models = db.count_models().unwrap_or(0);
    let total_size_bytes = db.total_size().unwrap_or(0);

    // Real figures, computed from the alias table (A7):
    // - `deduped_saved_bytes`: bytes occupied by links, i.e. duplicates that were
    //   avoided. Zero until something is actually deduplicated.
    // - `duplicate_waste_bytes`: bytes still wasted by *independent* copies.
    //   Links cost nothing, so this falls after a dedup instead of rising.
    // - `last_scan_at`: the newest `path_index.last_seen`.
    let deduped_saved_bytes = db.deduped_saved_bytes().unwrap_or(0);
    let duplicate_waste_bytes = db.duplicate_waste_bytes().unwrap_or(0);
    let last_scan_at = db.last_scan_at().unwrap_or(None);

    // Compute duplicate stats: use DB to find models with 2+ aliases
    let models = db.list_models(None).unwrap_or_default();
    let mut duplicate_groups = 0usize;
    let mut frontend_map: std::collections::HashMap<String, FrontendStat> = Default::default();

    for model in &models {
        let aliases = db.get_aliases_for_model(&model.blake3_hash).unwrap_or_default();

        if aliases.len() >= 2 {
            duplicate_groups += 1;
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
        deduped_saved_bytes,
        frontends,
        last_scan_at,
        daemon_version: env!("CARGO_PKG_VERSION"),
    }))
}
#[cfg(test)]
mod tests {
    use modeld_core::db::{AliasType, Frontend};
    use modeld_core::{Blake3Hash, Database};

    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Database::open(&dir.path().join("modeld.db")).unwrap();
        (dir, db)
    }

    /// A hard link costs no disk space, so it must count as *saved* bytes and
    /// must not appear in the waste figure. This is the regression that made
    /// "wasted space" grow after a successful dedup.
    #[test]
    fn waste_ignores_hardlinks() {
        let (_dir, mut db) = temp_db();
        let hash = Blake3Hash::from_hex(&"a".repeat(64)).unwrap();
        db.insert_or_update_model(&hash, 1000, None, None, None, None).unwrap();

        db.insert_alias(&hash, "/a/model.bin", Frontend::User, AliasType::Original).unwrap();
        db.insert_alias(&hash, "/b/model.bin", Frontend::User, AliasType::Original).unwrap();

        // Two independent copies: one of them is pure waste.
        assert_eq!(db.duplicate_waste_bytes().unwrap(), 1000);
        assert_eq!(db.deduped_saved_bytes().unwrap(), 0);

        // After dedup the second path is a hard link to the first.
        db.delete_alias("/b/model.bin").unwrap();
        db.insert_alias(&hash, "/b/model.bin", Frontend::User, AliasType::Hardlink).unwrap();

        assert_eq!(db.duplicate_waste_bytes().unwrap(), 0, "links are not waste");
        assert_eq!(db.deduped_saved_bytes().unwrap(), 1000, "the link saved a copy");
    }

    /// `last_scan_at` comes from `path_index` and is `None` on an empty store.
    #[test]
    fn last_scan_at_follows_the_path_index() {
        let (_dir, mut db) = temp_db();
        assert_eq!(db.last_scan_at().unwrap(), None);

        let hash = Blake3Hash::from_hex(&"b".repeat(64)).unwrap();
        db.insert_or_update_model(&hash, 10, None, None, None, None).unwrap();
        db.upsert_path_index("/x/model.bin", &hash, 10, 0, 0, 0).unwrap();

        let stamp = db.last_scan_at().unwrap().expect("a scan timestamp");
        assert!(!stamp.is_empty());
    }
}
