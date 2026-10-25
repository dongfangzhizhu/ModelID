//! `modeld gc` — collect unreferenced objects and clean quarantine.

use crate::format_bytes;
use anyhow::{Context, Result};
use colored::*;
use modeld_core::{t, tf, Database, GcEngine, StoreLock};
use std::path::PathBuf;
use std::time::Duration;

pub fn gc_command(
    store: PathBuf,
    preview: bool,
    cleanup_quarantine: bool,
    cleanup_tmp: bool,
) -> Result<()> {
    let db_path = store.join("modeld.db");
    if !db_path.exists() {
        anyhow::bail!("{}", t("store.not_initialized_exit"));
    }

    // GC deletes objects; it must not run concurrently with another layout
    // mutation (dedup, another gc) in the same store.
    let _lock = StoreLock::acquire(&store, "gc", Duration::from_secs(30))
        .with_context(|| tf("lock.busy", &[("path", &store.display().to_string())]))?;

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
                    tf(
                        "gc.preview.quarantine_line",
                        &[("prefix", &item.hash_prefix), ("size", &size)]
                    )
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
            println!("  {}", tf("gc.cleanup_done", &[("count", &cleaned)]).green().bold());
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
            println!("  {}", tf("gc.cleanup_tmp_done", &[("count", &removed)]).green().bold());
        } else {
            println!("  {}", t("gc.cleanup_tmp_none"));
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Verify (fsck) command
// ─────────────────────────────────────────────────────────────────────────────
