//! modeld-webui: Embedded Web UI HTTP server for modeld
//!
//! Provides a local web interface accessible at http://localhost:8234
//! All UI assets are embedded into the binary via rust-embed.

pub mod api;
pub mod auth;
pub mod error;
pub mod metrics;
pub mod server;
pub mod state;
pub mod static_files;
pub mod ws;

pub use metrics::Metrics;
pub use server::{run, WebUiConfig};
pub use state::AppState;
