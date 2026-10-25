//! `modeld dedup` — plan and apply content-identical replacements.

use crate::parse_size;
use anyhow::{Context, Result};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use modeld_core::{t, tf, Database, DedupEngine, DedupMode, DedupStrategy, StoreLock};
use std::path::PathBuf;
use std::time::Duration;

#[allow(clippy::too_many_arguments)]
pub fn dedup_command(
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

    // Two processes deduping the same store would interleave renames and the
    // WAL would describe a store that no longer exists.  Wait briefly, then
    // fail with the holder's identity instead of corrupting anything.
    let _lock = StoreLock::acquire(&store_path, "dedup", Duration::from_secs(30))
        .with_context(|| tf("lock.busy", &[("path", &store_path.display().to_string())]))?;

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
