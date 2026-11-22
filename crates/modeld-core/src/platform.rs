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

use anyhow::Result;
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
    let probe_root = if path.is_dir() {
        path.to_path_buf()
    } else {
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

/// Flush a directory's entries to disk.
///
/// After an atomic rename the new name is durable only once the *directory*
/// holding it is flushed; `fsync` on the file does not cover its name.
///
/// - **Unix**: opens the directory and calls `fsync(2)`.
/// - **Windows**: a no-op that returns `Ok(())`. NTFS journals directory metadata and
///   the Win32 API has no way to flush a directory handle, so there is nothing
///   to do; callers can invoke this unconditionally.
pub fn fsync_dir(dir: &Path) -> Result<()> {
    fsync_dir_impl(dir)
}

#[cfg(unix)]
fn fsync_dir_impl(dir: &Path) -> Result<()> {
    use anyhow::{anyhow, Context};
    use std::fs;
    use std::os::unix::io::AsRawFd;
    let dir_file =
        fs::File::open(dir).with_context(|| format!("Open dir for fsync: {}", dir.display()))?;
    let ret = unsafe { libc_fsync(dir_file.as_raw_fd()) };
    if ret != 0 {
        return Err(anyhow!(
            "fsync dir {} failed with errno {}",
            dir.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(unix)]
extern "C" {
    fn fsync(fd: i32) -> i32;
}

#[cfg(unix)]
unsafe fn libc_fsync(fd: i32) -> i32 {
    fsync(fd)
}

#[cfg(not(unix))]
fn fsync_dir_impl(_dir: &Path) -> Result<()> {
    Ok(())
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
    // Drive-letter and UNC paths resolve directly, without touching the disk.
    if let Some(root) = volume_root_of(&path.to_string_lossy()) {
        return Some(root);
    }
    // Relative paths (e.g. `--store target\store`) only become absolute after
    // canonicalisation. `Path::canonicalize` on Windows returns a *verbatim*
    // path (`\\?\D:\...`), which is why `volume_root_of` strips that prefix —
    // without the strip the drive letter sits at index 4 and every relative
    // store path looked like a different volume. That silently downgraded
    // `dedup` to "record a reference_only alias and reclaim nothing".
    let canonical = path.canonicalize().ok()?;
    volume_root_of(&canonical.to_string_lossy())
}

/// Parse a volume root out of a Windows path string.
///
/// `C:\dir` → `C:\`, `\\server\share\x` → `\\server\share`,
/// `\\?\D:\dir` → `D:\` (the verbatim prefix is stripped first).
#[cfg(windows)]
fn volume_root_of(raw: &str) -> Option<String> {
    let s = normalize_for_compare(raw);
    if s.len() >= 3 && s.as_bytes()[1] == b':' {
        return Some(s[..3].to_ascii_uppercase());
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        let parts: Vec<&str> = rest.splitn(3, '\\').collect();
        if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            return Some(format!(r"\\{}\{}", parts[0], parts[1]).to_ascii_uppercase());
        }
    }
    None
}

/// Remove the `\\?\` / `\\?\UNC\` prefix that `canonicalize` adds on Windows.
#[cfg(windows)]
fn normalize_for_compare(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

#[cfg(not(windows))]
fn normalize_for_compare(s: &str) -> String {
    s.to_string()
}

#[cfg(windows)]
fn check_file_locked_impl(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;

    // Convert path to wide string
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

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

/// Fallback same-volume check: compare the leading path component of both
/// paths (drive letter on Windows, `/` on Unix).
///
/// The verbatim prefix from `canonicalize` is stripped first: `\\?\C:\` and
/// `\\?\D:\` both start with `\\?\`, so the raw strings compared equal and two
/// *different* drives were reported as the same volume — the opposite failure
/// of the relative-path bug above, and just as damaging.
fn same_volume_by_path_prefix(p1: &Path, p2: &Path) -> bool {
    let c1 = p1.canonicalize().ok().unwrap_or_else(|| p1.to_path_buf());
    let c2 = p2.canonicalize().ok().unwrap_or_else(|| p2.to_path_buf());

    let s1 = normalize_for_compare(&c1.to_string_lossy());
    let s2 = normalize_for_compare(&c2.to_string_lossy());

    // Compare the first three *chars*, not bytes: slicing a UTF-8 string at
    // byte 3 would panic when a path component starts with a multi-byte char.
    let head = |s: &str| s.chars().take(3).collect::<String>().to_ascii_uppercase();
    head(&s1) == head(&s2)
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
        tracing::debug!("caps: {:?}", caps);
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

    /// Regression (B6): a *relative* store path made `dedup` believe the CAS and
    /// the library were on different volumes, so it silently recorded a
    /// `reference_only` alias and reclaimed nothing. Relative paths must
    /// resolve to the same volume as their absolute counterparts.
    #[cfg(windows)]
    #[test]
    fn relative_paths_on_the_same_drive_are_same_volume() {
        // Work inside the current directory so a relative path genuinely points
        // at the files we create; `TempDir` would be outside the CWD.
        let base = std::path::Path::new("target").join("volume-rel-test");
        let lib = base.join("lib");
        let cas = base.join("store").join("cas");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&cas).unwrap();
        let dup = lib.join("model.bin");
        let blob = cas.join("blob.bin");
        std::fs::write(&dup, b"same").unwrap();
        std::fs::write(&blob, b"same").unwrap();

        assert!(is_same_volume(&dup, &blob), "relative paths under one drive must share a volume");
        // Control: the absolute form agrees with the relative one.
        assert!(is_same_volume(&dup.canonicalize().unwrap(), &blob.canonicalize().unwrap()));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// `canonicalize` yields `\\?\D:\...`; the verbatim prefix must be stripped
    /// before the drive letter is read, and two different drives must not be
    /// conflated just because both strings begin with `\\?\`.
    #[cfg(windows)]
    #[test]
    fn verbatim_prefix_is_stripped_before_volume_compare() {
        assert_eq!(volume_root_of(r"\\?\D:\AI\store\cas\ab"), Some(r"D:\".to_string()));
        assert_eq!(volume_root_of(r"D:\AI\store"), Some(r"D:\".to_string()));
        assert_eq!(volume_root_of(r"d:\lower"), Some(r"D:\".to_string()));
        assert_eq!(volume_root_of(r"\\?\UNC\srv\share\x"), Some(r"\\SRV\SHARE".to_string()));
        assert_eq!(volume_root_of("relative/path"), None);
        assert_ne!(volume_root_of(r"\\?\C:\a"), volume_root_of(r"\\?\D:\b"));
    }
}
