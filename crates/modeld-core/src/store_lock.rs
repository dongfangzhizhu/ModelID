//! Cross-process lock over a store directory.
//!
//! Every operation that mutates the store's *layout* (dedup, gc) takes this
//! lock so two processes cannot interleave renames in the same store.  The
//! in-process mutexes are not enough: `modeld dedup` and a running
//! `modeld serve` are different processes over the same SQLite file.
//!
//! The lock is a file (`<store>/.modeld.lock`) created with `O_EXCL`, so acquiring it
//! is atomic on every platform.  Its content names the holder, which is what
//! the failure message shows.  A lock whose file has not been touched for
//! `DEFAULT_STALE_AFTER` is treated as abandoned and broken — that is the only
//! automatic recovery, because detecting a dead pid portably needs platform
//! code we do not want in the hot path.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Name of the lock file inside the store root.
pub const LOCK_FILE_NAME: &str = ".modeld.lock";

/// How long an untouched lock file is considered abandoned.
pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Poll interval while waiting for a held lock.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Who holds the lock.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockHolder {
    /// Process id of the holder.
    pub pid: u32,
    /// Operation the holder is running (`dedup`, `gc`, …).
    pub op: String,
    /// RFC 3339 timestamp of acquisition.
    pub started_at: String,
    /// Host name, so a lock on a shared volume is identifiable.
    pub host: String,
}

impl fmt::Display for LockHolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pid {} on {} running '{}' since {}",
            self.pid, self.host, self.op, self.started_at
        )
    }
}

/// Why the lock could not be acquired.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another process holds the lock.
    #[error("the store at {} is locked by {}", path.display(), holder)]
    Busy {
        /// Store root.
        path: PathBuf,
        /// Current holder.
        holder: LockHolder,
    },
    /// The lock file could not be created or read.
    #[error("failed to {} the store lock {}: {source}", action, path.display())]
    Io {
        /// What we were doing (`create` / `read` / `remove`).
        action: &'static str,
        /// Lock file path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
}

impl LockError {
    /// Return the holder if the failure was contention rather than I/O.
    pub fn holder(&self) -> Option<&LockHolder> {
        match self {
            LockError::Busy { holder, .. } => Some(holder),
            LockError::Io { .. } => None,
        }
    }
}

/// An acquired store lock; released on drop.
#[derive(Debug)]
pub struct StoreLock {
    path: PathBuf,
    holder: LockHolder,
}

impl StoreLock {
    /// Path of the lock file for a store root.
    pub fn path_for(store: &Path) -> PathBuf {
        store.join(LOCK_FILE_NAME)
    }

    /// Acquire the lock, failing immediately if it is held.
    pub fn try_acquire(store: &Path, op: &str) -> std::result::Result<StoreLock, LockError> {
        Self::acquire(store, op, Duration::ZERO)
    }

    /// Acquire the lock, waiting up to `timeout` for a holder to finish.
    ///
    /// `timeout` of `Duration::ZERO` means "try once".
    pub fn acquire(
        store: &Path,
        op: &str,
        timeout: Duration,
    ) -> std::result::Result<StoreLock, LockError> {
        let path = Self::path_for(store);
        let deadline = SystemTime::now() + timeout;
        loop {
            match Self::try_create(&path, op) {
                Ok(lock) => return Ok(lock),
                Err(LockError::Busy { holder, .. }) => {
                    if SystemTime::now() >= deadline {
                        return Err(LockError::Busy { path, holder });
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn try_create(path: &Path, op: &str) -> std::result::Result<StoreLock, LockError> {
        let holder = LockHolder {
            pid: std::process::id(),
            op: op.to_string(),
            started_at: Utc::now().to_rfc3339(),
            host: hostname(),
        };
        for attempt in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    // The content is advisory; a write failure does not undo the
                    // lock, so it is not worth failing the operation over.
                    let _ =
                        writeln!(file, "{}", serde_json::to_string(&holder).unwrap_or_default());
                    let _ = file.sync_all();
                    return Ok(StoreLock { path: path.to_path_buf(), holder });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 0 && Self::is_stale(path) {
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    let existing = Self::read_holder(path).unwrap_or_else(|| LockHolder {
                        pid: 0,
                        op: "unknown".into(),
                        started_at: "unknown".into(),
                        host: "unknown".into(),
                    });
                    return Err(LockError::Busy { path: path.to_path_buf(), holder: existing });
                }
                Err(source) => {
                    return Err(LockError::Io {
                        action: "create",
                        path: path.to_path_buf(),
                        source,
                    })
                }
            }
        }
        unreachable!("the retry loop either returns or fails")
    }

    /// Read the holder recorded in the lock file, if it parses.
    pub fn read_holder(path: &Path) -> Option<LockHolder> {
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(raw.trim()).ok()
    }

    /// Return the current holder without acquiring.
    pub fn current_holder(store: &Path) -> Option<LockHolder> {
        Self::read_holder(&Self::path_for(store))
    }

    /// `true` if the lock file is missing, empty, unparsable or untouched for
    /// `DEFAULT_STALE_AFTER`.
    pub fn is_stale(path: &Path) -> bool {
        let Ok(meta) = fs::metadata(path) else {
            return false;
        };
        if Self::read_holder(path).is_none() {
            return true;
        }
        match meta.modified() {
            Ok(modified) => match SystemTime::now().duration_since(modified) {
                Ok(age) => age > DEFAULT_STALE_AFTER,
                // Clock skew: a file from the future is not abandoned.
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    /// The operation this guard was acquired for.
    pub fn holder(&self) -> &LockHolder {
        &self.holder
    }

    /// Path of the held lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Best-effort host name, used only to make lock messages readable.
fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Run `f` while holding the store lock.
pub fn with_lock<T>(store: &Path, op: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let _lock = StoreLock::try_acquire(store, op).with_context(|| {
        format!("another modeld process is already working on {}", store.display())
    })?;
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let first = StoreLock::try_acquire(dir.path(), "dedup").unwrap();
        assert_eq!(first.holder().op, "dedup");

        let err = StoreLock::try_acquire(dir.path(), "gc").unwrap_err();
        let holder = err.holder().expect("contention, not I/O");
        assert_eq!(holder.op, "dedup");
        assert_eq!(holder.pid, std::process::id());
        assert!(matches!(err, LockError::Busy { .. }));
    }

    #[test]
    fn dropping_the_lock_releases_it() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _lock = StoreLock::try_acquire(dir.path(), "dedup").unwrap();
            assert!(StoreLock::path_for(dir.path()).exists());
        }
        assert!(!StoreLock::path_for(dir.path()).exists());
        assert!(StoreLock::try_acquire(dir.path(), "gc").is_ok());
    }

    #[test]
    fn waiting_acquire_succeeds_once_the_holder_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let holder = std::thread::spawn(move || {
            let _lock = StoreLock::try_acquire(&path, "dedup").unwrap();
            std::thread::sleep(Duration::from_millis(400));
        });
        // Give the thread time to take the lock.
        std::thread::sleep(Duration::from_millis(100));
        let lock = StoreLock::acquire(dir.path(), "gc", Duration::from_secs(5)).unwrap();
        assert_eq!(lock.holder().op, "gc");
        holder.join().unwrap();
    }

    #[test]
    fn a_stale_lock_is_broken() {
        let dir = tempfile::tempdir().unwrap();
        let path = StoreLock::path_for(dir.path());
        fs::write(
            &path,
            serde_json::to_string(&LockHolder {
                pid: 999_999,
                op: "dedup".into(),
                started_at: "2020-01-01T00:00:00Z".into(),
                host: "old".into(),
            })
            .unwrap(),
        )
        .unwrap();
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - DEFAULT_STALE_AFTER - Duration::from_secs(60))
            .unwrap();
        drop(file);

        assert!(StoreLock::is_stale(&path));
        let lock = StoreLock::try_acquire(dir.path(), "gc").unwrap();
        assert_eq!(lock.holder().op, "gc");
    }

    #[test]
    fn an_unparsable_lock_file_is_broken() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(StoreLock::path_for(dir.path()), "garbage").unwrap();
        assert!(StoreLock::is_stale(&StoreLock::path_for(dir.path())));
        assert!(StoreLock::try_acquire(dir.path(), "gc").is_ok());
    }

    #[test]
    fn current_holder_reports_the_holder() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = StoreLock::try_acquire(dir.path(), "dedup").unwrap();
        let holder = StoreLock::current_holder(dir.path()).expect("holder is recorded");
        assert_eq!(holder.pid, std::process::id());
        assert_eq!(holder.op, "dedup");
    }
}
