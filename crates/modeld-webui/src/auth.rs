//! Bearer-token authentication middleware for the modeld Web UI server.
//!
//! When `state.auth_token` is set, every request to protected routes must
//! carry an `Authorization: Bearer <token>` header.  Invalid or missing
//! tokens receive a `401 Unauthorized` response.
//!
//! **Security**: the token value is never logged anywhere in this file.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::state::AppState;

/// Axum middleware that enforces bearer-token authentication.
///
/// If `state.auth_token` is `None` or empty the middleware is a transparent
/// pass-through — all requests proceed without checking credentials.
pub async fn require_bearer_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // Authentication disabled — pass through.
    let required_token = match state.auth_token.as_deref() {
        Some(t) if !t.is_empty() => t.to_owned(),
        _ => return next.run(request).await,
    };

    // Extract the token from the `Authorization: Bearer <token>` header.
    let provided_token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim().to_owned());

    match provided_token {
        Some(t) if t == required_token => next.run(request).await,
        _ => {
            // DO NOT log the provided or required token values.
            (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
        }
    }
}
