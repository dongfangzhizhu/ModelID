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

//! ## B4 additions (downloader hardening)
//! - BLAKE3 + SHA256 are computed **streaming** while bytes arrive; neither
//!   `hash_file` nor `verify_sha256` re-reads the finished file.
//! - `<part>.meta` records URL / ETag / expected size.  A partial file whose
//!   metadata no longer matches is discarded instead of corrupting the target.
//! - Resume sends `If-Range` with the stored ETag; a server that ignores Range
//!   (HTTP 200) restarts from byte 0.
//! - `bytes_done == expected_size` is enforced when the size is known.
//! - Missing metadata is reported via `unverified` in `DownloadResult` and a
//!   warning — never silently treated as verified.
//! - The final DB writes (model / mapping / download / alias / WAL commit) are
//!   wrapped in `Database::in_transaction`, so a crash leaves the WAL row for
//!   `recover_store` instead of a half-imported model.

use crate::cas::CasStore;
use crate::db::{
    TransactionStatus, {AliasType, Database, DownloadStatus, Frontend},
};
use crate::hash::Blake3Hash;
use crate::hf_cache::{HfCache, RepoType};
use anyhow::{anyhow, Context, Result};
use blake3::Hasher as Blake3Hasher;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Progress callback: (bytes_done, bytes_total, filename)
pub type ProgressCallback = Box<dyn Fn(u64, u64, &str) + Send + Sync>;

/// Build the snapshot path used in WAL/alias records for a downloaded file.
///
/// `HfCache::snapshot_file_path_with_type` takes `(repo_id, revision,
/// filename, repo_type)` — revision first. Keeping this wrapper next to the
/// downloader makes the argument order explicit: `create_cache_entry_with_type`
/// below uses the opposite `(repo_id, filename, revision, …)` order.
fn snapshot_path_for_download(
    hf_cache: &HfCache,
    repo_id: &str,
    revision: &str,
    filename: &str,
    repo_type: RepoType,
) -> PathBuf {
    hf_cache.snapshot_file_path_with_type(repo_id, revision, filename, repo_type)
}

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
    /// ETag used for `If-Range` and partial-file validation.
    pub etag: Option<String>,
    /// Resolved 40-hex commit SHA from the final redirect URL, used for
    /// `refs/{branch}` so huggingface_hub can resolve branches correctly.
    pub commit_sha: Option<String>,
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
    /// True when HEAD metadata (size / ETag / SHA256) was unavailable and the
    /// download could not be fully verified.  Never silently claims verified.
    pub unverified: bool,
    /// True when the HF cache layout could not be built (e.g. Windows without
    /// Developer Mode denies symlink creation).  Bytes are still in CAS, but
    /// HF frameworks will not see a transparent cache hit.  Never silent.
    pub hf_cache_degraded: bool,
}

/// Side-car metadata for a resumable `.part` file.
///
/// Stored as `<part>.meta` (JSON).  A partial download is only resumed when
/// URL, ETag and expected size still match; otherwise it is discarded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PartMeta {
    pub url: String,
    /// `x-linked-etag` / `etag` from the HEAD response (quotes stripped).
    pub etag: Option<String>,
    pub expected_size: Option<u64>,
    pub sha256: Option<String>,
}

impl PartMeta {
    fn path_for(part: &Path) -> PathBuf {
        let mut os = part.as_os_str().to_os_string();
        os.push(".meta");
        PathBuf::from(os)
    }

    fn load(part: &Path) -> Result<Option<Self>> {
        let path = Self::path_for(part);
        match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| anyhow!("Corrupt part metadata {}: {}", path.display(), e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
        }
    }

    fn save(&self, part: &Path) -> Result<()> {
        let path = Self::path_for(part);
        let tmp = path.with_extension("meta.tmp");
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&tmp, json).with_context(|| format!("Cannot write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("Cannot rename to {}", path.display()))?;
        Ok(())
    }

    fn remove(part: &Path) {
        let path = Self::path_for(part);
        if let Err(e) = fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("cannot remove part metadata {}: {}", path.display(), e);
            }
        }
    }

    /// Whether `other` describes the same remote object as this metadata.
    fn matches(&self, other: &Self) -> bool {
        self.url == other.url
            && self.etag == other.etag
            && self.expected_size == other.expected_size
    }
}

/// Result of one (possibly resumed) streaming download.
#[derive(Debug)]
struct DownloadOutcome {
    bytes_written: u64,
    blake3: Blake3Hash,
    sha256: Option<String>,
    /// SHA256 was computed but no trusted expected value existed to compare.
    sha256_unverified: bool,
}

const DEFAULT_HF_BASE: &str = "https://huggingface.co";

pub struct Downloader {
    store_path: PathBuf,
    hf_token: Option<String>,
    hf_base_url: String,
    /// True when the token was supplied via `.with_token()` (CLI `--token` flag).
    token_from_cli: bool,
    /// HF repository type — selects the cache directory prefix.
    repo_type: crate::hf_cache::RepoType,
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
            repo_type: crate::hf_cache::RepoType::Model,
        }
    }

    /// Select the HF repository type (model / dataset / space).
    pub fn with_repo_type(mut self, repo_type: crate::hf_cache::RepoType) -> Self {
        self.repo_type = repo_type;
        self
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
        if hf_cache.check_cache_with_type(repo_id, revision, filename, self.repo_type) {
            return Ok(Some(hf_cache.snapshot_file_path_with_type(
                repo_id,
                revision,
                filename,
                self.repo_type,
            )));
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
            tracing::warn!(
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

        // Step 1: Fetch metadata (sha256, size, ETag) from HF API.
        // B4: a failure is no longer silently converted into "no metadata":
        // we warn and mark the download `unverified` in the result.
        let metadata = match self.fetch_hf_metadata(repo_id, filename, rev) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    "HF metadata unavailable for {}/{} ({}); download will be marked unverified \
                     and no size/integrity guarantee can be given",
                    repo_id,
                    filename,
                    e
                );
                HfFileMetadata {
                    repo_id: repo_id.to_string(),
                    filename: filename.to_string(),
                    revision: rev.to_string(),
                    sha256: None,
                    size_bytes: None,
                    etag: None,
                    commit_sha: None,
                    download_url: format!(
                        "{}/{}/resolve/{}/{}",
                        self.hf_base_url, repo_id, rev, filename
                    ),
                }
            }
        };
        let metadata_available = metadata.size_bytes.is_some() || metadata.sha256.is_some();

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
                        unverified: false,
                        hf_cache_degraded: false,
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

        // Step 4: Download to tmp with streaming hashes (B4)
        let tmp_dir = self.store_path.join("tmp").join("downloads");
        fs::create_dir_all(&tmp_dir).context("Failed to create tmp/downloads dir")?;
        let tmp_file = tmp_dir.join(&part_name);

        let part_meta = PartMeta {
            url: metadata.download_url.clone(),
            etag: metadata.etag.clone(),
            expected_size: metadata.size_bytes,
            sha256: metadata.sha256.clone(),
        };
        let outcome = self
            .download_to_file(&tmp_file, &part_meta, progress)
            .with_context(|| format!("Failed to download {}/{}", repo_id, filename))?;

        db.update_download_progress(
            dl_id,
            DownloadStatus::Downloading,
            outcome.bytes_written as i64,
            metadata.size_bytes.map(|s| s as i64).unwrap_or(outcome.bytes_written as i64),
        )?;

        // Step 5: use the streaming hashes; no second pass over the file.
        let blake3 = outcome.blake3;
        let sha256 = metadata.sha256.clone().or(outcome.sha256);
        let unverified = !metadata_available || outcome.sha256_unverified;

        // Record the intent before the bytes become visible. A crash between
        // the CAS write and the rows below leaves this row behind; recovery
        // finishes the import when the object exists and rolls it back when it
        // does not.
        let tx_id = uuid::Uuid::new_v4().to_string();
        let snapshot_path =
            snapshot_path_for_download(&hf_cache, repo_id, rev, filename, self.repo_type)
                .to_string_lossy()
                .to_string();
        db.insert_wal_transaction(
            &tx_id,
            "download",
            TransactionStatus::Pending,
            Some(&snapshot_path),
            Some(blake3.as_hex()),
            Some(&serde_json::json!({ "repo": repo_id, "file": filename }).to_string()),
        )?;

        // Step 6: CAS dedup — skip copy if content already known
        let cas_path = if db.get_model(&blake3)?.is_some() {
            let _ = fs::remove_file(&tmp_file);
            PartMeta::remove(&tmp_file);
            cas.get(&blake3).ok_or_else(|| anyhow!("CAS path not found after dedup check"))?
        } else {
            let staging_dir = self.store_path.join("tmp").join("cas_staging");
            fs::create_dir_all(&staging_dir)?;
            let staging = staging_dir.join(format!("{}.tmp", blake3.as_hex()));
            fs::copy(&tmp_file, &staging)?;
            let _ = fs::remove_file(&tmp_file);
            PartMeta::remove(&tmp_file);

            let size = fs::metadata(&staging)?.len() as i64;
            db.insert_or_update_model(&blake3, size, None, None, None, None)?;
            cas.store(&staging, &blake3)?
        };

        // Step 7–9: all DB state changes in one SQLite transaction.  If any
        // statement fails, the WAL row stays `pending`/`copied` for recovery.
        let snapshot_str =
            snapshot_path_for_download(&hf_cache, repo_id, rev, filename, self.repo_type)
                .to_string_lossy()
                .to_string();
        let sha_for_db = sha256.clone();
        let alias_hash = blake3.clone();
        let snapshot_for_db = snapshot_str.clone();
        db.in_transaction(|tx| {
            if let Some(ref sha256_str) = sha_for_db {
                tx.upsert_hf_mapping(sha256_str, blake3.as_hex(), Some(repo_id), Some(filename))?;
            }
            tx.complete_download_with_verification(
                dl_id,
                blake3.as_hex(),
                sha_for_db.as_deref(),
                !unverified,
            )?;
            if tx.get_alias_by_path(&snapshot_for_db)?.is_none() {
                tx.insert_alias(
                    &alias_hash,
                    &snapshot_for_db,
                    Frontend::HfCache,
                    AliasType::Symlink,
                )?;
            }
            tx.update_wal_status(&tx_id, TransactionStatus::Committed)?;
            Ok(())
        })?;

        // Step 8: create the HF cache layout on disk after the DB commit.
        // A symlink denial (Windows without Developer Mode) is a real degradation:
        // report it loudly and expose it on the result, but do not discard an
        // otherwise successful, CAS-verified download.
        let mut hf_cache_degraded = false;
        if let Some(ref sha256_str) = sha256 {
            hf_cache.init().ok();
            if let Err(e) = hf_cache.create_cache_entry_with_type(
                repo_id,
                filename,
                rev,
                sha256_str,
                &blake3,
                Some("main"),
                self.repo_type,
                metadata.commit_sha.as_deref(),
            ) {
                tracing::warn!(
                    "HF cache layout not created for {}/{} ({}); the model is in CAS but \
                     huggingface_hub will not see a cache hit. On Windows enable Developer Mode \
                     or run as administrator to allow symlinks.",
                    repo_id,
                    filename,
                    e
                );
                hf_cache_degraded = true;
            }
        }

        db.delete_wal_transaction(&tx_id)?;

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
            size_bytes: outcome.bytes_written,
            cas_path,
            was_cached: false,
            provenance,
            unverified,
            hf_cache_degraded,
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

        let etag = resp
            .header("x-linked-etag")
            .or_else(|| resp.header("etag"))
            .map(|v| v.trim().trim_matches('"').to_string());

        let sha256 = etag
            .as_deref()
            .map(|v| v.to_lowercase())
            .filter(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()));

        let size_bytes = resp
            .header("x-linked-size")
            .or_else(|| resp.header("content-length"))
            .and_then(|v| v.parse::<u64>().ok());

        // huggingface_hub resolves refs/{branch} → commit SHA. Extract it from
        // the final redirect URL: /{repo}/resolve/{commit_sha}/{filename}.
        let final_url = resp.get_url().to_string();
        let commit_sha = extract_commit_sha(final_url.as_str());

        Ok(HfFileMetadata {
            repo_id: repo_id.to_string(),
            filename: filename.to_string(),
            revision: revision.to_string(),
            sha256,
            size_bytes,
            etag,
            commit_sha,
            download_url: url,
        })
    }

    /// Download `meta.url` to `dest`, supporting safe resume via Range requests.
    ///
    /// B4 guarantees:
    /// - `<dest>.meta` must match the current request; a mismatched or missing
    ///   meta discards the partial file (no silent corruption on URL/ETag change).
    /// - Resume sends `If-Range`; a server ignoring Range (HTTP 200) restarts
    ///   from byte 0.
    /// - BLAKE3 and SHA256 are computed on the fly in a 64 KB buffer.
    /// - If the expected size is known, the final byte count must match exactly.
    fn download_to_file(
        &self,
        dest: &Path,
        meta: &PartMeta,
        progress: Option<&ProgressCallback>,
    ) -> Result<DownloadOutcome> {
        let agent = build_agent();
        let filename = dest
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string());

        // Validate the partial file against its side-car before trusting it.
        let saved_meta = PartMeta::load(dest)?;
        let partial_size = match (fs::metadata(dest).ok(), saved_meta) {
            (Some(m), Some(saved)) if saved.matches(meta) => m.len(),
            (Some(_), Some(_)) => {
                tracing::warn!(
                    "discarding partial download {} — URL/ETag/size changed",
                    dest.display()
                );
                let _ = fs::remove_file(dest);
                PartMeta::remove(dest);
                0
            }
            (Some(_), None) => {
                tracing::warn!(
                    "discarding partial download {} — metadata missing or unreadable",
                    dest.display()
                );
                let _ = fs::remove_file(dest);
                0
            }
            (None, _) => 0,
        };

        // Determine whether we can resume or must start fresh.
        let (mut file, resp, mut bytes_done) = if partial_size > 0 {
            let mut req = agent
                .get(&meta.url)
                .set("Range", &format!("bytes={}-", partial_size))
                .timeout(Duration::from_secs(600));
            if let Some(etag) = meta.etag.as_deref() {
                req = req.set("If-Range", etag);
            }
            if let Some(hdr) = auth_header(self.hf_token.as_deref()) {
                req = req.set("Authorization", &hdr);
            }
            let r = req.call().map_err(|e| friendly_http_error(e, &meta.url))?;

            if r.status() == 206 {
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
            let mut req = agent.get(&meta.url).timeout(Duration::from_secs(600));
            if let Some(hdr) = auth_header(self.hf_token.as_deref()) {
                req = req.set("Authorization", &hdr);
            }
            let r = req.call().map_err(|e| friendly_http_error(e, &meta.url))?;
            let f = fs::File::create(dest).context("Failed to create download file")?;
            (f, r, 0u64)
        };

        // Persist the side-car only after we know which mode we are in, so the
        // next run can validate the partial file.
        meta.save(dest)?;

        let total = meta.expected_size.unwrap_or(0);
        let mut buf = vec![0u8; 65536]; // bounded memory: 64 KB, independent of file size
        let mut reader = resp.into_reader();
        let mut blake3_hasher = Blake3Hasher::new();
        let mut sha256_hasher = Sha256::new();

        loop {
            let n = reader.read(&mut buf).context("Error reading response body")?;
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            blake3_hasher.update(chunk);
            sha256_hasher.update(chunk);
            file.write_all(chunk).context("Error writing to file")?;
            bytes_done += n as u64;
            if let Some(cb) = progress {
                cb(bytes_done, total, &filename);
            }
        }
        file.flush().context("Error flushing download file")?;
        drop(file);

        // Enforce the size contract before handing the file to the CAS.
        if let Some(expected) = meta.expected_size {
            anyhow::ensure!(
                bytes_done == expected,
                "download size mismatch for {}: expected {} bytes, got {}",
                dest.display(),
                expected,
                bytes_done
            );
        }

        let blake3 = Blake3Hash::from_hex(blake3_hasher.finalize().to_hex().as_str())?;
        let sha256_digest = format!("{:x}", sha256_hasher.finalize());
        let (sha256, sha256_unverified) = match meta.sha256.as_deref() {
            Some(expected) if expected.to_lowercase() == sha256_digest => {
                (Some(sha256_digest), false)
            }
            Some(expected) => {
                return Err(anyhow!(
                    "SHA256 integrity check failed for {}: expected {}, got {}",
                    dest.display(),
                    expected,
                    sha256_digest
                ));
            }
            None => {
                tracing::warn!(
                    "no trusted SHA256 for {}; recording computed digest as unverified",
                    dest.display()
                );
                (Some(sha256_digest), true)
            }
        };

        Ok(DownloadOutcome { bytes_written: bytes_done, blake3, sha256, sha256_unverified })
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
/// Kept for tests and for operators verifying an on-disk part manually;
/// the download path itself computes SHA256 streaming (B4).
#[cfg(test)]
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

/// Extract a 40-hex commit SHA from a HF resolve URL.
/// Matches `/resolve/{sha}/...` and also bare `{sha}` path segments.
fn extract_commit_sha(url: &str) -> Option<String> {
    let segments: Vec<&str> = url.split(&['/', '?', '#'][..]).filter(|s| !s.is_empty()).collect();
    for (i, seg) in segments.iter().enumerate() {
        if *seg == "resolve" {
            if let Some(candidate) = segments.get(i + 1) {
                if candidate.len() == 40 && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Some(candidate.to_ascii_lowercase());
                }
            }
        }
    }
    // Some CDN URLs carry the commit in a query string (`?download=true` etc.)
    None
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
    fn snapshot_path_for_download_keeps_revision_before_filename() {
        let tmp = TempDir::new().unwrap();
        let cache = HfCache::new(tmp.path());
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let filename = "model.safetensors";

        let path =
            snapshot_path_for_download(&cache, "org/repo", revision, filename, RepoType::Model);
        let relative = path.strip_prefix(tmp.path()).unwrap().to_string_lossy().replace('\\', "/");

        assert_eq!(
            relative,
            format!("hf_cache/hub/models--org--repo/snapshots/{revision}/{filename}")
        );
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
    fn test_extract_commit_sha() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            extract_commit_sha(&format!(
                "https://huggingface.co/org/repo/resolve/{}/model.safetensors?download=true",
                sha
            )),
            Some(sha.to_string())
        );
        assert_eq!(
            extract_commit_sha("https://huggingface.co/org/repo/resolve/main/model.bin"),
            None
        );
        assert_eq!(extract_commit_sha("https://cdn-lfs.example.com/blob/abc"), None);
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

    // ── B4: downloader hardening ───────────────────────────────────────────

    fn sample_meta(url: &str, body: &[u8]) -> PartMeta {
        use sha2::{Digest, Sha256};
        PartMeta {
            url: url.to_string(),
            etag: Some("v1".to_string()),
            expected_size: Some(body.len() as u64),
            sha256: Some(format!("{:x}", Sha256::digest(body))),
        }
    }

    /// Minimal single-response HTTP server (no extra dependencies).
    fn serve_once(body: &[u8], extra_headers: &str) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.to_vec();
        let extra = extra_headers.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).unwrap();
            let header =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}\r\n", body.len(), extra);
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
            let _ = stream.flush();
        });
        (format!("http://{}/model.bin", addr), handle)
    }

    #[test]
    fn part_meta_roundtrip_and_mismatch() {
        let tmp = TempDir::new().unwrap();
        let part = tmp.path().join("model.part");
        let meta = PartMeta {
            url: "http://example/model".to_string(),
            etag: Some("etag-1".to_string()),
            expected_size: Some(1024),
            sha256: None,
        };
        meta.save(&part).unwrap();
        let loaded = PartMeta::load(&part).unwrap().unwrap();
        assert_eq!(loaded, meta);
        assert!(loaded.matches(&meta));

        let changed = PartMeta { etag: Some("etag-2".to_string()), ..meta.clone() };
        assert!(!loaded.matches(&changed));
        let changed = PartMeta { url: "http://other/model".to_string(), ..meta.clone() };
        assert!(!loaded.matches(&changed));
        let changed = PartMeta { expected_size: Some(2048), ..meta };
        assert!(!loaded.matches(&changed));

        PartMeta::remove(&part);
        assert!(PartMeta::load(&part).unwrap().is_none());
    }

    #[test]
    fn download_streams_hashes_and_validates_size() {
        let body = b"modeld-b4-streaming-hash-body";
        let (url, server) = serve_once(body, "");
        let meta = sample_meta(&url, body);
        let tmp = TempDir::new().unwrap();
        let part = tmp.path().join("one.part");
        let dl = Downloader::new(tmp.path());

        let outcome = dl.download_to_file(&part, &meta, None).unwrap();
        server.join().unwrap();

        assert_eq!(outcome.bytes_written, body.len() as u64);
        assert_eq!(fs::read(&part).unwrap(), body);
        assert!(!outcome.sha256_unverified);
        // The streaming BLAKE3 must equal hash_file's standard result.
        assert_eq!(outcome.blake3, crate::hash::hash_file(&part).unwrap());
        // Side-car is written for the next resume.
        assert_eq!(PartMeta::load(&part).unwrap().unwrap(), meta);
    }

    #[test]
    fn download_rejects_size_mismatch() {
        let declared = b"short";
        let (url, server) = serve_once(b"a much longer body than declared", "");
        let mut meta = sample_meta(&url, declared);
        meta.expected_size = Some(declared.len() as u64);
        let tmp = TempDir::new().unwrap();
        let part = tmp.path().join("mismatch.part");
        let dl = Downloader::new(tmp.path());

        let err = dl.download_to_file(&part, &meta, None).unwrap_err();
        server.join().unwrap();
        assert!(format!("{:#}", err).contains("size mismatch"), "{:#}", err);
    }

    #[test]
    fn download_without_sha256_marks_unverified() {
        let body = b"no trusted sha256 here";
        let (url, server) = serve_once(body, "");
        let mut meta = sample_meta(&url, body);
        meta.sha256 = None;
        let tmp = TempDir::new().unwrap();
        let part = tmp.path().join("unverified.part");
        let dl = Downloader::new(tmp.path());

        let outcome = dl.download_to_file(&part, &meta, None).unwrap();
        server.join().unwrap();
        assert!(outcome.sha256_unverified);
        assert!(outcome.sha256.is_some(), "computed digest is still recorded");
    }

    #[test]
    fn stale_part_without_meta_is_discarded() {
        let body = b"fresh";
        let (url, server) = serve_once(body, "");
        let tmp = TempDir::new().unwrap();
        let part = tmp.path().join("stale.part");
        fs::write(&part, b"garbage from previous download").unwrap();
        let meta = sample_meta(&url, body);
        let dl = Downloader::new(tmp.path());

        // A stale partial with no side-car must be removed and re-downloaded
        // from byte 0 — never appended to.
        let outcome = dl.download_to_file(&part, &meta, None).unwrap();
        server.join().unwrap();
        assert_eq!(outcome.bytes_written, body.len() as u64);
        assert_eq!(fs::read(&part).unwrap(), body);
    }

    #[test]
    fn download_discards_part_when_etag_changes() {
        let body = b"new-object-content";
        let (url, server) = serve_once(body, "");
        let tmp = TempDir::new().unwrap();
        let part = tmp.path().join("etag.part");
        fs::write(&part, b"old-object").unwrap();
        let old_meta = PartMeta {
            url: url.clone(),
            etag: Some("old-etag".to_string()),
            expected_size: Some(9),
            sha256: None,
        };
        old_meta.save(&part).unwrap();

        let new_meta = sample_meta(&url, body); // etag = "v1"
        let dl = Downloader::new(tmp.path());
        let outcome = dl.download_to_file(&part, &new_meta, None).unwrap();
        server.join().unwrap();
        assert_eq!(outcome.bytes_written, body.len() as u64, "must not append stale bytes");
        assert_eq!(fs::read(&part).unwrap(), body);
        assert_eq!(PartMeta::load(&part).unwrap().unwrap(), new_meta);
    }
}
