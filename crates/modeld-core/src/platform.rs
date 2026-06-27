//! Platform capability detection
//!
//! Detects filesystem and OS-level capabilities needed by the store:
//! - Symlink privilege (Windows: `SeCreateSymbolicLinkPrivilege`)
//! - Hardlink support
//! - Junction point support (Windows)
//! - Long path support (Windows: registry opt-in or manifest)
//! - Filesystem type for a given path
//! - Same-volume detection (required for hardlinks)
//! - File lock detection (Windows: `ERROR_SHARING_VIOLATION`)

use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of platform capabilities for a given store path.
#[derive(Debug, Clone)]
pub struct PlatformCapabilities {
    /// Whether the current process can create symbolic links.
    ///
    /// On Windows this requires the `SeCreateSymbolicLinkPrivilege` or
    /// Developer Mode.  On Unix it is always available.
    pub has_symlink_privilege: bool,
    /// Whether the filesystem supports hard links.
    pub supports_hardlink: bool,
    /// Whether the OS supports NTFS junction points (Windows only).
    pub supports_junction: bool,
    /// Whether the OS / filesystem supports paths longer than MAX_PATH (260
    /// characters on Windows without the registry opt-in).
    pub supports_long_path: bool,
    /// Human-readable filesystem type name (e.g. "NTFS", "ext4", "apfs",
    /// "unknown").
    pub filesystem_type: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Primary API
// ─────────────────────────────────────────────────────────────────────────────

/// Detect the capabilities of the filesystem at `path`.
///
/// Uses probe-based detection where possible so that the results reflect the
/// actual runtime environment rather than compile-time assumptions.
pub fn detect_capabilities(path: &Path) -> PlatformCapabilities {
    let probe_root = if path.is_dir() { path.to_path_buf() } else {
        path.parent().unwrap_or(path).to_path_buf()
    };

    PlatformCapabilities {
        has_symlink_privilege: probe_symlink_privilege(&probe_root),
        supports_hardlink: probe_hardlink(&probe_root),
        supports_junction: probe_junction_support(),
        supports_long_path: probe_long_path_support(),
        filesystem_type: detect_filesystem_type(path),
    }
}

/// Return `true` if both paths reside on the same volume / block device.
///
/// - **Unix**: compares the `st_dev` field from `stat(2)`.
/// - **Windows**: compares the drive-letter / UNC root obtained from
///   `GetVolumePathName`.  Falls back to comparing the first 3 characters of
///   the canonicalised path (e.g. `C:\`) when the Windows API is unavailable.
pub fn is_same_volume(p1: &Path, p2: &Path) -> bool {
    is_same_volume_impl(p1, p2)
}

/// Return `true` if `path` is currently held open by another process in a way
/// that would prevent writes or deletes.
///
/// - **Windows**: attempts to open the file with `GENERIC_READ | GENERIC_WRITE`
///   and `FILE_SHARE_READ` only.  Returns `true` when the result is
///   `ERROR_SHARING_VIOLATION` (error code 32).
/// - **Unix**: advisory locks are not enforced by the kernel, so this always
///   returns `false`.  (Use `flock(2)` or `lockf(3)` for advisory lock checks
///   if needed.)
pub fn check_file_locked(path: &Path) -> bool {
    check_file_locked_impl(path)
}

// ─────────────────────────────────────────────────────────────────────────────
// Unix implementation
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(unix)]
fn probe_symlink_privilege(dir: &Path) -> bool {
    use std::fs;
    let src = dir.join(".probe_symlink_src");
    let dst = dir.join(".probe_symlink_dst");
    // Create a real source file first
    if fs::write(&src, b"").is_err() {
        return false;
    }
    let ok = std::os::unix::fs::symlink(&src, &dst).is_ok();
    let _ = fs::remove_file(&dst);
    let _ = fs::remove_file(&src);
    ok
}

#[cfg(unix)]
fn probe_hardlink(dir: &Path) -> bool {
    use std::fs;
    let src = dir.join(".probe_hardlink_src");
    let dst = dir.join(".probe_hardlink_dst");
    if fs::write(&src, b"").is_err() {
        return false;
    }
    let ok = fs::hard_link(&src, &dst).is_ok();
    let _ = fs::remove_file(&dst);
    let _ = fs::remove_file(&src);
    ok
}

#[cfg(unix)]
fn probe_junction_support() -> bool {
    false // junctions are Windows-only
}

#[cfg(unix)]
fn probe_long_path_support() -> bool {
    true // POSIX systems support long paths natively
}

#[cfg(unix)]
fn detect_filesystem_type(path: &Path) -> String {
    // Read /proc/mounts (Linux) or /proc/self/mounts and match the device
    detect_fs_from_proc_mounts(path).unwrap_or_else(|| "unknown".to_string())
}

#[cfg(unix)]
fn detect_fs_from_proc_mounts(path: &Path) -> Option<String> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    // Try both Linux paths
    let mounts_path = if std::path::Path::new("/proc/mounts").exists() {
        "/proc/mounts"
    } else if std::path::Path::new("/proc/self/mounts").exists() {
        "/proc/self/mounts"
    } else {
        return None;
    };

    let canonical = path.canonicalize().ok()?;
    let canon_str = canonical.to_string_lossy().to_string();

    let file = File::open(mounts_path).ok()?;
    let reader = BufReader::new(file);

    // Find the longest mount point that is a prefix of the target path
    let mut best_mount: Option<(String, String)> = None; // (mount_point, fs_type)
    for line in reader.lines().flatten() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let mount_point = parts[1];
        let fs_type = parts[2];
        if canon_str.starts_with(mount_point) {
            if best_mount.as_ref().map_or(0, |(mp, _)| mp.len()) < mount_point.len() {
                best_mount = Some((mount_point.to_string(), fs_type.to_string()));
            }
        }
    }

    best_mount.map(|(_, fs)| fs)
}

#[cfg(unix)]
fn is_same_volume_impl(p1: &Path, p2: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let m1 = std::fs::metadata(p1);
    let m2 = std::fs::metadata(p2);
    match (m1, m2) {
        (Ok(m1), Ok(m2)) => m1.dev() == m2.dev(),
        _ => {
            // Fall back to comparing the canonicalised path prefixes
            same_volume_by_path_prefix(p1, p2)
        }
    }
}

#[cfg(unix)]
fn check_file_locked_impl(_path: &Path) -> bool {
    // Advisory locks are not enforced on Unix at the kernel level.
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Windows implementation
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
fn probe_symlink_privilege(dir: &Path) -> bool {
    use std::fs;
    // Developer Mode / elevated symlink creation
    let src = dir.join(".probe_symlink_src");
    let dst = dir.join(".probe_symlink_dst");
    if fs::write(&src, b"").is_err() {
        return false;
    }
    let ok = std::os::windows::fs::symlink_file(&src, &dst).is_ok();
    let _ = fs::remove_file(&dst);
    let _ = fs::remove_file(&src);
    ok
}

#[cfg(windows)]
fn probe_hardlink(dir: &Path) -> bool {
    use std::fs;
    let src = dir.join(".probe_hardlink_src");
    let dst = dir.join(".probe_hardlink_dst");
    if fs::write(&src, b"").is_err() {
        return false;
    }
    let ok = fs::hard_link(&src, &dst).is_ok();
    let _ = fs::remove_file(&dst);
    let _ = fs::remove_file(&src);
    ok
}

#[cfg(windows)]
fn probe_junction_support() -> bool {
    // NTFS junctions are available on all supported Windows versions.
    // The only exception would be non-NTFS file systems; we optimistically
    // return true here — the caller should verify with is_same_volume first.
    true
}

#[cfg(windows)]
fn probe_long_path_support() -> bool {
    // Check HKLM\SYSTEM\CurrentControlSet\Control\FileSystem\LongPathsEnabled
    // We use a registry read via the winreg-compatible approach using std APIs.
    // If we can't read it, assume not supported (safe default).
    read_long_path_registry().unwrap_or(false)
}

#[cfg(windows)]
fn read_long_path_registry() -> Option<bool> {
    // Use the Windows registry via raw winapi-compatible constants.
    // We avoid the `winreg` crate to stay dependency-free; instead we call
    // the underlying Windows API through `std::process::Command` as a last
    // resort, or just check the environment.

    // Query via `reg query` (always available on Windows).
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\FileSystem",
            "/v",
            "LongPathsEnabled",
        ])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // The output contains a line like: "    LongPathsEnabled    REG_DWORD    0x1"
    for line in stdout.lines() {
        if line.contains("LongPathsEnabled") {
            return Some(line.contains("0x1"));
        }
    }
    Some(false)
}

#[cfg(windows)]
fn detect_filesystem_type(path: &Path) -> String {
    detect_fs_windows(path).unwrap_or_else(|| "unknown".to_string())
}

#[cfg(windows)]
fn detect_fs_windows(path: &Path) -> Option<String> {
    // Extract the volume root (e.g. "C:\") from the path.
    let volume_root = get_volume_root(path)?;

    // Use `fsutil fsinfo volumeinfo <root>` — available on all modern Windows.
    let output = std::process::Command::new("fsutil")
        .args(["fsinfo", "volumeinfo", &volume_root])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let lower = line.to_lowercase();
        if lower.contains("file system name") || lower.contains("file system type") {
            // e.g. "File System Name : NTFS"
            if let Some(pos) = line.rfind(':') {
                let fs = line[pos + 1..].trim().to_string();
                if !fs.is_empty() {
                    return Some(fs);
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn is_same_volume_impl(p1: &Path, p2: &Path) -> bool {
    match (get_volume_root(p1), get_volume_root(p2)) {
        (Some(v1), Some(v2)) => v1.eq_ignore_ascii_case(&v2),
        _ => same_volume_by_path_prefix(p1, p2),
    }
}

#[cfg(windows)]
fn get_volume_root(path: &Path) -> Option<String> {
    // For drive-letter paths: "C:\..." → "C:\"
    // For UNC paths: "\\server\share\..." → "\\server\share\"
    let s = path.to_string_lossy();
    if s.len() >= 3 && s.chars().nth(1) == Some(':') {
        // Drive-letter path
        return Some(s[..3].to_string());
    }
    if s.starts_with(r"\\") {
        // UNC path — take first two components
        let parts: Vec<&str> = s[2..].splitn(3, '\\').collect();
        if parts.len() >= 2 {
            return Some(format!(r"\\{}\{}\", parts[0], parts[1]));
        }
    }
    // Try canonicalize as fallback
    let canonical = path.canonicalize().ok()?;
    let c = canonical.to_string_lossy();
    if c.len() >= 3 && c.chars().nth(1) == Some(':') {
        return Some(c[..3].to_string());
    }
    None
}

#[cfg(windows)]
fn check_file_locked_impl(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;

    // Convert path to wide string
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // Try to open with GENERIC_READ | GENERIC_WRITE, sharing READ only.
    // If another process has the file open exclusively, this will fail with
    // ERROR_SHARING_VIOLATION (0x20 / 32).
    unsafe {
        // SAFETY: calling Windows API with a null-terminated wide string.
        let handle = windows_open_file_probe(wide.as_ptr());
        if handle == usize::MAX {
            // INVALID_HANDLE_VALUE — check GetLastError
            let err = get_last_error();
            return err == 32; // ERROR_SHARING_VIOLATION
        }
        // Successfully opened — close the handle
        windows_close_handle(handle);
        false
    }
}

// Thin wrappers around kernel32 functions to avoid the winapi/windows-sys dep.
#[cfg(windows)]
unsafe fn windows_open_file_probe(path_wide: *const u16) -> usize {
    extern "system" {
        fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut std::ffi::c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
    }
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const OPEN_EXISTING: u32 = 3;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

    let h = CreateFileW(
        path_wide,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ,
        std::ptr::null_mut(),
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        std::ptr::null_mut(),
    );
    h as usize
}

#[cfg(windows)]
unsafe fn windows_close_handle(handle: usize) {
    extern "system" {
        fn CloseHandle(hObject: *mut std::ffi::c_void) -> i32;
    }
    CloseHandle(handle as *mut std::ffi::c_void);
}

#[cfg(windows)]
fn get_last_error() -> u32 {
    extern "system" {
        fn GetLastError() -> u32;
    }
    unsafe { GetLastError() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Portable fallback helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Fallback same-volume check: compare first 3 characters of canonicalised paths
/// (works for single-letter drive paths on Windows and root on Unix).
fn same_volume_by_path_prefix(p1: &Path, p2: &Path) -> bool {
    let c1 = p1.canonicalize().ok().unwrap_or_else(|| p1.to_path_buf());
    let c2 = p2.canonicalize().ok().unwrap_or_else(|| p2.to_path_buf());

    let s1 = c1.to_string_lossy();
    let s2 = c2.to_string_lossy();

    // For drive paths ("C:\...") compare first 2 chars; for UNC compare first
    // two path components; for Unix both paths start with "/" so this returns
    // true (which is the correct assumption on most Unix systems).
    let prefix_len = 3.min(s1.len()).min(s2.len());
    s1[..prefix_len].eq_ignore_ascii_case(&s2[..prefix_len])
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_detect_capabilities_runs() {
        let tmp = TempDir::new().unwrap();
        let caps = detect_capabilities(tmp.path());
        // Just ensure it doesn't panic and returns something
        println!("caps: {:?}", caps);
        // filesystem_type should not be empty
        assert!(!caps.filesystem_type.is_empty());
    }

    #[test]
    fn test_is_same_volume_same_dir() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        // Files in the same tmpdir must be on the same volume
        assert!(is_same_volume(&a, &b));
    }

    #[test]
    fn test_check_file_locked_non_existent() {
        // A non-existent file is not locked (can't be opened → no sharing violation)
        let path = std::path::PathBuf::from("/nonexistent/path/that/does/not/exist.bin");
        // On Windows: ERROR_FILE_NOT_FOUND (2), not ERROR_SHARING_VIOLATION (32)
        assert!(!check_file_locked(&path));
    }

    #[test]
    fn test_is_same_volume_fallback() {
        // Ensure the fallback path-prefix comparison doesn't panic
        let p1 = std::path::Path::new(".");
        let p2 = std::path::Path::new(".");
        let _ = is_same_volume(p1, p2);
    }
}
