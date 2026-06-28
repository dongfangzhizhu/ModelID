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

    // Validate store path
    if !args.store.exists() {
        eprintln!(
            "⚠️  Store path does not exist: {}\n   Create it with: modeld init --store {}",
            args.store.display(),
            args.store.display()
        );
        std::process::exit(1);
    }

    // Open the SQLite database
    let db_path = args.store.join("metadata.db");
    let db = modeld_core::Database::open(&db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;

    let (state, _) = modeld_webui::AppState::new(db, args.store, None, None);

    let config = modeld_webui::WebUiConfig {
        host: args.host,
        port: args.port,
        open_browser: args.open,
    };

    modeld_webui::run(state, config).await
}
