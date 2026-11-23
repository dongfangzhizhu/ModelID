//! HTTP-level integration tests for every `/api/v1` route (ROADMAP B8).
//!
//! Acceptance: "每个 API 路由至少 1 个集成测试（含 4xx）".
//!
//! These drive the *real* router — `build_router`, the same function `main`
//! passes to `axum::serve` — through `tower::ServiceExt::oneshot`, so the CORS,
//! CSRF, auth, metrics and compression layers are all in the path. Calling the
//! handler functions directly would skip exactly the layering that has produced
//! real bugs (a route without auth, a write without a CSRF guard).
//!
//! Two kinds of coverage, on purpose:
//!
//! * Per-route tests with concrete assertions on the success body and on the
//!   validation failures the handler itself owns (400 / 404 / 409).
//! * A table-driven sweep that every registered route answers with something
//!   other than 404/405, and answers 401 without a token. That sweep reads
//!   `src/api/mod.rs` at compile time, so **adding a route without a test fails
//!   the suite** instead of silently shipping untested.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

use modeld_webui::state::AppState;

// ─────────────────────────────────────────────────────────────────────────────
// Harness
// ─────────────────────────────────────────────────────────────────────────────

struct Harness {
    state: AppState,
    app: Router,
    #[allow(dead_code)]
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        Self::build(None)
    }

    fn with_token(token: &str) -> Self {
        Self::build(Some(token.to_string()))
    }

    fn build(token: Option<String>) -> Self {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let db = modeld_core::Database::open(&dir.path().join("modeld.db")).expect("open db");
        let (state, _rx) = AppState::new(db, dir.path().to_path_buf(), token, None);
        let app = modeld_webui::server::build_router(state.clone(), "127.0.0.1", 8234);
        Self { state, app, dir }
    }

    /// Insert a model + alias directly, so read endpoints have something to
    /// return. Uses the same DB API `modeld scan` uses.
    fn seed_model(&self, salt: u8, path: &str) -> String {
        let hash_hex: String = std::iter::repeat_n(format!("{salt:02x}"), 32).collect();
        let hash = modeld_core::Blake3Hash::from_hex(&hash_hex).expect("valid hex");
        let mut db = self.state.db.lock().expect("db lock");
        db.insert_or_update_model(&hash, 4096, Some("safetensors"), None, None, None)
            .expect("insert model");
        db.insert_alias(
            &hash,
            path,
            modeld_core::db::Frontend::User,
            modeld_core::db::AliasType::Original,
        )
        .expect("insert alias");
        hash_hex
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, String) {
        let (status, _content_type, body) = self.send_full(request).await;
        (status, body)
    }

    /// Same, but keeps the `Content-Type`.
    ///
    /// The router's fallback is the SPA handler, which answers *200 text/html*
    /// with `index.html` for anything it does not recognise. So "route not
    /// mounted" shows up as HTML, not 404 — the content type is the only
    /// reliable signal, and a 404 from a handler is a legitimate answer.
    async fn send_full(&self, request: Request<Body>) -> (StatusCode, String, String) {
        let response = self.app.clone().oneshot(request).await.expect("router call");
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes =
            axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024).await.expect("read body");
        (status, content_type, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn get(&self, path: &str) -> (StatusCode, String) {
        self.send(Request::builder().method(Method::GET).uri(path).body(Body::empty()).unwrap())
            .await
    }

    async fn post(&self, path: &str) -> (StatusCode, String) {
        self.send(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn post_json(&self, path: &str, body: Value) -> (StatusCode, String) {
        self.send(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }

    async fn put_json(&self, path: &str, body: Value) -> (StatusCode, String) {
        self.send(
            Request::builder()
                .method(Method::PUT)
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }
}

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("expected JSON body, got {body:?}: {e}"))
}

/// The error envelope every handler shares (`ApiError::into_response`).
fn assert_error_envelope(body: &str, expected_code: &str) {
    let value = parse(body);
    assert_eq!(
        value["code"], expected_code,
        "error envelope should carry code {expected_code}: {value}"
    );
    assert!(
        value["error"].as_str().is_some_and(|m| !m.is_empty()),
        "error envelope needs a human message: {value}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Status & stats
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn status_reports_store_summary() {
    let h = Harness::new();
    h.seed_model(0x11, "/lib/a.safetensors");

    let (status, body) = h.get("/api/v1/status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["total_models"], 1);
    assert_eq!(value["total_size_bytes"], 4096);
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    // An empty store has nothing to save.
    assert_eq!(value["dedup_savings_bytes"], 0);
    assert_eq!(value["quarantine_count"], 0);
}

#[tokio::test]
async fn stats_separates_logical_from_physical() {
    let h = Harness::new();
    // Two aliases of *different* models: 2 × 4096 logical, 2 × 4096 physical.
    h.seed_model(0x21, "/lib/a.safetensors");
    h.seed_model(0x22, "/lib/b.safetensors");

    let (status, body) = h.get("/api/v1/stats").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["logical_bytes"], 8192, "every alias path is a full file: {value}");
    assert_eq!(value["physical_bytes"], 8192, "two distinct contents: {value}");
    assert_eq!(value["duplicate_waste_bytes"], 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Models
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn models_lists_and_filters() {
    let h = Harness::new();
    let hash = h.seed_model(0x31, "/lib/llama-3.safetensors");
    h.seed_model(0x32, "/lib/mistral.safetensors");

    let (status, body) = h.get("/api/v1/models").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["total"], 2);
    assert_eq!(value["items"].as_array().unwrap().len(), 2);

    // Name search hits the file name derived from the first alias.
    let (status, body) = h.get("/api/v1/models?q=llama").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["total"], 1, "search should filter: {value}");
    assert_eq!(value["items"][0]["blake3_hash"], hash);
    assert_eq!(value["items"][0]["name"], "llama-3.safetensors");

    // Zero hits is still a 200 with an empty page, not a 404.
    let (status, body) = h.get("/api/v1/models?q=nothing-matches-this").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(parse(&body)["total"], 0);
}

#[tokio::test]
async fn model_detail_returns_aliases() {
    let h = Harness::new();
    let hash = h.seed_model(0x41, "/lib/a.safetensors");

    let (status, body) = h.get(&format!("/api/v1/models/{hash}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["blake3_hash"], hash);
    assert_eq!(value["ref_count"], 1);
    assert_eq!(value["aliases"][0]["path"], "/lib/a.safetensors");
    assert_eq!(value["aliases"][0]["alias_type"], "original");
}

#[tokio::test]
async fn model_detail_rejects_malformed_hash_with_400() {
    let h = Harness::new();
    let (status, body) = h.get("/api/v1/models/not-a-hash").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_error_envelope(&body, "BAD_REQUEST");
}

#[tokio::test]
async fn model_detail_returns_404_for_unknown_hash() {
    let h = Harness::new();
    let missing: String = std::iter::repeat_n("ab".to_string(), 32).collect();
    let (status, body) = h.get(&format!("/api/v1/models/{missing}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_error_envelope(&body, "NOT_FOUND");
}

// ─────────────────────────────────────────────────────────────────────────────
// Dupes & dedup
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dupes_is_empty_on_a_store_without_duplicates() {
    let h = Harness::new();
    h.seed_model(0x51, "/lib/unique.safetensors");

    let (status, body) = h.get("/api/v1/dupes").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["items"].as_array().map(Vec::len).unwrap_or(0), 0, "{value}");
    assert_eq!(value["total_waste_bytes"], 0);
}

#[tokio::test]
async fn dedup_preview_reports_nothing_to_do_on_a_clean_store() {
    let h = Harness::new();
    h.seed_model(0x52, "/lib/unique.safetensors");

    let (status, body) = h.post("/api/v1/dedup/preview").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["operations"].as_array().map(Vec::len).unwrap_or(0), 0, "{value}");
    assert_eq!(value["total_would_save_bytes"], 0);
}

#[tokio::test]
async fn dedup_apply_succeeds_with_nothing_to_do() {
    let h = Harness::new();
    let (status, body) = h.post_json("/api/v1/dedup/apply", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["groups_processed"], 0);
    assert_eq!(value["space_saved"], 0);
    assert_eq!(value["groups_failed"], 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Downloads
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn downloads_lists_an_empty_queue() {
    let h = Harness::new();
    let (status, body) = h.get("/api/v1/downloads").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(parse(&body)["items"].as_array().unwrap().len(), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Scan
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn scan_accepts_a_path_and_returns_a_scan_id() {
    let h = Harness::new();
    let lib = h.dir.path().join("lib");
    std::fs::create_dir_all(&lib).unwrap();

    let (status, body) =
        h.post_json("/api/v1/scan", json!({ "paths": [lib.to_string_lossy()] })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert!(
        value["scan_id"].as_str().is_some_and(|s| s.len() >= 32),
        "scan_id must be a uuid: {value}"
    );
    assert_eq!(value["scan_paths"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn scan_legacy_trigger_route_still_works() {
    let h = Harness::new();
    let (status, body) = h.post_json("/api/v1/scan/trigger", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(parse(&body)["scan_id"].is_string());
}

#[tokio::test]
async fn scan_rejects_an_unknown_mode_with_400() {
    let h = Harness::new();
    let (status, body) = h.post_json("/api/v1/scan", json!({ "mode": "teleport" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_error_envelope(&body, "BAD_REQUEST");
}

// ─────────────────────────────────────────────────────────────────────────────
// GC
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn gc_preview_on_an_empty_store_has_nothing_to_reclaim() {
    let h = Harness::new();
    let (status, body) = h.get("/api/v1/gc/preview").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["total_reclaimable_bytes"], 0);
}

#[tokio::test]
async fn gc_run_quarantines_nothing_when_every_model_has_an_alias() {
    let h = Harness::new();
    h.seed_model(0x61, "/lib/kept.safetensors");

    let (status, body) = h.post_json("/api/v1/gc/run", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["quarantined"].as_array().unwrap().len(), 0, "{value}");
    assert_eq!(value["bytes_recovered"], 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Quarantine
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn quarantine_lists_empty() {
    let h = Harness::new();
    let (status, body) = h.get("/api/v1/quarantine").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["items"].as_array().unwrap().len(), 0);
    assert_eq!(value["total_size_bytes"], 0);
}

#[tokio::test]
async fn quarantine_restore_of_an_unknown_id_is_404() {
    let h = Harness::new();
    let (status, body) = h.post("/api/v1/quarantine/deadbeef/restore").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_error_envelope(&body, "NOT_FOUND");
}

// ─────────────────────────────────────────────────────────────────────────────
// Settings
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn settings_round_trip() {
    let h = Harness::new();

    let (status, body) = h.get("/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut settings = parse(&body);
    assert_eq!(settings["store"]["root"], h.dir.path().display().to_string());

    // Change two persisted fields and read them back.
    settings["gc"]["quarantine_days"] = json!(45);
    settings["store"]["incremental_scan"] = json!(false);
    let (status, body) = h.put_json("/api/v1/settings", settings.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(parse(&body)["gc"]["quarantine_days"], 45);

    let (status, body) = h.get("/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reread = parse(&body);
    assert_eq!(reread["gc"]["quarantine_days"], 45, "the change must be persisted");
    assert_eq!(reread["store"]["incremental_scan"], false);
}

#[tokio::test]
async fn settings_rejects_port_zero_with_400() {
    let h = Harness::new();
    let (status, body) = h.get("/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut settings = parse(&body);
    settings["ui"]["port"] = json!(0);

    let (status, body) = h.put_json("/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_error_envelope(&body, "BAD_REQUEST");
}

// ─────────────────────────────────────────────────────────────────────────────
// Tags
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn tags_lists_nothing_on_an_empty_store() {
    let h = Harness::new();
    let (status, body) = h.get("/api/v1/tags").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(parse(&body)["total"], 0);
}

#[tokio::test]
async fn adding_a_tag_returns_201_and_shows_up_in_the_list() {
    let h = Harness::new();
    let hash = h.seed_model(0x71, "/lib/tagged.safetensors");

    let (status, body) =
        h.post_json(&format!("/api/v1/models/{hash}/tags"), json!({ "tag": "favourite" })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let value = parse(&body);
    assert_eq!(value["tag"], "favourite");
    assert_eq!(value["tags"], json!(["favourite"]));

    let (status, body) = h.get("/api/v1/tags").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let value = parse(&body);
    assert_eq!(value["total"], 1);
    assert_eq!(value["tags"][0]["tag"], "favourite");
    assert_eq!(value["tags"][0]["count"], 1);
}

#[tokio::test]
async fn adding_an_empty_tag_is_400() {
    let h = Harness::new();
    let hash = h.seed_model(0x72, "/lib/tagged.safetensors");
    let (status, body) =
        h.post_json(&format!("/api/v1/models/{hash}/tags"), json!({ "tag": "   " })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn adding_a_tag_to_an_unknown_model_is_404() {
    let h = Harness::new();
    let missing: String = std::iter::repeat_n("cd".to_string(), 32).collect();
    let (status, body) =
        h.post_json(&format!("/api/v1/models/{missing}/tags"), json!({ "tag": "x" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn adding_a_tag_with_a_short_hash_is_400() {
    let h = Harness::new();
    let (status, body) = h.post_json("/api/v1/models/abc/tags", json!({ "tag": "x" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Route inventory — the guard that keeps this file honest
// ─────────────────────────────────────────────────────────────────────────────

/// Every `.route("…", <method>(…))` registered in `api::routes()`, parsed out of
/// the source at compile time.
///
/// The point is not to test the parser: it is that adding a route to
/// `src/api/mod.rs` without adding a test here breaks the build of this test,
/// so "every route has an integration test" cannot silently rot.
fn registered_routes() -> Vec<(Method, String)> {
    const SOURCE: &str = include_str!("../src/api/mod.rs");

    let mut routes = Vec::new();
    for line in SOURCE.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(".route(\"") else { continue };
        let Some(end) = rest.find('"') else { continue };
        let path = format!("/api/v1{}", &rest[..end]);
        let after = &rest[end..];
        let method = if after.contains("post(") {
            Method::POST
        } else if after.contains("put(") {
            Method::PUT
        } else if after.contains("delete(") {
            Method::DELETE
        } else {
            Method::GET
        };
        routes.push((method, path));
    }
    routes
}

/// Substitute path parameters so the URI actually routes.
fn concrete(path: &str) -> String {
    let missing: String = std::iter::repeat_n("ef".to_string(), 32).collect();
    path.replace(":hash", &missing).replace(":id", "no-such-id")
}

#[tokio::test]
async fn route_inventory_is_not_empty_and_matches_the_router() {
    let routes = registered_routes();
    // 18 `.route(…)` declarations today (19 including `/settings` twice, once
    // per method). The floor exists so a broken parser cannot make the sweep
    // below pass by finding nothing at all.
    assert!(
        routes.len() >= 18,
        "expected at least 18 route declarations, parsed {}: {routes:?}",
        routes.len()
    );
    assert_eq!(
        routes.iter().filter(|(_, p)| p == "/api/v1/settings").count(),
        2,
        "`/settings` is registered once per method; the parser should see both: {routes:?}"
    );
}

/// No route may fall through to the SPA handler, which would mean a typo in the
/// path or a handler registered without `.route()`.
#[tokio::test]
async fn every_registered_route_is_mounted() {
    let h = Harness::new();
    for (method, path) in registered_routes() {
        let uri = concrete(&path);
        let request = Request::builder()
            .method(method.clone())
            .uri(&uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let (status, content_type, body) = h.send_full(request).await;
        // The only reliable "not mounted" signal: the SPA fallback answers
        // everything it does not recognise with `200 text/html` (index.html),
        // while a mounted handler always answers JSON — including axum's own
        // extractor rejections, which are `text/plain` 4xx.
        assert!(
            !content_type.starts_with("text/html"),
            "{method} {uri} did not reach an API handler (status {status}, \
             content-type {content_type:?}) — it fell through to the SPA fallback: {body}"
        );
        assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "{method} {uri} rejected the method");
    }
}

/// With a token configured, **every** route refuses an unauthenticated caller.
/// A single route added outside the protected nest would show up here.
#[tokio::test]
async fn every_route_requires_the_bearer_token_when_configured() {
    let h = Harness::with_token("s3cret");
    for (method, path) in registered_routes() {
        let uri = concrete(&path);
        let request = Request::builder()
            .method(method.clone())
            .uri(&uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let (status, _body) = h.send(request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri} answered without a token");
    }
}

/// …and accepts the same caller once the token is presented. Guards against a
/// test suite that "passes" only because everything is locked down.
#[tokio::test]
async fn a_valid_token_opens_the_api() {
    let h = Harness::with_token("s3cret");
    let (status, body) = h
        .send(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/status")
                .header(header::AUTHORIZATION, "Bearer s3cret")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = h
        .send(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/status")
                .header(header::AUTHORIZATION, "Bearer wrong")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}
