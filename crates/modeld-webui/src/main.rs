use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "modeld-webui", about = "modeld Web UI server")]
struct Args {
    /// Path to the modeld store directory
    #[arg(long, short, default_value = ".modeld")]
    store: PathBuf,

    /// Port to listen on
    #[arg(long, short, default_value_t = 8234)]
    port: u16,

    /// Host to bind to (default: 127.0.0.1, use 0.0.0.0 for LAN access)
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Open browser after starting
    #[arg(long)]
    open: bool,

    /// Bearer token required by the API (or set MODELD_AUTH_TOKEN)
    #[arg(long)]
    token: Option<String>,

    /// Allow binding to a non-loopback address (requires a token)
    #[arg(long, default_value = "false")]
    allow_lan: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "modeld_webui=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();

    // Resolve the token: --token wins over the environment.
    let token = args
        .token
        .clone()
        .or_else(|| std::env::var("MODELD_AUTH_TOKEN").ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    // Refuse to expose the UI on the network without authentication.
    if !is_loopback(&args.host) {
        if !args.allow_lan {
            eprintln!(
                "error: refusing to bind the Web UI to '{}' without --allow-lan.\n\
                 hint: keep the default 127.0.0.1, or pass --allow-lan together with --token.",
                args.host
            );
            std::process::exit(1);
        }
        if token.is_none() {
            eprintln!(
                "error: refusing to bind the Web UI to '{}' without a token.\n\
                 hint: pass --token <token> or set MODELD_AUTH_TOKEN.",
                args.host
            );
            std::process::exit(1);
        }
    }

    // Validate store path
    if !args.store.exists() {
        eprintln!(
            "⚠️  Store path does not exist: {}\n   Create it with: modeld init --store {}",
            args.store.display(),
            args.store.display()
        );
        std::process::exit(1);
    }

    // Open the SQLite database. The store layout is shared with the CLI, so the
    // file name must match what `modeld init` / `modeld scan` create.
    let db_path = args.store.join("modeld.db");
    let is_new = !db_path.exists();
    let db = modeld_core::Database::open(&db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;

    if is_new {
        eprintln!(
            "note: created a new store database at {}\n      \
             run `modeld scan <path>` to index existing models (or `modeld init` for an empty store).",
            db_path.display()
        );
    }

    // `[serve] open_browser` in modeld.toml is a persisted default; the CLI
    // flag still wins.
    let open_browser = args.open
        || modeld_core::load_config(&args.store).map(|cfg| cfg.serve.open_browser).unwrap_or(false);

    let (state, _) = modeld_webui::AppState::new(db, args.store, token, None);

    let config = modeld_webui::WebUiConfig { host: args.host, port: args.port, open_browser };

    modeld_webui::run(state, config).await
}

/// True for `127.0.0.0/8`, `::1` and `localhost`.
fn is_loopback(host: &str) -> bool {
    let host = host.trim();
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host == "[::1]"
        || host == "127.0.0.1"
        || host.starts_with("127.")
}
