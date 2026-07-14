use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{
    add_tag, build_model_lookup, classify_refs, explain_refs, favorite_model, find_workflow_files,
    get_tags_for_model, hash_file, index_workflow, list_all_tags, list_orphans, list_pinned,
    load_config, parse_workflow, pin_model, remove_tag, resolve_store_path, run_doctor, run_fsck,
    save_config, scan_workflow_refs, set_note, t, tf, token_remove, token_set, token_status,
    unfavorite_model, unlink_path, unpin_model, Blake3Hash, CasStore, CheckStatus, Database,
    DedupEngine, DedupMode, DedupStrategy, Downloader, GcEngine, HfCache, QuarantineManager,
    RefStatus, ScanOptions, Scanner, TransactionManager, TxFilter,
};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "modeld")]
#[command(version)] // enables `--version` and `-V` from CARGO_PKG_VERSION
#[command(about = "Content-Addressable Storage for AI models", long_about = None)]
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

#[derive(Subcommand)]
enum ProxyAction {
    /// Start the proxy server (foreground; Ctrl+C to stop)
    Start {
        /// Bind address (default: 0.0.0.0)
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        /// Port (default: 8234)
        #[arg(long)]
        port: Option<u16>,
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
        /// Require a bearer token (set this to enable auth)
        #[arg(long)]
        token: Option<String>,
        /// Allow anonymous (unauthenticated) access (default: false for security)
        #[arg(long, default_value = "false")]
        allow_anonymous: bool,
        /// Path to a modeld.toml config file
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Discover modeld proxy servers on the local network (mDNS)
    Discover {
        /// How long to scan, in seconds (default: 5)
        #[arg(short = 't', long, default_value = "5")]
        timeout: u64,
    },
    /// Query a running proxy server's health and model stats
    Status {
        /// Proxy base URL (e.g. http://192.168.1.5:8234)
        #[arg(short = 'u', long, default_value = "http://localhost:8234")]
        url: String,
        /// Bearer token (if the server requires one)
        #[arg(long)]
        token: Option<String>,
    },
    /// Rotate (regenerate) the bearer token stored in modeld.toml
    TokenRotate {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum StoreAction {
    /// Print the resolved store path
    Locate {
        /// Store directory (if set, just prints this path)
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Initialize the store (same as `modeld init`)
    Init {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Verify store consistency
    Verify {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Migrate store from one location to another
    Migrate {
        /// Source store path
        #[arg(long)]
        from: PathBuf,
        /// Destination store path
        #[arg(long)]
        to: PathBuf,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Read a configuration key from modeld.toml
    Get {
        /// Key in dot notation, e.g. store.path, serve.port
        key: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Write a configuration value to modeld.toml
    Set {
        /// Key in dot notation, e.g. serve.port
        key: String,
        /// New value
        value: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

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

// ── HF subcommand tree ────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum HfAction {
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
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
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
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
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
enum HfCacheAction {
    /// List files in the HF cache
    List {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Verify integrity of HF cache entries
    Verify {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum HfTokenAction {
    /// Store a HuggingFace access token (reads from stdin)
    Set {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Remove the stored HuggingFace token
    Remove {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Show whether a token is currently stored
    Status {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

// ── Governance subcommand trees ───────────────────────────────────────────────

#[derive(Subcommand)]
enum TagAction {
    /// Attach a tag to a model
    Add {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Tag to add
        tag: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Remove a tag from a model
    Remove {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Tag to remove
        tag: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// List all tags with usage counts
    List {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum NoteAction {
    /// Set or clear the note for a model (empty text clears the note)
    Set {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Note text (empty string clears the note)
        text: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum PinAction {
    /// Pin a model (protect from GC)
    Add {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Unpin a model
    Remove {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// List all pinned models
    List {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FavoriteAction {
    /// Mark a model as favourite
    Add {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Remove the favourite flag from a model
    Remove {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum RefsAction {
    /// Parse a ComfyUI workflow JSON and index its model references
    Scan {
        /// Workflow JSON file (or directory of JSON files)
        path: PathBuf,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Explain why a model cannot be deleted
    Why {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// List models with no workflow references and no aliases (orphans)
    Orphans {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show reference graph: all models with their reference status
    Graph {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

// ── Transaction management ────────────────────────────────────────────────────

#[derive(Subcommand)]
enum TxAction {
    /// List transaction records
    List {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show details of a specific transaction
    Show {
        /// Transaction ID (UUID)
        tx_id: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Roll back a transaction
    Rollback {
        /// Transaction ID (UUID)
        tx_id: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Recover from a crash: mark PENDING transactions as FAILED and clean staging
    Recover {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Delete transaction records older than a given duration
    Cleanup {
        /// Age threshold, e.g. "30d", "7d", "1h"
        #[arg(long)]
        older_than: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

// ── Database management ───────────────────────────────────────────────────────

#[derive(Subcommand)]
enum DbAction {
    /// Show database status (schema version, file sizes, integrity)
    Status {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Backup the database to <store>/backups/
    Backup {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Restore the database from a backup file
    Restore {
        /// Path to the backup file
        backup: PathBuf,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// VACUUM the database to reclaim disk space
    Vacuum {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Run any pending schema migrations
    Migrate {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path, interactive } => init_command(path, interactive)?,
        Commands::Scan { path, store, incremental, full, exclude_globs, follow_symlinks } => {
            scan_command(
                path,
                resolve_store(store),
                incremental,
                full,
                exclude_globs,
                follow_symlinks,
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
        Commands::Serve { host, port, open, store, read_only } => {
            serve_command(host, port, open, resolve_store(store), read_only)?
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
fn resolve_store(store: Option<PathBuf>) -> PathBuf {
    resolve_store_path(store.as_deref())
}

fn require_store_db(store_path: &Path) -> Result<PathBuf> {
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

fn init_command(path: Option<PathBuf>, interactive: bool) -> Result<()> {
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

    let scanner = Scanner::new()
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
    let scan_id = Uuid::new_v4().to_string();
    let ingest_result = modeld_core::ingest_scan_results(&store_path, &mut db, &results, &scan_id)?;

    println!("\n{}", t("scan.complete").green().bold());
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

fn open_db(store: &std::path::Path) -> Result<Database> {
    // Use the same DB filename as every other command so HF downloads land in
    // the same metadata store that `scan`/`status`/`dedup`/`gc`/`workflow-scan`
    // read from. (Previously this used "index.db", which caused the HF path to
    // silently diverge from the rest of the pipeline.)
    let db_path = store.join("modeld.db");
    Database::open(&db_path)
}

/// `modeld hf-check <repo_id> <filename>` — check modeld cache for a HF file
fn hf_check_command(
    store: PathBuf,
    repo_id: &str,
    filename: &str,
    revision: &str,
    json_output: bool,
) -> Result<()> {
    let db = open_db(&store)?;
    let hf_cache = HfCache::new(&store);

    // Check snapshot path
    let snapshot_path = hf_cache.snapshot_file_path(repo_id, revision, filename);
    let found = snapshot_path.exists();

    if json_output {
        if found {
            println!(r#"{{"found": true, "path": "{}"}}"#, snapshot_path.display());
        } else {
            println!(r#"{{"found": false}}"#);
        }
    } else if found {
        println!(
            "{} {}",
            "✓".green().bold(),
            tf("hf.check.hit", &[("repo", &repo_id), ("file", &filename), ("rev", &revision)])
        );
        let p = snapshot_path.display().to_string();
        println!("  {}", tf("hf.check.path", &[("path", &p)]).dimmed());
    } else {
        println!(
            "{} {}",
            "✗".red(),
            tf("hf.check.miss", &[("repo", &repo_id), ("file", &filename), ("rev", &revision)])
        );
        std::process::exit(1);
    }

    drop(db);
    Ok(())
}

/// `modeld hf-download <repo_id> <filename>` — download from HF via modeld CAS
fn hf_download_command(
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

/// `modeld hf-setup` — configure HF_HOME environment variable
fn hf_setup_command(store: PathBuf, print_path: bool) -> Result<()> {
    let hf_cache = HfCache::new(&store);
    hf_cache.init()?;
    let hf_home = hf_cache.hf_home();

    if print_path {
        // Machine-readable: just print the path (used by Python hook)
        println!("{}", hf_home.display());
        return Ok(());
    }

    println!("{}", t("hf.setup.header").bold().cyan());
    println!();
    let p = hf_home.display().to_string();
    println!("{} {}", t("hf.setup.dir"), p.green());
    println!();
    println!("{}", t("hf.setup.activate").bold());
    println!();

    #[cfg(windows)]
    {
        let p = hf_home.display().to_string();
        println!("  {}", t("hf.setup.windows_ps").yellow());
        println!("  {}", tf("hf.setup.set_ps", &[("path", &p)]));
        println!();
        println!("  {}", t("hf.setup.windows_cmd").yellow());
        println!("  {}", tf("hf.setup.set_cmd", &[("path", &p)]));
    }

    #[cfg(unix)]
    {
        let p = hf_home.display().to_string();
        println!("  {}", t("hf.setup.unix").yellow());
        println!("  {}", tf("hf.setup.set_unix", &[("path", &p)]));
    }

    println!();
    println!("{}", t("hf.setup.hook_header").bold());
    println!("  {}", t("hf.setup.hook_pip"));
    println!("  {}", t("hf.setup.hook_use"));
    println!("  {}", t("hf.setup.hook_import"));

    Ok(())
}

/// `modeld hf-status` — show HF cache statistics
fn hf_status_command(store: PathBuf) -> Result<()> {
    let hf_cache = HfCache::new(&store);
    let db = open_db(&store)?;

    println!("{}", t("hf.status.header").bold().cyan());
    println!("{}", "─".repeat(40).dimmed());

    if !hf_cache.hf_home().exists() {
        println!("{}", t("hf.status.not_initialized").yellow());
        return Ok(());
    }

    let stats = hf_cache.stats()?;

    let p = hf_cache.hf_home().display().to_string();
    println!("  {} {}", t("hf.status.hf_home"), p.green());
    println!("  {}", tf("hf.status.repos", &[("count", &stats.total_repos)]).bold());
    println!("  {}", tf("hf.status.blobs", &[("count", &stats.total_blobs)]).bold());

    // List repos
    let repos = hf_cache.list_repos()?;
    if !repos.is_empty() {
        println!();
        println!("{}", t("hf.status.cached_repos").bold());
        for repo in &repos {
            println!("  {} {}", "•".cyan(), tf("hf.status.repo_line", &[("repo", repo)]).cyan());
        }
    }

    // Downloads from DB
    let downloads = db.list_downloads(None)?;
    let done_count =
        downloads.iter().filter(|d| matches!(d.status, modeld_core::DownloadStatus::Done)).count();

    if !downloads.is_empty() {
        println!();
        println!(
            "  {}",
            tf("hf.status.downloads", &[("total", &downloads.len()), ("done", &done_count)]).bold()
        );
    }

    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.2} {}", size, UNITS[unit_idx])
}

/// Escape a string for CSV output.  Fields containing commas, quotes, or
/// newlines are wrapped in double-quotes with internal quotes doubled.
fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 4: Workflow & Reference Graph commands
// ─────────────────────────────────────────────────────────────────────────────

fn workflow_scan_command(store: PathBuf, workflow_dir: PathBuf) -> Result<()> {
    let db_path = store.join("modeld.db");
    let db = Database::open(&db_path)?;

    if !workflow_dir.exists() {
        let p = workflow_dir.display().to_string();
        anyhow::bail!("{}", tf("error.workflow_dir_missing", &[("path", &p)]));
    }

    let p = workflow_dir.display().to_string();
    println!("{}", tf("wf.scan.scanning", &[("path", &p)]).bold());

    // Find all .json files
    let files = find_workflow_files(&workflow_dir)?;
    if files.is_empty() {
        println!("{}", t("wf.scan.no_files").yellow());
        return Ok(());
    }

    println!("  {}", tf("wf.scan.found", &[("count", &files.len())]).bold());
    println!();

    // Build model lookup from database
    let lookup = build_model_lookup(&db)?;
    let lookup_size = lookup.len();
    println!("  {}", tf("wf.scan.lookup", &[("count", &lookup_size)]).bold());
    println!();

    let pb = ProgressBar::new(files.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    let mut total_resolved = 0usize;
    let mut total_unresolved = 0usize;
    let mut errors = 0usize;

    for wf_path in &files {
        let fname = wf_path.file_name().unwrap_or_default().to_string_lossy();
        pb.set_message(fname.to_string());

        match index_workflow(&db, wf_path, &lookup) {
            Ok((_, resolved, unresolved)) => {
                total_resolved += resolved;
                total_unresolved += unresolved;
            }
            Err(e) => {
                errors += 1;
                let wp = wf_path.display().to_string();
                let es = format!("{:#}", e);
                pb.println(format!(
                    "  {}",
                    tf("wf.warn", &[("path", &wp), ("error", &es)]).yellow().bold()
                ));
            }
        }
        pb.inc(1);
    }

    pb.finish_and_clear();

    println!("{}", t("wf.scan.complete").green().bold());
    println!("  {}", tf("wf.scan.workflows", &[("count", &files.len())]).bold());
    println!("  {}", tf("wf.scan.resolved", &[("count", &total_resolved)]).green().bold());
    println!("  {}", tf("wf.scan.unresolved", &[("count", &total_unresolved)]).yellow().bold());
    if errors > 0 {
        println!("  {}", tf("wf.scan.errors", &[("count", &errors)]).red().bold());
    }

    Ok(())
}

fn workflow_deps_command(store: PathBuf, workflow_file: PathBuf) -> Result<()> {
    if !workflow_file.exists() {
        let p = workflow_file.display().to_string();
        anyhow::bail!("{}", tf("error.workflow_file_missing", &[("path", &p)]));
    }

    let db_path = store.join("modeld.db");

    let p = workflow_file.display().to_string();
    println!("{}", tf("wf.deps.workflow", &[("path", &p)]).bold());
    println!();

    let parsed = parse_workflow(&workflow_file)?;

    if let Some(ref title) = parsed.title {
        println!("  {}", tf("wf.deps.title", &[("title", title)]).bold());
        println!();
    }

    // Try to resolve against DB if available
    let lookup = if db_path.exists() {
        let db = Database::open(&db_path)?;
        build_model_lookup(&db)?
    } else {
        std::collections::HashMap::new()
    };

    // Print known refs
    let all_refs: Vec<_> = parsed.refs.iter().chain(parsed.unresolved.iter()).collect();

    if all_refs.is_empty() {
        println!("{}", t("wf.deps.no_refs").yellow());
        return Ok(());
    }

    println!("{}", t("wf.deps.header").bold());
    println!();

    // Group by ref_type
    let mut by_type: std::collections::BTreeMap<&str, Vec<_>> = std::collections::BTreeMap::new();
    for r in &all_refs {
        by_type.entry(r.ref_type.as_str()).or_default().push(r);
    }

    for (ref_type, refs) in &by_type {
        let type_key = format!("wf.deps.type.{}", ref_type);
        let type_label = t(&type_key);
        println!("  {}:", type_label.bold());
        for r in refs {
            let resolved = lookup.contains_key(&r.model_name)
                || lookup.contains_key(
                    std::path::Path::new(&r.model_name)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                        .as_str(),
                );
            let status = if resolved { "✓".green().bold() } else { "?".yellow() };
            println!("    {} {}", status, r.model_name);
        }
        println!();
    }

    println!(
        "  {}",
        tf(
            "wf.deps.total",
            &[
                ("count", &all_refs.len()),
                ("resolved", &parsed.refs.len()),
                ("unresolved", &parsed.unresolved.len()),
            ]
        )
        .bold(),
    );

    Ok(())
}

fn refs_orphans_command(store: PathBuf, json_output: bool) -> Result<()> {
    let db_path = store.join("modeld.db");
    if !db_path.exists() {
        anyhow::bail!("{}", t("store.not_initialized_exit"));
    }

    let db = Database::open(&db_path)?;
    let orphans = db.orphan_models()?;

    if json_output {
        let arr: Vec<serde_json::Value> = orphans
            .iter()
            .map(|m| {
                serde_json::json!({
                    "hash": m.blake3_hash.as_hex(),
                    "size_bytes": m.size_bytes,
                    "format": m.format,
                    "arch": m.arch,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    if orphans.is_empty() {
        println!("{}", t("orphans.none").green());
        return Ok(());
    }

    let total_size: i64 = orphans.iter().map(|m| m.size_bytes).sum();

    println!("{}", t("orphans.header").bold().yellow());
    println!();
    println!(
        "  {:<66} {:>10}  {}",
        t("orphans.col_hash").dimmed(),
        t("orphans.col_size").dimmed(),
        t("orphans.col_format").dimmed()
    );
    println!("  {}", "─".repeat(90).dimmed());

    for m in &orphans {
        let hash_str = &m.blake3_hash.as_hex()[..16];
        let size_str = format_bytes(m.size_bytes as u64);
        let fmt = m.format.as_deref().unwrap_or("unknown");
        println!("  {}...  {:>10}  {}", hash_str.yellow(), size_str, fmt);
    }

    println!();
    let size = format_bytes(total_size as u64);
    println!("  {}", tf("orphans.summary", &[("count", &orphans.len()), ("size", &size)]).bold());
    println!();
    println!("{}", t("orphans.tip").dimmed());

    Ok(())
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

fn verify_command(store_path: PathBuf, json_output: bool) -> Result<()> {
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
) -> Result<()> {
    use colored::Colorize;

    let port = port.unwrap_or(8234);

    // Warn loudly when binding to all interfaces.
    if host == "0.0.0.0" {
        println!(
            "{}",
            "⚠  Warning: binding to 0.0.0.0 exposes the Web UI to all network interfaces."
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

    // Load existing config; generate a token if none is configured.
    let mut config = load_config(&store_path).unwrap_or_default();
    let auth_token = if config.auth.token.is_empty() {
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
    } else {
        config.auth.token.clone()
    };

    let (state, _rx) = modeld_webui::AppState::new(db, store_path, Some(auth_token), None);

    let webui_config = modeld_webui::WebUiConfig { host, port, open_browser };

    // Run the async server on a fresh Tokio runtime.
    tokio::runtime::Runtime::new()
        .context("Failed to create Tokio runtime")?
        .block_on(modeld_webui::run(state, webui_config))
}

// ─────────────────────────────────────────────────────────────────────────────

fn proxy_command(action: ProxyAction) -> Result<()> {
    match action {
        ProxyAction::Start { bind, port, store, token, allow_anonymous, config } => {
            // `modeld proxy start` is a compatibility alias for the legacy proxy server.
            proxy_start_command(bind, port, store, token, allow_anonymous, config)
        }
        ProxyAction::Discover { timeout } => proxy_discover_command(timeout),
        ProxyAction::Status { url, token } => proxy_status_command(&url, token),
        ProxyAction::TokenRotate { store } => proxy_token_rotate_command(resolve_store(store)),
    }
}

fn proxy_start_command(
    bind: String,
    port: Option<u16>,
    store: PathBuf,
    token: Option<String>,
    allow_anonymous: bool,
    config: Option<PathBuf>,
) -> Result<()> {
    // Load config from file if provided; otherwise build from CLI flags.
    let mut cfg = if let Some(ref cfg_path) = config {
        modeld_proxy::ProxyConfig::load(cfg_path)?
    } else {
        modeld_proxy::ProxyConfig::from_cli(
            port,
            Some(bind.clone()),
            store.clone(),
            token,
            allow_anonymous,
        )
    };
    // CLI bind override always wins (most explicit).
    cfg.bind_address = bind;
    if let Some(p) = port {
        cfg.port = p;
    }

    // Initialize the store components if missing so a fresh store works.
    let cas = modeld_core::CasStore::new(&store);
    cas.init()?;
    let db_path = store.join("modeld.db");
    Database::open(&db_path)?;

    let server = modeld_proxy::ProxyServer::new(cfg, store);
    // mDNS announcement is best-effort; it logs the intended registration.
    // (Publication needs the host's mDNS daemon — see discovery.rs docs.)
    server.start()
}

fn proxy_discover_command(timeout: u64) -> Result<()> {
    println!("{}", t("proxy.discover.scanning").cyan().bold());
    println!("  {}", t("proxy.discover.service"));
    println!();

    let servers = modeld_proxy::discover(timeout);

    if servers.is_empty() {
        println!("{}", t("proxy.discover.none").yellow());
        println!();
        println!("{}", t("proxy.discover.make_sure").dimmed());
        println!("  {}", t("proxy.discover.tip_server"));
        println!("  {}", t("proxy.discover.tip_mdns"));
        println!("  {}", t("proxy.discover.tip_subnet"));
        println!("  {}", t("proxy.discover.tip_firewall"));
        return Ok(());
    }

    println!("{} {}", "✓".green().bold(), tf("proxy.discover.found", &[("count", &servers.len())]));
    println!();
    for s in &servers {
        let url = format!("http://{}:{}", s.address, s.port);
        println!("  {} {}", "•".cyan(), tf("proxy.discover.line", &[("url", &url)]).bold());
        if !s.txt.is_empty() {
            let parts: Vec<String> = s.txt.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            println!("      {}", parts.join(", ").dimmed());
        }
    }

    Ok(())
}

fn proxy_status_command(url: &str, token: Option<String>) -> Result<()> {
    let mut builder = modeld_client::ModeldClient::new(url);
    if let Some(t) = token {
        builder = builder.with_token(t);
    }

    let health = builder.health().map_err(|e| {
        let es = format!("{:#}", e);
        anyhow::anyhow!("{}", tf("proxy.status.reach_failed", &[("url", &url), ("error", &es)]))
    })?;

    println!("{}", t("proxy.status.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    println!("  {}", tf("proxy.status.url", &[("url", &url)]));
    println!("  {}", tf("proxy.status.status", &[("value", &health.status)]).green());
    println!("  {}", tf("proxy.status.version", &[("value", &health.version)]));
    println!("  {}", tf("proxy.status.uptime", &[("value", &health.uptime_seconds)]));
    println!("  {}", tf("proxy.status.models", &[("count", &health.model_count)]).bold());
    let size = format_bytes(health.total_bytes as u64);
    println!("  {}", tf("proxy.status.total_size", &[("value", &size)]));
    println!("{}", "─".repeat(40).cyan());

    match builder.list_models() {
        Ok(models) => {
            if models.is_empty() {
                println!("{}", t("proxy.status.no_models").dimmed());
            } else {
                println!();
                println!("{}", t("proxy.status.recent_header").cyan());
                for m in models.iter().take(5) {
                    let hash_short = &m.hash[..16.min(m.hash.len())];
                    let size = format_bytes(m.size_bytes as u64);
                    let fmt = m.format.as_deref().unwrap_or("unknown");
                    println!(
                        "  {}",
                        tf(
                            "proxy.status.recent_line",
                            &[("hash", &hash_short), ("size", &size), ("format", &fmt)]
                        )
                        .dimmed()
                    );
                }
                if models.len() > 5 {
                    let more = models.len() - 5;
                    println!("  {}", tf("proxy.status.more", &[("count", &more)]));
                }
            }
        }
        Err(e) => {
            let es = format!("{:#}", e);
            println!("  {} {}", "⚠".yellow(), tf("proxy.status.list_failed", &[("error", &es)]));
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// proxy token-rotate command
// ─────────────────────────────────────────────────────────────────────────────

fn proxy_token_rotate_command(store_path: PathBuf) -> Result<()> {
    let mut config = load_config(&store_path).unwrap_or_default();

    let new_token = Uuid::new_v4().to_string();
    config.auth.token = new_token.clone();
    save_config(&store_path, &config)?;

    println!("{}", "✓ Bearer token rotated successfully.".green().bold());
    println!("  {}", format!("New token: {}", new_token).bold());
    println!("  {}", "Restart the proxy server for the change to take effect.".dimmed());
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// doctor command
// ─────────────────────────────────────────────────────────────────────────────

fn doctor_command(store_path: PathBuf, json_output: bool) -> Result<()> {
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

fn store_command(action: StoreAction) -> Result<()> {
    match action {
        StoreAction::Locate { store } => {
            let path = resolve_store(store);
            println!("{}", path.display());
            Ok(())
        }
        StoreAction::Init { store } => {
            let path = resolve_store(store);
            init_command(Some(path), false)
        }
        StoreAction::Verify { store, json } => {
            let path = resolve_store(store);
            verify_command(path, json)
        }
        StoreAction::Migrate { from, to } => store_migrate_command(from, to),
    }
}

fn store_migrate_command(from: PathBuf, to: PathBuf) -> Result<()> {
    if !from.exists() {
        anyhow::bail!("Source store not found: {}", from.display());
    }

    println!(
        "{}",
        format!("Migrating store from {} to {}", from.display(), to.display()).cyan().bold()
    );

    // Simple migration: copy the entire store directory tree
    std::fs::create_dir_all(&to)
        .with_context(|| format!("Failed to create destination: {}", to.display()))?;

    copy_dir_all(&from, &to)?;

    println!("{}", "✓ Migration complete.".green().bold());
    println!("  {}", format!("New store: {}", to.display()).dimmed());
    println!(
        "  {}",
        "You can now update your MODELD_STORE or modeld.toml to point to the new location."
            .dimmed()
    );

    Ok(())
}

/// Recursively copy a directory tree (best-effort; skips CAS read-only files on Windows).
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src)
        .with_context(|| format!("Failed to read directory: {}", src.display()))?
    {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            std::fs::create_dir_all(&dst_path)?;
            copy_dir_all(&entry.path(), &dst_path)?;
        } else if ty.is_file() {
            // CAS objects are read-only; copy with overwrite
            if let Err(e) = std::fs::copy(entry.path(), &dst_path) {
                eprintln!("  {} skipping {}: {}", "⚠".yellow(), entry.path().display(), e);
            }
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// config command
// ─────────────────────────────────────────────────────────────────────────────

fn config_command(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Get { key, store } => {
            let store_path = resolve_store(store);
            let config = load_config(&store_path)?;
            let value = config_get_value(&config, &key)?;
            println!("{}", value);
            Ok(())
        }
        ConfigAction::Set { key, value, store } => {
            let store_path = resolve_store(store);
            let mut config = load_config(&store_path)?;
            config_set_value(&mut config, &key, &value)?;
            save_config(&store_path, &config)?;
            println!("{} {} = {}", "✓".green().bold(), key.bold(), value);
            Ok(())
        }
    }
}

fn config_get_value(config: &modeld_core::ModeldConfig, key: &str) -> Result<String> {
    match key {
        "store.path" => Ok(config
            .store
            .path
            .as_deref()
            .unwrap_or("<not set>")
            .to_string()),
        "serve.host" => Ok(config.serve.host.clone()),
        "serve.port" => Ok(config.serve.port.to_string()),
        "dedup.strategy" => Ok(config.dedup.strategy.clone()),
        "gc.quarantine_ttl_days" => Ok(config.gc.quarantine_ttl_days.to_string()),
        other => anyhow::bail!("Unknown config key: '{}'. Supported keys: store.path, serve.host, serve.port, dedup.strategy, gc.quarantine_ttl_days", other),
    }
}

/// Write a config value by dotted key.
fn config_set_value(config: &mut modeld_core::ModeldConfig, key: &str, value: &str) -> Result<()> {
    match key {
        "store.path" => {
            config.store.path = if value.is_empty() { None } else { Some(value.to_string()) };
        }
        "serve.host" => {
            config.serve.host = value.to_string();
        }
        "serve.port" => {
            config.serve.port = value
                .parse::<u16>()
                .with_context(|| format!("Invalid port number: {}", value))?;
        }
        "dedup.strategy" => {
            match value {
                "hardlink" | "symlink" | "copy_to_cas" | "virtual_alias" => {
                    config.dedup.strategy = value.to_string();
                }
                other => anyhow::bail!(
                    "Invalid dedup strategy '{}'. Valid: hardlink, symlink, copy_to_cas, virtual_alias",
                    other
                ),
            }
        }
        "gc.quarantine_ttl_days" => {
            config.gc.quarantine_ttl_days = value
                .parse::<u32>()
                .with_context(|| format!("Invalid TTL days: {}", value))?;
        }
        other => anyhow::bail!(
            "Unknown config key: '{}'. Supported keys: store.path, serve.host, serve.port, dedup.strategy, gc.quarantine_ttl_days",
            other
        ),
    }
    Ok(())
}

// =============================================================================
// `modeld hf` command — HuggingFace Hub integration
// =============================================================================

fn hf_command(action: HfAction) -> Result<()> {
    match action {
        HfAction::Download { repo_id, filename, revision, token, store } => {
            hf_subcommand_download(resolve_store(store), &repo_id, &filename, &revision, token)
        }
        HfAction::Snapshot { repo_id, revision, token, store } => {
            hf_subcommand_snapshot(resolve_store(store), &repo_id, &revision, token)
        }
        HfAction::Cache { action } => hf_cache_command(action),
        HfAction::Token { action } => hf_token_command(action),
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

fn hf_cache_command(action: HfCacheAction) -> Result<()> {
    match action {
        HfCacheAction::List { store } => {
            let store_path = resolve_store(store);
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
        HfCacheAction::Verify { store } => {
            let store_path = resolve_store(store);
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

fn hf_token_command(action: HfTokenAction) -> Result<()> {
    match action {
        HfTokenAction::Set { store } => {
            let store_path = resolve_store(store);
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
        HfTokenAction::Remove { store } => {
            let store_path = resolve_store(store);
            token_remove(&store_path)?;
            println!("{}", "Token removed.".yellow().bold());
            Ok(())
        }
        HfTokenAction::Status { store } => {
            let store_path = resolve_store(store);
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

// ── Tag management ────────────────────────────────────────────────────────────

fn tag_command(action: TagAction) -> Result<()> {
    match action {
        TagAction::Add { hash, tag, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            add_tag(&mut db, &h, &tag)?;
            println!("✓ Added tag '{}' to {}", tag, &hash[..16]);
        }
        TagAction::Remove { hash, tag, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            remove_tag(&mut db, &h, &tag)?;
            println!("✓ Removed tag '{}' from {}", tag, &hash[..16]);
        }
        TagAction::List { store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            let tags = list_all_tags(&db)?;
            if json {
                let j: Vec<_> =
                    tags.iter().map(|(t, c)| serde_json::json!({"tag": t, "count": c})).collect();
                println!("{}", serde_json::to_string_pretty(&j)?);
            } else {
                for (tag, count) in &tags {
                    println!("  {} ({})", tag, count);
                }
            }
        }
    }
    Ok(())
}

// ── Note management ───────────────────────────────────────────────────────────

fn note_command(action: NoteAction) -> Result<()> {
    match action {
        NoteAction::Set { hash, text, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            set_note(&mut db, &h, &text)?;
            println!("✓ Note set for {}", &hash[..16]);
        }
    }
    Ok(())
}

// ── Pin management ────────────────────────────────────────────────────────────

fn pin_command(action: PinAction) -> Result<()> {
    match action {
        PinAction::Add { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            pin_model(&mut db, &h)?;
            println!("✓ Pinned {}", &hash[..16]);
        }
        PinAction::Remove { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            unpin_model(&mut db, &h)?;
            println!("✓ Unpinned {}", &hash[..16]);
        }
        PinAction::List { store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            let pinned = list_pinned(&db)?;
            if json {
                let hashes: Vec<_> = pinned.iter().map(|h| h.as_hex()).collect();
                println!("{}", serde_json::to_string_pretty(&hashes)?);
            } else {
                for h in &pinned {
                    println!("  {}", h.as_hex());
                }
            }
        }
    }
    Ok(())
}

// ── Favorite management ───────────────────────────────────────────────────────

fn favorite_command(action: FavoriteAction) -> Result<()> {
    match action {
        FavoriteAction::Add { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            favorite_model(&mut db, &h)?;
            println!("✓ Favorited {}", &hash[..16]);
        }
        FavoriteAction::Remove { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            unfavorite_model(&mut db, &h)?;
            println!("✓ Unfavorited {}", &hash[..16]);
        }
    }
    Ok(())
}

// ── Reference graph commands ──────────────────────────────────────────────────

fn refs_command(action: RefsAction) -> Result<()> {
    match action {
        RefsAction::Scan { path, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let count = scan_workflow_refs(&mut db, &path)?;
            println!("✓ Indexed {} model references from {}", count, path.display());
        }
        RefsAction::Why { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            let reasons = explain_refs(&db, &h)?;
            if reasons.is_empty() {
                println!("  No references found — this model is an orphan candidate.");
            } else {
                for r in &reasons {
                    println!("  • {}", r);
                }
            }
        }
        RefsAction::Orphans { store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            let orphans = list_orphans(&db)?;
            if json {
                let hashes: Vec<_> = orphans.iter().map(|h| h.as_hex()).collect();
                println!("{}", serde_json::to_string_pretty(&hashes)?);
            } else {
                for h in &orphans {
                    println!("  {}", &h.as_hex()[..16]);
                }
            }
        }
        RefsAction::Graph { store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            refs_graph_command(&db, json)?;
        }
    }
    Ok(())
}

/// Display a reference graph for all models in the store.
///
/// Each model is shown with its RefStatus and alias / workflow-ref counts.
fn refs_graph_command(db: &Database, json_output: bool) -> Result<()> {
    let rows = db.get_gc_candidate_counts()?;

    if json_output {
        let entries: Vec<serde_json::Value> = rows
            .iter()
            .map(|(model, alias_count, wf_count)| {
                let status = classify_refs(db, &model.blake3_hash);
                let status_str = match status {
                    RefStatus::HardReference => "hard-reference",
                    RefStatus::SoftReference => "soft-reference",
                    RefStatus::Pinned => "pinned",
                    RefStatus::RecentlyUsed => "recently-used",
                    RefStatus::Unknown => "unknown",
                    RefStatus::OrphanCandidate => "orphan-candidate",
                };
                serde_json::json!({
                    "hash": model.blake3_hash.as_hex(),
                    "hash_prefix": &model.blake3_hash.as_hex()[..16],
                    "size_bytes": model.size_bytes,
                    "format": model.format,
                    "ref_status": status_str,
                    "alias_count": alias_count,
                    "workflow_ref_count": wf_count,
                    "pinned": model.pinned,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("{}", "No models in store.".yellow());
        return Ok(());
    }

    println!("{}", "Reference Graph".cyan().bold());
    println!("{}", "─".repeat(70).cyan());
    println!(
        "  {:<18}  {:<7}  {:<8}  {:<8}  {}",
        "Hash".dimmed(),
        "Size".dimmed(),
        "Aliases".dimmed(),
        "WfRefs".dimmed(),
        "Status".dimmed(),
    );
    println!("{}", "─".repeat(70).dimmed());

    // Sort by status severity: orphans first, then soft, then hard/pinned
    let mut sorted = rows;
    sorted.sort_by_key(|(model, _, _)| {
        let s = classify_refs(db, &model.blake3_hash);
        match s {
            RefStatus::OrphanCandidate => 0,
            RefStatus::SoftReference => 1,
            RefStatus::Unknown => 2,
            RefStatus::RecentlyUsed => 3,
            RefStatus::HardReference => 4,
            RefStatus::Pinned => 5,
        }
    });

    for (model, alias_count, wf_count) in &sorted {
        let status = classify_refs(db, &model.blake3_hash);
        let (status_str, colored_status) = match status {
            RefStatus::HardReference => ("hard-ref ", "hard-ref ".green().to_string()),
            RefStatus::SoftReference => ("soft-ref ", "soft-ref ".yellow().to_string()),
            RefStatus::Pinned => ("pinned   ", "pinned   ".cyan().to_string()),
            RefStatus::RecentlyUsed => ("recent   ", "recent   ".blue().to_string()),
            RefStatus::Unknown => ("unknown  ", "unknown  ".dimmed().to_string()),
            RefStatus::OrphanCandidate => ("orphan   ", "orphan   ".red().to_string()),
        };
        let _ = status_str;
        let size_str = format_bytes(model.size_bytes as u64);
        let hash_prefix = &model.blake3_hash.as_hex()[..16];
        println!(
            "  {}...  {:>7}  {:>8}  {:>8}  {}",
            hash_prefix, size_str, alias_count, wf_count, colored_status,
        );
    }

    println!("{}", "─".repeat(70).dimmed());
    println!("  Total: {} model(s)", sorted.len());
    Ok(())
}

// ── Transaction management ────────────────────────────────────────────────────

fn tx_command(action: TxAction) -> Result<()> {
    match action {
        TxAction::List { store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let tm = TransactionManager::new(&mut db, &store_path);
            let records = tm.list(TxFilter::default())?;
            if json {
                let j: Vec<_> = records
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "tx_id": r.tx_id,
                            "op_type": r.op_type.as_ref().map(|o| o.as_str()),
                            "status": format!("{:?}", r.status),
                            "start_time": r.start_time.to_rfc3339(),
                            "end_time": r.end_time.map(|t| t.to_rfc3339()),
                            "error": r.error_message,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&j)?);
            } else {
                for r in &records {
                    let op = r.op_type.as_ref().map(|o| o.as_str()).unwrap_or("unknown");
                    println!(
                        "  {} | {:?} | {} | {}",
                        &r.tx_id[..8],
                        r.status,
                        op,
                        r.start_time.format("%Y-%m-%d %H:%M")
                    );
                }
            }
        }
        TxAction::Show { tx_id, store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let tm = TransactionManager::new(&mut db, &store_path);
            let records = tm.list(TxFilter::default())?;
            let record = records.iter().find(|r| r.tx_id == tx_id || r.tx_id.starts_with(&tx_id));
            if let Some(r) = record {
                if json {
                    let obj = serde_json::json!({
                        "tx_id": r.tx_id,
                        "op_type": r.op_type.as_ref().map(|o| o.as_str()),
                        "status": format!("{:?}", r.status),
                        "start_time": r.start_time.to_rfc3339(),
                        "end_time": r.end_time.map(|t| t.to_rfc3339()),
                        "error": r.error_message,
                        "affected_paths": r.affected_paths.iter()
                            .map(|p| p.source.display().to_string())
                            .collect::<Vec<_>>(),
                    });
                    println!("{}", serde_json::to_string_pretty(&obj)?);
                } else {
                    println!("tx_id:  {}", r.tx_id);
                    println!("status: {:?}", r.status);
                    println!(
                        "op:     {}",
                        r.op_type.as_ref().map(|o| o.as_str()).unwrap_or("unknown")
                    );
                    println!("start:  {}", r.start_time);
                    if let Some(end) = r.end_time {
                        println!("end:    {}", end);
                    }
                    if let Some(err) = &r.error_message {
                        println!("error:  {}", err);
                    }
                }
            } else {
                anyhow::bail!("Transaction not found: {}", tx_id);
            }
        }
        TxAction::Rollback { tx_id, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let mut tm = TransactionManager::new(&mut db, &store_path);
            tm.rollback(&tx_id)?;
            println!("✓ Rolled back transaction {}", &tx_id[..8.min(tx_id.len())]);
        }
        TxAction::Recover { store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let mut tm = TransactionManager::new(&mut db, &store_path);
            let results = tm.recover()?;
            println!("✓ Recovered {} transaction(s)", results.len());
        }
        TxAction::Cleanup { older_than, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let mut tm = TransactionManager::new(&mut db, &store_path);
            let duration = parse_duration_str(&older_than)?;
            let removed = tm.cleanup_older_than(duration)?;
            println!("✓ Removed {} transaction record(s)", removed);
        }
    }
    Ok(())
}

fn parse_duration_str(s: &str) -> Result<std::time::Duration> {
    let s = s.trim();
    if let Some(days) = s.strip_suffix('d') {
        let n: u64 = days.parse().with_context(|| format!("Invalid duration: {}", s))?;
        return Ok(std::time::Duration::from_secs(n * 86400));
    }
    if let Some(hours) = s.strip_suffix('h') {
        let n: u64 = hours.parse().with_context(|| format!("Invalid duration: {}", s))?;
        return Ok(std::time::Duration::from_secs(n * 3600));
    }
    anyhow::bail!("Invalid duration '{}': use format like '30d' or '24h'", s);
}

// ── Database management ───────────────────────────────────────────────────────

fn db_command(action: DbAction) -> Result<()> {
    match action {
        DbAction::Status { store } => {
            let store_path = resolve_store(store);
            let db_path = store_path.join("modeld.db");
            if !db_path.exists() {
                anyhow::bail!("Database not found at {}", db_path.display());
            }
            let db = Database::open(&db_path)?;
            let version = db.schema_version()?;
            let db_size = std::fs::metadata(&db_path)?.len();
            let wal_path = db_path.with_extension("db-wal");
            let wal_size = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);
            let wal_active = wal_path.exists();
            println!("schema version : {}", version);
            println!("db size        : {} KB", db_size / 1024);
            println!(
                "wal status     : {} ({} KB)",
                if wal_active { "active" } else { "inactive" },
                wal_size / 1024
            );
            let integrity_result = db.integrity_check().unwrap_or_else(|_| "ERROR".to_string());
            let integrity_ok = integrity_result.eq_ignore_ascii_case("ok");
            println!("integrity      : {}", if integrity_ok { "OK" } else { &integrity_result });
        }
        DbAction::Backup { store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let backups_dir = store_path.join("backups");
            std::fs::create_dir_all(&backups_dir)?;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let backup_path = backups_dir.join(format!("modeld_backup_{}.db", ts));
            std::fs::copy(&db_path, &backup_path)?;
            println!("✓ Database backed up to {}", backup_path.display());
        }
        DbAction::Restore { backup, store, yes } => {
            if !backup.exists() {
                anyhow::bail!("Backup file not found: {}", backup.display());
            }
            let store_path = resolve_store(store);
            let db_path = store_path.join("modeld.db");
            if !yes {
                print!("Replace {} with {}? [y/N]: ", db_path.display(), backup.display());
                std::io::Write::flush(&mut std::io::stdout())?;
                let mut ans = String::new();
                std::io::stdin().read_line(&mut ans)?;
                if !ans.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            std::fs::copy(&backup, &db_path)?;
            println!("✓ Database restored from {}", backup.display());
        }
        DbAction::Vacuum { store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            db.vacuum()?;
            println!("✓ Database vacuumed");
        }
        DbAction::Migrate { store } => {
            let store_path = resolve_store(store);
            let db_path = store_path.join("modeld.db");
            if !db_path.exists() {
                anyhow::bail!("Database not found at {}", db_path.display());
            }
            // Read the version before migrations run.
            let version_before = Database::read_schema_version_raw(&db_path)?;
            // Opening the database triggers any pending migrations automatically.
            let db = Database::open(&db_path)?;
            let version_after = db.schema_version()?;
            let latest = Database::latest_schema_version();
            if version_before < version_after {
                println!(
                    "✓ Migrated database from schema v{} to v{}",
                    version_before, version_after
                );
            } else if version_after >= latest {
                println!("✓ Database is already at the latest schema version (v{})", version_after);
            } else {
                println!("  Database is at schema version {} (latest: {})", version_after, latest);
            }
        }
    }
    Ok(())
}
