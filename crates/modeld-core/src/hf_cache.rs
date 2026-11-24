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

//! ## B5 compatibility notes
//! - `refs/{branch}` stores the resolved **commit SHA**, never a branch name.
//! - Snapshot entries are **relative symlinks** (`../../blobs/{sha256}`), as
//!   `huggingface_hub` writes them.  A failed symlink is an error, never a
//!   silent copy — a copy would break HF's cache identity and dedup.
//! - `repo_type` selects the HF cache prefix (`models--` / `datasets--` /
//!   `spaces--`), matching `huggingface_hub`'s directory naming.

use crate::cas::CasStore;
use crate::hash::Blake3Hash;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// HF repository type, mapped to the cache directory prefix used by
/// `huggingface_hub` (e.g. `models--org--name`, `datasets--org--name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepoType {
    #[default]
    Model,
    Dataset,
    Space,
}

impl RepoType {
    pub fn prefix(self) -> &'static str {
        match self {
            RepoType::Model => "models--",
            RepoType::Dataset => "datasets--",
            RepoType::Space => "spaces--",
        }
    }

    /// Parse the CLI / Python value (`model` / `dataset` / `space`).
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "model" | "models" => Ok(RepoType::Model),
            "dataset" | "datasets" => Ok(RepoType::Dataset),
            "space" | "spaces" => Ok(RepoType::Space),
            other => Err(anyhow::anyhow!(
                "unknown HF repo type '{}' (expected model/dataset/space)",
                other
            )),
        }
    }
}

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
        Self::repo_to_dir_name_with_type(repo_id, RepoType::Model)
    }

    /// `repo_type`-aware variant of `repo_to_dir_name`.
    pub fn repo_to_dir_name_with_type(repo_id: &str, repo_type: RepoType) -> String {
        format!("{}{}", repo_type.prefix(), repo_id.replace('/', "--"))
    }

    /// Get the model-specific directory path
    pub fn model_dir(&self, repo_id: &str) -> PathBuf {
        self.model_dir_with_type(repo_id, RepoType::Model)
    }

    pub fn model_dir_with_type(&self, repo_id: &str, repo_type: RepoType) -> PathBuf {
        self.hub_dir().join(Self::repo_to_dir_name_with_type(repo_id, repo_type))
    }

    /// Get the blobs directory for a model
    pub fn blobs_dir(&self, repo_id: &str) -> PathBuf {
        self.blobs_dir_with_type(repo_id, RepoType::Model)
    }

    pub fn blobs_dir_with_type(&self, repo_id: &str, repo_type: RepoType) -> PathBuf {
        self.model_dir_with_type(repo_id, repo_type).join("blobs")
    }

    /// Get the snapshots directory for a model
    pub fn snapshots_dir(&self, repo_id: &str) -> PathBuf {
        self.snapshots_dir_with_type(repo_id, RepoType::Model)
    }

    pub fn snapshots_dir_with_type(&self, repo_id: &str, repo_type: RepoType) -> PathBuf {
        self.model_dir_with_type(repo_id, repo_type).join("snapshots")
    }

    /// Get the refs directory for a model
    pub fn refs_dir(&self, repo_id: &str) -> PathBuf {
        self.refs_dir_with_type(repo_id, RepoType::Model)
    }

    pub fn refs_dir_with_type(&self, repo_id: &str, repo_type: RepoType) -> PathBuf {
        self.model_dir_with_type(repo_id, repo_type).join("refs")
    }

    /// Get the snapshot path for a specific revision
    pub fn snapshot_dir(&self, repo_id: &str, revision: &str) -> PathBuf {
        self.snapshot_dir_with_type(repo_id, revision, RepoType::Model)
    }

    pub fn snapshot_dir_with_type(
        &self,
        repo_id: &str,
        revision: &str,
        repo_type: RepoType,
    ) -> PathBuf {
        self.snapshots_dir_with_type(repo_id, repo_type).join(revision)
    }

    /// Get the path of a specific file in a snapshot
    pub fn snapshot_file_path(&self, repo_id: &str, revision: &str, filename: &str) -> PathBuf {
        self.snapshot_file_path_with_type(repo_id, revision, filename, RepoType::Model)
    }

    pub fn snapshot_file_path_with_type(
        &self,
        repo_id: &str,
        revision: &str,
        filename: &str,
        repo_type: RepoType,
    ) -> PathBuf {
        self.snapshot_dir_with_type(repo_id, revision, repo_type).join(filename)
    }

    /// Get the blob path for a given SHA256 hash
    pub fn blob_path(&self, repo_id: &str, sha256: &str) -> PathBuf {
        self.blob_path_with_type(repo_id, sha256, RepoType::Model)
    }

    pub fn blob_path_with_type(&self, repo_id: &str, sha256: &str, repo_type: RepoType) -> PathBuf {
        self.blobs_dir_with_type(repo_id, repo_type).join(sha256)
    }

    /// Check if a file already exists in the fake HF cache
    pub fn check_cache(&self, repo_id: &str, revision: &str, filename: &str) -> bool {
        self.snapshot_file_path(repo_id, revision, filename).exists()
    }

    pub fn check_cache_with_type(
        &self,
        repo_id: &str,
        revision: &str,
        filename: &str,
        repo_type: RepoType,
    ) -> bool {
        self.snapshot_file_path_with_type(repo_id, revision, filename, repo_type).exists()
    }

    /// Check if a blob (by SHA256) is already in the fake HF cache
    pub fn has_blob(&self, repo_id: &str, sha256: &str) -> bool {
        self.blob_path(repo_id, sha256).exists()
    }

    /// Create a *relative* symlink at `link` pointing to `relative_target`.
    ///
    /// B5: symlink failure is a hard error.  Falling back to a copy silently
    /// would break `huggingface_hub`'s cache identity (it resolves blobs by
    /// inode/link, and a copy would never be deduplicated).
    /// Normalise a relative symlink target to the host separator.
    ///
    /// Windows `CreateSymbolicLinkW` does not resolve forward-slash relative
    /// targets when the link is dereferenced; they create dangling links even
    /// though `read_link()` returns the expected text. Unix accepts only `/`.
    fn relative_target_for_host(target: &str) -> String {
        #[cfg(windows)]
        {
            target.replace('/', "\\")
        }
        #[cfg(not(windows))]
        {
            target.to_string()
        }
    }

    fn create_relative_symlink(link: &Path, relative_target: &str) -> Result<()> {
        #[cfg(unix)]
        std::os::unix::fs::symlink(relative_target, link).with_context(|| {
            format!(
                "cannot create relative symlink {} -> {} (symlinks are required for HF cache compatibility)",
                link.display(), relative_target
            )
        })?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(relative_target, link).with_context(|| {
            format!(
                "cannot create relative symlink {} -> {} (enable Developer Mode or run as administrator; symlinks are required for HF cache compatibility)",
                link.display(), relative_target
            )
        })?;
        Ok(())
    }

    /// Create the full fake HF cache entry for a downloaded file.
    ///
    /// `commit_sha` must be the resolved 40-hex git commit — never a branch
    /// name.  `huggingface_hub` reads `refs/{branch}` to resolve a branch to a
    /// commit and then looks under `snapshots/{commit_sha}`.
    pub fn create_cache_entry(
        &self,
        repo_id: &str,
        filename: &str,
        revision: &str,
        sha256: &str,
        blake3_hash: &Blake3Hash,
        branch: Option<&str>,
    ) -> Result<PathBuf> {
        self.create_cache_entry_with_type(
            repo_id,
            filename,
            revision,
            sha256,
            blake3_hash,
            branch,
            RepoType::Model,
            None,
        )
    }

    /// `repo_type` / `commit_sha`-aware variant.
    #[allow(clippy::too_many_arguments)]
    pub fn create_cache_entry_with_type(
        &self,
        repo_id: &str,
        filename: &str,
        revision: &str,
        sha256: &str,
        blake3_hash: &Blake3Hash,
        branch: Option<&str>,
        repo_type: RepoType,
        commit_sha: Option<&str>,
    ) -> Result<PathBuf> {
        // Validate the commit SHA *before* touching the filesystem, so a
        // rejected branch name never leaves a half-built cache entry.
        let commit = match commit_sha {
            Some(sha) if is_commit_sha(sha) => Some(sha.to_ascii_lowercase()),
            Some(sha) => anyhow::bail!(
                "refusing to write refs: '{}' is not a 40-hex commit SHA \
                 (huggingface_hub requires a commit SHA, not a branch name)",
                sha
            ),
            None if is_commit_sha(revision) => Some(revision.to_ascii_lowercase()),
            None => {
                tracing::warn!(
                    "no resolved commit SHA for {}/{} revision '{}'; skipping refs/{} — \
                     branch resolution will not work in huggingface_hub",
                    repo_id,
                    filename,
                    revision,
                    branch.unwrap_or("main")
                );
                None
            }
        };

        // 1. Ensure directories exist (repo_type-aware)
        let blobs_dir = self.blobs_dir_with_type(repo_id, repo_type);
        let snapshot_dir = self.snapshot_dir_with_type(repo_id, revision, repo_type);
        let refs_dir = self.refs_dir_with_type(repo_id, repo_type);

        // Handle subdirectories in filename (e.g. "text_encoder/model.safetensors")
        let file_path_in_snapshot = snapshot_dir.join(filename);
        let parent_dir = file_path_in_snapshot.parent().unwrap_or(&snapshot_dir);

        fs::create_dir_all(&blobs_dir).context("Failed to create blobs directory")?;
        fs::create_dir_all(parent_dir).context("Failed to create snapshot subdirectory")?;
        fs::create_dir_all(&refs_dir)
            .context("Failed to write refs: failed to create refs directory")?;

        // 2. blob → CAS: relative symlink from blobs/{sha} to cas/blake3/{p}/{hash}.
        // The blob lives at <store>/hf_cache/hub/<model-dir>/blobs/{sha}; CAS is
        // <store>/cas/blake3/{p}/{hash}. Depth: blobs → model-dir → hub → hf_cache → store.
        let blob_path = blobs_dir.join(sha256);
        if !blob_path.exists() || fs::symlink_metadata(&blob_path).is_err() {
            let cas_path = self.cas.get(blake3_hash).ok_or_else(|| {
                anyhow::anyhow!("CAS object not found for blake3 hash: {}", blake3_hash.as_hex())
            })?;
            let cas_rel = cas_path
                .strip_prefix(self.root.parent().context("store root unavailable")?)
                .context("CAS path is not inside the store")?;
            let up = "../../../../"; // blobs → model-dir → hub → hf_cache → store
            let target = Self::relative_target_for_host(&format!(
                "{}{}",
                up,
                cas_rel.to_string_lossy().replace('\\', "/")
            ));
            if blob_path.exists() {
                fs::remove_file(&blob_path)?;
            }
            Self::create_relative_symlink(&blob_path, &target).with_context(|| {
                format!(
                    "Failed to create blob symlink {} -> {} (symlinks required for HF cache compatibility)",
                    blob_path.display(), target
                )
            })?;
        }

        // 3. snapshot/{revision}/{filename} → ../../../blobs/{sha256} (relative).
        // Depth depends on subfolder nesting inside the snapshot.
        if !file_path_in_snapshot.exists() || fs::symlink_metadata(&file_path_in_snapshot).is_err()
        {
            let depth = 2 + filename.matches('/').count(); // rev + snapshots, plus subdirs
            let relative_prefix = "../".repeat(depth);
            let relative_blob =
                Self::relative_target_for_host(&format!("{}blobs/{}", relative_prefix, sha256));
            if file_path_in_snapshot.exists() {
                fs::remove_file(&file_path_in_snapshot)?;
            }
            Self::create_relative_symlink(&file_path_in_snapshot, &relative_blob)
                .with_context(|| format!("Failed to create snapshot symlink for {}", filename))?;
        }

        // 4. refs/{branch} = resolved commit SHA (validated before any FS write).
        if let Some(sha) = commit {
            let branch_name = branch.unwrap_or("main");
            let ref_file = refs_dir.join(branch_name);
            let existing = fs::read_to_string(&ref_file).unwrap_or_default();
            if existing.trim() != sha {
                fs::write(&ref_file, &sha)
                    .with_context(|| format!("Failed to write refs file {}", ref_file.display()))?;
            }
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
            let cas_rel = cas_path
                .strip_prefix(self.root.parent().context("store root unavailable")?)
                .context("CAS path is not inside the store")?;
            let target = Self::relative_target_for_host(&format!(
                "../../../../{}",
                cas_rel.to_string_lossy().replace('\\', "/")
            ));
            Self::create_relative_symlink(&blob_path, &target)?;
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

/// True when `s` is a 40-character hex git commit SHA.
fn is_commit_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
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

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Detect whether this platform/account can create symlinks at all.
    /// Windows without Developer Mode / admin returns ERROR_PRIVILEGE_NOT_HELD.
    #[cfg(windows)]
    fn symlinks_supported(dir: &TempDir) -> bool {
        let target = dir.path().join("symlink-target-test");
        let link = dir.path().join("symlink-link-test");
        std::fs::write(&target, b"x").unwrap();
        std::os::windows::fs::symlink_file(&target, &link).is_ok()
    }

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
        let commit = "0123456789abcdef0123456789abcdef01234567";

        // B5: symlinks are mandatory.  On Windows without Developer Mode the
        // call must FAIL (never silently copy) — assert that exact behaviour.
        let result = cache.create_cache_entry_with_type(
            "stabilityai/sdxl-base",
            "model.safetensors",
            commit,
            &sha256,
            &hash,
            Some("main"),
            RepoType::Model,
            Some(commit),
        );

        #[cfg(windows)]
        if !symlinks_supported(&tmp) {
            let err = result.expect_err("symlink creation must fail without Developer Mode");
            assert!(
                format!("{:#}", err).contains("symlink")
                    || format!("{:#}", err).contains("privilege"),
                "error must explain the symlink requirement: {:#}",
                err
            );
            let blob_path = cache.blob_path("stabilityai/sdxl-base", &sha256);
            assert!(!blob_path.exists(), "no silent copy may be left behind");
            return;
        }

        let file_path = result.unwrap();

        // Verify structure
        assert!(file_path.exists(), "Snapshot file should exist");

        let blob_path = cache.blob_path("stabilityai/sdxl-base", &sha256);
        assert!(blob_path.exists(), "Blob should exist");

        // B9 regression: on Windows a forward-slash relative symlink target
        // is created but dangles. Both links must resolve to the CAS object.
        #[cfg(windows)]
        {
            let snap_canon = std::fs::canonicalize(&file_path)
                .expect("snapshot symlink must resolve on Windows");
            let cas_canon =
                std::fs::canonicalize(cas.path_for_hash(&hash)).expect("CAS object must exist");
            assert_eq!(snap_canon, cas_canon, "snapshot must resolve to CAS object");
        }

        // B5: snapshot and blob links must be relative symlinks.
        #[cfg(unix)]
        {
            let snap_link = std::fs::read_link(&file_path).expect("snapshot must be a symlink");
            let snap_str = snap_link.to_string_lossy();
            assert!(!snap_str.starts_with('/'), "snapshot link must be relative: {}", snap_str);
            assert!(snap_str.contains("blobs/"), "{}", snap_str);
            let blob_link = std::fs::read_link(&blob_path).expect("blob must be a symlink");
            assert!(!blob_link.to_string_lossy().starts_with('/'), "blob link must be relative");
            // The relative snapshot link must resolve to the blob.
            let resolved = file_path.parent().unwrap().join(&snap_link);
            assert_eq!(
                std::fs::canonicalize(resolved).unwrap(),
                std::fs::canonicalize(&blob_path).unwrap()
            );
        }

        let refs_file = cache.refs_dir("stabilityai/sdxl-base").join("main");
        assert!(refs_file.exists(), "refs/main should exist");

        let rev_in_ref = std::fs::read_to_string(&refs_file).unwrap();
        assert_eq!(rev_in_ref.trim(), commit, "refs/main must be a commit SHA, not a branch name");

        // Verify cache hit (HF resolves branches to commit SHA first).
        assert!(cache.check_cache("stabilityai/sdxl-base", commit, "model.safetensors"));
        assert!(!cache.check_cache("stabilityai/sdxl-base", "main", "model.safetensors"));

        // Verify repos list
        let repos = cache.list_repos().unwrap();
        assert!(repos.iter().any(|r| r.contains("stabilityai")));
    }

    #[test]
    fn test_refuses_to_write_branch_name_as_commit() {
        use crate::cas::CasStore;
        use tempfile::NamedTempFile;

        let tmp = TempDir::new().unwrap();
        let store_path = tmp.path();
        let cas = CasStore::new(store_path);
        cas.init().unwrap();
        let cache = HfCache::new(store_path);
        cache.init().unwrap();

        let model_file = NamedTempFile::new().unwrap();
        std::fs::write(model_file.path(), b"content").unwrap();
        let hash = crate::hash::hash_file(model_file.path()).unwrap();
        cas.store(model_file.path(), &hash).unwrap();

        let result = cache.create_cache_entry_with_type(
            "org/repo",
            "m.bin",
            "main",
            &"b".repeat(64),
            &hash,
            Some("main"),
            RepoType::Model,
            Some("main"),
        );
        assert!(result.is_err(), "branch name must be rejected as commit SHA");
        assert!(format!("{:#}", result.unwrap_err()).contains("not a 40-hex commit SHA"));
    }

    #[test]
    fn test_repo_type_prefixes() {
        assert_eq!(
            HfCache::repo_to_dir_name_with_type("org/data", RepoType::Dataset),
            "datasets--org--data"
        );
        assert_eq!(
            HfCache::repo_to_dir_name_with_type("org/space", RepoType::Space),
            "spaces--org--space"
        );
        assert_eq!(RepoType::parse("dataset").unwrap(), RepoType::Dataset);
        assert_eq!(RepoType::parse("MODELS").unwrap(), RepoType::Model);
        assert!(RepoType::parse("unknown").is_err());
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
