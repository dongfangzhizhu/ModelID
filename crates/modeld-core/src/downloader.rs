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
//!
//! ## v7.1 additions
//! - `Provenance` struct records download origin metadata.
//! - `DownloadResult.provenance` carries provenance for each successful download.
//! - Token helpers: `token_set`, `token_get`, `token_remove`, `token_status`
//!   store the HF token securely in `<store>/hf_token` (Unix: mode 0o600).
//! - `download_hf_file` warns when a `--token` argument is used instead of the
//!   `HF_TOKEN` environment variable.
//! - Gated/private repo 403 errors show a friendly prompt to set a token.
//! - Token values are never written to any log output.

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

/// Provenance records the origin of a downloaded model file.
///
/// All fields are optional because not every download channel provides them.
#[derive(Debug, Clone)]
pub struct Provenance {
    /// Channel: `"hf"` | `"url"` | `"local"` | `"imported"`
    pub source_type: String,
    /// HuggingFace repository ID, e.g. `"stabilityai/stable-diffusion-xl-base-1.0"`
    pub hf_repo_id: Option<String>,
    /// HuggingFace revision (branch / tag / commit), e.g. `"main"`
    pub revision: Option<String>,
    /// Full download URL
    pub download_url: Option<String>,
    /// License identifier from the model card (if available)
    pub license: Option<String>,
    /// UTC timestamp when the download finished
    pub downloaded_at: chrono::DateTime<chrono::Utc>,
    /// Original filename in the HF repository
    pub original_filename: Option<String>,
    /// URL to the model card page
    pub model_card_url: Option<String>,
}

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
    /// Provenance metadata captured at download time.
    pub provenance: Option<Provenance>,
}

const DEFAULT_HF_BASE: &str = "https://huggingface.co";

pub struct Downloader {
    store_path: PathBuf,
    hf_token: Option<String>,
    hf_base_url: String,
    /// True when the token was supplied via `.with_token()` (CLI `--token` flag).
    /// Used to emit the "use HF_TOKEN env var instead" warning.
    token_from_cli: bool,
}

impl Downloader {
    pub fn new(store_path: &Path) -> Self {
        Self {
            store_path: store_path.to_path_buf(),
            hf_token: std::env::var("HF_TOKEN")
                .ok()
                .or_else(|| std::env::var("HUGGING_FACE_HUB_TOKEN").ok()),
            hf_base_url: DEFAULT_HF_BASE.to_string(),
            token_from_cli: false,
        }
    }

    /// Supply a HuggingFace access token programmatically (e.g. from `--token` CLI flag).
    ///
    /// When this path is used, `download_hf_file` will print a warning recommending
    /// the `HF_TOKEN` environment variable instead.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.hf_token = Some(token.into());
        self.token_from_cli = true;
        self
    }

    pub fn with_hf_base_url(mut self, base: impl Into<String>) -> Self {
        let mut b = base.into();
        while b.ends_with('/') {
            b.pop();
        }
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
    ///
    /// If the downloader was configured via `.with_token()` (i.e. the token came
    /// from a CLI `--token` flag), a warning is printed recommending the
    /// `HF_TOKEN` environment variable instead.
    pub fn download_hf_file(
        &self,
        db: &mut Database,
        repo_id: &str,
        filename: &str,
        revision: Option<&str>,
        progress: Option<&ProgressCallback>,
    ) -> Result<DownloadResult> {
        // Warn when a CLI-supplied token is used instead of the env var.
        // Token value is never printed.
        if self.token_from_cli {
            eprintln!(
                "warning: --token flag detected. For security, prefer setting the \
                 HF_TOKEN environment variable instead of passing the token on the \
                 command line (it may appear in shell history and process listings)."
            );
        }

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
                    let blake3 =
                        Blake3Hash::from_hex(&mapping.blake3_hash).map_err(|e| anyhow!("{}", e))?;
                    return Ok(DownloadResult {
                        blake3_hash: blake3,
                        sha256_hash: Some(sha256.clone()),
                        size_bytes: size,
                        cas_path: cached_path,
                        was_cached: true,
                        provenance: Some(Provenance {
                            source_type: "hf".to_string(),
                            hf_repo_id: Some(repo_id.to_string()),
                            revision: Some(rev.to_string()),
                            download_url: Some(metadata.download_url.clone()),
                            license: None,
                            downloaded_at: chrono::Utc::now(),
                            original_filename: Some(filename.to_string()),
                            model_card_url: Some(format!("{}/{}", self.hf_base_url, repo_id)),
                        }),
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
        let snapshot_str =
            hf_cache.snapshot_file_path(repo_id, filename, rev).to_string_lossy().to_string();
        if db.get_alias_by_path(&snapshot_str)?.is_none() {
            db.insert_alias(&blake3, &snapshot_str, Frontend::HfCache, AliasType::Symlink)?;
        }

        // Build provenance
        let provenance = Some(Provenance {
            source_type: "hf".to_string(),
            hf_repo_id: Some(repo_id.to_string()),
            revision: Some(rev.to_string()),
            download_url: Some(metadata.download_url.clone()),
            license: None, // fetched from model card in future enhancement
            downloaded_at: chrono::Utc::now(),
            original_filename: Some(filename.to_string()),
            model_card_url: Some(format!("{}/{}", self.hf_base_url, repo_id)),
        });

        Ok(DownloadResult {
            blake3_hash: blake3,
            sha256_hash: sha256,
            size_bytes: download_size,
            cas_path,
            was_cached: false,
            provenance,
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
            let r = req.call().map_err(|e| friendly_http_error(e, url))?;

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
            let r = req.call().map_err(|e| friendly_http_error(e, url))?;
            let f = fs::File::create(dest).context("Failed to create download file")?;
            (f, r, 0u64)
        };

        let total = expected_size.unwrap_or(0);
        let mut buf = vec![0u8; 65536]; // 64 KB chunks
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

        drop(file); // flush + close before SHA256 read

        // Verify SHA256 integrity when the expected hash is known
        if let Some(expected) = expected_sha256 {
            verify_sha256(dest, expected)
                .with_context(|| format!("SHA256 integrity check failed for {}", dest.display()))?;
        }

        Ok(bytes_done)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Token helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Path to the on-disk token file.
fn token_file_path(store: &Path) -> PathBuf {
    store.join("hf_token")
}

/// Store a HuggingFace access token to `<store>/hf_token`.
///
/// On Unix the file is created with mode `0o600` (owner read/write only).
/// On Windows a plain file is written (no OS-level ACLs are set by this helper).
///
/// The token value is never logged or printed.
pub fn token_set(store: &Path, token: &str) -> Result<()> {
    let path = token_file_path(store);
    // Write to a temp file, then rename atomically
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, token.as_bytes())
        .with_context(|| format!("Cannot write token to {}", tmp.display()))?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        fs::set_permissions(&tmp, perms)
            .with_context(|| format!("Cannot set permissions on {}", tmp.display()))?;
    }

    fs::rename(&tmp, &path)
        .with_context(|| format!("Cannot rename token file to {}", path.display()))?;

    Ok(())
}

/// Read the stored HuggingFace token from `<store>/hf_token`.
///
/// Returns `Ok(None)` when no token file exists.
pub fn token_get(store: &Path) -> Result<Option<String>> {
    let path = token_file_path(store);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("Cannot read token from {}", path.display()))?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(trimmed))
}

/// Delete the stored HuggingFace token file.  No-op when no file exists.
pub fn token_remove(store: &Path) -> Result<()> {
    let path = token_file_path(store);
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("Cannot delete token file {}", path.display()))?;
    }
    Ok(())
}

/// Return a human-readable token status string.
///
/// Returns one of:
/// - `"set (file)"` — token is stored in `<store>/hf_token`
/// - `"not set"` — no token file exists
pub fn token_status(store: &Path) -> Result<String> {
    match token_get(store)? {
        Some(_) => Ok("set (file)".to_string()),
        None => Ok("not set".to_string()),
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
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let got = format!("{:x}", hasher.finalize());
    if got != expected.to_lowercase() {
        return Err(anyhow!("SHA256 mismatch: expected {}, got {}", expected, got));
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

/// Convert a ureq HTTP error into a user-friendly `anyhow::Error`.
///
/// 403 responses get a special message prompting the user to set a token.
fn friendly_http_error(err: ureq::Error, url: &str) -> anyhow::Error {
    match err {
        ureq::Error::Status(403, _) => anyhow!(
            "Access denied (HTTP 403) for {}.\n\
             This repository may be gated or private. To access it:\n\
             1. Accept the model license on huggingface.co\n\
             2. Set HF_TOKEN=<your_token> in your environment, or run:\n\
             \x20\x20 modeld hf token set",
            url
        ),
        ureq::Error::Status(code, resp) => {
            anyhow!("HTTP {} error for {}: {}", code, url, resp.status_text())
        }
        other => anyhow!("HTTP request failed for {}: {}", url, other),
    }
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
        verify_sha256(f.path(), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
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
