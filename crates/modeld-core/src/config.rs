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

/// Serialize `config` and write it to `<store_path>/modeld.toml`.
///
/// The parent directory is created if it does not yet exist.
pub fn save_config(store_path: &Path, config: &ModeldConfig) -> Result<()> {
    std::fs::create_dir_all(store_path)
        .with_context(|| format!("Failed to create store directory: {}", store_path.display()))?;

    let toml_path = store_path.join("modeld.toml");

    let contents = toml::to_string_pretty(config).context("Failed to serialize config to TOML")?;

    std::fs::write(&toml_path, contents)
        .with_context(|| format!("Failed to write config file: {}", toml_path.display()))?;

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
        // Empty token must not appear in the file to avoid leaking placeholder
        assert!(!raw.contains("token"), "empty token should be omitted from TOML");
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
