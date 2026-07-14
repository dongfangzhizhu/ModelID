//! Cross-platform store path resolver
//!
//! Priority chain (highest to lowest):
//!   1. CLI override (`--store <path>`)
//!   2. `MODELD_STORE` environment variable
//!   3. `store.path` key inside `<default_store>/modeld.toml`
//!   4. Platform default path

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Resolve the store path using the priority chain:
/// CLI override > `MODELD_STORE` env var > modeld.toml `store.path` > platform default.
///
/// The `modeld.toml` lookup reads only the minimal `[store]` section from the
/// *platform default* location, so the caller does not need to pre-initialise
/// the config.
pub fn resolve_store_path(cli_override: Option<&Path>) -> PathBuf {
    // 1. CLI override
    if let Some(p) = cli_override {
        return p.to_path_buf();
    }

    // 2. MODELD_STORE environment variable
    if let Ok(env_val) = std::env::var("MODELD_STORE") {
        if !env_val.is_empty() {
            return PathBuf::from(env_val);
        }
    }

    // 3. store.path key inside modeld.toml located at the platform default
    let default = default_store_path();
    if let Ok(path) = read_store_path_from_toml(&default) {
        return path;
    }

    // 4. Platform default
    default
}

/// Return the platform-appropriate default store directory.
///
/// | Platform | Path |
/// |---|---|
/// | Windows  | `%LOCALAPPDATA%\modeld` |
/// | macOS    | `~/Library/Application Support/modeld` |
/// | Linux    | `$XDG_DATA_HOME/modeld` or `~/.local/share/modeld` |
pub fn default_store_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // %LOCALAPPDATA%\modeld (e.g. C:\Users\Alice\AppData\Local\modeld)
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("modeld");
        }
        // Fallback: USERPROFILE\AppData\Local\modeld
        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            return PathBuf::from(user_profile).join("AppData").join("Local").join("modeld");
        }
        // Last resort
        PathBuf::from(r"C:\modeld")
    }

    #[cfg(target_os = "macos")]
    {
        // ~/Library/Application Support/modeld
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join("Library").join("Application Support").join("modeld");
        }
        PathBuf::from("/tmp/modeld")
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux / other Unix: $XDG_DATA_HOME/modeld or ~/.local/share/modeld
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("modeld");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".local").join("share").join("modeld");
        }
        PathBuf::from("/tmp/modeld")
    }
}

/// Read just the `[store] path` value from `<store_path>/modeld.toml`, if present.
fn read_store_path_from_toml(store_path: &Path) -> Result<PathBuf> {
    let toml_path = store_path.join("modeld.toml");
    if !toml_path.exists() {
        anyhow::bail!("modeld.toml not found at {}", toml_path.display());
    }

    let contents = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("Failed to read {}", toml_path.display()))?;

    let table: toml::Table =
        contents.parse().with_context(|| format!("Failed to parse {}", toml_path.display()))?;

    if let Some(store_section) = table.get("store").and_then(|v| v.as_table()) {
        if let Some(path_val) = store_section.get("path").and_then(|v| v.as_str()) {
            if !path_val.is_empty() {
                return Ok(PathBuf::from(path_val));
            }
        }
    }

    anyhow::bail!("store.path not set in modeld.toml");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_default_store_path_not_empty() {
        let p = default_store_path();
        assert!(!p.as_os_str().is_empty(), "default store path must not be empty");
        // The path should end with "modeld"
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("modeld"));
    }

    #[test]
    fn test_cli_override_takes_priority() {
        let tmp = TempDir::new().unwrap();
        let cli_path = tmp.path().join("cli_store");
        let result = resolve_store_path(Some(&cli_path));
        assert_eq!(result, cli_path);
    }

    #[test]
    fn test_env_var_takes_priority_over_default() {
        // Save and restore the env var so we don't pollute other tests
        let original = std::env::var("MODELD_STORE").ok();
        let tmp = TempDir::new().unwrap();
        let env_path = tmp.path().join("env_store");

        std::env::set_var("MODELD_STORE", env_path.to_str().unwrap());
        let result = resolve_store_path(None);
        // Restore
        match original {
            Some(v) => std::env::set_var("MODELD_STORE", v),
            None => std::env::remove_var("MODELD_STORE"),
        }
        assert_eq!(result, env_path);
    }

    #[test]
    fn test_cli_beats_env_var() {
        let original = std::env::var("MODELD_STORE").ok();
        let tmp = TempDir::new().unwrap();
        let cli_path = tmp.path().join("cli_store");
        let env_path = tmp.path().join("env_store");

        std::env::set_var("MODELD_STORE", env_path.to_str().unwrap());
        let result = resolve_store_path(Some(&cli_path));
        match original {
            Some(v) => std::env::set_var("MODELD_STORE", v),
            None => std::env::remove_var("MODELD_STORE"),
        }
        assert_eq!(result, cli_path);
    }

    #[test]
    fn test_toml_store_path_fallback() {
        // Ensure MODELD_STORE is not set for this test
        let original = std::env::var("MODELD_STORE").ok();
        std::env::remove_var("MODELD_STORE");

        let tmp = TempDir::new().unwrap();
        let toml_store = tmp.path().join("toml_store");

        // Write a minimal modeld.toml into tmp (which acts as the "default" store)
        let toml_content =
            format!("[store]\npath = \"{}\"\n", toml_store.to_str().unwrap().replace('\\', "\\\\"));
        let toml_path = tmp.path().join("modeld.toml");
        let mut f = std::fs::File::create(&toml_path).unwrap();
        f.write_all(toml_content.as_bytes()).unwrap();

        let result = read_store_path_from_toml(tmp.path()).unwrap();

        // Restore
        match original {
            Some(v) => std::env::set_var("MODELD_STORE", v),
            None => std::env::remove_var("MODELD_STORE"),
        }

        assert_eq!(result, toml_store);
    }
}
