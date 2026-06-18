use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{
    build_model_lookup, find_workflow_files, hash_file, index_workflow, parse_workflow,
    CasStore, Database, DedupEngine, DedupMode,
    Downloader, GcEngine, HfCache, QuarantineManager, Scanner,
};
use std::path::PathBuf;

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
        /// Store directory (default: ~/.local/share/modeld)
        #[arg(short, long)]
        path: Option<PathBuf>,
    },
    /// Scan a directory for model files
    Scan {
        /// Directory to scan
        path: PathBuf,
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
    },
    /// Show store statistics
    Status {
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
    },
    /// Compute BLAKE3 hash of a file
    Hash {
        /// File to hash
        file: PathBuf,
    },
    /// Deduplicate model files
    Dedup {
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
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
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
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
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
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
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
    },
    /// Configure HF_HOME to point to the modeld HF cache
    HfSetup {
        /// Only print the HF_HOME path without making changes
        #[arg(long)]
        print_path: bool,
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
    },
    /// Show HuggingFace cache statistics
    HfStatus {
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
    },
    /// Scan workflow files and index model dependencies
    WorkflowScan {
        /// Directory containing workflow JSON files
        path: PathBuf,
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
    },
    /// Show model dependencies of a single workflow file
    WorkflowDeps {
        /// Workflow JSON file
        file: PathBuf,
        /// Store directory (optional, for resolved hash lookup)
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
    },
    /// List models with no workflow references (orphans)
    RefsOrphans {
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Safe garbage collection: quarantine unreferenced models
    Gc {
        /// Store directory
        #[arg(short = 's', long, default_value = ".modeld")]
        store: PathBuf,
        /// Preview what would be collected without making changes
        #[arg(long)]
        preview: bool,
        /// Also clean up expired quarantine entries (>30 days)
        #[arg(long)]
        cleanup_quarantine: bool,
    },
    /// Local registry & proxy server (Phase 5)
    Proxy {
        #[command(subcommand)]
        action: ProxyAction,
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
        /// Allow anonymous (unauthenticated) access
        #[arg(long, default_value = "true")]
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
enum QuarantineAction {
    /// List quarantined files
    List,
    /// Clean up expired quarantine entries (>30 days)
    Cleanup,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path } => init_command(path)?,
        Commands::Scan { path, store } => scan_command(path, store)?,
        Commands::Status { store } => status_command(store)?,
        Commands::Hash { file } => hash_command(file)?,
        Commands::Dedup {
            store,
            dry_run,
            auto,
            report,
        } => dedup_command(store, dry_run, auto, report)?,
        Commands::Quarantine { store, action } => quarantine_command(store, action)?,
        Commands::HfCheck {
            repo_id, filename, revision, json, store,
        } => hf_check_command(store, &repo_id, &filename, &revision, json)?,
        Commands::HfDownload {
            repo_id, filename, revision, token, json, store,
        } => hf_download_command(store, &repo_id, &filename, &revision, token, json)?,
        Commands::HfSetup { print_path, store } => hf_setup_command(store, print_path)?,
        Commands::HfStatus { store } => hf_status_command(store)?,
        Commands::WorkflowScan { path, store } => workflow_scan_command(store, path)?,
        Commands::WorkflowDeps { file, store } => workflow_deps_command(store, file)?,
        Commands::RefsOrphans { store, json } => refs_orphans_command(store, json)?,
        Commands::Gc { store, preview, cleanup_quarantine } =>
            gc_command(store, preview, cleanup_quarantine)?,
        Commands::Proxy { action } => proxy_command(action)?,
    }

    Ok(())
}

fn init_command(path: Option<PathBuf>) -> Result<()> {
    let store_path = path.unwrap_or_else(|| {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .expect("Could not determine home directory");
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("modeld")
    });

    println!(
        "{} {}",
        "Initializing modeld store at:".green().bold(),
        store_path.display()
    );

    // Initialize CAS
    let cas = CasStore::new(&store_path);
    cas.init()?;

    // Initialize database
    let db_path = store_path.join("modeld.db");
    Database::open(&db_path)?;

    // Initialize quarantine directory
    let qm = QuarantineManager::new(&store_path);
    qm.init()?;

    println!("{}", "✓ Store initialized successfully".green());
    println!("\nNext steps:");
    println!("  {} modeld scan <directory>", "1.".bold());
    println!("  {} modeld status", "2.".bold());
    println!("  {} modeld dedup --dry-run", "3.".bold());

    Ok(())
}

fn scan_command(scan_path: PathBuf, store_path: PathBuf) -> Result<()> {
    println!("{} {}", "Scanning:".cyan().bold(), scan_path.display());

    // Quick count first
    let scanner = Scanner::new();
    let (file_count, total_size) = scanner.count_files(&scan_path)?;

    if file_count == 0 {
        println!("{}", "No model files found".yellow());
        return Ok(());
    }

    println!(
        "Found {} model files ({:.2} GB)\n",
        file_count.to_string().bold(),
        total_size as f64 / 1_073_741_824.0
    );

    // Initialize components
    let cas = CasStore::new(&store_path);
    let db_path = store_path.join("modeld.db");
    let mut db = Database::open(&db_path)?;

    // Create progress bar
    let pb = ProgressBar::new(file_count as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    // Scan and process
    let results = scanner.scan(&scan_path, |path, size| {
        pb.set_message(format!(
            "{} ({:.2} MB)",
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            size as f64 / 1_048_576.0
        ));
        pb.inc(1);
    })?;

    pb.finish_with_message("Done");

    // Store in CAS and database
    println!("\n{}", "Processing files...".cyan().bold());
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

    println!("\n{}", "✓ Scan complete".green().bold());
    println!("  Processed: {}", processed.to_string().bold());
    println!("  Total size: {:.2} GB", total_size as f64 / 1_073_741_824.0);

    Ok(())
}

fn status_command(store_path: PathBuf) -> Result<()> {
    let db_path = store_path.join("modeld.db");

    if !db_path.exists() {
        eprintln!(
            "{}",
            "Store not initialized. Run 'modeld init' first.".red()
        );
        std::process::exit(1);
    }

    let db = Database::open(&db_path)?;

    let count = db.count_models()?;
    let total_size = db.total_size()?;

    println!("{}", "modeld Store Status".cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    println!("  Store path: {}", store_path.display());
    println!("  Total models: {}", count.to_string().bold());
    println!(
        "  Total size: {:.2} GB",
        (total_size as f64 / 1_073_741_824.0)
            .to_string()
            .bold()
    );
    println!("{}", "─".repeat(40).cyan());

    if count > 0 {
        println!("\n{}", "Recent models:".cyan());
        let models = db.list_models(Some(5))?;
        for model in models {
            println!(
                "  {} - {:.2} MB",
                model.blake3_hash.as_hex()[..16].to_string().dimmed(),
                model.size_bytes as f64 / 1_048_576.0
            );
        }
    }

    // Show quarantine stats
    let qm = QuarantineManager::new(&store_path);
    if let Ok(qstats) = qm.stats() {
        if qstats.total_files > 0 {
            println!("\n{}", "Quarantine:".yellow());
            println!("  Files: {}", qstats.total_files.to_string().bold());
            println!(
                "  Size: {:.2} MB",
                qstats.total_size as f64 / 1_048_576.0
            );
            if qstats.expired_files > 0 {
                println!(
                    "  {} expired (run 'modeld quarantine cleanup')",
                    qstats.expired_files.to_string().red()
                );
            }
        }
    }

    Ok(())
}

fn hash_command(file: PathBuf) -> Result<()> {
    if !file.exists() {
        eprintln!(
            "{} File not found: {}",
            "Error:".red().bold(),
            file.display()
        );
        std::process::exit(1);
    }

    println!(
        "{} {}",
        "Computing BLAKE3 hash:".cyan(),
        file.display()
    );

    let hash = hash_file(&file)?;
    let metadata = std::fs::metadata(&file)?;

    println!("\n{}", "Results:".green().bold());
    println!("  Hash: {}", hash.as_hex().bold());
    println!("  Prefix: {}", hash.prefix().bold());
    println!(
        "  Size: {:.2} MB",
        metadata.len() as f64 / 1_048_576.0
    );

    Ok(())
}

fn dedup_command(
    store_path: PathBuf,
    dry_run: bool,
    auto: bool,
    report: bool,
) -> Result<()> {
    let db_path = store_path.join("modeld.db");

    if !db_path.exists() {
        eprintln!(
            "{}",
            "Store not initialized. Run 'modeld init' first.".red()
        );
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
        // Default: interactive (currently runs like auto with confirmation display)
        DedupMode::Auto
    };

    let mode_label = match mode {
        DedupMode::DryRun => "DRY RUN (no changes will be made)",
        DedupMode::Report => "REPORT (analysis only)",
        DedupMode::Auto => "AUTO",
        DedupMode::Interactive => "INTERACTIVE",
    };

    println!("{}", "modeld Deduplication".cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    println!("  Store: {}", store_path.display());
    println!("  Mode:  {}", mode_label.yellow());
    println!("{}", "─".repeat(40).cyan());

    let db = Database::open(&db_path)?;
    let mut engine = DedupEngine::new(db, store_path.clone());

    // First, find and report duplicates
    let groups = engine.find_duplicates()?;

    if groups.is_empty() {
        println!("\n{}", "✓ No duplicate files found".green());
        return Ok(());
    }

    let total_savings = engine.calculate_savings(&groups);

    println!(
        "\n{} {} duplicate groups found",
        "→".cyan(),
        groups.len().to_string().bold()
    );
    println!(
        "{} Potential space savings: {:.2} GB",
        "→".cyan(),
        total_savings as f64 / 1_073_741_824.0
    );

    // Show each group
    println!("\n{}", "Duplicate groups:".cyan().bold());
    for (i, group) in groups.iter().enumerate() {
        println!(
            "\n  Group {} ({:.2} MB per copy, {} copies):",
            (i + 1).to_string().bold(),
            group.total_size as f64 / 1_048_576.0,
            group.files.len()
        );
        for file in &group.files {
            println!("    {}", file.path.display().to_string().dimmed());
        }
        println!(
            "    {} Would save {:.2} MB",
            "→".green(),
            (group.total_size * (group.files.len() as u64 - 1)) as f64 / 1_048_576.0
        );
    }

    if mode == DedupMode::DryRun || mode == DedupMode::Report {
        println!("\n{}", "─".repeat(40).cyan());
        println!(
            "  {} Total potential savings: {:.2} GB",
            "✓".green(),
            total_savings as f64 / 1_073_741_824.0
        );
        println!(
            "  Run without --dry-run to apply deduplication"
        );
        return Ok(());
    }

    // Execute deduplication with progress
    println!("\n{}", "Executing deduplication...".cyan().bold());

    let pb = ProgressBar::new(groups.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let stats = engine.run_dedup(mode, |current, total, hash_prefix| {
        pb.set_message(format!("Processing {} ({}/{})", hash_prefix, current, total));
        pb.set_position(current as u64 - 1);
    })?;

    pb.finish_with_message("Done");

    // Final report
    println!("\n{}", "─".repeat(40).cyan());
    println!("{}", "Deduplication Complete".green().bold());
    println!(
        "  Groups processed: {}",
        stats.groups_processed.to_string().bold()
    );
    println!(
        "  Groups succeeded: {}",
        stats.groups_succeeded.to_string().green().bold()
    );
    if stats.groups_failed > 0 {
        println!(
            "  Groups failed:    {}",
            stats.groups_failed.to_string().red().bold()
        );
    }
    println!(
        "  Files deduplicated: {}",
        stats.files_deduplicated.to_string().bold()
    );
    println!(
        "  Space saved: {:.2} GB",
        (stats.space_saved as f64 / 1_073_741_824.0)
            .to_string()
            .green()
            .bold()
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
                println!("{}", "No quarantined files".green());
                return Ok(());
            }

            println!("{}", "Quarantined Files".cyan().bold());
            println!("{}", "─".repeat(60).cyan());

            for entry in &entries {
                let days_str = match entry.days_remaining {
                    Some(d) => format!("{} days remaining", d).yellow().to_string(),
                    None => "EXPIRED".red().to_string(),
                };

                println!(
                    "\n  {} {}",
                    entry.quarantine_path.display().to_string().bold(),
                    format!("({})", days_str)
                );
                println!(
                    "    Original: {}",
                    entry.meta.original_path.dimmed()
                );
                println!("    Hash:     {}", entry.meta.blake3_hash[..16].dimmed());
                println!(
                    "    Size:     {:.2} MB",
                    entry.meta.size_bytes as f64 / 1_048_576.0
                );
                println!("    Reason:   {}", entry.meta.reason);
                println!(
                    "    Quarantined: {}",
                    entry.meta.quarantined_at.format("%Y-%m-%d %H:%M UTC")
                );
            }

            println!("\n{}", "─".repeat(60).cyan());
            println!("  Total: {} files", entries.len().to_string().bold());

            let stats = qm.stats()?;
            println!(
                "  Size:  {:.2} MB",
                stats.total_size as f64 / 1_048_576.0
            );

            if stats.expired_files > 0 {
                println!(
                    "\n  {} expired files - run 'modeld quarantine cleanup'",
                    stats.expired_files.to_string().red()
                );
            }
        }

        QuarantineAction::Cleanup => {
            println!("{}", "Cleaning up expired quarantine entries...".cyan());

            let cleaned = qm.cleanup_expired()?;

            if cleaned == 0 {
                println!("{}", "✓ No expired entries to clean up".green());
            } else {
                println!(
                    "{} Removed {} expired quarantine entries",
                    "✓".green(),
                    cleaned.to_string().bold()
                );
            }
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
            println!(
                r#"{{"found": true, "path": "{}"}}"#,
                snapshot_path.display()
            );
        } else {
            println!(r#"{{"found": false}}"#);
        }
    } else if found {
        println!(
            "{} Cache hit: {}/{}@{}",
            "✓".green().bold(),
            repo_id.cyan(),
            filename.cyan(),
            revision
        );
        println!("  Path: {}", snapshot_path.display().to_string().dimmed());
    } else {
        println!(
            "{} Not cached: {}/{}@{}",
            "✗".red(),
            repo_id,
            filename,
            revision
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
    let progress: modeld_core::downloader::ProgressCallback = Box::new(move |done, total, _name| {
        if total > 0 {
            progress_pb.set_length(total);
        }
        progress_pb.set_position(done);
    });

    let mut downloader = Downloader::new(&store);
    if let Some(t) = token {
        downloader = downloader.with_token(t);
    }

    let result = downloader.download_hf_file(&mut db, repo_id, filename, Some(revision), Some(&progress))?;
    pb.finish_with_message("Done");

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
            "Cache hit (skipped download)".yellow().to_string()
        } else {
            "Downloaded".green().to_string()
        };

        println!("\n{} {}", "✓".green().bold(), status);
        println!("  Repo:     {}", repo_id.cyan());
        println!("  File:     {}", filename.cyan());
        println!("  BLAKE3:   {}", &result.blake3_hash.as_hex()[..16]);
        println!("  Size:     {}", format_bytes(result.size_bytes));
        println!("  CAS path: {}", result.cas_path.display().to_string().dimmed());
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

    println!("{}", "modeld HuggingFace Cache Setup".bold().cyan());
    println!();
    println!("HF cache directory: {}", hf_home.display().to_string().green());
    println!();
    println!("{}", "To activate, add to your shell profile:".bold());
    println!();

    #[cfg(windows)]
    {
        println!("  {} (PowerShell):", "Windows".yellow());
        println!(
            "  $env:HF_HOME = \"{}\"",
            hf_home.display()
        );
        println!();
        println!("  {} (CMD):", "Windows".yellow());
        println!("  set HF_HOME={}", hf_home.display());
    }

    #[cfg(unix)]
    {
        println!("  {} ~/.bashrc or ~/.zshrc:", "Linux/macOS:".yellow());
        println!("  export HF_HOME=\"{}\"", hf_home.display());
    }

    println!();
    println!("{}", "Or install the Python hook for automatic interception:".bold());
    println!("  pip install modeld-hook");
    println!("  # Then add to your script:");
    println!("  import modeld_hook  # Auto-activates on import");

    Ok(())
}

/// `modeld hf-status` — show HF cache statistics
fn hf_status_command(store: PathBuf) -> Result<()> {
    let hf_cache = HfCache::new(&store);
    let db = open_db(&store)?;

    println!("{}", "HuggingFace Cache Status".bold().cyan());
    println!("{}", "─".repeat(40).dimmed());

    if !hf_cache.hf_home().exists() {
        println!("{}", "HF cache not initialized. Run: modeld hf-setup".yellow());
        return Ok(());
    }

    let stats = hf_cache.stats()?;

    println!(
        "  HF_HOME:     {}",
        hf_cache.hf_home().display().to_string().green()
    );
    println!("  Repos:       {}", stats.total_repos.to_string().bold());
    println!("  Blobs:       {}", stats.total_blobs.to_string().bold());

    // List repos
    let repos = hf_cache.list_repos()?;
    if !repos.is_empty() {
        println!();
        println!("{}", "Cached repos:".bold());
        for repo in &repos {
            println!("  • {}", repo.cyan());
        }
    }

    // Downloads from DB
    let downloads = db.list_downloads(None)?;
    let done_count = downloads.iter().filter(|d| {
        matches!(d.status, modeld_core::DownloadStatus::Done)
    }).count();

    if !downloads.is_empty() {
        println!();
        println!(
            "  Downloads:   {} total, {} completed",
            downloads.len().to_string().bold(),
            done_count.to_string().green().bold()
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
        anyhow::bail!(
            "Workflow directory does not exist: {}",
            workflow_dir.display()
        );
    }

    println!(
        "{} {}",
        "Scanning workflows in:".bold(),
        workflow_dir.display().to_string().cyan()
    );

    // Find all .json files
    let files = find_workflow_files(&workflow_dir)?;
    if files.is_empty() {
        println!("{}", "No workflow JSON files found.".yellow());
        return Ok(());
    }

    println!("  Found {} workflow files", files.len().to_string().bold());
    println!();

    // Build model lookup from database
    let lookup = build_model_lookup(&db)?;
    let lookup_size = lookup.len();
    println!(
        "  Model lookup: {} entries in CAS index",
        lookup_size.to_string().bold()
    );
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
                pb.println(format!(
                    "  {} {}: {}",
                    "WARN".yellow().bold(),
                    wf_path.display(),
                    e
                ));
            }
        }
        pb.inc(1);
    }

    pb.finish_and_clear();

    println!("{}", "Workflow scan complete:".green().bold());
    println!(
        "  Workflows indexed:  {}",
        files.len().to_string().bold()
    );
    println!(
        "  Refs resolved:      {}",
        total_resolved.to_string().green().bold()
    );
    println!(
        "  Refs unresolved:    {}",
        total_unresolved.to_string().yellow().bold()
    );
    if errors > 0 {
        println!(
            "  Parse errors:       {}",
            errors.to_string().red().bold()
        );
    }

    Ok(())
}

fn workflow_deps_command(store: PathBuf, workflow_file: PathBuf) -> Result<()> {
    if !workflow_file.exists() {
        anyhow::bail!("File not found: {}", workflow_file.display());
    }

    let db_path = store.join("modeld.db");

    println!(
        "{} {}",
        "Workflow:".bold(),
        workflow_file.display().to_string().cyan()
    );
    println!();

    let parsed = parse_workflow(&workflow_file)?;

    if let Some(ref title) = parsed.title {
        println!("  Title: {}", title.bold());
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
        println!("{}", "No model references found.".yellow());
        return Ok(());
    }

    println!("{}", "Model Dependencies:".bold());
    println!();

    // Group by ref_type
    let mut by_type: std::collections::BTreeMap<&str, Vec<_>> = std::collections::BTreeMap::new();
    for r in &all_refs {
        by_type.entry(r.ref_type.as_str()).or_default().push(r);
    }

    for (ref_type, refs) in &by_type {
        let type_label = match *ref_type {
            "checkpoint" => "Checkpoints",
            "lora"       => "LoRAs",
            "vae"        => "VAEs",
            "clip"       => "CLIP Models",
            "controlnet" => "ControlNets",
            "ipadapter"  => "IPAdapters",
            "unet"       => "UNets",
            "upscale_model" => "Upscale Models",
            _            => ref_type,
        };
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
            let status = if resolved {
                "✓".green().bold()
            } else {
                "?".yellow()
            };
            println!("    {} {}", status, r.model_name);
        }
        println!();
    }

    println!(
        "  Total: {} refs ({} resolved, {} unresolved)",
        all_refs.len().to_string().bold(),
        parsed.refs.len().to_string().green(),
        parsed.unresolved.len().to_string().yellow(),
    );

    Ok(())
}

fn refs_orphans_command(store: PathBuf, json_output: bool) -> Result<()> {
    let db_path = store.join("modeld.db");
    if !db_path.exists() {
        anyhow::bail!("Store not initialized. Run `modeld init` first.");
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
        println!("{}", "No orphan models found. All models are referenced by at least one workflow.".green());
        return Ok(());
    }

    let total_size: i64 = orphans.iter().map(|m| m.size_bytes).sum();

    println!("{}", "Orphan Models (no workflow references):".bold().yellow());
    println!();
    println!(
        "  {:<66} {:>10}  {}",
        "Hash".dimmed(),
        "Size".dimmed(),
        "Format".dimmed()
    );
    println!("  {}", "─".repeat(90).dimmed());

    for m in &orphans {
        let hash_str = &m.blake3_hash.as_hex()[..16];
        let size_str = format_bytes(m.size_bytes as u64);
        let fmt = m.format.as_deref().unwrap_or("unknown");
        println!(
            "  {}...  {:>10}  {}",
            hash_str.yellow(),
            size_str,
            fmt
        );
    }

    println!();
    println!(
        "  {} orphans, {} reclaimable",
        orphans.len().to_string().bold(),
        format_bytes(total_size as u64).yellow().bold()
    );
    println!();
    println!("{}", "Tip: Run `modeld gc --preview` to see GC plan, or `modeld gc` to quarantine these models.".dimmed());

    Ok(())
}

fn gc_command(store: PathBuf, preview: bool, cleanup_quarantine: bool) -> Result<()> {
    let db_path = store.join("modeld.db");
    if !db_path.exists() {
        anyhow::bail!("Store not initialized. Run `modeld init` first.");
    }

    let db = Database::open(&db_path)?;
    let gc = GcEngine::new(&db, &store);

    if preview {
        let plan = gc.preview()?;

        println!("{}", "GC Preview (no changes will be made):".bold());
        println!();

        println!(
            "  {} models hard-protected (referenced by workflows)",
            plan.hard_protected.len().to_string().green().bold()
        );
        if !plan.soft_protected.is_empty() {
            println!(
                "  {} models soft-protected (have aliases but no workflow refs):",
                plan.soft_protected.len().to_string().yellow().bold()
            );
            for item in &plan.soft_protected {
                println!(
                    "    {}... ({} aliases, {})",
                    item.hash_prefix.yellow(),
                    item.alias_count,
                    format_bytes(item.size_bytes as u64)
                );
            }
        }
        if !plan.would_quarantine.is_empty() {
            println!(
                "  {} models would be quarantined:",
                plan.would_quarantine.len().to_string().red().bold()
            );
            for item in &plan.would_quarantine {
                println!(
                    "    {}... ({})",
                    item.hash_prefix.red(),
                    format_bytes(item.size_bytes as u64)
                );
            }
            println!(
                "  Total reclaimable: {}",
                format_bytes(plan.total_reclaimable_bytes as u64).yellow().bold()
            );
        } else {
            println!("  {}", "Nothing to quarantine.".green());
        }
        if plan.expired_quarantine_count > 0 {
            println!();
            println!(
                "  {} expired quarantine entries ({}) could be deleted permanently.",
                plan.expired_quarantine_count.to_string().yellow().bold(),
                format_bytes(plan.expired_quarantine_bytes as u64)
            );
        }
        println!();
        println!("{}", "Run `modeld gc` (without --preview) to execute.".dimmed());
        return Ok(());
    }

    // Execute GC
    println!("{}", "Running safe GC...".bold());
    let result = gc.run_safe()?;

    println!();
    if result.quarantined.is_empty() {
        println!("{}", "Nothing quarantined — store is clean.".green().bold());
    } else {
        println!(
            "  {} Quarantined {} models ({})",
            "✓".green().bold(),
            result.quarantined.len().to_string().bold(),
            format_bytes(result.bytes_recovered as u64).yellow()
        );
    }
    if !result.skipped_protected.is_empty() {
        println!(
            "  {} Skipped {} hard-protected models",
            "•".blue(),
            result.skipped_protected.len()
        );
    }
    if !result.skipped_soft.is_empty() {
        println!(
            "  {} Skipped {} soft-protected models (use --force to override)",
            "⚠".yellow(),
            result.skipped_soft.len()
        );
    }

    if cleanup_quarantine {
        println!();
        println!("{}", "Cleaning expired quarantine entries...".bold());
        let cleaned = gc.cleanup_quarantine()?;
        if cleaned > 0 {
            println!(
                "  {} Permanently deleted {} expired quarantine entries",
                "✓".green().bold(),
                cleaned
            );
        } else {
            println!("  No expired quarantine entries found.");
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 5: proxy commands
// ─────────────────────────────────────────────────────────────────────────────

fn proxy_command(action: ProxyAction) -> Result<()> {
    match action {
        ProxyAction::Start {
            bind,
            port,
            store,
            token,
            allow_anonymous,
            config,
        } => proxy_start_command(bind, port, store, token, allow_anonymous, config),
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
        modeld_proxy::ProxyConfig::from_cli(port, Some(bind.clone()), store.clone(), token, allow_anonymous)
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
    println!("{}", "Scanning LAN for modeld proxy servers...".cyan().bold());
    println!("  (mDNS service: _modeld._tcp.local.)");
    println!();

    let servers = modeld_proxy::discover(timeout);

    if servers.is_empty() {
        println!(
            "{}",
            "No modeld proxy servers found on the local network.".yellow()
        );
        println!();
        println!("{}", "Make sure:".dimmed());
        println!("  • A server is running: modeld proxy start");
        println!("  • mDNS is published (avahi-publish / dns-sd / Bonjour)");
        println!("  • The machine is on the same network/subnet");
        println!("  • mDNS traffic is allowed (UDP 5353)");
        return Ok(());
    }

    println!(
        "{} {} server(s) found:",
        "✓".green().bold(),
        servers.len()
    );
    println!();
    for s in &servers {
        println!(
            "  {} {}",
            "•".cyan(),
            format!("http://{}:{}", s.address, s.port).bold()
        );
        if !s.txt.is_empty() {
            let parts: Vec<String> = s
                .txt
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
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
        anyhow::anyhow!("failed to reach proxy at {}: {:#}", url, e)
    })?;

    println!("{}", "modeld Proxy Status".cyan().bold());
    println!("{}", "─".repeat(40).cyan());
    println!("  URL:           {}", url);
    println!("  Status:        {}", health.status.green());
    println!("  Version:       {}", health.version);
    println!("  Uptime:        {}s", health.uptime_seconds);
    println!("  Models:        {}", health.model_count.to_string().bold());
    println!(
        "  Total size:    {}",
        format_bytes(health.total_bytes as u64)
    );
    println!("{}", "─".repeat(40).cyan());

    match builder.list_models() {
        Ok(models) => {
            if models.is_empty() {
                println!("{}", "No models in store.".dimmed());
            } else {
                println!();
                println!(
                    "{} (showing first 5):",
                    "Recent models".cyan()
                );
                for m in models.iter().take(5) {
                    let hash_short = &m.hash[..16.min(m.hash.len())];
                    println!(
                        "  {}  {}  {}",
                        hash_short.dimmed(),
                        format_bytes(m.size_bytes as u64),
                        m.format.as_deref().unwrap_or("unknown")
                    );
                }
                if models.len() > 5 {
                    println!("  ... and {} more", models.len() - 5);
                }
            }
        }
        Err(e) => {
            println!("  {} list_models failed: {:#}", "⚠".yellow(), e);
        }
    }

    Ok(())
}
