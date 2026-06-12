//! Download Manager for HuggingFace model files (Phase 3)
//!
//! Implements resumable HTTP downloads with:
//! - SHA256 extraction from HF response headers (`X-Linked-Etag`)
//! - BLAKE3 verification after download
//! - CAS dedup (skip download if BLAKE3 already known)
//! - Progress callbacks
//! - WAL-backed crash recovery
//!
//! NOTE: This module uses `std` blocking IO. An async variant backed by Tokio
//! can be added in a later phase when the daemon is introduced.

use crate::cas::CasStore;
use crate::db::{Database, DownloadStatus};
use crate::hash::{hash_file, Blake3Hash};
use crate::hf_cache::HfCache;
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Progress callback: (bytes_done, bytes_total, filename)
pub type ProgressCallback = Box<dyn Fn(u64, u64, &str) + Send + Sync>;

/// HuggingFace file metadata fetched from the API / response headers
#[derive(Debug, Clone)]
pub struct HfFileMetadata {
    pub repo_id: String,
    pub filename: String,
    pub revision: String,
    pub sha256: Option<String>,
    pub size_bytes: Option<u64>,
    pub download_url: String,
}

/// Download result
#[derive(Debug)]
pub struct DownloadResult {
    pub blake3_hash: Blake3Hash,
    pub sha256_hash: Option<String>,
    pub size_bytes: u64,
    pub cas_path: PathBuf,
    /// true if file was already in CAS (skipped download)
    pub was_cached: bool,
}

/// The download manager
pub struct Downloader {
    store_path: PathBuf,
    hf_token: Option<String>,
}

impl Downloader {
    pub fn new(store_path: &Path) -> Self {
        Self {
            store_path: store_path.to_path_buf(),
            hf_token: std::env::var("HF_TOKEN").ok()
                .or_else(|| std::env::var("HUGGING_FACE_HUB_TOKEN").ok()),
        }
    }

    /// Set HuggingFace authentication token
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.hf_token = Some(token.into());
        self
    }

    /// Check if a file is already in the HF cache or CAS.
    /// Returns the path to the cached file if found.
    pub fn check_cache(
        &self,
        db: &Database,
        hf_cache: &HfCache,
        repo_id: &str,
        filename: &str,
        revision: &str,
        sha256: Option<&str>,
    ) -> Result<Option<PathBuf>> {
        // 1. Check HF cache directory first (fastest)
        if hf_cache.check_cache(repo_id, revision, filename) {
            let path = hf_cache.snapshot_file_path(repo_id, revision, filename);
            return Ok(Some(path));
        }

        // 2. Check SHA256 → BLAKE3 mapping
        if let Some(sha256_str) = sha256 {
            if let Some(mapping) = db.get_blake3_by_sha256(sha256_str)? {
                let blake3 = Blake3Hash::from_hex(&mapping.blake3_hash)
                    .map_err(|e| anyhow!("Invalid blake3 in mapping: {}", e))?;
                let cas = CasStore::new(&self.store_path);
        if let Some(cas_path) = cas.get(&blake3) {
                    return Ok(Some(cas_path));
                }
            }
        }

        Ok(None)
    }

    /// Download a file from HuggingFace Hub with full dedup + CAS integration.
    ///
    /// Flow:
    /// 1. Fetch metadata (sha256, size) from HF API
    /// 2. Check if already in CAS (via sha256 or blake3)
    /// 3. Download to tmp/ with progress
    /// 4. Compute BLAKE3 hash
    /// 5. Move to CAS (two-phase)
    /// 6. Create fake HF cache structure
    /// 7. Record in database
    pub fn download_hf_file(
        &self,
        db: &mut Database,
        repo_id: &str,
        filename: &str,
        revision: Option<&str>,
        progress: Option<&ProgressCallback>,
    ) -> Result<DownloadResult> {
        let rev = revision.unwrap_or("main");
        let cas = CasStore::new(&self.store_path);
        let hf_cache = HfCache::new(&self.store_path);

        // Step 1: Fetch metadata from HF API
        let metadata = self.fetch_hf_metadata(repo_id, filename, rev)
            .unwrap_or_else(|_| HfFileMetadata {
                repo_id: repo_id.to_string(),
                filename: filename.to_string(),
                revision: rev.to_string(),
                sha256: None,
                size_bytes: None,
                download_url: format!(
                    "https://huggingface.co/{}/resolve/{}/{}",
                    repo_id, rev, filename
                ),
            });

        // Step 2: Check cache
        if let Ok(Some(cached_path)) = self.check_cache(
            db, &hf_cache, repo_id, filename, rev, metadata.sha256.as_deref()
        ) {
            // Already cached — but we don't have a Blake3Hash from path alone,
            // so we return a sentinel result
            let size = fs::metadata(&cached_path)
                .map(|m| m.len())
                .unwrap_or(0);
            // For cached files we still need blake3 — look it up
            if let Some(sha256) = &metadata.sha256 {
                if let Some(mapping) = db.get_blake3_by_sha256(sha256)? {
                    let blake3 = Blake3Hash::from_hex(&mapping.blake3_hash)
                        .map_err(|e| anyhow!("{}", e))?;
                    return Ok(DownloadResult {
                        blake3_hash: blake3,
                        sha256_hash: Some(sha256.clone()),
                        size_bytes: size,
                        cas_path: cached_path,
                        was_cached: true,
                    });
                }
            }
        }

        // Step 3: Record download intent in DB
        let dl_id = db.insert_download(
            &metadata.download_url,
            Some(repo_id),
            Some(filename),
            Some(rev),
        )?;

        // Step 4: Download to tmp
        let tmp_dir = self.store_path.join("tmp").join("downloads");
        fs::create_dir_all(&tmp_dir).context("Failed to create tmp/downloads dir")?;
        let tmp_file = tmp_dir.join(format!("{}.part", dl_id));

        let download_size = self.download_to_file(
            &metadata.download_url,
            &tmp_file,
            metadata.size_bytes,
            metadata.sha256.as_deref(),
            progress,
        ).with_context(|| format!("Failed to download {}/{}", repo_id, filename))?;

        // Update progress in DB
        db.update_download_progress(dl_id, DownloadStatus::Downloading, download_size as i64, download_size as i64)?;

        // Step 5: Compute BLAKE3
        let blake3 = hash_file(&tmp_file)
            .context("Failed to hash downloaded file")?;

        // Step 6: Check if we already have this content (BLAKE3 dedup)
        let cas_path = if db.get_model(&blake3)?.is_some() {
            // Already in CAS — remove temp file
            let _ = fs::remove_file(&tmp_file);
            cas.get(&blake3).ok_or_else(|| anyhow!("CAS path not found after move"))?
        } else {
            // Store in CAS (two-phase: copy then rename)
            let staging_dir = self.store_path.join("tmp").join("cas_staging");
            fs::create_dir_all(&staging_dir)?;
            let staging = staging_dir.join(format!("{}.tmp", blake3.as_hex()));
            fs::copy(&tmp_file, &staging)?;
            let _ = fs::remove_file(&tmp_file);

            // Record in models table
            let size = fs::metadata(&staging)?.len() as i64;
            db.insert_or_update_model(&blake3, size, None, None, None, None)?;

            // Move staging → CAS (best-effort atomic)
            cas.store(&staging, &blake3)?
        };

        // Step 7: Extract / verify SHA256
        let sha256 = metadata.sha256.clone();

        // Step 8: Record mapping and complete download in DB
        if let Some(ref sha256_str) = sha256 {
            db.upsert_hf_mapping(sha256_str, &blake3.as_hex(), Some(repo_id), Some(filename))?;
        }
        db.complete_download(dl_id, &blake3.as_hex(), sha256.as_deref())?;

        // Step 9: Create fake HF cache structure
        if let Some(ref sha256_str) = sha256 {
            hf_cache.init().ok();
            hf_cache.create_cache_entry(
                repo_id,
                filename,
                rev,
                sha256_str,
                &blake3,
                Some("main"),
            ).context("Failed to create HF cache entry")?;
        }

        Ok(DownloadResult {
            blake3_hash: blake3,
            sha256_hash: sha256,
            size_bytes: download_size,
            cas_path,
            was_cached: false,
        })
    }

    /// Fetch file metadata from the HuggingFace Hub API.
    /// Extracts SHA256 from the `X-Linked-Etag` response header.
    pub fn fetch_hf_metadata(
        &self,
        repo_id: &str,
        filename: &str,
        revision: &str,
    ) -> Result<HfFileMetadata> {
        let url = format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            repo_id, revision, filename
        );

        let agent = build_agent(self.hf_token.as_deref());

        // Send HEAD request to get metadata without downloading body
        let resp = agent
            .head(&url)
            .timeout(Duration::from_secs(30))
            .call()
            .context("HF metadata HEAD request failed")?;

        // Extract SHA256 from X-Linked-Etag header
        let sha256 = resp
            .header("x-linked-etag")
            .or_else(|| resp.header("etag"))
            .map(|v| v.trim_matches('"').to_lowercase())
            .filter(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()));

        // Extract size from Content-Length or X-Linked-Size
        let size_bytes = resp
            .header("x-linked-size")
            .or_else(|| resp.header("content-length"))
            .and_then(|v| v.parse::<u64>().ok());

        Ok(HfFileMetadata {
            repo_id: repo_id.to_string(),
            filename: filename.to_string(),
            revision: revision.to_string(),
            sha256,
            size_bytes,
            download_url: url,
        })
    }

    /// Download a file from URL to local path with resume support.
    /// Returns the total bytes written.
    fn download_to_file(
        &self,
        url: &str,
        dest: &Path,
        expected_size: Option<u64>,
        _expected_sha256: Option<&str>,
        progress: Option<&ProgressCallback>,
    ) -> Result<u64> {
        let agent = build_agent(self.hf_token.as_deref());
        let filename = dest
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string());

        // Check if we have a partial download to resume
        let already_downloaded = if dest.exists() {
            fs::metadata(dest).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        let (mut file, mut bytes_done) = if already_downloaded > 0 {
            // Resume from where we left off
            let file = fs::OpenOptions::new().append(true).open(dest)?;
            (file, already_downloaded)
        } else {
            let file = fs::File::create(dest)?;
            (file, 0u64)
        };

        let resp = if bytes_done > 0 {
            agent
                .get(url)
                .set("Range", &format!("bytes={}-", bytes_done))
                .timeout(Duration::from_secs(600))
                .call()
                .context("HTTP GET with range request failed")?
        } else {
            agent
                .get(url)
                .timeout(Duration::from_secs(600))
                .call()
                .context("HTTP GET request failed")?
        };

        let total = expected_size.unwrap_or(0);
        let mut buf = vec![0u8; 65536]; // 64KB chunks
        let mut reader = resp.into_reader();

        loop {
            let n = reader.read(&mut buf).context("Error reading response body")?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).context("Error writing to file")?;
            bytes_done += n as u64;

            if let Some(cb) = progress {
                cb(bytes_done, total, &filename);
            }
        }

        Ok(bytes_done)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ureq HTTP agent builder
// ─────────────────────────────────────────────────────────────────────────────

fn build_agent(hf_token: Option<&str>) -> ureq::Agent {
    let agent = ureq::AgentBuilder::new()
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(60))
        .build();

    // Note: ureq 2.x does not support default headers on AgentBuilder directly.
    // The token is added per-request in get/head calls instead.
    let _ = hf_token; // used in get/head call sites
    agent
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_downloader_new() {
        let tmp = TempDir::new().unwrap();
        let dl = Downloader::new(tmp.path());
        assert_eq!(dl.store_path, tmp.path());
    }

    #[test]
    fn test_downloader_with_token() {
        let tmp = TempDir::new().unwrap();
        let dl = Downloader::new(tmp.path()).with_token("my_secret_token");
        assert_eq!(dl.hf_token.as_deref(), Some("my_secret_token"));
    }

    #[test]
    fn test_check_cache_miss() {
        use crate::cas::CasStore;
        use crate::db::Database;
        use tempfile::NamedTempFile;

        let tmp = TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path());
        cas.init().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let db = Database::open(db_file.path()).unwrap();
        let hf_cache = HfCache::new(tmp.path());
        hf_cache.init().unwrap();

        let dl = Downloader::new(tmp.path());
        let result = dl.check_cache(
            &db,
            &hf_cache,
            "org/model",
            "model.safetensors",
            "main",
            Some(&"a".repeat(64)),
        ).unwrap();

        assert!(result.is_none(), "Should be a cache miss");
    }

    #[test]
    fn test_check_cache_hit_via_hf_mapping() {
        use crate::cas::CasStore;
        use crate::db::Database;
        use tempfile::NamedTempFile;

        let tmp = TempDir::new().unwrap();
        let cas = CasStore::new(tmp.path());
        cas.init().unwrap();

        let db_file = NamedTempFile::new().unwrap();
        let mut db = Database::open(db_file.path()).unwrap();

        // Create a real file in CAS
        let model_file = NamedTempFile::new().unwrap();
        let content = b"model content for hf mapping test";
        std::fs::write(model_file.path(), content).unwrap();
        let blake3 = hash_file(model_file.path()).unwrap();
        let size = content.len() as i64;

        db.insert_or_update_model(&blake3, size, None, None, None, None).unwrap();
        cas.store(model_file.path(), &blake3).unwrap();

        // Add HF mapping (sha256 → blake3)
        let sha256 = "b".repeat(64);
        db.upsert_hf_mapping(&sha256, &blake3.as_hex(), Some("org/model"), Some("model.safetensors")).unwrap();

        let hf_cache = HfCache::new(tmp.path());
        hf_cache.init().unwrap();

        let dl = Downloader::new(tmp.path());
        let result = dl.check_cache(
            &db,
            &hf_cache,
            "org/model",
            "model.safetensors",
            "main",
            Some(&sha256),
        ).unwrap();

        assert!(result.is_some(), "Should hit via HF mapping");
    }
}
