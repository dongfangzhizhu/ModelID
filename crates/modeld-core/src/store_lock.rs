//! Cross-process lock over a store directory.
//!
//! Every operation that mutates the store's *layout* (scan, dedup, gc, unlink,
//! quarantine restore) takes this lock, so two processes cannot interleave
//! renames in the same store. The in-process mutexes are not enough:
//! `modeld dedup` and a running `modeld serve` are different processes over the
//! same SQLite file.
//!
//! # Why an OS lock and not an `O_EXCL` marker file
//!
//! The lock is an **advisory exclusive lock on `<store>/.modeld.lock`**
//! (`flock(LOCK_EX)` on Unix, `LockFileEx` on Windows), taken non-blockingly in
//! a retry loop. The kernel owns it, so it is released when the holder exits —
//! including after `Ctrl-C`, `kill -9`, a panic abort or a power cut. An
//! `O_EXCL` marker file cannot do that: a crashed process leaves the file
//! behind and every later mutator is blocked until somebody deletes it by hand
//! or a staleness timeout expires.
//!
//! The file is deliberately **not deleted on release**. Unlinking it would
//! reintroduce the classic race: a waiter that already opened the old inode
//! locks it while a newcomer creates a fresh path and locks that, and both
//! believe they own the store. The path is therefore stable for the lifetime of
//! the store; only its content changes.
//!
//! The content is an advisory record of the holder (pid / op / host / start)
//! used solely to make the contention message useful. It is never trusted for
//! correctness — the OS lock is the truth.
//!
//! # Windows detail
//!
//! Windows locks are mandatory, so an exclusive lock over the bytes we write
//! would also block the diagnostic read from another process. The lock is
//! therefore taken over a sentinel byte far past the JSON payload.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Name of the lock file inside the store root.
pub const LOCK_FILE_NAME: &str = ".modeld.lock";

/// Poll interval while waiting for a held lock.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

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
    /// The lock file could not be created, opened or locked.
    #[error("failed to {} the store lock {}: {source}", action, path.display())]
    Io {
        /// What we were doing (`create` / `lock` / `read`).
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

/// An acquired store lock; released when dropped (and by the kernel on crash).
#[derive(Debug)]
pub struct StoreLock {
    path: PathBuf,
    /// Held open for the lifetime of the guard: the OS lock lives on this
    /// handle, so closing it — or process death — is what releases the lock.
    file: File,
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
        let deadline = Instant::now() + timeout;
        loop {
            match Self::open_and_lock(&path) {
                Ok(Some(mut file)) => {
                    let holder = LockHolder {
                        pid: std::process::id(),
                        op: op.to_string(),
                        started_at: Utc::now().to_rfc3339(),
                        host: hostname(),
                    };
                    // The record is advisory: a write failure must not cost us
                    // the lock we already hold.
                    let _ = Self::write_holder(&mut file, &holder);
                    return Ok(StoreLock { path, file, holder });
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let holder = Self::read_holder(&path).unwrap_or_else(unknown_holder);
                        return Err(LockError::Busy { path, holder });
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(source) => {
                    return Err(LockError::Io { action: "lock", path, source });
                }
            }
        }
    }

    /// Open (creating if needed) and try to take the exclusive OS lock.
    ///
    /// `Ok(None)` means another process holds it.
    fn open_and_lock(path: &Path) -> std::io::Result<Option<File>> {
        let file =
            OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        if try_lock_exclusive(&file)? {
            Ok(Some(file))
        } else {
            Ok(None)
        }
    }

    /// Replace the advisory holder record.
    ///
    /// Truncate-then-write, not append: a crashed holder may have left a longer
    /// record behind, and a partially overwritten one would not parse.
    fn write_holder(file: &mut File, holder: &LockHolder) -> std::io::Result<()> {
        let json = serde_json::to_string(holder).unwrap_or_default();
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(json.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()
    }

    /// Read the holder recorded in the lock file, if it parses.
    pub fn read_holder(path: &Path) -> Option<LockHolder> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(raw.trim()).ok()
    }

    /// Return the current holder, or `None` when the store is free.
    ///
    /// The recorded content outlives the lock (the file is never deleted), so
    /// this probes the lock first: if *we* can take it, nobody else holds it and
    /// the record on disk is just history.
    pub fn current_holder(store: &Path) -> Option<LockHolder> {
        let path = Self::path_for(store);
        if !path.exists() {
            return None;
        }
        let file = OpenOptions::new().read(true).write(true).open(&path).ok()?;
        match try_lock_exclusive(&file) {
            Ok(true) => {
                let _ = unlock(&file);
                None
            }
            // Held by someone else, or we cannot tell — report what is on disk.
            _ => Self::read_holder(&path),
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
        // Closing the handle releases the lock, but be explicit: on Unix this
        // makes the release immediate rather than "when the File is dropped",
        // and it keeps the intent readable.
        let _ = unlock(&self.file);
        // NOTE: the file is intentionally left in place — see the module docs.
    }
}

fn unknown_holder() -> LockHolder {
    LockHolder {
        pid: 0,
        op: "unknown".into(),
        started_at: "unknown".into(),
        host: "unknown".into(),
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

// ─────────────────────────────────────────────────────────────────────────────
// Platform primitives
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        // EWOULDBLOCK and EAGAIN are the same value on Linux but distinct on
        // some BSDs; accept both.
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => Ok(false),
        _ => Err(err),
    }
}

#[cfg(unix)]
fn unlock(file: &File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    h_event: *mut std::ffi::c_void,
}

/// Sentinel byte offset for the locked range.
///
/// Windows locks are mandatory, so locking the byte range that holds the holder
/// JSON would make the diagnostic read from another process fail. Locking 4 KB
/// past the payload keeps the payload readable while still excluding a second
/// writer.
#[cfg(windows)]
const WINDOWS_LOCK_OFFSET: u32 = 4095;

#[cfg(windows)]
fn windows_overlapped() -> Overlapped {
    Overlapped {
        internal: 0,
        internal_high: 0,
        offset: WINDOWS_LOCK_OFFSET,
        offset_high: 0,
        h_event: std::ptr::null_mut(),
    }
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;

    extern "system" {
        fn LockFileEx(
            h_file: *mut std::ffi::c_void,
            dw_flags: u32,
            dw_reserved: u32,
            n_bytes_low: u32,
            n_bytes_high: u32,
            lp_overlapped: *mut Overlapped,
        ) -> i32;
    }
    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x0000_0001;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x0000_0002;
    const ERROR_LOCK_VIOLATION: i32 = 33;

    let mut ov = windows_overlapped();
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_FAIL_IMMEDIATELY | LOCKFILE_EXCLUSIVE_LOCK,
            0,
            1,
            0,
            &mut ov,
        )
    };
    if ok != 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
        Ok(false)
    } else {
        Err(err)
    }
}

#[cfg(windows)]
fn unlock(file: &File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;

    extern "system" {
        fn UnlockFileEx(
            h_file: *mut std::ffi::c_void,
            dw_reserved: u32,
            n_bytes_low: u32,
            n_bytes_high: u32,
            lp_overlapped: *mut Overlapped,
        ) -> i32;
    }

    let mut ov = windows_overlapped();
    let ok = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut ov) };
    if ok != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn try_lock_exclusive(_file: &File) -> std::io::Result<bool> {
    // Refuse rather than pretend: a lock that is not enforced is worse than no
    // lock, because callers would believe the store is protected.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "store locking is not implemented on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn unlock(_file: &File) -> std::io::Result<()> {
    Ok(())
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
    fn dropping_the_lock_releases_it_and_keeps_the_file() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _lock = StoreLock::try_acquire(dir.path(), "dedup").unwrap();
            assert!(StoreLock::path_for(dir.path()).exists());
        }
        // The path survives on purpose: unlinking it would let two processes
        // lock different inodes and both believe they own the store.
        assert!(
            StoreLock::path_for(dir.path()).exists(),
            "the lock file is stable for the lifetime of the store"
        );
        assert!(StoreLock::try_acquire(dir.path(), "gc").is_ok());
    }

    #[test]
    fn waiting_acquire_succeeds_once_the_holder_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let lock = StoreLock::try_acquire(&path, "dedup").unwrap();
            // A sleep is not synchronisation: under an instrumented build the
            // thread may not even have started before the main thread tries to
            // acquire, and the "waiter" wins the lock. Announce it instead.
            ready_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(400));
            drop(lock);
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).expect("holder never took the lock");
        let lock = StoreLock::acquire(dir.path(), "gc", Duration::from_secs(5)).unwrap();
        assert_eq!(lock.holder().op, "gc");
        holder.join().unwrap();
    }

    #[test]
    fn garbage_content_does_not_block_acquire() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(StoreLock::path_for(dir.path()), "garbage").unwrap();
        // Content is advisory; only the OS lock decides.
        let lock = StoreLock::try_acquire(dir.path(), "gc").unwrap();
        assert_eq!(lock.holder().op, "gc");
        // And we overwrote the garbage with a parseable record.
        let recorded = StoreLock::read_holder(&StoreLock::path_for(dir.path())).unwrap();
        assert_eq!(recorded.pid, std::process::id());
    }

    #[test]
    fn current_holder_reports_the_holder_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = StoreLock::try_acquire(dir.path(), "dedup").unwrap();
        let holder = StoreLock::current_holder(dir.path()).expect("holder is recorded");
        assert_eq!(holder.pid, std::process::id());
        assert_eq!(holder.op, "dedup");
    }

    #[test]
    fn current_holder_is_none_when_the_store_is_free() {
        let dir = tempfile::tempdir().unwrap();
        // Create the file the way a finished operation would leave it behind.
        drop(StoreLock::try_acquire(dir.path(), "dedup").unwrap());
        assert!(StoreLock::path_for(dir.path()).exists());
        assert_eq!(
            StoreLock::current_holder(dir.path()),
            None,
            "a leftover record must not be reported as a live holder"
        );
    }

    /// Child half of `a_killed_process_releases_the_lock`. Inert unless the
    /// parent sets the env var, so a normal `cargo test` never blocks here.
    #[test]
    fn lock_holder_child() {
        let Ok(store) = std::env::var("MODELD_LOCK_CHILD_STORE") else {
            return;
        };
        let store = Path::new(&store);
        // Wait rather than fail: the parent probes the lock while we start.
        let _lock = StoreLock::acquire(store, "child", Duration::from_secs(15)).unwrap();
        // Tell the parent we own it, then hold until killed.
        let _ = std::fs::write(store.join("child-ready"), b"1");
        std::thread::sleep(Duration::from_secs(30));
    }

    /// Acceptance (B7): "锁在持有进程崩溃后自动释放".
    ///
    /// A real second process takes the lock and is killed with SIGKILL /
    /// `TerminateProcess`, so its `Drop` never runs and the file is left behind.
    /// The kernel must release the lock; the next acquire has to succeed
    /// immediately — no staleness timeout, no manual file deletion.
    #[test]
    fn a_killed_process_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("child-ready");
        let _ = std::fs::remove_file(&marker);

        let exe = std::env::current_exe().expect("test binary path");
        let mut child = std::process::Command::new(exe)
            .args(["--exact", "store_lock::tests::lock_holder_child", "--nocapture"])
            .env("MODELD_LOCK_CHILD_STORE", dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child test");

        // Wait for the child to signal that it holds the lock. Polling a marker
        // instead of the lock keeps the parent from competing for it.
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && !marker.exists() {
            std::thread::sleep(Duration::from_millis(50));
        }
        if !marker.exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child never acquired the lock");
        }

        // Sanity: while the child is alive, the store really is locked.
        assert!(
            StoreLock::try_acquire(dir.path(), "probe").is_err(),
            "the child should be holding the lock"
        );

        child.kill().expect("kill the holder");
        let _ = child.wait();

        // No `Drop`, no cleanup: only the kernel can release this.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match StoreLock::try_acquire(dir.path(), "after-crash") {
                Ok(lock) => {
                    assert_eq!(lock.holder().op, "after-crash");
                    break;
                }
                Err(e) => {
                    assert!(Instant::now() < deadline, "lock still held after crash: {e}");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
}
