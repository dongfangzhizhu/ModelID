//! The clap tree: every command, subcommand and flag.
//!
//! Kept separate from `main.rs` so the dispatch and the helpers stay readable;
//! the nested action enums live in `commands/*.rs` next to their handlers.

use crate::commands::config::ConfigAction;
use crate::commands::db::DbAction;
use crate::commands::hf::HfAction;
use crate::commands::meta::{FavoriteAction, NoteAction, PinAction, RefsAction, TagAction};
use crate::commands::proxy::ProxyAction;
use crate::commands::repair::QuarantineAction;
use crate::commands::store::StoreAction;
use crate::commands::tx::TxAction;
use crate::output::EXIT_CODE_HELP;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "modeld")]
#[command(version)] // enables `--version` and `-V` from CARGO_PKG_VERSION
#[command(about = "Content-Addressable Storage for AI models", long_about = None)]
#[command(after_help = EXIT_CODE_HELP)]
pub struct Cli {
    /// Store directory (overrides MODELD_STORE and modeld.toml)
    #[arg(short = 's', long, global = true, value_name = "PATH")]
    pub store: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
