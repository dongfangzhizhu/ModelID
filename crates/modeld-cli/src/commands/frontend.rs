//! `modeld frontend` — discover templates and connect AI frontends (C7).

use crate::util::lock_store;
use anyhow::Result;
use clap::Subcommand;
use modeld_core::{
    connect as connect_frontend, sync as sync_virtual, template, Database, FrontendTemplate,
    BUILTIN_FRONTENDS,
};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum FrontendAction {
    /// List built-in frontend templates and how to connect them
    List,
    /// Point a frontend models directory at the store's virtual tree
    Connect {
        /// comfyui | forge | a1111
        frontend: String,
        /// Frontend models directory to replace with a symlink
        #[arg(long, value_name = "PATH")]
        target: PathBuf,
    },
    /// Build / refresh virtual model directories from CAS
    Sync {
        /// Frontends to sync (repeatable; defaults to all three)
        #[arg(long = "frontend", value_name = "NAME")]
        frontends: Vec<String>,
        /// Print the plan without creating links
        #[arg(long)]
        dry_run: bool,
        /// Seconds to wait for another store mutation (0 = fail immediately)
        #[arg(long, value_name = "SECS", default_value_t = 30)]
        wait: u64,
    },
}

fn resolve(names: &[String]) -> Result<Vec<FrontendTemplate>> {
    let selected: Vec<String> = if names.is_empty() {
        BUILTIN_FRONTENDS.iter().map(|s| (*s).to_string()).collect()
    } else {
        names.to_vec()
    };
    selected
        .iter()
        .map(|name| {
            template(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown frontend '{name}'; available: {}",
                    BUILTIN_FRONTENDS.join(", ")
                )
            })
        })
        .collect()
}

pub fn frontend_command(store: PathBuf, action: FrontendAction) -> Result<()> {
    match action {
        FrontendAction::List => {
            for name in BUILTIN_FRONTENDS {
                let t = template(name).expect("built-in template");
                println!("{} — {}", t.name, t.display_name);
                println!("  virtual: <store>/virtual/{}", t.name);
                println!("  setup:   {}", t.setup);
            }
        }
        FrontendAction::Connect { frontend, target } => {
            let dir = connect_frontend(&store, &frontend, &target)?;
            println!("{} now points at {}", target.display(), dir.display());
        }
        FrontendAction::Sync { frontends, dry_run, wait } => {
            let templates = resolve(&frontends)?;
            let db_path = store.join("modeld.db");
            if !db_path.exists() {
                anyhow::bail!("store is not initialized: run `modeld init` first");
            }

            if dry_run {
                let db = Database::open(&db_path)?;
                let count = db.list_models(None)?.len();
                println!("Dry run: {count} model(s) considered");
                for t in &templates {
                    println!("  <store>/virtual/{}", t.name);
                }
                return Ok(());
            }

            let _lock = lock_store(&store, "frontend-sync", wait)?;
            let mut db = Database::open(&db_path)?;
            let names: Vec<String> = templates.iter().map(|t| t.name.to_string()).collect();
            let report = sync_virtual(Path::new(&store), &mut db, &names)?;
            println!(
                "Virtual FS sync: created={} valid={} missing_cas={} failed={}",
                report.created,
                report.valid,
                report.missing_cas,
                report.failed.len()
            );
            for failure in &report.failed {
                eprintln!("  ! {failure}");
            }
        }
    }
    Ok(())
}
