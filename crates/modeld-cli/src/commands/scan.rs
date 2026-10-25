//! `modeld scan` — walk a directory and ingest the models it finds.

use crate::load_config;
use anyhow::{Context, Result};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{t, tf, CasStore, Database, IngestMode, QuarantineManager, ScanOptions, Scanner};
use std::path::PathBuf;
use uuid::Uuid;

#[tracing::instrument(skip_all, fields(store = %store_path.display()))]
pub fn scan_command(
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
