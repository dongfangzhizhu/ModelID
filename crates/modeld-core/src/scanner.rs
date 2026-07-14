//! File system scanner for discovering model files
//!
//! ## Changes (audit)
//! - 3.3: Files are now hashed in parallel via Rayon after a serial WalkDir
//!   collection pass.  The progress callback is `Fn + Sync` so it can be
//!   invoked safely from multiple threads.
//! - 5.6: Incremental scan — pass a pre-indexed cache from the DB so unchanged
//!   files (same path + same size) are returned immediately without re-hashing.
//! - 14.3: ScanOptions (incremental/full flags, exclude globs, follow_symlinks),
//!   symlink cycle detection via visited HashSet, glob-pattern exclusions,
//!   full mtime + inode incremental check, WsBroadcaster progress events.

use crate::db::FileIndexEntry;
use crate::hash::{hash_file, Blake3Hash};
use anyhow::Result;
use rayon::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Supported model file extensions
const MODEL_EXTENSIONS: &[&str] = &["safetensors", "gguf", "ckpt", "pth", "pt", "bin"];

/// Options that control how a scan is performed.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Skip re-hashing files whose size (and mtime when available) match the
    /// pre-indexed cache.  This is the default behaviour.
    pub incremental: bool,
    /// Force a full re-hash of every file, ignoring any cached values.
    /// Takes precedence over `incremental`.
    pub full: bool,
    /// Glob-style patterns for paths to exclude.  Each entry is matched
    /// against the full path string of every file/directory encountered.
    /// A simple wildcard `*` is supported (matches any sequence of chars
    /// within a single path component).  Use `**` for recursive matches.
    ///
    /// Examples: `"*.tmp"`, `"venv/*"`, `"**/__pycache__/**"`
    pub exclude_globs: Vec<String>,
    /// Follow symbolic links when traversing directories.
    /// Symlink cycles are detected and skipped automatically.
    /// Default: `false`.
    pub follow_symlinks: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { incremental: true, full: false, exclude_globs: Vec::new(), follow_symlinks: false }
    }
}

/// A single file discovered (and hashed) during a scan.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub hash: Blake3Hash,
    /// True when the hash was taken from the incremental cache (no disk read).
    pub from_cache: bool,
    /// Last-modified time as Unix timestamp (seconds since epoch).
    /// `0` if the platform could not provide reliable mtime.
    pub mtime: i64,
    /// Inode number (Unix) or file index (Windows).  `0` if unavailable.
    pub inode: i64,
    /// Device ID (Unix) or volume serial number (Windows).  `0` if unavailable.
    pub device_id: i64,
}

/// Scanner configuration
pub struct Scanner {
    extensions: Vec<String>,
    /// Absolute paths excluded from scanning (e.g. the modeld store directory).
    excluded_dirs: Vec<PathBuf>,
    /// path → FileIndexEntry pre-indexed from the DB for incremental scan.
    preindexed: HashMap<String, FileIndexEntry>,
    /// Scan-time options (incremental flags, globs, symlink following).
    scan_options: ScanOptions,
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            extensions: MODEL_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            excluded_dirs: Vec::new(),
            preindexed: HashMap::new(),
            scan_options: ScanOptions::default(),
        }
    }

    pub fn with_extensions(mut self, extensions: Vec<String>) -> Self {
        self.extensions = extensions;
        self
    }

    /// Exclude specific directories from scanning (e.g. the modeld store).
    pub fn with_excluded_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.excluded_dirs = dirs;
        self
    }

    /// Supply a pre-indexed cache for incremental scanning.
    ///
    /// Build `cache` from `db.get_all_indexed_paths()`.  Files whose path,
    /// size, mtime, and inode/file-id all match the cached entry are returned
    /// without re-hashing, saving all I/O for large unchanged model files.
    pub fn with_preindexed(mut self, cache: HashMap<String, FileIndexEntry>) -> Self {
        self.preindexed = cache;
        self
    }

    /// Set scan-time options (incremental/full flags, globs, symlink following).
    pub fn with_scan_options(mut self, opts: ScanOptions) -> Self {
        self.scan_options = opts;
        self
    }

    /// Scan a directory recursively.
    ///
    /// Phase 1 (serial): WalkDir collects all matching paths + sizes + mtime +
    ///   inode/device_id for incremental skip decisions.
    ///   - Symlink cycle detection is active when `follow_symlinks = true`.
    ///   - Glob exclusions from `ScanOptions::exclude_globs` are applied here.
    ///
    /// Phase 2 (parallel): Rayon hashes each file concurrently.  Files found in
    /// the incremental cache with matching size **and** mtime **and** inode are
    /// returned without hashing (unless `ScanOptions::full = true`).
    ///
    /// The `progress_callback` receives `(path, size)` per file and is called
    /// from the Rayon thread pool — it must be `Fn + Sync`.  The caller can
    /// also forward these calls to a `WsBroadcaster` for real-time WebSocket
    /// progress events.
    ///
    /// Individual file errors are logged to stderr and skipped; the outer
    /// `Result` only propagates fatal directory-level failures.
    pub fn scan<F>(&self, root: &Path, progress_callback: F) -> Result<Vec<ScannedFile>>
    where
        F: Fn(&Path, u64) + Sync,
    {
        let opts = &self.scan_options;

        // ── Phase 1: collect paths (serial — WalkDir is single-threaded) ────

        // Track visited directory identities to detect symlink cycles.
        // Uses (dev, inode) on Unix; (0, canonical_path_hash) on Windows.
        let visited: RefCell<HashSet<(u64, u64)>> = RefCell::new(HashSet::new());

        // (path, size, mtime_secs, inode, device_id)
        let mut to_process: Vec<(PathBuf, u64, i64, i64, i64)> = Vec::new();

        for entry in
            WalkDir::new(root).follow_links(opts.follow_symlinks).into_iter().filter_entry(|e| {
                let path = e.path();

                // ── Symlink cycle detection for directories ──────────────────
                if e.file_type().is_dir() {
                    // Use the real (non-symlink) metadata for ID purposes
                    let meta_result = if opts.follow_symlinks && e.path_is_symlink() {
                        std::fs::metadata(path)
                    } else {
                        std::fs::symlink_metadata(path)
                    };

                    if let Ok(meta) = meta_result {
                        if let Some(id) = get_dir_id(path, &meta) {
                            let mut vis = visited.borrow_mut();
                            if vis.contains(&id) {
                                eprintln!(
                                    "scan: symlink cycle detected at {}, skipping",
                                    path.display()
                                );
                                return false;
                            }
                            vis.insert(id);
                        }
                    }
                }

                // ── Standard exclusion check ─────────────────────────────────
                !self.is_excluded_with_opts(path, opts)
            })
        {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("scan: skipping entry ({e})");
                    continue;
                }
            };

            if entry.file_type().is_dir() {
                continue;
            }

            let path = entry.path().to_path_buf();
            if !self.matches_extension(&path) {
                continue;
            }

            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("scan: skipping {} (metadata: {e})", path.display());
                    continue;
                }
            };

            let size = meta.len();

            // Extract mtime as Unix timestamp (seconds since epoch).
            let mtime: i64 = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            // Extract (inode, device_id) for the file.
            let (inode, device_id) = get_file_id(&path, &meta);

            to_process.push((path, size, mtime, inode, device_id));
        }

        // ── Phase 2: hash in parallel (Rayon work-stealing pool) ────────────
        let force_full = opts.full;
        let use_incremental = opts.incremental && !force_full;

        let results: Vec<ScannedFile> = to_process
            .par_iter()
            .filter_map(|(path, size, mtime, inode, device_id)| {
                let path_str = path.to_string_lossy();

                // Incremental: use cached hash when size, mtime, and inode
                // all match, unless --full was requested.
                if use_incremental {
                    if let Some(entry) = self.preindexed.get(path_str.as_ref()) {
                        let size_match = entry.size == *size as i64;
                        // mtime check: skip if both are non-zero and match
                        let mtime_match = *mtime == 0 || entry.mtime == 0 || entry.mtime == *mtime;
                        // inode check: skip if both are non-zero and match
                        let inode_match = *inode == 0
                            || entry.inode == 0
                            || (entry.inode == *inode && entry.device_id == *device_id);

                        if size_match && mtime_match && inode_match {
                            progress_callback(path, *size);
                            return Some(ScannedFile {
                                path: path.clone(),
                                size: *size,
                                hash: entry.hash.clone(),
                                from_cache: true,
                                mtime: *mtime,
                                inode: *inode,
                                device_id: *device_id,
                            });
                        }
                    }
                }

                progress_callback(path, *size);

                match hash_file(path) {
                    Ok(hash) => Some(ScannedFile {
                        path: path.clone(),
                        size: *size,
                        hash,
                        from_cache: false,
                        mtime: *mtime,
                        inode: *inode,
                        device_id: *device_id,
                    }),
                    Err(e) => {
                        eprintln!("scan: skipping {} (hash error: {e})", path.display());
                        None
                    }
                }
            })
            .collect();

        Ok(results)
    }

    /// Check if path should be excluded from scanning (using current options).
    fn is_excluded_with_opts(&self, path: &Path, opts: &ScanOptions) -> bool {
        // 1. Hard-coded excluded dir prefixes
        for excl in &self.excluded_dirs {
            if path.starts_with(excl) {
                return true;
            }
        }

        // 2. Built-in pattern exclusions for well-known noise directories
        for component in path.components() {
            if let Some(name) = component.as_os_str().to_str() {
                if matches!(name, "node_modules" | "venv" | "__pycache__") {
                    return true;
                }
                if name.starts_with('.') && !name.starts_with(".tmp") && name != "." && name != ".."
                {
                    return true;
                }
            }
        }

        // 3. Caller-supplied glob patterns
        if !opts.exclude_globs.is_empty() {
            let path_str = path.to_string_lossy();
            // Normalise separators so patterns work cross-platform
            let path_norm = path_str.replace('\\', "/");
            for pattern in &opts.exclude_globs {
                if glob_matches(pattern, &path_norm) {
                    return true;
                }
            }
        }

        false
    }

    /// Check if path should be excluded from scanning (legacy — uses default options).
    fn is_excluded(&self, path: &Path) -> bool {
        self.is_excluded_with_opts(path, &self.scan_options)
    }

    fn matches_extension(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            self.extensions.iter().any(|a| a.eq_ignore_ascii_case(ext))
        } else {
            false
        }
    }

    /// Count matching files without hashing (for progress-bar initialisation).
    pub fn count_files(&self, root: &Path) -> Result<(usize, u64)> {
        let mut count = 0usize;
        let mut total_size = 0u64;

        for entry in WalkDir::new(root)
            .follow_links(self.scan_options.follow_symlinks)
            .into_iter()
            .filter_entry(|e| !self.is_excluded(e.path()))
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.file_type().is_dir() {
                continue;
            }
            if self.matches_extension(entry.path()) {
                if let Ok(m) = entry.metadata() {
                    count += 1;
                    total_size += m.len();
                }
            }
        }

        Ok((count, total_size))
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Return a unique identifier (device_id, inode/file_index) for a directory,
/// used to detect symlink cycles.
///
/// - Unix: `(dev, ino)` from `MetadataExt`.
/// - Windows: falls back to a hash of the canonicalised path since
///   `file_index()` requires a file handle obtained from `CreateFile`.
/// - Other: returns `None` (cycle detection disabled).
fn get_dir_id(path: &Path, meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    get_dir_id_impl(path, meta)
}

/// Return `(inode, device_id)` for a regular file.
///
/// Used to detect whether a file is identical to a previously-indexed entry
/// (same file, not just same name/size/mtime).
///
/// - Unix: returns `(ino, dev)` from `MetadataExt`.
/// - Windows: returns `(0, 0)` — a future improvement could use
///   `GetFileInformationByHandle`.
/// - Other: returns `(0, 0)`.
fn get_file_id(_path: &Path, meta: &std::fs::Metadata) -> (i64, i64) {
    get_file_id_impl(_path, meta)
}

#[cfg(unix)]
fn get_dir_id_impl(_path: &Path, meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn get_dir_id_impl(path: &Path, _meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    // Windows: use canonicalized path hash as a best-effort cycle breaker.
    // True inode-equivalent detection would require CreateFile +
    // GetFileInformationByHandle, which is left for future work.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let canonical = std::fs::canonicalize(path).ok()?;
    let mut hasher = DefaultHasher::new();
    canonical.hash(&mut hasher);
    Some((0u64, hasher.finish()))
}

#[cfg(not(any(unix, windows)))]
fn get_dir_id_impl(_path: &Path, _meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

// ── get_file_id implementations ──────────────────────────────────────────────

#[cfg(unix)]
fn get_file_id_impl(_path: &Path, meta: &std::fs::Metadata) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (meta.ino() as i64, meta.dev() as i64)
}

#[cfg(windows)]
fn get_file_id_impl(_path: &Path, _meta: &std::fs::Metadata) -> (i64, i64) {
    // True file-index detection on Windows requires opening the file with
    // CreateFile + GetFileInformationByHandle, which is non-trivial.  We
    // return (0, 0) so the incremental check falls back to size+mtime only.
    (0, 0)
}

#[cfg(not(any(unix, windows)))]
fn get_file_id_impl(_path: &Path, _meta: &std::fs::Metadata) -> (i64, i64) {
    (0, 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Glob matching
// ─────────────────────────────────────────────────────────────────────────────

/// Minimal glob matcher for path exclusions.
///
/// Supports:
/// - `**` — matches any sequence of characters including path separators.
/// - `*`  — matches any sequence of characters within a single path segment.
/// - Literal prefix/suffix without wildcards.
///
/// Both the pattern and the path should use `/` as the separator.
pub(crate) fn glob_matches(pattern: &str, path: &str) -> bool {
    // Fast path: no wildcard — check if path contains the literal pattern
    if !pattern.contains('*') {
        return path.contains(pattern);
    }

    // Split on ** first (greedy recursive wildcard)
    if let Some(pos) = pattern.find("**") {
        let before = &pattern[..pos];
        let after = &pattern[pos + 2..];

        // Strip leading/trailing slashes from segments
        let before = before.trim_end_matches('/');
        let after = after.trim_start_matches('/');

        let prefix_ok = before.is_empty() || path.contains(before);
        if !prefix_ok {
            return false;
        }
        // `after` may itself contain further wildcards — handle recursively.
        if after.is_empty() {
            return true;
        }
        return glob_matches(after, path);
    }

    // Single-star glob: split pattern into segments separated by *
    // and ensure each literal segment appears in order in the path.
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut remaining = path;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            // First segment must match a prefix of the path (or anywhere inside)
            if let Some(idx) = remaining.find(part) {
                remaining = &remaining[idx + part.len()..];
            } else {
                return false;
            }
        } else if let Some(idx) = remaining.find(part) {
            remaining = &remaining[idx + part.len()..];
        } else {
            return false;
        }
    }

    // If the pattern ends without a *, the path must end with the last segment
    if !pattern.ends_with('*') {
        if let Some(last) = parts.last() {
            if !last.is_empty() && !path.ends_with(last) {
                return false;
            }
        }
    }

    true
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::FileIndexEntry;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn create_test_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_scanner_basic() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "model1.safetensors", b"test1");
        create_test_file(tmp.path(), "model2.gguf", b"test2");
        create_test_file(tmp.path(), "readme.txt", b"not a model");

        let scanner = Scanner::new();
        let results = scanner.scan(tmp.path(), |_, _| {}).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|f| f.path.file_name().unwrap() == "model1.safetensors"));
        assert!(results.iter().any(|f| f.path.file_name().unwrap() == "model2.gguf"));
    }

    #[test]
    fn test_scanner_recursive() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "root.safetensors", b"root");
        create_test_file(tmp.path(), "sub/nested.gguf", b"nested");
        create_test_file(tmp.path(), "sub/deep/model.ckpt", b"deep");

        let results = Scanner::new().scan(tmp.path(), |_, _| {}).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_scanner_exclude_hidden() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "visible.safetensors", b"visible");
        create_test_file(tmp.path(), ".hidden/model.safetensors", b"hidden");

        let results = Scanner::new().scan(tmp.path(), |_, _| {}).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].path.file_name().unwrap() == "visible.safetensors");
    }

    #[test]
    fn test_count_files() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "model1.safetensors", b"test1");
        create_test_file(tmp.path(), "model2.gguf", b"test22");
        create_test_file(tmp.path(), "readme.txt", b"not a model");

        let (count, size) = Scanner::new().count_files(tmp.path()).unwrap();
        assert_eq!(count, 2);
        assert_eq!(size, 5 + 6); // "test1" + "test22"
    }

    #[test]
    fn test_custom_extensions() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "model.custom", b"custom");
        create_test_file(tmp.path(), "model.safetensors", b"standard");

        let results = Scanner::new()
            .with_extensions(vec!["custom".to_string()])
            .scan(tmp.path(), |_, _| {})
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].path.file_name().unwrap() == "model.custom");
    }

    #[test]
    fn test_progress_callback_thread_safe() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "model.safetensors", b"test");

        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        Scanner::new()
            .scan(tmp.path(), move |_, _| {
                c.fetch_add(1, Ordering::Relaxed);
            })
            .unwrap();

        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_incremental_cache_skips_hashing() {
        let tmp = TempDir::new().unwrap();
        let f = create_test_file(tmp.path(), "model.safetensors", b"hello world");

        // Pre-index: pretend the file is already known with a specific hash
        let fake_hash = Blake3Hash::from_hex(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let mut cache = HashMap::new();
        let meta = fs::metadata(&f).unwrap();
        let size = meta.len();
        let mtime: i64 = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        cache.insert(
            f.to_string_lossy().to_string(),
            FileIndexEntry {
                hash: fake_hash.clone(),
                size: size as i64,
                mtime,
                inode: 0,
                device_id: 0,
            },
        );

        let results = Scanner::new().with_preindexed(cache).scan(tmp.path(), |_, _| {}).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].from_cache);
        assert_eq!(results[0].hash.as_hex(), fake_hash.as_hex());
    }

    #[test]
    fn test_incremental_mtime_change_forces_rehash() {
        let tmp = TempDir::new().unwrap();
        let f = create_test_file(tmp.path(), "model.safetensors", b"hello world");

        let fake_hash = Blake3Hash::from_hex(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let mut cache = HashMap::new();
        let size = fs::metadata(&f).unwrap().len();
        // Pretend the cached mtime is much older (different from actual)
        cache.insert(
            f.to_string_lossy().to_string(),
            FileIndexEntry {
                hash: fake_hash.clone(),
                size: size as i64,
                mtime: 1000, // old mtime — will not match current mtime
                inode: 0,
                device_id: 0,
            },
        );

        let results = Scanner::new().with_preindexed(cache).scan(tmp.path(), |_, _| {}).unwrap();
        assert_eq!(results.len(), 1);
        // mtime mismatch → should re-hash (not from cache)
        assert!(!results[0].from_cache);
        assert_ne!(results[0].hash.as_hex(), fake_hash.as_hex());
    }

    #[test]
    fn test_full_flag_bypasses_cache() {
        let tmp = TempDir::new().unwrap();
        let f = create_test_file(tmp.path(), "model.safetensors", b"hello world");

        let fake_hash = Blake3Hash::from_hex(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let mut cache = HashMap::new();
        let meta = fs::metadata(&f).unwrap();
        let size = meta.len();
        let mtime: i64 = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        cache.insert(
            f.to_string_lossy().to_string(),
            FileIndexEntry {
                hash: fake_hash.clone(),
                size: size as i64,
                mtime,
                inode: 0,
                device_id: 0,
            },
        );

        // With --full, even though size+mtime match, we must not use cache
        let opts = ScanOptions { full: true, incremental: true, ..Default::default() };
        let results = Scanner::new()
            .with_preindexed(cache)
            .with_scan_options(opts)
            .scan(tmp.path(), |_, _| {})
            .unwrap();

        assert_eq!(results.len(), 1);
        // from_cache must be false because full=true forces re-hash
        assert!(!results[0].from_cache);
        // The real hash is different from the fake one
        assert_ne!(results[0].hash.as_hex(), fake_hash.as_hex());
    }

    #[test]
    fn test_excluded_dirs() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("store");
        fs::create_dir_all(&store).unwrap();
        create_test_file(tmp.path(), "model.safetensors", b"model");
        create_test_file(&store, "cas_object.safetensors", b"cas");

        let results =
            Scanner::new().with_excluded_dirs(vec![store]).scan(tmp.path(), |_, _| {}).unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].path.file_name().unwrap() == "model.safetensors");
    }

    #[test]
    fn test_exclude_glob_pattern() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "model.safetensors", b"model");
        create_test_file(tmp.path(), "cache/model.gguf", b"cached");

        let opts = ScanOptions { exclude_globs: vec!["cache".to_string()], ..Default::default() };
        let results = Scanner::new().with_scan_options(opts).scan(tmp.path(), |_, _| {}).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].path.file_name().unwrap() == "model.safetensors");
    }

    #[test]
    fn test_exclude_glob_star_pattern() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "model.safetensors", b"model");
        create_test_file(tmp.path(), "temp/model.gguf", b"temp");

        let opts = ScanOptions { exclude_globs: vec!["temp*".to_string()], ..Default::default() };
        let results = Scanner::new().with_scan_options(opts).scan(tmp.path(), |_, _| {}).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_glob_matches_fn() {
        assert!(glob_matches("venv", "project/venv/lib/model.pt"));
        assert!(glob_matches("*.tmp", "model.safetensors.tmp"));
        assert!(!glob_matches("*.tmp", "model.safetensors"));
        assert!(glob_matches("**/__pycache__/**", "src/__pycache__/cache.py"));
        assert!(glob_matches("cache/*", "cache/model.gguf"));
        assert!(!glob_matches("cache/*", "other/model.gguf"));
    }

    #[test]
    fn test_scan_options_default() {
        let opts = ScanOptions::default();
        assert!(opts.incremental);
        assert!(!opts.full);
        assert!(!opts.follow_symlinks);
        assert!(opts.exclude_globs.is_empty());
    }

    #[test]
    fn test_scanned_file_has_metadata() {
        let tmp = TempDir::new().unwrap();
        create_test_file(tmp.path(), "model.safetensors", b"test content");

        let results = Scanner::new().scan(tmp.path(), |_, _| {}).unwrap();
        assert_eq!(results.len(), 1);
        let f = &results[0];
        assert_eq!(f.size, 12); // "test content"
                                // mtime should be populated (non-zero on most filesystems)
                                // inode should be non-zero on Unix
        #[cfg(unix)]
        {
            assert!(f.inode != 0, "inode should be non-zero on Unix");
            assert!(f.device_id != 0, "device_id should be non-zero on Unix");
        }
    }
}
