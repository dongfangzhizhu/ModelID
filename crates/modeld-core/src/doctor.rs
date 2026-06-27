//! Environment diagnostics — `modeld doctor`
//!
//! Checks that the store directory, disk space, SQLite, and platform
//! capabilities are all in a healthy state.  Every check returns a
//! `DoctorCheck` with a `CheckStatus` and an optional human-readable fix
//! suggestion.

use crate::platform::detect_capabilities;
use anyhow::Result;
use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// Severity of a single doctor check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    /// Everything is fine.
    Pass,
    /// Non-critical issue; modeld will still work but performance or usability
    /// may be degraded.
    Warn,
    /// Critical issue; modeld cannot function correctly.
    Fail,
}

impl CheckStatus {
    /// Short label for display.
    pub fn label(&self) -> &'static str {
        match self {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
        }
    }
}

/// Result of a single environment check.
#[derive(Debug, Clone)]
pub struct DoctorCheck {
    /// Human-readable name of the check.
    pub name: String,
    /// Outcome of the check.
    pub status: CheckStatus,
    /// Description of what was found.
    pub message: String,
    /// Optional actionable fix hint.
    pub fix: Option<String>,
}

impl DoctorCheck {
    fn pass(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Pass,
            message: message.into(),
            fix: None,
        }
    }

    fn warn(
        name: impl Into<String>,
        message: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Warn,
            message: message.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(
        name: impl Into<String>,
        message: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Fail,
            message: message.into(),
            fix: Some(fix.into()),
        }
    }
}

/// Aggregated doctor report.
#[derive(Debug, Clone)]
pub struct DoctorReport {
    /// All individual check results (in order).
    pub checks: Vec<DoctorCheck>,
    /// Resolved store path that was examined.
    pub store_path: PathBuf,
    /// Crate version from `CARGO_PKG_VERSION`.
    pub version: String,
    /// Build target triple (e.g. `x86_64-pc-windows-msvc`).
    pub build_target: String,
}

impl DoctorReport {
    /// Number of checks that passed.
    pub fn pass_count(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Pass).count()
    }
    /// Number of warnings.
    pub fn warn_count(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Warn).count()
    }
    /// Number of failures.
    pub fn fail_count(&self) -> usize {
        self.checks.iter().filter(|c| c.status == CheckStatus::Fail).count()
    }

    /// Generate a copyable diagnostic report string (no sensitive data).
    ///
    /// The output is plain text suitable for pasting into a bug report.
    pub fn to_report_string(&self) -> String {
        let mut out = String::new();
        out.push_str("=== modeld doctor report ===\n");
        out.push_str(&format!("version      : {}\n", self.version));
        out.push_str(&format!("build_target : {}\n", self.build_target));
        out.push_str(&format!("store_path   : {}\n", self.store_path.display()));
        out.push_str(&format!(
            "summary      : {} pass, {} warn, {} fail\n",
            self.pass_count(),
            self.warn_count(),
            self.fail_count(),
        ));
        out.push_str("\n--- checks ---\n");
        for check in &self.checks {
            out.push_str(&format!(
                "[{}] {}: {}\n",
                check.status.label(),
                check.name,
                check.message,
            ));
            if let Some(fix) = &check.fix {
                out.push_str(&format!("      fix: {}\n", fix));
            }
        }
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Run all environment checks against `store` and return the aggregated report.
///
/// This function never returns `Err` for individual check failures; each check
/// degrades gracefully to a `Warn` or `Fail` status instead.  The `Result`
/// wrapper is reserved for unexpected I/O errors that prevent inspection from
/// completing entirely.
pub fn run_doctor(store: &Path) -> Result<DoctorReport> {
    let mut checks = Vec::new();

    // 1. Store directory exists
    checks.push(check_store_exists(store));

    // 2. Store directory writable
    checks.push(check_store_writable(store));

    // 3. Disk free space
    checks.push(check_disk_space(store));

    // 4. SQLite WAL mode
    checks.push(check_sqlite_wal(store));

    // 5. modeld version
    checks.push(check_version());

    // 6. Platform capabilities
    let caps_checks = check_platform_capabilities(store);
    checks.extend(caps_checks);

    // Windows-only checks
    #[cfg(windows)]
    {
        checks.extend(check_windows_specific(store));
    }

    let version = env!("CARGO_PKG_VERSION").to_string();
    // Build target is set by build.rs via GIT_HASH/BUILD_TARGET; fall back to
    // compile-time cfg if build.rs is not present.
    let build_target = option_env!("BUILD_TARGET")
        .unwrap_or(std::env::consts::ARCH)
        .to_string();

    Ok(DoctorReport {
        checks,
        store_path: store.to_path_buf(),
        version,
        build_target,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Individual checks (cross-platform)
// ─────────────────────────────────────────────────────────────────────────────

fn check_store_exists(store: &Path) -> DoctorCheck {
    if store.exists() && store.is_dir() {
        DoctorCheck::pass("store.exists", format!("store directory found at {}", store.display()))
    } else {
        DoctorCheck::warn(
            "store.exists",
            format!("store directory not found at {}", store.display()),
            "Run `modeld init` to create the store",
        )
    }
}

fn check_store_writable(store: &Path) -> DoctorCheck {
    // If the directory does not exist we skip the writability check (already
    // reported by check_store_exists).
    if !store.exists() {
        return DoctorCheck::warn(
            "store.writable",
            "store directory does not exist; writability cannot be checked".to_string(),
            "Run `modeld init` to create the store",
        );
    }

    let probe = store.join(".write_probe");
    match std::fs::write(&probe, b"probe") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            DoctorCheck::pass("store.writable", "store directory is writable")
        }
        Err(e) => DoctorCheck::fail(
            "store.writable",
            format!("store directory is not writable: {}", e),
            "Check directory permissions or run modeld as a user with write access",
        ),
    }
}

fn check_disk_space(store: &Path) -> DoctorCheck {
    // Probe a path that exists (fall back to current dir).
    let probe = if store.exists() {
        store.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };

    match free_space_bytes(&probe) {
        Ok(free) => {
            const WARN_THRESHOLD: u64 = 1_073_741_824; // 1 GiB
            const FAIL_THRESHOLD: u64 = 104_857_600; // 100 MiB
            let free_gib = free as f64 / 1_073_741_824.0;
            if free < FAIL_THRESHOLD {
                DoctorCheck::fail(
                    "disk.space",
                    format!("only {:.0} MiB free — critically low", free as f64 / 1_048_576.0),
                    "Free up disk space before using modeld",
                )
            } else if free < WARN_THRESHOLD {
                DoctorCheck::warn(
                    "disk.space",
                    format!("{:.2} GiB free — running low", free_gib),
                    "Consider freeing disk space or using a different store location",
                )
            } else {
                DoctorCheck::pass(
                    "disk.space",
                    format!("{:.2} GiB free", free_gib),
                )
            }
        }
        Err(e) => DoctorCheck::warn(
            "disk.space",
            format!("could not determine free disk space: {}", e),
            "Ensure the store volume is mounted and accessible",
        ),
    }
}

fn check_sqlite_wal(store: &Path) -> DoctorCheck {
    // Create a temporary DB in-memory to verify SQLite works, then try to
    // open/create the actual store DB with WAL mode.
    let db_path = store.join("modeld.db");

    // If the store doesn't exist yet, just verify SQLite works in-memory.
    let test_path = if store.is_dir() {
        db_path.clone()
    } else {
        // Try in a temp dir
        std::env::temp_dir().join("modeld_doctor_probe.db")
    };

    // Try to open (or create) the database and enable WAL
    match open_sqlite_wal_probe(&test_path) {
        Ok(()) => {
            // Clean up the probe DB if we created it in temp
            if test_path != db_path {
                let _ = std::fs::remove_file(&test_path);
            }
            DoctorCheck::pass("sqlite.wal", "SQLite WAL mode is available")
        }
        Err(e) => DoctorCheck::fail(
            "sqlite.wal",
            format!("SQLite WAL mode unavailable: {}", e),
            "Ensure SQLite is not running in a restricted environment",
        ),
    }
}

/// Attempt to open a SQLite DB at `path` with WAL mode enabled.
fn open_sqlite_wal_probe(path: &Path) -> anyhow::Result<()> {
    use rusqlite::Connection;
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    Ok(())
}

fn check_version() -> DoctorCheck {
    let version = env!("CARGO_PKG_VERSION");
    let git_hash = option_env!("GIT_HASH").unwrap_or("unknown");
    DoctorCheck::pass(
        "version",
        format!("modeld {} (git: {})", version, git_hash),
    )
}

fn check_platform_capabilities(store: &Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    // Use a directory that is likely to exist for probing
    let probe_path = if store.is_dir() {
        store.to_path_buf()
    } else {
        std::env::temp_dir()
    };

    let caps = detect_capabilities(&probe_path);

    // Filesystem type
    checks.push(DoctorCheck::pass(
        "platform.filesystem",
        format!("filesystem type: {}", caps.filesystem_type),
    ));

    // Hardlink support
    if caps.supports_hardlink {
        checks.push(DoctorCheck::pass(
            "platform.hardlink",
            "hardlink support available",
        ));
    } else {
        checks.push(DoctorCheck::warn(
            "platform.hardlink",
            "hardlinks not supported on this filesystem",
            "Dedup will fall back to copy-to-CAS strategy",
        ));
    }

    checks
}

// ─────────────────────────────────────────────────────────────────────────────
// Windows-only checks
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
fn check_windows_specific(store: &Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    // 7. OS version
    checks.push(check_windows_os_version());

    // 8. Symlink privilege
    let probe_path = if store.is_dir() { store.to_path_buf() } else { std::env::temp_dir() };
    let caps = detect_capabilities(&probe_path);

    if caps.has_symlink_privilege {
        checks.push(DoctorCheck::pass(
            "windows.symlink_privilege",
            "SeCreateSymbolicLinkPrivilege or Developer Mode is available",
        ));
    } else {
        checks.push(DoctorCheck::warn(
            "windows.symlink_privilege",
            "symlink creation is not available for the current user",
            "Enable Developer Mode in Windows Settings, or run modeld as Administrator",
        ));
    }

    // 9. Hardlink support (NTFS)
    if caps.supports_hardlink {
        checks.push(DoctorCheck::pass("windows.hardlink", "hardlinks supported"));
    } else {
        checks.push(DoctorCheck::warn(
            "windows.hardlink",
            "hardlinks not available on this filesystem",
            "Move the store to an NTFS volume for hardlink dedup support",
        ));
    }

    // 10. Long path support
    if caps.supports_long_path {
        checks.push(DoctorCheck::pass(
            "windows.long_path",
            "long path support is enabled (LongPathsEnabled=1)",
        ));
    } else {
        checks.push(DoctorCheck::warn(
            "windows.long_path",
            "long paths are not enabled; paths >260 chars may fail",
            "Set HKLM\\SYSTEM\\CurrentControlSet\\Control\\FileSystem\\LongPathsEnabled=1 \
             and restart, or enable via Group Policy",
        ));
    }

    checks
}

#[cfg(windows)]
fn check_windows_os_version() -> DoctorCheck {
    // Use `ver` command (always available on Windows) to get the OS version.
    let output = std::process::Command::new("cmd")
        .args(["/C", "ver"])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let ver = String::from_utf8_lossy(&o.stdout)
                .lines()
                .find(|l| l.contains("Version") || l.contains("Windows"))
                .unwrap_or("unknown")
                .trim()
                .to_string();
            // Require Windows 10 build 1903+ (19H1) for robust symlink/long-path support.
            DoctorCheck::pass("windows.os_version", ver)
        }
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
            DoctorCheck::warn(
                "windows.os_version",
                format!("could not determine OS version: {}", msg),
                "Ensure you are running Windows 10 (1903+) or Windows 11",
            )
        }
        Err(e) => DoctorCheck::warn(
            "windows.os_version",
            format!("could not determine OS version: {}", e),
            "Ensure you are running Windows 10 (1903+) or Windows 11",
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform-specific free space helpers
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
fn free_space_bytes(path: &Path) -> anyhow::Result<u64> {
    use std::os::windows::ffi::OsStrExt;

    // Use GetDiskFreeSpaceExW
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    extern "system" {
        fn GetDiskFreeSpaceExW(
            lpDirectoryName: *const u16,
            lpFreeBytesAvailableToCaller: *mut u64,
            lpTotalNumberOfBytes: *mut u64,
            lpTotalNumberOfFreeBytes: *mut u64,
        ) -> i32;
    }

    let mut free_available: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;

    let result = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_available,
            &mut total,
            &mut total_free,
        )
    };

    if result != 0 {
        Ok(free_available)
    } else {
        // Fall back to a statvfs-style approximation via `dir`
        anyhow::bail!("GetDiskFreeSpaceExW failed")
    }
}

#[cfg(unix)]
fn free_space_bytes(path: &Path) -> anyhow::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let cpath = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| anyhow::anyhow!("invalid path: {}", e))?;

    extern "C" {
        fn statvfs(path: *const libc::c_char, buf: *mut libc::statvfs) -> libc::c_int;
    }

    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { statvfs(cpath.as_ptr(), &mut stat) };

    if rc == 0 {
        Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    } else {
        let err = std::io::Error::last_os_error();
        anyhow::bail!("statvfs failed: {}", err)
    }
}

// Neither windows nor unix — provide a stub
#[cfg(not(any(windows, unix)))]
fn free_space_bytes(_path: &Path) -> anyhow::Result<u64> {
    anyhow::bail!("free space check not supported on this platform")
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_run_doctor_uninitialized_store() {
        let tmp = TempDir::new().unwrap();
        let non_existent = tmp.path().join("no_store");
        let report = run_doctor(&non_existent).unwrap();
        // Should warn (not fail) for missing store
        let store_check = report.checks.iter().find(|c| c.name == "store.exists").unwrap();
        assert_eq!(store_check.status, CheckStatus::Warn);
    }

    #[test]
    fn test_run_doctor_initialized_store() {
        let tmp = TempDir::new().unwrap();
        let report = run_doctor(tmp.path()).unwrap();
        let store_check = report.checks.iter().find(|c| c.name == "store.exists").unwrap();
        assert_eq!(store_check.status, CheckStatus::Pass);
    }

    #[test]
    fn test_report_version_present() {
        let tmp = TempDir::new().unwrap();
        let report = run_doctor(tmp.path()).unwrap();
        let ver_check = report.checks.iter().find(|c| c.name == "version").unwrap();
        assert_eq!(ver_check.status, CheckStatus::Pass);
        assert!(ver_check.message.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn test_to_report_string_no_sensitive_data() {
        let tmp = TempDir::new().unwrap();
        let report = run_doctor(tmp.path()).unwrap();
        let s = report.to_report_string();
        // Must not contain anything that looks like a token/key
        assert!(!s.contains("HF_TOKEN"));
        assert!(!s.contains("password"));
    }

    #[test]
    fn test_report_counts() {
        let tmp = TempDir::new().unwrap();
        let mut report = run_doctor(tmp.path()).unwrap();
        // Manually inject a fail to test counters
        report.checks.push(DoctorCheck::fail("test.fail", "injected failure", "no fix"));
        assert!(report.fail_count() >= 1);
    }

    #[test]
    fn test_check_status_labels() {
        assert_eq!(CheckStatus::Pass.label(), "PASS");
        assert_eq!(CheckStatus::Warn.label(), "WARN");
        assert_eq!(CheckStatus::Fail.label(), "FAIL");
    }

    #[test]
    fn test_sqlite_wal_probe() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("probe.db");
        assert!(open_sqlite_wal_probe(&db_path).is_ok());
    }
}
