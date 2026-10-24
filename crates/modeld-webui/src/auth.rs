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
        .map(|s| s.trim().to_owned())
        // Browsers cannot attach headers to a WebSocket handshake, so `/ws`
        // additionally accepts `?token=…`. Kept to that path only, so tokens do
        // not end up in URLs (and thus logs / browser history) for normal calls.
        .or_else(|| ws_query_token(&request));

    match provided_token {
        Some(t) if tokens_equal(&t, &required_token) => next.run(request).await,
        _ => {
            // DO NOT log the provided or required token values.
            (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
        }
    }
}

/// Extract `?token=…` from a WebSocket upgrade request.
fn ws_query_token(request: &Request) -> Option<String> {
    if request.uri().path() != "/ws" {
        return None;
    }
    request.uri().query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if key != "token" {
            return None;
        }
        let decoded = percent_decode(value);
        if decoded.is_empty() {
            None
        } else {
            Some(decoded)
        }
    })
}

/// Minimal `application/x-www-form-urlencoded` value decoder (`%XX` and `+`).
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Constant-time equality so a wrong token cannot be guessed byte by byte.
fn tokens_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn ws_request(uri: &str) -> Request {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    #[test]
    fn test_ws_query_token_is_read_only_for_the_ws_path() {
        assert_eq!(ws_query_token(&ws_request("/ws?token=abc")).as_deref(), Some("abc"));
        assert_eq!(
            ws_query_token(&ws_request("/ws?foo=1&token=abc&bar=2")).as_deref(),
            Some("abc")
        );
        assert_eq!(ws_query_token(&ws_request("/ws?foo=1")), None);
        // Ordinary API routes must never accept a token from the URL.
        assert_eq!(ws_query_token(&ws_request("/api/v1/models?token=abc")), None);
    }

    #[test]
    fn test_percent_decode() {
        assert_eq!(percent_decode("abc"), "abc");
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("trailing%"), "trailing%");
    }

    #[test]
    fn test_tokens_equal() {
        assert!(tokens_equal("secret", "secret"));
        assert!(!tokens_equal("secret", "secreT"));
        assert!(!tokens_equal("secret", "secret!"));
        assert!(!tokens_equal("", "x"));
        assert!(tokens_equal("", ""));
    }
}
