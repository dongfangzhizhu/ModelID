use anyhow::Result;
use axum::{routing::get, Router};
use tower_http::cors::CorsLayer;
use tower_http::compression::CompressionLayer;

use crate::api;
use crate::state::AppState;
use crate::static_files::static_handler;
use crate::ws::ws_handler;

/// Configuration for the Web UI server.
#[derive(Clone)]
pub struct WebUiConfig {
    pub host: String,
    pub port: u16,
    pub open_browser: bool,
}

impl Default for WebUiConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8234,
            open_browser: false,
        }
    }
}

/// Build and run the axum HTTP server.
pub async fn run(state: AppState, config: WebUiConfig) -> Result<()> {
    let app = Router::new()
        // REST API routes (mounted under /api/v1)
        .nest("/api/v1", api::routes())
        // WebSocket endpoint
        .route("/ws", get(ws_handler))
        // Static file fallback (serves embedded UI)
        .fallback(static_handler)
        // Provide shared state to ALL routes
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new());

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    let url = format!("http://{}", addr);
    tracing::info!("modeld Web UI listening on {}", url);
    println!("🌐  modeld Web UI: {}", url);

    if config.open_browser {
        let _ = open_browser(&url);
    }

    axum::serve(listener, app).await?;
    Ok(())
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd").args(["/c", "start", url]).spawn()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(url).spawn()?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(url).spawn()?;
    Ok(())
}
