//! `modeld tx …` — inspect, roll back and recover write-ahead-log transactions.

use crate::{require_store_db, resolve_store};
use anyhow::{Context, Result};
use clap::Subcommand;
use modeld_core::{t, tf, Database, TransactionManager, TxFilter};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum TxAction {
    /// List transaction records
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show details of a specific transaction
    Show {
        /// Transaction ID (UUID)
        tx_id: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Roll back a transaction
    Rollback {
        /// Transaction ID (UUID)
        tx_id: String,
    },
    /// Recover from a crash: mark PENDING transactions as FAILED and clean staging
    Recover {},
    /// Delete transaction records older than a given duration
    Cleanup {
        /// Age threshold, e.g. "30d", "7d", "1h"
        #[arg(long)]
        older_than: String,
    },
}

pub fn tx_command(action: TxAction, store: Option<PathBuf>) -> Result<()> {
    match action {
        TxAction::List { json } => {
            let store_path = resolve_store(store.clone());
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let tm = TransactionManager::new(&mut db, &store_path);
            let records = tm.list(TxFilter::default())?;
            if json {
                let j: Vec<_> = records
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "tx_id": r.tx_id,
                            "op_type": r.op_type.as_ref().map(|o| o.as_str()),
                            "status": format!("{:?}", r.status),
                            "start_time": r.start_time.to_rfc3339(),
                            "end_time": r.end_time.map(|t| t.to_rfc3339()),
                            "error": r.error_message,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&j)?);
            } else {
                for r in &records {
                    let op = r.op_type.as_ref().map(|o| o.as_str()).unwrap_or("unknown");
                    println!(
                        "  {} | {:?} | {} | {}",
                        &r.tx_id[..8],
                        r.status,
                        op,
                        r.start_time.format("%Y-%m-%d %H:%M")
                    );
                }
            }
        }
        TxAction::Show { tx_id, json } => {
            let store_path = resolve_store(store.clone());
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let tm = TransactionManager::new(&mut db, &store_path);
            let records = tm.list(TxFilter::default())?;
            let record = records.iter().find(|r| r.tx_id == tx_id || r.tx_id.starts_with(&tx_id));
            if let Some(r) = record {
                if json {
                    let obj = serde_json::json!({
                        "tx_id": r.tx_id,
                        "op_type": r.op_type.as_ref().map(|o| o.as_str()),
                        "status": format!("{:?}", r.status),
                        "start_time": r.start_time.to_rfc3339(),
                        "end_time": r.end_time.map(|t| t.to_rfc3339()),
                        "error": r.error_message,
                        "affected_paths": r.affected_paths.iter()
                            .map(|p| p.source.display().to_string())
                            .collect::<Vec<_>>(),
                    });
                    println!("{}", serde_json::to_string_pretty(&obj)?);
                } else {
                    println!("tx_id:  {}", r.tx_id);
                    println!("status: {:?}", r.status);
                    println!(
                        "op:     {}",
                        r.op_type.as_ref().map(|o| o.as_str()).unwrap_or("unknown")
                    );
                    println!("start:  {}", r.start_time);
                    if let Some(end) = r.end_time {
                        println!("end:    {}", end);
                    }
                    if let Some(err) = &r.error_message {
                        println!("error:  {}", err);
                    }
                }
            } else {
                anyhow::bail!("Transaction not found: {}", tx_id);
            }
        }
        TxAction::Rollback { tx_id } => {
            let store_path = resolve_store(store.clone());
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let mut tm = TransactionManager::new(&mut db, &store_path);
            tm.rollback(&tx_id)?;
            println!("✓ Rolled back transaction {}", &tx_id[..8.min(tx_id.len())]);
        }
        TxAction::Recover {} => {
            let store_path = resolve_store(store.clone());
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let mut tm = TransactionManager::new(&mut db, &store_path);
            let reports = tm.recover()?;
            if reports.is_empty() {
                println!("{}", t("tx.recover.none"));
            } else {
                println!("{}", tf("tx.recover.header", &[("count", &reports.len().to_string())]));
                for report in &reports {
                    println!(
                        "  {} {} [{}]",
                        &report.tx_id[..8.min(report.tx_id.len())],
                        report.op,
                        report.action.as_str()
                    );
                    for path in &report.paths {
                        println!(
                            "    {} {} — {}",
                            path.action.as_str(),
                            path.path.display(),
                            path.note
                        );
                    }
                }
            }
        }
        TxAction::Cleanup { older_than } => {
            let store_path = resolve_store(store.clone());
            let db_path = require_store_db(&store_path)?;
            let mut db = Database::open(&db_path)?;
            let mut tm = TransactionManager::new(&mut db, &store_path);
            let duration = parse_duration_str(&older_than)?;
            let removed = tm.cleanup_older_than(duration)?;
            println!("✓ Removed {} transaction record(s)", removed);
        }
    }
    Ok(())
}

fn parse_duration_str(s: &str) -> Result<std::time::Duration> {
    let s = s.trim();
    if let Some(days) = s.strip_suffix('d') {
        let n: u64 = days.parse().with_context(|| format!("Invalid duration: {}", s))?;
        return Ok(std::time::Duration::from_secs(n * 86400));
    }
    if let Some(hours) = s.strip_suffix('h') {
        let n: u64 = hours.parse().with_context(|| format!("Invalid duration: {}", s))?;
        return Ok(std::time::Duration::from_secs(n * 3600));
    }
    anyhow::bail!("Invalid duration '{}': use format like '30d' or '24h'", s);
}
