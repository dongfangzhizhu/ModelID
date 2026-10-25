//! `modeld db …` — status, backup, restore, vacuum and migrations.

use crate::{require_store_db, resolve_store};
use anyhow::Result;
use clap::Subcommand;
use modeld_core::Database;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum DbAction {
    /// Show database status (schema version, file sizes, integrity)
    Status {},
    /// Backup the database to <store>/backups/
    Backup {},
    /// Restore the database from a backup file
    Restore {
        /// Path to the backup file
        backup: PathBuf,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// VACUUM the database to reclaim disk space
    Vacuum {},
    /// Run any pending schema migrations
    Migrate {},
}

pub fn db_command(action: DbAction, store: Option<PathBuf>) -> Result<()> {
    match action {
        DbAction::Status {} => {
            let store_path = resolve_store(store.clone());
            let db_path = store_path.join("modeld.db");
            if !db_path.exists() {
                anyhow::bail!("Database not found at {}", db_path.display());
            }
            let db = Database::open(&db_path)?;
            let version = db.schema_version()?;
            let db_size = std::fs::metadata(&db_path)?.len();
            let wal_path = db_path.with_extension("db-wal");
            let wal_size = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);
            let wal_active = wal_path.exists();
            println!("schema version : {}", version);
            println!("db size        : {} KB", db_size / 1024);
            println!(
                "wal status     : {} ({} KB)",
                if wal_active { "active" } else { "inactive" },
                wal_size / 1024
            );
            let integrity_result = db.integrity_check().unwrap_or_else(|_| "ERROR".to_string());
            let integrity_ok = integrity_result.eq_ignore_ascii_case("ok");
            println!("integrity      : {}", if integrity_ok { "OK" } else { &integrity_result });
        }
        DbAction::Backup {} => {
            let store_path = resolve_store(store.clone());
            let db_path = require_store_db(&store_path)?;
            let backups_dir = store_path.join("backups");
            std::fs::create_dir_all(&backups_dir)?;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let backup_path = backups_dir.join(format!("modeld_backup_{}.db", ts));
            std::fs::copy(&db_path, &backup_path)?;
            println!("✓ Database backed up to {}", backup_path.display());
        }
        DbAction::Restore { backup, yes } => {
            if !backup.exists() {
                anyhow::bail!("Backup file not found: {}", backup.display());
            }
            let store_path = resolve_store(store.clone());
            let db_path = store_path.join("modeld.db");
            if !yes {
                print!("Replace {} with {}? [y/N]: ", db_path.display(), backup.display());
                std::io::Write::flush(&mut std::io::stdout())?;
                let mut ans = String::new();
                std::io::stdin().read_line(&mut ans)?;
                if !ans.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }
            std::fs::copy(&backup, &db_path)?;
            println!("✓ Database restored from {}", backup.display());
        }
        DbAction::Vacuum {} => {
            let store_path = resolve_store(store.clone());
            let db_path = require_store_db(&store_path)?;
            let db = Database::open(&db_path)?;
            db.vacuum()?;
            println!("✓ Database vacuumed");
        }
        DbAction::Migrate {} => {
            let store_path = resolve_store(store.clone());
            let db_path = store_path.join("modeld.db");
            if !db_path.exists() {
                anyhow::bail!("Database not found at {}", db_path.display());
            }
            // Read the version before migrations run.
            let version_before = Database::read_schema_version_raw(&db_path)?;
            // Opening the database triggers any pending migrations automatically.
            let db = Database::open(&db_path)?;
            let version_after = db.schema_version()?;
            let latest = Database::latest_schema_version();
            if version_before < version_after {
                println!(
                    "✓ Migrated database from schema v{} to v{}",
                    version_before, version_after
                );
            } else if version_after >= latest {
                println!("✓ Database is already at the latest schema version (v{})", version_after);
            } else {
                println!("  Database is at schema version {} (latest: {})", version_after, latest);
            }
        }
    }
    Ok(())
}
