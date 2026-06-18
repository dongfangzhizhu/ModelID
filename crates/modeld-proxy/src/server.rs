//! HTTP proxy server core (Phase 5).
//!
//! Routes (see `docs/proxy-api.md`):
//! - `GET /health`                          → service health (no auth)
//! - `GET /v1/models`                       → list all CAS models
//! - `GET /v1/blobs/{blake3_hash}`          → stream a CAS object (Range-supported)
//! - `GET /v1/hf-proxy/{org}/{repo}/resolve/{revision}/{file}`
//!                                           → HuggingFace-compatible proxy
//!
//! All routes except `/health` pass through the auth + IP middleware.
//! Uses blocking `tiny_http` with a per-connection thread, matching the rest
//! of modeld's synchronous design.
//!
//! Note on ownership: `tiny_http::Request::respond(self, ...)` consumes the
//! request, so every handler takes `Request` by value and moves it into the
//! single `respond` call for that request.

use crate::auth::{check_auth, check_ip};
use crate::config::ProxyConfig;
use crate::range::parse_range;
use anyhow::{anyhow, Result};
use modeld_core::{Blake3Hash, CasStore, Database, Downloader, HfCache};
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tiny_http::{Header, Method, Response, Server, StatusCode};

/// Runtime statistics surfaced via `GET /health` and the CLI.
#[derive(Debug, Clone)]
pub struct ServerStats {
    pub model_count: i64,
    pub total_bytes: i64,
}

/// The proxy server. Construct with [`ProxyServer::new`], run with [`ProxyServer::start`].
pub struct ProxyServer {
    pub config: ProxyConfig,
    pub store_path: PathBuf,
}

impl ProxyServer {
    pub fn new(config: ProxyConfig, store_path: PathBuf) -> Self {
        Self { config, store_path }
    }

    /// Bind and serve HTTP until interrupted (Ctrl+C) or an unrecoverable error.
    pub fn start(&self) -> Result<()> {
        let bind = self.config.bind_addr();
        let server = Server::http(bind.as_str())
            .map_err(|e| anyhow!("Failed to bind proxy server to {}: {}", bind, e))?;
        eprintln!(
            "modeld proxy listening on {} (store: {})",
            bind,
            self.store_path.display()
        );

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_for_handler = shutdown.clone();
        ctrlc::set_handler(move || {
            shutdown_for_handler.store(true, Ordering::SeqCst);
        })
        .ok();

        self.run_loop(server, shutdown)
    }

    /// Serve on an externally-bound Server. Used by tests to bind a random
    /// ephemeral port and retrieve its address before serving starts.
    /// Runs until the `shutdown` flag is set.
    #[doc(hidden)]
    pub fn serve_on(self, server: Server, shutdown: Arc<AtomicBool>) -> Result<()> {
        self.run_loop(server, shutdown)
    }

    fn run_loop(&self, server: Server, shutdown: Arc<AtomicBool>) -> Result<()> {
        let db = open_db(&self.store_path)?;
        let db = Arc::new(Mutex::new(db));
        let cas = Arc::new(CasStore::new(&self.store_path));
        let hf_cache = Arc::new(HfCache::new(&self.store_path));
        let store_for_dl = self.store_path.clone();
        let started_at = Instant::now();

        for request in server.incoming_requests() {
            if shutdown.load(Ordering::SeqCst) {
                eprintln!("shutdown requested, stopping proxy server");
                break;
            }
            let ctx = RequestContext {
                config: self.config.clone(),
                db: db.clone(),
                cas: cas.clone(),
                hf_cache: hf_cache.clone(),
                downloader: Downloader::new(&store_for_dl),
                started_at,
            };
            if let Err(e) = handle_request(request, &ctx) {
                eprintln!("request handler error: {:#}", e);
            }
        }

        Ok(())
    }
}

/// Per-request shared context.
struct RequestContext {
    config: ProxyConfig,
    db: Arc<Mutex<Database>>,
    cas: Arc<CasStore>,
    hf_cache: Arc<HfCache>,
    downloader: Downloader,
    started_at: Instant,
}

/// Handle a single request. The request is consumed by exactly one
/// `request.respond(...)` call along the taken branch.
fn handle_request(request: tiny_http::Request, ctx: &RequestContext) -> Result<()> {
    let url = request.url().to_string();
    let method = request.method().clone();
    let peer = peer_ip(&request);

    // /health is always public.
    if url == "/health" && method == Method::Get {
        let body = json_health(ctx);
        return respond(request, StatusCode(200), json_content_type(), body.as_bytes());
    }

    // Auth + IP gate for everything else.
    if !check_ip(&peer, &ctx.config.network) {
        return respond(request, StatusCode(403), text_content_type(), b"forbidden: ip not allowed");
    }
    let provided_token = bearer_token(&request);
    if !ctx.config.network.allow_anonymous && provided_token.is_none() {
        return respond(
            request,
            StatusCode(401),
            text_content_type(),
            b"unauthorized: anonymous access disabled",
        );
    }
    if !check_auth(provided_token.as_deref(), &ctx.config.auth) {
        return respond(
            request,
            StatusCode(401),
            text_content_type(),
            b"unauthorized: invalid or missing token",
        );
    }

    // Route table. The request is moved into exactly one branch.
    if url == "/v1/models" && method == Method::Get {
        return handle_list_models(request, ctx);
    }
    if let Some(hash) = match_blob_url(&url) {
        if method == Method::Get {
            return handle_get_blob(request, ctx, &hash);
        }
        return respond(request, StatusCode(405), text_content_type(), b"method not allowed");
    }
    if let Some(parts) = match_hf_proxy_url(&url) {
        if method == Method::Get {
            return handle_hf_proxy(request, ctx, &parts);
        }
        return respond(request, StatusCode(405), text_content_type(), b"method not allowed");
    }
    respond(request, StatusCode(404), text_content_type(), b"not found")
}

// ─────────────────────────────────────────────────────────────────────────────
// Route handlers
// ─────────────────────────────────────────────────────────────────────────────

fn handle_list_models(request: tiny_http::Request, ctx: &RequestContext) -> Result<()> {
    let db = ctx.db.lock().expect("db lock poisoned");
    let models = db.list_models(None)?;
    drop(db);

    let total_count = models.len();
    let total_size: i64 = models.iter().map(|m| m.size_bytes).sum();

    let mut arr = String::from("[");
    for (i, m) in models.iter().enumerate() {
        if i > 0 {
            arr.push(',');
        }
        arr.push_str(
            &serde_json::json!({
                "hash": m.blake3_hash.as_hex(),
                "size_bytes": m.size_bytes,
                "format": m.format,
                "category": m.category,
            })
            .to_string(),
        );
    }
    arr.push(']');
    let body = format!(
        r#"{{"models":{},"total":{},"total_size_bytes":{}}}"#,
        arr, total_count, total_size
    );
    respond(request, StatusCode(200), json_content_type(), body.as_bytes())
}

fn handle_get_blob(request: tiny_http::Request, ctx: &RequestContext, hash_hex: &str) -> Result<()> {
    let hash = match Blake3Hash::from_hex(hash_hex) {
        Ok(h) => h,
        Err(_) => {
            return respond(
                request,
                StatusCode(400),
                text_content_type(),
                b"bad request: invalid blake3 hash",
            );
        }
    };

    let path = match ctx.cas.get(&hash) {
        Some(p) => p,
        None => {
            return respond(request, StatusCode(404), text_content_type(), b"not found: blob absent");
        }
    };

    let total_len = std::fs::metadata(&path)?.len();
    let range_header = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str());

    if let Some((start, end)) = parse_range(range_header, total_len) {
        stream_range(request, &path, start, end, total_len)
    } else {
        stream_full(request, &path, total_len)
    }
}

fn handle_hf_proxy(request: tiny_http::Request, ctx: &RequestContext, parts: &HfProxyParts) -> Result<()> {
    let repo_id = format!("{}/{}", parts.org, parts.repo);

    // Fast path: the fake HF cache already has this revision/file.
    if ctx.hf_cache.check_cache(&repo_id, &parts.revision, &parts.file) {
        let snapshot = ctx
            .hf_cache
            .snapshot_file_path(&repo_id, &parts.revision, &parts.file);
        if snapshot.exists() {
            return stream_file_with_headers(request, &snapshot, "hit", None);
        }
    }

    // Cache miss → pull through modeld (real HuggingFace). The Downloader
    // stores into CAS + fake HF cache, so subsequent requests hit above.
    let mut db = ctx.db.lock().expect("db lock poisoned");
    match ctx
        .downloader
        .download_hf_file(&mut db, &repo_id, &parts.file, Some(&parts.revision), None)
    {
        Ok(result) => {
            drop(db);
            let blake3 = result.blake3_hash.as_hex().to_string();
            stream_file_with_headers(request, &result.cas_path, "miss", Some(&blake3))
        }
        Err(e) => {
            drop(db);
            let msg = format!("hf proxy download failed: {:#}", e);
            respond(request, StatusCode(502), text_content_type(), msg.as_bytes())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Response helpers (each consumes the request via respond())
// ─────────────────────────────────────────────────────────────────────────────

fn respond(
    request: tiny_http::Request,
    status: StatusCode,
    content_type: Header,
    body: &[u8],
) -> Result<()> {
    let response = Response::empty(status)
        .with_header(content_type)
        .with_data(Cursor::new(Vec::from(body)), Some(body.len()));
    request
        .respond(response)
        .map_err(|e| anyhow!("failed to write response: {}", e))
}

fn stream_full(request: tiny_http::Request, path: &std::path::Path, total_len: u64) -> Result<()> {
    let file = File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut all = Vec::with_capacity(total_len as usize);
    reader.read_to_end(&mut all)?;
    let response = Response::empty(StatusCode(200))
        .with_header(octet_content_type())
        .with_header(header("Content-Length", &total_len.to_string()))
        .with_header(header("Accept-Ranges", "bytes"))
        .with_data(Cursor::new(all), Some(total_len as usize));
    request
        .respond(response)
        .map_err(|e| anyhow!("stream full failed: {}", e))
}

fn stream_range(
    request: tiny_http::Request,
    path: &std::path::Path,
    start: u64,
    end: u64,
    total_len: u64,
) -> Result<()> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let length = end - start + 1;
    let mut buf = vec![0u8; length as usize];
    file.read_exact(&mut buf)?;

    let content_range = format!("bytes {}-{}/{}", start, end, total_len);
    let response = Response::empty(StatusCode(206))
        .with_header(octet_content_type())
        .with_header(header("Content-Length", &length.to_string()))
        .with_header(header("Content-Range", &content_range))
        .with_header(header("Accept-Ranges", "bytes"))
        .with_data(Cursor::new(buf), Some(length as usize));
    request
        .respond(response)
        .map_err(|e| anyhow!("stream range failed: {}", e))
}

fn stream_file_with_headers(
    request: tiny_http::Request,
    path: &std::path::Path,
    cache_status: &str,
    blake3: Option<&str>,
) -> Result<()> {
    let len = std::fs::metadata(path)?.len();
    let mut file = File::open(path)?;
    let mut data = Vec::with_capacity(len as usize);
    file.read_to_end(&mut data)?;

    let mut response = Response::empty(StatusCode(200))
        .with_header(octet_content_type())
        .with_header(header("Content-Length", &len.to_string()))
        .with_header(header("X-Modeld-Cache", cache_status))
        .with_header(header("Accept-Ranges", "bytes"));
    if let Some(h) = blake3 {
        response = response.with_header(header("X-Modeld-Blake3", h));
    }
    let response = response.with_data(Cursor::new(data), Some(len as usize));
    request
        .respond(response)
        .map_err(|e| anyhow!("stream hf failed: {}", e))
}

// ─────────────────────────────────────────────────────────────────────────────
// Header / URL parsing helpers
// ─────────────────────────────────────────────────────────────────────────────

fn json_health(ctx: &RequestContext) -> String {
    let uptime = ctx.started_at.elapsed().as_secs();
    let (model_count, total_bytes) = match ctx.db.lock() {
        Ok(db) => (db.count_models().unwrap_or(0), db.total_size().unwrap_or(0)),
        Err(_) => (0, 0),
    };
    format!(
        r#"{{"status":"ok","version":"{}","uptime_seconds":{},"model_count":{},"total_bytes":{}}}"#,
        env!("CARGO_PKG_VERSION"),
        uptime,
        model_count,
        total_bytes
    )
}

/// Match `/v1/blobs/{hash}` and return the hash segment.
fn match_blob_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("/v1/blobs/")?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(rest.to_string())
}

/// Parsed components of `/v1/hf-proxy/{org}/{repo}/resolve/{revision}/{file...}`.
struct HfProxyParts {
    org: String,
    repo: String,
    revision: String,
    file: String,
}

fn match_hf_proxy_url(url: &str) -> Option<HfProxyParts> {
    let rest = url.strip_prefix("/v1/hf-proxy/")?;
    let mut segments = rest.split('/');
    let org = segments.next()?.to_string();
    let repo = segments.next()?.to_string();
    let resolve = segments.next()?;
    if resolve != "resolve" {
        return None;
    }
    let revision = segments.next()?.to_string();
    let file = segments.collect::<Vec<_>>().join("/");
    if org.is_empty() || repo.is_empty() || revision.is_empty() || file.is_empty() {
        return None;
    }
    Some(HfProxyParts {
        org,
        repo,
        revision,
        file,
    })
}

fn open_db(store_path: &std::path::Path) -> Result<Database> {
    let db_path = store_path.join("modeld.db");
    Database::open(&db_path)
}

fn bearer_token(request: &tiny_http::Request) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| {
            h.value
                .as_str()
                .strip_prefix("Bearer ")
                .map(|s| s.to_string())
        })
}

fn peer_ip(request: &tiny_http::Request) -> String {
    request
        .remote_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default()
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes())
        .unwrap_or_else(|_| Header::from_bytes("X-Modeld", "0").unwrap())
}

fn json_content_type() -> Header {
    header("Content-Type", "application/json")
}

fn text_content_type() -> Header {
    header("Content-Type", "text/plain")
}

fn octet_content_type() -> Header {
    header("Content-Type", "application/octet-stream")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_blob_url_valid() {
        assert_eq!(
            match_blob_url("/v1/blobs/abcdef0123456789"),
            Some("abcdef0123456789".to_string())
        );
    }

    #[test]
    fn test_match_blob_url_rejects_nested() {
        assert_eq!(match_blob_url("/v1/blobs/"), None);
        assert_eq!(match_blob_url("/v1/blobs/ab/cd"), None);
        assert_eq!(match_blob_url("/v1/models"), None);
    }

    #[test]
    fn test_match_hf_proxy_url_valid() {
        let parts = match_hf_proxy_url(
            "/v1/hf-proxy/stabilityai/sdxl-base/resolve/main/model.safetensors",
        )
        .unwrap();
        assert_eq!(parts.org, "stabilityai");
        assert_eq!(parts.repo, "sdxl-base");
        assert_eq!(parts.revision, "main");
        assert_eq!(parts.file, "model.safetensors");
    }

    #[test]
    fn test_match_hf_proxy_url_nested_file() {
        let parts =
            match_hf_proxy_url("/v1/hf-proxy/org/model/resolve/v1/sub/dir/file.bin").unwrap();
        assert_eq!(parts.file, "sub/dir/file.bin");
    }

    #[test]
    fn test_match_hf_proxy_url_rejects_bad() {
        assert!(match_hf_proxy_url("/v1/hf-proxy/org/model/main/file").is_none());
        assert!(match_hf_proxy_url("/v1/hf-proxy/org//resolve/main/file").is_none());
        assert!(match_hf_proxy_url("/v1/blobs/abc").is_none());
    }
}
