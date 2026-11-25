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

/// Append `entry` to `<store_path>/audit.log`, ignoring failures.
///
/// Auditing must never break the operation it records: a full disk or a
/// permission problem is reported on stderr and the caller carries on.
pub fn record(store_path: &Path, entry: AuditEntry) {
    if let Err(e) = AuditLogger::new(store_path).log(&entry) {
        tracing::warn!("warning: could not write audit log: {e}");
    }
}

/// Filters accepted by [`AuditLogger::query`].
#[derive(Debug, Default, Clone)]
pub struct AuditQuery {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub op: Option<String>,
    pub actor_ip: Option<String>,
    pub outcome: Option<String>,
    /// Case-sensitive substring match on the resource field.
    pub resource_contains: Option<String>,
    /// Keep only the newest N matches.
    pub limit: Option<usize>,
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

    /// Return every valid entry in the log (chronological order, oldest first).
    pub fn all(&self) -> anyhow::Result<Vec<AuditEntry>> {
        self.query(&AuditQuery::default())
    }

    /// Filter the audit log.
    ///
    /// All conditions are AND-combined; `None` means "no constraint".
    /// `since` / `until` compare against the entry's UTC timestamp with
    /// inclusive lower and exclusive upper bounds. `limit` keeps the most
    /// recent `limit` matches (so a "what happened last week" query still
    /// returns the newest events first when read bottom-up).
    pub fn query(&self, q: &AuditQuery) -> anyhow::Result<Vec<AuditEntry>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.log_path)?;
        let mut entries: Vec<AuditEntry> = content
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .filter(|e: &AuditEntry| {
                if let Some(since) = q.since {
                    if e.timestamp < since {
                        return false;
                    }
                }
                if let Some(until) = q.until {
                    if e.timestamp >= until {
                        return false;
                    }
                }
                if let Some(op) = &q.op {
                    if !e.op.eq_ignore_ascii_case(op) {
                        return false;
                    }
                }
                if let Some(actor) = &q.actor_ip {
                    if !e.actor_ip.eq_ignore_ascii_case(actor) {
                        return false;
                    }
                }
                if let Some(outcome) = &q.outcome {
                    if !e.outcome.eq_ignore_ascii_case(outcome) {
                        return false;
                    }
                }
                if let Some(resource) = &q.resource_contains {
                    if !e.resource.contains(resource) {
                        return false;
                    }
                }
                true
            })
            .collect();

        if let Some(limit) = q.limit {
            let skip = entries.len().saturating_sub(limit);
            entries.drain(..skip);
        }
        Ok(entries)
    }

    /// Count entries grouped by `(op, outcome)`. Used by `/metrics` so every
    /// audited change path is represented in Prometheus without a second write
    /// path that could drift from the audit log.
    pub fn counts_by_op(&self) -> anyhow::Result<Vec<(String, String, u64)>> {
        let mut counts: std::collections::BTreeMap<(String, String), u64> =
            std::collections::BTreeMap::new();
        for entry in self.all()? {
            *counts.entry((entry.op, entry.outcome)).or_insert(0) += 1;
        }
        Ok(counts.into_iter().map(|((op, outcome), n)| (op, outcome, n)).collect())
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

    #[test]
    fn test_query_filters_op_actor_and_outcome() {
        let tmp = tempdir().unwrap();
        let logger = AuditLogger::new(tmp.path());
        logger.log(&AuditEntry::success("unlink", "/models/a.safetensors")).unwrap();
        logger.log(&AuditEntry::success("dedup", "hash_b")).unwrap();
        logger
            .log(&AuditEntry {
                actor_ip: "192.168.1.5".to_string(),
                ..AuditEntry::success("unlink", "/models/c.gguf")
            })
            .unwrap();
        logger.log(&AuditEntry::failure("unlink", "/models/d.bin", "permission denied")).unwrap();

        let unlinks =
            logger.query(&AuditQuery { op: Some("unlink".into()), ..Default::default() }).unwrap();
        assert_eq!(unlinks.len(), 3);
        let remote = logger
            .query(&AuditQuery {
                op: Some("unlink".into()),
                actor_ip: Some("192.168.1.5".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(remote.len(), 1);
        assert_eq!(remote[0].resource, "/models/c.gguf");
        let failed = logger
            .query(&AuditQuery { outcome: Some("failure".into()), ..Default::default() })
            .unwrap();
        assert_eq!(failed.len(), 1);
        let newest_two =
            logger.query(&AuditQuery { limit: Some(2), ..Default::default() }).unwrap();
        assert_eq!(newest_two.len(), 2);
        // 日志按追加顺序保存，最后写入的两条是 remote c.gguf 与 failed d.bin。
        assert_eq!(newest_two[0].resource, "/models/c.gguf");
        assert_eq!(newest_two[1].resource, "/models/d.bin");
    }

    #[test]
    fn test_query_time_range_and_resource_contains() {
        let tmp = tempdir().unwrap();
        let logger = AuditLogger::new(tmp.path());
        let now = Utc::now();
        logger
            .log(&AuditEntry {
                timestamp: now - chrono::Duration::days(9),
                ..AuditEntry::success("unlink", "/models/old.safetensors")
            })
            .unwrap();
        logger
            .log(&AuditEntry {
                timestamp: now - chrono::Duration::days(2),
                ..AuditEntry::success("unlink", "/models/recent.safetensors")
            })
            .unwrap();

        let last_week = logger
            .query(&AuditQuery {
                since: Some(now - chrono::Duration::days(7)),
                op: Some("unlink".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(last_week.len(), 1, "上周的删除查询只应命中近 7 天记录");
        assert_eq!(last_week[0].resource, "/models/recent.safetensors");

        let filtered = logger
            .query(&AuditQuery { resource_contains: Some("old".into()), ..Default::default() })
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].resource, "/models/old.safetensors");
    }

    #[test]
    fn test_counts_by_op() {
        let tmp = tempdir().unwrap();
        let logger = AuditLogger::new(tmp.path());
        logger.log(&AuditEntry::success("unlink", "a")).unwrap();
        logger.log(&AuditEntry::success("unlink", "b")).unwrap();
        logger.log(&AuditEntry::failure("unlink", "c", "err")).unwrap();
        let counts = logger.counts_by_op().unwrap();
        assert!(counts.contains(&("unlink".into(), "success".into(), 2)));
        assert!(counts.contains(&("unlink".into(), "failure".into(), 1)));
    }
}
