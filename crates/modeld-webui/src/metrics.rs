//! Prometheus-compatible metrics for the modeld Web UI server.

use dashmap::DashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

/// Shared metrics counters for the Web UI server.
///
/// All fields are safe to use concurrently without taking a lock.
pub struct Metrics {
    /// Total HTTP requests handled.
    pub requests_total: AtomicU64,
    /// Total CAS cache hits (file served from CAS without re-download).
    pub cache_hits: AtomicU64,
    /// Total CAS cache misses (file not found in CAS, needed download).
    pub cache_misses: AtomicU64,
    /// Number of downloads currently in-flight (gauge).
    pub active_downloads: AtomicI64,
    /// Total bytes sent to clients.
    pub bytes_served: AtomicU64,
    /// Error counts keyed by HTTP status code.
    pub errors: Arc<DashMap<u16, u64>>,
}

impl Metrics {
    /// Construct a new zeroed-out metrics instance wrapped in `Arc`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            requests_total: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            active_downloads: AtomicI64::new(0),
            bytes_served: AtomicU64::new(0),
            errors: Arc::new(DashMap::new()),
        })
    }

    /// Render all metrics in the Prometheus text exposition format (version 0.0.4).
    ///
    /// Each metric family is preceded by `# HELP` and `# TYPE` comment lines.
    pub fn render_prometheus(&self) -> String {
        let mut out = String::with_capacity(512);

        // requests_total
        out.push_str("# HELP modeld_requests_total Total number of HTTP requests handled\n");
        out.push_str("# TYPE modeld_requests_total counter\n");
        out.push_str(&format!(
            "modeld_requests_total {}\n\n",
            self.requests_total.load(Ordering::Relaxed)
        ));

        // cache_hits_total
        out.push_str("# HELP modeld_cache_hits_total Total number of CAS cache hits\n");
        out.push_str("# TYPE modeld_cache_hits_total counter\n");
        out.push_str(&format!(
            "modeld_cache_hits_total {}\n\n",
            self.cache_hits.load(Ordering::Relaxed)
        ));

        // cache_misses_total
        out.push_str("# HELP modeld_cache_misses_total Total number of CAS cache misses\n");
        out.push_str("# TYPE modeld_cache_misses_total counter\n");
        out.push_str(&format!(
            "modeld_cache_misses_total {}\n\n",
            self.cache_misses.load(Ordering::Relaxed)
        ));

        // active_downloads (gauge)
        out.push_str("# HELP modeld_active_downloads Number of downloads currently in progress\n");
        out.push_str("# TYPE modeld_active_downloads gauge\n");
        out.push_str(&format!(
            "modeld_active_downloads {}\n\n",
            self.active_downloads.load(Ordering::Relaxed)
        ));

        // bytes_served_total
        out.push_str("# HELP modeld_bytes_served_total Total bytes sent to clients\n");
        out.push_str("# TYPE modeld_bytes_served_total counter\n");
        out.push_str(&format!(
            "modeld_bytes_served_total {}\n\n",
            self.bytes_served.load(Ordering::Relaxed)
        ));

        // errors by HTTP status code
        out.push_str("# HELP modeld_errors_total Total HTTP errors by status code\n");
        out.push_str("# TYPE modeld_errors_total counter\n");
        for entry in self.errors.iter() {
            out.push_str(&format!(
                "modeld_errors_total{{status=\"{}\"}} {}\n",
                entry.key(),
                entry.value()
            ));
        }
        out.push('\n');

        out
    }

    /// Increment the total request counter by 1.
    pub fn inc_requests(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one cache hit.
    pub fn inc_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one cache miss.
    pub fn inc_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one error for the given HTTP `status` code.
    pub fn inc_error(&self, status: u16) {
        *self.errors.entry(status).or_insert(0) += 1;
    }

    /// Add `n` bytes to the bytes-served counter.
    pub fn add_bytes_served(&self, n: u64) {
        self.bytes_served.fetch_add(n, Ordering::Relaxed);
    }

    /// Increment active download gauge by 1.
    pub fn inc_active_downloads(&self) {
        self.active_downloads.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement active download gauge by 1 (saturates at 0).
    pub fn dec_active_downloads(&self) {
        self.active_downloads.fetch_sub(1, Ordering::Relaxed);
    }

    /// Set the active-downloads gauge to an absolute value.
    pub fn set_active_downloads(&self, n: i64) {
        self.active_downloads.store(n, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counters_increment() {
        let m = Metrics::new();
        m.inc_requests();
        m.inc_requests();
        assert_eq!(m.requests_total.load(Ordering::Relaxed), 2);

        m.inc_cache_hit();
        assert_eq!(m.cache_hits.load(Ordering::Relaxed), 1);

        m.inc_cache_miss();
        m.inc_cache_miss();
        assert_eq!(m.cache_misses.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_error_counts() {
        let m = Metrics::new();
        m.inc_error(404);
        m.inc_error(404);
        m.inc_error(500);

        assert_eq!(*m.errors.get(&404).unwrap(), 2);
        assert_eq!(*m.errors.get(&500).unwrap(), 1);
    }

    #[test]
    fn test_render_prometheus_format() {
        let m = Metrics::new();
        m.inc_requests();
        m.inc_cache_hit();
        m.inc_error(503);
        m.add_bytes_served(1024);

        let output = m.render_prometheus();
        assert!(output.contains("modeld_requests_total 1"));
        assert!(output.contains("modeld_cache_hits_total 1"));
        assert!(output.contains("modeld_bytes_served_total 1024"));
        assert!(output.contains("modeld_errors_total{status=\"503\"} 1"));
        // Must include TYPE and HELP lines
        assert!(output.contains("# TYPE modeld_requests_total counter"));
        assert!(output.contains("# HELP modeld_cache_hits_total"));
    }

    #[test]
    fn test_active_downloads_gauge() {
        let m = Metrics::new();
        m.inc_active_downloads();
        m.inc_active_downloads();
        assert_eq!(m.active_downloads.load(Ordering::Relaxed), 2);
        m.dec_active_downloads();
        assert_eq!(m.active_downloads.load(Ordering::Relaxed), 1);
    }
}
