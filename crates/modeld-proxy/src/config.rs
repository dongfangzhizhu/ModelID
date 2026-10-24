//! Proxy configuration — the `[proxy]` table of `modeld.toml`.
//!
//! The schema itself lives in `modeld_core::config::ProxyConfig` so that a single
//! `ModeldConfig` describes the whole file. This module re-exports it (with the
//! historical names used across the proxy crate) and nothing else.
//!
//! ```toml
//! [proxy]
//! port = 8234
//! bind_address = "127.0.0.1"
//!
//! [proxy.auth]
//! require_token = true
//! tokens = []
//!
//! [proxy.network]
//! allow_anonymous = false
//! allowed_ips = ["192.168.1.0/24"]
//! denied_ips = []
//! ```
//!
//! Every field has a safe default, so a minimal `modeld.toml` (or none at all)
//! starts a **closed** server on port 8234 — anonymous requests return 401.

pub use modeld_core::config::{
    is_loopback_addr, ProxyAuthConfig as AuthConfig, ProxyConfig,
    ProxyNetworkConfig as NetworkConfig, DEFAULT_RANGE_CHUNK_BYTES,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::{Path, PathBuf};
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
