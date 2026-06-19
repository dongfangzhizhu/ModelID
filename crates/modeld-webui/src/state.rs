use std::sync::{Arc, Mutex};

use modeld_core::Database;
use tokio::sync::broadcast;

/// Shared state for all API handlers.
#[derive(Clone)]
pub struct AppState {
    /// SQLite database (wrapped for async-safe access)
    pub db: Arc<Mutex<Database>>,
    /// Store root directory path
    pub store_path: std::path::PathBuf,
    /// Broadcast channel for WebSocket events
    pub event_tx: broadcast::Sender<WsEvent>,
}

/// WebSocket event types broadcast to all connected clients.
#[derive(Clone, serde::Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    ScanProgress(ScanProgressPayload),
    DedupProgress(DedupProgressPayload),
    DaemonStatus(DaemonStatusPayload),
}

#[derive(Clone, serde::Serialize, Debug)]
pub struct ScanProgressPayload {
    pub files_scanned: u64,
    pub files_total: u64,
    pub new_models_found: u64,
    pub duplicates_found: u64,
    pub phase: String,
    pub current_path: String,
}

#[derive(Clone, serde::Serialize, Debug)]
pub struct DedupProgressPayload {
    pub completed: u64,
    pub total: u64,
    pub current_file: String,
    pub bytes_saved_so_far: u64,
}

#[derive(Clone, serde::Serialize, Debug)]
pub struct DaemonStatusPayload {
    pub scanning: bool,
    pub deduping: bool,
    pub uptime_secs: u64,
}

impl AppState {
    pub fn new(db: Database, store_path: std::path::PathBuf) -> (Self, broadcast::Receiver<WsEvent>) {
        let (event_tx, event_rx) = broadcast::channel(256);
        let state = Self {
            db: Arc::new(Mutex::new(db)),
            store_path,
            event_tx,
        };
        (state, event_rx)
    }
}
