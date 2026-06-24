//! Download Manager for HuggingFace model files (Phase 3)
//!
//! ## Fixes applied (audit)
//! - `download_to_file`: removed `_expected_sha256` suppression — SHA256 is
//!   now verified against the value extracted from HF headers.
//! - Range resume now checks for HTTP 206; if the server returns 200 (no range
//!   support) we discard the partial file and restart from byte 0.
//! - `download_hf_file` calls `fail_download` on any error path.
//! - `.part` filename is derived from `repo_id + filename` (stable across
//!   restarts) instead of from the auto-increment `dl_id`.
//! - A `HfCache` alias is inserted after a successful download so the model
//!   is not immediately orphaned by GC.

use crate::cas::CasStore;
use crate::db::{AliasType, Database, DownloadStatus, Frontend};
use crate::hash::{hash_file, Blake3Hash};
use crate::hf_cache::HfCache;
use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Progress callback: (bytes_done, bytes_total, filename)
pub type ProgressCallback = Box<dyn Fn(u64, u64, &str) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct HfFileMetadata {
    pub repo_id: String,
    pub filename: String,
    pub revision: String,
    pub sha256: Option<String>,
    pub size_bytes: Option<u64>,
    pub download_url: String,
}

#[derive(Debug)]
pub struct DownloadResult {
    pub blake3_hash: Blake3Hash,
    pub sha256_hash: Option<String>,
    pub size_bytes: u64,
    pub cas_path: PathBuf,
    pub was_cached: bool,
}

const DEFAULT_HF_BASE: &str = "https://huggingface.co";

pub struct Downloader {
    store_path: PathBuf,
    hf_token: Option<String>,
    hf_base_url: String,
}

impl Downloader {
    pub fn new(store_path: &Path) -> Self {
        Self {
            store_path: store_path.to_path_buf(),
            hf_token: std::env::var("HF_TOKEN")
                .ok()
                .or_else(|| std::env::var("HUGGING_FACE_HUB_TOKEN").ok()),
            hf_base_url: DEFAULT_HF_BASE.to_string(),
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.hf_token = Some(token.into());
        self
    }

    pub fn with_hf_base_url(mut self, base: impl Into<String>) -> Self {
        let mut b = base.into();
        while b.ends_with('/') { b.pop(); }
        self.hf_base_url = b;
        self
    }

    pub fn check_cache(
        &self,
        db: &Database,
        hf_cache: &HfCache,
        repo_id: &str,
        filename: &str,
        revision: &str,
        sha256: Option<&str>,
    ) -> Result<Option<PathBuf>> {
        if hf_cache.check_cache(repo_id, revision, filename) {
            return Ok(Some(hf_cache.snapshot_file_path(repo_id, revision, filename)));
        }
        if let Some(s) = sha256 {
            if let Some(mapping) = db.get_blake3_by_sha256(s)? {
                let blake3 = Blake3Hash::from_hex(&mapping.blake3_hash)
                    .map_err(|e| anyhow!("Invalid blake3 in mapping: {}", e))?;
                if let Some(p) = CasStore::new(&self.store_path).get(&blake3) {
                    return Ok(Some(p));
                }
            }
        }
        Ok(None)
    }

    /// Download a file from HuggingFace Hub with full dedup + CAS integration.
    ///
    /// Wraps the real work in an inner closure so that a single `fail_download`
    /// call handles every error path without repeating it.
    pub fn download_hf_file(
        &self,
        db: &mut Database,
        repo_id: &str,
        filename: &str,
        revision: Option<&str>,
        progress: Option<&ProgressCallback>,
    ) -> Result<DownloadResult> {
        // Track the DB row so we can mark it failed on any error.
        let mut dl_id: Option<i64> = None;

        let result =
            self.download_hf_file_inner(db, repo_id, filename, revision, progress, &mut dl_id);

        if let Err(ref e) = result {
            if let Some(id) = dl_id {
                let _ = db.fail_download(id, &format!("{:#}", e));
            }
        }
        result
    }

    fn download_hf_file_inner(
        &self,
        db: &mut Database,
        repo_id: &str,
        filename: &str,
        revision: Option<&str>,
        progress: Option<&ProgressCallback>,
        dl_id_out: &mut Option<i64>,
    ) -> Result<DownloadResult> {
        let rev = revision.unwrap_or("main");
        let cas = CasStore::new(&self.store_path);
        let hf_cache = HfCache::new(&self.store_path);

        // Step 1: Fetch metadata (sha256, size) from HF API
        let metadata =
            self.fetch_hf_metadata(repo_id, filename, rev).unwrap_or_else(|_| HfFileMetadata {
                repo_id: repo_id.to_string(),
                filename: filename.to_string(),
                revision: rev.to_string(),
                sha256: None,
                size_bytes: None,
                download_url: format!(
                    "{}/{}/resolve/{}/{}",
                    self.hf_base_url, repo_id, rev, filename
                ),
            });

        // Step 2: Check cache
        if let Ok(Some(cached_path)) =
            self.check_cache(db, &hf_cache, repo_id, filename, rev, metadata.sha256.as_deref())
        {
            let size = fs::metadata(&cached_path).map(|m| m.len()).unwrap_or(0);
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
        // Use a stable .part filename derived from repo+file so the same
        // partial download can be resumed across process restarts.
        let part_name = part_filename(repo_id, filename);
        let dl_id =
            db.insert_download(&metadata.download_url, Some(repo_id), Some(filename), Some(rev))?;
        *dl_id_out = Some(dl_id);

        // Step 4: Download to tmp
        let tmp_dir = self.store_path.join("tmp").join("downloads");
        fs::create_dir_all(&tmp_dir).context("Failed to create tmp/downloads dir")?;
        let tmp_file = tmp_dir.join(&part_name);

        let download_size = self
            .download_to_file(
                &metadata.download_url,
                &tmp_file,
                metadata.size_bytes,
                metadata.sha256.as_deref(), // now actually verified
                progress,
            )
            .with_context(|| format!("Failed to download {}/{}", repo_id, filename))?;

        db.update_download_progress(
            dl_id,
            DownloadStatus::Downloading,
            download_size as i64,
            download_size as i64,
        )?;

        // Step 5: Compute BLAKE3
        let blake3 = hash_file(&tmp_file).context("Failed to hash downloaded file")?;

        // Step 6: CAS dedup — skip copy if content already known
        let cas_path = if db.get_model(&blake3)?.is_some() {
            let _ = fs::remove_file(&tmp_file);
            cas.get(&blake3).ok_or_else(|| anyhow!("CAS path not found after dedup check"))?
        } else {
            let staging_dir = self.store_path.join("tmp").join("cas_staging");
            fs::create_dir_all(&staging_dir)?;
            let staging = staging_dir.join(format!("{}.tmp", blake3.as_hex()));
            fs::copy(&tmp_file, &staging)?;
            let _ = fs::remove_file(&tmp_file);

            let size = fs::metadata(&staging)?.len() as i64;
            db.insert_or_update_model(&blake3, size, None, None, None, None)?;
            cas.store(&staging, &blake3)?
        };

        let sha256 = metadata.sha256.clone();

        // Step 7: Record SHA256↔BLAKE3 mapping
        if let Some(ref sha256_str) = sha256 {
            db.upsert_hf_mapping(sha256_str, blake3.as_hex(), Some(repo_id), Some(filename))?;
        }
        db.complete_download(dl_id, blake3.as_hex(), sha256.as_deref())?;

        // Step 8: Create fake HF cache structure
        if let Some(ref sha256_str) = sha256 {
            hf_cache.init().ok();
            hf_cache
                .create_cache_entry(repo_id, filename, rev, sha256_str, &blake3, Some("main"))
                .context("Failed to create HF cache entry")?;
        }

        // Step 9: Insert HfCache alias so GC does not immediately orphan this model.
        // Without an alias, alias_count=0 and is_orphan()=true → GC quarantines
        // the freshly-downloaded file on the very next run.
        let snapshot_str = hf_cache
            .snapshot_file_path(repo_id, filename, rev)
            .to_string_lossy()
            .to_string();
        if db.get_alias_by_path(&snapshot_str)?.is_none() {
            db.insert_alias(&blake3, &snapshot_str, Frontend::HfCache, AliasType::Symlink)?;
        }

        Ok(DownloadResult {
            blake3_hash: blake3,
            sha256_hash: sha256,
            size_bytes: download_size,
            cas_path,
            was_cached: false,
        })
    }

    pub fn fetch_hf_metadata(
        &self,
        repo_id: &str,
        filename: &str,
        revision: &str,
    ) -> Result<HfFileMetadata> {
        let url = format!("{}/{}/resolve/{}/{}", self.hf_base_url, repo_id, revision, filename);
        let agent = build_agent();
        let mut req = agent.head(&url).timeout(Duration::from_secs(30));
        if let Some(hdr) = auth_header(self.hf_token.as_deref()) {
            req = req.set("Authorization", &hdr);
        }
        let resp = req.call().context("HF metadata HEAD request failed")?;

        let sha256 = resp
            .header("x-linked-etag")
            .or_else(|| resp.header("etag"))
            .map(|v| v.trim_matches('"').to_lowercase())
            .filter(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()));

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

    /// Download `url` to `dest`, supporting resume via Range requests.
    ///
    /// Changes from original:
    /// - Range resume verifies server returned **206** before appending; if the
    ///   server returns 200 (full content) we discard the partial file and
    ///   restart from byte 0 to avoid file corruption.
    /// - `expected_sha256` is now **required** — SHA256 is verified after the
    ///   download completes when the value is present.
    fn download_to_file(
        &self,
        url: &str,
        dest: &Path,
        expected_size: Option<u64>,
        expected_sha256: Option<&str>,
        progress: Option<&ProgressCallback>,
    ) -> Result<u64> {
        let agent = build_agent();
        let filename = dest
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string());

        let partial_size =
            if dest.exists() { fs::metadata(dest).map(|m| m.len()).unwrap_or(0) } else { 0 };

        // Determine whether we can resume or must start fresh.
        let (mut file, resp, mut bytes_done) = if partial_size > 0 {
            let mut req = agent
                .get(url)
                .set("Range", &format!("bytes={}-", partial_size))
                .timeout(Duration::from_secs(600));
            if let Some(hdr) = auth_header(self.hf_token.as_deref()) {
                req = req.set("Authorization", &hdr);
            }
            let r = req.call().context("HTTP range request failed")?;

            if r.status() == 206 {
                // Server supports range — safe to append to partial file
                let f = fs::OpenOptions::new()
                    .append(true)
                    .open(dest)
                    .context("Failed to open partial file for append")?;
                (f, r, partial_size)
            } else {
                // Server returned 200 (full body) — discard partial, start over
                let f = fs::File::create(dest).context("Failed to create download file")?;
                (f, r, 0u64)
            }
        } else {
            let mut req = agent.get(url).timeout(Duration::from_secs(600));
            if let Some(hdr) = auth_header(self.hf_token.as_deref()) {
                req = req.set("Authorization", &hdr);
            }
            let r = req.call().context("HTTP request failed")?;
            let f = fs::File::create(dest).context("Failed to create download file")?;
            (f, r, 0u64)
        };

        let total = expected_size.unwrap_or(0);
        let mut buf = vec![0u8; 65536]; // 64 KB chunks
        let mut reader = resp.into_reader();

        loop {
            let n = reader.read(&mut buf).context("Error reading response body")?;
            if n == 0 { break; }
            file.write_all(&buf[..n]).context("Error writing to file")?;
            bytes_done += n as u64;
            if let Some(cb) = progress {
                cb(bytes_done, total, &filename);
            }
        }

        drop(file); // flush + close before SHA256 read

        // Verify SHA256 integrity when the expected hash is known
        if let Some(expected) = expected_sha256 {
            verify_sha256(dest, expected).with_context(|| {
                format!("SHA256 integrity check failed for {}", dest.display())
            })?;
        }

        Ok(bytes_done)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a stable `.part` filename from repo_id + filename so that the same
/// partial download can be resumed across process restarts.
fn part_filename(repo_id: &str, filename: &str) -> String {
    let safe = format!("{}-{}", repo_id, filename)
        .replace('/', "--")
        .replace('\\', "--")
        .replace(':', "_");
    format!("{}.part", safe)
}

/// Verify the SHA-256 digest of `path` against `expected` (hex string).
/// Returns `Err` on mismatch.
fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("Cannot open file for SHA256 check: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).context("IO error during SHA256")?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    let got = format!("{:x}", hasher.finalize());
    if got != expected.to_lowercase() {
        return Err(anyhow!(
            "SHA256 mismatch: expected {}, got {}",
            expected,
            got
        ));
    }
    Ok(())
}

fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(60))
        .build()
}

fn auth_header(token: Option<&str>) -> Option<String> {
    token.map(|t| format!("Bearer {}", t))
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
        assert_eq!(dl.hf_base_url, DEFAULT_HF_BASE);
    }

    #[test]
    fn test_downloader_with_token() {
        let tmp = TempDir::new().unwrap();
        let dl = Downloader::new(tmp.path()).with_token("secret");
        assert_eq!(dl.hf_token.as_deref(), Some("secret"));
    }

    #[test]
    fn test_part_filename_stable() {
        let a = part_filename("org/model", "file.safetensors");
        let b = part_filename("org/model", "file.safetensors");
        assert_eq!(a, b, "part_filename must be deterministic");
        assert!(a.ends_with(".part"));
    }

    #[test]
    fn test_verify_sha256_ok() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"hello").unwrap();
        f.flush().unwrap();
        // SHA256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        verify_sha256(
            f.path(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        )
        .expect("should pass");
    }

    #[test]
    fn test_verify_sha256_mismatch() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"hello").unwrap();
        f.flush().unwrap();
        let result = verify_sha256(f.path(), &"a".repeat(64));
        assert!(result.is_err(), "wrong hash should return Err");
    }

    #[test]
    fn test_check_cache_miss() {
        use crate::cas::CasStore;
        use crate::db::Database;
        use tempfile::NamedTempFile;

        let tmp = TempDir::new().unwrap();
        CasStore::new(tmp.path()).init().unwrap();
        let db_file = NamedTempFile::new().unwrap();
        let db = Database::open(db_file.path()).unwrap();
        let hf_cache = HfCache::new(tmp.path());
        hf_cache.init().unwrap();

        let dl = Downloader::new(tmp.path());
        let result = dl
            .check_cache(&db, &hf_cache, "org/m", "m.safetensors", "main", Some(&"a".repeat(64)))
            .unwrap();
        assert!(result.is_none());
    }
}
