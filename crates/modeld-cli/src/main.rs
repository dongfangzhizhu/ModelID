mod commands;
mod output;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use commands::config::{config_command, ConfigAction};
use commands::db::{db_command, DbAction};
use commands::hf::{hf_command, HfAction};
use commands::hfstore::{
    hf_check_command, hf_setup_command, hf_status_command, refs_orphans_command,
    workflow_deps_command, workflow_scan_command,
};
use commands::meta::{
    favorite_command, note_command, pin_command, refs_command, tag_command, FavoriteAction,
    NoteAction, PinAction, RefsAction, TagAction,
};
use commands::proxy::{proxy_command, ProxyAction};
use commands::store::{store_command, StoreAction};
use commands::tx::{tx_command, TxAction};
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::get_tags_for_model;
use modeld_core::{
    hash_file, load_config, resolve_store_path, run_doctor, run_fsck, save_config, t, tf,
    token_remove, token_set, token_status, unlink_path, CasStore, CheckStatus, Database,
    DedupEngine, DedupMode, DedupStrategy, Downloader, GcEngine, IngestMode, QuarantineManager,
    ScanOptions, Scanner, StoreLock,
};
use output::{ExitCode, EXIT_CODE_HELP};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "modeld")]
#[command(version)] // enables `--version` and `-V` from CARGO_PKG_VERSION
#[command(about = "Content-Addressable Storage for AI models", long_about = None)]
#[command(after_help = EXIT_CODE_HELP)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new modeld store
    Init {
        /// Store directory (default: platform data dir, or $MODELD_STORE)
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Interactive setup wizard (prompts for all options)
        #[arg(long)]
        interactive: bool,
    },
    /// Scan a directory for model files
    Scan {
        /// Directory to scan
        path: PathBuf,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Skip re-hashing files whose size matches the cache (default: true)
        #[arg(long, default_value = "true")]
        incremental: bool,
        /// Force full re-hash of every file, ignoring any cache
        #[arg(long)]
        full: bool,
        /// Exclude paths matching a glob pattern (can be specified multiple times)
        #[arg(long = "exclude", value_name = "GLOB")]
        exclude_globs: Vec<String>,
        /// Follow symbolic links when traversing directories (default: false)
        #[arg(long)]
        follow_symlinks: bool,
        /// Only catalogue files in the database; do not store their content (default)
        #[arg(long, default_value = "false", conflicts_with_all = ["adopt", "copy"])]
        index_only: bool,
        /// Hard-link files into the store (same volume, zero extra space)
        #[arg(long, default_value = "false", conflicts_with_all = ["index_only", "copy"])]
        adopt: bool,
        /// Copy files into the store (works across volumes, uses extra space)
        #[arg(long, default_value = "false", conflicts_with_all = ["index_only", "adopt"])]
        copy: bool,
    },
    /// Show store statistics
    Status {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Show store statistics and duplicate-space summary
    Stats {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Report duplicate model files
    Dupes {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Minimum duplicate file size to include, e.g. 100MB, 2GB
        #[arg(long)]
        min_size: Option<String>,
        /// Output result as JSON
        #[arg(long)]
        json: bool,
    },
    /// List indexed models
    List {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Maximum number of models to show
        #[arg(short, long)]
        limit: Option<i64>,
        /// Output result as JSON
        #[arg(long)]
        json: bool,
        /// Export a license report CSV to a file
        /// (columns: hash, size, format, source_type, hf_repo_id, license, downloaded_at)
        #[arg(long = "export-csv", value_name = "FILE")]
        export_csv: Option<PathBuf>,
    },
    /// Show details for a model hash
    Info {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Compute BLAKE3 hash of a file
    Hash {
        /// File to hash
        file: PathBuf,
    },
    /// Deduplicate model files
    Dedup {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Preview mode - show what would be done without making changes
        #[arg(long)]
        dry_run: bool,
        /// Auto mode - execute without confirmation prompts (alias for --apply)
        #[arg(long)]
        auto: bool,
        /// Apply deduplication (execute changes); without this flag runs as dry-run
        #[arg(long)]
        apply: bool,
        /// Report mode - analyze only, no modifications
        #[arg(long)]
        report: bool,
        /// Dedup strategy: hardlink, symlink, copy-to-cas, virtual-alias
        #[arg(long, value_name = "STRATEGY")]
        strategy: Option<String>,
        /// Minimum file size to consider for dedup, e.g. 100MB, 2GB
        #[arg(long)]
        min_size: Option<String>,
        /// Only process files whose path matches this glob pattern
        /// (can be specified multiple times)
        #[arg(long = "include", value_name = "GLOB")]
        include_globs: Vec<String>,
        /// Exclude files whose path matches this glob pattern
        /// (can be specified multiple times)
        #[arg(long = "exclude", value_name = "GLOB")]
        exclude_globs: Vec<String>,
        /// Protect a directory: files inside are preferred as the canonical copy
        /// (can be specified multiple times)
        #[arg(long = "protect", value_name = "PATH")]
        protect: Vec<PathBuf>,
        /// Pin a specific file path as the highest-priority canonical for this
        /// run (ephemeral, not persisted to the DB; can be specified multiple times)
        #[arg(long = "pin", value_name = "PATH")]
        pin: Vec<PathBuf>,
        /// Skip the confirmation prompt when applying changes
        #[arg(long)]
        yes: bool,
        /// Output result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage quarantine
    Quarantine {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        #[command(subcommand)]
        action: QuarantineAction,
    },
    /// Check if a HuggingFace file is in modeld cache
    HfCheck {
        /// HuggingFace repo ID (e.g. stabilityai/stable-diffusion-xl-base-1.0)
        repo_id: String,
        /// Filename within the repo
        filename: String,
        /// Git revision / branch (default: main)
        #[arg(long, default_value = "main")]
        revision: String,
        /// Output result as JSON
        #[arg(long)]
        json: bool,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Download a file from HuggingFace via modeld CAS
    HfDownload {
        /// HuggingFace repo ID
        repo_id: String,
        /// Filename within the repo
        filename: String,
        /// Git revision / branch (default: main)
        #[arg(long, default_value = "main")]
        revision: String,
        /// HuggingFace access token (or set HF_TOKEN env var)
        #[arg(long)]
        token: Option<String>,
        /// Output result as JSON
        #[arg(long)]
        json: bool,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Configure HF_HOME to point to the modeld HF cache
    HfSetup {
        /// Only print the HF_HOME path without making changes
        #[arg(long)]
        print_path: bool,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Show HuggingFace cache statistics
    HfStatus {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Scan workflow files and index model dependencies
    WorkflowScan {
        /// Directory containing workflow JSON files
        path: PathBuf,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Show model dependencies of a single workflow file
    WorkflowDeps {
        /// Workflow JSON file
        file: PathBuf,
        /// Store directory (optional, for resolved hash lookup)
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// List models with no workflow references (orphans)
    RefsOrphans {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Safe garbage collection: quarantine unreferenced models
    Gc {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Preview what would be collected without making changes
        #[arg(long)]
        preview: bool,
        /// Also clean up expired quarantine entries (>30 days)
        #[arg(long)]
        cleanup_quarantine: bool,
        /// Also remove stale .part and .tmp files from the tmp directory
        #[arg(long)]
        cleanup_tmp: bool,
    },
    /// Verify store consistency: check CAS, aliases, and DB agree
    Verify {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Restore a hardlinked/symlinked file to an independent physical copy
    Unlink {
        /// File path to restore (must have a hardlink/symlink alias in the DB)
        path: PathBuf,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Local registry & proxy server (Phase 5)
    Proxy {
        #[command(subcommand)]
        action: ProxyAction,
    },
    /// Run environment diagnostics
    Doctor {
        /// Output result as JSON
        #[arg(long)]
        json: bool,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Start the modeld Web UI server (unified serve command)
    ///
    /// The bearer token is read from `[auth] token` in `modeld.toml`, then from
    /// `MODELD_AUTH_TOKEN`; if neither is set, one is generated for this run and
    /// printed. The env var is never written to the store.
    Serve {
        /// Bind address (default: 127.0.0.1)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to listen on (default: 8234)
        #[arg(long, short)]
        port: Option<u16>,
        /// Open browser after starting
        #[arg(long)]
        open: bool,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Reject all write operations (read-only mode)
        #[arg(long)]
        read_only: bool,
        /// Allow binding to a non-loopback address (a token is always enforced)
        #[arg(long, default_value = "false")]
        allow_lan: bool,
    },
    /// Manage the modeld store
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },
    /// Read or write modeld.toml configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// HuggingFace Hub integration (download, cache, token management)
    Hf {
        #[command(subcommand)]
        action: HfAction,
    },
    /// Tag management for models
    Tag {
        #[command(subcommand)]
        action: TagAction,
    },
    /// Model notes management
    Note {
        #[command(subcommand)]
        action: NoteAction,
    },
    /// Pin management (protect models from GC)
    Pin {
        #[command(subcommand)]
        action: PinAction,
    },
    /// Favourite management for models
    Favorite {
        #[command(subcommand)]
        action: FavoriteAction,
    },
    /// Reference graph commands
    Refs {
        #[command(subcommand)]
        action: RefsAction,
    },
    /// Transaction management
    Tx {
        #[command(subcommand)]
        action: TxAction,
    },
    /// Database management
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
}

// ── HF subcommand tree ────────────────────────────────────────────────────────

// ── Transaction management ────────────────────────────────────────────────────

// ── Database management ───────────────────────────────────────────────────────

#[derive(Subcommand)]
enum QuarantineAction {
    /// List quarantined files
    List,
    /// Clean up expired quarantine entries (>30 days)
    Cleanup {
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Restore a quarantined file to its original location
    Restore {
        /// Full path to the quarantined file (shown by `quarantine list`)
        path: PathBuf,
    },
}

pub(crate) fn doctor_command(store_path: PathBuf, json_output: bool) -> Result<()> {
    let report = run_doctor(&store_path)?;

    if json_output {
        let checks_json: Vec<serde_json::Value> = report
            .checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "status": c.status.label(),
                    "message": c.message,
                    "fix": c.fix,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "version": report.version,
                "build_target": report.build_target,
                "store_path": report.store_path.display().to_string(),
                "pass": report.pass_count(),
                "warn": report.warn_count(),
                "fail": report.fail_count(),
                "checks": checks_json,
            })
        );
        return Ok(());
    }

    // Human-readable output
    println!("{}", "modeld doctor".cyan().bold());
    println!("{}", "─".repeat(50).cyan());
    println!("  version      : {}", report.version);
    let git_hash = option_env!("GIT_HASH").unwrap_or("unknown");
    println!("  git hash     : {}", git_hash);
    println!("  build target : {}", report.build_target);
    println!("  store path   : {}", report.store_path.display());
    println!("{}", "─".repeat(50).cyan());
    println!();

    for check in &report.checks {
        let (icon, colored_name) = match check.status {
            CheckStatus::Pass => ("✓".green().bold(), check.name.green()),
            CheckStatus::Warn => ("⚠".yellow().bold(), check.name.yellow()),
            CheckStatus::Fail => ("✗".red().bold(), check.name.red().bold()),
        };
        println!("  {} {} — {}", icon, colored_name, check.message);
        if let Some(fix) = &check.fix {
            println!("      {} {}", "→".dimmed(), fix.dimmed());
        }
    }

    println!();
    println!("{}", "─".repeat(50).cyan());
    let summary = format!(
        "Summary: {} pass  {} warn  {} fail",
        report.pass_count(),
        report.warn_count(),
        report.fail_count(),
    );
    if report.fail_count() > 0 {
        println!("  {}", summary.red().bold());
    } else if report.warn_count() > 0 {
        println!("  {}", summary.yellow().bold());
    } else {
        println!("  {}", summary.green().bold());
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// store command
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// config command
// ─────────────────────────────────────────────────────────────────────────────

/// Escape a string for CSV output.  Fields containing commas, quotes, or
/// newlines are wrapped in double-quotes with internal quotes doubled.
fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.2} {}", size, UNITS[unit_idx])
}

/// `modeld hf-download <repo_id> <filename>` — download from HF via modeld CAS
pub(crate) fn hf_download_command(
    store: PathBuf,
    repo_id: &str,
    filename: &str,
    revision: &str,
    token: Option<String>,
    json_output: bool,
) -> Result<()> {
    let mut db = open_db(&store)?;

    let pb = ProgressBar::new(0);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.cyan} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta}) {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.set_message(format!("{}/{}", repo_id, filename));

    let progress_pb = pb.clone();
    let progress: modeld_core::downloader::ProgressCallback =
        Box::new(move |done, total, _name| {
            if total > 0 {
                progress_pb.set_length(total);
            }
            progress_pb.set_position(done);
        });

    let mut downloader = Downloader::new(&store);
    if let Some(t) = token {
        downloader = downloader.with_token(t);
    }

    let result =
        downloader.download_hf_file(&mut db, repo_id, filename, Some(revision), Some(&progress))?;
    pb.finish_with_message(t("progress.done"));

    if json_output {
        println!(
            r#"{{"path": "{}", "blake3": "{}", "size": {}, "cached": {}}}"#,
            result.cas_path.display(),
            result.blake3_hash.as_hex(),
            result.size_bytes,
            result.was_cached,
        );
    } else {
        let status = if result.was_cached {
            t("hf.dl.cached").yellow().to_string()
        } else {
            t("hf.dl.downloaded").green().to_string()
        };

        println!("\n{} {}", "✓".green().bold(), tf("hf.dl.success", &[("status", &status)]));
        println!("  {}", tf("hf.dl.repo", &[("repo", &repo_id)]).cyan());
        println!("  {}", tf("hf.dl.file", &[("file", &filename)]).cyan());
        let h = result.blake3_hash.as_hex()[..16].to_string();
        println!("  {}", tf("hf.dl.blake3", &[("hash", &h)]));
        let size = format_bytes(result.size_bytes);
        println!("  {}", tf("hf.dl.size", &[("value", &size)]));
        let p = result.cas_path.display().to_string();
        println!("  {}", tf("hf.dl.cas_path", &[("path", &p)]).dimmed());
    }

    Ok(())
}

fn main() -> Result<()> {
    // clap exits with 2 for usage errors on its own; everything else goes
    // through ExitCode so the documented table is actually what the shell sees.
    let cli = Cli::parse();
    let _ = ExitCode::Ok;

    match cli.command {
        Commands::Init { path, interactive } => init_command(path, interactive)?,
        Commands::Scan {
            path,
            store,
            incremental,
            full,
            exclude_globs,
            follow_symlinks,
            index_only,
            adopt,
            copy,
        } => {
            // Index is the default: a catalogue scan must not duplicate data.
            let mode = if adopt {
                IngestMode::Adopt
            } else if copy {
                IngestMode::Copy
            } else {
                let _ = index_only;
                IngestMode::Index
            };
            scan_command(
                path,
                resolve_store(store),
                incremental,
                full,
                exclude_globs,
                follow_symlinks,
                mode,
            )?
        }
        Commands::Status { store } => status_command(resolve_store(store))?,
        Commands::Stats { store } => stats_command(resolve_store(store))?,
        Commands::Dupes { store, min_size, json } => {
            dupes_command(resolve_store(store), min_size, json)?
        }
        Commands::List { store, limit, json, export_csv } => {
            list_command(resolve_store(store), limit, json, export_csv)?
        }
        Commands::Info { hash, store, json } => info_command(resolve_store(store), &hash, json)?,
        Commands::Hash { file } => hash_command(file)?,
        Commands::Dedup {
            store,
            dry_run,
            auto,
            apply,
            report,
            strategy,
            min_size,
            include_globs,
            exclude_globs,
            protect,
            pin,
            yes,
            json,
        } => dedup_command(
            resolve_store(store),
            dry_run,
            auto || apply,
            report,
            strategy,
            min_size,
            include_globs,
            exclude_globs,
            protect,
            pin,
            yes,
            json,
        )?,
        Commands::Quarantine { store, action } => quarantine_command(resolve_store(store), action)?,
        Commands::HfCheck { repo_id, filename, revision, json, store } => {
            hf_check_command(resolve_store(store), &repo_id, &filename, &revision, json)?
        }
        Commands::HfDownload { repo_id, filename, revision, token, json, store } => {
            hf_download_command(resolve_store(store), &repo_id, &filename, &revision, token, json)?
        }
        Commands::HfSetup { print_path, store } => {
            hf_setup_command(resolve_store(store), print_path)?
        }
        Commands::HfStatus { store } => hf_status_command(resolve_store(store))?,
        Commands::WorkflowScan { path, store } => {
            workflow_scan_command(resolve_store(store), path)?
        }
        Commands::WorkflowDeps { file, store } => {
            workflow_deps_command(resolve_store(store), file)?
        }
        Commands::RefsOrphans { store, json } => refs_orphans_command(resolve_store(store), json)?,
        Commands::Gc { store, preview, cleanup_quarantine, cleanup_tmp } => {
            gc_command(resolve_store(store), preview, cleanup_quarantine, cleanup_tmp)?
        }
        Commands::Verify { store, json } => verify_command(resolve_store(store), json)?,
        Commands::Unlink { path, store } => unlink_command(resolve_store(store), path)?,
        Commands::Proxy { action } => proxy_command(action)?,
        Commands::Doctor { json, store } => doctor_command(resolve_store(store), json)?,
        Commands::Store { action } => store_command(action)?,
        Commands::Config { action } => config_command(action)?,
        Commands::Hf { action } => hf_command(action)?,
        Commands::Serve { host, port, open, store, read_only, allow_lan } => {
            serve_command(host, port, open, resolve_store(store), read_only, allow_lan)?
        }
        Commands::Tag { action } => tag_command(action)?,
        Commands::Note { action } => note_command(action)?,
        Commands::Pin { action } => pin_command(action)?,
        Commands::Favorite { action } => favorite_command(action)?,
        Commands::Refs { action } => refs_command(action)?,
        Commands::Tx { action } => tx_command(action)?,
        Commands::Db { action } => db_command(action)?,
    }

    Ok(())
}

/// Resolve an optional CLI store override to a concrete PathBuf.
///
/// Priority: `--store <path>` > `MODELD_STORE` env > `modeld.toml` > platform default.
pub(crate) fn resolve_store(store: Option<PathBuf>) -> PathBuf {
    resolve_store_path(store.as_deref())
}

pub(crate) fn require_store_db(store_path: &Path) -> Result<PathBuf> {
    let db_path = store_path.join("modeld.db");
    if !db_path.exists() {
        anyhow::bail!("{}", t("store.not_initialized"));
    }
    Ok(db_path)
}

fn parse_size(input: &str) -> Result<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{}", t("error.size_empty"));
    }

    let split_at =
        trimmed.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split_at);
    let value: f64 =
        number.parse().with_context(|| tf("error.size_invalid", &[("input", &input)]))?;
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        other => anyhow::bail!("{}", tf("error.size_unit", &[("unit", &other)])),
    };
    Ok((value * multiplier) as u64)
}

pub(crate) fn init_command(path: Option<PathBuf>, interactive: bool) -> Result<()> {
    // Resolve store path: CLI arg > MODELD_STORE env > platform default
    let store_path = resolve_store_path(path.as_deref());

    // Warn if the store already exists
    let already_exists = store_path.join("modeld.db").exists();
    if already_exists {
        println!("{}", format!("⚠  Store already exists at: {}", store_path.display()).yellow());
        println!(
            "{}",
            "   Re-initializing will add missing components but won't delete data.".dimmed()
        );
        println!();
    }

    println!("{}", tf("init.at", &[("path", &store_path.display())]).green().bold());

    // Gather settings — interactive mode asks the user, otherwise use defaults.
    let mut config = if already_exists {
        load_config(&store_path).unwrap_or_default()
    } else {
        modeld_core::ModeldConfig::default()
    };

    if interactive {
        println!();
        println!("{}", "Interactive setup wizard".cyan().bold());
        println!("{}", "─".repeat(40).cyan());

        // 1. Store path
        println!("{}", format!("Store path [{}]: ", store_path.display()).bold());
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim();
        if !input.is_empty() {
            config.store.path = Some(input.to_string());
        }

        // 2. Dedup strategy
        println!("{}", "Dedup strategy (hardlink/symlink/copy_to_cas) [hardlink]: ".bold());
        let mut strategy = String::new();
        std::io::stdin().read_line(&mut strategy)?;
        let strategy = strategy.trim();
        if !strategy.is_empty() {
            config.dedup.strategy = strategy.to_string();
        }

        // 3. Quarantine TTL
        println!("{}", "Quarantine TTL in days [30]: ".bold());
        let mut ttl = String::new();
        std::io::stdin().read_line(&mut ttl)?;
        let ttl = ttl.trim();
        if !ttl.is_empty() {
            if let Ok(days) = ttl.parse::<u32>() {
                config.gc.quarantine_ttl_days = days;
            }
        }

        println!();
    }

    // Initialize CAS
    let cas = CasStore::new(&store_path);
    cas.init()?;

    // Initialize database
    let db_path = store_path.join("modeld.db");
    Database::open(&db_path)?;

    // Initialize quarantine directory
    let qm = QuarantineManager::new(&store_path);
    qm.init()?;

    // Write config
    save_config(&store_path, &config)?;

    println!("{}", t("init.success").green());
    println!("\n{}", t("init.next_steps"));
    println!("  {}", t("init.step1").bold());
    println!("  {}", t("init.step2").bold());
    println!("  {}", t("init.step3").bold());

    Ok(())
}

fn scan_command(
    scan_path: PathBuf,
    store_path: PathBuf,
    incremental: bool,
    full: bool,
    exclude_globs: Vec<String>,
    follow_symlinks: bool,
    mode: IngestMode,
) -> Result<()> {
    println!("{}", tf("scan.scanning", &[("path", &scan_path.display())]).cyan().bold());

    // Exclude the store directory from scanning to prevent CAS objects,
    // quarantine files, and staging temps from being re-ingested (audit 5.8).
    let store_canonical = std::fs::canonicalize(&store_path).unwrap_or(store_path.clone());

    // Auto-initialize the store so users don't need `modeld init` first.
    std::fs::create_dir_all(&store_path)
        .with_context(|| format!("Failed to create store directory: {}", store_path.display()))?;
    let cas = CasStore::new(&store_path);
    cas.init()?;
    let db_path = store_path.join("modeld.db");
    let mut db = Database::open(&db_path)?;
    let qm = QuarantineManager::new(&store_path);
    qm.init()?;

    // Build incremental cache: path → (hash, size) for already-indexed files.
    // scan() will skip re-hashing files whose on-disk size matches the cached
    // size (audit 5.6).
    let preindexed = db.get_all_indexed_paths().unwrap_or_default();
    let cached_count = preindexed.len();

    // Extensions come from `modeld.toml [scan] extensions` when configured,
    // otherwise the built-in default list (weights + exported graph formats).
    let store_config = load_config(&store_path).unwrap_or_default();
    let extensions = store_config.scan_extensions();

    let scanner = Scanner::new()
        .with_extensions(extensions)
        .with_excluded_dirs(vec![store_canonical])
        .with_preindexed(preindexed)
        .with_scan_options(ScanOptions { incremental, full, exclude_globs, follow_symlinks });

    // Quick count first
    let (file_count, total_size) = scanner.count_files(&scan_path)?;

    if file_count == 0 {
        println!("{}", t("scan.no_files").yellow());
        return Ok(());
    }

    let gb = format!("{:.2}", total_size as f64 / 1_073_741_824.0);
    println!("{}\n", tf("scan.found", &[("count", &file_count), ("gb", &gb)]).bold());
    if cached_count > 0 {
        println!("  {}", tf("scan.incremental", &[("count", &cached_count)]).dimmed());
    }

    // Create progress bar
    let pb = ProgressBar::new(file_count as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    // Scan and process (hashing now runs in parallel via Rayon — audit 3.3)
    let results = scanner.scan(&scan_path, |path, size| {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let mb = format!("{:.2}", size as f64 / 1_048_576.0);
        pb.set_message(tf("scan.progress_msg", &[("name", &name), ("mb", &mb)]));
        pb.inc(1);
    })?;

    pb.finish_with_message(t("progress.done"));

    // Store in CAS and database via the shared ingestion function so that
    // CLI scans and WebUI scans produce identical CAS/DB state (audit
    // Wave 1). `scan_id` is generated here since a CLI scan_command run has
    // no pre-existing scan_id, and is passed through to
    // `CasStore::store_crash_safe` as the tx_id (audit Wave 5.4).
    println!("\n{}", t("scan.processing").cyan().bold());
    println!("  {}", ingest_mode_notice(mode).dimmed());
    let scan_id = Uuid::new_v4().to_string();
    let ingest_result =
        modeld_core::ingest_scan_results(&store_path, &mut db, &results, &scan_id, mode)?;

    println!("\n{}", t("scan.complete").green().bold());
    println!("  {}", tf("scan.mode", &[("mode", &mode.as_str())]));
    let gb = format!("{:.2}", total_size as f64 / 1_073_741_824.0);
    println!("  {}", tf("scan.processed", &[("count", &ingest_result.processed_count)]).bold());
    println!("  {}", tf("scan.total_size", &[("gb", &gb)]));

    if !ingest_result.errors.is_empty() {
        println!(
            "  {}",
            tf("scan.errors_header", &[("count", &ingest_result.errors.len())]).red().bold()
        );
        for err in &ingest_result.errors {
            println!(
                "{}",
                tf("scan.error_line", &[("path", &err.path), ("reason", &err.reason)]).red()
            );
        }
    }

    Ok(())
}

/// One-line explanation of what the ingest mode does to the user's files.
fn ingest_mode_notice(mode: IngestMode) -> &'static str {
    match mode {
        IngestMode::Index => {
            "mode: index — files are catalogued only; nothing is copied or linked \
             (use --adopt to share them, --copy to duplicate them into the store)"
        }
        IngestMode::Adopt => {
            "mode: adopt — files are hard-linked into the store and become read-only \
             (zero extra disk space; requires the store and the files on one volume)"
        }
        IngestMode::Copy => "mode: copy — files are copied into the store (needs extra disk space)",
    }
}

fn status_command(store_path: PathBuf) -> Result<()> {
    let db_path = store_path.join("modeld.db");

    if !db_path.exists() {
        eprintln!("{}", t("store.not_initialized").red());
        std::process::exit(1);
    }

    let db = Database::open(&db_path)?;

    let count = db.count_models()?;
    let total_size = db.total_size()?;

    println!("{}", t("status.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    println!("  {}", tf("status.store_path", &[("path", &store_path.display())]));
    println!("  {}", tf("status.total_models", &[("count", &count)]).bold());
    let gb = format!("{:.2}", total_size as f64 / 1_073_741_824.0);
    println!("  {}", tf("status.total_size", &[("gb", &gb)]).bold());
    println!("{}", "─".repeat(40).cyan());

    if count > 0 {
        println!("\n{}", t("status.recent").cyan());
        let models = db.list_models(Some(5))?;
        for model in models {
            let hash = model.blake3_hash.as_hex()[..16].to_string();
            let mb = format!("{:.2}", model.size_bytes as f64 / 1_048_576.0);
            println!("  {}", tf("status.recent_line", &[("hash", &hash), ("mb", &mb)]).dimmed());
        }
    }

    // Show quarantine stats
    let qm = QuarantineManager::new(&store_path);
    if let Ok(qstats) = qm.stats() {
        if qstats.total_files > 0 {
            println!("\n{}", t("status.quarantine").yellow());
            println!("  {}", tf("status.q_files", &[("count", &qstats.total_files)]).bold());
            let mb = format!("{:.2}", qstats.total_size as f64 / 1_048_576.0);
            println!("  {}", tf("status.q_size", &[("mb", &mb)]));
            if qstats.expired_files > 0 {
                println!("  {}", tf("status.q_expired", &[("count", &qstats.expired_files)]).red());
            }
        }
    }

    Ok(())
}

fn stats_command(store_path: PathBuf) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;
    let count = db.count_models()?;
    let total_size = db.total_size()?;
    let aliases = db.list_models(None)?.iter().try_fold(0i64, |acc, model| {
        Ok::<_, anyhow::Error>(acc + db.count_aliases(&model.blake3_hash)?)
    })?;

    let engine = DedupEngine::new(db, store_path.clone());
    let groups = engine.find_duplicates()?;
    let duplicate_bytes = engine.calculate_savings(&groups);

    println!("{}", t("stats.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    println!("  {}", tf("stats.store_path", &[("path", &store_path.display())]));
    println!("  {}", tf("stats.models", &[("count", &count)]).bold());
    println!("  {}", tf("stats.aliases", &[("count", &aliases)]).bold());
    let gb = format!("{:.2}", total_size as f64 / 1_073_741_824.0);
    println!("  {}", tf("stats.indexed_size", &[("gb", &gb)]));
    println!("  {}", tf("stats.dup_groups", &[("count", &groups.len())]).bold());
    let gb = format!("{:.2}", duplicate_bytes as f64 / 1_073_741_824.0);
    println!("  {}", tf("stats.savings", &[("gb", &gb)]));
    println!("{}", "─".repeat(40).cyan());

    Ok(())
}

fn dupes_command(store_path: PathBuf, min_size: Option<String>, json_output: bool) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;
    let engine = DedupEngine::new(db, store_path);
    let min_size_bytes = min_size.as_deref().map(parse_size).transpose()?.unwrap_or(0);

    let groups: Vec<_> =
        engine.find_duplicates()?.into_iter().filter(|g| g.total_size >= min_size_bytes).collect();
    let duplicate_bytes = engine.calculate_savings(&groups);

    if json_output {
        let groups_json: Vec<_> = groups
            .iter()
            .map(|group| {
                serde_json::json!({
                    "hash": group.hash.as_hex(),
                    "size_bytes": group.total_size,
                    "duplicate_count": group.files.len(),
                    "potential_savings_bytes": group.total_size.saturating_mul(group.files.len().saturating_sub(1) as u64),
                    "files": group.files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "duplicate_groups": groups_json,
                "total_groups": groups.len(),
                "potential_savings_bytes": duplicate_bytes,
            })
        );
        return Ok(());
    }

    println!("{}", t("dupes.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    if groups.is_empty() {
        println!("{}", t("dupes.none").green());
        return Ok(());
    }

    for group in &groups {
        let hash = group.hash.as_hex()[..16].to_string();
        let mb = format!("{:.2}", group.total_size as f64 / 1_048_576.0);
        println!(
            "\n{}",
            tf("dupes.group_line", &[("hash", &hash), ("mb", &mb), ("count", &group.files.len())])
                .bold()
        );
        for file in &group.files {
            let path = file.path.display().to_string();
            println!("  {}", tf("dupes.file_line", &[("path", &path)]));
        }
    }
    println!("{}", "─".repeat(40).cyan());
    let gb = format!("{:.2}", duplicate_bytes as f64 / 1_073_741_824.0);
    println!("{}", tf("dupes.savings", &[("gb", &gb)]));

    Ok(())
}

fn list_command(
    store_path: PathBuf,
    limit: Option<i64>,
    json_output: bool,
    export_csv: Option<PathBuf>,
) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;
    let models = db.list_models(limit)?;

    // ── CSV export ──────────────────────────────────────────────────────────
    if let Some(csv_path) = export_csv {
        use std::io::Write as IoWrite;
        let mut file = std::fs::File::create(&csv_path)
            .with_context(|| format!("Failed to create CSV file: {}", csv_path.display()))?;

        // Header
        writeln!(file, "hash,size,format,source_type,hf_repo_id,license,downloaded_at")?;

        for m in &models {
            let hash = m.blake3_hash.as_hex();
            let size = m.size_bytes;
            let format = csv_escape(m.format.as_deref().unwrap_or(""));
            let source_type = csv_escape(m.source_type.as_deref().unwrap_or("local"));
            let hf_repo_id = csv_escape(m.hf_repo_id.as_deref().unwrap_or(""));
            let license = csv_escape(m.license.as_deref().unwrap_or(""));
            let downloaded_at = m.downloaded_at.map(|d| d.to_rfc3339()).unwrap_or_default();
            writeln!(
                file,
                "{},{},{},{},{},{},{}",
                hash, size, format, source_type, hf_repo_id, license, downloaded_at
            )?;
        }

        println!(
            "{} Exported {} model(s) to {}",
            "✓".green().bold(),
            models.len(),
            csv_path.display()
        );
        return Ok(());
    }

    if json_output {
        let rows: Vec<_> = models
            .iter()
            .map(|m| {
                serde_json::json!({
                    "hash": m.blake3_hash.as_hex(),
                    "size_bytes": m.size_bytes,
                    "format": m.format,
                    "arch": m.arch,
                    "category": m.category,
                    "base_model": m.base_model,
                    "created_at": m.created_at.to_rfc3339(),
                    "last_seen": m.last_seen.to_rfc3339(),
                    "quarantined_at": m.quarantined_at.map(|d| d.to_rfc3339()),
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "models": rows, "total": rows.len() }));
        return Ok(());
    }

    println!("{}", t("list.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    if models.is_empty() {
        println!("{}", t("list.none").yellow());
        return Ok(());
    }
    for model in models {
        let hash = model.blake3_hash.as_hex()[..16].to_string();
        let mb = format!("{:>9.2}", model.size_bytes as f64 / 1_048_576.0);
        let format = model.format.as_deref().unwrap_or("unknown");
        println!(
            "{}",
            tf("list.line", &[("hash", &hash), ("mb", &mb), ("format", &format)]).bold()
        );
    }

    Ok(())
}

fn info_command(store_path: PathBuf, hash_hex: &str, json_output: bool) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;
    let hash = modeld_core::Blake3Hash::from_hex(hash_hex)?;
    let Some(model) = db.get_model(&hash)? else {
        anyhow::bail!("{}", tf("error.model_not_found", &[("hash", &hash_hex)]));
    };
    let aliases = db.get_aliases_for_model(&hash)?;
    let cas_path = CasStore::new(&store_path).get(&hash);
    let tags = get_tags_for_model(&db, &hash)?;

    if json_output {
        let aliases_json: Vec<_> = aliases
            .iter()
            .map(|a| {
                serde_json::json!({
                    "path": a.path,
                    "frontend": a.frontend.as_str(),
                    "alias_type": a.alias_type.as_str(),
                    "created_at": a.created_at.to_rfc3339(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "hash": model.blake3_hash.as_hex(),
                "size_bytes": model.size_bytes,
                "format": model.format,
                "arch": model.arch,
                "category": model.category,
                "base_model": model.base_model,
                "created_at": model.created_at.to_rfc3339(),
                "last_seen": model.last_seen.to_rfc3339(),
                "quarantined_at": model.quarantined_at.map(|d| d.to_rfc3339()),
                "cas_path": cas_path.map(|p| p.display().to_string()),
                "aliases": aliases_json,
                // Governance fields
                "note": model.note,
                "pinned": model.pinned,
                "favorited": model.favorited,
                "tags": tags,
                // Provenance fields
                "source_type": model.source_type,
                "hf_repo_id": model.hf_repo_id,
                "revision": model.revision,
                "download_url": model.download_url,
                "license": model.license,
                "downloaded_at": model.downloaded_at.map(|d| d.to_rfc3339()),
                "original_filename": model.original_filename,
                "model_card_url": model.model_card_url,
            })
        );
        return Ok(());
    }

    println!("{}", t("info.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    let hash_str = model.blake3_hash.as_hex().to_string();
    println!("  {}", tf("info.hash", &[("hash", &hash_str)]).bold());
    let mb = format!("{:.2}", model.size_bytes as f64 / 1_048_576.0);
    println!("  {}", tf("info.size", &[("mb", &mb)]));
    let format = model.format.as_deref().unwrap_or("unknown");
    println!("  {}", tf("info.format", &[("value", &format)]));
    let category = model.category.as_deref().unwrap_or("unknown");
    println!("  {}", tf("info.category", &[("value", &category)]));
    let last_seen = model.last_seen.to_rfc3339();
    println!("  {}", tf("info.last_seen", &[("value", &last_seen)]));
    if let Some(path) = cas_path {
        let p = path.display().to_string();
        println!("  {}", tf("info.cas_path", &[("path", &p)]));
    }

    // ── Governance ──────────────────────────────────────────────────────────
    println!("\n{}", "Governance".cyan());
    let pin_icon = if model.pinned {
        "📌 pinned".green().to_string()
    } else {
        "  not pinned".dimmed().to_string()
    };
    let fav_icon = if model.favorited {
        "⭐ favorited".yellow().to_string()
    } else {
        "  not favorited".dimmed().to_string()
    };
    println!("  {}", pin_icon);
    println!("  {}", fav_icon);
    if let Some(ref note) = model.note {
        println!("  note: {}", note);
    }
    if !tags.is_empty() {
        println!("  tags: {}", tags.join(", "));
    }

    // ── Provenance ───────────────────────────────────────────────────────────
    let has_provenance = model.hf_repo_id.is_some()
        || model.source_type.as_deref().map(|s| s != "local").unwrap_or(false)
        || model.original_filename.is_some()
        || model.license.is_some()
        || model.downloaded_at.is_some();

    if has_provenance {
        println!("\n{}", "Provenance".cyan());
        if let Some(ref src) = model.source_type {
            println!("  source:   {}", src);
        }
        if let Some(ref repo) = model.hf_repo_id {
            println!("  hf_repo:  {}", repo);
        }
        if let Some(ref rev) = model.revision {
            println!("  revision: {}", rev);
        }
        if let Some(ref fname) = model.original_filename {
            println!("  filename: {}", fname);
        }
        if let Some(ref lic) = model.license {
            println!("  license:  {}", lic);
        }
        if let Some(ref dl_at) = model.downloaded_at {
            println!("  downloaded: {}", dl_at.to_rfc3339());
        }
        if let Some(ref url) = model.model_card_url {
            println!("  model card: {}", url.dimmed());
        }
    }

    println!("\n{}", t("info.aliases").cyan());
    if aliases.is_empty() {
        println!("{}", t("info.aliases_none"));
    } else {
        for alias in aliases {
            let path = alias.path.clone();
            let frontend = alias.frontend.as_str().to_string();
            let kind = alias.alias_type.as_str().to_string();
            println!(
                "  {}",
                tf("info.alias_line", &[("path", &path), ("frontend", &frontend), ("kind", &kind)])
            );
        }
    }

    Ok(())
}

fn hash_command(file: PathBuf) -> Result<()> {
    if !file.exists() {
        let p = file.display().to_string();
        eprintln!(
            "{} {}",
            t("error.prefix").red().bold(),
            tf("error.file_not_found", &[("path", &p)])
        );
        std::process::exit(1);
    }

    let p = file.display().to_string();
    println!("{}", tf("hash.computing", &[("path", &p)]).cyan());

    let hash = hash_file(&file)?;
    let metadata = std::fs::metadata(&file)?;

    println!("\n{}", t("hash.results").green().bold());
    let hex = hash.as_hex().to_string();
    println!("  {}", tf("hash.value", &[("hash", &hex)]).bold());
    let prefix = hash.prefix().to_string();
    println!("  {}", tf("hash.prefix", &[("prefix", &prefix)]).bold());
    let mb = format!("{:.2}", metadata.len() as f64 / 1_048_576.0);
    println!("  {}", tf("hash.size", &[("mb", &mb)]));

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn dedup_command(
    store_path: PathBuf,
    dry_run: bool,
    auto: bool,
    report: bool,
    strategy_str: Option<String>,
    min_size: Option<String>,
    include_globs: Vec<String>,
    exclude_globs: Vec<String>,
    protect: Vec<PathBuf>,
    pin: Vec<PathBuf>,
    yes: bool,
    json: bool,
) -> Result<()> {
    let db_path = store_path.join("modeld.db");

    if !db_path.exists() {
        eprintln!("{}", t("store.not_initialized").red());
        std::process::exit(1);
    }

    // Determine mode
    let mode = if report {
        DedupMode::Report
    } else if dry_run {
        DedupMode::DryRun
    } else if auto {
        DedupMode::Auto
    } else {
        // Default: safe dry-run preview. Pass --auto to actually modify files.
        DedupMode::DryRun
    };

    let mode_str = match mode {
        DedupMode::DryRun => "dry_run",
        DedupMode::Report => "report",
        DedupMode::Auto => "apply",
        DedupMode::Interactive => "interactive",
    };

    if !json {
        let mode_label = match mode {
            DedupMode::DryRun => t("dedup.mode.dry_run"),
            DedupMode::Report => t("dedup.mode.report"),
            DedupMode::Auto => t("dedup.mode.auto"),
            DedupMode::Interactive => t("dedup.mode.interactive"),
        };
        println!("{}", t("dedup.header").cyan().bold());
        println!("{}", "─".repeat(40).cyan());
        let sp = store_path.display().to_string();
        println!("  {}", tf("dedup.store", &[("path", &sp)]));
        println!("  {}", tf("dedup.mode", &[("mode", &mode_label)]).yellow());
        println!("{}", "─".repeat(40).cyan());
    }

    // Two processes deduping the same store would interleave renames and the
    // WAL would describe a store that no longer exists.  Wait briefly, then
    // fail with the holder's identity instead of corrupting anything.
    let _lock = StoreLock::acquire(&store_path, "dedup", Duration::from_secs(30))
        .with_context(|| tf("lock.busy", &[("path", &store_path.display().to_string())]))?;

    let db = Database::open(&db_path)?;
    let mut engine = DedupEngine::new(db, store_path.clone());

    // Apply optional strategy override
    if let Some(ref s) = strategy_str {
        let strategy: DedupStrategy =
            s.parse().with_context(|| format!("Invalid dedup strategy: {s}"))?;
        engine = engine.with_strategy(strategy);
    }

    // Apply min-size filter
    if let Some(ref ms) = min_size {
        let bytes = parse_size(ms)?;
        engine = engine.with_min_size(bytes);
    }

    // Apply protected paths
    if !protect.is_empty() {
        engine = engine.with_protected_paths(protect);
    }

    // Apply include/exclude glob filters
    if !include_globs.is_empty() {
        engine = engine.with_include_globs(include_globs);
    }
    if !exclude_globs.is_empty() {
        engine = engine.with_exclude_globs(exclude_globs);
    }

    // Apply ephemeral pin paths (highest-priority canonicals for this run)
    if !pin.is_empty() {
        engine = engine.with_pin_paths(pin);
    }

    // First, find and report duplicates
    let groups = engine.find_duplicates()?;

    if groups.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "mode": mode_str,
                    "groups": [],
                    "total_groups": 0,
                    "potential_savings_bytes": 0,
                })
            );
        } else {
            println!("\n{}", t("dedup.no_dupes").green());
        }
        return Ok(());
    }

    let total_savings = engine.calculate_savings(&groups);

    if mode == DedupMode::DryRun || mode == DedupMode::Report {
        if json {
            // JSON mode: output a single JSON object with the preview data
            let groups_json: Vec<serde_json::Value> = groups
                .iter()
                .map(|g| {
                    serde_json::json!({
                        "hash": g.hash.as_hex(),
                        "size_bytes": g.total_size,
                        "file_count": g.files.len(),
                        "potential_savings_bytes": g.total_size.saturating_mul(
                            (g.files.len().saturating_sub(1)) as u64
                        ),
                        "files": g.files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "mode": mode_str,
                    "groups": groups_json,
                    "total_groups": groups.len(),
                    "potential_savings_bytes": total_savings,
                })
            );
        } else {
            println!(
                "\n{} {}",
                "→".cyan(),
                tf("dedup.groups_found", &[("count", &groups.len())]).bold()
            );
            let gb = format!("{:.2}", total_savings as f64 / 1_073_741_824.0);
            println!("{} {}", "→".cyan(), tf("dedup.potential_savings", &[("gb", &gb)]));

            println!("\n{}", t("dedup.groups_header").cyan().bold());
            for (i, group) in groups.iter().enumerate() {
                let mb = format!("{:.2}", group.total_size as f64 / 1_048_576.0);
                println!(
                    "\n  {}",
                    tf(
                        "dedup.group_header",
                        &[("i", &(i + 1)), ("mb", &mb), ("count", &group.files.len())]
                    )
                    .bold()
                );
                for file in &group.files {
                    let p = file.path.display().to_string();
                    println!("    {}", tf("dedup.file_line", &[("path", &p)]).dimmed());
                }
                let saved =
                    (group.total_size * (group.files.len() as u64 - 1)) as f64 / 1_048_576.0;
                let saved = format!("{:.2}", saved);
                println!("    {} {}", "→".green(), tf("dedup.would_save", &[("mb", &saved)]));
            }

            println!("\n{}", "─".repeat(40).cyan());
            let gb = format!("{:.2}", total_savings as f64 / 1_073_741_824.0);
            println!("  {} {}", "✓".green(), tf("dedup.total_potential", &[("gb", &gb)]));
            println!("{}", t("dedup.run_without_dry_run"));
        }
        return Ok(());
    }

    // Executing for real: require explicit y/N confirmation to prevent
    // accidental data-modifying runs (audit item 1.6).
    // Skip prompt when --yes is passed or when --auto --yes is used.
    // In JSON mode without --yes: require --yes to prevent silent destructive ops.
    if !yes {
        if json {
            anyhow::bail!("dedup --json --apply requires --yes to confirm destructive operation");
        }
        println!(
            "\n{}",
            "⚠  This will replace duplicate files with hard/symlinks on disk.".yellow().bold()
        );
        println!("{}", "   The operation is not easily reversible.".yellow());
        print!("{}", "   Proceed? [y/N]: ".bold());
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("{}", t("dedup.aborted").yellow());
            return Ok(());
        }
    }

    // Execute deduplication with progress
    if !json {
        println!("\n{}", t("dedup.executing").cyan().bold());
    }

    let pb = ProgressBar::new(groups.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    // In JSON mode, redirect the progress bar to stderr so stdout stays clean
    if json {
        pb.set_draw_target(indicatif::ProgressDrawTarget::stderr());
    }

    let stats = engine.run_dedup(mode, |current, total, hash_prefix| {
        pb.set_message(tf(
            "progress.processing",
            &[("prefix", &hash_prefix), ("current", &current), ("total", &total)],
        ));
        pb.set_position((current as u64).saturating_sub(1));
    })?;

    pb.finish_and_clear();

    if json {
        // JSON mode: output a single JSON object with the apply stats
        println!(
            "{}",
            serde_json::json!({
                "mode": mode_str,
                "groups_processed": stats.groups_processed,
                "groups_succeeded": stats.groups_succeeded,
                "groups_failed": stats.groups_failed,
                "files_deduplicated": stats.files_deduplicated,
                "space_saved_bytes": stats.space_saved,
                "locked_files_skipped": stats.locked_files_skipped,
                "locked_files": stats.locked_files.iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>(),
            })
        );
    } else {
        // Human-readable final report
        println!("\n{}", "─".repeat(40).cyan());
        println!("{}", t("dedup.complete").green().bold());
        println!(
            "  {}",
            tf("dedup.groups_processed", &[("count", &stats.groups_processed)]).bold()
        );
        println!(
            "  {}",
            tf("dedup.groups_succeeded", &[("count", &stats.groups_succeeded)]).green().bold()
        );
        if stats.groups_failed > 0 {
            println!(
                "  {}",
                tf("dedup.groups_failed", &[("count", &stats.groups_failed)]).red().bold()
            );
        }
        println!("  {}", tf("dedup.files_dedup", &[("count", &stats.files_deduplicated)]).bold());
        let gb = format!("{:.2}", stats.space_saved as f64 / 1_073_741_824.0);
        println!("  {}", tf("dedup.space_saved", &[("gb", &gb)]).green().bold());
        if stats.locked_files_skipped > 0 {
            println!(
                "  {}",
                format!(
                    "⚠  {} file(s) were locked by another process and skipped:",
                    stats.locked_files_skipped
                )
                .yellow()
                .bold()
            );
            for locked in &stats.locked_files {
                println!("    {}", locked.display().to_string().yellow());
            }
        }
        println!("{}", "─".repeat(40).cyan());
    }

    Ok(())
}

fn quarantine_command(store_path: PathBuf, action: QuarantineAction) -> Result<()> {
    let qm = QuarantineManager::new(&store_path);

    match action {
        QuarantineAction::List => {
            let entries = qm.list()?;

            if entries.is_empty() {
                println!("{}", t("quarantine.list.none").green());
                return Ok(());
            }

            println!("{}", t("quarantine.list.header").cyan().bold());
            println!("{}", "─".repeat(60).cyan());

            for entry in &entries {
                let status = match entry.days_remaining {
                    Some(d) => {
                        tf("quarantine.days_remaining", &[("count", &d)]).yellow().to_string()
                    }
                    None => t("quarantine.expired").red().to_string(),
                };
                let qpath = entry.quarantine_path.display().to_string();

                println!(
                    "\n  {}",
                    tf("quarantine.entry_header", &[("path", &qpath), ("status", &status)]).bold()
                );
                let orig = entry.meta.original_path.clone();
                println!("    {}", tf("quarantine.original", &[("path", &orig)]).dimmed());
                let h = entry.meta.blake3_hash[..16].to_string();
                println!("    {}", tf("quarantine.hash", &[("hash", &h)]).dimmed());
                let mb = format!("{:.2}", entry.meta.size_bytes as f64 / 1_048_576.0);
                println!("    {}", tf("quarantine.size", &[("mb", &mb)]));
                let reason = entry.meta.reason.clone();
                println!("    {}", tf("quarantine.reason", &[("reason", &reason)]));
                let when = entry.meta.quarantined_at.format("%Y-%m-%d %H:%M UTC").to_string();
                println!("    {}", tf("quarantine.quarantined_at", &[("value", &when)]));
            }

            println!("\n{}", "─".repeat(60).cyan());
            println!("  {}", tf("quarantine.total", &[("count", &entries.len())]).bold());

            let stats = qm.stats()?;
            let mb = format!("{:.2}", stats.total_size as f64 / 1_048_576.0);
            println!("  {}", tf("quarantine.size_line", &[("mb", &mb)]));

            if stats.expired_files > 0 {
                println!(
                    "{}",
                    tf("quarantine.cleanup_expired_hint", &[("count", &stats.expired_files)]).red()
                );
            }
        }

        QuarantineAction::Cleanup { yes } => {
            if !yes {
                print!("This will permanently delete all expired quarantine entries (>30 days). Proceed? [y/N]: ");
                std::io::Write::flush(&mut std::io::stdout())?;
                let mut ans = String::new();
                std::io::stdin().read_line(&mut ans)?;
                if !ans.trim().eq_ignore_ascii_case("y") {
                    println!("{}", "Aborted.".yellow());
                    return Ok(());
                }
            }

            println!("{}", t("quarantine.cleanup.start").cyan());

            let cleaned = qm.cleanup_expired()?;

            if cleaned == 0 {
                println!("{}", t("quarantine.cleanup.none").green());
            } else {
                println!(
                    "{} {}",
                    "✓".green(),
                    tf("quarantine.cleanup.done", &[("count", &cleaned)]).bold()
                );
            }
        }

        QuarantineAction::Restore { path } => {
            let restored = qm.restore(&path)?;
            println!(
                "{} {}",
                "✓".green().bold(),
                tf("quarantine.restore.done", &[("path", &restored.display().to_string())]).bold()
            );
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// HF Command implementations
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) fn open_db(store: &std::path::Path) -> Result<Database> {
    // Use the same DB filename as every other command so HF downloads land in
    // the same metadata store that `scan`/`status`/`dedup`/`gc`/`workflow-scan`
    // read from. (Previously this used "index.db", which caused the HF path to
    // silently diverge from the rest of the pipeline.)
    let db_path = store.join("modeld.db");
    Database::open(&db_path)
}

fn gc_command(
    store: PathBuf,
    preview: bool,
    cleanup_quarantine: bool,
    cleanup_tmp: bool,
) -> Result<()> {
    let db_path = store.join("modeld.db");
    if !db_path.exists() {
        anyhow::bail!("{}", t("store.not_initialized_exit"));
    }

    // GC deletes objects; it must not run concurrently with another layout
    // mutation (dedup, another gc) in the same store.
    let _lock = StoreLock::acquire(&store, "gc", Duration::from_secs(30))
        .with_context(|| tf("lock.busy", &[("path", &store.display().to_string())]))?;

    let mut db = Database::open(&db_path)?;
    let mut gc = GcEngine::new(&mut db, &store);

    if preview {
        let plan = gc.preview()?;

        println!("{}", t("gc.preview.header").bold());
        println!();

        println!(
            "  {}",
            tf("gc.preview.hard", &[("count", &plan.hard_protected.len())]).green().bold()
        );
        if !plan.soft_protected.is_empty() {
            println!(
                "  {}",
                tf("gc.preview.soft_header", &[("count", &plan.soft_protected.len())])
                    .yellow()
                    .bold()
            );
            for item in &plan.soft_protected {
                let size = format_bytes(item.size_bytes as u64);
                println!(
                    "    {}",
                    tf(
                        "gc.preview.soft_line",
                        &[
                            ("prefix", &item.hash_prefix),
                            ("aliases", &item.alias_count),
                            ("size", &size),
                        ]
                    )
                    .yellow()
                );
            }
        }
        if !plan.would_quarantine.is_empty() {
            println!(
                "  {}",
                tf("gc.preview.quarantine_header", &[("count", &plan.would_quarantine.len())])
                    .red()
                    .bold()
            );
            for item in &plan.would_quarantine {
                let size = format_bytes(item.size_bytes as u64);
                println!(
                    "    {}",
                    tf(
                        "gc.preview.quarantine_line",
                        &[("prefix", &item.hash_prefix), ("size", &size)]
                    )
                    .red()
                );
            }
            let size = format_bytes(plan.total_reclaimable_bytes as u64);
            println!("  {}", tf("gc.preview.reclaimable", &[("size", &size)]).yellow().bold());
        } else {
            println!("  {}", t("gc.preview.nothing").green());
        }
        if plan.expired_quarantine_count > 0 {
            println!();
            let size = format_bytes(plan.expired_quarantine_bytes as u64);
            println!(
                "  {}",
                tf(
                    "gc.preview.expired",
                    &[("count", &plan.expired_quarantine_count), ("size", &size)]
                )
                .yellow()
                .bold()
            );
        }
        println!();
        println!("{}", t("gc.preview.run").dimmed());
        return Ok(());
    }

    // Execute GC
    println!("{}", t("gc.running").bold());
    let result = gc.run_safe()?;

    println!();
    if result.quarantined.is_empty() {
        println!("{}", t("gc.nothing_quarantined").green().bold());
    } else {
        let size = format_bytes(result.bytes_recovered as u64);
        println!(
            "  {}",
            tf("gc.quarantined", &[("count", &result.quarantined.len()), ("size", &size)]).bold()
        );
    }
    if !result.skipped_protected.is_empty() {
        println!(
            "  {} {}",
            "•".blue(),
            tf("gc.skipped_hard", &[("count", &result.skipped_protected.len())])
        );
    }
    if !result.skipped_soft.is_empty() {
        println!(
            "  {} {}",
            "⚠".yellow(),
            tf("gc.skipped_soft", &[("count", &result.skipped_soft.len())])
        );
    }

    if cleanup_quarantine {
        println!();
        println!("{}", t("gc.cleanup_header").bold());
        let cleaned = gc.cleanup_quarantine()?;
        if cleaned > 0 {
            println!("  {}", tf("gc.cleanup_done", &[("count", &cleaned)]).green().bold());
        } else {
            println!("  {}", t("gc.cleanup_none"));
        }
    }

    if cleanup_tmp {
        println!("\n{}", t("gc.cleanup_tmp_header").bold());
        // Part files: keep for 7 days (in case a slow download can be resumed)
        // Staging files: keep for 1 day (should be renamed to CAS quickly)
        let removed = modeld_core::GcEngine::cleanup_tmp(&store, 7 * 24, 24)?;
        if removed > 0 {
            println!("  {}", tf("gc.cleanup_tmp_done", &[("count", &removed)]).green().bold());
        } else {
            println!("  {}", t("gc.cleanup_tmp_none"));
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Verify (fsck) command
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) fn verify_command(store_path: PathBuf, json_output: bool) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;

    if !json_output {
        println!("{}", t("verify.running").cyan().bold());
    }
    let report = run_fsck(&db, &store_path)?;

    if json_output {
        println!(
            "{}",
            serde_json::json!({
                "clean": report.is_clean(),
                "total_issues": report.total_issues(),
                "missing_cas": report.missing_cas.iter().map(|h| h.as_hex()).collect::<Vec<_>>(),
                "dangling_aliases": report.dangling_aliases.iter().map(|a| &a.path).collect::<Vec<_>>(),
                "size_mismatches": report.size_mismatches.iter().map(|s| {
                    serde_json::json!({"hash": s.hash.as_hex(), "db_bytes": s.db_size, "disk_bytes": s.disk_size})
                }).collect::<Vec<_>>(),
                "orphan_cas_count": report.orphan_cas.len(),
            })
        );
        return Ok(());
    }

    if report.is_clean() {
        println!("{}", t("verify.clean").green().bold());
        return Ok(());
    }

    println!(
        "{} {}",
        "⚠".yellow().bold(),
        tf("verify.issues_found", &[("count", &report.total_issues())]).yellow().bold()
    );
    println!("{}", "─".repeat(50).yellow());

    if !report.missing_cas.is_empty() {
        println!(
            "\n  {} {}",
            "✗".red(),
            tf("verify.missing_cas", &[("count", &report.missing_cas.len())]).red().bold()
        );
        for h in &report.missing_cas {
            println!("    {}", h.as_hex()[..16].to_string().dimmed());
        }
    }

    if !report.dangling_aliases.is_empty() {
        println!(
            "\n  {} {}",
            "✗".red(),
            tf("verify.dangling_aliases", &[("count", &report.dangling_aliases.len())])
                .red()
                .bold()
        );
        for a in &report.dangling_aliases {
            println!("    {}", a.path.dimmed());
        }
    }

    if !report.size_mismatches.is_empty() {
        println!(
            "\n  {} {}",
            "⚠".yellow(),
            tf("verify.size_mismatches", &[("count", &report.size_mismatches.len())])
                .yellow()
                .bold()
        );
        for s in &report.size_mismatches {
            println!(
                "    {} db={} disk={}",
                s.hash.as_hex()[..16].to_string().dimmed(),
                format_bytes(s.db_size as u64),
                format_bytes(s.disk_size),
            );
        }
    }

    if !report.orphan_cas.is_empty() {
        println!(
            "\n  {} {}",
            "•".blue(),
            tf("verify.orphan_cas", &[("count", &report.orphan_cas.len())]).bold()
        );
    }

    println!();
    println!("{}", t("verify.fix_hint").dimmed());
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Unlink (undo-dedup) command
// ─────────────────────────────────────────────────────────────────────────────

fn unlink_command(store_path: PathBuf, path: PathBuf) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let mut db = Database::open(&db_path)?;

    println!("{}", tf("unlink.start", &[("path", &path.display())]).cyan());
    let result = unlink_path(&mut db, &store_path, &path)?;

    println!(
        "{} {}",
        "✓".green().bold(),
        tf("unlink.done", &[("path", &result.path.display())]).bold()
    );
    let size = format_bytes(result.size_bytes);
    println!("  {}", tf("unlink.was", &[("type", &result.previous_type.as_str())]).dimmed());
    println!("  {}", tf("unlink.size", &[("size", &size)]).dimmed());
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 5: proxy commands
// ─────────────────────────────────────────────────────────────────────────────

// ─── modeld serve ──────────────────────────────────────────────────────────

/// Start the embedded Web UI server.
///
/// If no auth token is configured one is auto-generated and printed to stdout.
/// The token value is **never** written to a log file.
fn serve_command(
    host: String,
    port: Option<u16>,
    open_browser: bool,
    store_path: PathBuf,
    _read_only: bool,
    allow_lan: bool,
) -> Result<()> {
    use colored::Colorize;

    let port = port.unwrap_or(8234);

    // Exposing the UI on the network must be an explicit decision, matching
    // `modeld-webui` and `modeld proxy start`.
    let loopback = modeld_proxy::config::is_loopback_addr(&host);
    if !loopback && !allow_lan {
        anyhow::bail!(
            "refusing to bind the Web UI to '{}': pass --allow-lan to expose it on the network \
             (a bearer token is always enforced)",
            host
        );
    }

    // Warn loudly when binding to all interfaces.
    if !loopback {
        println!(
            "{}",
            "⚠  Warning: the Web UI is reachable from the network; a bearer token is required."
                .yellow()
                .bold()
        );
        println!(
            "{}",
            "   Restrict access with a firewall if this machine is reachable from the internet."
                .yellow()
        );
        println!();
    }

    // Ensure the store is initialized.
    std::fs::create_dir_all(&store_path)?;
    let db_path = store_path.join("modeld.db");
    let db = Database::open(&db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;

    // Load existing config. Token resolution: `modeld.toml` → `MODELD_AUTH_TOKEN` →
    // generate one for this session. The env var is never written to disk, so a
    // container can inject a secret without it ending up in the store.
    let mut config = load_config(&store_path).unwrap_or_default();
    let env_token = std::env::var("MODELD_AUTH_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    let auth_token = if !config.auth.token.is_empty() {
        config.auth.token.clone()
    } else if let Some(token) = env_token {
        println!("{}", "🔑  Using the token from MODELD_AUTH_TOKEN.".cyan().bold());
        println!();
        token
    } else {
        let new_token = uuid::Uuid::new_v4().to_string();
        config.auth.token = new_token.clone();
        // Print to console only — NOT logged anywhere.
        println!(
            "{}",
            "🔑  No auth token configured. Auto-generated token for this session:".cyan().bold()
        );
        println!("    {}", new_token.yellow().bold());
        println!();
        println!("{}", "   Pass it in the Authorization header:".dimmed());
        println!("   {}", format!("Authorization: Bearer {}", new_token).dimmed());
        println!();
        new_token
    };

    let (state, _rx) = modeld_webui::AppState::new(db, store_path, Some(auth_token), None);

    let open_browser = open_browser || config.serve.open_browser;
    let webui_config = modeld_webui::WebUiConfig { host, port, open_browser };

    // Run the async server on a fresh Tokio runtime.
    tokio::runtime::Runtime::new()
        .context("Failed to create Tokio runtime")?
        .block_on(modeld_webui::run(state, webui_config))
}

// ─────────────────────────────────────────────────────────────────────────────

// ── Transaction management ────────────────────────────────────────────────────

// ── Database management ───────────────────────────────────────────────────────
