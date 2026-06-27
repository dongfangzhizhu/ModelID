use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{
    build_model_lookup, find_workflow_files, hash_file, index_workflow, parse_workflow, run_fsck,
    run_doctor, load_config, save_config, resolve_store_path,
    t, tf, unlink_path, CasStore, CheckStatus, Database, DedupEngine, DedupMode, Downloader,
    GcEngine, HfCache, QuarantineManager, Scanner,
};
use std::path::{Path, PathBuf};

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
        /// Auto mode - execute without confirmation prompts
        #[arg(long)]
        auto: bool,
        /// Report mode - analyze only, no modifications
        #[arg(long)]
        report: bool,
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
    Cleanup,
    /// Restore a quarantined file to its original location
    Restore {
        /// Full path to the quarantined file (shown by `quarantine list`)
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path, interactive } => init_command(path, interactive)?,
        Commands::Scan { path, store } => scan_command(path, resolve_store(store))?,
        Commands::Status { store } => status_command(resolve_store(store))?,
        Commands::Stats { store } => stats_command(resolve_store(store))?,
        Commands::Dupes { store, min_size, json } => dupes_command(resolve_store(store), min_size, json)?,
        Commands::List { store, limit, json } => list_command(resolve_store(store), limit, json)?,
        Commands::Info { hash, store, json } => info_command(resolve_store(store), &hash, json)?,
        Commands::Hash { file } => hash_command(file)?,
        Commands::Dedup { store, dry_run, auto, report } => {
            dedup_command(resolve_store(store), dry_run, auto, report)?
        }
        Commands::Quarantine { store, action } => quarantine_command(resolve_store(store), action)?,
        Commands::HfCheck { repo_id, filename, revision, json, store } => {
            hf_check_command(resolve_store(store), &repo_id, &filename, &revision, json)?
        }
        Commands::HfDownload { repo_id, filename, revision, token, json, store } => {
            hf_download_command(resolve_store(store), &repo_id, &filename, &revision, token, json)?
        }
        Commands::HfSetup { print_path, store } => hf_setup_command(resolve_store(store), print_path)?,
        Commands::HfStatus { store } => hf_status_command(resolve_store(store))?,
        Commands::WorkflowScan { path, store } => workflow_scan_command(resolve_store(store), path)?,
        Commands::WorkflowDeps { file, store } => workflow_deps_command(resolve_store(store), file)?,
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
        println!(
            "{}",
            format!("⚠  Store already exists at: {}", store_path.display()).yellow()
        );
        println!("{}", "   Re-initializing will add missing components but won't delete data.".dimmed());
        println!();
    }

    println!(
        "{}",
        tf("init.at", &[("path", &store_path.display())]).green().bold()
    );

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
        println!(
            "{}",
            format!("Store path [{}]: ", store_path.display()).bold()
        );
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

fn scan_command(scan_path: PathBuf, store_path: PathBuf) -> Result<()> {
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
        .with_preindexed(preindexed);

    // Quick count first
    let (file_count, total_size) = scanner.count_files(&scan_path)?;

    if file_count == 0 {
        println!("{}", t("scan.no_files").yellow());
        return Ok(());
    }

    let gb = format!("{:.2}", total_size as f64 / 1_073_741_824.0);
    println!(
        "{}\n",
        tf("scan.found", &[("count", &file_count), ("gb", &gb)]).bold()
    );
    if cached_count > 0 {
        println!(
            "  {}",
            tf("scan.incremental", &[("count", &cached_count)]).dimmed()
        );
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

    // Store in CAS and database
    println!("\n{}", t("scan.processing").cyan().bold());
    let mut processed = 0;
    for file in &results {
        // Store in CAS
        cas.store(&file.path, &file.hash)?;

        // Record in database
        db.insert_or_update_model(&file.hash, file.size as i64, None, None, None, None)?;

        // Record original alias
        let path_str = file.path.to_string_lossy().to_string();
        if db.get_alias_by_path(&path_str)?.is_none() {
            db.insert_alias(
                &file.hash,
                &path_str,
                modeld_core::db::Frontend::User,
                modeld_core::db::AliasType::Original,
            )?;
        }

        processed += 1;
    }

    println!("\n{}", t("scan.complete").green().bold());
    let gb = format!("{:.2}", total_size as f64 / 1_073_741_824.0);
    println!("  {}", tf("scan.processed", &[("count", &processed)]).bold());
    println!("  {}", tf("scan.total_size", &[("gb", &gb)]));

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
                println!(
                    "  {}",
                    tf("status.q_expired", &[("count", &qstats.expired_files)]).red()
                );
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

fn list_command(store_path: PathBuf, limit: Option<i64>, json_output: bool) -> Result<()> {
    let db_path = require_store_db(&store_path)?;
    let db = Database::open(&db_path)?;
    let models = db.list_models(limit)?;

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
            })
        );
        return Ok(());
    }

    println!("{}", t("info.header").cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    let hash = model.blake3_hash.as_hex().to_string();
    println!("  {}", tf("info.hash", &[("hash", &hash)]).bold());
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
                tf(
                    "info.alias_line",
                    &[("path", &path), ("frontend", &frontend), ("kind", &kind)]
                )
            );
        }
    }

    Ok(())
}

fn hash_command(file: PathBuf) -> Result<()> {
    if !file.exists() {
        let p = file.display().to_string();
        eprintln!("{} {}", t("error.prefix").red().bold(), tf("error.file_not_found", &[("path", &p)]));
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

fn dedup_command(store_path: PathBuf, dry_run: bool, auto: bool, report: bool) -> Result<()> {
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

    let db = Database::open(&db_path)?;
    let mut engine = DedupEngine::new(db, store_path.clone());

    // First, find and report duplicates
    let groups = engine.find_duplicates()?;

    if groups.is_empty() {
        println!("\n{}", t("dedup.no_dupes").green());
        return Ok(());
    }

    let total_savings = engine.calculate_savings(&groups);

    println!(
        "\n{} {}",
        "→".cyan(),
        tf("dedup.groups_found", &[("count", &groups.len())]).bold()
    );
    let gb = format!("{:.2}", total_savings as f64 / 1_073_741_824.0);
    println!("{} {}", "→".cyan(), tf("dedup.potential_savings", &[("gb", &gb)]));

    // Show each group
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
        let saved = (group.total_size * (group.files.len() as u64 - 1)) as f64 / 1_048_576.0;
        let saved = format!("{:.2}", saved);
        println!("    {} {}", "→".green(), tf("dedup.would_save", &[("mb", &saved)]));
    }

    if mode == DedupMode::DryRun || mode == DedupMode::Report {
        println!("\n{}", "─".repeat(40).cyan());
        let gb = format!("{:.2}", total_savings as f64 / 1_073_741_824.0);
        println!("  {} {}", "✓".green(), tf("dedup.total_potential", &[("gb", &gb)]));
        println!("{}", t("dedup.run_without_dry_run"));
        return Ok(());
    }

    // Executing for real: require explicit y/N confirmation to prevent
    // accidental data-modifying runs (audit item 1.6).
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

    // Execute deduplication with progress
    println!("\n{}", t("dedup.executing").cyan().bold());

    let pb = ProgressBar::new(groups.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let stats = engine.run_dedup(mode, |current, total, hash_prefix| {
        pb.set_message(tf(
            "progress.processing",
            &[("prefix", &hash_prefix), ("current", &current), ("total", &total)],
        ));
        pb.set_position((current as u64).saturating_sub(1));
    })?;

    pb.finish_with_message(t("progress.done"));

    // Final report
    println!("\n{}", "─".repeat(40).cyan());
    println!("{}", t("dedup.complete").green().bold());
    println!("  {}", tf("dedup.groups_processed", &[("count", &stats.groups_processed)]).bold());
    println!(
        "  {}",
        tf("dedup.groups_succeeded", &[("count", &stats.groups_succeeded)]).green().bold()
    );
    if stats.groups_failed > 0 {
        println!("  {}", tf("dedup.groups_failed", &[("count", &stats.groups_failed)]).red().bold());
    }
    println!("  {}", tf("dedup.files_dedup", &[("count", &stats.files_deduplicated)]).bold());
    let gb = format!("{:.2}", stats.space_saved as f64 / 1_073_741_824.0);
    println!(
        "  {}",
        tf("dedup.space_saved", &[("gb", &gb)]).green().bold()
    );
    println!("{}", "─".repeat(40).cyan());

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
                    Some(d) => tf("quarantine.days_remaining", &[("count", &d)]).yellow().to_string(),
                    None => t("quarantine.expired").red().to_string(),
                };
                let qpath = entry.quarantine_path.display().to_string();

                println!("\n  {}", tf("quarantine.entry_header", &[("path", &qpath), ("status", &status)]).bold());
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

        QuarantineAction::Cleanup => {
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
                tf("quarantine.restore.done", &[("path", &restored.display().to_string())])
                    .bold()
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
            tf("hf.status.downloads", &[("total", &downloads.len()), ("done", &done_count)])
                .bold()
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
    println!(
        "  {}",
        tf("orphans.summary", &[("count", &orphans.len()), ("size", &size)]).bold()
    );
    println!();
    println!("{}", t("orphans.tip").dimmed());

    Ok(())
}

fn gc_command(store: PathBuf, preview: bool, cleanup_quarantine: bool, cleanup_tmp: bool) -> Result<()> {
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
                    tf("gc.preview.quarantine_line", &[("prefix", &item.hash_prefix), ("size", &size)])
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
            println!(
                "  {}",
                tf("gc.cleanup_done", &[("count", &cleaned)]).green().bold()
            );
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
            println!(
                "  {}",
                tf("gc.cleanup_tmp_done", &[("count", &removed)]).green().bold()
            );
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

    println!("{}", t("verify.running").cyan().bold());
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
        println!("\n  {} {}", "✗".red(), tf("verify.missing_cas", &[("count", &report.missing_cas.len())]).red().bold());
        for h in &report.missing_cas {
            println!("    {}", h.as_hex()[..16].to_string().dimmed());
        }
    }

    if !report.dangling_aliases.is_empty() {
        println!("\n  {} {}", "✗".red(), tf("verify.dangling_aliases", &[("count", &report.dangling_aliases.len())]).red().bold());
        for a in &report.dangling_aliases {
            println!("    {}", a.path.dimmed());
        }
    }

    if !report.size_mismatches.is_empty() {
        println!("\n  {} {}", "⚠".yellow(), tf("verify.size_mismatches", &[("count", &report.size_mismatches.len())]).yellow().bold());
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
        println!("\n  {} {}", "•".blue(), tf("verify.orphan_cas", &[("count", &report.orphan_cas.len())]).bold());
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

fn proxy_command(action: ProxyAction) -> Result<()> {
    match action {
        ProxyAction::Start { bind, port, store, token, allow_anonymous, config } => {
            proxy_start_command(bind, port, store, token, allow_anonymous, config)
        }
        ProxyAction::Discover { timeout } => proxy_discover_command(timeout),
        ProxyAction::Status { url, token } => proxy_status_command(&url, token),
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

    println!(
        "{} {}",
        "✓".green().bold(),
        tf("proxy.discover.found", &[("count", &servers.len())])
    );
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
    println!(
        "  version      : {}",
        report.version
    );
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
        StoreAction::Migrate { from, to } => {
            store_migrate_command(from, to)
        }
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
    println!(
        "  {}",
        format!("New store: {}", to.display()).dimmed()
    );
    println!(
        "  {}",
        "You can now update your MODELD_STORE or modeld.toml to point to the new location.".dimmed()
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
                eprintln!(
                    "  {} skipping {}: {}",
                    "⚠".yellow(),
                    entry.path().display(),
                    e
                );
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
            println!(
                "{} {} = {}",
                "✓".green().bold(),
                key.bold(),
                value
            );
            Ok(())
        }
    }
}

/// Read a config value by dotted key.
///
/// Supported keys:
/// - `store.path`
/// - `serve.host`, `serve.port`
/// - `dedup.strategy`
/// - `gc.quarantine_ttl_days`
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
fn config_set_value(
    config: &mut modeld_core::ModeldConfig,
    key: &str,
    value: &str,
) -> Result<()> {
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
