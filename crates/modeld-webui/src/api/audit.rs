//! `GET /api/v1/audit` — 审计日志查询（C2）。
//!
//! 支持 since/until（ISO 8601 或相对时长）、op、actor、outcome、
//! resource_contains、limit；返回按时间升序的记录数组。
//! 用于回答「上周谁删了什么」等事后追溯问题。

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use modeld_core::{AuditLogger, AuditQuery};
use serde::Deserialize;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct AuditParams {
    pub since: Option<String>,
    pub until: Option<String>,
    pub op: Option<String>,
    pub actor: Option<String>,
    pub outcome: Option<String>,
    pub resource_contains: Option<String>,
    pub limit: Option<usize>,
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(iso) = DateTime::parse_from_rfc3339(value) {
        return Some(iso.with_timezone(&Utc));
    }
    let (num, unit) = if let Some(rest) = value.strip_suffix('d') {
        (rest, 86400i64)
    } else if let Some(rest) = value.strip_suffix('h') {
        (rest, 3600i64)
    } else {
        (value.strip_suffix('m')?, 60i64)
    };
    num.parse::<i64>().ok().map(|n| Utc::now() - Duration::seconds(n.saturating_mul(unit)))
}

pub async fn query_audit(
    State(state): State<AppState>,
    Query(params): Query<AuditParams>,
) -> ApiResult<Json<serde_json::Value>> {
    let invalid = [params.since.as_deref(), params.until.as_deref()]
        .into_iter()
        .flatten()
        .any(|v| parse_time(v).is_none());
    if invalid {
        return Err(crate::error::ApiError::bad_request(
            "invalid since/until: expected ISO 8601 or e.g. 7d / 2h / 30m",
        ));
    }

    let query = AuditQuery {
        since: params.since.as_deref().and_then(parse_time),
        until: params.until.as_deref().and_then(parse_time),
        op: params.op,
        actor_ip: params.actor,
        outcome: params.outcome,
        resource_contains: params.resource_contains,
        limit: params.limit,
    };
    let entries = AuditLogger::new(&state.store_path)
        .query(&query)
        .map_err(|e| crate::error::ApiError::internal(format!("could not read audit log: {e}")))?;
    Ok(Json(serde_json::json!({ "entries": entries, "count": entries.len() })))
}
