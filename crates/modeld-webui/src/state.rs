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
        scan_id: String,
        /// One of: "walking" | "hashing" | "indexing" | "done" | "error"
        phase: String,
        current_path: Option<String>,
        files_scanned: u64,
        files_total: u64,
        bytes_scanned: u64,
        bytes_total: u64,
    },
    /// Deduplication progress.
    DedupProgress { group: String, done: usize, total: usize },
    /// Per-download transfer progress.
    DownloadProgress {
        download_id: String,
        bytes_done: u64,
        bytes_total: u64,
        /// Transfer speed in bytes per second.
        speed_bps: u64,
    },
    /// Garbage-collection progress.
    GcProgress { done: usize, total: usize },
    /// Emitted when a background operation finishes.
    OperationComplete { operation: String, success: bool, message: String },
    /// Emitted when a background operation encounters a fatal error.
    Error { message: String },
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

// ─── Serialization stability tests (audit Wave 3, Req 3.11) ───────────────────
//
// These tests exist as a regression guard: the WebUI frontend (`ui/main.js`
// and `ui/pages/dashboard.js`) deserializes WsEvent JSON messages and reads
// these exact field names. If a field is renamed or removed here without
// updating the frontend, these tests should fail first.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_event_scan_progress_field_names_are_stable() {
        let event = WsEvent::ScanProgress {
            scan_id: "scan-abc123".to_string(),
            phase: "hashing".to_string(),
            current_path: Some("/models/file.safetensors".to_string()),
            files_scanned: 42,
            files_total: 100,
            bytes_scanned: 1024,
            bytes_total: 2048,
        };

        let value = serde_json::to_value(&event).expect("serialize WsEvent::ScanProgress");
        let obj = value.as_object().expect("WsEvent::ScanProgress serializes to an object");

        // Check the "type" tag field (from #[serde(tag = "type")])
        assert_eq!(
            obj.get("type").and_then(|v| v.as_str()),
            Some("scan_progress"),
            "WsEvent::ScanProgress type tag must be 'scan_progress'"
        );

        // Check all variant fields
        for field in [
            "scan_id",
            "phase",
            "current_path",
            "files_scanned",
            "files_total",
            "bytes_scanned",
            "bytes_total",
        ] {
            assert!(obj.contains_key(field), "WsEvent::ScanProgress missing field `{field}`");
        }
    }

    #[test]
    fn ws_event_dedup_progress_field_names_are_stable() {
        let event = WsEvent::DedupProgress { group: "group-1".to_string(), done: 5, total: 10 };

        let value = serde_json::to_value(&event).expect("serialize WsEvent::DedupProgress");
        let obj = value.as_object().expect("WsEvent::DedupProgress serializes to an object");

        assert_eq!(
            obj.get("type").and_then(|v| v.as_str()),
            Some("dedup_progress"),
            "WsEvent::DedupProgress type tag must be 'dedup_progress'"
        );

        for field in ["group", "done", "total"] {
            assert!(obj.contains_key(field), "WsEvent::DedupProgress missing field `{field}`");
        }
    }

    #[test]
    fn ws_event_download_progress_field_names_are_stable() {
        let event = WsEvent::DownloadProgress {
            download_id: "dl-xyz".to_string(),
            bytes_done: 512,
            bytes_total: 1024,
            speed_bps: 2048,
        };

        let value = serde_json::to_value(&event).expect("serialize WsEvent::DownloadProgress");
        let obj = value.as_object().expect("WsEvent::DownloadProgress serializes to an object");

        assert_eq!(
            obj.get("type").and_then(|v| v.as_str()),
            Some("download_progress"),
            "WsEvent::DownloadProgress type tag must be 'download_progress'"
        );

        for field in ["download_id", "bytes_done", "bytes_total", "speed_bps"] {
            assert!(obj.contains_key(field), "WsEvent::DownloadProgress missing field `{field}`");
        }
    }

    #[test]
    fn ws_event_gc_progress_field_names_are_stable() {
        let event = WsEvent::GcProgress { done: 7, total: 14 };

        let value = serde_json::to_value(&event).expect("serialize WsEvent::GcProgress");
        let obj = value.as_object().expect("WsEvent::GcProgress serializes to an object");

        assert_eq!(
            obj.get("type").and_then(|v| v.as_str()),
            Some("gc_progress"),
            "WsEvent::GcProgress type tag must be 'gc_progress'"
        );

        for field in ["done", "total"] {
            assert!(obj.contains_key(field), "WsEvent::GcProgress missing field `{field}`");
        }
    }

    #[test]
    fn ws_event_operation_complete_field_names_are_stable() {
        let event = WsEvent::OperationComplete {
            operation: "scan".to_string(),
            success: true,
            message: "Scan completed successfully".to_string(),
        };

        let value = serde_json::to_value(&event).expect("serialize WsEvent::OperationComplete");
        let obj = value.as_object().expect("WsEvent::OperationComplete serializes to an object");

        assert_eq!(
            obj.get("type").and_then(|v| v.as_str()),
            Some("operation_complete"),
            "WsEvent::OperationComplete type tag must be 'operation_complete'"
        );

        for field in ["operation", "success", "message"] {
            assert!(obj.contains_key(field), "WsEvent::OperationComplete missing field `{field}`");
        }
    }

    #[test]
    fn ws_event_error_field_names_are_stable() {
        let event = WsEvent::Error { message: "An error occurred".to_string() };

        let value = serde_json::to_value(&event).expect("serialize WsEvent::Error");
        let obj = value.as_object().expect("WsEvent::Error serializes to an object");

        assert_eq!(
            obj.get("type").and_then(|v| v.as_str()),
            Some("error"),
            "WsEvent::Error type tag must be 'error'"
        );

        assert!(obj.contains_key("message"), "WsEvent::Error missing field `message`");
    }

    #[test]
    fn ws_event_daemon_status_field_names_are_stable() {
        let event = WsEvent::DaemonStatus(DaemonStatusPayload {
            scanning: true,
            deduping: false,
            uptime_secs: 3600,
        });

        let value = serde_json::to_value(&event).expect("serialize WsEvent::DaemonStatus");
        let obj = value.as_object().expect("WsEvent::DaemonStatus serializes to an object");

        assert_eq!(
            obj.get("type").and_then(|v| v.as_str()),
            Some("daemon_status"),
            "WsEvent::DaemonStatus type tag must be 'daemon_status'"
        );

        // For this variant, the DaemonStatusPayload is flattened into the object
        // (it's a tuple variant containing the payload, so check for the payload fields)
        for field in ["scanning", "deduping", "uptime_secs"] {
            assert!(obj.contains_key(field), "WsEvent::DaemonStatus missing field `{field}`");
        }
    }
}
