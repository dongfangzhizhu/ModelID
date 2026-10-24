//! `modeld store …` — locate, initialise, verify and migrate a store.

use crate::{init_command, resolve_store, verify_command};
use anyhow::{Context, Result};
use clap::Subcommand;
use colored::*;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum StoreAction {
    /// Print the resolved store path
    Locate {
        /// Store directory (if set, just prints this path)
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Initialize the store (same as `modeld init`)
    Init {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
    },
    /// Verify store consistency
    Verify {
        /// Store directory
        #[arg(short = 's', long)]
        store: Option<PathBuf>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Migrate store from one location to another
    Migrate {
        /// Source store path
        #[arg(long)]
        from: PathBuf,
        /// Destination store path
        #[arg(long)]
        to: PathBuf,
    },
}

pub fn store_command(action: StoreAction) -> Result<()> {
    match action {
        StoreAction::Locate { store } => {
            let path = resolve_store(store);
            println!("{}", path.display());
            Ok(())
        }
        StoreAction::Init { store } => {
            let path = resolve_store(store);
            init_command(Some(path), false)
        }
        StoreAction::Verify { store, json } => {
            let path = resolve_store(store);
            verify_command(path, json)
        }
        StoreAction::Migrate { from, to } => store_migrate_command(from, to),
    }
}

fn store_migrate_command(from: PathBuf, to: PathBuf) -> Result<()> {
    if !from.exists() {
        anyhow::bail!("Source store not found: {}", from.display());
    }

    println!(
        "{}",
        format!("Migrating store from {} to {}", from.display(), to.display()).cyan().bold()
    );

    // Simple migration: copy the entire store directory tree
    std::fs::create_dir_all(&to)
        .with_context(|| format!("Failed to create destination: {}", to.display()))?;

    copy_dir_all(&from, &to)?;

    println!("{}", "✓ Migration complete.".green().bold());
    println!("  {}", format!("New store: {}", to.display()).dimmed());
    println!(
        "  {}",
        "You can now update your MODELD_STORE or modeld.toml to point to the new location."
            .dimmed()
    );

    Ok(())
}

/// Recursively copy a directory tree (best-effort; skips CAS read-only files on Windows).
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src)
        .with_context(|| format!("Failed to read directory: {}", src.display()))?
    {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            std::fs::create_dir_all(&dst_path)?;
            copy_dir_all(&entry.path(), &dst_path)?;
        } else if ty.is_file() {
            // CAS objects are read-only; copy with overwrite
            if let Err(e) = std::fs::copy(entry.path(), &dst_path) {
                eprintln!("  {} skipping {}: {}", "⚠".yellow(), entry.path().display(), e);
            }
        }
    }
    Ok(())
}
