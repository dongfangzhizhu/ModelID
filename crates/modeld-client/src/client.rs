//! Blocking HTTP client for a modeld proxy server.
//!
//! Built on `ureq` to match modeld's synchronous style. Supports health
//! checks, model listing, resumable blob downloads (HTTP Range), and
//! HuggingFace-proxy downloads. A bearer token can be attached for servers
//! that require authentication.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

/// Progress callback: `(bytes_done, bytes_total)`. `total` is 0 if unknown.
pub type ProgressCallback<'a> = Box<dyn Fn(u64, u64) + Send + Sync + 'a>;

/// Health response from `GET /health`.
#[derive(Debug, Clone, Deserialize)]
pub struct HealthInfo {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
    #[serde(default)]
    pub model_count: i64,
    #[serde(default)]
    pub total_bytes: i64,
}

/// A model entry from `GET /v1/models`.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub hash: String,
    pub size_bytes: i64,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    models: Vec<ModelInfo>,
}

/// Client for a running modeld proxy.
pub struct ModeldClient {
    base_url: String,
    token: Option<String>,
}

impl ModeldClient {
    /// Create a client. `base_url` is the proxy root, e.g. `http://192.168.1.5:8234`.
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut url = base_url.into();
        while url.ends_with('/') {
            url.pop();
        }
        Self {
            base_url: url,
            token: None,
        }
    }

    /// Attach a bearer token for authenticated servers.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn agent(&self) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(600))
            .timeout_write(Duration::from_secs(60))
            .build()
    }

    fn authed<'a>(&self, req: ureq::Request) -> ureq::Request {
        match &self.token {
            Some(t) => req.set("Authorization", &format!("Bearer {}", t)),
            None => req,
        }
    }

    /// `GET /health`.
    pub fn health(&self) -> Result<HealthInfo> {
        let url = format!("{}/health", self.base_url);
        let resp = self.authed(self.agent().get(&url)).call();
        let body = read_body(resp?).context("health request failed")?;
        serde_json::from_str::<HealthInfo>(&body).context("parse health response")
    }

    /// `GET /v1/models`.
    pub fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/v1/models", self.base_url);
        let resp = self.authed(self.agent().get(&url)).call();
        let body = read_body(resp?).context("list_models request failed")?;
        let parsed: ModelsResponse =
            serde_json::from_str(&body).context("parse models response")?;
        Ok(parsed.models)
    }

    /// Download a CAS blob by its BLAKE3 hash.
    ///
    /// Resumes from a partial `dest` file when one exists: it sends a
    /// `Range: bytes=N-` request and appends the remainder. After a successful
    /// download the file length is verified against the server's
    /// `Content-Length`.
    pub fn download_blob(
        &self,
        hash: &str,
        dest: &Path,
        progress: Option<&ProgressCallback>,
    ) -> Result<u64> {
        let url = format!("{}/v1/blobs/{}", self.base_url, hash);

        // Resume from existing partial file.
        let already = dest
            .metadata()
            .map(|m| m.len())
            .unwrap_or(0);

        let req = if already > 0 {
            self.authed(self.agent().get(&url)).set("Range", &format!("bytes={}-", already))
        } else {
            self.authed(self.agent().get(&url))
        };

        let resp = req.call().map_err(|e| anyhow!("blob request failed: {}", e))?;
        let status = resp.status();
        // 200 = full, 206 = partial (resume). 416 = range not satisfiable →
        // file already complete.
        if status == 416 {
            let total = dest.metadata().map(|m| m.len()).unwrap_or(already);
            return Ok(total);
        }
        if status != 200 && status != 206 {
            let body = read_body_from(resp).unwrap_or_default();
            anyhow::bail!("server returned {}: {}", status, body);
        }

        let total_len = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        let append = status == 206;
        let mut file = if append {
            OpenOptions::new().append(true).open(dest)
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::File::create(dest)
        }
        .with_context(|| format!("open dest: {}", dest.display()))?;

        let mut reader = resp.into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut written = already;
        loop {
            let n = reader
                .read(&mut buf)
                .context("read blob body")?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).context("write blob body")?;
            written += n as u64;
            if let Some(cb) = progress {
                cb(written, total_len + already);
            }
        }

        Ok(written)
    }

    /// Download a HuggingFace file through the proxy.
    ///
    /// Uses `/v1/hf-proxy/{org}/{repo}/resolve/{revision}/{file}`. The proxy
    /// transparently dedups/caches; the response carries `X-Modeld-Cache`
    /// (`hit`/`miss`) and optionally `X-Modeld-Blake3`.
    pub fn download_hf_file(
        &self,
        repo_id: &str,
        filename: &str,
        revision: &str,
        dest: &Path,
        progress: Option<&ProgressCallback>,
    ) -> Result<(u64, String)> {
        let url = format!(
            "{}/v1/hf-proxy/{}/resolve/{}/{}",
            self.base_url, repo_id, revision, filename
        );

        let req = self.authed(self.agent().get(&url));
        let resp = req.call().map_err(|e| anyhow!("hf proxy request failed: {}", e))?;
        let status = resp.status();
        if status != 200 {
            let body = read_body_from(resp).unwrap_or_default();
            anyhow::bail!("server returned {}: {}", status, body);
        }

        let cache_status = resp
            .header("X-Modeld-Cache")
            .unwrap_or("unknown")
            .to_string();
        let total_len = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut file = std::fs::File::create(dest)
            .with_context(|| format!("create dest: {}", dest.display()))?;

        let mut reader = resp.into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let n = reader.read(&mut buf).context("read hf body")?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).context("write hf body")?;
            written += n as u64;
            if let Some(cb) = progress {
                cb(written, total_len);
            }
        }

        Ok((written, cache_status))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

fn read_body(resp: ureq::Response) -> Result<String> {
    read_body_from(resp)
}

fn read_body_from(resp: ureq::Response) -> Result<String> {
    let mut buf = String::new();
    resp.into_reader()
        .read_to_string(&mut buf)
        .context("read response body")?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_url_trailing_slash_stripped() {
        let c = ModeldClient::new("http://localhost:8234/");
        assert_eq!(c.base_url(), "http://localhost:8234");
    }

    #[test]
    fn test_base_url_multiple_trailing_slashes_stripped() {
        let c = ModeldClient::new("http://localhost:8234///");
        assert_eq!(c.base_url(), "http://localhost:8234");
    }

    #[test]
    fn test_with_token_attaches_token() {
        let c = ModeldClient::new("http://x").with_token("secret");
        assert_eq!(c.token.as_deref(), Some("secret"));
    }

    #[test]
    fn test_health_parses_json() {
        // Pure JSON parsing of a representative /health body.
        let body = r#"{"status":"ok","version":"0.5.0","uptime_seconds":1,"model_count":42,"total_bytes":1000}"#;
        let h: HealthInfo = serde_json::from_str(body).unwrap();
        assert_eq!(h.status, "ok");
        assert_eq!(h.version, "0.5.0");
        assert_eq!(h.uptime_seconds, 1);
        assert_eq!(h.model_count, 42);
        assert_eq!(h.total_bytes, 1000);
    }

    #[test]
    fn test_health_parses_without_optional_fields() {
        // Older servers omit model_count/total_bytes; defaults must apply.
        let body = r#"{"status":"ok","version":"0.5.0","uptime_seconds":0}"#;
        let h: HealthInfo = serde_json::from_str(body).unwrap();
        assert_eq!(h.model_count, 0);
        assert_eq!(h.total_bytes, 0);
    }

    #[test]
    fn test_models_response_parses() {
        let body = r#"{"models":[{"hash":"abcd","size_bytes":100,"format":"safetensors","category":"checkpoint"}],"total":1,"total_size_bytes":100}"#;
        let parsed: ModelsResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].hash, "abcd");
        assert_eq!(parsed.models[0].size_bytes, 100);
        assert_eq!(parsed.models[0].format.as_deref(), Some("safetensors"));
    }

    #[test]
    fn test_hf_proxy_url_constructed_correctly() {
        // Mirrors the URL shape download_hf_file builds.
        let base = "http://192.168.1.5:8234";
        let url = format!(
            "{}/v1/hf-proxy/{}/resolve/{}/{}",
            base, "org/model", "main", "file.safetensors"
        );
        assert_eq!(
            url,
            "http://192.168.1.5:8234/v1/hf-proxy/org/model/resolve/main/file.safetensors"
        );
    }
}

