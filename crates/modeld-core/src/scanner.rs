//! File system scanner for discovering model files
//!
//! Implements recursive directory traversal with:
//! - File extension filtering
//! - Hash computation with caching
//! - Progress tracking

use crate::hash::{hash_file, Blake3Hash};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Supported model file extensions
const MODEL_EXTENSIONS: &[&str] = &["safetensors", "gguf", "ckpt", "pth", "pt", "bin"];

/// Scanned file information
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub hash: Blake3Hash,
}

/// Scanner configuration
pub struct Scanner {
    /// Extensions to scan for
    extensions: Vec<String>,
}

impl Scanner {
    /// Create a new scanner with default extensions
    pub fn new() -> Self {
        Self { extensions: MODEL_EXTENSIONS.iter().map(|s| s.to_string()).collect() }
    }

    /// Set custom extensions to scan for
    pub fn with_extensions(mut self, extensions: Vec<String>) -> Self {
        self.extensions = extensions;
        self
    }

    /// Scan a directory recursively
    pub fn scan<F>(&self, root: &Path, mut progress_callback: F) -> Result<Vec<ScannedFile>>
    where
        F: FnMut(&Path, u64),
    {
        let mut results = Vec::new();

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !self.is_excluded(e.path()))
        {
            let entry =
                entry.with_context(|| format!("Failed to read entry in {}", root.display()))?;

            // Skip directories
            if entry.file_type().is_dir() {
                continue;
            }

            let path = entry.path();

            // Check if file matches our extensions
            if !self.matches_extension(path) {
                continue;
            }

            // Get file size
            let metadata = entry.metadata()?;
            let size = metadata.len();

            // Compute hash
            progress_callback(path, size);
            let hash =
                hash_file(path).with_context(|| format!("Failed to hash {}", path.display()))?;

            results.push(ScannedFile { path: path.to_path_buf(), size, hash });
        }

        Ok(results)
    }

    /// Check if path should be excluded
    fn is_excluded(&self, path: &Path) -> bool {
        // Check each path component
        for component in path.components() {
            if let Some(name) = component.as_os_str().to_str() {
                // Exclude common non-model directories
                if name == "node_modules" || name == "venv" || name == "__pycache__" {
                    return true;
                }
                // Exclude hidden directories that start with dot but are actual directory names
                // (not .tmp* which is used by tempfile crate)
                if name.starts_with('.')
                    && !name.starts_with(".tmp")
                    && name != "."
                    && name != ".."
                    && name != ".git"
                // Keep .git for now in case
                {
                    return true;
                }
            }
        }
        false
    }

    /// Check if file matches our extension filters
    fn matches_extension(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            self.extensions.iter().any(|allowed| allowed.eq_ignore_ascii_case(ext))
        } else {
            false
        }
    }

    /// Count matching files without hashing (for quick preview)
    pub fn count_files(&self, root: &Path) -> Result<(usize, u64)> {
        let mut count = 0;
        let mut total_size = 0;

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !self.is_excluded(e.path()))
        {
            let entry = entry?;

            if entry.file_type().is_dir() {
                continue;
            }

            if self.matches_extension(entry.path()) {
                count += 1;
                total_size += entry.metadata()?.len();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
        let temp_dir = TempDir::new().unwrap();

        // Create test files
        create_test_file(temp_dir.path(), "model1.safetensors", b"test1");
        create_test_file(temp_dir.path(), "model2.gguf", b"test2");
        create_test_file(temp_dir.path(), "readme.txt", b"not a model");

        let scanner = Scanner::new();
        let results = scanner.scan(temp_dir.path(), |_, _| {}).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|f| f.path.to_str().unwrap().contains("model1.safetensors")));
        assert!(results.iter().any(|f| f.path.to_str().unwrap().contains("model2.gguf")));
    }

    #[test]
    fn test_scanner_recursive() {
        let temp_dir = TempDir::new().unwrap();

        // Create nested structure
        create_test_file(temp_dir.path(), "root.safetensors", b"root");
        create_test_file(temp_dir.path(), "sub/nested.gguf", b"nested");
        create_test_file(temp_dir.path(), "sub/deep/model.ckpt", b"deep");

        let scanner = Scanner::new();
        let results = scanner.scan(temp_dir.path(), |_, _| {}).unwrap();

        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_scanner_exclude_hidden() {
        let temp_dir = TempDir::new().unwrap();

        // Create files, some in hidden directories
        create_test_file(temp_dir.path(), "visible.safetensors", b"visible");
        create_test_file(temp_dir.path(), ".hidden/model.safetensors", b"hidden");

        let scanner = Scanner::new();
        let results = scanner.scan(temp_dir.path(), |_, _| {}).unwrap();

        // Should only find the visible file
        assert_eq!(results.len(), 1);
        assert!(results[0].path.to_str().unwrap().contains("visible.safetensors"));
    }

    #[test]
    fn test_count_files() {
        let temp_dir = TempDir::new().unwrap();

        create_test_file(temp_dir.path(), "model1.safetensors", b"test1");
        create_test_file(temp_dir.path(), "model2.gguf", b"test22");
        create_test_file(temp_dir.path(), "readme.txt", b"not a model");

        let scanner = Scanner::new();
        let (count, total_size) = scanner.count_files(temp_dir.path()).unwrap();

        assert_eq!(count, 2);
        assert_eq!(total_size, 5 + 6); // "test1" + "test22"
    }

    #[test]
    fn test_custom_extensions() {
        let temp_dir = TempDir::new().unwrap();

        create_test_file(temp_dir.path(), "model.custom", b"custom");
        create_test_file(temp_dir.path(), "model.safetensors", b"standard");

        let scanner = Scanner::new().with_extensions(vec!["custom".to_string()]);
        let results = scanner.scan(temp_dir.path(), |_, _| {}).unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].path.to_str().unwrap().contains("model.custom"));
    }

    #[test]
    fn test_progress_callback() {
        let temp_dir = TempDir::new().unwrap();
        create_test_file(temp_dir.path(), "model.safetensors", b"test");

        let scanner = Scanner::new();
        let mut progress_count = 0;

        scanner
            .scan(temp_dir.path(), |_path, _size| {
                progress_count += 1;
            })
            .unwrap();

        assert_eq!(progress_count, 1);
    }
}
