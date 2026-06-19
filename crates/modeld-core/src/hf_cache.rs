//! HuggingFace fake cache layout management (Phase 3)
//!
//! Emulates the official `~/.cache/huggingface/hub/` directory structure so
//! all HF-based frameworks (diffusers, transformers, ComfyUI, etc.) can load
//! models transparently from modeld's CAS storage.
//!
//! ## Directory Structure
//!
//! ```text
//! $MODELD_STORE/hf_cache/hub/
//! └── models--{org}--{model}/
//!     ├── blobs/
//!     │   └── {sha256}  →  symlink → ../../../../cas/blake3/{prefix}/{blake3}
//!     ├── refs/
//!     │   └── main      →  text file containing revision hash
//!     └── snapshots/
//!         └── {revision}/
//!             └── {filename}  →  ../../blobs/{sha256}
//! ```

use crate::cas::CasStore;
use crate::hash::Blake3Hash;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Manages the fake HuggingFace cache directory structure
pub struct HfCache {
    /// Root: $MODELD_STORE/hf_cache
    root: PathBuf,
    /// CAS store reference (for CAS path resolution)
    cas: CasStore,
}

impl HfCache {
    /// Create a new HfCache manager
    pub fn new(store_path: &Path) -> Self {
        let root = store_path.join("hf_cache");
        let cas = CasStore::new(store_path);
        Self { root, cas }
    }

    /// Initialize the HF cache directory
    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(self.root.join("hub"))
            .context("Failed to create hf_cache/hub directory")?;
        Ok(())
    }

    /// Get the HF_HOME path (to set as environment variable)
    pub fn hf_home(&self) -> &Path {
        &self.root
    }

    /// Get the hub directory path
    pub fn hub_dir(&self) -> PathBuf {
        self.root.join("hub")
    }

    /// Convert a HuggingFace repo_id to a model directory name
    /// e.g. "stabilityai/sdxl-base" → "models--stabilityai--sdxl-base"
    pub fn repo_to_dir_name(repo_id: &str) -> String {
        format!("models--{}", repo_id.replace('/', "--"))
    }

    /// Get the model-specific directory path
    pub fn model_dir(&self, repo_id: &str) -> PathBuf {
        self.hub_dir().join(Self::repo_to_dir_name(repo_id))
    }

    /// Get the blobs directory for a model
    pub fn blobs_dir(&self, repo_id: &str) -> PathBuf {
        self.model_dir(repo_id).join("blobs")
    }

    /// Get the snapshots directory for a model
    pub fn snapshots_dir(&self, repo_id: &str) -> PathBuf {
        self.model_dir(repo_id).join("snapshots")
    }

    /// Get the refs directory for a model
    pub fn refs_dir(&self, repo_id: &str) -> PathBuf {
        self.model_dir(repo_id).join("refs")
    }

    /// Get the snapshot path for a specific revision
    pub fn snapshot_dir(&self, repo_id: &str, revision: &str) -> PathBuf {
        self.snapshots_dir(repo_id).join(revision)
    }

    /// Get the path of a specific file in a snapshot
    pub fn snapshot_file_path(&self, repo_id: &str, revision: &str, filename: &str) -> PathBuf {
        self.snapshot_dir(repo_id, revision).join(filename)
    }

    /// Get the blob path for a given SHA256 hash
    pub fn blob_path(&self, repo_id: &str, sha256: &str) -> PathBuf {
        self.blobs_dir(repo_id).join(sha256)
    }

    /// Check if a file already exists in the fake HF cache
    pub fn check_cache(&self, repo_id: &str, revision: &str, filename: &str) -> bool {
        self.snapshot_file_path(repo_id, revision, filename).exists()
    }

    /// Check if a blob (by SHA256) is already in the fake HF cache
    pub fn has_blob(&self, repo_id: &str, sha256: &str) -> bool {
        self.blob_path(repo_id, sha256).exists()
    }

    /// Create the full fake HF cache entry for a downloaded file.
    ///
    /// This creates:
    /// - `blobs/{sha256}` → symlink to CAS object
    /// - `snapshots/{revision}/{filename}` → relative symlink to blob
    /// - `refs/{branch}` → text file with revision hash
    pub fn create_cache_entry(
        &self,
        repo_id: &str,
        filename: &str,
        revision: &str,
        sha256: &str,
        blake3_hash: &Blake3Hash,
        branch: Option<&str>,
    ) -> Result<PathBuf> {
        // 1. Ensure directories exist
        let blobs_dir = self.blobs_dir(repo_id);
        let snapshot_dir = self.snapshot_dir(repo_id, revision);
        let refs_dir = self.refs_dir(repo_id);

        // Handle subdirectories in filename (e.g. "text_encoder/model.safetensors")
        let file_path_in_snapshot = snapshot_dir.join(filename);
        let parent_dir = file_path_in_snapshot.parent().unwrap_or(&snapshot_dir);

        fs::create_dir_all(&blobs_dir).context("Failed to create blobs directory")?;
        fs::create_dir_all(parent_dir).context("Failed to create snapshot subdirectory")?;
        fs::create_dir_all(&refs_dir).context("Failed to create refs directory")?;

        // 2. Create blob → CAS symlink
        let blob_path = blobs_dir.join(sha256);
        if !blob_path.exists() {
            let cas_path = self.cas.get(blake3_hash).ok_or_else(|| {
                anyhow::anyhow!("CAS object not found for blake3 hash: {}", blake3_hash.as_hex())
            })?;
            create_symlink_or_copy(&cas_path, &blob_path).with_context(|| {
                format!(
                    "Failed to create blob symlink {} → {}",
                    blob_path.display(),
                    cas_path.display()
                )
            })?;
        }

        // 3. Create snapshot/{revision}/{filename} → ../../blobs/{sha256}
        if !file_path_in_snapshot.exists() {
            // Calculate relative path from snapshot file to blob
            let depth = filename.matches('/').count() + 1; // how many dirs deep
            let relative_prefix = "../".repeat(depth + 1); // +1 for snapshots/{rev}/
            let relative_blob = format!("{}blobs/{}", relative_prefix, sha256);
            create_symlink_or_copy_rel(&blob_path, &file_path_in_snapshot, &relative_blob)
                .with_context(|| format!("Failed to create snapshot symlink for {}", filename))?;
        }

        // 4. Write refs/{branch} = revision
        let branch_name = branch.unwrap_or("main");
        let ref_file = refs_dir.join(branch_name);
        if !ref_file.exists() {
            fs::write(&ref_file, revision).context("Failed to write refs file")?;
        }

        Ok(file_path_in_snapshot)
    }

    /// Create a blob entry pointing to a CAS file without creating a snapshot symlink.
    /// Useful when we know the sha256 but don't yet have the full snapshot structure.
    pub fn create_blob_only(
        &self,
        repo_id: &str,
        sha256: &str,
        blake3_hash: &Blake3Hash,
    ) -> Result<PathBuf> {
        let blobs_dir = self.blobs_dir(repo_id);
        fs::create_dir_all(&blobs_dir)?;
        let blob_path = blobs_dir.join(sha256);
        if !blob_path.exists() {
            let cas_path = self
                .cas
                .get(blake3_hash)
                .ok_or_else(|| anyhow::anyhow!("CAS object not found for blake3 hash"))?;
            create_symlink_or_copy(&cas_path, &blob_path)?;
        }
        Ok(blob_path)
    }

    /// List all repo IDs present in the fake HF cache
    pub fn list_repos(&self) -> Result<Vec<String>> {
        let hub = self.hub_dir();
        if !hub.exists() {
            return Ok(vec![]);
        }
        let mut repos = Vec::new();
        for entry in fs::read_dir(&hub)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("models--") {
                // Convert "models--org--model" back to "org/model"
                let repo_id = name.strip_prefix("models--").unwrap_or(&name).replacen("--", "/", 1);
                repos.push(repo_id);
            }
        }
        repos.sort();
        Ok(repos)
    }

    /// Get statistics about the HF cache
    pub fn stats(&self) -> Result<HfCacheStats> {
        let repos = self.list_repos()?;
        let total_repos = repos.len();
        let mut total_blobs = 0usize;

        for repo_id in &repos {
            let blobs_dir = self.blobs_dir(repo_id);
            if blobs_dir.exists() {
                if let Ok(entries) = fs::read_dir(&blobs_dir) {
                    total_blobs += entries.count();
                }
            }
        }

        Ok(HfCacheStats { total_repos, total_blobs, hf_home: self.root.clone() })
    }
}

/// Statistics about the fake HF cache
#[derive(Debug)]
pub struct HfCacheStats {
    pub total_repos: usize,
    pub total_blobs: usize,
    pub hf_home: PathBuf,
}

// ─────────────────────────────────────────────────────────────────────────────
// Platform-specific symlink creation
// ─────────────────────────────────────────────────────────────────────────────

/// Create a symlink (absolute) from `target` ← `link`.
/// Falls back to copy if symlink creation fails.
fn create_symlink_or_copy(target: &Path, link: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)?;
    }
    #[cfg(windows)]
    {
        // Try symlink first (requires Developer Mode or admin)
        let result = std::os::windows::fs::symlink_file(target, link);
        if result.is_err() {
            // Fall back to hard link (same volume) or copy
            if fs::hard_link(target, link).is_err() {
                fs::copy(target, link).context("Failed to copy as fallback")?;
            }
        }
    }
    Ok(())
}

/// Create a symlink using a relative path string (for snapshot → blob links)
fn create_symlink_or_copy_rel(target: &Path, link: &Path, _relative: &str) -> Result<()> {
    // On all platforms, use the absolute target for simplicity
    // The relative path is what HF clients expect but absolute also works
    create_symlink_or_copy(target, link)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_repo_to_dir_name() {
        assert_eq!(
            HfCache::repo_to_dir_name("stabilityai/sdxl-base"),
            "models--stabilityai--sdxl-base"
        );
        assert_eq!(
            HfCache::repo_to_dir_name("meta-llama/Llama-3-8B"),
            "models--meta-llama--Llama-3-8B"
        );
        assert_eq!(HfCache::repo_to_dir_name("single"), "models--single");
    }

    #[test]
    fn test_hf_cache_init() {
        let tmp = TempDir::new().unwrap();
        let cache = HfCache::new(tmp.path());
        cache.init().unwrap();

        assert!(tmp.path().join("hf_cache/hub").exists());
        assert!(cache.hf_home().exists());
    }

    #[test]
    fn test_check_cache_miss() {
        let tmp = TempDir::new().unwrap();
        let cache = HfCache::new(tmp.path());
        cache.init().unwrap();

        // No entries, should be a miss
        assert!(!cache.check_cache("org/model", "main", "model.safetensors"));
    }

    #[test]
    fn test_list_repos_empty() {
        let tmp = TempDir::new().unwrap();
        let cache = HfCache::new(tmp.path());
        cache.init().unwrap();

        let repos = cache.list_repos().unwrap();
        assert!(repos.is_empty());
    }

    #[test]
    fn test_create_cache_entry() {
        use crate::cas::CasStore;
        use tempfile::NamedTempFile;

        let tmp = TempDir::new().unwrap();
        let store_path = tmp.path();

        // Initialize CAS and HF cache
        let cas = CasStore::new(store_path);
        cas.init().unwrap();

        let cache = HfCache::new(store_path);
        cache.init().unwrap();

        // Create a fake model file and store in CAS
        let model_file = NamedTempFile::new().unwrap();
        let content = b"fake safetensors model data for testing 1234567890";
        std::fs::write(model_file.path(), content).unwrap();

        let hash = crate::hash::hash_file(model_file.path()).unwrap();
        cas.store(model_file.path(), &hash).unwrap();

        // Create fake SHA256 (normally from HF API)
        let sha256 = "a".repeat(64);

        // Create the cache entry
        let file_path = cache
            .create_cache_entry(
                "stabilityai/sdxl-base",
                "model.safetensors",
                "abc123def456",
                &sha256,
                &hash,
                Some("main"),
            )
            .unwrap();

        // Verify structure
        assert!(file_path.exists(), "Snapshot file should exist");

        let blob_path = cache.blob_path("stabilityai/sdxl-base", &sha256);
        assert!(blob_path.exists(), "Blob should exist");

        let refs_file = cache.refs_dir("stabilityai/sdxl-base").join("main");
        assert!(refs_file.exists(), "refs/main should exist");

        let rev_in_ref = std::fs::read_to_string(&refs_file).unwrap();
        assert_eq!(rev_in_ref.trim(), "abc123def456");

        // Verify cache hit
        assert!(cache.check_cache("stabilityai/sdxl-base", "abc123def456", "model.safetensors"));

        // Verify repos list
        let repos = cache.list_repos().unwrap();
        assert!(repos.iter().any(|r| r.contains("stabilityai")));
    }

    #[test]
    fn test_hf_cache_stats() {
        let tmp = TempDir::new().unwrap();
        let cache = HfCache::new(tmp.path());
        cache.init().unwrap();

        let stats = cache.stats().unwrap();
        assert_eq!(stats.total_repos, 0);
        assert_eq!(stats.total_blobs, 0);
    }
}
