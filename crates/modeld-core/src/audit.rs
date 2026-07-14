//! Append-only NDJSON audit log for security-relevant operations.
//!
//! Each entry is written as a single JSON line to `<store>/audit.log`.
//! **Token values (bearer tokens, HF_TOKEN, etc.) must never be written here.**

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// UTC timestamp of the operation.
    pub timestamp: DateTime<Utc>,
    /// Operation kind, e.g. "download", "dedup", "gc", "delete", "config_change".
    pub op: String,
    /// Origin: "local" for CLI/same-machine, or an IP address string.
    pub actor_ip: String,
    /// Identifies the resource acted upon: hash, relative path, etc.
    /// MUST NOT contain token values.
    pub resource: String,
    /// "success" or "failure".
    pub outcome: String,
    /// Optional human-readable detail (no secrets).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl AuditEntry {
    /// Convenience constructor for a successful local operation.
    pub fn success(op: impl Into<String>, resource: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            op: op.into(),
            actor_ip: "local".to_string(),
            resource: resource.into(),
            outcome: "success".to_string(),
            details: None,
        }
    }

    /// Convenience constructor for a failed local operation.
    pub fn failure(
        op: impl Into<String>,
        resource: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            op: op.into(),
            actor_ip: "local".to_string(),
            resource: resource.into(),
            outcome: "failure".to_string(),
            details: Some(details.into()),
        }
    }
}

/// Writes audit entries as NDJSON to `<store>/audit.log`.
///
/// The file is opened in append mode on every write, making it safe to use
/// from multiple processes.
pub struct AuditLogger {
    log_path: PathBuf,
}

impl AuditLogger {
    /// Create an `AuditLogger` that writes to `<store_path>/audit.log`.
    pub fn new(store_path: &Path) -> Self {
        Self { log_path: store_path.join("audit.log") }
    }

    /// Append one entry to the audit log.
    ///
    /// # Security
    /// Callers MUST sanitize `resource` and `details` before passing them here;
    /// bearer tokens, HF_TOKEN, and other secrets must never appear in those fields.
    pub fn log(&self, entry: &AuditEntry) -> anyhow::Result<()> {
        use std::io::Write;

        let line = serde_json::to_string(entry)?;

        let mut file =
            std::fs::OpenOptions::new().create(true).append(true).open(&self.log_path).map_err(
                |e| anyhow::anyhow!("Failed to open audit log {}: {}", self.log_path.display(), e),
            )?;

        writeln!(file, "{}", line)?;
        Ok(())
    }

    /// Return the most recent `limit` entries (chronological order, oldest first).
    pub fn recent(&self, limit: usize) -> anyhow::Result<Vec<AuditEntry>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&self.log_path)?;

        // Collect all valid entries then take the last `limit`.
        let entries: Vec<AuditEntry> = content
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();

        let skip = entries.len().saturating_sub(limit);
        Ok(entries.into_iter().skip(skip).collect())
    }

    /// Return the path of the underlying log file.
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_log_and_read_recent() {
        let tmp = tempdir().unwrap();
        let logger = AuditLogger::new(tmp.path());

        for i in 0..5 {
            logger.log(&AuditEntry::success(format!("op_{}", i), format!("hash_{}", i))).unwrap();
        }

        let all = logger.recent(10).unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].op, "op_0");
        assert_eq!(all[4].op, "op_4");

        let last2 = logger.recent(2).unwrap();
        assert_eq!(last2.len(), 2);
        assert_eq!(last2[0].op, "op_3");
    }

    #[test]
    fn test_no_file_returns_empty() {
        let tmp = tempdir().unwrap();
        let logger = AuditLogger::new(tmp.path());
        let result = logger.recent(10).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_failure_entry() {
        let tmp = tempdir().unwrap();
        let logger = AuditLogger::new(tmp.path());
        let entry = AuditEntry::failure("download", "hash_abc", "network timeout");
        logger.log(&entry).unwrap();
        let entries = logger.recent(1).unwrap();
        assert_eq!(entries[0].outcome, "failure");
        assert_eq!(entries[0].details.as_deref(), Some("network timeout"));
    }

    #[test]
    fn test_actor_ip_from_remote() {
        let tmp = tempdir().unwrap();
        let logger = AuditLogger::new(tmp.path());
        let entry = AuditEntry {
            timestamp: Utc::now(),
            op: "download".to_string(),
            actor_ip: "192.168.1.5".to_string(),
            resource: "abc123".to_string(),
            outcome: "success".to_string(),
            details: None,
        };
        logger.log(&entry).unwrap();
        let entries = logger.recent(1).unwrap();
        assert_eq!(entries[0].actor_ip, "192.168.1.5");
    }
}
