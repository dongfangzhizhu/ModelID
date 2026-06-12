use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{hash_file, CasStore, Database, DedupEngine, DedupMode, QuarantineManager, Scanner};
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
