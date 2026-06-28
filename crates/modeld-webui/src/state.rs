use std::sync::{Arc, Mutex};

use modeld_core::Database;
use tokio::sync::broadcast;

use crate::metrics::Metrics;

/// Shared state for all API handlers.
#[derive(Clone)]
pub struct AppState {
    /// SQLite database (wrapped for async-safe access)
    pub db: Arc<Mutex<Database>>,
    /// Store root directory path
    pub store_path: std::path::PathBuf,
    /// Broadcast channel for WebSocket events
    pub event_tx: broadcast::Sender<WsEvent>,
    /// Prometheus-style metrics
    pub metrics: Arc<Metrics>,
    /// Optional bearer token for API authentication.
    /// `None` or empty string = authentication disabled.
    pub auth_token: Option<String>,
    /// Optional per-minute rate limit per IP.
    /// `None` = no rate limiting.
    pub rate_limit_per_min: Option<u32>,
}

/// WebSocket event types broadcast to all connected clients.
#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    /// File-system scan progress.
    ScanProgress {
        path: String,
        done: u64,
        total: u64,
    },
    /// Deduplication progress.
    DedupProgress {
        group: String,
        done: usize,
        total: usize,
    },
    /// Per-download transfer progress.
    DownloadProgress {
        download_id: String,
        bytes_done: u64,
        bytes_total: u64,
        /// Transfer speed in bytes per second.
        speed_bps: u64,
    },
    /// Garbage-collection progress.
    GcProgress {
        done: usize,
        total: usize,
    },
    /// Emitted when a background operation finishes.
    OperationComplete {
        operation: String,
        success: bool,
        message: String,
    },
    /// Emitted when a background operation encounters a fatal error.
    Error {
        message: String,
    },
    /// Daemon heartbeat / status snapshot.
    DaemonStatus(DaemonStatusPayload),
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct DaemonStatusPayload {
    pub scanning: bool,
    pub deduping: bool,
    pub uptime_secs: u64,
}

impl AppState {
    /// Create a new `AppState`.
    ///
    /// - `auth_token`: optional bearer token; `None` disables authentication.
    /// - `rate_limit_per_min`: optional per-IP request rate limit; `None` disables.
    pub fn new(
        db: Database,
        store_path: std::path::PathBuf,
        auth_token: Option<String>,
        rate_limit_per_min: Option<u32>,
    ) -> (Self, broadcast::Receiver<WsEvent>) {
        let (event_tx, event_rx) = broadcast::channel(256);
        let state = Self {
            db: Arc::new(Mutex::new(db)),
            store_path,
            event_tx,
            metrics: Metrics::new(),
            auth_token,
            rate_limit_per_min,
        };
        (state, event_rx)
    }
}
