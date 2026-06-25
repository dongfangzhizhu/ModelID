//! File system scanner for discovering model files
//!
//! ## Changes (audit)
//! - 3.3: Files are now hashed in parallel via Rayon after a serial WalkDir
//!   collection pass.  The progress callback is `Fn + Sync` so it can be
//!   invoked safely from multiple threads.
//! - 5.6: Incremental scan — pass a pre-indexed cache from the DB so unchanged
//!   files (same path + same size) are returned immediately without re-hashing.

use crate::hash::{hash_file, Blake3Hash};
use anyhow::Result;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Supported model file extensions
const MODEL_EXTENSIONS: &[&str] = &["safetensors", "gguf", "ckpt", "pth", "pt", "bin"];

/// A single file discovered (and hashed) during a scan.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub hash: Blake3Hash,
    /// True when the hash was taken from the incremental cache (no disk read).
    pub from_cache: bool,
}

/// Scanner configuration
pub struct Scanner {
    extensions: Vec<String>,
    /// Absolute paths excluded from scanning (e.g. the modeld store directory).
    excluded_dirs: Vec<PathBuf>,
    /// path → (hash, size_bytes) pre-indexed from the DB for incremental scan.
    preindexed: HashMap<String, (Blake3Hash, i64)>,
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            extensions: MODEL_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            excluded_dirs: Vec::new(),
            preindexed: HashMap::new(),
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
    /// Build `cache` from `db.get_all_indexed_paths()`.  Files whose path is
    /// present in the cache **and** whose current on-disk size matches the
    /// cached size are returned without re-hashing — saving all the I/O for
    /// large unchanged model files.
    pub fn with_preindexed(mut self, cache: HashMap<String, (Blake3Hash, i64)>) -> Self {
        self.preindexed = cache;
        self
    }

    /// Scan a directory recursively.
    ///
    /// Phase 1 (serial): WalkDir collects all matching paths + sizes.
    /// Phase 2 (parallel): Rayon hashes each file concurrently.  Files found in
    /// the incremental cache with a matching size are returned without hashing.
    ///
    /// The `progress_callback` is called from the Rayon thread pool; it must be
    /// `Fn + Sync` (a simple `ProgressBar::inc` call works fine — indicatif's
    /// `ProgressBar` is `Clone + Send + Sync`).
    ///
    /// Individual file errors are logged to stderr and skipped; the outer
    /// `Result` only propagates fatal directory-level failures.
    pub fn scan<F>(&self, root: &Path, progress_callback: F) -> Result<Vec<ScannedFile>>
    where
        F: Fn(&Path, u64) + Sync,
    {
        // ── Phase 1: collect paths (serial — WalkDir is single-threaded) ────
        let mut to_process: Vec<(PathBuf, u64)> = Vec::new();

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !self.is_excluded(e.path()))
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

            let size = match entry.metadata() {
                Ok(m) => m.len(),
                Err(e) => {
                    eprintln!("scan: skipping {} (metadata: {e})", path.display());
                    continue;
                }
            };

            to_process.push((path, size));
        }

        // ── Phase 2: hash in parallel (Rayon work-stealing pool) ────────────
        let results: Vec<ScannedFile> = to_process
            .par_iter()
            .filter_map(|(path, size)| {
                let path_str = path.to_string_lossy();

                // Incremental: use cached hash when size hasn't changed
                if let Some((cached_hash, cached_size)) = self.preindexed.get(path_str.as_ref()) {
                    if *cached_size == *size as i64 {
                        progress_callback(path, *size);
                        return Some(ScannedFile {
                            path: path.clone(),
                            size: *size,
                            hash: cached_hash.clone(),
                            from_cache: true,
                        });
                    }
                }

                progress_callback(path, *size);

                match hash_file(path) {
                    Ok(hash) => Some(ScannedFile { path: path.clone(), size: *size, hash, from_cache: false }),
                    Err(e) => {
                        eprintln!("scan: skipping {} (hash error: {e})", path.display());
                        None
                    }
                }
            })
            .collect();

        Ok(results)
    }

    /// Check if path should be excluded from scanning.
    fn is_excluded(&self, path: &Path) -> bool {
        for excl in &self.excluded_dirs {
            if path.starts_with(excl) {
                return true;
            }
        }
        for component in path.components() {
            if let Some(name) = component.as_os_str().to_str() {
                if matches!(name, "node_modules" | "venv" | "__pycache__") {
                    return true;
                }
                if name.starts_with('.')
                    && !name.starts_with(".tmp")
                    && name != "."
                    && name != ".."
                {
                    return true;
                }
            }
        }
        false
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
            .follow_links(false)
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
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
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
            .scan(tmp.path(), move |_, _| { c.fetch_add(1, Ordering::Relaxed); })
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
        let size = fs::metadata(&f).unwrap().len();
        cache.insert(f.to_string_lossy().to_string(), (fake_hash.clone(), size as i64));

        let results = Scanner::new().with_preindexed(cache).scan(tmp.path(), |_, _| {}).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].from_cache);
        assert_eq!(results[0].hash.as_hex(), fake_hash.as_hex());
    }

    #[test]
    fn test_excluded_dirs() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("store");
        fs::create_dir_all(&store).unwrap();
        create_test_file(tmp.path(), "model.safetensors", b"model");
        create_test_file(&store, "cas_object.safetensors", b"cas");

        let results = Scanner::new()
            .with_excluded_dirs(vec![store])
            .scan(tmp.path(), |_, _| {})
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].path.file_name().unwrap() == "model.safetensors");
    }
}
