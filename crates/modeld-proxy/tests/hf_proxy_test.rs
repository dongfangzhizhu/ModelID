//! HF-proxy *miss* path: verify the proxy downloads from a (mock) HuggingFace
//! server, stores into CAS, and serves the content — then a second request
//! hits the cache.
//!
//! This is a standalone integration test binary because it sets the
//! `MODELD_HF_BASE` environment variable, which is process-global; keeping it
//! isolated avoids races with other proxy tests that read the same variable.

use modeld_core::{hash_file, CasStore, Database, HfCache};
use modeld_proxy::{ProxyConfig, ProxyServer};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;
use tiny_http::{Header, Server, StatusCode};

/// Run a tiny_http server that mimics HuggingFace's resolve endpoint:
/// returns the file body plus the `X-Linked-Etag` (SHA256) and
/// `X-Linked-Size` headers that modeld's Downloader reads.
fn start_mock_hf(body: Vec<u8>, sha256: String) -> String {
    let server = Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr();
    let url = format!("http://{}", addr);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let path = request.url().to_string();
            let method = request.method().clone();
            // modeld issues a HEAD for metadata then a GET for the body.
            if path.contains("/resolve/") && (method.as_str() == "HEAD" || method.as_str() == "GET")
            {
                let len = body.len();
                // Common headers for both HEAD and GET. Size is carried via
                // X-Linked-Size (HF's actual header); Content-Length is only
                // attached on GET — a HEAD with Content-Length but no body
                // would make the client block forever waiting for the body.
                let common = tiny_http::Response::empty(StatusCode(200))
                    .with_header(Header::from_bytes("X-Linked-Etag", sha256.as_str()).unwrap())
                    .with_header(
                        Header::from_bytes("X-Linked-Size", len.to_string().as_str()).unwrap(),
                    );
                if method.as_str() == "GET" {
                    let resp = common.with_header(
                        Header::from_bytes("Content-Length", len.to_string().as_str()).unwrap(),
                    );
                    let _ = request
                        .respond(resp.with_data(std::io::Cursor::new(body.clone()), Some(len)));
                } else {
                    // HEAD: no body. Must set Content-Length: 0 explicitly so
                    // the client knows the body is empty — without it, an
                    // HTTP/1.1 keep-alive client waits for connection close.
                    let resp =
                        common.with_header(Header::from_bytes("Content-Length", "0").unwrap());
                    let _ = request.respond(resp);
                }
            } else {
                let resp = tiny_http::Response::empty(StatusCode(404));
                let _ = request.respond(resp);
            }
        }
    });
    url
}

#[test]
fn test_hf_proxy_miss_then_hit() {
    // --- Set up a mock HuggingFace server on an ephemeral port. ---
    let payload = b"mock model bytes for hf proxy miss test 0123456789".to_vec();
    let mock_sha256 = "b".repeat(64);
    let mock_hf_url = start_mock_hf(payload.clone(), mock_sha256.clone());

    // --- Point the proxy at the mock HF via the env var. ---
    // SAFETY w.r.t. other tests: this is a standalone test binary, so no other
    // proxy test in this process reads MODELD_HF_BASE concurrently.
    std::env::set_var("MODELD_HF_BASE", &mock_hf_url);

    // --- Set up a proxy with a temp store. ---
    let store = TempDir::new().unwrap();
    let store_path = store.path().to_path_buf();
    CasStore::new(&store_path).init().unwrap();
    Database::open(&store_path.join("modeld.db")).unwrap();

    let server = Server::http("127.0.0.1:0").unwrap();
    let proxy_addr = server.server_addr();
    let proxy_url = format!("http://{}", proxy_addr);

    let shutdown = Arc::new(AtomicBool::new(false));
    let proxy = ProxyServer::new(ProxyConfig::default(), store_path.clone());
    let shutdown_for_thread = shutdown.clone();
    let join = thread::spawn(move || {
        let _ = proxy.serve_on(server, shutdown_for_thread);
    });
    thread::sleep(std::time::Duration::from_millis(150));

    // --- Sanity check: the mock HF itself responds to HEAD + GET. ---
    let mock_resolve = format!("{}/mockorg/mockmodel/resolve/main/mock.safetensors", mock_hf_url);
    let head = ureq::head(&mock_resolve).timeout(std::time::Duration::from_secs(5)).call();
    assert!(head.is_ok(), "mock HF HEAD failed: {:?}", head.err());
    let get = ureq::get(&mock_resolve).timeout(std::time::Duration::from_secs(10)).call();
    assert!(get.is_ok(), "mock HF GET failed: {:?}", get.err());
    let mut probe = Vec::new();
    get.unwrap().into_reader().read_to_end(&mut probe).unwrap();
    assert_eq!(probe, payload);

    // --- First request: cache MISS. The proxy must download from mock HF,
    //     store into CAS, and return the content. ---
    let url = format!("{}/v1/hf-proxy/mockorg/mockmodel/resolve/main/mock.safetensors", proxy_url);
    let resp = ureq::get(&url).call().expect("first hf-proxy request");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.header("X-Modeld-Cache").unwrap(), "miss");
    let blake3_hdr = resp.header("X-Modeld-Blake3").map(|s| s.to_string());
    let mut got = Vec::new();
    resp.into_reader().read_to_end(&mut got).unwrap();
    assert_eq!(got, payload);

    // The returned BLAKE3 must match the content.
    let expected_blake3 = hash_bytes(&payload);
    assert_eq!(blake3_hdr.as_deref(), Some(expected_blake3.as_str()));

    // --- Verify the file is now in CAS. ---
    let cas = CasStore::new(&store_path);
    let blake3_hash = modeld_core::Blake3Hash::from_hex(&expected_blake3).unwrap();
    let cas_path = cas.get(&blake3_hash).expect("blob stored in CAS after miss");
    let cas_bytes = std::fs::read(&cas_path).unwrap();
    assert_eq!(cas_bytes, payload);

    // --- Verify the fake HF cache entry was created. ---
    let hf_cache = HfCache::new(&store_path);
    assert!(
        hf_cache.check_cache("mockorg/mockmodel", "main", "mock.safetensors"),
        "fake HF cache entry created"
    );

    // --- Second request: cache HIT (served from fake HF cache, no download). ---
    let resp2 = ureq::get(&url).call().expect("second hf-proxy request");
    assert_eq!(resp2.status(), 200);
    assert_eq!(resp2.header("X-Modeld-Cache").unwrap(), "hit");
    let mut got2 = Vec::new();
    resp2.into_reader().read_to_end(&mut got2).unwrap();
    assert_eq!(got2, payload);

    // --- Shut down cleanly and restore the env var. ---
    shutdown.store(true, Ordering::SeqCst);
    let _ = join.join();
    std::env::remove_var("MODELD_HF_BASE");
}

/// Compute BLAKE3 hex of an in-memory byte slice by writing it to a temp file.
fn hash_bytes(payload: &[u8]) -> String {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), payload).unwrap();
    hash_file(tmp.path()).unwrap().as_hex().to_string()
}
