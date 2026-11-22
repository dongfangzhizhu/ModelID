use axum::{extract::State, response::Json};
use serde::Serialize;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Serialize)]
pub struct StatsResponse {
    pub total_models: i64,
    /// Every alias path counted as a full file (B6).
    pub logical_bytes: u64,
    /// Unique content actually stored (B6) — compare with `du -sb <store>`.
    pub physical_bytes: u64,
    /// Kept for existing clients: same meaning as `physical_bytes`.
    pub total_size_bytes: u64,
    /// Content reachable through 2+ paths. Same meaning as before B6 (it used
    /// to be `aliases.len() >= 2`), so existing dashboards do not shift.
    pub duplicate_groups: usize,
    /// Groups where a dedup would still reclaim bytes (2+ independent copies).
    /// This is the actionable number; `duplicate_groups` includes groups that
    /// are already fully linked and therefore waste nothing.
    pub waste_groups: usize,
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
///
/// Two aggregate queries, no per-model loop (B6). The previous version walked
/// every model and issued `get_aliases_for_model` for each, which is O(N+M)
/// round trips and made a large library's dashboard the slowest page.
pub async fn get_stats(State(state): State<AppState>) -> ApiResult<Json<StatsResponse>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    // A failed aggregate must not be rendered as "0 bytes" — that is exactly
    // the silent degradation the dashboard is supposed to expose. Surface it.
    let storage = db
        .storage_stats()
        .map_err(|e| crate::error::ApiError::internal(format!("stats aggregate failed: {e}")))?;
    let last_scan_at = db
        .last_scan_at()
        .map_err(|e| crate::error::ApiError::internal(format!("last_scan_at failed: {e}")))?;

    let frontends: Vec<FrontendStat> = db
        .frontend_stats()
        .map_err(|e| crate::error::ApiError::internal(format!("frontend_stats failed: {e}")))?
        .into_iter()
        .map(|row| FrontendStat {
            name: row.frontend,
            model_count: row.model_count,
            size_bytes: row.size_bytes,
            duplicate_bytes: row.duplicate_bytes,
        })
        .collect();

    Ok(Json(StatsResponse {
        total_models: storage.model_count,
        logical_bytes: storage.logical_bytes,
        physical_bytes: storage.physical_bytes,
        total_size_bytes: storage.physical_bytes,
        duplicate_groups: storage.multi_path_groups,
        waste_groups: storage.duplicate_groups,
        duplicate_waste_bytes: storage.duplicate_waste_bytes,
        deduped_saved_bytes: storage.deduped_saved_bytes,
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

    /// The endpoint must expose logical and physical size separately (B6).
    /// Before this, `total_size_bytes` was the only figure, so a dashboard
    /// could not tell a 2 GB library from 1 GB stored twice.
    #[tokio::test]
    async fn stats_endpoint_separates_logical_from_physical() {
        use crate::state::AppState;
        use axum::extract::State;
        use axum::response::Json;
        use modeld_core::db::{AliasType, Frontend};

        let dir = tempfile::TempDir::new().unwrap();
        let mut db = Database::open(&dir.path().join("modeld.db")).unwrap();

        let hash = Blake3Hash::from_hex(&"c".repeat(64)).unwrap();
        db.insert_or_update_model(&hash, 1000, None, None, None, None).unwrap();
        db.insert_alias(&hash, "/a/model.bin", Frontend::User, AliasType::Original).unwrap();
        db.insert_alias(&hash, "/b/model.bin", Frontend::User, AliasType::Hardlink).unwrap();

        let (state, _rx) = AppState::new(db, dir.path().to_path_buf(), None, None);
        let Json(resp) = super::get_stats(State(state)).await.expect("stats must succeed");

        assert_eq!(resp.total_models, 1);
        assert_eq!(resp.logical_bytes, 2000, "two paths, each a full file");
        assert_eq!(resp.physical_bytes, 1000, "one unique content on disk");
        assert_eq!(resp.total_size_bytes, 1000, "legacy field mirrors physical");
        assert_eq!(resp.deduped_saved_bytes, 1000, "the hard link saved a copy");
        assert_eq!(resp.duplicate_waste_bytes, 0);
        assert_eq!(resp.duplicate_groups, 1, "two paths is still one group");
        assert_eq!(resp.waste_groups, 0, "but nothing is recoverable any more");
        assert_eq!(resp.frontends.len(), 1);
        assert_eq!(resp.frontends[0].model_count, 1);
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
