use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{
    hash_file, CasStore, Database, DedupEngine, DedupMode,
    Downloader, HfCache, QuarantineManager, Scanner,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "modeld")]
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
    let db_path = store.join("index.db");
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
