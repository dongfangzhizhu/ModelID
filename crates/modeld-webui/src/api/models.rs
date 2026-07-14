use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Deserialize, Default)]
pub struct ModelQuery {
    pub q: Option<String>,
    #[serde(rename = "type")]
    pub model_type: Option<String>,
    pub arch: Option<String>,
    pub page: Option<usize>,
    pub per_page: Option<usize>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub orphan: Option<bool>,
}

#[derive(Serialize)]
pub struct ModelItem {
    pub blake3_hash: String,
    pub name: String,
    pub format: Option<String>,
    pub arch: Option<String>,
    #[serde(rename = "type")]
    pub model_type: Option<String>,
    pub size_bytes: i64,
    pub ref_count: usize,
    pub is_orphan: bool,
    pub frontends: Vec<String>,
    pub paths: Vec<String>,
    pub created_at: String,
    pub last_seen: String,
}

#[derive(Serialize)]
pub struct ModelsResponse {
    pub items: Vec<ModelItem>,
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
}

pub async fn list_models(
    State(state): State<AppState>,
    Query(params): Query<ModelQuery>,
) -> ApiResult<Json<ModelsResponse>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    let all_models = db.list_models(None).unwrap_or_default();
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).min(200);

    let mut items: Vec<ModelItem> = all_models
        .iter()
        .filter_map(|m| {
            let aliases = db.get_aliases_for_model(&m.blake3_hash).unwrap_or_default();
            let ref_count = aliases.len();
            let frontends: Vec<String> =
                aliases.iter().map(|a| a.frontend.as_str().to_string()).collect();
            let paths: Vec<String> = aliases.iter().map(|a| a.path.clone()).collect();

            // Derive name from first path
            let name = paths
                .first()
                .and_then(|p| std::path::Path::new(p).file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| m.blake3_hash.as_hex()[..8].to_string());

            // Text search filter
            if let Some(q) = &params.q {
                let q_lower = q.to_lowercase();
                if !name.to_lowercase().contains(&q_lower)
                    && !m.blake3_hash.as_hex().contains(&q_lower)
                {
                    return None;
                }
            }
            // Arch filter
            if let Some(arch) = &params.arch {
                if m.arch.as_deref() != Some(arch.as_str()) {
                    return None;
                }
            }
            // Type/category filter
            if let Some(t) = &params.model_type {
                if m.category.as_deref() != Some(t.as_str()) {
                    return None;
                }
            }
            // Orphan filter
            if params.orphan == Some(true) && ref_count > 0 {
                return None;
            }

            Some(ModelItem {
                blake3_hash: m.blake3_hash.as_hex().to_string(),
                name,
                format: m.format.clone(),
                arch: m.arch.clone(),
                model_type: m.category.clone(),
                size_bytes: m.size_bytes,
                ref_count,
                is_orphan: ref_count == 0,
                frontends,
                paths,
                created_at: m.created_at.to_rfc3339(),
                last_seen: m.last_seen.to_rfc3339(),
            })
        })
        .collect();

    // Sorting
    match params.sort.as_deref() {
        Some("size") => items.sort_by_key(|b| std::cmp::Reverse(b.size_bytes)),
        Some("name") => items.sort_by(|a, b| a.name.cmp(&b.name)),
        Some("ref_count") => items.sort_by_key(|b| std::cmp::Reverse(b.ref_count)),
        _ => items.sort_by(|a, b| b.last_seen.cmp(&a.last_seen)),
    }
    if params.order.as_deref() == Some("asc") {
        items.reverse();
    }

    let total = items.len();
    let start = (page - 1) * per_page;
    let items = items.into_iter().skip(start).take(per_page).collect();

    Ok(Json(ModelsResponse { items, total, page, per_page }))
}

#[derive(Serialize)]
pub struct AliasDetail {
    pub path: String,
    pub frontend: String,
    pub alias_type: String,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct ModelDetail {
    #[serde(flatten)]
    pub item: ModelItem,
    pub aliases: Vec<AliasDetail>,
}

pub async fn get_model(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> ApiResult<Json<ModelDetail>> {
    let db = state.db.lock().map_err(|_| crate::error::ApiError::internal("DB lock poisoned"))?;

    let hash_obj = modeld_core::Blake3Hash::from_hex(&hash)
        .map_err(|_| crate::error::ApiError::bad_request("Invalid hash format"))?;

    let model = db
        .get_model(&hash_obj)
        .unwrap_or(None)
        .ok_or_else(|| crate::error::ApiError::not_found("Model not found"))?;

    let aliases = db.get_aliases_for_model(&hash_obj).unwrap_or_default();
    let ref_count = aliases.len();
    let frontends: Vec<String> = aliases.iter().map(|a| a.frontend.as_str().to_string()).collect();
    let paths: Vec<String> = aliases.iter().map(|a| a.path.clone()).collect();

    let name = paths
        .first()
        .and_then(|p| std::path::Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| hash[..8].to_string());

    let alias_details: Vec<AliasDetail> = aliases
        .iter()
        .map(|a| AliasDetail {
            path: a.path.clone(),
            frontend: a.frontend.as_str().to_string(),
            alias_type: a.alias_type.as_str().to_string(),
            created_at: a.created_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(ModelDetail {
        item: ModelItem {
            blake3_hash: model.blake3_hash.as_hex().to_string(),
            name,
            format: model.format,
            arch: model.arch,
            model_type: model.category,
            size_bytes: model.size_bytes,
            ref_count,
            is_orphan: ref_count == 0,
            frontends,
            paths,
            created_at: model.created_at.to_rfc3339(),
            last_seen: model.last_seen.to_rfc3339(),
        },
        aliases: alias_details,
    }))
}
