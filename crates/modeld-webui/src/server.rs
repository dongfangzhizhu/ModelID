use anyhow::Result;
use axum::{
    extract::{Request, State},
    http::{header, HeaderName, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::sync::Arc;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::api;
use crate::auth::require_bearer_token;
use crate::state::AppState;
use crate::static_files::static_handler;
use crate::ws::ws_handler;

/// Configuration for the Web UI server.
#[derive(Clone)]
pub struct WebUiConfig {
    pub host: String,
    pub port: u16,
    pub open_browser: bool,
}

impl Default for WebUiConfig {
    fn default() -> Self {
        Self { host: "127.0.0.1".to_string(), port: 8234, open_browser: false }
    }
}

/// `GET /metrics` — Prometheus text format metrics (unauthenticated by design
/// so scraping tools like prometheus-server don't need bearer tokens; restrict
/// via firewall if needed in production).
async fn metrics_handler(State(state): State<AppState>) -> Response {
    let body = state.metrics.render_prometheus();
    ([(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], body)
        .into_response()
}

/// Custom header that browser clients must send on state-changing requests.
///
/// A cross-origin page cannot set it without a successful CORS preflight, so
/// requiring it (together with the origin check) blocks CSRF.
pub const CSRF_HEADER: &str = "x-modeld-csrf";

/// Origins the UI may legitimately be served from.
///
/// The embedded UI is same-origin, so this list only matters for CORS and for
/// rejecting cross-site writes; loopback aliases are always included.
pub fn allowed_origins(host: &str, port: u16) -> Vec<String> {
    let mut origins = vec![
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://[::1]:{port}"),
    ];
    if !host.is_empty() && host != "0.0.0.0" && host != "::" && host != "[::]" {
        origins.push(format!("http://{host}:{port}"));
    }
    origins.sort();
    origins.dedup();
    origins
}

fn origin_allowed(allowed: &[String], origin: &HeaderValue) -> bool {
    let value = origin.as_bytes();
    allowed.iter().any(|o| o.as_bytes() == value)
}

/// Reject cross-site state changes.
///
/// Same-origin browser requests carry an `Origin` header; those must come
/// from an allowed origin *and* carry the `X-Modeld-CSRF` header. Non-browser
/// clients (curl, scripts, the CLI) send no `Origin` and are unaffected —
/// they are not reachable by a malicious web page.
async fn require_csrf_guard(allowed: Arc<Vec<String>>, request: Request, next: Next) -> Response {
    let mutating =
        matches!(*request.method(), Method::POST | Method::PUT | Method::PATCH | Method::DELETE);
    if !mutating {
        return next.run(request).await;
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        if !origin_allowed(&allowed, origin) {
            return (StatusCode::FORBIDDEN, "cross-origin write rejected").into_response();
        }
        if request.headers().get(CSRF_HEADER).is_none() {
            return (StatusCode::FORBIDDEN, "missing X-Modeld-CSRF header").into_response();
        }
    }
    next.run(request).await
}

/// Build the router with CORS, CSRF and auth middleware applied.
///
/// Split out from `run` so tests can drive it through `tower::ServiceExt::oneshot`
/// without binding a port.
pub fn build_router(state: AppState, host: &str, port: u16) -> Router {
    let origins = Arc::new(allowed_origins(host, port));

    let csrf = {
        let origins = Arc::clone(&origins);
        move |request: Request, next: Next| {
            let origins = Arc::clone(&origins);
            async move { require_csrf_guard(origins, request, next).await }
        }
    };

    let cors = {
        let origins = Arc::clone(&origins);
        CorsLayer::new()
            .allow_origin(AllowOrigin::predicate(move |origin, _parts| {
                origin_allowed(&origins, origin)
            }))
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                HeaderName::from_static(CSRF_HEADER),
            ])
    };

    // Protected API routes — require bearer token when configured.
    let protected_api = api::routes()
        .route_layer(middleware::from_fn_with_state(state.clone(), require_bearer_token));

    // WebSocket also requires the same bearer-token middleware.
    let protected_ws = axum::Router::new()
        .route("/ws", get(ws_handler))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_bearer_token));

    Router::new()
        // Prometheus metrics (not behind auth — restrict at network level)
        .route("/metrics", get(metrics_handler))
        // Protected REST API routes (bearer token when auth_token is set)
        .nest("/api/v1", protected_api)
        // Protected WebSocket endpoint
        .merge(protected_ws)
        // Static file fallback (serves embedded UI)
        .fallback(static_handler)
        // Provide shared state to ALL routes
        .with_state(state)
        // Innermost of the two: reject cross-site writes before they reach a handler.
        .layer(middleware::from_fn(csrf))
        // Outermost: answers preflight requests and sets Access-Control-* only
        // for our own origins (previously: every origin).
        .layer(cors)
        .layer(CompressionLayer::new())
}

/// Build and run the axum HTTP server.
pub async fn run(state: AppState, config: WebUiConfig) -> Result<()> {
    let app = build_router(state, &config.host, config.port);

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    let url = format!("http://{}", addr);
    tracing::info!("modeld Web UI listening on {}", url);
    println!("🌐  modeld Web UI: {}", url);

    if config.open_browser {
        let _ = open_browser(&url);
    }

    axum::serve(listener, app).await?;
    Ok(())
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd").args(["/c", "start", url]).spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(url).spawn()?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(url).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    fn test_state() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = modeld_core::Database::open(&dir.path().join("modeld.db")).unwrap();
        let (state, _rx) = AppState::new(db, dir.path().to_path_buf(), None, None);
        (state, dir)
    }

    fn post(path: &str) -> axum::http::request::Builder {
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
    }

    async fn status_of(app: Router, request: Request<Body>) -> StatusCode {
        app.oneshot(request).await.unwrap().status()
    }

    #[test]
    fn test_allowed_origins_include_loopback_aliases() {
        let origins = allowed_origins("127.0.0.1", 8234);
        assert!(origins.contains(&"http://127.0.0.1:8234".to_string()));
        assert!(origins.contains(&"http://localhost:8234".to_string()));

        // A wildcard bind is not a browsable origin and must not be listed.
        let origins = allowed_origins("0.0.0.0", 8234);
        assert!(!origins.iter().any(|o| o.contains("0.0.0.0")));

        // A concrete LAN host is its own origin.
        let origins = allowed_origins("192.168.1.5", 8234);
        assert!(origins.contains(&"http://192.168.1.5:8234".to_string()));
    }

    #[test]
    fn test_origin_allowed_rejects_cross_site() {
        let origins = allowed_origins("127.0.0.1", 8234);
        assert!(origin_allowed(&origins, &HeaderValue::from_static("http://127.0.0.1:8234")));
        assert!(!origin_allowed(&origins, &HeaderValue::from_static("http://evil.example")));
        assert!(!origin_allowed(&origins, &HeaderValue::from_static("http://127.0.0.1:9999")));
    }

    #[tokio::test]
    async fn test_cross_origin_write_without_csrf_header_is_rejected() {
        let (state, _dir) = test_state();
        let app = build_router(state, "127.0.0.1", 8234);

        let request = post("/api/v1/dedup/apply")
            .header(header::ORIGIN, "http://evil.example")
            .body(Body::from("{}"))
            .unwrap();
        assert_eq!(status_of(app, request).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_allowed_origin_with_csrf_header_passes_the_guard() {
        let (state, _dir) = test_state();
        let app = build_router(state, "127.0.0.1", 8234);

        let request = post("/api/v1/dedup/apply")
            .header(header::ORIGIN, "http://127.0.0.1:8234")
            .header(CSRF_HEADER, "1")
            .body(Body::from("{}"))
            .unwrap();
        // Whatever the handler decides, the CSRF guard must not answer 403.
        assert_ne!(status_of(app, request).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_allowed_origin_without_csrf_header_is_rejected() {
        let (state, _dir) = test_state();
        let app = build_router(state, "127.0.0.1", 8234);

        let request = post("/api/v1/dedup/apply")
            .header(header::ORIGIN, "http://127.0.0.1:8234")
            .body(Body::from("{}"))
            .unwrap();
        assert_eq!(status_of(app, request).await, StatusCode::FORBIDDEN);
    }

    fn authed_state(token: &str) -> (AppState, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = modeld_core::Database::open(&dir.path().join("modeld.db")).unwrap();
        let (state, _rx) =
            AppState::new(db, dir.path().to_path_buf(), Some(token.to_string()), None);
        (state, dir)
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().method(Method::GET).uri(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn test_api_requires_the_bearer_token_when_configured() {
        let (state, _dir) = authed_state("tok");
        let app = build_router(state, "127.0.0.1", 8234);

        assert_eq!(status_of(app.clone(), get("/api/v1/stats")).await, StatusCode::UNAUTHORIZED);

        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/stats")
            .header(header::AUTHORIZATION, "Bearer wrong")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(app.clone(), request).await, StatusCode::UNAUTHORIZED);

        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/stats")
            .header(header::AUTHORIZATION, "Bearer tok")
            .body(Body::empty())
            .unwrap();
        assert_ne!(status_of(app, request).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_api_rejects_a_token_in_the_query_string() {
        // `?token=` is a WebSocket-only affordance; it must not work for API calls.
        let (state, _dir) = authed_state("tok");
        let app = build_router(state, "127.0.0.1", 8234);
        assert_eq!(status_of(app, get("/api/v1/stats?token=tok")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_reads_are_not_csrf_gated() {
        let (state, _dir) = test_state();
        let app = build_router(state, "127.0.0.1", 8234);

        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/stats")
            .header(header::ORIGIN, "http://evil.example")
            .body(Body::empty())
            .unwrap();
        assert_ne!(status_of(app, request).await, StatusCode::FORBIDDEN);
    }
}
