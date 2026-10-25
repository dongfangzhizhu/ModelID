mod commands;
mod output;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use commands::config::{config_command, ConfigAction};
use commands::db::{db_command, DbAction};
use commands::dedup::dedup_command;
use commands::gc::gc_command;
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
use commands::query::{dupes_command, info_command, list_command, stats_command, status_command};
use commands::repair::{quarantine_command, unlink_command, verify_command, QuarantineAction};
use commands::scan::scan_command;
use commands::serve::serve_command;
use commands::store::{store_command, StoreAction};
use commands::tx::{tx_command, TxAction};
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{
    hash_file, load_config, resolve_store_path, run_doctor, save_config, t, tf, token_remove,
    token_set, token_status, CasStore, CheckStatus, Database, Downloader, IngestMode,
    QuarantineManager,
};
use output::{ExitCode, EXIT_CODE_HELP};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "modeld")]
#[command(version)] // enables `--version` and `-V` from CARGO_PKG_VERSION
#[command(about = "Content-Addressable Storage for AI models", long_about = None)]
#[command(after_help = EXIT_CODE_HELP)]
struct Cli {
    /// Store directory (overrides MODELD_STORE and modeld.toml)
    #[arg(short = 's', long, global = true, value_name = "PATH")]
    store: Option<PathBuf>,
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
    Status {},
    /// Show store statistics and duplicate-space summary
    Stats {},
    /// Report duplicate model files
    Dupes {
        /// Minimum duplicate file size to include, e.g. 100MB, 2GB
        #[arg(long)]
        min_size: Option<String>,
        /// Output result as JSON
        #[arg(long)]
        json: bool,
    },
    /// List indexed models
    List {
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
    },
    /// Configure HF_HOME to point to the modeld HF cache
    HfSetup {
        /// Only print the HF_HOME path without making changes
        #[arg(long)]
        print_path: bool,
    },
    /// Show HuggingFace cache statistics
    HfStatus {},
    /// Scan workflow files and index model dependencies
    WorkflowScan {
        /// Directory containing workflow JSON files
        path: PathBuf,
    },
    /// Show model dependencies of a single workflow file
    WorkflowDeps {
        /// Workflow JSON file
        file: PathBuf,
    },
    /// List models with no workflow references (orphans)
    RefsOrphans {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Safe garbage collection: quarantine unreferenced models
    Gc {
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
        /// Output result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Restore a hardlinked/symlinked file to an independent physical copy
    Unlink {
        /// File path to restore (must have a hardlink/symlink alias in the DB)
        path: PathBuf,
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
    // `--store` is a global flag: resolve it once and hand a clone to whichever
    // command runs, so no subcommand has to declare it.
    let store = cli.store.clone();

    match cli.command {
        Commands::Init { path, interactive } => init_command(path, interactive)?,
        Commands::Scan {
            path,
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
                resolve_store(store.clone()),
                incremental,
                full,
                exclude_globs,
                follow_symlinks,
                mode,
            )?
        }
        Commands::Status {} => status_command(resolve_store(store.clone()))?,
        Commands::Stats {} => stats_command(resolve_store(store.clone()))?,
        Commands::Dupes { min_size, json } => {
            dupes_command(resolve_store(store.clone()), min_size, json)?
        }
        Commands::List { limit, json, export_csv } => {
            list_command(resolve_store(store.clone()), limit, json, export_csv)?
        }
        Commands::Info { hash, json } => info_command(resolve_store(store.clone()), &hash, json)?,
        Commands::Hash { file } => hash_command(file)?,
        Commands::Dedup {
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
            resolve_store(store.clone()),
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
        Commands::Quarantine { action } => {
            quarantine_command(resolve_store(store.clone()), action)?
        }
        Commands::HfCheck { repo_id, filename, revision, json } => {
            hf_check_command(resolve_store(store.clone()), &repo_id, &filename, &revision, json)?
        }
        Commands::HfDownload { repo_id, filename, revision, token, json } => hf_download_command(
            resolve_store(store.clone()),
            &repo_id,
            &filename,
            &revision,
            token,
            json,
        )?,
        Commands::HfSetup { print_path } => {
            hf_setup_command(resolve_store(store.clone()), print_path)?
        }
        Commands::HfStatus {} => hf_status_command(resolve_store(store.clone()))?,
        Commands::WorkflowScan { path } => {
            workflow_scan_command(resolve_store(store.clone()), path)?
        }
        Commands::WorkflowDeps { file } => {
            workflow_deps_command(resolve_store(store.clone()), file)?
        }
        Commands::RefsOrphans { json } => refs_orphans_command(resolve_store(store.clone()), json)?,
        Commands::Gc { preview, cleanup_quarantine, cleanup_tmp } => {
            gc_command(resolve_store(store.clone()), preview, cleanup_quarantine, cleanup_tmp)?
        }
        Commands::Verify { json } => verify_command(resolve_store(store.clone()), json)?,
        Commands::Unlink { path } => unlink_command(resolve_store(store.clone()), path)?,
        Commands::Proxy { action } => proxy_command(action, store.clone())?,
        Commands::Doctor { json } => doctor_command(resolve_store(store.clone()), json)?,
        Commands::Store { action } => store_command(action, store.clone())?,
        Commands::Config { action } => config_command(action, store.clone())?,
        Commands::Hf { action } => hf_command(action, store.clone())?,
        Commands::Serve { host, port, open, read_only, allow_lan } => {
            serve_command(host, port, open, resolve_store(store.clone()), read_only, allow_lan)?
        }
        Commands::Tag { action } => tag_command(action, store.clone())?,
        Commands::Note { action } => note_command(action, store.clone())?,
        Commands::Pin { action } => pin_command(action, store.clone())?,
        Commands::Favorite { action } => favorite_command(action, store.clone())?,
        Commands::Refs { action } => refs_command(action, store.clone())?,
        Commands::Tx { action } => tx_command(action, store.clone())?,
        Commands::Db { action } => db_command(action, store.clone())?,
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

pub(crate) fn parse_size(input: &str) -> Result<u64> {
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

pub(crate) fn open_db(store: &std::path::Path) -> Result<Database> {
    // Use the same DB filename as every other command so HF downloads land in
    // the same metadata store that `scan`/`status`/`dedup`/`gc`/`workflow-scan`
    // read from. (Previously this used "index.db", which caused the HF path to
    // silently diverge from the rest of the pipeline.)
    let db_path = store.join("modeld.db");
    Database::open(&db_path)
}
