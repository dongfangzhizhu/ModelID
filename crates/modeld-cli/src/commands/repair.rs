//! Repair commands: `quarantine`, `verify`, `unlink`.

use crate::{format_bytes, require_store_db};
use anyhow::Result;
use clap::Subcommand;
use colored::*;
use modeld_core::{run_fsck, t, tf, unlink_path, Database, QuarantineManager};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum QuarantineAction {
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

pub fn quarantine_command(store_path: PathBuf, action: QuarantineAction) -> Result<()> {
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

pub fn verify_command(store_path: PathBuf, json_output: bool) -> Result<()> {
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

pub fn unlink_command(store_path: PathBuf, path: PathBuf) -> Result<()> {
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
