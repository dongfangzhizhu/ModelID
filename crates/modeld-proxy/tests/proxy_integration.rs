//! End-to-end integration tests for the modeld proxy server.
//!
//! Each test:
//! 1. Creates a temp store, inits CAS + DB.
//! 2. Binds a real `tiny_http` Server on an ephemeral port (`127.0.0.1:0`).
//! 3. Spawns the proxy request loop on a background thread.
//! 4. Talks to it via `modeld_client::ModeldClient` (or raw `ureq`).
//!
//! The HF-proxy *miss* path is not exercised here (it would hit the real
//! HuggingFace servers); the *hit* path is covered by pre-seeding the fake
//! HF cache.

use modeld_client::ModeldClient;
use modeld_core::{hash_file, CasStore, Database};
use modeld_proxy::{ProxyConfig, ProxyServer};
use std::io::Read;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;
use tiny_http::Server;

/// Extract the HTTP status code from a ureq error, or 0 for transport errors.
fn status_of_err(err: ureq::Error) -> u16 {
    match err {
        ureq::Error::Status(code, _) => code,
        ureq::Error::Transport(_) => 0,
    }
}

/// A running test proxy: holds the temp store, the shutdown flag, and the
/// base URL clients should target. The server thread is detached; tests set
/// the shutdown flag (or simply let the process exit) to stop it.
struct TestProxy {
    url: String,
    _store: TempDir,
    _shutdown: Arc<AtomicBool>,
    store_path: std::path::PathBuf,
}

impl TestProxy {
    /// Start a proxy with the given config on an ephemeral port.
    fn start(config: ProxyConfig) -> Self {
        let store = TempDir::new().unwrap();
        let store_path = store.path().to_path_buf();

        // Initialize store components so the proxy can open the DB / CAS.
        CasStore::new(&store_path).init().unwrap();
        Database::open(&store_path.join("modeld.db")).unwrap();

        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr();
        let url = format!("http://{}", addr);

        let shutdown = Arc::new(AtomicBool::new(false));
        let proxy = ProxyServer::new(config, store_path.clone());
        let shutdown_for_thread = shutdown.clone();
        thread::spawn(move || {
            let _ = proxy.serve_on(server, shutdown_for_thread);
        });

        // Give the listener a moment to be ready.
        thread::sleep(std::time::Duration::from_millis(100));

        Self { url, _store: store, _shutdown: shutdown, store_path }
    }

    fn client(&self) -> ModeldClient {
        ModeldClient::new(&self.url)
    }
}

/// Store a file into CAS + DB and return its BLAKE3 hash (hex) + size.
fn seed_blob(store_path: &std::path::Path, contents: &[u8]) -> (String, u64) {
    use modeld_core::hash::hash_file;
    // Write a temp source file, hash it, store in CAS, record in DB.
    let src = store_path.join("_seed_src.bin");
    std::fs::write(&src, contents).unwrap();
    let hash = hash_file(&src).unwrap();
    let cas = CasStore::new(store_path);
    cas.store(&src, &hash).unwrap();

    let mut db = Database::open(&store_path.join("modeld.db")).unwrap();
    db.insert_or_update_model(&hash, contents.len() as i64, None, None, None, None).unwrap();
    // cleanup temp source
    let _ = std::fs::remove_file(&src);
    (hash.as_hex().to_string(), contents.len() as u64)
}

#[test]
fn test_health_endpoint() {
    let proxy = TestProxy::start(ProxyConfig::default().with_open_access());
    let h = proxy.client().health().unwrap();
    assert_eq!(h.status, "ok");
    assert_eq!(h.version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn test_list_models_empty() {
    let proxy = TestProxy::start(ProxyConfig::default().with_open_access());
    let models = proxy.client().list_models().unwrap();
    assert!(models.is_empty());
}

#[test]
fn test_list_models_after_seed() {
    let proxy = TestProxy::start(ProxyConfig::default().with_open_access());
    let (hash, size) = seed_blob(&proxy.store_path, b"hello modeld world");
    let models = proxy.client().list_models().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].hash, hash);
    assert_eq!(models[0].size_bytes, size as i64);
}

#[test]
fn test_blob_download_full_and_verify_hash() {
    let payload = b"the quick brown fox jumps over the lazy dog 0123456789";
    let proxy = TestProxy::start(ProxyConfig::default().with_open_access());
    let (hash, _size) = seed_blob(&proxy.store_path, payload);

    let dest = proxy.store_path.join("downloaded.bin");
    let client = proxy.client();
    let n = client.download_blob(&hash, &dest, None).unwrap();
    assert_eq!(n as usize, payload.len());

    let got = std::fs::read(&dest).unwrap();
    assert_eq!(got, payload);

    // Verify the downloaded content hashes back to the same BLAKE3.
    let re_hash = hash_file(&dest).unwrap();
    assert_eq!(re_hash.as_hex(), hash);
}

#[test]
fn test_blob_range_request() {
    // Build a payload with known bytes.
    let payload: Vec<u8> = (0..200u8).collect();
    let proxy = TestProxy::start(ProxyConfig::default().with_open_access());
    let (hash, _size) = seed_blob(&proxy.store_path, &payload);

    // Request bytes 10-29 via raw ureq (Range header).
    let url = format!("{}/v1/blobs/{}", proxy.url, hash);
    let resp = ureq::get(&url).set("Range", "bytes=10-29").call().unwrap();
    assert_eq!(resp.status(), 206);
    let content_range = resp.header("Content-Range").unwrap().to_string();
    assert_eq!(content_range, format!("bytes 10-29/{}", payload.len()));

    let mut body = String::new();
    resp.into_reader().read_to_string(&mut body).unwrap();
    let got: Vec<u8> = body.bytes().collect();
    assert_eq!(got, &payload[10..30]);
}

#[test]
fn test_blob_404_for_unknown_hash() {
    let proxy = TestProxy::start(ProxyConfig::default().with_open_access());
    let url = format!("{}/v1/blobs/{}", proxy.url, "f".repeat(64));
    let resp = ureq::get(&url).call();
    assert!(resp.is_err());
    let err = resp.unwrap_err();
    assert_eq!(status_of_err(err), 404);
}

#[test]
fn test_auth_required_rejects_without_token() {
    let mut config = ProxyConfig::default();
    config.auth.require_token = true;
    config.auth.tokens = vec!["secret".to_string()];
    let proxy = TestProxy::start(config);

    // No token → 401.
    let url = format!("{}/v1/models", proxy.url);
    let resp = ureq::get(&url).call();
    assert!(resp.is_err());
    assert_eq!(status_of_err(resp.unwrap_err()), 401);
}

#[test]
fn test_auth_allows_with_valid_token() {
    let mut config = ProxyConfig::default();
    config.auth.require_token = true;
    config.auth.tokens = vec!["secret".to_string()];
    let proxy = TestProxy::start(config);

    let client = proxy.client().with_token("secret");
    let models = client.list_models().unwrap();
    assert!(models.is_empty());
}

#[test]
fn test_health_is_public_without_token() {
    // Even when auth is required, /health must remain open.
    let mut config = ProxyConfig::default();
    config.auth.require_token = true;
    config.auth.tokens = vec!["secret".to_string()];
    let proxy = TestProxy::start(config);

    let h = proxy.client().health().unwrap();
    assert_eq!(h.status, "ok");
}

#[test]
fn test_unknown_route_returns_404() {
    let proxy = TestProxy::start(ProxyConfig::default().with_open_access());
    let url = format!("{}/v1/nonexistent", proxy.url);
    let resp = ureq::get(&url).call();
    assert!(resp.is_err());
    assert_eq!(status_of_err(resp.unwrap_err()), 404);
}

#[test]
fn test_hf_proxy_hit_from_fake_cache() {
    // Pre-seed the fake HF cache so the proxy serves it as a cache hit,
    // without touching the real HuggingFace network.
    use modeld_core::HfCache;

    let proxy = TestProxy::start(ProxyConfig::default().with_open_access());
    let cas = CasStore::new(&proxy.store_path);
    cas.init().unwrap();
    let hf_cache = HfCache::new(&proxy.store_path);
    hf_cache.init().unwrap();

    // Create a model in CAS.
    let content = b"fake safetensors model data for hf proxy test";
    let src = proxy.store_path.join("_hf_src.bin");
    std::fs::write(&src, content).unwrap();
    let hash = hash_file(&src).unwrap();
    cas.store(&src, &hash).unwrap();

    let mut db = Database::open(&proxy.store_path.join("modeld.db")).unwrap();
    db.insert_or_update_model(&hash, content.len() as i64, None, None, None, None).unwrap();

    // Build a fake HF cache entry pointing at this CAS object.
    let sha256 = "a".repeat(64);
    hf_cache
        .create_cache_entry(
            "org/model",
            "model.safetensors",
            "mainrev",
            &sha256,
            &hash,
            Some("main"),
        )
        .unwrap();

    // Hit the HF-proxy endpoint for that revision/file.
    let url = format!("{}/v1/hf-proxy/org/model/resolve/mainrev/model.safetensors", proxy.url);
    let resp = ureq::get(&url).call().unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.header("X-Modeld-Cache").unwrap(), "hit");

    let mut body = Vec::new();
    resp.into_reader().read_to_end(&mut body).unwrap();
    assert_eq!(body, content);
}
