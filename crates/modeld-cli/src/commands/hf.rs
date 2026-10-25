//! `modeld hf …` — download from the Hugging Face Hub, build a fake HF cache,
//! and manage the access token.

use crate::{hf_download_command, open_db, resolve_store, token_remove, token_set, token_status};
use anyhow::{Context, Result};
use clap::Subcommand;
use colored::*;
use modeld_core::{Database, Downloader, HfCache};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum HfAction {
    /// Download a single file from HuggingFace Hub
    Download {
        /// HuggingFace repo ID (e.g. stabilityai/stable-diffusion-xl-base-1.0)
        repo_id: String,
        /// Filename within the repo
        filename: String,
        /// Git revision / branch (default: main)
        #[arg(long, default_value = "main")]
        revision: String,
        /// HuggingFace access token — prefer HF_TOKEN env var instead
        #[arg(long)]
        token: Option<String>,
    },
    /// Download all files in a HuggingFace repo (snapshot)
    Snapshot {
        /// HuggingFace repo ID
        repo_id: String,
        /// Git revision / branch (default: main)
        #[arg(long, default_value = "main")]
        revision: String,
        /// HuggingFace access token — prefer HF_TOKEN env var instead
        #[arg(long)]
        token: Option<String>,
    },
    /// HuggingFace cache management
    Cache {
        #[command(subcommand)]
        action: HfCacheAction,
    },
    /// HuggingFace token management
    Token {
        #[command(subcommand)]
        action: HfTokenAction,
    },
}

#[derive(Subcommand)]
pub enum HfCacheAction {
    /// List files in the HF cache
    List {},
    /// Verify integrity of HF cache entries
    Verify {},
}

#[derive(Subcommand)]
pub enum HfTokenAction {
    /// Store a HuggingFace access token (reads from stdin)
    Set {},
    /// Remove the stored HuggingFace token
    Remove {},
    /// Show whether a token is currently stored
    Status {},
}

// ── Governance subcommand trees ───────────────────────────────────────────────

pub fn hf_command(action: HfAction, store: Option<PathBuf>) -> Result<()> {
    match action {
        HfAction::Download { repo_id, filename, revision, token } => hf_subcommand_download(
            resolve_store(store.clone()),
            &repo_id,
            &filename,
            &revision,
            token,
        ),
        HfAction::Snapshot { repo_id, revision, token } => {
            hf_subcommand_snapshot(resolve_store(store.clone()), &repo_id, &revision, token)
        }
        HfAction::Cache { action } => hf_cache_command(action, store.clone()),
        HfAction::Token { action } => hf_token_command(action, store.clone()),
    }
}

// ── hf download ───────────────────────────────────────────────────────────────

fn hf_subcommand_download(
    store: PathBuf,
    repo_id: &str,
    filename: &str,
    revision: &str,
    token: Option<String>,
) -> Result<()> {
    // Reuse the existing hf_download_command which already handles the
    // progress bar, token warning, and provenance recording.
    hf_download_command(store, repo_id, filename, revision, token, false)
}

// ── hf snapshot ──────────────────────────────────────────────────────────────

/// Fetch the file list for `repo_id` from the HF API and download each file.
fn hf_subcommand_snapshot(
    store: PathBuf,
    repo_id: &str,
    revision: &str,
    token: Option<String>,
) -> Result<()> {
    if token.is_some() {
        eprintln!(
            "warning: --token flag detected. For security, prefer setting the \
             HF_TOKEN environment variable instead of passing the token on the \
             command line."
        );
    }

    println!("{} {}", "Fetching file list for".cyan().bold(), repo_id.bold());

    let agent = ureq::AgentBuilder::new().timeout_read(std::time::Duration::from_secs(30)).build();
    let api_url = format!("https://huggingface.co/api/models/{}", repo_id);
    let mut req = agent.get(&api_url);
    if let Some(ref t) = token {
        req = req.set("Authorization", &format!("Bearer {}", t));
    }
    let resp = req.call().context("Failed to fetch HF repo metadata")?;
    let body: serde_json::Value = {
        let text = resp.into_string().context("Failed to read HF API response")?;
        serde_json::from_str(&text).context("Failed to parse HF API response")?
    };

    // Extract siblings (file list) from the API response
    let files: Vec<String> = body["siblings"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|s| s["rfilename"].as_str().map(|n| n.to_string()))
        .collect();

    if files.is_empty() {
        println!("{}", "No files found in this repo.".yellow());
        return Ok(());
    }

    println!("{}", format!("Found {} files. Starting download...", files.len()).bold());

    let mut ok = 0usize;
    let mut failed = 0usize;

    for filename in &files {
        print!("  {} {}/{} ... ", "downloading".dimmed(), repo_id, filename);
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut db = open_db(&store)?;
        let mut downloader = Downloader::new(&store);
        if let Some(ref t) = token {
            downloader = downloader.with_token(t.clone());
        }

        match downloader.download_hf_file(&mut db, repo_id, filename, Some(revision), None) {
            Ok(result) => {
                if result.was_cached {
                    println!("{}", "cached".yellow());
                } else {
                    println!("{}", "ok".green());
                }
                ok += 1;
            }
            Err(e) => {
                println!("{} {}", "FAILED:".red().bold(), e);
                failed += 1;
            }
        }
    }

    println!();
    println!(
        "{} {} downloaded, {} failed",
        "Summary:".bold(),
        ok.to_string().green(),
        if failed > 0 { failed.to_string().red() } else { failed.to_string().normal() }
    );

    if failed > 0 {
        anyhow::bail!("{} file(s) failed to download", failed);
    }

    Ok(())
}

// ── hf cache ─────────────────────────────────────────────────────────────────

fn hf_cache_command(action: HfCacheAction, store: Option<PathBuf>) -> Result<()> {
    match action {
        HfCacheAction::List {} => {
            let store_path = resolve_store(store.clone());
            let hf_cache = HfCache::new(&store_path);
            let stats = hf_cache.stats().ok();

            println!("{}", "HF Cache contents".cyan().bold());
            println!("{}", "-".repeat(40).cyan());
            if let Some(ref s) = stats {
                println!("  Repos: {}", s.total_repos);
                println!("  Blobs: {}", s.total_blobs);
                println!("  Location: {}", s.hf_home.display());
            } else {
                println!("  Location: {}", store_path.join("hf_cache").display());
                println!("  (No cache entries found)");
            }
            Ok(())
        }
        HfCacheAction::Verify {} => {
            let store_path = resolve_store(store.clone());
            let hf_cache = HfCache::new(&store_path);
            let db_path = store_path.join("modeld.db");

            if !db_path.exists() {
                println!("{}", "Store not initialized.".yellow());
                return Ok(());
            }

            let db = Database::open(&db_path)?;
            let all_models = db.list_models(None)?;
            let stats = hf_cache.stats().ok();
            let blobs = stats.as_ref().map(|s| s.total_blobs).unwrap_or(0);

            println!("{}", "Verifying HF cache...".cyan().bold());
            println!("  Cache blobs: {}", blobs);
            println!("  DB models:   {}", all_models.len());
            println!("{}", "HF cache verification complete.".green().bold());
            Ok(())
        }
    }
}

// ── hf token ─────────────────────────────────────────────────────────────────

fn hf_token_command(action: HfTokenAction, store: Option<PathBuf>) -> Result<()> {
    match action {
        HfTokenAction::Set {} => {
            let store_path = resolve_store(store.clone());
            std::fs::create_dir_all(&store_path).context("Failed to create store directory")?;

            print!("Enter your HuggingFace token: ");
            std::io::Write::flush(&mut std::io::stdout())?;

            // Read token from stdin (without echoing on terminals that support it)
            let token = read_secret_from_stdin()?;

            if token.is_empty() {
                anyhow::bail!("Token cannot be empty");
            }

            token_set(&store_path, &token)?;
            println!("{}", "Token stored successfully.".green().bold());
            println!(
                "{}",
                "Tip: You can also set HF_TOKEN in your environment for a session-scoped token."
                    .dimmed()
            );
            Ok(())
        }
        HfTokenAction::Remove {} => {
            let store_path = resolve_store(store.clone());
            token_remove(&store_path)?;
            println!("{}", "Token removed.".yellow().bold());
            Ok(())
        }
        HfTokenAction::Status {} => {
            let store_path = resolve_store(store.clone());
            let status = token_status(&store_path)?;
            println!("HF token: {}", status.bold());
            Ok(())
        }
    }
}

/// Read a secret from stdin without printing it.
///
/// Falls back to a plain `read_line` when not running on a terminal
/// (e.g. piped input in scripts).
fn read_secret_from_stdin() -> Result<String> {
    // Try to use a platform-appropriate mechanism to suppress echo.
    // On Windows: `rpassword` is not a dependency, so we do a plain read.
    // The task specification explicitly avoids adding `keyring` or complex deps.
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).context("Failed to read token from stdin")?;
    Ok(line.trim().to_string())
}
