//! `modeld config …` — read and write `modeld.toml`.

use crate::{load_config, resolve_store, save_config};
use anyhow::{Context, Result};
use clap::Subcommand;
use colored::*;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Read a configuration key from modeld.toml
    Get {
        /// Key in dot notation, e.g. store.path, serve.port
        key: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Write a configuration value to modeld.toml
    Set {
        /// Key in dot notation, e.g. serve.port
        key: String,
        /// New value
        value: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// List every configurable key and its current value
    List {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

pub fn config_command(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Get { key, store } => {
            let store_path = resolve_store(store);
            let config = load_config(&store_path)?;
            let value = config_get_value(&config, &key)?;
            println!("{}", value);
            Ok(())
        }
        ConfigAction::Set { key, value, store } => {
            let store_path = resolve_store(store);
            let mut config = load_config(&store_path)?;
            config_set_value(&mut config, &key, &value)?;
            // `save_config` merges into the existing document, so unrelated
            // sections (`[proxy]`, experiments, comments) survive (A6).
            save_config(&store_path, &config)?;
            // Config changes are security-relevant: record them, but never let a
            // failed audit write fail the command.
            modeld_core::audit::record(
                &store_path,
                modeld_core::AuditEntry {
                    details: Some(if key == "auth.token" || key == "proxy.auth.tokens" {
                        "value redacted".to_string()
                    } else {
                        value.clone()
                    }),
                    ..modeld_core::AuditEntry::success("config_change", key.clone())
                },
            );
            println!("{} {} = {}", "✓".green().bold(), key.bold(), value);
            Ok(())
        }
        ConfigAction::List { store } => {
            let store_path = resolve_store(store);
            let config = load_config(&store_path)?;
            println!(
                "{} {}",
                "modeld config".cyan().bold(),
                store_path.join("modeld.toml").display().to_string().dimmed()
            );
            println!("{}", "─".repeat(60).cyan());
            for key in CONFIG_KEYS {
                // Every key in CONFIG_KEYS is handled by config_get_value.
                let value = config_get_value(&config, key).unwrap_or_else(|_| "<error>".into());
                println!("  {:<28} {}", key, value);
            }
            Ok(())
        }
    }
}

/// Every key `modeld config` can read or write, in display order.
const CONFIG_KEYS: &[&str] = &[
    "store.path",
    "serve.host",
    "serve.port",
    "serve.webui",
    "serve.proxy",
    "serve.open_browser",
    "dedup.strategy",
    "dedup.min_size_bytes",
    "gc.quarantine_ttl_days",
    "gc.auto_gc",
    "auth.token",
    "auth.allow_ips",
    "scan.extensions",
    "scan.incremental",
    "proxy.port",
    "proxy.bind_address",
    "proxy.range_chunk_bytes",
    "proxy.auth.require_token",
    "proxy.auth.tokens",
    "proxy.network.allow_anonymous",
    "proxy.network.allowed_ips",
    "proxy.network.denied_ips",
];

/// Levenshtein distance, used only for "did you mean" suggestions.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Closest known key to `input`, if it is close enough to be worth suggesting.
fn suggest_key(input: &str) -> Option<&'static str> {
    let lowered = input.to_ascii_lowercase();
    let mut best: Option<(usize, &'static str)> = None;
    for key in CONFIG_KEYS {
        let distance = edit_distance(&lowered, key);
        if best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, key));
        }
    }
    match best {
        // Accept up to a third of the key length (e.g. "serve.prot" → "serve.port").
        Some((distance, key)) if distance * 3 <= key.len() => Some(key),
        _ => None,
    }
}

fn unknown_config_key(key: &str) -> anyhow::Error {
    let hint = match suggest_key(key) {
        Some(candidate) => format!(" Did you mean '{}'?", candidate),
        None => String::new(),
    };
    anyhow::anyhow!(
        "Unknown config key: '{}'.{hint} Run 'modeld config list' to see all keys.",
        key
    )
}

fn parse_list(value: &str) -> Vec<String> {
    value.split(',').map(|item| item.trim().to_string()).filter(|item| !item.is_empty()).collect()
}

fn parse_bool(key: &str, value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => anyhow::bail!("Invalid boolean '{}' for {}: use true or false", other, key),
    }
}

/// Tokens are secrets: `config get` prints a fingerprint, not the value.
fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        "<not set>".to_string()
    } else if value.len() <= 8 {
        "********".to_string()
    } else {
        format!("{}…{} (masked)", &value[..4], &value[value.len() - 4..])
    }
}

fn config_get_value(config: &modeld_core::ModeldConfig, key: &str) -> Result<String> {
    match key {
        "store.path" => Ok(config.store.path.as_deref().unwrap_or("<not set>").to_string()),
        "serve.host" => Ok(config.serve.host.clone()),
        "serve.port" => Ok(config.serve.port.to_string()),
        "serve.webui" => Ok(config.serve.webui.to_string()),
        "serve.proxy" => Ok(config.serve.proxy.to_string()),
        "serve.open_browser" => Ok(config.serve.open_browser.to_string()),
        "dedup.strategy" => Ok(config.dedup.strategy.clone()),
        "dedup.min_size_bytes" => Ok(config.dedup.min_size_bytes.to_string()),
        "gc.quarantine_ttl_days" => Ok(config.gc.quarantine_ttl_days.to_string()),
        "gc.auto_gc" => Ok(config.gc.auto_gc.to_string()),
        "auth.token" => Ok(mask_secret(&config.auth.token)),
        "auth.allow_ips" => Ok(config.auth.allow_ips.join(",")),
        "scan.extensions" => Ok(config.scan_extensions().join(",")),
        "scan.incremental" => Ok(config.scan.incremental.to_string()),
        "proxy.port" => Ok(config.proxy.port.to_string()),
        "proxy.bind_address" => Ok(config.proxy.bind_address.clone()),
        "proxy.range_chunk_bytes" => Ok(config.proxy.range_chunk_bytes.to_string()),
        "proxy.auth.require_token" => Ok(config.proxy.auth.require_token.to_string()),
        "proxy.auth.tokens" => Ok(mask_secret(&config.proxy.auth.tokens.join(","))),
        "proxy.network.allow_anonymous" => Ok(config.proxy.network.allow_anonymous.to_string()),
        "proxy.network.allowed_ips" => Ok(config.proxy.network.allowed_ips.join(",")),
        "proxy.network.denied_ips" => Ok(config.proxy.network.denied_ips.join(",")),
        other => Err(unknown_config_key(other)),
    }
}

/// Write a config value by dotted key.
fn config_set_value(config: &mut modeld_core::ModeldConfig, key: &str, value: &str) -> Result<()> {
    match key {
        "store.path" => {
            config.store.path = if value.is_empty() { None } else { Some(value.to_string()) };
        }
        "serve.host" => {
            config.serve.host = value.to_string();
        }
        "serve.port" => {
            config.serve.port =
                value.parse::<u16>().with_context(|| format!("Invalid port number: {}", value))?;
        }
        "serve.webui" => config.serve.webui = parse_bool(key, value)?,
        "serve.proxy" => config.serve.proxy = parse_bool(key, value)?,
        "serve.open_browser" => config.serve.open_browser = parse_bool(key, value)?,
        "dedup.strategy" => match value {
            "hardlink" | "symlink" | "copy_to_cas" | "virtual_alias" => {
                config.dedup.strategy = value.to_string();
            }
            other => anyhow::bail!(
                "Invalid dedup strategy '{}'. Valid: hardlink, symlink, copy_to_cas, virtual_alias",
                other
            ),
        },
        "dedup.min_size_bytes" => {
            config.dedup.min_size_bytes = value
                .parse::<u64>()
                .with_context(|| format!("Invalid size in bytes: {}", value))?;
        }
        "gc.quarantine_ttl_days" => {
            config.gc.quarantine_ttl_days =
                value.parse::<u32>().with_context(|| format!("Invalid TTL days: {}", value))?;
        }
        "gc.auto_gc" => config.gc.auto_gc = parse_bool(key, value)?,
        // The token exists in two schemas; write both so the proxy and older
        // readers agree (A6).
        "auth.token" => {
            if value.is_empty() {
                config.auth.token.clear();
                config.proxy.auth.tokens.clear();
            } else {
                config.set_proxy_token(value);
            }
        }
        "auth.allow_ips" => config.auth.allow_ips = parse_list(value),
        "scan.extensions" => config.scan.extensions = parse_list(value),
        "scan.incremental" => config.scan.incremental = parse_bool(key, value)?,
        "proxy.port" => {
            config.proxy.port =
                value.parse::<u16>().with_context(|| format!("Invalid port number: {}", value))?;
        }
        "proxy.bind_address" => config.proxy.bind_address = value.to_string(),
        "proxy.range_chunk_bytes" => {
            config.proxy.range_chunk_bytes =
                value.parse::<u64>().with_context(|| format!("Invalid byte count: {}", value))?;
        }
        "proxy.auth.require_token" => {
            config.proxy.auth.require_token = parse_bool(key, value)?;
        }
        "proxy.auth.tokens" => {
            let tokens = parse_list(value);
            config.auth.token = tokens.first().cloned().unwrap_or_default();
            config.proxy.auth.tokens = tokens;
        }
        "proxy.network.allow_anonymous" => {
            config.proxy.network.allow_anonymous = parse_bool(key, value)?;
        }
        "proxy.network.allowed_ips" => config.proxy.network.allowed_ips = parse_list(value),
        "proxy.network.denied_ips" => config.proxy.network.denied_ips = parse_list(value),
        other => return Err(unknown_config_key(other)),
    }
    Ok(())
}

// =============================================================================
// `modeld hf` command — HuggingFace Hub integration
// =============================================================================
