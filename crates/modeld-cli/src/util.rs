//! Helpers shared by more than one command module.

use anyhow::{Context, Result};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
pub(crate) use modeld_core::{
    hash_file, load_config, resolve_store_path, run_doctor, save_config, t, tf, token_remove,
    token_set, token_status, CasStore, CheckStatus, Database, Downloader, IngestMode,
    QuarantineManager,
};
use std::path::{Path, PathBuf};

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
pub(crate) fn csv_escape(value: &str) -> String {
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

pub(crate) fn hash_command(file: PathBuf) -> Result<()> {
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
