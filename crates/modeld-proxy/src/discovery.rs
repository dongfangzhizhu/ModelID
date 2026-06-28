//! mDNS-based LAN service discovery.
//!
//! modeld proxies advertise themselves as `_modeld._tcp.local.` so other
//! machines on the LAN can find them without manual IP configuration.
//!
//! Two roles:
//! - [`MdnsAnnouncer`]: holds the TXT-record description this server would
//!   publish. Note: the `mdns` 3.x crate is *discovery-only*; actual service
//!   publication must be performed by the host's mDNS daemon (Avahi / Bonjour
//!   / the Windows `mdnsResponder` service) using the values returned here.
//!   See [`MdnsAnnouncer::start`] for details.
//! - [`discover`]: scans the LAN for `_modeld._tcp.local.` services and
//!   returns their address/port/TXT records.

use futures_util::{pin_mut, stream::StreamExt};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Duration;

/// Compute the lowercase hex-encoded SHA-256 digest of a string.
/// Used to store a token fingerprint without persisting the token itself.
fn hex_sha256(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// The mDNS service type modeld registers as.
pub const SERVICE_TYPE: &str = "_modeld._tcp.local.";

/// A modeld server discovered on the LAN.
#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub address: String,
    pub port: u16,
    pub txt: HashMap<String, String>,
}

/// Description of this server for mDNS publication.
pub struct MdnsAnnouncer {
    port: u16,
    txt: HashMap<String, String>,
    /// SHA-256 fingerprint of the bearer token (never the token itself).
    fingerprint: String,
    started: bool,
}

impl MdnsAnnouncer {
    /// Build TXT records for a server. `model_count`/`total_bytes` are
    /// published so clients can pick the richest cache without querying.
    pub fn build_txt(version: &str, model_count: i64, total_bytes: i64) -> HashMap<String, String> {
        let mut txt = HashMap::new();
        txt.insert("version".to_string(), version.to_string());
        txt.insert("models".to_string(), model_count.to_string());
        txt.insert("bytes".to_string(), total_bytes.to_string());
        txt
    }

    /// Create an announcer for the given port and TXT records.
    pub fn new(port: u16, txt: HashMap<String, String>) -> Self {
        Self { port, txt, fingerprint: "none".to_string(), started: false }
    }

    /// Create an announcer for the given port with optional token authentication.
    ///
    /// The token itself is **never** stored; only its SHA-256 fingerprint is kept
    /// so that the TXT record lets remote clients verify configuration parity
    /// without exposing credentials.
    pub fn new_with_token(port: u16, token: Option<&str>) -> Self {
        let fingerprint = token
            .map(|t| hex_sha256(t))
            .unwrap_or_else(|| "none".to_string());
        let mut txt = HashMap::new();
        txt.insert("fingerprint".to_string(), fingerprint.clone());
        Self { port, txt, fingerprint, started: false }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn txt(&self) -> &HashMap<String, String> {
        &self.txt
    }

    /// SHA-256 fingerprint of the bearer token, or `"none"` if no token is set.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// "Start" advertising.
    ///
    /// The `mdns` 3.x crate does not implement service publication — it can
    /// only *query* for services. To actually be discoverable, register this
    /// service with the host mDNS daemon. On Linux/Avahi:
    ///
    /// ```sh
    /// avahi-publish -s modeld _modeld._tcp 8234 version=0.5.0 models=42
    /// ```
    ///
    /// This method logs the intended registration and returns; it does not
    /// itself broadcast packets. It is kept as a no-op-with-log so the proxy
    /// can run portably on systems without a publication backend.
    pub fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let txt_str =
            self.txt.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join(" ");
        let stype = SERVICE_TYPE;
        let port_str = self.port.to_string();
        eprintln!(
            "{}",
            modeld_core::i18n::tf(
                "warn.mdns_announce",
                &[("stype", &stype), ("port", &port_str), ("txt", &txt_str)],
            )
        );
        eprintln!("{}", modeld_core::i18n::t("warn.mdns_announce_hint"));
    }
}

/// Discover modeld proxy servers on the local network.
///
/// Blocks for up to `timeout_secs` collecting responses, then returns them
/// deduplicated and sorted by address. Returns an empty vec if the network is
/// unavailable or no services respond.
pub fn discover(timeout_secs: u64) -> Vec<DiscoveredServer> {
    // Cap the query interval so a short timeout still fires at least one query.
    let query_interval = Duration::from_secs(timeout_secs.clamp(1, 15));
    let deadline = Duration::from_secs(timeout_secs.max(1));

    let stream = match mdns::discover::all(SERVICE_TYPE, query_interval) {
        Ok(s) => s.listen(),
        Err(_) => return Vec::new(),
    };

    let mut found: Vec<DiscoveredServer> = Vec::new();

    // Drive the async discovery stream on the async-std runtime, but bound it
    // by a wall-clock deadline so callers get control back.
    async_std::task::block_on(async {
        pin_mut!(stream);
        let futures_timer = async_std::future::timeout(deadline, async {
            while let Some(Ok(response)) = stream.next().await {
                if let Some(addr) = response.ip_addr() {
                    let port = response.port().unwrap_or(8234);
                    let mut txt = HashMap::new();
                    for t in response.txt_records() {
                        if let Some((k, v)) = t.split_once('=') {
                            txt.insert(k.to_string(), v.to_string());
                        }
                    }
                    let server = DiscoveredServer { address: addr.to_string(), port, txt };
                    // dedupe by (address, port)
                    if !found.iter().any(|s| s.address == server.address && s.port == server.port) {
                        found.push(server);
                    }
                }
            }
        });
        let _ = futures_timer.await; // ignore timeout error; we just stop collecting
    });

    found.sort_by(|a, b| a.address.cmp(&b.address));
    found
}

// ─────────────────────────────────────────────────────────────────────────────
// Simplified API for callers that prefer a flat struct
// ─────────────────────────────────────────────────────────────────────────────

/// A modeld service discovered on the local network (simplified view).
#[derive(Debug, Clone)]
pub struct DiscoveredService {
    pub name: String,
    pub address: String,
    pub port: u16,
}

/// Scan the local network for `_modeld._tcp.local.` services.
///
/// Equivalent to [`discover`] but returns [`DiscoveredService`] with a
/// synthesised `name` field (`modeld@<address>:<port>`).
pub fn discover_services(timeout_secs: u64) -> Vec<DiscoveredService> {
    discover(timeout_secs)
        .into_iter()
        .map(|s| DiscoveredService {
            name: format!("modeld@{}:{}", s.address, s.port),
            address: s.address,
            port: s.port,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_txt_contains_keys() {
        let txt = MdnsAnnouncer::build_txt("0.5.0", 42, 1_000_000);
        assert_eq!(txt.get("version").unwrap(), "0.5.0");
        assert_eq!(txt.get("models").unwrap(), "42");
        assert_eq!(txt.get("bytes").unwrap(), "1000000");
    }

    #[test]
    fn test_announcer_stores_port() {
        let txt = MdnsAnnouncer::build_txt("0.5.0", 0, 0);
        let a = MdnsAnnouncer::new(8234, txt);
        assert_eq!(a.port(), 8234);
        assert_eq!(a.txt().get("models").unwrap(), "0");
    }

    #[test]
    fn test_announcer_start_is_idempotent() {
        let txt = MdnsAnnouncer::build_txt("0.5.0", 0, 0);
        let mut a = MdnsAnnouncer::new(8234, txt);
        a.start();
        assert!(a.started);
        a.start(); // should not panic / double-log path is guarded
    }

    #[test]
    fn test_discover_returns_empty_without_network() {
        // No service is publishing in the test environment; discovery must
        // return an empty vec (not panic) within the short timeout.
        let result = discover(1);
        assert!(result.is_empty());
    }

    #[test]
    fn test_new_with_token_stores_fingerprint_not_token() {
        let token = "super-secret-token";
        let a = MdnsAnnouncer::new_with_token(8234, Some(token));
        assert_eq!(a.port(), 8234);
        // Fingerprint must be a 64-char hex SHA-256 string
        assert_eq!(a.fingerprint().len(), 64);
        assert!(a.fingerprint().chars().all(|c| c.is_ascii_hexdigit()));
        // Must NOT contain the raw token
        assert_ne!(a.fingerprint(), token);
        assert!(a.txt().contains_key("fingerprint"));
    }

    #[test]
    fn test_new_with_token_none_is_none_fingerprint() {
        let a = MdnsAnnouncer::new_with_token(8234, None);
        assert_eq!(a.fingerprint(), "none");
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let a1 = MdnsAnnouncer::new_with_token(8234, Some("tok"));
        let a2 = MdnsAnnouncer::new_with_token(8234, Some("tok"));
        assert_eq!(a1.fingerprint(), a2.fingerprint());
    }

    #[test]
    fn test_discover_services_returns_empty_without_network() {
        let result = discover_services(1);
        assert!(result.is_empty());
    }
}
