//! `modeld` — command dispatch.
//!
//! The clap tree is in `cli.rs`, the shared helpers in `util.rs`, and every
//! command family has a module under `commands/`.

mod cli;
mod commands;
mod output;
mod util;

pub(crate) use commands::repair::verify_command;
pub(crate) use util::*;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use commands::config::config_command;
use commands::db::db_command;
use commands::dedup::dedup_command;
use commands::gc::gc_command;
use commands::hf::hf_command;
use commands::hfstore::{
    hf_check_command, hf_setup_command, hf_status_command, refs_orphans_command,
    workflow_deps_command, workflow_scan_command,
};
use commands::meta::{favorite_command, note_command, pin_command, refs_command, tag_command};
use commands::proxy::proxy_command;
use commands::query::{dupes_command, info_command, list_command, stats_command, status_command};
use commands::repair::{quarantine_command, unlink_command};
use commands::scan::scan_command;
use commands::serve::serve_command;
use commands::store::store_command;
use commands::tx::tx_command;
use output::ExitCode;
use util::hash_command;

fn main() -> Result<()> {
    // clap exits with 2 for usage errors on its own; everything else goes
    // through ExitCode so the documented table is actually what the shell sees.
    let cli = Cli::parse();
    util::init_logging(cli.log_format);
    let _ = ExitCode::Ok;
    // `--store` is a global flag: resolve it once and hand a clone to whichever
    // command runs, so no subcommand has to declare it.
    let store = cli.store.clone();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        store = %store.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
        "modeld start"
    );

    match cli.command {
        Commands::Init { path, interactive } => init_command(path, interactive)?,
        Commands::Scan {
            path,
            incremental,
            full,
            exclude_globs,
            follow_symlinks,
            index_only,
            adopt,
            copy,
        } => {
            // Index is the default: a catalogue scan must not duplicate data.
            let mode = if adopt {
                IngestMode::Adopt
            } else if copy {
                IngestMode::Copy
            } else {
                let _ = index_only;
                IngestMode::Index
            };
            scan_command(
                path,
                resolve_store(store.clone()),
                incremental,
                full,
                exclude_globs,
                follow_symlinks,
                mode,
            )?
        }
        Commands::Status {} => status_command(resolve_store(store.clone()))?,
        Commands::Stats {} => stats_command(resolve_store(store.clone()))?,
        Commands::Dupes { min_size, json } => {
            dupes_command(resolve_store(store.clone()), min_size, json)?
        }
        Commands::List { limit, json, export_csv } => {
            list_command(resolve_store(store.clone()), limit, json, export_csv)?
        }
        Commands::Info { hash, json } => info_command(resolve_store(store.clone()), &hash, json)?,
        Commands::Hash { file } => hash_command(file)?,
        Commands::Dedup {
            dry_run,
            auto,
            apply,
            report,
            strategy,
            min_size,
            include_globs,
            exclude_globs,
            protect,
            pin,
            yes,
            json,
        } => dedup_command(
            resolve_store(store.clone()),
            dry_run,
            auto || apply,
            report,
            strategy,
            min_size,
            include_globs,
            exclude_globs,
            protect,
            pin,
            yes,
            json,
        )?,
        Commands::Quarantine { action } => {
            quarantine_command(resolve_store(store.clone()), action)?
        }
        Commands::HfCheck { repo_id, filename, revision, json } => {
            hf_check_command(resolve_store(store.clone()), &repo_id, &filename, &revision, json)?
        }
        Commands::HfDownload { repo_id, filename, revision, token, json } => hf_download_command(
            resolve_store(store.clone()),
            &repo_id,
            &filename,
            &revision,
            token,
            json,
        )?,
        Commands::HfSetup { print_path } => {
            hf_setup_command(resolve_store(store.clone()), print_path)?
        }
        Commands::HfStatus {} => hf_status_command(resolve_store(store.clone()))?,
        Commands::WorkflowScan { path } => {
            workflow_scan_command(resolve_store(store.clone()), path)?
        }
        Commands::WorkflowDeps { file } => {
            workflow_deps_command(resolve_store(store.clone()), file)?
        }
        Commands::RefsOrphans { json } => refs_orphans_command(resolve_store(store.clone()), json)?,
        Commands::Gc { preview, cleanup_quarantine, cleanup_tmp } => {
            gc_command(resolve_store(store.clone()), preview, cleanup_quarantine, cleanup_tmp)?
        }
        Commands::Verify { json } => verify_command(resolve_store(store.clone()), json)?,
        Commands::Unlink { path } => unlink_command(resolve_store(store.clone()), path)?,
        Commands::Proxy { action } => proxy_command(action, store.clone())?,
        Commands::Doctor { json } => doctor_command(resolve_store(store.clone()), json)?,
        Commands::Store { action } => store_command(action, store.clone())?,
        Commands::Config { action } => config_command(action, store.clone())?,
        Commands::Hf { action } => hf_command(action, store.clone())?,
        Commands::Serve { host, port, open, read_only, allow_lan } => {
            serve_command(host, port, open, resolve_store(store.clone()), read_only, allow_lan)?
        }
        Commands::Tag { action } => tag_command(action, store.clone())?,
        Commands::Note { action } => note_command(action, store.clone())?,
        Commands::Pin { action } => pin_command(action, store.clone())?,
        Commands::Favorite { action } => favorite_command(action, store.clone())?,
        Commands::Refs { action } => refs_command(action, store.clone())?,
        Commands::Tx { action } => tx_command(action, store.clone())?,
        Commands::Db { action } => db_command(action, store.clone())?,
    }

    Ok(())
}
