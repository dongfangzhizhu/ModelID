use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{hash_file, CasStore, Database, Scanner};
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path } => init_command(path)?,
        Commands::Scan { path, store } => scan_command(path, store)?,
        Commands::Status { store } => status_command(store)?,
        Commands::Hash { file } => hash_command(file)?,
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

    println!("{} {}", "Initializing modeld store at:".green().bold(), store_path.display());

    // Initialize CAS
    let cas = CasStore::new(&store_path);
    cas.init()?;

    // Initialize database
    let db_path = store_path.join("modeld.db");
    Database::open(&db_path)?;

    println!("{}", "✓ Store initialized successfully".green());
    println!("\nNext steps:");
    println!("  {} modeld scan <directory>", "1.".bold());
    println!("  {} modeld status", "2.".bold());

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
    let mut processed = 0;
    let results = scanner.scan(&scan_path, |path, size| {
        pb.set_message(format!(
            "{} ({:.2} MB)",
            path.file_name().unwrap_or_default().to_string_lossy(),
            size as f64 / 1_048_576.0
        ));
        pb.inc(1);
    })?;

    pb.finish_with_message("Done");

    // Store in CAS and database
    println!("\n{}", "Processing files...".cyan().bold());
    for file in &results {
        // Store in CAS
        cas.store(&file.path, &file.hash)?;

        // Record in database
        db.insert_or_update_model(&file.hash, file.size as i64, None, None, None, None)?;

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
        eprintln!("{}", "Store not initialized. Run 'modeld init' first.".red());
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
        (total_size as f64 / 1_073_741_824.0).to_string().bold()
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

    Ok(())
}

fn hash_command(file: PathBuf) -> Result<()> {
    if !file.exists() {
        eprintln!("{} File not found: {}", "Error:".red().bold(), file.display());
        std::process::exit(1);
    }

    println!("{} {}", "Computing BLAKE3 hash:".cyan(), file.display());

    let hash = hash_file(&file)?;
    let metadata = std::fs::metadata(&file)?;

    println!("\n{}", "Results:".green().bold());
    println!("  Hash: {}", hash.as_hex().bold());
    println!("  Prefix: {}", hash.prefix().bold());
    println!("  Size: {:.2} MB", metadata.len() as f64 / 1_048_576.0);

    Ok(())
}
