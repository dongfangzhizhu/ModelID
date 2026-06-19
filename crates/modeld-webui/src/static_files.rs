use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "ui/"]
#[include = "*.html"]
#[include = "*.js"]
#[include = "*.css"]
#[include = "*.ico"]
#[include = "*.svg"]
#[include = "*.png"]
struct Assets;

/// Static file handler — serves embedded UI files, falls back to index.html for SPA routing.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Redirect root to index.html
    let path = if path.is_empty() { "index.html" } else { path };

    serve_asset(path)
}

fn serve_asset(path: &str) -> Response {
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                content.data.to_vec(),
            )
                .into_response()
        }
        // SPA fallback: return index.html for unknown paths (hash router handles client-side routing)
        None => match Assets::get("index.html") {
            Some(index) => (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8".to_string())],
                index.data.to_vec(),
            )
                .into_response(),
            None => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
        },
    }
}
