//! Proxy configuration.
//!
//! modeld proxy settings live inside the shared `modeld.toml` under the
//! `[proxy]` section (with `[proxy.auth]` and `[proxy.network]` sub-tables):
//!
//! ```toml
//! [proxy]
//! port = 8234
//! bind_address = "0.0.0.0"
//! store_path = ".modeld"
//!
//! [proxy.auth]
//! require_token = false
//! tokens = []
//!
//! [proxy.network]
//! allow_anonymous = false   # default: require auth or explicit allow
//! allowed_ips = ["192.168.1.0/24"]
//! denied_ips = []
//! ```
//!
//! Every field has a safe default, so a minimal `modeld.toml` (or none at all)
//! starts a **closed** server on port 8234 — anonymous requests return 401.
//! To allow open access on a trusted LAN, set `allow_anonymous = true`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level proxy configuration (the `[proxy]` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub port: u16,
    pub bind_address: String,
    pub store_path: PathBuf,
    pub auth: AuthConfig,
    pub network: NetworkConfig,
    /// Largest byte range served in a single `206` response.
    ///
    /// A `Range: bytes=0-` request for a 20 GB model would otherwise be one
    /// 20 GB response; chunking keeps memory flat and lets clients resume.
    pub range_chunk_bytes: u64,
}

/// Default for [`ProxyConfig::range_chunk_bytes`]: 64 MiB.
pub const DEFAULT_RANGE_CHUNK_BYTES: u64 = 64 * 1024 * 1024;

/// `[proxy.auth]` — Bearer-token access control.
///
/// Secure by default: a proxy started without any token configuration refuses
/// every authenticated route until a token exists. The CLI generates and
/// persists one on first start.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// When true, requests must carry a valid `Authorization: Bearer <token>`.
    pub require_token: bool,
    /// Accepted bearer tokens.
    pub tokens: Vec<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self { require_token: true, tokens: Vec::new() }
    }
}

/// `[proxy.network]` — IP allow/deny lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct NetworkConfig {
    /// Whether unauthenticated (anonymous) access is permitted at all.
    pub allow_anonymous: bool,
    /// Allow-list; empty means "allow all (that aren't denied)".
    pub allowed_ips: Vec<String>,
    /// Deny-list; takes precedence over the allow-list.
    pub denied_ips: Vec<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            port: 8234,
            bind_address: "127.0.0.1".to_string(),
            store_path: PathBuf::from(".modeld"),
            auth: AuthConfig::default(),
            network: NetworkConfig::default(),
            range_chunk_bytes: DEFAULT_RANGE_CHUNK_BYTES,
        }
    }
}

impl ProxyConfig {
    /// Load `[proxy]` from a `modeld.toml` file.
    ///
    /// If the file or the `[proxy]` table is absent, returns the safe default.
    /// Missing sub-fields fall back to their individual defaults via `#[serde(default)]`.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        // The modeld.toml file may contain other tables; we only deserialize
        // the [proxy] section into our typed config.
        #[derive(Deserialize, Default)]
        struct ModeldToml {
            #[serde(default)]
            proxy: ProxyConfigFile,
        }

        // We deserialize [proxy] directly so that a [proxy] table with only
        // some fields still merges with defaults. Using #[serde(default)] on
        // ProxyConfig means an entirely missing [proxy] yields defaults.
        let parsed: ModeldToml = toml::from_str(&raw)
            .with_context(|| format!("Failed to parse TOML config: {}", path.display()))?;
        Ok(parsed.proxy.into_config())
    }

    /// Build a config from CLI overrides applied on top of defaults.
    pub fn from_cli(
        port: Option<u16>,
        bind: Option<String>,
        store_path: PathBuf,
        token: Option<String>,
        allow_anonymous: bool,
    ) -> Self {
        let mut cfg = Self::default();
        if let Some(p) = port {
            cfg.port = p;
        }
        if let Some(b) = bind {
            cfg.bind_address = b;
        }
        cfg.store_path = store_path;
        if let Some(t) = token {
            cfg.auth.require_token = true;
            cfg.auth.tokens.push(t);
        }
        cfg.network.allow_anonymous = allow_anonymous;
        // Anonymous access and token auth are mutually exclusive; only the
        // loopback bind may run without a token (enforced by the invariants).
        if allow_anonymous && cfg.auth.tokens.is_empty() {
            cfg.auth.require_token = false;
        }
        cfg
    }

    /// `bind_address:port` as a single string for `tiny_http::Server::http`.
    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.bind_address, self.port)
    }

    /// Return a copy of this config with anonymous access enabled.
    ///
    /// Convenience for integration tests and trusted-LAN setups:
    /// `ProxyConfig::default().with_open_access()`. Anonymous access only
    /// passes `check_startup_invariants` for a loopback bind.
    pub fn with_open_access(mut self) -> Self {
        self.network.allow_anonymous = true;
        self.auth.require_token = false;
        self
    }

    /// True when the configured bind address is a loopback address.
    pub fn is_loopback_bind(&self) -> bool {
        is_loopback_addr(&self.bind_address)
    }

    /// Reject configurations that would expose the store without authentication.
    ///
    /// Called from `ProxyServer::start` and from the CLI so a misconfigured
    /// server fails before it binds the port, not after it starts leaking data.
    pub fn check_startup_invariants(&self) -> Result<()> {
        if !self.is_loopback_bind() && self.network.allow_anonymous {
            anyhow::bail!(
                "refusing to allow anonymous access on non-loopback bind '{}': \
                 anonymous access is only permitted on 127.0.0.1/::1",
                self.bind_address
            );
        }
        if self.auth.require_token && self.auth.tokens.is_empty() {
            anyhow::bail!(
                "token authentication is required but no token is configured \
                 (pass --token or run 'modeld proxy token-rotate')"
            );
        }
        Ok(())
    }
}

/// True for `127.0.0.0/8`, `::1` and `localhost`.
pub fn is_loopback_addr(addr: &str) -> bool {
    let addr = addr.trim();
    addr.eq_ignore_ascii_case("localhost")
        || addr == "::1"
        || addr == "[::1]"
        || addr == "127.0.0.1"
        || addr.starts_with("127.")
}

/// Intermediate deserialization type so a missing `[proxy]` table yields full
/// defaults rather than failing.
#[derive(Deserialize, Default)]
struct ProxyConfigFile {
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    bind_address: Option<String>,
    #[serde(default)]
    store_path: Option<PathBuf>,
    #[serde(default)]
    auth: AuthConfig,
    #[serde(default)]
    network: NetworkConfig,
    #[serde(default)]
    range_chunk_bytes: Option<u64>,
}

impl ProxyConfigFile {
    fn into_config(self) -> ProxyConfig {
        let default = ProxyConfig::default();
        ProxyConfig {
            port: self.port.unwrap_or(default.port),
            bind_address: self.bind_address.unwrap_or(default.bind_address),
            store_path: self.store_path.unwrap_or(default.store_path),
            auth: self.auth,
            network: self.network,
            range_chunk_bytes: self.range_chunk_bytes.unwrap_or(default.range_chunk_bytes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_default_port_is_8234() {
        let cfg = ProxyConfig::default();
        assert_eq!(cfg.port, 8234);
        assert_eq!(cfg.bind_address, "127.0.0.1");
        // Secure default: a token is required and anonymous access is off.
        assert!(cfg.auth.require_token);
        assert!(cfg.auth.tokens.is_empty());
        assert!(!cfg.network.allow_anonymous);
    }

    #[test]
    fn test_missing_file_returns_default() {
        let cfg = ProxyConfig::load(Path::new("nonexistent_modeld.toml")).unwrap();
        assert_eq!(cfg.port, 8234);
    }

    #[test]
    fn test_full_config_parsed() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[proxy]
port = 9000
bind_address = "127.0.0.1"
store_path = "/data/modeld"

[proxy.auth]
require_token = true
tokens = ["abc", "def"]

[proxy.network]
allow_anonymous = false
allowed_ips = ["10.0.0.0/8"]
denied_ips = ["10.0.0.5"]
"#
        )
        .unwrap();
        f.flush().unwrap();

        let cfg = ProxyConfig::load(f.path()).unwrap();
        assert_eq!(cfg.port, 9000);
        assert_eq!(cfg.bind_address, "127.0.0.1");
        assert_eq!(cfg.store_path, PathBuf::from("/data/modeld"));
        assert!(cfg.auth.require_token);
        assert_eq!(cfg.auth.tokens, vec!["abc", "def"]);
        assert!(!cfg.network.allow_anonymous);
        assert_eq!(cfg.network.allowed_ips, vec!["10.0.0.0/8"]);
        assert_eq!(cfg.network.denied_ips, vec!["10.0.0.5"]);
    }

    #[test]
    fn test_partial_config_uses_defaults() {
        // Only port specified; everything else defaults.
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[proxy]\nport = 7777\n").unwrap();
        f.flush().unwrap();

        let cfg = ProxyConfig::load(f.path()).unwrap();
        assert_eq!(cfg.port, 7777);
        assert_eq!(cfg.bind_address, "127.0.0.1"); // default
        assert!(cfg.auth.require_token); // default: auth is on
    }

    #[test]
    fn test_other_tables_ignored() {
        // modeld.toml may have unrelated tables; [proxy] still parses.
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[scan]\nextensions = ['.safetensors']\n\n[proxy]\nport = 8234\n").unwrap();
        f.flush().unwrap();

        let cfg = ProxyConfig::load(f.path()).unwrap();
        assert_eq!(cfg.port, 8234);
    }

    #[test]
    fn test_from_cli_overrides() {
        let cfg = ProxyConfig::from_cli(
            Some(9001),
            Some("127.0.0.1".to_string()),
            PathBuf::from("/store"),
            Some("tok".to_string()),
            false,
        );
        assert_eq!(cfg.port, 9001);
        assert_eq!(cfg.bind_address, "127.0.0.1");
        assert_eq!(cfg.store_path, PathBuf::from("/store"));
        assert!(cfg.auth.require_token);
        assert_eq!(cfg.auth.tokens, vec!["tok"]);
        assert!(!cfg.network.allow_anonymous);
    }

    #[test]
    fn test_default_config_requires_a_token() {
        let cfg = ProxyConfig::default();
        assert!(cfg.auth.require_token, "token auth must be on by default");
        assert!(!cfg.network.allow_anonymous, "anonymous access must be off by default");
        assert_eq!(cfg.bind_address, "127.0.0.1");
        // No token configured yet → the invariant refuses to start.
        assert!(cfg.check_startup_invariants().is_err());
    }

    #[test]
    fn test_open_access_disables_token_and_is_loopback_only() {
        let cfg = ProxyConfig::default().with_open_access();
        assert!(!cfg.auth.require_token);
        assert!(cfg.check_startup_invariants().is_ok());

        // The same flags on a public bind are rejected.
        let mut public = cfg.clone();
        public.bind_address = "0.0.0.0".to_string();
        assert!(public.check_startup_invariants().is_err());
    }

    #[test]
    fn test_loopback_detection() {
        for addr in ["127.0.0.1", "127.0.0.5", "::1", "[::1]", "localhost", "LOCALHOST"] {
            assert!(is_loopback_addr(addr), "{addr} should be loopback");
        }
        for addr in ["0.0.0.0", "192.168.1.5", "10.0.0.1", "::", "example.com"] {
            assert!(!is_loopback_addr(addr), "{addr} should not be loopback");
        }
    }

    #[test]
    fn test_from_cli_anonymous_needs_no_token() {
        let cfg = ProxyConfig::from_cli(
            Some(9000),
            Some("127.0.0.1".to_string()),
            PathBuf::from("/store"),
            None,
            true,
        );
        assert!(!cfg.auth.require_token);
        assert!(cfg.check_startup_invariants().is_ok());
    }

    #[test]
    fn test_from_cli_token_enables_auth() {
        let cfg = ProxyConfig::from_cli(
            Some(9000),
            Some("0.0.0.0".to_string()),
            PathBuf::from("/store"),
            Some("tok".to_string()),
            false,
        );
        assert!(cfg.auth.require_token);
        assert_eq!(cfg.auth.tokens, vec!["tok".to_string()]);
        assert!(cfg.check_startup_invariants().is_ok());
    }

    #[test]
    fn test_bind_addr() {
        let cfg = ProxyConfig::from_cli(
            Some(8234),
            Some("0.0.0.0".to_string()),
            PathBuf::from("."),
            None,
            true,
        );
        assert_eq!(cfg.bind_addr(), "0.0.0.0:8234");
    }
}
