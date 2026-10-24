//! `modeld tag|note|pin|favorite|refs …` — per-model metadata and the
//! reference graph.
//!
//! These five families share one shape: a hash, an optional store override and
//! an optional JSON flag, so they live together instead of each getting a
//! module of boilerplate.

use crate::{format_bytes, require_store_db, resolve_store};
use anyhow::Result;
use clap::Subcommand;
use colored::*;
use modeld_core::Database;
use modeld_core::{
    add_tag, classify_refs, explain_refs, favorite_model, list_all_tags, list_orphans, list_pinned,
    pin_model, remove_tag, scan_workflow_refs, set_note, unfavorite_model, unpin_model, Blake3Hash,
    RefStatus,
};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum NoteAction {
    /// Set or clear the note for a model (empty text clears the note)
    Set {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Note text (empty string clears the note)
        text: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum PinAction {
    /// Pin a model (protect from GC)
    Add {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Unpin a model
    Remove {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// List all pinned models
    List {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum FavoriteAction {
    /// Mark a model as favourite
    Add {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Remove the favourite flag from a model
    Remove {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum RefsAction {
    /// Parse a ComfyUI workflow JSON and index its model references
    Scan {
        /// Workflow JSON file (or directory of JSON files)
        path: PathBuf,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Explain why a model cannot be deleted
    Why {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// List models with no workflow references and no aliases (orphans)
    Orphans {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show reference graph: all models with their reference status
    Graph {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

// ── Tag management ────────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum TagAction {
    /// Attach a tag to a model
    Add {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Tag to add
        tag: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Remove a tag from a model
    Remove {
        /// Full 64-character BLAKE3 hash
        hash: String,
        /// Tag to remove
        tag: String,
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// List all tags with usage counts
    List {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

pub fn tag_command(action: TagAction) -> Result<()> {
    match action {
        TagAction::Add { hash, tag, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            add_tag(&mut db, &h, &tag)?;
            println!("✓ Added tag '{}' to {}", tag, &hash[..16]);
        }
        TagAction::Remove { hash, tag, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            remove_tag(&mut db, &h, &tag)?;
            println!("✓ Removed tag '{}' from {}", tag, &hash[..16]);
        }
        TagAction::List { store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            let tags = list_all_tags(&db)?;
            if json {
                let j: Vec<_> =
                    tags.iter().map(|(t, c)| serde_json::json!({"tag": t, "count": c})).collect();
                println!("{}", serde_json::to_string_pretty(&j)?);
            } else {
                for (tag, count) in &tags {
                    println!("  {} ({})", tag, count);
                }
            }
        }
    }
    Ok(())
}

// ── Note management ───────────────────────────────────────────────────────────

pub fn note_command(action: NoteAction) -> Result<()> {
    match action {
        NoteAction::Set { hash, text, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            set_note(&mut db, &h, &text)?;
            println!("✓ Note set for {}", &hash[..16]);
        }
    }
    Ok(())
}

// ── Pin management ────────────────────────────────────────────────────────────

pub fn pin_command(action: PinAction) -> Result<()> {
    match action {
        PinAction::Add { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            pin_model(&mut db, &h)?;
            println!("✓ Pinned {}", &hash[..16]);
        }
        PinAction::Remove { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            unpin_model(&mut db, &h)?;
            println!("✓ Unpinned {}", &hash[..16]);
        }
        PinAction::List { store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            let pinned = list_pinned(&db)?;
            if json {
                let hashes: Vec<_> = pinned.iter().map(|h| h.as_hex()).collect();
                println!("{}", serde_json::to_string_pretty(&hashes)?);
            } else {
                for h in &pinned {
                    println!("  {}", h.as_hex());
                }
            }
        }
    }
    Ok(())
}

// ── Favorite management ───────────────────────────────────────────────────────

pub fn favorite_command(action: FavoriteAction) -> Result<()> {
    match action {
        FavoriteAction::Add { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            favorite_model(&mut db, &h)?;
            println!("✓ Favorited {}", &hash[..16]);
        }
        FavoriteAction::Remove { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            unfavorite_model(&mut db, &h)?;
            println!("✓ Unfavorited {}", &hash[..16]);
        }
    }
    Ok(())
}

// ── Reference graph commands ──────────────────────────────────────────────────

pub fn refs_command(action: RefsAction) -> Result<()> {
    match action {
        RefsAction::Scan { path, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let count = scan_workflow_refs(&mut db, &path)?;
            println!("✓ Indexed {} model references from {}", count, path.display());
        }
        RefsAction::Why { hash, store } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            let h = Blake3Hash::from_hex(&hash)?;
            let reasons = explain_refs(&db, &h)?;
            if reasons.is_empty() {
                println!("  No references found — this model is an orphan candidate.");
            } else {
                for r in &reasons {
                    println!("  • {}", r);
                }
            }
        }
        RefsAction::Orphans { store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            let orphans = list_orphans(&db)?;
            if json {
                let hashes: Vec<_> = orphans.iter().map(|h| h.as_hex()).collect();
                println!("{}", serde_json::to_string_pretty(&hashes)?);
            } else {
                for h in &orphans {
                    println!("  {}", &h.as_hex()[..16]);
                }
            }
        }
        RefsAction::Graph { store, json } => {
            let store_path = resolve_store(store);
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            refs_graph_command(&db, json)?;
        }
    }
    Ok(())
}

/// Display a reference graph for all models in the store.
///
/// Each model is shown with its RefStatus and alias / workflow-ref counts.
fn refs_graph_command(db: &Database, json_output: bool) -> Result<()> {
    let rows = db.get_gc_candidate_counts()?;

    if json_output {
        let entries: Vec<serde_json::Value> = rows
            .iter()
            .map(|(model, alias_count, wf_count)| {
                let status = classify_refs(db, &model.blake3_hash);
                let status_str = match status {
                    RefStatus::HardReference => "hard-reference",
                    RefStatus::SoftReference => "soft-reference",
                    RefStatus::Pinned => "pinned",
                    RefStatus::RecentlyUsed => "recently-used",
                    RefStatus::Unknown => "unknown",
                    RefStatus::OrphanCandidate => "orphan-candidate",
                };
                serde_json::json!({
                    "hash": model.blake3_hash.as_hex(),
                    "hash_prefix": &model.blake3_hash.as_hex()[..16],
                    "size_bytes": model.size_bytes,
                    "format": model.format,
                    "ref_status": status_str,
                    "alias_count": alias_count,
                    "workflow_ref_count": wf_count,
                    "pinned": model.pinned,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("{}", "No models in store.".yellow());
        return Ok(());
    }

    println!("{}", "Reference Graph".cyan().bold());
    println!("{}", "─".repeat(70).cyan());
    println!(
        "  {:<18}  {:<7}  {:<8}  {:<8}  {}",
        "Hash".dimmed(),
        "Size".dimmed(),
        "Aliases".dimmed(),
        "WfRefs".dimmed(),
        "Status".dimmed(),
    );
    println!("{}", "─".repeat(70).dimmed());

    // Sort by status severity: orphans first, then soft, then hard/pinned
    let mut sorted = rows;
    sorted.sort_by_key(|(model, _, _)| {
        let s = classify_refs(db, &model.blake3_hash);
        match s {
            RefStatus::OrphanCandidate => 0,
            RefStatus::SoftReference => 1,
            RefStatus::Unknown => 2,
            RefStatus::RecentlyUsed => 3,
            RefStatus::HardReference => 4,
            RefStatus::Pinned => 5,
        }
    });

    for (model, alias_count, wf_count) in &sorted {
        let status = classify_refs(db, &model.blake3_hash);
        let (status_str, colored_status) = match status {
            RefStatus::HardReference => ("hard-ref ", "hard-ref ".green().to_string()),
            RefStatus::SoftReference => ("soft-ref ", "soft-ref ".yellow().to_string()),
            RefStatus::Pinned => ("pinned   ", "pinned   ".cyan().to_string()),
            RefStatus::RecentlyUsed => ("recent   ", "recent   ".blue().to_string()),
            RefStatus::Unknown => ("unknown  ", "unknown  ".dimmed().to_string()),
            RefStatus::OrphanCandidate => ("orphan   ", "orphan   ".red().to_string()),
        };
        let _ = status_str;
        let size_str = format_bytes(model.size_bytes as u64);
        let hash_prefix = &model.blake3_hash.as_hex()[..16];
        println!(
            "  {}...  {:>7}  {:>8}  {:>8}  {}",
            hash_prefix, size_str, alias_count, wf_count, colored_status,
        );
    }

    println!("{}", "─".repeat(70).dimmed());
    println!("  Total: {} model(s)", sorted.len());
    Ok(())
}
