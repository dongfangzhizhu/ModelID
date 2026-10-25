//! `modeld serve` — run the embedded Web UI.

use crate::load_config;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Start the embedded Web UI server.
///
/// If no auth token is configured one is auto-generated and printed to stdout.
/// The token value is **never** written to a log file.
use modeld_core::Database;

pub fn serve_command(
    host: String,
    port: Option<u16>,
    open_browser: bool,
    store_path: PathBuf,
    _read_only: bool,
    allow_lan: bool,
) -> Result<()> {
    use colored::Colorize;

    let port = port.unwrap_or(8234);

    // Exposing the UI on the network must be an explicit decision, matching
    // `modeld-webui` and `modeld proxy start`.
    let loopback = modeld_proxy::config::is_loopback_addr(&host);
    if !loopback && !allow_lan {
        anyhow::bail!(
            "refusing to bind the Web UI to '{}': pass --allow-lan to expose it on the network \
             (a bearer token is always enforced)",
            host
        );
    }

    // Warn loudly when binding to all interfaces.
    if !loopback {
        println!(
            "{}",
            "⚠  Warning: the Web UI is reachable from the network; a bearer token is required."
                .yellow()
                .bold()
        );
        println!(
            "{}",
            "   Restrict access with a firewall if this machine is reachable from the internet."
                .yellow()
        );
        println!();
    }

    // Ensure the store is initialized.
    std::fs::create_dir_all(&store_path)?;
    let db_path = store_path.join("modeld.db");
    let db = Database::open(&db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;

    // Load existing config. Token resolution: `modeld.toml` → `MODELD_AUTH_TOKEN` →
    // generate one for this session. The env var is never written to disk, so a
    // container can inject a secret without it ending up in the store.
    let mut config = load_config(&store_path).unwrap_or_default();
    let env_token = std::env::var("MODELD_AUTH_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    let auth_token = if !config.auth.token.is_empty() {
        config.auth.token.clone()
    } else if let Some(token) = env_token {
        println!("{}", "🔑  Using the token from MODELD_AUTH_TOKEN.".cyan().bold());
        println!();
        token
    } else {
        let new_token = uuid::Uuid::new_v4().to_string();
        config.auth.token = new_token.clone();
        // Print to console only — NOT logged anywhere.
        println!(
            "{}",
            "🔑  No auth token configured. Auto-generated token for this session:".cyan().bold()
        );
        println!("    {}", new_token.yellow().bold());
        println!();
        println!("{}", "   Pass it in the Authorization header:".dimmed());
        println!("   {}", format!("Authorization: Bearer {}", new_token).dimmed());
        println!();
        new_token
    };

    let (state, _rx) = modeld_webui::AppState::new(db, store_path, Some(auth_token), None);

    let open_browser = open_browser || config.serve.open_browser;
    let webui_config = modeld_webui::WebUiConfig { host, port, open_browser };

    // Run the async server on a fresh Tokio runtime.
    tokio::runtime::Runtime::new()
        .context("Failed to create Tokio runtime")?
        .block_on(modeld_webui::run(state, webui_config))
}

// ─────────────────────────────────────────────────────────────────────────────

// ── Transaction management ────────────────────────────────────────────────────

// ── Database management ───────────────────────────────────────────────────────
