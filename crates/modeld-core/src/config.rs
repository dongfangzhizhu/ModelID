//! modeld.toml configuration read / write
//!
//! The configuration file lives at `<store_path>/modeld.toml`.
//! Missing keys are filled with `Default` values so old config files remain
//! forward-compatible with new fields.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Sub-section structs
// ─────────────────────────────────────────────────────────────────────────────

/// [store] section
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct StoreConfig {
    /// Explicit store root path (overrides platform default when set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// [serve] section
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServeConfig {
    /// Address to bind the HTTP server (default: 127.0.0.1)
    #[serde(default = "ServeConfig::default_host")]
    pub host: String,
    /// Port to listen on (default: 8234)
    #[serde(default = "ServeConfig::default_port")]
    pub port: u16,
    /// Enable Web UI alongside API (default: true)
    #[serde(default = "ServeConfig::default_webui")]
    pub webui: bool,
    /// Enable HF proxy (default: true)
    #[serde(default = "ServeConfig::default_proxy")]
    pub proxy: bool,
    /// Open the Web UI in the default browser on start (default: false)
    #[serde(default)]
    pub open_browser: bool,
}

impl ServeConfig {
    fn default_host() -> String {
        "127.0.0.1".to_string()
    }
    fn default_port() -> u16 {
        8234
    }
    fn default_webui() -> bool {
        true
    }
    fn default_proxy() -> bool {
        true
    }
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            host: Self::default_host(),
            port: Self::default_port(),
            webui: Self::default_webui(),
            proxy: Self::default_proxy(),
            open_browser: false,
        }
    }
}

/// [dedup] section
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DedupConfig {
    /// Default dedup strategy: "hardlink" | "symlink" | "copy_to_cas" | "virtual_alias"
    #[serde(default = "DedupConfig::default_strategy")]
    pub strategy: String,
    /// Minimum file size (bytes) to consider for dedup (default: 1 MiB)
    #[serde(default = "DedupConfig::default_min_size")]
    pub min_size_bytes: u64,
}

impl DedupConfig {
    fn default_strategy() -> String {
        "hardlink".to_string()
    }
    fn default_min_size() -> u64 {
        1_048_576
    }
}

impl Default for DedupConfig {
    fn default() -> Self {
        Self { strategy: Self::default_strategy(), min_size_bytes: Self::default_min_size() }
    }
}

/// [gc] section
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GcConfig {
    /// Quarantine TTL in days before permanent deletion (default: 30)
    #[serde(default = "GcConfig::default_quarantine_ttl_days")]
    pub quarantine_ttl_days: u32,
    /// Run GC automatically after each scan (default: false)
    #[serde(default)]
    pub auto_gc: bool,
}

impl GcConfig {
    fn default_quarantine_ttl_days() -> u32 {
        30
    }
}

impl Default for GcConfig {
    fn default() -> Self {
        Self { quarantine_ttl_days: Self::default_quarantine_ttl_days(), auto_gc: false }
    }
}

/// `[scan]` section
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScanConfig {
    /// File extensions (without the leading dot) treated as model files.
    /// Empty means `scanner::MODEL_EXTENSIONS` (the built-in default list).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
    /// Re-hash only files whose size/mtime changed since the last scan (default:
    /// true). The WebUI scan endpoint uses this unless the request says otherwise.
    #[serde(default = "ScanConfig::default_incremental")]
    pub incremental: bool,
}

impl ScanConfig {
    fn default_incremental() -> bool {
        true
    }
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self { extensions: Vec::new(), incremental: true }
    }
}

/// [auth] section
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AuthConfig {
    /// Bearer token for API authentication.  Empty string = not set.
    /// **Never log this value.**
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// IP CIDR allow-list (empty = allow all)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_ips: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// [proxy] section
// ─────────────────────────────────────────────────────────────────────────────

/// Largest byte range served in a single `206` response (64 MiB).
pub const DEFAULT_RANGE_CHUNK_BYTES: u64 = 64 * 1024 * 1024;

/// `[proxy]` — the local registry / proxy server.
///
/// Every field has a safe default, so a minimal `modeld.toml` (or none at all)
/// starts a **closed** server on `127.0.0.1:8234`: anonymous requests get 401.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProxyConfig {
    pub port: u16,
    pub bind_address: String,
    /// Overrides the store root when the proxy is started without `--store`.
    /// Not written back unless it differs from the default, since the real store
    /// path is resolved by the CLI.
    #[serde(skip_serializing_if = "is_default_proxy_store_path")]
    pub store_path: std::path::PathBuf,
    pub auth: ProxyAuthConfig,
    pub network: ProxyNetworkConfig,
    /// Largest byte range served in a single `206` response. A `Range: bytes=0-`
    /// request for a 20 GB model would otherwise be one 20 GB response.
    pub range_chunk_bytes: u64,
}

fn is_default_proxy_store_path(path: &std::path::Path) -> bool {
    path == std::path::Path::new(".modeld")
}

/// `[proxy.auth]` — bearer-token access control.
///
/// Secure by default: a proxy started without any token configuration refuses
/// every authenticated route until a token exists. The CLI generates and
/// persists one on first start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProxyAuthConfig {
    /// When true, requests must carry a valid `Authorization: Bearer <token>`.
    pub require_token: bool,
    /// Accepted bearer tokens.
    pub tokens: Vec<String>,
}

impl Default for ProxyAuthConfig {
    fn default() -> Self {
        Self { require_token: true, tokens: Vec::new() }
    }
}

/// `[proxy.network]` — IP allow/deny lists.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct ProxyNetworkConfig {
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
            store_path: std::path::PathBuf::from(".modeld"),
            auth: ProxyAuthConfig::default(),
            network: ProxyNetworkConfig::default(),
            range_chunk_bytes: DEFAULT_RANGE_CHUNK_BYTES,
        }
    }
}

impl ProxyConfig {
    /// Load `[proxy]` from a `modeld.toml` **file path**.
    ///
    /// If the file or the `[proxy]` table is absent, returns the safe default.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let parsed: ModeldConfig = toml::from_str(&raw)
            .with_context(|| format!("Failed to parse TOML config: {}", path.display()))?;
        Ok(parsed.proxy)
    }

    /// Build a config from CLI overrides applied on top of defaults.
    pub fn from_cli(
        port: Option<u16>,
        bind: Option<String>,
        store_path: std::path::PathBuf,
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

// ─────────────────────────────────────────────────────────────────────────────
// Root config struct
// ─────────────────────────────────────────────────────────────────────────────

/// Root structure that maps to the `modeld.toml` file.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModeldConfig {
    #[serde(default)]
    pub store: StoreConfig,
    #[serde(default)]
    pub serve: ServeConfig,
    #[serde(default)]
    pub dedup: DedupConfig,
    #[serde(default)]
    pub gc: GcConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub scan: ScanConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
}

impl ModeldConfig {
    /// Effective bearer tokens for the proxy, from either schema.
    ///
    /// `[proxy.auth] tokens` wins when it is non-empty; otherwise the legacy
    /// `[auth] token` is used. Before A6 the two were written independently, so a
    /// `token-rotate` that only touched `[auth]` left the proxy reading a stale
    /// `[proxy.auth]` list — see `set_proxy_token`.
    pub fn proxy_tokens(&self) -> Vec<String> {
        if !self.proxy.auth.tokens.is_empty() {
            self.proxy.auth.tokens.clone()
        } else if !self.auth.token.is_empty() {
            vec![self.auth.token.clone()]
        } else {
            Vec::new()
        }
    }

    /// Set the proxy bearer token in **both** schemas.
    ///
    /// Keeps `[auth] token` (read by older versions and by `MODELD_AUTH_TOKEN`
    /// users) and `[proxy.auth]` in sync, and turns token auth on. Callers persist
    /// with `save_config`.
    pub fn set_proxy_token(&mut self, token: &str) {
        self.auth.token = token.to_string();
        self.proxy.auth.tokens = vec![token.to_string()];
        self.proxy.auth.require_token = true;
        self.proxy.network.allow_anonymous = false;
    }

    /// Extensions the scanner should accept, honouring `[scan] extensions`.
    ///
    /// Entries are normalised: a leading dot is stripped and case is lowered, so
    /// `[".ONNX"]` and `["onnx"]` behave the same.
    pub fn scan_extensions(&self) -> Vec<String> {
        if self.scan.extensions.is_empty() {
            return crate::scanner::MODEL_EXTENSIONS.iter().map(|e| e.to_string()).collect();
        }
        self.scan
            .extensions
            .iter()
            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Load / save helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Load `modeld.toml` from `<store_path>/modeld.toml`.
///
/// If the file does not exist, returns `ModeldConfig::default()` so callers
/// don't have to special-case a first-run scenario.
pub fn load_config(store_path: &Path) -> Result<ModeldConfig> {
    let toml_path = store_path.join("modeld.toml");

    if !toml_path.exists() {
        return Ok(ModeldConfig::default());
    }

    let contents = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("Failed to read config file: {}", toml_path.display()))?;

    let config: ModeldConfig = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {}", toml_path.display()))?;

    Ok(config)
}

/// Serialize `config` and merge it into `<store_path>/modeld.toml`.
///
/// **Only the sections this crate owns are rewritten.** Any other table — a
/// `[proxy]` section written by an older version, an experimental
/// `[my.experiment]` block, or a comment in an untouched section — is preserved.
/// The previous implementation serialized the whole struct and overwrote the
/// file, silently deleting every section it did not know about.
///
/// The parent directory is created if it does not yet exist. The write is
/// atomic (temporary file + rename) so a crash cannot truncate the config.
pub fn save_config(store_path: &Path, config: &ModeldConfig) -> Result<()> {
    std::fs::create_dir_all(store_path)
        .with_context(|| format!("Failed to create store directory: {}", store_path.display()))?;

    let toml_path = store_path.join("modeld.toml");

    // Parse the existing document (comments and unknown tables included), then
    // replace only the tables we serialize.
    let mut doc = match std::fs::read_to_string(&toml_path) {
        Ok(existing) => existing.parse::<toml_edit::DocumentMut>().with_context(|| {
            format!("Failed to parse existing config file: {}", toml_path.display())
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to read config file: {}", toml_path.display()))
        }
    };

    let fresh: toml_edit::DocumentMut = toml::to_string_pretty(config)
        .context("Failed to serialize config to TOML")?
        .parse()
        .context("Serialized config was not valid TOML")?;

    for (key, item) in fresh.iter() {
        // Don't introduce empty tables (`[store]`, `[auth]`, …) that carry no
        // information; keep them only if the file already had them.
        let empty_new_table = matches!(item, toml_edit::Item::Table(t) if t.is_empty());
        if empty_new_table && doc.get(key).is_none() {
            continue;
        }

        let mut replaced = item.clone();
        // Keep the decor (the comment block above a table header, blank lines)
        // that belonged to the section we are replacing.
        if let Some(existing) = doc.get(key) {
            copy_decor(existing, &mut replaced);
        }
        doc[key] = replaced;
    }

    write_atomic(&toml_path, doc.to_string().as_bytes())
}

/// Copy the surrounding comments/blank lines from one TOML item to another.
fn copy_decor(from: &toml_edit::Item, to: &mut toml_edit::Item) {
    use toml_edit::Item;
    match (from, to) {
        (Item::Table(src), Item::Table(dst)) => *dst.decor_mut() = src.decor().clone(),
        (Item::Value(src), Item::Value(dst)) => *dst.decor_mut() = src.decor().clone(),
        // Arrays of tables keep their own formatting; nothing to copy.
        _ => {}
    }
}

/// Write `bytes` to `path` via a sibling temporary file + rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(".modeld_tmp_{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, bytes)
        .with_context(|| format!("Failed to write temporary config: {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("Failed to replace config: {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let config = ModeldConfig::default();
        save_config(tmp.path(), &config).unwrap();

        let loaded = load_config(tmp.path()).unwrap();
        assert_eq!(loaded, config);
    }

    #[test]
    fn test_roundtrip_preserves_unknown_sections() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("modeld.toml"),
            r#"# hand-written comment, keep me
[serve]
port = 8234

[proxy]
port = 9999

[proxy.auth]
tokens = ["old-token"]

[my.experiment]
enabled = true
"#,
        )
        .unwrap();

        let mut config = load_config(tmp.path()).unwrap();
        assert_eq!(config.proxy.port, 9999);
        config.serve.port = 8500;
        save_config(tmp.path(), &config).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("modeld.toml")).unwrap();
        // The section we changed...
        assert!(raw.contains("8500"), "new value missing:\n{raw}");
        // ...and everything we did not touch.
        assert!(raw.contains("[my.experiment]"), "unknown section dropped:\n{raw}");
        assert!(raw.contains("enabled = true"), "unknown key dropped:\n{raw}");
        assert!(raw.contains("[proxy]"), "[proxy] section dropped:\n{raw}");
        assert!(raw.contains("9999"), "[proxy] port dropped:\n{raw}");
        assert!(raw.contains("# hand-written comment, keep me"), "comment dropped:\n{raw}");
    }

    #[test]
    fn test_save_does_not_write_a_placeholder_store_path() {
        let tmp = TempDir::new().unwrap();
        save_config(tmp.path(), &ModeldConfig::default()).unwrap();
        let raw = std::fs::read_to_string(tmp.path().join("modeld.toml")).unwrap();
        assert!(
            !raw.contains(".modeld"),
            "the proxy's default store_path is a placeholder and must not be persisted:\n{raw}"
        );
    }

    #[test]
    fn test_token_rotate_updates_both_sections() {
        let tmp = TempDir::new().unwrap();
        let mut config = ModeldConfig::default();
        config.auth.token = "old-token".to_string();
        config.proxy.auth.tokens = vec!["old-token".to_string()];
        save_config(tmp.path(), &config).unwrap();

        let mut config = load_config(tmp.path()).unwrap();
        config.set_proxy_token("new-token");
        save_config(tmp.path(), &config).unwrap();

        let reloaded = load_config(tmp.path()).unwrap();
        assert_eq!(reloaded.auth.token, "new-token", "[auth] token not rotated");
        assert_eq!(
            reloaded.proxy.auth.tokens,
            vec!["new-token".to_string()],
            "[proxy.auth] tokens not rotated"
        );
        assert!(reloaded.proxy.auth.require_token, "token auth must stay on");
        assert_eq!(reloaded.proxy_tokens(), vec!["new-token".to_string()]);
        assert!(!reloaded.proxy_tokens().contains(&"old-token".to_string()));
    }

    #[test]
    fn test_proxy_tokens_falls_back_to_the_legacy_section() {
        let mut config = ModeldConfig::default();
        config.auth.token = "legacy".to_string();
        assert_eq!(config.proxy_tokens(), vec!["legacy".to_string()]);

        // A configured [proxy.auth] list wins over the legacy single token.
        config.proxy.auth.tokens = vec!["modern".to_string()];
        assert_eq!(config.proxy_tokens(), vec!["modern".to_string()]);
    }

    #[test]
    fn test_old_config_file_loads_with_defaults() {
        // A pre-A6 file: no [proxy], no [scan], no [auth].
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("modeld.toml"),
            "[store]\npath = \"/data\"\n\n[serve]\nhost = \"0.0.0.0\"\nport = 9000\n",
        )
        .unwrap();

        let config = load_config(tmp.path()).unwrap();
        assert_eq!(config.serve.port, 9000);
        assert_eq!(config.store.path.as_deref(), Some("/data"));
        // Missing sections fall back to safe defaults.
        assert!(config.proxy.auth.require_token);
        assert_eq!(config.proxy.bind_address, "127.0.0.1");
        assert_eq!(config.scan_extensions().len(), crate::scanner::MODEL_EXTENSIONS.len());
    }

    #[test]
    fn test_scan_extensions_default_to_the_builtin_list() {
        let config = ModeldConfig::default();
        let extensions = config.scan_extensions();
        // Weights and exported graph formats are both covered by default.
        for expected in
            ["safetensors", "gguf", "ckpt", "onnx", "tflite", "h5", "pb", "engine", "npz"]
        {
            assert!(extensions.contains(&expected.to_string()), "missing default {expected}");
        }
    }

    #[test]
    fn test_scan_extensions_from_config_are_normalised() {
        let mut config = ModeldConfig::default();
        config.scan.extensions = vec![".ONNX".to_string(), " TFLite ".to_string(), "".to_string()];
        assert_eq!(config.scan_extensions(), vec!["onnx".to_string(), "tflite".to_string()]);
    }

    #[test]
    fn test_scan_section_parses_from_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("modeld.toml"),
            "[scan]\nextensions = [\".safetensors\", \"onnx\"]\n",
        )
        .unwrap();
        let config = load_config(tmp.path()).unwrap();
        assert_eq!(config.scan_extensions(), vec!["safetensors".to_string(), "onnx".to_string()]);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let tmp = TempDir::new().unwrap();
        let config = load_config(tmp.path()).unwrap();
        assert_eq!(config, ModeldConfig::default());
    }

    #[test]
    fn test_partial_toml_fills_defaults() {
        let tmp = TempDir::new().unwrap();
        // Write only the [serve] section
        std::fs::write(tmp.path().join("modeld.toml"), "[serve]\nport = 9000\n").unwrap();

        let config = load_config(tmp.path()).unwrap();
        assert_eq!(config.serve.port, 9000);
        assert_eq!(config.serve.host, "127.0.0.1"); // default preserved
        assert_eq!(config.gc.quarantine_ttl_days, 30); // sibling section default
    }

    #[test]
    fn test_store_path_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mut config = ModeldConfig::default();
        config.store.path = Some("/custom/store".to_string());
        save_config(tmp.path(), &config).unwrap();

        let loaded = load_config(tmp.path()).unwrap();
        assert_eq!(loaded.store.path.as_deref(), Some("/custom/store"));
    }

    #[test]
    fn test_auth_token_not_serialized_when_empty() {
        let tmp = TempDir::new().unwrap();
        let config = ModeldConfig::default();
        save_config(tmp.path(), &config).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("modeld.toml")).unwrap();
        // An empty token must not appear as a value in the file (a placeholder
        // like `token = ""` would be copied around and look configured).
        assert!(
            !raw.lines().any(|line| line.trim() == "token = \"\""),
            "empty token should be omitted from TOML:\n{raw}"
        );
    }

    #[test]
    fn test_save_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        let config = ModeldConfig::default();
        save_config(&nested, &config).unwrap();
        assert!(nested.join("modeld.toml").exists());
    }
}
